//! Shows the children of a `subagent` call as Scramjet's tool renders them: a row per child with
//! how it is going, its model and effort, then its latest tool calls and texts and its usage;
//! whole, with its task and its result as markdown, once Ctrl+O expands results.

use super::markdown::Markdown;
use super::style::{
    self, MUTED, SUBAGENT_DONE, SUBAGENT_FAILED, SUBAGENT_NAME, SUBAGENT_RUNNING, Style,
    TOOL_FAILED, TOOL_OUTPUT, WARNING,
};
use super::text::{Wrap, clip, printable, visible};
use scramjet::agent::{ChildItem, ChildRun, ChildState, ChildUsage, Children, Delegation};

/// Items shown of a child while results are cut: Scramjet's `COLLAPSED_ITEM_COUNT` for one
/// child, 5 for each of several.
const ITEMS: usize = 10;
const EACH_ITEMS: usize = 5;
/// Lines shown of each text while results are cut.
const TEXT_LINES: usize = 3;

/// How the rows are laid out: `width` columns, styled with `color`, wrapped lines marked for the
/// full screen's copy with `marked`, everything shown with `full`, and `hint` ending the notes
/// about what is left out.
pub(super) struct Look {
    pub(super) width: usize,
    pub(super) color: bool,
    pub(super) marked: bool,
    pub(super) full: bool,
    pub(super) hint: String,
}

/// The rows showing `children`, running or ended.
pub(super) fn rows(children: &Children, look: &Look) -> Vec<String> {
    let mut rows = Rows {
        look,
        rows: Vec::new(),
    };
    match (children.mode, &children.runs[..]) {
        (_, []) => {}
        (Delegation::Single, [run]) => rows.single(run),
        (mode, runs) => rows.several(mode, runs),
    }
    rows.rows
}

/// One line per child, for the lines above the status line of printed output: how it is going
/// and what it did last.
pub(super) fn activity(children: &Children, width: usize) -> Vec<String> {
    let lines = children.runs.iter().map(|run| {
        let last = match run.items.last() {
            Some(ChildItem::Call { name, summary }) => format!("→ {}", call(name, summary)),
            Some(ChildItem::Text(text)) => text.lines().next().unwrap_or_default().to_owned(),
            None => String::new(),
        };
        let line = format!("{} {} {last}", icon(&run.state).0, run.agent);
        clip(&printable(line.trim_end()), width)
    });
    lines.collect()
}

struct Rows<'a> {
    look: &'a Look,
    rows: Vec<String>,
}

