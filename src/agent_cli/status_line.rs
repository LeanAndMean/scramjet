//! The `statusLine` setting, Claude Code's: a command run with the session's state as JSON on
//! its standard input, in Claude Code's fields, whose output shows under the footer. It runs
//! once the state settles after a change, every `refreshInterval` seconds if set, never two at
//! once, and on the screen's thread's behalf in a thread of its own.

use super::footer::Shown;
use super::style::{self, RESET};
use super::text::{clip, hidden, style_at};
use serde_json::{Value, json};
use std::{
    io::{PipeWriter, Write},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// How long the state must stay unchanged before the command runs, as Claude Code's.
const SETTLE: Duration = Duration::from_millis(300);
/// How long the command may run, as Claude Code's.
const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    pub(super) command: String,
    /// Columns left blank before each row.
    pub(super) padding: usize,
    /// How often it runs again when nothing changes.
    pub(super) refresh: Option<Duration>,
}

impl Config {
    pub(super) fn new(command: String) -> Self {
        Self {
            command,
            padding: 0,
            refresh: None,
        }
    }
}

/// How the last run ended: its output, or why it failed.
type Ran = Arc<Mutex<Option<Result<String, String>>>>;

pub(super) struct StatusLine {
    config: Option<Config>,
    /// The state the command was last asked about, without the time that passes.
    asked: Option<Value>,
    due: Option<Instant>,
    started: Option<Instant>,
    ran: Ran,
    running: bool,
    /// The running command's thread, and what stops the command.
    thread: Option<(JoinHandle<()>, Arc<AtomicBool>)>,
    /// The rows shown, as the command printed them.
    shown: Vec<String>,
    /// Since when the session's screen runs, for `cost.total_duration_ms`.
    since: Instant,
}

impl StatusLine {
    pub(super) fn new() -> Self {
        Self {
            config: None,
            asked: None,
            due: None,
            started: None,
            ran: Arc::default(),
            running: false,
            thread: None,
            shown: Vec::new(),
            since: Instant::now(),
        }
    }

    /// Follows `config`; a changed command runs at once.
    pub(super) fn configure(&mut self, config: Option<Config>) {
        if config == self.config {
            return;
        }
        if config.is_none() {
            self.shown.clear();
        }
        (self.config, self.asked, self.due) = (config, None, None);
    }

    /// Runs the command when it is due, about `shown` in `root`, its end waking the screen
    /// through `wake`; takes what an ended run printed. How long until it is due next.
    pub(super) fn poll(
        &mut self,
        shown: &Shown,
        root: &Path,
        wake: &PipeWriter,
    ) -> Option<Duration> {
        let config = self.config.clone()?;
        // Not before the session has said what it is.
        if shown.session_id.is_empty() {
            return None;
        }
        if let Some(ran) = self
            .ran
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            self.running = false;
            self.shown = match ran {
                Ok(output) => rows_of(&output),
                Err(why) => vec![format!("{}statusLine: {why}{RESET}", style::MUTED)],
            };
        }
        let now = Instant::now();
        let state = input(shown, root);
        if self.asked.as_ref() != Some(&state) {
            // The first run is at once, as there is nothing to wait for.
            let settle = if self.asked.is_some() {
                SETTLE
            } else {
                Duration::ZERO
            };
            (self.asked, self.due) = (Some(state.clone()), Some(now + settle));
        }
        let refresh = (config.refresh).and_then(|every| Some(self.started? + every));
        let due = match (self.due, refresh) {
            (Some(due), Some(refresh)) => due.min(refresh),
            (due, refresh) => due.or(refresh)?,
        };
        if self.running {
            return None;
        }
        if due > now {
            return Some(due - now);
        }
        let Ok(wake) = wake.try_clone() else {
            return None;
        };
        let mut state = state;
        state["cost"]["total_duration_ms"] = json!(self.since.elapsed().as_millis() as u64);
        let (ran, root) = (self.ran.clone(), root.to_owned());
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let started = std::thread::Builder::new()
            .name("status line".into())
            .spawn(move || {
                let input = state.to_string().into_bytes();
                let command = &config.command;
                let output =
                    scramjet::agent::run_with_input(&root, command, &input, TIMEOUT, &stop);
                *ran.lock().unwrap_or_else(PoisonError::into_inner) = Some(output);
                let _ = (&wake).write(&[0]);
            });
        if let Ok(thread) = started {
            (self.running, self.due, self.started) = (true, None, Some(now));
            self.thread = Some((thread, cancel));
        }
        None
    }

    /// Stops the command if it runs, and waits for it to end.
    pub(super) fn stop(&mut self) {
        if let Some((thread, cancel)) = self.thread.take() {
            cancel.store(true, Ordering::Relaxed);
            let _ = thread.join();
        }
    }

    /// The rows under the footer on a screen `width` wide, at most `most` of them.
    pub(super) fn rows(&self, width: usize, most: usize) -> Vec<String> {
        let Some(config) = &self.config else {
            return Vec::new();
        };
        let room = width.saturating_sub(1 + config.padding);
        let pad = " ".repeat(config.padding);
        (self.shown.iter().take(most))
            .map(|row| format!("{}{pad}{}{RESET}", style::FOOTER, clip(row, room)))
            .collect()
    }
}

