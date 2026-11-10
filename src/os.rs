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

/// Writes `bytes` over the contents of `file`, opened for writing at its start, once the disk
/// has given them room: a full disk leaves the contents as they were, where cutting first
/// would lose them.
pub fn overwrite(file: &mut std::fs::File, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let size = bytes.len() as u64;
    #[cfg(unix)]
    {
        use rustix::{fs::FallocateFlags, io::Errno};
        match rustix::fs::fallocate(&*file, FallocateFlags::empty(), 0, size) {
            Ok(()) | Err(Errno::OPNOTSUPP | Errno::NOSYS) => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Windows gives a file room as its end moves out.
    #[cfg(not(unix))]
    if file.metadata()?.len() < size {
        file.set_len(size)?;
    }
    file.write_all(bytes)?;
    file.set_len(size)
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

/// The PowerShell that runs the `powershell` tool's commands on Windows, as Claude Code finds it:
/// `pwsh` 7 on the `PATH`, installed or as a .NET tool, else Windows PowerShell 5.1 on the `PATH`
/// or in the system's directory. `None` elsewhere, or when there is none.
pub fn powershell() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let variable = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    windows_powershell(
        &path,
        variable("ProgramFiles").as_deref(),
        variable("USERPROFILE").as_deref(),
        variable("SystemRoot").as_deref(),
    )
}

fn windows_powershell(
    path: &[PathBuf],
    programs: Option<&Path>,
    profile: Option<&Path>,
    windows: Option<&Path>,
) -> Option<PathBuf> {
    let on_path = |name: &'static str| path.iter().map(move |directory| directory.join(name));
    let installed = programs.map(|programs| programs.join("PowerShell").join("7").join("pwsh.exe"));
    let tool = profile.map(|profile| profile.join(".dotnet").join("tools").join("pwsh.exe"));
    let system = windows.map(|windows| {
        let directory = windows.join("System32").join("WindowsPowerShell");
        directory.join("v1.0").join("powershell.exe")
    });
    on_path("pwsh.exe")
        .chain(installed)
        .chain(tool)
        .chain(on_path("powershell.exe"))
        .chain(system)
        .find(|program| program.is_file())
}

/// Whether `powershell` is PowerShell 7 (`pwsh`) rather than Windows PowerShell 5.1.
pub fn is_pwsh(powershell: &Path) -> bool {
    let stem = powershell.file_stem().unwrap_or_default();
    stem.to_string_lossy().eq_ignore_ascii_case("pwsh")
}

/// `cmd.exe` on Windows: `ComSpec`, else the system's. `None` elsewhere.
pub fn cmd() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let system = std::env::var_os("SystemRoot")
        .map(|windows| PathBuf::from(windows).join("System32").join("cmd.exe"));
    (std::env::var_os("ComSpec").map(PathBuf::from).into_iter())
        .chain(system)
        .find(|program| program.is_file())
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

/// The first argument that starts scramjet-rs as `low_integrity`'s helper.
pub const LOW_INTEGRITY: &str = "__low-integrity";

/// What the helper prints when lowered without a program to run.
pub const LOWERED: &str = "lowered";

/// On Windows, when the command line starts with `LOW_INTEGRITY`, never returns: lowers this
/// process to Windows' low integrity level, at which no file or registry key the user's programs
/// made can be changed, then runs the program the rest of the command line gives, with a `TEMP`
/// of its own, and exits with its code; without a program, prints `LOWERED`. Run before
/// anything else, as the read-only sandbox of subagents' commands there.
pub fn low_integrity() {
    #[cfg(windows)]
    {
        let arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if let Some((first, rest)) = arguments.split_first()
            && first == LOW_INTEGRITY
        {
            windows::low_integrity(rest)
        }
    }
}

/// Keeps scramjet-rs's own standard input, output and error from every program it starts, and
/// from theirs: on Windows a child inherits each inheritable handle, so a program a command left
/// running would hold `-p`'s output open after scramjet-rs exits. A child given them by
/// `Stdio::inherit` (the default) gets copies made for it.
pub fn keep_own_handles() {
    #[cfg(windows)]
    windows::keep_own_handles();
}

/// Starts `command` in a process group of its own, which `kill_group` and `end_group` reach.
pub fn own_group(command: &mut Command) -> &mut Command {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(command, windows::NEW_GROUP);
    command
}

/// As `own_group`, and on Windows with a console of its own that has no window, so that the code
/// page it sets, and what it or its programs write to the console rather than their output, stay
/// out of scramjet-rs's.
pub fn own_group_and_console(command: &mut Command) -> &mut Command {
    own_group(command);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(
        command,
        windows::NEW_GROUP | windows::NO_WINDOW,
    );
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

/// Keeps track of all that `child`, started by `own_group`, starts, for `kill_group` and
/// `group_left`, until `release_group`. On Windows it puts the child in a job, which reaches the
/// processes whose parent has ended, as Git Bash's often have; a process group does on Unix.
pub fn hold_group(child: &std::process::Child) {
    #[cfg(windows)]
    windows::hold(child);
    #[cfg(unix)]
    let _ = child;
}

/// Stops keeping track of the group `hold_group` held as `id`; what still runs in it goes on.
pub fn release_group(id: u32) {
    #[cfg(windows)]
    windows::release(id);
    #[cfg(unix)]
    let _ = id;
}

/// The processes still running in the group `hold_group` held as `id` on Windows: each one's id
/// and program file name. `None` elsewhere, or where it was not held.
pub fn group_left(id: u32) -> Option<Vec<(u32, String)>> {
    #[cfg(windows)]
    {
        windows::left(id)
    }
    #[cfg(unix)]
    {
        let _ = id;
        None
    }
}

/// Kills the process group `own_group` started as `id`, all it started too.
pub fn kill_group(id: u32) {
    #[cfg(unix)]
    if let Some(pid) = rustix::process::Pid::from_raw(id as i32) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    #[cfg(windows)]
    {
        windows::kill_tree(id);
        windows::kill_job(id);
    }
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

/// Whether a process `pid` runs, as Node's `process.kill(pid, 0)` tells it: one that only
/// refuses the signal runs.
pub fn process_runs(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let pid = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw);
        pid.is_some_and(|pid| {
            rustix::process::test_kill_process(pid) != Err(rustix::io::Errno::SRCH)
        })
    }
    #[cfg(windows)]
    {
        windows::process_runs(pid)
    }
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

    pub(super) fn keep_own_handles() {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: a handle missing or not settable fails the call, which changes nothing.
            unsafe { SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0) };
        }
    }

    pub(super) fn low_integrity(arguments: &[std::ffi::OsString]) -> ! {
        if let Err(error) = lower() {
            eprintln!("scramjet-rs: cannot lower the integrity level, so nothing runs: {error}");
            std::process::exit(125);
        }
        let Some((program, rest)) = arguments.split_first() else {
            print!("{}", super::LOWERED);
            std::process::exit(0)
        };
        let temporary = temporary();
        let mut command = std::process::Command::new(program);
        command.args(rest);
        if let Some(temporary) = &temporary {
            command.env("TEMP", temporary).env("TMP", temporary);
        }
        // Ctrl+Break is for the program, whose code this passes on once it ends.
        unsafe extern "system" fn ignore(_: u32) -> windows_sys::core::BOOL {
            1
        }
        // SAFETY: the handler is a function for the process's lifetime.
        unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(ignore), 1) };
        let code = match command.status() {
            Ok(status) => status.code().unwrap_or(1),
            Err(error) => {
                eprintln!("scramjet-rs: cannot start {}: {error}", program.display());
                127
            }
        };
        if let Some(temporary) = temporary {
            let _ = std::fs::remove_dir_all(temporary);
        }
        std::process::exit(code)
    }

    /// Sets this process's token to the low integrity level, and checks that it took.
    fn lower() -> std::io::Result<()> {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            Security::{
                CreateWellKnownSid, GetSidSubAuthority, GetSidSubAuthorityCount,
                GetTokenInformation, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES,
                SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
                TokenIntegrityLevel, WinLowLabelSid,
            },
            System::{
                SystemServices::{SE_GROUP_INTEGRITY, SECURITY_MANDATORY_LOW_RID},
                Threading::{GetCurrentProcess, OpenProcessToken},
            },
        };
        let failed = std::io::Error::last_os_error;
        let mut token = std::ptr::null_mut();
        // SAFETY: the token is checked and closed once; each buffer's size is given to the call
        // that fills it, and the label read back points into its own buffer.
        unsafe {
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_DEFAULT | TOKEN_QUERY,
                &mut token,
            ) == 0
            {
                return Err(failed());
            }
            let result = (|| {
                let mut sid = [0usize; SECURITY_MAX_SID_SIZE as usize / 8 + 1];
                let mut size = std::mem::size_of_val(&sid) as u32;
                let sid = sid.as_mut_ptr().cast();
                if CreateWellKnownSid(WinLowLabelSid, std::ptr::null_mut(), sid, &mut size) == 0 {
                    return Err(failed());
                }
                let label = TOKEN_MANDATORY_LABEL {
                    Label: SID_AND_ATTRIBUTES {
                        Sid: sid,
                        Attributes: SE_GROUP_INTEGRITY as u32,
                    },
                };
                let length = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + size;
                if SetTokenInformation(
                    token,
                    TokenIntegrityLevel,
                    (&raw const label).cast(),
                    length,
                ) == 0
                {
                    return Err(failed());
                }
                let mut read = [0usize; 16];
                let mut needed = 0;
                let room = std::mem::size_of_val(&read) as u32;
                if GetTokenInformation(
                    token,
                    TokenIntegrityLevel,
                    read.as_mut_ptr().cast(),
                    room,
                    &mut needed,
                ) == 0
                {
                    return Err(failed());
                }
                let sid = (*read.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
                let count = u32::from(*GetSidSubAuthorityCount(sid));
                match count > 0
                    && *GetSidSubAuthority(sid, count - 1) <= SECURITY_MANDATORY_LOW_RID as u32
                {
                    true => Ok(()),
                    false => Err(std::io::Error::other("the token kept a higher level")),
                }
            })();
            CloseHandle(token);
            result
        }
    }

    /// A directory of this process's own under `LocalLow`, which processes at the low level can
    /// write, after removing those of helpers no longer running.
    fn temporary() -> Option<std::path::PathBuf> {
        let local = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
        let base = local.parent()?.join("LocalLow").join("scramjet-rs-tmp");
        for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let pid = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok());
            if pid.is_some_and(|pid| !process_runs(pid)) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        let own = base.join(std::process::id().to_string());
        std::fs::create_dir_all(&own).ok().map(|()| own)
    }

    pub(super) const NEW_GROUP: u32 = CREATE_NEW_PROCESS_GROUP;
    pub(super) const NO_WINDOW: u32 = CREATE_NO_WINDOW;

    struct Job(windows_sys::Win32::Foundation::HANDLE);

    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is the job's own, closed once.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }

    /// The jobs `hold` made, by the id of the process each was made for.
    static JOBS: std::sync::Mutex<Vec<(u32, Job)>> = std::sync::Mutex::new(Vec::new());

    fn jobs() -> std::sync::MutexGuard<'static, Vec<(u32, Job)>> {
        JOBS.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Puts `child` in a job of its own. A process it started before this escapes it; a shell
    /// takes far longer to start one. A job that cannot be made leaves `kill_tree` alone.
    pub(super) fn hold(child: &std::process::Child) {
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        // SAFETY: no attributes nor name; the handle is checked, and owned by `Job` once made.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return;
        }
        let job = Job(job);
        // SAFETY: both handles are open for the call.
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } != 0 {
            jobs().push((child.id(), job));
        }
    }

    pub(super) fn release(id: u32) {
        jobs().retain(|(held, _)| *held != id);
    }

    pub(super) fn kill_job(id: u32) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        if let Some((_, job)) = jobs().iter().find(|(held, _)| *held == id) {
            // SAFETY: the handle is open while the list is locked.
            unsafe { TerminateJobObject(job.0, 1) };
        }
    }

    pub(super) fn left(id: u32) -> Option<Vec<(u32, String)>> {
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_PROCESS_ID_LIST, JobObjectBasicProcessIdList, QueryInformationJobObject,
        };
        const ROOM: usize = 256;
        #[repr(C)]
        struct List {
            head: JOBOBJECT_BASIC_PROCESS_ID_LIST,
            more: [usize; ROOM - 1],
        }
        // SAFETY: the structure is the call's to fill, with room for ROOM ids as its size says.
        let mut list: List = unsafe { std::mem::zeroed() };
        let ids: Vec<u32> = {
            let jobs = jobs();
            let (_, job) = jobs.iter().find(|(held, _)| *held == id)?;
            let got = unsafe {
                QueryInformationJobObject(
                    job.0,
                    JobObjectBasicProcessIdList,
                    (&raw mut list).cast(),
                    std::mem::size_of::<List>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if got == 0 {
                return None;
            }
            let listed = (list.head.NumberOfProcessIdsInList as usize).min(ROOM);
            // SAFETY: the list's ids run on from its first, within the structure.
            let first = (&raw const list.head.ProcessIdList).cast::<usize>();
            (0..listed)
                .map(|at| unsafe { *first.add(at) } as u32)
                .collect()
        };
        // A console's host ends once its programs have; one that serves a program still running
        // comes with that program, which is listed.
        let listed = ids.into_iter().map(|pid| (pid, image(pid)));
        Some((listed.filter(|(_, program)| !program.eq_ignore_ascii_case("conhost.exe"))).collect())
    }

    /// The file name of process `pid`'s program, or `?`.
    fn image(pid: u32) -> String {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            QueryFullProcessImageNameW,
        };
        let mut name = [0u16; 1024];
        let mut size = name.len() as u32;
        // SAFETY: the handle is checked and closed once; the buffer's size is given.
        let got = unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return "?".to_owned();
            }
            let got = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                name.as_mut_ptr(),
                &mut size,
            );
            CloseHandle(handle);
            got
        };
        if got == 0 {
            return "?".to_owned();
        }
        let path = String::from_utf16_lossy(&name[..size as usize]);
        path.rsplit('\\').next().unwrap_or(&path).to_owned()
    }

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

    /// As libuv's `uv_kill` with signal 0: a process that cannot be opened for want of rights
    /// runs, and one that has an exit code does not.
    pub(super) fn process_runs(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{
            CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE,
        };
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: the handle is checked, closed once, and the code is the call's to fill.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0;
            let got = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            got == 0 || code == STILL_ACTIVE as u32
        }
    }

    pub(super) fn control_break(id: u32) {
        // SAFETY: no pointers; a group that is gone makes it fail.
        unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, id) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_locked_directory_is_locked_against_other_handles() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-rs-lock-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let _scratch = Scratch(directory.clone());
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
        assert!(refused && taken);
    }

    #[test]
    fn windows_finds_git_bash_or_another_bash_but_never_wsls() {
        let root = std::env::temp_dir().join(format!("scramjet-rs-bash-{}", std::process::id()));
        let _scratch = Scratch(root.clone());
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
    }

    #[test]
    fn windows_prefers_powershell_7_to_windows_powershell() {
        let root = std::env::temp_dir().join(format!("scramjet-rs-pwsh-{}", std::process::id()));
        let _scratch = Scratch(root.clone());
        let at = |parts: &[&str]| (parts.iter()).fold(root.clone(), |path, part| path.join(part));
        let file = |parts: &[&str]| {
            let path = at(parts);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
            path
        };
        let (programs, profile, windows) =
            (at(&["Program Files"]), at(&["Users", "a"]), at(&["W"]));
        let path = [at(&["bin"])];
        let found = || windows_powershell(&path, Some(&programs), Some(&profile), Some(&windows));
        assert_eq!(found(), None);
        let system = file(&[
            "W",
            "System32",
            "WindowsPowerShell",
            "v1.0",
            "powershell.exe",
        ]);
        assert_eq!(found(), Some(system.clone()));
        let tool = file(&["Users", "a", ".dotnet", "tools", "pwsh.exe"]);
        assert_eq!(found(), Some(tool));
        let installed = file(&["Program Files", "PowerShell", "7", "pwsh.exe"]);
        assert_eq!(found(), Some(installed.clone()));
        let on_path = file(&["bin", "pwsh.exe"]);
        assert_eq!(found(), Some(on_path.clone()));
        assert!(is_pwsh(&on_path) && is_pwsh(Path::new("PWSH.EXE")));
        assert!(!is_pwsh(&system));
    }

    #[test]
    #[cfg(windows)]
    fn killing_a_group_ends_what_it_started_whose_parent_has_ended() {
        let pings = |left: &[(u32, String)]| {
            left.iter()
                .filter(|(_, program)| program.eq_ignore_ascii_case("ping.exe"))
                .count()
        };
        let mut command = Command::new("cmd");
        command.args([
            "/c",
            "cmd /c start /b ping -n 30 127.0.0.1 >nul & ping -n 30 127.0.0.1 >nul",
        ]);
        let mut child = own_group(&mut command).spawn().unwrap();
        hold_group(&child);
        let id = child.id();
        let started = std::time::Instant::now();
        while pings(&group_left(id).unwrap()) < 2 {
            assert!(started.elapsed().as_secs() < 10, "{:?}", group_left(id));
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        kill_group(id);
        child.wait().unwrap();
        let started = std::time::Instant::now();
        while !group_left(id).unwrap().is_empty() {
            assert!(started.elapsed().as_secs() < 10, "{:?}", group_left(id));
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        release_group(id);
        assert_eq!(group_left(id), None);
    }

    #[test]
    fn windows_programs_are_found_with_their_extension_in_path_order() {
        let root = std::env::temp_dir().join(format!("scramjet-rs-program-{}", std::process::id()));
        let _scratch = Scratch(root.clone());
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
