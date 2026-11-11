//! The agent's output as text: printed as it streams for `-p` and piped input, with a status
//! line under it on a terminal, or captured for the full screen to show; and the terminal modes
//! of a `-p` turn.
use super::banner::{self, Banner, Opening};
use super::design::{Design, Gutter, Outcome};
use super::editor::NOTE_TIME;
use super::highlight::{self, Highlighter};
use super::interactive::thinking;
use super::keymap::{self, Binding};
use super::settings::MarkdownReads;
use super::style::{self, Band, Style};
use super::text::{FROM, RULE, Wrap, call_line, clip, printable, style_at, visible};
use super::{children, console, diff, editor, job, markdown, record};
use scramjet::agent::{
    Agent, AgentError, AgentUi, Answer, AutoCompaction, Children, Choice, CompactRead, Compaction,
    DELEGATE, DELEGATE_SCOPE_WARNING, Ending, PUBLICATION_TOOLS, Purpose, READ_MORE,
    StreamProgress, ToolDetail, UserCommand, publication_choices, read_line_too_long,
};
use std::{
    borrow::Cow,
    collections::VecDeque,
    io::{BufRead, IsTerminal, Write},
    sync::{
        Arc, LazyLock, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// The status line of the terminal's output, for an exit to remove.
static STATUS: Mutex<Option<Arc<Mutex<Status>>>> = Mutex::new(None);

/// Removes the status line, if drawn, before an exit ends the process, and keeps it from being
/// drawn again, which would leave it under the shell's prompt.
pub(super) fn clear_on_exit() {
    let status = STATUS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(status) = status {
        let mut status = status.lock().unwrap_or_else(PoisonError::into_inner);
        status.clear();
        std::mem::forget(status);
    }
}

/// Scramjet's spinner, a frame each `SPIN`.
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const SPIN: Duration = Duration::from_millis(80);
/// Counted from one start, so that the spinner turns on evenly as the activity changes.
static SPUN: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The spinner's frame now, in `color`.
pub(super) fn spinner(color: Style) -> String {
    format!("{color}{}{}", frame(SPUN.elapsed()), style::RESET)
}

/// How long until the spinner's next frame.
pub(super) fn next_spin() -> Duration {
    let spun = SPUN.elapsed();
    SPIN.saturating_sub(Duration::from_millis(
        (spun.as_millis() % SPIN.as_millis()) as u64,
    ))
}

/// The spinner's frame once it has turned for `spun`.
fn frame(spun: Duration) -> char {
    SPINNER[(spun.as_millis() / SPIN.as_millis()) as usize % SPINNER.len()]
}

/// Changes the terminal's input settings until dropped.
pub(super) struct InputMode(Option<console::Mode>);

impl InputMode {
    /// As `console::Mode::turn` says.
    pub(super) fn turn() -> Self {
        Self::change(console::Mode::turn)
    }
    /// An approval answer: an echoed line, edited by the terminal.
    fn answer() -> Self {
        Self::change(console::Mode::answer)
    }
    fn change(change: impl FnOnce(&console::Mode) -> console::Mode) -> Self {
        let Some(previous) = console::Mode::get() else {
            return Self(None);
        };
        let _ = change(&previous).set_in_foreground();
        Self(Some(previous))
    }
}

impl Drop for InputMode {
    fn drop(&mut self) {
        if let Some(previous) = &self.0 {
            let _ = previous.set_in_foreground();
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
    fn activity_line(&self) -> Option<String> {
        let (activity, since) = self.activity.as_ref()?;
        Some(match self.countdown {
            Some(wait) => {
                let left = wait.saturating_sub(since.elapsed()).as_secs_f64().ceil();
                format!("{activity} {left}s")
            }
            None => format!("{activity}{}", elapsed(since.elapsed().as_secs())),
        })
    }
    fn draw(&mut self) {
        let Some(text) = self.line() else {
            return;
        };
        // Drawn in place, it would overwrite the shell's prompt; redrawn at `fg`.
        if !console::foreground() {
            return;
        }
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
        let color = match self.countdown {
            Some(_) => style::SPINNER_RETRYING,
            None => style::SPINNER,
        };
        let text = clip(&text, width.saturating_sub(2));
        block += &format!("{} {}{text}{}", spinner(color), style::STATUS, style::RESET);
        if self.drawn.as_deref() == Some(&block) {
            return;
        }
        print(false, &format!("{}{block}", self.erase()));
        self.rows_above = self.output.len();
        self.drawn = Some(block);
    }
    /// Clears the rows drawn, from the last up to the first, where it leaves the cursor. Row by
    /// row: tmux keeps a screen cleared to its end from the top left in its history, which on
    /// the top row would fill it with the spinner's frames.
    fn erase(&self) -> String {
        "\r\x1b[2K".to_owned() + &"\x1b[A\x1b[2K".repeat(self.rows_above)
    }
    /// Removes the line, the output above it and the activity. Returns whether it was shown.
    fn clear(&mut self) -> bool {
        let drawn = self.drawn.take().is_some();
        if drawn && !job::continued() {
            print(false, &self.erase());
        }
        self.activity = None;
        self.countdown = None;
        self.output.clear();
        self.rows_above = 0;
        drawn
    }
}

/// A count of tokens, as Scramjet's footer shows it: `950`, `3.4k`, `53k`, `1.1M`.
pub(super) fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{:.1}k", n as f64 / 1e3),
        10_000..1_000_000 => format!("{}k", (n as f64 / 1e3).round()),
        1_000_000..10_000_000 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{}M", (n as f64 / 1e6).round()),
    }
}

/// An output rate in tokens a second, as Scramjet shows it: `52tok/s`.
pub(super) fn rate(rate: f64) -> String {
    format!("{}tok/s", rate.round())
}

/// What was spent, as Scramjet shows it (`$0.123` with 3 `decimals`), with `+?` after it when
/// some of it is not known; `None` when nothing priced was spent.
pub(super) fn dollars(cost: f64, unpriced: bool, decimals: usize) -> Option<String> {
    match (cost > 0.0, unpriced) {
        (true, false) => Some(format!("${cost:.decimals$}")),
        (true, true) => Some(format!("${cost:.decimals$}+?")),
        (false, _) => None,
    }
}

/// What was spent, in words, as `/session` and `/export` say it (`$0.1234`, `$0.1234 + unknown`
/// when some of it is not known, `unknown` when none is); `None` when nothing was spent.
pub(super) fn spent(cost: f64, unpriced: bool) -> Option<String> {
    match (cost > 0.0, unpriced) {
        (true, false) => Some(format!("${cost:.4}")),
        (true, true) => Some(format!("${cost:.4} + unknown")),
        (false, true) => Some("unknown".to_owned()),
        (false, false) => None,
    }
}

/// How full the context is, as Scramjet's footer shows it: `53k/1.1M (5.0%)`.
pub(super) fn context(count: u64, window: Option<u64>) -> String {
    match window {
        Some(window) if window > 0 => format!(
            "{}/{} ({:.1}%)",
            tokens(count),
            tokens(window),
            count as f64 * 100.0 / window as f64
        ),
        _ => tokens(count),
    }
}

/// What the status line says while a request for `purpose` runs, before anything of it arrives.
pub(super) fn waiting_for(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::Reply => "Waiting for reply",
        Purpose::Compaction(None) => "Compacting context",
        Purpose::Compaction(Some(AutoCompaction::Threshold)) => "Auto-compacting",
        Purpose::Compaction(Some(AutoCompaction::Overflow)) => "Context overflow, compacting",
        Purpose::BranchSummary => "Summarizing branch",
    }
}

/// What the status line says while a request waits to be sent again, before the seconds left.
pub(super) fn retrying(attempt: u32, retries: u32) -> String {
    format!("Retrying ({attempt}/{retries}) in")
}

/// What the status line says while a request waits out the provider's Retry-After, before the
/// seconds left.
pub(super) const COOLING: &str = "Provider asked to wait, sending in";

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
    /// The call shown last is a compact read, whose result shows only if it failed.
    compact: bool,
    /// The command the user runs with `!` (true) or `!!`, while it runs.
    command: Option<(String, bool)>,
    /// How many of the last lines of a command the user ran its block shows.
    command_lines: usize,
    pub(super) hide_thinking: Arc<AtomicBool>,
    pub(super) markdown_reads: MarkdownReads,
    /// The reasoning streamed in the current block, and whether only its marker is shown.
    reasoning: String,
    reasoning_hidden: bool,
    /// Output goes here instead of the terminal, styled for this width, for the full screen.
    capture: Option<(String, usize)>,
    /// Cut previews say that Ctrl+O shows them whole, as it does on the full screen.
    expandable: bool,
    /// Every tool result is shown whole, as Ctrl+O shows them on the full screen.
    full: bool,
    /// The output marks gutters and wrapped lines for the full screen's copy (`text::FROM`).
    marked: bool,
    /// How prompts, reasoning, calls and replies look; the classic look but on the full screen.
    pub(super) design: Design,
    /// The banner a session starts with (`banner`).
    pub(super) banner: Banner,
    /// How each call still to be shown went, in turn, and how long it ran (`record::outcomes`),
    /// for a look whose headings say it.
    pub(super) outcomes: VecDeque<(Outcome, Option<Duration>)>,
    /// How the call shown last went.
    outcome: Outcome,
    /// A question ended with a pad, which the call's rows after it take as their own.
    padded_above: bool,
    /// What goes before each row of the block streamed now.
    lead: Option<Lead>,
}

/// Puts a gutter before each row of streamed text that has text in it, keeping its styles on
/// after the gutter.
struct Lead {
    gutter: Gutter,
    /// The first row had its gutter.
    started: bool,
    /// At the start of a row, before its text.
    at_start: bool,
    /// The style sequences on since the last reset.
    active: String,
}

