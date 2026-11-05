//! Text selected with the mouse in the full screen's conversation and the dock under it, as
//! Scramjet's retained view selects it: between the starts of the cells pressed and dragged to,
//! shown reversed, copied as displayed but for gutters, rules and the line breaks of wrapping.
//! Lines are the screen's (`lines`), which hold only style sequences besides the text.

use super::style::{self, Band};
use super::text::{FROM, JOINED, RULE, SPACE, UNTIL};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A line of the screen, and what a copy of it holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Line {
    /// Its text and style sequences.
    pub(super) shown: String,
    /// The column its copy starts at; the cells before are a gutter.
    from: usize,
    /// The column its copy ends at, if not its end.
    until: Option<usize>,
    /// When the next line continues it, what goes between them in a copy.
    joined: Option<String>,
    /// A rule between blocks, which a copy leaves out.
    rule: bool,
    /// The background it is drawn on across the screen, if not the base.
    pub(super) band: Option<Band>,
}

impl Line {
    /// `shown`, a rule that a copy leaves out.
    pub(super) fn rule(shown: &str) -> Self {
        Self {
            rule: true,
            ..row(shown)
        }
    }
}

/// The lines of rendered text, each ending its styles and starting with those still on from the
/// lines before, so that any of them can be drawn first. Only style sequences are kept of the
/// escape sequences, and no other control characters; the marks of `text::FROM` say what a
/// copy holds.
pub(super) fn lines(text: &str) -> Vec<Line> {
    let text = text.trim_end_matches('\n');
    match text.is_empty() {
        true => Vec::new(),
        false => split(text.split('\n')),
    }
}

/// As `lines`, a line for each of `rows`.
pub(super) fn rows(rows: &[impl AsRef<str>]) -> Vec<Line> {
    split(rows.iter().map(AsRef::as_ref))
}

/// As `lines`, the line of a row.
pub(super) fn row(shown: &str) -> Line {
    split(std::iter::once(shown)).pop().unwrap_or_default()
}

fn split<'a>(rows: impl Iterator<Item = &'a str>) -> Vec<Line> {
    let mut active = String::new();
    let mut lines: Vec<Line> = Vec::new();
    for line in rows {
        let mut shown = active.clone();
        let (mut column, mut from, mut joined, mut rule) = (0, 0, None::<String>, false);
        let (mut band, mut until) = (None, None);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' if chars.peek() == Some(&'[') => {
                    chars.next();
                    let mut sequence = String::from("\x1b[");
                    for c in chars.by_ref() {
                        sequence.push(c);
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                    if sequence.ends_with('m') {
                        if sequence == style::RESET || sequence == "\x1b[m" {
                            active.clear();
                        } else {
                            active += &sequence;
                        }
                        shown += &sequence;
                    }
                }
                '\t' => {
                    shown += "    ";
                    column += 4;
                }
                FROM => from = column,
                RULE => rule = true,
                UNTIL => until = Some(column),
                JOINED => {
                    from = column;
                    joined = Some(String::new());
                }
                SPACE => joined.iter_mut().for_each(|joined| joined.push(' ')),
                c if let Some(marked) = Band::marked(c) => band = Some(marked),
                c if c.is_control() => {}
                c => {
                    shown.push(c);
                    column += c.width().unwrap_or(0);
                }
            }
        }
        if !active.is_empty() {
            shown += style::RESET;
        }
        if let Some(previous) = lines.last_mut().filter(|_| joined.is_some()) {
            previous.joined = joined;
        }
        lines.push(Line {
            shown,
            from,
            until,
            joined: None,
            rule,
            band,
        });
    }
    lines
}

enum Piece<'a> {
    /// A style sequence, which takes no columns.
    Style(&'a str),
    /// A grapheme and the columns it takes.
    Cell(&'a str, usize),
}

fn pieces(line: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let length = match rest.strip_prefix("\x1b[") {
            Some(sequence) => {
                let end = sequence.find(|c: char| ('@'..='~').contains(&c));
                2 + end.map_or(sequence.len(), |end| end + 1)
            }
            None if rest.starts_with('\x1b') => 1,
            None => {
                let text = &rest[..rest.find('\x1b').unwrap_or(rest.len())];
                pieces.extend(
                    text.graphemes(true)
                        .map(|cell| Piece::Cell(cell, cell.width())),
                );
                rest = &rest[text.len()..];
                continue;
            }
        };
        pieces.push(Piece::Style(&rest[..length]));
        rest = &rest[length..];
    }
    pieces
}

