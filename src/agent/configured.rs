//! Configured values of `models.json` and `auth.json` (`apiKey`, `headers`), as Scramjet resolves
//! them: `!command` runs the command when a request needs it and takes what it prints, kept for
//! `SCRAMJET_KEY_COMMAND_TTL_MS` (5 minutes, as Claude Code's `apiKeyHelper`) or until a 401;
//! another value names an environment variable when one is set, else is the text itself.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Scramjet's limit on a command's run.
const LIMIT: Duration = Duration::from_secs(10);
/// Claude Code's default for its `apiKeyHelper`'s value.
const KEPT_FOR: Duration = Duration::from_mins(5);

/// How long a command's value is kept; 0 runs the command for each request.
static TTL: LazyLock<Duration> = LazyLock::new(|| {
    match std::env::var("SCRAMJET_KEY_COMMAND_TTL_MS").map(|ms| ms.trim().parse::<u64>()) {
        Ok(Ok(ms)) => Duration::from_millis(ms),
        _ => KEPT_FOR,
    }
});

/// Each command's last value and when it was printed. A command's own lock makes requests made
/// together wait for one run rather than each starting its own.
type Slot = Arc<tokio::sync::Mutex<Option<(String, Instant)>>>;
static KEPT: LazyLock<Mutex<HashMap<String, Slot>>> = LazyLock::new(Mutex::default);

fn slot(command: &str) -> Slot {
    let mut kept = KEPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.entry(command.to_owned()).or_default().clone()
}

/// Drops what `commanded`'s commands printed, so the next request runs them again.
pub(super) fn forget(commanded: &[Commanded]) {
    for Commanded { command, .. } in commanded {
        drop_value(command);
    }
}

/// Unless a request is running the command now, which gives a new value anyway.
fn drop_value(command: &str) {
    if let Ok(mut value) = slot(command).try_lock() {
        *value = None;
    }
}

pub(super) enum Configured {
    Text(String),
    /// The command after the `!`.
    Command(String),
}

pub(super) fn read(value: &str) -> Configured {
    if let Some(command) = value.strip_prefix('!') {
        return Configured::Command(command.to_owned());
    }
    match std::env::var(value) {
        Ok(set) if !set.is_empty() => Configured::Text(set),
        _ => Configured::Text(value.to_owned()),
    }
}

/// A header whose value is `prefix` and what `command` prints.
#[derive(Clone, PartialEq, Hash)]
pub(super) struct Commanded {
    pub header: String,
    pub prefix: &'static str,
    pub command: String,
    /// The value's origin in problems: `apiKey`, `the header x-name`.
    pub what: String,
}

/// The headers a request sends.
pub(super) struct Given {
    pub headers: Vec<(String, String)>,
    /// Some value is one a command printed for an earlier request.
    pub kept: bool,
}

/// `given` and the headers `commanded` gives, each command run unless its value is kept, once
/// however many headers it gives. Problems name the command, never what it printed.
pub(super) async fn headers(
    given: &[(String, String)],
    commanded: &[Commanded],
) -> Result<Given, String> {
    headers_kept_for(given, commanded, *TTL).await
}

async fn headers_kept_for(
    given: &[(String, String)],
    commanded: &[Commanded],
    ttl: Duration,
) -> Result<Given, String> {
    let mut headers = given.to_vec();
    let mut kept = false;
    let mut printed: Vec<(&str, String)> = Vec::new();
    for Commanded {
        header,
        prefix,
        command,
        what,
    } in commanded
    {
        let value = match printed.iter().find(|(run, _)| run == command) {
            Some((_, value)) => value.clone(),
            None => {
                let slot = slot(command);
                let mut last = slot.lock().await;
                let value = match &*last {
                    Some((value, at)) if at.elapsed() < ttl => {
                        kept = true;
                        value.clone()
                    }
                    _ => {
                        *last = None;
                        let value = run(command).await.map_err(|problem| {
                            format!("{what}: its command `{command}` {problem}")
                        })?;
                        if !ttl.is_zero() {
                            *last = Some((value.clone(), Instant::now()));
                        }
                        value
                    }
                };
                printed.push((command, value.clone()));
                value
            }
        };
        let value = format!("{prefix}{value}");
        if hyper::header::HeaderValue::from_str(&value).is_err() {
            drop_value(command);
            return Err(format!(
                "{what}: its command `{command}` printed a value not valid in HTTP"
            ));
        }
        headers.push((header.clone(), value));
    }
    Ok(Given { headers, kept })
}

