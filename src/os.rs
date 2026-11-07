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

/// Opens a directory to lock it, which Windows allows only with backup semantics.
pub fn open_directory(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(
        &mut options,
        windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS,
    );
    options.open(path)
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
        .or_else(|| std::env::home_dir().map(|home| home.join(under_home)))
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

/// The shell that runs commands: `bash` on Unix; on Windows Git Bash, as Scramjet finds it: in
/// Git for Windows' places, then `bash.exe` on the `PATH`.
pub fn bash() -> PathBuf {
    if cfg!(windows) {
        let installed = ["ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|programs| PathBuf::from(programs).join(r"Git\bin\bash.exe"));
        let on_path = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|directory| directory.join("bash.exe"));
        if let Some(found) = installed.chain(on_path).find(|path| path.is_file()) {
            return found;
        }
    }
    PathBuf::from("bash")
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
        assert_eq!(tilde(&home.join("repos/x")), shown.display().to_string());
        assert_eq!(tilde(Path::new("/elsewhere/x")), "/elsewhere/x");
        let beside = format!("{}x", home.display());
        assert_eq!(tilde(Path::new(&beside)), beside);
    }
}
