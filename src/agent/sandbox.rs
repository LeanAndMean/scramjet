//! The read-only sandbox that subagents' commands run in: bubblewrap with the whole
//! filesystem mounted read-only, a private `/tmp` discarded after each command, no network, and
//! the user's credential stores hidden; the main agent's commands in it too under `--sandbox`,
//! with the project writable in `workspace-write` and the network as `sandboxNetwork` says; and
//! otherwise the main agent's commands with only the credential stores hidden.

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

/// What stays read-only inside a writable project, as in Codex's `workspace-write`
/// (`default_read_only_subpaths_for_writable_root`): Git's history and hooks, and the agents'
/// own settings and commands, through which a command would act beyond the sandbox's next turn.
const PROTECTED: [&str; 3] = [".git", ".scramjet", ".agents"];

/// How the main agent's tools are confined (`--sandbox`, settings `sandbox` and
/// `sandboxNetwork`), with Codex's names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxMode {
    /// Every file read-only to bash, and `write` and `edit` refused.
    ReadOnly,
    /// The same, with the project writable but `PROTECTED`.
    WorkspaceWrite,
}

impl SandboxMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }

    /// A mode by its name; `Some(None)` for none (Codex's `danger-full-access`, or `off`).
    pub fn parse(name: &str) -> Option<Option<Self>> {
        match name {
            "read-only" => Some(Some(Self::ReadOnly)),
            "workspace-write" => Some(Some(Self::WorkspaceWrite)),
            "danger-full-access" | "off" => Some(None),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sandbox {
    pub mode: SandboxMode,
    pub network: bool,
}

impl Sandbox {
    /// Whether bubblewrap can confine commands here; otherwise why not.
    pub fn check() -> Result<(), String> {
        available().map_err(|error| format!("the sandbox cannot start: {error}"))
    }

    /// Refuses a `write` or `edit` of `path` that the sandbox does not allow: any in
    /// `read-only`; outside `root`, or in what stays protected in it, in `workspace-write`. A
    /// path is taken by its real place, through the symlinks of the part that exists, which it
    /// returns with the real `root`.
    pub(super) fn check_write(
        self,
        root: &Path,
        path: &Path,
    ) -> Result<(PathBuf, PathBuf), String> {
        let refused = |why: &str| {
            Err(format!(
                "{}: {why} (--sandbox {})",
                path.display(),
                self.mode.name()
            ))
        };
        if self.mode == SandboxMode::ReadOnly {
            return refused("the sandbox is read-only");
        }
        let Some(real) = real_path(path) else {
            return refused("a symlink there points nowhere the sandbox can check");
        };
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
        if !real.starts_with(&root) {
            return refused("outside the project, which the sandbox leaves read-only");
        }
        match protected(&root).iter().any(|kept| real.starts_with(kept)) {
            true => refused("kept read-only in the sandbox"),
            false => Ok((real, root)),
        }
    }

    /// Writes `content` to `path` where `check_write` allows it, making the directories missing.
    /// Each directory below the project's is opened from the one above it without following a
    /// symlink, and so is the file, so that a symlink swapped in after the check (by a bash call
    /// running alongside) fails the write rather than leading it out of what was checked.
    pub(super) fn write(self, root: &Path, path: &Path, content: &[u8]) -> Result<(), String> {
        let (real, root) = self.check_write(root, path)?;
        write_beneath(&root, &real, content).map_err(|error| format!("{}: {error}", path.display()))
    }
}

/// Writes `content` to `real`, a path in the real directory `root`, opening each directory
/// below `root` from the one above it and the file without following a symlink.
fn write_beneath(root: &Path, real: &Path, content: &[u8]) -> Result<(), String> {
    use rustix::{
        fs::{Mode, OFlags},
        io::Errno,
    };
    use std::{io::Write, path::Component};
    let failed = |error: Errno| match error {
        Errno::LOOP | Errno::NOTDIR => {
            "a symlink or file appeared on the way after the sandbox checked it".to_owned()
        }
        error => std::io::Error::from(error).to_string(),
    };
    let names: Vec<_> = real
        .strip_prefix(root)
        .unwrap_or(real)
        .components()
        .collect();
    let Some((Component::Normal(file), parents)) = names.split_last() else {
        return Err("not a file".to_owned());
    };
    let below = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(failed)?;
    for name in parents {
        let Component::Normal(name) = name else {
            return Err("not a plain path".to_owned());
        };
        match rustix::fs::mkdirat(&directory, *name, Mode::from_raw_mode(0o777)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(error) => return Err(failed(error)),
        }
        directory = rustix::fs::openat(&directory, *name, below, Mode::empty()).map_err(failed)?;
    }
    // Non-blocking, so that a FIFO put there fails the write rather than hanging it.
    let flags = OFlags::WRONLY
        | OFlags::CREATE
        | OFlags::TRUNC
        | OFlags::NOFOLLOW
        | OFlags::NONBLOCK
        | OFlags::CLOEXEC;
    let opened =
        rustix::fs::openat(&directory, *file, flags, Mode::from_raw_mode(0o666)).map_err(failed)?;
    std::fs::File::from(opened)
        .write_all(content)
        .map_err(|error| error.to_string())
}

/// `path` with its longest existing part resolved, the rest as given; `None` when a part exists
/// but does not resolve, as a dangling symlink, which a write would follow out of the check.
fn real_path(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            return Some(rest.iter().rev().fold(real, |path, name| path.join(name)));
        }
        if std::fs::symlink_metadata(existing).is_ok() {
            return None;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return Some(path.to_owned()),
        }
    }
}

