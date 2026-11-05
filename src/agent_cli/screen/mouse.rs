//! Scrolling and the mouse: the wheel and the scrollbar, selecting text, copying it and pasting.

use super::transcript::Place;
use super::{
    App, CLICKS, DRAG, LEFT, Open, RIGHT, Shown, WHEEL, WHEEL_DOWN, WHEEL_UP, to_terminal,
};
use crate::agent_cli::editor::{Draft, Editor, Note};
use crate::agent_cli::keys::{Key, Mouse};
use crate::agent_cli::selection;
use crate::agent_cli::{clipboard, keys};
use std::{
    io,
    sync::mpsc::{self, TryRecvError},
    time::Instant,
};

/// Where a point of a selection is: a line of the conversation, or a row of the dock under it.
#[derive(Clone, Copy, PartialEq)]
enum At {
    Line(Place),
    Dock(usize),
}

/// A point between two cells: before column `column` of the line or row at `at`.
#[derive(Clone, Copy, PartialEq)]
struct Mark {
    at: At,
    column: usize,
}

/// Text selected with the mouse, from where the left button was pressed to where it went.
pub(super) struct Selection {
    /// Which of the selections made this is, counted from 1.
    id: u64,
    anchor: Mark,
    head: Mark,
    /// The button is held, and moved since it was pressed.
    pub(super) held: bool,
    dragged: bool,
    /// The view followed the end when the selection started, and goes back to it when the
    /// selection ends, unless the user scrolled meanwhile.
    follow: bool,
    /// It started in the dock, which it then reaches wherever the view is.
    in_dock: bool,
}

/// What is selected, by line and column, from its start to before its end; the dock's rows are
/// counted on from `length`, the conversation's.
pub(super) struct Selected {
    start: (usize, usize),
    end: (usize, usize),
    length: usize,
    /// The conversation's lines from here on are not selected: as in Scramjet, a selection
    /// that reaches into the dock takes the conversation only as far as it is shown.
    seam: usize,
}

impl Selected {
    /// The columns selected of line `line`, from and before, if any.
    pub(super) fn columns(&self, line: usize) -> Option<(usize, usize)> {
        let taken = (line < self.seam || line >= self.length)
            && (self.start.0..=self.end.0).contains(&line);
        taken.then(|| {
            let left = (line == self.start.0).then_some(self.start.1);
            let right = (line == self.end.0).then_some(self.end.1);
            (left.unwrap_or(0), right.unwrap_or(usize::MAX))
        })
    }
}

/// Where the pointer is while a selection is dragged at the top (-1) or bottom (1) edge of the
/// conversation, which then scrolls on.
pub(super) struct Edge {
    direction: isize,
    column: usize,
    row: usize,
    pub(super) scrolled: Instant,
}

/// The scrollbar's thumb being dragged, as Scramjet's: the row within it that was pressed, and
/// the rows it can move and the last top line, both kept from when it was pressed.
#[derive(Clone, Copy)]
pub(super) struct Thumb {
    grab: usize,
    travel: usize,
    last: usize,
}

/// A copy under way, of the selection `selection`.
pub(super) struct Copying {
    /// The terminal's sequence that copies, if any, or the failure.
    outcome: mpsc::Receiver<Result<Option<String>, String>>,
    selection: u64,
}

/// The clipboard read for a right click, or an image's path for Ctrl+V, pasted only into the
/// editor it was for, unchanged.
pub(super) struct Pasting {
    /// A text dialog's editor, rather than the prompt's.
    dialog: bool,
    text: String,
    read: mpsc::Receiver<Result<String, String>>,
}

/// The last press of the left button, which the next one may repeat as a double or triple click.
pub(super) struct Click {
    count: u8,
    at: Instant,
    /// The screen's column and row.
    cell: (usize, usize),
    mark: Mark,
    /// The line pressed, as shown.
    line: String,
    /// Released where it was pressed.
    released: bool,
}

