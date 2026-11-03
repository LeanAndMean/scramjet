//! The session as a tree: the entries `/tree` lists, and continuing the conversation from
//! one of them.
use super::tools::Call;
use super::{Agent, AgentError, AgentUi, Purpose, TYPED, compaction, message_text, session};
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
}

#[derive(Debug, PartialEq)]
pub enum TreeItem {
    Prompt(String),
    /// A `!` command's text.
    Command(String),
    Reply(String),
    /// A tool call's result, or the call itself, by the call's name and its call line's summary.
    Tool(String, String),
    Compaction(Option<u64>),
    BranchSummary(String),
    /// Listed only when it is the current entry, as are the three below.
    Reasoning,
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
}

/// Where `Agent::navigate` left the conversation.
#[derive(Debug, PartialEq)]
pub enum Navigated {
    /// The conversation already ended at that entry.
    AlreadyThere,
    /// It now ends there; `prompt`, if the entry was one, is to be edited and sent again.
    Moved { prompt: Option<String> },
}

impl Agent {
    /// The current session's entries, as Scramjet's `/tree` lists them by default: prompts,
    /// replies with text, tool results, `!` commands, compactions and branch summaries, and the
    /// current entry whatever it is.
    pub fn tree(&self) -> Result<SessionTree, AgentError> {
        let entries = self.session.entries()?;
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
        let tool = |value: &Value| {
            let (name, summary) = value["call_id"]
                .as_str()
                .and_then(|id| calls.get(id))
                .cloned()
                .unwrap_or_else(|| {
                    (
                        value["name"].as_str().unwrap_or("tool").to_owned(),
                        String::new(),
                    )
                });
            TreeItem::Tool(name, summary)
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
                Some("function_call_output") => Some(tool(value)),
                Some("compaction") => Some(TreeItem::Compaction(value["tokens_before"].as_u64())),
                Some("branch_summary") => Some(TreeItem::BranchSummary(
                    value["summary"].as_str().unwrap_or_default().to_owned(),
                )),
                Some("function_call") if is_leaf => Some(tool(value)),
                Some("reasoning") if is_leaf => Some(TreeItem::Reasoning),
                Some("model_change") if is_leaf => Some(TreeItem::Model(
                    value["model"].as_str().unwrap_or_default().to_owned(),
                )),
                Some("thinking_level_change") if is_leaf => Some(TreeItem::Effort(
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
        let summary = match summary {
            Some(focus) if !branch.is_empty() => {
                Some(self.summarize_branch(&branch, focus, ui, cancel)?)
            }
            _ => None,
        };
        self.conversation = self.session.move_to(destination.leaf)?;
        self.usage.context_tokens = self.session.context_tokens();
        self.autopilot = self.session.autopilot().unwrap_or(self.autopilot);
        self.restore_command();
        self.answer_interrupted_calls()?;
        if let Some((summary, files)) = summary {
            self.session
                .append(&json!({"type":"branch_summary", "summary":summary,
                "read_files":files.read, "modified_files":files.modified}))?;
            self.conversation
                .push(compaction::branch_summary_message(&summary));
        }
        Ok(Navigated::Moved {
            prompt: destination.prompt,
        })
    }

    /// Where continuing from the entry on `line` ends the conversation; `None` if it already
    /// ends there.
    fn destination(&self, line: usize) -> Result<Option<Destination>, AgentError> {
        if self.session.leaf() == Some(line) {
            return Ok(None);
        }
        let entries = self.session.entries()?;
        let Some(entry) = entries.iter().find(|entry| entry.line == line) else {
            return Err(AgentError::Session(std::io::Error::other(
                "no such entry in the session",
            )));
        };
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
            if leaf == self.session.leaf() {
                return Ok(None);
            }
        }
        // As in Scramjet, a prompt chosen to be sent again is not part of the branch left.
        let kept = if prompt.is_some() { Some(line) } else { leaf };
        Ok(Some(Destination {
            leaf,
            prompt,
            left: self.session.left_behind(kept)?,
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
            .saturating_sub(compaction::RESERVE_TOKENS)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{Session, show_items};
    use crate::provider_fixture as fixture;

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
                prompt: Some("two".into())
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
            Navigated::Moved { prompt: None }
        );
        agent.turn("three", &mut ui, &cancel).unwrap();

        // From before the first prompt, a conversation starts anew in the same file.
        assert_eq!(
            agent.navigate(1, None, &mut ui, &cancel).unwrap(),
            Navigated::Moved {
                prompt: Some("one".into())
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
        assert_eq!(agent.fork(3).unwrap().as_deref(), Some("three"));
        let forked: Vec<String> = std::fs::read_to_string(agent.session_path())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(forked[1..], source[1..5]);
        assert_eq!(agent.conversation().len(), 4);
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
                (2, None, &TreeItem::Prompt("go".into())),
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
            Navigated::Moved { prompt: None }
        );
        assert_eq!(agent.conversation().len(), 6);
        assert_eq!(agent.conversation()[5]["call_id"], "c2");
        assert_eq!(agent.last_model(), Some("a"));
        assert_eq!(agent.model(), "b");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
        let tree = agent.tree().unwrap();
        assert_eq!(tree.current, Some(7));
        assert_eq!(tree.entries.len(), 6);
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
                prompt: Some("explore".into())
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
            Navigated::Moved { prompt: None }
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
        show_items(&resumed, &mut shown);
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
