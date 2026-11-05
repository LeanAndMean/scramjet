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
/// its provider: common ones, not every secret. Packages name their own (`credentialFiles`).
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
const HIDDEN_DIRECTORIES: [&str; 9] = [
    ".aws",
    ".ssh",
    ".gnupg",
    ".config/gh",
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
    /// The `TMUX_TMPDIR` of tmux servers that commands with the stores hidden start.
    tmux: Option<PathBuf>,
}

impl Hidden {
    /// Those of this user's home directory, Scramjet's `auth.json` wherever
    /// `SCRAMJET_CODING_AGENT_DIR` puts it, the files and directories `named` (the providers'
    /// `credentialFiles`), and this user's tmux servers.
    pub(super) fn current(named: &[PathBuf]) -> Hidden {
        let mut hidden = Hidden::stores(named);
        let base = std::env::var_os("TMUX_TMPDIR").filter(|base| !base.is_empty());
        let base = base.map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        let own = super::session::state_directory().map(|directory| directory.join("tmux"));
        hidden.keep_tmux_apart(&base, own.ok());
        hidden
    }

    /// `current`'s credential stores, without the tmux servers.
    fn stores(named: &[PathBuf]) -> Hidden {
        let auth = super::subagent::agent_directory().map(|dir| dir.join("auth.json"));
        let also = auth.into_iter().chain(named.iter().cloned()).collect();
        std::env::home_dir().map_or_else(Hidden::default, |home| Hidden::under(&home, also))
    }

    /// The stores that exist, files and directories, by their real paths, but those in another.
    pub(super) fn listed(named: &[PathBuf]) -> Vec<PathBuf> {
        let hidden = Hidden::stores(named);
        let (files, directories) = hidden.mounted();
        let mut listed: Vec<PathBuf> = files.chain(directories).cloned().collect();
        listed.sort();
        listed
    }

    /// The files and the directories to mount over: bubblewrap cannot mount in a hidden
    /// directory, which is read-only and hides them anyway.
    fn mounted(
        &self,
    ) -> (
        impl Iterator<Item = &PathBuf>,
        impl Iterator<Item = &PathBuf>,
    ) {
        let outside = |path: &&PathBuf| {
            let mut directories = self.directories.iter();
            !directories.any(|directory| path != &directory && path.starts_with(directory))
        };
        (
            self.files.iter().filter(outside),
            self.directories.iter().filter(outside),
        )
    }

    /// Hides the sockets of tmux servers under `base` (as `TMUX_TMPDIR`), which run what a client
    /// asks with everything visible, and has hidden commands start their own under `own`, so that
    /// sessions the user opens on a server of `base` never have the stores hidden.
    fn keep_tmux_apart(&mut self, base: &Path, own: Option<PathBuf>) {
        use std::os::unix::fs::DirBuilderExt;
        let name = format!("tmux-{}", rustix::process::getuid().as_raw());
        let sockets = base.join(&name);
        // Made as tmux makes it: bubblewrap would leave a mount point it made open to others,
        // which tmux then refuses to use.
        let _ = std::fs::DirBuilder::new().mode(0o700).create(&sockets);
        let own = own
            .filter(|own| std::fs::create_dir_all(own).is_ok())
            .and_then(|own| std::fs::canonicalize(own).ok());
        // Run from a hidden command, or in a session one started, `base` is `own`, whose servers
        // have the stores hidden.
        let theirs = own.as_ref().map(|own| own.join(&name));
        if let Some(real) = std::fs::canonicalize(&sockets)
            .ok()
            .filter(|real| real.is_dir() && Some(real) != theirs.as_ref())
        {
            self.identities.extend(identity(&real));
            self.directories.push(real);
        }
        self.tmux = own;
    }

