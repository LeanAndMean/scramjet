//! The read-only sandbox that subagents' commands run in: bubblewrap with the whole
//! filesystem mounted read-only, a private `/tmp` discarded after each command, no network, and
//! the user's credential stores hidden.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) const PROGRAM: &str = "bwrap";

/// Credential stores in the home directory that subagents do not see, after an agent printed
/// `~/.scramjet/agent/auth.json` to its provider: common ones, not every secret.
const HIDDEN_FILES: [&str; 11] = [
    ".scramjet/agent/auth.json",
    ".pi/agent/auth.json",
    ".netrc",
    ".git-credentials",
    ".claude/.credentials.json",
    ".claude.json",
    ".codex/auth.json",
    ".npmrc",
    ".pypirc",
    ".docker/config.json",
    ".vault-token",
];
const HIDDEN_DIRECTORIES: [&str; 11] = [
    ".aws",
    ".ssh",
    ".gnupg",
    ".config/gh",
    ".config/proxy",
    ".local/share/proxy",
    ".kube",
    ".config/gcloud",
    ".azure",
    ".local/share/keyrings",
    ".password-store",
];

/// The credential stores that exist, by their real paths. In the sandbox a hidden file reads
/// as empty and a hidden directory has no entries, both read-only.
#[derive(Default)]
pub(super) struct Hidden {
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
}

impl Hidden {
    /// Those of this user's home directory.
    pub(super) fn current() -> Hidden {
        std::env::home_dir().map_or_else(Hidden::default, |home| Hidden::under(&home))
    }

    pub(super) fn under(home: &Path) -> Hidden {
        let existing = |names: &[&str], directories: bool| {
            names
                .iter()
                .filter_map(|name| std::fs::canonicalize(home.join(name)).ok())
                .filter(|path| path.is_dir() == directories)
                .collect()
        };
        Hidden {
            files: existing(&HIDDEN_FILES, false),
            directories: existing(&HIDDEN_DIRECTORIES, true),
        }
    }

    /// Refuses a path that is, or is in, a hidden store, also through a symlink.
    pub(super) fn check(&self, path: &Path) -> Result<(), String> {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        match self.files.contains(&real) || self.directories.iter().any(|d| real.starts_with(d)) {
            true => Err(format!(
                "{}: hidden from subagents, as it holds credentials",
                path.display()
            )),
            false => Ok(()),
        }
    }
}

/// `bash -c command` in `root`, confined.
pub(super) fn command(root: &Path, command: &str, hidden: &Hidden) -> Command {
    let mut process = confined(root, None, hidden);
    process.args(["bash", "-c"]).arg(command);
    process
}

/// `program` (its arguments to follow) confined as `command` is, with `path`, which it reads,
/// mounted again too.
pub(super) fn program(root: &Path, path: &Path, program: &Path, hidden: &Hidden) -> Command {
    let mut process = confined(root, Some(path), hidden);
    process.arg(program);
    process
}

/// Bubblewrap up to the program to run. Its own namespaces keep the program off the network
/// and let a cancellation kill everything it started; a new session keeps it from typing into
/// the terminal; an empty `/run` hides the session's sockets (D-Bus, WSL interop) through which
/// a command could act outside. `root` and `also` are mounted again in case they are under
/// `/tmp` or `/run`, before the hidden stores, so that those stay hidden in them.
fn confined(root: &Path, also: Option<&Path>, hidden: &Hidden) -> Command {
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
    for path in std::iter::once(root).chain(also) {
        process.arg("--ro-bind").arg(path).arg(path);
    }
    if !hidden.files.is_empty() {
        let empty = empty_file();
        for file in &hidden.files {
            process.arg("--ro-bind").arg(&empty).arg(file);
        }
    }
    for directory in &hidden.directories {
        process.arg("--tmpfs").arg(directory);
        process.arg("--remount-ro").arg(directory);
    }
    process.arg("--chdir").arg(root).arg("--");
    process
}

/// An empty file for hidden files to show, in scramjet-rs's state directory (emptied again if
/// something wrote to it); else `/dev/null`, which reads as permission denied, so that a search
/// reports an error for it.
fn empty_file() -> PathBuf {
    let made = || -> std::io::Result<PathBuf> {
        let path = super::session::state_directory()?.join("empty");
        if !std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == 0) {
            std::fs::create_dir_all(super::session::state_directory()?)?;
            std::fs::write(&path, "")?;
        }
        Ok(path)
    };
    made().unwrap_or_else(|_| PathBuf::from("/dev/null"))
}

/// Whether commands can run in the sandbox here; otherwise why not.
pub(super) fn available() -> Result<(), String> {
    let output = command(Path::new("/"), "true", &Hidden::current())
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
        let ran = run_sandboxed(
            &root,
            &script,
            &super::Hidden::current(),
            None,
            &cancel,
            &mut |_| {},
        )
        .unwrap();
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
    fn credential_stores_are_hidden_and_left_unchanged() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let home = std::env::temp_dir().join(format!("scramjet-hidden-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("project")).unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        std::fs::write(home.join(".ssh/id_x"), "SECRET-key\n").unwrap();
        std::fs::write(home.join("project/a.txt"), "plain\n").unwrap();
        std::os::unix::fs::symlink(home.join(".netrc"), home.join("project/link")).unwrap();
        let script = "cat ../.netrc link a.txt; ls -A ../.ssh; cat ../.ssh/id_x; grep -r SECRET ..; \
echo x > ../.netrc; touch ../.ssh/new; echo done";
        let output = super::command(&home.join("project"), script, &super::Hidden::under(&home))
            .output()
            .unwrap();
        let errors = String::from_utf8_lossy(&output.stderr);
        assert_eq!(String::from_utf8_lossy(&output.stdout), "plain\ndone\n");
        assert!(!errors.contains("SECRET"), "{errors}");
        assert!(errors.contains("../.ssh/id_x: No such file"), "{errors}");
        assert_eq!(
            errors.matches("Read-only file system").count(),
            2,
            "{errors}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".netrc")).unwrap(),
            "SECRET-netrc\n"
        );
        assert_eq!(std::fs::read_dir(home.join(".ssh")).unwrap().count(), 1);
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
                &super::Hidden::current(),
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
