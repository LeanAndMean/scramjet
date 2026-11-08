//! The terminal's modes, size and typed input, as a Unix terminal and the Windows console each
//! give them.

use std::{io, io::PipeReader, time::Duration};

/// The terminal's input settings (on Windows, the console's input and output modes).
#[derive(Clone)]
pub(super) struct Mode(imp::Mode);

impl Mode {
    /// The settings now; none when the input is not a terminal.
    pub(super) fn get() -> Option<Self> {
        imp::get().map(Self)
    }
    /// In a background job, the kernel stops the job until it has the terminal again.
    pub(super) fn set(&self) -> io::Result<()> {
        imp::set(&self.0)
    }
    /// Sets the mode now if the agent has the terminal; in a background job, holds it for
    /// `set_held` to set once `fg` gives the terminal back, rather than stop the job for it.
    pub(super) fn set_in_foreground(&self) -> io::Result<()> {
        imp::set_in_foreground(&self.0)
    }
    /// Keys read as typed, one at a time, with nothing done for them and nothing echoed.
    pub(super) fn raw(&self) -> Self {
        Self(imp::raw(&self.0))
    }
    /// While a turn runs, typed keys are not echoed, and each is readable at once, so a lone
    /// Escape can be told from the start of a sequence. On Unix Ctrl+T becomes the quit
    /// character, so the kernel sends SIGQUIT for it, and `NOFLSH` keeps unread keys when it does.
    pub(super) fn turn(&self) -> Self {
        Self(imp::turn(&self.0))
    }
    /// An echoed line, edited by the terminal.
    pub(super) fn answer(&self) -> Self {
        Self(imp::answer(&self.0))
    }
}

/// Whether the agent's job has the terminal; true when the input is not a terminal.
pub(super) fn foreground() -> bool {
    imp::foreground()
}

/// Sets the mode held by `Mode::set_in_foreground`, if the agent now has the terminal.
#[cfg(unix)]
pub(super) fn set_held() {
    imp::set_held();
}

/// Drops the keys typed and not read yet.
pub(super) fn flush_input() {
    imp::flush_input();
}

/// The terminal's columns and rows.
pub(super) fn size() -> Option<(u16, u16)> {
    imp::size().filter(|&(columns, rows)| columns > 0 && rows > 0)
}

/// Waits up to `timeout` (none: no limit) for typed input or a byte in `wake`: whether input is
/// readable, and whether `wake` holds a byte. Both false at the timeout or on a signal.
pub(super) fn wait(
    wake: Option<&PipeReader>,
    timeout: Option<Duration>,
) -> io::Result<(bool, bool)> {
    imp::wait(wake, timeout)
}

/// How much `read` is given room for.
pub(super) const READ: usize = imp::READ;

/// Reads what was typed into `buffer`, at least one byte unless the input ended.
pub(super) fn read(buffer: &mut [u8]) -> io::Result<usize> {
    imp::read(buffer)
}

/// Makes reads and writes of a pipe's end return at once rather than wait.
#[cfg(unix)]
pub(super) fn nonblocking(pipe: impl std::os::fd::AsFd) -> io::Result<()> {
    rustix::fs::fcntl_setfl(pipe, rustix::fs::OFlags::NONBLOCK)?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn nonblocking(pipe: impl std::os::windows::io::AsHandle) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    imp::nonblocking(pipe.as_handle().as_raw_handle())
}

#[cfg(unix)]
mod imp {
    use rustix::{
        event::{PollFd, PollFlags, Timespec, poll},
        fd::BorrowedFd,
        io::Errno,
        termios::{self, LocalModes, OptionalActions, QueueSelector, SpecialCodeIndex, Termios},
    };
    use std::{
        cell::UnsafeCell,
        io,
        io::PipeReader,
        mem::MaybeUninit,
        sync::atomic::{AtomicU8, Ordering},
        time::Duration,
    };

    pub(super) type Mode = Termios;

