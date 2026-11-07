//! Logins a package's program does. A `models.json`'s `logins` names, for each provider, what the
//! user knows it by and the program (`{"acme": {"name": "Acme", "command": ["bin/login"]}}`, a
//! path from the file's directory); `/login` lists them after its own. The program is run in the
//! file's directory once for each action, given one line on its input,
//! `{"version": 1, "action": …, "provider": …}`, then the input's end, and answers in lines of
//! JSON objects on its output:
//!
//! - `login`: `{"open": URL}` has the browser open a page, `{"say": TEXT}` tells the user what
//!   it waits for, each as often as it needs; then `{"credential": {"access", "expires", …}}`,
//!   the token requests carry and when it expires (milliseconds since the epoch), with anything
//!   else the program keeps. It is saved in `auth.json` as `/login`'s own logins are.
//! - `refresh`, given the saved `credential` once it expired: the renewed `credential`, whose
//!   fields replace the saved ones.
//! - `models`, given the `credential`, fresh: `{"provider": {…}}`, the provider's entry in a
//!   `models.json`'s format with its models, and a `warning` if it has one. It is saved as the
//!   lists of `/login`'s own logins are, and asked for again as they age.
//!
//! Any of them may answer `{"error": TEXT, "transient": BOOL}`, transient when asking again may
//! succeed. Keys this version does not know are passed over.

