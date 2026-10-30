mod diff;
mod editor;
mod markdown;

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    termios::{self, LocalModes, OptionalActions, Termios},
};
use scramjet_rs::agent::{
    Agent, AgentConfig, AgentError, AgentUi, Answer, SessionChoice, StreamProgress, ToolDetail,
};
use std::{
    ffi::OsString,
    io::{BufRead, IsTerminal, Write},
    net::SocketAddr,
    os::fd::BorrowedFd,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const USAGE: &str = "Usage: scramjet-rs agent [--project DIR] [--endpoint ADDR] [--model ID] \
[--effort LEVEL] [--max-output-tokens N] [--retries N] [--continue | --session FILE] [--yes]
[-p PROMPT]

Interactive coding agent using a Responses endpoint (default: Proxy at 127.0.0.1:18080).
  --project DIR          project directory (default: current directory)
  --model ID             model (default: gpt-6-luna)
  --effort LEVEL         reasoning effort, e.g. low, medium, high
  --retries N            resend a request that failed transiently up to N times (default 3)
  --continue             resume the most recent session for this project
  --session FILE         resume a specific session file
  --yes                  approve every tool call without asking (answering `a` to an
                         approval question does the same from then on)
  -p PROMPT              run one prompt and exit (`-p -` reads it from stdin)
In the interactive prompt, Enter sends; Shift+Enter, Alt+Enter, Ctrl-J or a trailing \\ starts
a new line; pasted text is inserted without sending. Up/Down recall earlier prompts.
Ctrl-C clears the prompt; on an empty prompt press it twice, or Ctrl-D, or type /exit, to quit.
/new starts a new session.
While a turn runs, typed text is kept for the next prompt without being shown; Ctrl-C cancels
the turn (and discards that text); press it twice to quit.";

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let mut project = None;
    let mut endpoint: SocketAddr = ([127, 0, 0, 1], 18080).into();
    let mut model = "gpt-6-luna".to_owned();
    let mut effort = None;
    let mut max_output_tokens = None;
    let mut choice = SessionChoice::New;
    let mut approve_all = false;
    let mut retries = 3;
    let mut prompt = None;
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .and_then(|v| v.into_string().ok())
                .ok_or(format!(
                    "{} requires a value\n\n{USAGE}",
                    argument.to_string_lossy()
                ))
        };
        match argument.to_str() {
            Some("--project") => project = Some(PathBuf::from(value()?)),
            Some("--endpoint") => {
                endpoint = value()?.parse().map_err(|_| "invalid --endpoint address")?
            }
            Some("--model") => model = value()?,
            Some("--effort") => effort = Some(value()?),
            Some("--max-output-tokens") => {
                max_output_tokens = Some(
                    value()?
                        .parse()
                        .map_err(|_| "invalid --max-output-tokens")?,
                )
            }
            Some("--retries") => retries = value()?.parse().map_err(|_| "invalid --retries")?,
            Some("--continue") => choice = SessionChoice::Continue,
            Some("--session") => choice = SessionChoice::Open(PathBuf::from(value()?)),
            Some("--yes") => approve_all = true,
            Some("-p" | "--print") => prompt = Some(value()?),
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}\n\n{USAGE}")),
        }
    }
    let root = match project {
        Some(path) => path,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let config = AgentConfig {
        endpoint,
        model,
        root,
        effort,
        max_output_tokens,
        approve_all,
        retries,
    };
    let mut agent = Agent::start(config, choice).map_err(|e| e.to_string())?;

    let cancel = Arc::new(AtomicBool::new(false));
    let startup = termios::tcgetattr(std::io::stdin()).ok();
    let flag = cancel.clone();
    // SAFETY: the handler only swaps an atomic, calls tcsetattr and _exit, all async-signal-safe.
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
            // A second Ctrl-C while a cancellation is still pending exits.
            if flag.swap(true, Ordering::Relaxed) {
                if let Some(startup) = &startup {
                    let _ = termios::tcsetattr(
                        BorrowedFd::borrow_raw(0),
                        OptionalActions::Now,
                        startup,
                    );
                }
                signal_hook::low_level::exit(130);
            }
        })
    }
    .map_err(|e| e.to_string())?;
    let mut ui = Terminal::new();

    if let Some(prompt) = prompt {
        let prompt = if prompt == "-" {
            std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
        } else {
            prompt
        };
        let echo = Echo::set(false);
        let result = agent.turn(&prompt, &mut ui, &cancel);
        drop(echo);
        ui.clear();
        ui.usage(&agent);
        return result.map_err(|e| e.to_string());
    }

    eprintln!(
        "scramjet-rs agent · {} · {}\nsession {}{}",
        agent.model(),
        agent.root().display(),
        agent.session_path().display(),
        if agent.conversation().is_empty() {
            String::new()
        } else {
            format!(" ({} items resumed)", agent.conversation().len())
        }
    );
    let mut editor =
        (std::io::stdin().is_terminal() && std::io::stderr().is_terminal()).then(|| {
            editor::Editor::new(
                agent
                    .conversation()
                    .iter()
                    .filter(|item| item["role"] == "user")
                    .filter_map(|item| item["content"].as_str().map(str::to_owned))
                    .collect(),
            )
        });
    loop {
        let prompt = match &mut editor {
            Some(editor) => {
                eprintln!();
                match editor.read().map_err(|e| e.to_string())? {
                    Some(prompt) => prompt,
                    None => return Ok(()),
                }
            }
            None => match read_lines()? {
                Some(prompt) => prompt,
                None => return Ok(()),
            },
        };
        let prompt = prompt.trim();
        if prompt.is_empty() {
            continue;
        }
        if matches!(prompt, "/exit" | "/quit") {
            return Ok(());
        }
        if prompt == "/new" {
            match agent.new_session() {
                Ok(()) => eprintln!(
                    "✓ New session started\nsession {}",
                    agent.session_path().display()
                ),
                Err(error) => eprintln!("error: {error}"),
            }
            continue;
        }
        if let Some(editor) = &mut editor {
            editor.remember(prompt);
            eprintln!();
        }
        cancel.store(false, Ordering::Relaxed);
        let echo = Echo::set(false);
        let result = agent.turn(prompt, &mut ui, &cancel);
        drop(echo);
        ui.clear();
        match result {
            Ok(()) => {}
            Err(AgentError::Cancelled) => eprintln!("{}", ui.dim("(cancelled)")),
            Err(error) => eprintln!("error: {error}"),
        }
        cancel.store(false, Ordering::Relaxed);
        ui.usage(&agent);
    }
}

