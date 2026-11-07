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
        let in_git = real
            .strip_prefix(&root)
            .is_ok_and(|inside| inside.components().any(|name| name.as_os_str() == ".git"));
        match in_git || protected(&root).iter().any(|kept| real.starts_with(kept)) {
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

/// What stays read-only in the real directory `root`: what exists of `PROTECTED` at its top,
/// the `.git` of each repository nested in it (a submodule's, or a checkout's in a directory
/// holding several), whose hooks and `core.fsmonitor` would run at the user's next `git` there,
/// and the Git directory each `.git` file points to (a worktree's or a submodule's) with the
/// common one it names. Each by its real place, and only in `root`: what is outside is read-only
/// already, and binding it could bring back what the sandbox's empty `/run` and `/tmp` leave out.
fn protected(root: &Path) -> Vec<PathBuf> {
    let nested = nested_repositories(root);
    let mut found: Vec<PathBuf> = PROTECTED
        .iter()
        .map(|name| root.join(name))
        .chain(nested.iter().cloned())
        .collect();
    let pointed: Vec<PathBuf> = found.iter().filter_map(|path| pointed_to(path)).collect();
    let common = pointed.iter().filter_map(|gitdir| common_dir(gitdir));
    found.extend(common.collect::<Vec<_>>());
    found.extend(pointed);
    found
        .into_iter()
        .filter_map(|path| match std::fs::canonicalize(&path) {
            Ok(real) => Some(real),
            // The walk's paths are real already; one a command made unsearchable stays kept.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                nested.contains(&path).then_some(path)
            }
            Err(_) => None,
        })
        .filter(|real| real.starts_with(root))
        .collect()
}

/// The common Git directory, with the config and hooks, that a worktree's Git directory names.
fn common_dir(gitdir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    std::fs::canonicalize(gitdir.join(text.trim())).ok()
}

/// The Git directory that the `.git` file `dotgit` names.
fn pointed_to(dotgit: &Path) -> Option<PathBuf> {
    if dotgit.file_name()? != ".git" {
        return None;
    }
    let text = std::fs::read_to_string(dotgit).ok()?;
    let gitdir = text.trim().strip_prefix("gitdir:")?.trim();
    std::fs::canonicalize(dotgit.parent()?.join(gitdir)).ok()
}

/// The `.git` of each repository below the real directory `root`, at each command. A walk of a
/// large tree takes about a second (830,000 entries), so each directory's listing is kept with
/// its change time and read again only once that changed: an entry made, removed or renamed in
/// it changes it, and unlike the modification time a command cannot set it back.
fn nested_repositories(root: &Path) -> Vec<PathBuf> {
    use std::{
        collections::{BTreeMap, HashMap},
        ffi::OsString,
        os::unix::fs::MetadataExt,
        sync::{Mutex, PoisonError},
    };
    struct Listing {
        changed: (i64, i64),
        directories: Vec<OsString>,
        git: bool,
    }
    type Listings = HashMap<PathBuf, Listing>;
    static KNOWN: Mutex<BTreeMap<PathBuf, Listings>> = Mutex::new(BTreeMap::new());
    fn read(directory: &Path, changed: (i64, i64), old: Option<Listing>) -> Listing {
        let Ok(entries) = std::fs::read_dir(directory) else {
            // Unreadable (a command can `chmod 300` it): keep what it held, and its `.git`.
            let mut listing = old.unwrap_or(Listing {
                changed,
                directories: Vec::new(),
                git: false,
            });
            listing.changed = changed;
            listing.git |= std::fs::symlink_metadata(directory.join(".git")).is_ok();
            return listing;
        };
        let mut listing = Listing {
            changed,
            directories: Vec::new(),
            git: false,
        };
        for entry in entries.flatten() {
            if entry.file_name() == ".git" {
                listing.git = true;
            } else if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                listing.directories.push(entry.file_name());
            }
        }
        listing
    }
    fn walk(directory: PathBuf, old: &mut Listings, new: &mut Listings) {
        let metadata = match std::fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            // Behind a directory a command made unsearchable: keep what was known below it.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                if let Some(listing) = old.remove(&directory) {
                    for name in &listing.directories {
                        walk(directory.join(name), old, new);
                    }
                    new.insert(directory, listing);
                }
                return;
            }
            Err(_) => return,
        };
        let changed = (metadata.ctime(), metadata.ctime_nsec());
        // A change in the same clock tick as the read leaves the change time as it was, so a
        // listing changed in the last second is read again next time.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        let recent = changed.0 >= now - 1;
        let listing = match old.remove(&directory) {
            Some(listing) if listing.changed == changed => listing,
            listing => read(&directory, changed, listing),
        };
        let listing = Listing {
            changed: if recent { (i64::MIN, 0) } else { changed },
            ..listing
        };
        for name in &listing.directories {
            walk(directory.join(name), old, new);
        }
        new.insert(directory, listing);
    }
    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);
    let old = known.entry(root.to_owned()).or_default();
    let mut new = Listings::with_capacity(old.len());
    walk(root.to_owned(), old, &mut new);
    let mut found: Vec<PathBuf> = new
        .iter()
        .filter(|(directory, listing)| listing.git && *directory != root)
        .map(|(directory, _)| directory.join(".git"))
        .collect();
    found.sort();
    *old = new;
    found
}

