//! How the full screen's bottom looks (the `bottom` setting): the status row, the prompt or a
//! dialog in its place, and the footer. `pi` is Scramjet's; the others are made for the user to
//! pick from, each keeping all that the footer and the status row tell.

use super::footer::Parts;
use super::selection;
use super::style::{self, Band, RESET, Style};
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
    /// After Codex's composer: the prompt, or a dialog in its place, on the prompt's background
    /// with a row of it above and below, a `▎` edge down its left in the thinking level's
    /// color (which alone marks it in a theme without backgrounds); under it the figures the
    /// user reads most with the model at the right, then where the session is with its modes,
    /// wrapped onto more rows rather than cut.
    Composer,
    /// After status bars' segments: the prompt and dialogs as pi's; under them where the
    /// session is with its modes, and at the right chips of the output rate, the context's fill
    /// with a gauge, the cost and the model after a mark in the thinking level's color, wrapped
    /// onto more rows rather than cut.
    Bar,
}

impl Bottom {
    pub(super) const ALL: [Bottom; 4] = [Self::Pi, Self::Boxed, Self::Composer, Self::Bar];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Pi => "pi",
            Self::Boxed => "boxed",
            Self::Composer => "composer",
            Self::Bar => "bar",
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

/// `segments` packed onto rows of at most `room` columns, `between` them; one wider than a row
/// is clipped.
fn pack(segments: &[Segment], room: usize, between: &str) -> Vec<Vec<Segment>> {
    let mut rows: Vec<(Vec<Segment>, usize)> = Vec::new();
    for (text, style) in segments.iter().filter(|(text, _)| !text.is_empty()) {
        let text = clip(text, room);
        let used = columns(&text);
        match rows.last_mut() {
            Some((row, width)) if *width + columns(between) + used <= room => {
                row.push((text, *style));
                *width += columns(between) + used;
            }
            _ => rows.push((vec![(text, *style)], used)),
        }
    }
    rows.into_iter().map(|(row, _)| row).collect()
}

fn joined(row: &[Segment], between: &str) -> (String, usize) {
    let shown_between = format!("{}{between}{RESET}", style::FOOTER);
    let shown = (row.iter())
        .map(|(text, style)| format!("{style}{text}{RESET}"))
        .collect::<Vec<_>>()
        .join(&shown_between);
    let width = (row.iter()).map(|(text, _)| columns(text)).sum::<usize>()
        + columns(between) * row.len().saturating_sub(1);
    (shown, width)
}

/// `left`'s segments from the left of rows `room` columns wide, and `right`'s at the right of
/// the last of them, two columns apart at least, or where they do not fit there on rows of their
/// own at the right.
pub(super) fn spread(left: &[Segment], right: &[Segment], room: usize) -> Vec<String> {
    spread_apart(left, (right, BETWEEN), room)
}

/// As `spread`, with `between` between `right`'s segments.
fn spread_apart(
    left: &[Segment],
    (right, between): (&[Segment], &str),
    room: usize,
) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut last = 0;
    for row in pack(left, room, BETWEEN) {
        let (shown, width) = joined(&row, BETWEEN);
        rows.push(shown);
        last = width;
    }
    let right = pack(right, room, between);
    let beside = rows.len() == 1 || right.len() == 1;
    for (index, row) in right.iter().enumerate() {
        let (shown, width) = joined(row, between);
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

/// The context's gauge, `CELLS` wide.
const CELLS: usize = 10;

/// A gauge `cells` wide of `percent`, in half cells as rich's progress bars: the filled part,
/// `━` and a `╸` for a half, and the rest. Any fill shows half a cell at least.
fn gauge(percent: f64, cells: usize) -> (String, String) {
    let halves = (percent.clamp(0.0, 100.0) * (cells * 2) as f64 / 100.0).round() as usize;
    let halves = match percent > 0.0 {
        true => halves.max(1),
        false => 0,
    };
    let (full, half) = (halves / 2, halves % 2);
    let filled = "━".repeat(full) + if half == 1 { "╸" } else { "" };
    (filled, "━".repeat(cells - full - half))
}

/// `pieces` as a chip on `on`, a background's sequence, with a column of it either side, but
/// on the left when the first piece is its `edge`; as they are where `on` is empty, the theme
/// painting no backgrounds.
fn chip(on: &str, pieces: &[(&str, Style)], edge: bool) -> Segment {
    let pad = if on.is_empty() { "" } else { " " };
    let mut shown = format!("{on}{}", if edge { "" } else { pad });
    for (text, style) in pieces {
        shown += &format!("{RESET}{on}{style}{text}");
    }
    shown += &format!("{RESET}{on}{pad}");
    (shown, style::NONE)
}

/// The footer as a status bar, rows `room` columns wide: where the session is and its modes,
/// then at the right the chips of the output rate, how full the context is with its gauge, the
/// cost and the model, after a mark in `level`, the thinking level's style.
pub(super) fn bar(parts: &Parts, level: Style, room: usize) -> Vec<String> {
    let on = match Band::Selected.painted() {
        true => Band::Selected.get(),
        false => "",
    };
    let mut chips: Vec<Segment> = Vec::new();
    if let Some((rate, style)) = &parts.rate {
        chips.push(chip(on, &[(rate, *style)], false));
    }
    let (context, style) = &parts.context;
    if !context.is_empty() {
        let plain = chip(on, &[(context, *style)], false);
        chips.push(match parts.fill {
            Some(fill) => {
                let (filled, rest) = gauge(fill, CELLS);
                let gauged = chip(
                    on,
                    &[
                        (&filled, *style),
                        (&rest, style::FOOTER),
                        (" ", style::NONE),
                        (context, *style),
                    ],
                    false,
                );
                match columns(&gauged.0) <= room {
                    true => gauged,
                    false => plain,
                }
            }
            None => plain,
        });
    }
    if let Some((cost, style)) = &parts.cost {
        chips.push(chip(on, &[(cost, *style)], false));
    }
    let (model, _) = &parts.model;
    chips.push(chip(
        on,
        &[
            ("▌", level),
            (" ", style::NONE),
            (model, style::FOOTER_FIGURE),
        ],
        true,
    ));
    let between = match on.is_empty() {
        true => BETWEEN,
        false => " ",
    };
    let left: Vec<Segment> = parts.place.iter().chain(&parts.modes).cloned().collect();
    spread_apart(&left, (&chips, between), room)
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

/// `rows`, each at most `width - 3` columns, on the prompt's band after a `▎` edge in `edge`,
/// with a row of the band above and below holding `above` and `below` at their right, `width`
/// columns wide; a copy of a row leaves the edge out.
pub(super) fn tinted(
    edge: Style,
    rows: &[String],
    (above, below): (&str, &str),
    width: usize,
) -> Vec<String> {
    let band = Band::Prompt.mark();
    let side = format!("{band}{edge}▎{RESET} ");
    let padding = |label: &str| {
        let room = width.saturating_sub(2);
        let label = clip(label, room);
        let gap = " ".repeat(room.saturating_sub(columns(&label)));
        format!("{side}{gap}{}{label}{RESET}", style::MUTED)
    };
    let mut shown = vec![padding(above)];
    shown.extend(rows.iter().map(|row| format!("{side}{FROM}{row}")));
    shown.push(padding(below));
    shown
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
        assert_eq!(Bottom::Boxed.next(), Bottom::Composer);
        assert_eq!(Bottom::Composer.next(), Bottom::Bar);
        assert_eq!(Bottom::Bar.next(), Bottom::Pi);
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
    fn the_gauge_fills_by_half_cells_and_shows_any_fill() {
        let shown = |percent| {
            let (filled, rest) = gauge(percent, 10);
            assert_eq!(columns(&filled) + columns(&rest), 10);
            filled
        };
        assert_eq!(shown(0.0), "");
        assert_eq!(shown(0.4), "╸");
        assert_eq!(shown(9.2), "━");
        assert_eq!(shown(85.0), "━━━━━━━━╸");
        assert_eq!(shown(88.0), "━━━━━━━━━");
        assert_eq!(shown(100.0), "━━━━━━━━━━");
        assert_eq!(shown(130.0), "━━━━━━━━━━");
    }

    #[test]
    fn the_bar_puts_the_figures_and_the_model_at_the_right_of_where_the_session_is() {
        let parts = Parts {
            place: segments(&["~/p (main)"]),
            modes: segments(&["autopilot"]),
            rate: Some(("52tok/s".into(), style::FOOTER_FIGURE)),
            context: ("92k/1.0M (9.2%)".into(), style::FOOTER_FIGURE),
            fill: Some(9.2),
            cost: Some(("$0.053".into(), style::FOOTER)),
            model: ("gpt-6-luna • medium".into(), style::FOOTER),
        };
        // The terminal's colors paint no chips: the parts as the other bottoms show them.
        let rows = shown(&bar(&parts, style::PROMPT_MEDIUM, 110));
        let gauge = "━".repeat(10);
        let right = format!("52tok/s · {gauge} 92k/1.0M (9.2%) · $0.053 · ▌ gpt-6-luna • medium");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].starts_with("~/p (main) · autopilot  "), "{rows:?}");
        assert!(
            rows[0].ends_with(&right) && columns(&rows[0]) == 110,
            "{rows:?}"
        );
        // Narrower, the chips go on a row of their own, then onto more, and the gauge goes
        // before the figure is cut.
        let rows = shown(&bar(&parts, style::PROMPT_MEDIUM, 60));
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0], "~/p (main) · autopilot");
        assert!(rows[2].ends_with("  ▌ gpt-6-luna • medium"), "{rows:?}");
        let rows = shown(&bar(&parts, style::PROMPT_MEDIUM, 24));
        assert!(
            rows.contains(&"92k/1.0M (9.2%) · $0.053".to_owned()),
            "{rows:?}"
        );
        assert!(rows.iter().all(|row| columns(row) <= 24), "{rows:?}");
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
        let rows = tinted(style::FOOTER, &["> hi".into()], ("", "↓ 2 more"), 20);
        let lines: Vec<_> = rows.iter().map(|row| selection::row(row)).collect();
        let shown: Vec<_> = lines.iter().map(|line| plain(&line.shown)).collect();
        assert_eq!(
            shown,
            [
                format!("▎{}", " ".repeat(19)),
                "▎ > hi".into(),
                format!("▎{}↓ 2 more", " ".repeat(11))
            ]
        );
        assert!(lines.iter().all(|line| line.band == Some(Band::Prompt)));
        assert_eq!(lines[1].copied(0, usize::MAX), Some((2, usize::MAX)));
        assert!(!is_rule("── a ──") && !is_rule(""));
    }
}
