//! GitHub through its CLI, as Scramjet's `forge-publication-provider.ts` publishes: before a write,
//! `gh api` checks the repository's identity and a comment's parent, and `git` that a pull
//! request's branches exist; after it, what the write made is read back and compared with what
//! was asked. Every way a write may have happened without being confirmed is reported as
//! ambiguous, never retried.

use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// How long each `gh` or `git` process may run, as in Scramjet.
const TIMEOUT: Duration = Duration::from_secs(30);
/// More output than any GitHub reply this reads is not read.
const OUTPUT_BYTES: u64 = 16 << 20;

/// A GitHub repository, as its owner and name are written in a URL.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Repository {
    pub owner: String,
    pub name: String,
}

impl Repository {
    pub fn shown(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// GitHub's names are compared without case.
    pub fn same(&self, other: &Self) -> bool {
        self.owner.to_lowercase() == other.owner.to_lowercase()
            && self.name.to_lowercase() == other.name.to_lowercase()
    }

    fn api(&self) -> String {
        format!("repos/{}/{}", self.owner, self.name)
    }
}

/// What a publication writes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Request {
    Issue {
        title: String,
        body: String,
    },
    Pull {
        title: String,
        body: String,
        head: String,
        base: String,
        draft: bool,
    },
    /// A comment on issue or pull request `number`; `pull` says which it must be.
    Comment {
        number: u64,
        pull: bool,
        body: String,
    },
}

/// How a write went.
#[derive(Debug, PartialEq)]
pub(super) enum Posted {
    /// Read back as asked, at this URL.
    Verified(String),
    /// Nothing was written, for this reason.
    NotSent(String),
    /// It may have been written; this says why it is not known.
    Ambiguous(String),
}

/// How a process ran.
#[derive(Debug, Default)]
pub(super) struct Ran {
    pub spawned: bool,
    /// It exited with status 0, its input and output complete.
    pub ok: bool,
    pub stdout: String,
    /// The first line of what it wrote to stderr.
    pub error: String,
}

/// Runs `gh` and `git`; tests give scripted results instead.
pub(super) trait Exec {
    /// Runs `program` with `args`, `input` on its stdin.
    fn run(&mut self, program: &str, args: &[&str], input: Option<&str>) -> Ran;
}

/// The system's `gh` and `git`, run in `root` until they end, time out or `cancel` is set.
pub(super) struct System<'a> {
    pub root: &'a Path,
    pub cancel: &'a AtomicBool,
}

impl Exec for System<'_> {
    fn run(&mut self, program: &str, args: &[&str], input: Option<&str>) -> Ran {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(self.root)
            // Neither may wait for a password on the terminal, which the session owns.
            .env("GH_PROMPT_DISABLED", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(match input {
                Some(_) => Stdio::piped(),
                None => Stdio::null(),
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let Ok((mut child, group)) = super::tools::Group::spawn(&mut command, false) else {
            return Ran::default();
        };
        let writer = child.stdin.take().map(|mut stdin| {
            let input = input.unwrap_or_default().to_owned();
            std::thread::spawn(move || stdin.write_all(input.as_bytes()).is_ok())
        });
        let stdout = collect(child.stdout.take());
        let stderr = collect(child.stderr.take());
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if self.cancel.load(Ordering::Relaxed) || started.elapsed() > TIMEOUT {
                group.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let written = writer.is_none_or(|writer| writer.join().unwrap_or(false));
        let (stdout, complete) = joined(stdout);
        let (stderr, _) = joined(stderr);
        let error = String::from_utf8_lossy(&stderr);
        let error = (error.lines().map(str::trim)).find(|line| !line.is_empty());
        Ran {
            spawned: true,
            ok: status.is_some_and(|status| status.success()) && written && complete,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            error: error.map(one_line).unwrap_or_default(),
        }
    }
}

fn collect<R: Read + Send + 'static>(pipe: Option<R>) -> Option<JoinHandle<(Vec<u8>, bool)>> {
    pipe.map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let read = pipe.take(OUTPUT_BYTES + 1).read_to_end(&mut bytes);
            let complete = read.is_ok() && bytes.len() as u64 <= OUTPUT_BYTES;
            (bytes, complete)
        })
    })
}

