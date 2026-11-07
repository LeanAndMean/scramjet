//! Exiting on SIGTERM, SIGHUP (on Windows, the console closing) or a second Ctrl-C, as
//! Scramjet's shutdown does: the commands running killed, and the terminal given back as the
//! shell left it.

use super::{console, screen};
use std::{
    io::{self, PipeWriter, Read, Write},
    sync::{
        OnceLock,
        atomic::{AtomicI32, Ordering},
    },
};

/// The signal the process exits for, the first one that came.
static SIGNAL: AtomicI32 = AtomicI32::new(0);
/// The pipe that wakes the thread that exits.
static WAKE: OnceLock<PipeWriter> = OnceLock::new();

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