/// What exists of `PROTECTED` in the real directory `root`, and the Git directory a `.git` file
/// points to (a worktree's or a submodule's).
fn protected(root: &Path) -> Vec<PathBuf> {
    let mut kept: Vec<PathBuf> = PROTECTED
        .iter()
        .map(|name| root.join(name))
        .filter(|path| path.exists())
        .collect();
    let pointer = std::fs::read_to_string(root.join(".git")).ok();
    if let Some(gitdir) = pointer
        .as_deref()
        .and_then(|text| text.trim().strip_prefix("gitdir:"))
    {
        kept.extend(std::fs::canonicalize(root.join(gitdir.trim())).ok());
    }
    kept
}

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
    /// `SCRAMJET_CODING_AGENT_DIR` puts it, scramjet-rs's own (`login::auth_file`), the files and
    /// directories `named` (the providers' `credentialFiles`), and this user's tmux servers.
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
        let own = super::login::auth_file();
        if let Some(own) = &own {
            super::login::make(own);
        }
        let also = (auth.into_iter().chain(own))
            .chain(named.iter().cloned())
            .collect();
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
    let mut process = confined(root, None, hidden, None);
    process.args(["bash", "-c"]).arg(command);
    process
}

/// `bash -c command` in `root`, confined as `sandbox` says.
pub(super) fn sandboxed(root: &Path, command: &str, hidden: &Hidden, sandbox: Sandbox) -> Command {
    let mut process = confined(root, None, hidden, Some(sandbox));
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
    let mut process = confined(root, Some(path), hidden, None);
    process.arg(program);
    process
}

