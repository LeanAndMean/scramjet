//! Prompt editor for the interactive agent: raw-mode editing, bracketed paste and history.

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    termios::{self, OptionalActions, Termios},
};
use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How long the rest of an escape sequence may take to arrive before Escape counts as a lone key.
pub(super) const ESCAPE_WAIT: Duration = Duration::from_millis(30);
/// How often an idle editor checks for a terminal resize.
const RESIZE_CHECK: Duration = Duration::from_millis(250);
const TAB: &str = "    ";
const INDENT: usize = 2;

#[derive(Debug, PartialEq)]
enum Key {
    Text(String),
    Paste(String),
    Enter,
    NewLine,
    Backspace,
    Delete,
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    DeleteWordBack,
    DeleteWordForward,
    KillStart,
    KillEnd,
    Interrupt,
    EndOfInput,
    ToggleThinking,
    External,
    Suspend,
    Idle,
    Ignored,
}

trait ByteSource {
    /// The next input byte; `None` when `wait` elapses first. `wait: None` blocks.
    fn byte(&mut self, wait: Option<Duration>) -> io::Result<Option<u8>>;
}

fn read_key(source: &mut impl ByteSource, wait: Option<Duration>) -> io::Result<Key> {
    let Some(byte) = source.byte(wait)? else {
        return Ok(Key::Idle);
    };
    Ok(match byte {
        0x1b => escape(source)?,
        b'\r' => Key::Enter,
        b'\n' => Key::NewLine,
        b'\t' => Key::Text("\t".into()),
        0x7f | 8 => Key::Backspace,
        1 => Key::LineStart,
        2 => Key::Left,
        3 => Key::Interrupt,
        4 => Key::EndOfInput,
        5 => Key::LineEnd,
        6 => Key::Right,
        7 => Key::External,
        11 => Key::KillEnd,
        14 => Key::Down,
        16 => Key::Up,
        20 => Key::ToggleThinking,
        21 => Key::KillStart,
        23 => Key::DeleteWordBack,
        26 => Key::Suspend,
        0x20..=0x7e => Key::Text(char::from(byte).to_string()),
        0xc2..=0xf4 => {
            let length = match byte {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            let mut bytes = vec![byte];
            while bytes.len() < length {
                match source.byte(Some(ESCAPE_WAIT))? {
                    Some(next) => bytes.push(next),
                    None => return Ok(Key::Ignored),
                }
            }
            String::from_utf8(bytes).map_or(Key::Ignored, Key::Text)
        }
        _ => Key::Ignored,
    })
}

fn escape(source: &mut impl ByteSource) -> io::Result<Key> {
    let mut next = source.byte(Some(ESCAPE_WAIT))?;
    // Some terminals send Alt+key as an extra Escape before the key's own sequence.
    while next == Some(0x1b) {
        next = source.byte(Some(ESCAPE_WAIT))?;
    }
    Ok(match next {
        None => Key::Ignored,
        Some(b'[') => csi(source)?,
        Some(b'O') => match source.byte(Some(ESCAPE_WAIT))? {
            Some(b'A') => Key::Up,
            Some(b'B') => Key::Down,
            Some(b'C') => Key::Right,
            Some(b'D') => Key::Left,
            Some(b'H') => Key::LineStart,
            Some(b'F') => Key::LineEnd,
            _ => Key::Ignored,
        },
        Some(b'\r' | b'\n') => Key::NewLine,
        Some(0x7f | 8) => Key::DeleteWordBack,
        Some(b'b') => Key::WordLeft,
        Some(b'f') => Key::WordRight,
        Some(b'd') => Key::DeleteWordForward,
        Some(_) => Key::Ignored,
    })
}

fn csi(source: &mut impl ByteSource) -> io::Result<Key> {
    let mut parameters = Vec::new();
    let last = loop {
        let Some(byte) = source.byte(Some(ESCAPE_WAIT))? else {
            return Ok(Key::Ignored);
        };
        if (0x40..=0x7e).contains(&byte) {
            break byte;
        }
        if parameters.len() < 32 {
            parameters.push(byte);
        }
    };
    let parameters = String::from_utf8_lossy(&parameters);
    let modifier = parameters.split(';').nth(1);
    let word = matches!(modifier, Some("3" | "5"));
    Ok(match (last, parameters.as_ref()) {
        (b'A', _) => Key::Up,
        (b'B', _) => Key::Down,
        (b'C', _) if word => Key::WordRight,
        (b'D', _) if word => Key::WordLeft,
        (b'C', _) => Key::Right,
        (b'D', _) => Key::Left,
        (b'H', _) | (b'~', "1" | "7") => Key::LineStart,
        (b'F', _) | (b'~', "4" | "8") => Key::LineEnd,
        (b'~', "3") => Key::Delete,
        (b'~', "3;3" | "3;5") => Key::DeleteWordForward,
        (b'~', "200") => paste(source)?,
        (b'~', "27;2;13" | "27;3;13") | (b'u', "13;2" | "13;3") => Key::NewLine,
        (b'u', "13") => Key::Enter,
        _ => Key::Ignored,
    })
}

fn paste(source: &mut impl ByteSource) -> io::Result<Key> {
    const END: &[u8] = b"\x1b[201~";
    let mut bytes = Vec::new();
    while !bytes.ends_with(END) {
        if let Some(byte) = source.byte(None)? {
            bytes.push(byte);
        }
    }
    bytes.truncate(bytes.len() - END.len());
    Ok(Key::Paste(clean(&String::from_utf8_lossy(&bytes))))
}

/// Pasted text keeps its line breaks and tabs; other control characters are dropped.
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

#[derive(Default)]
struct Buffer {
    text: String,
    /// Byte offset, always on a grapheme boundary.
    cursor: usize,
}

impl Buffer {
    fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }
    fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }
    fn remove(&mut self, start: usize, end: usize) {
        self.text.replace_range(start..end, "");
        self.cursor = start;
    }
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }
    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |g| self.cursor + g.len())
    }
    fn word_start(&self) -> usize {
        let before = self.text[..self.cursor].trim_end();
        before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(index, c)| index + c.len_utf8())
    }
    fn word_end(&self) -> usize {
        let after = &self.text[self.cursor..];
        let rest = after.trim_start();
        let skipped = after.len() - rest.len();
        self.cursor + skipped + rest.find(char::is_whitespace).unwrap_or(rest.len())
    }
    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self) -> usize {
        self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |i| self.cursor + i)
    }
    fn apply(&mut self, key: &Key) {
        match key {
            Key::Text(text) | Key::Paste(text) => self.insert(text),
            Key::NewLine => self.insert("\n"),
            Key::Backspace => self.remove(self.previous(), self.cursor),
            Key::Delete => {
                let end = self.next();
                self.remove(self.cursor, end);
            }
            Key::Left => self.cursor = self.previous(),
            Key::Right => self.cursor = self.next(),
            Key::WordLeft => self.cursor = self.word_start(),
            Key::WordRight => self.cursor = self.word_end(),
            Key::LineStart => self.cursor = self.line_start(),
            Key::LineEnd => self.cursor = self.line_end(),
            Key::DeleteWordBack => self.remove(self.word_start(), self.cursor),
            Key::DeleteWordForward => {
                let end = self.word_end();
                self.remove(self.cursor, end);
            }
            Key::KillStart => match self.line_start() {
                start if start == self.cursor => self.remove(self.previous(), self.cursor),
                start => self.remove(start, self.cursor),
            },
            Key::KillEnd => match self.line_end() {
                end if end == self.cursor => {
                    let next = self.next();
                    self.remove(self.cursor, next);
                }
                end => self.remove(self.cursor, end),
            },
            _ => {}
        }
    }
}