/// What the command printed, a row a line, with only its style sequences kept of the escapes
/// and control characters; the blank lines at its end left out.
fn rows_of(output: &str) -> Vec<String> {
    let mut rows: Vec<String> = output.lines().map(printed).collect();
    while rows
        .last()
        .is_some_and(|row| super::text::plain(row).trim().is_empty())
    {
        rows.pop();
    }
    rows
}

/// `line` with its style sequences (`ESC [ … m`) and its text, tabs as spaces; other escape
/// sequences, control and hidden characters left out, as they could move the cursor or hide
/// what the screen shows.
fn printed(line: &str) -> String {
    let (mut shown, mut rest) = (String::new(), line);
    while let Some(c) = rest.chars().next() {
        if let Some(sequence) = style_at(rest) {
            shown += sequence;
            rest = &rest[sequence.len()..];
            continue;
        }
        rest = &rest[c.len_utf8()..];
        match c {
            '\x1b' => rest = after_escape(rest),
            '\t' => shown.push(' '),
            c if c.is_control() || hidden(c) => {}
            c => shown.push(c),
        }
    }
    shown
}

/// `text` after the escape sequence whose `ESC` came just before it: a CSI's up to its final
/// byte, an OSC's or another string's up to BEL or ST, else one character.
fn after_escape(text: &str) -> &str {
    let mut chars = text.char_indices();
    match chars.next() {
        Some((_, '[')) => (chars.find(|(_, c)| ('@'..='~').contains(c)))
            .map_or("", |(at, c)| &text[at + c.len_utf8()..]),
        Some((_, ']' | 'P' | '_' | '^' | 'X')) => {
            let mut previous = '\0';
            for (at, c) in chars {
                if c == '\x07' {
                    return &text[at + 1..];
                }
                if previous == '\x1b' && c == '\\' {
                    return &text[at + 1..];
                }
                previous = c;
            }
            ""
        }
        Some((_, c)) => &text[c.len_utf8()..],
        None => "",
    }
}

