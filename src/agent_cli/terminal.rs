//! The inline renderer: the agent's output printed to the terminal as it streams, with a
//! status line under it, and the terminal modes of a turn.
use super::text::{FROM, Wrap, call_line, clip, printable, visible};
use super::{children, diff, editor, job, markdown, select, style, type_ahead, view};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    termios::{self, LocalModes, OptionalActions, SpecialCodeIndex, Termios},
};
use scramjet_rs::agent::{
    Agent, AgentError, AgentUi, Answer, AutoCompaction, Children, Choice, Compaction, Purpose,
    StreamProgress, ToolDetail,
};
use std::{
    io::{BufRead, IsTerminal},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Changes the terminal's input settings until dropped.
pub(super) struct InputMode(Option<Termios>);

impl InputMode {
    /// While a turn runs, typed keys are not echoed, and each is readable at once, so a lone
    /// Escape can be told from the start of a sequence. Ctrl+T becomes the quit character, so
    /// the kernel sends SIGQUIT for it, and `NOFLSH` keeps unread keys when it does.
    pub(super) fn turn() -> Self {
        Self::change(|mode| {
            mode.local_modes
                .remove(LocalModes::ECHO | LocalModes::ICANON);
            mode.local_modes.insert(LocalModes::NOFLSH);
            mode.special_codes[SpecialCodeIndex::VQUIT] = 0x14;
            mode.special_codes[SpecialCodeIndex::VMIN] = 1;
            mode.special_codes[SpecialCodeIndex::VTIME] = 0;
        })
    }
    /// An approval answer: an echoed line, edited by the terminal.
    fn answer() -> Self {
        Self::change(|mode| {
            mode.local_modes
                .insert(LocalModes::ECHO | LocalModes::ICANON)
        })
    }
    fn change(change: impl FnOnce(&mut Termios)) -> Self {
        let Ok(previous) = termios::tcgetattr(std::io::stdin()) else {
            return Self(None);
        };
        let mut changed = previous.clone();
        change(&mut changed);
        let _ = termios::tcsetattr(std::io::stdin(), OptionalActions::Now, &changed);
        Self(Some(previous))
    }
}

impl Drop for InputMode {
    fn drop(&mut self) {
        if let Some(previous) = &self.0 {
            let _ = termios::tcsetattr(std::io::stdin(), OptionalActions::Now, previous);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Streaming {
    None,
    Reasoning,
    Text,
}

/// The dim line under the output saying what the agent waits for and for how long, under the
/// last lines of a running command's output. A ticker thread redraws it while an activity is
/// set; other output first clears it.
#[derive(Default)]
pub(super) struct Status {
    activity: Option<(String, Instant)>,
    /// Shows the seconds left of this wait instead of the elapsed seconds.
    countdown: Option<Duration>,
    /// Follows the activity for a moment, such as the reply to Ctrl+T.
    note: Option<(&'static str, Instant)>,
    /// Shown above the line, each cut to fit one row.
    output: Vec<String>,
    /// The text on the screen, while it is shown, and the number of output rows above the line.
    drawn: Option<String>,
    rows_above: usize,
}

impl Status {
    fn line(&self) -> Option<String> {
        let note = match self.note {
            Some((note, at)) if at.elapsed() < NOTE_TIME => format!(" · {note}"),
            _ => String::new(),
        };
        Some(self.activity_line()? + &note)
    }
    /// The line without its note.
    pub(super) fn activity_line(&self) -> Option<String> {
        let (activity, since) = self.activity.as_ref()?;
        Some(match self.countdown {
            Some(wait) => {
                let left = wait.saturating_sub(since.elapsed()).as_secs_f64().ceil();
                format!("… {activity} {left}s")
            }
            None => format!("… {activity}{}", elapsed(since.elapsed().as_secs())),
        })
    }
    fn draw(&mut self) {
        let Some(text) = self.line() else {
            return;
        };
        // After a stop the shell has written below the drawn rows, which stay as they are.
        if job::continued() {
            self.drawn = None;
            self.rows_above = 0;
        }
        let width = editor::terminal_size().0.saturating_sub(1).max(10);
        let mut block: String = self
            .output
            .iter()
            .map(|line| format!("  {}{line}{}\n", style::STATUS, style::RESET))
            .collect();
        block += &format!("{}{}{}", style::STATUS, clip(&text, width), style::RESET);
        if self.drawn.as_deref() == Some(&block) {
            return;
        }
        view::print(false, &format!("{}{block}", self.erase()));
        self.rows_above = self.output.len();
        self.drawn = Some(block);
    }
    /// Moves to the first row drawn and clears from there.
    fn erase(&self) -> String {
        match self.rows_above {
            0 => "\r\x1b[J".into(),
            rows => format!("\r\x1b[{rows}A\x1b[J"),
        }
    }
    /// Removes the line, the output above it and the activity. Returns whether it was shown.
    fn clear(&mut self) -> bool {
        let drawn = self.drawn.take().is_some();
        if drawn && !job::continued() {
            view::print(false, &self.erase());
        }
        self.activity = None;
        self.countdown = None;
        self.output.clear();
        self.rows_above = 0;
        drawn
    }
}

const NOTE_TIME: Duration = Duration::from_secs(2);

/// How full the context is, as Scramjet's footer shows it: `53k/1.1M (5.0%)`.
fn context(tokens: u64, window: Option<u64>) -> String {
    let count = |n: u64| match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1e3),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1e3).round()),
        1_000_000..10_000_000 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{}M", (n as f64 / 1e6).round()),
    };
    match window {
        Some(window) if window > 0 => format!(
            "{}/{} ({:.1}%)",
            count(tokens),
            count(window),
            tokens as f64 * 100.0 / window as f64
        ),
        _ => count(tokens),
    }
}

pub(super) fn elapsed(seconds: u64) -> String {
    match seconds {
        0 => String::new(),
        1..60 => format!(" {seconds}s"),
        _ => format!(" {}m {:02}s", seconds / 60, seconds % 60),
    }
}

pub(super) struct Terminal {
    color: bool,
    pub(super) status: Arc<Mutex<Status>>,
    streaming: Streaming,
    /// Assistant text streamed during the current response, and how much of it the response's
    /// messages have matched so far.
    streamed: String,
    consumed: usize,
    /// Rows printed by the response's text while it is a single block, the latest output and on
    /// a terminal.
    text_rows: Option<usize>,
    /// Newlines ending the streamed block so far, not yet printed.
    held_newlines: usize,
    /// Newlines ending the streamed block that were printed early.
    printed_newlines: usize,
    markdown: markdown::Markdown,
    wrap: Wrap,
    tool_started: Option<Instant>,
    pub(super) hide_thinking: Arc<AtomicBool>,
    /// The reasoning streamed in the current block, and whether only its marker is shown.
    reasoning: String,
    reasoning_hidden: bool,
    /// Reads the keys typed during a turn on a terminal.
    pub(super) type_ahead: Option<type_ahead::TypeAhead>,
    /// Output goes here instead of the terminal, styled for this width, with every tool result
    /// in full, for the conversation view.
    capture: Option<(String, usize)>,
    /// Cut previews say that Ctrl+O shows them whole, as it does at the interactive prompt.
    pub(super) expandable: bool,
    /// Every tool result is shown whole, as the conversation view shows them.
    full: bool,
    /// The output marks gutters and wrapped lines for the full screen's copy (`text::FROM`).
    marked: bool,
}

impl Terminal {
    /// A tool's result block; `bash_lines` is how many of a command's last lines it shows.
    fn result(
        &mut self,
        name: &str,
        output: &str,
        detail: Option<&ToolDetail>,
        failed: bool,
        bash_lines: usize,
    ) {
        self.clear();
        let width = self.width().saturating_sub(2).max(20);
        let full = self.full;
        let took = self.tool_started.take().map(|started| started.elapsed());
        let style = match () {
            _ if !self.color => "",
            _ if failed => style::TOOL_FAILED,
            _ => style::TOOL_OUTPUT,
        };
        let reset = if style.is_empty() { "" } else { style::RESET };
        let styled = |lines: Vec<String>| -> Vec<String> {
            let row = |line| format!("{style}{line}{reset}");
            lines.into_iter().map(row).collect()
        };
        let shown = |limited: Shown| if full { Shown::All } else { limited };
        let hint = if self.expandable { EXPAND_HINT } else { "" };
        let marked = self.marked;
        let preview = |text: &str, shown: Shown, width| preview(text, shown, width, hint, marked);
        let rows = match (name, detail) {
            (_, Some(ToolDetail::Diff(diff))) => {
                let limit = if full { usize::MAX } else { diff::ROWS };
                diff::rows(diff, width, self.color, marked, limit, hint)
            }
            // As in Scramjet, trailing blank lines of a written file are not counted.
            (_, Some(ToolDetail::Written(content))) => styled(preview(
                content.trim_end_matches(['\n', '\r']),
                shown(Shown::First(WRITE_LINES)),
                width,
            )),
            // The user's own answer, not dimmed as tool output is.
            (_, Some(ToolDetail::Answer(answer))) => answer
                .lines()
                .flat_map(|line| {
                    let mut wrap = self.wrapper(width);
                    let rows = wrap.push(&printable(line)) + &wrap.finish();
                    rows.split('\n').map(str::to_owned).collect::<Vec<_>>()
                })
                .collect(),
            (_, Some(ToolDetail::Subagents(children))) => {
                children::rows(children, &self.look(width))
            }
            ("bash", None) => styled(preview(
                &bash_shown(output, failed),
                shown(Shown::Last(bash_lines)),
                width,
            )),
            _ => styled(preview(output, shown(Shown::First(RESULT_LINES)), width)),
        };
        let gutter = self.gutter();
        for row in rows {
            self.err(&format!("{gutter}{row}\n"));
        }
        if let Some(took) = took.filter(|took| name == "bash" && *took >= Duration::from_secs(1)) {
            let took = self.dim(&format!("took {:.1}s", took.as_secs_f64()));
            self.err(&format!("{gutter}{took}\n"));
        }
        // As in Scramjet, a blank line separates each tool block from what follows.
        self.err("\n");
    }
    /// Renders into a buffer `width` columns wide, styled, with every tool result in full.
    pub(super) fn capture(width: usize, hide_thinking: Arc<AtomicBool>) -> Self {
        let mut terminal = Self::new(hide_thinking, false);
        terminal.color = true;
        terminal.capture = Some((String::new(), width));
        terminal.full = true;
        terminal
    }
    /// As `capture`, with tool results cut as the inline output shows them unless `full`.
    pub(super) fn capture_shown(width: usize, hide_thinking: Arc<AtomicBool>, full: bool) -> Self {
        let mut terminal = Self::capture(width, hide_thinking);
        terminal.full = full;
        terminal.expandable = !full;
        terminal.marked = true;
        terminal
    }
    /// What was rendered into the buffer, after ending any streamed block.
    pub(super) fn captured(mut self) -> String {
        self.clear();
        self.capture
            .take()
            .map(|(text, _)| text)
            .unwrap_or_default()
    }
    /// Writes to stderr, or the capture buffer.
    fn err(&mut self, text: &str) {
        match &mut self.capture {
            Some((captured, _)) => captured.push_str(text),
            None => view::print(false, text),
        }
    }
    /// Writes to stdout, where replies go, or the capture buffer.
    fn out(&mut self, text: &str) {
        match &mut self.capture {
            Some((captured, _)) => captured.push_str(text),
            None => view::print(true, text),
        }
    }
    fn width(&self) -> usize {
        match self.capture {
            Some((_, width)) => width,
            None => editor::terminal_size().0,
        }
    }
    /// Whether replies are styled and wrapped: on a terminal, or when captured.
    fn styled_replies(&self) -> bool {
        self.capture.is_some() || std::io::stdout().is_terminal()
    }
    /// Whether the status line is drawn.
    fn live(&self) -> bool {
        self.color && self.capture.is_none()
    }
    /// With `status_thread`, on a terminal, a thread draws the status line.
    pub(super) fn new(hide_thinking: Arc<AtomicBool>, status_thread: bool) -> Self {
        let color = std::io::stderr().is_terminal();
        let status = Arc::new(Mutex::<Status>::default());
        if color && status_thread {
            let status = status.clone();
            let hide = hide_thinking.clone();
            let mut hidden = hide.load(Ordering::Relaxed);
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(200));
                    let mut status = status.lock().unwrap();
                    // Ctrl+T during a turn arrives as a signal; say at once what it did.
                    if hide.load(Ordering::Relaxed) != hidden {
                        hidden = !hidden;
                        status.note = Some((editor::thinking_note(hidden), Instant::now()));
                        status.draw();
                    }
                    // An activity set without drawing appears once it has lasted a second.
                    if status
                        .activity
                        .as_ref()
                        .is_some_and(|(_, since)| since.elapsed() >= Duration::from_secs(1))
                    {
                        status.draw();
                    }
                }
            });
        }
        Self {
            color,
            status,
            streaming: Streaming::None,
            streamed: String::new(),
            consumed: 0,
            text_rows: None,
            held_newlines: 0,
            printed_newlines: 0,
            markdown: markdown::Markdown::new(false, ""),
            wrap: Wrap::new(0),
            tool_started: None,
            hide_thinking,
            reasoning: String::new(),
            reasoning_hidden: false,
            type_ahead: None,
            capture: None,
            expandable: false,
            full: false,
            marked: false,
        }
    }
    /// Shows `activity` on the status line now, or only once it lasts a second.
    fn show(&mut self, activity: String, now: bool) {
        if self.live() {
            let mut status = self.status.lock().unwrap();
            status.activity = Some((activity, Instant::now()));
            status.countdown = None;
            if now {
                status.draw();
            }
        }
    }
    fn show_countdown(&mut self, activity: String, delay: Duration) {
        if self.live() {
            let mut status = self.status.lock().unwrap();
            status.activity = Some((activity, Instant::now()));
            status.countdown = Some(delay);
            status.draw();
        }
    }
    /// `text` broken between words to fit the terminal, for output to stderr.
    pub(super) fn wrapped(&self, text: &str) -> String {
        let mut wrap = self.wrapper(if self.color { self.width() } else { 0 });
        wrap.push(text) + &wrap.finish()
    }
    fn wrapper(&self, width: usize) -> Wrap {
        Wrap::new(width).marking(self.marked)
    }
    /// The indent of a block under its heading, such as a tool's result under its call.
    fn gutter(&self) -> String {
        match self.marked {
            true => format!("  {FROM}"),
            false => "  ".into(),
        }
    }
    pub(super) fn dim(&self, text: &str) -> String {
        self.styled(style::MUTED, text)
    }
    pub(super) fn styled(&self, style: &str, text: &str) -> String {
        if self.color {
            format!("{style}{text}{}", style::RESET)
        } else {
            text.to_owned()
        }
    }
    /// Continues or starts a streamed block. Trailing newlines wait for more of the block, so
    /// that ending it adds exactly one blank line.
    fn stream(&mut self, kind: Streaming, delta: &str) {
        if self.streaming == kind {
            // The status line's style reset ended the style of the streamed text.
            if self.status.lock().unwrap().clear() {
                let reopen = self.markdown.reopen();
                self.print_streamed(&reopen);
            }
        } else {
            self.clear();
            let styled = match kind {
                Streaming::Text => self.styled_replies(),
                _ => self.color,
            };
            let base = if kind == Streaming::Reasoning {
                style::REASONING
            } else {
                ""
            };
            self.markdown = markdown::Markdown::new(styled, base);
            self.wrap = self.wrapper(if styled { self.width() } else { 0 });
            self.streaming = kind;
            self.text_rows =
                (kind == Streaming::Text && styled && self.streamed.is_empty()).then_some(0);
            let reopen = self.markdown.reopen();
            self.print_streamed(&reopen);
        }
        let body = delta.trim_end_matches('\n');
        if !body.is_empty() {
            let text = "\n".repeat(std::mem::take(&mut self.held_newlines)) + body;
            let text = self.wrap.push(&self.markdown.push(&text));
            self.print_streamed(&text);
            self.printed_newlines = 0;
        }
        self.held_newlines += delta.len() - body.len();
        if self.held_newlines == 0 {
            return;
        }
        let mut text = self.wrap.push(&self.markdown.end_line());
        text += &self.wrap.end_word();
        // An ended reasoning paragraph is finished now, so that while the model thinks on, the
        // status line can show under it.
        let paragraph =
            kind == Streaming::Reasoning && self.held_newlines >= 2 && self.printed_newlines == 0;
        if paragraph {
            text += &self.wrap.push(&self.markdown.push("\n\n"));
            self.held_newlines -= 2;
            self.printed_newlines = 2;
        }
        self.print_streamed(&text);
        if paragraph {
            self.show("thinking".into(), false);
        }
    }
    /// Streams reasoning, or while Ctrl+T hides it, Scramjet's `Thinking...` line in its place.
    /// Shown again during the block, the block so far follows that line.
    fn reason(&mut self, delta: &str) {
        let started = self.streaming == Streaming::Reasoning;
        if !started {
            self.reasoning.clear();
        }
        self.reasoning.push_str(delta);
        let hide = self.hide_thinking.load(Ordering::Relaxed);
        match (hide, started && self.reasoning_hidden) {
            (false, false) => self.stream(Streaming::Reasoning, delta),
            (false, true) => {
                self.clear();
                let text = self.reasoning.clone();
                self.stream(Streaming::Reasoning, &text);
            }
            (true, false) => {
                self.clear();
                let marker = match self.color {
                    true => format!("{}Thinking...{}", style::THINKING_HIDDEN, style::RESET),
                    false => "Thinking...".into(),
                };
                self.err(&format!("{marker}\n\n"));
                self.streaming = Streaming::Reasoning;
                self.printed_newlines = 2;
                self.show("thinking".into(), true);
            }
            (true, true) => {}
        }
        self.reasoning_hidden = hide;
    }
    fn print_streamed(&mut self, text: &str) {
        if self.streaming == Streaming::Text {
            if let Some(rows) = &mut self.text_rows {
                *rows += text.matches('\n').count();
            }
            self.out(text);
        } else {
            self.err(text);
        }
    }
    /// Removes the status line and ends any streamed block before other output.
    pub(super) fn clear(&mut self) {
        self.status.lock().unwrap().clear();
        self.held_newlines = 0;
        let rest = self.wrap.push(&self.markdown.finish());
        let rest = rest + &self.wrap.finish();
        self.print_streamed(&rest);
        match self.streaming {
            Streaming::None => {}
            Streaming::Reasoning => {
                let reset = if self.color { style::RESET } else { "" };
                self.err(&format!(
                    "{reset}{}",
                    "\n".repeat(2 - self.printed_newlines)
                ));
            }
            Streaming::Text => {
                self.text_rows = self.text_rows.map(|rows| rows + 2);
                self.out("\n\n");
            }
        }
        self.printed_newlines = 0;
        self.streaming = Streaming::None;
    }
    pub(super) fn usage(&self, agent: &Agent) {
        eprintln!("{}", self.dim(&self.wrapped(&usage_line(agent))));
    }
}