/// Screen rows for the text, with the row and column of every cursor position.
struct Layout {
    rows: Vec<String>,
    stops: Vec<Stop>,
}

#[derive(Clone, Copy)]
struct Stop {
    offset: usize,
    row: usize,
    column: usize,
}

/// Where the word being placed starts, when moving it to a new row would leave text behind.
struct WordStart {
    stop: usize,
    byte: usize,
    column: usize,
}

impl Layout {
    /// Wraps lines between words where it can. As in Scramjet's editor the last column stays
    /// free for the cursor; spaces that reach it hang there instead of starting a row.
    fn new(text: &str, width: usize) -> Self {
        let limit = width.max(INDENT + 3) - 1;
        let mut rows = vec!["> ".to_owned()];
        let mut stops = Vec::new();
        let mut offset = 0;
        for (number, line) in text.split('\n').enumerate() {
            if number > 0 {
                rows.push(". ".into());
            }
            let mut column = INDENT;
            let mut row_has_text = false;
            let mut after_space = false;
            let mut hanging = false;
            let mut word: Option<WordStart> = None;
            for (index, grapheme) in line.grapheme_indices(true) {
                let shown = if grapheme == "\t" { TAB } else { grapheme };
                let cells = shown.width();
                let row = rows.len() - 1;
                if grapheme == " " || grapheme == "\t" {
                    after_space = true;
                    word = None;
                    hanging |= column + cells > limit;
                    stops.push(Stop {
                        offset: offset + index,
                        row,
                        column,
                    });
                    if !hanging {
                        rows[row].push_str(shown);
                        column += cells;
                    }
                    continue;
                }
                let starts_word = std::mem::take(&mut after_space);
                if hanging {
                    rows.push(" ".repeat(INDENT));
                    column = INDENT;
                    hanging = false;
                } else if starts_word && row_has_text {
                    word = Some(WordStart {
                        stop: stops.len(),
                        byte: rows[row].len(),
                        column,
                    });
                }
                if column + cells > limit && column > INDENT {
                    let tail = match word.take() {
                        Some(start) => {
                            for stop in &mut stops[start.stop..] {
                                stop.row += 1;
                                stop.column = stop.column - start.column + INDENT;
                            }
                            column = column - start.column + INDENT;
                            rows[row].split_off(start.byte)
                        }
                        None => {
                            column = INDENT;
                            String::new()
                        }
                    };
                    rows.push(" ".repeat(INDENT) + &tail);
                    if column + cells > limit && column > INDENT {
                        rows.push(" ".repeat(INDENT));
                        column = INDENT;
                    }
                }
                stops.push(Stop {
                    offset: offset + index,
                    row: rows.len() - 1,
                    column,
                });
                rows.last_mut().unwrap().push_str(shown);
                column += cells;
                row_has_text = true;
            }
            let end = offset + line.len();
            stops.push(Stop {
                offset: end,
                row: rows.len() - 1,
                column,
            });
            offset = end + 1;
        }
        Self { rows, stops }
    }

