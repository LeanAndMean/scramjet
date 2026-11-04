//! How the agent shows its work: what it reports to the interface, and a saved
//! conversation replayed as its turns showed.
use super::tools::Call;
use super::{
    AgentError, AutoCompaction, Choice, Compaction, DIFF, FAILED, PROBE, SUBAGENTS, StreamProgress,
    TYPED, compaction, message_text, publish, question, status,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{sync::atomic::AtomicBool, time::Duration};

/// What a model request asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Purpose {
    /// The next step of a turn, whose text streams as progress.
    Reply,
    /// A summary of older history: `/compact` (`None`) or automatic, saying why.
    Compaction(Option<AutoCompaction>),
    /// The summary of a branch that `/tree` leaves.
    BranchSummary,
}

/// How the agent presents progress and asks for approval.
pub trait AgentUi {
    /// A model request is sent.
    fn waiting(&mut self, purpose: Purpose);
    /// Text or reasoning summary as it streams; may never be called for a response.
    fn progress(&mut self, update: &StreamProgress);
    /// A model request failed transiently and will be sent again after `delay`.
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration);
    /// The complete text of an assistant message, after any streamed progress for it.
    fn assistant(&mut self, text: &str);
    fn tool_call(&mut self, name: &str, summary: &str);
    /// The tool call announced last starts running, after any approval.
    fn tool_running(&mut self);
    /// The end of the running tool's output so far.
    fn tool_output(&mut self, _output: &str) {}
    /// The children of the running `subagent` call, each time one starts, does something or
    /// ends.
    fn subagents(&mut self, _children: &Children) {}
    /// `output` is what the model receives; `detail`, if any, is better shown to the user.
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool);
    /// The result of a command the user ran with `!` or `!!`.
    fn command_result(&mut self, output: &str, failed: bool);
    /// A turn stopped before its end: cancelled (`None`), or failed with `error`.
    fn stopped(&mut self, error: Option<&str>);
    /// Shows a finished compaction; `automatic` says why the agent ran it without `/compact`.
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>);
    /// An automatic compaction before a prompt failed; the prompt is sent anyway.
    fn compaction_failed(&mut self, error: &AgentError);
    /// A prompt of a resumed conversation, shown as the user typed it.
    fn user_prompt(&mut self, prompt: &str);
    /// A command's prompt, as typed, and the text it sent in its place.
    fn command_prompt(&mut self, typed: &str, _sent: &str) {
        self.user_prompt(typed);
    }
    /// The summary of a compaction in a resumed conversation.
    fn compaction_summary(&mut self, summary: &str);
    /// The summary of a branch the conversation came back from, in a resumed conversation.
    fn branch_summary(&mut self, summary: &str);
    /// Asks whether a tool call may run; returns `Deny` without an answer once `cancel` is set.
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer;
    /// The markdown message of a question the model asks the user, or of a publication the user
    /// is asked to approve, shown before its options or, in a resumed conversation, before a
    /// question's answer.
    fn question(&mut self, message: &str);
    /// Something to tell the user, such as a command waiting for them.
    fn notice(&mut self, text: &str);
    /// Scramjet's record of the next step chosen after a command completed, or of the list
    /// dismissed, as `Agent::record_next_step` returns it.
    fn next_step(&mut self, record: &str);
    /// Messages the user sent while the turn runs, for the model before its next request
    /// (Scramjet's steering).
    fn steering(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// Messages the user queued for when the turn would end, for the model then instead of
    /// ending it (Scramjet's follow-up). Asked only when no steering is queued.
    fn follow_up(&mut self) -> Vec<String> {
        Vec::new()
    }
    /// An effort level the user chose while the turn runs, for the model's next request.
    fn effort(&mut self) -> Option<String> {
        None
    }
    /// Whether the user gave the session something to do with the agent while the turn runs,
    /// such as a command: the turn then returns `AgentError::Paused` before its next request,
    /// and `Agent::resume_turn` goes on with it.
    fn pause(&mut self) -> bool {
        false
    }
    /// Lets the user choose one of `options` under `title`, `recommended` selected at first;
    /// `None` when they cancel.
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize>;
    /// Asks whether to publish what `question` just showed, under `title` with
    /// `publication_choices`; `true` only when approved.
    fn approve_publication(&mut self, title: &str) -> bool {
        self.choose(title, &publication_choices(), None) == Some(0)
    }
}

