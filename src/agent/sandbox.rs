//! The read-only sandbox that subagents' bash commands run in: bubblewrap with the whole
//! filesystem mounted read-only, a private `/tmp` discarded after each command, and no network.

use std::{
    path::Path,
    process::{Command, Stdio},
};

pub(super) const PROGRAM: &str = "bwrap";

/// `bash -c command` in `root`, confined. Its own namespaces keep it off the network and let a
/// cancellation kill everything it started; a new session keeps it from typing into the
/// terminal; an empty `/run` hides the session's sockets (D-Bus, WSL interop) through which a
/// command could act outside. `root` is mounted again in case it is under `/tmp` or `/run`.
pub(super) fn command(root: &Path, command: &str) -> Command {
    let mut process = Command::new(PROGRAM);
    process.args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/run",
    ]);
    process.arg("--ro-bind").arg(root).arg(root);
    process.arg("--chdir").arg(root);
    process.args(["--", "bash", "-c"]).arg(command);
    process
}

/// Whether commands can run in the sandbox here; otherwise why not.
pub(super) fn available() -> Result<(), String> {
    let output = command(Path::new("/"), "true")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot start {PROGRAM}: {error}"))?;
    match output.status.success() {
        true => Ok(()),
        false => Err(format!(
            "{PROGRAM} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tools::run_sandboxed;
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };

    #[test]
    fn sandboxed_commands_read_but_cannot_write_or_connect() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        super::available().expect("bubblewrap works here");
        let root = std::env::temp_dir().join(format!("scramjet-sandbox-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        std::fs::write(root.join("kept.txt"), "original\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let script = format!(
            "pwd; cat kept.txt; echo changed > kept.txt; truncate -s0 kept.txt; touch new.txt; \
touch ~/.scramjet-sandbox-probe; echo private > /tmp/x && cat /tmp/x; \
(exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null && echo connected || echo no network; true"
        );
        let ran = run_sandboxed(&root, &script, None, &cancel, &mut |_| {}).unwrap();
        let output = ran.tool_output();
        assert!(!output.failed, "{}", output.text);
        let lines: Vec<&str> = output.text.lines().collect();
        assert_eq!(lines[0], root.display().to_string());
        assert_eq!(lines[1], "original");
        assert!(
            output.text.contains("Read-only file system"),
            "{}",
            output.text
        );
        assert!(
            output.text.contains("private\nno network"),
            "{}",
            output.text
        );
        assert_eq!(
            std::fs::read_to_string(root.join("kept.txt")).unwrap(),
            "original\n"
        );
        assert!(!root.join("new.txt").exists());
        let home = std::env::home_dir().unwrap();
        assert!(!home.join(".scramjet-sandbox-probe").exists());
    }

    #[test]
    fn a_cancelled_sandboxed_command_stops_with_everything_it_started() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let marker = format!("{}.{}", 300 + std::process::id() % 100, std::process::id());
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                cancel.store(true, Ordering::Relaxed);
            });
            let command = format!("sleep {marker} & sleep {marker}");
            let ran = run_sandboxed(
                std::path::Path::new("/"),
                &command,
                None,
                &cancel,
                &mut |_| {},
            )
            .unwrap();
            assert!(ran.tool_output().text.contains("cancelled by the user"));
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(200));
        let left = std::process::Command::new("pgrep")
            .args(["-f", &format!("sleep {marker}")])
            .output()
            .unwrap();
        assert!(
            left.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&left.stdout)
        );
    }
}