    fn at(&self, cursor: usize) -> Stop {
        *self
            .stops
            .iter()
            .find(|stop| stop.offset == cursor)
            .unwrap_or_else(|| self.stops.last().unwrap())
    }

    /// The cursor offset on the row above or below, nearest the current column.
    fn vertical(&self, cursor: usize, up: bool) -> Option<usize> {
        let here = self.at(cursor);
        let row = if up {
            here.row.checked_sub(1)?
        } else {
            here.row + 1
        };
        let mut row_stops = self.stops.iter().filter(|stop| stop.row == row);
        let first = *row_stops.next()?;
        Some(
            std::iter::once(first)
                .chain(row_stops.copied())
                .take_while(|stop| stop.column <= here.column)
                .last()
                .unwrap_or(first)
                .offset,
        )
    }
}

struct RawMode {
    original: Termios,
}

impl RawMode {
    fn enter() -> io::Result<Self> {
        let original = termios::tcgetattr(io::stdin())?;
        let mode = Self { original };
        mode.resume()?;
        Ok(mode)
    }

    /// Raw mode with bracketed paste on.
    fn resume(&self) -> io::Result<()> {
        let mut raw = self.original.clone();
        raw.make_raw();
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &raw)?;
        eprint!("\x1b[?2004h");
        Ok(())
    }

    /// The terminal as the shell left it, for an external editor or the shell itself.
    fn suspend(&self) {
        eprint!("\x1b[?2004l");
        let _ = io::stderr().flush();
        let _ = termios::tcsetattr(io::stdin(), OptionalActions::Now, &self.original);
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        self.suspend();
    }
}

#[derive(Default)]
struct Stdin {
    buffer: Vec<u8>,
    start: usize,
}

impl Stdin {
    fn pending(&self) -> bool {
        self.start < self.buffer.len()
    }
}

impl ByteSource for Stdin {
    fn byte(&mut self, wait: Option<Duration>) -> io::Result<Option<u8>> {
        if self.start == self.buffer.len() {
            let stdin = io::stdin();
            let timeout = wait.map(|wait| Timespec {
                tv_sec: wait.as_secs() as i64,
                tv_nsec: i64::from(wait.subsec_nanos()),
            });
            let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
            match poll(&mut fds, timeout.as_ref()) {
                Ok(0) => return Ok(None),
                Ok(_) => {}
                Err(Errno::INTR) => return Ok(None),
                Err(error) => return Err(error.into()),
            }
            self.buffer.resize(4096, 0);
            let read = loop {
                match rustix::io::read(&stdin, &mut self.buffer) {
                    Err(Errno::INTR) => continue,
                    result => break result?,
                }
            };
            if read == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.buffer.truncate(read);
            self.start = 0;
        }
        self.start += 1;
        Ok(Some(self.buffer[self.start - 1]))
    }
}

