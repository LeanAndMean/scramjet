//! How the full screen shows prompts, reasoning, tool calls and replies (the `look` setting):
//! the classic look, or one of the variants made for the user to pick from. Printed output
//! (`-p`) keeps the classic look.

use super::style::{self, Band, Style};
use super::text::{FROM, UNTIL, Wrap, columns, plain, visible};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Design {
    /// `>` before prompts, a `●` line for each call in the color of how it went, dim reasoning.
    #[default]
    Classic,
    /// A rail down the left of each block: the accent's beside prompts, a row above and below,
    /// a dotted one beside reasoning, a thin one beside replies, and beside a call's result one
    /// in the color of how it went.
    Rails,
    /// Blocks on the theme's backgrounds: a band for each prompt, and each call on the
    /// background of how it went.
    Panels,
    /// A column of labels down the left: who speaks, the tool that ran in the color of how it
    /// went, or the kind of notice; a rule above each prompt, and a call's output under a thin
    /// rail at the left edge, so that it keeps its width.
    Margin,
    /// Rounded cards: one around each prompt in the accent, and around each call in the color
    /// of how it went, its tool, glyph and time on the top edge; a `✦` before each reply.
    Cards,
    /// A log of actions, after Codex: a `•` before each call in the color of how it went, a bold
    /// verb for what it did, its output under a `└`, a `•` before each reply, and a rule saying
    /// how long a turn worked once it ends.
    Outline,
}

