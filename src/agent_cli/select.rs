//! A list to choose from with the arrow keys, drawn under the prompt, as Scramjet's selectors
//! for `/resume`, `/fork` and `/model` are; those for `/resume` and `/model` search as you type.

use super::editor::{Key, RESIZE_CHECK, RawMode, Stdin, read_key, terminal_size};
use super::fuzzy::{self, Haystack};
use super::style;
use super::text;
use scramjet_rs::agent::Choice;
use std::io::{self, Write};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Rows shown at once, as in Scramjet's selectors other than its tree.
pub(super) const VISIBLE: usize = 10;
const HINT: &str = "↑↓ move · Enter select · Esc cancel";
/// Options shown at once in a question's list, as in Scramjet.
const QUESTION_VISIBLE: usize = 8;
/// Rows an option's label or description takes at most, as in Scramjet.
const OPTION_ROWS: usize = 4;
const DESCRIPTION_INDENT: &str = "     ";

/// Shows `title` over `count` rows, at most `visible` at once, `row(index, width)` giving each
/// one's text in at most `width` columns, and returns the index chosen with Enter, starting at
/// `initial`; `None` when cancelled with Escape or Ctrl-C. The `current` row is marked. The rows
/// are erased when it returns.
pub(super) fn select(
    title: &str,
    visible: usize,
    count: usize,
    initial: usize,
    current: Option<usize>,
    row: impl Fn(usize, usize) -> String,
) -> io::Result<Option<usize>> {
    let list = List::new(count, initial, visible);
    choose(list, |list, size| lines(title, list, current, size, &row))
}

/// As `select`, with a line to type a query on, as Scramjet's session and model selectors have:
/// the query shows only the rows whose `texts` match it, best first (see `fuzzy`), and `none`
/// when no row does.
pub(super) fn search(
    title: &str,
    visible: usize,
    texts: &[String],
    initial: usize,
    current: Option<usize>,
    none: &str,
    row: impl Fn(usize, usize) -> String,
) -> io::Result<Option<usize>> {
    let list = List::searching(texts, initial, visible, none);
    choose(list, |list, size| lines(title, list, current, size, &row))
}

/// As Scramjet's list for a question the model asks: each option's label with its description
/// under it, at most `QUESTION_VISIBLE` options at once, framed with `title` and the keys to use.
pub(super) fn question(
    title: &str,
    options: &[Choice],
    recommended: Option<usize>,
) -> io::Result<Option<usize>> {
    let list = List::new(
        options.len(),
        recommended.unwrap_or(0),
        question_visible(options),
    );
    choose(list, |list, size| {
        question_lines(title, options, recommended, list, size)
    })
}

fn question_visible(options: &[Choice]) -> usize {
    options.len().min(QUESTION_VISIBLE)
}

/// A list as `select`, `search` and `question` show it, for a screen that draws it and passes
/// it the keys.
pub(super) struct Picker {
    list: List,
    shape: Shape,
}

enum Shape {
    /// A selector's rows, given at the width they take on the terminal when the list opens.
    Rows {
        title: String,
        current: Option<usize>,
        rows: Vec<String>,
    },
    Question {
        title: String,
        options: Vec<Choice>,
        recommended: Option<usize>,
    },
}

impl Picker {
    /// As `select`.
    pub(super) fn select(
        title: &str,
        visible: usize,
        count: usize,
        initial: usize,
        current: Option<usize>,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        let list = List::new(count, initial, visible);
        Self::rows(title, list, current, row)
    }

    /// As `search`.
    pub(super) fn search(
        title: &str,
        visible: usize,
        texts: &[String],
        initial: usize,
        current: Option<usize>,
        none: &str,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        let list = List::searching(texts, initial, visible, none);
        Self::rows(title, list, current, row)
    }

    /// As `question`.
    pub(super) fn question(title: &str, options: &[Choice], recommended: Option<usize>) -> Self {
        Self {
            list: List::new(
                options.len(),
                recommended.unwrap_or(0),
                question_visible(options),
            ),
            shape: Shape::Question {
                title: title.to_owned(),
                options: options.to_vec(),
                recommended,
            },
        }
    }

