//! Text selected with the mouse in the full screen's conversation, as Scramjet's retained view
//! selects it: between the starts of the cells pressed and dragged to, shown reversed, copied
//! as displayed. Lines are the screen's (`view::screen_lines`), which hold only style sequences
//! besides the text.

use super::style;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

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
                    shown += style::SELECTION;
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
/// the last, a line each, without their trailing spaces.
pub(super) fn text(lines: &[String], left: usize, right: usize) -> String {
    let last = lines.len().saturating_sub(1);
    let rows: Vec<_> = (lines.iter().enumerate())
        .map(|(index, line)| {
            let from = if index == 0 { left } else { 0 };
            let to = if index == last { right } else { usize::MAX };
            cut(line, from, to).trim_end().to_owned()
        })
        .collect();
    rows.join("\n")
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
    fn the_text_copied_is_the_cells_selected_a_line_each() {
        let lines = [
            "\x1b[2m> first line  \x1b[0m".to_owned(),
            String::new(),
            "  indented 界 wide".to_owned(),
        ];
        assert_eq!(text(&lines, 2, 13), "first line\n\n  indented 界");
        assert_eq!(text(&lines[2..], 11, 12), "界");
        assert_eq!(text(&lines[..1], 2, 7), "first");
    }
}
