//! The prompt editor of the full screen: editing, its layout on screen, and history. Its
//! keys are read in `keys.rs`, its completion list is in `completion.rs`.

use super::completion::{Completion, Completions, Kind, Lookup};
use super::file_search;
use super::keymap::{self, Binding};
use super::keys::Key;
use super::text::{FROM, JOINED, SPACE};
use super::{console, spelling, style};
use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How often an idle editor checks for a terminal resize.
pub(super) const RESIZE_CHECK: Duration = Duration::from_millis(250);
/// The longest time between the two Escapes that open `/tree`, as in Scramjet.
const DOUBLE_ESCAPE: Duration = Duration::from_millis(500);
/// How long a note that is not an error stays, if no key comes first.
pub(super) const NOTE_TIME: Duration = Duration::from_secs(3);
const TAB: &str = "    ";
const INDENT: usize = 2;

#[derive(Clone, Default)]
pub(super) struct Buffer {
    pub(super) text: String,
    /// Byte offset, always on a grapheme boundary.
    pub(super) cursor: usize,
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
    /// Moves to the next grapheme after the cursor that starts with `target` (`forward`), or the
    /// last one before it, as Scramjet's jump to a character; stays when there is none.
    fn jump(&mut self, target: &str, forward: bool) {
        let found = match forward {
            true => {
                let from = self.next();
                (self.text[from..].grapheme_indices(true))
                    .find(|(_, grapheme)| grapheme.starts_with(target))
                    .map(|(index, _)| from + index)
            }
            false => (self.text[..self.cursor].grapheme_indices(true).rev())
                .find(|(_, grapheme)| grapheme.starts_with(target))
                .map(|(index, _)| index),
        };
        if let Some(at) = found {
            self.cursor = at;
        }
    }
    fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::Backspace => self.remove(self.previous(), self.cursor),
            Edit::Delete => {
                let end = self.next();
                self.remove(self.cursor, end);
            }
            Edit::Left => self.cursor = self.previous(),
            Edit::Right => self.cursor = self.next(),
            Edit::WordLeft => self.cursor = self.word_start(),
            Edit::WordRight => self.cursor = self.word_end(),
            Edit::LineStart => self.cursor = self.line_start(),
            Edit::LineEnd => self.cursor = self.line_end(),
            Edit::DeleteWordBack => self.remove(self.word_start(), self.cursor),
            Edit::DeleteWordForward => {
                let end = self.word_end();
                self.remove(self.cursor, end);
            }
            Edit::KillStart => match self.line_start() {
                start if start == self.cursor => self.remove(self.previous(), self.cursor),
                start => self.remove(start, self.cursor),
            },
            Edit::KillEnd => match self.line_end() {
                end if end == self.cursor => {
                    let next = self.next();
                    self.remove(self.cursor, next);
                }
                end => self.remove(self.cursor, end),
            },
        }
    }
}

/// What a key does to the text and the cursor, apart from typing.
#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Backspace,
    Delete,
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DeleteWordBack,
    DeleteWordForward,
    KillStart,
    KillEnd,
}

/// The edits' actions, the kills' last.
const EDITS: [(Binding, Edit); 12] = [
    (Binding::DeleteCharBackward, Edit::Backspace),
    (Binding::DeleteCharForward, Edit::Delete),
    (Binding::CursorLeft, Edit::Left),
    (Binding::CursorRight, Edit::Right),
    (Binding::CursorWordLeft, Edit::WordLeft),
    (Binding::CursorWordRight, Edit::WordRight),
    (Binding::CursorLineStart, Edit::LineStart),
    (Binding::CursorLineEnd, Edit::LineEnd),
    (Binding::DeleteWordBackward, Edit::DeleteWordBack),
    (Binding::DeleteWordForward, Edit::DeleteWordForward),
    (Binding::DeleteToLineStart, Edit::KillStart),
    (Binding::DeleteToLineEnd, Edit::KillEnd),
];