/// The editor's place on screen between redraws.
#[derive(Default)]
struct Screen {
    /// Row of the terminal cursor, counted from the first drawn row.
    cursor_row: usize,
    /// First layout row drawn when the text is taller than the terminal.
    top: usize,
    width: usize,
}

/// The rows a prompt takes on screen as the editor shows it, wrapped to `width`.
pub(super) fn prompt_rows(text: &str, width: usize) -> Vec<String> {
    Layout::new(text, width).rows
}

pub(super) fn terminal_size() -> (usize, usize) {
    termios::tcgetwinsize(io::stderr())
        .ok()
        .filter(|size| size.ws_col > 0 && size.ws_row > 0)
        .map_or((80, 24), |size| {
            (usize::from(size.ws_col), usize::from(size.ws_row))
        })
}

impl Screen {
    /// Clears the drawn rows and leaves the cursor where the first one starts.
    fn erase(&mut self) -> io::Result<()> {
        let mut stderr = io::stderr().lock();
        if self.cursor_row > 0 {
            write!(stderr, "\x1b[{}A", self.cursor_row)?;
        }
        stderr.write_all(b"\r\x1b[J")?;
        self.cursor_row = 0;
        self.top = 0;
        stderr.flush()
    }

    /// Redraws the editor. `done` draws every row and leaves the cursor below them.
    fn draw(&mut self, buffer: &Buffer, hint: Option<&str>, done: bool) -> io::Result<()> {
        let (width, height) = terminal_size();
        self.width = width;
        let layout = Layout::new(&buffer.text, width);
        let cursor = layout.at(buffer.cursor);
        let total = layout.rows.len();
        let visible = if done {
            total
        } else {
            height.saturating_sub(2).max(1).min(total)
        };
        self.top = self
            .top
            .min(cursor.row)
            .max((cursor.row + 1).saturating_sub(visible))
            .min(total - visible);
        let mut out = String::new();
        if self.cursor_row > 0 {
            write!(out, "\x1b[{}A", self.cursor_row).unwrap();
        }
        out.push_str("\r\x1b[J");
        out.push_str(&layout.rows[self.top..self.top + visible].join("\r\n"));
        if let Some(hint) = hint {
            write!(out, "\r\n\x1b[2m{hint}\x1b[0m").unwrap();
        }
        if done {
            out.push_str("\r\n");
            self.cursor_row = 0;
            self.top = 0;
        } else {
            let row = cursor.row - self.top;
            let last = visible - 1 + usize::from(hint.is_some());
            if last > row {
                write!(out, "\x1b[{}A", last - row).unwrap();
            }
            out.push('\r');
            if cursor.column > 0 {
                write!(out, "\x1b[{}C", cursor.column).unwrap();
            }
            self.cursor_row = row;
        }
        let mut stderr = io::stderr().lock();
        stderr.write_all(out.as_bytes())?;
        stderr.flush()
    }
}

pub struct Editor {
    history: Vec<String>,
    hide_thinking: Arc<AtomicBool>,
    /// Keys typed during the last turn, read before the terminal.
    typed: Vec<u8>,
    /// The text the next prompt starts with.
    text: String,
}

impl Editor {
    /// Ctrl+T flips `hide_thinking`.
    pub fn new(history: Vec<String>, hide_thinking: Arc<AtomicBool>) -> Self {
        Self {
            history,
            hide_thinking,
            typed: Vec::new(),
            text: String::new(),
        }
    }

    pub fn type_ahead(&mut self, typed: Vec<u8>) {
        self.typed = typed;
    }

    /// Starts the next prompt with `text`, the cursor at its end.
    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    /// Makes `prompt` the most recent entry that Up recalls.
    pub fn remember(&mut self, prompt: &str) {
        if self.history.last().is_none_or(|last| last != prompt) {
            self.history.push(prompt.to_owned());
        }
    }

