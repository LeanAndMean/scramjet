//! The branch's open pull request on GitHub, for the footer, as Scramjet 1's `scramjet` package
//! found it (`pr-indicator.ts`): `origin` on GitHub, a branch checked out, and exactly one open
//! pull request from it in `gh pr list`; nothing in every other case. Each lookup runs on a
//! thread of its own, so that `git` and `gh` never hold up a frame.

use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// How long each `git` or `gh` may run, as in Scramjet 1.
const TIMEOUT: Duration = Duration::from_secs(3);
/// A lookup asked for this soon after the last one started is not made, but for a new branch.
const AGAIN: Duration = Duration::from_secs(10);

/// A branch, with its open pull request's number if it has one.
type Found = (String, Option<u64>);

pub(super) struct PullRequest {
    root: PathBuf,
    /// The `PATH` the programs are found on, the process's own unless a test gives one.
    path: Option<OsString>,
    /// The branch the last lookup read, and its pull request's number if it has one.
    found: Arc<Mutex<Option<Found>>>,
    running: Arc<AtomicBool>,
    asked: Option<Instant>,
}

impl PullRequest {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            path: None,
            found: Arc::default(),
            running: Arc::default(),
            asked: None,
        }
    }

    /// Looks the pull request up again, unless a lookup is running or started within `AGAIN`.
    pub(super) fn look(&mut self) {
        if self.asked.is_some_and(|asked| asked.elapsed() < AGAIN) {
            return;
        }
        self.look_now();
    }

    /// Looks the pull request up again, unless a lookup is running.
    pub(super) fn look_now(&mut self) {
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        self.asked = Some(Instant::now());
        let (root, path) = (self.root.clone(), self.path.clone());
        let (found, running) = (self.found.clone(), self.running.clone());
        std::thread::spawn(move || {
            let result = lookup(&root, path.as_deref());
            *found.lock().unwrap_or_else(|e| e.into_inner()) = result;
            running.store(false, Ordering::Release);
        });
    }

    #[cfg(test)]
    pub(super) fn found(&self, branch: &str, number: Option<u64>) {
        *self.found.lock().unwrap() = Some((branch.to_owned(), number));
    }

    /// The pull request's number, if the last lookup found one for `branch`.
    pub(super) fn of(&self, branch: &str) -> Option<u64> {
        match &*self.found.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((read, number)) if read == branch => *number,
            _ => None,
        }
    }
}

/// The branch checked out in `root`, with its one open pull request's number if it has exactly
/// one; `None` when the branch cannot be read or no lookup is possible.
fn lookup(root: &Path, path: Option<&std::ffi::OsStr>) -> Option<Found> {
    let run = |program: &str, args: &[&str]| run(root, path, program, args);
    let remote = run("git", &["remote", "get-url", "origin"])?;
    let branch = run("git", &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = branch.trim().to_owned();
    if branch.is_empty() || branch == "HEAD" {
        return None;
    }
    if !remote.contains("github.com") {
        return Some((branch, None));
    }
    let listed = run(
        "gh",
        &[
            "pr", "list", "--head", &branch, "--state", "open", "--json", "number",
        ],
    );
    let number = listed.and_then(|listed| one(&listed));
    Some((branch, number))
}

/// The number of the only pull request in `gh pr list`'s JSON.
fn one(listed: &str) -> Option<u64> {
    match serde_json::from_str::<serde_json::Value>(listed).ok()? {
        serde_json::Value::Array(pulls) if pulls.len() == 1 => pulls[0]["number"].as_u64(),
        _ => None,
    }
}

/// What `program` wrote to stdout, if it succeeded within `TIMEOUT`.
fn run(
    root: &Path,
    path: Option<&std::ffi::OsStr>,
    program: &str,
    args: &[&str],
) -> Option<String> {
    let mut command = Command::new(scramjet::os::program(program, path));
    command
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(path) = path {
        command.env("PATH", path);
    }
    // Nothing they start may ask on the session's terminal, or open a console window.
    let mut command = scramjet::os::own_session_and_console(command).ok()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = scramjet::os::spawn_session(&mut command).ok()?;
    scramjet::os::hold_group(&child);
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stdout).take(1 << 20).read_to_end(&mut bytes);
        bytes
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                scramjet::os::kill_group(child.id());
                let _ = child.wait();
                break None;
            }
        }
    };
    scramjet::os::release_group(child.id());
    let bytes = reader.join().ok()?;
    status?
        .success()
        .then(|| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@localhost"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A `gh` that writes `listed`, or fails when it is `None`, and records its arguments.
    fn gh(bin: &Path, listed: Option<&str>) {
        let body = match listed {
            Some(listed) => format!("echo '{listed}'"),
            None => "echo 'not logged in' >&2; exit 1".to_owned(),
        };
        let script = bin.join("gh");
        let log = bin.join("gh.log");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn only_one_open_pull_request_from_a_github_branch_is_shown() {
        let guard = Scratch(
            std::env::temp_dir().join(format!("scramjet-rs-pull-request-{}", std::process::id())),
        );
        let scratch = &guard.0;
        let _ = std::fs::remove_dir_all(scratch);
        let (root, bin) = (scratch.join("repo"), scratch.join("bin"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let mut path = OsString::from(&bin);
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap_or_default());
        let look = || lookup(&root, Some(&path));

        // Not a repository yet.
        gh(&bin, Some(r#"[{"number":72}]"#));
        assert_eq!(look(), None);
        git(&root, &["init", "-q"]);
        git(&root, &["commit", "-q", "--allow-empty", "-m", "start"]);
        git(&root, &["checkout", "-q", "-b", "feature/x"]);
        // No `origin`, then one elsewhere than GitHub: `gh` is not asked.
        assert_eq!(look(), None);
        git(
            &root,
            &["remote", "add", "origin", "https://gitlab.com/o/r.git"],
        );
        assert_eq!(look(), Some(("feature/x".into(), None)));
        assert!(!bin.join("gh.log").exists());

        git(
            &root,
            &["remote", "set-url", "origin", "git@github.com:o/r.git"],
        );
        assert_eq!(look(), Some(("feature/x".into(), Some(72))));
        let asked = std::fs::read_to_string(bin.join("gh.log")).unwrap();
        assert_eq!(
            asked,
            "pr list --head feature/x --state open --json number\n"
        );
        for listed in [
            Some("[]"),
            Some(r#"[{"number":1},{"number":2}]"#),
            Some("{"),
            None,
        ] {
            gh(&bin, listed);
            assert_eq!(look(), Some(("feature/x".into(), None)), "{listed:?}");
        }

        // A detached `HEAD` has no branch to show one for.
        gh(&bin, Some(r#"[{"number":72}]"#));
        git(&root, &["checkout", "-q", "--detach"]);
        assert_eq!(look(), None);
    }

    #[test]
    fn a_program_that_hangs_is_stopped() {
        let guard = Scratch(std::env::temp_dir().join(format!(
            "scramjet-rs-pull-request-hang-{}",
            std::process::id()
        )));
        std::fs::create_dir_all(&guard.0).unwrap();
        let started = Instant::now();
        // What it started is stopped too, so that its output ends.
        assert_eq!(
            run(&guard.0, None, "sh", &["-c", "sleep 30 & sleep 30"]),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_lookup_counts_only_for_the_branch_it_read() {
        let pull = PullRequest::new(Path::new("/"));
        *pull.found.lock().unwrap() = Some(("main".into(), Some(5)));
        assert_eq!(pull.of("main"), Some(5));
        assert_eq!(pull.of("other"), None);
    }
}