use super::{CANCELLED, Logins};
use crate::agent::client::Failure;
use serde_json::{Map, Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const VERSION: u64 = 1;
/// How long a renewal or a model list may take; a login takes as long as the user does.
const LIMIT: Duration = Duration::from_secs(60);
/// How long the program has to end once it answered, before it is stopped.
const ENDING: Duration = Duration::from_secs(2);
const OUTPUT_BYTES: u64 = 16 << 20;
const ERROR_BYTES: usize = 64 << 10;
/// The keys of an answer, the last line read.
const ANSWERS: [&str; 3] = ["credential", "provider", "error"];

/// A login a package's program does.
#[derive(Clone, Debug, PartialEq)]
pub struct LoginProgram {
    /// The provider it logs in to, as `auth.json` and the models name it.
    pub provider: String,
    /// What the user knows it by.
    pub name: String,
    /// The program and its arguments, as the file gives them.
    command: Vec<String>,
    /// The file's directory, where it runs.
    directory: PathBuf,
}

/// What a login's program has the user told while it waits.
#[derive(Debug, PartialEq)]
pub enum Said {
    /// A page to open in the browser.
    Open(String),
    Say(String),
}

impl LoginProgram {
    /// `command` is a program and its arguments, not empty.
    pub(in crate::agent) fn new(
        provider: String,
        name: String,
        command: Vec<String>,
        directory: PathBuf,
    ) -> Self {
        Self {
            provider,
            name,
            command,
            directory,
        }
    }

    /// Logs in, `heard` hearing what the program tells the user, and saves the credential, then
    /// the models it lists, in `logins`; with a warning when those could not be read or the
    /// program gave one. `cancel` stops it.
    pub fn login(
        &self,
        logins: &Logins,
        cancel: &AtomicBool,
        mut heard: impl FnMut(Said),
    ) -> Result<Option<String>, String> {
        let answer = self.run("login", None, None, cancel, &mut heard)?;
        let credential = self.credential(&answer)?;
        let mut saved = credential.clone();
        saved.insert("type".to_owned(), Value::from("oauth"));
        super::save(&logins.auth, &self.provider, Value::Object(saved))?;
        match self.list(logins, &credential, cancel, &mut heard) {
            Ok(warning) => Ok(warning),
            Err(error) => Ok(Some(format!(
                "{}'s models could not be read: {error}; they are asked for again in the \
                 background",
                self.name
            ))),
        }
    }

    /// Reads again the models of the login saved in `logins`, its token renewed first when it
    /// expired; with the program's warning if it gave one.
    pub fn relist(&self, logins: &Logins) -> Result<Option<String>, String> {
        super::access(&logins.auth, &self.provider, Some(self))?;
        let saved = super::saved(&logins.auth)?;
        let credential = (saved.get(&self.provider)).and_then(Value::as_object);
        let credential =
            credential.ok_or_else(|| format!("not logged in to {} (/login)", self.name))?;
        self.list(logins, credential, &AtomicBool::new(false), &mut |_| {})
    }

    /// The credential renewed from the expired `credential`.
    pub(in crate::agent) fn refresh(&self, credential: &Value) -> Result<Value, Failure> {
        let credential = credential.as_object();
        let cancel = AtomicBool::new(false);
        let answer = self.run("refresh", credential, Some(LIMIT), &cancel, &mut |_| {});
        let renewed = answer.and_then(|answer| Ok(self.credential(&answer)?));
        renewed.map(Value::Object).map_err(|failure| Failure {
            message: format!(
                "{} (renewing the {} login; /login if it goes on)",
                failure.message, self.name
            ),
            ..failure
        })
    }

    /// Asks for the models of the login with `credential`, and saves them in `logins`.
    fn list(
        &self,
        logins: &Logins,
        credential: &Map<String, Value>,
        cancel: &AtomicBool,
        heard: &mut dyn FnMut(Said),
    ) -> Result<Option<String>, String> {
        let answer = self.run("models", Some(credential), Some(LIMIT), cancel, heard)?;
        let entry = answer.get("provider").filter(|entry| entry.is_object());
        let entry = entry.ok_or_else(|| format!("{}'s provider is not an object", self.shown()))?;
        let list = json!({"fetched": super::now_ms(), "provider": entry});
        super::save_list(&logins.listed(&self.provider), &list)?;
        Ok(answer
            .get("warning")
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// The credential an answer gives, without a `type`, which `auth.json` gives it.
    fn credential(&self, answer: &Map<String, Value>) -> Result<Map<String, Value>, String> {
        let program = self.shown();
        let credential = answer.get("credential").and_then(Value::as_object);
        let mut credential = credential
            .ok_or_else(|| format!("{program}'s answer has no credential object"))?
            .clone();
        let access = credential.get("access").and_then(Value::as_str);
        let access = access.filter(|access| !access.is_empty());
        let expires = credential.get("expires").and_then(Value::as_f64);
        let (Some(access), Some(_)) = (access, expires) else {
            return Err(format!(
                "{program}'s credential lacks its access token or when it expires"
            ));
        };
        if hyper::header::HeaderValue::from_str(&format!("Bearer {access}")).is_err() {
            return Err(format!(
                "{program}'s access token cannot be sent in a header"
            ));
        }
        credential.remove("type");
        Ok(credential)
    }

    /// The program, as problems name it.
    fn shown(&self) -> &str {
        &self.command[0]
    }

    /// Runs the program for `action`, given `credential`, for at most `limit`, `heard` hearing
    /// what it tells the user; its answer, unless it is an error.
    fn run(
        &self,
        action: &str,
        credential: Option<&Map<String, Value>>,
        limit: Option<Duration>,
        cancel: &AtomicBool,
        heard: &mut dyn FnMut(Said),
    ) -> Result<Map<String, Value>, Failure> {
        let program = self.shown();
        let mut request = json!({"version": VERSION, "action": action, "provider": self.provider});
        if let Some(credential) = credential {
            let mut credential = credential.clone();
            credential.remove("type");
            request["credential"] = Value::Object(credential);
        }
        let path = crate::agent::providers::program_path(program, &self.directory);
        // In a group of its own, so that the processes it starts end with it.
        let mut command = Command::new(&path);
        let mut child = crate::os::own_group(&mut command)
            .args(&self.command[1..])
            .current_dir(&self.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Failure::from(format!("cannot run {program}: {error}")))?;
        let mut input = child.stdin.take().expect("piped");
        let line = request.to_string() + "\n";
        // Apart, so that a program that reads none of it does not hold this one up.
        std::thread::spawn(move || input.write_all(line.as_bytes()));
        let lines = read_lines(child.stdout.take().expect("piped"));
        let (errors, errors_read) = read_errors(child.stderr.take().expect("piped"));
        // Once the program ended: what it wrote last may not have been read yet.
        let said = || {
            let _ = errors_read.recv_timeout(ENDING);
            let errors = errors.lock().unwrap_or_else(PoisonError::into_inner);
            let said = String::from_utf8_lossy(&errors);
            let said = said.lines().map(str::trim).find(|line| !line.is_empty());
            said.map(|line| format!(": {line}")).unwrap_or_default()
        };
        let deadline = limit.map(|limit| Instant::now() + limit);
        let late = |deadline: Option<Instant>| deadline.is_some_and(|at| Instant::now() >= at);
        let unanswered = || {
            let seconds = limit.unwrap_or_default().as_secs();
            Failure::from(format!("{program} gave no answer within {seconds}s"))
        };
        let answer = loop {
            if cancel.load(Ordering::Relaxed) {
                stop(&mut child);
                return Err(Failure::from(CANCELLED.to_owned()));
            }
            if late(deadline) {
                stop(&mut child);
                return Err(unanswered());
            }
            let line = match lines.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(line)) => line,
                Ok(Err(problem)) => {
                    stop(&mut child);
                    return Err(Failure::from(format!("{program}: {problem}")));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break None,
            };
            if line.trim_ascii().is_empty() {
                continue;
            }
            let object = match serde_json::from_slice::<Value>(&line) {
                Ok(Value::Object(object)) => object,
                Ok(_) => {
                    stop(&mut child);
                    let problem = format!("{program}: a line of its output is not a JSON object");
                    return Err(Failure::from(problem));
                }
                Err(error) => {
                    stop(&mut child);
                    let problem = format!("{program}: a line of its output is not JSON: {error}");
                    return Err(Failure::from(problem));
                }
            };
            if let Some(url) = object.get("open").and_then(Value::as_str) {
                heard(Said::Open(url.to_owned()));
            }
            if let Some(text) = object.get("say").and_then(Value::as_str) {
                heard(Said::Say(text.to_owned()));
            }
            if ANSWERS.iter().any(|key| object.contains_key(*key)) {
                break Some(object);
            }
        };
        // Once it answered, what it does after does not matter.
        let ending = match &answer {
            Some(_) => Some(Instant::now() + ENDING),
            None => deadline,
        };
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if answer.is_none() && cancel.load(Ordering::Relaxed) => {
                    stop(&mut child);
                    return Err(Failure::from(CANCELLED.to_owned()));
                }
                Ok(None) if late(ending) => {
                    stop(&mut child);
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    stop(&mut child);
                    return Err(Failure::from(format!("{program}: {error}")));
                }
            }
        };
        match (answer, status) {
            (Some(answer), _) => match answer.get("error") {
                None => Ok(answer),
                Some(error) => {
                    let transient = answer.get("transient") == Some(&Value::Bool(true));
                    let error = match error {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    };
                    Err(Failure::new(error, transient))
                }
            },
            (None, None) => Err(unanswered()),
            (None, Some(status)) if !status.success() => Err(Failure::from(format!(
                "{program} failed ({status}){}",
                said()
            ))),
            (None, Some(_)) => Err(Failure::from(format!(
                "{program} ended without an answer{}",
                said()
            ))),
        }
    }
}