    /// Reads one prompt. `None` means the user asked to exit.
    pub fn read(&mut self) -> io::Result<Option<String>> {
        let raw = RawMode::enter()?;
        let mut source = Stdin {
            buffer: std::mem::take(&mut self.typed),
            start: 0,
        };
        let mut screen = Screen::default();
        let mut buffer = Buffer::default();
        buffer.set(std::mem::take(&mut self.text));
        let mut browsing = self.history.len();
        let mut draft = String::new();
        let mut exit_armed = false;
        let mut note: Option<String> = None;
        let mut dirty = false;
        screen.draw(&buffer, None, false)?;
        loop {
            let key = match read_key(&mut source, Some(RESIZE_CHECK)) {
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Key::EndOfInput,
                result => result?,
            };
            let armed = std::mem::take(&mut exit_armed);
            let noted = note.take();
            let changed = match key {
                // Stopped by a SIGTSTP from outside: the shell wrote below the prompt.
                Key::Idle if super::job::continued() => {
                    exit_armed = armed;
                    note = noted;
                    screen = Screen::default();
                    true
                }
                Key::Idle => {
                    exit_armed = armed;
                    note = noted;
                    terminal_size().0 != screen.width
                }
                Key::Suspend => {
                    screen.erase()?;
                    raw.suspend();
                    super::job::suspend();
                    // Drawn again below, not once more at the next idle check.
                    super::job::continued();
                    raw.resume()?;
                    exit_armed = armed;
                    note = noted;
                    true
                }
                Key::Ignored => {
                    exit_armed = armed;
                    note = noted;
                    false
                }
                Key::ToggleThinking => {
                    let hidden = !self.hide_thinking.fetch_xor(true, Ordering::Relaxed);
                    note = Some(thinking_note(hidden).to_owned());
                    true
                }
                Key::External => {
                    if let Some(command) = editor_command() {
                        // A full-screen editor gives the screen back as it was, so the prompt
                        // is drawn again in its place.
                        screen.erase()?;
                        raw.suspend();
                        let edited = run_editor(&command, &buffer.text);
                        raw.resume()?;
                        match edited {
                            Ok(Some(text)) => buffer.set(text),
                            Ok(None) => {}
                            Err(error) => note = Some(error),
                        }
                    } else {
                        note = Some(NO_EDITOR.to_owned());
                    }
                    true
                }
                Key::Enter if buffer.text[..buffer.cursor].ends_with('\\') => {
                    buffer.apply(&Key::Backspace);
                    buffer.apply(&Key::NewLine);
                    true
                }
                Key::Enter if buffer.text.trim().is_empty() => true,
                Key::Enter => {
                    // As in Scramjet, the prompt is sent trimmed, and shown as sent.
                    buffer.set(buffer.text.trim().to_owned());
                    screen.draw(&buffer, None, true)?;
                    return Ok(Some(buffer.text));
                }
                Key::Interrupt if buffer.text.is_empty() => {
                    if armed {
                        screen.draw(&buffer, None, true)?;
                        return Ok(None);
                    }
                    exit_armed = true;
                    true
                }
                Key::Interrupt => {
                    buffer = Buffer::default();
                    browsing = self.history.len();
                    true
                }
                Key::EndOfInput if buffer.text.is_empty() => {
                    screen.draw(&buffer, None, true)?;
                    return Ok(None);
                }
                Key::Up | Key::Down => {
                    let up = key == Key::Up;
                    let layout = Layout::new(&buffer.text, terminal_size().0);
                    if let Some(cursor) = layout.vertical(buffer.cursor, up) {
                        buffer.cursor = cursor;
                    } else if up && browsing > 0 {
                        if browsing == self.history.len() {
                            draft = std::mem::take(&mut buffer.text);
                        }
                        browsing -= 1;
                        buffer.set(self.history[browsing].clone());
                    } else if !up && browsing < self.history.len() {
                        browsing += 1;
                        buffer.set(match self.history.get(browsing) {
                            Some(entry) => entry.clone(),
                            None => std::mem::take(&mut draft),
                        });
                    }
                    true
                }
                Key::EndOfInput => {
                    buffer.apply(&Key::Delete);
                    true
                }
                key => {
                    buffer.apply(&key);
                    true
                }
            };
            dirty |= changed;
            if dirty && !source.pending() {
                let hint = exit_armed.then_some(EXIT_HINT).or(note.as_deref());
                screen.draw(&buffer, hint, false)?;
                dirty = false;
            }
        }
    }
}