    /// The common stores in `home`, and `also`, each a file or a directory.
    pub(super) fn under(home: &Path, also: Vec<PathBuf>) -> Hidden {
        let common = HIDDEN_FILES.iter().chain(&HIDDEN_DIRECTORIES);
        let stores = common.map(|name| home.join(name)).chain(also);
        let mut real: Vec<PathBuf> = stores
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .collect();
        real.sort();
        real.dedup();
        let (directories, files): (Vec<_>, Vec<_>) = real.into_iter().partition(|p| p.is_dir());
        let identities = files
            .iter()
            .chain(&directories)
            .filter_map(|path| identity(path));
        Hidden {
            identities: identities.collect(),
            files,
            directories,
            tmux: None,
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
    if let Some(own) = &hidden.tmux {
        process.env("TMUX_TMPDIR", own);
    }
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
    // Servers that commands with the stores hidden started run what a client asks writable and
    // on the network.
    if let Some(own) = &hidden.tmux {
        process.arg("--tmpfs").arg(own).arg("--remount-ro").arg(own);
    }
    process.arg("--chdir").arg(root).arg("--");
    process
}

/// Mounts an empty file over each hidden file and an empty directory over each hidden directory,
/// both read-only, and leaves out the environment variables that hold credentials and those
/// that lead tmux to the server the agent runs in.
fn hide(process: &mut Command, hidden: &Hidden) {
    for (name, _) in std::env::vars_os() {
        if holds_credential(&name.to_string_lossy()) {
            process.env_remove(name);
        }
    }
    process.env_remove("TMUX").env_remove("TMUX_PANE");
    let (files, directories) = hidden.mounted();
    let files: Vec<&PathBuf> = files.collect();
    if !files.is_empty() {
        let empty = empty_file();
        for file in files {
            process.arg("--ro-bind").arg(&empty).arg(file);
        }
    }
    for directory in directories {
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
    let output = command(Path::new("/"), "true", &Hidden::current(&[]))
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
            &super::Hidden::current(&[]),
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
            &super::Hidden::under(&home, Vec::new()),
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
    fn named_stores_are_hidden_also_around_and_inside_others() {
        let home = std::env::temp_dir().join(format!("scramjet-named-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        for directory in [".config/gh", ".config/fake", ".ssh", "project"] {
            std::fs::create_dir_all(home.join(directory)).unwrap();
        }
        for (file, text) in [
            (".config/gh/hosts.yml", "SECRET-gh\n"),
            (".config/fake/token", "SECRET-fake\n"),
            (".ssh/named", "SECRET-inside\n"),
            ("token.txt", "SECRET-file\n"),
            ("project/a.txt", "plain\n"),
        ] {
            std::fs::write(home.join(file), text).unwrap();
        }
        let named = [".config", ".ssh/named", "token.txt", "absent"].map(|name| home.join(name));
        let hidden = super::Hidden::under(&home, named.to_vec());
        let (files, directories) = hidden.mounted();
        let real = |name: &str| std::fs::canonicalize(home.join(name)).unwrap();
        assert_eq!(files.collect::<Vec<_>>(), [&real("token.txt")]);
        let directories: Vec<_> = directories.collect();
        assert_eq!(directories, [&real(".config"), &real(".ssh")]);
        let script = "cat a.txt; ls -A ../.config; cat ../token.txt ../.ssh/named; grep -r SECRET ..; echo done";
        let root = home.join("project");
        for mut process in [
            super::command(&root, script, &hidden),
            super::hiding(&root, script, &hidden),
        ] {
            let output = process.output().unwrap();
            let errors = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                "plain\ndone\n",
                "{errors}"
            );
            assert!(!errors.contains("SECRET"), "{errors}");
        }
        for path in [".config/fake/token", ".ssh/named", "token.txt"] {
            let refused = hidden.check(&home.join(path)).unwrap_err();
            assert!(
                refused.ends_with("hidden, as it holds credentials"),
                "{refused}"
            );
        }
        assert!(hidden.check(&root.join("a.txt")).is_ok());
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
            &super::Hidden::under(&home, Vec::new()),
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

    /// Ends the tmux servers of these `TMUX_TMPDIR`s, also when a test fails.
    struct TmuxServers(Vec<std::path::PathBuf>);

    impl TmuxServers {
        fn run(base: &std::path::Path, args: &[&str]) -> std::process::Output {
            let mut tmux = std::process::Command::new("tmux");
            tmux.env("TMUX_TMPDIR", base).env_remove("TMUX");
            tmux.args(args).output().unwrap()
        }
    }

    impl Drop for TmuxServers {
        fn drop(&mut self) {
            for base in &self.0 {
                Self::run(base, &["kill-server"]);
            }
        }
    }

    #[test]
    fn tmux_servers_outside_and_those_hidden_commands_start_stay_apart() {
        if std::process::Command::new("tmux")
            .arg("-V")
            .output()
            .is_err()
        {
            return eprintln!("tmux is not installed; skipped");
        }
        let home = std::env::temp_dir().join(format!("scramjet-tmux-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let (outside, own, fresh) = (home.join("out"), home.join("own"), home.join("fresh"));
        for directory in [&outside, &fresh, &home.join("project")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let netrc = home.join(".netrc");
        std::fs::write(&netrc, "SECRET-netrc\n").unwrap();
        let _servers = TmuxServers(vec![outside.clone(), own.clone()]);
        let run = TmuxServers::run;
        let cat = format!("cat {}", netrc.display());
        assert!(
            run(&outside, &["new-session", "-d", "-s", "out", "sleep 300"])
                .status
                .success()
        );
        // A server outside runs a client's command with everything visible.
        let shown = run(&outside, &["run-shell", &cat]);
        assert_eq!(String::from_utf8_lossy(&shown.stdout), "SECRET-netrc\n");

        let mut hidden = super::Hidden::under(&home, Vec::new());
        hidden.keep_tmux_apart(&outside, Some(own.clone()));
        let socket = outside.join(format!(
            "tmux-{}/default",
            rustix::process::getuid().as_raw()
        ));
        let script = format!(
            "tmux ls; TMUX_TMPDIR={out} tmux run-shell '{cat}'; tmux -S {socket} run-shell '{cat}'; \
TMUX={socket},1,0 tmux run-shell '{cat}'; tmux new-session -d -s own 'sleep 300' && echo started",
            out = outside.display(),
            socket = socket.display(),
        );
        let mut process = super::hiding(&home.join("project"), &script, &hidden);
        let envs: Vec<_> = process.get_envs().collect();
        assert!(envs.contains(&("TMUX".as_ref(), None)), "{envs:?}");
        assert!(envs.contains(&("TMUX_TMPDIR".as_ref(), Some(own.as_ref()))));
        let output = process.output().unwrap();
        let errors = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "started\n",
            "{errors}"
        );
        assert!(!errors.contains("SECRET"), "{errors}");

        // The server a hidden command started is apart from the one outside, and keeps the
        // stores hidden from what the user runs on it.
        let sessions =
            |base| String::from_utf8_lossy(&run(base, &["ls", "-F", "#S"]).stdout).into_owned();
        assert_eq!(sessions(&outside), "out\n");
        assert_eq!(sessions(&own), "own\n");
        let shown = run(&own, &["run-shell", &cat]);
        assert!(
            shown.status.success() && shown.stdout.is_empty(),
            "{shown:?}"
        );

        // Nor can a subagent's sandbox, read-only and off the network, have it act for it. `own`
        // as the root is mounted again, as the state directory is in the home directory.
        let reached = home.join("reached");
        let script = format!(
            "TMUX_TMPDIR={} tmux run-shell 'touch {}'",
            own.display(),
            reached.display()
        );
        let output = super::command(&own, &script, &hidden).output().unwrap();
        assert!(!reached.exists(), "{output:?}");

        // scramjet-rs run where TMUX_TMPDIR is already its own, as in a session a hidden command
        // started, still has tmux in its hidden commands.
        let mut nested = super::Hidden::under(&home, Vec::new());
        nested.keep_tmux_apart(&own, Some(own.clone()));
        let script = "tmux new-session -d -s nested 'sleep 300' && tmux ls -F '#S'";
        let output = super::hiding(&home.join("project"), script, &nested)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "nested\nown\n",
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Made as tmux makes it, so that tmux outside can use it afterwards.
        super::Hidden::default().keep_tmux_apart(&fresh, None);
        let made = std::fs::read_dir(&fresh).unwrap().next().unwrap().unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(&made.metadata().unwrap().permissions());
        assert_eq!(mode & 0o777, 0o700);
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
                &super::Hidden::current(&[]),
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