/// The usage line after a turn: requests, tokens and how full the context is.
pub(super) fn usage_line(agent: &Agent) -> String {
    let usage = agent.usage;
    format!(
        "[{} requests · {} input tokens ({} cached) · {} output tokens{}]",
        usage.requests,
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
        usage
            .context_tokens
            .map(|tokens| format!(" · context {}", context(tokens, agent.context_window())))
            .unwrap_or_default()
    )
}

impl AgentUi for Terminal {
    fn waiting(&mut self, purpose: Purpose) {
        self.clear();
        self.streamed.clear();
        self.consumed = 0;
        self.text_rows = None;
        let activity = match purpose {
            Purpose::Reply => "thinking",
            Purpose::Compaction => "compacting",
            Purpose::BranchSummary => "summarizing branch",
        };
        self.show(activity.into(), true);
    }
    fn progress(&mut self, update: &StreamProgress) {
        match update {
            StreamProgress::Reasoning(delta) => self.reason(delta),
            StreamProgress::Text(delta) => {
                self.stream(Streaming::Text, delta);
                self.streamed.push_str(delta);
            }
            StreamProgress::Call(name) => {
                self.clear();
                self.show(format!("preparing {name}"), true);
            }
        }
    }
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration) {
        self.clear();
        let text = format!(
            "{error}\nretrying in {}s ({attempt}/{retries}; Ctrl-C cancels)",
            delay.as_secs_f64().ceil()
        );
        let text = self.styled(style::WARNING, &text);
        self.err(&format!("{text}\n"));
        self.show_countdown("retrying in".into(), delay);
    }
    fn assistant(&mut self, text: &str) {
        self.clear();
        let shown = &self.streamed[self.consumed..];
        if shown.starts_with(text) {
            self.consumed += text.len();
            return;
        }
        let mut text = text.to_owned();
        if !shown.is_empty() {
            // The deltas missed part of the final text (Proxy's DeepSeek route drops a reply's first
            // one). As Scramjet replaces the streamed block with the final text, rewrite it while
            // it is still on screen; otherwise show the final text after it.
            let height = editor::terminal_size().1;
            let captured = self.capture.is_some();
            let rows = self
                .text_rows
                .filter(|&rows| rows > 0 && (captured || rows < height));
            if let Some(rows) = rows {
                self.rewind(rows);
                text = self.streamed[..self.consumed].to_owned() + &text;
            }
            self.streamed.clear();
            self.consumed = 0;
        }
        self.text_rows = None;
        if !text.is_empty() {
            let width = match self.styled_replies() {
                true => self.width(),
                false => 0,
            };
            let mut markdown = markdown::Markdown::new(width > 0, "");
            let text = markdown.push(text.trim_end()) + &markdown.finish();
            let mut wrap = self.wrapper(width);
            let text = wrap.push(&text) + &wrap.finish();
            self.out(&format!("{text}\n\n"));
        }
    }
    fn tool_call(&mut self, name: &str, summary: &str) {
        self.clear();
        let width = if self.color { self.width() } else { 0 };
        let line = self.dim(&call_line(name, summary, width, self.marked));
        self.err(&format!("{line}\n"));
    }
    fn tool_running(&mut self) {
        self.tool_started = Some(Instant::now());
        self.show("running".into(), false);
    }
    fn tool_output(&mut self, output: &str) {
        if !self.live() {
            return;
        }
        // Two columns of indent, and one spare so that no line reaches the edge and wraps.
        let width = editor::terminal_size().0.saturating_sub(3).max(20);
        let mut status = self.status.lock().unwrap();
        let mut lines: Vec<String> = output
            .lines()
            .rev()
            .take(BASH_LINES)
            .map(|line| clip(&printable(line), width))
            .collect();
        lines.reverse();
        status.output = lines;
        // Like the line itself, the output appears once the command has run for a second.
        if status.drawn.is_some() {
            status.draw();
        }
    }
    /// Above the status line, a line per child.
    fn subagents(&mut self, shown: &Children) {
        if !self.live() {
            return;
        }
        let width = editor::terminal_size().0.saturating_sub(3).max(20);
        let mut status = self.status.lock().unwrap();
        status.output = children::activity(shown, width);
        if status.drawn.is_some() {
            status.draw();
        }
    }
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        self.result(name, output, detail, failed, BASH_LINES);
    }
    fn command_result(&mut self, output: &str, failed: bool) {
        self.result("bash", output, None, failed, COMMAND_LINES);
    }
    /// As Scramjet shows a compaction: the estimated context size before it, then the summary;
    /// an automatic one also says why it ran.
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>) {
        self.clear();
        let heading = format!(
            "✓ Compacted: the context was about {} tokens; kept the last {} items",
            context(compaction.tokens_before, None),
            compaction.kept_items,
        );
        let reason = automatic
            .map(|reason| format!("\n{}", self.dim(&self.wrapped(reason.reason()))))
            .unwrap_or_default();
        let block = format!(
            "{}{reason}\n\n{}\n",
            self.wrapped(&heading),
            self.summary_block(&compaction.summary)
        );
        self.err(&block);
        // During a turn, as after a tool block; after `/compact` the prompt's blank line follows.
        if automatic.is_some() {
            self.err("\n");
        }
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        self.compaction_error(&error.to_string());
    }
    fn user_prompt(&mut self, prompt: &str) {
        self.prompt_rows(prompt);
        self.err("\n");
    }
    /// As typed, and what it sent once results are shown whole.
    fn command_prompt(&mut self, typed: &str, sent: &str) {
        self.prompt_rows(typed);
        let body = command_body(sent);
        if self.full {
            let block = self.summary_block(body);
            self.err(&format!("\n{block}\n"));
        } else if self.expandable {
            let lines = body.lines().count();
            let hint = self.dim(&format!("… (sent as {lines} lines{EXPAND_HINT})"));
            self.err(&format!("{hint}\n"));
        }
        self.err("\n");
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.clear();
        let block = format!(
            "{}\n\n{}\n\n",
            self.wrapped("✓ Compacted: the earlier conversation is summarized"),
            self.summary_block(summary)
        );
        self.err(&block);
    }
    fn branch_summary(&mut self, summary: &str) {
        self.clear();
        let block = format!(
            "{}\n\n{}\n\n",
            self.wrapped("✓ Summarized the branch this conversation came back from"),
            self.summary_block(summary)
        );
        self.err(&block);
    }
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer {
        self.without_type_ahead(|ui| ui.ask(question, cancel))
    }
    /// A block of its own, ending with a blank line as a tool block does.
    fn notice(&mut self, text: &str) {
        self.clear();
        let text = self.wrapped(&visible(text));
        let text = self.styled(style::NOTICE, &text);
        self.err(&format!("{text}\n\n"));
    }
    /// As Scramjet shows the message in the tool's row: markdown, under the call line.
    fn question(&mut self, message: &str) {
        self.clear();
        let width = match self.color {
            true => self.width().saturating_sub(2).max(20),
            false => 0,
        };
        let mut markdown = markdown::Markdown::new(self.color, "");
        let text = markdown.push(visible(message).trim_end()) + &markdown.finish();
        let mut wrap = self.wrapper(width);
        let gutter = self.gutter();
        for line in (wrap.push(&text) + &wrap.finish()).lines() {
            match line.is_empty() {
                true => self.err("\n"),
                false => self.err(&format!("{gutter}{line}\n")),
            }
        }
        self.err("\n");
    }
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        self.clear();
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return numbered_choice(title, options, recommended);
        }
        self.without_type_ahead(|_| {
            let _ = termios::tcflush(&stdin, termios::QueueSelector::IFlush);
            select::question(title, options, recommended).unwrap_or(None)
        })
    }
}

