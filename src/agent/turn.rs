//! A turn: model requests and tool calls from a prompt to the final answer, the command it
//! runs, and the `!` commands the user runs without one.
use super::tools::{self, Approval, Call, Output};
use super::{
    Agent, AgentError, AgentUi, Answer, AutoCompaction, CONTEXT, Completed, DIFF, FAILED,
    MAX_STEPS, PROBE, PRODUCER, Purpose, Routing, TYPED, ToolDetail, USAGE, autonomy, message_text,
    question, sandbox, status, subagent,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

impl Agent {
    /// Runs one user prompt to completion: model requests and tool calls until a final answer.
    /// As Scramjet does, the conversation is compacted before the prompt and after the reply if
    /// its context passed the model's window minus `compaction::RESERVE_TOKENS`. A request that
    /// fails because its input exceeded the window is compacted and sent again, once per turn.
    pub fn turn(
        &mut self,
        prompt: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        self.compact_before_prompt(ui, cancel)?;
        self.completed = None;
        let command = self
            .commands
            .named(prompt)
            .map(|command| command.name.clone());
        self.lifecycle.prompted(prompt, command.as_deref());
        // A command prompt sends its expansion; the prompt as typed is kept for recall and display.
        let message = match self.commands.expand(prompt) {
            Some(sent) => json!({"role":"user","content":sent,TYPED:prompt}),
            None => json!({"role":"user","content":prompt}),
        };
        self.add(message)?;
        self.save_command()?;
        self.prompts.push(prompt.to_owned());
        let mut ran = self.run(ui, cancel);
        // As Scramjet's lifecycle: a command's turn that ends without a report gets a status check.
        loop {
            match ran {
                Err(AgentError::Cancelled) => {
                    if let Some(notice) = self.lifecycle.cancelled() {
                        ui.notice(&notice);
                    }
                    self.save_command()?;
                    return Err(AgentError::Cancelled);
                }
                Err(error) => return Err(error),
                Ok(()) => {}
            }
            let ended = self.lifecycle.turn_ended(&self.commands);
            self.save_command()?;
            match ended {
                status::Ended::Probe(check) => {
                    self.add(json!({"role":"user","content":check,PROBE:true}))?;
                    ran = self.run(ui, cancel);
                }
                status::Ended::Completed(completed) => {
                    self.completed = Some(completed);
                    return Ok(());
                }
                status::Ended::Notice(notice) => {
                    ui.notice(&notice);
                    return Ok(());
                }
                status::Ended::Nothing => return Ok(()),
            }
        }
    }

    /// The command the last turn reported completed, for `route`.
    pub fn take_completed(&mut self) -> Option<Completed> {
        self.completed.take()
    }

    /// The next step of a completed command, from its `next:` policy and the `autonomy.yaml`
    /// edges; `interactive` when the user can choose from a list. What is said about the
    /// `autonomy.yaml` files is said once, until it changes, as in Scramjet.
    pub fn route(&mut self, completed: &Completed, interactive: bool) -> Routing {
        let mut edges = autonomy::Edges::load(self.autonomy.as_deref(), &self.config.root);
        if edges.diagnostics == self.autonomy_diagnostics {
            edges.diagnostics.clear();
        } else {
            self.autonomy_diagnostics = edges.diagnostics.clone();
        }
        status::route(completed, &self.commands, &edges, interactive)
    }

    /// Model requests and tool calls until a final answer.
    fn run(&mut self, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> Result<(), AgentError> {
        let mut overflow_recovered = false;
        for _ in 0..MAX_STEPS {
            if cancel.load(Ordering::Relaxed) {
                return Err(AgentError::Cancelled);
            }
            let request = self.request();
            let reply = match self.respond(&request, Purpose::Reply, ui, cancel) {
                Ok(reply) => reply,
                Err(AgentError::ContextOverflow(message)) if !overflow_recovered => {
                    overflow_recovered = true;
                    if !self.auto_compact(AutoCompaction::Overflow, ui, cancel)? {
                        return Err(AgentError::ContextOverflow(format!(
                            "{message} (nothing is old enough to compact)"
                        )));
                    }
                    continue;
                }
                Err(AgentError::ContextOverflow(message)) => {
                    return Err(AgentError::ContextOverflow(format!(
                        "{message} (again after compacting once; reduce the context or switch \
to a larger-context model)"
                    )));
                }
                Err(error) => return Err(error),
            };
            self.usage.context_tokens = reply.usage.as_ref().and_then(|usage| usage.total_tokens);
            let mut calls = Vec::new();
            let last = reply.items.len().saturating_sub(1);
            let saved = reply.usage.as_ref().map(saved_usage);
            for (index, mut item) in reply.items.into_iter().enumerate() {
                if let Some(object) = item.as_object_mut() {
                    object.insert(PRODUCER.into(), json!(self.config.model));
                    if let Some(tokens) = self.usage.context_tokens.filter(|_| index == last) {
                        object.insert(CONTEXT.into(), json!(tokens));
                    }
                    if let Some(usage) = saved.as_ref().filter(|_| index == last) {
                        object.insert(USAGE.into(), usage.clone());
                    }
                }
                match item["type"].as_str() {
                    Some("message") => ui.assistant(&message_text(&item)),
                    Some("function_call") => calls.push(item.clone()),
                    _ => {}
                }
                self.add(item)?;
            }
            if calls.is_empty() {
                self.compact_when_full(ui, cancel)?;
                return Ok(());
            }
            // As in Pi, the turn ends without another request when every call ends it.
            let mut ends = true;
            for item in calls {
                let (output, ends_turn) = self.call(&item, ui, cancel);
                ends &= ends_turn;
                let mut result = json!({"type":"function_call_output","call_id":item["call_id"],"output":output.text});
                if output.failed {
                    result[FAILED] = json!(true);
                }
                if let Some(ToolDetail::Diff(diff)) = output.detail {
                    result[DIFF] = json!(diff);
                }
                self.add(result)?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(AgentError::Cancelled);
                }
            }
            // After the results, so one reply's results stay a chain (see `destination`).
            self.save_command()?;
            if ends {
                self.compact_when_full(ui, cancel)?;
                return Ok(());
            }
        }
        Err(AgentError::TooManySteps)
    }

    /// A `!` or `!!` command the user runs without a model turn: `include` adds it to the
    /// conversation as a user message, as Scramjet's `!` does, and `!!` leaves it out.
    pub fn shell(
        &mut self,
        command: &str,
        include: bool,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        ui.tool_call("bash", command);
        ui.tool_running();
        let ran =
            match tools::run_command(&self.config.root, command, None, cancel, &mut |output| {
                ui.tool_output(output)
            }) {
                Ok(ran) => ran,
                Err(error) => {
                    ui.command_result(&format!("Error: {error}"), true);
                    return Ok(());
                }
            };
        let shown = ran.tool_output();
        ui.command_result(&shown.text, shown.failed);
        if include {
            let prompt = format!("!{command}");
            let mut message = json!({"role":"user","content":ran.message(command),TYPED:prompt});
            if shown.failed {
                message[FAILED] = json!(true);
            }
            self.add(message)?;
            self.prompts.push(prompt);
        }
        Ok(())
    }

    /// Runs a tool call, after any approval; also says whether the turn ends after it, as a
    /// question the user cancelled or is to answer at the prompt ends it.
    fn call(&mut self, item: &Value, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> (Output, bool) {
        let failure = |text: String| Output {
            text,
            detail: None,
            failed: true,
        };
        let call = match Call::parse(item) {
            Ok(call) => call,
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(item["name"].as_str().unwrap_or(""), &output, None, true);
                return (failure(output), false);
            }
        };
        ui.tool_call(call.name, &call.summary());
        if call.name == question::NAME {
            return ask(&call.arguments, &mut self.lifecycle, ui);
        }
        if call.name == subagent::NAME {
            let (output, usage) = {
                let parent = subagent::Parent {
                    endpoint: self.config.endpoint,
                    retries: self.config.retries,
                    model: &self.config.model,
                    info: &self.info,
                    effort: self.effort().or(self.info.default_level.as_deref()),
                    max_output_tokens: self.config.max_output_tokens,
                    root: &self.config.root,
                    cache_key: self.session.id(),
                    sources: subagent::Sources {
                        user: self.agents_directory.as_deref(),
                        sets: self.sets_root.as_deref(),
                    },
                };
                subagent::run(&call.arguments, &parent, ui, cancel)
            };
            self.usage.add(&usage);
            ui.tool_result(
                call.name,
                &output.text,
                output.detail.as_ref(),
                output.failed,
            );
            return (output, false);
        }
        if call.name == status::NAME {
            let (text, failed, ends) = self.lifecycle.report(&call.arguments);
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
            };
            return (output, ends);
        }
        if !self.config.tools.iter().any(|tool| tool == call.name) {
            let output = format!("Tool {} not found", call.name);
            ui.tool_result(call.name, &output, None, true);
            return (failure(output), false);
        }
        let answer = match call.approval(&self.config.root) {
            Approval::Ask(question) if self.config.ask => ui.approve(&question, cancel),
            _ => Answer::Allow,
        };
        if answer == Answer::AllowAll {
            self.config.ask = false;
        }
        if answer == Answer::Deny {
            let output = if cancel.load(Ordering::Relaxed) {
                "The user cancelled the turn before this tool call ran."
            } else {
                "The user denied this tool call."
            };
            ui.tool_result(call.name, output, None, true);
            return (failure(output.to_owned()), false);
        }
        ui.tool_running();
        let hidden = self.hide_credentials.then(sandbox::Hidden::current);
        let output = call
            .run(&self.config.root, hidden.as_ref(), cancel, &mut |output| {
                ui.tool_output(output)
            })
            .unwrap_or_else(|error| failure(format!("Error: {error}")));
        ui.tool_result(
            call.name,
            &output.text,
            output.detail.as_ref(),
            output.failed,
        );
        (output, false)
    }
}