fn joined(reader: Option<JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default()
}

/// A process's message as one short line of printable text.
fn one_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// The project's `origin` remote, which must be a GitHub repository.
pub(super) fn origin(exec: &mut dyn Exec) -> Result<Repository, String> {
    let ran = exec.run("git", &["remote", "get-url", "origin"], None);
    if !ran.ok {
        return Err("Unable to read the current repository origin".into());
    }
    let url = (ran.stdout.strip_suffix("\r\n"))
        .or_else(|| ran.stdout.strip_suffix('\n'))
        .unwrap_or(&ran.stdout);
    parse_origin(url)
}

/// The repository of an `origin` URL, as Scramjet's `parseForgeOrigin` accepts them:
/// `git@github.com:owner/name`, `https://github.com/owner/name` or `ssh://git@github.com/...`,
/// with or without `.git`, and nothing that could make it name something else.
pub(super) fn parse_origin(url: &str) -> Result<Repository, String> {
    let not_canonical = || Err("origin is not canonical".to_owned());
    if url != url.trim() || url.contains('%') || has_dot_segment(url) {
        return not_canonical();
    }
    if let Some(rest) = url.strip_prefix("git@") {
        let Some((host, path)) = rest.split_once(':') else {
            return Err("origin is not a supported URL".into());
        };
        if !path.contains(['?', '#']) && matches!(host, "github.com" | "gitlab.com") {
            return from_path(host, path);
        }
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("origin is not a supported URL")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "ssh" {
        return Err("origin protocol is unsupported".into());
    }
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (user, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    if let Some((_, port)) = host.split_once(':') {
        return match !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            true => not_canonical(),
            false => Err("origin is not a supported URL".into()),
        };
    }
    let host = host.to_ascii_lowercase();
    if host != "github.com" && host != "gitlab.com" {
        return Err("origin host is unsupported".into());
    }
    if path.contains(['?', '#']) || user.is_some_and(|user| user.contains(':')) {
        return Err("origin contains unsupported URL components".into());
    }
    match (scheme.as_str(), user) {
        ("https", Some(_)) => Err("HTTPS origin must not contain credentials".into()),
        ("ssh", user) if user != Some("git") => Err("SSH origin must use the git user".into()),
        _ => from_path(&host, path),
    }
}

/// The repository a `repository` argument names: exactly `https://github.com/owner/name`, as
/// Scramjet's `parseForgeRepository` accepts it.
pub(super) fn parse_target(url: &str) -> Result<Repository, String> {
    let invalid = || {
        "Repository target must be a canonical public HTTPS URL without credentials or suffixes"
            .to_owned()
    };
    let (host, path) = ["github.com", "gitlab.com"]
        .into_iter()
        .find_map(|host| Some((host, url.strip_prefix(&format!("https://{host}"))?)))
        .ok_or_else(invalid)?;
    if !path.starts_with('/')
        || path.contains(['%', '\\', '?', '#', '@', ':'])
        || has_dot_segment(url)
        || path.ends_with('/')
        || path.ends_with(".git")
    {
        return Err(invalid());
    }
    from_path(host, path)
}

/// Whether a segment of `url` is `.` or `..`.
fn has_dot_segment(url: &str) -> bool {
    ["/./", "/../"].iter().any(|dots| url.contains(dots))
        || url.ends_with("/.")
        || url.ends_with("/..")
}

fn from_path(host: &str, path: &str) -> Result<Repository, String> {
    if host == "gitlab.com" {
        return Err("GitLab repositories are not supported yet; only GitHub ones are".into());
    }
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    let unsafe_text = |segment: &str| {
        segment.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{fdd0}'..='\u{fdef}')
                || (c as u32) & 0xfffe == 0xfffe
        })
    };
    if segments.iter().any(|segment| {
        segment.is_empty() || *segment == "." || *segment == ".." || unsafe_text(segment)
    }) {
        return Err("origin path is malformed".into());
    }
    match segments[..] {
        [owner, name] => Ok(Repository {
            owner: owner.to_owned(),
            name: name.to_owned(),
        }),
        _ => Err("GitHub origin must contain owner and repository".into()),
    }
}

