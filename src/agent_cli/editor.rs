//! The prompt editor of the full screen: keys read from the terminal, editing, completion and
//! history.

use super::file_search;
use super::text::{self, FROM, JOINED, SPACE};
use rustix::termios::{self, OptionalActions, Termios};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How long the rest of an escape sequence may take to arrive before Escape counts as a lone key.
pub(super) const ESCAPE_WAIT: Duration = Duration::from_millis(30);
/// How often an idle editor checks for a terminal resize.
pub(super) const RESIZE_CHECK: Duration = Duration::from_millis(250);
/// The longest time between the two Escapes that open `/tree`, as in Scramjet.
const DOUBLE_ESCAPE: Duration = Duration::from_millis(500);
const TAB: &str = "    ";
const INDENT: usize = 2;

#[derive(Debug, PartialEq)]
pub(super) enum Key {
    Text(String),
    Paste(String),
    Enter,
    NewLine,
    /// A new line in a dialog's text; at the prompt the full screen sends with it, or queues a
    /// follow-up during a turn.
    AltEnter,
    Backspace,
    Delete,
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    /// Puts the messages queued during a turn back in the editor; Up in a dialog's text; moves
    /// a model up in `/scoped-models`.
    AltUp,
    /// Down, but in `/scoped-models`, where it moves a model down.
    AltDown,
    PageUp,
    PageDown,
    LineStart,
    /// Ctrl+A: as Home (`LineStart`), but in `/scoped-models`, where it switches on all models.
    CtrlA,
    LineEnd,
    DeleteWordBack,
    DeleteWordForward,
    KillStart,
    KillEnd,
    Interrupt,
    EndOfInput,
    /// Ctrl+V, Scramjet's key to paste an image, or Alt+V, which still reaches the program in
    /// a terminal that takes Ctrl+V to paste text, as Windows Terminal does.
    PasteImage,
    ToggleThinking,
    /// Shift+Tab, `ESC [ Z`.
    BackTab,
    /// Ctrl+P, Scramjet's key to switch to the next model; `false`: Shift+Ctrl+P, to the one
    /// before, which only a terminal reporting modified keys tells apart.
    CycleModel(bool),
    External,
    /// Ctrl+O, Scramjet's key to expand tool output.
    Expand,
    /// Ctrl+R, Scramjet's key to rename a session in `/resume`.
    Rename,
    /// Ctrl+S, Scramjet's key to save the models chosen in `/scoped-models`.
    Save,
    /// Ctrl+X, Scramjet's key to switch off all models in `/scoped-models`.
    ClearAll,
    Suspend,
    /// A lone Escape.
    Escape,
    /// A mouse report, which only the full screen asks the terminal for.
    Mouse(Mouse),
    Idle,
    Ignored,
}

/// An SGR mouse report (`ESC [ < button ; column ; row M`, `m` on release).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mouse {
    /// The button and modifier bits: 0 left, 2 right, 32 a move with the left button held, 64
    /// and 65 the wheel up and down.
    pub(super) button: u16,
    /// Where, from 0.
    pub(super) column: usize,
    pub(super) row: usize,
    pub(super) release: bool,
}

pub(super) trait ByteSource {
    /// The next input byte; `None` when `wait` elapses first. `wait: None` blocks.
    fn byte(&mut self, wait: Option<Duration>) -> io::Result<Option<u8>>;
}