/// Scramjet's choices for a publication: approve it, then cancel.
pub fn publication_choices() -> [Choice; 2] {
    ["Approve publication", "Cancel"].map(|label| Choice {
        label: label.into(),
        description: None,
        value: label.into(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Deny,
    Allow,
    /// Allow this call and every later one until the agent exits.
    AllowAll,
}

/// What the user sees of a successful tool call instead of the text the model receives.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolDetail {
    /// The lines an edit changed, each marked ` ` (unchanged), `-` or `+` and numbered, with
    /// `...` for skipped lines.
    Diff(String),
    /// The content a write put in the file.
    Written(String),
    /// What the user answered a question, as Scramjet's result row shows it.
    Answer(String),
    /// The children of a `subagent` call, as they ended.
    Subagents(Children),
    /// What a publication call proposed, each field under a markdown heading.
    Publication(String),
}

/// The children of a `subagent` call as the user sees them, as Scramjet's tool details keep
/// them: what each was given, how it went and what it did. Stored with the call's result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Children {
    pub mode: Delegation,
    /// In the order of the call's tasks; a chain's are added as its steps start.
    pub runs: Vec<ChildRun>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delegation {
    Single,
    Parallel,
    Chain,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChildRun {
    pub agent: String,
    /// Where the agent's file is from (`user`, `project`, a command set's `global`), or
    /// `unknown` when no agent has the name.
    pub source: String,
    /// Empty when no agent has the name.
    pub model: String,
    /// The thinking level it asks for, as Scramjet names it.
    pub effort: Option<String>,
    pub task: String,
    pub state: ChildState,
    /// Its replies' texts and tool calls, in order; the last text is its result.
    pub items: Vec<ChildItem>,
    pub usage: ChildUsage,
    /// What was said about the agent's file and the tools it did not get.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildState {
    /// Running, or waiting for its turn among the parallel tasks.
    Running,
    Done,
    /// Scramjet's stop reason (`error`, `aborted`), if any, and what went wrong.
    Failed {
        reason: Option<String>,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildItem {
    Text(String),
    /// A tool call's name and the one-line summary its call line shows.
    Call {
        name: String,
        summary: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChildUsage {
    /// Responses received.
    pub turns: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

impl ChildRun {
    /// The text of its last reply that had text.
    pub fn output(&self) -> Option<&str> {
        self.items.iter().rev().find_map(|item| match item {
            ChildItem::Text(text) => Some(text.as_str()),
            ChildItem::Call { .. } => None,
        })
    }
}

/// Shows stored items as their turns showed them, as Scramjet redraws a resumed session: each
/// prompt, reply, reasoning summary, tool call with its result, `!` and `!!` command, compaction
/// summary and how a turn stopped.
pub fn show_items(items: &[Value], ui: &mut dyn AgentUi) {
    // A reply's calls are stored before their results, but each ran, and showed, in turn.
    let mut calls: Vec<(&str, &str, Option<Call>)> = Vec::new();
    let unanswered = |calls: &mut Vec<(&str, &str, Option<Call>)>, ui: &mut dyn AgentUi| {
        for (_, _, call) in calls.drain(..) {
            if let Some(call) = call {
                ui.tool_call(call.name, &call.summary());
            }
        }
    };
    for item in items {
        let failed = item[FAILED] == true;
        if !matches!(
            item["type"].as_str(),
            Some("function_call" | "function_call_output")
        ) {
            unanswered(&mut calls, ui);
        }
        match item["type"].as_str() {
            Some("message") => ui.assistant(&message_text(item)),
            Some("reasoning") => {
                // Each part ends with a paragraph break, as it streamed.
                let parts = item["summary"].as_array().into_iter().flatten();
                for text in parts.filter_map(|part| part["text"].as_str()) {
                    if !text.trim().is_empty() {
                        ui.progress(&StreamProgress::Reasoning(text.to_owned()));
                        ui.progress(&StreamProgress::Reasoning("\n\n".into()));
                    }
                }
            }
            Some("function_call") => calls.push((
                item["call_id"].as_str().unwrap_or_default(),
                item["name"].as_str().unwrap_or_default(),
                Call::parse(item).ok(),
            )),
            Some("function_call_output") => {
                let id = item["call_id"].as_str().unwrap_or_default();
                let (name, call) = match calls.iter().position(|(call_id, ..)| *call_id == id) {
                    Some(index) => {
                        let (_, name, call) = calls.remove(index);
                        (name, call)
                    }
                    None => ("", None),
                };
                let output = item["output"].as_str().unwrap_or_default();
                if name == status::RECORD && !failed {
                    ui.next_step(output);
                    continue;
                }
                // A call whose arguments do not parse had no call line, only its error.
                if let Some(call) = &call {
                    ui.tool_call(call.name, &call.summary());
                }
                let asked = call
                    .as_ref()
                    .filter(|call| call.name == question::NAME)
                    .and_then(|call| question::shown(&call.arguments, output));
                let children = item.get(SUBAGENTS).cloned();
                let children = children.and_then(|value| serde_json::from_value(value).ok());
                let detail = match (item[DIFF].as_str(), asked, children) {
                    (Some(diff), ..) => Some(ToolDetail::Diff(diff.to_owned())),
                    (None, None, Some(children)) => Some(ToolDetail::Subagents(children)),
                    (None, Some((message, answer)), _) => {
                        ui.question(&message);
                        Some(ToolDetail::Answer(answer))
                    }
                    // A write's preview is the content it was given.
                    (None, None, None) if name == "write" && !failed => call
                        .as_ref()
                        .and_then(|call| call.arguments["content"].as_str())
                        .map(|content| ToolDetail::Written(content.to_owned())),
                    (None, None, None) if publish::TOOLS.contains(&name) => call
                        .as_ref()
                        .and_then(|call| publish::payload(name, &call.arguments)),
                    (None, None, None) => None,
                };
                ui.tool_result(name, output, detail.as_ref(), failed);
            }
            Some("turn_stopped") => ui.stopped(item["error"].as_str()),
            Some("bang_excluded") => {
                ui.tool_call("bash", item["command"].as_str().unwrap_or_default());
                ui.command_result(item["output"].as_str().unwrap_or_default(), failed);
            }
            // Scramjet's status check is not shown, as the user never typed it.
            _ if item.get(PROBE).is_some() => {}
            _ if item["role"] == "user" => {
                let content = item["content"].as_str().unwrap_or_default();
                let command = item[TYPED]
                    .as_str()
                    .and_then(|typed| typed.strip_prefix('!'));
                if let Some(summary) = compaction::previous_summary(std::slice::from_ref(item)) {
                    ui.compaction_summary(summary);
                } else if let Some(summary) = compaction::branch_summary(item) {
                    ui.branch_summary(summary);
                } else if let Some(command) = command {
                    ui.tool_call("bash", command);
                    let output = content
                        .strip_prefix(&format!("Ran `{command}`\n"))
                        .map_or_else(|| content.to_owned(), command_output);
                    ui.command_result(&output, failed);
                } else if let Some(typed) = item[TYPED].as_str() {
                    ui.command_prompt(typed, content);
                } else {
                    ui.user_prompt(content);
                }
            }
            _ => {}
        }
    }
    unanswered(&mut calls, ui);
}

/// The output of a `!` command from its stored message, which fences the output and ends with
/// how the command ended, as the live result shows it.
fn command_output(message: &str) -> String {
    match message
        .strip_prefix("```\n")
        .and_then(|rest| rest.rsplit_once("\n```"))
    {
        Some((output, ending)) => format!("{output}{ending}"),
        None => message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::agent::tests::*;
    use crate::provider_fixture as fixture;
    use serde_json::json;

    #[test]
    fn resumed_conversation_is_shown_as_its_turns_ran() {
        let call = |id: &str, name: &str, arguments: Value| json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments.to_string()});
        let output = |id: &str, output: &str| json!({"type":"function_call_output","call_id":id,"output":output});
        let summary = |text: &str| json!({"type":"summary_text","text":text});
        let items = [
            compaction::summary_message("old work"),
            json!({"role":"user","content":"fix it"}),
            json!({"type":"reasoning","summary":[summary("look"), summary("then fix")],"encrypted_content":"x"}),
            json!({"type":"reasoning","summary":[],"encrypted_content":"y"}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Reading."}]}),
            call("c1", "read", json!({"path":"a.rs"})),
            output("c1", "fn main() {}"),
            call("c2", "edit", json!({"path":"a.rs","edits":[]})),
            json!({"type":"function_call_output","call_id":"c2","output":"Edited","_scramjet_diff":"-1 a"}),
            call("c3", "write", json!({"path":"b.rs","content":"b\n"})),
            output("c3", "Wrote"),
            json!({"role":"user","content":"Ran `ls`\n```\na.rs\n```\n\nCommand exited with code 2",
                "_scramjet_typed":"!ls","_scramjet_failed":true}),
            json!({"type":"function_call","call_id":"c4","name":"bash","arguments":"not json"}),
            json!({"type":"function_call_output","call_id":"c4","output":"Error: invalid","_scramjet_failed":true}),
            call("c5", "write", json!({"path":"/","content":"c"})),
            json!({"type":"function_call_output","call_id":"c5","output":"Error: dir","_scramjet_failed":true}),
            // Claude calls two tools in one reply; they ran one after the other.
            call("c6", "bash", json!({"command":"one"})),
            call("c7", "bash", json!({"command":"two"})),
            output("c6", "1"),
            output("c7", "2"),
            call(
                "c9",
                "add_issue_comment",
                json!({"number":3,"body":"Me too."}),
            ),
            output("c9", "Cancelled."),
            call("c10", "add_issue_comment", json!({"number":0,"body":"No."})),
            json!({"type":"function_call_output","call_id":"c10","output":"Invalid","_scramjet_failed":true}),
            // Its turn cancelled, this call has no result here.
            call("c8", "read", json!({"path":"x"})),
            json!({"role":"user","content":"next"}),
        ];
        let mut recorder = Recorder::default();
        show_items(&items, &mut recorder);
        assert_eq!(
            recorder.shown,
            [
                "summary old work",
                "prompt fix it",
                "thinking look",
                "thinking \n\n",
                "thinking then fix",
                "thinking \n\n",
                "reply Reading.",
                "call read a.rs",
                "result read fn main() {}",
                "call edit a.rs",
                "result edit Edited Diff(\"-1 a\")",
                "call write b.rs",
                "result write Wrote Written(\"b\\n\")",
                "call bash ls",
                "command a.rs\n\nCommand exited with code 2 failed",
                "result bash Error: invalid failed",
                "call write /",
                "result write Error: dir failed",
                "call bash one",
                "result bash 1",
                "call bash two",
                "result bash 2",
                "call add_issue_comment #3",
                "result add_issue_comment Cancelled. Publication(\"## Target\\n\\n#3\\n\\n## \
Comment\\n\\nMe too.\")",
                "call add_issue_comment #0",
                "result add_issue_comment Invalid failed",
                "call read x",
                "prompt next",
            ]
        );
        assert_eq!(command_output("(no output)"), "(no output)");
        assert_eq!(command_output("```\n\n```"), "");
    }

    #[test]
    fn results_keep_what_was_shown_for_a_resume_but_not_for_the_model() {
        let call = |id: &str, name: &str, arguments: Value| {
            let events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text
                .replace("\"inert\"", &format!("\"{name}\""))
                .replace("call_1", id);
            stream(&serde_json::from_str::<Vec<Value>>(&text).unwrap())
        };
        let (endpoint, server) = serve(vec![
            call(
                "call_1",
                "edit",
                json!({"path":"a.txt","edits":[{"oldText":"one","newText":"two"}]}),
            ),
            call("call_2", "bash", json!({"command":"exit 2"})),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "shown-results");
        std::fs::write(agent.root().join("a.txt"), "one\n").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("change it", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        assert!(
            !bodies[2].to_string().contains("\"_scramjet_"),
            "{}",
            bodies[2]
        );
        let (_, resumed, _) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, agent.conversation());
        let mut replay = Recorder::default();
        agent.show_conversation(&mut replay);
        let results: Vec<_> = replay
            .shown
            .iter()
            .filter(|shown| shown.starts_with("result "))
            .collect();
        let edited = format!(
            "result edit Applied 1 edit(s) to {} Diff(\"-1 one\\n+1 two\\n\")",
            agent.root().join("a.txt").display()
        );
        assert_eq!(
            results,
            [&edited, "result bash Command exited with code 2 failed"]
        );
    }
}
