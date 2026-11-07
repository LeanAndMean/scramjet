//! Ctrl-Z: stopping the agent's job with the terminal as the shell left it, and taking the
//! terminal back when the shell continues it. Windows has no job control: there nothing stops.

use super::console;
#[cfg(unix)]
use rustix::process::Signal;
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(unix)]
use std::{sync::atomic::AtomicUsize, time::Duration};

/// Set when the agent continues after a stop: the shell has written below the agent's output,
/// so output drawn in place starts again at the cursor.
static CONTINUED: AtomicBool = AtomicBool::new(false);
/// Counts the stops the handler has come back from.
#[cfg(unix)]
static STOPS: AtomicUsize = AtomicUsize::new(0);

/// Handles SIGTSTP, sent by Ctrl-Z during a turn or by `suspend`, with `startup` the terminal
/// mode the agent started in.
#[cfg(unix)]
pub(super) fn install(startup: Option<console::Mode>) -> io::Result<()> {
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
#[cfg(unix)]
pub(super) fn suspend() {
    let stops = STOPS.load(Ordering::SeqCst);
    if rustix::process::kill_current_process_group(Signal::TSTP).is_err() {
        return;
    }
    // The handler may run on another thread, after the signal is sent.
    while STOPS.load(Ordering::SeqCst) == stops {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Whether the agent was continued after a stop since the last call.
pub(super) fn continued() -> bool {
    CONTINUED.swap(false, Ordering::Relaxed)
}

/// Stops with SIGTSTP's default action, so the kernel still discards it in an orphaned process
/// group, where no shell could continue the agent. Keys the agent has not read yet are dropped
/// as the default Ctrl-Z drops them; the turn's `NOFLSH` would otherwise give them to the shell
/// to run.
#[cfg(unix)]
fn stop(startup: Option<&console::Mode>) {
    let mode = console::Mode::get();
    console::flush_input();
    if let Some(startup) = startup {
        let _ = startup.set();
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
        let _ = mode.set();
    }
    CONTINUED.store(true, Ordering::Relaxed);
    STOPS.fetch_add(1, Ordering::SeqCst);
}

#[cfg(windows)]
pub(super) fn install(_startup: Option<console::Mode>) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub(super) fn suspend() {}
