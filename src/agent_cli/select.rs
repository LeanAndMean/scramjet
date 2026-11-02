//! A list to choose from with the arrow keys, drawn under the prompt, as Scramjet's selectors
//! for `/resume`, `/fork` and `/model` are.

use super::editor::{Key, RESIZE_CHECK, RawMode, Stdin, read_key, terminal_size};
use super::style;
use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

/// Rows shown at once, as in Scramjet's selectors.
const VISIBLE: usize = 10;
const HINT: &str = "↑↓ move · Enter select · Esc cancel";

/// Shows `title` over `count` rows, `row(index, width)` giving each one's text in at most `width`
/// columns, and returns the index chosen with Enter, starting at `initial`; `None` when cancelled
/// with Escape or Ctrl-C. The `current` row is marked. The rows are erased when it returns.
pub(super) fn select(
    title: &str,
    count: usize,
    initial: usize,
    current: Option<usize>,
    row: impl Fn(usize, usize) -> String,
) -> io::Result<Option<usize>> {
    if count == 0 {
        return Ok(None);
    }
    let raw = RawMode::enter()?;
    let _cursor = HiddenCursor::new();
    let mut list = List {
        count,
        selected: initial.min(count - 1),
    };
    let mut source = Stdin::default();
    let mut drawn = 0;
    let mut width = 0;
    let mut redraw = true;
    loop {
        if redraw && !source.pending() {
            let size = terminal_size();
            width = size.0;
            let lines = lines(title, &list, current, size, &row);
            let mut stderr = io::stderr().lock();
            write!(stderr, "{}{}", erase(drawn), lines.join("\r\n"))?;
            stderr.flush()?;
            drawn = lines.len();
            redraw = false;
        }
        let key = match read_key(&mut source, Some(RESIZE_CHECK)) {
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Key::Interrupt,
            result => result?,
        };
        match key {
            // Stopped from outside: the shell wrote below the list.
            Key::Idle if super::job::continued() => (drawn, redraw) = (0, true),
            Key::Idle => redraw |= terminal_size().0 != width,
            Key::Suspend => {
                eprint!("{}\x1b[?25h", erase(drawn));
                raw.suspend();
                super::job::suspend();
                super::job::continued();
                raw.resume()?;
                eprint!("\x1b[?25l");
                (drawn, redraw) = (0, true);
            }
            key => match list.key(&key, VISIBLE) {
                Some(chosen) => {
                    eprint!("{}", erase(drawn));
                    return Ok(chosen);
                }
                None => redraw = true,
            },
        }
    }
}

/// Moves to the first of the `drawn` rows and clears from there.
fn erase(drawn: usize) -> String {
    match drawn {
        0 | 1 => "\r\x1b[J".into(),
        rows => format!("\x1b[{}A\r\x1b[J", rows - 1),
    }
}

struct HiddenCursor;

impl HiddenCursor {
    fn new() -> Self {
        eprint!("\x1b[?25l");
        Self
    }
}

impl Drop for HiddenCursor {
    fn drop(&mut self) {
        eprint!("\x1b[?25h");
        let _ = io::stderr().flush();
    }
}

struct List {
    count: usize,
    selected: usize,
}

impl List {
    /// Moves the selection, or returns what was chosen: `Some(None)` when cancelled.
    fn key(&mut self, key: &Key, page: usize) -> Option<Option<usize>> {
        let last = self.count - 1;
        self.selected = match key {
            Key::Up if self.selected == 0 => last,
            Key::Up => self.selected - 1,
            Key::Down if self.selected == last => 0,
            Key::Down => self.selected + 1,
            Key::PageUp => self.selected.saturating_sub(page),
            Key::PageDown => (self.selected + page).min(last),
            Key::LineStart => 0,
            Key::LineEnd => last,
            Key::Enter => return Some(Some(self.selected)),
            Key::Escape | Key::Interrupt => return Some(None),
            _ => self.selected,
        };
        None
    }
}

