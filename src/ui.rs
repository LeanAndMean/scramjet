mod coding_approval;
mod coding_history;
mod command_projection;
mod command_reader_projection;
mod diagnostic;
mod haiku_document;
mod haiku_projection;
mod input;
mod project_projection;
mod reader_projection;
mod runtime_projection;
mod writer;
use writer::Output;
#[cfg(test)]
mod resources;

use input::{Decoder, Event};
use ratatui_core::{
    buffer::{Buffer, CellWidth},
    layout::Rect,
    style::{Color, Style},
    terminal::{Terminal, TerminalOptions, Viewport},
};
use ratatui_crossterm::CrosstermBackend;
use rustix::{
    fd::OwnedFd,
    fs::{Mode, OFlags},
    termios::{OptionalActions, QueueSelector, Termios},
};
use std::{
    collections::VecDeque,
    io::{self, IsTerminal, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;

const EDITOR_LIMIT: usize = 8192;
const FRAME_LIMIT: usize = 65536;
const ENABLE: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?1002h\x1b[?1006h\x1b[?1004h\x1b[?2004h";
const DISABLE: &[u8] = b"\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1002l\x1b[?25h\x1b[?1049l";

#[derive(Clone)]
struct Line {
    id: u64,
    text: String,
    hard: bool,
}

#[derive(Clone, Debug)]
struct Glyph {
    text: String,
    column: u16,
    width: u16,
    source: usize,
}

#[derive(Clone, Debug)]
struct Row {
    line: u64,
    start: usize,
    glyphs: Vec<Glyph>,
    hard: bool,
}

#[derive(Clone, Default)]
struct Display {
    rows: Vec<Row>,
    width: u16,
    height: u16,
    transcript_height: u16,
    scanned: usize,
    text_bytes: usize,
    exhausted: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Transcript,
    Editor,
}

struct Model {
    lines: VecDeque<Line>,
    next_id: u64,
    evicted: u64,
    editor: String,
    revision: u64,
    caret: usize,
    focus: Focus,
    focused: bool,
    anchor: Option<(u64, usize)>,
    display: Display,
    selection: Option<((u16, u16), (u16, u16))>,
    copied: String,
    dragging: bool,
    status: &'static str,
    ticks: u64,
    runtime_summary: Option<String>,
    reader_mode: bool,
    command_mode: bool,
}

impl Model {
    fn new() -> Self {
        let mut model = Self {
            lines: VecDeque::with_capacity(256),
            next_id: 0,
            evicted: 0,
            editor: {
                let mut text = String::with_capacity(EDITOR_LIMIT);
                text.push_str("leftright");
                text
            },
            revision: 0,
            caret: 4,
            focus: Focus::Editor,
            focused: true,
            anchor: None,
            display: Display::default(),
            selection: None,
            copied: String::new(),
            dragging: false,
            status: "synthetic only; clipboard receipt unverified",
            ticks: 0,
            runtime_summary: None,
            reader_mode: false,
            command_mode: false,
        };
        for _ in 0..80 {
            model.append("browse é界👩‍💻 — soft wrap sample: abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz").unwrap();
        }
        model
    }

    fn append(&mut self, text: &str) -> Result<(), &'static str> {
        if text.len() > 1024 || text.chars().any(char::is_control) {
            return Err("invalid synthetic line");
        }
        if self.lines.len() == 256 {
            let removed = self.lines.pop_front().unwrap();
            self.evicted += 1;
            if self.anchor.is_some_and(|(id, _)| id == removed.id) {
                self.anchor = self.lines.front().map(|line| (line.id, 0));
                self.clear_selection();
                self.status = "browse anchor evicted; clamped to oldest retained line";
            }
        }
        self.lines.push_back(Line {
            id: self.next_id,
            text: text.into(),
            hard: true,
        });
        self.next_id += 1;
        Ok(())
    }

    fn tick(&mut self) {
        self.ticks += 1;
        self.append("stream é界👩‍💻 — synthetic fragment; hard break")
            .unwrap();
    }

    fn insert(&mut self, text: &str) {
        let mut normalized = String::with_capacity(EDITOR_LIMIT);
        let mut chars = text.chars().peekable();
        while let Some(character) = chars.next() {
            let value = match character {
                '\r' => {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    "\n"
                }
                '\t' => "    ",
                '\n' => "\n",
                c if c.is_control() => {
                    self.status = "insertion rejected: control character";
                    return;
                }
                c => {
                    let mut bytes = [0; 4];
                    let value = c.encode_utf8(&mut bytes);
                    if normalized.len() + value.len() + self.editor.len() > EDITOR_LIMIT {
                        self.status = "insertion rejected: 8192-byte editor limit";
                        return;
                    }
                    normalized.push_str(value);
                    continue;
                }
            };
            if normalized.len() + value.len() + self.editor.len() > EDITOR_LIMIT {
                self.status = "insertion rejected: 8192-byte editor limit";
                return;
            }
            normalized.push_str(value);
        }
        if !normalized.is_empty() {
            let Some(revision) = self.revision.checked_add(1) else {
                self.status = "draft revision exhausted";
                return;
            };
            self.revision = revision;
        }
        self.editor.insert_str(self.caret, &normalized);
        self.caret += normalized.len();
        self.snap_caret();
        self.status = "draft edited; Enter never submits";
    }

    fn snap_caret(&mut self) {
        if self.caret == 0 {
            return;
        }
        self.caret = self
            .editor
            .grapheme_indices(true)
            .map(|(i, g)| i + g.len())
            .find(|end| *end >= self.caret)
            .unwrap_or(self.editor.len());
    }

    fn clear_selection(&mut self) {
        self.selection = None;
        self.copied.clear();
        self.dragging = false;
    }

    fn browse(&mut self, amount: isize) {
        self.clear_selection();
        let mut keys = VecDeque::with_capacity(self.display.transcript_height as usize + 1);
        let current = self.display.rows.first().map(|r| (r.line, r.start));
        let target = current.unwrap_or((0, 0));
        let mut seen = false;
        let mut after = amount.max(0) as usize;
        let mut previous = None;
        for line in &self.lines {
            for row in wrap_line(line, self.display.width.max(1)) {
                let key = (row.line, row.start);
                if key == target {
                    seen = true;
                    if amount < 0 {
                        self.anchor = keys.front().copied().or(Some(key));
                        return;
                    }
                }
                if seen {
                    if after == 0 {
                        self.anchor = Some(key);
                        return;
                    }
                    after -= 1;
                }
                keys.push_back(key);
                if keys.len() > amount.unsigned_abs().max(1) {
                    keys.pop_front();
                }
                previous = Some(key);
            }
        }
        self.anchor = previous;
    }

    fn event(&mut self, event: Event) -> bool {
        if event == Event::Quit {
            return true;
        }
        if let Event::Focus(focused) = event {
            self.focused = focused;
            self.clear_selection();
            self.status = if focused {
                "focus returned; draft retained"
            } else {
                "focus lost; edits paused"
            };
            return false;
        }
        if !self.focused {
            return false;
        }
        match event {
            Event::Copy => {
                self.status = if self.copied.is_empty() {
                    "copy no-op: no displayed selection"
                } else {
                    "copy requested"
                };
            }
            Event::Paste(text) => self.insert(&text),
            Event::Tab => {
                self.focus = if self.focus == Focus::Editor {
                    Focus::Transcript
                } else {
                    Focus::Editor
                };
            }
            Event::Follow => {
                self.anchor = None;
                self.clear_selection();
                self.status = "following synthetic tail";
            }
            Event::PageUp => self.browse(-(self.display.transcript_height.max(1) as isize)),
            Event::PageDown => self.browse(self.display.transcript_height.max(1) as isize),
            Event::End if self.focus == Focus::Transcript => {
                self.anchor = None;
                self.clear_selection();
            }
            Event::Mouse {
                button,
                x,
                y,
                release,
            } => {
                if button & 64 != 0 {
                    self.browse(if button & 1 == 0 { -3 } else { 3 });
                } else if y < self.display.transcript_height && x < self.display.width {
                    self.focus = Focus::Transcript;
                    if button & 3 == 0 && button & 32 == 0 && !release {
                        self.selection = Some(((x, y), (x, y)));
                        self.dragging = true;
                    } else if self.dragging {
                        if let Some((_, end)) = &mut self.selection {
                            *end = (x, y);
                        }
                        if release {
                            self.dragging = false;
                        }
                    }
                } else if y < self.display.height && !release {
                    self.focus = Focus::Editor;
                    self.dragging = false;
                }
            }
            Event::Text(text) if self.focus == Focus::Editor => self.insert(&text),
            Event::Left if self.focus == Focus::Editor => {
                self.caret = self.editor[..self.caret]
                    .grapheme_indices(true)
                    .next_back()
                    .map_or(0, |(i, _)| i);
            }
            Event::Right if self.focus == Focus::Editor => {
                self.caret += self.editor[self.caret..]
                    .graphemes(true)
                    .next()
                    .map_or(0, str::len);
            }
            Event::Home if self.focus == Focus::Editor => {
                self.caret = self.editor[..self.caret].rfind('\n').map_or(0, |i| i + 1);
            }
            Event::End if self.focus == Focus::Editor => {
                self.caret += self.editor[self.caret..]
                    .find('\n')
                    .unwrap_or(self.editor.len() - self.caret);
            }
            Event::Up | Event::Down if self.focus == Focus::Editor => {
                self.vertical(event == Event::Down)
            }
            Event::Backspace if self.focus == Focus::Editor && self.caret > 0 => {
                let start = self.editor[..self.caret]
                    .grapheme_indices(true)
                    .next_back()
                    .unwrap()
                    .0;
                let Some(revision) = self.revision.checked_add(1) else {
                    return false;
                };
                self.revision = revision;
                self.editor.drain(start..self.caret);
                self.caret = start;
                self.snap_caret();
            }
            Event::Delete if self.focus == Focus::Editor => {
                let end = self.caret
                    + self.editor[self.caret..]
                        .graphemes(true)
                        .next()
                        .map_or(0, str::len);
                if end != self.caret {
                    let Some(revision) = self.revision.checked_add(1) else {
                        return false;
                    };
                    self.revision = revision;
                }
                self.editor.drain(self.caret..end);
                self.snap_caret();
            }
            Event::Unknown => self.status = "unknown bounded input ignored",
            _ => {}
        }
        false
    }

    fn vertical(&mut self, down: bool) {
        let start = self.editor[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        let column = self.editor[start..self.caret].graphemes(true).count();
        let target = if down {
            let Some(offset) = self.editor[self.caret..].find('\n') else {
                return;
            };
            self.caret + offset + 1
        } else {
            if start == 0 {
                return;
            }
            self.editor[..start - 1].rfind('\n').map_or(0, |i| i + 1)
        };
        let end = self.editor[target..]
            .find('\n')
            .map_or(self.editor.len(), |i| target + i);
        self.caret = self.editor[target..end]
            .grapheme_indices(true)
            .nth(column)
            .map_or(end, |(i, _)| target + i);
    }

    fn project(&self, width: u16, height: u16) -> Display {
        let (width, height) = clamp(width, height);
        let transcript_height = height.saturating_sub(7);
        if self.selection.is_some() && self.display.width == width && self.display.height == height
        {
            return self.display.clone();
        }
        let mut display = Display {
            width,
            height,
            transcript_height,
            ..Display::default()
        };
        if width < 10 || transcript_height == 0 {
            return display;
        }
        let mut rows = VecDeque::with_capacity(transcript_height as usize);
        let mut reached = self.anchor.is_none();
        for line in &self.lines {
            display.scanned += line.text.len();
            let wrapped = wrap_line(line, width);
            for (index, row) in wrapped.iter().enumerate() {
                if self.anchor.is_some_and(|(id, offset)| {
                    row.line > id
                        || (row.line == id
                            && row.start <= offset
                            && wrapped
                                .get(index + 1)
                                .is_none_or(|next| next.start > offset))
                }) {
                    reached = true;
                }
                if !reached {
                    continue;
                }
                if self.anchor.is_some() && rows.len() == transcript_height as usize {
                    break;
                }
                rows.push_back(row.clone());
                if rows.len() > transcript_height as usize {
                    rows.pop_front();
                }
            }
            if self.anchor.is_some() && rows.len() == transcript_height as usize {
                break;
            }
        }
        // Reserve cell text for editor replacements, padding, status and legends.
        let transcript_budget = FRAME_LIMIT - EDITOR_LIMIT * 3 - 12000 - 1024;
        for mut row in rows {
            let keep = row
                .glyphs
                .iter()
                .take_while(|glyph| {
                    if display.text_bytes + glyph.text.len() > transcript_budget {
                        return false;
                    }
                    display.text_bytes += glyph.text.len();
                    true
                })
                .count();
            let exhausted = keep != row.glyphs.len();
            row.glyphs.truncate(keep);
            if exhausted {
                display.exhausted = true;
                row.hard = false;
                display.rows.push(row);
                break;
            }
            display.rows.push(row);
        }
        display
    }

    fn paint(&self, display: &Display, buffer: &mut Buffer) {
        if display.width < 10 || display.height < 8 {
            buffer.set_stringn(
                0,
                0,
                "terminal too small; draft retained",
                display.width as usize,
                Style::default(),
            );
            return;
        }
        for (y, row) in display.rows.iter().enumerate() {
            for glyph in &row.glyphs {
                let style = if selected(self.selection, glyph, y as u16) {
                    Style::default().bg(Color::Blue)
                } else {
                    Style::default()
                };
                buffer.set_stringn(
                    glyph.column,
                    y as u16,
                    &glyph.text,
                    glyph.width as usize,
                    style,
                );
                if selected(self.selection, glyph, y as u16) {
                    for column in glyph.column..glyph.column + glyph.width {
                        buffer[(column, y as u16)].set_style(style);
                    }
                }
            }
        }
        let divider = display.transcript_height;
        let label = format!(
            "{} evicted={} ticks={} | draft {} bytes caret={}",
            if self.anchor.is_none() {
                "TAIL"
            } else {
                "BROWSE"
            },
            self.evicted,
            self.ticks,
            self.editor.len(),
            self.caret
        );
        buffer.set_stringn(
            0,
            divider,
            label,
            display.width as usize,
            Style::default().fg(Color::Yellow),
        );
        let editor_rows = self.editor_rows(display.width);
        let caret_row = editor_rows
            .iter()
            .rposition(|r| r.start <= self.caret)
            .unwrap_or(0);
        let skip = caret_row.saturating_sub(2);
        for (offset, row) in editor_rows.iter().skip(skip).take(4).enumerate() {
            for glyph in &row.glyphs {
                buffer.set_stringn(
                    glyph.column,
                    divider + 1 + offset as u16,
                    &glyph.text,
                    glyph.width as usize,
                    Style::default().fg(if self.focus == Focus::Editor {
                        Color::Green
                    } else {
                        Color::White
                    }),
                );
            }
        }
        buffer.set_stringn(
            0,
            display.height - 2,
            if self.runtime_summary.is_some() {
                if self.command_mode {
                    self.status
                } else if self.reader_mode {
                    "Tab focus | F3 tail | F4 tree | F5 next | F6 child | Ctrl+Q quit"
                } else {
                    "Tab focus | PgUp/Dn | F3 tail | F4 cancel | Ctrl+Q quit"
                }
            } else {
                "Tab focus | PgUp/Dn browse | F3 tail | F2 copy | Ctrl+Q quit"
            },
            display.width as usize,
            Style::default(),
        );
        buffer.set_stringn(
            0,
            display.height - 1,
            if display.exhausted {
                "display text cap reached; remainder not displayed"
            } else {
                self.runtime_summary.as_deref().unwrap_or(self.status)
            },
            display.width as usize,
            Style::default().fg(Color::Yellow),
        );
    }

    fn editor_rows(&self, width: u16) -> Vec<Row> {
        let mut rows = wrap(&self.editor, 0, width);
        for index in (0..rows.len()).rev() {
            let row = &rows[index];
            if row.hard
                && let Some(glyph) = row.glyphs.last()
                && glyph.column + glyph.width == width
            {
                let start = glyph.source
                    + self.editor[glyph.source..]
                        .graphemes(true)
                        .next()
                        .unwrap()
                        .len();
                rows.insert(
                    index + 1,
                    Row {
                        line: 0,
                        start,
                        glyphs: Vec::new(),
                        hard: false,
                    },
                );
            }
        }
        rows
    }

    fn cursor(&self, display: &Display) -> Option<(u16, u16)> {
        if self.focus != Focus::Editor || !self.focused || display.width < 10 || display.height < 8
        {
            return None;
        }
        let rows = self.editor_rows(display.width);
        let index = rows
            .iter()
            .rposition(|r| r.start <= self.caret)
            .unwrap_or(0);
        let row = &rows[index];
        let column = row
            .glyphs
            .iter()
            .find(|g| g.source >= self.caret)
            .map_or_else(
                || {
                    row.glyphs
                        .last()
                        .map_or(0, |g| (g.column + g.width).min(display.width - 1))
                },
                |g| g.column,
            );
        Some((
            column,
            display.transcript_height + 1 + (index - index.saturating_sub(2)) as u16,
        ))
    }

    fn displayed(&mut self, display: Display) {
        self.copied = selection_text(&display, self.selection);
        self.display = display;
    }
}

fn clamp(width: u16, height: u16) -> (u16, u16) {
    (width.clamp(1, 200), height.clamp(1, 60))
}

fn wrap(text: &str, line: u64, width: u16) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut row = Row {
        line,
        start: 0,
        glyphs: Vec::new(),
        hard: false,
    };
    let mut column = 0;
    for (index, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" {
            row.hard = true;
            rows.push(row);
            row = Row {
                line,
                start: index + 1,
                glyphs: Vec::new(),
                hard: false,
            };
            column = 0;
            continue;
        }
        let original_width = grapheme.cell_width();
        let shown = if grapheme.len() > 64 || original_width == 0 || original_width > width {
            "�"
        } else {
            grapheme
        };
        let size = shown.cell_width();
        if column + size > width {
            rows.push(row);
            row = Row {
                line,
                start: index,
                glyphs: Vec::new(),
                hard: false,
            };
            column = 0;
        }
        row.glyphs.push(Glyph {
            text: shown.into(),
            column,
            width: size,
            source: index,
        });
        column += size;
    }
    row.hard = true;
    rows.push(row);
    rows
}

fn wrap_line(line: &Line, width: u16) -> Vec<Row> {
    let mut rows = wrap(&line.text, line.id, width);
    rows.last_mut().unwrap().hard = line.hard;
    rows
}

fn selected(selection: Option<((u16, u16), (u16, u16))>, glyph: &Glyph, y: u16) -> bool {
    let Some((a, b)) = selection else {
        return false;
    };
    let (start, end) = if (a.1, a.0) <= (b.1, b.0) {
        (a, b)
    } else {
        (b, a)
    };
    (y, glyph.column + glyph.width - 1) >= (start.1, start.0) && (y, glyph.column) <= (end.1, end.0)
}

fn selection_text(display: &Display, selection: Option<((u16, u16), (u16, u16))>) -> String {
    let mut text = String::new();
    let Some((a, b)) = selection else {
        return text;
    };
    let end_y = a.1.max(b.1);
    for (y, row) in display.rows.iter().enumerate() {
        for glyph in &row.glyphs {
            if selected(selection, glyph, y as u16) {
                if text.len() + glyph.text.len() > FRAME_LIMIT {
                    return String::new();
                }
                text.push_str(&glyph.text);
            }
        }
        if row.hard && y as u16 >= a.1.min(b.1) && (y as u16) < end_y {
            if text.len() == FRAME_LIMIT {
                return String::new();
            }
            text.push('\n');
        }
    }
    text
}

fn osc52(text: &str) -> Result<Vec<u8>, &'static str> {
    if text.len() > FRAME_LIMIT {
        return Err("copy rejected: 65536-byte limit");
    }
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::with_capacity(8 + text.len().div_ceil(3) * 4);
    result.extend(b"\x1b]52;c;");
    for chunk in text.as_bytes().chunks(3) {
        let value = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        result.push(ALPHABET[(value >> 18) as usize]);
        result.push(ALPHABET[((value >> 12) & 63) as usize]);
        result.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize]
        } else {
            b'='
        });
        result.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize]
        } else {
            b'='
        });
    }
    result.push(7);
    Ok(result)
}

