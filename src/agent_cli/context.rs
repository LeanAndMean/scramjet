//! `/context`: how much of the window each part of the context takes, as Claude Code's.
use super::report::{ReportView, Tab};
use super::slash::grouped;
use super::terminal::tokens;
use scramjet::agent::context::Breakdown;

/// The breakdown as text: its categories, then free space and the compaction reserve, with the
/// items under each category with `details`.
pub(super) fn report(breakdown: &Breakdown, details: bool) -> String {
    let window = breakdown.window;
    let reserve = breakdown.reserve.unwrap_or(0);
    let free = window.map(|window| window.saturating_sub(breakdown.used + reserve));
    let mut rows: Vec<(String, u64)> = Vec::new();
    for category in &breakdown.categories {
        rows.push((category.name.to_owned(), category.tokens));
        if details {
            rows.extend(
                (category.items.iter()).map(|(name, tokens)| (format!("  {name}"), *tokens)),
            );
        }
    }
    rows.extend(free.map(|free| ("Free space".to_owned(), free)));
    rows.extend((breakdown.reserve).map(|reserve| ("Compaction reserve".to_owned(), reserve)));
    let name_width = rows.iter().map(|(name, _)| name.chars().count()).max();
    let name_width = name_width.unwrap_or(0);
    let count_width = (rows.iter()).map(|(_, n)| tokens(*n).len()).max();
    let count_width = count_width.unwrap_or(0);
    let used = tokens(breakdown.used);
    let mut text = match window {
        Some(window) => format!(
            "{}: {used} of {} tokens ({})\n",
            breakdown.model,
            tokens(window),
            share(breakdown.used, window)
        ),
        None => format!(
            "{}: {used} tokens; its window is not known\n",
            breakdown.model
        ),
    };
    text += &match breakdown.reported {
        true => format!(
            "Estimated: each part's size from its text, scaled to the {} tokens the last \
response reported.\n\n",
            grouped(breakdown.used)
        ),
        false => "Estimated from each part's text alone: no response has reported the \
context's size yet.\n\n"
            .to_owned(),
    };
    for (name, n) in rows {
        let line = format!("  {name:<name_width$}  {:>count_width$}", tokens(n));
        text += &match window {
            Some(window) => format!("{line}  {:>6}\n", share(n, window)),
            None => format!("{}\n", line),
        };
    }
    text
}

fn share(n: u64, window: u64) -> String {
    match window {
        0 => String::new(),
        _ => format!("{:.1}%", n as f64 * 100.0 / window as f64),
    }
}

/// The full screen's view: the categories, the items under each a key away, and `prompt`, the
/// system prompt's view, another key away.
pub(super) fn view(breakdown: &Breakdown, prompt: ReportView) -> ReportView {
    let tab = Tab {
        name: "Context".to_owned(),
        summary: report(breakdown, false),
        details: report(breakdown, true),
    };
    ReportView::new("Context", "summary", vec![tab], 0, false).linking("s", "system prompt", prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet::agent::context::Category;

    fn breakdown(reported: bool, window: Option<u64>) -> Breakdown {
        Breakdown {
            model: "claude-haiku-5-5".to_owned(),
            window,
            used: 30_000,
            reported,
            reserve: window.map(|_| 16_384),
            categories: vec![
                Category {
                    name: "System prompt",
                    tokens: 4_000,
                    items: vec![
                        ("Opening".into(), 1_500),
                        ("Scramjet's directives".into(), 2_500),
                    ],
                },
                Category {
                    name: "Messages",
                    tokens: 26_000,
                    items: vec![("Prompts and summaries".into(), 26_000)],
                },
            ],
        }
    }

    #[test]
    fn the_report_shows_each_part_s_share_of_the_window_then_free_space_and_the_reserve() {
        assert_eq!(
            report(&breakdown(true, Some(200_000)), false),
            "claude-haiku-5-5: 30k of 200k tokens (15.0%)
Estimated: each part's size from its text, scaled to the 30,000 tokens the last response reported.

  System prompt       4.0k    2.0%
  Messages             26k   13.0%
  Free space          154k   76.8%
  Compaction reserve   16k    8.2%
"
        );
        let details = report(&breakdown(false, Some(200_000)), true);
        assert!(details.contains("no response has reported"), "{details}");
        assert!(
            details.contains("\n    Scramjet's directives  2.5k    1.2%\n"),
            "{details}"
        );
        let unknown = report(&breakdown(true, None), true);
        assert!(unknown.starts_with("claude-haiku-5-5: 30k tokens; its window is not known\n"));
        assert!(
            unknown.contains("\n  Messages                  26k\n"),
            "{unknown}"
        );
        assert!(!unknown.contains("Free space"));
    }
}