/// A question's options for piped input: numbered, answered by a line with a number, or an empty
/// one for the recommended (else the first) option; anything else cancels.
fn numbered_choice(title: &str, options: &[Choice], recommended: Option<usize>) -> Option<usize> {
    let initial = recommended.unwrap_or(0);
    let mut text = format!("{title}\n");
    for (index, option) in options.iter().enumerate() {
        text += &format!("  {}. {}", index + 1, visible(&option.label));
        if let Some(description) = &option.description {
            text += &format!(" — {}", visible(description));
        }
        if recommended == Some(index) {
            text += " [recommended]";
        }
        text += "\n";
    }
    eprint!(
        "{text}Choice [1-{}, Enter = {}]: ",
        options.len(),
        initial + 1
    );
    let mut typed = String::new();
    let read = std::io::stdin().lock().read_line(&mut typed).unwrap_or(0);
    // Piped input is not echoed.
    eprintln!();
    if read == 0 {
        return None;
    }
    match typed.trim() {
        "" => Some(initial),
        typed => typed
            .parse::<usize>()
            .ok()
            .filter(|number| (1..=options.len()).contains(number))
            .map(|number| number - 1),
    }
}

impl Terminal {
    /// Runs `question` with the keys typed during the turn so far kept for the next prompt, so
    /// that none of them answers it.
    fn without_type_ahead<T>(&mut self, question: impl FnOnce(&mut Self) -> T) -> T {
        let reading = self
            .type_ahead
            .take()
            .map(|typed| (typed.cancel(), typed.expand(), typed.finish()));
        let answer = question(self);
        self.type_ahead = reading
            .map(|(cancel, expand, typed)| type_ahead::TypeAhead::resume(cancel, expand, typed));
        answer
    }
    /// A message of the session's own, wrapped, as a block ending with a blank line.
    pub(super) fn said(&mut self, text: &str) {
        self.clear();
        let text = self.wrapped(text);
        self.err(&format!("{text}\n\n"));
    }
    pub(super) fn compaction_error(&mut self, error: &str) {
        self.clear();
        let text = self.wrapped(&format!("error: automatic compaction failed: {error}"));
        self.err(&format!("{text}\n\n"));
    }
    /// An approval question as it stays on screen once answered.
    pub(super) fn approval(&mut self, question: &str, answer: Answer) {
        self.clear();
        let typed = match answer {
            Answer::Allow => "y",
            Answer::AllowAll => "a",
            Answer::Deny => "n",
        };
        self.err(&format!("{}\n{ALLOW}{typed}\n", visible(question)));
        if answer == Answer::AllowAll {
            let note = self.dim(ALLOWED_ALL);
            self.err(&format!("{note}\n"));
        }
    }
    /// The output of the running tool so far, as the status line shows its end; all of it
    /// when the results are shown whole.
    /// The rows of a running `subagent` call's children, as its result shows them.
    pub(super) fn running_subagents(&mut self, shown: &Children) {
        let width = self.width().saturating_sub(2).max(20);
        let gutter = self.gutter();
        for row in children::rows(shown, &self.look(width)) {
            self.err(&format!("{gutter}{row}\n"));
        }
    }
    /// How a tool block's rows `width` columns wide are laid out.
    fn look(&self, width: usize) -> children::Look {
        children::Look {
            width,
            color: self.color,
            marked: self.marked,
            full: self.full,
            hint: if self.expandable { EXPAND_HINT } else { "" },
        }
    }
    pub(super) fn running_output(&mut self, output: &str) {
        let width = self.width().saturating_sub(2).max(20);
        let shown = match self.full {
            true => Shown::All,
            false => Shown::Last(BASH_LINES),
        };
        let gutter = self.gutter();
        for row in preview(output.trim_end_matches('\n'), shown, width, "", self.marked) {
            let row = self.styled(style::STATUS, &row);
            self.err(&format!("{gutter}{row}\n"));
        }
    }
    /// Removes the last `rows` rows of output, the cursor at the start of the row after them.
    fn rewind(&mut self, rows: usize) {
        match &mut self.capture {
            Some((captured, _)) => {
                let keep = captured
                    .rmatch_indices('\n')
                    .nth(rows)
                    .map_or(0, |(index, _)| index + 1);
                captured.truncate(keep);
            }
            None => self.out(&format!("\r\x1b[{rows}A\x1b[J")),
        }
    }
    /// A compaction's summary as Scramjet shows it: dim markdown, wrapped to the terminal.
    /// Its first lines while tool results are cut, as Scramjet previews a compaction's.
    fn summary_block(&self, summary: &str) -> String {
        let mut markdown = markdown::Markdown::new(self.color, style::SUMMARY);
        let text = markdown.reopen() + &markdown.push(summary.trim_end()) + &markdown.finish();
        let reset = if self.color { style::RESET } else { "" };
        let text = self.wrapped(&text);
        let rows: Vec<&str> = text.split('\n').collect();
        if self.full || !self.expandable || rows.len() <= SUMMARY_LINES {
            return format!("{text}{reset}");
        }
        let more = rows.len() - SUMMARY_LINES;
        let hint = format!("… ({more} more lines, {} total{EXPAND_HINT})", rows.len());
        let kept = rows[..SUMMARY_LINES].join("\n");
        format!("{kept}{reset}\n{}", self.dim(&hint))
    }
    fn prompt_rows(&mut self, prompt: &str) {
        self.clear();
        // As the editor left it on screen; output to a pipe is not wrapped.
        let width = match self.color {
            true => self.width(),
            false => usize::MAX,
        };
        for row in editor::prompt_rows(prompt, width, self.marked) {
            self.err(&format!("{row}\n"));
        }
    }
    fn ask(&mut self, question: &str, cancel: &AtomicBool) -> Answer {
        self.clear();
        let question = visible(question);
        let stdin = std::io::stdin();
        let ask = format!("{question}\n{ALLOW}");
        let typed = if !stdin.is_terminal() {
            eprint!("{ask}");
            let mut typed = String::new();
            if stdin.lock().read_line(&mut typed).is_err() {
                return Answer::Deny;
            }
            typed
        } else {
            let _ = termios::tcflush(&stdin, termios::QueueSelector::IFlush);
            eprint!("{ask}");
            let _answer = InputMode::answer();
            // SIGINT does not interrupt a blocking read, so wait in poll and watch for Ctrl-C.
            loop {
                if cancel.load(Ordering::Relaxed) {
                    eprintln!();
                    return Answer::Deny;
                }
                let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
                match poll(
                    &mut fds,
                    Some(&Timespec {
                        tv_sec: 0,
                        tv_nsec: 50_000_000,
                    }),
                ) {
                    Ok(0) | Err(Errno::INTR) => {}
                    Ok(_) => break,
                    Err(_) => return Answer::Deny,
                }
            }
            // In canonical mode one read returns one typed line.
            let mut typed = [0; 4096];
            match rustix::io::read(&stdin, &mut typed) {
                Ok(n) => String::from_utf8_lossy(&typed[..n]).into_owned(),
                Err(_) => return Answer::Deny,
            }
        };
        let answer = answer(&typed);
        if answer == Answer::AllowAll {
            eprintln!("{}", self.dim(ALLOWED_ALL));
        }
        answer
    }
}

