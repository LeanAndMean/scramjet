//! How the full screen's bottom looks (the `bottom` setting): the status row, the prompt or a
//! dialog in its place, and the footer. `pi` is Scramjet's; the others are made for the user to
//! pick from, each keeping all that the footer and the status row tell.

use super::selection;
use super::style::{self, RESET, Style};
use super::text::{FROM, UNTIL, clip, columns, plain};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Bottom {
    /// As Scramjet's: rules above and below the prompt in the thinking level's color, and two
    /// footer lines, the second's counts cut first, then the model, on a narrow screen.
    #[default]
    Pi,
    /// After Gemini CLI and Claude Code: the prompt in a rounded box in the thinking level's
    /// color, each dialog in a box of its own; under it where the session is with its modes at
    /// the right, then the output rate, the context's fill and the cost with the model at the
    /// right, wrapped onto more rows rather than cut where they do not fit.
    Boxed,
}

impl Bottom {
    pub(super) const ALL: [Bottom; 2] = [Self::Pi, Self::Boxed];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Boxed => "boxed",
        }
    }

    pub(super) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|bottom| bottom.name() == name)
    }

    pub(super) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|&bottom| bottom == self);
        Self::ALL[(at.unwrap_or(0) + 1) % Self::ALL.len()]
    }
}

/// A piece of the footer's text and its style.
pub(super) type Segment = (String, Style);

const BETWEEN: &str = " · ";

/// `segments` packed onto rows of at most `room` columns, `BETWEEN` them; one wider than a row
/// is clipped.
fn pack(segments: &[Segment], room: usize) -> Vec<Vec<Segment>> {
    let mut rows: Vec<(Vec<Segment>, usize)> = Vec::new();
    for (text, style) in segments.iter().filter(|(text, _)| !text.is_empty()) {
        let text = clip(text, room);
        let used = columns(&text);
        match rows.last_mut() {
            Some((row, width)) if *width + columns(BETWEEN) + used <= room => {
                row.push((text, *style));
                *width += columns(BETWEEN) + used;
            }
            _ => rows.push((vec![(text, *style)], used)),
        }
    }
    rows.into_iter().map(|(row, _)| row).collect()
}

fn joined(row: &[Segment]) -> (String, usize) {
    let between = format!("{}{BETWEEN}{RESET}", style::FOOTER);
    let shown = (row.iter())
        .map(|(text, style)| format!("{style}{text}{RESET}"))
        .collect::<Vec<_>>()
        .join(&between);
    let width = (row.iter()).map(|(text, _)| columns(text)).sum::<usize>()
        + columns(BETWEEN) * row.len().saturating_sub(1);
    (shown, width)
}

/// `left`'s segments from the left of rows `room` columns wide, and `right`'s at the right of
/// the last of them, two columns apart at least, or where they do not fit there on rows of their
/// own at the right.
pub(super) fn spread(left: &[Segment], right: &[Segment], room: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut last = 0;
    for row in pack(left, room) {
        let (shown, width) = joined(&row);
        rows.push(shown);
        last = width;
    }
    let right = pack(right, room);
    let beside = rows.len() == 1 || right.len() == 1;
    for (index, row) in right.iter().enumerate() {
        let (shown, width) = joined(row);
        match rows.last_mut() {
            Some(previous) if index == 0 && beside && last > 0 && last + 2 + width <= room => {
                *previous += &" ".repeat(room - last - width);
                *previous += &shown;
            }
            _ => rows.push(" ".repeat(room - width) + &shown),
        }
        last = 0;
    }
    rows
}

/// A box's top or bottom edge `width` columns wide in `border`, from `left` to `right`, with
/// `label` (styled) after its first stretch.
pub(super) fn edge(
    border: Style,
    (left, right): (char, char),
    label: &str,
    width: usize,
) -> String {
    let inner = width.saturating_sub(2);
    let label = match label.is_empty() {
        true => String::new(),
        false => clip(&format!(" {label}{} ", border), inner.saturating_sub(3)),
    };
    let lead = match label.is_empty() {
        true => "",
        false => "───",
    };
    let rest = inner.saturating_sub(columns(lead) + columns(&label));
    format!(
        "{border}{left}{lead}{RESET}{label}{border}{}{right}{RESET}",
        "─".repeat(rest)
    )
}