impl Rows<'_> {
    fn paint(&self, style: Style, text: &str) -> String {
        match self.look.color && !text.is_empty() {
            true => format!("{style}{text}{}", style::RESET),
            false => text.to_owned(),
        }
    }

    /// `text`, already styled, wrapped to the width.
    fn wrapped(&mut self, text: &str) {
        let wrap = Wrap::new(self.look.width).marking(self.look.marked);
        self.wrapped_with(wrap, text);
    }

    fn wrapped_with(&mut self, mut wrap: Wrap, text: &str) {
        let text = wrap.push(text) + &wrap.finish();
        self.rows.extend(text.split('\n').map(str::to_owned));
    }

    /// Each line of `text`, wrapped, in `style`.
    fn text(&mut self, style: Style, text: &str) {
        for line in visible(text.trim_end()).lines() {
            let line = self.paint(style, line);
            self.wrapped(&line);
        }
    }

    /// One row, cut to the width.
    fn clipped(&mut self, style: Style, text: &str) {
        let row = self.paint(style, &clip(&printable(text), self.look.width));
        self.rows.push(row);
    }

    fn blank(&mut self) {
        self.rows.push(String::new());
    }

    /// The agent's name, after `before`, and its icon, model and effort.
    fn header(&mut self, before: &str, run: &ChildRun, single: bool) {
        let (glyph, glyph_style) = icon(&run.state);
        let glyph = self.paint(glyph_style, glyph);
        let mut text = match single {
            true => format!(
                "{glyph} {}{}",
                self.paint(SUBAGENT_NAME, &visible(&run.agent)),
                self.paint(MUTED, &format!(" ({})", run.source))
            ),
            false => format!(
                "{}{} {glyph}",
                self.paint(MUTED, before),
                self.paint(SUBAGENT_NAME, &visible(&run.agent))
            ),
        };
        if !run.model.is_empty() {
            text += &format!(" {}", self.paint(MUTED, &visible(&run.model)));
        }
        if let Some(effort) = &run.effort {
            text += &format!(" {}", self.paint(MUTED, &format!("[Effort:{effort}]")));
        }
        if let (
            true,
            ChildState::Failed {
                reason: Some(reason),
                ..
            },
        ) = (single, &run.state)
        {
            text += &format!(" {}", self.paint(SUBAGENT_FAILED, &format!("[{reason}]")));
        }
        self.wrapped(&text);
    }

    fn warnings(&mut self, run: &ChildRun) {
        if run.warnings.is_empty() {
            return;
        }
        let mut text = "Agent invocation warnings:".to_owned();
        for warning in &run.warnings {
            text += &format!("\n- {warning}");
        }
        self.text(WARNING, &text);
    }

    /// Nothing when it has no counts, rather than the model alone.
    fn usage(&mut self, usage: &ChildUsage, model: Option<&str>, prefix: &str) {
        if usage_line(usage, None).is_empty() {
            return;
        }
        let line = usage_line(usage, model.filter(|model| !model.is_empty()));
        self.text(MUTED, &format!("{prefix}{line}"));
    }

    /// The child's latest items, at most `limit` of them, each text cut to its first lines.
    fn latest(&mut self, run: &ChildRun, limit: usize) {
        let skipped = run.items.len().saturating_sub(limit);
        if skipped > 0 {
            let note = format!("… ({skipped} earlier items{})", self.look.hint);
            self.clipped(MUTED, &note);
        }
        for item in &run.items[skipped..] {
            match item {
                ChildItem::Call { name, summary } => {
                    self.clipped(TOOL_OUTPUT, &format!("→ {}", call(name, summary)))
                }
                ChildItem::Text(text) => {
                    let lines: Vec<&str> = text.trim_end().lines().collect();
                    for line in lines.iter().take(TEXT_LINES) {
                        self.clipped(TOOL_OUTPUT, line);
                    }
                    if lines.len() > TEXT_LINES {
                        let more = lines.len() - TEXT_LINES;
                        let note = format!("… ({more} more lines{})", self.look.hint);
                        self.clipped(MUTED, &note);
                    }
                }
            }
        }
    }

    /// Every item but the result, which follows them.
    fn every_item(&mut self, run: &ChildRun) {
        let last_text = run
            .items
            .iter()
            .rposition(|item| matches!(item, ChildItem::Text(_)));
        for (index, item) in run.items.iter().enumerate() {
            match item {
                ChildItem::Call { name, summary } => {
                    self.text(TOOL_OUTPUT, &format!("→ {}", call(name, summary)))
                }
                ChildItem::Text(_) if Some(index) == last_text => {}
                ChildItem::Text(text) => self.text(TOOL_OUTPUT, text),
            }
        }
    }

    /// The child's result, rendered as a reply is.
    fn result(&mut self, text: &str) {
        let mut markdown =
            Markdown::new(self.look.color, "", self.look.width).marking(self.look.marked);
        let text = markdown.push(visible(text).trim_end()) + &markdown.finish();
        let wrap = Wrap::new(self.look.width).marking(self.look.marked);
        self.wrapped_with(wrap.keeping_gutters(), &text);
    }

    /// When nothing else shows how it went.
    fn status(&mut self, run: &ChildRun) {
        let text = match run.state {
            ChildState::Running => "(running...)",
            _ => "(no output)",
        };
        self.text(MUTED, text);
    }

    fn single(&mut self, run: &ChildRun) {
        self.header("", run, true);
        let failure = match &run.state {
            ChildState::Failed { message, .. } => Some(message.as_str()),
            _ => None,
        };
        if !self.look.full {
            self.warnings(run);
            match failure {
                Some(message) => self.text(TOOL_FAILED, &format!("Error: {message}")),
                None if run.items.is_empty() => self.status(run),
                None => self.latest(run, ITEMS),
            }
            self.usage(&run.usage, Some(&run.model), "");
            return;
        }
        if let Some(message) = failure {
            self.text(TOOL_FAILED, &format!("Error: {message}"));
        }
        self.warnings(run);
        self.blank();
        self.text(MUTED, "─── Task ───");
        self.text(MUTED, &run.task);
        self.blank();
        self.text(MUTED, "─── Output ───");
        match run.output() {
            None if run.items.is_empty() && failure.is_none() => self.status(run),
            None if run.items.is_empty() => {}
            output => {
                self.every_item(run);
                if let Some(output) = output {
                    self.blank();
                    self.result(output);
                }
            }
        }
        if !usage_line(&run.usage, None).is_empty() {
            self.blank();
        }
        self.usage(&run.usage, Some(&run.model), "");
    }

    fn several(&mut self, mode: Delegation, runs: &[ChildRun]) {
        let count =
            |state: fn(&ChildState) -> bool| runs.iter().filter(|r| state(&r.state)).count();
        let running = count(|state| *state == ChildState::Running);
        let failed = count(|state| matches!(state, ChildState::Failed { .. }));
        let done = runs.len() - running - failed;
        let (glyph, glyph_style, title, status) = match mode {
            Delegation::Chain => {
                let (glyph, glyph_style) = match (running, failed) {
                    (0, 0) => ("✓", SUBAGENT_DONE),
                    (0, _) => ("✗", SUBAGENT_FAILED),
                    _ => ("⏳", SUBAGENT_RUNNING),
                };
                let status = format!("{done}/{} steps", runs.len());
                (glyph, glyph_style, "chain", status)
            }
            _ => {
                let (glyph, glyph_style) = match (running, failed) {
                    (0, 0) => ("✓", SUBAGENT_DONE),
                    (0, _) => ("◐", SUBAGENT_RUNNING),
                    _ => ("⏳", SUBAGENT_RUNNING),
                };
                let status = match running {
                    0 => format!("{done}/{} tasks", runs.len()),
                    _ => format!("{}/{} done, {running} running", done + failed, runs.len()),
                };
                (glyph, glyph_style, "parallel", status)
            }
        };
        let header = format!(
            "{} {} {status}",
            self.paint(glyph_style, glyph),
            self.paint(SUBAGENT_NAME, title)
        );
        self.wrapped(&header);
        let mut total = ChildUsage {
            cost: Some(0.0),
            ..ChildUsage::default()
        };
        for (index, run) in runs.iter().enumerate() {
            total.turns += run.usage.turns;
            total.input_tokens += run.usage.input_tokens;
            total.cached_input_tokens += run.usage.cached_input_tokens;
            total.cache_write_tokens += run.usage.cache_write_tokens;
            total.output_tokens += run.usage.output_tokens;
            total.cost = match (total.cost, run.usage.cost) {
                (Some(total), Some(cost)) => Some(total + cost),
                (total, _) if run.usage.turns == 0 => total,
                _ => None,
            };
            let before = match mode {
                Delegation::Chain => format!("─── Step {}: ", index + 1),
                _ => "─── ".to_owned(),
            };
            self.blank();
            self.header(&before, run, false);
            let failure = match &run.state {
                ChildState::Failed { message, .. } => Some(message.as_str()),
                _ => None,
            };
            if !self.look.full {
                self.warnings(run);
                match failure {
                    Some(message) => self.text(TOOL_FAILED, message),
                    None if run.items.is_empty() => self.status(run),
                    None => self.latest(run, EACH_ITEMS),
                }
                self.usage(&run.usage, None, "");
                continue;
            }
            self.text(MUTED, &format!("Task: {}", run.task));
            self.warnings(run);
            self.every_item(run);
            match (failure, run.output()) {
                (Some(message), output) => {
                    self.blank();
                    self.text(TOOL_FAILED, message);
                    if let Some(output) = output.filter(|output| output.trim() != message.trim()) {
                        self.text(MUTED, "Pre-error child output:");
                        self.text(TOOL_OUTPUT, output);
                    }
                }
                (None, Some(output)) => {
                    self.blank();
                    self.result(output);
                }
                (None, None) => {}
            }
            self.usage(&run.usage, Some(&run.model), "");
        }
        if !usage_line(&total, None).is_empty() {
            self.blank();
            self.usage(&total, None, "Total: ");
        }
    }
}