/// `gh api` reading `endpoint`: its JSON, else what `gh` said, if anything.
fn fetch(exec: &mut dyn Exec, endpoint: &str) -> Result<Value, String> {
    let ran = exec.run("gh", &["api", "--hostname", "github.com", endpoint], None);
    match ran.ok {
        true => serde_json::from_str(&ran.stdout).map_err(|_| String::new()),
        false => Err(ran.error),
    }
}

/// `message`, with what `gh` said after it.
fn with_cause(message: &str, cause: &str) -> String {
    match cause.is_empty() {
        true => message.to_owned(),
        false => format!("{message} ({cause})"),
    }
}

/// Checks that `repository` is GitHub's own name for it, and that a comment's parent is the
/// issue or pull request asked for; returns the repository's id, which must not change before
/// the write.
pub(super) fn preflight(
    exec: &mut dyn Exec,
    repository: &Repository,
    request: &Request,
) -> Result<u64, String> {
    let root = repository.api();
    let metadata = fetch(exec, &root).map_err(|cause| {
        with_cause(
            "Unable to read the requested GitHub repository metadata",
            &cause,
        )
    })?;
    let id = metadata["id"].as_u64().filter(|id| *id > 0);
    let (Some(id), Some(full_name), Some(url)) = (
        id,
        metadata["full_name"].as_str(),
        metadata["html_url"].as_str(),
    ) else {
        return Err("Unable to read the requested GitHub repository metadata".into());
    };
    if full_name.to_lowercase() != repository.shown().to_lowercase()
        || !github_url(url, "github.com", &repository.shown(), None)
    {
        return Err("Repository origin is not the forge canonical identity".into());
    }
    if let Request::Comment { number, pull, .. } = request {
        let mismatch = || "Publication target does not match the requested GitHub artifact type";
        let parent = fetch(exec, &format!("{root}/issues/{number}"))
            .map_err(|cause| with_cause(mismatch(), &cause))?;
        let is_pull = parent.get("pull_request").is_some();
        let kind = if is_pull { "pull" } else { "issues" };
        let path = format!("{}/{kind}/{number}", repository.shown());
        let matches = parent["number"].as_u64() == Some(*number)
            && (parent["html_url"].as_str())
                .is_some_and(|url| github_url(url, "github.com", &path, None))
            && is_pull == *pull;
        if !matches {
            return Err(mismatch().into());
        }
    }
    Ok(id)
}

/// Checks that a pull request's `head` and `base` are plain branches that exist on the remote:
/// `origin`, or `external` when the repository is another one.
pub(super) fn branches(
    exec: &mut dyn Exec,
    head: &str,
    base: &str,
    external: Option<&Repository>,
) -> Result<(), String> {
    let remote = match external {
        Some(repository) => format!("https://github.com/{}.git", repository.shown()),
        None => "origin".to_owned(),
    };
    for branch in [head, base] {
        if branch.is_empty()
            || branch == "HEAD"
            || branch.starts_with(['-'])
            || branch.starts_with("origin/")
            || branch.starts_with("refs/")
            || branch.contains(':')
        {
            return Err(
                "PR branches must be unqualified branches in the current repository".into(),
            );
        }
        if !exec
            .run("git", &["check-ref-format", "--branch", branch], None)
            .ok
        {
            return Err(format!("Invalid PR branch: {branch}"));
        }
        let reference = format!("refs/heads/{branch}");
        let args = [
            "ls-remote",
            "--exit-code",
            "--refs",
            "--heads",
            &remote,
            &reference,
        ];
        let listed = exec.run("git", &args, None);
        let found =
            (listed.stdout.lines()).any(|line| line.split('\t').nth(1) == Some(reference.as_str()));
        if !listed.ok || !found {
            return Err(format!("Remote branch does not exist: {branch}"));
        }
    }
    Ok(())
}