    fn rows(
        title: &str,
        list: List,
        current: Option<usize>,
        row: impl Fn(usize, usize) -> String,
    ) -> Self {
        // As wide as `lines` asks for them, the mark after the current row included.
        let width = terminal_size().0.saturating_sub(1).max(8) - 2;
        let rows = (0..list.rows.len())
            .map(|index| row(index, width - if current == Some(index) { 2 } else { 0 }))
            .collect();
        Self {
            list,
            shape: Shape::Rows {
                title: title.to_owned(),
                current,
                rows,
            },
        }
    }

    /// Whether there is nothing to choose from, as `select` returns `None` at once then.
    pub(super) fn is_empty(&self) -> bool {
        self.list.rows.is_empty()
    }

    /// Moves the selection or edits the query, or returns what was chosen: `Some(None)` when
    /// cancelled.
    pub(super) fn key(&mut self, key: &Key) -> Option<Option<usize>> {
        self.list.key(key)
    }

    /// The list's lines for a screen of `(width, height)`.
    pub(super) fn lines(&self, size: (usize, usize)) -> Vec<String> {
        match &self.shape {
            Shape::Rows {
                title,
                current,
                rows,
            } => lines(title, &self.list, *current, size, &|index, width| {
                text::clip(&rows[index], width)
            }),
            Shape::Question {
                title,
                options,
                recommended,
            } => question_lines(title, options, *recommended, &self.list, size),
        }
    }
}