/// The directories between the real directory `root` and each path `kept` in it, to be mounted
/// on themselves: a command can rename a directory holding a mount, then put another `.git`
/// where the protected one was, but not rename a mount point itself.
fn pinned(root: &Path, kept: &[PathBuf]) -> std::collections::BTreeSet<PathBuf> {
    kept.iter()
        .filter_map(|path| path.strip_prefix(root).ok())
        .flat_map(|inside| inside.ancestors().skip(1))
        .filter(|directory| !directory.as_os_str().is_empty())
        .map(|directory| root.join(directory))
        .collect()
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

    /// Whether `path` is in a hidden directory or holds a hidden store, which a mount there would
    /// fail or uncover.
    fn overlaps(&self, path: &Path) -> bool {
        let mut directories = self.directories.iter();
        directories.any(|directory| path.starts_with(directory))
            || self
                .files
                .iter()
                .chain(&self.directories)
                .any(|store| store.starts_with(path))
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

    /// `file`, opened at `path`, refused as `check` refuses a path, checked on the file opened: a
    /// symlink swapped in after a check by path (by a bash call running alongside) cannot lead a
    /// read or write into a store.
    pub(super) fn opened(
        &self,
        path: &Path,
        file: std::io::Result<std::fs::File>,
    ) -> Result<std::fs::File, String> {
        let failed = |error: std::io::Error| format!("{}: {error}", path.display());
        let file = file.map_err(failed)?;
        match self.holds(&file, path).map_err(failed)? {
            true => Err(format!(
                "{}: hidden, as it holds credentials",
                path.display()
            )),
            false => Ok(file),
        }
    }

    /// Whether `file` (opened at `path`) is, or is in, a store, walking up from the directory
    /// holding it by `..`, never by path: a bash call with the stores hidden cannot move their
    /// mount points, but can rename the directories above them while a path is looked up.
    fn holds(&self, file: &std::fs::File, path: &Path) -> std::io::Result<bool> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags};
        use std::os::fd::AsRawFd;
        let up = LOOKUP | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let key = |stat: &rustix::fs::Stat| (stat.st_dev, stat.st_ino);
        let stat = rustix::fs::fstat(file)?;
        if self.identities.contains(&key(&stat)) {
            return Ok(true);
        }
        let mut directory = if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            rustix::fs::openat(file, ".", up, Mode::empty())?
        } else {
            let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
                .or_else(|_| std::fs::canonicalize(path))?;
            let (Some(parent), Some(name)) = (real.parent(), real.file_name()) else {
                return Err(std::io::Error::other("not a file"));
            };
            let directory = rustix::fs::open(parent, up, Mode::empty())?;
            let named = rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
            if key(&named) != key(&stat) {
                return Err(std::io::Error::other("moved while it was checked"));
            }
            directory
        };
        loop {
            let here = key(&rustix::fs::fstat(&directory)?);
            if self.identities.contains(&here) {
                return Ok(true);
            }
            let parent = rustix::fs::openat(&directory, "..", up, Mode::empty())?;
            if key(&rustix::fs::fstat(&parent)?) == here {
                return Ok(false);
            }
            directory = parent;
        }
    }
}

