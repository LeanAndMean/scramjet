//! A turn's display calls, kept as they pass to the terminal, so that the conversation view can
//! show the turn so far, rendered again at its own width, while the turn runs; the full-screen
//! interface keeps the whole conversation so.

use super::terminal::{Status, Terminal};
use super::view;
use scramjet_rs::agent::{
    self, AgentError, AgentUi, Answer, AutoCompaction, Choice, Compaction, Purpose, StreamProgress,
    ToolDetail,
};
use serde_json::Value;
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// A display call, and what the session says between turns (`Text`).
pub(super) enum Event {
    Waiting(Purpose),
    Progress(StreamProgress),
    Retrying(String, u32, u32, Duration),
    Assistant(String),
    ToolCall(String, String),
    ToolRunning,
    ToolOutput(String),
    ToolResult(String, String, Option<ToolDetail>, bool),
    CommandResult(String, bool),
    Compacted(Compaction, Option<AutoCompaction>),
    CompactionFailed(String),
    UserPrompt(String),
    CompactionSummary(String),
    BranchSummary(String),
    Approval(String, Answer),
    Question(String),
    Notice(String),
    /// A message of the session's own, such as a command's result, styled but not wrapped.
    Text(String),
}

#[derive(Default)]
struct Log {
    events: Vec<Event>,
    version: usize,
}

/// What the view shows of a turn: the conversation before it and the prompt that started it,
/// then what the turn has shown since.
pub(super) struct Shown {
    before: Vec<Value>,
    prompt: Option<String>,
    log: Mutex<Log>,
    status: Arc<Mutex<Status>>,
    hide_thinking: Arc<AtomicBool>,
}

impl Shown {
    pub(super) fn new(before: &[Value], prompt: Option<&str>, ui: &Terminal) -> Self {
        Self {
            before: before.to_vec(),
            prompt: prompt.map(str::to_owned),
            log: Mutex::default(),
            status: ui.status.clone(),
            hide_thinking: ui.hide_thinking.clone(),
        }
    }

    fn record(&self, event: Event) {
        let mut log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        log.version += 1;
        // Only a running tool's latest output is shown.
        if let (Event::ToolOutput(output), Some(Event::ToolOutput(last))) =
            (&event, log.events.last_mut())
        {
            last.clone_from(output);
            return;
        }
        log.events.push(event);
    }

    /// Shows the view until the user closes it, given the keys typed after Ctrl+O; `waiting`
    /// is set once the agent waits for the user. Returns the keys typed after it closed.
    pub(super) fn follow(&self, typed: Vec<u8>, waiting: &AtomicBool) -> Vec<u8> {
        let render = |width| {
            let mut ui = Terminal::capture(width, self.hide_thinking.clone());
            agent::show_items(&self.before, &mut ui);
            if let Some(prompt) = &self.prompt {
                ui.user_prompt(prompt);
            }
            ui.captured()
        };
        let live = view::Live {
            turn: self,
            waiting,
        };
        match view::show(render, Some(live), &self.hide_thinking, typed) {
            Ok(typed) => typed,
            Err(error) => {
                view::print(false, &format!("error: {error}\n"));
                Vec::new()
            }
        }
    }
}

impl view::Turn for Shown {
    fn version(&self) -> usize {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .version
    }

    fn render(&self, width: usize) -> String {
        let mut ui = Terminal::capture(width, self.hide_thinking.clone());
        let log = self.log.lock().unwrap_or_else(PoisonError::into_inner);
        for event in &log.events {
            replay(event, &mut ui);
        }
        if let Some(Event::ToolOutput(output)) = log.events.last() {
            ui.running_output(output);
        }
        drop(log);
        ui.captured()
    }

    fn activity(&self) -> Option<String> {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .activity_line()
    }
}