    const NONE: u8 = 0;
    const HELD: u8 = 1;
    const BUSY: u8 = 2;
    /// Whether `HELD_MODE` holds a mode to set in the foreground, or is being accessed.
    static HOLD: AtomicU8 = AtomicU8::new(NONE);
    static HELD_MODE: Held = Held(UnsafeCell::new(MaybeUninit::uninit()));
    struct Held(UnsafeCell<MaybeUninit<Mode>>);
    // SAFETY: only whoever moved `HOLD` to BUSY touches the cell.
    unsafe impl Sync for Held {}

    /// Gives `access` the held mode if the state is `from`, leaving `after`. Never waits, as a
    /// signal handler may have interrupted the thread that has it.
    fn with_held(from: u8, after: u8, access: impl FnOnce(&mut MaybeUninit<Mode>)) -> bool {
        if HOLD
            .compare_exchange(from, BUSY, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        // SAFETY: BUSY gives the cell to this call alone.
        access(unsafe { &mut *HELD_MODE.0.get() });
        HOLD.store(after, Ordering::Release);
        true
    }

    pub(super) fn foreground() -> bool {
        termios::tcgetpgrp(stdin()).map_or(true, |group| group == rustix::process::getpgrp())
    }

    // Fd 0 stays open for the life of the process; these run in signal handlers too, where only
    // their system calls are made.
    fn stdin() -> BorrowedFd<'static> {
        // SAFETY: as above.
        unsafe { BorrowedFd::borrow_raw(0) }
    }

    pub(super) fn get() -> Option<Mode> {
        termios::tcgetattr(stdin()).ok()
    }

    pub(super) fn set(mode: &Mode) -> io::Result<()> {
        // A mode held from before would otherwise replace this one at `fg`.
        with_held(HELD, NONE, |_| ());
        Ok(termios::tcsetattr(stdin(), OptionalActions::Now, mode)?)
    }

    pub(super) fn set_in_foreground(mode: &Mode) -> io::Result<()> {
        if foreground() {
            return set(mode);
        }
        let hold = |cell: &mut MaybeUninit<Mode>| {
            cell.write(mode.clone());
        };
        if with_held(NONE, HELD, hold) || with_held(HELD, HELD, hold) {
            // `fg` may have come before the mode was held.
            set_held();
        }
        Ok(())
    }

    pub(super) fn set_held() {
        if !foreground() {
            return;
        }
        let mut held = None;
        // SAFETY: HELD means the cell was written.
        with_held(HELD, NONE, |cell| {
            held = Some(unsafe { cell.assume_init_ref() }.clone())
        });
        if let Some(mode) = held {
            let _ = termios::tcsetattr(stdin(), OptionalActions::Now, &mode);
        }
    }

    pub(super) fn raw(mode: &Mode) -> Mode {
        let mut raw = mode.clone();
        raw.make_raw();
        raw
    }

    pub(super) fn turn(mode: &Mode) -> Mode {
        let mut mode = mode.clone();
        mode.local_modes
            .remove(LocalModes::ECHO | LocalModes::ICANON);
        mode.local_modes.insert(LocalModes::NOFLSH);
        mode.special_codes[SpecialCodeIndex::VQUIT] = 0x14;
        mode.special_codes[SpecialCodeIndex::VMIN] = 1;
        mode.special_codes[SpecialCodeIndex::VTIME] = 0;
        mode
    }

    pub(super) fn answer(mode: &Mode) -> Mode {
        let mut mode = mode.clone();
        mode.local_modes
            .insert(LocalModes::ECHO | LocalModes::ICANON);
        mode
    }

    pub(super) fn flush_input() {
        let _ = termios::tcflush(stdin(), QueueSelector::IFlush);
    }

    pub(super) fn size() -> Option<(u16, u16)> {
        let size = termios::tcgetwinsize(io::stderr()).ok()?;
        Some((size.ws_col, size.ws_row))
    }

