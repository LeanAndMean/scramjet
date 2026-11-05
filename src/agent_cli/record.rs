//! The agent's display calls, kept as events: the full screen keeps the conversation so, and
//! renders it again from them at its width.

use super::design::Outcome;
use super::style::Style;
use super::terminal::Terminal;
use scramjet_rs::agent::{
    AgentUi, Answer, AutoCompaction, Children, CompactRead, Compaction, Purpose, StreamProgress,
    ToolDetail, UserCommand,
};
use std::time::{Duration, Instant};

/// A display call, and what the session says between turns (`Text`).
pub(super) enum Event {
    Waiting(Purpose),
    Progress(StreamProgress),
    Retrying(String, u32, u32, Duration),
    Assistant(String),
    ToolCall(String, String, Option<CompactRead>),
    /// Since when.
    ToolRunning(Instant),
    ToolOutput(String),
    /// The children of a running `subagent` call, as they are now.
    Subagents(Children),
    /// A tool's result, and when it came.
    ToolResult(String, String, Option<ToolDetail>, bool, Instant),
    /// The calls of a reply that run at the same time, each with its events.
    Calls(Vec<Vec<Event>>),
    /// A command the user runs with `!` (true) or `!!` starts.
    CommandStarted(String, bool),
    CommandResult(UserCommand),
    /// A turn stopped before its end: cancelled (`None`), or failed with the error.
    Stopped(Option<String>),
    Compacted(Compaction, Option<AutoCompaction>),
    CompactionFailed(String),
    UserPrompt(String),
    /// A command's prompt as typed, and the text it sent.
    CommandPrompt(String, String),
    CompactionSummary(String),
    BranchSummary(String),
    Approval(String, Answer),
    Question(String),
    Notice(String),
    NextStep(String),
    /// A message of the session's own, such as a command's result, styled but not wrapped.
    Text(String),
    /// One in a style, which takes the theme it is shown in.
    Styled(Style, String),
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
        Event::ToolCall(name, summary, compact) => ui.tool_call(name, summary, compact.as_ref()),
        Event::ToolRunning(since) => ui.tool_running(*since),
        Event::ToolOutput(_) | Event::Subagents(_) => {}
        Event::ToolResult(name, output, detail, failed, at) => {
            ui.tool_ended(name, output, detail.as_ref(), *failed, *at)
        }
        // In call order, each as far as it got, a running one's output or children under it.
        Event::Calls(calls) => {
            let started: Vec<_> = calls.iter().filter(|events| !events.is_empty()).collect();
            for (index, events) in started.iter().enumerate() {
                for event in *events {
                    replay(event, ui);
                }
                if !ended(events) {
                    running(events.last(), ui);
                    // As a result's block ends, but the part's end gives the last its blank line.
                    if index + 1 < started.len() {
                        ui.blank_line();
                    }
                }
            }
        }
        Event::CommandStarted(command, include) => ui.command_started(command, *include),
        Event::CommandResult(ran) => ui.command_result(ran),
        Event::Stopped(error) => ui.stopped(error.as_deref()),
        Event::Compacted(compaction, automatic) => ui.compacted(compaction, *automatic),
        Event::CompactionFailed(error) => ui.compaction_error(error),
        Event::UserPrompt(prompt) => ui.user_prompt(prompt),
        Event::CommandPrompt(typed, sent) => ui.command_prompt(typed, sent),
        Event::CompactionSummary(summary) => ui.compaction_summary(summary),
        Event::BranchSummary(summary) => ui.branch_summary(summary),
        Event::Approval(question, answer) => ui.approval(question, *answer),
        Event::Question(message) => ui.question(message),
        Event::Notice(text) => ui.notice(text),
        Event::NextStep(record) => ui.next_step(record),
        Event::Text(text) => ui.said(text),
        Event::Styled(style, text) => {
            let text = ui.styled(*style, text);
            ui.said(&text)
        }
    }
}

/// How each call among `events` went, in the order `replay` shows them: done or failed once
/// its result came, with how long it ran, else still running.
pub(super) fn outcomes(events: &[Event]) -> Vec<(Outcome, Option<Duration>)> {
    let mut found = Vec::new();
    scan(events, &mut found);
    found
}

fn scan(events: &[Event], found: &mut Vec<(Outcome, Option<Duration>)>) {
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::ToolCall(..) => {
                let rest = &events[index + 1..];
                let end = (rest
                    .iter()
                    .position(|event| matches!(event, Event::ToolCall(..))))
                .unwrap_or(rest.len());
                let mut since = None;
                let mut outcome = (Outcome::Running, None);
                for event in &rest[..end] {
                    match event {
                        Event::ToolRunning(at) => since = Some(*at),
                        Event::ToolResult(_, _, _, failed, at) => {
                            let took = since.map(|since| at.saturating_duration_since(since));
                            outcome = match failed {
                                true => (Outcome::Failed, took),
                                false => (Outcome::Done, took),
                            };
                            break;
                        }
                        _ => {}
                    }
                }
                found.push(outcome);
            }
            Event::Calls(calls) => calls.iter().for_each(|call| scan(call, found)),
            _ => {}
        }
    }
}

/// Whether the events of a call include its result.
pub(super) fn ended(events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::ToolResult(..)))
}

/// What a tool or command still running shows, when `last` is its latest event: its output so
/// far, or its children.
pub(super) fn running(last: Option<&Event>, ui: &mut Terminal) {
    match last {
        Some(Event::ToolOutput(output)) => ui.running_output(output),
        Some(Event::CommandStarted(..)) => ui.running_output(""),
        Some(Event::Subagents(shown)) => ui.running_subagents(shown),
        _ => {}
    }
}