impl App {
    /// The first line of the conversation shown: where the user scrolled to, else the end's. A
    /// selection keeps the view where it is, also at the end.
    pub(super) fn shown_top(&self) -> usize {
        match self.top.map(|top| self.transcript.line(top)) {
            Some(top) if top < self.last || self.selection.is_some() => top.min(self.last),
            _ => self.last,
        }
    }

    /// Scrolls by `lines`, up when negative; returns whether the view moved.
    fn scroll(&mut self, lines: isize) -> bool {
        let top = self.shown_top();
        let next = top.saturating_add_signed(lines).min(self.last);
        if next != top {
            self.top = Some(self.transcript.locate(next));
        }
        next != top
    }

    /// Shows the conversation from line `line`, or its end.
    fn scroll_to(&mut self, line: usize) {
        let line = line.min(self.last);
        self.top = (line < self.last).then(|| self.transcript.locate(line));
    }

    /// Whether `column` is the last, the scrollbar's, which shows nothing else.
    fn on_scrollbar(&self, column: usize) -> bool {
        self.size.0 > 1 && column + 1 >= self.size.0
    }

    /// The scrollbar's thumb, its rows and the first of them, while the conversation is taller
    /// than its rows. As Scramjet's, sized to the share of the conversation shown, and while it
    /// is dragged, the size it had when taken.
    pub(super) fn thumb(&self) -> Option<(usize, usize)> {
        let (rows, length) = (self.rows, self.transcript.len());
        if length <= rows || self.size.0 <= 1 {
            return None;
        }
        let share = |line: usize, of: usize, travel: usize| {
            (line as f64 / of.max(1) as f64 * travel as f64).round() as usize
        };
        let top = self.shown_top();
        let (size, first) = match self.thumb {
            Some(Thumb { travel, last, .. }) => {
                let size = rows.saturating_sub(travel).max(1);
                (size, share(top, last, travel))
            }
            None => {
                let size = (rows * rows / length).clamp(1, rows);
                (size, share(top, self.last, rows - size))
            }
        };
        Some((size, first.min(rows - size)))
    }

    /// Scrolls as the thumb taken is dragged to the conversation's row `row`.
    fn drag_thumb(&mut self, row: usize) {
        let Some(Thumb { grab, travel, last }) = self.thumb else {
            return;
        };
        let moved = row.saturating_sub(grab).min(travel);
        let line = (moved as f64 / travel.max(1) as f64 * last as f64).round() as usize;
        self.scroll_to(line);
    }

    /// Scrolls as the user asks, which leaves the view there when a selection ends.
    pub(super) fn navigate(&mut self, lines: isize) {
        if let Some(selection) = &mut self.selection {
            selection.follow = false;
        }
        self.scroll(lines);
    }

    /// Whether the selection reaches into the dock, as in Scramjet: it started there, or the
    /// view is at the end.
    fn reaches_dock(&self) -> bool {
        (self.selection.as_ref()).is_some_and(|selection| selection.in_dock)
            || self.shown_top() == self.last
    }

    /// The point at the screen's `column` and `row`: in the dock if `row` is there and `dock`,
    /// else in the conversation, kept to the lines shown.
    fn mark(&self, column: usize, row: usize, dock: bool) -> Mark {
        if dock && row >= self.rows && !self.docked.is_empty() {
            let row = (row - self.rows).min(self.docked.len() - 1);
            let column = selection::snap(&self.docked[row].shown, column);
            return Mark {
                at: At::Dock(row),
                column,
            };
        }
        let line = self.shown_top() + row.min(self.rows.saturating_sub(1));
        let line = line.min(self.transcript.len().saturating_sub(1));
        let text = self.transcript.shown(line);
        Mark {
            at: At::Line(self.transcript.locate(line)),
            column: selection::snap(&text, column),
        }
    }

