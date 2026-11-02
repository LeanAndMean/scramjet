//! Ctrl-Z: stopping the agent's job with the terminal as the shell left it, and taking the
//! terminal back when the shell continues it.

use rustix::{
    fd::BorrowedFd,
    process::Signal,
    termios::{self, OptionalActions, QueueSelector, Termios},
};
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

/// Set when the agent continues after a stop: the shell has written below the agent's output,
/// so output drawn in place starts again at the cursor.
static CONTINUED: AtomicBool = AtomicBool::new(false);

/// Handles SIGTSTP, sent by Ctrl-Z during a turn or by `suspend`, with `startup` the terminal
/// mode the agent started in.
pub(super) fn install(startup: Option<Termios>) -> io::Result<()> {
    // SAFETY: `stop` makes only async-signal-safe calls.
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGTSTP, move || {
            stop(startup.as_ref())
        })
    }
    .map(drop)
}

/// Stops the agent's job, as Ctrl-Z does when the terminal is not in raw mode. Returns once the
/// job is continued.
pub(super) fn suspend() {
    // Linux delivers the signal to the calling thread first, so the handler has stopped and
    // continued the agent before this returns.
    let _ = rustix::process::kill_current_process_group(Signal::TSTP);
}

/// Whether the agent was continued after a stop since the last call.
pub(super) fn continued() -> bool {
    CONTINUED.swap(false, Ordering::Relaxed)
}

/// Stops with SIGTSTP's default action, so the kernel still discards it in an orphaned process
/// group, where no shell could continue the agent. Typed-ahead text is dropped as the default
/// Ctrl-Z drops it; the turn's `NOFLSH` would otherwise give it to the shell to run.
fn stop(startup: Option<&Termios>) {
    // SAFETY: fd 0 stays open for the life of the process.
    let stdin = unsafe { BorrowedFd::borrow_raw(0) };
    let mode = termios::tcgetattr(stdin).ok();
    let _ = termios::tcflush(stdin, QueueSelector::IFlush);
    if let Some(startup) = startup {
        let _ = termios::tcsetattr(stdin, OptionalActions::Now, startup);
    }
    // SAFETY: sigaction, pthread_sigmask and raise are async-signal-safe; the handler is put
    // back as it was before this returns.
    unsafe {
        let mut default: libc::sigaction = std::mem::zeroed();
        default.sa_sigaction = libc::SIG_DFL;
        let mut handler: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGTSTP, &default, &mut handler) == 0 {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            libc::sigaddset(&mut set, libc::SIGTSTP);
            // Blocked while its handler runs; unblocking delivers it.
            libc::raise(libc::SIGTSTP);
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            libc::sigaction(libc::SIGTSTP, &handler, std::ptr::null_mut());
        }
    }
    if let Some(mode) = &mode {
        let _ = termios::tcsetattr(stdin, OptionalActions::Now, mode);
    }
    CONTINUED.store(true, Ordering::Relaxed);
}
