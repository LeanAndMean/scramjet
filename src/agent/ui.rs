//! How the agent shows its work: what it reports to the interface, and a saved
//! conversation replayed as its turns showed.
use super::tools::Call;
use super::{
    AgentError, AutoCompaction, Choice, Compaction, DIFF, FAILED, PROBE, StreamProgress, TYPED,
    compaction, message_text, question, session,
};
use serde_json::Value;
use std::{sync::atomic::AtomicBool, time::Duration};

/// What a model request asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Purpose {
    /// The next step of a turn, whose text streams as progress.
    Reply,
    Compaction,
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
    /// `output` is what the model receives; `detail`, if any, is better shown to the user.
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool);
    /// The result of a command the user ran with `!` or `!!`.
    fn command_result(&mut self, output: &str, failed: bool);
    /// Shows a finished compaction; `automatic` says why the agent ran it without `/compact`.
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>);
    /// An automatic compaction before a prompt failed; the prompt is sent anyway.
    fn compaction_failed(&mut self, error: &AgentError);
    /// A prompt of a resumed conversation, shown as the user typed it.
    fn user_prompt(&mut self, prompt: &str);
    /// The summary of a compaction in a resumed conversation.
    fn compaction_summary(&mut self, summary: &str);
    /// The summary of a branch the conversation came back from, in a resumed conversation.
    fn branch_summary(&mut self, summary: &str);
    /// Asks whether a tool call may run; returns `Deny` without an answer once `cancel` is set.
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer;
    /// The markdown message of a question the model asks the user, shown before its options or,
    /// in a resumed conversation, before the answer.
    fn question(&mut self, message: &str);
    /// Something Scramjet would notify the user of, such as a command reported blocked.
    fn notice(&mut self, text: &str);
    /// Lets the user choose one of `options` under `title`, `recommended` selected at first;
    /// `None` when they cancel.
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize>;
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
    /// The subagents' token usage, shown under their result.
    Subagents(String),
}

/// Shows stored items as their turns showed them, as Scramjet redraws a resumed session: each
/// prompt, reply, reasoning summary, tool call with its result, `!` command and compaction summary.
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
                // A call whose arguments do not parse had no call line, only its error.
                if let Some(call) = &call {
                    ui.tool_call(call.name, &call.summary());
                }
                let output = item["output"].as_str().unwrap_or_default();
                let asked = call
                    .as_ref()
                    .filter(|call| call.name == question::NAME)
                    .and_then(|call| question::shown(&call.arguments, output));
                let detail = match (item[DIFF].as_str(), asked) {
                    (Some(diff), _) => Some(ToolDetail::Diff(diff.to_owned())),
                    (None, Some((message, answer))) => {
                        ui.question(&message);
                        Some(ToolDetail::Answer(answer))
                    }
                    // A write's preview is the content it was given.
                    (None, None) if name == "write" && !failed => call
                        .as_ref()
                        .and_then(|call| call.arguments["content"].as_str())
                        .map(|content| ToolDetail::Written(content.to_owned())),
                    (None, None) => None,
                };
                ui.tool_result(name, output, detail.as_ref(), failed);
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
                } else {
                    ui.user_prompt(session::prompt_text(item));
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
