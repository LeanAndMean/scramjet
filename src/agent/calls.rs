//! A reply's tool calls: which run together, the approval question, the harness's own tools
//! (questions, reports, suggestions, model switches, delegations, publications), and the
//! coding tools and subagents.
use super::tools::{self, Approval, Call, Output};
use super::{
    Agent, AgentError, AgentUi, Answer, DIFF, FAILED, SUBAGENTS, ToolDetail, Usage, delegate,
    forge, publish, question, sandbox, status, subagent, suggest, switch,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

impl Agent {
    /// The calls of a reply that run at the same time, as Pi runs them: when there are several
    /// and each is a coding tool or `subagent` that asks no approval question. Otherwise they
    /// run one after another, as the user's questions, the command's reports and delegations and
    /// the publications (sequential in Scramjet too) need.
    pub(super) fn together<'a>(&self, calls: &'a [Value]) -> Option<Vec<Call<'a>>> {
        let parsed: Vec<Call> = calls
            .iter()
            .map(Call::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let offered = self.offered();
        let runs = |call: &Call| {
            let tool = (call.name == subagent::NAME || tools::CODING_TOOLS.contains(&call.name))
                && offered.contains(&call.name);
            let asks = matches!(call.approval(&self.config.root), Approval::Ask(_));
            tool && !(asks && self.config.ask)
        };
        (parsed.len() > 1 && parsed.iter().all(runs)).then_some(parsed)
    }

    /// The `Tools` that run the agent's calls now.
    pub(super) fn tools(&self) -> Tools<'_> {
        Tools {
            parent: subagent::Parent {
                providers: &self.config.providers,
                retries: self.config.retries,
                model: &self.config.model,
                info: &self.info,
                effort: self.effort(),
                max_output_tokens: self.config.max_output_tokens,
                root: &self.config.root,
                cache_key: self.session.id(),
                sources: subagent::Sources {
                    user: self.agents_directory.as_deref(),
                    sets: &self.sets,
                },
                skills: &self.skills,
                agent_directory: self.agent_directory.as_deref(),
                approvals: self.approvals.as_deref(),
                rates: self.rates.as_deref(),
            },
            hide_credentials: self.hide_credentials,
        }
    }

    /// Adds `output` as the result of the call `item`, with what is shown of it on a resume.
    pub(super) fn add_result(
        &mut self,
        item: &Value,
        output: &Output,
        ui: &mut dyn AgentUi,
    ) -> Result<(), AgentError> {
        let mut result =
            json!({"type":"function_call_output","call_id":item["call_id"],"output":output.sent()});
        if output.failed {
            result[FAILED] = json!(true);
        }
        match &output.detail {
            Some(ToolDetail::Diff(diff)) => result[DIFF] = json!(diff),
            Some(ToolDetail::Subagents(children)) => {
                result[SUBAGENTS] = serde_json::to_value(children).unwrap_or_default()
            }
            _ => {}
        }
        let spent = result.get(SUBAGENTS).is_some();
        self.add(result)?;
        if spent {
            ui.usage(
                self.session_tokens(),
                self.context_tokens(),
                self.output_rate(),
            );
        }
        Ok(())
    }

    /// Answers a call of a reply whose turn stopped before it ran.
    pub(super) fn not_run(&mut self, item: &Value, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
        let name = item["name"].as_str().unwrap_or_default();
        let call = Call::parse(item).ok();
        let summary = call.as_ref().map(Call::summary).unwrap_or_default();
        let compact = call.and_then(|call| call.compact(&self.config.root));
        ui.tool_call(name, &summary, compact.as_ref());
        let output = "Error: the turn was stopped before this tool call ran; it did not run.";
        ui.tool_result(name, output, None, true);
        self.add(
            json!({"type":"function_call_output","call_id":item["call_id"],
            "output":output,FAILED:true}),
        )
    }

    /// Runs a tool call, after any approval; also says whether the turn ends after it, as a
    /// question the user cancelled or is to answer at the prompt ends it.
    pub(super) fn call(
        &mut self,
        item: &Value,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> (Output, bool) {
        let failure = |text: String| Output {
            text,
            detail: None,
            failed: true,
            image: None,
        };
        let call = match Call::parse(item) {
            Ok(call) => call,
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(item["name"].as_str().unwrap_or(""), &output, None, true);
                return (failure(output), false);
            }
        };
        let compact = call.compact(&self.config.root);
        ui.tool_call(call.name, &call.summary(), compact.as_ref());
        if !self.offers(call.name) {
            let output = format!("Tool {} not found", call.name);
            ui.tool_result(call.name, &output, None, true);
            return (failure(output), false);
        }
        if call.name == question::NAME {
            let (output, ends) = ask(&call.arguments, &mut self.lifecycle, ui);
            let freetext = question::Question::parse(&call.arguments)
                .is_ok_and(|question| question.kind == question::Kind::Freetext);
            if freetext {
                self.suggestion = suggest::Pending::Reply;
            }
            return (output, ends);
        }
        if call.name == suggest::NAME {
            let (active, shows) = (self.lifecycle.active(), ui.offers_suggestions());
            let (commands, pending) = (&self.commands, &self.suggestion);
            let (text, failed) =
                match suggest::run(&call.arguments, commands, active, shows, pending) {
                    Ok((suggested, text)) => {
                        self.suggestion = suggest::Pending::Steps(suggested);
                        (text, false)
                    }
                    Err(refused) => refused,
                };
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
                image: None,
            };
            return (output, false);
        }
        if call.name == switch::NAME {
            let (text, failed) = match self.switch_model(&call.arguments) {
                switch::Switched::To(text) => {
                    ui.model_switched(self);
                    (text, false)
                }
                switch::Switched::Not(text, failed) => (text, failed),
            };
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
                image: None,
            };
            return (output, false);
        }
        if call.name == delegate::NAME {
            let active = self.lifecycle.active();
            let output =
                delegate::run(&call.arguments, &self.commands, active, &mut self.delegated);
            ui.tool_result(call.name, &output.text, None, output.failed);
            return (output, false);
        }
        if publish::TOOLS.contains(&call.name) {
            let policy = publish::Policy {
                commands: &self.commands,
                active: self.lifecycle.active(),
                autonomy: self.autonomy.as_deref(),
            };
            let mut forge = forge::System {
                root: &self.config.root,
                cancel,
            };
            let output = publish::run(call.name, &call.arguments, &policy, ui, &mut forge, cancel);
            let detail = output.detail.as_ref();
            ui.tool_result(call.name, &output.text, detail, output.failed);
            return (output, false);
        }
        if call.name == status::NAME {
            let (text, failed, ends) = match self.lifecycle.report(&call.arguments) {
                status::Reported::Answer(text, failed, ends) => (text, failed, ends),
                status::Reported::Abandon(abandon) => {
                    ui.question(&abandon.message());
                    let choices = status::Abandon::choices();
                    let chosen = ui.choose(&abandon.title(), &choices, Some(1));
                    self.lifecycle.abandon(&abandon, chosen)
                }
            };
            ui.tool_result(call.name, &text, None, failed);
            let output = Output {
                text,
                detail: None,
                failed,
                image: None,
            };
            return (output, ends);
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
        let (output, usage) = self.tools().run(&call, ui, cancel);
        self.usage.add(&usage);
        (output, false)
    }
}