const EXIT_HINT: &str = "(Ctrl-C again or Ctrl-D to exit)";
const NO_EDITOR: &str = "No editor configured. Set $VISUAL or $EDITOR environment variable.";

/// `$VISUAL`, else `$EDITOR`, as Scramjet reads them.
fn editor_command() -> Option<String> {
    ["VISUAL", "EDITOR"].iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

/// Runs the editor on a temporary file holding `text`. `Ok(None)` when it exits with another
/// status than 0, which keeps the prompt text. Its file's text, without one trailing newline,
/// replaces the prompt text.
fn run_editor(command: &str, text: &str) -> Result<Option<String>, String> {
    let mut parts = command.split(' ').filter(|part| !part.is_empty());
    let Some(program) = parts.next() else {
        return Err(NO_EDITOR.to_owned());
    };
    let (path, mut file) = create_prompt_file()
        .map_err(|error| format!("cannot create a file for the prompt: {error}"))?;
    let outcome = file
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())
        .and_then(|()| {
            drop(file);
            let status = Command::new(program)
                .args(parts)
                .arg(&path)
                .status()
                .map_err(|error| format!("cannot start {program}: {error}"))?;
            if !status.success() {
                return Ok(None);
            }
            fs::read_to_string(&path)
                .map(Some)
                .map_err(|error| error.to_string())
        });
    let _ = fs::remove_file(&path);
    outcome.map(|edited| edited.map(|text| text.strip_suffix('\n').unwrap_or(&text).to_owned()))
}