/// Reads a prompt from non-terminal input, where a line ending in \ continues on the next line.
fn read_lines() -> Result<Option<String>, String> {
    let mut prompt = String::new();
    loop {
        eprint!("{}", if prompt.is_empty() { "\n> " } else { ". " });
        let mut line = String::new();
        if std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\n', '\r']);
        if let Some(start) = line.strip_suffix('\\') {
            prompt.push_str(start);
            prompt.push('\n');
        } else {
            prompt.push_str(line);
            return Ok(Some(prompt));
        }
    }
}

/// Sets whether the terminal echoes typed keys until dropped. Keys typed while a turn runs stay
/// queued for the prompt editor instead of appearing in the output.
struct Echo(Option<Termios>);

impl Echo {
    fn set(on: bool) -> Self {
        let Ok(previous) = termios::tcgetattr(std::io::stdin()) else {
            return Self(None);
        };
        let mut changed = previous.clone();
        changed.local_modes.set(LocalModes::ECHO, on);
        let _ = termios::tcsetattr(std::io::stdin(), OptionalActions::Now, &changed);
        Self(Some(previous))
    }
}

impl Drop for Echo {
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
struct Status {
    activity: Option<(String, Instant)>,
    /// Shows the seconds left of this wait instead of the elapsed seconds.
    countdown: Option<Duration>,
    /// Shown above the line, each cut to fit one row.
    output: Vec<String>,
    /// The text on the screen, while it is shown, and the number of output rows above the line.
    drawn: Option<String>,
    rows_above: usize,
}

impl Status {
    fn line(&self) -> Option<String> {
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
        let width = editor::terminal_size().0.saturating_sub(1).max(10);
        let mut block: String = self
            .output
            .iter()
            .map(|line| format!("  \x1b[2m{line}\x1b[0m\n"))
            .collect();
        block += &format!("\x1b[2m{}\x1b[0m", clip(&text, width));
        if self.drawn.as_deref() == Some(&block) {
            return;
        }
        eprint!("{}{block}", self.erase());
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
        if drawn {
            eprint!("{}", self.erase());
        }
        self.activity = None;
        self.countdown = None;
        self.output.clear();
        self.rows_above = 0;
        drawn
    }
}

/// Breaks streamed text into lines that fit the terminal, between words where it can, instead of
/// leaving the terminal to split words at its edge. A word is held until it is complete.
struct Wrap {
    /// Zero passes text through unchanged.
    width: usize,
    column: usize,
    word: String,
    word_width: usize,
    /// Spaces before `word`, dropped if it starts a new line after a break.
    spaces: usize,
    /// `word` continues one already broken at the edge of the line.
    continued: bool,
    /// No word has been placed since the last newline in the text.
    line_start: bool,
    /// Where lines wrapped from the current one start: under the text of a list item, or at
    /// the indentation of an indented line.
    indent: usize,
    escape: Escape,
}

/// Where the text is in an escape sequence, which takes no columns.
#[derive(Clone, Copy, Default, PartialEq)]
enum Escape {
    #[default]
    None,
    Start,
    Csi,
}

impl Escape {
    /// Whether `c` is part of an escape sequence.
    fn step(&mut self, c: char) -> bool {
        *self = match (*self, c) {
            (Self::None, '\x1b') => Self::Start,
            (Self::None, _) => return false,
            (Self::Start, '[') => Self::Csi,
            (Self::Csi, c) if !('@'..='~').contains(&c) => Self::Csi,
            _ => Self::None,
        };
        true
    }
}

impl Wrap {
    fn new(width: usize) -> Self {
        Self {
            width,
            column: 0,
            word: String::new(),
            word_width: 0,
            spaces: 0,
            continued: false,
            line_start: true,
            indent: 0,
            escape: Escape::None,
        }
    }
    fn push(&mut self, text: &str) -> String {
        if self.width == 0 {
            return text.to_owned();
        }
        let mut out = String::new();
        for c in text.chars() {
            if self.escape.step(c) {
                self.word.push(c);
                continue;
            }
            match c {
                '\n' => {
                    self.place(&mut out);
                    self.spaces = 0;
                    self.column = 0;
                    self.line_start = true;
                    self.indent = 0;
                    out.push('\n');
                }
                ' ' | '\t' => {
                    self.place(&mut out);
                    self.spaces += if c == '\t' { 4 } else { 1 };
                }
                c => {
                    self.word.push(c);
                    self.word_width += c.width().unwrap_or(0);
                    if self.word_width > self.width {
                        self.place(&mut out);
                        self.continued = true;
                    }
                }
            }
        }
        out
    }
    /// The held word, once a newline that is not yet passed on has ended it.
    fn end_word(&mut self) -> String {
        let mut out = String::new();
        self.place(&mut out);
        out
    }
    /// The held word, at the end of the text.
    fn finish(&mut self) -> String {
        let mut out = String::new();
        self.place(&mut out);
        *self = Self::new(self.width);
        out
    }
    fn place(&mut self, out: &mut String) {
        if self.word.is_empty() {
            self.continued = false;
            return;
        }
        if self.line_start {
            let marker = matches!(self.word.as_str(), "-" | "*" | "+" | "•" | "●")
                || self.word.strip_suffix(['.', ')']).is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                });
            let indent = self.spaces + if marker { self.word_width + 1 } else { 0 };
            self.indent = if indent < self.width / 2 { indent } else { 0 };
            self.line_start = false;
        }
        if !self.continued {
            if self.column > self.indent && self.column + self.spaces + self.word_width > self.width
            {
                self.break_line(out);
                self.spaces = 0;
            }
            out.extend(std::iter::repeat_n(' ', self.spaces));
            self.column += self.spaces;
            self.spaces = 0;
        }
        let word = std::mem::take(&mut self.word);
        let mut escape = Escape::None;
        for c in word.chars() {
            let cells = match escape.step(c) {
                true => 0,
                false => c.width().unwrap_or(0),
            };
            if self.column > self.indent && self.column + cells > self.width {
                self.break_line(out);
            }
            out.push(c);
            self.column += cells;
        }
        self.word_width = 0;
        self.continued = false;
    }
    fn break_line(&mut self, out: &mut String) {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', self.indent));
        self.column = self.indent;
    }
}