/// Asks the user the question of a `get_scramjet_user_input` call, as Scramjet's tool does.
/// A freetext question parks the active command for the user's reply, and a cancelled one leaves
/// it for the next reply to resume.
fn ask(
    arguments: &Value,
    lifecycle: &mut status::Lifecycle,
    ui: &mut dyn AgentUi,
) -> (Output, bool) {
    let question = match question::Question::parse(arguments) {
        Ok(question) => question,
        Err(error) => {
            ui.tool_result(question::NAME, &error, None, true);
            let output = Output {
                text: error,
                detail: None,
                failed: true,
            };
            return (output, false);
        }
    };
    ui.question(&question.message);
    let chosen = match question.kind {
        question::Kind::Freetext => None,
        question::Kind::Confirm => ui.choose("Confirm", &question.options, None),
        question::Kind::Select => {
            ui.choose("Choose an option", &question.options, question.recommended)
        }
    };
    let parked = question.kind == question::Kind::Freetext && lifecycle.park();
    if chosen.is_none() && question.kind != question::Kind::Freetext {
        lifecycle.question_cancelled();
    }
    let (text, ends) = question.answer(chosen, parked);
    let detail = question::shown(arguments, &text).map(|(_, shown)| ToolDetail::Answer(shown));
    ui.tool_result(question::NAME, &text, detail.as_ref(), false);
    let output = Output {
        text,
        detail,
        failed: false,
    };
    (output, ends)
}

