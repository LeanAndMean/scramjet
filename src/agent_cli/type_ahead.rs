//! Keys typed while a turn runs: as in Scramjet, a lone Escape cancels the turn; Ctrl+O shows the
//! conversation so far; the other bytes wait for the next prompt.

use super::editor::ESCAPE_WAIT;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fd::{AsFd, BorrowedFd},
    io::Errno,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// Set by Ctrl-C's signal handler, which cannot take the bytes: they are dropped.
pub(super) static DISCARD: AtomicBool = AtomicBool::new(false);

/// How often the reader checks whether to stop.
const CHECK: Duration = Duration::from_millis(20);

/// Ctrl+O, Scramjet's key to expand tool output.
const EXPAND: u8 = 0x0f;

/// Shows the conversation, given the keys typed after Ctrl+O and the flag that is set when
/// reading should stop, until the user closes it; returns the keys typed after that.
pub(super) type Expand = Arc<dyn Fn(Vec<u8>, &AtomicBool) -> Vec<u8> + Send + Sync>;

/// Reads the terminal in a thread until `finish`; the terminal must be in non-canonical mode.
pub(super) struct TypeAhead {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    expand: Option<Expand>,
    thread: JoinHandle<Vec<u8>>,
}

impl TypeAhead {
    /// Reads stdin; a lone Escape sets `cancel`, and Ctrl+O calls `expand`.
    pub(super) fn start(cancel: Arc<AtomicBool>, expand: Option<Expand>) -> Self {
        DISCARD.store(false, Ordering::Relaxed);
        Self::reading(std::io::stdin(), cancel, expand, Vec::new())
    }

    /// Reads stdin again after `finish` returned `typed`, which a Ctrl-C since then discards.
    pub(super) fn resume(cancel: Arc<AtomicBool>, expand: Option<Expand>, typed: Vec<u8>) -> Self {
        Self::reading(std::io::stdin(), cancel, expand, typed)
    }

    fn reading(
        input: impl AsFd + Send + 'static,
        cancel: Arc<AtomicBool>,
        expand: Option<Expand>,
        typed: Vec<u8>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (stop, cancel, expand) = (stop.clone(), cancel.clone(), expand.clone());
            move || collect(input.as_fd(), &stop, &cancel, expand.as_ref(), typed)
        });
        Self {
            stop,
            cancel,
            expand,
            thread,
        }
    }

    pub(super) fn cancel(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    pub(super) fn expand(&self) -> Option<Expand> {
        self.expand.clone()
    }

    /// Stops reading and returns what was typed since Ctrl-C last discarded it, once a view
    /// that Ctrl+O opened is closed.
    pub(super) fn finish(self) -> Vec<u8> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap_or_default()
    }
}

fn collect(
    input: BorrowedFd,
    stop: &AtomicBool,
    cancel: &AtomicBool,
    expand: Option<&Expand>,
    mut typed: Vec<u8>,
) -> Vec<u8> {
    // Escapes ending the last read: a key of their own unless more bytes follow at once, as
    // they do in the sequence of an arrow key or Alt+key.
    let mut escapes = 0;
    let mut buffer = [0; 4096];
    while !stop.load(Ordering::Relaxed) {
        if DISCARD.swap(false, Ordering::Relaxed) {
            typed.clear();
            escapes = 0;
        }
        let wait = if escapes > 0 { ESCAPE_WAIT } else { CHECK };
        let wait = Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        match poll(&mut [PollFd::new(&input, PollFlags::IN)], Some(&wait)) {
            Ok(0) => {
                if escapes > 0 {
                    escapes = 0;
                    cancel.store(true, Ordering::Relaxed);
                }
                continue;
            }
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(_) => break,
        }
        let read = match rustix::io::read(input, &mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(Errno::INTR | Errno::AGAIN) => continue,
            Err(_) => break,
        };
        typed.extend(std::iter::repeat_n(0x1b, escapes));
        let mut chunk = buffer[..read].to_vec();
        while let Some((expand, at)) =
            expand.and_then(|expand| Some((expand, chunk.iter().position(|&b| b == EXPAND)?)))
        {
            typed.extend_from_slice(&chunk[..at]);
            chunk = expand(chunk.split_off(at + 1), stop);
        }
        escapes = chunk.iter().rev().take_while(|&&byte| byte == 0x1b).count();
        typed.extend_from_slice(&chunk[..chunk.len() - escapes]);
    }
    if DISCARD.swap(false, Ordering::Relaxed) {
        typed.clear();
    }
    typed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Write, pipe},
        time::Instant,
    };

    fn wait_for(cancel: &AtomicBool) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
        while !cancel.load(Ordering::Relaxed) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.load(Ordering::Relaxed)
    }

    #[test]
    fn a_lone_escape_cancels_and_is_not_kept() {
        let (reader, mut writer) = pipe().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let typed = TypeAhead::reading(reader, cancel.clone(), None, Vec::new());
        writer.write_all(b"kept\x1b").unwrap();
        assert!(wait_for(&cancel));
        writer.write_all(b" after").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(typed.finish(), b"kept after");
    }

    #[test]
    fn escape_sequences_are_typed_text() {
        let (reader, mut writer) = pipe().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let typed = TypeAhead::reading(reader, cancel.clone(), None, Vec::new());
        // Up, Alt+b, Alt+Up as some terminals send it.
        writer.write_all(b"a\x1b[A\x1bb\x1b\x1b[A").unwrap();
        std::thread::sleep(ESCAPE_WAIT * 4);
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(typed.finish(), b"a\x1b[A\x1bb\x1b\x1b[A");
    }

    #[test]
    fn ctrl_o_shows_the_view_with_the_keys_after_it() {
        let (reader, mut writer) = pipe().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let shown = Arc::new(std::sync::Mutex::new(Vec::new()));
        let expand: Expand = Arc::new({
            let shown = shown.clone();
            // As the view: the first key closes it, the rest is typed after.
            move |keys: Vec<u8>, _stop: &AtomicBool| {
                shown.lock().unwrap().push(keys.clone());
                keys.get(1..).unwrap_or_default().to_vec()
            }
        });
        let typed = TypeAhead::reading(reader, cancel.clone(), Some(expand), Vec::new());
        writer.write_all(b"before\x0fq after\x0f").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(typed.finish(), b"before after");
        assert_eq!(
            *shown.lock().unwrap(),
            [b"q after\x0f".to_vec(), Vec::new()]
        );
    }
}