    pub(super) fn wait(
        wake: Option<&PipeReader>,
        timeout: Option<Duration>,
    ) -> io::Result<(bool, bool)> {
        let timeout = timeout.map(|wait| Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        });
        let stdin = stdin();
        let mut fds = vec![PollFd::new(&stdin, PollFlags::IN)];
        if let Some(wake) = wake {
            fds.push(PollFd::new(wake, PollFlags::IN));
        }
        match poll(&mut fds, timeout.as_ref()) {
            Ok(0) | Err(Errno::INTR) => Ok((false, false)),
            Ok(_) => Ok((
                !fds[0].revents().is_empty(),
                fds.get(1).is_some_and(|fd| !fd.revents().is_empty()),
            )),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) const READ: usize = 4096;

    pub(super) fn read(buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            match rustix::io::read(stdin(), &mut *buffer) {
                Err(Errno::INTR) => continue,
                result => return Ok(result?),
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::{
        io::{self, PipeReader, Read},
        os::windows::io::{AsRawHandle, RawHandle},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{HANDLE, WAIT_OBJECT_0},
        System::{
            Console::{
                CONSOLE_MODE, CONSOLE_SCREEN_BUFFER_INFO, DISABLE_NEWLINE_AUTO_RETURN,
                ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, ENABLE_PROCESSED_INPUT,
                ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
                FlushConsoleInputBuffer, GetConsoleMode, GetConsoleScreenBufferInfo, INPUT_RECORD,
                KEY_EVENT, PeekConsoleInputW, ReadConsoleInputW, SetConsoleMode,
            },
            Pipes::{PIPE_NOWAIT, PeekNamedPipe, SetNamedPipeHandleState},
            Threading::WaitForSingleObject,
        },
    };

    /// The input's mode, and the output's where stderr is the console.
    #[derive(Clone)]
    pub(super) struct Mode {
        input: CONSOLE_MODE,
        output: Option<CONSOLE_MODE>,
    }

    fn stdin() -> HANDLE {
        io::stdin().as_raw_handle()
    }

    fn stderr() -> HANDLE {
        io::stderr().as_raw_handle()
    }

    fn mode_of(handle: HANDLE) -> Option<CONSOLE_MODE> {
        let mut mode = 0;
        // SAFETY: the handle is the process's own; the call fills `mode`.
        (unsafe { GetConsoleMode(handle, &mut mode) } != 0).then_some(mode)
    }

    pub(super) fn get() -> Option<Mode> {
        Some(Mode {
            input: mode_of(stdin())?,
            output: mode_of(stderr()),
        })
    }

    pub(super) fn set(mode: &Mode) -> io::Result<()> {
        // SAFETY: the handles are the process's own.
        if unsafe { SetConsoleMode(stdin(), mode.input) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some(output) = mode.output {
            unsafe { SetConsoleMode(stderr(), output) };
        }
        Ok(())
    }

    /// No job control: the agent always has the console.
    pub(super) fn set_in_foreground(mode: &Mode) -> io::Result<()> {
        set(mode)
    }

    pub(super) fn foreground() -> bool {
        true
    }

    /// The keys arrive as the escape sequences a Unix terminal sends, and the output takes them
    /// too; a line feed only moves down, as with `OPOST` off.
    pub(super) fn raw(mode: &Mode) -> Mode {
        Mode {
            input: (mode.input & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT))
                | ENABLE_VIRTUAL_TERMINAL_INPUT,
            output: mode.output.map(|output| {
                output | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN
            }),
        }
    }

    /// Ctrl+C stays the console's, which the C runtime gives as SIGINT.
    pub(super) fn turn(mode: &Mode) -> Mode {
        Mode {
            input: (mode.input & !(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))
                | ENABLE_PROCESSED_INPUT
                | ENABLE_VIRTUAL_TERMINAL_INPUT,
            output: mode
                .output
                .map(|output| output | ENABLE_VIRTUAL_TERMINAL_PROCESSING),
        }
    }

    pub(super) fn answer(mode: &Mode) -> Mode {
        Mode {
            input: mode.input | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_PROCESSED_INPUT,
            output: mode.output,
        }
    }

    pub(super) fn flush_input() {
        // SAFETY: the handle is the process's own.
        unsafe { FlushConsoleInputBuffer(stdin()) };
    }

    pub(super) fn size() -> Option<(u16, u16)> {
        // SAFETY: the structure is the call's to fill.
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        if unsafe { GetConsoleScreenBufferInfo(stderr(), &mut info) } == 0 {
            return None;
        }
        let window = info.srWindow;
        let columns = u16::try_from(window.Right - window.Left + 1).ok()?;
        let rows = u16::try_from(window.Bottom - window.Top + 1).ok()?;
        Some((columns, rows))
    }

    /// The console's handle is signaled for any event, a resize, focus or a key's release
    /// included, which a read passes over and then waits on: those are taken off the queue
    /// here, and only a typed character counts.
    fn typed() -> bool {
        let mut records = [INPUT_RECORD::default(); 64];
        let mut count = 0;
        // SAFETY: the buffer holds the records the calls are given room for.
        if unsafe { PeekConsoleInputW(stdin(), records.as_mut_ptr(), 64, &mut count) } == 0 {
            return true;
        }
        let records = &records[..count as usize];
        let character = records.iter().any(|record| {
            // SAFETY: a key event's record holds a key event.
            record.EventType == KEY_EVENT as u16
                && unsafe {
                    record.Event.KeyEvent.bKeyDown != 0
                        && record.Event.KeyEvent.uChar.UnicodeChar != 0
                }
        });
        if !character && count > 0 {
            let mut discarded = [INPUT_RECORD::default(); 64];
            unsafe { ReadConsoleInputW(stdin(), discarded.as_mut_ptr(), count, &mut count) };
        }
        character
    }

    fn available(pipe: &PipeReader) -> bool {
        let mut bytes = 0;
        // SAFETY: no buffer is given, only the count to fill.
        let peeked = unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut bytes,
                std::ptr::null_mut(),
            )
        };
        peeked != 0 && bytes > 0
    }