impl Design {
    pub(super) const ALL: [Design; 6] = [
        Self::Classic,
        Self::Rails,
        Self::Panels,
        Self::Margin,
        Self::Cards,
        Self::Outline,
    ];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Rails => "rails",
            Self::Panels => "panels",
            Self::Margin => "margin",
            Self::Cards => "cards",
            Self::Outline => "outline",
        }
    }

    pub(super) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|design| design.name() == name)
    }

    pub(super) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|&design| design == self);
        Self::ALL[(at.unwrap_or(0) + 1) % Self::ALL.len()]
    }

    /// What goes before a prompt's rows, `width` columns wide; `None`: the editor's own `>`, in
    /// the accent.
    pub(super) fn prompt(self, width: usize, marked: bool) -> Option<Gutter> {
        let mark = |text: &str| format!("{}{text}{}", style::PROMPT_MARK, style::RESET);
        match self {
            Self::Classic => None,
            Self::Rails => Some(Gutter::same(&format!("{} ", mark("▌")), 2, None, marked)),
            Self::Panels => Some(Gutter::new(
                &format!(" {} ", mark("›")),
                "   ",
                3,
                Some(Band::Prompt),
                marked,
            )),
            Self::Margin => Some(Gutter::new(
                &label("you", style::PROMPT_LABEL),
                MARGIN_BLANK,
                MARGIN,
                None,
                marked,
            )),
            Self::Cards => {
                let side = format!("{}│{}", style::PROMPT_MARK, style::RESET);
                let first = format!("{side} {} ", mark("›"));
                // Its side at the right takes 2 of the 6 columns.
                let gutter = Gutter::new(&first, &format!("{side}   "), 6, None, marked);
                Some(gutter.closed(&format!(" {side}"), width))
            }
            Self::Outline => Some(Gutter::new(
                &format!("{} ", mark("›")),
                "  ",
                2,
                None,
                marked,
            )),
        }
    }

    /// The rows above and below a prompt's, `width` columns wide: its band, its rail alone, or
    /// a rule above it where a turn starts.
    pub(super) fn prompt_pads(self, width: usize) -> (Option<String>, Option<String>) {
        match self {
            Self::Panels => {
                let band = pad_row(Band::Prompt);
                (Some(band.clone()), Some(band))
            }
            Self::Rails => {
                let rail = format!("{}▌{}", style::PROMPT_MARK, style::RESET);
                (Some(rail.clone()), Some(rail))
            }
            Self::Margin => {
                let rule = format!("{}{}{}", style::RAIL, "─".repeat(width), style::RESET);
                (Some(rule), None)
            }
            Self::Cards => {
                let edge = |left, right| card_edge(style::PROMPT_MARK, left, right, width);
                (Some(edge('╭', '╮')), Some(edge('╰', '╯')))
            }
            Self::Classic | Self::Outline => (None, None),
        }
    }

    /// The row a call's block ends with, `width` columns wide, where the look closes it: a
    /// card's bottom edge.
    pub(super) fn bottom(self, outcome: Outcome, width: usize) -> Option<String> {
        (self == Self::Cards).then(|| card_edge(outcome.style(), '╰', '╯', width))
    }

    /// What goes before each row of reasoning; `None`: nothing.
    pub(super) fn reasoning(self, marked: bool) -> Option<Gutter> {
        match self {
            Self::Classic => None,
            Self::Rails => {
                let rail = format!("{}┊{} ", style::RAIL, style::RESET);
                Some(Gutter::same(&rail, 2, None, marked))
            }
            Self::Panels => Some(Gutter::same(" ", 1, None, marked)),
            Self::Cards | Self::Outline => Some(Gutter::same("  ", 2, None, marked)),
            Self::Margin => Some(Gutter::new(
                &label("think", style::THOUGHT_LABEL),
                MARGIN_BLANK,
                MARGIN,
                None,
                marked,
            )),
        }
    }

    /// The style reasoning is shown in.
    pub(super) fn reasoning_style(self) -> Style {
        match self {
            Self::Classic => style::REASONING,
            _ => style::THOUGHT,
        }
    }

    /// What goes before each row of a reply; `None`: nothing.
    pub(super) fn reply(self, marked: bool) -> Option<Gutter> {
        match self {
            Self::Panels => Some(Gutter::same(" ", 1, None, marked)),
            Self::Margin => Some(Gutter::new(
                &label("reply", style::REPLY_LABEL),
                MARGIN_BLANK,
                MARGIN,
                None,
                marked,
            )),
            Self::Rails => {
                let rail = format!("{}▏{} ", style::REPLY_RAIL, style::RESET);
                Some(Gutter::same(&rail, 2, None, marked))
            }
            Self::Cards => {
                let star = format!("{}✦{} ", style::PROMPT_MARK, style::RESET);
                Some(Gutter::new(&star, "  ", 2, None, marked))
            }
            Self::Outline => {
                let bullet = format!("{}•{} ", style::MUTED, style::RESET);
                Some(Gutter::new(&bullet, "  ", 2, None, marked))
            }
            Self::Classic => None,
        }
    }

    /// What goes before each row of a notice of the session's own (an error, a retry), whose
    /// text is in `style`, of the kind `kind` names in a word; `None`: nothing.
    pub(super) fn notice(self, kind: &str, style: Style, marked: bool) -> Option<Gutter> {
        match self {
            Self::Rails | Self::Cards => {
                let mark = format!("{style}!{} ", style::RESET);
                Some(Gutter::new(&mark, "  ", 2, None, marked))
            }
            Self::Margin => {
                let first = label(kind, style);
                Some(Gutter::new(&first, MARGIN_BLANK, MARGIN, None, marked))
            }
            Self::Outline => {
                let mark = format!("{style}■{} ", style::RESET);
                Some(Gutter::new(&mark, "  ", 2, None, marked))
            }
            Self::Panels => Some(Gutter::same(" ", 1, None, marked)),
            Self::Classic => None,
        }
    }

    /// What goes before each row of a message of the session's own (a command's report,
    /// `(cancelled)`): nothing but the room that brings it to the replies' left edge.
    pub(super) fn message(self, marked: bool) -> Option<Gutter> {
        match self {
            Self::Classic => None,
            Self::Panels => Some(Gutter::same(" ", 1, None, marked)),
            Self::Rails | Self::Cards | Self::Outline => Some(Gutter::same("  ", 2, None, marked)),
            Self::Margin => Some(Gutter::same(MARGIN_BLANK, MARGIN, None, marked)),
        }
    }

    /// What goes before each row of a call's result, or of its output while it runs, `width`
    /// columns wide; in cards, also what closes it at the right.
    pub(super) fn output(self, outcome: Outcome, width: usize, marked: bool) -> Gutter {
        let color = match outcome {
            Outcome::Failed => style::RAIL_FAILED,
            Outcome::Running => style::RAIL_RUNNING,
            Outcome::Done if self == Self::Rails => style::RAIL_DONE,
            Outcome::Done => style::RAIL,
        };
        let rail = |rail: &str| {
            let rail = format!("{color}{rail}{} ", style::RESET);
            Gutter::same(&rail, 2, None, marked)
        };
        match self {
            Self::Classic => Gutter {
                blank: String::new(),
                ..Gutter::same("  ", 2, None, marked)
            },
            Self::Rails => rail("│"),
            Self::Panels => Gutter::same(" ", 1, Some(outcome.band()), marked),
            Self::Margin => rail("▏"),
            Self::Cards => {
                let side = format!("{}│{}", outcome.style(), style::RESET);
                let gutter = Gutter::same(&format!("{side} "), 4, None, marked);
                gutter.closed(&format!(" {side}"), width)
            }
            Self::Outline => {
                let elbow = format!("  {color}└{} ", style::RESET);
                Gutter::new(&elbow, "    ", 4, None, marked)
            }
        }
    }

    /// A call's heading, `width` columns wide: the tool and what it was called with, how it went,
    /// and `took` (how long it ran) at the right; the lines of a command after its first go
    /// under the first's text. Not for the classic look, whose `text::call_line` shows the call
    /// before its result.
    pub(super) fn heading(
        self,
        name: &str,
        summary: &str,
        outcome: Outcome,
        took: Option<&str>,
        width: usize,
        marked: bool,
    ) -> String {
        let name_shown = format!("{}{name}{}", style::TOOL_NAME, style::RESET);
        let glyph = format!("{}{}{}", outcome.style(), outcome.glyph(), style::RESET);
        let took = took.map(|took| format!("{}{took}{}", style::MUTED, style::RESET));
        let (lead, lead_columns, right, band) = match self {
            Self::Classic | Self::Rails => {
                let lead = format!("{glyph} {name_shown} ");
                (lead, name.chars().count() + 3, took, None)
            }
            Self::Panels => {
                let right = match took {
                    Some(took) => format!("{took} {glyph} "),
                    None => format!("{glyph} "),
                };
                let edge = edge(outcome.band()).unwrap_or_else(|| " ".into());
                let lead = format!("{edge}{name_shown}  ");
                (
                    lead,
                    name.chars().count() + 3,
                    Some(right),
                    Some(outcome.band()),
                )
            }
            Self::Margin => {
                let (shown, full) = margin_label(name);
                let lead = label(&shown, outcome.label_style());
                // A name the label shortens shows whole before what it was called with.
                let lead = match full {
                    true => lead,
                    false => format!("{lead}{name_shown} "),
                };
                let columns = MARGIN + if full { 0 } else { name.chars().count() + 1 };
                let right = match took {
                    Some(took) => format!("{took} {glyph}"),
                    None => glyph,
                };
                (lead, columns, Some(right), None)
            }
            Self::Cards => {
                let border = outcome.style();
                let lead = format!("{border}╭─{} {glyph} {name_shown} ", style::RESET);
                let right = match took {
                    Some(took) => format!(" {took} {border}─╮{}", style::RESET),
                    None => format!("{border}─╮{}", style::RESET),
                };
                (lead, name.chars().count() + 6, Some(right), None)
            }
            Self::Outline => {
                let bullet = match outcome {
                    Outcome::Running => '◦',
                    Outcome::Done => '•',
                    Outcome::Failed => '✗',
                };
                let verb = verb(name, outcome);
                let lead = format!(
                    "{}{bullet}{} {}{verb}{} ",
                    outcome.style(),
                    style::RESET,
                    style::TOOL_NAME,
                    style::RESET
                );
                (lead, verb.chars().count() + 3, took, None)
            }
        };
        // A card's top edge runs on to its right, and its sides down the rows under it.
        let card = (self == Self::Cards).then(|| outcome.style());
        // Rows under the first keep a panel's edge.
        let below = match (band.and_then(edge), card) {
            (Some(edge), _) => format!("{edge}{}", " ".repeat(indent_of(lead_columns, width) - 1)),
            (None, Some(border)) => format!(
                "{border}│{}{}",
                style::RESET,
                " ".repeat(indent_of(lead_columns, width) - 1)
            ),
            (None, None) => " ".repeat(indent_of(lead_columns, width)),
        };
        let until = if marked {
            UNTIL.to_string()
        } else {
            String::new()
        };
        let band = band.map(Band::mark).map(String::from).unwrap_or_default();
        let from = if marked {
            FROM.to_string()
        } else {
            String::new()
        };
        // What it was called with starts under the first line's when that leaves room.
        let indent = indent_of(lead_columns, width);
        let right_columns = right.as_deref().map_or(0, |right| columns(right) + 1);
        let room = width.saturating_sub(indent + right_columns).max(10);
        let summary = visible(summary.trim_end_matches('\n'));
        let path = matches!(name, "read" | "edit" | "write" | "ls" | "find" | "grep");
        let range = match name {
            "read" => line_range(&summary),
            _ => 0,
        };
        let mut rows: Vec<String> = Vec::new();
        for line in summary.split('\n') {
            let mut wrap = Wrap::new(room).marking(marked);
            let wrapped = wrap.push(line) + &wrap.finish();
            let count = wrapped.split('\n').count();
            for (at, row) in wrapped.split('\n').enumerate() {
                let row = match (path, name) {
                    (false, _) => row.to_owned(),
                    // `grep` and `find` show a pattern before `in` and the path.
                    (true, "grep" | "find") => match row.rfind(" in ") {
                        Some(cut) => format!(
                            "{} in {}{}{}",
                            &row[..cut],
                            style::TOOL_PATH,
                            &row[cut + 4..],
                            style::RESET
                        ),
                        None => row.to_owned(),
                    },
                    (true, _) => {
                        let lines = &summary[summary.len() - range..];
                        let cut = match at + 1 == count && row.ends_with(lines) {
                            true => row.len() - range,
                            false => row.len(),
                        };
                        let (path, lines) = row.split_at(cut);
                        let lines = match lines.is_empty() {
                            true => String::new(),
                            false => format!("{}{lines}{}", style::TOOL_RANGE, style::RESET),
                        };
                        format!("{}{path}{}{lines}", style::TOOL_PATH, style::RESET)
                    }
                };
                let row = row.as_str();
                rows.push(match rows.is_empty() {
                    true => {
                        let used = lead_columns + columns(row);
                        // A copy of the row ends before what is at its right.
                        let pad = match (&right, card) {
                            (Some(right), Some(border)) => {
                                let pad = width.saturating_sub(used + columns(right));
                                // The lead's space parts the rule from a call without a summary.
                                let fill = match (pad, row.is_empty()) {
                                    (0, _) => String::new(),
                                    (_, true) => {
                                        format!("{border}{}{}", "─".repeat(pad), style::RESET)
                                    }
                                    (_, false) => {
                                        let rule = "─".repeat(pad - 1);
                                        format!(" {border}{rule}{}", style::RESET)
                                    }
                                };
                                format!("{}{until}{fill}{right}", style::RESET)
                            }
                            (Some(right), None) => {
                                let pad = width.saturating_sub(used + columns(right));
                                until.clone() + &" ".repeat(pad) + right
                            }
                            (None, _) => String::new(),
                        };
                        format!("{band}{lead}{from}{row}{pad}")
                    }
                    false => match card {
                        Some(border) => {
                            let used = indent + cells(row) + 2;
                            let pad = " ".repeat(width.saturating_sub(used));
                            let side = format!("{border}│{}", style::RESET);
                            let reset = style::RESET;
                            format!("{below}{from}{row}{reset}{until}{pad} {side}")
                        }
                        None => format!("{band}{below}{from}{row}"),
                    },
                });
            }
        }
        rows.join("\n")
    }
}