const ALLOW: &str = "Allow? [y/N, a = allow all until exit] ";
const ALLOWED_ALL: &str = "Tool calls now run without asking until exit.";

fn answer(typed: &str) -> Answer {
    match typed.trim() {
        "y" | "Y" | "yes" => Answer::Allow,
        "a" | "A" | "all" => Answer::AllowAll,
        _ => Answer::Deny,
    }
}

/// Scramjet shows the end of bash output, where errors and the exit code are.
const BASH_LINES: usize = 5;
/// As Scramjet shows the output of a command the user ran with `!`.
const COMMAND_LINES: usize = 20;
const WRITE_LINES: usize = 10;
const RESULT_LINES: usize = 6;
const SUMMARY_LINES: usize = 10;
/// Ends the note about the lines a preview leaves out, as Scramjet's does.
const EXPAND_HINT: &str = ", Ctrl+O to expand";

enum Shown {
    First(usize),
    Last(usize),
    /// Every line, wrapped rather than cut.
    All,
}

/// What a command sent, without the `<scramjet-command>` tags around it.
fn command_body(sent: &str) -> &str {
    let body = sent
        .strip_prefix("<scramjet-command")
        .and_then(|rest| rest.split_once(">\n"))
        .map_or(sent, |(_, body)| body)
        .trim_end();
    body.strip_suffix("</scramjet-command>")
        .unwrap_or(body)
        .trim_end()
}