/// Creates a new file in the temporary directory; an existing file is never opened.
fn create_prompt_file() -> io::Result<(PathBuf, File)> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut attempt = 0;
    loop {
        let path = std::env::temp_dir().join(format!(
            "scramjet-rs-prompt-{}-{nanos}-{attempt}.md",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

/// As Scramjet reports a Ctrl+T.
pub(super) fn thinking_note(hidden: bool) -> &'static str {
    match hidden {
        true => "Thinking blocks: hidden",
        false => "Thinking blocks: visible",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    impl ByteSource for VecDeque<u8> {
        fn byte(&mut self, _wait: Option<Duration>) -> io::Result<Option<u8>> {
            Ok(self.pop_front())
        }
    }

    fn keys(bytes: &[u8]) -> Vec<Key> {
        let mut source: VecDeque<u8> = bytes.iter().copied().collect();
        let mut keys = Vec::new();
        loop {
            match read_key(&mut source, None).unwrap() {
                Key::Idle => return keys,
                key => keys.push(key),
            }
        }
    }

    fn edit(text: &str, cursor: usize, input: &[u8]) -> (String, usize) {
        let mut buffer = Buffer {
            text: text.into(),
            cursor,
        };
        for key in keys(input) {
            buffer.apply(&key);
        }
        (buffer.text, buffer.cursor)
    }

    #[test]
    fn decodes_keys_terminals_send() {
        assert_eq!(
            keys("aé界\r\n\x1b\r\x1b[13;2u\x1b[27;2;13~".as_bytes()),
            [
                Key::Text("a".into()),
                Key::Text("é".into()),
                Key::Text("界".into()),
                Key::Enter,
                Key::NewLine,
                Key::NewLine,
                Key::NewLine,
                Key::NewLine,
            ]
        );
        assert_eq!(
            keys(b"\x1b[D\x1bOC\x1b[1;5D\x1b[1;3C\x1bb\x1b[H\x1b[4~\x1b[3~\x1b[3;5~\x1b\x7f\x17"),
            [
                Key::Left,
                Key::Right,
                Key::WordLeft,
                Key::WordRight,
                Key::WordLeft,
                Key::LineStart,
                Key::LineEnd,
                Key::Delete,
                Key::DeleteWordForward,
                Key::DeleteWordBack,
                Key::DeleteWordBack,
            ]
        );
        assert_eq!(
            keys(b"\x03\x04\x14\x1b\x1b[D\x1b[99;5u\x1b[5~\x1b"),
            [
                Key::Interrupt,
                Key::EndOfInput,
                Key::ToggleThinking,
                Key::Left,
                Key::Ignored,
                Key::Ignored,
                Key::Ignored
            ]
        );
    }

    #[test]
    fn ctrl_g_opens_the_external_editor_and_ctrl_z_suspends() {
        assert_eq!(
            keys(b"ab\x07\x1a"),
            [
                Key::Text("a".into()),
                Key::Text("b".into()),
                Key::External,
                Key::Suspend
            ]
        );
    }

    #[test]
    fn paste_is_inserted_literally_without_submitting() {
        assert_eq!(
            keys(b"\x1b[200~one\r\ntwo\rthree\x03\x1b[A\tend\x1b[201~\r"),
            [Key::Paste("one\ntwo\nthree[A\tend".into()), Key::Enter]
        );
        assert_eq!(
            edit("ab", 1, b"\x1b[200~x\ny\x1b[201~"),
            ("ax\nyb".into(), 4)
        );
    }

    #[test]
    fn edits_by_grapheme_word_and_line() {
        let family = "👨‍👩‍👧";
        let text = format!("a{family}é");
        assert_eq!(edit(&text, text.len(), b"\x7f\x7f"), ("a".into(), 1));
        assert_eq!(
            edit(&text, 1, b"\x1b[C\x1b[C\x7f"),
            (format!("a{family}"), 1 + family.len())
        );
        assert_eq!(
            edit("cargo test  --all", 17, b"\x17"),
            ("cargo test  ".into(), 12)
        );
        assert_eq!(
            edit("cargo test  --all", 17, b"\x17\x17"),
            ("cargo ".into(), 6)
        );
        assert_eq!(edit("one two", 0, b"\x1bd"), (" two".into(), 0));
        assert_eq!(edit("one\ntwo", 5, b"\x15"), ("one\nwo".into(), 4));
        assert_eq!(edit("one\ntwo", 4, b"\x15"), ("onetwo".into(), 3));
        assert_eq!(edit("one\ntwo", 1, b"\x0b"), ("o\ntwo".into(), 1));
        assert_eq!(edit("one\ntwo", 5, b"\x01\x02\x05"), ("one\ntwo".into(), 3));
    }

    #[test]
    fn layout_wraps_by_display_width_and_places_the_cursor() {
        let layout = Layout::new("abcdefg\n界界界", 7);
        assert_eq!(layout.rows, ["> abcd", "  efg", ". 界界", "  界"]);
        let cursor = layout.at(7);
        assert_eq!((cursor.row, cursor.column), (1, 5));
        let wide = layout.at(8 + 6);
        assert_eq!((wide.row, wide.column), (3, 2));
        let exact = Layout::new("abcd", 7);
        assert_eq!(exact.rows, ["> abcd"]);
        assert_eq!((exact.at(4).row, exact.at(4).column), (0, 6));
        assert_eq!(Layout::new("a\tb", 80).rows, ["> a    b"]);
    }

    #[test]
    fn layout_wraps_between_words() {
        let at = |layout: &Layout, offset| {
            let stop = layout.at(offset);
            (stop.row, stop.column)
        };
        let layout = Layout::new("hello world again", 10);
        assert_eq!(layout.rows, ["> hello ", "  world ", "  again"]);
        assert_eq!(at(&layout, 6), (1, 2));
        assert_eq!(at(&layout, 5), (0, 7));
        // A space reaching the last free column hangs there; the next word starts a row.
        let hung = Layout::new("one two three", 10);
        assert_eq!(hung.rows, ["> one two", "  three"]);
        assert_eq!(at(&hung, 7), (0, 9));
        assert_eq!(at(&hung, 8), (1, 2));
        let typed = Layout::new("one two ", 10);
        assert_eq!(typed.rows, ["> one two"]);
        assert_eq!(at(&typed, 8), (0, 9));
        // A word longer than a row starts a new row and breaks at its edge.
        assert_eq!(
            Layout::new("a bcdefghij", 8).rows,
            ["> a ", "  bcdef", "  ghij"]
        );
        // Indentation is not left alone on a row.
        assert_eq!(Layout::new("   abcdef", 8).rows, [">    ab", "  cdef"]);
        assert_eq!(
            Layout::new("ab 界界\nx y", 8).rows,
            ["> ab ", "  界界", ". x y"]
        );
    }

    #[test]
    fn vertical_moves_follow_screen_rows_then_reach_history() {
        let text = "abcdef\nxy";
        let layout = Layout::new(text, 7);
        assert_eq!(layout.vertical(9, true), Some(6));
        assert_eq!(layout.vertical(6, true), Some(2));
        assert_eq!(layout.vertical(2, true), None);
        assert_eq!(layout.vertical(1, false), Some(5));
        assert_eq!(layout.vertical(9, false), None);
    }
}