/// What outline's heading calls what tool `name` does, as it runs or once it ran; a tool with no
/// verb of its own, by its name.
fn verb(name: &str, outcome: Outcome) -> &str {
    let running = outcome == Outcome::Running;
    match name {
        "bash" => ["Ran", "Running"][running as usize],
        "read" => ["Read", "Reading"][running as usize],
        "edit" => ["Edited", "Editing"][running as usize],
        "write" => ["Wrote", "Writing"][running as usize],
        "grep" | "find" => ["Searched", "Searching"][running as usize],
        "ls" => ["Listed", "Listing"][running as usize],
        name => name,
    }
}

/// Where the lines of a heading after its first start: under the first's text when that leaves
/// room.
fn indent_of(lead_columns: usize, width: usize) -> usize {
    match lead_columns < width / 2 {
        true => lead_columns,
        false => 2,
    }
}

/// Where the theme gives `band` no background of its own (as "Terminal colors"), the edge a
/// panel shows in place of its first column, in the color of what it holds, so that its rows
/// still read as one block.
fn edge(band: Band) -> Option<String> {
    let style = match band {
        Band::Prompt => style::PROMPT_MARK,
        Band::Running => style::OUTCOME_RUNNING,
        Band::Done => style::OUTCOME_DONE,
        Band::Failed => style::OUTCOME_FAILED,
        Band::Selected | Band::Message => return None,
    };
    (!band.painted()).then(|| format!("{style}▎{}", style::RESET))
}