impl Lead {
    fn new(gutter: Gutter) -> Self {
        Self {
            gutter,
            started: false,
            at_start: true,
            active: String::new(),
        }
    }
    fn apply(&mut self, text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            if let Some(sequence) = style_at(rest) {
                match sequence {
                    style::RESET | "\x1b[m" => self.active.clear(),
                    _ => self.active += sequence,
                }
                out += sequence;
                rest = &rest[sequence.len()..];
                continue;
            }
            rest = &rest[c.len_utf8()..];
            if c == '\n' {
                self.at_start = true;
            } else if std::mem::take(&mut self.at_start) {
                let gutter = match std::mem::replace(&mut self.started, true) {
                    true => &self.gutter.rest,
                    false => &self.gutter.first,
                };
                out += style::RESET;
                out += gutter;
                out += &self.active;
            }
            out.push(c);
        }
        out
    }
}

impl Terminal {
    /// A tool's result block, for a result that came `at`.
    pub(super) fn tool_ended(
        &mut self,
        name: &str,
        output: &str,
        detail: Option<&ToolDetail>,
        failed: bool,
        at: Instant,
    ) {
        self.clear();
        let outcome = match failed {
            true => Outcome::Failed,
            false => Outcome::Done,
        };
        let gutter = self.design.output(outcome, self.marked);
        let width = self.width().saturating_sub(gutter.columns).max(20);
        let full = self.full;
        let compact = std::mem::take(&mut self.compact);
        let took = (self.tool_started.take()).map(|started| at.saturating_duration_since(started));
        let style = match () {
            _ if !self.color => "",
            // Panels say a call failed by its band; red text on it is hard to read.
            _ if failed && self.design != Design::Panels => style::TOOL_FAILED.get(),
            _ => style::TOOL_OUTPUT.get(),
        };
        let reset = if style.is_empty() { "" } else { style::RESET };
        let styled = |lines: Vec<String>| -> Vec<String> {
            let row = |line| format!("{style}{line}{reset}");
            lines.into_iter().map(row).collect()
        };
        let shown = |limited: Shown| if full { Shown::All } else { limited };
        let hint = self.expand_hint();
        let marked = self.marked;
        let preview = |text: &str, shown: Shown, width| preview(text, shown, width, &hint, marked);
        // As Scramjet's, a file's text in its language when its name shows one: the lines of
        // `source` it starts with, the rest and the note on what was cut in the tool's style.
        let code = |text: &str, path: &str, source: &str, limited: Shown| {
            let language = highlight::of_path(path).filter(|_| self.color)?;
            let mut highlighter = Highlighter::new(language, "");
            let (mut index, lines) = (0, source.lines().count());
            let mut show = |line: &str| {
                index += 1;
                match index <= lines {
                    true => highlighter.line(&printable(line)),
                    false => format!("{style}{}{reset}", printable(line)),
                }
            };
            let shown = shown(limited);
            let mut rows = preview_shown(text, shown, width, &hint, marked, &mut show);
            if matches!(shown, Shown::First(n) if text.lines().count() > n) {
                let note = rows.pop().unwrap_or_default();
                rows.push(format!("{style}{note}{reset}"));
            }
            Some(rows)
        };
        let rows = match (name, detail) {
            _ if compact && !failed => Vec::new(),
            (_, Some(ToolDetail::Diff(diff))) => {
                let limit = if full { usize::MAX } else { diff::ROWS };
                diff::rows(diff, width, self.color, marked, limit, &hint)
            }
            // As in Scramjet, trailing blank lines of a written file are not counted.
            (_, Some(ToolDetail::Written { path, content })) => {
                let content = content.trim_end_matches(['\n', '\r']);
                let limited = Shown::First(WRITE_LINES);
                (code(content, path, content, limited))
                    .unwrap_or_else(|| styled(preview(content, shown(limited), width)))
            }
            (_, Some(ToolDetail::Read(_))) if read_line_too_long(output) => {
                styled(preview(output, Shown::All, width))
            }
            // As Scramjet's, a Markdown file's first lines rendered as a document, and the note
            // after a cut read in the tool's style.
            (_, Some(ToolDetail::Read(path)))
                if self.color
                    && self.markdown_reads == MarkdownReads::Rendered
                    && highlight::is_markdown(path) =>
            {
                let more = output.rfind(&format!("\n{READ_MORE}"));
                let source = more.map_or(output, |at| &output[..at]);
                let mut lines: Vec<String> = source.lines().map(printable).collect();
                while lines.last().is_some_and(String::is_empty) {
                    lines.pop();
                }
                let total = lines.len();
                if !full {
                    lines.truncate(READ_LINES);
                }
                let mut rows = self.markdown_rows(&lines.join("\n"), width);
                if lines.len() < total {
                    let more = total - lines.len();
                    let note = format!("… ({more} more source lines, {total} total{hint})");
                    rows.extend(styled(vec![note]));
                }
                if let Some(at) = more {
                    rows.push(String::new());
                    rows.extend(styled(preview(&output[at + 1..], Shown::All, width)));
                }
                rows
            }
            // The note after a cut read is not code.
            (_, Some(ToolDetail::Read(path))) => {
                let limited = Shown::First(READ_LINES);
                let more = output.rfind(&format!("\n{READ_MORE}"));
                let source = more.map_or(output, |at| &output[..at]);
                (code(output, path, source, limited))
                    .unwrap_or_else(|| styled(preview(output, shown(limited), width)))
            }
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
            // As in Scramjet, the instructions a command delegated to returns are shown expanded.
            (DELEGATE, None) if !failed => {
                let body = command_body(output);
                if full {
                    styled(preview(body, Shown::All, width))
                } else {
                    let mut rows = Vec::new();
                    if body.starts_with(DELEGATE_SCOPE_WARNING) {
                        let warning = "WARNING: caller and delegate allowed-tools scopes do not \
overlap; tool calls outside them are not blocked. Widen the caller scope or abort delegation.";
                        let mut wrap = self.wrapper(width);
                        let warning = wrap.push(warning) + &wrap.finish();
                        rows.extend(
                            warning
                                .split('\n')
                                .map(|row| self.styled(style::WARNING, row)),
                        );
                    }
                    if self.expandable {
                        let lines = match body.lines().count() {
                            1 => "1 line".to_owned(),
                            count => format!("{count} lines"),
                        };
                        rows.push(self.dim(&format!("… ({lines}{})", self.expand_hint())));
                    }
                    rows
                }
            }
            // As Scramjet's expanded call, each field whole, then how it went.
            (name, Some(ToolDetail::Publication(payload))) => {
                let mut rows = Vec::new();
                if full {
                    rows = self.markdown_rows(payload, width);
                    rows.push(String::new());
                }
                rows.extend(styled(preview(output, Shown::All, width)));
                if !full && self.expandable {
                    let what = match name {
                        "create_issue" => "the issue",
                        "create_pr" => "the pull request",
                        _ => "the comment",
                    };
                    let lines = match payload.lines().count() {
                        1 => "1 line".to_owned(),
                        count => format!("{count} lines"),
                    };
                    let hint = self.expand_hint();
                    rows.push(self.dim(&format!("… ({what}, {lines}{hint})")));
                }
                rows
            }
            // One sentence, with the URL of what was published.
            (name, None) if PUBLICATION_TOOLS.contains(&name) => {
                styled(preview(output, Shown::All, width))
            }
            (name, None) if scramjet::agent::is_shell(name) => styled(preview(
                &bash_shown(output, failed),
                shown(Shown::Last(BASH_LINES)),
                width,
            )),
            _ => {
                let lines = match name {
                    "grep" => GREP_LINES,
                    "find" | "ls" => LISTING_LINES,
                    _ => RESULT_LINES,
                };
                styled(preview(output, shown(Shown::First(lines)), width))
            }
        };
        self.padded(outcome, &gutter, &rows);
        // The other looks say it in the call's heading.
        let took = took.filter(|_| self.design == Design::Classic);
        if let Some(took) =
            took.filter(|took| scramjet::agent::is_shell(name) && *took >= Duration::from_secs(1))
        {
            let took = self.dim(&format!("Took {:.1}s", took.as_secs_f64()));
            self.err(&format!("{}{took}\n", gutter.rest));
        }
        // As in Scramjet, a blank line separates each tool block from what follows.
        self.err("\n");
    }
    /// Renders into a buffer `width` columns wide, styled, with every tool result in full.
    fn capture(width: usize, hide_thinking: Arc<AtomicBool>) -> Self {
        let mut terminal = Self::new(hide_thinking, false);
        terminal.color = true;
        terminal.capture = Some((String::new(), width));
        terminal.full = true;
        terminal
    }
    /// As `capture`, marked for the full screen's copy, with tool results cut as `-p` prints
    /// them unless `full`.
    pub(super) fn capture_shown(width: usize, hide_thinking: Arc<AtomicBool>, full: bool) -> Self {
        let mut terminal = Self::capture(width, hide_thinking);
        terminal.full = full;
        terminal.expandable = !full;
        terminal.marked = true;
        terminal
    }
    /// As `capture_shown`, cut, for the dock under the conversation, where a command the user
    /// runs shows at most `lines` lines and Ctrl+O does not expand it.
    pub(super) fn capture_dock(width: usize, hide_thinking: Arc<AtomicBool>, lines: usize) -> Self {
        let mut terminal = Self::capture_shown(width, hide_thinking, false);
        terminal.expandable = false;
        terminal.command_lines = lines;
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
            None => print(false, text),
        }
    }
    /// Writes to stdout, where replies go, or the capture buffer.
    fn out(&mut self, text: &str) {
        match &mut self.capture {
            Some((captured, _)) => captured.push_str(text),
            None => print(true, text),
        }
    }
    fn width(&self) -> usize {
        match self.capture {
            Some((_, width)) => width,
            None => editor::terminal_size().0,
        }
    }
    /// Whether replies are styled and wrapped: on a terminal, or when captured.
    /// In panels, a row of a call's band alone, as Scramjet's boxes are padded; a copy leaves
    /// it out.
    fn pad(&self, outcome: Outcome) -> String {
        if self.design != Design::Panels {
            return String::new();
        }
        let rule = if self.marked {
            RULE.to_string()
        } else {
            String::new()
        };
        format!("{}{rule}\n", outcome.band().mark())
    }
    /// A call's `rows` under its heading, then its bottom pad: in panels, a pad between the
    /// heading and any rows.
    fn padded(&mut self, outcome: Outcome, gutter: &Gutter, rows: &[String]) {
        let pad = self.pad(outcome);
        let above = std::mem::take(&mut self.padded_above);
        let mut block = match rows.is_empty() || above {
            true => String::new(),
            false => pad.clone(),
        };
        for (index, row) in rows.iter().enumerate() {
            block += &gutter.row(index == 0, row);
            block.push('\n');
        }
        block += &pad;
        self.err(&block);
    }
    /// The model's `text` as it may go where `kind` goes: on a terminal, its control characters
    /// as escapes, so that a reply cannot set the clipboard (OSC 52) or rewrite the screen; to a
    /// pipe or file, exact.
    fn shown<'a>(&self, kind: Streaming, text: &'a str) -> Cow<'a, str> {
        let terminal = match kind {
            Streaming::Text => self.styled_replies(),
            _ => self.color,
        };
        match terminal {
            true => Cow::Owned(visible(text)),
            false => Cow::Borrowed(text),
        }
    }
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
        if color {
            *STATUS.lock().unwrap_or_else(PoisonError::into_inner) = Some(status.clone());
        }
        if color && status_thread {
            let status = status.clone();
            let hide = hide_thinking.clone();
            let mut hidden = hide.load(Ordering::Relaxed);
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(next_spin());
                    let mut status = status.lock().unwrap();
                    // Ctrl+T during a turn arrives as a signal; say at once what it did.
                    if hide.load(Ordering::Relaxed) != hidden {
                        hidden = !hidden;
                        status.note = Some((editor::thinking_note(hidden), Instant::now()));
                        status.draw();
                    }
                    // An activity set without drawing appears once it has lasted a second.
                    if status.drawn.is_some()
                        || (status.activity.as_ref())
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
            markdown: markdown::Markdown::new(false, "", 0),
            wrap: Wrap::new(0),
            tool_started: None,
            compact: false,
            command: None,
            command_lines: COMMAND_LINES,
            hide_thinking,
            markdown_reads: MarkdownReads::Rendered,
            reasoning: String::new(),
            reasoning_hidden: false,
            capture: None,
            expandable: false,
            full: false,
            marked: false,
            design: Design::Classic,
            banner: Banner::Plain,
            outcomes: VecDeque::new(),
            outcome: Outcome::Running,
            padded_above: false,
            lead: None,
        }
    }
    /// Shows `activity` on the status line now, or only once it lasts a second.
    fn show(&mut self, activity: String, now: bool) {
        self.show_since(activity, Instant::now());
        if now && self.live() {
            self.status.lock().unwrap().draw();
        }
    }
    /// As `show`, counting from `since`, drawn by the ticker.
    fn show_since(&mut self, activity: String, since: Instant) {
        if self.live() {
            let mut status = self.status.lock().unwrap();
            status.activity = Some((activity, since));
            status.countdown = None;
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
    /// A markdown renderer for text wrapped to `width`, which a `markdown_wrapper` wraps.
    fn markdown(&self, enabled: bool, base: &'static str, width: usize) -> markdown::Markdown {
        markdown::Markdown::new(enabled, base, width).marking(self.marked)
    }
    fn markdown_wrapper(&self, width: usize) -> Wrap {
        self.wrapper(width).keeping_gutters()
    }
    /// Text from the model as markdown, its hidden characters shown, wrapped to `width`.
    fn markdown_rows(&self, text: &str, width: usize) -> Vec<String> {
        let mut markdown = self.markdown(self.color, "", width);
        let text = markdown.push(visible(text).trim_end()) + &markdown.finish();
        let mut wrap = self.markdown_wrapper(width);
        let rows = wrap.push(&text) + &wrap.finish();
        rows.lines().map(str::to_owned).collect()
    }
    pub(super) fn dim(&self, text: &str) -> String {
        self.styled(style::MUTED, text)
    }
    /// `text`'s `style::mark`ed spans in the theme in use.
    pub(super) fn themed(&self, text: &str) -> String {
        style::themed(text, self.color)
    }
    pub(super) fn styled(&self, style: Style, text: &str) -> String {
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
                self.design.reasoning_style().get()
            } else {
                ""
            };
            let lead = match kind {
                Streaming::Reasoning => self.design.reasoning(self.marked),
                Streaming::Text => self.design.reply(self.marked),
                Streaming::None => None,
            };
            self.lead = lead.filter(|_| styled).map(Lead::new);
            let width = match (styled, &self.lead) {
                (false, _) => 0,
                (true, Some(lead)) => self.width().saturating_sub(lead.gutter.columns).max(10),
                (true, None) => self.width(),
            };
            self.markdown = self.markdown(styled, base, width);
            self.wrap = self.markdown_wrapper(width);
            self.streaming = kind;
            self.text_rows =
                (kind == Streaming::Text && styled && self.streamed.is_empty()).then_some(0);
            let reopen = self.markdown.reopen();
            self.print_streamed(&reopen);
        }
        let trimmed = delta.trim_end_matches('\n');
        if !trimmed.is_empty() {
            let body = self.shown(kind, trimmed);
            let text = "\n".repeat(std::mem::take(&mut self.held_newlines)) + &body;
            let text = self.wrap.push(&self.markdown.push(&text));
            self.print_streamed(&text);
            self.printed_newlines = 0;
        }
        self.held_newlines += delta.len() - trimmed.len();
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
            self.show("Thinking".into(), false);
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
                let lead = (self.design.reasoning(self.marked)).map_or(String::new(), |g| g.first);
                self.err(&format!("{lead}{marker}\n\n"));
                self.streaming = Streaming::Reasoning;
                self.printed_newlines = 2;
                self.show("Thinking".into(), true);
            }
            (true, true) => {}
        }
        self.reasoning_hidden = hide;
    }
    fn print_streamed(&mut self, text: &str) {
        let led;
        let text = match &mut self.lead {
            Some(lead) => {
                led = lead.apply(text);
                &led
            }
            None => text,
        };
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
        self.lead = None;
    }
    pub(super) fn print_usage(&self, agent: &Agent) {
        eprintln!("{}", self.dim(&self.wrapped(&usage_line(agent))));
    }
}