/// A failed command's ending follows its output after a blank line, which is not worth one of
/// the few lines shown.
fn bash_shown(output: &str, failed: bool) -> String {
    match output.rsplit_once("\n\n") {
        Some((body, ending)) if failed => format!("{body}\n{ending}"),
        _ => output.to_owned(),
    }
}

/// The lines of a tool result to show, made safe for the terminal and cut to `width` cells,
/// with a note for lines left out that ends with `hint`; lines wrapped are `marked` for a copy.
fn preview(text: &str, shown: Shown, width: usize, hint: &str, marked: bool) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let fit = |line: &&str| clip(&printable(line), width);
    match shown {
        Shown::First(n) if lines.len() > n => {
            let mut kept: Vec<String> = lines[..n].iter().map(fit).collect();
            kept.push(format!(
                "… ({} more lines, {} total{hint})",
                lines.len() - n,
                lines.len()
            ));
            kept
        }
        Shown::Last(n) if lines.len() > n => {
            let mut kept = vec![format!("… ({} earlier lines{hint})", lines.len() - n)];
            kept.extend(lines[lines.len() - n..].iter().map(fit));
            kept
        }
        Shown::All => lines
            .iter()
            .flat_map(|line| {
                let mut wrap = Wrap::new(width).marking(marked);
                let rows = wrap.push(&printable(line)) + &wrap.finish();
                rows.split('\n').map(str::to_owned).collect::<Vec<_>>()
            })
            .collect(),
        _ => lines.iter().map(fit).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::text::Escape;

    #[test]
    fn previews_keep_the_end_of_bash_output_and_are_safe_to_print() {
        let output: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let failed = format!("{output}\nCommand exited with code 1");
        assert_eq!(
            preview(
                &bash_shown(&failed, true),
                Shown::Last(5),
                80,
                EXPAND_HINT,
                false
            ),
            [
                "… (16 earlier lines, Ctrl+O to expand)",
                "line 17",
                "line 18",
                "line 19",
                "line 20",
                "Command exited with code 1"
            ]
        );
        assert_eq!(
            bash_shown("started\n\nCommand timed out after 5s", true),
            "started\nCommand timed out after 5s"
        );
        assert_eq!(
            bash_shown("Command exited with code 1", true),
            "Command exited with code 1"
        );
        assert_eq!(bash_shown("a\n\nb", false), "a\n\nb");
        let head = preview(&output, Shown::First(2), 80, "", false);
        assert_eq!(head, ["line 1", "line 2", "… (18 more lines, 20 total)"]);
        assert_eq!(
            printable("\x1b[31merror\x1b[0m:\tx\x1b]0;title\x07\x08 10%\r100%"),
            "100%"
        );
        assert_eq!(
            printable("\x1b[1m+\x1b(B\x1b[m    )\x1b#8\x1b7."),
            "+    )."
        );
        assert_eq!(
            printable("\x1b[1;31merror\x1b[0m:\tbad\x07"),
            "error:    bad"
        );
        assert_eq!(clip("abcdefgh", 5), "abcd…");
        assert_eq!(clip("日本語です", 6), "日本…");
        assert_eq!(clip("short", 5), "short");
    }

    #[test]
    fn the_captured_conversation_shows_every_tool_result_in_full() {
        let mut ui = Terminal::capture(30, Arc::new(AtomicBool::new(false)));
        ui.user_prompt("count");
        ui.progress(&StreamProgress::Reasoning("Counting **now**.\n\n".into()));
        ui.tool_call("bash", "seq 1 12");
        let output: String = (1..=12).map(|n| format!("{n}\n")).collect();
        ui.tool_result("bash", &output, None, false);
        ui.tool_call("read", "a.txt");
        ui.tool_result("read", &"word ".repeat(10), None, false);
        let diff: String = (1..=50).map(|n| format!("+{n:>2} line {n}\n")).collect();
        ui.tool_result("edit", "ok", Some(&ToolDetail::Diff(diff)), false);
        ui.assistant("Counted to `12`.");
        let text = ui.captured();
        let plain = |text: &str| {
            let mut plain = String::new();
            let mut escape = Escape::None;
            plain.extend(text.chars().filter(|&c| !escape.step(c)));
            plain
        };
        let lines: Vec<String> = text.lines().map(plain).collect();
        let at = |line: &str| lines.iter().position(|shown| shown == line);
        let first = at("  1").expect("the first output line");
        let numbers: Vec<_> = (1..=12).map(|n| format!("  {n}")).collect();
        assert_eq!(lines[first..first + 12], numbers);
        // A long line wraps instead of being cut.
        let read = at("● read a.txt").unwrap();
        assert_eq!(lines[read + 1], "  word word word word word");
        assert_eq!(lines[read + 2], "  word word word word word");
        assert!(at("  +50 line 50").is_some(), "{lines:?}");
        assert!(!text.contains("more lines") && !text.contains("earlier lines"));
        assert!(at("Counted to 12.").is_some(), "{lines:?}");
        assert!(text.contains(style::REASONING));
    }

    #[test]
    fn summaries_and_command_texts_are_cut_until_expanded() {
        let summary: String = (1..=12).map(|n| format!("Point {n}\n")).collect();
        let sent = "<scramjet-command name=\"rs:loop\">\n# Build\n\nGo.\n</scramjet-command>";
        let shown = |full| {
            let mut ui = Terminal::capture_shown(30, Arc::new(AtomicBool::new(false)), full);
            ui.command_prompt("/rs:loop", sent);
            ui.compaction_summary(&summary);
            ui.branch_summary("Short");
            let text = ui.captured();
            let mut plain = String::new();
            let mut escape = Escape::None;
            plain.extend(text.chars().filter(|&c| !escape.step(c) && c != FROM));
            plain
        };
        let cut = shown(false);
        let points: String = (1..=10).map(|n| format!("Point {n}\n")).collect();
        assert!(cut.starts_with("> /rs:loop\n… (sent as 3 lines, Ctrl+O to expand)\n\n"));
        assert!(cut.contains(&format!(
            "{points}… (2 more lines, 12 total, Ctrl+O to expand)\n"
        )));
        assert!(cut.ends_with("\n\nShort\n\n"), "{cut:?}");
        let whole = shown(true);
        assert!(
            whole.starts_with("> /rs:loop\n\nBuild\n\nGo.\n\n"),
            "{whole:?}"
        );
        assert!(whole.contains(&summary) && !whole.contains("Ctrl+O"));
    }

    #[test]
    fn a_captured_reply_missing_deltas_is_replaced_by_its_final_text() {
        let mut ui = Terminal::capture(30, Arc::new(AtomicBool::new(false)));
        ui.user_prompt("go");
        ui.waiting(Purpose::Reply);
        ui.progress(&StreamProgress::Text(" of the reply that\nwraps.".into()));
        ui.assistant("The start of the reply that\nwraps.");
        assert_eq!(
            ui.captured(),
            "> go\n\nThe start of the reply that\nwraps.\n\n"
        );
    }

    #[test]
    fn approval_text_shows_hidden_characters() {
        assert_eq!(
            visible("rm -rf ~\r\x1b[2Kecho é\t\u{202e}txt.sh\nls"),
            "rm -rf ~\\r\\u{1b}[2Kecho é\t\\u{202e}txt.sh\nls"
        );
        let answers: Vec<_> = ["y\n", "yes", "a\n", "all", "", "n", "ya"]
            .map(answer)
            .into();
        use Answer::*;
        assert_eq!(
            answers,
            [Allow, Allow, AllowAll, AllowAll, Deny, Deny, Deny]
        );
    }
    #[test]
    fn context_is_shown_as_scramjet_formats_it() {
        assert_eq!(context(52_500, Some(1_050_000)), "53k/1.1M (5.0%)");
        assert_eq!(context(999, Some(2_000)), "999/2.0k (50.0%)");
        assert_eq!(context(9_950, Some(200_000)), "9.9k/200k (5.0%)");
        assert_eq!(context(12_000_000, Some(20_000_000)), "12M/20M (60.0%)");
        assert_eq!(context(53_000, None), "53k");
    }
    #[test]
    fn status_shows_whole_seconds_then_minutes() {
        let shown: Vec<_> = [0, 1, 59, 60, 185].map(elapsed).into();
        assert_eq!(shown, ["", " 1s", " 59s", " 1m 00s", " 3m 05s"]);
        let started = Instant::now() - Duration::from_millis(1500);
        let mut status = Status {
            activity: Some(("retrying in".into(), started)),
            countdown: Some(Duration::from_secs(4)),
            ..Status::default()
        };
        assert_eq!(status.line().unwrap(), "… retrying in 3s");
        status.countdown = Some(Duration::from_secs(1));
        assert_eq!(status.line().unwrap(), "… retrying in 0s");
        status.countdown = None;
        status.note = Some(("Thinking blocks: hidden", Instant::now()));
        assert_eq!(
            status.line().unwrap(),
            "… retrying in 1s · Thinking blocks: hidden"
        );
        status.note = Some(("Thinking blocks: hidden", started - NOTE_TIME));
        assert_eq!(status.line().unwrap(), "… retrying in 1s");
    }
}