/// A row of `band` alone, as panels pad their blocks.
pub(super) fn pad_row(band: Band) -> String {
    format!("{}{}", band.mark(), edge(band).unwrap_or_default())
}

/// A card's top or bottom edge, `width` columns wide, between its `left` and `right` corners, in
/// `style`; a copy leaves it out.
fn card_edge(style: Style, left: char, right: char, width: usize) -> String {
    let rule = "─".repeat(width.saturating_sub(2));
    format!("{style}{left}{rule}{right}{}", style::RESET)
}

/// The cells `text` takes on screen: its marks for the full screen take none.
fn cells(text: &str) -> usize {
    use unicode_width::UnicodeWidthChar;
    let shown = plain(text);
    shown
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| c.width().unwrap_or(0))
        .sum()
}

/// The length of the line range a read's summary ends with (`:3-10`, `:3`), or 0.
fn line_range(summary: &str) -> usize {
    let Some(colon) = summary.rfind(':') else {
        return 0;
    };
    let range = &summary[colon + 1..];
    let mut parts = range.splitn(2, '-');
    let digits = |part: Option<&str>| {
        part.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    };
    let first = parts.next();
    let ok = digits(first) && parts.next().is_none_or(|end| digits(Some(end)));
    match ok {
        true => summary.len() - colon,
        false => 0,
    }
}