/// Shows `event` on `ui` as it showed when it happened.
pub(super) fn replay(event: &Event, ui: &mut Terminal) {
    match event {
        Event::Waiting(purpose) => ui.waiting(*purpose),
        Event::Progress(update) => ui.progress(update),
        Event::Retrying(error, attempt, retries, delay) => {
            ui.retrying(error, *attempt, *retries, *delay)
        }
        Event::Assistant(text) => ui.assistant(text),
        Event::ToolCall(name, summary) => ui.tool_call(name, summary),
        Event::ToolRunning => ui.tool_running(),
        Event::ToolOutput(_) => {}
        Event::ToolResult(name, output, detail, failed) => {
            ui.tool_result(name, output, detail.as_ref(), *failed)
        }
        Event::CommandResult(output, failed) => ui.command_result(output, *failed),
        Event::Compacted(compaction, automatic) => ui.compacted(compaction, *automatic),
        Event::CompactionFailed(error) => ui.compaction_error(error),
        Event::UserPrompt(prompt) => ui.user_prompt(prompt),
        Event::CompactionSummary(summary) => ui.compaction_summary(summary),
        Event::BranchSummary(summary) => ui.branch_summary(summary),
        Event::Approval(question, answer) => ui.approval(question, *answer),
        Event::Question(message) => ui.question(message),
        Event::Notice(text) => ui.notice(text),
        Event::Text(text) => ui.said(text),
    }
}

/// The terminal, with each call it gets kept for the view.
pub(super) struct Recording<'a> {
    pub(super) ui: &'a mut Terminal,
    pub(super) shown: &'a Shown,
}