struct Guard {
    tty: OwnedFd,
    termios: Termios,
    raw: bool,
    modes: bool,
    signals: Vec<signal_hook::SigId>,
    output: Output,
}

impl Guard {
    fn restore(&mut self) -> io::Result<()> {
        let mut failures = Vec::new();
        if self.modes {
            self.output.begin();
            if let Err(error) = self
                .output
                .write_all(DISABLE)
                .and_then(|()| self.output.flush())
            {
                failures.push(error.to_string());
            }
            self.modes = false;
        }
        if self.raw {
            if let Err(error) = rustix::termios::tcflush(&self.tty, QueueSelector::IFlush) {
                failures.push(error.to_string());
            }
            if let Err(error) =
                rustix::termios::tcsetattr(&self.tty, OptionalActions::Now, &self.termios)
            {
                failures.push(error.to_string());
            }
            self.raw = false;
        }
        for id in self.signals.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(failures.join("; ")))
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn dispatch(
    model: &mut Model,
    diagnostic: &mut Option<diagnostic::Diagnostic>,
    event: Event,
    output: &mut impl Write,
) -> io::Result<bool> {
    let record = diagnostic
        .as_mut()
        .map(|observer| observer.before(&event, model));
    let mut copy = diagnostic::CopyOutcome::NotCopy;
    let result = if event == Event::Copy && model.focused && !model.copied.is_empty() {
        let result = osc52(&model.copied)
            .map_err(io::Error::other)
            .and_then(|bytes| output.write_all(&bytes).and_then(|()| output.flush()));
        copy = if result.is_ok() {
            model.status = "copy sent; receipt unverified";
            diagnostic::CopyOutcome::Sent
        } else {
            diagnostic::CopyOutcome::Failed
        };
        result.map(|()| false)
    } else {
        if event == Event::Copy {
            copy = if model.focused {
                diagnostic::CopyOutcome::Noop
            } else {
                diagnostic::CopyOutcome::Ignored
            };
        }
        Ok(model.event(event))
    };
    if let Some(record) = record {
        diagnostic.as_mut().unwrap().after(record, model, copy);
    }
    result
}

#[derive(Default)]
struct Overlays<'a> {
    approval: Option<&'a mut coding_approval::Approval>,
    history: Option<&'a coding_history::History>,
    project_history: Option<&'a project_projection::History>,
    haiku: Option<&'a mut haiku_projection::Presentation>,
}
fn draw(
    terminal: &mut Terminal<CrosstermBackend<impl Write>>,
    model: &mut Model,
    diagnostic: &mut Option<diagnostic::Diagnostic>,
    display: Display,
    overlays: Overlays<'_>,
) -> io::Result<bool> {
    let Overlays {
        mut approval,
        history,
        project_history,
        mut haiku,
    } = overlays;
    let mut complete_approval = false;
    let mut complete_submission = false;
    terminal.draw(|frame| {
        model.paint(&display, frame.buffer_mut());
        if let Some(approval) = &approval {
            complete_approval = approval.paint(model, frame.buffer_mut());
        }
        if let Some(history) = history {
            history.paint(model, frame.buffer_mut());
        }
        if let Some(history) = project_history {
            history.paint(model, frame.buffer_mut());
        }
        if let Some(haiku) = &haiku {
            complete_submission = haiku.paint_submission(model, frame.buffer_mut());
            complete_approval = haiku.paint(model, frame.buffer_mut());
        }
        if let Some(observer) = &diagnostic {
            observer.paint_footer(model, &display, frame.buffer_mut());
        }
        if let Some(position) = model.cursor(&display) {
            frame.set_cursor_position(position);
        }
    })?;
    model.displayed(display);
    if let Some(observer) = diagnostic {
        observer.displayed(model);
    }
    if let Some(haiku) = &mut haiku {
        haiku.submission_painted(complete_submission);
        Ok(haiku.painted(complete_approval))
    } else {
        Ok(approval
            .as_mut()
            .is_some_and(|approval| approval.painted(complete_approval)))
    }
}