    /// Neither a console nor an anonymous pipe can be waited on together with the other, so the
    /// pipe is looked at between short waits on the console.
    pub(super) fn wait(
        wake: Option<&PipeReader>,
        timeout: Option<Duration>,
    ) -> io::Result<(bool, bool)> {
        const SLICE: Duration = Duration::from_millis(10);
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        loop {
            let woken = wake.is_some_and(available);
            let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
            let slice = match (woken, left) {
                (true, _) => Duration::ZERO,
                (false, Some(left)) => left.min(SLICE),
                (false, None) => SLICE,
            };
            // SAFETY: the handle is the process's own.
            let signaled =
                unsafe { WaitForSingleObject(stdin(), slice.as_millis() as u32) } == WAIT_OBJECT_0;
            let typed = signaled && typed();
            if typed || woken || left.is_some_and(|left| left.is_zero()) {
                return Ok((typed, woken));
            }
        }
    }

    /// The standard library's own buffer's size: a read at least as large bypasses it, so that
    /// nothing typed waits there unseen.
    pub(super) const READ: usize = 8192;

    /// Through the standard library, which reads the console as UTF-16 and gives UTF-8. Its read
    /// drops a Ctrl+Z that ends what was read, the console's end of input.
    pub(super) fn read(buffer: &mut [u8]) -> io::Result<usize> {
        io::stdin().lock().read(buffer)
    }

    pub(super) fn nonblocking(pipe: RawHandle) -> io::Result<()> {
        let mode = PIPE_NOWAIT;
        // SAFETY: an anonymous pipe is a named pipe with a unique name, which this call takes.
        let set =
            unsafe { SetNamedPipeHandleState(pipe, &mode, std::ptr::null(), std::ptr::null()) };
        match set {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }
}
