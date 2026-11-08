//! What Unix and Windows do differently, for the rest to call the same way on both.

use std::{
    fs::{DirBuilder, Metadata, OpenOptions},
    path::{Path, PathBuf},
    process::Command,
};

/// Has `options` make a file only its owner can read (on Windows, the profile's own access).
pub fn private_file(options: &mut OpenOptions) -> &mut OpenOptions {
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(options, 0o600);
    options
}

/// Has `builder` make a directory only its owner can enter.
pub fn private_directory(builder: &mut DirBuilder) -> &mut DirBuilder {
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(builder, 0o700);
    builder
}

/// Locks `directory` against other processes until the file returned is dropped. Windows cannot
/// lock a directory's handle (ERROR_INVALID_PARAMETER), so there a `.lock` file in it is locked.
/// A lock that cannot be taken is passed over (`None` where that file cannot be made, as on a full
/// disk); only a directory that cannot be opened fails.
pub fn lock_directory(directory: &Path) -> std::io::Result<Option<std::fs::File>> {
    #[cfg(windows)]
    let lock = match private_file(OpenOptions::new().create(true).write(true).truncate(false))
        .open(directory.join(".lock"))
    {
        Ok(lock) => lock,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(error),
        Err(_) => return Ok(None),
    };
    #[cfg(not(windows))]
    let lock = std::fs::File::open(directory)?;
    let _ = lock.lock();
    Ok(Some(lock))
}

/// Makes an open file readable by its owner only.
pub fn make_private(file: &std::fs::File) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(())
    }
}

/// Cuts `file`, opened at `path` for appending, to `size` bytes. On Windows a file opened for
/// appending may not change its length (it lacks `FILE_WRITE_DATA`), so another handle cuts it.
pub fn truncate(file: &std::fs::File, path: &Path, size: u64) -> std::io::Result<()> {
    if cfg!(windows) {
        OpenOptions::new().write(true).open(path)?.set_len(size)
    } else {
        file.set_len(size)
    }
}

/// Whether the directory found is this user's own (on Windows, any).
pub fn owned(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.uid() == rustix::process::geteuid().as_raw()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

/// The owner of `path` (through links) when it is neither this user nor root, never on Windows:
/// what another user put where this one looks, as in `/tmp`, which Git refuses too.
pub fn stranger(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(path).ok()?.uid();
        #[cfg(test)]
        let me = PRETEND_USER
            .get()
            .unwrap_or_else(|| rustix::process::geteuid().as_raw());
        #[cfg(not(test))]
        let me = rustix::process::geteuid().as_raw();
        (owner != me && owner != 0).then_some(owner)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
thread_local! {
    /// The user `stranger` takes this thread's to be, so that a test can see its files as another's.
    pub static PRETEND_USER: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

/// Why `path` is passed over, as another user owns it.
pub fn left_out(path: &Path, owner: u32) -> String {
    format!(
        "{}: left out, as another user (uid {owner}) owns it",
        path.display()
    )
}

/// Whether `metadata` is of a file a program can be run from.
pub fn executable(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    metadata.is_file()
}

/// An XDG base directory: `$variable` when it is an absolute path, else `under_home` in the home
/// directory (`%USERPROFILE%` on Windows, which keeps the same layout).
pub fn base_directory(variable: &str, under_home: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| join(&home, under_home)))
}

/// `base` joined with `relative` as `Path::join` does, but one component at a time, so that a
/// `/` in it shows as `\` on Windows and an empty one adds no trailing separator.
pub fn join(base: &Path, relative: impl AsRef<Path>) -> PathBuf {
    let mut joined = base.to_path_buf();
    joined.extend(relative.as_ref().components());
    joined
}

/// The home directory with a separator after it, as it starts the paths in it.
pub fn home_prefix() -> Option<String> {
    let home = std::env::home_dir()?.display().to_string();
    let home = home.trim_end_matches(std::path::is_separator);
    (!home.is_empty()).then(|| format!("{home}{}", std::path::MAIN_SEPARATOR))
}

/// The rest of a path in the home directory written with `~` (`~`, `~/…`, and on Windows `~\…`),
/// without its leading separators.
pub fn after_tilde(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('~')?;
    let separator = |c: char| std::path::is_separator(c);
    (rest.is_empty() || rest.starts_with(separator)).then(|| rest.trim_start_matches(separator))
}

/// `path` in the home directory as `~/…` (`~\…` on Windows), else as it is.
pub fn tilde(path: &Path) -> String {
    let home = std::env::home_dir().filter(|home| home.parent().is_some());
    match home.and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
}

/// `std::fs::canonicalize`, but on Windows without the verbatim `\\?\` it gives where the plain
/// form names the same file: a verbatim path's `?` cannot be in a file name, and every path
/// shown or given to programs would carry it.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    let real = std::fs::canonicalize(path)?;
    if cfg!(windows)
        && let Some(plain) = real.to_str().and_then(plain_form)
    {
        return Ok(PathBuf::from(plain));
    }
    Ok(real)
}