fn elapsed(seconds: u64) -> String {
    match seconds {
        0 => String::new(),
        1..60 => format!(" {seconds}s"),
        _ => format!(" {}m {:02}s", seconds / 60, seconds % 60),
    }
}

struct Terminal {
    color: bool,
    status: Arc<Mutex<Status>>,
    streaming: Streaming,
    /// Whether assistant text streamed during the current response.
    streamed_text: bool,
    /// Newlines ending the streamed block so far, not yet printed.
    held_newlines: usize,
    /// Newlines ending the streamed block that were printed early.
    printed_newlines: usize,
    markdown: markdown::Markdown,
    wrap: Wrap,
    tool_started: Option<Instant>,
}

impl Terminal {
    fn new() -> Self {
        let color = std::io::stderr().is_terminal();
        let status = Arc::new(Mutex::<Status>::default());
        if color {
            let status = status.clone();
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(200));
                    let mut status = status.lock().unwrap();
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
            streamed_text: false,
            held_newlines: 0,
            printed_newlines: 0,
            markdown: markdown::Markdown::new(false, ""),
            wrap: Wrap::new(0),
            tool_started: None,
        }
    }
    /// Shows `activity` on the status line now, or only once it lasts a second.
    fn show(&mut self, activity: String, now: bool) {
        if self.color {
            let mut status = self.status.lock().unwrap();
            status.activity = Some((activity, Instant::now()));
            status.countdown = None;
            if now {
                status.draw();
            }
        }
    }
    fn show_countdown(&mut self, activity: String, delay: Duration) {
        if self.color {
            let mut status = self.status.lock().unwrap();
            status.activity = Some((activity, Instant::now()));
            status.countdown = Some(delay);
            status.draw();
        }
    }
    /// `text` broken between words to fit the terminal, for output to stderr.
    fn wrapped(&self, text: &str) -> String {
        let mut wrap = Wrap::new(if self.color {
            editor::terminal_size().0
        } else {
            0
        });
        wrap.push(text) + &wrap.finish()
    }
    fn dim(&self, text: &str) -> String {
        if self.color {
            format!("\x1b[2m{text}\x1b[0m")
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
                self.print_streamed(&self.markdown.reopen());
            }
        } else {
            self.clear();
            let styled = match kind {
                Streaming::Text => std::io::stdout().is_terminal(),
                _ => self.color,
            };
            let base = if kind == Streaming::Reasoning {
                "\x1b[2m"
            } else {
                ""
            };
            self.markdown = markdown::Markdown::new(styled, base);
            self.wrap = Wrap::new(if styled { editor::terminal_size().0 } else { 0 });
            self.streaming = kind;
            self.print_streamed(&self.markdown.reopen());
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
    fn print_streamed(&self, text: &str) {
        if self.streaming == Streaming::Text {
            print!("{text}");
            let _ = std::io::stdout().flush();
        } else {
            eprint!("{text}");
        }
    }
    /// Removes the status line and ends any streamed block before other output.
    fn clear(&mut self) {
        self.status.lock().unwrap().clear();
        self.held_newlines = 0;
        let rest = self.wrap.push(&self.markdown.finish());
        let rest = rest + &self.wrap.finish();
        self.print_streamed(&rest);
        match self.streaming {
            Streaming::None => {}
            Streaming::Reasoning => eprint!(
                "{}{}",
                if self.color { "\x1b[0m" } else { "" },
                "\n".repeat(2 - self.printed_newlines)
            ),
            Streaming::Text => {
                println!("\n");
                let _ = std::io::stdout().flush();
            }
        }
        self.printed_newlines = 0;
        self.streaming = Streaming::None;
    }
    fn usage(&self, agent: &Agent) {
        let usage = agent.usage;
        let line = format!(
            "[{} requests · {} input tokens ({} cached) · {} output tokens]",
            usage.requests, usage.input_tokens, usage.cached_input_tokens, usage.output_tokens
        );
        eprintln!("{}", self.dim(&self.wrapped(&line)));
    }
}