/// Where a press or drag at `column` falls in `line`: the start of the cell under it, or the
/// line's end past it.
pub(super) fn snap(line: &str, column: usize) -> usize {
    let mut at = 0;
    for piece in pieces(line) {
        if let Piece::Cell(_, width) = piece {
            if at + width > column {
                break;
            }
            at += width;
        }
    }
    at
}

/// The columns `line`'s cells take.
pub(super) fn width(line: &str) -> usize {
    (pieces(line).into_iter())
        .map(|piece| match piece {
            Piece::Cell(_, width) => width,
            Piece::Style(_) => 0,
        })
        .sum()
}

/// What a double click at `column` selects in `line`, as in Scramjet: the run of cells of the
/// same kind (whitespace, punctuation or other) around the cell there, from its first column
/// to after its last; `None` past the line's end.
pub(super) fn word(line: &str, column: usize) -> Option<(usize, usize)> {
    let mut cells = Vec::new();
    let mut at = 0;
    for piece in pieces(line) {
        if let Piece::Cell(cell, width) = piece {
            cells.push((at, at + width, kind(cell)));
            at += width;
        }
    }
    let index = cells
        .iter()
        .position(|&(start, end, _)| start <= column && column < end)?;
    let same = cells[index].2;
    let first = (cells[..index].iter())
        .rposition(|cell| cell.2 != same)
        .map_or(0, |before| before + 1);
    let last = (cells[index..].iter())
        .position(|cell| cell.2 != same)
        .map_or(cells.len(), |after| index + after)
        - 1;
    Some((cells[first].0, cells[last].1))
}

/// Scramjet's kinds of cell for a double click: 0 whitespace, 1 punctuation, 2 other.
fn kind(cell: &str) -> u8 {
    const PUNCTUATION: &str = "(){}[]<>.,;:'\"!?+-=*/\\|&%^$#@~`";
    if cell.chars().any(char::is_whitespace) {
        0
    } else if cell.chars().any(|c| PUNCTUATION.contains(c)) {
        1
    } else {
        2
    }
}

/// The text of `line`'s cells that start from `left` and before `right`.
fn cut(line: &str, left: usize, right: usize) -> String {
    let mut at = 0;
    let mut text = String::new();
    for piece in pieces(line) {
        if let Piece::Cell(cell, width) = piece {
            if (left..right).contains(&at) {
                text.push_str(cell);
            }
            at += width;
        }
    }
    text
}

/// `line` with its cells from `left` and before `right` shown selected: reversed, without their
/// own styles, which come back after them.
pub(super) fn highlight(line: &str, left: usize, right: usize) -> String {
    let mut shown = String::new();
    // The styles in effect where the selection ends.
    let mut active = String::new();
    let (mut at, mut opened, mut closed) = (0, false, false);
    for piece in pieces(line) {
        match piece {
            Piece::Style(sequence) => {
                if sequence == style::RESET || sequence == "\x1b[m" {
                    active.clear();
                } else {
                    active.push_str(sequence);
                }
                if !opened || closed {
                    shown.push_str(sequence);
                }
            }
            Piece::Cell(cell, width) => {
                if !opened && (left..right).contains(&at) {
                    shown += style::RESET;
                    shown += style::SELECTION.get();
                    opened = true;
                }
                if opened && !closed && at >= right {
                    shown += style::RESET;
                    shown += &active;
                    closed = true;
                }
                shown.push_str(cell);
                at += width;
            }
        }
    }
    if opened && !closed {
        shown += style::RESET;
    }
    shown
}