/// The edit `key` makes, if any.
fn edit(key: &Key) -> Option<Edit> {
    (EDITS.iter())
        .find(|&&(binding, _)| key.is(binding))
        .map(|&(_, edit)| edit)
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
        Self::marked(text, width, false, &[])
    }

    /// Wraps lines between words where it can. As in Scramjet's editor the last column stays
    /// free for the cursor; spaces that reach it hang there instead of starting a row. With
    /// `marked`, the rows are marked for a copy (`text::FROM`). The `misspelled` byte ranges,
    /// in order, are underlined.
    fn marked(text: &str, width: usize, marked: bool, misspelled: &[Range<usize>]) -> Self {
        let mut misspelled = misspelled.iter().peekable();
        let (underline, end) = (spelling_style(), style::RESET);
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
                let at = offset + index;
                while misspelled.next_if(|range| range.end <= at).is_some() {}
                let last = rows.last_mut().unwrap();
                match misspelled.peek().is_some_and(|range| range.start <= at) {
                    // Each grapheme closes its underline, so that a word moved to the next row
                    // or broken across rows carries its own; neighbors join again.
                    true => {
                        match last.ends_with(end) {
                            true => last.truncate(last.len() - end.len()),
                            false => last.push_str(&underline),
                        }
                        last.push_str(shown);
                        last.push_str(end);
                    }
                    false => last.push_str(shown),
                }
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

/// The rows a prompt takes on screen as the editor shows it, wrapped to `width`, `marked` for
/// a copy (`text::FROM`) if asked.
pub(super) fn prompt_rows(text: &str, width: usize, marked: bool) -> Vec<String> {
    Layout::marked(text, width, marked, &[]).rows
}

/// The curly underline Scramjet marks a misspelled word with, in the theme's color for it; a
/// straight one in Apple's Terminal and XTerm, which draw the curly one wrongly, unless tmux or
/// screen is between.
fn spelling_style() -> String {
    static STRAIGHT: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        let var = |name| std::env::var(name).unwrap_or_default();
        let term = var("TERM");
        let mediated = !var("TMUX").is_empty()
            || !var("STY").is_empty()
            || term.starts_with("tmux")
            || term.starts_with("screen");
        let xterm = var("XTERM_VERSION");
        let digits = xterm
            .strip_prefix("XTerm(")
            .and_then(|v| v.strip_suffix(')'));
        let xterm = var("TERM_PROGRAM").is_empty()
            && digits.is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
            && (term == "xterm" || term == "xterm-256color");
        !mediated && (var("TERM_PROGRAM") == "Apple_Terminal" || xterm)
    });
    let underline = if *STRAIGHT { "\x1b[4m" } else { "\x1b[4:3m" };
    format!("{underline}{}", style::SPELLING)
}

pub(super) fn terminal_size() -> (usize, usize) {
    console::size().map_or((80, 24), |(columns, rows)| {
        (usize::from(columns), usize::from(rows))
    })
}

pub struct Editor {
    history: Vec<String>,
    hide_thinking: Arc<AtomicBool>,
    /// Where Tab completes relative paths from.
    root: PathBuf,
    completions: Completions,
    /// The providers' models, which complete `/model`'s argument.
    models: Vec<String>,
    /// Whether Escape twice on an empty prompt returns `Action::DoubleEscape`.
    double_escape: bool,
    /// Edits one text for a question, as Scramjet's dialog editor: Escape or Ctrl-C cancels.
    dialog: bool,
    /// The texts Ctrl+U, Ctrl+K, Ctrl+W and Alt+D deleted, the latest last, which Ctrl+Y puts
    /// in again: Scramjet's kill ring, kept from one prompt to the next.
    kills: Vec<String>,
}

/// What the last key did, which decides what the next one does to the undo snapshots and
/// deleted texts, as Scramjet's editor's last action.
#[derive(Clone, Copy, Default, PartialEq)]
enum Last {
    #[default]
    Other,
    /// Typed a character: a word typed on is undone with it, from the space before it.
    Typing,
    /// Deleted text that the next deletion joins.
    Kill,
    /// Ctrl+Y or Alt+Y, which Alt+Y can replace.
    Yank,
}