/// The selector's lines for a terminal of `(width, height)`: the title, the rows around the
/// selected one and, when some are not shown, the selected row's position.
fn lines(
    title: &str,
    list: &List,
    current: Option<usize>,
    (width, height): (usize, usize),
    row: &dyn Fn(usize, usize) -> String,
) -> Vec<String> {
    use style::{CURRENT, MUTED, RESET, SELECTED, SELECTOR_MARK, SELECTOR_TITLE};
    // The last column stays empty, so that a full row does not move the cursor to the next.
    let width = width.saturating_sub(1).max(8);
    let visible = VISIBLE.min(list.count).min(height.saturating_sub(3).max(1));
    // As in Scramjet, the selected row stays in the middle where it can.
    let top = list
        .selected
        .saturating_sub(visible / 2)
        .min(list.count - visible);
    let hint_room = width.saturating_sub(title.width() + 2);
    let hint = super::clip(HINT, hint_room);
    let mut lines = vec![format!(
        "{SELECTOR_TITLE}{title}{RESET}  {MUTED}{hint}{RESET}"
    )];
    for index in top..top + visible {
        let is_current = current == Some(index);
        let text = row(index, width - 2 - if is_current { 2 } else { 0 });
        let mark = if is_current {
            format!(" {CURRENT}✓{RESET}")
        } else {
            String::new()
        };
        lines.push(if index == list.selected {
            format!("{SELECTOR_MARK}›{RESET} {SELECTED}{text}{RESET}{mark}")
        } else {
            format!("  {text}{mark}")
        });
    }
    if visible < list.count {
        lines.push(format!(
            "  {MUTED}({}/{}){RESET}",
            list.selected + 1,
            list.count
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| super::super::printable(&line))
            .collect()
    }

    #[test]
    fn keys_move_the_selection_around_and_choose_or_cancel() {
        let mut list = List {
            count: 25,
            selected: 0,
        };
        let mut moved = Vec::new();
        for key in [
            Key::Up,
            Key::Up,
            Key::Down,
            Key::Down,
            Key::PageUp,
            Key::PageUp,
            Key::PageDown,
            Key::LineStart,
            Key::LineEnd,
            Key::Text("j".into()),
        ] {
            assert_eq!(list.key(&key, 10), None);
            moved.push(list.selected);
        }
        assert_eq!(moved, [24, 23, 24, 0, 0, 0, 10, 0, 24, 24]);
        assert_eq!(list.key(&Key::Enter, 10), Some(Some(24)));
        assert_eq!(list.key(&Key::Escape, 10), Some(None));
        assert_eq!(list.key(&Key::Interrupt, 10), Some(None));
    }

    #[test]
    fn shows_ten_rows_around_the_selection_with_its_position() {
        let row = |index: usize, width: usize| super::super::clip(&format!("row {index}"), width);
        let list = |selected| List {
            count: 25,
            selected,
        };
        let shown = plain(lines("Pick", &list(12), Some(9), (50, 24), &row));
        assert_eq!(
            shown,
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "  row 7",
                "  row 8",
                "  row 9 ✓",
                "  row 10",
                "  row 11",
                "› row 12",
                "  row 13",
                "  row 14",
                "  row 15",
                "  row 16",
                "  (13/25)",
            ]
        );
        let first = plain(lines("Pick", &list(0), None, (50, 24), &row));
        assert_eq!(first[1], "› row 0");
        assert_eq!(first[10], "  row 9");
        let last = plain(lines("Pick", &list(24), None, (50, 24), &row));
        assert_eq!(last[1], "  row 15");
        assert_eq!(last[10], "› row 24");
        // A short terminal shows fewer rows; a narrow one cuts the hint and the rows.
        let short = plain(lines("Pick", &list(3), None, (12, 6), &row));
        assert_eq!(
            short,
            ["Pick  ↑↓ m…", "  row 2", "› row 3", "  row 4", "  (4/25)"]
        );
        let few = List {
            count: 2,
            selected: 1,
        };
        assert_eq!(
            plain(lines("Pick", &few, None, (50, 24), &row)),
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "  row 0",
                "› row 1"
            ]
        );
    }
}