/// Writes `request` to `repository` with one `gh api` POST, then reads back what it made.
pub(super) fn publish(exec: &mut dyn Exec, repository: &Repository, request: &Request) -> Posted {
    let root = repository.api();
    let (endpoint, payload) = match request {
        Request::Issue { title, body } => {
            (format!("{root}/issues"), json!({"title":title,"body":body}))
        }
        Request::Pull {
            title,
            body,
            head,
            base,
            draft,
        } => (
            format!("{root}/pulls"),
            json!({"title":title,"body":body,"head":head,"base":base,"draft":draft}),
        ),
        Request::Comment { number, body, .. } => (
            format!("{root}/issues/{number}/comments"),
            json!({"body":body}),
        ),
    };
    let args = [
        "api",
        "--hostname",
        "github.com",
        "--method",
        "POST",
        "--input",
        "-",
        &endpoint,
    ];
    let ran = exec.run("gh", &args, Some(&payload.to_string()));
    if !ran.spawned {
        return Posted::NotSent("mutation-process-did-not-spawn".into());
    }
    if !ran.ok {
        return Posted::Ambiguous(with_cause(
            "mutation-process-failed-after-spawn",
            &ran.error,
        ));
    }
    let Ok(item) = serde_json::from_str::<Value>(&ran.stdout) else {
        return Posted::Ambiguous("mutation-response-identity-invalid".into());
    };
    let ambiguous = |reason: &str| Posted::Ambiguous(reason.into());
    match request {
        Request::Issue { title, body } | Request::Pull { title, body, .. } => {
            let pull = matches!(request, Request::Pull { .. });
            let (Some(number), Some(url)) = (item["number"].as_u64(), item["html_url"].as_str())
            else {
                return ambiguous("mutation-response-identity-invalid");
            };
            let kind = if pull { "pull" } else { "issues" };
            let path = format!("{}/{kind}/{number}", repository.shown());
            if !github_url(url, "github.com", &path, None) {
                return ambiguous("mutation-response-identity-invalid");
            }
            let kind = if pull { "pulls" } else { "issues" };
            let Ok(value) = fetch(exec, &format!("{root}/{kind}/{number}")) else {
                return ambiguous("verification-request-failed");
            };
            let same = value["number"].as_u64() == Some(number)
                && value["html_url"].as_str() == Some(url)
                && value["title"].as_str() == Some(title)
                && same_body(&value["body"], body);
            if !same || (!pull && value.get("pull_request").is_some()) {
                return ambiguous("verification-mismatch");
            }
            if let Request::Pull {
                head, base, draft, ..
            } = request
            {
                let repository_is = |side: &Value| {
                    (side["repo"]["full_name"].as_str()).is_some_and(|name| {
                        name.to_lowercase() == repository.shown().to_lowercase()
                    })
                };
                let (got_head, got_base) = (&value["head"], &value["base"]);
                let same = got_head["ref"].as_str() == Some(head)
                    && got_base["ref"].as_str() == Some(base)
                    && repository_is(got_head)
                    && repository_is(got_base)
                    && value["draft"].as_bool() == Some(*draft);
                if !same {
                    return ambiguous("verification-mismatch");
                }
            }
            Posted::Verified(url.to_owned())
        }
        Request::Comment { number, pull, body } => {
            let (Some(id), Some(url)) = (item["id"].as_u64(), item["html_url"].as_str()) else {
                return ambiguous("mutation-response-identity-invalid");
            };
            let kind = if *pull { "pull" } else { "issues" };
            let path = format!("{}/{kind}/{number}", repository.shown());
            let fragment = format!("issuecomment-{id}");
            if !github_url(url, "github.com", &path, Some(&fragment)) {
                return ambiguous("mutation-response-identity-invalid");
            }
            let Ok(value) = fetch(exec, &format!("{root}/issues/comments/{id}")) else {
                return ambiguous("verification-request-failed");
            };
            let issue = format!("repos/{}/issues/{number}", repository.shown());
            let same = value["id"].as_u64() == Some(id)
                && value["html_url"].as_str() == Some(url)
                && same_body(&value["body"], body)
                && (value["issue_url"].as_str())
                    .is_some_and(|url| github_url(url, "api.github.com", &issue, None));
            match same {
                true => Posted::Verified(url.to_owned()),
                false => ambiguous("verification-mismatch"),
            }
        }
    }
}

/// GitHub gives an empty body as `null`.
fn same_body(got: &Value, body: &str) -> bool {
    match got {
        Value::String(got) => got == body,
        Value::Null => body.is_empty(),
        _ => false,
    }
}