/// Opens a directory to look in, needing no permission to list it where the system allows.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) const LOOKUP: rustix::fs::OFlags = rustix::fs::OFlags::PATH;
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(super) const LOOKUP: rustix::fs::OFlags = rustix::fs::OFlags::RDONLY;

fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

/// `bash -c command` in `root`, confined.
pub(super) fn command(root: &Path, command: &str, hidden: &Hidden) -> Command {
    let mut process = confined(root, None, hidden, None, None);
    process.args(["bash", "-c"]).arg(command);
    process
}

/// `bash -c command` in `root`, confined as `sandbox` says.
pub(super) fn sandboxed(root: &Path, command: &str, hidden: &Hidden, sandbox: Sandbox) -> Command {
    let mut process = confined(root, None, hidden, Some(sandbox), None);
    process.args(["bash", "-c"]).arg(command);
    process
}

/// `program` (its arguments to follow) in `root`, confined as `sandbox` says: an MCP server.
/// Its package managers' caches (uv's, npm's, and others' under `XDG_CACHE_HOME`) are moved to
/// a directory only sandboxed servers use, writable to them, as `uvx` and `npx` fail on a
/// read-only cache; nothing outside the sandbox runs what is put there.
pub(super) fn server(root: &Path, program: &str, hidden: &Hidden, sandbox: Sandbox) -> Command {
    let cache = super::session::state_directory().map(|directory| directory.join("sandbox-cache"));
    let cache = (cache.ok())
        .filter(|cache| std::fs::create_dir_all(cache).is_ok())
        .and_then(|cache| std::fs::canonicalize(cache).ok());
    let mut process = confined(root, None, hidden, Some(sandbox), cache.as_deref());
    if let Some(cache) = &cache {
        process
            .env("XDG_CACHE_HOME", cache)
            .env("UV_CACHE_DIR", cache.join("uv"))
            .env("npm_config_cache", cache.join("npm"));
    }
    process.arg(program);
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
    let mut process = confined(root, Some(path), hidden, None, None);
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
/// `/etc/resolv.conf` leads to under `/run` or `/tmp` (systemd-resolved's). `writable_too`, a real
/// path, is mounted writable last.
fn confined(
    root: &Path,
    also: Option<&Path>,
    hidden: &Hidden,
    sandbox: Option<Sandbox>,
    writable_too: Option<&Path>,
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
    // Before what is hidden, which a mount of a directory above it would uncover.
    let kept = if writable {
        let mut kept = protected(&real_root);
        kept.retain(|path| !hidden.overlaps(path));
        kept
    } else {
        Vec::new()
    };
    for directory in pinned(&real_root, &kept) {
        process.arg("--bind").arg(&directory).arg(&directory);
    }
    if let Some(path) = also {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        process.arg("--ro-bind").arg(&real).arg(&real);
    }
    if Path::new(WSLG).is_dir() {
        process.args(["--tmpfs", WSLG]);
    }
    hide(&mut process, hidden);
    for kept in &kept {
        process.arg("--ro-bind").arg(kept).arg(kept);
    }
    // Servers that commands with the stores hidden started run what a client asks writable and
    // on the network.
    if let Some(own) = &hidden.tmux {
        process.arg("--tmpfs").arg(own).arg("--remount-ro").arg(own);
    }
    if let Some(path) = writable_too {
        process.arg("--bind").arg(path).arg(path);
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
    fn the_main_agents_sandbox_keeps_nested_repositories_read_only() {
        use super::{Sandbox, SandboxMode};
        let home = std::env::temp_dir().join(format!("scramjet-nested-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        for directory in [
            ".git/modules/sub",
            "vendor/lib/.git/hooks",
            "sub",
            "inner",
            "gitdirs/inner",
            "secrets/.git",
            "bare.git/hooks",
            "bare.git/worktrees/wt",
            "wt",
            "broken",
        ] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::create_dir_all(home.join("outside")).unwrap();
        std::fs::write(home.join("outside/visible"), "OUTSIDE\n").unwrap();
        std::fs::create_dir_all(root.join("away")).unwrap();
        std::fs::write(
            root.join("away/.git"),
            format!("gitdir: {}\n", home.join("outside").display()),
        )
        .unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: ../bare.git/worktrees/wt\n").unwrap();
        std::fs::write(root.join("bare.git/worktrees/wt/commondir"), "../..\n").unwrap();
        std::os::unix::fs::symlink("/nonexistent", root.join("broken/.git")).unwrap();
        std::fs::write(root.join("sub/.git"), "gitdir: ../.git/modules/sub\n").unwrap();
        std::fs::write(root.join("inner/.git"), "gitdir: ../gitdirs/inner\n").unwrap();
        std::fs::write(root.join("secrets/key"), "SECRET\n").unwrap();
        let script = "touch vendor/lib/.git/hooks/post-checkout gitdirs/inner/config; \
echo 'gitdir: /tmp/evil' > sub/.git; mv vendor v2; mv vendor/lib vendor/lib2; mv gitdirs g2; \
mv inner inner2; cat secrets/key; cat ../outside/visible; touch bare.git/hooks/post-checkout; \
echo made > vendor/lib/made && echo wrote";
        let hidden = super::Hidden::under(&home, vec![root.join("secrets")]);
        let sandbox = Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
        };
        let output = super::sandboxed(&root, script, &hidden, sandbox)
            .output()
            .unwrap();
        let errors = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "wrote\n",
            "{errors}"
        );
        assert!(!errors.contains("SECRET"), "{errors}");
        assert_eq!(
            std::fs::read_to_string(root.join("sub/.git")).unwrap(),
            "gitdir: ../.git/modules/sub\n"
        );
        for left in [
            "vendor/lib/.git/hooks/post-checkout",
            "gitdirs/inner/config",
            "v2",
            "vendor/lib2",
            "g2",
            "inner2",
            "bare.git/hooks/post-checkout",
        ] {
            assert!(!root.join(left).exists(), "{left}");
        }
        assert!(root.join("vendor/lib/made").exists());
        for path in [
            "vendor/lib/.git/config",
            "gitdirs/inner/HEAD",
            "sub/.git",
            "new/.git/config",
        ] {
            let refused = sandbox
                .check_write(&root, &root.join(path))
                .expect_err(path);
            assert!(refused.contains("kept read-only"), "{path}: {refused}");
        }
        assert!(
            sandbox
                .check_write(&root, &root.join("vendor/lib/a.rs"))
                .is_ok()
        );
        // Repositories made after the first command, by the user or by a command that sets its
        // directory's modification time back, are kept at the next one.
        std::fs::create_dir_all(root.join("later/clone/.git/hooks")).unwrap();
        let made = super::sandboxed(
            &root,
            "stamp=$(stat -c %y vendor/lib); mkdir -p vendor/lib/deep/.git/hooks && touch -m -d \"$stamp\" vendor/lib",
            &hidden,
            sandbox,
        )
        .status()
        .unwrap();
        assert!(made.success());
        let output = super::sandboxed(
            &root,
            "touch later/clone/.git/hooks/x vendor/lib/deep/.git/hooks/x; touch later/clone/a && echo wrote",
            &hidden,
            sandbox,
        )
        .output()
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "wrote\n");
        assert!(root.join("later/clone/a").exists());
        for left in ["later/clone/.git/hooks/x", "vendor/lib/deep/.git/hooks/x"] {
            assert!(!root.join(left).exists(), "{left}");
        }
        // A directory a command makes unreadable or unsearchable hides no repository below it.
        for mode in ["300", "000"] {
            let hid = super::sandboxed(&root, &format!("chmod {mode} vendor"), &hidden, sandbox)
                .status()
                .unwrap();
            assert!(hid.success());
            let output = super::sandboxed(
                &root,
                "chmod 700 vendor; touch vendor/lib/.git/hooks/y; echo done",
                &hidden,
                sandbox,
            )
            .output()
            .unwrap();
            std::fs::set_permissions(
                root.join("vendor"),
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            )
            .unwrap();
            assert_eq!(String::from_utf8_lossy(&output.stdout), "done\n", "{mode}");
            assert!(!root.join("vendor/lib/.git/hooks/y").exists(), "{mode}");
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
    fn a_read_does_not_follow_a_symlink_swapped_in_after_the_check_into_a_store() {
        let home = std::env::temp_dir().join(format!("scramjet-read-swap-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), "SECRET-key\n").unwrap();
        std::fs::write(root.join("a.txt"), "plain\n").unwrap();
        std::fs::create_dir(root.join("keys")).unwrap();
        std::fs::write(root.join("keys/id_ed25519"), "plain\n").unwrap();
        let hidden = super::Hidden::under(&home, Vec::new());
        let read = |path: &std::path::Path| {
            let mut text = String::new();
            let file = hidden.opened(path, std::fs::File::open(path))?;
            std::io::Read::read_to_string(&mut &file, &mut text).unwrap();
            Ok::<_, String>(text)
        };
        for path in [root.join("a.txt"), root.join("keys/id_ed25519")] {
            hidden.check(&path).unwrap();
            assert_eq!(read(&path).unwrap(), "plain\n");
        }

        std::fs::remove_file(root.join("a.txt")).unwrap();
        std::os::unix::fs::symlink(home.join(".netrc"), root.join("a.txt")).unwrap();
        std::fs::remove_dir_all(root.join("keys")).unwrap();
        std::os::unix::fs::symlink(home.join(".ssh"), root.join("keys")).unwrap();
        for path in [root.join("a.txt"), root.join("keys/id_ed25519")] {
            let error = read(&path).unwrap_err();
            assert!(error.contains("hidden, as it holds credentials"), "{error}");
        }
    }

    #[test]
    fn renaming_a_directory_above_a_store_does_not_let_a_file_in_it_through() {
        let home = std::env::temp_dir().join(format!("scramjet-rename-up-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(home.join(".config/gh")).unwrap();
        std::fs::write(home.join(".config/gh/hosts.yml"), "SECRET\n").unwrap();
        let hidden = super::Hidden::under(&home, Vec::new());
        std::os::unix::fs::symlink(home.join(".config/gh/hosts.yml"), root.join("a.txt")).unwrap();
        let done = std::sync::atomic::AtomicBool::new(false);
        let mut passed = 0;
        std::thread::scope(|scope| {
            // As a bash call with the stores hidden can: their mount points stay, but not the
            // directories above them.
            scope.spawn(|| {
                let (config, moved) = (home.join(".config"), home.join(".c"));
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    std::fs::rename(&config, &moved).unwrap();
                    std::fs::rename(&moved, &config).unwrap();
                }
            });
            for _ in 0..20000 {
                let path = root.join("a.txt");
                passed += usize::from(hidden.opened(&path, std::fs::File::open(&path)).is_ok());
            }
            done.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        assert_eq!(passed, 0);
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