/// A verbatim Windows path (`\\?\C:\…`, `\\?\UNC\server\…`) as a plain one, where that names the
/// same file: shorter than `MAX_PATH`, and no component the plain form would read otherwise
/// (`.`, `..`, a name ending in a dot or space, a device name such as `NUL` or `COM1.txt`).
fn plain_form(verbatim: &str) -> Option<String> {
    let (plain, rest) = if let Some(rest) = verbatim.strip_prefix(r"\\?\UNC\") {
        (format!(r"\\{rest}"), rest)
    } else {
        let rest = verbatim.strip_prefix(r"\\?\")?;
        let bytes = rest.as_bytes();
        let disk = bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == br":\";
        (disk.then(|| rest.to_owned())?, &rest[3..])
    };
    let device = |name: &str| {
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        let numbered = |prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
        };
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
    };
    let unclear =
        |name: &str| name == "." || name == ".." || name.ends_with(['.', ' ']) || device(name);
    let components = rest.split('\\').filter(|name| !name.is_empty());
    let clear = plain.len() < 260 && !rest.contains('/') && !components.clone().any(unclear);
    clear.then_some(plain)
}

/// The name a program's file has here: `name.exe` on Windows.
pub fn program_file(name: &str) -> String {
    match cfg!(windows) {
        true => format!("{name}.exe"),
        false => name.to_owned(),
    }
}

/// Whether two open files, or paths, are the same file however each is named.
pub fn same_file(a: &std::fs::File, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (a.metadata(), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        std::fs::File::open(b)
            .is_ok_and(|b| windows::file_id(a).is_some_and(|a| Some(a) == windows::file_id(&b)))
    }
}

/// A file's inode number, which a save that replaces the file changes (on Windows, 0: its
/// index is not in the metadata).
pub fn inode(metadata: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

/// Whether an open file was removed since it was opened.
pub fn removed(file: &std::fs::File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(file.metadata()?.nlink() == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        Ok(false)
    }
}

/// The shell that runs commands: `bash` on Unix; on Windows Git Bash, as Claude Code finds it:
/// in Git for Windows' places, then beside `git.exe` on the `PATH`; then another `bash.exe` on
/// the `PATH` (MSYS2, Cygwin), as Scramjet takes one, but WSL's, which would run it in Linux.
pub fn bash() -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return Ok(PathBuf::from("bash"));
    }
    let programs: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    let path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let windows = std::env::var_os("SystemRoot").map(PathBuf::from);
    windows_bash(&programs, &path, windows.as_deref()).ok_or_else(|| {
        "no bash to run commands: install Git for Windows (https://git-scm.com/download/win), \
         or put an MSYS2 or Cygwin bash.exe on the PATH (WSL's bash.exe is passed over: it \
         would run them in Linux)"
            .to_owned()
    })
}

fn windows_bash(programs: &[PathBuf], path: &[PathBuf], windows: Option<&Path>) -> Option<PathBuf> {
    let in_bin = |git: &Path| git.join("bin").join("bash.exe");
    let installed = programs
        .iter()
        .map(|programs| in_bin(&programs.join("Git")));
    let beside_git = path
        .iter()
        .filter(|directory| directory.join("git.exe").is_file())
        .filter_map(|directory| directory.parent().map(in_bin));
    let folded = |path: &Path| -> Vec<String> {
        let parts = path.components();
        parts
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let windows = windows.map(folded);
    let on_path = path
        .iter()
        .map(|directory| directory.join("bash.exe"))
        .filter(|bash| {
            let bash = folded(bash);
            let launcher = windows
                .as_ref()
                .is_some_and(|windows| bash.starts_with(windows))
                || bash.iter().any(|part| part == "windowsapps");
            !launcher
        });
    installed
        .chain(beside_git)
        .chain(on_path)
        .find(|bash| bash.is_file())
}

/// The program `name` runs: on Windows, a bare name is looked for in the directories of `path`
/// (else the `PATH`) with each of `PATHEXT`'s extensions, as `cmd.exe` and Codex find `npx.cmd`,
/// which `Command` misses (it looks only for `.exe`). Otherwise, or when none is there, `name`.
pub fn program(name: &str, path: Option<&std::ffi::OsStr>) -> PathBuf {
    if !cfg!(windows) || name.contains(std::path::is_separator) || name.contains(':') {
        return PathBuf::from(name);
    }
    let path = path
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var_os("PATH"));
    let directories: Vec<PathBuf> = path
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let extensions: Vec<&str> = extensions.split(';').filter(|e| !e.is_empty()).collect();
    windows_program(name, &directories, &extensions).unwrap_or_else(|| PathBuf::from(name))
}