/// Whether `url` is `https://<host>/<path>`, compared without case, with nothing else but
/// `#<fragment>` when one is given.
fn github_url(url: &str, host: &str, path: &str, fragment: Option<&str>) -> bool {
    let Some(rest) = url.strip_prefix(&format!("https://{host}/")) else {
        return false;
    };
    let (got, got_fragment) = match rest.split_once('#') {
        Some((got, fragment)) => (got, Some(fragment)),
        None => (rest, None),
    };
    got_fragment == fragment && !got.contains('?') && got.to_lowercase() == path.to_lowercase()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// Scripted `gh` and `git`: each command line, as `program args…`, gets the first reply
    /// whose key it starts with; the lines run are kept with their input.
    #[derive(Default)]
    pub(in crate::agent) struct Script {
        pub replies: Vec<(String, Ran)>,
        pub ran: Vec<(String, Option<String>)>,
    }

    impl Script {
        pub fn reply(mut self, key: &str, stdout: Value) -> Self {
            let ran = Ran {
                spawned: true,
                ok: true,
                stdout: stdout.to_string(),
                error: String::new(),
            };
            self.replies.push((key.to_owned(), ran));
            self
        }
        pub fn fail(mut self, key: &str, error: &str) -> Self {
            let ran = Ran {
                spawned: true,
                ok: false,
                stdout: String::new(),
                error: error.to_owned(),
            };
            self.replies.push((key.to_owned(), ran));
            self
        }
        /// A GitHub repository `o/r` as origin, with branches `main` and `feature`, issue 41 and
        /// pull request 42.
        pub fn github() -> Self {
            let listed = |branch: &str| Ran {
                spawned: true,
                ok: true,
                stdout: format!("abc123\trefs/heads/{branch}\n"),
                error: String::new(),
            };
            let mut script = Self::default();
            script.replies.extend([
                (
                    "git remote get-url origin".to_owned(),
                    Ran {
                        spawned: true,
                        ok: true,
                        stdout: "git@github.com:o/r.git\n".into(),
                        error: String::new(),
                    },
                ),
                (
                    "git check-ref-format".to_owned(),
                    Ran {
                        spawned: true,
                        ok: true,
                        ..Ran::default()
                    },
                ),
                (
                    "git ls-remote --exit-code --refs --heads origin refs/heads/main".to_owned(),
                    listed("main"),
                ),
                (
                    "git ls-remote --exit-code --refs --heads origin refs/heads/feature".to_owned(),
                    listed("feature"),
                ),
            ]);
            script
                .reply(
                    "gh api --hostname github.com repos/o/r/issues/41",
                    json!({
                    "number":41,"html_url":"https://github.com/o/r/issues/41"}),
                )
                .reply(
                    "gh api --hostname github.com repos/o/r/issues/42",
                    json!({
                    "number":42,"html_url":"https://github.com/o/r/pull/42","pull_request":{}}),
                )
                .reply(
                    "gh api --hostname github.com repos/o/r",
                    json!({
                    "id":7,"full_name":"o/r","html_url":"https://github.com/o/r"}),
                )
        }
        /// A POST to `endpoint` creating what `made` is, which reads back as `read` says.
        pub fn created(self, endpoint: &str, made: Value, read: (&str, Value)) -> Self {
            let post = format!("gh api --hostname github.com --method POST --input - {endpoint}");
            let mut script = self.reply(&post, made);
            // The read must come before the shorter keys of the same prefix.
            let get = format!("gh api --hostname github.com {}", read.0);
            let ran = Ran {
                spawned: true,
                ok: true,
                stdout: read.1.to_string(),
                error: String::new(),
            };
            script.replies.insert(0, (get, ran));
            script
        }
        pub fn posts(&self) -> Vec<&(String, Option<String>)> {
            self.ran
                .iter()
                .filter(|(line, _)| line.contains("POST"))
                .collect()
        }
    }

    impl Exec for Script {
        fn run(&mut self, program: &str, args: &[&str], input: Option<&str>) -> Ran {
            let line = [&[program], args].concat().join(" ");
            self.ran.push((line.clone(), input.map(str::to_owned)));
            let reply = self
                .replies
                .iter()
                .find(|(key, _)| line.starts_with(key.as_str()));
            match reply {
                Some((_, ran)) => Ran {
                    spawned: ran.spawned,
                    ok: ran.ok,
                    stdout: ran.stdout.clone(),
                    error: ran.error.clone(),
                },
                None => Ran {
                    spawned: true,
                    ok: false,
                    stdout: String::new(),
                    error: format!("gh: Not Found (HTTP 404) for {line}"),
                },
            }
        }
    }

    fn repository() -> Repository {
        Repository {
            owner: "o".into(),
            name: "r".into(),
        }
    }

    #[test]
    fn origins_are_accepted_only_in_forms_that_name_one_github_repository() {
        for url in [
            "git@github.com:o/r.git",
            "git@github.com:o/r",
            "https://github.com/o/r.git",
            "https://GitHub.com/o/r",
            "ssh://git@github.com/o/r.git",
        ] {
            assert_eq!(parse_origin(url), Ok(repository()), "{url}");
        }
        for (url, error) in [
            (" git@github.com:o/r", "origin is not canonical"),
            ("https://github.com/o/%72", "origin is not canonical"),
            ("https://github.com/o/../r", "origin is not canonical"),
            ("https://github.com:443/o/r", "origin is not canonical"),
            (
                "https://token@github.com/o/r",
                "HTTPS origin must not contain credentials",
            ),
            (
                "https://user:pass@github.com/o/r",
                "origin contains unsupported URL components",
            ),
            (
                "ssh://me@github.com/o/r",
                "SSH origin must use the git user",
            ),
            (
                "https://github.com/o/r?x=1",
                "origin contains unsupported URL components",
            ),
            ("http://github.com/o/r", "origin protocol is unsupported"),
            ("https://example.com/o/r", "origin host is unsupported"),
            ("git@example.com:o/r", "origin is not a supported URL"),
            ("/home/me/r", "origin is not a supported URL"),
            (
                "https://github.com/o/r/x",
                "GitHub origin must contain owner and repository",
            ),
            ("https://github.com/o//r", "origin path is malformed"),
            ("https://github.com/o/r\u{202e}", "origin path is malformed"),
            (
                "git@gitlab.com:group/sub/r.git",
                "GitLab repositories are not supported yet; only GitHub ones are",
            ),
        ] {
            assert_eq!(parse_origin(url), Err(error.to_owned()), "{url}");
        }
    }

    #[test]
    fn a_repository_target_must_be_its_canonical_https_url() {
        assert_eq!(parse_target("https://github.com/o/r"), Ok(repository()));
        let invalid = "Repository target must be a canonical public HTTPS URL without credentials \
or suffixes";
        for url in [
            "https://github.com/o/r/",
            "https://github.com/o/r.git",
            "https://github.com/o/r?tab=issues",
            "https://github.com/o/r#readme",
            "https://github.com/o/%72",
            "https://github.com/o/./r",
            "https://user@github.com/o/r",
            "https://github.com.evil/o/r",
            "http://github.com/o/r",
            "github.com/o/r",
            "o/r",
        ] {
            assert_eq!(parse_target(url), Err(invalid.to_owned()), "{url}");
        }
        assert_eq!(
            parse_target("https://github.com/o/r/issues"),
            Err("GitHub origin must contain owner and repository".to_owned())
        );
    }

    #[test]
    fn preflight_checks_the_repositorys_identity_and_a_comments_parent() {
        let comment = |number, pull| Request::Comment {
            number,
            pull,
            body: "b".into(),
        };
        let issue = Request::Issue {
            title: "t".into(),
            body: "b".into(),
        };
        let mut script = Script::github();
        assert_eq!(preflight(&mut script, &repository(), &issue), Ok(7));
        assert_eq!(
            preflight(&mut script, &repository(), &comment(41, false)),
            Ok(7)
        );
        assert_eq!(
            preflight(&mut script, &repository(), &comment(42, true)),
            Ok(7)
        );
        let mismatch = "Publication target does not match the requested GitHub artifact type";
        for (number, pull) in [(41, true), (42, false)] {
            let result = preflight(&mut script, &repository(), &comment(number, pull));
            assert_eq!(result, Err(mismatch.to_owned()), "{number} {pull}");
        }
        let mut missing = Script::github();
        missing.replies.insert(
            0,
            (
                "gh api --hostname github.com repos/o/r/issues/43".into(),
                Ran {
                    spawned: true,
                    error: "gh: Not Found (HTTP 404)".into(),
                    ..Ran::default()
                },
            ),
        );
        assert_eq!(
            preflight(&mut missing, &repository(), &comment(43, false)),
            Err(format!("{mismatch} (gh: Not Found (HTTP 404))"))
        );
        // A repository that was renamed or moved answers with its new name.
        let mut moved = Script::default().reply(
            "gh api --hostname github.com repos/o/r",
            json!({"id":7,"full_name":"o/new","html_url":"https://github.com/o/new"}),
        );
        assert_eq!(
            preflight(&mut moved, &repository(), &issue),
            Err("Repository origin is not the forge canonical identity".to_owned())
        );
        let mut unknown = Script::default().fail("gh api", "gh: Not Found (HTTP 404)");
        assert_eq!(
            preflight(&mut unknown, &repository(), &issue),
            Err(
                "Unable to read the requested GitHub repository metadata (gh: Not Found (HTTP \
404))"
                    .to_owned()
            )
        );
    }

    #[test]
    fn pull_request_branches_must_be_plain_and_on_the_remote() {
        let mut script = Script::github();
        assert_eq!(branches(&mut script, "feature", "main", None), Ok(()));
        let plain = "PR branches must be unqualified branches in the current repository";
        for branch in ["", "HEAD", "-x", "origin/main", "refs/heads/main", "a:b"] {
            let result = branches(&mut script, branch, "main", None);
            assert_eq!(result, Err(plain.to_owned()), "{branch}");
        }
        assert_eq!(
            branches(&mut script, "gone", "main", None),
            Err("Remote branch does not exist: gone".to_owned())
        );
        // Another repository's branches are checked there, not on origin.
        let mut script = Script::github();
        let other = Repository {
            owner: "x".into(),
            name: "y".into(),
        };
        assert!(branches(&mut script, "feature", "main", Some(&other)).is_err());
        assert!(script.ran.iter().any(|(line, _)| line
            == "git ls-remote --exit-code --refs --heads https://github.com/x/y.git \
refs/heads/feature"));
    }

    #[test]
    fn a_write_is_verified_by_reading_back_what_it_made() {
        let issue = Request::Issue {
            title: "Title".into(),
            body: "".into(),
        };
        let made = json!({"number":5,"html_url":"https://github.com/o/r/issues/5"});
        let read = json!({"number":5,"html_url":"https://github.com/o/r/issues/5",
            "title":"Title","body":null});
        let mut script = Script::github().created(
            "repos/o/r/issues",
            made.clone(),
            ("repos/o/r/issues/5", read),
        );
        assert_eq!(
            publish(&mut script, &repository(), &issue),
            Posted::Verified("https://github.com/o/r/issues/5".into())
        );
        let posts = script.posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(
            posts[0].1.as_deref(),
            Some(r#"{"body":"","title":"Title"}"#)
        );
        // What reads back differently, or not at all, may still have been written.
        let other = json!({"number":5,"html_url":"https://github.com/o/r/issues/5",
            "title":"Other","body":null});
        let mut script = Script::github().created(
            "repos/o/r/issues",
            made.clone(),
            ("repos/o/r/issues/5", other),
        );
        assert_eq!(
            publish(&mut script, &repository(), &issue),
            Posted::Ambiguous("verification-mismatch".into())
        );
        let mut script = Script::github().reply(
            "gh api --hostname github.com --method POST --input - repos/o/r/issues",
            made,
        );
        script.replies.insert(
            0,
            (
                "gh api --hostname github.com repos/o/r/issues/5".into(),
                Ran {
                    spawned: true,
                    ..Ran::default()
                },
            ),
        );
        assert_eq!(
            publish(&mut script, &repository(), &issue),
            Posted::Ambiguous("verification-request-failed".into())
        );
        let mut script = Script::github().fail(
            "gh api --hostname github.com --method POST",
            "gh: \
Validation Failed (HTTP 422)",
        );
        assert_eq!(
            publish(&mut script, &repository(), &issue),
            Posted::Ambiguous(
                "mutation-process-failed-after-spawn (gh: Validation Failed (HTTP 422))".into()
            )
        );
        let mut script = Script::default();
        script.replies.push(("gh".into(), Ran::default()));
        assert_eq!(
            publish(&mut script, &repository(), &issue),
            Posted::NotSent("mutation-process-did-not-spawn".into())
        );
        // A reply naming another repository's issue is not this write's.
        let elsewhere = json!({"number":5,"html_url":"https://github.com/o/other/issues/5"});
        let mut script = Script::github().reply(
            "gh api --hostname github.com --method POST --input - repos/o/r/issues",
            elsewhere,
        );
        assert_eq!(
            publish(&mut script, &repository(), &issue),
            Posted::Ambiguous("mutation-response-identity-invalid".into())
        );
    }

    #[test]
    fn pull_requests_and_comments_are_read_back_with_their_own_fields() {
        let pull = Request::Pull {
            title: "T".into(),
            body: "B".into(),
            head: "feature".into(),
            base: "main".into(),
            draft: true,
        };
        let side = |branch: &str| json!({"ref":branch,"repo":{"full_name":"O/R"}});
        let read = |draft: bool| {
            json!({"number":9,"html_url":"https://github.com/o/r/pull/9","title":"T","body":"B",
                "head":side("feature"),"base":side("main"),"draft":draft})
        };
        let made = json!({"number":9,"html_url":"https://github.com/o/r/pull/9"});
        for (draft, written) in [
            (
                true,
                Posted::Verified("https://github.com/o/r/pull/9".into()),
            ),
            (false, Posted::Ambiguous("verification-mismatch".into())),
        ] {
            let mut script = Script::github().created(
                "repos/o/r/pulls",
                made.clone(),
                ("repos/o/r/pulls/9", read(draft)),
            );
            assert_eq!(publish(&mut script, &repository(), &pull), written);
            assert_eq!(
                script.posts()[0].1.as_deref(),
                Some(r#"{"base":"main","body":"B","draft":true,"head":"feature","title":"T"}"#)
            );
        }
        let comment = Request::Comment {
            number: 42,
            pull: true,
            body: "Looks good.".into(),
        };
        let url = "https://github.com/o/r/pull/42#issuecomment-77";
        let made = json!({"id":77,"html_url":url});
        let read = json!({"id":77,"html_url":url,"body":"Looks good.",
            "issue_url":"https://api.github.com/repos/o/r/issues/42"});
        let mut script = Script::github().created(
            "repos/o/r/issues/42/comments",
            made,
            ("repos/o/r/issues/comments/77", read),
        );
        assert_eq!(
            publish(&mut script, &repository(), &comment),
            Posted::Verified(url.into())
        );
        assert_eq!(
            script.posts()[0].1.as_deref(),
            Some(r#"{"body":"Looks good."}"#)
        );
        // A comment that landed on an issue's page is not one on the pull request.
        let made = json!({"id":77,"html_url":"https://github.com/o/r/issues/42#issuecomment-77"});
        let mut script = Script::github().reply(
            "gh api --hostname github.com --method POST --input - repos/o/r/issues/42/comments",
            made,
        );
        assert_eq!(
            publish(&mut script, &repository(), &comment),
            Posted::Ambiguous("mutation-response-identity-invalid".into())
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn the_system_runs_a_program_with_its_input_and_reports_how_it_ended() {
        let root = std::env::temp_dir();
        let cancel = AtomicBool::new(false);
        let mut system = System {
            root: &root,
            cancel: &cancel,
        };
        let ran = system.run("sh", &["-c", "cat; echo; echo oops >&2"], Some("in"));
        assert!(ran.spawned && ran.ok);
        assert_eq!((ran.stdout.as_str(), ran.error.as_str()), ("in\n", "oops"));
        let ran = system.run("sh", &["-c", "exit 3"], None);
        assert!(ran.spawned && !ran.ok);
        let ran = system.run("/nonexistent/gh", &[], None);
        assert!(!ran.spawned && !ran.ok);
        cancel.store(true, Ordering::Relaxed);
        let started = Instant::now();
        let ran = system.run("sleep", &["10"], None);
        assert!(ran.spawned && !ran.ok && started.elapsed() < Duration::from_secs(5));
    }
}
