//! The agent's display calls, kept as events: the full screen keeps the conversation so, and
//! renders it again from them at its width.

use super::terminal::Terminal;
use scramjet_rs::agent::{
    AgentUi, Answer, AutoCompaction, Children, Compaction, Purpose, StreamProgress, ToolDetail,
    UserCommand,
};
use std::time::{Duration, Instant};

/// A display call, and what the session says between turns (`Text`).
pub(super) enum Event {
    Waiting(Purpose),
    Progress(StreamProgress),
    Retrying(String, u32, u32, Duration),
    Assistant(String),
    ToolCall(String, String),
    /// Since when.
    ToolRunning(Instant),
    ToolOutput(String),
    /// The children of a running `subagent` call, as they are now.
    Subagents(Children),
    ToolResult(String, String, Option<ToolDetail>, bool),
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
        Event::ToolRunning(since) => ui.tool_running(*since),
        Event::ToolOutput(_) | Event::Subagents(_) => {}
        Event::ToolResult(name, output, detail, failed) => {
            ui.tool_result(name, output, detail.as_ref(), *failed)
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
    }
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