/// What `command` prints, trimmed, run as Scramjet runs it: by `sh` (on Windows Git Bash, else
/// PowerShell, else `cmd.exe`), with no input, its errors not shown, stopped after `LIMIT`.
async fn run(command: &str) -> Result<String, String> {
    let mut shell = shell(command);
    shell
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::os::own_group_async(&mut shell);
    let mut child = shell
        .spawn()
        .map_err(|error| format!("could not start: {error}"))?;
    // Ended with all it started when the request is cancelled or the limit passes. Declared after
    // `child` so it drops first: Windows finds the tree only while the shell is alive.
    let mut group = Group(child.id());
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut printed = Vec::new();
    let finished = tokio::time::timeout(LIMIT, async {
        let (read, status) = tokio::join!(
            tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut printed),
            child.wait()
        );
        read.and(status)
    })
    .await;
    let status = match finished {
        Err(_) => return Err(format!("did not finish within {} seconds", LIMIT.as_secs())),
        Ok(status) => status.map_err(|error| format!("failed: {error}"))?,
    };
    group.0 = None;
    if !status.success() {
        return Err(match status.code() {
            Some(code) => format!("exited with {code}"),
            None => "was killed".to_owned(),
        });
    }
    let printed = String::from_utf8(printed).map_err(|_| "printed no text".to_owned())?;
    match printed.trim() {
        "" => Err("printed nothing".to_owned()),
        printed => Ok(printed.to_owned()),
    }
}

struct Group(Option<u32>);

impl Drop for Group {
    fn drop(&mut self) {
        if let Some(id) = self.0 {
            crate::os::kill_group(id);
        }
    }
}

#[cfg(not(windows))]
fn shell(command: &str) -> tokio::process::Command {
    let mut shell = tokio::process::Command::new("sh");
    shell.arg("-c").arg(command);
    shell
}