/// The usage line after a turn: requests, tokens, what they cost, how full the context is and
/// the effort.
pub(super) fn usage_line(agent: &Agent) -> String {
    let usage = agent.usage;
    let written = match usage.cache_write_tokens {
        0 => String::new(),
        n => format!(", {n} written to the cache"),
    };
    format!(
        "[{} requests · {} input tokens ({} cached{written}) · {} output tokens{}{}{}]",
        usage.requests,
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
        dollars(usage.cost, usage.unpriced, 4)
            .map(|cost| format!(" · {cost}"))
            .unwrap_or_default(),
        usage
            .context_tokens
            .map(|tokens| format!(" · context {}", context(tokens, agent.context_window())))
            .unwrap_or_default(),
        thinking(agent)
            .map(|thinking| format!(" · {thinking}"))
            .unwrap_or_default()
    )
}

impl AgentUi for Terminal {
    fn waiting(&mut self, purpose: Purpose) {
        self.clear();
        self.streamed.clear();
        self.consumed = 0;
        self.text_rows = None;
        self.show(waiting_for(purpose).into(), true);
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
                self.show(format!("Preparing {name}"), true);
            }
        }
    }
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration) {
        self.clear();
        // The backoff varies by a tenth: 2.1 seconds reads as the 2 it doubles from.
        let seconds = match delay.as_secs_f64() {
            short if short < 1.0 => short.ceil(),
            long => long.round(),
        };
        let text = format!(
            "{}\n{} {seconds}s (Ctrl-C to cancel)",
            visible(error),
            retrying(attempt, retries),
        );
        let text = self.styled(style::WARNING, &self.wrapped(&text));
        // As every block, it ends with a blank line before what follows.
        self.err(&format!("{text}\n\n"));
        self.show_countdown(retrying(attempt, retries), delay);
    }
    fn cooling(&mut self, delay: Duration) {
        self.show_countdown(COOLING.into(), delay);
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
            // The deltas missed part of the final text (a gateway's DeepSeek route drops a reply's
            // first one). As Scramjet replaces the streamed block with the final text, rewrite it
            // while it is still on screen; otherwise show the final text after it.
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
            let lead = self
                .design
                .reply(self.marked)
                .filter(|_| self.styled_replies());
            let width = match (self.styled_replies(), &lead) {
                (false, _) => 0,
                (true, Some(lead)) => self.width().saturating_sub(lead.columns).max(10),
                (true, None) => self.width(),
            };
            let mut markdown = self.markdown(width > 0, "", width);
            let text = self.shown(Streaming::Text, text.trim_end());
            let text = markdown.push(&text) + &markdown.finish();
            let mut wrap = self.markdown_wrapper(width);
            let text = wrap.push(&text) + &wrap.finish();
            let text = match lead {
                Some(lead) => Lead::new(lead).apply(&text),
                None => text,
            };
            self.out(&format!("{text}\n\n"));
        }
    }
    fn tool_call(&mut self, name: &str, summary: &str, compact: Option<&CompactRead>) {
        self.clear();
        self.command = None;
        let width = if self.color { self.width() } else { 0 };
        let compact = compact.filter(|_| !self.full);
        self.compact = compact.is_some();
        let tool = name;
        let (name, summary) = match compact {
            // As Scramjet's compact reads: what was read, without its result until expanded.
            Some(compact) => {
                let (name, read) = match compact {
                    CompactRead::Skill(skill) => ("[skill]", skill),
                    CompactRead::Resource(path) => ("read resource", path),
                    CompactRead::Docs(path) => ("read docs", path),
                };
                let hint = match keymap::key(Binding::ToolsExpand) {
                    Some(key) if self.expandable => format!(" ({key} to expand)"),
                    _ => String::new(),
                };
                (name, format!("{read}{hint}"))
            }
            None => (name, summary.to_owned()),
        };
        let (outcome, took) = (self.outcomes.pop_front()).unwrap_or((Outcome::Running, None));
        self.outcome = outcome;
        let line = match self.design {
            Design::Classic => self.dim(&call_line(name, &summary, width, self.marked)),
            design => {
                let took = (took.filter(|took| {
                    scramjet::agent::is_shell(tool) && *took >= Duration::from_secs(1)
                }))
                .map(|took| format!("{:.1}s", took.as_secs_f64()));
                design.heading(name, &summary, outcome, took.as_deref(), width, self.marked)
            }
        };
        let pad = self.pad(outcome);
        self.padded_above = false;
        self.err(&format!("{pad}{line}\n"));
    }
    fn tool_running(&mut self, since: Instant) {
        self.tool_started = Some(since);
        self.show_since("Running".into(), since);
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
        self.tool_ended(name, output, detail, failed, Instant::now());
    }
    fn tool_result_at(
        &mut self,
        name: &str,
        output: &str,
        detail: Option<&ToolDetail>,
        failed: bool,
        at: Instant,
    ) {
        self.tool_ended(name, output, detail, failed, at);
    }
    fn command_started(&mut self, command: &str, include: bool) {
        self.command = Some((command.to_owned(), include));
        self.show("Running".into(), false);
    }
    fn command_result(&mut self, ran: &UserCommand) {
        self.command = None;
        self.command_block(&ran.command, ran.include, &ran.output, Some(&ran.ending));
    }
    fn stopped(&mut self, error: Option<&str>) {
        let text = match error {
            Some(error) => format!("Error: {}", visible(error)),
            None => self.dim("(cancelled)"),
        };
        self.said(&text);
    }
    /// As Scramjet shows a compaction: the estimated context size before it, then the summary;
    /// an automatic one also says why it ran.
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>) {
        self.clear();
        let mut heading = self.styled(
            style::SUMMARY,
            &format!(
                "The context was about {} tokens; kept the last {} items",
                context(compaction.tokens_before, None),
                compaction.kept_items,
            ),
        );
        if let Some(reason) = automatic {
            heading += &format!("\n{}", self.dim(reason.reason()));
        }
        let block = self.message_box("compaction", &heading, Some(&compaction.summary));
        self.err(&format!("{block}\n"));
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        self.compaction_error(&error.to_string());
    }
    fn user_prompt(&mut self, prompt: &str) {
        self.prompt_rows(prompt);
        self.err("\n");
    }
    /// As typed, and what it sent once results are shown whole: a command's or skill's text in
    /// Scramjet's box, labeled as Scramjet's.
    fn command_prompt(&mut self, typed: &str, sent: &str) {
        self.prompt_rows(typed);
        if let Some(framed) = framed(sent) {
            let name = self.styled(style::SUMMARY, framed.name);
            if self.full {
                let block = self.message_box(framed.label, &name, Some(framed.body));
                self.err(&format!("{block}\n"));
            } else if self.expandable {
                let sent =
                    self.styled(style::SUMMARY, &format!(", sent as {}", lines(framed.body)));
                let hint = match keymap::key(Binding::ToolsExpand) {
                    Some(key) => self.dim(&format!(" ({key} to expand)")),
                    None => String::new(),
                };
                let block = self.message_box(framed.label, &format!("{name}{sent}{hint}"), None);
                self.err(&format!("{block}\n"));
            } else {
                self.err("\n");
            }
            return;
        }
        let body = command_body(sent);
        let shown = if self.full {
            let width = if self.color { self.box_width() } else { 0 };
            Some(self.summary_block(body, width))
        } else if self.expandable {
            let lines = lines(body);
            Some(self.dim(&format!("… (sent as {lines}{})", self.expand_hint())))
        } else {
            None
        };
        match shown {
            Some(shown) if self.boxed() => self.err(&boxed(&shown)),
            Some(shown) if self.full => self.err(&format!("\n{shown}\n")),
            Some(shown) => self.err(&format!("{shown}\n")),
            None => {}
        }
        self.err("\n");
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.clear();
        let heading = self.styled(style::SUMMARY, "The earlier conversation is summarized");
        let block = self.message_box("compaction", &heading, Some(summary));
        self.err(&format!("{block}\n"));
    }
    fn branch_summary(&mut self, summary: &str) {
        self.clear();
        let heading = self.styled(
            style::SUMMARY,
            "The branch this conversation came back from is summarized",
        );
        let block = self.message_box("branch", &heading, Some(summary));
        self.err(&format!("{block}\n"));
    }
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer {
        self.ask(question, cancel)
    }
    /// A block of its own, ending with a blank line as a tool block does.
    fn notice(&mut self, text: &str) {
        self.clear();
        let text = self.wrapped(&visible(text));
        let text = self.styled(style::NOTICE, &text);
        self.err(&format!("{text}\n\n"));
    }
    fn next_step(&mut self, record: &str) {
        self.clear();
        let text = self.wrapped(&visible(record));
        let text = self.styled(style::NEXT_STEP, &text);
        self.err(&format!("{text}\n\n"));
    }
    /// As Scramjet shows the message in the tool's row: markdown, under the call line.
    fn question(&mut self, message: &str) {
        self.clear();
        let gutter = self.design.output(self.outcome, self.marked);
        let width = match self.color {
            true => self.width().saturating_sub(gutter.columns).max(20),
            false => 0,
        };
        // In panels, pads part the question from its heading and from its answer.
        let pad = self.pad(self.outcome);
        let mut block = pad.clone();
        for line in self.markdown_rows(message, width) {
            block += &gutter.row(false, &line);
            block.push('\n');
        }
        block += match pad.is_empty() {
            true => format!("{}\n", gutter.blank),
            false => pad,
        }
        .as_str();
        self.padded_above = self.design == Design::Panels;
        self.err(&block);
    }
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        self.numbered(title, options, recommended, recommended.unwrap_or(0))
    }
    /// Unlike a question's, an empty line cancels: in piped input it may not be an answer.
    fn approve_publication(&mut self, title: &str) -> bool {
        self.numbered(title, &publication_choices(), None, 1) == Some(0)
    }
}