/// Bubblewrap up to the program to run. Its own namespaces keep the program off the network
/// and let a cancellation kill everything it started; a new session keeps it from typing into
/// the terminal; an empty `/run` and `/mnt/wslg` hide the session's sockets (D-Bus, WSL interop,
/// WSLg) through which a command could act outside, and WSL's second mount of `/`. `root` and
/// `also` are mounted again by their real paths (bubblewrap cannot mount over an absolute
/// symlink) in case they are under `/tmp` or `/run`, before what is hidden, so that it stays
/// hidden in them. A `sandbox` makes `root` writable in `workspace-write`, but what stays
/// protected in it, and shares the network as it says, with the name servers that
/// `/etc/resolv.conf` leads to under `/run` or `/tmp` (systemd-resolved's).
fn confined(
    root: &Path,
    also: Option<&Path>,
    hidden: &Hidden,
    sandbox: Option<Sandbox>,
) -> Command {
    let writable = sandbox.is_some_and(|sandbox| sandbox.mode == SandboxMode::WorkspaceWrite);
    let network = sandbox.is_some_and(|sandbox| sandbox.network);
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
    if network {
        process.arg("--share-net");
        let resolver = std::fs::canonicalize("/etc/resolv.conf").ok();
        if let Some(real) =
            resolver.filter(|real| real.starts_with("/run") || real.starts_with("/tmp"))
        {
            process.arg("--ro-bind").arg(&real).arg(&real);
        }
    }
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    process.arg(if writable { "--bind" } else { "--ro-bind" });
    process.arg(&real_root).arg(&real_root);
    if let Some(path) = also {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        process.arg("--ro-bind").arg(&real).arg(&real);
    }
    if Path::new(WSLG).is_dir() {
        process.args(["--tmpfs", WSLG]);
    }
    hide(&mut process, hidden);
    if writable {
        for kept in protected(&real_root) {
            process.arg("--ro-bind").arg(&kept).arg(&kept);
        }
    }
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
    fn the_main_agents_sandbox_writes_only_the_project_but_its_history() {
        use super::{Sandbox, SandboxMode};
        let home = std::env::temp_dir().join(format!("scramjet-workspace-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        for directory in [".git/hooks", ".scramjet", "src"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let windows = std::path::Path::new("/mnt/c/Users/Public");
        let probe = windows.join(format!("scramjet-sandbox-{}", std::process::id()));
        let script = format!(
            "echo made > src/made.txt && echo wrote; rm -rf .git && echo removed; \
rm .git/HEAD; touch .git/hooks/pre-commit .scramjet/autonomy.yaml ../outside {probe}; \
cat ../.netrc; echo private > /tmp/x && cat /tmp/x; \
(exec 3<>/dev/tcp/127.0.0.1/{port}) 2>/dev/null && echo connected || echo no network; \
ls /run/WSL 2>/dev/null | wc -l; cmd.exe /c echo windows 2>/dev/null; true",
            probe = probe.display()
        );
        let hidden = super::Hidden::under(&home, Vec::new());
        for (mode, network, wrote, net) in [
            (SandboxMode::WorkspaceWrite, true, "wrote\n", "connected"),
            (SandboxMode::ReadOnly, false, "", "no network"),
        ] {
            let sandbox = Sandbox { mode, network };
            let output = super::sandboxed(&root, &script, &hidden, sandbox)
                .output()
                .unwrap();
            let _ = std::fs::remove_file(&probe);
            let errors = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!("{wrote}private\n{net}\n0\n"),
                "{mode:?}: {errors}"
            );
            assert!(!errors.contains("SECRET"), "{errors}");
            assert_eq!(
                std::fs::read_to_string(root.join(".git/HEAD")).unwrap(),
                "ref: refs/heads/main\n"
            );
            for left in [
                ".git/hooks/pre-commit",
                ".scramjet/autonomy.yaml",
                "../outside",
            ] {
                assert!(!root.join(left).exists(), "{mode:?}: {left}");
            }
        }
        assert!(root.join("src/made.txt").exists());
    }

    #[test]
    fn the_main_agents_sandbox_refuses_writes_it_does_not_allow() {
        use super::{Sandbox, SandboxMode};
        let home = std::env::temp_dir().join(format!("scramjet-writes-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(home.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere"), root.join("out")).unwrap();
        std::os::unix::fs::symlink(root.join(".git/hooks/pre-commit"), root.join("hook")).unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere/new"), root.join("away")).unwrap();
        std::os::unix::fs::symlink(root.join("loop"), root.join("loop")).unwrap();
        let write = Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: true,
        };
        let read = Sandbox {
            mode: SandboxMode::ReadOnly,
            ..write
        };
        assert!(write.check_write(&root, &root.join("src/new/a.rs")).is_ok());
        let refused = |sandbox: Sandbox, path: &str| {
            sandbox
                .check_write(&root, &root.join(path))
                .expect_err(path)
        };
        assert!(refused(read, "a.rs").ends_with("the sandbox is read-only (--sandbox read-only)"));
        for path in ["../a.rs", "out/a.rs", "out/new/a.rs"] {
            assert!(
                refused(write, path).contains("outside the project"),
                "{path}"
            );
        }
        assert!(refused(write, ".git/config").contains("kept read-only"));
        for path in ["hook", "away", "away/a.rs", "loop"] {
            assert!(refused(write, path).contains("points nowhere"), "{path}");
        }
    }

    #[test]
    fn a_sandboxed_write_does_not_follow_a_symlink_swapped_in_after_the_check() {
        use super::{Sandbox, SandboxMode};
        let home = std::env::temp_dir().join(format!("scramjet-swap-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(root.join(".git/hooks")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(home.join("elsewhere")).unwrap();
        let write = Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: true,
        };
        write
            .write(&root, &root.join("src/new/deeper/a.rs"), b"made")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("src/new/deeper/a.rs")).unwrap(),
            "made"
        );

        let (real, real_root) = write.check_write(&root, &root.join("src/a.rs")).unwrap();
        std::fs::remove_dir_all(root.join("src")).unwrap();
        for target in [root.join(".git/hooks"), home.join("elsewhere")] {
            std::os::unix::fs::symlink(&target, root.join("src")).unwrap();
            let error = super::write_beneath(&real_root, &real, b"x").unwrap_err();
            assert!(error.contains("appeared on the way"), "{error}");
            assert!(!target.join("a.rs").exists());
            std::fs::remove_file(root.join("src")).unwrap();
        }

        std::fs::create_dir(root.join("src")).unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere/a.rs"), root.join("src/a.rs")).unwrap();
        let error = super::write_beneath(&real_root, &real, b"x").unwrap_err();
        assert!(error.contains("appeared on the way"), "{error}");
        assert!(!home.join("elsewhere/a.rs").exists());
        std::fs::remove_file(root.join("src/a.rs")).unwrap();

        rustix::fs::mknodat(
            rustix::fs::CWD,
            root.join("src/a.rs"),
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(super::write_beneath(&real_root, &real, b"x").is_err());
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