/// Ends the program and what it started.
fn stop(child: &mut Child) {
    crate::os::kill_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// The lines of `output` as they come, up to `OUTPUT_BYTES` in all.
fn read_lines(output: impl Read + Send + 'static) -> mpsc::Receiver<Result<Vec<u8>, String>> {
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output.take(OUTPUT_BYTES));
        loop {
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if sender.send(Ok(line)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    return;
                }
            }
        }
        if reader.into_inner().limit() == 0 {
            let over = format!("its output is over {} MB", OUTPUT_BYTES >> 20);
            let _ = sender.send(Err(over));
        }
    });
    lines
}

/// What `errors` has had written, its first `ERROR_BYTES`, as it comes, and what hears when it
/// closed; the rest is dropped, as a closed pipe would end the program.
fn read_errors(
    mut errors: impl Read + Send + 'static,
) -> (Arc<Mutex<Vec<u8>>>, mpsc::Receiver<()>) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let keeping = kept.clone();
    let (closed, ended) = mpsc::channel();
    std::thread::spawn(move || {
        let mut chunk = [0; 4096];
        while let Ok(read @ 1..) = errors.read(&mut chunk) {
            let mut kept = keeping.lock().unwrap_or_else(PoisonError::into_inner);
            let room = ERROR_BYTES.saturating_sub(kept.len()).min(read);
            kept.extend_from_slice(&chunk[..room]);
        }
        let _ = closed.send(());
    });
    (kept, ended)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Providers;
    use crate::agent::tests::ScratchDir;

    fn scratch(name: &str) -> (ScratchDir, Logins) {
        let path = std::env::temp_dir().join(format!("scramjet-rs-{name}-{}", std::process::id()));
        let scratch = ScratchDir::new(path);
        std::fs::create_dir_all(&scratch.0).unwrap();
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        (scratch, logins)
    }

    fn script(scratch: &ScratchDir, name: &str, body: &str) {
        crate::agent::tests::script(&scratch.0.join(name), body);
    }

    fn program(scratch: &ScratchDir, name: &str) -> LoginProgram {
        let command = vec![format!("./{name}")];
        LoginProgram::new("acme".into(), "Acme".into(), command, scratch.0.clone())
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn a_packages_program_logs_in_renews_the_token_and_lists_the_models() {
        let (scratch, logins) = scratch("login-program");
        let expires = (super::super::now_ms() + 600_000.0) as u64;
        let credential = format!(
            r#"{{"credential":{{"access":"first","refresh":"r1","expires":{expires},"epoch":"e1"}}}}"#
        );
        let provider = r#"{"baseUrl":"http://127.0.0.1:9/v1","api":"openai-responses","models":[{"id":"m1"}]}"#;
        script(
            &scratch,
            "login",
            &format!(
                r#"read -r request; echo "$request" >> requests.log
case "$request" in
*'"action":"login"'*) echo '{{"open":"https://sign-in.example/start?state=1"}}'
  echo; echo '{{"say":"Waiting for the browser sign-in.","later":true}}'; echo '{credential}' ;;
*'"action":"refresh"'*) echo '{{"credential":{{"access":"renewed","expires":99999999999999}}}}' ;;
*'"action":"models"'*) echo '{{"say":"Reading the models."}}'
  echo '{{"provider":{provider},"warning":"one source is stale"}}' ;;
esac"#
            ),
        );
        let file = scratch.0.join("models.json");
        let declared = r#"{"logins": {"acme": {"name": "Acme", "command": ["./login"]}}}"#;
        std::fs::write(&file, declared).unwrap();
        let providers = Providers::read(vec![file.clone()], Some(logins.clone()));
        assert_eq!(providers.problems(), [] as [String; 0]);
        let program = providers.login_programs()[0].clone();
        assert_eq!(program, self::program(&scratch, "login"));
        assert!(providers.models().is_empty());

        let mut heard = Vec::new();
        let cancel = AtomicBool::new(false);
        let warning = program.login(&logins, &cancel, |said| heard.push(said));
        assert_eq!(warning.unwrap().as_deref(), Some("one source is stale"));
        assert_eq!(
            heard,
            [
                Said::Open("https://sign-in.example/start?state=1".into()),
                Said::Say("Waiting for the browser sign-in.".into()),
                Said::Say("Reading the models.".into()),
            ]
        );
        let saved = super::super::saved(&logins.auth).unwrap();
        assert_eq!(
            saved["acme"],
            json!({"type": "oauth", "access": "first", "refresh": "r1", "expires": expires,
                "epoch": "e1"})
        );
        // Its models, offered with the login's token.
        let providers = Providers::read(vec![file.clone()], Some(logins.clone()));
        assert_eq!(providers.problems(), [] as [String; 0]);
        let model = providers.find("acme/m1").unwrap();
        let login = model.info.route.as_ref().unwrap().login.as_ref().unwrap();
        assert_eq!(login.program.as_ref(), Some(&program));
        assert!(model.info.route.as_ref().unwrap().headers.is_empty());

        // Expired, it is renewed by the program, what else the login keeps staying.
        let mut expired = saved["acme"].clone();
        expired["expires"] = json!(0);
        super::super::save(&logins.auth, "acme", expired).unwrap();
        let token = super::super::access(&logins.auth, "acme", Some(&program)).unwrap();
        assert_eq!(token, "renewed");
        let renewed = &super::super::saved(&logins.auth).unwrap()["acme"];
        assert_eq!(
            (&renewed["refresh"], &renewed["epoch"]),
            (&json!("r1"), &json!("e1"))
        );
        assert_eq!(
            program.relist(&logins).unwrap().as_deref(),
            Some("one source is stale")
        );

        let requests = std::fs::read_to_string(scratch.0.join("requests.log")).unwrap();
        let requests: Vec<Value> = (requests.lines())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let given = |at: usize| requests[at]["credential"].clone();
        let actions: Vec<&str> = (requests.iter())
            .map(|request| request["action"].as_str().unwrap())
            .collect();
        assert_eq!(actions, ["login", "models", "refresh", "models"]);
        assert_eq!(
            requests[0],
            json!({"version": 1, "action": "login", "provider": "acme"})
        );
        // Given without the type auth.json adds.
        assert_eq!(given(1)["access"], "first");
        assert_eq!(
            given(2),
            json!({"access": "first", "refresh": "r1", "expires": 0, "epoch": "e1"})
        );
        assert_eq!(given(3)["access"], "renewed");

        // Logged out, its models go.
        assert!(super::super::logout(&logins, "acme").unwrap());
        let providers = Providers::read(vec![file], Some(logins.clone()));
        assert!(providers.models().is_empty());
        assert!(!logins.listed("acme").exists());
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn what_goes_wrong_in_a_login_program_is_said_and_escape_stops_it() {
        let (scratch, logins) = scratch("login-program-failures");
        let cancel = AtomicBool::new(false);
        let run = |name: &str| {
            program(&scratch, name).run("refresh", None, Some(LIMIT), &cancel, &mut |_| {})
        };
        script(
            &scratch,
            "busy",
            r#"read -r r; echo '{"error":"the gateway is busy","transient":true}'"#,
        );
        let failed = run("busy").unwrap_err();
        assert_eq!(
            (failed.message.as_str(), failed.transient),
            ("the gateway is busy", true)
        );
        let renewal = program(&scratch, "busy")
            .refresh(&json!({"access": "a"}))
            .unwrap_err();
        assert_eq!(
            renewal.message,
            "the gateway is busy (renewing the Acme login; /login if it goes on)"
        );
        assert!(renewal.transient);
        for (name, body, said) in [
            (
                "fails",
                "echo 'cannot reach the server' >&2; exit 3",
                "./fails failed (exit status: 3): cannot reach the server",
            ),
            (
                "quiet",
                "read -r r; echo '{\"later\": 1}'",
                "./quiet ended without an answer",
            ),
            (
                "garbled",
                "echo hello",
                "./garbled: a line of its output is not JSON: expected value at line 1 column 1",
            ),
            (
                "listed",
                "echo '[1]'",
                "./listed: a line of its output is not a JSON object",
            ),
            (
                "refused",
                r#"echo '{"error":{"code":401}}'"#,
                r#"{"code":401}"#,
            ),
        ] {
            script(&scratch, name, body);
            let failed = run(name).unwrap_err();
            assert_eq!(
                (failed.message.as_str(), failed.transient),
                (said, false),
                "{name}"
            );
        }
        let failed = run("absent").unwrap_err();
        assert_eq!(
            failed.message,
            "cannot run ./absent: No such file or directory (os error 2)"
        );
        script(
            &scratch,
            "tokenless",
            r#"echo '{"credential":{"access":"","expires":1}}'"#,
        );
        let failed = program(&scratch, "tokenless").login(&logins, &cancel, |_| {});
        assert_eq!(
            failed.unwrap_err(),
            "./tokenless's credential lacks its access token or when it expires"
        );
        assert!(super::super::saved(&logins.auth).unwrap().is_empty());
        // A credential is taken even when the program goes on after it, which is stopped.
        script(
            &scratch,
            "lingers",
            r#"echo '{"credential":{"access":"a","expires":1}}'; echo $$ > lingers.pid; sleep 30"#,
        );
        let started = Instant::now();
        let answer = run("lingers").unwrap();
        assert_eq!(answer["credential"]["access"], "a");
        assert!(started.elapsed() < ENDING + Duration::from_secs(2));
        let gone = |name: &str| {
            let pid = std::fs::read_to_string(scratch.0.join(name)).unwrap();
            let process = PathBuf::from(format!("/proc/{}", pid.trim()));
            let until = Instant::now() + Duration::from_secs(2);
            while process.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            !process.exists()
        };
        assert!(gone("lingers.pid"));
        // Escape while it waits for the browser ends it and what it started.
        script(
            &scratch,
            "waits",
            r#"read -r r; sleep 30 & echo $! > waits.pid; echo '{"open":"https://x.example"}'; wait"#,
        );
        let stopping = AtomicBool::new(false);
        let started = Instant::now();
        let failed = program(&scratch, "waits").login(&logins, &stopping, |said| {
            assert_eq!(said, Said::Open("https://x.example".into()));
            stopping.store(true, Ordering::Relaxed);
        });
        assert_eq!(failed.unwrap_err(), CANCELLED);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(gone("waits.pid"));
    }
}