/// How a call went, as its heading and the rows under it show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Outcome {
    Running,
    Done,
    Failed,
}

impl Outcome {
    fn glyph(self) -> char {
        match self {
            Self::Running => '○',
            Self::Done => '✓',
            Self::Failed => '✗',
        }
    }
    pub(super) fn style(self) -> Style {
        match self {
            Self::Running => style::OUTCOME_RUNNING,
            Self::Done => style::OUTCOME_DONE,
            Self::Failed => style::OUTCOME_FAILED,
        }
    }
    fn label_style(self) -> Style {
        match self {
            Self::Running => style::LABEL_RUNNING,
            Self::Done => style::LABEL_DONE,
            Self::Failed => style::LABEL_FAILED,
        }
    }
    pub(super) fn band(self) -> Band {
        match self {
            Self::Running => Band::Running,
            Self::Done => Band::Done,
            Self::Failed => Band::Failed,
        }
    }
}

/// What goes before each row of a block: the first row's, then the others', which take the
/// same `columns`; a gutter for a copy (`text::FROM`) if marked, and on a band if any.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Gutter {
    pub(super) first: String,
    pub(super) rest: String,
    /// What an empty row of the block shows.
    pub(super) blank: String,
    pub(super) columns: usize,
    /// What closes each row at the right, after `fill` columns of text, as a card's side.
    right: String,
    fill: usize,
    marked: bool,
}

impl Gutter {
    fn new(first: &str, rest: &str, columns: usize, band: Option<Band>, marked: bool) -> Self {
        let edged = |gutter: &str| match (band.and_then(edge), gutter.strip_prefix(' ')) {
            (Some(edge), Some(rest)) => edge + rest,
            _ => gutter.to_owned(),
        };
        let (first, rest) = (edged(first), edged(rest));
        let band = band.map(Band::mark).map(String::from).unwrap_or_default();
        let from = if marked {
            FROM.to_string()
        } else {
            String::new()
        };
        Self {
            first: format!("{band}{first}{from}"),
            rest: format!("{band}{rest}{from}"),
            blank: format!("{band}{rest}{from}"),
            columns,
            right: String::new(),
            fill: 0,
            marked,
        }
    }
    /// The gutter with `right` closing each row of a block `width` wide, its columns among the
    /// gutter's.
    fn closed(self, right: &str, width: usize) -> Self {
        let fill = width.saturating_sub(self.columns);
        let mut gutter = Self {
            right: right.to_owned(),
            fill,
            ..self
        };
        gutter.blank = gutter.row(false, " ");
        gutter
    }
    fn same(gutter: &str, columns: usize, band: Option<Band>, marked: bool) -> Self {
        Self::new(gutter, gutter, columns, band, marked)
    }
    /// A column of `band` before each row, as Scramjet's boxes pad their text.
    pub(super) fn boxed(band: Band, marked: bool) -> Self {
        Self::same(" ", 1, Some(band), marked)
    }
    /// Row `row` of the block, the first if `first`.
    pub(super) fn row(&self, first: bool, row: &str) -> String {
        let lead = match (row.is_empty(), first) {
            (true, _) => return self.blank.clone(),
            (false, true) => &self.first,
            (false, false) => &self.rest,
        };
        if self.right.is_empty() {
            return format!("{lead}{row}");
        }
        // A copy of the row ends before the padding and the side.
        let until = if self.marked {
            UNTIL.to_string()
        } else {
            String::new()
        };
        let pad = " ".repeat(self.fill.saturating_sub(cells(row)));
        format!("{lead}{row}{}{until}{pad}{}", style::RESET, self.right)
    }
}