    /// The line or row at `at`, as drawn.
    fn shown(&self, at: At) -> String {
        match at {
            At::Line(place) => self.transcript.shown(self.transcript.line(place)),
            At::Dock(row) => {
                (self.docked.get(row)).map_or_else(String::new, |line| line.shown.clone())
            }
        }
    }

    pub(super) fn selected(&self) -> Option<Selected> {
        let selection = self.selection.as_ref()?;
        let length = self.transcript.len();
        let point = |mark: Mark| match mark.at {
            At::Line(place) => (self.transcript.line(place), mark.column),
            At::Dock(row) => {
                let row = row.min(self.docked.len().saturating_sub(1));
                (length + row, mark.column)
            }
        };
        let (anchor, head) = (point(selection.anchor), point(selection.head));
        let (start, end) = (anchor.min(head), anchor.max(head));
        let seam = match end.0 >= length {
            true => (self.shown_top() + self.rows).min(length),
            false => length,
        };
        (anchor != head).then_some(Selected {
            start,
            end,
            length,
            seam,
        })
    }

    pub(super) fn clear_selection(&mut self) {
        self.edge = None;
        if self
            .selection
            .take()
            .is_some_and(|selection| selection.follow)
        {
            self.top = None;
        }
    }

    /// As Scramjet's retained view: the wheel scrolls; the left button selects from where it is
    /// pressed in the conversation to where it is dragged, a drag to an edge scrolling on, and
    /// pressed twice or three times selects a word or the line; a right click copies what is
    /// selected, else pastes. In the last column, the scrollbar's, a press scrolls to that place
    /// and dragging the thumb scrolls with it.
    pub(super) fn mouse(&mut self, mouse: Mouse) {
        let Mouse {
            button,
            column,
            row,
            release,
        } = mouse;
        if release {
            // Dragged to the end, it follows the conversation again, however far it grew.
            if let Some(thumb) = self.thumb.take()
                && self.shown_top() >= thumb.last
            {
                self.top = None;
            }
            if let Some(click) = &mut self.click
                && button == LEFT
                && click.cell == (column, row)
            {
                click.released = true;
            }
            self.edge = None;
            if let Some(selection) = &mut self.selection {
                let completed = selection.held;
                selection.held = false;
                if selection.anchor == selection.head {
                    self.clear_selection();
                } else if completed && self.copy_on_select {
                    self.copy();
                }
            }
            return;
        }
        if button != LEFT && button != DRAG {
            self.click = None;
        }
        match button {
            WHEEL_UP | WHEEL_DOWN => {
                self.navigate(if button == WHEEL_UP { -WHEEL } else { WHEEL });
                let head = self.mark(column, row, self.reaches_dock());
                if let Some(selection) = &mut self.selection
                    && selection.held
                    && selection.dragged
                {
                    selection.head = head;
                }
            }
            RIGHT if self.selection.is_some() => self.copy(),
            RIGHT if self.copying.is_none() && !self.on_scrollbar(column) => {
                self.paste(clipboard::paste);
            }
            LEFT if row < self.rows && self.on_scrollbar(column) => {
                self.click = None;
                self.clear_selection();
                if let Some((size, first)) = self.thumb() {
                    let grab = match (first..first + size).contains(&row) {
                        true => row - first,
                        false => size / 2,
                    };
                    self.thumb = Some(Thumb {
                        grab,
                        travel: self.rows - size,
                        last: self.last,
                    });
                    self.drag_thumb(row);
                }
            }
            DRAG if self.thumb.is_some() => self.drag_thumb(row),
            LEFT => {
                let previous = self.click.take();
                self.clear_selection();
                let top = self.shown_top();
                let in_dock = row >= self.rows && row - self.rows < self.docked.len();
                if in_dock || (row < self.rows && top + row < self.transcript.len()) {
                    let follow = self.top.is_none();
                    self.top = Some(self.transcript.locate(top));
                    let mark = self.mark(column, row, in_dock);
                    let line = self.shown(mark.at);
                    let count = match previous {
                        Some(click)
                            if click.released
                                && click.at.elapsed() <= CLICKS
                                && click.cell == (column, row)
                                && click.mark == mark
                                && click.line == line =>
                        {
                            click.count % 3 + 1
                        }
                        _ => 1,
                    };
                    let at = |column| Mark { column, ..mark };
                    let (anchor, head) = match count {
                        2 => selection::word(&line, column)
                            .map_or((mark, mark), |(start, end)| (at(start), at(end))),
                        3 => (at(0), at(selection::width(&line))),
                        _ => (mark, mark),
                    };
                    self.selections += 1;
                    self.selection = Some(Selection {
                        id: self.selections,
                        anchor,
                        head,
                        held: true,
                        dragged: false,
                        follow,
                        in_dock,
                    });
                    self.click = Some(Click {
                        count,
                        at: Instant::now(),
                        cell: (column, row),
                        mark,
                        line,
                        released: false,
                    });
                }
            }
            DRAG => {
                // A press reported again where it was is not yet a drag.
                if self
                    .click
                    .as_ref()
                    .is_some_and(|click| click.cell == (column, row))
                {
                    return;
                }
                self.click = None;
                let head = self.mark(column, row, self.reaches_dock());
                let Some(selection) = self.selection.as_mut().filter(|selection| selection.held)
                else {
                    return;
                };
                selection.dragged = true;
                selection.head = head;
                let direction = match row {
                    0 => -1,
                    row if row + 1 >= self.rows && !selection.in_dock => 1,
                    _ => 0,
                };
                let scrolled = (self.edge.as_ref())
                    .filter(|edge| edge.direction == direction)
                    .map_or_else(Instant::now, |edge| edge.scrolled);
                self.edge = (direction != 0).then_some(Edge {
                    direction,
                    column,
                    row,
                    scrolled,
                });
            }
            _ => {}
        }
    }