impl AgentUi for Recording<'_> {
    fn waiting(&mut self, purpose: Purpose) {
        self.shown.record(Event::Waiting(purpose));
        self.ui.waiting(purpose);
    }
    fn progress(&mut self, update: &StreamProgress) {
        self.shown.record(Event::Progress(update.clone()));
        self.ui.progress(update);
    }
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration) {
        let event = Event::Retrying(error.to_owned(), attempt, retries, delay);
        self.shown.record(event);
        self.ui.retrying(error, attempt, retries, delay);
    }
    fn assistant(&mut self, text: &str) {
        self.shown.record(Event::Assistant(text.to_owned()));
        self.ui.assistant(text);
    }
    fn tool_call(&mut self, name: &str, summary: &str) {
        let event = Event::ToolCall(name.to_owned(), summary.to_owned());
        self.shown.record(event);
        self.ui.tool_call(name, summary);
    }
    fn tool_running(&mut self) {
        self.shown.record(Event::ToolRunning);
        self.ui.tool_running();
    }
    fn tool_output(&mut self, output: &str) {
        self.shown.record(Event::ToolOutput(output.to_owned()));
        self.ui.tool_output(output);
    }
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        let event = Event::ToolResult(name.to_owned(), output.to_owned(), detail.cloned(), failed);
        self.shown.record(event);
        self.ui.tool_result(name, output, detail, failed);
    }
    fn command_result(&mut self, output: &str, failed: bool) {
        self.shown
            .record(Event::CommandResult(output.to_owned(), failed));
        self.ui.command_result(output, failed);
    }
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>) {
        self.shown
            .record(Event::Compacted(compaction.clone(), automatic));
        self.ui.compacted(compaction, automatic);
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        self.shown
            .record(Event::CompactionFailed(error.to_string()));
        self.ui.compaction_failed(error);
    }
    fn user_prompt(&mut self, prompt: &str) {
        self.shown.record(Event::UserPrompt(prompt.to_owned()));
        self.ui.user_prompt(prompt);
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.shown
            .record(Event::CompactionSummary(summary.to_owned()));
        self.ui.compaction_summary(summary);
    }
    fn branch_summary(&mut self, summary: &str) {
        self.shown.record(Event::BranchSummary(summary.to_owned()));
        self.ui.branch_summary(summary);
    }
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer {
        let answer = self.ui.approve(question, cancel);
        // A cancelled question shows no answer.
        let shown = match cancel.load(Ordering::Relaxed) {
            true => None,
            false => Some(answer),
        };
        if let Some(answer) = shown {
            self.shown
                .record(Event::Approval(question.to_owned(), answer));
        }
        answer
    }
    fn question(&mut self, message: &str) {
        self.shown.record(Event::Question(message.to_owned()));
        self.ui.question(message);
    }
    fn notice(&mut self, text: &str) {
        self.shown.record(Event::Notice(text.to_owned()));
        self.ui.notice(text);
    }
    /// The list leaves nothing on screen; the question's result shows the answer.
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        self.ui.choose(title, options, recommended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Every call the view replays.
    fn calls(ui: &mut dyn AgentUi) {
        ui.waiting(Purpose::Reply);
        ui.progress(&StreamProgress::Reasoning("Looking **closely**.".into()));
        ui.progress(&StreamProgress::Reasoning("\n\n".into()));
        ui.progress(&StreamProgress::Text("The fi".into()));
        ui.progress(&StreamProgress::Text("rst reply.".into()));
        ui.assistant("The first reply.");
        ui.retrying("busy", 1, 3, Duration::from_secs(2));
        ui.tool_call("bash", "seq 1 30");
        ui.tool_running();
        ui.tool_output("1\n2");
        ui.tool_output("1\n2\n3");
        let output: Vec<_> = (1..=30).map(|n| n.to_string()).collect();
        ui.tool_result("bash", &output.join("\n"), None, false);
        ui.tool_call("edit", "src/a.rs");
        let diff = ToolDetail::Diff("-1 old\n+1 new".into());
        ui.tool_result("edit", "Edited.", Some(&diff), false);
        ui.question("Which **one**?");
        ui.notice("scramjet: blocked");
        ui.compaction_failed(&AgentError::Provider("down".into()));
        let compaction = Compaction {
            tokens_before: 12_000,
            kept_items: 3,
            summary: "## Goal\nTest.".into(),
        };
        ui.compacted(&compaction, Some(AutoCompaction::Threshold));
        ui.command_result("done", true);
        ui.user_prompt("next");
        ui.compaction_summary("Earlier.");
        ui.branch_summary("Elsewhere.");
    }

    #[test]
    fn a_replayed_turn_shows_as_it_did() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut direct = Terminal::capture(40, hide.clone());
        calls(&mut direct);
        let direct = direct.captured();

        let mut terminal = Terminal::capture(40, hide.clone());
        let shown = Shown::new(&[], None, &terminal);
        calls(&mut Recording {
            ui: &mut terminal,
            shown: &shown,
        });
        assert_eq!(view::Turn::render(&shown, 40), direct);
        // Every line of the result, its diff and the reply streamed in parts, once.
        assert!(direct.contains("  \x1b[2m1\x1b[0m\n") && direct.contains("  \x1b[2m30\x1b[0m\n"));
        assert!(direct.contains("\x1b[32m+1 \x1b[7mnew"), "{direct:?}");
        assert_eq!(direct.matches("The first reply.").count(), 1, "{direct:?}");
    }

    #[test]
    fn a_running_tool_shows_its_latest_output() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut terminal = Terminal::capture(40, hide);
        let shown = Shown::new(&[], Some("go"), &terminal);
        let mut ui = Recording {
            ui: &mut terminal,
            shown: &shown,
        };
        ui.tool_call("bash", "make");
        ui.tool_running();
        ui.tool_output("first");
        let before = view::Turn::version(&shown);
        ui.tool_output("first\nsecond");
        assert_ne!(view::Turn::version(&shown), before);
        let text = view::Turn::render(&shown, 40);
        assert!(
            text.contains("make") && text.ends_with("second\x1b[0m\n"),
            "{text:?}"
        );
        assert!(!text.contains("first\x1b[0m\n  \x1b[2mfirst"), "{text:?}");
        ui.tool_result("bash", "first\nsecond\ndone", None, false);
        let text = view::Turn::render(&shown, 40);
        assert_eq!(text.matches("second").count(), 1, "{text:?}");
    }
}