/// The session's state in Claude Code's fields for its status line (its 2.1.259), those
/// Scramjet keeps: what it does not count (lines changed, the API's time, rate limits) is left
/// out rather than given as zero.
fn input(shown: &Shown, root: &Path) -> Value {
    let root = root.to_string_lossy();
    let tokens = &shown.tokens;
    let percent = match (shown.context, shown.window) {
        (Some(context), Some(window @ 1..)) => Some(context as f64 * 100.0 / window as f64),
        _ => None,
    };
    let thinking =
        (shown.effort.as_deref()).is_some_and(|effort| !matches!(effort, "off" | "none"));
    let mut state = json!({
        "hook_event_name": "Status",
        "session_id": shown.session_id,
        "transcript_path": shown.transcript.to_string_lossy(),
        "cwd": root,
        "model": {
            "id": shown.model,
            "display_name": shown.model_name.as_deref().unwrap_or(&shown.model),
        },
        "workspace": {"current_dir": root, "project_dir": root, "added_dirs": []},
        "version": env!("CARGO_PKG_VERSION"),
        "output_style": {"name": "default"},
        "cost": {"total_cost_usd": tokens.cost},
        "context_window": {
            "total_input_tokens": tokens.input + tokens.cache_read + tokens.cache_write,
            "total_output_tokens": tokens.output,
            "context_window_size": shown.window,
            "current_usage": null,
            "used_percentage": percent,
            "remaining_percentage": percent.map(|percent| (100.0 - percent).max(0.0)),
        },
        "exceeds_200k_tokens": shown.context.is_some_and(|context| context > 200_000),
        "thinking": {"enabled": thinking},
    });
    if let Some(name) = &shown.name {
        state["session_name"] = json!(name);
    }
    if let Some(effort) = shown.effort.as_deref().filter(|_| thinking) {
        state["effort"] = json!({"level": effort});
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet::agent::Tokens;

    fn shown() -> Shown {
        Shown {
            name: Some("Refactor".into()),
            tokens: Tokens {
                input: 1_000,
                output: 500,
                cache_read: 20_000,
                cache_write: 3_000,
                cost: 0.25,
                unpriced: false,
            },
            context: Some(50_000),
            window: Some(200_000),
            model: "claude-opus-5-5".into(),
            model_name: Some("Claude Opus 5.5".into()),
            effort: Some("high".into()),
            session_id: "1795684883549-21904".into(),
            transcript: "/state/sessions/1795684883549-21904.jsonl".into(),
            ..Shown::default()
        }
    }

    #[test]
    fn the_input_has_claude_codes_fields_and_leaves_out_what_is_not_counted() {
        let state = input(&shown(), Path::new("/home/u/project"));
        assert_eq!(state["hook_event_name"], "Status");
        assert_eq!(state["session_id"], "1795684883549-21904");
        assert_eq!(state["session_name"], "Refactor");
        assert_eq!(state["cwd"], "/home/u/project");
        assert_eq!(state["workspace"]["current_dir"], "/home/u/project");
        assert_eq!(state["workspace"]["project_dir"], "/home/u/project");
        assert_eq!(
            state["model"],
            json!({"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5"})
        );
        assert_eq!(state["cost"], json!({"total_cost_usd": 0.25}));
        let window = &state["context_window"];
        assert_eq!(window["total_input_tokens"], 24_000);
        assert_eq!(window["total_output_tokens"], 500);
        assert_eq!(window["context_window_size"], 200_000);
        assert_eq!(window["used_percentage"], 25.0);
        assert_eq!(window["remaining_percentage"], 75.0);
        assert_eq!(state["exceeds_200k_tokens"], false);
        assert_eq!(state["effort"], json!({"level": "high"}));
        assert_eq!(state["thinking"], json!({"enabled": true}));
        // Without a name the model's id stands for it, and without an effort no thinking.
        let plain = Shown {
            model: "gpt-6-luna".into(),
            ..Shown::default()
        };
        let state = input(&plain, Path::new("/p"));
        assert_eq!(state["model"]["display_name"], "gpt-6-luna");
        assert_eq!(state["context_window"]["used_percentage"], Value::Null);
        assert_eq!(state["thinking"], json!({"enabled": false}));
        assert!(state.get("effort").is_none() && state.get("session_name").is_none());
    }

    #[test]
    fn only_style_sequences_are_kept_of_what_the_command_prints() {
        let output = "\x1b[36mu@h\x1b[0m:\x1b[34m~/p\x1b[0m (main) | \x1b[35mOpus\x1b[0m\r\n\
                      a\tb\x1b[2J\x1b[H\x1b]0;title\x07c\x1b]8;;https://x\x1b\\link\x1b]8;;\x1b\\\u{200b}\n\n \n";
        assert_eq!(
            rows_of(output),
            [
                "\x1b[36mu@h\x1b[0m:\x1b[34m~/p\x1b[0m (main) | \x1b[35mOpus\x1b[0m",
                "a bclink",
            ]
        );
        assert_eq!(rows_of(""), Vec::<String>::new());
        assert_eq!(printed("cut \x1b["), "cut ");
    }

    #[test]
    fn a_command_still_running_is_stopped_with_what_it_started() {
        if !scramjet::agent::found("bash") {
            return;
        }
        let (_woken, wake) = std::io::pipe().unwrap();
        let scratch = std::env::temp_dir().join(format!("status-line-{}", std::process::id()));
        std::fs::create_dir_all(&scratch).unwrap();
        let mut line = StatusLine::new();
        line.configure(Some(Config::new("sleep 30 & echo $! > child; wait".into())));
        // Nothing runs before the session's state is in.
        assert_eq!(line.poll(&Shown::default(), &scratch, &wake), None);
        assert!(!line.running);
        line.poll(&shown(), &scratch, &wake);
        let child = scratch.join("child");
        while std::fs::read_to_string(&child).map_or(true, |pid| !pid.ends_with('\n')) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid: u32 = std::fs::read_to_string(&child)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let started = Instant::now();
        line.stop();
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!scramjet::os::process_runs(pid), "{pid} runs");
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn rows_are_padded_clipped_and_shown_only_while_configured() {
        let mut line = StatusLine::new();
        line.shown = vec!["one two three".into(), "second".into(), "third".into()];
        assert!(line.rows(80, 5).is_empty());
        line.configure(Some(Config {
            padding: 2,
            ..Config::new("true".into())
        }));
        let rows: Vec<String> = (line.rows(10, 2).iter())
            .map(|row| super::super::text::plain(row))
            .collect();
        assert_eq!(rows, ["  one tw…", "  second"]);
        line.configure(None);
        assert!(line.shown.is_empty() && line.rows(80, 5).is_empty());
    }

    #[test]
    fn the_command_runs_at_once_then_after_the_state_settles_and_never_twice_at_once() {
        if !scramjet::agent::found("bash") {
            return;
        }
        let (mut woken, wake) = std::io::pipe().unwrap();
        let root = std::env::temp_dir();
        let mut line = StatusLine::new();
        line.configure(Some(Config::new(
            r#"read -r input; printf '%s\n' "${input:0:30}"; echo "$PWD""#.into(),
        )));
        let mut shown = shown();
        assert_eq!(line.poll(&shown, &root, &wake), None);
        assert!(line.running);
        // While it runs, a change waits for it.
        shown.tokens.output = 600;
        assert_eq!(line.poll(&shown, &root, &wake), None);
        let mut byte = [0];
        std::io::Read::read_exact(&mut woken, &mut byte).unwrap();
        let wait = line.poll(&shown, &root, &wake).unwrap();
        assert!(wait <= SETTLE && wait > SETTLE / 2, "{wait:?}");
        let rows = line.rows(200, 5);
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows[0].contains(r#"{"context_window":{"context_wi"#),
            "{rows:?}"
        );
        assert!(rows[1].contains(root.to_string_lossy().trim_end_matches('/')));
        std::thread::sleep(wait);
        assert_eq!(line.poll(&shown, &root, &wake), None);
        assert!(line.running);
        std::io::Read::read_exact(&mut woken, &mut byte).unwrap();
        // Unchanged, it does not run again; a failure says why.
        assert_eq!(line.poll(&shown, &root, &wake), None);
        assert!(!line.running);
        line.configure(Some(Config::new("exit 3".into())));
        line.poll(&shown, &root, &wake);
        std::io::Read::read_exact(&mut woken, &mut byte).unwrap();
        line.poll(&shown, &root, &wake);
        let rows: Vec<String> = (line.rows(80, 5).iter())
            .map(|row| super::super::text::plain(row))
            .collect();
        assert_eq!(rows, ["statusLine: exited with 3"]);
    }
}
