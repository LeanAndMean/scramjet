//! The read-only sandbox that subagents' commands run in: bubblewrap with the whole
//! filesystem mounted read-only, a private `/tmp` discarded after each command, no network, and
//! the user's credential stores hidden; and the main agent's commands with only those hidden.

use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) const PROGRAM: &str = "bwrap";

/// Credential stores in the home directory that subagents, and the main agent when its
/// credentials are hidden, do not see, after an agent printed `~/.scramjet/agent/auth.json` to
/// its provider: common ones, not every secret.
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

/// Words that mark an environment variable as holding a credential, at the end of its name or
/// between underscores: `ANTHROPIC_API_KEY`, `HF_TOKEN`, `AWS_SECRET_ACCESS_KEY`, `PGPASSWORD`.
const CREDENTIAL_WORDS: [&str; 6] = [
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIALS",
    "KEY",
];

/// WSL2 mounts the root filesystem again here (`distro`), next to WSLg's X11 and audio sockets.
const WSLG: &str = "/mnt/wslg";
const WSLG_DISTRO: &str = "/mnt/wslg/distro";

/// The credential stores that exist, by their real paths. In the sandbox a hidden file reads
/// as empty and a hidden directory has no entries, both read-only.
#[derive(Default)]
pub(super) struct Hidden {
    files: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    /// Device and inode of each, which another mount of the same filesystem shares.
    identities: Vec<(u64, u64)>,
}

impl Hidden {
    /// Those of this user's home directory, and Scramjet's `auth.json` wherever
    /// `SCRAMJET_CODING_AGENT_DIR` puts it.
    pub(super) fn current() -> Hidden {
        let auth = super::subagent::agent_directory().map(|dir| dir.join("auth.json"));
        std::env::home_dir().map_or_else(Hidden::default, |home| Hidden::under(&home, auth))
    }

    pub(super) fn under(home: &Path, auth: Option<PathBuf>) -> Hidden {
        let existing = |paths: Vec<PathBuf>, directories: bool| {
            let mut real: Vec<PathBuf> = paths
                .into_iter()
                .filter_map(|path| std::fs::canonicalize(path).ok())
                .filter(|path| path.is_dir() == directories)
                .collect();
            real.sort();
            real.dedup();
            real
        };
        let files = HIDDEN_FILES.iter().map(|name| home.join(name)).chain(auth);
        let files = existing(files.collect(), false);
        let directories = HIDDEN_DIRECTORIES.iter().map(|name| home.join(name));
        let directories = existing(directories.collect(), true);
        let identities = files
            .iter()
            .chain(&directories)
            .filter_map(|path| identity(path));
        Hidden {
            identities: identities.collect(),
            files,
            directories,
        }
    }

    /// Refuses a path that is, or is in, a hidden store, also through a symlink or another
    /// mount of its filesystem.
    pub(super) fn check(&self, path: &Path) -> Result<(), String> {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let mut identities = real.ancestors().filter_map(identity);
        match identities.any(|found| self.identities.contains(&found)) {
            true => Err(format!(
                "{}: hidden, as it holds credentials",
                path.display()
            )),
            false => Ok(()),
        }
    }
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

/// `bash -c command` in `root`, confined.
pub(super) fn command(root: &Path, command: &str, hidden: &Hidden) -> Command {
    let mut process = confined(root, None, hidden);
    process.args(["bash", "-c"]).arg(command);
    process
}

/// `bash -c command` in `root` with everything as it is, writable and on the network, except the
/// credential stores, hidden as in the sandbox (`scripts/without-credentials.sh`). Unlike the
/// sandbox, it shares the process ids and session, so that a command's `setsid nohup … &` outlives
/// it, as outside.
pub(super) fn hiding(root: &Path, command: &str, hidden: &Hidden) -> Command {
    let mut process = Command::new(PROGRAM);
    process.args(["--dev-bind", "/", "/", "--die-with-parent"]);
    hide(&mut process, hidden);
    if Path::new(WSLG_DISTRO).is_dir() {
        process.args(["--tmpfs", WSLG_DISTRO, "--remount-ro", WSLG_DISTRO]);
    }
    process.arg("--chdir").arg(root).args(["--", "bash", "-c"]);
    process.arg(command);
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
/// the terminal; an empty `/run` and `/mnt/wslg` hide the session's sockets (D-Bus, WSL interop,
/// WSLg) through which a command could act outside, and WSL's second mount of `/`. `root` and
/// `also` are mounted again by their real paths (bubblewrap cannot mount over an absolute
/// symlink) in case they are under `/tmp` or `/run`, before what is hidden, so that it stays
/// hidden in them.
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
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        process.arg("--ro-bind").arg(&real).arg(&real);
    }
    if Path::new(WSLG).is_dir() {
        process.args(["--tmpfs", WSLG]);
    }
    hide(&mut process, hidden);
    process.arg("--chdir").arg(root).arg("--");
    process
}