/// `rows`, each at most `width - 4` columns, between a box's sides in `border`, `width` columns
/// wide; a copy of a row leaves the sides out.
pub(super) fn sides(border: Style, rows: &[String], width: usize) -> Vec<String> {
    let inner = width.saturating_sub(4);
    (rows.iter())
        .map(|row| {
            let used = selection::width(&selection::row(row).shown);
            let padding = " ".repeat(inner.saturating_sub(used));
            let until = match row.contains(UNTIL) {
                true => String::new(),
                false => UNTIL.to_string(),
            };
            format!("{border}│{RESET} {FROM}{row}{padding}{until} {border}│{RESET}")
        })
        .collect()
}

/// Whether `row` is a rule, all `─`.
pub(super) fn is_rule(row: &str) -> bool {
    let text = plain(row);
    let text = text.trim();
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(rows: &[String]) -> Vec<String> {
        rows.iter().map(|row| plain(row)).collect()
    }

    fn segments(texts: &[&str]) -> Vec<Segment> {
        texts
            .iter()
            .map(|text| (text.to_string(), style::FOOTER))
            .collect()
    }

    #[test]
    fn names_round_trip_and_cycle() {
        for bottom in Bottom::ALL {
            assert_eq!(Bottom::named(bottom.name()), Some(bottom));
        }
        assert_eq!(Bottom::named("tall"), None);
        assert_eq!(Bottom::Pi.next(), Bottom::Boxed);
        assert_eq!(Bottom::Boxed.next(), Bottom::Pi);
        assert_eq!(Bottom::default(), Bottom::Pi);
    }

    #[test]
    fn segments_share_a_row_where_they_fit_and_wrap_where_they_do_not() {
        let left = segments(&["~/repos/project (main)", "Refactor"]);
        let right = segments(&["sandbox read-only", "autopilot"]);
        assert_eq!(
            shown(&spread(&left, &right, 70)),
            ["~/repos/project (main) · Refactor        sandbox read-only · autopilot"]
        );
        // The right goes on a row of its own, at the right, rather than being cut.
        assert_eq!(
            shown(&spread(&left, &right, 40)),
            [
                "~/repos/project (main) · Refactor",
                "           sandbox read-only · autopilot"
            ]
        );
        assert_eq!(
            shown(&spread(&left, &right, 24)),
            [
                "~/repos/project (main)",
                "Refactor",
                "       sandbox read-only",
                "               autopilot"
            ]
        );
        // Beside the last row of the left when it fits there.
        assert_eq!(
            shown(&spread(&left, &segments(&["auto"]), 24)),
            ["~/repos/project (main)", "Refactor            auto"]
        );
        assert_eq!(shown(&spread(&[], &right, 40)).len(), 1);
        // A segment wider than the row is clipped.
        assert_eq!(
            shown(&spread(&segments(&["a-very-long-directory-name"]), &[], 10)),
            ["a-very-lo…"]
        );
    }

    #[test]
    fn a_box_has_rounded_edges_with_labels_and_sides_a_copy_leaves_out() {
        let top = edge(style::FOOTER, ('╭', '╮'), "↑ 3 more", 20);
        assert_eq!(plain(&top), "╭─── ↑ 3 more ─────╮");
        assert_eq!(columns(&top), 20);
        assert_eq!(
            plain(&edge(style::FOOTER, ('╰', '╯'), "", 10)),
            "╰────────╯"
        );
        let rows = sides(style::FOOTER, &["> hello".into()], 20);
        let line = selection::row(&rows[0]);
        assert_eq!(plain(&line.shown), "│ > hello          │");
        assert_eq!(line.copied(0, usize::MAX), Some((2, 18)));
        assert!(is_rule("\x1b[2m──────\x1b[0m"));
        assert!(!is_rule("── a ──") && !is_rule(""));
    }
}