/// What running coding tools and `subagent` calls takes from the agent, shared by the calls
/// that run at the same time.
pub(super) struct Tools<'a> {
    parent: subagent::Parent<'a>,
    hide_credentials: bool,
}

impl Tools<'_> {
    /// Runs `call` and shows its result; with the usage of its children, if any.
    pub(super) fn run(
        &self,
        call: &Call,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> (Output, Usage) {
        let (output, usage) = match call.name {
            subagent::NAME => subagent::run(&call.arguments, &self.parent, ui, cancel),
            _ => {
                ui.tool_running(std::time::Instant::now());
                let named = self.parent.providers.credential_files();
                let hidden = (self.hide_credentials).then(|| sandbox::Hidden::current(named));
                let root = self.parent.root;
                let output = call
                    .run(root, hidden.as_ref(), cancel, &mut |output| {
                        ui.tool_output(output)
                    })
                    .unwrap_or_else(|error| Output {
                        text: format!("Error: {error}"),
                        detail: None,
                        failed: true,
                        image: None,
                    });
                (output, Usage::default())
            }
        };
        ui.tool_result(
            call.name,
            &output.text,
            output.detail.as_ref(),
            output.failed,
        );
        (output, usage)
    }
}

/// Asks the user the question of a `get_scramjet_user_input` call, as Scramjet's tool does. An
/// answer is the user's input to the active command; a freetext question or a dismissed one
/// leaves it waiting for their reply.
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
                image: None,
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
    let parked = match (&question.kind, chosen) {
        (question::Kind::Freetext, _) => lifecycle.wait(),
        (_, None) => {
            lifecycle.wait();
            false
        }
        (_, Some(_)) => {
            lifecycle.answered();
            false
        }
    };
    let (text, ends) = question.answer(chosen, parked);
    let detail = question::shown(arguments, &text).map(|(_, shown)| ToolDetail::Answer(shown));
    ui.tool_result(question::NAME, &text, detail.as_ref(), false);
    let output = Output {
        text,
        detail,
        failed: false,
        image: None,
    };
    (output, ends)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{
        ChildItem, ChildState, Delegation, PRODUCER, Providers, Session, approvals, instructions,
        show_items,
    };
    use crate::provider_fixture as fixture;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn a_command_delegates_to_another_once_per_turn_for_the_same_arguments() {
        let delegate = |id: &str, args: &str| {
            let arguments = json!({"command":"rs:read","args":args});
            tool_call(id, delegate::NAME, arguments)
        };
        let done = || stream(&fixture::text_events("done", fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            delegate("call_1", "42"),
            delegate("call_2", "43"),
            delegate("call_3", "42"),
            tool_call(
                "call_4",
                status::NAME,
                json!({"summary":"Read.","status":"completed"}),
            ),
            delegate("call_5", "42"),
            done(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "delegate");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("rs:top.md"), "Read the issues.\n").unwrap();
        let helper = "---\ndelegate-only: true\n---\nRead issue $1.\n";
        std::fs::write(commands.join("rs:read.md"), helper).unwrap();
        project_sets(&mut agent);
        agent.autonomy = None;
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:top", &mut ui, &cancel).unwrap();
        // The next prompt starts a turn in which the command runs again.
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let read = |issue| {
            format!("<scramjet-command name=\"rs:read\">\nRead issue {issue}.\n</scramjet-command>")
        };
        let outputs: Vec<_> = [1, 2, 3, 5].map(|at| last_output(&bodies[at])).into();
        assert_eq!(bodies.len(), 6);
        let refused = "ERROR: cycle detected in delegation chain rs:read -> rs:read -> rs:read: \
'rs:read' was already delegated with these arguments in this turn. Refusing to recurse; its \
instructions are in that earlier result.";
        assert_eq!(outputs, [read(42), read(43), refused.into(), read(42)]);
        let shown: Vec<_> = (ui.shown.iter())
            .filter(|shown| shown.contains("delegate"))
            .map(|shown| shown.lines().next().unwrap())
            .collect();
        assert_eq!(
            shown,
            [
                "call delegate /rs:read 42",
                "result delegate <scramjet-command name=\"rs:read\">",
                "call delegate /rs:read 43",
                "result delegate <scramjet-command name=\"rs:read\">",
                "call delegate /rs:read 42",
                &format!("result delegate {refused} failed"),
                "call delegate /rs:read 42",
                "result delegate <scramjet-command name=\"rs:read\">",
            ]
        );
    }

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
        // The child has the parent's model, and so its prices.
        agent.info.prices = prices(1_000.0, 100.0, 10_000.0);
        let rates = agent.root().join("output-rates.json");
        agent.rates = Some(rates.clone());
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("review", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 5);
        // The child's replies keep their rates with the parent's.
        let saved: Value = serde_json::from_slice(&std::fs::read(&rates).unwrap()).unwrap();
        assert_eq!(saved["samples"].as_array().unwrap().len(), 5);
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
        assert!(
            ui.shown
                .contains(&format!("result subagent {result} {:?}", ui.details[0]))
        );
        // The child as it started, after each response, call and text, and as it ended.
        let started = &ui.children[0].runs[0];
        assert_eq!(
            (started.state.clone(), started.items.len()),
            (ChildState::Running, 0)
        );
        let call = |name: &str, summary: &str| ChildItem::Call {
            name: name.into(),
            summary: summary.into(),
        };
        let items = vec![
            call("bash", "cat kept.txt; echo changed > kept.txt"),
            call("write", "kept.txt"),
            ChildItem::Text("kept.txt says original".into()),
        ];
        let ToolDetail::Subagents(ended) = &ui.details[0] else {
            panic!("{:?}", ui.details);
        };
        let run = &ended.runs[0];
        assert_eq!(ended.mode, Delegation::Single);
        assert_eq!(
            (
                &run.agent[..],
                &run.source[..],
                &run.model[..],
                &run.task[..]
            ),
            ("reviewer", "user", "m", "Check kept.txt\nCarefully.")
        );
        assert_eq!((&run.state, &run.items), (&ChildState::Done, &items));
        assert_eq!(run.usage.turns, 3);
        // Each response: 7 input tokens not from the cache, 3 from it and 5 output tokens.
        let each = 7e-3 + 3e-4 + 5e-2;
        close(run.usage.cost.unwrap(), 3.0 * each);
        // The session's cost has the child's, its tokens only the parent's; the footer is told
        // after each response and after the child's result.
        let costs: Vec<_> = ui.usage.iter().map(|(tokens, _)| tokens.cost).collect();
        assert_eq!(costs.len(), 3);
        for (cost, expected) in costs.into_iter().zip([1.0, 4.0, 5.0]) {
            close(cost, expected * each);
        }
        assert_eq!(agent.session_tokens().input, 14);
        close(agent.usage.cost, 5.0 * each);
        assert_eq!(
            run.warnings,
            ["write: left out, as subagents run read-only here"]
        );
        assert_eq!(ui.children.last(), Some(ended));
        assert_eq!(ui.children.len(), 1 + 3 + 3 + 1);
        // Stored with the result, so that a resumed conversation shows them.
        let mut resumed = Recorder::default();
        show_items(agent.conversation(), None, &mut resumed);
        assert_eq!(resumed.details, ui.details);
    }

    #[test]
    fn the_stores_providers_name_are_hidden_from_the_agent_and_its_subagents() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let peek = json!({"command":"cat store/token; echo ran"});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", peek.clone()),
            tool_call(
                "call_2",
                subagent::NAME,
                json!({"agent":"reviewer","task":"Peek."}),
            ),
            tool_call("call_3", "bash", peek),
            tool_call("call_4", "read", json!({"path":"store/token"})),
            reply("peeked"),
            reply("done"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "named-stores");
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews\ntools: read, bash\n---\n\nYou review.\n",
        )
        .unwrap();
        agent.agents_directory = Some(agents);
        let store = agent.root().join("store");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("token"), "SECRET-named\n").unwrap();
        let mut document = served(endpoint);
        document["credentialFiles"] = json!([store]);
        agent.set_providers(Providers::given(document));
        agent.set_hide_credentials(true);
        let cancel = AtomicBool::new(false);
        agent
            .turn("peek", &mut Recorder::default(), &cancel)
            .unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);
        let (parent, child) = (last_output(&bodies[1]), last_output(&bodies[3]));
        for output in [&parent, &child] {
            assert!(
                output.ends_with("ran\n") && !output.contains("SECRET"),
                "{output}"
            );
        }
        let refused = format!(
            "{}: hidden, as it holds credentials",
            store.join("token").display()
        );
        assert!(
            last_output(&bodies[4]).contains(&refused),
            "{}",
            last_output(&bodies[4])
        );
    }

    #[test]
    fn a_subagent_reply_at_the_output_limit_returns_its_text_as_a_failure_without_its_call() {
        let reply = |blocks: Vec<Value>, reason: &str| {
            let mut events = vec![
                json!({"type":"message_start","message":{"id":"msg","usage":{"input_tokens":3,"output_tokens":1}}}),
            ];
            for (index, block) in blocks.into_iter().enumerate() {
                events.push(
                    json!({"type":"content_block_start","index":index,"content_block":block}),
                );
                events.push(json!({"type":"content_block_stop","index":index}));
            }
            events.push(json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":5}}));
            events.push(json!({"type":"message_stop"}));
            messages_stream(&events)
        };
        let call = |id: &str, name: &str, input: Value| json!({"type":"tool_use","id":id,"name":name,"input":input});
        let (endpoint, server) = serve(vec![
            reply(
                vec![call(
                    "toolu_1",
                    subagent::NAME,
                    json!({"agent":"reviewer","task":"Look"}),
                )],
                "tool_use",
            ),
            reply(
                vec![
                    json!({"type":"text","text":"Found two"}),
                    call("toolu_2", "bash", json!({"command":"touch ran"})),
                ],
                "max_tokens",
            ),
            reply(vec![json!({"type":"text","text":"done"})], "end_turn"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent-limit");
        agent.set_model("claude-opus-5-5").unwrap();
        let agents = agent.root().join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews\ntools: bash\n---\n\nYou review.\n",
        )
        .unwrap();
        agent.agents_directory = Some(agents);
        let mut ui = Recorder::default();
        agent
            .turn("review", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 3);
        assert_eq!(
            bodies[2]["messages"][2]["content"][0]["content"],
            "Agent error: response incomplete: it reached the output token limit (max_tokens); \
the tool call it was making did not run"
        );
        assert!(!agent.root().join("ran").exists());
        let ToolDetail::Subagents(ended) = &ui.details[0] else {
            panic!("{:?}", ui.details);
        };
        let run = &ended.runs[0];
        assert_eq!(run.items, [ChildItem::Text("Found two".into())]);
        assert!(
            matches!(&run.state, ChildState::Failed { reason, .. } if reason.as_deref() == Some("error")),
            "{:?}",
            run.state
        );
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
        let shown: Vec<_> = (ui.details.iter())
            .map(|detail| {
                let ToolDetail::Subagents(children) = detail else {
                    panic!("{detail:?}");
                };
                let runs = children.runs.iter().map(|run| {
                    let failed = matches!(run.state, ChildState::Failed { reason: None, .. });
                    (
                        &run.agent[..],
                        &run.source[..],
                        run.effort.as_deref(),
                        failed,
                    )
                });
                (children.mode, runs.collect::<Vec<_>>())
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    Delegation::Parallel,
                    vec![
                        ("a", "user", Some("low"), false),
                        ("nope", "unknown", None, true),
                        ("b", "user", Some("low"), false)
                    ]
                ),
                (
                    Delegation::Chain,
                    vec![
                        ("a", "user", Some("low"), false),
                        ("b", "user", Some("minimal"), false)
                    ]
                )
            ]
        );
        // A chain's steps are shown as they start.
        let steps: Vec<usize> = (ui.children.iter())
            .filter(|children| children.mode == Delegation::Chain)
            .map(|children| children.runs.len())
            .collect();
        assert_eq!((steps.first(), steps.last()), (Some(&1), Some(&2)));
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
for trusted repositories. Yes is remembered for this project until one of its agent files is added \
or changed.",
            project.display()
        );
        assert!(ui.shown.contains(&asked), "{:?}", ui.shown);
        assert!(
            ui.shown
                .contains(&"choose Run project-local agents? [\"Yes\", \"No\"] None".to_owned())
        );
        assert!(!agent.root().join("approved-agents.json").exists());
    }

    #[test]
    fn approving_project_agents_is_remembered_until_one_is_added_or_changed() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let call = |id: &str| {
            let arguments = json!({"agent":"p","task":"go","agentScope":"project"});
            tool_call(id, subagent::NAME, arguments)
        };
        let (endpoint, server) = serve(vec![
            call("call_1"),
            reply("p ran"),
            call("call_2"),
            reply("p ran again"),
            reply("one"),
            call("call_3"),
            reply("two"),
            call("call_4"),
            reply("p ran with q there"),
            reply("three"),
            call("call_5"),
            reply("four"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "subagent-approvals");
        let project = agent.root().join(".scramjet/agents");
        std::fs::create_dir_all(&project).unwrap();
        let p = "---\nname: p\ndescription: P\ntools: read\n---\n";
        let q = "---\nname: q\ndescription: Q\n---\n";
        std::fs::write(project.join("p.md"), p).unwrap();
        let mut ui = Recorder {
            choices: vec![Some(0), Some(1), Some(0), None],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        std::fs::write(project.join("q.md"), q).unwrap();
        agent.turn("two", &mut ui, &cancel).unwrap();
        agent.turn("three", &mut ui, &cancel).unwrap();
        std::fs::write(project.join("p.md"), format!("{p}Changed.\n")).unwrap();
        agent.turn("four", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let canceled = "Canceled: project-local agents not approved.";
        let outputs: Vec<String> = [2, 4, 6, 9, 11].map(|i| last_output(&bodies[i])).into();
        assert_eq!(
            outputs,
            [
                "p ran",
                "p ran again",
                canceled,
                "p ran with q there",
                canceled
            ]
        );
        let asked = |changed: &str| {
            format!(
                "question Agents: p\nSource: {}{changed}\n\nProject agents are repo-controlled. \
Only continue for trusted repositories. Yes is remembered for this project until one of its agent \
files is added or changed.",
                project.display()
            )
        };
        let questions: Vec<&String> = (ui.shown.iter())
            .filter(|shown| shown.starts_with("question "))
            .collect();
        let added = asked("\nAdded or changed since you approved them: q");
        let changed = asked("\nAdded or changed since you approved them: p");
        assert_eq!(questions, [&asked(""), &added, &added, &changed]);
        let remembered =
            std::fs::read_to_string(agent.root().join("approved-agents.json")).unwrap();
        let remembered: Value = serde_json::from_str(&remembered).unwrap();
        let file = |name: &str| project.join(name).display().to_string();
        assert_eq!(
            remembered,
            json!({file("p.md"): approvals::digest(p), file("q.md"): approvals::digest(q)})
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
        agent.instructions = instructions::agent(
            agent.root(),
            &agent.offered(),
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
        );
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
                "create_issue",
                "create_pr",
                "add_issue_comment",
                "add_pr_comment",
                question::NAME,
                delegate::NAME,
                subagent::NAME,
                switch::NAME,
                suggest::NAME
            ]
        );
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        for line in [
            "- read: Read file contents\n- grep: Search file contents for patterns (respects \
.gitignore)\n- ls: List directory contents\n- report_scramjet_command_status:",
            "Guidelines:\n- Use read to examine files instead of cat or sed.\n- When idle in an \
interactive session, prefer `suggest_scramjet_next_steps`",
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
    fn the_tools_the_command_line_gives_are_the_only_ones_offered_and_run() {
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("read", json!({"path":"a.txt"})),
                (subagent::NAME, json!({"agent":"x","task":"y"})),
            ]),
            tool_call("call_2", "bash", json!({"command":"touch x.txt"})),
            tool_call(
                "call_3",
                question::NAME,
                json!({"type":"confirm","message":"Go?"}),
            ),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "only-tools");
        agent.config.only_tools = Some(["grep", "read", "nope"].map(String::from).to_vec());
        agent.instructions = instructions::agent(
            agent.root(),
            &agent.offered(),
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
        );
        std::fs::write(agent.root().join("a.txt"), "a\n").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("look", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        let names: Vec<_> = (bodies[0]["tools"].as_array().unwrap().iter())
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["read", "grep"]);
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        let listed = "Available tools:\n- read: Read file contents\n- grep: Search file contents for \
patterns (respects .gitignore)\n\nGuidelines:\n- Use read to examine files instead of cat or \
sed.\n- Be concise";
        assert!(instructions.contains(listed), "{instructions}");
        let outputs: Vec<_> = (agent.conversation().iter())
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                "a\n",
                "Tool subagent not found",
                "Tool bash not found",
                "Tool get_scramjet_user_input not found"
            ]
        );
        assert!(!agent.root().join("x.txt").exists());
        assert!(!ui.shown.iter().any(|shown| shown.contains("Go?")));
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
    fn calls_after_the_one_a_stop_interrupted_are_answered_as_not_run() {
        // With a question among them, a reply's calls run one after another.
        let question = json!({"type":"confirm","message":"Go on?"});
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", json!({"command":"sleep 30"})),
                (question::NAME, question),
            ]),
            stream(&fixture::text_events("ok", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "stopped-calls");
        let mut ui = Recorder::default();
        let cancel = stop_after(Duration::from_millis(300));
        let stopped = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(stopped, Err(AgentError::Cancelled)), "{stopped:?}");
        let not_run = "Error: the turn was stopped before this tool call ran; it did not run.";
        assert_eq!(ui.calls, ["bash", question::NAME]);
        assert!(!ui.shown.iter().any(|shown| shown.starts_with("question")));
        assert_eq!(
            ui.shown.last().unwrap(),
            &format!("result {} {not_run} failed", question::NAME)
        );
        cancel.store(false, Ordering::Relaxed);
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies[0]["parallel_tool_calls"], true);
        let outputs = outputs(&bodies[1]);
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].0, "call_0");
        assert_eq!(outputs[1], ("call_1".into(), not_run.into()));
    }

    /// A flag set after `delay`, as when the user stops the turn then.
    fn stop_after(delay: Duration) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            setter.store(true, Ordering::Relaxed);
        });
        cancel
    }

    #[test]
    fn a_replys_calls_run_together_and_show_one_after_another_in_their_order() {
        // Each command waits up to 5 s for the other's file, which one after the other the first
        // would not see.
        let meet = |mine: &str, theirs: &str| {
            json!({"command":format!("touch {mine}; for _ in $(seq 100); do [ -e {theirs} ] && \
break; sleep 0.05; done; [ -e {theirs} ] && echo met || echo alone")})
        };
        let edit = |path: &str, old: &str, new: &str| json!({"path":path,"edits":[{"oldText":old,"newText":new}]});
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", meet("a", "b")),
                ("write", json!({"path":"f.txt","content":"one\n"})),
                ("edit", edit("f.txt", "one", "two")),
                ("edit", edit("./f.txt", "two", "three")),
                ("bash", meet("b", "a")),
            ]),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-together");
        let mut ui = Recorder::default();
        let started = std::time::Instant::now();
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(4));
        // The edits of one file ran in their order.
        let file = agent.root().join("f.txt");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "three\n");
        let shown: Vec<String> = (ui.shown.iter())
            .map(|shown| shown.splitn(3, ' ').take(2).collect::<Vec<_>>().join(" "))
            .collect();
        let each = ["bash", "write", "edit", "edit", "bash"]
            .map(|name| [format!("call {name}"), format!("result {name}")]);
        let mut expected: Vec<String> = each.concat();
        expected.push("reply done".into());
        assert_eq!(shown, expected);
        assert!(!ui.shown.iter().any(|shown| shown.ends_with(" failed")));
        let bodies = server.join().unwrap();
        let outputs = outputs(&bodies[1]);
        let ids: Vec<&str> = outputs.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["call_0", "call_1", "call_2", "call_3", "call_4"]);
        assert_eq!(
            (outputs[0].1.as_str(), outputs[4].1.as_str()),
            ("met\n", "met\n")
        );
    }

    #[test]
    fn a_question_of_calls_running_together_waits_for_the_calls_before_it() {
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", json!({"command":"sleep 0.3; echo slept"})),
                (
                    subagent::NAME,
                    json!({"agent":"p","task":"go","agentScope":"both"}),
                ),
            ]),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-together-question");
        let project = agent.root().join(".scramjet/agents");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("p.md"), "---\nname: p\ndescription: P\n---\n").unwrap();
        let mut ui = Recorder {
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        let shown: Vec<&str> = (ui.shown.iter())
            .map(|shown| {
                shown
                    .split_once(['\n', ':'])
                    .map_or(shown.as_str(), |(head, _)| head)
            })
            .collect();
        assert_eq!(
            shown,
            [
                "call bash sleep 0.3; echo slept",
                "result bash slept",
                "call subagent p [both]",
                "question Agents",
                "choose Run project-local agents? [\"Yes\", \"No\"] None",
                "result subagent Canceled",
                "reply done",
            ]
        );
        let bodies = server.join().unwrap();
        assert_eq!(
            last_output(&bodies[1]),
            "Canceled: project-local agents not approved."
        );
    }

    #[test]
    fn calls_running_together_show_each_as_it_runs_where_the_interface_can() {
        let meet = |mine: &str, theirs: &str| {
            json!({"command":format!("touch {mine}; for _ in $(seq 100); do [ -e {theirs} ] && \
break; sleep 0.05; done; [ -e {theirs} ] && echo met || echo alone")})
        };
        let (endpoint, server) = serve(vec![
            tool_calls(&[("bash", meet("a", "b")), ("bash", meet("b", "a"))]),
            stream(&fixture::text_events("met", fixture::known_usage())),
            tool_calls(&[
                ("bash", json!({"command":"sleep 0.5; echo slept"})),
                (
                    subagent::NAME,
                    json!({"agent":"p","task":"go","agentScope":"both"}),
                ),
            ]),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-shown-together");
        let mut ui = Recorder {
            together: true,
            choices: vec![Some(1)],
            ..Recorder::default()
        };
        agent
            .turn("meet", &mut ui, &AtomicBool::new(false))
            .unwrap();
        // Each call's display, after the `of` that says it is that call's.
        let each = |shown: &[String]| {
            let mut calls: Vec<Vec<String>> = vec![Vec::new(), Vec::new()];
            let mut of = None;
            for shown in shown {
                match shown.strip_prefix("of ") {
                    Some(index) => of = index.parse::<usize>().ok(),
                    None => calls[of.unwrap()]
                        .push(shown.split(['\n', ':', ';']).next().unwrap().into()),
                }
            }
            calls
        };
        let (first, last) = (ui.shown.first().unwrap(), ui.shown.last().unwrap());
        assert_eq!((first.as_str(), last.as_str()), ("2 calls", "reply met"));
        let calls = &ui.shown[1..ui.shown.len() - 2];
        assert_eq!(ui.shown[ui.shown.len() - 2], "of none");
        let results = calls.iter().filter(|shown| shown.starts_with("result"));
        assert_eq!(results.count(), 2);
        // They met: both had started before either ended.
        let started = calls.iter().rposition(|shown| shown.starts_with("call"));
        let ended = calls.iter().position(|shown| shown.starts_with("result"));
        assert!(started < ended, "{calls:?}");
        let meeting = ["call bash touch a", "result bash met"];
        assert_eq!(each(calls)[0], meeting);
        assert_eq!(each(calls)[1], ["call bash touch b", "result bash met"]);
        // A question among them is asked at once, while the call before it runs.
        let project = agent.root().join(".scramjet/agents");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("p.md"), "---\nname: p\ndescription: P\n---\n").unwrap();
        ui.shown.clear();
        agent.turn("ask", &mut ui, &AtomicBool::new(false)).unwrap();
        let calls = each(&ui.shown[1..ui.shown.len() - 2]);
        assert_eq!(calls[0], ["call bash sleep 0.5", "result bash slept"]);
        assert_eq!(
            calls[1],
            [
                "call subagent p [both]",
                "question Agents",
                "choose Run project-local agents? [\"Yes\", \"No\"] None",
                "result subagent Canceled",
            ]
        );
        let asked = ui
            .shown
            .iter()
            .position(|shown| shown.starts_with("question"));
        let slept = (ui.shown.iter()).position(|shown| shown.starts_with("result bash slept"));
        assert!(asked < slept, "{:?}", ui.shown);
        server.join().unwrap();
    }

    #[test]
    fn a_stop_during_calls_running_together_stops_each_with_its_own_result() {
        let (endpoint, server) = serve(vec![
            tool_calls(&[
                ("bash", json!({"command":"sleep 30"})),
                ("write", json!({"path":"x.txt","content":"x"})),
            ]),
            stream(&fixture::text_events("ok", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "calls-together-stopped");
        let mut ui = Recorder::default();
        let cancel = stop_after(Duration::from_millis(300));
        let stopped = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(stopped, Err(AgentError::Cancelled)), "{stopped:?}");
        // As in Pi, the write ran beside the command.
        assert!(agent.root().join("x.txt").exists());
        cancel.store(false, Ordering::Relaxed);
        agent.turn("again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let outputs = outputs(&bodies[1]);
        assert_eq!(outputs.len(), 2);
        assert!(outputs[0].1.starts_with("Command cancelled"), "{outputs:?}");
        assert!(outputs[1].1.starts_with("Wrote 1 bytes"), "{outputs:?}");
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
    fn the_agent_switches_the_model_for_its_next_requests_as_scramjets_tool_does() {
        let switch = |provider: &str, model: &str| {
            (switch::NAME, json!({"provider":provider,"model":model}))
        };
        let (endpoint, server) = serve_requests(vec![
            tool_calls(&[
                (switch::NAME, json!({"model":3})),
                switch("anthropic", "gpt-x"),
                switch("openai", "m"),
                switch("openai", "gpt-x"),
            ]),
            stream(&fixture::text_events("Done.", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "switch-model");
        let base = format!("http://{endpoint}/v1");
        agent.set_providers(crate::agent::Providers::given(json!({"providers": {
            "openai": {"baseUrl": base, "api": "openai-responses", "models": [
                {"id": "m", "name": "M"},
                {"id": "gpt-x", "name": "GPT X", "contextWindow": 400_000},
            ]},
            "anthropic": {"baseUrl": base, "api": "anthropic-messages",
                "models": [{"id": "claude-y", "name": "Claude Y"}]},
        }})));
        agent.set_model_scope(vec!["m".into(), "claude-y".into()]);
        let mut ui = Recorder::default();
        (agent.turn("switch", &mut ui, &AtomicBool::new(false))).unwrap();
        let requests = server.join().unwrap();
        let (first, last) = (&requests[0].1, &requests[1].1);
        assert_eq!(
            (first["model"].as_str(), last["model"].as_str()),
            (Some("m"), Some("gpt-x"))
        );
        let identity =
            "Your model for the imminent response is: GPT X (ID: gpt-x, provider: openai).";
        assert!(last["instructions"].as_str().unwrap().contains(identity));
        let results: Vec<String> = outputs(last).into_iter().map(|(_, text)| text).collect();
        assert!(
            results[0].starts_with(
                "Validation failed for tool \"switch_scramjet_model\":\n  - provider: must have \
required property\n  - model: must be string\n\nReceived arguments:"
            ),
            "{}",
            results[0]
        );
        // An unknown model lists the user's scope; a model of another provider is unknown.
        assert_eq!(
            results[1..],
            [
                "Unknown model \"anthropic/gpt-x\". The model was not switched.\nAvailable models \
(provider/id):\n- anthropic/claude-y (Claude Y)\n- openai/m (M)",
                "Already on M (openai/m). No switch needed.",
                "Switched to GPT X (ID: gpt-x, provider: openai).",
            ]
        );
        let failed: Vec<bool> = (agent.conversation().iter())
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| item[FAILED] == true)
            .collect();
        assert_eq!(failed, [true, false, false, false]);

        // The interface heard of the one switch; the reply after it is the new model's, which
        // the session keeps for a resume, with its window.
        let switched: Vec<_> = (ui.shown.iter())
            .filter(|shown| shown.starts_with("switched"))
            .collect();
        assert_eq!(switched, ["switched gpt-x"]);
        let reply = agent.conversation().last().unwrap();
        assert_eq!(reply[PRODUCER], "gpt-x");
        assert_eq!(
            (agent.model(), agent.context_window()),
            ("gpt-x", Some(400_000))
        );
        let (session, ..) = Session::open(agent.session_path()).unwrap();
        assert_eq!(session.model(), Some("gpt-x"));
    }
}