/// A response's usage as saved with its last item: input tokens not read from the cache, output
/// tokens and cache reads, as Scramjet's usage counts them.
fn saved_usage(usage: &crate::http::responses::ProviderUsage) -> Value {
    let cached = usage.cached_input_tokens.unwrap_or(0);
    json!({
        "input": usage.input_tokens.unwrap_or(0).saturating_sub(cached),
        "output": usage.output_tokens.unwrap_or(0),
        "cache_read": cached,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{
        AgentConfig, Commands, Navigated, Next, Session, SessionChoice, TreeItem, instructions,
        show_items,
    };
    use crate::provider_fixture as fixture;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn a_subagent_runs_its_task_read_only_and_returns_its_last_reply() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                subagent::NAME,
                json!({"agent":"reviewer","task":"Check kept.txt\nCarefully."}),
            ),
            tool_call(
                "call_2",
                "bash",
                json!({"command":"cat kept.txt; echo changed > kept.txt"}),
            ),
            tool_call(
                "call_3",
                "write",
                json!({"path":"kept.txt","content":"changed"}),
            ),
            reply("kept.txt says original"),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews\ntools: read, ls, bash, write\n---\n\n\
You review code.\n",
        )
        .unwrap();
        let kept = agent.root().join("kept.txt");
        std::fs::write(&kept, "original\n").unwrap();
        agent.agents_directory = Some(agents);
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("review", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 5);
        let parent = bodies[0]["instructions"].as_str().unwrap();
        assert!(parent.contains(&format!("- subagent: {}\n", subagent::SNIPPET)));
        let child = &bodies[1];
        let tools: Vec<_> = child["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["name"])
            .collect();
        assert_eq!(tools, ["read", "ls", "bash"]);
        let instructions = child["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("You are a subagent of scramjet-rs"));
        assert!(instructions.contains("\n\nYou review code.\nCurrent date: "));
        assert_eq!(
            child["input"],
            json!([{"role":"user","content":"Task: Check kept.txt\nCarefully."}])
        );
        assert_eq!(child["model"], "m");
        let key = format!("{}:reviewer", agent.session_id());
        assert_eq!(child["prompt_cache_key"], key.as_str());
        let bash = last_output(&bodies[2]);
        assert!(bash.starts_with("original\n"), "{bash}");
        assert!(bash.contains("Read-only file system"), "{bash}");
        assert_eq!(last_output(&bodies[3]), "Tool write not found");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "original\n");
        let result = "Agent invocation warnings:\n- write: left out, as subagents run read-only \
here\n\nkept.txt says original";
        assert_eq!(last_output(&bodies[4]), result);
        assert_eq!(agent.usage.requests, 5);
        assert_eq!(ui.replies, ["done"]);
        assert!(
            ui.shown
                .contains(&"call subagent reviewer [user]\nCheck kept.txt …".to_owned())
        );
        let shown = ui
            .shown
            .iter()
            .find(|line| line.starts_with("result subagent"))
            .unwrap();
        assert!(shown.starts_with(&format!("result subagent {result} Subagents(\"3 turns ")));
        assert!(shown.ends_with(" m\")"), "{shown}");
    }

    #[test]
    fn parallel_and_chained_subagents_share_out_tasks_at_most_at_the_parents_effort() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let mut parent_requests = 0;
        let (endpoint, server) = serve_answering(8, move |body| {
            let instructions = body["instructions"].as_str().unwrap();
            if instructions.starts_with("You are a subagent") {
                let task = body["input"][0]["content"].as_str().unwrap();
                let effort = body["reasoning"]["effort"].as_str().unwrap_or("default");
                return reply(&format!("{task} at {effort}"));
            }
            parent_requests += 1;
            match parent_requests {
                1 => tool_call(
                    "call_1",
                    subagent::NAME,
                    json!({"tasks":[{"agent":"a","task":"one","effort":"high"},
                        {"agent":"nope","task":"two"},{"agent":"b","task":"three"}]}),
                ),
                3 => tool_call(
                    "call_2",
                    subagent::NAME,
                    json!({"chain":[{"agent":"a","task":"first"},
                        {"agent":"b","task":"after {previous}","effort":"minimal"}]}),
                ),
                _ => reply("ok"),
            }
        });
        let (mut agent, _scratch) = agent(endpoint, "subagents");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        for name in ["a", "b"] {
            let file = format!("---\nname: {name}\ndescription: Agent {name}\ntools: read\n---\n");
            std::fs::write(agents.join(format!("{name}.md")), file).unwrap();
        }
        agent.agents_directory = Some(agents);
        agent.config.effort = Some("low".into());
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("split", &mut ui, &cancel).unwrap();
        agent.turn("chain", &mut ui, &cancel).unwrap();
        let bodies: Vec<Value> = server
            .join()
            .unwrap()
            .into_iter()
            .map(|(_, body)| body)
            .collect();
        let parents: Vec<&Value> = bodies
            .iter()
            .filter(|body| {
                body["instructions"]
                    .as_str()
                    .unwrap()
                    .starts_with("You are scramjet-rs")
            })
            .collect();
        assert_eq!(parents.len(), 4);
        assert_eq!(
            last_output(parents[1]),
            "Parallel: 2/3 succeeded\n\n[a] completed: Task: one at low\n\n[nope] failed: \
Unknown agent: \"nope\". Available agents: \"a\", \"b\".\n\n[b] completed: Task: three at low"
        );
        assert_eq!(
            last_output(parents[3]),
            "Task: after Task: first at low at minimal"
        );
        assert_eq!(agent.usage.requests, 8);
    }

    #[test]
    fn subagent_calls_are_checked_and_project_agents_need_the_users_approval() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                subagent::NAME,
                json!({"tasks":[{"agent":"a"}],"effort":"huge"}),
            ),
            tool_call("call_2", subagent::NAME, json!({"agent":"a","task":""})),
            tool_call(
                "call_3",
                subagent::NAME,
                json!({"agent":"p","task":"go","agentScope":"both"}),
            ),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent-checks");
        let agents = agent.root().join("agents");
        let project = agent.root().join(".scramjet/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(agents.join("a.md"), "---\nname: a\ndescription: A\n---\n").unwrap();
        std::fs::write(project.join("p.md"), "---\nname: p\ndescription: P\n---\n").unwrap();
        agent.agents_directory = Some(agents);
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("go", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let received =
            serde_json::to_string_pretty(&json!({"tasks":[{"agent":"a"}],"effort":"huge"}))
                .unwrap();
        assert_eq!(
            last_output(&bodies[1]),
            format!(
                "Validation failed for tool \"subagent\":\n  - tasks/0/task: must have required \
property\n  - effort: must be one of off, minimal, low, medium, high, xhigh, max\n\nReceived \
arguments:\n{received}"
            )
        );
        assert_eq!(
            last_output(&bodies[2]),
            "Invalid parameters. Provide exactly one mode.\nAvailable agents: a (user)"
        );
        assert_eq!(
            last_output(&bodies[3]),
            "Canceled: project-local agents not approved."
        );
        let asked = format!(
            "question Agents: p\nSource: {}\n\nProject agents are repo-controlled. Only continue \
for trusted repositories.",
            project.display()
        );
        assert!(ui.shown.contains(&asked), "{:?}", ui.shown);
        assert!(
            ui.shown
                .contains(&"choose Run project-local agents? [\"Yes\", \"No\"] None".to_owned())
        );
    }

    #[test]
    fn allowing_all_stops_later_approval_questions() {
        let bash = |id: &str| {
            let events = fixture::call_events(r#"{"command":"true"}"#, fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text.replace("\"inert\"", "\"bash\"").replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let (endpoint, server) = serve(vec![
            stream(&bash("call_1")),
            stream(&bash("call_2")),
            stream(&fixture::text_events("done", fixture::known_usage())),
            stream(&bash("call_3")),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "allow-all");
        agent.config.ask = true;
        let mut ui = Recorder {
            answers: vec![Answer::AllowAll],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        agent.turn("two", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (1, 3));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(outputs, ["(no output)"; 3]);
    }

    #[test]
    fn the_configured_coding_tools_are_offered_and_only_they_run() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "grep", json!({"pattern":"needle","path":"src"})),
            tool_call("call_2", "ls", json!({"path":"src"})),
            tool_call("call_3", "write", json!({"path":"x.txt","content":"x"})),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "configured-tools");
        agent.config.tools = ["ls", "grep", "read"].map(String::from).to_vec();
        agent.instructions =
            instructions::agent(agent.root(), &agent.config.tools, &agent.commands, &[]);
        std::fs::create_dir(agent.root().join("src")).unwrap();
        std::fs::write(agent.root().join("src/a.txt"), "a needle\n").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("look", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        let names: Vec<_> = (bodies[0]["tools"].as_array().unwrap().iter())
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "read",
                "grep",
                "ls",
                status::NAME,
                question::NAME,
                subagent::NAME
            ]
        );
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        for line in [
            "- read: Read file contents\n- grep: Search file contents for patterns (respects \
.gitignore)\n- ls: List directory contents\n- report_scramjet_command_status:",
            "Guidelines:\n- Use read to examine files instead of cat or sed.\n- Be concise",
        ] {
            assert!(instructions.contains(line), "{instructions}");
        }
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            ["a.txt:1: a needle", "a.txt", "Tool write not found"]
        );
        assert!(!agent.root().join("x.txt").exists());
    }

    #[test]
    fn a_tool_call_with_large_arguments_runs() {
        let content = "let x = 1;\n".repeat(4_000);
        let arguments = json!({ "path": "big.rs", "content": content }).to_string();
        let events = fixture::call_events(&arguments, fixture::known_usage());
        let text = serde_json::to_string(&events)
            .unwrap()
            .replace("\"inert\"", "\"write\"");
        let (endpoint, server) = serve(vec![
            stream(&serde_json::from_str::<Vec<Value>>(&text).unwrap()),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "large-arguments");
        let mut ui = Recorder::default();
        agent
            .turn("write it", &mut ui, &AtomicBool::new(false))
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            std::fs::read_to_string(agent.root().join("big.rs")).unwrap(),
            content
        );
    }

    #[test]
    fn tool_calls_run_without_questions_unless_asked() {
        let bash = |id: &str, command: &str| {
            let arguments = json!({ "command": command }).to_string();
            let events = fixture::call_events(&arguments, fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text.replace("\"inert\"", "\"bash\"").replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let (endpoint, server) = serve(vec![
            stream(&bash("call_1", "ls -a > listing")),
            stream(&fixture::text_events("done", fixture::known_usage())),
            stream(&bash("call_2", "ls -a")),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "unasked");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("list", &mut ui, &cancel).unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (0, 1));
        assert!(agent.root().join("listing").exists());
        agent.config.ask = true;
        agent.turn("list again", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (1, 2));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(outputs, ["(no output)", "The user denied this tool call."]);
    }

    #[test]
    fn bang_commands_run_without_a_turn_and_are_recalled_as_typed() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "ok",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "bang");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        // As Scramjet's `!` gives the model its output, without escape sequences.
        let command = r"printf '\e[1mhi\e[0m\n10%%\r100%%\n'; exit 3";
        let typed = format!("!{command}");
        agent.shell(command, true, &mut ui, &cancel).unwrap();
        agent.shell("echo secret", false, &mut ui, &cancel).unwrap();
        assert_eq!(ui.calls, ["bash", "bash"]);
        assert_eq!(agent.usage.requests, 0);
        let message = format!("Ran `{command}`\n```\nhi\n100%\n```\n\nCommand exited with code 3");
        assert_eq!(
            agent.conversation(),
            [
                json!({"role":"user","content":message,"_scramjet_typed":typed,
                "_scramjet_failed":true})
            ]
        );
        assert_eq!(agent.prompts(), [typed.as_str()]);
        agent.turn("next", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(
            bodies[0]["input"],
            json!([{"role":"user","content":message},{"role":"user","content":"next"}])
        );
        assert!(!bodies[0].to_string().contains("\"_scramjet_"));
        let path = agent.session_path().to_owned();
        let (_, _, prompts) = Session::open(&path).unwrap();
        assert_eq!(prompts, [typed.as_str(), "next"]);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("secret"));
        // As Scramjet's session list names it by its first message, not a `!` command.
        let listed = agent.sessions().unwrap();
        assert_eq!(listed[0].first_prompt.as_deref(), Some("next"));
    }

    #[test]
    fn a_command_prompt_sends_its_expansion_and_is_recalled_as_typed() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("one", fixture::known_usage())),
            // Scramjet's status check after the command's turn, answered without a report.
            stream(&fixture::text_events("checked", fixture::known_usage())),
            stream(&fixture::text_events("two", fixture::known_usage())),
        ]);
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-command-{}", std::process::id()));
        let _scratch = ScratchDir::new(root.clone());
        let commands = root.join(".scramjet/sjrstest/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(
            commands.join("sjrstest:echo.md"),
            "---\ndescription: Echo\n---\nSay $1 to ${@:2}\n",
        )
        .unwrap();
        let path = root.join("session.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"version\":1}\n").unwrap();
        let config = AgentConfig {
            endpoint,
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
        };
        let mut agent = Agent::start(config, SessionChoice::Open(path.clone())).unwrap();
        agent.instructions = instructions::agent(&root, &agent.config.tools, &agent.commands, &[]);
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let typed = "/sjrstest:echo hi 'the user'";
        agent.turn(typed, &mut ui, &cancel).unwrap();
        agent.turn("/sjrstest:other x", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let expansion =
            "<scramjet-command name=\"sjrstest:echo\">\nSay hi to the user\n</scramjet-command>";
        assert_eq!(
            bodies[0]["input"],
            json!([{"role":"user","content":expansion}])
        );
        assert_eq!(
            bodies[2]["input"][4],
            json!({"role":"user","content":"/sjrstest:other x"})
        );
        assert!(!bodies[2].to_string().contains("\"_scramjet_"));
        let sent = bodies[0]["instructions"].as_str().unwrap();
        assert!(sent.contains("\n# Command framing\n\nCommands injected by the Scramjet harness"));
        assert!(sent.contains(
            "consumer contracts, and required side effects remain controlling.\n\n\
# Available commands\n\nThe following slash commands can be invoked directly:\n\n\
- /sjrstest:echo: Echo\n\nCurrent date: "
        ));
        let prompts = [typed, "/sjrstest:other x"];
        assert_eq!(agent.prompts(), prompts);
        assert_eq!(agent.fork_prompts().unwrap(), prompts);
        // The status check is in the total, as Pi counts its custom messages, not a user message.
        assert_eq!((agent.stats().user, agent.stats().total), (2, 6));
        let tree = agent.tree().unwrap();
        assert_eq!(tree.entries[0].item, TreeItem::Prompt(typed.into()));
        let (_, conversation, resumed) = Session::open(&path).unwrap();
        assert_eq!(resumed, prompts);
        let mut shown = Recorder::default();
        show_items(&conversation, &mut shown);
        assert_eq!(shown.shown[0], format!("prompt {typed}"));
    }

    #[test]
    fn a_cancelled_bang_command_is_recorded_as_cancelled() {
        let (endpoint, _server) = serve(vec![]);
        let (mut agent, _scratch) = agent(endpoint, "bang-cancel");
        let mut ui = Recorder::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            setter.store(true, Ordering::Relaxed);
        });
        agent.shell("sleep 30", true, &mut ui, &cancel).unwrap();
        assert_eq!(
            agent.conversation()[0]["content"],
            "Ran `sleep 30`\n(no output)\n\n(command cancelled)"
        );
    }

    #[test]
    fn questions_wait_for_the_users_choice_and_a_cancel_or_freetext_ends_the_turn() {
        let ask = |id: &str, arguments: Value| {
            let events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text
                .replace("\"inert\"", &format!("\"{}\"", question::NAME))
                .replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let select = json!({"type":"select","message":"Which?","recommended":1,
            "options":[{"value":"a","label":"Alpha","description":"first"},{"value":"b","label":"Beta"}]});
        let confirm = json!({"type":"confirm","message":"Go on?"});
        let (endpoint, server) = serve(vec![
            stream(&ask("call_1", select.clone())),
            stream(&ask("call_2", json!({"type":"select","message":"Which?"}))),
            stream(&ask("call_3", confirm.clone())),
            stream(&fixture::text_events("chose", fixture::known_usage())),
            // Cancelled: no request follows within the turn.
            stream(&ask("call_4", confirm)),
            stream(&ask("call_5", json!({"type":"freetext","message":"Why?"}))),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "question");
        let mut ui = Recorder {
            choices: vec![Some(0), Some(1)],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("pick", &mut ui, &cancel).unwrap();
        agent.turn("again", &mut ui, &cancel).unwrap();
        agent.turn("explain", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);
        let tools = bodies[0]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == question::NAME));
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        assert!(instructions.contains(&format!("- {}: {}\n", question::NAME, question::SNIPPET)));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                r#"{"selected":"a"}"#,
                "Validation error: 'options' is required and must be a non-empty array for \
select type.",
                r#"{"confirmed":false}"#,
                r#"{"cancelled":true}"#,
                r#"{"parked":false}"#,
            ]
        );
        let first = [
            "call get_scramjet_user_input ",
            "question Which?",
            "choose Choose an option [\"Alpha\", \"Beta\"] Some(1)",
            "result get_scramjet_user_input {\"selected\":\"a\"} Answer(\"→ Alpha — first\\n  Beta\")",
        ];
        assert_eq!(ui.shown[..4], first);
        assert_eq!(
            ui.shown[5],
            "result get_scramjet_user_input Validation error: 'options' is required and must be \
a non-empty array for select type. failed"
        );
        assert_eq!(ui.shown[8], "choose Confirm [\"Yes\", \"No\"] None");
        assert_eq!(ui.replies, ["chose"]);
        // A resumed session shows each question with its answer.
        let mut replay = Recorder::default();
        agent.show_conversation(&mut replay);
        let asked: Vec<_> = replay
            .shown
            .iter()
            .filter(|shown| shown.starts_with("question") || shown.starts_with("result"))
            .map(|shown| shown.split(" {").next().unwrap())
            .collect();
        assert_eq!(
            asked,
            [
                "question Which?",
                "result get_scramjet_user_input",
                "result get_scramjet_user_input Validation error: 'options' is required and must \
be a non-empty array for select type. failed",
                "question Go on?",
                "result get_scramjet_user_input",
                "question Go on?",
                "result get_scramjet_user_input",
                "question Why?",
                "result get_scramjet_user_input",
            ]
        );
        assert!(
            replay
                .shown
                .last()
                .unwrap()
                .ends_with("Answer(\"Reply at the prompt\")")
        );
    }

    #[test]
    fn a_resumed_session_keeps_its_command_as_scramjet_rebuilds_it() {
        let call = |id: &str, name: &str, arguments: Value| {
            let events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text
                .replace("\"inert\"", &format!("\"{name}\""))
                .replace("call_1", id);
            stream(&serde_json::from_str::<Vec<Value>>(&text).unwrap())
        };
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let failed = b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let (endpoint, server) = serve(vec![
            call(
                "call_1",
                question::NAME,
                json!({"type":"freetext","message":"Which colour?"}),
            ),
            reply("used blue"),
            call(
                "call_2",
                status::NAME,
                json!({"summary":"Used blue.","status":"completed","recommended_next_step":0,
                "next_steps":[{"message":"/rs:work again","fresh_session":false,"reason":"More."}]}),
            ),
            failed.to_vec(),
            reply("hello"),
            reply("hi"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "command-resume");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(
            commands.join("rs:work.md"),
            "---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:work\n---\nWork.\n",
        )
        .unwrap();
        let resume = |agent: Agent| {
            let config = AgentConfig {
                endpoint,
                model: "m".into(),
                root: agent.root().to_owned(),
                effort: None,
                max_output_tokens: None,
                ask: false,
                retries: 0,
                tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
            };
            let path = agent.session_path().to_owned();
            drop(agent);
            let mut agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
            agent.commands = Commands::discover(None, &agent.root().join(".scramjet"));
            agent.autonomy = None;
            agent
        };
        agent.commands = Commands::discover(None, &agent.root().join(".scramjet"));
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();

        // The question waits for the reply after a resume; the reply's turn gets the status
        // check, and the completed command's next step is offered.
        let mut agent = resume(agent);
        agent.turn("blue", &mut ui, &cancel).unwrap();
        let completed = agent.take_completed().unwrap();
        let routing = agent.route(&completed, true);
        let Next::Offer { steps, .. } = routing.next else {
            panic!("no next step offered: {routing:?}");
        };
        assert_eq!(steps[0].command.as_deref(), Some("/rs:work again"));
        let tree = agent.tree().unwrap();
        assert!(matches!(
            agent.navigate(tree.current.unwrap(), None, &mut ui, &cancel),
            Ok(Navigated::AlreadyThere)
        ));

        // A command whose turn failed is dormant after a resume: a reply gets no status check.
        assert!(agent.turn("/rs:work", &mut ui, &cancel).is_err());
        let mut agent = resume(agent);
        agent.turn("hello", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_none());

        // Continuing from the completed command's report leaves no command.
        let tree = agent.tree().unwrap();
        let report = tree
            .entries
            .iter()
            .find(|entry| matches!(&entry.item, TreeItem::Tool(name, _) if name == status::NAME))
            .unwrap()
            .line;
        agent.navigate(report, None, &mut ui, &cancel).unwrap();
        agent.turn("hi", &mut ui, &cancel).unwrap();

        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);
        let last_input = |body: &Value| {
            let input = body["input"].as_array().unwrap();
            input.last().unwrap()["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        assert!(last_input(&bodies[2]).starts_with("Scramjet status check for `rs:work`."));
        let dormant: Vec<_> = bodies
            .iter()
            .map(|body| {
                let instructions = body["instructions"].as_str().unwrap();
                instructions.contains("# Dormant Scramjet Command\n\nThe command `rs:work`")
            })
            .collect();
        assert_eq!(dormant, [false, false, false, false, true, false]);
        assert_eq!(last_input(&bodies[4]), "hello");
        assert_eq!(last_input(&bodies[5]), "hi");
        assert!(
            bodies
                .iter()
                .all(|body| !body.to_string().contains("command_state"))
        );
        let saved: Vec<_> = std::fs::read_to_string(agent.session_path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|line| line["type"] == "command_state")
            .collect();
        let started = json!({"type":"command_state","command":"rs:work"});
        let parked = json!({"type":"command_state","command":"rs:work","state":"parked"});
        let ended = json!({"type":"command_state","command":null});
        assert_eq!(
            saved,
            [&started, &parked, &started, &ended, &started].map(Clone::clone)
        );

        // A fork before the reply waits for it again.
        assert_eq!(agent.fork(1).unwrap().as_deref(), Some("blue"));
        assert_eq!(agent.lifecycle.saved()["state"], "parked");
    }

    #[test]
    fn a_command_is_reported_inline_or_after_scramjets_status_check() {
        let call = |id: &str, name: &str, arguments: Value| {
            let events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text
                .replace("\"inert\"", &format!("\"{name}\""))
                .replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let next = json!([{"message":"/rs:work again","fresh_session":true,"reason":"More."}]);
        let (endpoint, server) = serve(vec![
            stream(&call(
                "call_1",
                status::NAME,
                json!({"summary":"Done.","status":"completed",
                "recommended_next_step":0,"next_steps":next}),
            )),
            reply("worked"),
            stream(&call(
                "call_2",
                status::NAME,
                json!({"summary":"Needs a key.","status":"blocked"}),
            )),
            reply("hello"),
            stream(&call(
                "call_3",
                question::NAME,
                json!({"type":"freetext","message":"Which?"}),
            )),
            reply("used a.txt"),
            stream(&call(
                "call_4",
                status::NAME,
                json!({"summary":"Used it.","status":"completed"}),
            )),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "status");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(
            commands.join("rs:work.md"),
            "---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:work\n---\nWork on $1.\n",
        )
        .unwrap();
        agent.commands = Commands::discover(None, &agent.root().join(".scramjet"));
        agent.autonomy = None;
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work go", &mut ui, &cancel).unwrap();
        let completed = agent.take_completed().unwrap();
        let Next::Offer { steps, recommended } = agent.route(&completed, true).next else {
            panic!("no next step offered");
        };
        assert_eq!(steps[0].command.as_deref(), Some("/rs:work again"));
        assert!(steps[0].fresh_session);
        assert_eq!(recommended, Some(0));
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_none());
        agent.turn("hi", &mut ui, &cancel).unwrap();
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        agent.turn("a.txt", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_some());
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 7);

        let check = bodies[2]["input"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(check["role"], "user");
        assert_eq!(check.as_object().unwrap().len(), 2);
        let check = check["content"].as_str().unwrap();
        assert!(check.starts_with("Scramjet status check for `rs:work`."));
        assert!(check.contains("  - [0] rs:work\n"));
        let dormant = |body: &Value| {
            let instructions = body["instructions"].as_str().unwrap();
            instructions.contains("# Dormant Scramjet Command\n\nThe command `rs:work` is dormant")
        };
        let dormant: Vec<_> = bodies.iter().map(dormant).collect();
        assert_eq!(dormant, [false, false, false, true, false, false, false]);
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        assert!(instructions.contains(&format!("- {}: {}\n", status::NAME, status::SNIPPET)));
        let tools: Vec<_> = bodies[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| &t["name"])
            .collect();
        assert!(tools.contains(&&json!(status::NAME)));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                "→ /rs:work again",
                "status: blocked",
                r#"{"parked":true}"#,
                "status: completed"
            ]
        );
        assert!(
            ui.shown
                .contains(&"notice scramjet: command blocked — Needs a key.".to_owned())
        );
        assert!(
            ui.shown
                .contains(&"call report_scramjet_command_status completed".to_owned())
        );
        // The status checks are neither recalled nor shown again.
        let prompts = ["/rs:work go", "/rs:work", "hi", "/rs:work", "a.txt"];
        assert_eq!(agent.prompts(), prompts);
        let (_, _, resumed) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, prompts);
        let mut replay = Recorder::default();
        agent.show_conversation(&mut replay);
        let shown: Vec<_> = replay
            .shown
            .iter()
            .filter(|s| s.starts_with("prompt "))
            .collect();
        assert_eq!(shown.len(), 5);
        assert_eq!(agent.fork_prompts().unwrap(), prompts);
    }
}
