//! Exiting on SIGTERM, SIGHUP (on Windows, the console closing) or a second Ctrl-C, as
//! Scramjet's shutdown does: the commands running killed, and the terminal given back as the
//! shell left it.

use super::{console, screen};
use std::{
    io::{self, PipeWriter, Read, Write},
    path::PathBuf,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicI32, Ordering},
    },
};

/// The signal the process exits for, the first one that came.
static SIGNAL: AtomicI32 = AtomicI32::new(0);
/// The pipe that wakes the thread that exits.
static WAKE: OnceLock<PipeWriter> = OnceLock::new();
/// The process id of the external editor that has the terminal, or 0.
#[cfg(unix)]
static EDITOR: AtomicI32 = AtomicI32::new(0);
/// The temporary file of the prompt the editor has, removed by an exit as by the edit's end.
static PROMPT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Records the temporary file the prompt is edited in, or `None` once it is removed.
pub(super) fn prompt_file(path: Option<PathBuf>) {
    *PROMPT.lock().unwrap_or_else(PoisonError::into_inner) = path;
}

/// Records that the external editor `pid` has the terminal, so that an exit stops it first;
/// one that started as the exit began gets the signal at once.
#[cfg(unix)]
pub(super) fn editing(pid: u32) {
    let pid = pid as i32;
    EDITOR.store(pid, Ordering::SeqCst);
    let signal = SIGNAL.load(Ordering::SeqCst);
    if signal != 0 {
        // SAFETY: a plain kill(2) of the editor this process started and has not reaped.
        unsafe { libc::kill(pid, signal) };
    }
}

/// Records that the editor was reaped. During an exit the caller never returns, so that it does
/// not draw the screen again while the exit gives the terminal back.
#[cfg(unix)]
pub(super) fn edited() {
    EDITOR.store(0, Ordering::SeqCst);
    while SIGNAL.load(Ordering::SeqCst) != 0 {
        std::thread::park();
    }
}

/// Passes the exit's signal on to the external editor and waits for it to end, as it then saves
/// its state and gives the terminal back itself; one still running after 5 seconds is killed.
/// Left running, it would stop at its next read once the shell took the terminal back.
#[cfg(unix)]
fn stop_editor() {
    let gone = |seconds: u64| {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        while EDITOR.load(Ordering::SeqCst) != 0 {
            if std::time::Instant::now() > end {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        true
    };
    for (signal, seconds) in [(SIGNAL.load(Ordering::SeqCst), 5), (libc::SIGKILL, 1)] {
        let pid = EDITOR.load(Ordering::SeqCst);
        if pid == 0 {
            return;
        }
        // SAFETY: a plain kill(2); the editor is unreaped while `EDITOR` names it.
        unsafe { libc::kill(pid, signal) };
        if gone(seconds) {
            return;
        }
    }
}

/// Makes SIGTERM and SIGHUP, and `on_signal`, exit with `startup` the terminal mode the agent
/// started in.
pub(super) fn install(startup: Option<console::Mode>) -> io::Result<()> {
    let (mut reader, writer) = io::pipe()?;
    let _ = WAKE.set(writer);
    // The exit runs here, not in a handler, as it waits for other threads' writes.
    std::thread::Builder::new()
        .name("exit".into())
        .spawn(move || {
            if reader.read_exact(&mut [0]).is_ok() {
                exit(startup.as_ref());
            }
        })?;
    #[cfg(unix)]
    let signals = [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP];
    #[cfg(windows)]
    let signals = [signal_hook::consts::SIGTERM];
    for signal in signals {
        // SAFETY: `on_signal` makes only async-signal-safe calls.
        unsafe { signal_hook::low_level::register(signal, move || on_signal(signal)) }?;
    }
    #[cfg(windows)]
    windows::install()?;
    Ok(())
}

/// Has the process exit for `signal`, with the status 128 + `signal` a shell gives a process
/// killed by it. Async-signal-safe: an atomic exchange and a write.
pub(super) fn on_signal(signal: i32) {
    if SIGNAL
        .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
        && let Some(mut wake) = WAKE.get()
    {
        let _ = wake.write(&[0]);
    }
}

/// The screen is left only while shown, not while an external editor has the terminal. After a
/// hangup the terminal's calls fail at once.
fn exit(startup: Option<&console::Mode>) -> ! {
    #[cfg(unix)]
    stop_editor();
    if let Some(prompt) = PROMPT.lock().unwrap_or_else(PoisonError::into_inner).take() {
        let _ = std::fs::remove_file(prompt);
    }
    scramjet_rs::agent::end();
    let mut stderr = io::stderr().lock();
    if let Some(leave) = screen::leave_on_exit() {
        let _ = stderr.write_all(leave.as_bytes());
        let _ = stderr.flush();
    }
    if let Some(startup) = startup {
        let _ = startup.set();
    }
    std::process::exit(128 + SIGNAL.load(Ordering::SeqCst))
}

#[cfg(windows)]
mod windows {
    use std::{io, time::Duration};
    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
    };

    /// The console closing is Windows' hangup: Windows ends the process once the handler
    /// returns, so it waits there for the exit, which it is given about 5 seconds for.
    pub(super) fn install() -> io::Result<()> {
        unsafe extern "system" fn closing(event: u32) -> i32 {
            if !matches!(
                event,
                CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
            ) {
                return 0;
            }
            super::on_signal(signal_hook::consts::SIGTERM);
            std::thread::sleep(Duration::from_secs(10));
            1
        }
        // SAFETY: the handler is a plain function for the life of the process.
        match unsafe { SetConsoleCtrlHandler(Some(closing), 1) } {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }
}