/// The margin's columns: a label of up to 6 and a space.
const MARGIN: usize = 7;
const MARGIN_BLANK: &str = "       ";

/// `text` at the right of the margin, in `style`.
fn label(text: &str, style: Style) -> String {
    format!("{style}{text:>width$}{} ", style::RESET, width = MARGIN - 1)
}

/// The margin's label for a tool, and whether it is the tool's whole name: a short word for the
/// harness's own tools with long names, others cut.
fn margin_label(name: &str) -> (String, bool) {
    let short = match name {
        "subagent" => "agent",
        "delegate" => "invoke",
        "report_scramjet_command_status" => "report",
        "get_scramjet_user_input" => "ask",
        "suggest_scramjet_next_steps" => "next",
        "switch_scramjet_model" => "model",
        "create_issue" => "issue",
        "create_pr" => "pr",
        "add_issue_comment" | "add_pr_comment" => "post",
        name if name.chars().count() < MARGIN => return (name.to_owned(), true),
        name => {
            let cut: String = name.chars().take(MARGIN - 2).collect();
            return (format!("{cut}…"), false);
        }
    };
    (short.to_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_look_has_a_name_and_the_next_follows_in_turn() {
        for design in Design::ALL {
            assert_eq!(Design::named(design.name()), Some(design));
        }
        assert_eq!(Design::named("plain"), None);
        let mut design = Design::Classic;
        let seen: Vec<_> = (0..6)
            .map(|_| {
                design = design.next();
                design
            })
            .collect();
        assert_eq!(
            seen,
            [
                Design::Rails,
                Design::Panels,
                Design::Margin,
                Design::Cards,
                Design::Outline,
                Design::Classic
            ]
        );
    }

    #[test]
    fn a_heading_shows_the_call_how_it_went_and_how_long_it_took_at_the_right() {
        let shown = |design: Design, outcome, took| {
            let heading = design.heading("bash", "cargo test", outcome, took, 30, false);
            heading.split('\n').map(plain).collect::<Vec<_>>()
        };
        assert_eq!(
            shown(Design::Rails, Outcome::Failed, Some("1.2s")),
            ["✗ bash cargo test         1.2s"]
        );
        assert_eq!(
            shown(Design::Panels, Outcome::Done, None),
            ["\u{13}▎bash  cargo test           ✓ "]
        );
        assert_eq!(
            shown(Design::Margin, Outcome::Running, None),
            ["  bash cargo test            ○"]
        );
        assert_eq!(
            shown(Design::Margin, Outcome::Done, Some("3.0s")),
            ["  bash cargo test       3.0s ✓"]
        );
        // A command's later lines go under its first.
        let heading = Design::Rails.heading("bash", "cd x\nmake", Outcome::Done, None, 30, false);
        assert_eq!(plain(&heading), "✓ bash cd x\n       make");
        // A panel's edge, in 16 colors as here, runs down them too.
        let heading = Design::Panels.heading("bash", "cd x\nmake", Outcome::Done, None, 30, false);
        let rows: Vec<String> = heading.split('\n').map(plain).collect();
        assert_eq!(rows[1], "\u{13}▎      make");
        // Long names have a short label, or show whole after a cut one.
        let heading = Design::Margin.heading("subagent", "2 tasks", Outcome::Done, None, 20, false);
        assert_eq!(plain(&heading), " agent 2 tasks     ✓");
        let heading = Design::Margin.heading("mcp_search", "q", Outcome::Done, None, 22, false);
        assert_eq!(plain(&heading), "mcp_s… mcp_search q  ✓");
    }

    #[test]
    fn a_card_has_its_call_on_the_top_edge_and_its_sides_down_each_row() {
        let heading = Design::Cards.heading("bash", "cd x\nmake", Outcome::Done, None, 24, true);
        let rows: Vec<String> = heading.split('\n').map(plain).collect();
        // A copy of each row holds the command alone, without the edge or the side.
        assert_eq!(
            rows,
            [
                format!("╭─ ✓ bash {FROM}cd x{UNTIL} ────────╮"),
                format!("│         {FROM}make{UNTIL}         │"),
            ]
        );
        assert!(heading.contains(&format!("{}╭─", style::OUTCOME_DONE)));
        let heading =
            Design::Cards.heading("bash", "make", Outcome::Failed, Some("2.0s"), 24, false);
        assert_eq!(plain(&heading), "╭─ ✗ bash make ─ 2.0s ─╮");
        let output = Design::Cards.output(Outcome::Running, 12, true);
        assert_eq!(
            plain(&output.row(true, "ab")),
            format!("│ {FROM}ab{UNTIL}       │")
        );
        assert_eq!(plain(&output.blank), format!("│ {FROM} {UNTIL}        │"));
        assert!(
            output
                .first
                .contains(&format!("{}│", style::OUTCOME_RUNNING))
        );
        // A row too long for the card keeps its text whole.
        assert_eq!(
            plain(&output.row(false, "abcdefghij")),
            format!("│ {FROM}abcdefghij{UNTIL} │")
        );
        let prompt = Design::Cards.prompt(12, false).unwrap();
        assert_eq!(
            (plain(&prompt.row(true, "hi")), prompt.columns),
            ("│ › hi     │".into(), 6)
        );
        let (above, below) = Design::Cards.prompt_pads(6);
        assert_eq!(
            (above.as_deref().map(plain), below.as_deref().map(plain)),
            (Some("╭────╮".into()), Some("╰────╯".into()))
        );
        assert_eq!(Design::Panels.bottom(Outcome::Done, 6), None);
        let bottom = Design::Cards.bottom(Outcome::Failed, 6).unwrap();
        assert!(bottom.starts_with(&format!("{}╰────╯", style::OUTCOME_FAILED)));
    }

    #[test]
    fn outline_shows_a_verb_after_how_a_call_went_and_its_output_under_an_elbow() {
        let shown = |name, outcome, took| {
            plain(&Design::Outline.heading(name, "src", outcome, took, 30, true))
        };
        // A copy of the heading holds what the call was called with.
        assert_eq!(
            shown("bash", Outcome::Running, Some("4s")),
            format!("◦ Running {FROM}src{UNTIL}               4s")
        );
        assert_eq!(
            shown("grep", Outcome::Done, None),
            format!("• Searched {FROM}src")
        );
        assert_eq!(
            shown("read", Outcome::Failed, None),
            format!("✗ Read {FROM}src")
        );
        assert_eq!(
            shown("mcp_call", Outcome::Done, None),
            format!("• mcp_call {FROM}src")
        );
        let heading = Design::Outline.heading("bash", "x", Outcome::Failed, None, 30, false);
        assert!(heading.starts_with(&format!("{}✗", style::OUTCOME_FAILED)));
        let output = Design::Outline.output(Outcome::Failed, 30, true);
        assert_eq!(plain(&output.row(true, "a")), format!("  └ {FROM}a"));
        assert_eq!(plain(&output.row(false, "b")), format!("    {FROM}b"));
        assert!(output.first.contains(&format!("{}└", style::RAIL_FAILED)));
        let reply = Design::Outline.reply(false).unwrap();
        assert_eq!(plain(&reply.row(true, "ok")), "• ok");
        assert_eq!(plain(&reply.row(false, "more")), "  more");
        let notice = Design::Outline.notice("retry", style::WARNING, false);
        assert_eq!(plain(&notice.unwrap().row(true, "busy")), "■ busy");
        let prompt = Design::Outline.prompt(30, false).unwrap();
        assert_eq!(plain(&prompt.row(true, "hi")), "› hi");
        assert_eq!(Design::Outline.prompt_pads(30), (None, None));
    }

    #[test]
    fn a_file_tools_path_takes_the_accent_and_a_reads_lines_the_warning_color() {
        let (path, range, reset) = (style::TOOL_PATH, style::TOOL_RANGE, style::RESET);
        let heading =
            |name, summary| Design::Panels.heading(name, summary, Outcome::Done, None, 60, false);
        assert!(
            heading("read", "src/main.rs:3-10")
                .contains(&format!("{path}src/main.rs{reset}{range}:3-10{reset}"))
        );
        assert!(heading("edit", "a:b.rs").contains(&format!("{path}a:b.rs{reset}")));
        assert!(
            heading("grep", "/x in y/ in src").contains(&format!("/x in y/ in {path}src{reset}"))
        );
        assert!(!heading("bash", "cat src/main.rs").contains(path.get()));
        assert_eq!(line_range("a.rs:3"), 2);
        assert_eq!(line_range("a.rs:3-"), 0);
        assert_eq!(line_range("a.rs"), 0);
    }

    #[test]
    fn gutters_go_before_each_row_and_blank_rows_keep_rails_and_bands() {
        let rails = Design::Rails.output(Outcome::Failed, 40, true);
        assert_eq!(plain(&rails.row(true, "x")), format!("│ {FROM}x"));
        // A copy of an empty row holds nothing of its rail.
        assert_eq!(plain(&rails.blank), format!("│ {FROM}"));
        assert!(rails.first.contains(style::RAIL_FAILED.get()));
        let done = Design::Rails.output(Outcome::Done, 40, false);
        assert!(done.first.contains(style::RAIL_DONE.get()));
        let reply = Design::Rails.reply(false).unwrap();
        assert_eq!(plain(&reply.row(true, "ok")), "▏ ok");
        let notice = Design::Rails
            .notice("retry", style::WARNING, false)
            .unwrap();
        assert_eq!(plain(&notice.row(true, "a")), "! a");
        assert_eq!(plain(&notice.row(false, "b")), "  b");
        // Panels put a notice, and a message of the session's own, at the replies' edge.
        let notice = Design::Panels
            .notice("retry", style::WARNING, false)
            .unwrap();
        assert_eq!(plain(&notice.row(true, "a")), " a");
        for design in Design::ALL {
            let reply = design.reply(false).map_or(0, |gutter| gutter.columns);
            let message = design.message(false).map_or(0, |gutter| gutter.columns);
            assert_eq!(message, reply, "{design:?}");
        }
        assert_eq!(
            plain(&Design::Margin.message(true).unwrap().blank),
            format!("       {FROM}")
        );
        // The margin labels a notice with its kind, in its color.
        let notice = Design::Margin
            .notice("retry", style::WARNING, false)
            .unwrap();
        assert_eq!(plain(&notice.row(true, "a")), " retry a");
        assert_eq!(plain(&notice.row(false, "b")), "       b");
        assert!(notice.first.contains(style::WARNING.get()));
        // Its output keeps the width but for a thin rail in the color of how the call went.
        let failed = Design::Margin.output(Outcome::Failed, 40, false);
        assert_eq!(
            (plain(&failed.row(true, "x")), failed.columns),
            ("▏ x".into(), 2)
        );
        assert!(failed.first.contains(style::RAIL_FAILED.get()));
        assert!(
            Design::Margin
                .output(Outcome::Done, 40, false)
                .first
                .contains(style::RAIL.get())
        );
        // A rule above a prompt says where a turn starts.
        let (above, below) = Design::Margin.prompt_pads(5);
        assert_eq!(
            (above.as_deref().map(plain), below),
            (Some("─".repeat(5)), None)
        );
        let classic = Design::Classic.output(Outcome::Done, 40, true);
        assert_eq!(classic.row(false, "x"), format!("  {FROM}x"));
        assert_eq!(classic.row(false, ""), "");
        let panels = Design::Panels.prompt(40, false).unwrap();
        // In 16 colors, as here, an edge in the accent stands for the prompt's band.
        assert_eq!(plain(&panels.row(true, "hi")), "\u{11}▎› hi");
        assert_eq!(plain(&panels.row(false, "")), "\u{11}▎  ");
        assert!(panels.first.contains(&format!("{}▎", style::PROMPT_MARK)));
        assert_eq!(plain(&pad_row(Band::Prompt)), "\u{11}▎");
        assert_eq!(plain(&pad_row(Band::Message)), "\u{16}");
        let margin = Design::Margin.reasoning(false).unwrap();
        assert_eq!(plain(&margin.row(true, "a")), " think a");
        assert_eq!(plain(&margin.row(false, "b")), "       b");
        // A reply is labeled, so it does not read as part of the call before it.
        let reply = Design::Margin.reply(false).unwrap();
        assert_eq!(plain(&reply.row(true, "ok")), " reply ok");
        assert_eq!(plain(&reply.row(false, "more")), "       more");
    }
}