/// The glyph showing how a child is going, and its style.
pub(super) fn icon(state: &ChildState) -> (&'static str, Style) {
    match state {
        ChildState::Running => ("⏳", SUBAGENT_RUNNING),
        ChildState::Done => ("✓", SUBAGENT_DONE),
        ChildState::Failed { .. } => ("✗", SUBAGENT_FAILED),
    }
}

/// A child's tool call as Scramjet lists it: a command after `$`, else the tool's name and what
/// it acts on; only the first line.
pub(super) fn call(name: &str, summary: &str) -> String {
    let first = summary.lines().next().unwrap_or_default();
    match name {
        "bash" => format!("$ {first}"),
        name => format!("{name} {first}"),
    }
}

/// As Scramjet's `formatUsageStats`: turns, input neither read from the cache nor written to it,
/// output, cache reads and writes, the cost when it is known and the model.
pub(super) fn usage_line(usage: &ChildUsage, model: Option<&str>) -> String {
    let count = |n: u64| match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1e3),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1e3).round()),
        _ => format!("{:.1}M", n as f64 / 1e6),
    };
    let mut parts = Vec::new();
    if usage.turns > 0 {
        let plural = if usage.turns > 1 { "s" } else { "" };
        parts.push(format!("{} turn{plural}", usage.turns));
    }
    let cache = usage.cached_input_tokens + usage.cache_write_tokens;
    let uncached = usage.input_tokens.saturating_sub(cache);
    if uncached > 0 {
        parts.push(format!("↑{}", count(uncached)));
    }
    if usage.output_tokens > 0 {
        parts.push(format!("↓{}", count(usage.output_tokens)));
    }
    if usage.cached_input_tokens > 0 {
        parts.push(format!("R{}", count(usage.cached_input_tokens)));
    }
    if usage.cache_write_tokens > 0 {
        parts.push(format!("W{}", count(usage.cache_write_tokens)));
    }
    if let Some(cost) = usage.cost.filter(|cost| *cost > 0.0) {
        parts.push(format!("${cost:.4}"));
    }
    parts.extend(model.map(str::to_owned));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(agent: &str, state: ChildState, items: Vec<ChildItem>) -> ChildRun {
        ChildRun {
            agent: agent.into(),
            source: "user".into(),
            model: "gpt-6-luna".into(),
            effort: Some("low".into()),
            task: "Look at the parser\nand say what breaks.".into(),
            state,
            items,
            usage: ChildUsage {
                turns: 3,
                input_tokens: 12_400,
                cached_input_tokens: 8_000,
                cache_write_tokens: 0,
                output_tokens: 950,
                cost: None,
            },
            responses: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn call(name: &str, summary: &str) -> ChildItem {
        ChildItem::Call {
            name: name.into(),
            summary: summary.into(),
        }
    }

    fn shown(children: &Children, full: bool) -> Vec<String> {
        let look = Look {
            width: 50,
            color: false,
            marked: false,
            full,
            hint: ", Ctrl+O to expand".to_owned(),
        };
        rows(children, &look)
    }

    #[test]
    fn usage_shows_cache_writes_and_the_cost_when_it_is_known() {
        let usage = ChildUsage {
            turns: 2,
            input_tokens: 12_400,
            cached_input_tokens: 8_000,
            cache_write_tokens: 2_400,
            output_tokens: 950,
            cost: Some(0.01234),
        };
        assert_eq!(
            usage_line(&usage, Some("claude-haiku-5-5")),
            "2 turns ↑2.0k ↓950 R8.0k W2.4k $0.0123 claude-haiku-5-5"
        );
        let priced = |cost| ChildRun {
            usage: ChildUsage { cost, ..usage },
            ..run("scout", ChildState::Done, Vec::new())
        };
        let total = |runs| {
            let children = Children {
                mode: Delegation::Parallel,
                runs,
            };
            shown(&children, false).last().unwrap().clone()
        };
        assert_eq!(
            total(vec![priced(Some(0.01)), priced(Some(0.02))]),
            "Total: 4 turns ↑4.0k ↓1.9k R16k W4.8k $0.0300"
        );
        // A child whose cost is not known leaves the total's out.
        assert_eq!(
            total(vec![priced(Some(0.01)), priced(None)]),
            "Total: 4 turns ↑4.0k ↓1.9k R16k W4.8k"
        );
    }

    fn found() -> Vec<ChildItem> {
        vec![
            call("read", "src/parser.rs:1-40"),
            ChildItem::Text("First look.".into()),
            call("bash", "rg -n parse src\n| head"),
            ChildItem::Text("**Tabs** break it.\nLine 2\nLine 3\nLine 4\nLine 5".into()),
        ]
    }

    #[test]
    fn a_child_shows_its_latest_items_until_expanded() {
        let mut items: Vec<ChildItem> = (1..=8).map(|n| call("ls", &format!("d{n}"))).collect();
        items.extend(found());
        let single = Children {
            mode: Delegation::Single,
            runs: vec![run("scout", ChildState::Done, items)],
        };
        let rows = shown(&single, false);
        assert_eq!(
            rows[..3],
            [
                "✓ scout (user) gpt-6-luna [Effort:low]",
                "… (2 earlier items, Ctrl+O to expand)",
                "→ ls d3"
            ]
        );
        assert_eq!(
            rows[8..],
            [
                "→ read src/parser.rs:1-40",
                "First look.",
                "→ $ rg -n parse src",
                "**Tabs** break it.",
                "Line 2",
                "Line 3",
                "… (2 more lines, Ctrl+O to expand)",
                "3 turns ↑4.4k ↓950 R8.0k gpt-6-luna"
            ]
        );
        let rows = shown(&single, true);
        assert_eq!(
            rows[..7],
            [
                "✓ scout (user) gpt-6-luna [Effort:low]",
                "",
                "─── Task ───",
                "Look at the parser",
                "and say what breaks.",
                "",
                "─── Output ───"
            ]
        );
        assert_eq!(rows[7], "→ ls d1");
        // The result follows the other items, whole.
        assert_eq!(
            rows[15..],
            [
                "→ read src/parser.rs:1-40",
                "First look.",
                "→ $ rg -n parse src",
                "",
                "**Tabs** break it.",
                "Line 2",
                "Line 3",
                "Line 4",
                "Line 5",
                "",
                "3 turns ↑4.4k ↓950 R8.0k gpt-6-luna"
            ]
        );
    }

    #[test]
    fn a_failed_or_running_child_says_so() {
        let mut failed = run(
            "scout",
            ChildState::Failed {
                reason: Some("aborted".into()),
                message: "Subagent was aborted".into(),
            },
            Vec::new(),
        );
        failed.warnings = vec!["write: left out".into()];
        failed.usage = ChildUsage::default();
        let single = |run: ChildRun| Children {
            mode: Delegation::Single,
            runs: vec![run],
        };
        assert_eq!(
            shown(&single(failed.clone()), false),
            [
                "✓ scout (user) gpt-6-luna [Effort:low] [aborted]".replace('✓', "✗"),
                "Agent invocation warnings:".into(),
                "- write: left out".into(),
                "Error: Subagent was aborted".into()
            ]
        );
        assert_eq!(
            shown(&single(failed), true)[1],
            "Error: Subagent was aborted"
        );
        let running = run("scout", ChildState::Running, Vec::new());
        assert_eq!(
            shown(&single(running.clone()), false)[..2],
            ["⏳ scout (user) gpt-6-luna [Effort:low]", "(running...)"]
        );
        assert_eq!(shown(&single(running), true)[7], "(running...)");
    }

    #[test]
    fn several_children_each_show_how_they_went() {
        let unknown = ChildRun {
            source: "unknown".into(),
            model: String::new(),
            effort: None,
            usage: ChildUsage::default(),
            ..run(
                "nope",
                ChildState::Failed {
                    reason: None,
                    message: "Unknown agent: \"nope\". Available agents: \"scout\", \"critic\"."
                        .into(),
                },
                Vec::new(),
            )
        };
        let parallel = Children {
            mode: Delegation::Parallel,
            runs: vec![run("scout", ChildState::Running, found()), unknown.clone()],
        };
        assert_eq!(
            shown(&parallel, false),
            [
                "⏳ parallel 1/2 done, 1 running",
                "",
                "─── scout ⏳ gpt-6-luna [Effort:low]",
                "→ read src/parser.rs:1-40",
                "First look.",
                "→ $ rg -n parse src",
                "**Tabs** break it.",
                "Line 2",
                "Line 3",
                "… (2 more lines, Ctrl+O to expand)",
                "3 turns ↑4.4k ↓950 R8.0k",
                "",
                "─── nope ✗",
                "Unknown agent: \"nope\". Available agents: \"scout\",",
                "\"critic\".",
                "",
                "Total: 3 turns ↑4.4k ↓950 R8.0k"
            ]
        );
        let expanded = shown(&parallel, true);
        assert_eq!(
            expanded[3..5],
            ["Task: Look at the parser", "and say what breaks."]
        );
        let unknown_at = expanded.iter().position(|row| row == "─── nope ✗").unwrap();
        assert_eq!(
            expanded[unknown_at + 1..unknown_at + 4],
            ["Task: Look at the parser", "and say what breaks.", ""]
        );
        let chain = Children {
            mode: Delegation::Chain,
            runs: vec![
                run("scout", ChildState::Done, found()),
                ChildRun {
                    agent: "critic".into(),
                    ..unknown
                },
            ],
        };
        let rows = shown(&chain, false);
        assert_eq!(
            rows[..3],
            [
                "✗ chain 1/2 steps",
                "",
                "─── Step 1: scout ✓ gpt-6-luna [Effort:low]"
            ]
        );
        assert_eq!(rows[12], "─── Step 2: critic ✗");
        let done = Children {
            mode: Delegation::Parallel,
            runs: vec![run("scout", ChildState::Done, Vec::new())],
        };
        assert_eq!(
            shown(&done, false)[..4],
            [
                "✓ parallel 1/1 tasks",
                "",
                "─── scout ✓ gpt-6-luna [Effort:low]",
                "(no output)"
            ]
        );
        assert_eq!(
            activity(&parallel, 30),
            ["⏳ scout **Tabs** break it.", "✗ nope"]
        );
    }

    #[test]
    fn rows_are_styled_and_marked_for_the_full_screen() {
        let single = Children {
            mode: Delegation::Single,
            runs: vec![run("scout", ChildState::Done, found())],
        };
        let look = Look {
            width: 12,
            color: true,
            marked: true,
            full: true,
            hint: String::new(),
        };
        let rows = rows(&single, &look);
        assert!(
            rows[0].starts_with("\x1b[32m✓\x1b[0m \x1b[1mscout\x1b[0m"),
            "{rows:?}"
        );
        // A wrapped row is marked as joined to the one before.
        assert!(
            rows.iter()
                .any(|row| row.contains(super::super::text::JOINED)),
            "{rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("\x1b[1mTabs\x1b[0m")),
            "{rows:?}"
        );
    }
}