    /// Scrolls on while a selection is dragged at an edge, until the view can go no further.
    pub(super) fn edge_scroll(&mut self) {
        let Some(Edge {
            direction,
            column,
            row,
            ..
        }) = self.edge
        else {
            return;
        };
        if let Some(selection) = &mut self.selection {
            selection.follow = false;
        }
        let scrolled = self.scroll(direction);
        // At the end, the pointer may be on the dock's rows.
        let head = self.mark(column, row, self.reaches_dock());
        if let Some(selection) = &mut self.selection {
            selection.head = head;
        }
        match (scrolled, &mut self.edge) {
            (true, Some(edge)) => edge.scrolled = Instant::now(),
            _ => self.edge = None,
        }
    }

    /// Copies the text selected, as shown, on a thread of its own: a clipboard tool may take a
    /// while.
    pub(super) fn copy(&mut self) {
        let Some(id) = (self.selection.as_ref()).map(|selection| selection.id) else {
            return;
        };
        let Some(selected) = self.selected().filter(|_| self.copying.is_none()) else {
            return;
        };
        let Selected {
            start,
            end,
            length,
            seam,
        } = selected;
        let shown = start.0.min(seam)..(end.0 + 1).min(seam);
        let mut lines = self.transcript.window(shown.start, shown.len());
        if end.0 >= length {
            let rows = start.0.max(length) - length..=end.0 - length;
            lines.extend(self.docked.get(rows).into_iter().flatten().cloned());
        }
        let left = match start.0 < seam || start.0 >= length {
            true => start.1,
            false => 0,
        };
        let text = selection::text(&lines, left, end.1);
        if text.is_empty() {
            return;
        }
        self.edge = None;
        let (done, outcome) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _ = done.send(clipboard::copy(&text, true));
            });
        match started {
            Ok(_) => {
                self.copying = Some(Copying {
                    outcome,
                    selection: id,
                })
            }
            Err(error) => self.note(Note::Error(format!("Copy failed: {error}"))),
        }
    }

    /// Ends a copy that has finished: the selection copied goes, and stays if it failed. With
    /// copy-on-select, a selection completed meanwhile is copied next.
    pub(super) fn copied(&mut self) -> io::Result<()> {
        let Some(copying) = &self.copying else {
            return Ok(());
        };
        let outcome = match copying.outcome.try_recv() {
            Err(TryRecvError::Empty) => return Ok(()),
            Ok(outcome) => outcome,
            Err(TryRecvError::Disconnected) => Err("the clipboard failed".to_owned()),
        };
        let copied = copying.selection;
        self.copying = None;
        match outcome {
            Ok(sequence) => {
                if let Some(sequence) = sequence {
                    to_terminal(&sequence)?;
                }
                if self.selection.as_ref().is_some_and(|s| s.id == copied) {
                    self.clear_selection();
                }
            }
            Err(error) => self.note(Note::Error(format!("Copy failed: {error}"))),
        }
        let waiting = (self.selection.as_ref())
            .is_some_and(|selection| !selection.held && selection.id != copied);
        if waiting && self.copy_on_select {
            self.copy();
        }
        Ok(())
    }

    /// The editor shown, if any: a text dialog's, or the prompt's when no dialog is open.
    pub(super) fn editing(&mut self) -> Option<(&mut Editor, &mut Draft)> {
        match &mut self.dialog {
            None => Some((&mut self.editor, &mut self.draft)),
            Some(Open {
                shown: Shown::Text(text),
                ..
            }) => Some((&mut text.editor, &mut text.draft)),
            Some(_) => None,
        }
    }

    /// Reads the `clipboard`, on a thread of its own, for the editor shown.
    pub(super) fn paste(&mut self, clipboard: fn() -> Result<String, String>) {
        if self.pasting.is_some() {
            return;
        }
        let dialog = self.dialog.is_some();
        let Some((_, draft)) = self.editing() else {
            return;
        };
        let text = draft.text().to_owned();
        let (done, read) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _ = done.send(clipboard());
            });
        match started {
            Ok(_) => self.pasting = Some(Pasting { dialog, text, read }),
            Err(error) => {
                if let Some((_, draft)) = self.editing() {
                    draft.set_note(Note::Error(format!("Paste failed: {error}")));
                }
            }
        }
    }

    /// Ends a paste whose clipboard was read: as Scramjet's, it goes into the editor only if
    /// that is still shown and unchanged, as if pasted from the terminal.
    pub(super) fn pasted(&mut self) {
        let read = match self.pasting.as_ref().map(|pasting| pasting.read.try_recv()) {
            None | Some(Err(TryRecvError::Empty)) => return,
            Some(Ok(read)) => read,
            Some(Err(TryRecvError::Disconnected)) => Err("the clipboard failed".to_owned()),
        };
        let pasting = self.pasting.take().expect("checked above");
        if self.exiting || self.dialog.is_some() != pasting.dialog {
            return;
        }
        let Some((editor, draft)) = self
            .editing()
            .filter(|(_, draft)| draft.text() == pasting.text)
        else {
            return;
        };
        match read {
            Ok(text) => {
                editor.key(draft, Key::Paste(keys::clean(&text)), false);
            }
            Err(error) => draft.set_note(Note::Error(format!("Paste failed: {error}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_into_the_dock_takes_the_conversation_as_far_as_shown() {
        // Lines 0 to 9 of the conversation, shown as far as 7, then the dock's rows from 10.
        let selected = Selected {
            start: (3, 4),
            end: (11, 2),
            length: 10,
            seam: 8,
        };
        let columns = [2, 3, 7, 8, 9, 10, 11, 12].map(|line| selected.columns(line));
        let all = Some((0, usize::MAX));
        assert_eq!(
            columns,
            [
                None,
                Some((4, usize::MAX)),
                all,
                None,
                None,
                all,
                Some((0, 2)),
                None
            ]
        );
    }
}