impl Editor {
    /// Ctrl+T flips `hide_thinking`.
    pub fn new(
        history: Vec<String>,
        hide_thinking: Arc<AtomicBool>,
        root: PathBuf,
        completions: Completions,
        models: Vec<String>,
        double_escape: bool,
    ) -> Self {
        Self {
            history,
            hide_thinking,
            root,
            completions,
            models,
            double_escape,
            dialog: false,
            kills: Vec::new(),
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
                Completions::default(),
                Vec::new(),
                false,
            )
        }
    }

    pub fn set_models(&mut self, models: Vec<String>) {
        self.models = models;
    }

    /// Completes what `completions` lists, and makes Escape twice on an empty prompt return
    /// `Action::DoubleEscape` when `double_escape`.
    pub fn reloaded(&mut self, completions: Completions, double_escape: bool) {
        self.completions = completions;
        self.double_escape = double_escape;
    }

    /// The list of commands, or of a built-in command's arguments, that `before` begins.
    fn complete_command(&self, before: &str) -> Option<Completion> {
        let listed = &self.completions;
        Completion::slash(before, &listed.commands)
            .or_else(|| Completion::argument(before, &self.models, &listed.mcp_servers))
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

    /// Adds `text`, deleted, to the kill ring; it joins the latest one when `join`, before it
    /// when deleted `backward`, as Scramjet's consecutive kills accumulate.
    fn kill(&mut self, text: String, backward: bool, join: bool) {
        match self.kills.last_mut() {
            Some(last) if join && backward => last.insert_str(0, &text),
            Some(last) if join => last.push_str(&text),
            _ => self.kills.push(text),
        }
    }

    /// Edits `draft` with `key`, or says what the caller should do for it; `pending`: more keys
    /// are already typed, so an `@` search waits for them.
    pub(super) fn key(&mut self, draft: &mut Draft, key: Key, pending: bool) -> Action {
        let Draft {
            buffer,
            browsing,
            saved,
            exit_armed,
            last_escape,
            note,
            completion,
            lookup,
            undo,
            last,
            jump,
        } = draft;
        let escape = key.is(Binding::Interrupt);
        if escape {
            *lookup = None;
        }
        let armed = std::mem::take(exit_armed);
        let noted = note.take();
        let previous = std::mem::take(last);
        let prior = buffer.clone();
        let (up, down) = (key.is(Binding::CursorUp), key.is(Binding::CursorDown));
        // As Scramjet's, going through history is undone at once, to the text it started from.
        let browsing_on = browsing.is_some() && (up || down);
        let undoing = key.is(Binding::Undo);
        let tab = key.is(Binding::Tab);
        // At the prompt the follow-up key sends between turns, as Scramjet's; a dialog's text has
        // no queue, and it is a new line there.
        let follow_up = key.is(Binding::FollowUp);
        let submit = key.is(Binding::Submit) || (follow_up && !self.dialog);
        let new_line = key.is(Binding::NewLine) || (follow_up && self.dialog);
        let jump_key = match (key.is(Binding::JumpForward), key.is(Binding::JumpBackward)) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        };
        let kill = [
            Binding::DeleteToLineStart,
            Binding::DeleteToLineEnd,
            Binding::DeleteWordBackward,
            Binding::DeleteWordForward,
        ]
        .into_iter()
        .any(|binding| key.is(binding));
        let edit = edit(&key);
        let empty = buffer.text.is_empty();
        // As Scramjet's, a space is undone apart from the word before it, and with the one after.
        let typed = match &key {
            Key::Text(text) if !tab => Some(text.starts_with(char::is_whitespace)),
            _ => None,
        };
        let jumping = jump.take();
        let changed = match &key {
            // The jump's key again cancels it; a key that types no character cancels it and
            // does what it does.
            _ if jump_key.is_some() && jumping.is_some() => {
                *last = previous;
                true
            }
            Key::Text(text) if jumping.is_some() => {
                *completion = None;
                buffer.jump(text, jumping == Some(true));
                true
            }
            _ if jump_key.is_some() => {
                *jump = jump_key;
                *exit_armed = armed;
                *note = noted;
                *last = previous;
                false
            }
            _ if undoing => {
                if let Some(snapshot) = undo.pop() {
                    *buffer = snapshot;
                }
                *completion = None;
                *browsing = None;
                true
            }
            _ if key.is(Binding::Yank) => {
                match self.kills.last() {
                    Some(text) => {
                        buffer.insert(text);
                        *last = Last::Yank;
                        *completion = None;
                    }
                    None => *last = previous,
                }
                true
            }
            _ if key.is(Binding::YankPop) => {
                let yanked = self.kills.last().map_or(0, String::len);
                if previous == Last::Yank
                    && self.kills.len() > 1
                    && buffer.text[..buffer.cursor].ends_with(&self.kills[self.kills.len() - 1])
                {
                    buffer.remove(buffer.cursor - yanked, buffer.cursor);
                    self.kills.rotate_right(1);
                    buffer.insert(&self.kills[self.kills.len() - 1]);
                    *last = Last::Yank;
                } else {
                    *last = previous;
                }
                true
            }
            _ if kill && let Some(edit) = edit => {
                *completion = None;
                buffer.apply(edit);
                let removed = prior.text.len() - buffer.text.len();
                if removed > 0 {
                    let text = prior.text[buffer.cursor..buffer.cursor + removed].to_owned();
                    let backward = matches!(edit, Edit::KillStart | Edit::DeleteWordBack);
                    self.kill(text, backward, previous == Last::Kill);
                    *last = Last::Kill;
                } else {
                    *last = previous;
                }
                true
            }
            _ if key == Key::Idle || key.is(Binding::Suspend) => {
                *exit_armed = armed;
                *note = noted;
                *last = previous;
                *jump = jumping;
                return match key {
                    Key::Idle => Action::Edited(false),
                    _ => Action::Suspend,
                };
            }
            _ if escape && completion.is_some() => {
                *completion = None;
                true
            }
            _ if (up || down) && completion.is_some() => {
                if let Some(list) = completion {
                    list.select(up);
                }
                true
            }
            // As in Scramjet, Enter on a command also submits it, and on an argument does not,
            // but on one already typed in full, which Scramjet puts in again for a second Enter:
            // its value, or the id a model's reference is shown by.
            _ if (tab || submit) && completion.is_some() => {
                let list = completion.take().expect("checked above");
                let typed = &buffer.text[list.start..buffer.cursor];
                let selected = &list.items[list.selected];
                let submit = submit
                    && match list.kind {
                        Kind::Slash => true,
                        Kind::Argument => selected.value == typed || selected.label == typed,
                        Kind::Path | Kind::File => false,
                    };
                list.apply(buffer);
                if submit {
                    buffer.set(buffer.text.trim().to_owned());
                    return Action::Submit(buffer.text.clone());
                }
                true
            }
            _ if self.dialog && key.is(Binding::SelectCancel) => return Action::Exit,
            _ if self.dialog && empty && key.is(Binding::Exit) => return Action::Exit,
            _ if escape && self.double_escape && buffer.text.trim().is_empty() => {
                if last_escape.is_some_and(|at| at.elapsed() < DOUBLE_ESCAPE) {
                    *last_escape = None;
                    return Action::DoubleEscape;
                }
                *last_escape = Some(Instant::now());
                *exit_armed = armed;
                *note = noted;
                false
            }
            _ if !self.dialog && key.is(Binding::ThinkingCycle) => return Action::CycleEffort,
            _ if !self.dialog && key.is(Binding::ModelCycleForward) => {
                return Action::CycleModel(1);
            }
            _ if !self.dialog && key.is(Binding::ModelCycleBackward) => {
                return Action::CycleModel(-1);
            }
            _ if key.is(Binding::ThinkingToggle) => {
                let hidden = !self.hide_thinking.fetch_xor(true, Ordering::Relaxed);
                *note = Some((Note::Info(thinking_note(hidden).to_owned()), Instant::now()));
                true
            }
            _ if key.is(Binding::External) => {
                *completion = None;
                return Action::External;
            }
            _ if submit && buffer.text[..buffer.cursor].ends_with('\\') => {
                buffer.apply(Edit::Backspace);
                buffer.insert("\n");
                true
            }
            _ if submit && buffer.text.trim().is_empty() => true,
            _ if submit => {
                // As in Scramjet, the prompt is sent trimmed, and shown as sent.
                buffer.set(buffer.text.trim().to_owned());
                return Action::Submit(buffer.text.clone());
            }
            _ if empty && key.is(Binding::Clear) => {
                if armed {
                    return Action::Exit;
                }
                *exit_armed = true;
                true
            }
            _ if key.is(Binding::Clear) => {
                *buffer = Buffer::default();
                *completion = None;
                *browsing = None;
                true
            }
            _ if empty && key.is(Binding::Exit) => return Action::Exit,
            // Scramjet completes only commands and their arguments after a `/` that starts the
            // prompt; a path there is completed when nothing else matches. An `@` word is
            // looked for.
            _ if tab && !self.dialog => {
                let before = &buffer.text[..buffer.cursor];
                *completion = self.complete_command(before);
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
            _ if tab => {
                buffer.insert("\t");
                true
            }
            // A list follows what is typed, a path's while the same word is typed; commands
            // are listed as soon as a `/` starts the prompt, entries for an `@` word once
            // typing pauses, the list before staying until they are found.
            Key::Text(_) if !self.dialog => self.typed(buffer, completion, lookup, &key, pending),
            _ if !self.dialog && edit == Some(Edit::Backspace) => {
                self.typed(buffer, completion, lookup, &key, pending)
            }
            _ if up || down => {
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
            _ if new_line => {
                *completion = None;
                buffer.insert("\n");
                true
            }
            Key::Text(text) | Key::Paste(text) => {
                *completion = None;
                buffer.insert(text);
                true
            }
            _ if let Some(edit) = edit => {
                *completion = None;
                buffer.apply(edit);
                true
            }
            // Other keys leave the prompt as it is.
            _ => {
                *exit_armed = armed;
                *note = noted;
                *last = previous;
                false
            }
        };
        if let Some(space) = typed.filter(|_| buffer.text != prior.text) {
            *last = Last::Typing;
            if !space && previous == Last::Typing {
                return Action::Edited(changed);
            }
        }
        if buffer.text != prior.text && !browsing_on && !undoing {
            undo.push(prior);
        }
        Action::Edited(changed)
    }

    /// Text typed, or Backspace, at the prompt: the completion list follows.
    fn typed(
        &self,
        buffer: &mut Buffer,
        completion: &mut Option<Completion>,
        lookup: &mut Option<Lookup>,
        key: &Key,
        pending: bool,
    ) -> bool {
        match key {
            Key::Text(text) => buffer.insert(text),
            _ => buffer.apply(Edit::Backspace),
        }
        let before = &buffer.text[..buffer.cursor];
        *completion = match (file_search::Mention::typed(before), completion.take()) {
            (Some(mention), list) => {
                let kept =
                    list.filter(|list| list.kind == Kind::File && list.start == mention.start);
                if !pending {
                    *lookup = Lookup::start(&self.root, mention, buffer, false);
                }
                kept
            }
            (None, Some(list)) if list.kind == Kind::Path => {
                Completion::path(before, &self.root).filter(|next| next.start == list.start)
            }
            (None, _) => self.complete_command(before),
        };
        true
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
    /// Shown on the status row until the next key, with when it was set.
    note: Option<(Note, Instant)>,
    completion: Option<Completion>,
    lookup: Option<Lookup>,
    /// The text and cursor before each edit, the latest last, which Ctrl+- restores.
    undo: Vec<Buffer>,
    last: Last,
    /// Ctrl+] (`true`) or Ctrl+Alt+] waits for the character to jump to.
    jump: Option<bool>,
}

/// What the status row says until the next key, about a key or what it did.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Note {
    /// Goes after `NOTE_TIME` too.
    Info(String),
    Error(String),
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

    /// The status row's note: how to exit after a first Ctrl-C, or the note set, while it lasts.
    pub(super) fn note(&self) -> Option<String> {
        if self.exit_armed {
            return Some(exit_hint());
        }
        match &self.note {
            Some((Note::Error(text), _)) => Some(text.clone()),
            Some((Note::Info(text), at)) if at.elapsed() < NOTE_TIME => Some(text.clone()),
            _ => None,
        }
    }

    pub(super) fn set_note(&mut self, note: Note) {
        self.note = Some((note, Instant::now()));
    }

    /// The completion list's rows, if one is open.
    pub(super) fn completion_rows(&self, width: usize) -> Vec<String> {
        self.completion
            .as_ref()
            .map_or_else(Vec::new, |list| list.rows(width))
    }

    /// The prompt's rows at `width`, marked for a copy (`text::FROM`), and the row and column of
    /// the cursor in them.
    /// With `spelled`, misspelled words are underlined, except in a shell command (`!`).
    pub(super) fn rows(&self, width: usize, spelled: bool) -> (Vec<String>, usize, usize) {
        let text = &self.buffer.text;
        let misspelled = match spelled && !text.trim_start().starts_with('!') {
            true => spelling::misspelled(text),
            false => Vec::new(),
        };
        let layout = Layout::marked(text, width, true, &misspelled);
        let cursor = layout.at(self.buffer.cursor);
        (layout.rows, cursor.row, cursor.column)
    }

    /// Puts `text` in place of the draft's, the cursor at its end, as Scramjet's `setText`:
    /// Ctrl+- brings back the text it replaced.
    pub(super) fn replace(&mut self, text: String) {
        let mut undo = std::mem::take(&mut self.undo);
        if text != self.buffer.text {
            undo.push(self.buffer.clone());
        }
        *self = Self {
            undo,
            ..Self::default()
        };
        self.buffer.set(text);
    }

    /// Takes what the external editor did: its text, or the reason it did nothing.
    pub(super) fn edited(&mut self, edited: Result<Option<String>, String>) {
        match edited {
            Ok(Some(text)) => self.replace(text),
            Ok(None) => {}
            Err(error) => self.set_note(Note::Error(error)),
        }
    }
}

fn exit_hint() -> String {
    match (keymap::key(Binding::Clear), keymap::key(Binding::Exit)) {
        (Some(clear), Some(exit)) => format!("{clear} again or {exit} to exit"),
        (Some(clear), None) => format!("{clear} again to exit"),
        (None, Some(exit)) => format!("{exit} to exit"),
        (None, None) => "/exit to exit".to_owned(),
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
    use crate::agent_cli::keys::tests::keys;

    fn edit(text: &str, cursor: usize, input: &[u8]) -> (String, usize) {
        let mut buffer = Buffer {
            text: text.into(),
            cursor,
        };
        for key in keys(input) {
            match (&key, super::edit(&key)) {
                (Key::Text(text) | Key::Paste(text), _) => buffer.insert(text),
                (_, Some(edit)) => buffer.apply(edit),
                _ => panic!("{key:?} is no edit"),
            }
        }
        (buffer.text, buffer.cursor)
    }

    #[test]
    fn paste_is_inserted_literally_without_submitting() {
        assert_eq!(
            keys(b"\x1b[200~one\r\ntwo\rthree\x03\x1b[A\tend\x1b[201~\r"),
            [
                Key::Paste("one\ntwo\nthree[A\tend".into()),
                crate::agent_cli::keys::tests::key("enter")
            ]
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
    #[allow(clippy::single_range_in_vec_init)]
    fn misspelled_words_are_underlined_on_every_row_they_reach() {
        let (on, off) = (spelling_style(), style::RESET);
        let marked =
            |text, width, ranges: &[Range<usize>]| Layout::marked(text, width, false, ranges).rows;
        assert_eq!(
            marked("teh cat", 80, &[0..3]),
            [format!("> {on}teh{off} cat")]
        );
        // A word moved to the next row takes its underline along; the stops are unchanged.
        assert_eq!(
            marked("hello wrld", 10, &[6..10]),
            ["> hello ".to_owned(), format!("  {on}wrld{off}")]
        );
        assert_eq!(
            Layout::marked("hello wrld", 10, false, &[6..10])
                .at(8)
                .column,
            Layout::new("hello wrld", 10).at(8).column
        );
        // One broken across rows is underlined on each, not the gutter between.
        assert_eq!(
            marked("a bcdefghij", 8, &[2..11]),
            [
                "> a ".to_owned(),
                format!("  {on}bcdef{off}"),
                format!("  {on}ghij{off}")
            ]
        );
        assert_eq!(
            marked("ab\ncd", 80, &[3..5]),
            ["> ab".to_owned(), format!(". {on}cd{off}")]
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

    /// Types `input` into `draft`, as the terminal sends it.
    fn type_in(editor: &mut Editor, draft: &mut Draft, input: &[u8]) {
        for key in keys(input) {
            editor.key(draft, key, false);
        }
    }

    fn prompt_editor(history: Vec<String>) -> Editor {
        let root = PathBuf::new();
        Editor::new(
            history,
            Arc::default(),
            root,
            Completions::default(),
            Vec::new(),
            false,
        )
    }

    #[test]
    fn a_note_goes_at_the_next_key_and_one_not_an_error_after_a_while() {
        let mut editor = prompt_editor(Vec::new());
        let mut draft = editor.draft(String::new());
        draft.set_note(Note::Info("Copied".into()));
        assert_eq!(draft.note().as_deref(), Some("Copied"));
        // Keys that leave the prompt as it is keep it.
        type_in(&mut editor, &mut draft, b"\x1b[5~");
        assert_eq!(draft.note().as_deref(), Some("Copied"));
        type_in(&mut editor, &mut draft, b"a");
        assert_eq!(draft.note(), None);
        let set = |draft: &mut Draft, note, ago| {
            draft.note = Some((note, Instant::now() - ago));
        };
        set(&mut draft, Note::Info("Copied".into()), NOTE_TIME);
        assert_eq!(draft.note(), None);
        set(
            &mut draft,
            Note::Error("Copy failed".into()),
            NOTE_TIME * 10,
        );
        assert_eq!(draft.note().as_deref(), Some("Copy failed"));
        type_in(&mut editor, &mut draft, b"\x7f");
        assert_eq!(draft.note(), None);
        // The exit's note lasts as long as the second Ctrl-C would exit.
        type_in(&mut editor, &mut draft, b"\x15\x03");
        assert_eq!(
            draft.note().as_deref(),
            Some("Ctrl+C again or Ctrl+D to exit")
        );
        type_in(&mut editor, &mut draft, b"b");
        assert_eq!(draft.note(), None);
    }

    #[test]
    fn deleted_text_is_put_back_and_cycled_across_prompts() {
        let mut editor = prompt_editor(Vec::new());
        let mut draft = editor.draft("one two three".into());
        // Deletions in a row join, in the order the text had.
        type_in(&mut editor, &mut draft, b"\x17\x17\x19");
        assert_eq!(draft.text(), "one two three");
        type_in(&mut editor, &mut draft, b"\x01\x0b");
        assert_eq!(draft.text(), "");
        assert_eq!(editor.kills, ["two three", "one two three"]);
        // A key between them keeps them apart; Ctrl+D is no deletion that joins.
        let mut draft = editor.draft("ab cd".into());
        type_in(&mut editor, &mut draft, b"\x1b[D\x0b\x02\x04\x15");
        assert_eq!(draft.text(), "");
        assert_eq!(editor.kills, ["two three", "one two three", "d", "ab "]);
        // The editor keeps them for the next prompt; Alt+Y right after Ctrl+Y goes back.
        let mut draft = editor.draft("x".into());
        type_in(&mut editor, &mut draft, b"\x19");
        assert_eq!(draft.text(), "xab ");
        type_in(&mut editor, &mut draft, b"\x1by");
        assert_eq!(draft.text(), "xd");
        type_in(&mut editor, &mut draft, b"\x1by\x1by");
        assert_eq!(draft.text(), "xtwo three");
        type_in(&mut editor, &mut draft, b"\x1b[D\x1by");
        assert_eq!(draft.text(), "xtwo three");
        // One undo takes out what one Alt+Y put in.
        type_in(&mut editor, &mut draft, b"\x1b[C\x1f");
        assert_eq!(draft.text(), "xone two three");
        // Over lines: Ctrl+K at a line's end deletes the line break.
        let mut draft = editor.draft("ab\ncd".into());
        type_in(&mut editor, &mut draft, b"\x02\x02\x02\x02\x0b\x0b");
        assert_eq!(draft.text(), "acd");
        assert_eq!(editor.kills.last().unwrap(), "b\n");
        // Without deleted text, Ctrl+Y and Alt+Y do nothing.
        let mut editor = prompt_editor(Vec::new());
        let mut draft = editor.draft("a".into());
        type_in(&mut editor, &mut draft, b"\x19\x1by");
        assert_eq!(draft.text(), "a");
    }

    #[test]
    fn undo_takes_back_words_edits_and_cleared_prompts_as_scramjets() {
        let mut editor = prompt_editor(vec!["first".into(), "second".into()]);
        let mut draft = editor.draft(String::new());
        type_in(&mut editor, &mut draft, b"hello big world");
        // A word goes with the space before it.
        type_in(&mut editor, &mut draft, b"\x1f");
        assert_eq!(draft.text(), "hello big");
        type_in(&mut editor, &mut draft, b"\x1f");
        assert_eq!(draft.text(), "hello");
        // Each deletion is undone on its own, the cursor back where it was.
        type_in(&mut editor, &mut draft, b"\x7f\x7f\x1f");
        assert_eq!((draft.text(), draft.buffer.cursor), ("hell", 4));
        // Ctrl-C's clearing too.
        type_in(&mut editor, &mut draft, b"\x03");
        assert_eq!(draft.text(), "");
        type_in(&mut editor, &mut draft, b"\x1f");
        assert_eq!(draft.text(), "hell");
        // Going through history is undone at once.
        type_in(&mut editor, &mut draft, b"\x1b[A\x1b[A");
        assert_eq!(draft.text(), "first");
        type_in(&mut editor, &mut draft, b"\x1f");
        assert_eq!(draft.text(), "hell");
        // Back to the start, where Ctrl+- changes nothing.
        type_in(&mut editor, &mut draft, b"\x1f\x1f\x1f\x1f");
        assert_eq!(draft.text(), "");
        // Text put in for the user, as a fork's prompt, is undone too.
        draft.replace("forked".into());
        type_in(&mut editor, &mut draft, b"\x1f");
        assert_eq!(draft.text(), "");
    }

    #[test]
    fn jumps_to_the_next_or_previous_character_typed() {
        let mut editor = prompt_editor(Vec::new());
        let mut draft = editor.draft("a,b\nc,d".into());
        let cursor = |draft: &Draft| draft.buffer.cursor;
        type_in(&mut editor, &mut draft, b"\x01\x1b[A");
        assert_eq!(cursor(&draft), 0);
        // Past the character at the cursor, and over lines.
        type_in(&mut editor, &mut draft, b"\x1d,");
        assert_eq!(cursor(&draft), 1);
        type_in(&mut editor, &mut draft, b"\x1d,");
        assert_eq!(cursor(&draft), 5);
        // Not found: the cursor stays.
        type_in(&mut editor, &mut draft, b"\x1d,");
        assert_eq!(cursor(&draft), 5);
        type_in(&mut editor, &mut draft, b"\x1b\x1da");
        assert_eq!(cursor(&draft), 0);
        assert_eq!(draft.text(), "a,b\nc,d");
        // The key again cancels, as does a key that types nothing, which then does its own.
        type_in(&mut editor, &mut draft, b"\x1d\x1db");
        assert_eq!(draft.text(), "ba,b\nc,d");
        type_in(&mut editor, &mut draft, b"\x1d\x05");
        assert_eq!(cursor(&draft), 4);
        // A character of several bytes, and not inside another's grapheme.
        let mut draft = editor.draft("e\u{301}x é e".into());
        type_in(&mut editor, &mut draft, b"\x01");
        type_in(&mut editor, &mut draft, "\x1d\u{301}\x1dé".as_bytes());
        assert_eq!(cursor(&draft), "e\u{301}x ".len());
        type_in(&mut editor, &mut draft, b"\x1de");
        assert_eq!(cursor(&draft), "e\u{301}x é ".len());
    }
}