pub fn run(diagnostic: bool) -> io::Result<()> {
    run_with(diagnostic, || {})
}

fn run_with(diagnostic_enabled: bool, on_ready: impl FnOnce()) -> io::Result<()> {
    run_mode(diagnostic_enabled, on_ready, None)
}

pub(crate) fn runtime_output() -> io::Result<impl Write> {
    let mut output = Output::tty()?;
    output.begin();
    Ok(output)
}

pub(crate) fn run_runtime(bridge: &crate::runtime_ui_demo::Bridge) -> io::Result<()> {
    run_mode(false, || bridge.ready(), Some(DemoBridge::Runtime(bridge)))
}

pub(crate) fn run_reader(bridge: &crate::reader_ui_demo::Bridge) -> io::Result<()> {
    run_mode(false, || bridge.ready(), Some(DemoBridge::Reader(bridge)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodingReturn {
    Checkpoint,
    Settlement,
    Exit,
}
pub(crate) struct CodingUi {
    model: Option<Model>,
    stopped: Arc<AtomicBool>,
    signals: Vec<signal_hook::SigId>,
    deadline: Instant,
    returned: CodingReturn,
    settled: bool,
    project_history: Option<project_projection::History>,
    haiku: Option<haiku_projection::Presentation>,
}
impl CodingUi {
    pub(crate) fn new() -> io::Result<Self> {
        let mut state = Self {
            model: None,
            stopped: Arc::new(AtomicBool::new(false)),
            signals: Vec::with_capacity(4),
            deadline: Instant::now() + Duration::from_secs(30),
            returned: CodingReturn::Exit,
            settled: false,
            project_history: None,
            haiku: None,
        };
        for signal in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
            signal_hook::consts::SIGTSTP,
        ] {
            state
                .signals
                .push(signal_hook::flag::register(signal, state.stopped.clone())?);
        }
        Ok(state)
    }
    pub(crate) fn live(&self) -> bool {
        !self.stopped.load(Ordering::Relaxed) && Instant::now() < self.deadline
    }
    pub(crate) fn bind(&self, bridge: &crate::coding_ui_demo::Bridge) {
        bridge.bind_lifecycle(self.stopped.clone(), self.deadline);
    }
    pub(crate) fn read_only(&self) -> bool {
        self.settled
    }
    pub(crate) fn mode(&mut self, read_only: bool) {
        self.settled = read_only;
        if let Some(model) = &mut self.model {
            model.anchor = None;
            model.clear_selection();
        }
    }
}
impl Drop for CodingUi {
    fn drop(&mut self) {
        for id in self.signals.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
pub(crate) fn run_coding(
    bridge: &crate::coding_ui_demo::Bridge,
    state: &mut CodingUi,
) -> io::Result<CodingReturn> {
    state.bind(bridge);
    state.returned = CodingReturn::Exit;
    run_interval(false, || {}, Some(DemoBridge::Coding(bridge)), Some(state))?;
    Ok(state.returned)
}

pub(crate) fn run_command_reader(bridge: &crate::command_reader_ui_demo::Bridge) -> io::Result<()> {
    run_mode(false, || {}, Some(DemoBridge::CommandReader(bridge)))
}

pub(crate) fn run_command(bridge: &crate::command_ui_demo::Bridge) -> io::Result<()> {
    run_mode(false, || {}, Some(DemoBridge::Command(bridge)))
}

pub(crate) fn project_state(deadline: Instant) -> io::Result<CodingUi> {
    let mut state = CodingUi::new()?;
    state.deadline = deadline;
    Ok(state)
}
pub(crate) fn run_project(
    bridge: &crate::project_ui_demo::Bridge,
    state: &mut CodingUi,
) -> io::Result<CodingReturn> {
    state.returned = CodingReturn::Exit;
    run_interval(false, || {}, Some(DemoBridge::Project(bridge)), Some(state))?;
    Ok(state.returned)
}

pub(crate) fn run_haiku(
    bridge: &crate::haiku_ui_demo::Bridge,
    state: &mut CodingUi,
) -> io::Result<CodingReturn> {
    state.returned = CodingReturn::Exit;
    run_interval(false, || {}, Some(DemoBridge::Haiku(bridge)), Some(state))?;
    Ok(state.returned)
}

#[derive(Clone, Copy)]
enum DemoBridge<'a> {
    Haiku(&'a crate::haiku_ui_demo::Bridge),
    Project(&'a crate::project_ui_demo::Bridge),
    Runtime(&'a crate::runtime_ui_demo::Bridge),
    Reader(&'a crate::reader_ui_demo::Bridge),
    Coding(&'a crate::coding_ui_demo::Bridge),
    Command(&'a crate::command_ui_demo::Bridge),
    CommandReader(&'a crate::command_reader_ui_demo::Bridge),
}
impl DemoBridge<'_> {
    fn is_command(self) -> bool {
        matches!(self, Self::Command(_) | Self::CommandReader(_))
    }
    fn command_error(self) -> bool {
        match self {
            Self::Haiku(b) => {
                let p = b.snapshot();
                p.error.is_some() || p.snapshot.failure.is_some()
            }
            Self::Project(b) => {
                let p = b.snapshot();
                p.error.is_some() || p.snapshot.error.is_some()
            }
            Self::Command(b) => b.snapshot().error.is_some(),
            Self::CommandReader(b) => b.snapshot().error.is_some(),
            _ => false,
        }
    }
    fn command_painted(self, offer: Option<crate::command_ui_demo::Offer>) {
        match self {
            Self::Command(b) => b.painted(offer),
            Self::CommandReader(b) => b.painted(offer),
            _ => {}
        }
    }
    fn command_invoke(self, offer: Option<crate::command_ui_demo::Offer>) {
        match self {
            Self::Command(b) => {
                b.invoke(offer);
            }
            Self::CommandReader(b) => {
                b.invoke(offer);
            }
            _ => {}
        }
    }
    fn stop(self) {
        match self {
            Self::Haiku(bridge) => bridge.stop(),
            Self::Project(bridge) => bridge.stop(),
            Self::Runtime(bridge) => bridge.stop(),
            Self::Reader(bridge) => bridge.stop(),
            Self::Coding(bridge) => bridge.stop(),
            Self::Command(bridge) => bridge.stop(),
            Self::CommandReader(bridge) => bridge.stop(),
        }
    }
}

fn run_mode(
    diagnostic_enabled: bool,
    on_ready: impl FnOnce(),
    bridge: Option<DemoBridge<'_>>,
) -> io::Result<()> {
    run_interval(diagnostic_enabled, on_ready, bridge, None)
}

fn run_interval(
    diagnostic_enabled: bool,
    on_ready: impl FnOnce(),
    bridge: Option<DemoBridge<'_>>,
    mut coding: Option<&mut CodingUi>,
) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("requires TTY stdin and stdout"));
    }
    if coding.as_ref().is_some_and(|s| !s.live()) {
        return Ok(());
    }
    let tty = rustix::fs::open(
        "/dev/tty",
        OFlags::RDWR | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let termios = rustix::termios::tcgetattr(&tty)?;
    let retained = coding.as_mut().and_then(|s| s.model.take());
    let reentry = retained.is_some();
    let mut model = retained.unwrap_or_else(|| {
        if bridge.is_some() {
            Model::runtime()
        } else {
            Model::new()
        }
    });
    model.clear_selection();
    if matches!(bridge, Some(DemoBridge::Coding(_) | DemoBridge::Project(_))) && !reentry {
        model.editor.clear();
        model.caret = 0;
    }
    if matches!(bridge, Some(DemoBridge::Haiku(_))) && !reentry {
        model.editor = scramjet_rs::haiku_prompt();
        model.caret = model.editor.len();
    }
    let output = if bridge.is_some() {
        Output::tty()?
    } else {
        Output::Stdout
    };
    let mut diagnostic = diagnostic_enabled.then(|| diagnostic::Diagnostic::new(&model));
    let mut decoder = Decoder::default();
    let stopped = coding
        .as_ref()
        .map_or_else(|| Arc::new(AtomicBool::new(false)), |s| s.stopped.clone());
    let watchdog = if let Some(DemoBridge::Project(bridge)) = bridge {
        bridge.deadline
    } else {
        coding
            .as_ref()
            .map_or_else(|| Instant::now() + Duration::from_secs(30), |s| s.deadline)
    };
    let settled = coding.as_ref().is_some_and(|s| s.read_only());
    let mut intermission = false;
    let mut checkpoint = false;
    let mut guard = Guard {
        tty,
        termios,
        raw: false,
        modes: false,
        signals: Vec::with_capacity(4),
        output,
    };
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGTSTP,
    ] {
        if coding.is_none() {
            guard
                .signals
                .push(signal_hook::flag::register(signal, Arc::clone(&stopped))?);
        }
    }
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut terminal = None;
    let mut cancel_requested = false;
    let mut approval = coding_approval::Approval::default();
    let mut approval_ingress = None;
    let mut approval_geometry_ingress = (0, 0);
    let mut command_displayed = None;
    let mut command_ingress = None;
    let mut coding_displayed = None;
    let mut coding_ingress = None;
    let mut project_displayed: Option<crate::project_ui_demo::Offer> = None;
    let mut project_ingress = None;
    let mut project_history = coding
        .as_mut()
        .and_then(|s| s.project_history.take())
        .unwrap_or_default();
    let mut haiku = coding
        .as_mut()
        .and_then(|s| s.haiku.take())
        .unwrap_or_default();
    let mut haiku_displayed = None;
    let mut haiku_ingress = None;
    let mut haiku_approval_ingress = None;
    let mut haiku_geometry_ingress = (0, 0);
    let mut selected_child = 0u8;
    let mut child_cancel_mask = 0u8;
    let mut history = coding_history::History::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
        let (width, height) = crossterm::terminal::size()?;
        let (width, height) = clamp(width, height);
        guard.output.begin();
        terminal = Some(Terminal::with_options(
            CrosstermBackend::new(guard.output.clone()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, width, height)),
            },
        )?);
        let terminal = terminal.as_mut().unwrap();
        let mut raw = guard.termios.clone();
        raw.make_raw();
        guard.raw = true;
        rustix::termios::tcsetattr(&guard.tty, OptionalActions::Now, &raw)?;
        guard.modes = true;
        guard.output.begin();
        guard.output.write_all(ENABLE)?;
        guard.output.flush()?;
        let mut staging = [0; 1024];
        let (mut offset, mut length) = (0, 0);
        let mut next_tick = Instant::now() + Duration::from_millis(50);
        let mut next_paint = Instant::now();
        let mut on_ready = Some(on_ready);
        loop {
            if stopped.load(Ordering::Relaxed) {
                break;
            }
            if !settled && bridge.is_some_and(DemoBridge::command_error) {
                return Err(io::Error::other(
                    "command journey failed; terminal restoration requested",
                ));
            }
            let now = Instant::now();
            decoder.check_deadline(now).map_err(io::Error::other)?;
            if bridge.is_some() && now >= watchdog {
                return Err(io::Error::other(
                    if matches!(bridge, Some(DemoBridge::Project(_))) {
                        "120s project deadline requested Stop"
                    } else {
                        "30s demo watchdog requested Stop"
                    },
                ));
            }
            if bridge.is_none() && now >= next_tick {
                model.tick();
                next_tick = now + Duration::from_millis(50);
            }
            if offset == length {
                match rustix::io::read(&guard.tty, &mut staging) {
                    Ok(0) => return Err(io::Error::other("terminal input closed")),
                    Ok(n) => {
                        offset = 0;
                        length = n;
                        if let Some(observer) = &mut diagnostic {
                            observer.read(n);
                        }
                    }
                    Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            let mut events = 0;
            while offset < length && events < 64 {
                if let Some(observer) = &mut diagnostic {
                    observer.fed();
                }
                if decoder.idle() {
                    approval_ingress = approval.displayed.clone();
                    approval_geometry_ingress = (model.display.width, model.display.height);
                    command_ingress = command_displayed;
                    coding_ingress = coding_displayed;
                    project_ingress = project_displayed;
                    haiku_ingress = haiku_displayed;
                    haiku_approval_ingress = haiku.ingress();
                    haiku_geometry_ingress = (model.display.width, model.display.height);
                }
                let event = decoder
                    .feed(staging[offset], now)
                    .map_err(io::Error::other)?;
                offset += 1;
                if let Some(event) = event {
                    events += 1;
                    if let Some(DemoBridge::Haiku(bridge)) = bridge {
                        if event == Event::CancelAll {
                            stopped.store(true, Ordering::Relaxed);
                            bridge.stop();
                            return Ok(());
                        }
                        let (w, h) = crossterm::terminal::size()?;
                        let geometry = clamp(w, h) == (model.display.width, model.display.height)
                            && clamp(w, h) == haiku_geometry_ingress;
                        if event == Event::HistoryPrevious
                            && !haiku.active()
                            && !haiku.pending()
                            && !settled
                            && bridge.snapshot().snapshot.run_ended
                            && model.focused
                            && offset == length
                            && decoder.idle()
                        {
                            intermission = true;
                            return Ok(());
                        }
                        if geometry {
                            if let Some((proposal, approve)) =
                                haiku.event(&event, &model, haiku_approval_ingress, w, h)
                            {
                                bridge.decide(&proposal, approve);
                            }
                        } else {
                            haiku.disarm();
                        }
                        if event == Event::Submit {
                            if model.editor != scramjet_rs::haiku_prompt() {
                                bridge.unsupported();
                                continue;
                            }
                            if !settled
                                && model.focused
                                && model.focus == Focus::Editor
                                && model.anchor.is_none()
                                && !haiku.pending()
                                && !haiku.active()
                                && w >= 80
                                && h >= 24
                                && geometry
                                && !model.display.exhausted
                            {
                                bridge.submit(haiku_ingress, model.revision, &model.editor);
                            } else {
                                bridge.reject();
                            }
                            continue;
                        }
                        if matches!(
                            event,
                            Event::Focus(_)
                                | Event::Tab
                                | Event::PageUp
                                | Event::PageDown
                                | Event::Mouse { .. }
                        ) {
                            haiku_displayed = None;
                            bridge.painted(None);
                            haiku.disarm();
                        }
                        if haiku.suppresses(&event) {
                            continue;
                        }
                    }
                    if let Some(DemoBridge::Project(bridge)) = bridge {
                        project_history.observe(&bridge.snapshot());
                        if !approval.pending() && project_history.event(&event, &mut model) {
                            project_displayed = None;
                            bridge.painted(None);
                            continue;
                        }
                        if !approval.pending() && project_history.suppresses(&event) {
                            continue;
                        }
                        if event == Event::Follow
                            && model.focused
                            && model.focus == Focus::Transcript
                            && !approval.pending()
                            && !project_history.active()
                        {
                            model.event(Event::Follow);
                            continue;
                        }
                        if matches!(event, Event::Submit | Event::Follow) {
                            let (w, h) = crossterm::terminal::size()?;
                            let eligible = !settled
                                && model.focused
                                && model.focus == Focus::Editor
                                && model.anchor.is_none()
                                && !approval.pending()
                                && !project_history.active()
                                && w >= 80
                                && h >= 24
                                && clamp(w, h) == (model.display.width, model.display.height)
                                && !model.display.exhausted;
                            let intent = if event == Event::Submit {
                                crate::project_ui_demo::Intent::Request
                            } else {
                                crate::project_ui_demo::Intent::Verify
                            };
                            if eligible && project_ingress.is_some_and(|o| o.intent == intent) {
                                bridge.invoke(project_ingress, model.revision, &model.editor);
                            } else {
                                bridge.reject();
                            }
                            continue;
                        }
                        if matches!(
                            event,
                            Event::Focus(_)
                                | Event::Tab
                                | Event::PageUp
                                | Event::PageDown
                                | Event::Mouse { .. }
                        ) {
                            project_displayed = None;
                            bridge.painted(None);
                        }
                    }
                    if let Some(DemoBridge::Coding(bridge)) = bridge {
                        history.observe(&bridge.snapshot());
                        if !approval.pending() && history.event(&event, &mut model) {
                            continue;
                        }
                        if !approval.pending() && history.suppresses(&event) {
                            continue;
                        }
                        if event == Event::Submit {
                            if settled {
                                bridge.reject(
                                    coding_ingress,
                                    if bridge.snapshot().follow_up {
                                        "TwoInvocationLimit"
                                    } else {
                                        "TwoJourneyLimit"
                                    },
                                );
                            } else {
                                let (w, h) = crossterm::terminal::size()?;
                                model.submit(bridge, coding_ingress, approval.pending(), w, h);
                            }
                        }
                        if matches!(
                            event,
                            Event::Focus(_)
                                | Event::Tab
                                | Event::PageUp
                                | Event::PageDown
                                | Event::Mouse { .. }
                        ) {
                            coding_displayed = None;
                            bridge.painted(None);
                        }
                    }
                    if model.focused
                        && let Some(bridge) = bridge
                    {
                        match (bridge, &event) {
                            (DemoBridge::Project(bridge), Event::CancelAll) => {
                                stopped.store(true, Ordering::Relaxed);
                                bridge.stop();
                                return Ok(());
                            }
                            (DemoBridge::Project(bridge), _) => {
                                let (w, h) = crossterm::terminal::size()?;
                                if clamp(w, h) == (model.display.width, model.display.height)
                                    && clamp(w, h) == approval_geometry_ingress
                                    && let Some((proposal, decision)) =
                                        approval.event(&event, &model, &approval_ingress, w, h)
                                {
                                    bridge.decide(&proposal, decision);
                                }
                                if event == Event::HistoryJourney && !project_history.active() {
                                    selected_child = (selected_child + 1) % 3;
                                }
                                if event == Event::CancelChild && !project_history.active() {
                                    bridge.cancel_child(selected_child);
                                }
                            }
                            (DemoBridge::Runtime(bridge), Event::CancelAll) => {
                                bridge.cancel();
                                cancel_requested = true;
                            }
                            (DemoBridge::Reader(bridge), Event::CancelAll) => {
                                bridge.stop();
                                cancel_requested = true;
                            }
                            (DemoBridge::Coding(bridge), Event::CancelAll) => {
                                stopped.store(true, Ordering::Relaxed);
                                bridge.stop();
                                return Ok(());
                            }
                            (DemoBridge::Coding(bridge), _) => {
                                let (w, h) = crossterm::terminal::size()?;
                                if let Some((proposal, decision)) =
                                    approval.event(&event, &model, &approval_ingress, w, h)
                                {
                                    bridge.decide(&proposal, decision);
                                }
                            }
                            (
                                DemoBridge::Command(_) | DemoBridge::CommandReader(_),
                                Event::NextChild,
                            ) => {
                                let (w, h) = crossterm::terminal::size()?;
                                if w >= 80
                                    && h >= 24
                                    && (clamp(w, h)) == (model.display.width, model.display.height)
                                    && !model.display.exhausted
                                {
                                    bridge.command_invoke(command_ingress);
                                }
                            }
                            (
                                DemoBridge::Command(_) | DemoBridge::CommandReader(_),
                                Event::CancelAll,
                            ) => {
                                bridge.stop();
                                cancel_requested = true;
                            }
                            (DemoBridge::CommandReader(_), Event::Approve) => {
                                selected_child = (selected_child + 1) % 3
                            }
                            (DemoBridge::CommandReader(bridge), Event::CancelChild) => {
                                bridge.cancel_child(selected_child);
                                child_cancel_mask |= 1 << selected_child;
                            }
                            (DemoBridge::Reader(_), Event::NextChild) => {
                                selected_child = (selected_child + 1) % 3
                            }
                            (DemoBridge::Reader(bridge), Event::CancelChild) => {
                                bridge.cancel_child(selected_child);
                                child_cancel_mask |= 1 << selected_child;
                            }
                            _ => {}
                        }
                    }
                    if approval.pending()
                        && model.focus == Focus::Transcript
                        && matches!(event, Event::Copy | Event::Mouse { .. })
                    {
                        continue;
                    }
                    if matches!(event, Event::Focus(_) | Event::Tab) {
                        if let Some(bridge) = bridge.filter(|b| b.is_command()) {
                            command_displayed = None;
                            bridge.command_painted(None);
                        }
                        approval.armed = false;
                        approval.displayed = None;
                    }
                    guard.output.begin();
                    if dispatch(&mut model, &mut diagnostic, event, &mut guard.output)? {
                        stopped.store(true, Ordering::Relaxed);
                        return Ok(());
                    }
                }
            }
            if let Some(DemoBridge::Project(bridge)) = bridge {
                let p = bridge.snapshot();
                let ended_without_success = p
                    .snapshot
                    .current
                    .and_then(|id| {
                        p.snapshot.journeys[id.journey() as usize - 1]
                            .coding
                            .as_ref()
                    })
                    .is_some_and(|o| o.run_ended && !o.verified);
                if !settled
                    && offset == length
                    && decoder.idle()
                    && (ended_without_success
                        || (p.snapshot.phase == scramjet_rs::ProjectPhase::Verified
                            && p.snapshot.current.is_some_and(|id| id.journey() == 2)))
                {
                    intermission = true;
                    return Ok(());
                }
            }
            if let Some(DemoBridge::Coding(bridge)) = bridge {
                let projection = bridge.snapshot();
                if !settled && offset == length && decoder.idle() {
                    if projection.needs_settlement {
                        intermission = true;
                        checkpoint = projection.needs_checkpoint;
                        return Ok(());
                    }
                    if projection.error.is_some()
                        || projection
                            .observation
                            .as_ref()
                            .is_some_and(|o| o.error.is_some())
                    {
                        if !projection.settlement_failed {
                            intermission = true;
                            return Ok(());
                        }
                        return Err(io::Error::other(
                            "coding journey failed; terminal restoration requested",
                        ));
                    }
                }
            }
            if now >= next_paint && offset == length {
                let (width, height) = crossterm::terminal::size()?;
                let (width, height) = clamp(width, height);
                let geometry_changed =
                    (width, height) != (model.display.width, model.display.height);
                if geometry_changed {
                    haiku.disarm();
                    if let Some(DemoBridge::Haiku(bridge)) = bridge {
                        bridge.painted(None);
                        haiku_displayed = None;
                    }
                    let record = diagnostic
                        .as_mut()
                        .map(|observer| observer.resize(&model, width, height));
                    model.clear_selection();
                    guard.output.begin();
                    let result = terminal.resize(Rect::new(0, 0, width, height));
                    if let Some(record) = record {
                        diagnostic.as_mut().unwrap().after(
                            record,
                            &model,
                            diagnostic::CopyOutcome::NotCopy,
                        );
                    }
                    result?;
                }
                let mut command_offer = None;
                let mut coding_offer = None;
                let mut project_offer = None;
                let mut haiku_offer = None;
                match bridge {
                    Some(DemoBridge::Haiku(bridge)) => {
                        let projection = bridge.snapshot();
                        haiku.observe(&projection, settled);
                        haiku.project(&projection, &mut model, settled);
                        if haiku.pending() {
                            model.clear_selection();
                        }
                        if !settled
                            && projection.offered
                            && model.focused
                            && model.focus == Focus::Editor
                            && model.anchor.is_none()
                            && !haiku.pending()
                            && !haiku.active()
                        {
                            haiku_offer = Some(crate::haiku_ui_demo::Offer {
                                revision: model.revision,
                            });
                        }
                    }
                    Some(DemoBridge::Project(bridge)) => {
                        let projection = bridge.snapshot();
                        #[cfg(test)]
                        if std::env::var_os("S413B_UI_PANIC").is_some()
                            && projection.snapshot.phase == scramjet_rs::ProjectPhase::Readers
                        {
                            panic!("synthetic project presentation panic");
                        }
                        let pending = projection
                            .snapshot
                            .current
                            .and_then(|id| {
                                projection.snapshot.journeys[id.journey() as usize - 1]
                                    .coding
                                    .as_ref()
                            })
                            .filter(|o| o.decision.is_none() && !o.run_ended && o.error.is_none())
                            .and_then(|o| o.proposal.clone());
                        approval.observe(pending);
                        project_history.observe(&projection);
                        if approval.pending() {
                            model.clear_selection();
                        }
                        model.project_projection(&projection, selected_child);
                        if model.focused
                            && model.focus == Focus::Editor
                            && model.anchor.is_none()
                            && !approval.pending()
                            && !project_history.active()
                        {
                            project_offer = projection.offer.map(|mut offer| {
                                offer.revision = model.revision;
                                offer
                            });
                        }
                    }
                    Some(DemoBridge::Command(bridge)) => {
                        let projection = bridge.snapshot();
                        command_offer = projection.offer;
                        model.command_projection(&projection)?;
                    }
                    Some(DemoBridge::CommandReader(bridge)) => {
                        let projection = bridge.snapshot();
                        command_offer = projection.offer;
                        model.command_reader_projection(
                            &projection,
                            selected_child,
                            child_cancel_mask,
                            cancel_requested,
                        )?;
                    }
                    Some(DemoBridge::Runtime(bridge)) => {
                        model.runtime_projection(&bridge.snapshot(), cancel_requested)
                    }
                    Some(DemoBridge::Reader(bridge)) => model.reader_projection(
                        &bridge.snapshot(),
                        selected_child,
                        child_cancel_mask,
                        cancel_requested,
                    ),
                    Some(DemoBridge::Coding(bridge)) => {
                        let projection = bridge.snapshot();
                        let pending = projection
                            .observation
                            .as_ref()
                            .filter(|o| o.decision.is_none() && !o.run_ended && o.error.is_none())
                            .and_then(|o| o.proposal.clone());
                        approval.observe(if settled { None } else { pending });
                        history.observe(&projection);
                        if approval.pending() {
                            model.clear_selection();
                        }
                        model.coding_projection(&projection);
                        if model.focused
                            && model.focus == Focus::Editor
                            && model.anchor.is_none()
                            && !approval.pending()
                            && !history.active()
                            && !settled
                        {
                            coding_offer =
                                projection.offer.map(|id| crate::coding_ui_demo::Offer {
                                    id,
                                    revision: model.revision,
                                });
                        }
                    }
                    None => {}
                }
                let display = model.project(width, height);
                if !model.focused || width < 80 || height < 24 || display.exhausted {
                    command_offer = None;
                    coding_offer = None;
                    project_offer = None;
                    haiku_offer = None;
                    haiku.disarm();
                }
                if !decoder.idle() && (haiku_offer != haiku_displayed || geometry_changed) {
                    haiku_offer = None;
                    haiku.disarm();
                }
                if !decoder.idle() && (project_offer != project_displayed || geometry_changed) {
                    project_offer = None;
                }
                if !decoder.idle() && (command_offer != command_displayed || geometry_changed) {
                    command_offer = None;
                }
                guard.output.begin();
                let history_overlay = !approval.pending();
                if draw(
                    terminal,
                    &mut model,
                    &mut diagnostic,
                    display,
                    Overlays {
                        approval: Some(&mut approval),
                        history: history_overlay.then_some(&history),
                        project_history: history_overlay.then_some(&project_history),
                        haiku: if matches!(bridge, Some(DemoBridge::Haiku(_))) {
                            Some(&mut haiku)
                        } else {
                            None
                        },
                    },
                )? {
                    rustix::termios::tcflush(&guard.tty, QueueSelector::IFlush)?;
                    decoder = Decoder::default();
                    approval_ingress = None;
                }
                if let Some(DemoBridge::Haiku(bridge)) = bridge {
                    haiku_offer = haiku_offer.filter(|o| haiku.submission_visible(o.revision));
                    if haiku_offer.is_some() && haiku_offer != haiku_displayed {
                        rustix::termios::tcflush(&guard.tty, QueueSelector::IFlush)?;
                        decoder = Decoder::default();
                        haiku_ingress = None;
                    }
                    haiku_displayed = haiku_offer;
                    bridge.painted(haiku_offer);
                }
                if let Some(DemoBridge::Project(bridge)) = bridge {
                    if project_offer.is_some() && project_displayed.is_none() {
                        rustix::termios::tcflush(&guard.tty, QueueSelector::IFlush)?;
                        decoder = Decoder::default();
                        project_ingress = None;
                    }
                    project_displayed = project_offer;
                    bridge.painted(project_offer);
                }
                if let Some(bridge) = bridge.filter(|b| b.is_command()) {
                    if command_offer.is_some() && command_offer != command_displayed {
                        rustix::termios::tcflush(&guard.tty, QueueSelector::IFlush)?;
                        decoder = Decoder::default();
                        command_ingress = None;
                    }
                    command_displayed = command_offer;
                    bridge.command_painted(command_offer);
                }
                if let Some(DemoBridge::Coding(bridge)) = bridge {
                    if coding_offer.is_some() && coding_displayed.is_none() {
                        rustix::termios::tcflush(&guard.tty, QueueSelector::IFlush)?;
                        decoder = Decoder::default();
                        coding_ingress = None;
                    }
                    coding_displayed = coding_offer;
                    bridge.painted(coding_offer);
                }
                if let Some(on_ready) = on_ready.take() {
                    on_ready();
                }
                next_paint = now + Duration::from_millis(50);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }));
    if let Some(bridge) = bridge
        && !(intermission
            && (checkpoint
                || matches!(bridge, DemoBridge::Project(_) | DemoBridge::Haiku(_))
                || matches!(bridge, DemoBridge::Coding(b) if b.command_mode()))
            && matches!(&result, Ok(Ok(()))))
    {
        bridge.stop();
    }
    if let Some(DemoBridge::Haiku(bridge)) = bridge
        && !matches!(&result, Ok(Ok(())))
    {
        bridge.stop();
    }
    let restoration = guard.restore();
    let cursor_restoration = terminal.as_mut().map_or(Ok(()), |terminal| {
        guard.output.begin();
        terminal.show_cursor()
    });
    if bridge.is_some() {
        // Disarm destructor retries after recording the actual restoration result.
        guard.output.quiesce();
        if let Some(terminal) = &mut terminal {
            let _ = terminal.show_cursor();
        }
    }
    drop(terminal);
    std::panic::set_hook(panic_hook);
    if let Some(observer) = &mut diagnostic {
        let execution = match &result {
            Ok(Ok(())) => "ok",
            Ok(Err(_)) => "error",
            Err(_) => "panic",
        };
        observer.finish(
            &model,
            execution != "ok" || restoration.is_err() || cursor_restoration.is_err(),
        );
        observer.report(
            &mut io::stdout(),
            execution,
            restoration.is_ok(),
            cursor_restoration.is_ok(),
        )?;
    }
    if let Some(state) = &mut coding {
        state.returned = if intermission
            && restoration.is_ok()
            && cursor_restoration.is_ok()
            && matches!(&result, Ok(Ok(())))
        {
            if checkpoint {
                CodingReturn::Checkpoint
            } else {
                CodingReturn::Settlement
            }
        } else {
            if let Some(bridge) = bridge {
                bridge.stop();
            }
            state.stopped.store(true, Ordering::Relaxed);
            CodingReturn::Exit
        };
        model.clear_selection();
        state.model = Some(model);
        state.project_history = Some(project_history);
        state.haiku = Some(haiku);
    }
    let execution = match result {
        Ok(result) => result,
        Err(_) => Err(io::Error::other("UI panic; terminal restoration attempted")),
    };
    let errors = [execution, restoration, cursor_restoration]
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.to_string())
        .collect::<Vec<_>>();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}

#[cfg(test)]
pub(crate) fn check_project_history(
    before: &crate::project_ui_demo::Projection,
    after: &crate::project_ui_demo::Projection,
) {
    let history = project_projection::check_keyed_history(before, after);
    resources::project_presentation_probe(after);
    let mut state = project_state(Instant::now() + Duration::from_secs(120)).unwrap();
    state.project_history = Some(history);
    let mut model = Model::runtime();
    model.editor = "left界right".into();
    model.caret = 7;
    model.revision = 9;
    state.model = Some(model);
    state.mode(true);
    assert!(state.project_history.as_ref().unwrap().active());
    let retained = state.project_history.take().unwrap();
    assert!(retained.active());
    let mut model = state.model.take().unwrap();
    assert_eq!(
        (model.editor.as_str(), model.caret, model.revision),
        ("left界right", 7, 9)
    );
    model.project_projection(before, 0);
    let display = model.project(200, 60);
    model.displayed(display);
    model.focus = Focus::Transcript;
    model.event(Event::Mouse {
        button: 0,
        x: 0,
        y: 0,
        release: false,
    });
    model.event(Event::Mouse {
        button: 32,
        x: 8,
        y: 0,
        release: false,
    });
    model.event(Event::Mouse {
        button: 0,
        x: 8,
        y: 0,
        release: true,
    });
    let display = model.project(200, 60);
    model.displayed(display);
    let copied = model.copied.clone();
    assert!(!copied.is_empty());
    model.project_projection(after, 0);
    let display = model.project(200, 60);
    model.displayed(display);
    assert_eq!(model.copied, copied);
    model.event(Event::PageUp);
    let anchor = model.anchor;
    model.event(Event::Tab);
    model.event(Event::Text("e\u{301}".into()));
    assert_eq!(model.anchor, anchor);
    assert_eq!(model.revision, 10);
}

#[cfg(test)]
pub(crate) fn check_reentry_boundaries(p: &crate::coding_ui_demo::Projection) {
    let state = || CodingUi {
        model: Some(Model::runtime()),
        stopped: Arc::new(AtomicBool::new(false)),
        signals: Vec::new(),
        deadline: Instant::now() + Duration::from_secs(30),
        returned: CodingReturn::Settlement,
        settled: false,
        project_history: None,
        haiku: None,
    };
    let mut eligible = state();
    assert!(eligible.live() && p.settled());
    eligible.mode(true);
    assert!(eligible.read_only());
    let user_exit = state();
    user_exit.stopped.store(true, Ordering::Relaxed);
    assert!(!user_exit.live());
    let mut deadline = state();
    deadline.deadline = Instant::now();
    assert!(!deadline.live());
    let terminal_error = state();
    terminal_error.stopped.store(true, Ordering::Relaxed);
    assert!(!terminal_error.live());
    for mode in 0..6 {
        let mut invalid = p.clone();
        match mode {
            0 => invalid.history = None,
            1 => invalid.settlement_failed = true,
            2 => invalid.retained_root = Some("synthetic retained root".into()),
            3 => invalid.reconstructed = false,
            4 => invalid.fixture = None,
            _ => {
                let o = Arc::make_mut(invalid.observation.as_mut().unwrap());
                o.worker_joined = false;
            }
        }
        assert!(!invalid.settled());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) static SPAWN_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

    fn paint(model: &mut Model, width: u16, height: u16) -> Buffer {
        let display = model.project(width, height);
        let mut buffer = Buffer::empty(Rect::new(0, 0, display.width, display.height));
        model.paint(&display, &mut buffer);
        model.displayed(display);
        buffer
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn panic_restoration_pty() {
        if std::env::var_os("S12_PANIC_CHILD").is_some() {
            let error = run_with(std::env::var_os("S13_DIAGNOSTIC_CHILD").is_some(), || {
                panic!("synthetic panic")
            })
            .unwrap_err();
            eprintln!("ui-demo: {error}");
            return;
        }
        let _spawn = SPAWN_GATE.write().unwrap();
        let output = std::process::Command::new("timeout")
            .args(["15", "python3", "tests/ui_pty.py"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "panic",
                "panic_hidden",
                "panic_hidden_fault",
                "diagnostic_panic",
                "diagnostic_panic_hidden",
                "diagnostic_panic_hidden_fault",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn exact_width_editor_insertion_positions() {
        for (text, before, expected_before, inserted_before, inserted_after) in [
            ("leftrightx", 9, 9, "leftrightyx", "leftrightxy"),
            ("12345678界", 8, 8, "12345678y界", "12345678界y"),
        ] {
            let mut model = Model::new();
            model.editor = text.into();
            model.caret = before;
            let buffer = paint(&mut model, 10, 12);
            assert_eq!(model.cursor(&model.display), Some((expected_before, 6)));
            assert_eq!(buffer[(expected_before, 6)].symbol(), &text[before..]);
            model.event(Event::Right);
            paint(&mut model, 10, 12);
            assert_eq!(model.caret, text.len());
            assert_eq!(model.cursor(&model.display), Some((0, 7)));
            model.event(Event::Left);
            assert_eq!(model.caret, before);
            model.insert("y");
            assert_eq!(model.editor, inserted_before);
            model.editor = text.into();
            model.caret = text.len();
            model.insert("y");
            let buffer = paint(&mut model, 10, 12);
            assert_eq!(model.editor, inserted_after);
            assert_eq!(buffer[(0, 7)].symbol(), "y");
            assert_eq!(model.cursor(&model.display), Some((1, 7)));
        }
        let mut model = Model::new();
        model.editor = "1234567890\nright".into();
        model.caret = 10;
        paint(&mut model, 10, 12);
        assert_eq!(model.cursor(&model.display), Some((0, 7)));
        model.event(Event::Right);
        assert_eq!(model.caret, 11);
        paint(&mut model, 10, 12);
        assert_eq!(model.cursor(&model.display), Some((0, 8)));
        model.event(Event::Left);
        model.insert("hé\n界🙂");
        assert_eq!(model.editor, "1234567890hé\n界🙂\nright");
        assert_eq!(model.caret, "1234567890hé\n界🙂".len());
        let buffer = paint(&mut model, 10, 12);
        assert_eq!(buffer[(0, 8)].symbol(), "界");
        assert_eq!(model.cursor(&model.display), Some((4, 8)));
        model.editor = "x".repeat(EDITOR_LIMIT);
        model.caret = model.editor.len();
        paint(&mut model, 16, 12);
        assert_eq!(model.cursor(&model.display), Some((0, 8)));
        assert_eq!(model.editor.len(), EDITOR_LIMIT);
        assert_eq!(wrap("1234567890", 0, 10).len(), 1);
    }

    #[test]
    fn deletion_merge_and_painter_width_regressions() {
        let mut model = Model::new();
        model.editor = "a\n\u{301}b".into();
        model.caret = 2;
        model.event(Event::Backspace);
        assert_eq!(model.editor, "áb");
        assert_eq!(model.caret, "á".len());
        model.lines.clear();
        model.append("123456789ｶﾞﾊﾟ").unwrap();
        let buffer = paint(&mut model, 10, 12);
        assert_eq!(buffer[(0, 1)].symbol(), "ｶﾞ");
        model.selection = Some(((1, 1), (1, 1)));
        paint(&mut model, 10, 12);
        assert_eq!(model.copied, "ｶﾞ");
    }

    #[test]
    fn repeated_selection_after_paste_focus_and_streaming() {
        let mut model = Model::new();
        paint(&mut model, 80, 24);
        let caret = model.caret;
        for turn in 0..30 {
            for event in [
                Event::Mouse {
                    button: 0,
                    x: 0,
                    y: 1,
                    release: false,
                },
                Event::Mouse {
                    button: 32,
                    x: 15,
                    y: 2,
                    release: false,
                },
                Event::Mouse {
                    button: 0,
                    x: 15,
                    y: 2,
                    release: true,
                },
            ] {
                model.event(event);
            }
            let buffer = paint(&mut model, 80, 24);
            assert!(!model.copied.is_empty(), "selection {turn}");
            assert_eq!(buffer[(0, 1)].bg, Color::Blue);
            assert_eq!(
                model.caret,
                if turn == 0 { caret } else { "leftpaste".len() }
            );
            let selected = model.copied.clone();
            model.tick();
            paint(&mut model, 80, 24);
            assert_eq!(model.copied, selected);
            model.editor.clear();
            model.editor.push_str("leftright");
            model.caret = 4;
            model.event(Event::Paste("paste".into()));
            assert_eq!(model.editor, "leftpasteright");
            model.event(Event::Focus(false));
            assert!(!model.dragging);
            assert!(model.selection.is_none());
            model.event(Event::Focus(true));
            paint(&mut model, 80, 24);
        }
    }

    #[test]
    fn resize_keeps_row_containing_source_anchor() {
        let mut model = Model::new();
        model.lines.clear();
        model.append(&"x".repeat(90)).unwrap();
        model.anchor = Some((model.lines[0].id, 20));
        paint(&mut model, 10, 12);
        assert_eq!(model.display.rows[0].start, 20);
        paint(&mut model, 15, 12);
        assert_eq!(model.display.rows[0].start, 15);
        paint(&mut model, 11, 12);
        assert_eq!(model.display.rows[0].start, 11);
        assert_eq!(model.caret, 4);
    }

    #[test]
    fn interior_paste_normalization_and_atomic_rejection() {
        let mut model = Model::new();
        model.event(Event::Paste("hé\r\n界🙂".into()));
        assert_eq!(model.editor, "lefthé\n界🙂right");
        assert_eq!(model.caret, "lefthé\n界🙂".len());
        let before = (model.editor.clone(), model.caret);
        for text in ["\x1b[A".into(), "x".repeat(8192), "\t".repeat(2048)] {
            model.insert(&text);
            assert_eq!((model.editor.clone(), model.caret), before);
        }
        model.insert("\r\t");
        assert!(model.editor.contains("\n    right"));
    }

    #[test]
    fn editing_graphemes_and_focus_precedence() {
        let mut model = Model::new();
        model.editor = "é界👩‍💻".into();
        model.caret = model.editor.len();
        model.event(Event::Left);
        assert_eq!(model.caret, "é界".len());
        model.event(Event::Backspace);
        assert_eq!(model.editor, "é👩‍💻");
        model.event(Event::Delete);
        assert_eq!(model.editor, "é");
        model.event(Event::Home);
        assert_eq!(model.caret, 0);
        model.event(Event::End);
        assert_eq!(model.caret, model.editor.len());
        model.event(Event::Focus(false));
        model.event(Event::Text("bad".into()));
        assert_eq!(model.editor, "é");
        model.event(Event::Focus(true));
        model.event(Event::Tab);
        model.event(Event::Text("bad".into()));
        assert_eq!(model.editor, "é");
        model.event(Event::Paste("\nhi".into()));
        assert_eq!(model.editor, "é\nhi");
        model.event(Event::Tab);
        model.event(Event::Up);
        assert_eq!(model.caret, "é".len());
    }

    #[test]
    fn displayed_selection_wide_edges_hard_and_soft_wraps() {
        let mut model = Model::new();
        model.lines.clear();
        model.append("é界👩‍💻abcdef").unwrap();
        model.append("next").unwrap();
        let buffer = paint(&mut model, 10, 12);
        assert_eq!(buffer[(0, 0)].symbol(), "é");
        assert_eq!(buffer[(1, 0)].symbol(), "界");
        for x in [1, 2, 3, 4] {
            model.selection = Some(((x, 0), (x, 0)));
            paint(&mut model, 10, 12);
            assert_eq!(model.copied, if x < 3 { "界" } else { "👩‍💻" });
        }
        model.selection = Some(((0, 0), (3, 2)));
        paint(&mut model, 10, 12);
        assert_eq!(model.copied, "é界👩‍💻abcdef\nnext");
        model.selection = Some(((3, 2), (0, 0)));
        paint(&mut model, 10, 12);
        assert_eq!(model.copied, "é界👩‍💻abcdef\nnext");
        let copied = model.copied.clone();
        model.append("future hidden source").unwrap();
        paint(&mut model, 10, 12);
        assert_eq!(model.copied, copied);
        model.clear_selection();
        paint(&mut model, 11, 12);
        assert!(model.copied.is_empty());
    }

    #[test]
    fn browsing_eviction_follow_and_saturation() {
        let mut model = Model::new();
        paint(&mut model, 80, 24);
        model.event(Event::PageUp);
        paint(&mut model, 80, 24);
        let anchor = model.display.rows.first().map(|r| (r.line, r.start));
        model.append("stream").unwrap();
        model.insert("x");
        paint(&mut model, 80, 24);
        assert_eq!(
            model.display.rows.first().map(|r| (r.line, r.start)),
            anchor
        );
        for _ in 0..400 {
            model.append(&"x".repeat(1024)).unwrap();
        }
        assert_eq!(model.lines.len(), 256);
        assert!(model.evicted > 0);
        model.event(Event::Follow);
        for _ in 0..20 {
            let buffer = paint(&mut model, u16::MAX, u16::MAX);
            assert_eq!(buffer.area, Rect::new(0, 0, 200, 60));
            assert!(model.display.scanned <= 262144);
            assert!(model.display.text_bytes <= 65536);
            assert!(
                model
                    .display
                    .rows
                    .iter()
                    .map(|r| r.glyphs.iter().map(|g| g.width as usize).sum::<usize>())
                    .sum::<usize>()
                    <= 12000
            );
            model.tick();
        }
        let draft = model.editor.clone();
        paint(&mut model, 0, 0);
        assert_eq!(model.editor, draft);
    }

    #[test]
    fn oversized_clusters_copy_only_visible_replacements_and_osc52_bounds() {
        let mut model = Model::new();
        model.lines.clear();
        model.append(&format!("e{}", "\u{301}".repeat(40))).unwrap();
        paint(&mut model, 20, 10);
        model.selection = Some(((0, 0), (0, 0)));
        paint(&mut model, 20, 10);
        assert_eq!(model.copied, "�");
        assert_eq!(osc52("abc").unwrap(), b"\x1b]52;c;YWJj\x07");
        assert!(osc52(&"x".repeat(65537)).is_err());
        assert_eq!(
            osc52(&"x".repeat(65536)).unwrap().len(),
            8 + 65536_usize.div_ceil(3) * 4
        );
        model.clear_selection();
        model.event(Event::Copy);
        assert_eq!(model.status, "copy no-op: no displayed selection");
    }
}