/// Mounts an empty file over each hidden file and an empty directory over each hidden directory,
/// both read-only, and leaves out the environment variables that hold credentials.
fn hide(process: &mut Command, hidden: &Hidden) {
    for (name, _) in std::env::vars_os() {
        if holds_credential(&name.to_string_lossy()) {
            process.env_remove(name);
        }
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
}

fn holds_credential(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    CREDENTIAL_WORDS
        .iter()
        .any(|word| name.ends_with(word) || name.split('_').any(|part| part == *word))
}

/// An empty file for hidden files to show, in scramjet-rs's state directory; else `/dev/null`,
/// which reads as permission denied, so that a search reports an error for it.
fn empty_file() -> PathBuf {
    super::session::state_directory()
        .and_then(|directory| empty_in(&directory))
        .unwrap_or_else(|_| PathBuf::from("/dev/null"))
}

/// `directory/empty`, made again when it is anything but an empty file of its own. Commands with
/// the stores hidden can write `directory`, so it is never written through: a link there to a
/// credential store would have it emptied.
fn empty_in(directory: &Path) -> std::io::Result<PathBuf> {
    use std::io::ErrorKind;
    let path = directory.join("empty");
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_file() && m.len() == 0 && m.nlink() == 1 => return Ok(path),
        Ok(_) => std::fs::remove_file(&path)?,
        Err(error) if error.kind() == ErrorKind::NotFound => std::fs::create_dir_all(directory)?,
        Err(error) => return Err(error),
    }
    // Commands starting together may each make it.
    match std::fs::File::create_new(&path) {
        Err(error) if error.kind() != ErrorKind::AlreadyExists => Err(error),
        _ => Ok(path),
    }
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
        let output = super::command(
            &home.join("project"),
            script,
            &super::Hidden::under(&home, None),
        )
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
    fn variables_named_as_credentials_are_recognized() {
        for name in [
            "ANTHROPIC_API_KEY",
            "HF_TOKEN",
            "GH_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_ACCESS_KEY_ID",
            "PGPASSWORD",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "openai_api_key",
            "TAVILY_APIKEY",
        ] {
            assert!(super::holds_credential(name), "{name}");
        }
        for name in [
            "HOME",
            "PATH",
            "PWD",
            "TOKENIZERS_PARALLELISM",
            "GIT_AUTHOR_NAME",
            "SSH_AUTH_SOCK",
            "KEYTIMEOUT",
            "XAUTHORITY",
        ] {
            assert!(!super::holds_credential(name), "{name}");
        }
    }

    #[test]
    fn the_empty_file_is_made_again_rather_than_written_through() {
        let directory = std::env::temp_dir().join(format!("scramjet-empty-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(directory.clone());
        let store = directory.join("store");
        let empty = directory.join("empty");
        std::fs::write(&store, "SECRET\n").unwrap();
        let made_again = |planted: &dyn Fn()| {
            planted();
            assert_eq!(super::empty_in(&directory).unwrap(), empty);
            let metadata = std::fs::symlink_metadata(&empty).unwrap();
            assert!(metadata.is_file() && metadata.len() == 0);
            assert_eq!(std::fs::read_to_string(&store).unwrap(), "SECRET\n");
        };
        made_again(&|| {});
        made_again(&|| std::fs::write(&empty, "written").unwrap());
        made_again(&|| {
            std::fs::remove_file(&empty).unwrap();
            std::os::unix::fs::symlink(&store, &empty).unwrap();
        });
        made_again(&|| {
            std::fs::remove_file(&empty).unwrap();
            std::fs::hard_link(&store, &empty).unwrap();
        });
    }

    #[test]
    fn hiding_hides_only_the_credential_stores() {
        let home = std::env::temp_dir().join(format!("scramjet-hiding-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("project")).unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        std::fs::write(home.join(".ssh/id_x"), "SECRET-key\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let marker = format!("{}.{}", 200 + std::process::id() % 100, std::process::id());
        let script = format!(
            "pwd; cat ../.netrc; ls -A ../.ssh; echo x > ../.netrc; touch ../.ssh/new; \
echo made > made.txt; echo private > /tmp/scramjet-hiding-{marker}; \
(exec 3<>/dev/tcp/127.0.0.1/{port}) && echo connected; \
setsid sleep {marker} > /dev/null 2>&1 & echo done"
        );
        let output = super::hiding(
            &home.join("project"),
            &script,
            &super::Hidden::under(&home, None),
        )
        .output()
        .unwrap();
        let errors = String::from_utf8_lossy(&output.stderr);
        let lines = format!("{}\nconnected\ndone\n", home.join("project").display());
        assert_eq!(String::from_utf8_lossy(&output.stdout), lines, "{errors}");
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
        assert!(home.join("project/made.txt").exists());
        let shared = std::path::PathBuf::from(format!("/tmp/scramjet-hiding-{marker}"));
        assert_eq!(std::fs::read_to_string(&shared).unwrap(), "private\n");
        std::fs::remove_file(shared).unwrap();
        // A command started with `setsid` goes on after the command, as it does outside.
        let left = std::process::Command::new("pkill")
            .args(["-f", &format!("^sleep {marker}$")])
            .status()
            .unwrap();
        assert!(
            left.success(),
            "the background command did not outlive the command"
        );
    }

    #[test]
    fn a_cancelled_sandboxed_command_stops_with_everything_it_started() {
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