/// The text selected in `lines`, from column `left` of the first to before column `right` of
/// the last, without gutters and rules: a line each, without their trailing spaces, but for a
/// line that the next continues, which they join as it says.
pub(super) fn text(lines: &[Line], left: usize, right: usize) -> String {
    let last = lines.len().saturating_sub(1);
    let mut text = String::new();
    // What goes between the line copied last and the next.
    let mut between = "";
    for (index, line) in lines.iter().enumerate().filter(|(_, line)| !line.rule) {
        let from = if index == 0 { left } else { 0 }.max(line.from);
        let to = if index == last { right } else { usize::MAX };
        let to = to.min(line.until.unwrap_or(usize::MAX));
        let cut = cut(&line.shown, from, to);
        text += between;
        match &line.joined {
            Some(joined) if index < last => {
                text += &cut;
                between = joined;
            }
            _ => {
                text += cut.trim_end();
                between = "\n";
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_fall_on_the_start_of_a_cell() {
        let line = "\x1b[1m界a\x1b[0mb";
        assert_eq!(
            [0, 1, 2, 3, 4, 9].map(|column| snap(line, column)),
            [0, 0, 2, 3, 4, 4]
        );
    }

    #[test]
    fn the_selection_shows_reversed_and_the_styles_come_back_after_it() {
        let line = "plain \x1b[1mbold \x1b[33mcolor\x1b[0m end";
        assert_eq!(
            highlight(line, 3, 13),
            "pla\x1b[0m\x1b[7min bold co\x1b[0m\x1b[1m\x1b[33mlor\x1b[0m end"
        );
        assert_eq!(
            highlight(line, 17, usize::MAX),
            "plain \x1b[1mbold \x1b[33mcolor\x1b[0m \x1b[0m\x1b[7mend\x1b[0m"
        );
        assert_eq!(highlight(line, 0, 0), line);
        assert_eq!(highlight("", 0, usize::MAX), "");
    }

    #[test]
    fn a_double_click_takes_the_run_of_cells_of_its_kind() {
        // Columns: `snake_case` 0-9, `:` 10, ` ` 11, `src` 12-14, `/` 15, `a` 16, `.` 17,
        // `rs` 18-19, two spaces 20-21, `界界` 22-25, ` ` 26, `end` 27-29.
        let line = "\x1b[1msnake_case\x1b[0m: src/a.rs  界界 end";
        let at = |column| word(line, column);
        assert_eq!(at(0), Some((0, 10)));
        assert_eq!(at(9), Some((0, 10)));
        assert_eq!(at(10), Some((10, 11)));
        assert_eq!(at(14), Some((12, 15)));
        assert_eq!(at(15), Some((15, 16)));
        assert_eq!(at(19), Some((18, 20)));
        assert_eq!(at(21), Some((20, 22)));
        assert_eq!(at(23), Some((22, 26)));
        assert_eq!(at(28), Some((27, 30)));
        assert_eq!(at(30), None);
        assert_eq!(width(line), 30);
        assert_eq!(word("", 0), None);
    }

    #[test]
    fn the_text_copied_is_the_cells_selected_a_line_each() {
        let lines = lines("\x1b[2m> first line  \x1b[0m\n\n  indented 界 wide");
        assert_eq!(text(&lines, 2, 13), "first line\n\n  indented 界");
        assert_eq!(text(&lines[2..], 11, 12), "界");
        assert_eq!(text(&lines[..1], 2, 7), "first");
    }

    #[test]
    fn a_copy_leaves_out_gutters_and_joins_wrapped_lines() {
        // A wrapped paragraph, a word broken at the edge and a hanging indent.
        let reply = lines(&format!(
            "run \x1b[1mcargo\x1b[0m\n{JOINED}{SPACE}test --all\nsrc/a\n{JOINED}b.rs\n- item\n  {JOINED}{SPACE}{SPACE}more"
        ));
        assert_eq!(reply[1].shown, "test --all");
        assert_eq!(
            text(&reply, 0, 99),
            "run cargo test --all\nsrc/ab.rs\n- item  more"
        );
        assert_eq!(text(&reply[..2], 4, 4), "cargo test");
        assert_eq!(text(&reply[..1], 4, 9), "cargo");
        // Gutters are left out, also where the selection starts or ends in one; the last line
        // a copy takes is not joined.
        let output = lines(&format!(
            "  {FROM}first\n  {FROM}  second\n  {FROM}wrapped\n  {JOINED}on"
        ));
        assert_eq!(text(&output, 0, 99), "first\n  second\nwrappedon");
        assert_eq!(text(&output, 3, 1), "irst\n  second\nwrapped");
        // The prompt's rows keep the spaces they end with.
        let prompt = lines(&format!("> {FROM}one two \n  {JOINED}three\n. {FROM}next"));
        assert_eq!(prompt[1].shown, "  three");
        assert_eq!(text(&prompt, 0, 99), "one two three\nnext");
    }

    #[test]
    fn rows_make_a_line_each_and_a_copy_leaves_rules_out() {
        let dock = [
            rows(&["", "status"]),
            vec![Line::rule("\x1b[2m────\x1b[0m")],
            rows(&[format!("> {FROM}one "), format!("  {JOINED}two")]),
        ]
        .concat();
        assert_eq!(dock.len(), 5);
        assert_eq!(dock[2].shown, "\x1b[2m────\x1b[0m");
        assert_eq!(text(&dock, 0, 99), "\nstatus\none two");
        assert_eq!(text(&dock[2..], 0, 99), "one two");
        assert_eq!(text(&dock[1..3], 2, 99), "atus");
    }
}