/// Options for `-p` and piped input: numbered, answered by a line with a number, or an empty
/// one for option `blank`; anything else cancels. `echoed`: the line typed shows, its newline
/// included.
fn numbered_choice(
    title: &str,
    options: &[Choice],
    recommended: Option<usize>,
    blank: usize,
    echoed: bool,
) -> Option<usize> {
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
        blank + 1
    );
    let mut typed = String::new();
    let read = std::io::stdin().lock().read_line(&mut typed).unwrap_or(0);
    if !echoed || read == 0 {
        eprintln!();
    }
    if read == 0 {
        return None;
    }
    match typed.trim() {
        "" => Some(blank),
        typed => typed
            .parse::<usize>()
            .ok()
            .filter(|number| (1..=options.len()).contains(number))
            .map(|number| number - 1),
    }
}

/// Writes to stdout, where replies go, or stderr.
fn print(stdout: bool, text: &str) {
    let _ = match stdout {
        true => {
            let mut out = std::io::stdout().lock();
            out.write_all(text.as_bytes()).and_then(|()| out.flush())
        }
        false => std::io::stderr().lock().write_all(text.as_bytes()),
    };
}

impl Terminal {
    /// A message of the session's own, wrapped, as a block ending with a blank line.
    /// The blank line that separates a running call's rows from the next call's.
    pub(super) fn blank_line(&mut self) {
        self.err("\n");
    }
    pub(super) fn said(&mut self, text: &str) {
        self.clear();
        let text = self.wrapped(&self.themed(text));
        self.err(&format!("{text}\n\n"));
    }
    /// The banner a session starts with, every key and what was loaded when results are shown
    /// whole (Ctrl+O).
    pub(super) fn banner(&mut self, opening: &Opening) {
        if self.banner == Banner::Plain || !self.color {
            return self.said(&banner::plain(opening));
        }
        self.clear();
        let rows = banner::rows(self.banner, opening, self.width(), self.full, self.marked);
        self.err(&format!("{}\n\n", rows.join("\n")));
    }
    /// A changelog in a message box, cut as summaries are.
    pub(super) fn release_notes(&mut self, notes: &str) {
        self.clear();
        let version = env!("CARGO_PKG_VERSION");
        let heading = format!("What changed in Scramjet, newest first (this is {version})");
        let heading = self.styled(style::SUMMARY, &heading);
        let body = Some(notes).filter(|notes| !notes.is_empty());
        let block = self.message_box("release notes", &heading, body);
        self.err(&format!("{block}\n"));
    }
    pub(super) fn compaction_error(&mut self, error: &str) {
        self.clear();
        let text = self.wrapped(&format!("Error: automatic compaction failed: {error}"));
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
        let gutter = self.design.output(self.outcome, self.marked);
        let width = self.width().saturating_sub(gutter.columns).max(20);
        let rows = children::rows(shown, &self.look(width));
        self.padded(self.outcome, &gutter, &rows);
    }
    /// Ends the note about the lines a preview leaves out, as Scramjet's does: `, Ctrl+O to
    /// expand`, when the key shows everything.
    fn expand_hint(&self) -> String {
        match keymap::key(Binding::ToolsExpand) {
            Some(key) if self.expandable => format!(", {key} to expand"),
            _ => String::new(),
        }
    }
    /// How a tool block's rows `width` columns wide are laid out.
    fn look(&self, width: usize) -> children::Look {
        children::Look {
            width,
            color: self.color,
            marked: self.marked,
            full: self.full,
            hint: self.expand_hint(),
        }
    }
    pub(super) fn running_output(&mut self, output: &str) {
        if let Some((command, include)) = self.command.take() {
            return self.command_block(&command, include, output, None);
        }
        let gutter = self.design.output(self.outcome, self.marked);
        let width = self.width().saturating_sub(gutter.columns).max(20);
        let shown = match self.full {
            true => Shown::All,
            false => Shown::Last(BASH_LINES),
        };
        let rows: Vec<String> = preview(blank_trimmed(output), shown, width, "", self.marked)
            .iter()
            .map(|row| self.styled(style::STATUS, row))
            .collect();
        self.padded(self.outcome, &gutter, &rows);
    }
    /// A command the user ran, in Scramjet's block: between rules colored as it was run with `!`
    /// (`include`) or `!!`, `$ command`, the last lines of its output (all of them when tool
    /// results are whole), and how it ended (`None` while it runs) if not with exit code 0.
    fn command_block(
        &mut self,
        command: &str,
        include: bool,
        output: &str,
        ending: Option<&Ending>,
    ) {
        self.clear();
        let width = self.width();
        let inner = width.saturating_sub(2).max(20);
        let mut rows = Vec::new();
        for line in visible(command).split('\n') {
            let mut wrap = self.wrapper(if self.color { inner } else { 0 });
            let wrapped = wrap.push(&format!("$ {line}")) + &wrap.finish();
            let header = |row| self.styled(style::COMMAND_LINE, row);
            rows.extend(wrapped.split('\n').map(header));
        }
        let lines: Vec<&str> = output.lines().collect();
        let earlier = match self.full {
            true => 0,
            false => lines.len().saturating_sub(self.command_lines),
        };
        if !lines.is_empty() {
            rows.push(String::new());
        }
        for line in &lines[earlier..] {
            let line = printable(line);
            let shown = match self.full {
                true => {
                    let mut wrap = self.wrapper(inner);
                    wrap.push(&line) + &wrap.finish()
                }
                false => clip(&line, inner),
            };
            rows.extend(
                shown
                    .split('\n')
                    .map(|row| self.styled(style::TOOL_OUTPUT, row)),
            );
        }
        let mut status = Vec::new();
        if earlier > 0 {
            let hint = self.expand_hint();
            status.push(self.dim(&format!("… {earlier} earlier lines{hint}")));
        }
        match ending {
            Some(Ending::Exited(code)) => {
                status.push(self.styled(style::COMMAND_FAILED, &format!("(exit {code})")))
            }
            Some(Ending::Cancelled) => {
                status.push(self.styled(style::COMMAND_CANCELLED, "(cancelled)"))
            }
            Some(Ending::Failed(why)) => {
                let why = format!("({})", visible(why));
                status.push(self.styled(style::COMMAND_FAILED, &why))
            }
            Some(Ending::Done) | None => {}
        }
        if !status.is_empty() {
            rows.push(String::new());
            rows.extend(status);
        }
        let rule = self.color.then(|| {
            let rule_style = match include {
                true => style::COMMAND_RULE,
                false => style::COMMAND_EXCLUDED_RULE,
            };
            let rule = self.styled(rule_style, &"─".repeat(width.saturating_sub(1)));
            match self.marked {
                true => format!("{RULE}{rule}\n"),
                false => format!("{rule}\n"),
            }
        });
        let pad = if self.marked {
            format!(" {FROM}")
        } else {
            " ".into()
        };
        let mut block = rule.clone().unwrap_or_default();
        for row in rows {
            match row.is_empty() {
                true => block.push('\n'),
                false => block += &format!("{pad}{row}\n"),
            }
        }
        block += &rule.unwrap_or_default();
        self.err(&format!("{block}\n"));
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
    /// A message of the harness's own in Scramjet's box: `label` in bold before `heading`, then
    /// after a blank row `body` as a summary, wrapped to `box_width`; in `boxed` rows on the full
    /// screen. Ends with its last row's newline.
    fn message_box(&self, label: &str, heading: &str, body: Option<&str>) -> String {
        let width = self.box_width();
        let label = self.styled(style::MESSAGE_LABEL, &format!("[{label}]"));
        let mut wrap = self.wrapper(width);
        let mut text = wrap.push(&format!("{label} {heading}")) + &wrap.finish();
        if let Some(body) = body {
            text += "\n\n";
            text += &self.summary_block(body, width);
        }
        match self.boxed() {
            true => boxed(&text),
            false => text + "\n",
        }
    }
    /// An earlier session of the chain: a block with what ran and its summary, then, once tool
    /// results are shown whole, its conversation.
    pub(super) fn earlier(&mut self, earlier: &record::Earlier) {
        self.clear();
        let mut heading = self.styled(style::SUMMARY, &earlier.heading);
        match keymap::key(Binding::ToolsExpand) {
            Some(key) if self.expandable && !earlier.events.is_empty() => {
                heading += &self.dim(&format!(" ({key} to show its conversation)"));
            }
            _ => {}
        }
        let block = self.message_box("session", &heading, earlier.summary.as_deref());
        self.err(&format!("{block}\n"));
        if self.full {
            let outcomes = record::outcomes(&earlier.events).into();
            let outcomes = std::mem::replace(&mut self.outcomes, outcomes);
            for event in &earlier.events {
                record::replay(event, self);
            }
            self.clear();
            self.outcomes = outcomes;
        }
    }
    /// The full screen shows messages in a box on the theme's `customMessageBg`, if it has one.
    fn boxed(&self) -> bool {
        self.marked && Band::Message.painted()
    }
    /// The columns a message's text takes: the terminal's, but a column at each side of a box.
    fn box_width(&self) -> usize {
        match (self.color, self.boxed()) {
            (false, _) => 0,
            (true, false) => self.width(),
            (true, true) => self.width().saturating_sub(2).max(20),
        }
    }
    /// A message's text as Scramjet shows a compaction's summary: markdown in the message's
    /// color, wrapped to `width`. Its first lines while tool results are cut, as Scramjet
    /// previews a compaction's.
    fn summary_block(&self, summary: &str, width: usize) -> String {
        let mut markdown = self.markdown(self.color, style::SUMMARY.get(), width);
        let text = markdown.reopen() + &markdown.push(summary.trim_end()) + &markdown.finish();
        let reset = if self.color { style::RESET } else { "" };
        let mut wrap = self.markdown_wrapper(width);
        let text = wrap.push(&text) + &wrap.finish();
        let rows: Vec<&str> = text.split('\n').collect();
        if self.full || !self.expandable || rows.len() <= SUMMARY_LINES {
            return format!("{text}{reset}");
        }
        let more = rows.len() - SUMMARY_LINES;
        let hint = self.expand_hint();
        let hint = format!("… ({more} more lines, {} total{hint})", rows.len());
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
        let Some(gutter) = self.design.prompt(self.marked) else {
            for row in editor::prompt_rows(prompt, width, self.marked) {
                self.err(&format!("{row}\n"));
            }
            return;
        };
        // The editor's rows, in place of the two columns of its `>` before each.
        let rows = editor::prompt_rows(
            prompt,
            width.saturating_sub(gutter.columns - 2),
            self.marked,
        );
        // Rows of the band alone, which a copy leaves out.
        let rule = if self.marked {
            RULE.to_string()
        } else {
            String::new()
        };
        let band = (self.design.prompt_band())
            .map_or(String::new(), |band| format!("{}{rule}\n", band.mark()));
        let mut block = band.clone();
        for (index, row) in rows.iter().enumerate() {
            let text = format!("{}{}{}", style::PROMPT_TEXT, &row[2..], style::RESET);
            block += &gutter.row(index == 0, &text);
            block.push('\n');
        }
        block += &band;
        self.err(&block);
    }
    /// `options` numbered under `title`, as `numbered_choice` asks.
    fn numbered(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
        blank: usize,
    ) -> Option<usize> {
        self.clear();
        let stdin = std::io::stdin();
        // On a terminal, keys typed before it answer nothing, and the answer is an echoed line.
        let terminal = stdin.is_terminal();
        if terminal {
            console::flush_input();
        }
        let _answer = terminal.then(InputMode::answer);
        numbered_choice(title, options, recommended, blank, terminal)
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
            console::flush_input();
            eprint!("{ask}");
            let _answer = InputMode::answer();
            // SIGINT does not interrupt a blocking read, so wait and watch for Ctrl-C.
            loop {
                if cancel.load(Ordering::Relaxed) {
                    eprintln!();
                    return Answer::Deny;
                }
                match console::wait(None, Some(Duration::from_millis(50))) {
                    Ok((false, _)) => {}
                    Ok((true, _)) => break,
                    Err(_) => return Answer::Deny,
                }
            }
            // In canonical mode one read returns one typed line.
            let mut typed = [0; console::READ];
            match console::read(&mut typed) {
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
pub(super) const COMMAND_LINES: usize = 20;
/// The first lines of results shown until expanded, as Scramjet's write, read, grep, find and
/// ls show them; other tools' show `RESULT_LINES`.
const WRITE_LINES: usize = 10;
const READ_LINES: usize = 10;
const GREP_LINES: usize = 15;
const LISTING_LINES: usize = 20;
const RESULT_LINES: usize = 6;
const SUMMARY_LINES: usize = 10;

#[derive(Clone, Copy)]
enum Shown {
    First(usize),
    Last(usize),
    /// Every line, wrapped rather than cut.
    All,
}

/// What a command sent, without the `<scramjet-command>` tags around it.
pub(super) fn command_body(sent: &str) -> &str {
    let body = sent
        .strip_prefix("<scramjet-command")
        .and_then(|rest| rest.split_once(">\n"))
        .map_or(sent, |(_, body)| body)
        .trim_end();
    body.strip_suffix("</scramjet-command>")
        .unwrap_or(body)
        .trim_end()
}

/// A command's or skill's expansion as Scramjet's boxes show it.
#[derive(Debug, PartialEq)]
pub(super) struct Framed<'a> {
    pub(super) label: &'static str,
    /// The name in its tag.
    pub(super) name: &'a str,
    /// What is inside its tags; a skill's arguments after them are in the prompt as typed.
    pub(super) body: &'a str,
}

/// `sent` as a command's or skill's expansion, as Scramjet parses them; `None` for other text,
/// such as files attached to a prompt.
pub(super) fn framed(sent: &str) -> Option<Framed<'_>> {
    let (label, tag, rest) = match (
        sent.strip_prefix("<scramjet-command"),
        sent.strip_prefix("<skill"),
    ) {
        (Some(rest), _) => ("command", "scramjet-command", rest),
        (None, Some(rest)) => ("skill", "skill", rest),
        (None, None) => return None,
    };
    let (open, inner) = rest.split_once(">\n")?;
    let name = open.strip_prefix(" name=\"")?.split('"').next()?;
    let close = format!("\n</{tag}>");
    let body = match label {
        "skill" => inner.find(&close).map_or(inner, |end| &inner[..end]),
        _ => (inner.trim_end().strip_suffix(&close[1..])).unwrap_or(inner),
    };
    Some(Framed {
        label,
        name,
        body: body.trim_end(),
    })
}

/// The full screen's rows of `text` in Scramjet's box: on the theme's `customMessageBg`, with a
/// column at each side and a row above and below, which a copy leaves out.
fn boxed(text: &str) -> String {
    let gutter = Gutter::boxed(Band::Message, true);
    let pad = format!("{}{RULE}\n", Band::Message.mark());
    let mut block = pad.clone();
    for row in text.split('\n') {
        block += &gutter.row(false, row);
        block.push('\n');
    }
    block + &pad
}

/// How many lines `text` has, in words.
pub(super) fn lines(text: &str) -> String {
    match text.lines().count() {
        1 => "1 line".to_owned(),
        count => format!("{count} lines"),
    }
}

/// A failed command's ending follows its output after a blank line, which is not worth one of
/// the few lines shown.
fn bash_shown(output: &str, failed: bool) -> String {
    let output = blank_trimmed(output);
    match output.rsplit_once("\n\n") {
        Some((body, ending)) if failed => format!("{body}\n{ending}"),
        _ => output.to_owned(),
    }
}

/// `text` without its blank lines at the start and the end, as Scramjet shows a command's output.
fn blank_trimmed(text: &str) -> &str {
    let text = text.trim_end();
    let start = text.len() - text.trim_start().len();
    let start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    &text[start..]
}

/// The lines of a tool result to show, made safe for the terminal and wrapped to `width` cells,
/// with a note for lines left out that ends with `hint`; lines wrapped are `marked` for a copy.
fn preview(text: &str, shown: Shown, width: usize, hint: &str, marked: bool) -> Vec<String> {
    preview_shown(text, shown, width, hint, marked, &mut |line| {
        printable(line)
    })
}

/// As `preview`, each line it keeps shown by `show`, in order. `Shown::Last` keeps the last
/// rows, as Scramjet's: the whole lines that fit in them, or the last rows of one that does not.
fn preview_shown(
    text: &str,
    shown: Shown,
    width: usize,
    hint: &str,
    marked: bool,
    show: &mut dyn FnMut(&str) -> String,
) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut wrapped = |line: &str| -> Vec<String> {
        let mut wrap = Wrap::new(width).marking(marked);
        let rows = wrap.push(&show(line)) + &wrap.finish();
        rows.split('\n').map(str::to_owned).collect()
    };
    match shown {
        Shown::First(n) if lines.len() > n => {
            let mut kept: Vec<String> = lines[..n].iter().flat_map(|line| wrapped(line)).collect();
            kept.push(format!(
                "… ({} more lines, {} total{hint})",
                lines.len() - n,
                lines.len()
            ));
            kept
        }
        Shown::Last(n) => {
            let mut kept: Vec<String> = Vec::new();
            let mut from = lines.len();
            // Only the last `n` lines can show, each taking a row at least.
            for line in lines[lines.len().saturating_sub(n)..].iter().rev() {
                let rows = wrapped(line);
                if kept.len() + rows.len() > n {
                    if kept.is_empty() {
                        kept = rows[rows.len() - n..].to_vec();
                        from -= 1;
                    }
                    break;
                }
                kept.splice(0..0, rows);
                from -= 1;
            }
            if from > 0 {
                kept.insert(0, format!("… ({from} earlier lines{hint})"));
            }
            kept
        }
        _ => lines.iter().flat_map(|line| wrapped(line)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::selection;
    use crate::agent_cli::text::{Escape, JOINED, SPACE, plain};

    #[test]
    fn previews_keep_the_end_of_bash_output_and_are_safe_to_print() {
        let output: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let failed = format!("{output}\nCommand exited with code 1");
        assert_eq!(
            preview(
                &bash_shown(&failed, true),
                Shown::Last(5),
                80,
                ", Ctrl+O to expand",
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
        assert_eq!(bash_shown("\n \n  a\n\n", false), "  a");
        // Long lines wrap; the last rows hold the whole lines that fit, else one line's end.
        let long = "a\nbbbb bbbb bbbb\ncc";
        assert_eq!(
            preview(long, Shown::Last(3), 10, "", false),
            ["… (1 earlier lines)", "bbbb bbbb", "bbbb", "cc"]
        );
        assert_eq!(
            preview("x\none two three four five", Shown::Last(2), 10, "", false),
            ["… (1 earlier lines)", "three four", "five"]
        );
        assert_eq!(
            preview(long, Shown::First(2), 10, "", false),
            ["a", "bbbb bbbb", "bbbb", "… (1 more lines, 3 total)"]
        );
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
    fn a_users_command_shows_as_scramjets_block_and_copies_without_its_rules() {
        let plain = |text: &str| {
            let lines = crate::agent_cli::selection::lines(text);
            let rows: Vec<String> = (lines.iter())
                .map(|line| line.shown.replace(style::RESET, ""))
                .collect();
            (
                rows,
                crate::agent_cli::selection::text(&lines, 0, usize::MAX),
            )
        };
        let hide = Arc::new(AtomicBool::new(false));
        let output: String = (1..=25).map(|n| format!("{n}\n")).collect();
        let ran = UserCommand {
            command: "seq 25; exit 2 # then a long comment".into(),
            include: true,
            output: output.clone(),
            ending: Ending::Exited(2),
        };
        let mut ui = Terminal::capture_shown(30, hide.clone(), false);
        ui.command_result(&ran);
        let (rows, copied) = plain(&ui.captured());
        let rule = format!("{}{}", style::COMMAND_RULE, "─".repeat(29));
        let header = format!(" {}$ seq 25; exit 2 # then a", style::COMMAND_LINE);
        let output = |n| format!(" {}{n}", style::TOOL_OUTPUT);
        let wrapped = format!(" {}long comment", style::COMMAND_LINE);
        let mut expected = vec![rule.clone(), header, wrapped];
        expected.push(String::new());
        expected.extend((6..=25).map(output));
        expected.push(String::new());
        expected.extend([
            format!(" {}… 5 earlier lines, Ctrl+O to expand", style::MUTED),
            format!(" {}(exit 2)", style::COMMAND_FAILED),
            rule,
        ]);
        assert_eq!(rows, expected);
        let lines: String = (6..=25).map(|n| format!("{n}\n")).collect();
        assert_eq!(
            copied,
            format!(
                "$ seq 25; exit 2 # then a long comment\n\n{lines}\n… 5 earlier lines, Ctrl+O \
to expand\n(exit 2)"
            )
        );
        // Expanded, every line; a `!!` command's rules are dim, and a running one has no ending.
        let mut ui = Terminal::capture_shown(40, hide.clone(), true);
        ui.command_started("seq 25", false);
        ui.running_output(&output_text(25));
        let (rows, _) = plain(&ui.captured());
        assert_eq!(rows.len(), 1 + 1 + 1 + 25 + 1);
        assert_eq!(
            rows[0],
            format!("{}{}", style::COMMAND_EXCLUDED_RULE, "─".repeat(39))
        );
        assert_eq!(rows[3], format!(" {}1", style::TOOL_OUTPUT));
        let mut ui = Terminal::capture_shown(40, hide, false);
        let ran = UserCommand {
            output: String::new(),
            ending: Ending::Cancelled,
            ..ran
        };
        ui.command_result(&ran);
        let (rows, _) = plain(&ui.captured());
        assert_eq!(
            rows[2..4],
            [
                "".into(),
                format!(" {}(cancelled)", style::COMMAND_CANCELLED)
            ]
        );
    }

    fn output_text(lines: usize) -> String {
        (1..=lines).map(|n| format!("{n}\n")).collect()
    }

    #[test]
    fn the_captured_conversation_shows_every_tool_result_in_full() {
        let mut ui = Terminal::capture(30, Arc::new(AtomicBool::new(false)));
        ui.user_prompt("count");
        ui.progress(&StreamProgress::Reasoning("Counting **now**.\n\n".into()));
        ui.tool_call("bash", "seq 1 12", None);
        let output: String = (1..=12).map(|n| format!("{n}\n")).collect();
        ui.tool_result("bash", &output, None, false);
        ui.tool_call("read", "a.txt", None);
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
        assert!(text.contains(style::REASONING.get()));
    }

    #[test]
    fn files_read_and_written_show_in_the_language_their_names_show() {
        use style::{
            RESET, SYNTAX_COMMENT, SYNTAX_FUNCTION, SYNTAX_KEYWORD, SYNTAX_STRING, SYNTAX_TYPE,
        };
        let mut ui = Terminal::capture_shown(80, Arc::new(AtomicBool::new(false)), false);
        let read = format!("fn a() {{}}\n// x\n\n{READ_MORE}1-2 of 9. Use offset=3 to continue.]");
        ui.tool_result(
            "read",
            &read,
            Some(&ToolDetail::Read("src/a.rs".into())),
            false,
        );
        let content: String = (1..=12).map(|n| format!("print('{n}')\n")).collect();
        let path = "b.py".to_owned();
        let written = ToolDetail::Written { path, content };
        ui.tool_result("write", "Wrote", Some(&written), false);
        let plain = ToolDetail::Read("notes.txt".into());
        ui.tool_result("read", "fn a() {}", Some(&plain), false);
        let text = ui.captured().replace(FROM, "");
        let lines: Vec<&str> = text.lines().collect();
        let dim = |text: &str| format!("{}{text}{RESET}", style::TOOL_OUTPUT);
        let print = |n| format!("  {SYNTAX_TYPE}print{RESET}({SYNTAX_STRING}'{n}'{RESET})");
        let mut expected = vec![
            format!("  {SYNTAX_KEYWORD}fn{RESET} {SYNTAX_FUNCTION}a{RESET}() {{}}"),
            format!("  {SYNTAX_COMMENT}// x{RESET}"),
            format!("  {}", dim("")),
            format!(
                "  {}",
                dim("[Showing lines 1-2 of 9. Use offset=3 to continue.]")
            ),
            String::new(),
        ];
        expected.extend((1..=10).map(print));
        expected.push(format!(
            "  {}",
            dim("… (2 more lines, 12 total, Ctrl+O to expand)")
        ));
        expected.extend([
            String::new(),
            format!("  {}", dim("fn a() {}")),
            String::new(),
        ]);
        assert_eq!(lines, expected);
        // Without colors, as written.
        let mut ui = Terminal::new(Arc::new(AtomicBool::new(false)), false);
        ui.capture = Some((String::new(), 80));
        ui.tool_result(
            "read",
            "fn a() {}",
            Some(&ToolDetail::Read("a.rs".into())),
            false,
        );
        assert_eq!(ui.captured(), "  fn a() {}\n\n");
    }

    #[test]
    fn markdown_files_read_show_as_documents() {
        use style::{RESET, STRONG, TITLE};
        let mut source = "# Title\n\nSome **bold** text.\n".to_owned();
        source += &(4..=12)
            .map(|n| format!("- item {n}\n"))
            .collect::<String>();
        let read = format!("{source}\n{READ_MORE}1-12 of 30. Use offset=13 to continue.]");
        let detail = ToolDetail::Read("docs/notes.md".into());
        let shown = |full| {
            let mut ui = Terminal::capture_shown(80, Arc::new(AtomicBool::new(false)), full);
            ui.tool_result("read", &read, Some(&detail), false);
            ui.captured().replace(FROM, "")
        };
        let dim = |text: &str| format!("  {}{text}{RESET}", style::TOOL_OUTPUT);
        let bullet = |n| format!("  {}-{RESET} item {n}", style::LIST_BULLET);
        let note = dim("[Showing lines 1-12 of 30. Use offset=13 to continue.]");
        let mut cut = vec![
            format!("  {TITLE}Title{RESET}"),
            String::new(),
            format!("  Some {STRONG}bold{RESET} text."),
        ];
        cut.extend((4..=10).map(bullet));
        let mut whole = cut.clone();
        cut.extend([
            dim("… (2 more source lines, 12 total, Ctrl+O to expand)"),
            String::new(),
            note.clone(),
            String::new(),
        ]);
        assert_eq!(shown(false).lines().collect::<Vec<_>>(), cut);
        whole.extend((11..=12).map(bullet));
        whole.extend([String::new(), note, String::new()]);
        assert_eq!(shown(true).lines().collect::<Vec<_>>(), whole);
        // markdown.toolDisplay "source": its source, as other files read.
        let mut ui = Terminal::capture_shown(80, Arc::new(AtomicBool::new(false)), false);
        ui.markdown_reads = MarkdownReads::Source;
        ui.tool_result("read", &read, Some(&detail), false);
        let plain = crate::agent_cli::text::plain(&ui.captured().replace(FROM, ""));
        let mut source: Vec<String> = ["# Title", "", "Some **bold** text."]
            .map(String::from)
            .into();
        source.extend((4..=10).map(|n| format!("- item {n}")));
        let mut expected: Vec<String> = (source.iter())
            .map(|line| format!("  {line}").trim_end().to_owned())
            .collect();
        expected.extend([
            "  … (4 more lines, 14 total, Ctrl+O to expand)".into(),
            String::new(),
        ]);
        assert_eq!(plain.lines().collect::<Vec<_>>(), expected);
        // Without colors, its source.
        let mut ui = Terminal::new(Arc::new(AtomicBool::new(false)), false);
        ui.capture = Some((String::new(), 80));
        ui.tool_result("read", "# Title", Some(&detail), false);
        assert_eq!(ui.captured(), "  # Title\n\n");
    }

    #[test]
    fn compact_reads_show_what_was_read_until_expanded() {
        let call = |ui: &mut Terminal, compact: &CompactRead, failed| {
            ui.tool_call(
                "read",
                "~/skills/release-notes/SKILL.md:1-40",
                Some(compact),
            );
            let detail = ToolDetail::Read("~/skills/release-notes/SKILL.md".into());
            match failed {
                false => ui.tool_result("read", "# Release notes", Some(&detail), false),
                true => ui.tool_result("read", "Error: no such file", None, true),
            }
        };
        let lines = |ui: Terminal| {
            let text = crate::agent_cli::text::plain(&ui.captured().replace(FROM, ""));
            text.lines().map(str::to_owned).collect::<Vec<_>>()
        };
        let shown = |full, compact: &CompactRead, failed| {
            let mut ui = Terminal::capture_shown(80, Arc::new(AtomicBool::new(false)), full);
            call(&mut ui, compact, failed);
            lines(ui)
        };
        let skill = CompactRead::Skill("release-notes:1-40".into());
        let expand = " (Ctrl+O to expand)";
        assert_eq!(
            shown(false, &skill, false),
            [
                format!("● [skill] release-notes:1-40{expand}"),
                String::new()
            ]
        );
        assert_eq!(
            shown(true, &skill, false),
            [
                "● read ~/skills/release-notes/SKILL.md:1-40",
                "  Release notes",
                ""
            ]
        );
        let resource = CompactRead::Resource("CLAUDE.md".into());
        assert_eq!(
            shown(false, &resource, true),
            [
                format!("● read resource CLAUDE.md{expand}"),
                "  Error: no such file".into(),
                String::new()
            ]
        );
        // Printed, where nothing expands; the next call's result shows.
        let mut printed = Terminal::new(Arc::new(AtomicBool::new(false)), false);
        printed.capture = Some((String::new(), 80));
        call(&mut printed, &CompactRead::Docs("README.md".into()), false);
        printed.tool_call("read", "a.txt", None);
        printed.tool_result("read", "text", None, false);
        assert_eq!(
            lines(printed),
            ["● read docs README.md", "", "● read a.txt", "  text", ""]
        );
    }

    #[test]
    fn summaries_and_command_texts_are_labeled_and_cut_until_expanded() {
        let summary: String = (1..=12).map(|n| format!("Point {n}\n")).collect();
        let sent = "<scramjet-command name=\"rs:loop\">\n# Build\n\nGo.\n</scramjet-command>";
        let skill = "<skill name=\"pdf\" location=\"/s/pdf/SKILL.md\">\nReferences are relative to \
                     /s/pdf.\n\nFill forms.\n</skill>\n\nform.pdf";
        let attached = "<file name=\"/p/notes.md\">\nhi\n</file>\nWhat?";
        let shown = |full| {
            let mut ui = Terminal::capture_shown(70, Arc::new(AtomicBool::new(false)), full);
            ui.command_prompt("/rs:loop", sent);
            ui.command_prompt("/skill:pdf form.pdf", skill);
            ui.command_prompt("@notes.md What?", attached);
            ui.compaction_summary(&summary);
            ui.branch_summary("Short");
            let compaction = Compaction {
                tokens_before: 52_600,
                kept_items: 4,
                summary: "Done.".into(),
            };
            ui.compacted(&compaction, Some(AutoCompaction::Threshold));
            ui.compacted(&compaction, None);
            let text = ui.captured();
            let mut plain = String::new();
            let mut escape = Escape::None;
            let marker = |c| [FROM, JOINED, SPACE].contains(&c);
            plain.extend(text.chars().filter(|&c| !escape.step(c) && !marker(c)));
            plain
        };
        // The terminal's colors give the box no background, so it has no padding.
        let cut = shown(false);
        let points: String = (1..=10).map(|n| format!("Point {n}\n")).collect();
        assert!(
            cut.starts_with(
                "> /rs:loop\n[command] rs:loop, sent as 3 lines (Ctrl+O to expand)\n\n\
                 > /skill:pdf form.pdf\n[skill] pdf, sent as 3 lines (Ctrl+O to expand)\n\n\
                 > @notes.md What?\n… (sent as 4 lines, Ctrl+O to expand)\n\n\
                 [compaction] The earlier conversation is summarized\n\n"
            ),
            "{cut:?}"
        );
        assert!(cut.contains(&format!(
            "{points}… (2 more lines, 12 total, Ctrl+O to expand)\n\n"
        )));
        // A compaction says how large the context was, and why it ran when automatic; the next
        // block comes after a blank line, also after `/compact`.
        assert!(
            cut.ends_with(
                "\n[branch] The branch this conversation came back from is summarized\n\nShort\n\n\
                 [compaction] The context was about 53k tokens; kept the last 4 items\n\
                 Automatic: the context was nearing the model's window.\n\nDone.\n\n\
                 [compaction] The context was about 53k tokens; kept the last 4 items\n\nDone.\n\n"
            ),
            "{cut:?}"
        );
        let whole = shown(true);
        assert!(
            whole.starts_with(
                "> /rs:loop\n[command] rs:loop\n\nBuild\n\nGo.\n\n> /skill:pdf form.pdf\n\
                 [skill] pdf\n\nReferences are relative to /s/pdf.\n\nFill forms.\n\n\
                 > @notes.md What?\n\n"
            ),
            "{whole:?}"
        );
        assert!(whole.contains(&summary) && !whole.contains("Ctrl+O"));
    }

    #[test]
    fn a_message_box_is_on_its_band_with_padding_a_copy_leaves_out() {
        let rows = boxed("\x1b[1m[branch]\x1b[0m Went\n\nThere");
        assert_eq!(
            rows,
            "\x16\x1c\n\x16 \x1d\x1b[1m[branch]\x1b[0m Went\n\x16 \n\x16 \x1dThere\n\x16\x1c\n"
        );
        let lines = selection::lines(&rows);
        assert!(lines.iter().all(|line| line.band == Some(Band::Message)));
        assert_eq!(
            lines
                .iter()
                .map(|line| plain(&line.shown))
                .collect::<Vec<_>>(),
            ["", " [branch] Went", " ", " There", ""]
        );
    }

    #[test]
    fn commands_and_skills_are_framed_as_scramjet_parses_them() {
        let command = "<scramjet-command name=\"rs:loop\">\n# Build\n\nGo.\n</scramjet-command>\n";
        assert_eq!(
            framed(command),
            Some(Framed {
                label: "command",
                name: "rs:loop",
                body: "# Build\n\nGo."
            })
        );
        let skill = "<skill name=\"pdf\" location=\"/s/SKILL.md\">\nFill.\n</skill>\n\nform.pdf";
        assert_eq!(
            framed(skill),
            Some(Framed {
                label: "skill",
                name: "pdf",
                body: "Fill."
            })
        );
        assert_eq!(framed("<file name=\"a.md\">\nhi\n</file>\nWhat?"), None);
        assert_eq!(framed("<skill>\nno name\n</skill>"), None);
        assert_eq!(framed("plain"), None);
    }

    #[test]
    fn delegated_instructions_are_shown_when_expanded_and_refusals_always() {
        let body = "<scramjet-command name=\"rs:read\">\nRead issue 42.\n\nThen stop.\n</scramjet-command>";
        let disjoint = format!(
            "<scramjet-command name=\"rs:edit\">\n{DELEGATE_SCOPE_WARNING} scopes differ.\n\nEdit\n</scramjet-command>"
        );
        let shown = |full| {
            let mut ui = Terminal::capture_shown(40, Arc::new(AtomicBool::new(false)), full);
            ui.tool_call(DELEGATE, "/rs:read 42", None);
            ui.tool_result(DELEGATE, body, None, false);
            ui.tool_call(DELEGATE, "/rs:edit", None);
            ui.tool_result(DELEGATE, &disjoint, None, false);
            ui.tool_call(DELEGATE, "/rs:nope", None);
            ui.tool_result(DELEGATE, "ERROR: unknown command 'rs:nope'.", None, true);
            let text = ui.captured();
            let mut plain = String::new();
            let mut escape = Escape::None;
            let marker = |c| [FROM, JOINED, SPACE].contains(&c);
            plain.extend(text.chars().filter(|&c| !escape.step(c) && !marker(c)));
            plain
        };
        assert_eq!(
            shown(false),
            "● delegate /rs:read 42\n  … (3 lines, Ctrl+O to expand)\n\n● delegate /rs:edit\n  \
WARNING: caller and delegate\n  allowed-tools scopes do not overlap;\n  tool calls outside them are not\n  \
blocked. Widen the caller scope or\n  abort delegation.\n  … (3 lines, Ctrl+O to expand)\n\n● delegate \
/rs:nope\n  ERROR: unknown command 'rs:nope'.\n\n"
        );
        let whole = shown(true);
        assert!(
            whole.starts_with("● delegate /rs:read 42\n  Read issue 42.\n  \n  Then stop.\n\n"),
            "{whole:?}"
        );
        assert!(
            !whole.contains("WARNING: caller") && whole.contains("[scramjet/delegate] WARNING:")
        );
    }

    #[test]
    fn a_publications_result_is_shown_whole_with_its_url_and_its_payload_once_expanded() {
        let verified = "Create issue publication verified: https://github.com/o/r/issues/5";
        let payload = "## Title\n\n```\nFix it\n```\n\n## Body\n\n````\nIt **breaks**\x1b[2J.\n## \
Target\n![a](https://x.example/a.png) <!-- hid -->\n```\n````";
        let shown = |full, detail: Option<&ToolDetail>| {
            let mut ui = Terminal::capture_shown(40, Arc::new(AtomicBool::new(false)), full);
            ui.tool_call("create_issue", "Fix it", None);
            ui.tool_result("create_issue", verified, detail, false);
            let text = ui.captured();
            let mut plain = String::new();
            let mut escape = Escape::None;
            let marker = |c| [FROM, JOINED, SPACE].contains(&c);
            plain.extend(text.chars().filter(|&c| !escape.step(c) && !marker(c)));
            plain
        };
        let result = "  Create issue publication verified:\n  https://github.com/o/r/issues/5\n";
        assert_eq!(
            shown(false, None),
            format!("● create_issue Fix it\n{result}\n")
        );
        let detail = ToolDetail::Publication(payload.into());
        assert_eq!(
            shown(false, Some(&detail)),
            format!(
                "● create_issue Fix it\n{result}  … (the issue, 14 lines, Ctrl+O to \
expand)\n\n"
            )
        );
        assert_eq!(
            shown(true, Some(&detail)),
            format!(
                "● create_issue Fix it\n  Title\n\n  ```\n    Fix it\n  ```\n\n  Body\n\n  ```\n    \
It **breaks**\\u{{1b}}[2J.\n    ## Target\n    ![a](https://x.example/a.png) <!--\n    hid -->\n    \
```\n  ```\n\n{result}\n"
            )
        );
    }

    #[test]
    fn replies_and_reasoning_on_a_terminal_show_control_characters_as_escapes() {
        let reply = "a\x1b]52;c;aGk=\x07b \x1b[2J\r\n";
        let mut ui = Terminal::capture(40, Arc::new(AtomicBool::new(false)));
        ui.waiting(Purpose::Reply);
        ui.progress(&StreamProgress::Reasoning("think\x1b]0;T\x07\n\n".into()));
        ui.progress(&StreamProgress::Text(reply.into()));
        ui.assistant(reply);
        ui.waiting(Purpose::Reply);
        ui.assistant("only\x1b[2Jfinal");
        let shown = ui.captured();
        assert!(
            !shown.contains('\x07') && !shown.contains('\r'),
            "{shown:?}"
        );
        for escaped in [
            "think\\u{1b}]0;T\\u{7}",
            "a\\u{1b}]52;c;aGk=\\u{7}b \\u{1b}[2J\\r",
            "only\\u{1b}[2Jfinal",
        ] {
            assert!(shown.contains(escaped), "{escaped} in {shown:?}");
        }
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
            activity: Some((retrying(2, 3), started)),
            countdown: Some(Duration::from_secs(4)),
            ..Status::default()
        };
        assert_eq!(status.line().unwrap(), "Retrying (2/3) in 3s");
        status.countdown = Some(Duration::from_secs(1));
        assert_eq!(status.line().unwrap(), "Retrying (2/3) in 0s");
        status.activity = Some(("Thinking".into(), started));
        status.countdown = None;
        status.note = Some(("Thinking blocks: hidden", Instant::now()));
        assert_eq!(
            status.line().unwrap(),
            "Thinking 1s · Thinking blocks: hidden"
        );
        status.note = Some(("Thinking blocks: hidden", started - NOTE_TIME));
        assert_eq!(status.line().unwrap(), "Thinking 1s");
    }
    #[test]
    fn the_spinner_shows_scramjets_frames_in_turn() {
        let at = |ms| frame(Duration::from_millis(ms));
        assert_eq!(
            [0, 79, 80, 160, 799, 800].map(at),
            ['⠋', '⠋', '⠙', '⠹', '⠏', '⠋']
        );
        assert!(next_spin() > Duration::ZERO && next_spin() <= SPIN);
    }
}