/// `lines(list, (width, height))` draws the list for the terminal's size.
fn choose(
    mut list: List,
    lines: impl Fn(&List, (usize, usize)) -> Vec<String>,
) -> io::Result<Option<usize>> {
    if list.rows.is_empty() {
        return Ok(None);
    }
    let raw = RawMode::enter()?;
    let _cursor = HiddenCursor::new();
    let mut source = Stdin::default();
    let mut drawn = 0;
    let mut width = 0;
    let mut redraw = true;
    loop {
        if redraw && !source.pending() {
            let size = terminal_size();
            width = size.0;
            let lines = lines(&list, size);
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
            key => match list.key(&key) {
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
    /// The rows shown, in order: all of them, or those that match the query.
    rows: Vec<usize>,
    /// The selected row's position in `rows`.
    selected: usize,
    /// Rows shown at once, and moved by a page.
    visible: usize,
    search: Option<Search>,
}

struct Search {
    query: String,
    texts: Vec<Haystack>,
    /// Shown when no row matches.
    none: String,
    /// The selection without a query.
    initial: usize,
}

impl List {
    fn new(count: usize, initial: usize, visible: usize) -> Self {
        Self {
            rows: (0..count).collect(),
            selected: initial.min(count.saturating_sub(1)),
            visible,
            search: None,
        }
    }

    fn searching(texts: &[String], initial: usize, visible: usize, none: &str) -> Self {
        let mut list = Self::new(texts.len(), initial, visible);
        list.search = Some(Search {
            query: String::new(),
            texts: texts.iter().map(|text| Haystack::new(text)).collect(),
            none: none.into(),
            initial: list.selected,
        });
        list
    }

    /// Moves the selection or edits the query, or returns what was chosen: `Some(None)` when
    /// cancelled.
    fn key(&mut self, key: &Key) -> Option<Option<usize>> {
        if let Some(search) = &mut self.search {
            let edited = match key {
                Key::Text(text) | Key::Paste(text) => {
                    let text = text
                        .chars()
                        .map(|ch| if ch.is_whitespace() { ' ' } else { ch });
                    search.query.extend(text.filter(|ch| !ch.is_control()));
                    true
                }
                Key::Backspace => search.query.pop().is_some(),
                _ => false,
            };
            if edited {
                self.filter();
                return None;
            }
        }
        let Some(last) = self.rows.len().checked_sub(1) else {
            return matches!(key, Key::Escape | Key::Interrupt).then_some(None);
        };
        let page = self.visible;
        self.selected = match key {
            Key::Up if self.selected == 0 => last,
            Key::Up => self.selected - 1,
            Key::Down if self.selected == last => 0,
            Key::Down => self.selected + 1,
            Key::PageUp => self.selected.saturating_sub(page),
            Key::PageDown => (self.selected + page).min(last),
            Key::LineStart => 0,
            Key::LineEnd => last,
            Key::Enter => return Some(Some(self.rows[self.selected])),
            Key::Escape | Key::Interrupt => return Some(None),
            _ => self.selected,
        };
        None
    }

    /// Shows the rows that match the query, the best one selected, or all rows without one.
    fn filter(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        let tokens = fuzzy::parse(&search.query);
        if tokens.is_empty() {
            self.rows = (0..search.texts.len()).collect();
            self.selected = search.initial;
        } else {
            self.rows = fuzzy::filter(&tokens, &search.texts);
            self.selected = 0;
        }
    }
}

/// The selector's lines for a terminal of `(width, height)`: the title, the query being typed
/// (with no row matching it, `none`), the rows around the selected one and, when some are not
/// shown, the selected row's position.
fn lines(
    title: &str,
    list: &List,
    current: Option<usize>,
    (width, height): (usize, usize),
    row: &dyn Fn(usize, usize) -> String,
) -> Vec<String> {
    use style::{CURRENT, MUTED, RESET, SEARCH_CURSOR, SELECTED, SELECTOR_MARK, SELECTOR_TITLE};
    // The last column stays empty, so that a full row does not move the cursor to the next.
    let width = width.saturating_sub(1).max(8);
    let shown = list.rows.len();
    let search_rows = usize::from(list.search.is_some());
    let visible = list
        .visible
        .min(shown)
        .min(height.saturating_sub(3 + search_rows).max(1));
    // As in Scramjet, the selected row stays in the middle where it can.
    let top = list
        .selected
        .saturating_sub(visible / 2)
        .min(shown - visible);
    let hint_room = width.saturating_sub(title.width() + 2);
    let hint = text::clip(HINT, hint_room);
    let mut lines = vec![format!(
        "{SELECTOR_TITLE}{title}{RESET}  {MUTED}{hint}{RESET}"
    )];
    if let Some(search) = &list.search {
        let query = tail(&search.query, width - 3);
        lines.push(format!("> {query}{SEARCH_CURSOR} {RESET}"));
        if shown == 0 {
            let none = text::clip(&search.none, width - 2);
            lines.push(format!("  {MUTED}{none}{RESET}"));
        }
    }
    for position in top..top + visible {
        let index = list.rows[position];
        let is_current = current == Some(index);
        let text = row(index, width - 2 - if is_current { 2 } else { 0 });
        let mark = if is_current {
            format!(" {CURRENT}✓{RESET}")
        } else {
            String::new()
        };
        lines.push(if position == list.selected {
            format!("{SELECTOR_MARK}›{RESET} {SELECTED}{text}{RESET}{mark}")
        } else {
            format!("  {text}{mark}")
        });
    }
    if visible < shown {
        lines.push(format!("  {MUTED}({}/{shown}){RESET}", list.selected + 1));
    }
    lines
}

/// A question's lines for a terminal of `(width, height)`: a rule, the title, the options around
/// the selected one, the selected one's position when some are not shown, the keys, a rule.
fn question_lines(
    title: &str,
    options: &[Choice],
    recommended: Option<usize>,
    list: &List,
    (width, height): (usize, usize),
) -> Vec<String> {
    use style::{MUTED, QUESTION_BORDER, QUESTION_TITLE, RESET};
    let width = width.saturating_sub(1).max(8);
    let count = options.len();
    let selected = list.rows[list.selected];
    let rows: Vec<Vec<String>> = (0..count)
        .map(|index| {
            let option = &options[index];
            option_rows(option, index == selected, recommended == Some(index), width)
        })
        .collect();
    // The rules, the title and the keys; the rest is for the options and their position.
    let room = height.saturating_sub(4).max(1);
    let shown = list.visible.clamp(1, count);
    let mut top = selected.saturating_sub(shown / 2).min(count - shown);
    let mut end = top + shown;
    let needed = |top: usize, end: usize| {
        let position = usize::from(top > 0 || end < count);
        rows[top..end].iter().map(Vec::len).sum::<usize>() + position
    };
    // Options farthest from the selected one go first when they do not all fit.
    while end - top > 1 && needed(top, end) > room {
        if selected - top > end - 1 - selected {
            top += 1;
        } else {
            end -= 1;
        }
    }
    let rule = format!("{QUESTION_BORDER}{}{RESET}", "─".repeat(width));
    let mut lines = vec![rule.clone(), format!("{QUESTION_TITLE}{title}{RESET}")];
    let hidden = top > 0 || end < count;
    let option_room = room.saturating_sub(usize::from(hidden)).max(1);
    let options = rows[top..end].iter().flatten().take(option_room).cloned();
    lines.extend(options);
    if hidden {
        lines.push(format!("  {MUTED}({}/{count}){RESET}", selected + 1));
    }
    lines.push(format!("{MUTED}{}{RESET}", text::clip(HINT, width)));
    lines.push(rule);
    lines
}

/// An option as Scramjet's question list shows it: `→` before the selected one, its label, then
/// its description indented under it, each wrapped to at most `OPTION_ROWS` rows.
fn option_rows(option: &Choice, selected: bool, recommended: bool, width: usize) -> Vec<String> {
    use style::{MUTED, QUESTION_SELECTED, RESET};
    let style = |text: String, other: &str| match (selected, other) {
        (true, _) => format!("{QUESTION_SELECTED}{text}{RESET}"),
        (false, "") => text,
        (false, other) => format!("{other}{text}{RESET}"),
    };
    let mark = if selected { "→ " } else { "  " };
    let mut rows: Vec<String> = wrapped(&option.label, width - 2)
        .into_iter()
        .enumerate()
        .map(|(row, text)| style(format!("{}{text}", if row == 0 { mark } else { "  " }), ""))
        .collect();
    if let Some(description) = &option.description {
        // As in Scramjet, only a recommended option's description says it is recommended.
        let text = match recommended {
            true => format!("[recommended] {description}"),
            false => description.clone(),
        };
        let width = width - DESCRIPTION_INDENT.len();
        for text in wrapped(&text, width) {
            rows.push(style(format!("{DESCRIPTION_INDENT}{text}"), MUTED));
        }
    }
    rows
}

/// `text` wrapped between words to `width` columns, in at most `OPTION_ROWS` rows, the last
/// one ending with `…` when there is more.
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut wrap = text::Wrap::new(width.max(1));
        let line = wrap.push(&text::printable(line)) + &wrap.finish();
        rows.extend(line.split('\n').map(str::to_owned));
    }
    if rows.len() > OPTION_ROWS {
        rows.truncate(OPTION_ROWS);
        let last = &mut rows[OPTION_ROWS - 1];
        *last = text::clip(&format!("{last}…"), width);
    }
    rows
}

/// The end of `text` that fits in `width` columns, so that the end of a long query shows.
fn tail(text: &str, width: usize) -> &str {
    let mut used = 0;
    for (at, ch) in text.char_indices().rev() {
        used += ch.width().unwrap_or(0);
        if used > width {
            return &text[at + ch.len_utf8()..];
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| text::printable(&line))
            .collect()
    }

    #[test]
    fn keys_move_the_selection_around_and_choose_or_cancel() {
        let mut list = List::new(25, 0, 10);
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
            assert_eq!(list.key(&key), None);
            moved.push(list.selected);
        }
        assert_eq!(moved, [24, 23, 24, 0, 0, 0, 10, 0, 24, 24]);
        assert_eq!(list.key(&Key::Enter), Some(Some(24)));
        assert_eq!(list.key(&Key::Escape), Some(None));
        assert_eq!(list.key(&Key::Interrupt), Some(None));
    }

    #[test]
    fn shows_ten_rows_around_the_selection_with_its_position() {
        let row = |index: usize, width: usize| text::clip(&format!("row {index}"), width);
        let list = |selected| List::new(25, selected, 10);
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
        let few = List::new(2, 1, 10);
        assert_eq!(
            plain(lines("Pick", &few, None, (50, 24), &row)),
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "  row 0",
                "› row 1"
            ]
        );
    }

    #[test]
    fn question_options_show_their_descriptions_under_them() {
        let option = |label: &str, description: Option<&str>| Choice {
            label: label.into(),
            description: description.map(str::to_owned),
            value: String::new(),
        };
        let options = [
            option("Ship it", Some("Commit and push the change now")),
            option("Wait", None),
            option("Ask again later", Some(&"long words ".repeat(20))),
        ];
        let list = List::new(3, 2, 8);
        assert_eq!(
            plain(question_lines("Choose", &options, Some(2), &list, (31, 24))),
            [
                "─".repeat(30).as_str(),
                "Choose",
                "  Ship it",
                "     Commit and push the",
                "     change now",
                "  Wait",
                "→ Ask again later",
                "     [recommended] long words",
                "     long words long words",
                "     long words long words",
                "     long words long words…",
                "↑↓ move · Enter select · Esc …",
                "─".repeat(30).as_str(),
            ]
        );
        // A short terminal keeps the selected option and the ones nearest it, with its position.
        let first = List::new(3, 0, 8);
        let short = plain(question_lines("Choose", &options, None, &first, (31, 8)));
        assert_eq!(
            short[1..6],
            [
                "Choose",
                "→ Ship it",
                "     Commit and push the",
                "     change now",
                "  (1/3)"
            ]
        );
    }

    #[test]
    fn typing_shows_the_matching_rows_best_first() {
        let texts = [
            "fix the parser",
            "write a haiku",
            "parser tests",
            "release notes",
        ]
        .map(String::from);
        let row = |index: usize, width: usize| text::clip(&texts[index], width);
        let mut list = List::searching(&texts, 1, 10, "No matching rows");
        let shown = |list: &List| plain(lines("Pick", list, Some(0), (50, 24), &row));
        assert_eq!(shown(&list)[1..3], [">  ", "  fix the parser ✓"]);
        assert_eq!(shown(&list)[3], "› write a haiku");
        for key in [Key::Text("pa".into()), Key::Text("rs".into())] {
            assert_eq!(list.key(&key), None);
        }
        assert_eq!(
            shown(&list),
            [
                "Pick  ↑↓ move · Enter select · Esc cancel",
                "> pars ",
                "› parser tests",
                "  fix the parser ✓"
            ]
        );
        assert_eq!(list.key(&Key::Down), None);
        assert_eq!(list.key(&Key::Enter), Some(Some(0)));
        // Nothing matches: Enter and the arrows do nothing, Escape still cancels.
        assert_eq!(list.key(&Key::Paste("\nzz".into())), None);
        assert_eq!(shown(&list)[1..], ["> pars zz ", "  No matching rows"]);
        for key in [Key::Enter, Key::Up, Key::LineEnd] {
            assert_eq!(list.key(&key), None);
        }
        for _ in 0..3 {
            assert_eq!(list.key(&Key::Backspace), None);
        }
        assert_eq!(list.rows, [2, 0]);
        // Without a query every row is back, with the first selection.
        for _ in 0..4 {
            assert_eq!(list.key(&Key::Backspace), None);
        }
        assert_eq!((&list.rows[..], list.selected), (&[0, 1, 2, 3][..], 1));
        assert_eq!(list.key(&Key::Backspace), None);
        assert_eq!(list.key(&Key::Escape), Some(None));
        // A query wider than the line shows its end.
        list.key(&Key::Text("a very long query".into()));
        let narrow = plain(lines("Pick", &list, None, (12, 24), &row));
        assert_eq!(narrow[1], "> ng query ");
    }
}
