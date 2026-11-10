//! The session as a tree: the entries `/tree` lists, and continuing the conversation from
//! one of them.
use super::calls::{self, Calls};
use super::status::{self, Reopened};
use super::tools::Call;
use super::{
    Agent, AgentError, AgentUi, Label, Purpose, TYPED, compaction, message_text, question, session,
    suggest,
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

/// Scramjet's cap on a branch summary's output.
const BRANCH_SUMMARY_TOKENS: u64 = 2048;

/// An entry of the session as `/tree` lists it.
#[derive(Debug, PartialEq)]
pub struct TreeEntry {
    /// Its line in the session file, which `Agent::navigate` takes.
    pub line: usize,
    /// The nearest listed entry it continues; `None` at the start of a conversation.
    pub parent: Option<usize>,
    pub item: TreeItem,
    /// Whether it is on the path to the current entry.
    pub active: bool,
    pub label: Option<Label>,
}

#[derive(Debug, PartialEq)]
pub enum TreeItem {
    Prompt(String),
    /// A `!` or `!!` command's text.
    Command(String),
    Reply(String),
    /// Where a turn stopped: cancelled (`None`), or failed with the error.
    Stopped(Option<String>),
    /// A tool call's result, by the call's name and its call line's summary.
    Tool(String, String),
    /// A tool call, listed only when it is the current entry, as `Reasoning` is.
    Call(String, String),
    Compaction(Option<u64>),
    BranchSummary(String),
    Reasoning,
    /// Changes of the model and the effort, which Scramjet's `/tree` shows in its `all` view.
    Model(String),
    Effort(String),
}

/// The session's entries as `/tree` lists them, in file order.
pub struct SessionTree {
    pub entries: Vec<TreeEntry>,
    /// The listed entry nearest the current one, on its path.
    pub current: Option<usize>,
}

/// Where continuing from an entry of the session tree ends the conversation.
struct Destination {
    leaf: Option<usize>,
    /// The prompt chosen, to be edited and sent again.
    prompt: Option<String>,
    /// The entries of the branch it leaves.
    left: Vec<Value>,
    /// The next-step list offered there.
    offer: Option<Reopened>,
    /// A question to ask again there.
    question: Option<Asked>,
}

/// A select or confirm question gone back to, with the calls after it in its reply.
struct Asked {
    /// The question's call and those after it.
    calls: Vec<Value>,
    /// Whether every call before it ended the turn.
    ends: bool,
    /// The results the calls after it had, by call id, but those of the harness's report and
    /// suggestion, which run again for the turn's end to see them, and of calls that never ran.
    kept: std::collections::HashMap<String, Value>,
}

/// Where `Agent::navigate` left the conversation.
#[derive(Debug, PartialEq)]
pub enum Navigated {
    /// The conversation already ended at that entry.
    AlreadyThere,
    /// It now ends there; `prompt`, if the entry was one, is to be edited and sent again, and
    /// `offer` is the next-step list offered there, to be offered again. With `question`, the
    /// question gone back to is asked again by `Agent::resume_turn`, which goes on with the turn.
    Moved {
        prompt: Option<String>,
        offer: Option<Reopened>,
        question: bool,
    },
}

impl Agent {
    /// The current session's entries, as Scramjet's `/tree` lists them in its `all` view, which
    /// its filters choose from: prompts, replies with text, where turns stopped, tool results,
    /// `!` and `!!` commands, compactions, branch summaries, model and effort changes, and the
    /// current entry whatever it is.
    pub fn tree(&self) -> Result<SessionTree, AgentError> {
        let (entries, mut labels) = self.session.labeled_entries()?;
        let leaf = self.session.leaf();
        let mut calls = std::collections::HashMap::new();
        for entry in &entries {
            if entry.value["type"] == "function_call"
                && let (Some(id), Ok(call)) =
                    (entry.value["call_id"].as_str(), Call::parse(&entry.value))
            {
                calls.insert(id, (call.name.to_owned(), call.summary()));
            }
        }
        let call = |value: &Value| {
            value["call_id"]
                .as_str()
                .and_then(|id| calls.get(id))
                .cloned()
                .unwrap_or_else(|| {
                    (
                        value["name"].as_str().unwrap_or("tool").to_owned(),
                        String::new(),
                    )
                })
        };
        let mut active = std::collections::HashSet::new();
        let parents: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|entry| (entry.line, entry.parent))
            .collect();
        let mut at = leaf;
        while let Some(line) = at {
            active.insert(line);
            at = parents.get(&line).copied().flatten();
        }
        let mut listed = std::collections::HashSet::new();
        let mut tree = Vec::new();
        for entry in &entries {
            let value = &entry.value;
            let is_leaf = leaf == Some(entry.line);
            let item = match value["type"].as_str() {
                Some("message") => Some(TreeItem::Reply(message_text(value))),
                Some("function_call_output") => {
                    let (name, summary) = call(value);
                    Some(TreeItem::Tool(name, summary))
                }
                Some("turn_stopped") => Some(TreeItem::Stopped(
                    value["error"].as_str().map(str::to_owned),
                )),
                Some("bang_excluded") => Some(TreeItem::Command(
                    value["command"].as_str().unwrap_or_default().to_owned(),
                )),
                Some("compaction") => Some(TreeItem::Compaction(value["tokens_before"].as_u64())),
                Some("branch_summary") => Some(TreeItem::BranchSummary(
                    value["summary"].as_str().unwrap_or_default().to_owned(),
                )),
                Some("function_call") if is_leaf => {
                    let (name, summary) = call(value);
                    Some(TreeItem::Call(name, summary))
                }
                Some("reasoning") if is_leaf => Some(TreeItem::Reasoning),
                Some("model_change") => Some(TreeItem::Model(
                    value["model"].as_str().unwrap_or_default().to_owned(),
                )),
                Some("thinking_level_change") => Some(TreeItem::Effort(
                    value["level"].as_str().unwrap_or_default().to_owned(),
                )),
                _ if session::is_prompt(value) => {
                    Some(TreeItem::Prompt(session::prompt_text(value).to_owned()))
                }
                _ if value["role"] == "user" => value[TYPED]
                    .as_str()
                    .and_then(|typed| typed.strip_prefix('!'))
                    .map(|command| TreeItem::Command(command.to_owned())),
                _ => None,
            };
            let Some(item) = item else {
                continue;
            };
            let mut parent = entry.parent;
            while let Some(line) = parent.filter(|line| !listed.contains(line)) {
                parent = parents.get(&line).copied().flatten();
            }
            listed.insert(entry.line);
            tree.push(TreeEntry {
                line: entry.line,
                parent,
                item,
                active: active.contains(&entry.line),
                label: labels.remove(&entry.line),
            });
        }
        let mut current = leaf;
        while let Some(line) = current.filter(|line| !listed.contains(line)) {
            current = parents.get(&line).copied().flatten();
        }
        Ok(SessionTree {
            entries: tree,
            current,
        })
    }

    /// Gives the session's entry on `line` the label `text`, or takes its label away (`None` or
    /// empty), as Scramjet's tree labels entries. Returns the label given.
    pub fn set_label(
        &mut self,
        line: usize,
        text: Option<&str>,
    ) -> Result<Option<Label>, AgentError> {
        Ok(self.session.set_label(line, text)?)
    }

    /// Whether continuing from the session's entry on `line` leaves a branch with anything for
    /// a summary to tell.
    pub fn leaves_branch(&self, line: usize) -> Result<bool, AgentError> {
        Ok(self
            .destination(line)?
            .is_some_and(|destination| !compaction::Branch::of(&destination.left).is_empty()))
    }

    /// Continues the conversation from the session's entry on `line`, in the same session, as
    /// Scramjet's `/tree` does: from before it if it is a prompt, which is returned to be edited
    /// and sent again, otherwise from after it. The model stays as it is. With `summary`, a
    /// branch left behind is summarized there first, as Scramjet's branch summary, with
    /// `summary` as an additional focus unless empty. On failure or cancellation nothing changes.
    pub fn navigate(
        &mut self,
        line: usize,
        summary: Option<&str>,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Navigated, AgentError> {
        let Some(destination) = self.destination(line)? else {
            return Ok(Navigated::AlreadyThere);
        };
        let branch = compaction::Branch::of(&destination.left);
        let mut summary = match summary {
            Some(focus) if !branch.is_empty() => {
                Some(self.summarize_branch(&branch, focus, ui, cancel)?)
            }
            _ => None,
        };
        self.conversation = self.session.move_to(destination.leaf)?;
        self.usage.context_tokens = self.session.context_tokens();
        self.autopilot = self.session.autopilot().unwrap_or(self.autopilot);
        self.restore_command();
        let question = destination.question.is_some();
        self.calls = destination.question.map(|asked| {
            let again = calls::Again {
                kept: asked.kept,
                summary: summary.take(),
            };
            Calls::again(asked.calls, asked.ends, again)
        });
        if question {
            self.suggestion = suggest::Pending::Nothing;
            self.delegated.clear();
        }
        self.answer_interrupted_calls()?;
        if let Some((summary, files)) = summary {
            self.add_branch_summary(&summary, &files)?;
        }
        Ok(Navigated::Moved {
            prompt: destination.prompt,
            offer: destination.offer,
            question,
        })
    }

    /// Adds the summary of a branch left behind.
    pub(super) fn add_branch_summary(
        &mut self,
        summary: &str,
        files: &compaction::FileLists,
    ) -> Result<(), AgentError> {
        self.session
            .append(&json!({"type":"branch_summary", "summary":summary,
            "read_files":files.read, "modified_files":files.modified}))?;
        self.conversation
            .push(compaction::branch_summary_message(summary));
        Ok(())
    }

    /// Where continuing from the entry on `line` ends the conversation; `None` if it already
    /// ends there. Where a next-step list was offered, it ends before the list's record: at the
    /// record itself, at the prompt its choice sent, and after the report or the call that
    /// offered it, so that the list is offered again.
    fn destination(&self, line: usize) -> Result<Option<Destination>, AgentError> {
        let entries = self.session.entries()?;
        let at =
            |line: Option<usize>| line.and_then(|line| entries.iter().find(|e| e.line == line));
        let Some(entry) = at(Some(line)) else {
            return Err(AgentError::Session(std::io::Error::other(
                "no such entry in the session",
            )));
        };
        let record_output = |output: &session::Entry| is_record_output(&entries, output);
        if let Some(asked) = asked_at(&entries, entry) {
            return Ok(Some(Destination {
                leaf: entry.parent,
                prompt: None,
                left: self.session.left_behind(entry.parent)?,
                offer: None,
                question: Some(asked),
            }));
        }
        if self.session.leaf() == Some(line) && !record_output(entry) {
            return Ok(None);
        }
        let (mut leaf, prompt) = if session::is_prompt(&entry.value) {
            let prompt = session::prompt_text(&entry.value);
            (entry.parent, Some(prompt.to_owned()))
        } else {
            (Some(line), None)
        };
        // The results of one reply's calls continue one another; ending at one before the last
        // would leave a call without its result.
        if entry.value["type"] == "function_call_output" {
            while let Some(next) = entries
                .iter()
                .find(|next| next.parent == leaf && next.value["type"] == "function_call_output")
            {
                leaf = Some(next.line);
            }
        }
        // The command's state and the `/autopilot` toggles saved after an entry are the state
        // there; `/tree` does not list them.
        if prompt.is_none() {
            let state =
                |value: &Value| value["type"] == "command_state" || value["type"] == "autopilot";
            while let Some(next) =
                (entries.iter()).find(|next| next.parent == leaf && state(&next.value))
            {
                leaf = Some(next.line);
            }
        }
        // As in Scramjet, a prompt chosen to be sent again is not part of the branch left.
        let kept = if prompt.is_some() { Some(line) } else { leaf };
        let (leaf, offer) = before_offer(&entries, leaf);
        if leaf == self.session.leaf() && offer.is_none() {
            return Ok(None);
        }
        Ok(Some(Destination {
            leaf,
            prompt,
            left: self.session.left_behind(kept)?,
            offer,
            question: None,
        }))
    }

    /// Scramjet's branch summary of `branch` (its `generateBranchSummary`), and the files read
    /// and modified there.
    fn summarize_branch(
        &mut self,
        branch: &compaction::Branch,
        focus: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(String, compaction::FileLists), AgentError> {
        let budget = self
            .info
            .window
            .unwrap_or(128_000)
            .saturating_sub(self.config.compacting.branch_reserve)
            .max(1);
        let files = branch.files();
        let summary = match branch.prompt(budget, focus) {
            Some(prompt) => {
                let text = self.summarize(
                    prompt,
                    Purpose::BranchSummary,
                    BRANCH_SUMMARY_TOKENS,
                    ui,
                    cancel,
                )?;
                compaction::branch_summary_text(&text, &files)
            }
            None => "No content to summarize".to_owned(),
        };
        Ok((summary, files))
    }
}

/// Whether `entry` is the result of a next-step list's record.
fn is_record_output(entries: &[session::Entry], entry: &session::Entry) -> bool {
    entry.value["type"] == "function_call_output"
        && (entries.iter())
            .any(|call| Some(call.line) == entry.parent && call.value["name"] == status::RECORD)
}

/// Where a conversation ending at `leaf` ends instead for the next-step list offered there to
/// be offered again: before the list's record when `leaf` is its result. Returns that leaf and
/// the list, if one was offered there.
pub(super) fn before_offer(
    entries: &[session::Entry],
    mut leaf: Option<usize>,
) -> (Option<usize>, Option<Reopened>) {
    let at = |line: Option<usize>| line.and_then(|line| entries.iter().find(|e| e.line == line));
    if let Some(output) = at(leaf).filter(|output| is_record_output(entries, output)) {
        leaf = at(output.parent).and_then(|call| call.parent);
    }
    let offer = (entries.iter().rev())
        .filter(|next| next.parent == leaf && next.value["type"] == "function_call")
        .find(|next| next.value["name"] == status::RECORD)
        .and_then(|call| serde_json::from_str(call.value["arguments"].as_str()?).ok())
        .and_then(|arguments| Reopened::of(&arguments));
    (leaf, offer)
}

/// The question to ask again where `entry` is its result: a select or confirm question, with
/// the calls of its reply after it; `None` for any other entry.
fn asked_at(entries: &[session::Entry], entry: &session::Entry) -> Option<Asked> {
    if entry.value["type"] != "function_call_output" {
        return None;
    }
    let id = entry.value["call_id"].as_str()?;
    let at = |line: Option<usize>| line.and_then(|line| entries.iter().find(|e| e.line == line));
    // A reply's calls come one after another, then their results in the same order.
    let mut above = entry.parent;
    let mut earlier = Vec::new();
    while let Some(output) = at(above).filter(|e| e.value["type"] == "function_call_output") {
        earlier.push(&output.value);
        above = output.parent;
    }
    let mut reply = Vec::new();
    while let Some(call) = at(above).filter(|e| e.value["type"] == "function_call") {
        reply.push(&call.value);
        above = call.parent;
    }
    reply.reverse();
    let position = reply.iter().position(|call| call["call_id"] == id)?;
    let call = Call::parse(reply[position]).ok()?;
    let kind = question::Question::parse(&call.arguments).ok()?.kind;
    if call.name != question::NAME || kind == question::Kind::Freetext {
        return None;
    }
    let ends = earlier.iter().all(|output| {
        let id = &output["call_id"];
        (reply.iter()).any(|call| call["call_id"] == *id && calls::ended(call, output))
    });
    let mut kept = std::collections::HashMap::new();
    let mut answered = std::collections::HashSet::from([id]);
    let mut next = Some(entry.line);
    while let Some(output) =
        (entries.iter()).find(|e| e.parent == next && e.value["type"] == "function_call_output")
    {
        next = Some(output.line);
        let id = output.value["call_id"].as_str().unwrap_or_default();
        let call = reply.iter().find(|call| call["call_id"] == id);
        let runs_again = call.is_none_or(|call| {
            [status::NAME, suggest::NAME].contains(&call["name"].as_str().unwrap_or_default())
        });
        if !runs_again && output.value["output"] != calls::NOT_RUN {
            kept.insert(id.to_owned(), output.value.clone());
        }
        answered.insert(id);
    }
    // A call after it whose result was never saved may or may not have taken effect.
    for call in &reply[position + 1..] {
        let id = call["call_id"].as_str().unwrap_or_default();
        let name = call["name"].as_str().unwrap_or_default();
        if !answered.contains(id) && ![status::NAME, suggest::NAME].contains(&name) {
            let output = json!({"type":"function_call_output","call_id":id,
                "output":super::INTERRUPTED,super::FAILED:true});
            kept.insert(id.to_owned(), output);
        }
    }
    Some(Asked {
        calls: reply[position..]
            .iter()
            .map(|call| (*call).clone())
            .collect(),
        ends,
        kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{Session, delegate, show_items};
    use crate::provider_fixture as fixture;

    #[test]
    fn going_back_to_where_a_next_step_list_was_offered_offers_it_again() {
        let replies = ["Done.", "Looped.", "Steered."];
        let (endpoint, _server) = serve(
            replies
                .iter()
                .map(|text| stream(&fixture::text_events(text, fixture::known_usage())))
                .collect(),
        );
        let (mut agent, _scratch) = agent(endpoint, "tree-reopen");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("work", &mut ui, &cancel).unwrap();
        let step = |index, message: &str| status::Step {
            index,
            message: message.into(),
            reason: "Because.".into(),
            fresh_session: false,
            command: Some(message.into()),
        };
        let steps = vec![step(0, "/rs:loop"), step(2, "/rs:steer")];
        let source = status::Source::Completion("rs:work");
        agent
            .record_next_step(source, &steps, Some(1), Some(0))
            .unwrap();
        agent.turn("/rs:loop", &mut ui, &cancel).unwrap();
        let offered = Some(Reopened {
            command: Some("rs:work".into()),
            steps: steps.clone(),
            recommended: Some(1),
        });
        let tree = agent.tree().unwrap();
        let line = |wanted: &dyn Fn(&TreeItem) -> bool| {
            (tree.entries.iter())
                .find(|entry| wanted(&entry.item))
                .unwrap()
                .line
        };
        let reply = line(&|item| *item == TreeItem::Reply("Done.".into()));
        let record =
            line(&|item| matches!(item, TreeItem::Tool(name, _) if name == status::RECORD));
        let chosen = line(&|item| *item == TreeItem::Prompt("/rs:loop".into()));

        // The prompt the choice sent, and the record itself, go back to before the record, which
        // the conversation no longer holds, and offer its list again.
        assert_eq!(
            agent.navigate(chosen, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: Some("/rs:loop".into()),
                offer: offered.clone(),
                question: false,
            }
        );
        assert_eq!(agent.conversation().len(), 2);
        assert!(!agent.leaves_branch(record).unwrap());
        assert_eq!(
            agent.navigate(record, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: None,
                offer: offered.clone(),
                question: false,
            }
        );
        // A choice made there branches from there.
        agent
            .record_next_step(source, &steps, Some(1), Some(1))
            .unwrap();
        agent.turn("/rs:steer", &mut ui, &cancel).unwrap();
        let conversation = agent.conversation();
        assert_eq!(conversation.len(), 6);
        assert!(
            conversation[3]["output"]
                .as_str()
                .unwrap()
                .contains("→ /rs:steer")
        );

        // From after the reply that offered it, it is offered again; so is a list dismissed,
        // from right after it.
        assert_eq!(
            agent.navigate(reply, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: None,
                offer: offered.clone(),
                question: false,
            }
        );
        agent
            .record_next_step(source, &steps, Some(1), None)
            .unwrap();
        let dismissed = agent.session.leaf().unwrap();
        let Navigated::Moved { offer, .. } =
            agent.navigate(dismissed, None, &mut ui, &cancel).unwrap()
        else {
            panic!("not moved");
        };
        assert_eq!(offer, offered);
        assert_eq!(agent.conversation().len(), 2);
    }

    #[test]
    fn forking_before_a_prompt_a_next_step_sent_leaves_the_record_out_and_offers_its_list() {
        let replies = ["Done.", "Looped."];
        let (endpoint, _server) = serve(
            replies
                .iter()
                .map(|text| stream(&fixture::text_events(text, fixture::known_usage())))
                .collect(),
        );
        let (mut agent, _scratch) = agent(endpoint, "fork-reopen");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("work", &mut ui, &cancel).unwrap();
        let steps = vec![status::Step {
            index: 0,
            message: "/rs:loop".into(),
            reason: "Because.".into(),
            fresh_session: false,
            command: Some("/rs:loop".into()),
        }];
        let source = status::Source::Completion("rs:work");
        agent
            .record_next_step(source, &steps, Some(0), Some(0))
            .unwrap();
        agent.turn("/rs:loop", &mut ui, &cancel).unwrap();
        let original = agent.session.path().to_owned();

        let offer = Some(Reopened {
            command: Some("rs:work".into()),
            steps,
            recommended: Some(0),
        });
        assert_eq!(agent.fork(1).unwrap(), Some(("/rs:loop".into(), offer)));
        assert_ne!(agent.session.path(), original);
        let conversation = agent.conversation();
        assert_eq!(conversation.len(), 2);
        assert!(
            conversation
                .iter()
                .all(|item| item["name"] != status::RECORD)
        );
        // Forking before the first prompt offers nothing.
        assert_eq!(agent.fork(0).unwrap(), Some(("work".into(), None)));
    }

    #[test]
    fn going_back_to_a_question_asks_it_again_and_keeps_the_other_calls_results() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let question = json!({"type":"select","message":"Which?",
            "options":[{"value":"a","label":"A"},{"value":"b","label":"B"}]});
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                (question::NAME, question.clone()),
                ("bash", json!({"command":"echo ran >> log"})),
            ]),
            text("Chose a."),
            text("Chose b."),
            text("## Goal\nsummary"),
            text("Cancelled."),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "tree-question");
        let cancel = AtomicBool::new(false);
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        agent.turn("ask", &mut ui, &cancel).unwrap();
        let asked = |agent: &Agent| {
            let tree = agent.tree().unwrap();
            let answers = (tree.entries.iter()).filter(
                |entry| matches!(&entry.item, TreeItem::Tool(name, _) if name == question::NAME),
            );
            answers.map(|entry| entry.line).collect::<Vec<_>>()
        };
        let first = asked(&agent)[0];
        let moved = agent.navigate(first, None, &mut ui, &cancel).unwrap();
        let again = Navigated::Moved {
            prompt: None,
            offer: None,
            question: true,
        };
        assert_eq!(moved, again);
        // Neither call has a result until the question is answered again.
        let last = agent.conversation().last().unwrap();
        assert_eq!(
            (last["type"].as_str(), last["name"].as_str()),
            (Some("function_call"), Some("bash"))
        );
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        agent.resume_turn(&mut ui, &cancel).unwrap();
        assert!(
            ui.shown
                .contains(&"choose Choose an option [\"A\", \"B\"] None".to_owned())
        );
        assert!(
            ui.shown
                .iter()
                .any(|shown| shown.starts_with("result bash"))
        );
        // The bash call ran once, with the first answer.
        let log = std::fs::read_to_string(agent.root().join("log")).unwrap();
        assert_eq!(log, "ran\n");

        // With a summary of the branch left, it comes after the results.
        let second = *asked(&agent).last().unwrap();
        assert_ne!(second, first);
        let moved = agent.navigate(second, Some(""), &mut ui, &cancel).unwrap();
        assert_eq!(moved, again);
        let mut ui = Recorder::default();
        agent.resume_turn(&mut ui, &cancel).unwrap();
        assert_eq!(
            std::fs::read_to_string(agent.root().join("log")).unwrap(),
            "ran\n"
        );

        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 5);
        let answers = |body: &Value| {
            outputs(body)
                .into_iter()
                .map(|(_, text)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(answers(&bodies[1])[0], r#"{"selected":"a"}"#);
        assert_eq!(
            answers(&bodies[2]),
            answers(&bodies[1])
                .iter()
                .enumerate()
                .map(|(at, text)| match at {
                    0 => r#"{"selected":"b"}"#.to_owned(),
                    _ => text.clone(),
                })
                .collect::<Vec<_>>()
        );
        let input = bodies[4]["input"].as_array().unwrap();
        assert_eq!(answers(&bodies[4])[0], r#"{"cancelled":true}"#);
        assert_eq!(answers(&bodies[4])[1], answers(&bodies[1])[1]);
        let summary = input.last().unwrap()["content"].as_str().unwrap();
        assert!(summary.contains("## Goal\nsummary"), "{summary}");
        assert_eq!(input[input.len() - 2]["type"], "function_call_output");
    }

    #[test]
    fn a_freetext_question_kept_after_one_asked_again_leaves_the_command_waiting() {
        let select = json!({"type":"select","message":"Which?",
            "options":[{"value":"a","label":"A"},{"value":"b","label":"B"}]});
        let freetext = json!({"type":"freetext","message":"Why?"});
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                (question::NAME, select),
                (delegate::NAME, json!({"command":"rs:work","args":"x"})),
                (question::NAME, freetext),
            ]),
            text("Waiting."),
            text("Waiting again."),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "tree-freetext");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("work.md"), "Work on $1.\n").unwrap();
        project_sets(&mut agent);
        agent.autonomy = None;
        let cancel = AtomicBool::new(false);
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        agent.turn("/rs:work go", &mut ui, &cancel).unwrap();
        let tree = agent.tree().unwrap();
        let first = (tree.entries.iter())
            .find(|entry| matches!(&entry.item, TreeItem::Tool(name, _) if name == question::NAME))
            .unwrap()
            .line;
        agent.navigate(first, None, &mut ui, &cancel).unwrap();
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        agent.resume_turn(&mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), Some("rs:work"));
        assert_eq!(agent.delegated, [("rs:work".to_owned(), "x".to_owned())]);
        // No status check follows, as the freetext question left the command waiting.
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 3);
        let answers: Vec<_> = (outputs(&bodies[2]).into_iter())
            .map(|(_, text)| text)
            .collect();
        assert_eq!(answers[0], r#"{"selected":"b"}"#);
        assert_eq!(answers[2], r#"{"parked":true}"#);
    }

    #[test]
    fn tree_navigation_branches_the_session_in_place() {
        let replies = ["A", "B", "C", "D", "E"];
        let (endpoint, server) = serve(
            replies
                .iter()
                .map(|text| stream(&fixture::text_events(text, fixture::known_usage())))
                .collect(),
        );
        let (mut agent, _scratch) = agent(endpoint, "tree");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        agent.turn("two", &mut ui, &cancel).unwrap();
        let shape = |agent: &Agent| {
            let tree = agent.tree().unwrap();
            let entries: Vec<_> = tree
                .entries
                .into_iter()
                .map(|entry| {
                    let text = match entry.item {
                        TreeItem::Prompt(text) | TreeItem::Reply(text) => text,
                        item => format!("{item:?}"),
                    };
                    (entry.line, entry.parent, text, entry.active)
                })
                .collect();
            (entries, tree.current)
        };
        let line = |line, parent, text: &str, active| (line, parent, text.to_owned(), active);
        assert_eq!(
            shape(&agent),
            (
                vec![
                    line(1, None, "one", true),
                    line(2, Some(1), "A", true),
                    line(3, Some(2), "two", true),
                    line(4, Some(3), "B", true),
                ],
                Some(4)
            )
        );
        assert_eq!(
            agent.navigate(4, None, &mut ui, &cancel).unwrap(),
            Navigated::AlreadyThere
        );
        assert!(agent.navigate(9, None, &mut ui, &cancel).is_err());

        // A prompt is edited and sent again from before it; nothing is written until then.
        let saved = std::fs::read_to_string(agent.session_path()).unwrap();
        assert_eq!(
            agent.navigate(3, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: Some("two".into()),
                offer: None,
                question: false,
            }
        );
        assert_eq!(agent.conversation().len(), 2);
        assert_eq!(
            std::fs::read_to_string(agent.session_path()).unwrap(),
            saved
        );
        agent.turn("two again", &mut ui, &cancel).unwrap();
        let (entries, current) = shape(&agent);
        assert_eq!(entries[3], line(4, Some(3), "B", false));
        assert_eq!(entries[4], line(6, Some(2), "two again", true));
        assert_eq!((entries.len(), current), (6, Some(7)));
        let text = std::fs::read_to_string(agent.session_path()).unwrap();
        assert_eq!(text.lines().nth(5), Some(r#"{"parent":2,"type":"branch"}"#));

        // A resume continues the branch written last.
        let (_, resumed, _) = Session::open(agent.session_path()).unwrap();
        let texts: Vec<_> = resumed
            .iter()
            .map(|item| {
                item["content"]
                    .as_str()
                    .map_or_else(|| message_text(item), str::to_owned)
            })
            .collect();
        assert_eq!(texts, ["one", "A", "two again", "C"]);

        // From after a reply, its branch continues.
        assert_eq!(
            agent.navigate(4, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: None,
                offer: None,
                question: false,
            }
        );
        agent.turn("three", &mut ui, &cancel).unwrap();

        // From before the first prompt, a conversation starts anew in the same file.
        assert_eq!(
            agent.navigate(1, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: Some("one".into()),
                offer: None,
                question: false,
            }
        );
        assert!(agent.conversation().is_empty());
        agent.turn("first again", &mut ui, &cancel).unwrap();
        let (entries, current) = shape(&agent);
        assert_eq!(entries[8], line(12, None, "first again", true));
        assert_eq!(current, Some(13));
        assert!(!entries[0].3);

        let bodies = server.join().unwrap();
        let inputs: Vec<Vec<String>> = bodies
            .iter()
            .map(|body| {
                body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| {
                        item["content"]
                            .as_str()
                            .map_or_else(|| message_text(item), str::to_owned)
                    })
                    .collect()
            })
            .collect();
        assert_eq!(
            inputs,
            [
                vec!["one"],
                vec!["one", "A", "two"],
                vec!["one", "A", "two again"],
                vec!["one", "A", "two", "B", "three"],
                vec!["first again"],
            ]
        );

        // /fork lists the prompts of every branch and copies only the path to the one chosen.
        assert_eq!(
            agent.fork_prompts().unwrap(),
            ["one", "two", "two again", "three", "first again"]
        );
        let source: Vec<String> = std::fs::read_to_string(agent.session_path())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(agent.fork(3).unwrap(), Some(("three".into(), None)));
        let forked: Vec<String> = std::fs::read_to_string(agent.session_path())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(forked[1..], source[1..5]);
        assert_eq!(agent.conversation().len(), 4);
    }

    #[test]
    fn labels_are_lines_of_their_own_that_forks_clones_and_exports_carry_to_their_entries() {
        let (endpoint, _server) = serve(vec![stream(&fixture::text_events(
            "C",
            fixture::known_usage(),
        ))]);
        let (mut agent, scratch) = agent(endpoint, "tree-labels");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let path = scratch.0.join("labels.jsonl");
        let reply = |text: &str| {
            json!({"type":"message","role":"assistant",
                "content":[{"type":"output_text","text":text}],"_scramjet_model":"m"})
        };
        write_session(
            &path,
            &[
                json!({"type":"session","version":1}),
                json!({"type":"model_change","model":"m"}),
                json!({"role":"user","content":"one"}),
                reply("A"),
                json!({"role":"user","content":"two"}),
                reply("B"),
            ],
            std::time::SystemTime::now(),
        );
        agent.resume_session(&path).unwrap();
        let labels = |agent: &Agent| -> Vec<(usize, String)> {
            let tree = agent.tree().unwrap();
            (tree.entries.into_iter())
                .filter_map(|entry| Some((entry.line, entry.label?.text)))
                .collect()
        };
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let given = agent.set_label(3, Some("first answer")).unwrap().unwrap();
        assert_eq!(given.text, "first answer");
        assert!(given.time >= before && given.time < before + 60_000);
        assert!(agent.set_label(5, Some("second")).unwrap().is_some());
        // Taken away, also with an empty label.
        assert_eq!(agent.set_label(5, None).unwrap(), None);
        assert!(agent.set_label(4, Some("x")).unwrap().is_some());
        assert_eq!(agent.set_label(4, Some("")).unwrap(), None);
        assert!(agent.set_label(99, Some("x")).is_err());
        assert_eq!(labels(&agent), [(3, "first answer".to_owned())]);
        let lines: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            lines[6],
            json!({"type":"label","target":3,"label":"first answer","time":given.time})
        );
        assert_eq!(lines[8]["label"], Value::Null);
        assert_eq!((lines.len(), lines[8]["target"].as_u64()), (11, Some(5)));
        // The conversation is where it was; a label of no entry shows nowhere.
        assert_eq!(agent.tree().unwrap().current, Some(5));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(
            &mut file,
            b"{\"type\":\"label\",\"target\":9,\"label\":\"y\"}\n",
        )
        .unwrap();
        assert_eq!(labels(&agent).len(), 1);

        // On another branch, after the label lines, a prompt's label is on its own line.
        agent.navigate(4, None, &mut ui, &cancel).unwrap();
        agent.turn("two again", &mut ui, &cancel).unwrap();
        let again = (agent.tree().unwrap().entries.iter())
            .find(|entry| entry.item == TreeItem::Prompt("two again".into()))
            .map(|entry| entry.line)
            .unwrap();
        assert_eq!(again, 13);
        agent.set_label(again, Some("retry")).unwrap();

        // An export and a clone keep the labels of the entries on the path, at their lines there.
        let carried = |path: &std::path::Path| -> Vec<(u64, String)> {
            (std::fs::read_to_string(path).unwrap().lines())
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter(|line| line["type"] == "label")
                .map(|line| {
                    let text = line["label"].as_str().unwrap().to_owned();
                    (line["target"].as_u64().unwrap(), text)
                })
                .collect()
        };
        let exported = scratch.0.join("exported.jsonl");
        assert!(agent.export_session(&exported).unwrap());
        let both = vec![(3, "first answer".to_owned()), (4, "retry".to_owned())];
        assert_eq!(carried(&exported), both);
        let original = agent.session_path().to_owned();
        assert!(agent.clone_session().unwrap());
        assert_eq!(carried(agent.session_path()), both);
        assert_eq!(
            labels(&agent),
            [(3, "first answer".to_owned()), (4, "retry".to_owned())]
        );
        // A fork before a prompt keeps those before it.
        agent.resume_session(&original).unwrap();
        assert_eq!(agent.fork(2).unwrap(), Some(("two again".into(), None)));
        assert_eq!(labels(&agent), [(3, "first answer".to_owned())]);
    }

    #[test]
    fn the_tree_lists_tool_results_and_ends_after_a_reply_s_last_one() {
        let (endpoint, _server) = serve(vec![]);
        let (mut agent, scratch) = agent(endpoint, "tree-calls");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let path = scratch.0.join("calls.jsonl");
        let call = |id: &str, name: &str, arguments: Value| {
            json!({"type":"function_call","call_id":id,"name":name,
                "arguments":arguments.to_string(),"_scramjet_model":"a"})
        };
        write_session(
            &path,
            &[
                json!({"type":"session","version":1}),
                json!({"type":"model_change","model":"a"}),
                json!({"role":"user","content":"go"}),
                json!({"type":"reasoning","summary":[],"_scramjet_model":"a"}),
                call("c1", "read", json!({"path":"x","offset":3,"limit":2})),
                call("c2", "bash", json!({"command":"ls"})),
                json!({"type":"function_call_output","call_id":"c1","output":"x"}),
                json!({"type":"function_call_output","call_id":"c2","output":"y"}),
                json!({"type":"message","role":"assistant",
                    "content":[{"type":"output_text","text":"done"}],"_scramjet_model":"a"}),
                json!({"type":"compaction","first_kept":0,"summary":"s","tokens_before":1200}),
                json!({"role":"user","content":"Ran `ls`","_scramjet_typed":"!ls"}),
                json!({"type":"model_change","model":"b"}),
            ],
            std::time::SystemTime::now(),
        );
        agent.resume_session(&path).unwrap();
        let tree = agent.tree().unwrap();
        let listed: Vec<_> = tree
            .entries
            .iter()
            .map(|entry| (entry.line, entry.parent, &entry.item))
            .collect();
        let tool = |name: &str, summary: &str| TreeItem::Tool(name.into(), summary.into());
        assert_eq!(
            listed,
            [
                (1, None, &TreeItem::Model("a".into())),
                (2, Some(1), &TreeItem::Prompt("go".into())),
                (6, Some(2), &tool("read", "x:3-4")),
                (7, Some(6), &tool("bash", "ls")),
                (8, Some(7), &TreeItem::Reply("done".into())),
                (9, Some(8), &TreeItem::Compaction(Some(1200))),
                (10, Some(9), &TreeItem::Command("ls".into())),
                (11, Some(10), &TreeItem::Model("b".into())),
            ]
        );
        assert_eq!(tree.current, Some(11));

        let saved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            agent.navigate(6, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: None,
                offer: None,
                question: false,
            }
        );
        assert_eq!(agent.conversation().len(), 6);
        assert_eq!(agent.conversation()[5]["call_id"], "c2");
        assert_eq!(agent.last_model(), Some("a"));
        assert_eq!(agent.model(), "test/b");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        let tree = agent.tree().unwrap();
        assert_eq!(tree.current, Some(7));
        assert_eq!(tree.entries.len(), 8);
        assert_eq!(
            agent.navigate(7, None, &mut ui, &cancel).unwrap(),
            Navigated::AlreadyThere
        );
        assert_eq!(
            agent.navigate(6, None, &mut ui, &cancel).unwrap(),
            Navigated::AlreadyThere
        );
    }

    #[test]
    fn a_branch_left_by_tree_navigation_is_summarized_where_it_continues() {
        let (endpoint, server) = serve(vec![
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 15\r\n\r\ninvalid request".to_vec(),
            stream(&fixture::text_events(
                "## Goal\nlook",
                fixture::known_usage(),
            )),
            stream(&fixture::text_events("next reply", fixture::known_usage())),
        ]);
        let (mut agent, scratch) = agent(endpoint, "tree-summary");
        let path = scratch.0.join("branch.jsonl");
        let reply = |text: &str| {
            json!({"type":"message","role":"assistant",
                "content":[{"type":"output_text","text":text}],"_scramjet_model":"m"})
        };
        write_session(
            &path,
            &[
                json!({"type":"session","version":1}),
                json!({"type":"model_change","model":"m"}),
                json!({"role":"user","content":"start"}),
                reply("ready"),
                json!({"role":"user","content":"explore"}),
                json!({"type":"function_call","call_id":"c1","name":"read",
                    "arguments":r#"{"path":"x.rs"}"#,"_scramjet_model":"m"}),
                json!({"type":"function_call_output","call_id":"c1","output":"tool output"}),
                reply("found it"),
                json!({"type":"branch_summary","summary":"earlier",
                    "read_files":["old.rs"],"modified_files":["m.rs"]}),
                json!({"role":"user","content":"more"}),
                reply("done"),
            ],
            std::time::SystemTime::now(),
        );
        agent.resume_session(&path).unwrap();
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        assert!(!agent.leaves_branch(10).unwrap());
        assert!(agent.leaves_branch(4).unwrap());

        // A failed summary leaves everything as it was.
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(matches!(
            agent.navigate(4, Some("focus"), &mut ui, &cancel),
            Err(AgentError::Provider(_))
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        assert_eq!(agent.conversation().len(), 9);

        // From before the prompt chosen, which is not part of the branch summarized.
        assert_eq!(
            agent.navigate(4, Some("focus"), &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: Some("explore".into()),
                offer: None,
                question: false,
            }
        );
        assert_eq!(ui.waits.last(), Some(&Purpose::BranchSummary));
        let summary = "The user explored a different conversation branch before returning here.\n\
Summary of that exploration:\n\n## Goal\nlook\n\n<read-files>\nold.rs\nx.rs\n</read-files>\n\n\
<modified-files>\nm.rs\n</modified-files>";
        let message = compaction::branch_summary_message(summary);
        assert_eq!(agent.conversation().len(), 3);
        assert_eq!(agent.conversation()[2], message);
        let lines: Vec<Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines[11], json!({"type":"branch","parent":3}));
        assert_eq!(
            lines[12],
            json!({"type":"branch_summary","summary":summary,
                "read_files":["old.rs","x.rs"],"modified_files":["m.rs"]})
        );
        agent.turn("next", &mut ui, &cancel).unwrap();

        // Going back to before a prompt leaves its reply; moving on along a path leaves nothing.
        assert!(agent.leaves_branch(13).unwrap());
        assert_eq!(
            agent.navigate(3, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: None,
                offer: None,
                question: false,
            }
        );
        assert!(!agent.leaves_branch(14).unwrap());
        let tree = agent.tree().unwrap();
        let summarized = tree
            .entries
            .iter()
            .find(|entry| matches!(entry.item, TreeItem::BranchSummary(_)))
            .map(|entry| (entry.line, entry.parent));
        assert_eq!(summarized, Some((8, Some(7))));
        assert!(tree.entries.iter().any(|entry| entry.line == 12
            && entry.parent == Some(3)
            && entry.item == TreeItem::BranchSummary(summary.into())));

        // A resume sends the summary as a message where it was written, and shows it there.
        let (_, resumed, _) = Session::open(&path).unwrap();
        assert_eq!(resumed[2], message);
        let mut shown = Recorder::default();
        show_items(&resumed, None, &mut shown);
        assert_eq!(
            shown.shown,
            [
                "prompt start".to_owned(),
                "reply ready".into(),
                format!("branch {summary}"),
                "prompt next".into(),
                "reply next reply".into(),
            ]
        );

        let bodies = server.join().unwrap();
        let request = &bodies[1];
        assert_eq!(request["instructions"], compaction::SYSTEM_PROMPT);
        assert!(request.get("tools").is_none());
        assert_eq!(request["max_output_tokens"], 2048);
        assert_eq!(request["reasoning"]["effort"], "none");
        let prompt = request["input"][0]["content"].as_str().unwrap();
        let conversation = "<conversation>\n[Assistant tool calls]: read(path=\"x.rs\")\n\n\
[Assistant]: found it\n\n[User]: The following is a summary of a branch that this conversation \
came back from:\n\n<summary>\nearlier</summary>\n\n[User]: more\n\n[Assistant]: done\n\
</conversation>\n\nCreate a structured summary of this conversation branch";
        assert!(prompt.starts_with(conversation), "{prompt}");
        assert!(prompt.ends_with("error messages.\n\nAdditional focus: focus"));
        assert_eq!(bodies[2]["input"][2], message);
        assert_eq!(bodies[2]["input"][3]["content"], "next");
    }
}