impl AgentUi for Terminal {
    fn waiting(&mut self) {
        self.clear();
        self.streamed_text = false;
        self.show("thinking".into(), true);
    }
    fn progress(&mut self, update: &StreamProgress) {
        match update {
            StreamProgress::Reasoning(delta) => self.stream(Streaming::Reasoning, delta),
            StreamProgress::Text(delta) => {
                self.streamed_text = true;
                self.stream(Streaming::Text, delta);
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
        if self.color {
            eprintln!("\x1b[33m{text}\x1b[0m");
        } else {
            eprintln!("{text}");
        }
        self.show_countdown("retrying in".into(), delay);
    }
    fn assistant(&mut self, text: &str) {
        let streamed = self.streamed_text;
        self.clear();
        if !streamed && !text.is_empty() {
            let width = match std::io::stdout().is_terminal() {
                true => editor::terminal_size().0,
                false => 0,
            };
            let mut markdown = markdown::Markdown::new(width > 0, "");
            let text = markdown.push(text.trim_end()) + &markdown.finish();
            let mut wrap = Wrap::new(width);
            let text = wrap.push(&text) + &wrap.finish();
            println!("{text}\n");
            let _ = std::io::stdout().flush();
        }
    }
    fn tool_call(&mut self, name: &str, summary: &str) {
        self.clear();
        let mut lines = summary.lines();
        let first = lines.next().unwrap_or("");
        let more = if lines.next().is_some() { " …" } else { "" };
        let line = format!("● {name} {}{more}", visible(first));
        eprintln!("{}", self.dim(&self.wrapped(&line)));
    }
    fn tool_running(&mut self) {
        self.tool_started = Some(Instant::now());
        self.show("running".into(), false);
    }
    fn tool_output(&mut self, output: &str) {
        if !self.color {
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
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        self.clear();
        let width = editor::terminal_size().0.saturating_sub(2).max(20);
        let took = self.tool_started.take().map(|started| started.elapsed());
        let style = match () {
            _ if !self.color => "",
            _ if failed => "\x1b[31m",
            _ => "\x1b[2m",
        };
        let reset = if style.is_empty() { "" } else { "\x1b[0m" };
        let styled = |lines: Vec<String>| -> Vec<String> {
            let row = |line| format!("{style}{line}{reset}");
            lines.into_iter().map(row).collect()
        };
        let rows = match (name, detail) {
            (_, Some(ToolDetail::Diff(diff))) => diff::rows(diff, width, self.color),
            // As in Scramjet, trailing blank lines of a written file are not counted.
            (_, Some(ToolDetail::Written(content))) => styled(preview(
                content.trim_end_matches(['\n', '\r']),
                Shown::First(WRITE_LINES),
                width,
            )),
            ("bash", None) => styled(preview(
                &bash_shown(output, failed),
                Shown::Last(BASH_LINES),
                width,
            )),
            _ => styled(preview(output, Shown::First(RESULT_LINES), width)),
        };
        for row in rows {
            eprintln!("  {row}");
        }
        if let Some(took) = took.filter(|took| name == "bash" && *took >= Duration::from_secs(1)) {
            eprintln!(
                "  {}",
                self.dim(&format!("took {:.1}s", took.as_secs_f64()))
            );
        }
        // As in Scramjet, a blank line separates each tool block from what follows.
        eprintln!();
    }
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer {
        self.clear();
        let question = visible(question);
        let stdin = std::io::stdin();
        let ask = format!("{question}\nAllow? [y/N, a = allow all until exit] ");
        let typed = if !stdin.is_terminal() {
            eprint!("{ask}");
            let mut typed = String::new();
            if stdin.lock().read_line(&mut typed).is_err() {
                return Answer::Deny;
            }
            typed
        } else {
            // Only an answer typed after the question is shown may approve it.
            let _ = termios::tcflush(&stdin, termios::QueueSelector::IFlush);
            eprint!("{ask}");
            let _echo = Echo::set(true);
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
            eprintln!(
                "{}",
                self.dim("Tool calls now run without asking until exit.")
            );
        }
        answer
    }
}

fn answer(typed: &str) -> Answer {
    match typed.trim() {
        "y" | "Y" | "yes" => Answer::Allow,
        "a" | "A" | "all" => Answer::AllowAll,
        _ => Answer::Deny,
    }
}

/// Scramjet shows the end of bash output, where errors and the exit code are.
const BASH_LINES: usize = 5;
const WRITE_LINES: usize = 10;
const RESULT_LINES: usize = 6;

enum Shown {
    First(usize),
    Last(usize),
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
/// with a note for lines left out.
fn preview(text: &str, shown: Shown, width: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let fit = |line: &&str| clip(&printable(line), width);
    match shown {
        Shown::First(n) if lines.len() > n => {
            let mut kept: Vec<String> = lines[..n].iter().map(fit).collect();
            kept.push(format!(
                "… ({} more lines, {} total)",
                lines.len() - n,
                lines.len()
            ));
            kept
        }
        Shown::Last(n) if lines.len() > n => {
            let mut kept = vec![format!("… ({} earlier lines)", lines.len() - n)];
            kept.extend(lines[lines.len() - n..].iter().map(fit));
            kept
        }
        _ => lines.iter().map(fit).collect(),
    }
}

/// Drops escape sequences and control characters, expands tabs, and keeps only the text after
/// the last carriage return, as a terminal would show it.
fn printable(line: &str) -> String {
    let line = line
        .split('\r')
        .rfind(|part| !part.is_empty())
        .unwrap_or("");
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\t' => text.push_str("    "),
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next().is_some()) {
                            break;
                        }
                    }
                }
                // Intermediate bytes, then one final byte, such as `ESC ( B` from `tput sgr0`.
                Some(' '..='/') => {
                    for c in chars.by_ref() {
                        if !(' '..='/').contains(&c) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if c.is_control() => {}
            c => text.push(c),
        }
    }
    text
}

/// Shows control and invisible formatting characters (other than newlines and tabs) as escapes,
/// so that a command cannot make its approval question display something else.
fn visible(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\t' => c.to_string(),
            '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}' => c.escape_default().to_string(),
            c if c.is_control() => c.escape_default().to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut clipped = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cells = c.width().unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        used += cells;
        clipped.push(c);
    }
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_keep_the_end_of_bash_output_and_are_safe_to_print() {
        let output: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let failed = format!("{output}\nCommand exited with code 1");
        assert_eq!(
            preview(&bash_shown(&failed, true), Shown::Last(5), 80),
            [
                "… (16 earlier lines)",
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
        let head = preview(&output, Shown::First(2), 80);
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
    fn streamed_text_wraps_between_words() {
        let wrapped = |width, pieces: &[&str]| {
            let mut wrap = Wrap::new(width);
            let mut out: String = pieces.iter().map(|piece| wrap.push(piece)).collect();
            out += &wrap.finish();
            out
        };
        assert_eq!(
            wrapped(11, &["hel", "lo wo", "rld foo"]),
            "hello world\nfoo"
        );
        assert_eq!(wrapped(11, &["hello   ", "  world"]), "hello\nworld");
        assert_eq!(
            wrapped(20, &["- a\n    indented  \nend"]),
            "- a\n    indented\nend"
        );
        let long = format!("a {} b", "x".repeat(25));
        let broken = format!("a\n{0}\n{0}\n{1} b", "x".repeat(10), "x".repeat(5));
        assert_eq!(wrapped(10, &[&long]), broken);
        assert_eq!(
            wrapped(10, &[&format!("{} b", "x".repeat(11))]),
            "xxxxxxxxxx\nx b"
        );
        assert_eq!(wrapped(6, &["日本語 です"]), "日本語\nです");
        assert_eq!(
            wrapped(12, &["- one two three\n12. four five six\n  seven eight"]),
            "- one two\n  three\n12. four\n    five six\n  seven\n  eight"
        );
        assert_eq!(wrapped(12, &["-flag one two"]), "-flag one\ntwo");
        assert_eq!(
            wrapped(20, &["● bash cargo fmt --check && cargo clippy"]),
            "● bash cargo fmt\n  --check && cargo\n  clippy"
        );
        assert_eq!(wrapped(0, &["no  wrap \n"]), "no  wrap \n");
        assert_eq!(
            wrapped(11, &["\x1b[1mhel", "lo\x1b[0m world \x1b[36mfoo"]),
            "\x1b[1mhello\x1b[0m world\n\x1b[36mfoo"
        );
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
    }
}