pub(super) fn read_key(source: &mut impl ByteSource, wait: Option<Duration>) -> io::Result<Key> {
    let Some(byte) = source.byte(wait)? else {
        return Ok(Key::Idle);
    };
    Ok(match byte {
        0x1b => escape(source)?,
        b'\r' => Key::Enter,
        b'\n' => Key::NewLine,
        b'\t' => Key::Text("\t".into()),
        0x7f | 8 => Key::Backspace,
        1 => Key::CtrlA,
        2 => Key::Left,
        3 => Key::Interrupt,
        4 => Key::EndOfInput,
        5 => Key::LineEnd,
        6 => Key::Right,
        7 => Key::External,
        11 => Key::KillEnd,
        14 => Key::Down,
        15 => Key::Expand,
        16 => Key::CycleModel(true),
        18 => Key::Rename,
        19 => Key::Save,
        20 => Key::ToggleThinking,
        21 => Key::KillStart,
        22 => Key::PasteImage,
        23 => Key::DeleteWordBack,
        24 => Key::ClearAll,
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
    let mut alt = false;
    while next == Some(0x1b) {
        alt = true;
        next = source.byte(Some(ESCAPE_WAIT))?;
    }
    Ok(match next {
        None => Key::Escape,
        Some(b'[') => match csi(source)? {
            Key::Up if alt => Key::AltUp,
            Key::Down if alt => Key::AltDown,
            key => key,
        },
        Some(b'O') => match source.byte(Some(ESCAPE_WAIT))? {
            Some(b'A') => Key::Up,
            Some(b'B') => Key::Down,
            Some(b'C') => Key::Right,
            Some(b'D') => Key::Left,
            Some(b'H') => Key::LineStart,
            Some(b'F') => Key::LineEnd,
            _ => Key::Ignored,
        },
        Some(b'\r') => Key::AltEnter,
        Some(b'\n') => Key::NewLine,
        Some(0x7f | 8) => Key::DeleteWordBack,
        Some(b'b') => Key::WordLeft,
        Some(b'f') => Key::WordRight,
        Some(b'd') => Key::DeleteWordForward,
        Some(b'v') => Key::PasteImage,
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
    if let Some(report) = parameters.strip_prefix('<') {
        return Ok(mouse(report, last).map_or(Key::Ignored, Key::Mouse));
    }
    // A report in the older X10 form, from a terminal without SGR reports: three bytes follow.
    if (last, parameters.as_ref()) == (b'M', "") {
        for _ in 0..3 {
            source.byte(Some(ESCAPE_WAIT))?;
        }
        return Ok(Key::Ignored);
    }
    let modifier = parameters.split(';').nth(1);
    let word = matches!(modifier, Some("3" | "5"));
    Ok(match (last, parameters.as_ref()) {
        (b'A', "1;3") => Key::AltUp,
        (b'B', "1;3") => Key::AltDown,
        (b'A', _) => Key::Up,
        (b'B', _) => Key::Down,
        (b'C', _) if word => Key::WordRight,
        (b'D', _) if word => Key::WordLeft,
        (b'C', _) => Key::Right,
        (b'D', _) => Key::Left,
        (b'H', _) | (b'~', "1" | "7") => Key::LineStart,
        (b'F', _) | (b'~', "4" | "8") => Key::LineEnd,
        (b'~', "3") => Key::Delete,
        (b'~', "5") => Key::PageUp,
        (b'~', "6") => Key::PageDown,
        (b'~', "3;3" | "3;5") => Key::DeleteWordForward,
        (b'~', "200") => paste(source)?,
        (b'~', "27;2;13") | (b'u', "13;2") => Key::NewLine,
        (b'~', "27;3;13") | (b'u', "13;3") => Key::AltEnter,
        (b'u', "13") => Key::Enter,
        (b'Z', _) | (b'~', "27;2;9") | (b'u', "9;2") => Key::BackTab,
        (b'~', "27;5;112") | (b'u', "112;5") => Key::CycleModel(true),
        (b'~', "27;6;112" | "27;6;80") | (b'u', "112;6") => Key::CycleModel(false),
        _ => Key::Ignored,
    })
}

fn mouse(report: &str, last: u8) -> Option<Mouse> {
    let mut numbers = report.split(';').map(str::parse::<usize>);
    let (Some(Ok(button)), Some(Ok(column)), Some(Ok(row)), None) = (
        numbers.next(),
        numbers.next(),
        numbers.next(),
        numbers.next(),
    ) else {
        return None;
    };
    Some(Mouse {
        button: u16::try_from(button).ok()?,
        column: column.checked_sub(1)?,
        row: row.checked_sub(1)?,
        release: match last {
            b'M' => false,
            b'm' => true,
            _ => return None,
        },
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
pub(super) fn clean(text: &str) -> String {
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
            Key::NewLine | Key::AltEnter => self.insert("\n"),
            Key::Backspace => self.remove(self.previous(), self.cursor),
            Key::Delete => {
                let end = self.next();
                self.remove(self.cursor, end);
            }
            Key::Left => self.cursor = self.previous(),
            Key::Right => self.cursor = self.next(),
            Key::WordLeft => self.cursor = self.word_start(),
            Key::WordRight => self.cursor = self.word_end(),
            Key::LineStart | Key::CtrlA => self.cursor = self.line_start(),
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
    fn new(text: &str, width: usize) -> Self {
        Self::marked(text, width, false)
    }

    /// Wraps lines between words where it can. As in Scramjet's editor the last column stays
    /// free for the cursor; spaces that reach it hang there instead of starting a row. With
    /// `marked`, the rows are marked for a copy (`text::FROM`).
    fn marked(text: &str, width: usize, marked: bool) -> Self {
        let gutter = |first: &str| match marked {
            true => format!("{first}{FROM}"),
            false => first.to_owned(),
        };
        // A row that continues a line, in place of `hung` spaces that hung.
        let continued = |hung: usize| match marked {
            true => format!(
                "{}{JOINED}{}",
                " ".repeat(INDENT),
                SPACE.to_string().repeat(hung)
            ),
            false => " ".repeat(INDENT),
        };
        let limit = width.max(INDENT + 3) - 1;
        let mut rows = vec![gutter("> ")];
        let mut stops = Vec::new();
        let mut offset = 0;
        for (number, line) in text.split('\n').enumerate() {
            if number > 0 {
                rows.push(gutter(". "));
            }
            let mut column = INDENT;
            let mut row_has_text = false;
            let mut after_space = false;
            let mut hanging = false;
            let mut hung = 0;
            let mut word: Option<WordStart> = None;
            for (index, grapheme) in line.grapheme_indices(true) {
                let escaped: String;
                // Text the user did not type, such as a resumed or dispatched prompt, may hold
                // escape sequences; a control character is a grapheme of its own.
                let shown = if grapheme == "\t" {
                    TAB
                } else if grapheme.chars().any(char::is_control) {
                    escaped = grapheme.escape_default().to_string();
                    &escaped
                } else {
                    grapheme
                };
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
                    if hanging {
                        hung += 1;
                    } else {
                        rows[row].push_str(shown);
                        column += cells;
                    }
                    continue;
                }
                let starts_word = std::mem::take(&mut after_space);
                if hanging {
                    rows.push(continued(std::mem::take(&mut hung)));
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
                    rows.push(continued(0) + &tail);
                    if column + cells > limit && column > INDENT {
                        rows.push(continued(0));
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

pub(super) struct RawMode {
    original: Termios,
}

impl RawMode {
    pub(super) fn enter() -> io::Result<Self> {
        let original = termios::tcgetattr(io::stdin())?;
        let mode = Self { original };
        mode.resume()?;
        Ok(mode)
    }

    /// Raw mode with bracketed paste on.
    pub(super) fn resume(&self) -> io::Result<()> {
        let mut raw = self.original.clone();
        raw.make_raw();
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &raw)?;
        eprint!("\x1b[?2004h");
        Ok(())
    }

    /// The terminal as the shell left it, for an external editor or the shell itself.
    pub(super) fn suspend(&self) {
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

/// The rows a prompt takes on screen as the editor shows it, wrapped to `width`, `marked` for
/// a copy (`text::FROM`) if asked.
pub(super) fn prompt_rows(text: &str, width: usize, marked: bool) -> Vec<String> {
    Layout::marked(text, width, marked).rows
}

pub(super) fn terminal_size() -> (usize, usize) {
    termios::tcgetwinsize(io::stderr())
        .ok()
        .filter(|size| size.ws_col > 0 && size.ws_row > 0)
        .map_or((80, 24), |size| {
            (usize::from(size.ws_col), usize::from(size.ws_row))
        })
}

/// Scramjet's commands that this agent has, in Scramjet's order, with its descriptions.
const SLASH_COMMANDS: [(&str, &str); 16] = [
    ("settings", "Open settings menu"),
    ("model", "Select model (opens selector UI)"),
    ("scoped-models", "Enable/disable models for Ctrl+P cycling"),
    ("copy", "Copy last agent message to clipboard"),
    ("name", "Set session display name"),
    ("session", "Show session info and stats"),
    ("hotkeys", "Show all keyboard shortcuts"),
    ("fork", "Create a new fork from a previous user message"),
    (
        "clone",
        "Duplicate the current session at the current position",
    ),
    ("tree", "Navigate session tree (switch branches)"),
    ("new", "Start a new session"),
    ("compact", "Manually compact the session context"),
    ("resume", "Resume a different session"),
    (
        "reload",
        "Reload settings, command sets, agents, skills and context files",
    ),
    (
        "autopilot",
        "Toggle command auto-continuation: /autopilot on|off|status",
    ),
    ("quit", "Quit scramjet-rs"),
];

/// Rows of the completion list, as Scramjet's autocomplete shows at most.
const COMPLETION_ROWS: usize = 5;

pub(super) struct Suggestion {
    /// What replaces the completed text.
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Slash,
    /// A built-in command's argument.
    Argument,
    Path,
    /// An `@` file reference.
    File,
}

/// The list under the prompt that completes a slash command or its argument, a path or an `@`
/// file reference, as Scramjet's editor does. It replaces the text from `start` to the cursor.
struct Completion {
    start: usize,
    kind: Kind,
    items: Vec<Suggestion>,
    selected: usize,
}

/// An `@` search started for the prompt's text and cursor as they were then.
struct Lookup {
    search: file_search::Search,
    text: String,
    cursor: usize,
    /// Started by Tab, which puts in a single entry found at once.
    tab: bool,
}

impl Lookup {
    fn start(
        root: &Path,
        mention: file_search::Mention,
        buffer: &Buffer,
        tab: bool,
    ) -> Option<Self> {
        Some(Self {
            search: file_search::Search::start(root, mention)?,
            text: buffer.text.clone(),
            cursor: buffer.cursor,
            tab,
        })
    }
}

impl Completion {
    /// The commands whose names fuzzily match what follows a `/` that begins the prompt: the
    /// built-in ones, then `loaded` (names and descriptions).
    fn slash(before: &str, loaded: &[(String, String)]) -> Option<Self> {
        let typed = before.strip_prefix('/')?;
        if typed.contains(char::is_whitespace) {
            return None;
        }
        let builtin = SLASH_COMMANDS
            .iter()
            .map(|&(name, description)| (name, description));
        let loaded = loaded
            .iter()
            .map(|(name, description)| (&**name, &**description));
        let commands: Vec<_> = builtin.chain(loaded).collect();
        let names: Vec<_> = commands
            .iter()
            .map(|(name, _)| super::fuzzy::Haystack::new(name))
            .collect();
        let items = super::fuzzy::filter(&super::fuzzy::parse(typed), &names)
            .into_iter()
            .map(|index| {
                let (name, description) = commands[index];
                Suggestion {
                    value: format!("/{name} "),
                    label: format!("/{name}"),
                    description: description.to_owned(),
                }
            })
            .collect();
        Self::new(0, Kind::Slash, items)
    }

    /// The argument of a built-in command that Scramjet completes: after `/model `, the
    /// endpoint's `models` that match it fuzzily; after `/autopilot `, the words starting with it.
    fn argument(before: &str, models: &[String]) -> Option<Self> {
        let (name, typed) = before.strip_prefix('/')?.split_once(' ')?;
        if typed.contains('\n') {
            return None;
        }
        let words: Vec<&str> = match name {
            "autopilot" => ["on", "off", "status"]
                .into_iter()
                .filter(|word| word.starts_with(typed))
                .collect(),
            "model" => {
                let ids: Vec<_> = models
                    .iter()
                    .map(|id| super::fuzzy::Haystack::new(id))
                    .collect();
                super::fuzzy::filter(&super::fuzzy::parse(typed), &ids)
                    .into_iter()
                    .map(|index| &*models[index])
                    .collect()
            }
            _ => return None,
        };
        let items = (words.into_iter())
            .map(|word| Suggestion {
                value: word.to_owned(),
                label: word.to_owned(),
                description: String::new(),
            })
            .collect();
        let mut list = Self::new(before.len() - typed.len(), Kind::Argument, items)?;
        list.select_best(typed);
        Some(list)
    }

    /// The entries an `@` search found for the text from `start` to the cursor.
    fn found(start: usize, items: Vec<Suggestion>, buffer: &Buffer) -> Option<Self> {
        let mut list = Self::new(start, Kind::File, items)?;
        list.select_best(&buffer.text[start..buffer.cursor]);
        Some(list)
    }

    /// Selects the entry that is `typed` or else the first that starts with it, as Scramjet
    /// selects its best match.
    fn select_best(&mut self, typed: &str) {
        let best = (self.items.iter().position(|item| item.value == typed)).or_else(|| {
            self.items
                .iter()
                .position(|item| item.value.starts_with(typed))
        });
        self.selected = best.unwrap_or(0);
    }

    /// The entries of the directory named by the word before the cursor whose names start with
    /// the rest of it, as Scramjet's `getFileSuggestions` lists them: relative to `root`, `~/` in
    /// the home directory, ignoring case, directories first. The word is all that follows a
    /// quote left open where a word starts, and an entry with a space or after it is quoted.
    fn path(before: &str, root: &Path) -> Option<Self> {
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        let line = &before[line_start..];
        let quote = file_search::open_quote(line)
            .filter(|&at| at == 0 || line[..at].ends_with(file_search::DELIMITERS));
        let start = line_start
            + quote.unwrap_or_else(|| line.rfind(file_search::DELIMITERS).map_or(0, |at| at + 1));
        let word = &before[start + usize::from(quote.is_some())..];
        let (shown, name) = match word.rfind('/') {
            Some(slash) => word.split_at(slash + 1),
            None if word == "~" => ("~/", ""),
            None => ("", word),
        };
        let directory = match shown.strip_prefix("~/") {
            Some(rest) => std::env::home_dir()?.join(rest),
            None => root.join(shown),
        };
        let name = name.to_lowercase();
        let mut items: Vec<(bool, Suggestion)> = fs::read_dir(directory)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file = entry.file_name().into_string().ok()?;
                if !file.to_lowercase().starts_with(&name) {
                    return None;
                }
                // A link to a directory counts as one.
                let kind = entry.file_type().ok()?;
                let directory = kind.is_dir() || kind.is_symlink() && entry.path().is_dir();
                let label = if directory { format!("{file}/") } else { file };
                let value = format!("{shown}{label}");
                let value = match quote.is_some() || value.contains(' ') {
                    true => format!("\"{value}\""),
                    false => value,
                };
                Some((
                    directory,
                    Suggestion {
                        value,
                        label,
                        description: String::new(),
                    },
                ))
            })
            .collect();
        items.sort_by_cached_key(|(directory, item)| (!directory, item.label.to_lowercase()));
        Self::new(
            start,
            Kind::Path,
            items.into_iter().map(|(_, item)| item).collect(),
        )
    }

    fn new(start: usize, kind: Kind, items: Vec<Suggestion>) -> Option<Self> {
        (!items.is_empty()).then_some(Self {
            start,
            kind,
            items,
            selected: 0,
        })
    }

    fn select(&mut self, up: bool) {
        let count = self.items.len();
        self.selected = (self.selected + if up { count - 1 } else { 1 }) % count;
    }

    fn apply(self, buffer: &mut Buffer) {
        let item = &self.items[self.selected];
        let value = &item.value;
        if matches!(self.kind, Kind::Slash | Kind::Argument) {
            buffer.text.replace_range(self.start..buffer.cursor, value);
            buffer.cursor = self.start + value.len();
            return;
        }
        // As Scramjet: an `@` file is followed by a space, a directory not, so that its
        // entries can be completed next, inside its quotes when quoted, and the quote after the
        // cursor is the one a quoted entry closes.
        let directory = item.label.ends_with('/');
        let closed = value.ends_with('"');
        let typed = &buffer.text[self.start..buffer.cursor];
        let mut end = buffer.cursor;
        if closed
            && (typed.starts_with('"') || typed.starts_with("@\""))
            && buffer.text[end..].starts_with('"')
        {
            end += 1;
        }
        let suffix = if self.kind == Kind::File && !directory {
            " "
        } else {
            ""
        };
        buffer
            .text
            .replace_range(self.start..end, &format!("{value}{suffix}"));
        buffer.cursor = self.start + value.len() + suffix.len() - usize::from(directory && closed);
    }

    /// The rows under the prompt, as Scramjet's select list draws them: the selected one
    /// highlighted with `→ `, descriptions in a column, and the position when some are hidden.
    fn rows(&self, width: usize) -> Vec<String> {
        use super::style::{COMPLETION_SELECTED, MUTED, RESET};
        let width = width.saturating_sub(1);
        let count = self.items.len();
        let visible = count.min(COMPLETION_ROWS);
        let top = self
            .selected
            .saturating_sub(visible / 2)
            .min(count - visible);
        // Scramjet's commands' column fits their names; other lists' is 32 columns wide.
        let column = match self.kind {
            Kind::Slash => self
                .items
                .iter()
                .map(|item| text::visible(&item.label).width() + 2)
                .max(),
            _ => None,
        };
        let column = column.map_or(32, |column| column.clamp(12, 32));
        let mut rows: Vec<String> = (top..top + visible)
            .map(|index| {
                let item = &self.items[index];
                let room = width.saturating_sub(2 + column);
                let described = !item.description.is_empty() && room > 10;
                // As Scramjet's, a name before a description keeps 2 cells of its column free.
                let label_room = match described {
                    true => column - 2,
                    false => width.saturating_sub(2),
                };
                // File names and command descriptions come from the project.
                let label = text::clip(&text::visible(&item.label), label_room);
                let description = if !described {
                    String::new()
                } else {
                    let pad = " ".repeat(column.saturating_sub(label.width()));
                    format!(
                        "{pad}{}",
                        text::clip(&text::visible(&item.description), room)
                    )
                };
                if index == self.selected {
                    format!("{COMPLETION_SELECTED}→ {label}{description}{RESET}")
                } else if description.is_empty() {
                    format!("  {label}")
                } else {
                    format!("  {label}{MUTED}{description}{RESET}")
                }
            })
            .collect();
        if visible < count {
            let position = format!("  ({}/{count})", self.selected + 1);
            rows.push(format!("{MUTED}{}{RESET}", text::clip(&position, width)));
        }
        rows
    }
}

pub struct Editor {
    history: Vec<String>,
    hide_thinking: Arc<AtomicBool>,
    /// Where Tab completes relative paths from.
    root: PathBuf,
    /// The commands listed after the built-in ones: names and descriptions.
    commands: Vec<(String, String)>,
    /// The endpoint's models, which complete `/model`'s argument.
    models: Vec<String>,
    /// Whether Escape twice on an empty prompt returns `Action::DoubleEscape`.
    double_escape: bool,
    /// Edits one text for a question, as Scramjet's dialog editor: Escape or Ctrl-C cancels.
    dialog: bool,
}

impl Editor {
    /// Ctrl+T flips `hide_thinking`.
    pub fn new(
        history: Vec<String>,
        hide_thinking: Arc<AtomicBool>,
        root: PathBuf,
        commands: Vec<(String, String)>,
        models: Vec<String>,
        double_escape: bool,
    ) -> Self {
        Self {
            history,
            hide_thinking,
            root,
            commands,
            models,
            double_escape,
            dialog: false,
        }
    }

    /// An editor for one answer without history, as `dialog` says.
    pub fn dialog(hide_thinking: Arc<AtomicBool>) -> Self {
        Self {
            dialog: true,
            ..Self::new(
                Vec::new(),
                hide_thinking,
                PathBuf::new(),
                Vec::new(),
                Vec::new(),
                false,
            )
        }
    }

    pub fn set_models(&mut self, models: Vec<String>) {
        self.models = models;
    }

    /// Lists `commands` after the built-in ones, and makes Escape twice on an empty prompt
    /// return `Action::DoubleEscape` when `double_escape`.
    pub fn reloaded(&mut self, commands: Vec<(String, String)>, double_escape: bool) {
        self.commands = commands;
        self.double_escape = double_escape;
    }

    /// Makes `prompt` the most recent entry that Up recalls.
    pub fn remember(&mut self, prompt: &str) {
        if self.history.last().is_none_or(|last| last != prompt) {
            self.history.push(prompt.to_owned());
        }
    }

    /// A draft for a new prompt that starts with `text`, the cursor at its end.
    pub(super) fn draft(&self, text: String) -> Draft {
        let mut buffer = Buffer::default();
        buffer.set(text);
        Draft {
            buffer,
            ..Draft::default()
        }
    }

    /// Edits `draft` with `key`, or says what the caller should do for it; `pending`: more keys
    /// are already typed, so an `@` search waits for them.
    pub(super) fn key(&self, draft: &mut Draft, key: Key, pending: bool) -> Action {
        let key = match key {
            Key::AltEnter => Key::NewLine,
            Key::AltUp => Key::Up,
            Key::AltDown => Key::Down,
            key => key,
        };
        let Draft {
            buffer,
            browsing,
            saved,
            exit_armed,
            last_escape,
            note,
            completion,
            lookup,
        } = draft;
        if key == Key::Escape {
            *lookup = None;
        }
        let armed = std::mem::take(exit_armed);
        let noted = note.take();
        let tab = matches!(&key, Key::Text(text) if text == "\t");
        let changed = match key {
            Key::Idle | Key::Suspend => {
                *exit_armed = armed;
                *note = noted;
                return match key {
                    Key::Suspend => Action::Suspend,
                    _ => Action::Edited(false),
                };
            }
            Key::Escape if completion.is_some() => {
                *completion = None;
                true
            }
            Key::Up | Key::Down if completion.is_some() => {
                if let Some(list) = completion {
                    list.select(key == Key::Up);
                }
                true
            }
            // As in Scramjet, Enter on a command also submits it, and on an argument does not,
            // but on one already typed in full, which Scramjet puts in again for a second Enter.
            Key::Text(_) | Key::Enter if (tab || key == Key::Enter) && completion.is_some() => {
                let list = completion.take().expect("checked above");
                let typed = &buffer.text[list.start..buffer.cursor];
                let submit = key == Key::Enter
                    && match list.kind {
                        Kind::Slash => true,
                        Kind::Argument => list.items[list.selected].value == typed,
                        Kind::Path | Kind::File => false,
                    };
                list.apply(buffer);
                if submit {
                    buffer.set(buffer.text.trim().to_owned());
                    return Action::Submit(buffer.text.clone());
                }
                true
            }
            Key::Escape | Key::Interrupt if self.dialog => return Action::Exit,
            Key::EndOfInput if self.dialog && buffer.text.is_empty() => return Action::Exit,
            Key::Escape if self.double_escape && buffer.text.trim().is_empty() => {
                if last_escape.is_some_and(|at| at.elapsed() < DOUBLE_ESCAPE) {
                    *last_escape = None;
                    return Action::DoubleEscape;
                }
                *last_escape = Some(Instant::now());
                *exit_armed = armed;
                *note = noted;
                false
            }
            Key::BackTab if !self.dialog => return Action::CycleEffort,
            Key::CycleModel(forward) if !self.dialog => {
                return Action::CycleModel(if forward { 1 } else { -1 });
            }
            Key::Ignored
            | Key::Mouse(_)
            | Key::BackTab
            | Key::CycleModel(_)
            | Key::Save
            | Key::ClearAll
            | Key::Expand
            | Key::Escape
            | Key::PageUp
            | Key::PageDown => {
                *exit_armed = armed;
                *note = noted;
                false
            }
            Key::ToggleThinking => {
                let hidden = !self.hide_thinking.fetch_xor(true, Ordering::Relaxed);
                *note = Some(thinking_note(hidden).to_owned());
                true
            }
            Key::External => {
                *completion = None;
                return Action::External;
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
                return Action::Submit(buffer.text.clone());
            }
            Key::Interrupt if buffer.text.is_empty() => {
                if armed {
                    return Action::Exit;
                }
                *exit_armed = true;
                true
            }
            Key::Interrupt => {
                *buffer = Buffer::default();
                *completion = None;
                *browsing = None;
                true
            }
            Key::EndOfInput if buffer.text.is_empty() => return Action::Exit,
            // Scramjet completes only commands and their arguments after a `/` that starts the
            // prompt; a path there is completed when nothing else matches. An `@` word is
            // looked for.
            Key::Text(_) if tab && !self.dialog => {
                let before = &buffer.text[..buffer.cursor];
                *completion = Completion::slash(before, &self.commands)
                    .or_else(|| Completion::argument(before, &self.models));
                match file_search::Mention::find(before) {
                    Some(mention) if completion.is_none() => {
                        *lookup = Lookup::start(&self.root, mention, buffer, true);
                    }
                    _ => {
                        *completion = completion
                            .take()
                            .or_else(|| Completion::path(before, &self.root));
                        if let Some(list) = completion
                            .take_if(|list| list.items.len() == 1 && list.kind == Kind::Path)
                        {
                            list.apply(buffer);
                        }
                    }
                }
                true
            }
            // A list follows what is typed, a path's while the same word is typed; commands
            // are listed as soon as a `/` starts the prompt, entries for an `@` word once
            // typing pauses, the list before staying until they are found.
            Key::Text(_) | Key::Backspace if !self.dialog => {
                buffer.apply(&key);
                let before = &buffer.text[..buffer.cursor];
                *completion = match (file_search::Mention::typed(before), completion.take()) {
                    (Some(mention), list) => {
                        let kept = list
                            .filter(|list| list.kind == Kind::File && list.start == mention.start);
                        if !pending {
                            *lookup = Lookup::start(&self.root, mention, buffer, false);
                        }
                        kept
                    }
                    (None, Some(list)) if list.kind == Kind::Path => {
                        Completion::path(before, &self.root).filter(|next| next.start == list.start)
                    }
                    (None, _) => Completion::slash(before, &self.commands)
                        .or_else(|| Completion::argument(before, &self.models)),
                };
                true
            }
            Key::Up | Key::Down => {
                let up = key == Key::Up;
                let layout = Layout::new(&buffer.text, terminal_size().0);
                if let Some(cursor) = layout.vertical(buffer.cursor, up) {
                    buffer.cursor = cursor;
                } else if up && !self.history.is_empty() && *browsing != Some(0) {
                    let at = match *browsing {
                        Some(at) => at - 1,
                        None => {
                            *saved = std::mem::take(&mut buffer.text);
                            self.history.len() - 1
                        }
                    };
                    *browsing = Some(at);
                    buffer.set(self.history[at].clone());
                } else if let Some(at) = browsing.filter(|_| !up) {
                    *browsing = Some(at + 1).filter(|&next| next < self.history.len());
                    buffer.set(match *browsing {
                        Some(next) => self.history[next].clone(),
                        None => std::mem::take(saved),
                    });
                }
                true
            }
            Key::EndOfInput => {
                *completion = None;
                buffer.apply(&Key::Delete);
                true
            }
            key => {
                *completion = None;
                buffer.apply(&key);
                true
            }
        };
        Action::Edited(changed)
    }

    /// Shows the entries an `@` search found, once it found them for the draft as it is;
    /// returns whether the draft changed.
    pub(super) fn poll(&self, draft: &mut Draft) -> bool {
        let Draft {
            buffer,
            completion,
            lookup,
            ..
        } = draft;
        // A search for the text as it was before an edit or a cursor move finds nothing for it
        // now.
        lookup.take_if(|lookup| lookup.text != buffer.text || lookup.cursor != buffer.cursor);
        let Some(found) = lookup.as_mut().and_then(|lookup| lookup.search.poll()) else {
            return false;
        };
        let Lookup { search, tab, .. } = lookup.take().expect("polled");
        *completion = Completion::found(search.mention.start, found, buffer);
        if let Some(list) = completion.take_if(|list| tab && list.items.len() == 1) {
            list.apply(buffer);
        }
        true
    }
}

/// A prompt being edited, and what goes with it from one key to the next.
#[derive(Default)]
pub(super) struct Draft {
    buffer: Buffer,
    /// The history entry shown; `None` for the text typed.
    browsing: Option<usize>,
    /// The text typed, while Up shows history.
    saved: String,
    exit_armed: bool,
    last_escape: Option<Instant>,
    /// Shown under the prompt until the next key.
    note: Option<String>,
    completion: Option<Completion>,
    lookup: Option<Lookup>,
}

/// What the caller does for a key the editor does not handle itself.
#[derive(Debug, PartialEq)]
pub(super) enum Action {
    /// Whether the draft changed and is drawn again.
    Edited(bool),
    /// Enter: the text, trimmed, which the draft then holds.
    Submit(String),
    /// Ctrl-C twice or Ctrl-D on an empty prompt, or a dialog cancelled.
    Exit,
    /// Escape twice on an empty prompt, which opens `/tree` or `/fork` as in Scramjet.
    DoubleEscape,
    /// Shift+Tab.
    CycleEffort,
    /// Ctrl+P (1) or Shift+Ctrl+P (-1).
    CycleModel(isize),
    /// Ctrl-Z.
    Suspend,
    /// Ctrl-G: edit the text with `$VISUAL` or `$EDITOR` (see `Draft::edited`).
    External,
}

impl Draft {
    pub(super) fn text(&self) -> &str {
        &self.buffer.text
    }

    /// Whether an `@` search runs, whose entries `Editor::poll` shows once found.
    pub(super) fn searching(&self) -> bool {
        self.lookup.is_some()
    }

    pub(super) fn completing(&self) -> bool {
        self.completion.is_some()
    }

    /// The line under the prompt: how to exit after a first Ctrl-C, or a note.
    pub(super) fn hint(&self) -> Option<&str> {
        self.exit_armed
            .then_some(EXIT_HINT)
            .or(self.note.as_deref())
    }

    pub(super) fn set_note(&mut self, note: String) {
        self.note = Some(note);
    }

    /// The completion list's rows, if one is open.
    pub(super) fn completion_rows(&self, width: usize) -> Vec<String> {
        self.completion
            .as_ref()
            .map_or_else(Vec::new, |list| list.rows(width))
    }

    /// The prompt's rows at `width`, marked for a copy (`text::FROM`), and the row and column of
    /// the cursor in them.
    pub(super) fn rows(&self, width: usize) -> (Vec<String>, usize, usize) {
        let layout = Layout::marked(&self.buffer.text, width, true);
        let cursor = layout.at(self.buffer.cursor);
        (layout.rows, cursor.row, cursor.column)
    }

    /// Takes what the external editor did: its text, or the reason it did nothing.
    pub(super) fn edited(&mut self, edited: Result<Option<String>, String>) {
        match edited {
            Ok(Some(text)) => self.buffer.set(text),
            Ok(None) => {}
            Err(error) => self.note = Some(error),
        }
    }
}

/// `$VISUAL` or `$EDITOR` run on `text` with the terminal as the shell left it: its text, or
/// `None` when it did not save; an error when none is configured or it could not run.
pub(super) fn external(text: &str) -> Result<Option<String>, String> {
    match editor_command() {
        Some(command) => run_editor(&command, text),
        None => Err(NO_EDITOR.to_owned()),
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
            keys("aé界\r\n\x1b\r\x1b[13;2u\x1b[27;2;13~\x1b[13;3u\x1b[27;3;13~".as_bytes()),
            [
                Key::Text("a".into()),
                Key::Text("é".into()),
                Key::Text("界".into()),
                Key::Enter,
                Key::NewLine,
                Key::AltEnter,
                Key::NewLine,
                Key::NewLine,
                Key::AltEnter,
                Key::AltEnter,
            ]
        );
        assert_eq!(
            keys(b"\x1b[1;3A\x1b\x1b[A\x1b[A\x1b[1;5A"),
            [Key::AltUp, Key::AltUp, Key::Up, Key::Up]
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
            keys(b"\x03\x04\x14\x16\x1bv\x1b\x1b[D\x1b[99;5u\x1b[5~\x1b[6~\x1bx\x1b"),
            [
                Key::Interrupt,
                Key::EndOfInput,
                Key::ToggleThinking,
                Key::PasteImage,
                Key::PasteImage,
                Key::Left,
                Key::Ignored,
                Key::PageUp,
                Key::PageDown,
                Key::Ignored,
                Key::Escape
            ]
        );
    }

    #[test]
    fn decodes_sgr_mouse_reports() {
        let mouse = |button, column, row, release| {
            Key::Mouse(Mouse {
                button,
                column,
                row,
                release,
            })
        };
        assert_eq!(
            keys(
                b"\x1b[<0;1;1M\x1b[<32;120;40M\x1b[<0;120;40m\x1b[<65;3;2M\x1b[<0;0;1M\x1b[<1;2M\x1b[M`!!a"
            ),
            [
                mouse(0, 0, 0, false),
                mouse(32, 119, 39, false),
                mouse(0, 119, 39, true),
                mouse(65, 2, 1, false),
                Key::Ignored,
                Key::Ignored,
                Key::Ignored,
                Key::Text("a".into()),
            ]
        );
    }

    #[test]
    fn shift_tab_is_back_tab() {
        assert_eq!(
            keys(b"\x1b[Z\x1b[27;2;9~\x1b[9;2u"),
            [Key::BackTab, Key::BackTab, Key::BackTab]
        );
    }

    #[test]
    fn ctrl_p_cycles_models_and_with_shift_back_where_the_terminal_tells_them_apart() {
        let forward = keys(b"\x10\x1b[112;5u\x1b[27;5;112~");
        assert_eq!(forward, [const { Key::CycleModel(true) }; 3]);
        let back = keys(b"\x1b[112;6u\x1b[27;6;112~\x1b[27;6;80~");
        assert_eq!(back, [const { Key::CycleModel(false) }; 3]);
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
    fn control_characters_show_escaped_and_the_cursor_counts_them() {
        let layout = Layout::new("a\x1b]52;c;eA==\x07b\r", 80);
        assert_eq!(layout.rows, ["> a\\u{1b}]52;c;eA==\\u{7}b\\r"]);
        let after = layout.at("a\x1b".len());
        assert_eq!((after.row, after.column), (0, 9));
        let end = layout.at("a\x1b]52;c;eA==\x07b\r".len());
        assert_eq!(end.column, layout.rows[0].len());
    }

    #[test]
    fn completion_rows_escape_control_characters() {
        let loaded = [("x\x1b[2J".to_owned(), "clears\x1b]0;title\x07".to_owned())];
        let list = Completion::slash("/x", &loaded).unwrap();
        assert_eq!(
            plain(list.rows(60)),
            ["→ /x\\u{1b}[2J  clears\\u{1b}]0;title\\u{7}"]
        );
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

    fn plain(rows: Vec<String>) -> Vec<String> {
        rows.iter()
            .map(|row| {
                row.replace(crate::agent_cli::style::COMPLETION_SELECTED, "")
                    .replace(crate::agent_cli::style::MUTED, "")
                    .replace(crate::agent_cli::style::RESET, "")
            })
            .collect()
    }

    fn labels(list: &Completion) -> Vec<&str> {
        list.items.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn slash_commands_filter_and_complete_with_a_trailing_space() {
        let list = Completion::slash("/", &[]).unwrap();
        assert_eq!(
            labels(&list),
            [
                "/settings",
                "/model",
                "/scoped-models",
                "/copy",
                "/name",
                "/session",
                "/hotkeys",
                "/fork",
                "/clone",
                "/tree",
                "/new",
                "/compact",
                "/resume",
                "/reload",
                "/autopilot",
                "/quit",
            ]
        );
        assert_eq!(
            plain(list.rows(60)),
            [
                "→ /settings       Open settings menu",
                "  /model          Select model (opens selector UI)",
                "  /scoped-models  Enable/disable models for Ctrl+P cycling",
                "  /copy           Copy last agent message to clipboard",
                "  /name           Set session display name",
                "  (1/16)",
            ]
        );
        // The selected row stays in the middle where it can; a narrow terminal drops descriptions.
        let mut list = list;
        for _ in 0..8 {
            list.select(false);
        }
        assert_eq!(
            plain(list.rows(24)),
            [
                "  /hotkeys",
                "  /fork",
                "→ /clone",
                "  /tree",
                "  /new",
                "  (9/16)"
            ]
        );
        for _ in 0..8 {
            list.select(false);
        }
        assert_eq!(list.selected, 0);
        list.select(true);
        assert_eq!(list.items[list.selected].label, "/quit");

        let mut buffer = Buffer {
            text: "/res rest".into(),
            cursor: 4,
        };
        let list = Completion::slash(&buffer.text[..buffer.cursor], &[]).unwrap();
        assert_eq!(labels(&list), ["/resume"]);
        list.apply(&mut buffer);
        assert_eq!((buffer.text.as_str(), buffer.cursor), ("/resume  rest", 8));

        assert!(Completion::slash("/x", &[]).is_none());
        assert!(Completion::slash("/model ", &[]).is_none());
        assert!(Completion::slash("a /model", &[]).is_none());
    }

    #[test]
    fn slash_commands_match_fuzzily_and_rank_best_matches_first() {
        let list = Completion::slash("/mdl", &[]).unwrap();
        assert_eq!(labels(&list), ["/model", "/scoped-models"]);

        let list = Completion::slash("/m", &[]).unwrap();
        assert_eq!(labels(&list)[0], "/model");
        assert!(labels(&list).contains(&"/compact"));
        assert!(Completion::slash("/zzz", &[]).is_none());
    }

    #[test]
    fn built_in_arguments_complete_as_scramjets() {
        let models = ["gpt-6-luna", "claude-haiku-5-5", "gpt-6.1-sol"].map(String::from);
        // The models in the endpoint's order, then those matching what is typed, best first.
        let list = Completion::argument("/model ", &models).unwrap();
        assert_eq!(labels(&list), models);
        assert_eq!(
            plain(list.rows(40))[..2],
            ["→ gpt-6-luna", "  claude-haiku-5-5"]
        );
        let list = Completion::argument("/model 6 sol", &models).unwrap();
        assert_eq!(labels(&list), ["gpt-6.1-sol"]);
        // Equal matches put the shorter id first, as Scramjet's `fuzzyFilter` does.
        let gpt = ["gpt-6.1-sol", "gpt-6-luna"].map(String::from);
        let list = Completion::argument("/model gpt", &gpt).unwrap();
        assert_eq!(labels(&list), ["gpt-6-luna", "gpt-6.1-sol"]);
        let list = Completion::slash("/se", &[]).unwrap();
        assert_eq!(labels(&list)[..2], ["/session", "/settings"]);
        let mut buffer = Buffer {
            text: "/model hai".into(),
            cursor: 10,
        };
        Completion::argument(&buffer.text, &models)
            .unwrap()
            .apply(&mut buffer);
        assert_eq!(
            (buffer.text.as_str(), buffer.cursor),
            ("/model claude-haiku-5-5", 23)
        );
        // `/autopilot`'s words are those that start with what is typed, as typed.
        let words = |before| Completion::argument(before, &[]).map(|list| labels(&list).join(" "));
        assert_eq!(words("/autopilot ").unwrap(), "on off status");
        assert_eq!(words("/autopilot o").unwrap(), "on off");
        assert_eq!(words("/autopilot st").unwrap(), "status");
        assert!(words("/autopilot O").is_none());
        assert!(words("/autopilot on x").is_none());
        // Nothing else completes: other commands, a later line, no models.
        assert!(Completion::argument("/name ", &models).is_none());
        assert!(Completion::argument("/model x\nhai", &models).is_none());
        assert!(Completion::argument("say /model ", &models).is_none());
        assert!(Completion::argument("/model ", &[]).is_none());
        assert!(Completion::argument("/model zzz", &models).is_none());
    }

    #[test]
    fn enter_puts_in_an_argument_without_sending_it() {
        let models = vec!["claude-haiku-5-5".to_owned(), "gpt-6-luna".to_owned()];
        let editor = Editor::new(
            Vec::new(),
            Arc::default(),
            PathBuf::new(),
            Vec::new(),
            models,
            false,
        );
        let mut draft = editor.draft("/model".into());
        for text in [" ", "luna"] {
            let key = Key::Text(text.into());
            assert_eq!(editor.key(&mut draft, key, false), Action::Edited(true));
        }
        assert_eq!(plain(draft.completion_rows(40)), ["→ gpt-6-luna"]);
        assert_eq!(
            editor.key(&mut draft, Key::Enter, false),
            Action::Edited(true)
        );
        assert_eq!(draft.text(), "/model gpt-6-luna");
        assert!(!draft.completing());
        assert_eq!(
            editor.key(&mut draft, Key::Enter, false),
            Action::Submit("/model gpt-6-luna".into())
        );
        // One typed in full is sent at once.
        let mut draft = editor.draft("/autopilot o".into());
        assert_eq!(
            editor.key(&mut draft, Key::Text("n".into()), false),
            Action::Edited(true)
        );
        assert_eq!(plain(draft.completion_rows(40)), ["→ on"]);
        assert_eq!(
            editor.key(&mut draft, Key::Enter, false),
            Action::Submit("/autopilot on".into())
        );
    }

    #[test]
    fn loaded_commands_are_listed_after_the_built_in_ones() {
        let loaded = [
            (
                "rs:loop".to_owned(),
                "[focus] — Build one increment".to_owned(),
            ),
            ("rs:review".to_owned(), "Review".to_owned()),
        ];
        let list = Completion::slash("/", &loaded).unwrap();
        assert_eq!(
            labels(&list)[SLASH_COMMANDS.len()..],
            ["/rs:loop", "/rs:review"]
        );
        let mut list = Completion::slash("/rs:", &loaded).unwrap();
        assert_eq!(labels(&list), ["/rs:loop", "/rs:review"]);
        list.select(false);
        assert_eq!(
            plain(list.rows(60)),
            [
                "  /rs:loop    [focus] — Build one increment",
                "→ /rs:review  Review",
            ]
        );
        let mut buffer = Buffer {
            text: "/rs:l".into(),
            cursor: 5,
        };
        let list = Completion::slash("/rs:l", &loaded).unwrap();
        list.apply(&mut buffer);
        assert_eq!(buffer.text, "/rs:loop ");
        // A name too long for the column is cut before its description.
        let long = [(
            "skill:writing-scramjet-commands".to_owned(),
            "Use when creating commands".to_owned(),
        )];
        let list = Completion::slash("/skill:", &long).unwrap();
        assert_eq!(
            plain(list.rows(80)),
            ["→ /skill:writing-scramjet-comma…  Use when creating commands"]
        );
        assert_eq!(plain(list.rows(40)), ["→ /skill:writing-scramjet-commands"]);
    }

    #[test]
    fn paths_complete_from_the_word_before_the_cursor() {
        let root = std::env::temp_dir().join(format!("sjrs-editor-paths-{}", std::process::id()));
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _scratch = Scratch(root.clone());
        fs::create_dir_all(root.join("Alpha-dir")).unwrap();
        for file in ["alpine.txt", "also.txt", "beta", "Alpha-dir/inner.rs"] {
            fs::write(root.join(file), "").unwrap();
        }
        std::os::unix::fs::symlink(root.join("Alpha-dir"), root.join("link")).unwrap();

        // Directories first, then by name ignoring case.
        let list = Completion::path("see AL", &root).unwrap();
        assert_eq!(labels(&list), ["Alpha-dir/", "alpine.txt", "also.txt"]);
        assert_eq!(plain(list.rows(40))[0], "→ Alpha-dir/");
        let mut buffer = Buffer {
            text: "see AL".into(),
            cursor: 6,
        };
        let mut list = list;
        list.select(false);
        list.apply(&mut buffer);
        assert_eq!(buffer.text, "see alpine.txt");

        let list = |before: &str| {
            Completion::path(before, &root).map(|list| {
                list.items
                    .into_iter()
                    .map(|item| item.value)
                    .collect::<Vec<_>>()
            })
        };
        assert_eq!(list("li").unwrap(), ["link/"]);
        // As Scramjet completes the cursor's line, a word can start one.
        assert_eq!(list("see\nli").unwrap(), ["link/"]);
        assert_eq!(list("x=./Alpha-dir/").unwrap(), ["./Alpha-dir/inner.rs"]);
        assert_eq!(list("'Alpha-dir/I").unwrap(), ["Alpha-dir/inner.rs"]);
        let absolute = format!("{}/b", root.display());
        assert_eq!(list(&absolute).unwrap(), [format!("{absolute}eta")]);
        assert_eq!(list("").unwrap().len(), 5);
        assert!(list("nothing").is_none());
        assert!(list("missing/a").is_none());

        // As Scramjet's, a path with a space is quoted, and one is completed inside an open
        // quote where a word starts, the quote after the cursor closing it.
        fs::create_dir(root.join("my dir")).unwrap();
        fs::write(root.join("my dir/a file.txt"), "").unwrap();
        fs::write(root.join("my dir/b.txt"), "").unwrap();
        assert_eq!(list("see m").unwrap(), ["\"my dir/\""]);
        assert_eq!(list("\"my d").unwrap(), ["\"my dir/\""]);
        assert_eq!(list("x \"Alpha").unwrap(), ["\"Alpha-dir/\""]);
        assert_eq!(
            list("x=\"my dir/").unwrap(),
            ["\"my dir/a file.txt\"", "\"my dir/b.txt\""]
        );
        // A quote inside a word opens none; a closed one ends the word.
        assert_eq!(list("a\"Alpha").unwrap(), ["Alpha-dir/"]);
        assert_eq!(list("\"x\" Alpha").unwrap(), ["Alpha-dir/"]);
        // A quote on an earlier line is closed by its end.
        assert_eq!(list("\"a\nAlpha").unwrap(), ["Alpha-dir/"]);
        let tab = |text: &str, cursor: usize| {
            let mut buffer = Buffer {
                text: text.into(),
                cursor,
            };
            Completion::path(&text[..cursor], &root)
                .unwrap()
                .apply(&mut buffer);
            (buffer.text, buffer.cursor)
        };
        assert_eq!(tab("see my", 6), ("see \"my dir/\"".into(), 12));
        assert_eq!(
            tab("see \"my dir/b\" x", 13),
            ("see \"my dir/b.txt\" x".into(), 18)
        );
        assert_eq!(
            tab("\"Alpha-dir/", 11),
            ("\"Alpha-dir/inner.rs\"".into(), 20)
        );

        let home = std::env::home_dir().unwrap();
        let name = fs::read_dir(&home)
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
            .min()
            .unwrap();
        assert!(
            list("~")
                .unwrap()
                .iter()
                .all(|value| value.starts_with("~/"))
        );
        assert!(
            list(&format!("~/{name}"))
                .unwrap()
                .iter()
                .any(|value| value.starts_with(&format!("~/{name}")))
        );
    }

    #[test]
    fn file_references_select_the_best_match_and_are_put_in_as_scramjet_does() {
        let item = |value: &str, label: &str| Suggestion {
            value: value.into(),
            label: label.into(),
            description: value.trim_start_matches(['@', '"']).into(),
        };
        let apply = |text: &str, cursor: usize, start: usize, value: &str, label: &str| {
            let mut buffer = Buffer {
                text: text.into(),
                cursor,
            };
            Completion::found(start, vec![item(value, label)], &buffer)
                .unwrap()
                .apply(&mut buffer);
            (buffer.text, buffer.cursor)
        };
        // A file gets a space after it, a directory none.
        assert_eq!(
            apply("see @edi and", 8, 4, "@src/editor.rs", "editor.rs"),
            ("see @src/editor.rs  and".into(), 19)
        );
        assert_eq!(apply("@sr", 3, 0, "@src/", "src/"), ("@src/".into(), 5));
        // A quoted directory leaves the cursor inside its quotes, and a quoted entry closes the
        // quote that follows the cursor.
        assert_eq!(
            apply("@\"my", 4, 0, "@\"my dir/\"", "my dir/"),
            ("@\"my dir/\"".into(), 9)
        );
        assert_eq!(
            apply("@\"my\" x", 4, 0, "@\"my file\"", "my file"),
            ("@\"my file\"  x".into(), 11)
        );
        assert_eq!(
            apply("@my\"", 3, 0, "@\"my file\"", "my file"),
            ("@\"my file\" \"".into(), 11)
        );

        let buffer = Buffer {
            text: "@src/ed".into(),
            cursor: 7,
        };
        let items = || {
            vec![
                item("@lib/edit.rs", "edit.rs"),
                item("@src/editor.rs", "editor.rs"),
            ]
        };
        let list = Completion::found(0, items(), &buffer).unwrap();
        assert_eq!(list.selected, 1);
        assert_eq!(
            plain(list.rows(60)),
            [
                "  edit.rs                         lib/edit.rs",
                "→ editor.rs                       src/editor.rs",
            ]
        );
        let buffer = Buffer {
            text: "@xyz".into(),
            cursor: 4,
        };
        assert_eq!(Completion::found(0, items(), &buffer).unwrap().selected, 0);
        assert!(Completion::found(0, Vec::new(), &buffer).is_none());
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