fn windows_program(name: &str, path: &[PathBuf], extensions: &[&str]) -> Option<PathBuf> {
    let as_given = Path::new(name).extension().map(|_| name.to_owned());
    let names: Vec<String> = as_given
        .into_iter()
        .chain(
            extensions
                .iter()
                .map(|extension| format!("{name}{extension}")),
        )
        .collect();
    path.iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|program| program.is_file())
}

/// Starts `command` in a process group of its own, which `kill_group` and `end_group` reach.
pub fn own_group(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command, windows::NEW_GROUP);
    command
}

/// `own_group` for tokio's commands.
pub fn own_group_async(command: &mut tokio::process::Command) -> &mut tokio::process::Command {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(windows::NEW_GROUP);
    command
}

/// Kills the process group `own_group` started as `id`, all it started too.
pub fn kill_group(id: u32) {
    #[cfg(unix)]
    if let Some(pid) = rustix::process::Pid::from_raw(id as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    #[cfg(windows)]
    windows::kill_tree(id);
}

/// Asks the process group `own_group` started as `id` to end: SIGTERM, or Ctrl+Break.
pub fn end_group(id: u32) {
    #[cfg(unix)]
    if let Some(pid) = rustix::process::Pid::from_raw(id as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::TERM);
    }
    #[cfg(windows)]
    windows::control_break(id);
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle},
        System::{
            Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent},
            Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW},
        },
    };

    pub(super) const NEW_GROUP: u32 = CREATE_NEW_PROCESS_GROUP;

    /// The volume and index that name a file, as `st_dev` and `st_ino` do.
    pub(super) fn file_id(file: &std::fs::File) -> Option<(u32, u64)> {
        // SAFETY: the handle is open for the call, and the structure is the call's to fill.
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        let got = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) };
        (got != 0).then(|| {
            let index = (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow);
            (information.dwVolumeSerialNumber, index)
        })
    }

    /// Kills the process `id` and those it started, as Scramjet does: Windows has no group to
    /// signal, and `taskkill /T` follows the processes' parents.
    pub(super) fn kill_tree(id: u32) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &id.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    pub(super) fn control_break(id: u32) {
        // SAFETY: no pointers; a group that is gone makes it fail.
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, id) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_directory_is_locked_against_other_handles() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-rs-lock-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let held = lock_directory(&directory).unwrap().unwrap();
        #[cfg(not(windows))]
        let other = || std::fs::File::open(&directory).unwrap();
        #[cfg(windows)]
        let other = || {
            OpenOptions::new()
                .write(true)
                .open(directory.join(".lock"))
                .unwrap()
        };
        let (first, second) = (other(), other());
        let refused = matches!(first.try_lock(), Err(std::fs::TryLockError::WouldBlock));
        drop(held);
        let taken = second.try_lock().is_ok();
        drop((first, second));
        std::fs::remove_dir_all(&directory).unwrap();
        assert!(refused && taken);
    }

    #[test]
    fn windows_finds_git_bash_or_another_bash_but_never_wsls() {
        let root = std::env::temp_dir().join(format!("scramjet-rs-bash-{}", std::process::id()));
        let file = |parts: &[&str]| {
            let path = parts
                .iter()
                .fold(root.clone(), |path, part| path.join(part));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
            path
        };
        let at = |parts: &[&str]| {
            parts
                .iter()
                .fold(root.clone(), |path, part| path.join(part))
        };
        let wsl = file(&["Windows", "System32", "bash.exe"]);
        file(&["Users", "a", "WindowsApps", "bash.exe"]);
        let msys = file(&["msys64", "usr", "bin", "bash.exe"]);
        let path = [
            at(&["Windows", "System32"]),
            at(&["Users", "a", "WindowsApps"]),
            at(&["msys64", "usr", "bin"]),
            at(&["Tools", "Git", "cmd"]),
        ];
        let windows = at(&["WINDOWS"]);
        let found = |programs: &[PathBuf]| windows_bash(programs, &path, Some(&windows));
        assert_eq!(found(&[]), Some(msys.clone()));
        file(&["Tools", "Git", "cmd", "git.exe"]);
        assert_eq!(found(&[]), Some(msys));
        let beside_git = file(&["Tools", "Git", "bin", "bash.exe"]);
        assert_eq!(found(&[]), Some(beside_git));
        let installed = file(&["Program Files", "Git", "bin", "bash.exe"]);
        assert_eq!(found(&[at(&["Program Files"])]), Some(installed));
        let only_wsl = [wsl.parent().unwrap().to_owned()];
        assert_eq!(windows_bash(&[], &only_wsl, Some(&windows)), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn windows_programs_are_found_with_their_extension_in_path_order() {
        let root = std::env::temp_dir().join(format!("scramjet-rs-program-{}", std::process::id()));
        let (node, tools) = (root.join("node"), root.join("tools"));
        std::fs::create_dir_all(&node).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        let extensions = [".com", ".exe", ".bat", ".cmd"];
        let path = [node.clone(), tools.clone()];
        let found = |name: &str| windows_program(name, &path, &extensions);
        assert_eq!(found("npx"), None);
        std::fs::write(tools.join("npx.exe"), "").unwrap();
        std::fs::write(node.join("npx.cmd"), "").unwrap();
        assert_eq!(found("npx"), Some(node.join("npx.cmd")));
        std::fs::write(node.join("npx.bat"), "").unwrap();
        assert_eq!(found("npx"), Some(node.join("npx.bat")));
        assert_eq!(found("npx.exe"), Some(tools.join("npx.exe")));
        std::fs::create_dir(node.join("uvx.cmd")).unwrap();
        assert_eq!(found("uvx"), None);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(program("a/b", None), PathBuf::from("a/b"));
    }

    #[test]
    fn a_tilde_names_the_home_directory_only_before_a_separator() {
        assert_eq!(after_tilde("~"), Some(""));
        assert_eq!(after_tilde("~/a/b"), Some("a/b"));
        assert_eq!(after_tilde("~//a"), Some("a"));
        assert_eq!(after_tilde("~user/a"), None);
        assert_eq!(after_tilde("a/~/b"), None);
        assert_eq!(after_tilde("~\\a").is_some(), cfg!(windows));
    }

    #[test]
    fn verbatim_paths_are_plain_only_where_that_names_the_same_file() {
        assert_eq!(
            plain_form(r"\\?\C:\Users\a\repo").as_deref(),
            Some(r"C:\Users\a\repo")
        );
        assert_eq!(plain_form(r"\\?\C:\").as_deref(), Some(r"C:\"));
        let share = plain_form(r"\\?\UNC\server\share\x");
        assert_eq!(share.as_deref(), Some(r"\\server\share\x"));
        assert_eq!(plain_form(r"C:\Users"), None);
        assert_eq!(plain_form(r"\\?\Volume{1234}\x"), None);
        assert_eq!(plain_form(r"\\?\C:\a\NUL"), None);
        assert_eq!(plain_form(r"\\?\C:\a\com1.txt"), None);
        assert_eq!(plain_form(r"\\?\C:\a\trailing."), None);
        assert_eq!(plain_form(r"\\?\C:\a\space "), None);
        assert_eq!(plain_form(r"\\?\C:\a\..\b"), None);
        assert!(plain_form(r"\\?\C:\a\console").is_some());
        let long = format!(r"\\?\C:\{}", "a\\".repeat(130));
        assert_eq!(plain_form(&long), None);
    }

    #[test]
    fn paths_in_the_home_directory_are_shown_with_a_tilde() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(tilde(&home), "~");
        let shown = Path::new("~").join("repos").join("x");
        assert_eq!(
            tilde(&home.join("repos").join("x")),
            shown.display().to_string()
        );
        assert_eq!(tilde(Path::new("/elsewhere/x")), "/elsewhere/x");
        let beside = format!("{}x", home.display());
        assert_eq!(tilde(Path::new(&beside)), beside);
    }
}
