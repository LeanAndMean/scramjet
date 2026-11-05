//! How the full screen shows prompts, reasoning, tool calls and replies (the `look` setting):
//! the classic look, or one of the variants made for the user to pick from. Printed output
//! (`-p`) keeps the classic look.

use super::style::{self, Band, Style};
use super::text::{FROM, UNTIL, Wrap, columns, visible};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Design {
    /// `>` before prompts, a dim `●` line for each call, dim reasoning.
    #[default]
    Classic,
    /// A rail down the left of each block: the accent's beside prompts, a dotted one beside
    /// reasoning, one beside a call's result, red when it failed; replies have none.
    Rails,
    /// Blocks on the theme's backgrounds: a band for each prompt, and each call on the
    /// background of how it went.
    Panels,
    /// A column of labels down the left: who speaks, or the tool that ran, in the color of how
    /// it went; everything else to the right of it.
    Margin,
}

impl Design {
    pub(super) const ALL: [Design; 4] = [Self::Classic, Self::Rails, Self::Panels, Self::Margin];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Rails => "rails",
            Self::Panels => "panels",
            Self::Margin => "margin",
        }
    }

    pub(super) fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|design| design.name() == name)
    }

    pub(super) fn next(self) -> Self {
        let at = Self::ALL.iter().position(|&design| design == self);
        Self::ALL[(at.unwrap_or(0) + 1) % Self::ALL.len()]
    }

    /// What goes before a prompt's rows; `None`: the editor's own `>`.
    pub(super) fn prompt(self, marked: bool) -> Option<Gutter> {
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
        }
    }

    /// The band a prompt's rows and the empty rows around them are on.
    pub(super) fn prompt_band(self) -> Option<Band> {
        (self == Self::Panels).then_some(Band::Prompt)
    }

    /// What goes before each row of reasoning; `None`: nothing.
    pub(super) fn reasoning(self, marked: bool) -> Option<Gutter> {
        match self {
            Self::Classic => None,
            Self::Rails => {
                let rail = format!("{}┊{} ", style::RAIL, style::RESET);
                Some(Gutter::same(&rail, 2, None, marked))
            }
            Self::Panels => Some(Gutter::same("  ", 2, None, marked)),
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
        (self == Self::Margin).then(|| Gutter::same(MARGIN_BLANK, MARGIN, None, marked))
    }

    /// What goes before each row of a call's result, or of its output while it runs.
    pub(super) fn output(self, outcome: Outcome, marked: bool) -> Gutter {
        let rail = |rail: &str, columns| {
            let color = match outcome {
                Outcome::Failed => style::RAIL_FAILED,
                Outcome::Running => style::RAIL_RUNNING,
                Outcome::Done => style::RAIL,
            };
            let rail = format!("{color}{rail}{} ", style::RESET);
            Gutter::same(&rail, columns, None, marked)
        };
        match self {
            Self::Classic => Gutter {
                blank: String::new(),
                ..Gutter::same("  ", 2, None, marked)
            },
            Self::Rails => rail("│", 2),
            Self::Panels => Gutter::same(" ", 1, Some(outcome.band()), marked),
            Self::Margin => rail(&format!("{:>width$}", "│", width = MARGIN - 1), MARGIN),
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
                let lead = format!(" {name_shown}  ");
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
                (lead, columns, took, None)
            }
        };
        let band = band.map(Band::mark).map(String::from).unwrap_or_default();
        let from = if marked {
            FROM.to_string()
        } else {
            String::new()
        };
        // What it was called with starts under the first line's when that leaves room.
        let indent = match lead_columns < width / 2 {
            true => lead_columns,
            false => 2,
        };
        let right_columns = right.as_deref().map_or(0, |right| columns(right) + 1);
        let room = width.saturating_sub(indent + right_columns).max(10);
        let summary = visible(summary.trim_end_matches('\n'));
        let mut rows: Vec<String> = Vec::new();
        for line in summary.split('\n') {
            let mut wrap = Wrap::new(room).marking(marked);
            let wrapped = wrap.push(line) + &wrap.finish();
            for row in wrapped.split('\n') {
                rows.push(match rows.is_empty() {
                    true => {
                        let used = lead_columns + columns(row);
                        // A copy of the row ends before what is at its right.
                        let pad = match &right {
                            Some(right) => {
                                let pad = width.saturating_sub(used + columns(right));
                                let until = if marked {
                                    UNTIL.to_string()
                                } else {
                                    String::new()
                                };
                                until + &" ".repeat(pad) + right
                            }
                            None => String::new(),
                        };
                        format!("{band}{lead}{from}{row}{pad}")
                    }
                    false => format!("{band}{}{from}{row}", " ".repeat(indent)),
                });
            }
        }
        rows.join("\n")
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
    fn style(self) -> Style {
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
    fn band(self) -> Band {
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
}

impl Gutter {
    fn new(first: &str, rest: &str, columns: usize, band: Option<Band>, marked: bool) -> Self {
        let band = band.map(Band::mark).map(String::from).unwrap_or_default();
        let from = if marked {
            FROM.to_string()
        } else {
            String::new()
        };
        Self {
            first: format!("{band}{first}{from}"),
            rest: format!("{band}{rest}{from}"),
            blank: format!("{band}{rest}"),
            columns,
        }
    }
    fn same(gutter: &str, columns: usize, band: Option<Band>, marked: bool) -> Self {
        Self::new(gutter, gutter, columns, band, marked)
    }
    /// Row `row` of the block, the first if `first`.
    pub(super) fn row(&self, first: bool, row: &str) -> String {
        match (row.is_empty(), first) {
            (true, _) => self.blank.clone(),
            (false, true) => format!("{}{row}", self.first),
            (false, false) => format!("{}{row}", self.rest),
        }
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
    use crate::agent_cli::text::plain;

    #[test]
    fn each_look_has_a_name_and_the_next_follows_in_turn() {
        for design in Design::ALL {
            assert_eq!(Design::named(design.name()), Some(design));
        }
        assert_eq!(Design::named("plain"), None);
        let mut design = Design::Classic;
        let seen: Vec<_> = (0..4)
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
            ["\u{13} bash  cargo test           ✓ "]
        );
        assert_eq!(
            shown(Design::Margin, Outcome::Running, None),
            ["  bash cargo test"]
        );
        // A command's later lines go under its first.
        let heading = Design::Rails.heading("bash", "cd x\nmake", Outcome::Done, None, 30, false);
        assert_eq!(plain(&heading), "✓ bash cd x\n       make");
        // Long names have a short label, or show whole after a cut one.
        let heading = Design::Margin.heading("subagent", "2 tasks", Outcome::Done, None, 40, false);
        assert_eq!(plain(&heading), " agent 2 tasks");
        let heading = Design::Margin.heading("mcp_search", "q", Outcome::Done, None, 40, false);
        assert_eq!(plain(&heading), "mcp_s… mcp_search q");
    }

    #[test]
    fn gutters_go_before_each_row_and_blank_rows_keep_rails_and_bands() {
        let rails = Design::Rails.output(Outcome::Failed, true);
        assert_eq!(plain(&rails.row(true, "x")), format!("│ {FROM}x"));
        assert_eq!(plain(&rails.blank), "│ ");
        let classic = Design::Classic.output(Outcome::Done, true);
        assert_eq!(classic.row(false, "x"), format!("  {FROM}x"));
        assert_eq!(classic.row(false, ""), "");
        let panels = Design::Panels.prompt(false).unwrap();
        assert_eq!(plain(&panels.row(true, "hi")), "\u{11} › hi");
        assert_eq!(plain(&panels.row(false, "")), "\u{11}   ");
        let margin = Design::Margin.reasoning(false).unwrap();
        assert_eq!(plain(&margin.row(true, "a")), " think a");
        assert_eq!(plain(&margin.row(false, "b")), "       b");
    }
}