#[cfg(windows)]
fn shell(command: &str) -> tokio::process::Command {
    match crate::os::bash() {
        Ok(bash) => {
            let mut shell = tokio::process::Command::new(bash);
            shell.arg("-c").arg(command);
            shell
        }
        Err(_) if let Some(powershell) = crate::os::powershell() => {
            let mut shell = tokio::process::Command::new(powershell);
            (shell.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]))
                .arg(super::tools::POWERSHELL_BOOTSTRAP)
                .env(super::tools::COMMAND_VARIABLE, command);
            shell
        }
        Err(_) => {
            let mut shell = tokio::process::Command::new("cmd.exe");
            shell.raw_arg(format!("/d /s /c \"{command}\""));
            shell
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commanded(header: &str, prefix: &'static str, command: &str) -> Commanded {
        Commanded {
            header: header.to_owned(),
            prefix,
            command: command.to_owned(),
            what: "apiKey".to_owned(),
        }
    }

    fn headers(commanded: &[Commanded]) -> Result<Vec<(String, String)>, String> {
        kept_for(commanded, Duration::ZERO).map(|given| given.headers)
    }

    fn kept_for(commanded: &[Commanded], ttl: Duration) -> Result<Given, String> {
        let given = [("x-a".to_owned(), "a".to_owned())];
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(headers_kept_for(&given, commanded, ttl))
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` commands")]
    fn a_command_runs_once_for_each_request_unkept_and_its_output_is_the_value() {
        let path = std::env::temp_dir().join(format!("scramjet-configured-{}", std::process::id()));
        let scratch = crate::agent::tests::ScratchDir::new(path);
        let count = scratch.0.join("count");
        let command = format!("echo run >> '{}'; printf '  sk-1\\n\\n'", count.display());
        let key = [
            commanded("x-api-key", "", &command),
            commanded("authorization", "Bearer ", &command),
        ];
        let sent = headers(&key).unwrap();
        assert_eq!(
            sent,
            [
                ("x-a".to_owned(), "a".to_owned()),
                ("x-api-key".to_owned(), "sk-1".to_owned()),
                ("authorization".to_owned(), "Bearer sk-1".to_owned()),
            ]
        );
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "run\n");
        headers(&key).unwrap();
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "run\nrun\n");
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` commands")]
    fn a_kept_value_serves_later_requests_until_its_time_passes_or_it_is_forgotten() {
        let path = std::env::temp_dir().join(format!("scramjet-kept-{}", std::process::id()));
        let scratch = crate::agent::tests::ScratchDir::new(path);
        let count = scratch.0.join("count");
        let command = format!("echo run >> '{0}'; wc -l < '{0}'", count.display());
        let key = [commanded("x-api-key", "", &command)];
        let runs = || std::fs::read_to_string(&count).unwrap().lines().count();
        let sent = |ttl| {
            let given = kept_for(&key, ttl).unwrap();
            (given.headers[1].1.clone(), given.kept)
        };
        let long = Duration::from_secs(600);
        assert_eq!(sent(long), ("1".to_owned(), false));
        assert_eq!(sent(long), ("1".to_owned(), true));
        assert_eq!(runs(), 1);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(sent(Duration::from_millis(10)), ("2".to_owned(), false));
        forget(&key);
        assert_eq!(sent(long), ("3".to_owned(), false));
        // Requests made together wait for one run.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        forget(&key);
        let together = runtime.block_on(async {
            let one = || headers_kept_for(&[], &key, long);
            let (a, b, c) = tokio::join!(one(), one(), one());
            [a, b, c].map(|given| given.unwrap().headers[0].1.clone())
        });
        assert_eq!(together, ["4", "4", "4"]);
        assert_eq!(runs(), 4);
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` commands")]
    fn a_command_that_fails_says_how_and_never_what_it_printed() {
        let failed = |command: &str| headers(&[commanded("x-k", "", command)]).unwrap_err();
        assert_eq!(
            failed("echo secret; exit 3"),
            "apiKey: its command `echo secret; exit 3` exited with 3"
        );
        assert_eq!(
            failed("echo secret >&2"),
            "apiKey: its command `echo secret >&2` printed nothing"
        );
        assert_eq!(
            failed("printf 'sec\\nret'"),
            "apiKey: its command `printf 'sec\\nret'` printed a value not valid in HTTP"
        );
        assert_eq!(
            failed("printf '\\377'"),
            "apiKey: its command `printf '\\377'` printed no text"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_cancelled_command_ends_with_what_it_started() {
        let path = std::env::temp_dir().join(format!("scramjet-cancel-{}", std::process::id()));
        let scratch = crate::agent::tests::ScratchDir::new(path);
        let pid = scratch.0.join("pid");
        let command = format!("sleep 60 & echo $! > '{}'; wait", pid.display());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let running = run(&command);
            tokio::pin!(running);
            for _ in 0..200 {
                if std::fs::read_to_string(&pid).is_ok_and(|p| p.ends_with('\n')) {
                    break;
                }
                let _ = tokio::time::timeout(Duration::from_millis(10), &mut running).await;
            }
        });
        let pid: i32 = std::fs::read_to_string(&pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let gone = (0..200).any(|_| {
            let alive =
                rustix::process::test_kill_process(rustix::process::Pid::from_raw(pid).unwrap())
                    .is_ok();
            std::thread::sleep(Duration::from_millis(10));
            !alive
        });
        assert!(gone, "the command's `sleep` still runs");
    }

    #[test]
    fn values_name_a_command_a_variable_or_themselves() {
        assert!(matches!(read("!pass show k"), Configured::Command(c) if c == "pass show k"));
        assert!(matches!(read("PATH"), Configured::Text(t) if t != "PATH"));
        let unset = "SCRAMJET_RS_TEST_UNSET_VARIABLE";
        assert!(matches!(read(unset), Configured::Text(t) if t == unset));
    }
}
