//! Exiting on SIGTERM, SIGHUP or a second Ctrl-C, as Scramjet's shutdown does: the commands
//! running killed, and the terminal given back as the shell left it.

use super::screen;
use rustix::{
    fd::{BorrowedFd, IntoRawFd},
    termios::{self, OptionalActions, Termios},
};
use std::{
    io::{self, Read, Write},
    sync::atomic::{AtomicI32, Ordering},
};

/// The signal the process exits for, the first one that came.
static SIGNAL: AtomicI32 = AtomicI32::new(0);
/// The pipe that wakes the thread that exits.
static WAKE: AtomicI32 = AtomicI32::new(-1);

/// Makes SIGTERM and SIGHUP, and `on_signal`, exit with `startup` the terminal mode the agent
/// started in.
pub(super) fn install(startup: Option<Termios>) -> io::Result<()> {
    let (mut reader, writer) = io::pipe()?;
    WAKE.store(writer.into_raw_fd(), Ordering::SeqCst);
    // The exit runs here, not in a handler, as it waits for other threads' writes.
    std::thread::Builder::new()
        .name("exit".into())
        .spawn(move || {
            if reader.read_exact(&mut [0]).is_ok() {
                exit(startup.as_ref());
            }
        })?;
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGHUP] {
        // SAFETY: `on_signal` makes only async-signal-safe calls.
        unsafe { signal_hook::low_level::register(signal, move || on_signal(signal)) }?;
    }
    Ok(())
}

/// Has the process exit for `signal`, with the status 128 + `signal` a shell gives a process
/// killed by it. Async-signal-safe: an atomic exchange and a write.
pub(super) fn on_signal(signal: i32) {
    if SIGNAL
        .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // SAFETY: the pipe's write end stays open for the life of the process.
        let wake = unsafe { BorrowedFd::borrow_raw(WAKE.load(Ordering::SeqCst)) };
        let _ = rustix::io::write(wake, &[0]);
    }
}

/// The screen is left only while shown, not while an external editor has the terminal. After a
/// hangup the terminal's calls fail at once.
fn exit(startup: Option<&Termios>) -> ! {
    scramjet_rs::agent::end();
    let mut stderr = io::stderr().lock();
    if let Some(leave) = screen::leave_on_exit() {
        let _ = stderr.write_all(leave.as_bytes());
        let _ = stderr.flush();
    }
    if let Some(startup) = startup {
        let _ = termios::tcsetattr(io::stdin(), OptionalActions::Now, startup);
    }
    std::process::exit(128 + SIGNAL.load(Ordering::SeqCst))
}
