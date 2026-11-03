//! A turn: model requests and tool calls from a prompt to the final answer, the command it
//! runs, and the `!` commands the user runs without one.
use super::tools::{self, Approval, Call, Output};
use super::{
    Agent, AgentError, AgentUi, Answer, AutoCompaction, CONTEXT, Completed, DIFF, FAILED,
    MAX_STEPS, PROBE, PRODUCER, Purpose, Routing, SUBAGENTS, Step, TYPED, ToolDetail, USAGE,
    autonomy, delegate, forge, message_text, publish, question, sandbox, status, subagent,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

/// A `!` or `!!` command the user ran, which `Agent::add_shell` adds to the conversation: run
/// while a turn runs, it waits for the turn to end, as Scramjet keeps it from coming between a
/// tool call and its result.
pub struct Shell {
    command: String,
    /// Added to the conversation (`!`), or only shown (`!!`).
    include: bool,
    ran: Result<tools::Ran, String>,
}

impl Shell {
    /// Runs `command` with bash in `root` until it ends or `cancel` is set; `progress` receives
    /// the end of its output as it grows.
    pub fn run(
        root: &std::path::Path,
        command: &str,
        include: bool,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Self {
        Self {
            command: command.to_owned(),
            include,
            ran: tools::run_command(root, command, None, cancel, progress),
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// What it showed, as its result shows it, and whether it failed.
    pub fn shown(&self) -> (String, bool) {
        match &self.ran {
            Ok(ran) => {
                let shown = ran.tool_output();
                (shown.text, shown.failed)
            }
            Err(error) => (format!("Error: {error}"), true),
        }
    }
}

impl Agent {
    /// Runs one user prompt to completion: model requests and tool calls until a final answer.
    /// As Scramjet does, the conversation is compacted before the prompt and after the reply if
    /// its context passed the model's window minus `compaction::RESERVE_TOKENS`. A request that
    /// fails because its input exceeded the window is compacted and sent again, once per turn.
    /// When `ui` asks for a pause, the turn returns `AgentError::Paused` for `resume_turn`.
    pub fn turn(
        &mut self,
        prompt: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        self.compact_before_prompt(ui, cancel)?;
        self.completed = None;
        self.delegated.clear();
        let message = self.user_message(prompt, ui);
        self.add(message)?;
        self.save_command()?;
        self.prompts.push(prompt.to_owned());
        self.resume_turn(ui, cancel)
    }

    /// The message `prompt` sends, typed or queued during a turn, starting the command it names
    /// as Scramjet's input handling does: a command or skill prompt sends its expansion, and the
    /// prompt as typed is kept for recall and display. A skill whose file cannot be read now is
    /// said so, and the prompt sent as typed, as Pi does.
    fn user_message(&mut self, prompt: &str, ui: &mut dyn AgentUi) -> Value {
        let command = (self.commands.named(prompt)).map(|command| command.name.clone());
        let skill = command
            .is_none()
            .then(|| self.skills.expand(prompt))
            .flatten();
        self.lifecycle.prompted(match (&command, &skill) {
            (Some(name), _) => status::Prompt::Command(name),
            (None, None) if prompt.starts_with('/') => status::Prompt::Unknown,
            _ => status::Prompt::Reply,
        });
        let sent = match self.commands.expand(prompt) {
            Some(sent) => Some(sent),
            None => match skill {
                Some(Ok(sent)) => Some(sent),
                Some(Err(error)) => {
                    ui.notice(&error);
                    None
                }
                None => None,
            },
        };
        match sent {
            Some(sent) => json!({"role":"user","content":sent,TYPED:prompt}),
            None => json!({"role":"user","content":prompt}),
        }
    }

    /// Goes on with the turn that `turn` or this returned `AgentError::Paused` from, before its
    /// next request.
    pub fn resume_turn(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let mut ran = self.run(ui, cancel);
        // A command's reply that ends without a report gets a status check.
        loop {
            match ran {
                Err(AgentError::Paused) => return Err(AgentError::Paused),
                Err(error) => {
                    if let Some(notice) = self.lifecycle.interrupted() {
                        ui.notice(&notice);
                    }
                    self.save_command()?;
                    return Err(error);
                }
                Ok(()) => {}
            }
            let ended = self.lifecycle.turn_ended(&self.commands, self.autopilot);
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
    /// edges; `interactive` when the user can choose from a list, `autopilot` when the
    /// recommended command runs unless the user chooses otherwise (usually `self.autopilot()`).
    /// What is said about the `autonomy.yaml` files is said once, until it changes, as in
    /// Scramjet.
    pub fn route(&mut self, completed: &Completed, interactive: bool, autopilot: bool) -> Routing {
        let mut edges =
            autonomy::Edges::load(self.autonomy.as_deref(), &self.config.root, &self.commands);
        if edges.diagnostics == self.autonomy_diagnostics {
            edges.diagnostics.clear();
        } else {
            self.autonomy_diagnostics = edges.diagnostics.clone();
        }
        status::route(completed, &self.commands, &edges, interactive, autopilot)
    }

    /// Adds Scramjet's record of the next step chosen from `steps` from the command `source`
    /// (`None`: the list was dismissed) to the conversation, as Scramjet invokes its harness
    /// tool: a call the model never made and its result, which the model sees. Returns the
    /// record as the result has it.
    pub fn record_next_step(
        &mut self,
        source: &str,
        steps: &[Step],
        chosen: Option<usize>,
    ) -> Result<String, AgentError> {
        let record = status::record(source, steps, chosen);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let call_id = format!("harness-tool-{nanos}");
        let arguments = status::record_arguments(source, steps, chosen).to_string();
        self.add(
            json!({"type":"function_call","call_id":call_id,"name":status::RECORD,
            "arguments":arguments}),
        )?;
        self.add(json!({"type":"function_call_output","call_id":call_id,"output":record}))?;
        Ok(record)
    }

    /// Model requests and tool calls until a final answer. As in Pi, messages the user sends
    /// meanwhile go to the model before its next request, and those queued for the end keep
    /// the turn going when it would end.
    fn run(&mut self, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> Result<(), AgentError> {
        let mut overflow_recovered = false;
        for _ in 0..MAX_STEPS {
            if cancel.load(Ordering::Relaxed) {
                return Err(AgentError::Cancelled);
            }
            let steering = ui.steering();
            self.add_queued(steering, ui)?;
            if let Some(level) = ui.effort() {
                self.set_effort(level)?;
            }
            if ui.pause() {
                return Err(AgentError::Paused);
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
                if self.continue_queued(ui)? {
                    continue;
                }
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
                match &output.detail {
                    Some(ToolDetail::Diff(diff)) => result[DIFF] = json!(diff),
                    Some(ToolDetail::Subagents(children)) => {
                        result[SUBAGENTS] = serde_json::to_value(children).unwrap_or_default()
                    }
                    _ => {}
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
                if self.continue_queued(ui)? {
                    continue;
                }
                return Ok(());
            }
        }
        Err(AgentError::TooManySteps)
    }

    /// Where the turn would end: adds the steering messages queued, else the follow-ups; returns
    /// whether there were any, so the turn goes on.
    fn continue_queued(&mut self, ui: &mut dyn AgentUi) -> Result<bool, AgentError> {
        let mut messages = ui.steering();
        if messages.is_empty() {
            messages = ui.follow_up();
        }
        let any = !messages.is_empty();
        self.add_queued(messages, ui)?;
        Ok(any)
    }

    /// Adds messages the user queued during the turn as typed prompts are added: a command
    /// starts there, replacing the one running, as in Scramjet.
    fn add_queued(
        &mut self,
        messages: Vec<String>,
        ui: &mut dyn AgentUi,
    ) -> Result<(), AgentError> {
        if messages.is_empty() {
            return Ok(());
        }
        for prompt in messages {
            let message = self.user_message(&prompt, ui);
            match message.get(TYPED) {
                Some(_) => {
                    ui.command_prompt(&prompt, message["content"].as_str().unwrap_or_default())
                }
                None => ui.user_prompt(&prompt),
            }
            self.add(message)?;
            self.prompts.push(prompt);
        }
        self.save_command()
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
        let shell = Shell::run(&self.config.root, command, include, cancel, &mut |output| {
            ui.tool_output(output)
        });
        self.add_shell(shell, ui)
    }

    /// Shows how `shell`, run apart from the agent, ended, and adds it to the conversation as
    /// `shell` says.
    pub fn add_shell(&mut self, shell: Shell, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
        let (text, failed) = shell.shown();
        ui.command_result(&text, failed);
        if let (true, Ok(ran)) = (shell.include, &shell.ran) {
            let prompt = format!("!{}", shell.command);
            let message = ran.message(&shell.command);
            let mut message = json!({"role":"user","content":message,TYPED:prompt});
            if failed {
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
                        sets: &self.sets,
                    },
                    skills: &self.skills,
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
        AgentConfig, ChildItem, ChildState, Delegation, Navigated, Next, Session, SessionChoice,
        TreeItem, instructions, messages, show_items,
    };
    use crate::provider_fixture as fixture;
    use std::{sync::Arc, time::Duration};

    #[test]
    fn an_effort_chosen_during_a_turn_goes_with_its_next_request() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", json!({"command":"true"})),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "effort-during");
        agent.set_effort("low").unwrap();
        let mut ui = Recorder {
            efforts: vec![None, Some("high".into())],
            ..Recorder::default()
        };
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        let bodies = server.join().unwrap();
        let efforts: Vec<_> = (bodies.iter())
            .map(|body| body["reasoning"]["effort"].clone())
            .collect();
        assert_eq!(efforts, ["low", "high"]);
        assert_eq!(agent.effort(), Some("high"));
        assert_eq!(agent.last_effort(), Some("high"));
    }

    #[test]
    fn a_paused_turn_goes_on_where_it_stopped_with_what_the_session_changed() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", json!({"command":"true"})),
            stream(&fixture::text_events("worked", fixture::known_usage())),
            tool_call(
                "call_2",
                status::NAME,
                json!({"summary":"Done.","status":"completed"}),
            ),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "paused");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join("rs:work.md"), "Work on $1.\n").unwrap();
        project_sets(&mut agent);
        agent.autonomy = None;
        // Before the second request, and before the status check's.
        let mut ui = Recorder {
            pauses: vec![false, true, false, true],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        let paused = agent.turn("/rs:work go", &mut ui, &cancel);
        assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        agent.set_model("other").unwrap();
        let paused = agent.resume_turn(&mut ui, &cancel);
        assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        agent.resume_turn(&mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_some());
        let bodies = server.join().unwrap();
        let models: Vec<_> = bodies.iter().map(|body| body["model"].clone()).collect();
        assert_eq!(models, ["m", "other", "other"]);
        assert_eq!(last_output(&bodies[1]), "(no output)");
        let check = bodies[2]["input"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert!((check["content"].as_str().unwrap()).starts_with("Status check for `/rs:work`: "));
        assert_eq!(ui.replies, ["worked"]);
        assert_eq!(agent.prompts(), ["/rs:work go"]);
    }

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
    fn queued_messages_steer_the_next_request_and_follow_ups_continue_the_turn() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", json!({"command":"echo ran"})),
            reply("steered"),
            reply("followed"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "queued");
        let mut ui = Recorder {
            // Nothing before the first request; two messages after the call.
            steering: vec![vec![], vec!["steer one".into(), "steer two".into()]],
            follow_ups: vec![vec!["and then this".into()]],
            ..Recorder::default()
        };
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        let bodies = server.join().unwrap();
        let users = |body: &Value| -> Vec<String> {
            let input = body["input"].as_array().unwrap();
            let users = input.iter().filter(|item| item["role"] == "user");
            users.map(|item| item["content"].to_string()).collect()
        };
        assert_eq!(users(&bodies[0]), ["\"go\""]);
        // After the call's result, in the order typed.
        let input = bodies[1]["input"].as_array().unwrap();
        let output = input
            .iter()
            .position(|item| item["type"] == "function_call_output");
        assert_eq!(output, Some(input.len() - 3), "{input:?}");
        assert_eq!(users(&bodies[1])[1..], ["\"steer one\"", "\"steer two\""]);
        assert_eq!(users(&bodies[2])[3..], ["\"and then this\""]);
        let prompts: Vec<_> = (ui.shown.iter())
            .filter(|shown| shown.starts_with("prompt"))
            .collect();
        assert_eq!(
            prompts,
            [
                "prompt steer one",
                "prompt steer two",
                "prompt and then this"
            ]
        );
        assert_eq!(ui.replies, ["steered", "followed"]);
        assert_eq!(
            agent.prompts(),
            ["go", "steer one", "steer two", "and then this"]
        );
    }

    #[test]
    fn a_command_queued_during_a_turn_starts_there_in_place_of_the_one_running() {
        let report = |id: &str, next: &str| {
            let next = json!([{"message":next,"fresh_session":false,"reason":"More."}]);
            tool_call(
                id,
                status::NAME,
                json!({"summary":"Done.","status":"completed","next_steps":next}),
            )
        };
        let (endpoint, server) = serve(vec![
            report("call_1", "/rs:work again"),
            report("call_2", "/rs:other again"),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "queued-command");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        for name in ["work", "other"] {
            std::fs::write(
                commands.join(format!("rs:{name}.md")),
                format!("---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:{name}\n---\nDo $1.\n"),
            )
            .unwrap();
        }
        project_sets(&mut agent);
        agent.autonomy = None;
        let mut ui = Recorder {
            // Nothing before the first request; then where the reported turn would end.
            steering: vec![vec![], vec!["/rs:other now".into()]],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work go", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let expansion = "<scramjet-command name=\"rs:other\">\nDo now.\n</scramjet-command>";
        let last = bodies[1]["input"].as_array().unwrap().last().unwrap();
        assert_eq!(last, &json!({"role":"user","content":expansion}));
        let shown = format!("command /rs:other now sent {expansion}");
        assert!(ui.shown.contains(&shown), "{:?}", ui.shown);
        let completed = agent.take_completed().unwrap();
        let Next::Offer { steps, .. } = agent.route(&completed, true, false).next else {
            panic!("no next step offered");
        };
        assert_eq!(steps[0].command.as_deref(), Some("/rs:other again"));
        assert_eq!(agent.prompts(), ["/rs:work go", "/rs:other now"]);
        let (_, conversation, resumed) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, ["/rs:work go", "/rs:other now"]);
        let mut replay = Recorder::default();
        show_items(&conversation, &mut replay);
        assert!(replay.shown.contains(&shown), "{:?}", replay.shown);
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
        assert_eq!(
            run.warnings,
            ["write: left out, as subagents run read-only here"]
        );
        assert_eq!(ui.children.last(), Some(ended));
        assert_eq!(ui.children.len(), 1 + 3 + 3 + 1);
        // Stored with the result, so that a resumed conversation shows them.
        let mut resumed = Recorder::default();
        show_items(agent.conversation(), &mut resumed);
        assert_eq!(resumed.details, ui.details);
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
        agent.instructions = instructions::agent(
            agent.root(),
            &agent.config.tools,
            &agent.commands,
            &[],
            &agent.skills,
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
            // The status check after the command's turn, answered with a report.
            tool_call(
                "call_1",
                status::NAME,
                json!({"summary":"Said it.","status":"completed"}),
            ),
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
            command_sets: Vec::new(),
            skills: Vec::new(),
        };
        let mut agent = Agent::start(config, SessionChoice::Open(path.clone())).unwrap();
        agent.instructions = instructions::agent(
            &root,
            &agent.config.tools,
            &agent.commands,
            &[],
            &agent.skills,
        );
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
            bodies[2]["input"][5],
            json!({"role":"user","content":"/sjrstest:other x"})
        );
        assert!(!bodies[2].to_string().contains("\"_scramjet_"));
        let sent = bodies[0]["instructions"].as_str().unwrap();
        assert!(sent.contains("\n# Command framing\n\nCommands injected by the Scramjet harness"));
        assert!(sent.contains(
            "\n# Available commands\n\nThe following slash commands can be invoked directly:\n\n"
        ));
        // Among the user's installed sets, if any.
        assert!(sent.contains("\n- /sjrstest:echo: Echo\n"));
        let prompts = [typed, "/sjrstest:other x"];
        assert_eq!(agent.prompts(), prompts);
        assert_eq!(agent.fork_prompts().unwrap(), prompts);
        // The status check is in the total, as Pi counts its custom messages, not a user message.
        assert_eq!((agent.stats().user, agent.stats().total), (2, 7));
        let tree = agent.tree().unwrap();
        assert_eq!(tree.entries[0].item, TreeItem::Prompt(typed.into()));
        let (_, conversation, resumed) = Session::open(&path).unwrap();
        assert_eq!(resumed, prompts);
        let mut shown = Recorder::default();
        show_items(&conversation, &mut shown);
        assert_eq!(shown.shown[0], format!("command {typed} sent {expansion}"));
        assert!(shown.shown.contains(&"prompt /sjrstest:other x".to_owned()));
    }

    #[test]
    fn a_skill_prompt_sends_its_instructions_which_the_model_is_told_where_to_read() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("one", fixture::known_usage())),
            stream(&fixture::text_events("two", fixture::known_usage())),
        ]);
        let (mut agent, scratch) = agent(endpoint, "skill");
        let file = scratch.0.join("skills/pdf/SKILL.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            "---\nname: pdf\ndescription: Fills PDF forms.\n---\nUse ./fill.sh\n",
        )
        .unwrap();
        agent.skills = super::super::Skills::in_directory(&scratch.0.join("skills"));
        agent.instructions = instructions::agent(
            agent.root(),
            &agent.config.tools,
            &agent.commands,
            &[],
            &agent.skills,
        );
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let typed = "/skill:pdf fill form.pdf";
        agent.turn(typed, &mut ui, &cancel).unwrap();
        std::fs::remove_file(&file).unwrap();
        agent.turn("/skill:pdf again", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let sent = format!(
            "<skill name=\"pdf\" location=\"{}\">\nReferences are relative to {}.\n\nUse \
./fill.sh\n</skill>\n\nfill form.pdf",
            file.display(),
            file.parent().unwrap().display()
        );
        assert_eq!(bodies[0]["input"], json!([{"role":"user","content":sent}]));
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        let listed = "\n\nThe following skills provide specialized instructions for specific \
tasks.\nUse the read tool to load a skill's file when the task matches its description.\n";
        assert!(instructions.contains(listed), "{instructions}");
        assert!(instructions.ends_with(&format!(
            "<available_skills>\n  <skill>\n    <name>pdf</name>\n    <description>Fills PDF \
forms.</description>\n    <location>{}</location>\n  </skill>\n</available_skills>\n",
            file.display()
        )));
        // Gone since, it is said so and sent as typed; neither is a command, so no status check.
        assert_eq!(bodies.len(), 2);
        assert_eq!(
            bodies[1]["input"][2],
            json!({"role":"user","content":"/skill:pdf again"})
        );
        let notice = format!("notice skill pdf: cannot read {}", file.display());
        assert!(
            ui.shown.iter().any(|shown| shown.starts_with(&notice)),
            "{:?}",
            ui.shown
        );
        assert_eq!(agent.prompts(), [typed, "/skill:pdf again"]);
        let mut shown = Recorder::default();
        agent.show_conversation(&mut shown);
        assert_eq!(shown.shown[0], format!("command {typed} sent {sent}"));
        // Without the read tool the model is not told to read them.
        let text = instructions::agent(agent.root(), &[], &agent.commands, &[], &agent.skills);
        assert!(!text.contains("<available_skills>"));
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
    fn a_resumed_session_keeps_its_command_waiting_for_the_user() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let completed = json!({"summary":"Done.","status":"completed"});
        let failed = b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                question::NAME,
                json!({"type":"freetext","message":"Which colour?"}),
            ),
            reply("used blue"),
            tool_call(
                "call_2",
                status::NAME,
                json!({"summary":"Used blue.","status":"completed","recommended_next_step":0,
                "next_steps":[{"message":"/rs:work again","fresh_session":false,"reason":"More."}]}),
            ),
            failed.to_vec(),
            reply("hello"),
            tool_call("call_3", status::NAME, completed),
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
                command_sets: Vec::new(),
                skills: Vec::new(),
            };
            let path = agent.session_path().to_owned();
            drop(agent);
            let mut agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
            project_sets(&mut agent);
            agent.autonomy = None;
            agent
        };
        project_sets(&mut agent);
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();

        // The question waits for the reply after a resume; the reply's turn gets the status
        // check, and the completed command's next step is offered.
        let mut agent = resume(agent);
        agent.turn("blue", &mut ui, &cancel).unwrap();
        let completed = agent.take_completed().unwrap();
        let routing = agent.route(&completed, true, false);
        let Next::Offer { steps, .. } = routing.next else {
            panic!("no next step offered: {routing:?}");
        };
        assert_eq!(steps[0].command.as_deref(), Some("/rs:work again"));
        let tree = agent.tree().unwrap();
        assert!(matches!(
            agent.navigate(tree.current.unwrap(), None, &mut ui, &cancel),
            Ok(Navigated::AlreadyThere)
        ));

        // A command whose turn failed waits for the user, also after a resume: their reply
        // resumes it, and its turn gets the status check.
        assert!(agent.turn("/rs:work", &mut ui, &cancel).is_err());
        let waiting = "notice /rs:work is waiting for you. Reply to continue.".to_owned();
        assert_eq!(ui.shown.last(), Some(&waiting));
        let mut agent = resume(agent);
        agent.turn("hello", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_some());

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
        assert_eq!(bodies.len(), 7);
        let last_input = |body: &Value| {
            let input = body["input"].as_array().unwrap();
            input.last().unwrap()["content"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        };
        let checks: Vec<_> = (bodies.iter())
            .map(|body| last_input(body).starts_with("Status check for `/rs:work`: "))
            .collect();
        assert_eq!(checks, [false, false, true, false, false, true, false]);
        assert_eq!(last_input(&bodies[4]), "hello");
        assert_eq!(last_input(&bodies[6]), "hi");
        assert!(bodies.iter().all(|body| {
            let body = body.to_string();
            !body.contains("command_state") && !body.contains("Dormant")
        }));
        let saved: Vec<_> = std::fs::read_to_string(agent.session_path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|line| line["type"] == "command_state")
            .collect();
        let started = json!({"type":"command_state","command":"rs:work"});
        let ended = json!({"type":"command_state","command":null});
        assert_eq!(
            saved,
            [&started, &ended, &started, &ended].map(Clone::clone)
        );

        // A fork before the reply waits for it again.
        assert_eq!(agent.fork(1).unwrap().as_deref(), Some("blue"));
        assert_eq!(agent.lifecycle.active(), Some("rs:work"));
    }

    #[test]
    fn autopilot_is_saved_restored_and_told_to_the_model() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let events = fixture::call_events(
            &json!({"summary":"Done.","status":"completed","recommended_next_step":0,
                "next_steps":[{"message":"/rs:work again","fresh_session":false,"reason":"More."}]})
            .to_string(),
            fixture::known_usage(),
        );
        let report = serde_json::to_string(&events)
            .unwrap()
            .replace("\"inert\"", &format!("\"{}\"", status::NAME));
        let report = stream(&serde_json::from_str::<Vec<Value>>(&report).unwrap());
        let (endpoint, server) = serve(vec![reply("worked"), report]);
        let (mut agent, scratch) = agent(endpoint, "autopilot");
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(
            commands.join("rs:work.md"),
            "---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:work\n---\nWork.\n",
        )
        .unwrap();
        project_sets(&mut agent);
        agent.autonomy = None;
        assert!(!agent.autopilot());
        agent.set_autopilot(true).unwrap();
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        // The toggle is not part of the conversation; the status check says what autopilot does.
        let inputs = bodies[1]["input"].as_array().unwrap();
        assert!(inputs.iter().all(|item| item["type"] != "autopilot"));
        let check = inputs.last().unwrap()["content"].as_str().unwrap();
        assert!(
            check.contains(
                "With `/autopilot on`, set it to highlight the selected entry; Scramjet \
auto-dispatches it only when its message is a slash command."
            ),
            "{check}"
        );
        let completed = agent.take_completed().unwrap();
        let routing = agent.route(&completed, false, agent.autopilot());
        let Next::Run { steps, chosen } = routing.next else {
            panic!("{routing:?}")
        };
        assert_eq!(steps[chosen].command.as_deref(), Some("/rs:work again"));

        // A resume restores it; a session or branch without a toggle keeps it as it is.
        let path = agent.session_path().to_owned();
        let without = scratch.0.join("without.jsonl");
        write_session(
            &without,
            &[json!({"type":"session","version":1})],
            std::time::SystemTime::now(),
        );
        agent.resume_session(&without).unwrap();
        assert!(agent.autopilot());
        agent.set_autopilot(false).unwrap();
        agent.resume_session(&path).unwrap();
        assert!(agent.autopilot());
        agent.new_session().unwrap();
        assert!(agent.autopilot());
        let saved = std::fs::read_to_string(agent.session_path()).unwrap();
        assert!(!saved.contains(r#""type":"autopilot""#), "{saved}");
        agent.resume_session(&without).unwrap();
        assert!(!agent.autopilot());
        // `/tree` does not list a toggle: the entry before it is where the conversation is. A
        // move restores the toggle on the path there.
        agent.resume_session(&path).unwrap();
        assert!(agent.autopilot());
        let last = agent.tree().unwrap().current.unwrap();
        agent.set_autopilot(false).unwrap();
        assert_eq!(agent.tree().unwrap().current, Some(last));
        let moved = agent.navigate(last, None, &mut ui, &cancel).unwrap();
        assert!(matches!(moved, Navigated::AlreadyThere), "{moved:?}");
        assert!(!agent.autopilot());
        let tree = agent.tree().unwrap();
        let worked = tree
            .entries
            .iter()
            .find(|entry| entry.item == TreeItem::Reply("worked".into()));
        agent
            .navigate(worked.unwrap().line, None, &mut ui, &cancel)
            .unwrap();
        assert!(agent.autopilot());
    }

    #[test]
    fn a_next_step_choice_is_recorded_for_the_model_and_a_resume_as_scramjet_records_it() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "ok",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "next-step-record");
        let step = |index, message: &str, reason: &str| Step {
            index,
            message: message.into(),
            reason: reason.into(),
            fresh_session: false,
            command: Some(message.into()),
        };
        let steps = [step(0, "/rs:review", "risky"), step(2, "/rs:loop", "")];
        let shown = agent.record_next_step("rs:loop", &steps, Some(1)).unwrap();
        assert_eq!(
            shown,
            "Next step (from rs:loop):\n  /rs:review — risky\n→ /rs:loop"
        );
        let dismissed = agent.record_next_step("rs:loop", &steps, None).unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("go on", &mut ui, &AtomicBool::new(false))
            .unwrap();

        // The model sees each record as a call it did not make, and its result.
        let input = server.join().unwrap()[0]["input"].clone();
        let input = input.as_array().unwrap();
        assert_eq!(input.len(), 5, "{input:#?}");
        assert_eq!(input[0]["name"], status::RECORD);
        let arguments: Value =
            serde_json::from_str(input[0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(
            arguments,
            json!({"outcome":"selected","options":[{"message":"/rs:review","reason":"risky"},
                {"message":"/rs:loop"}],"selectedIndex":1,"sourceCommand":"rs:loop",
                "source":"completion"})
        );
        assert_eq!(input[1]["call_id"], input[0]["call_id"]);
        assert_eq!(input[1]["output"].as_str(), Some(shown.as_str()));
        assert_ne!(input[2]["call_id"], input[0]["call_id"]);
        assert_eq!(
            serde_json::from_str::<Value>(input[2]["arguments"].as_str().unwrap()).unwrap()["outcome"],
            "dismissed"
        );
        assert_eq!(input[4]["content"], "go on");
        // Claude's Messages: a tool use for each record, answered first in the next message.
        let request = json!({"model":"claude-x","instructions":"i","input":input});
        let messages = messages::request(&request, 1000, false).unwrap()["messages"].clone();
        let shapes: Vec<_> = (messages.as_array().unwrap().iter())
            .map(|message| {
                let blocks = message["content"].as_array().unwrap().iter();
                let types: Vec<_> = blocks
                    .map(|block| block["type"].as_str().unwrap())
                    .collect();
                format!("{} {}", message["role"].as_str().unwrap(), types.join(" "))
            })
            .collect();
        assert_eq!(
            shapes,
            [
                "assistant tool_use",
                "user tool_result",
                "assistant tool_use",
                "user tool_result text"
            ]
        );

        // A resume shows each record as it showed, and `/tree` lists it with the step chosen.
        let (_, resumed, _) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, agent.conversation());
        let mut replay = Recorder::default();
        show_items(&resumed, &mut replay);
        assert_eq!(
            replay.shown[..3],
            [
                format!("next step {shown}"),
                format!("next step {dismissed}"),
                "prompt go on".to_owned()
            ]
        );
        let tree = agent.tree().unwrap();
        let items: Vec<_> = tree
            .entries
            .iter()
            .map(|entry| &entry.item)
            .take(2)
            .collect();
        assert_eq!(
            items,
            [
                &TreeItem::Tool(status::RECORD.into(), "/rs:loop".into()),
                &TreeItem::Tool(status::RECORD.into(), "dismissed".into())
            ]
        );
    }

    /// An agent in a scratch project whose `rs:work` command has `next`, and `rs:other`.
    fn command_agent(
        endpoint: std::net::SocketAddr,
        name: &str,
        next: &str,
    ) -> (Agent, ScratchDir) {
        let (mut agent, scratch) = agent(endpoint, name);
        let commands = agent.root().join(".scramjet/rs/commands");
        std::fs::create_dir_all(&commands).unwrap();
        let work = format!("---\nnext:\n{next}---\nWork on $1.\n");
        std::fs::write(commands.join("rs:work.md"), work).unwrap();
        std::fs::write(commands.join("rs:other.md"), "Other.\n").unwrap();
        project_sets(&mut agent);
        agent.autonomy = None;
        (agent, scratch)
    }

    const CLOSED: &str = "  mode: closed\n  candidates:\n    - name: rs:work\n";

    fn last_input(body: &Value) -> String {
        let input = body["input"].as_array().unwrap();
        input.last().unwrap()["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn is_check(body: &Value) -> bool {
        last_input(body).starts_with("Status check for `/rs:work`: ")
    }

    #[test]
    fn a_command_is_reported_inline_or_after_a_status_check() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let next = json!([{"message":"/rs:work again","fresh_session":true,"reason":"More."}]);
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                status::NAME,
                json!({"summary":"Done.","status":"completed",
                "recommended_next_step":0,"next_steps":next}),
            ),
            reply("worked"),
            tool_call(
                "call_2",
                status::NAME,
                json!({"summary":"Needs a key.","status":"blocked"}),
            ),
            tool_call(
                "call_3",
                question::NAME,
                json!({"type":"freetext","message":"Which?"}),
            ),
            reply("used a.txt"),
            tool_call(
                "call_4",
                status::NAME,
                json!({"summary":"Used it.","status":"completed"}),
            ),
        ]);
        let (mut agent, _scratch) = command_agent(endpoint, "status", CLOSED);
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work go", &mut ui, &cancel).unwrap();
        let completed = agent.take_completed().unwrap();
        let Next::Offer {
            steps, recommended, ..
        } = agent.route(&completed, true, false).next
        else {
            panic!("no next step offered");
        };
        assert_eq!(steps[0].command.as_deref(), Some("/rs:work again"));
        assert!(steps[0].fresh_session);
        assert_eq!(recommended, Some(0));
        // `blocked` is refused, and the model asks instead; the reply resumes the command.
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_none());
        agent.turn("a.txt", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_some());
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);

        let checks: Vec<_> = bodies.iter().map(is_check).collect();
        assert_eq!(checks, [false, false, true, false, false, true]);
        let check = bodies[2]["input"].as_array().unwrap().last().unwrap();
        assert_eq!(check["role"], "user");
        assert_eq!(check.as_object().unwrap().len(), 2);
        assert!(
            check["content"]
                .as_str()
                .unwrap()
                .contains("  - [0] rs:work\n")
        );
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        assert!(instructions.contains(&format!("- {}: {}\n", status::NAME, status::SNIPPET)));
        assert!(
            bodies
                .iter()
                .all(|body| body["instructions"] == bodies[0]["instructions"])
        );
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
        assert_eq!(outputs.len(), 4);
        assert_eq!(outputs[0], "→ /rs:work again");
        assert!(outputs[1].starts_with("`blocked` is not a status a command reports."));
        assert_eq!(outputs[2..], [r#"{"parked":true}"#, "status: completed"]);
        assert!(ui.shown.iter().any(|shown| {
            shown.starts_with("result report_scramjet_command_status `blocked` is not")
                && shown.ends_with(" failed")
        }));
        assert!(!ui.shown.iter().any(|shown| shown.starts_with("notice")));
        assert!(
            ui.shown
                .contains(&"call report_scramjet_command_status completed".to_owned())
        );
        // The status checks are neither recalled nor shown again.
        let prompts = ["/rs:work go", "/rs:work", "a.txt"];
        assert_eq!(agent.prompts(), prompts);
        let (_, _, resumed) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, prompts);
        let mut replay = Recorder::default();
        agent.show_conversation(&mut replay);
        let shown: Vec<_> = replay
            .shown
            .iter()
            .filter(|s| s.starts_with("prompt ") || s.starts_with("command "))
            .collect();
        assert_eq!(shown.len(), 3);
        assert_eq!(agent.fork_prompts().unwrap(), prompts);
    }

    #[test]
    fn status_checks_stop_after_three_in_a_row_without_the_users_input() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let confirm = json!({"type":"confirm","message":"Use a.txt?"});
        let (endpoint, server) = serve(vec![
            reply("looking"),
            reply("still looking"),
            tool_call("call_1", question::NAME, confirm),
            reply("using a.txt"),
            reply("reading it"),
            reply("still on it"),
            reply("almost"),
            tool_call(
                "call_2",
                status::NAME,
                json!({"summary":"Used it.","status":"completed"}),
            ),
        ]);
        let (mut agent, _scratch) = command_agent(endpoint, "check-limit", CLOSED);
        let mut ui = Recorder {
            choices: vec![Some(0)],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        // Two checks, then an answer during the second counts them again from none: three more.
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        let waiting = "notice /rs:work is waiting for you: it was checked on 3 times in a row and \
did not finish or ask you anything. Reply to continue.";
        assert_eq!(ui.shown.last().map(String::as_str), Some(waiting));
        assert_eq!(agent.lifecycle.saved()["command"], "rs:work");
        agent.turn("go on", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_some());
        let bodies = server.join().unwrap();
        let checks: Vec<_> = bodies.iter().map(is_check).collect();
        assert_eq!(checks, [false, true, true, false, true, true, true, false]);
        assert_eq!(last_output(&bodies[3]), r#"{"confirmed":true}"#);
        assert_eq!(last_input(&bodies[7]), "go on");
        assert_eq!(
            ui.shown.iter().filter(|s| s.starts_with("notice")).count(),
            1
        );
    }

    #[test]
    fn a_reply_with_neither_text_nor_tool_calls_gets_the_status_check() {
        let empty = messages_stream(&[
            json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ]);
        let (endpoint, server) = serve(vec![empty; 4]);
        let (mut agent, _scratch) = command_agent(endpoint, "empty-reply", CLOSED);
        agent.set_model("claude-opus-5-5").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("/rs:work", &mut ui, &AtomicBool::new(false))
            .unwrap();
        let bodies = server.join().unwrap();
        let checks = bodies.iter().filter(|body| {
            let messages = body["messages"].as_array().unwrap();
            // The checks join the prompt's message: no assistant message comes between.
            let blocks = messages.last().unwrap()["content"].as_array().unwrap();
            let text = blocks.last().unwrap()["text"].as_str().unwrap_or_default();
            text.starts_with("Status check for `/rs:work`: ")
        });
        assert_eq!((bodies.len(), checks.count()), (4, 3));
        assert!(
            ui.shown
                .last()
                .unwrap()
                .starts_with("notice /rs:work is waiting for you")
        );
    }

    #[test]
    fn a_command_is_abandoned_only_once_the_user_confirms() {
        let abandoned = json!({"summary":"Half **done**.","status":"abandoned",
            "next_steps":[{"message":"/rs:other","fresh_session":false}]});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", status::NAME, abandoned.clone()),
            tool_call("call_2", status::NAME, abandoned.clone()),
            tool_call("call_3", status::NAME, abandoned),
        ]);
        let forced = "  mode: forced\n  target: rs:other\n";
        let (mut agent, _scratch) = command_agent(endpoint, "abandon", forced);
        // Kept running, then dismissed: the command waits; then abandoned.
        let mut ui = Recorder {
            choices: vec![Some(1), None, Some(0)],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), Some("rs:work"));
        agent.turn("stop it", &mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), None);
        assert!(agent.take_completed().is_none());
        assert_eq!(server.join().unwrap().len(), 3);
        let asked =
            (ui.shown.iter()).filter(|s| s.starts_with("question") || s.starts_with("choose"));
        let question = "question Half **done**.\n\nAbandoning ends `/rs:work` now. No next step \
will run.";
        let choose = r#"choose Abandon /rs:work? ["Abandon", "Keep working"] Some(1)"#;
        assert_eq!(asked.collect::<Vec<_>>(), [question, choose].repeat(3));
        let outputs: Vec<_> = (agent.conversation().iter())
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(
            outputs,
            [
                "The user chose to keep /rs:work running: go on with its work.",
                "The user dismissed the question: /rs:work is waiting for their reply.",
                "The user confirmed: /rs:work is abandoned, and no next step will run.",
            ]
        );
        let saved = std::fs::read_to_string(agent.session_path()).unwrap();
        assert!(
            saved.ends_with("{\"command\":null,\"type\":\"command_state\"}\n"),
            "{saved}"
        );
    }

    #[test]
    fn a_message_after_a_report_keeps_it_and_a_skill_is_a_reply_to_the_command() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let freetext = json!({"type":"freetext","message":"Which file?"});
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                status::NAME,
                json!({"summary":"Done.","status":"completed"}),
            ),
            reply("noted"),
            tool_call("call_2", question::NAME, freetext.clone()),
            reply("checked"),
            tool_call("call_3", question::NAME, freetext),
            reply("plain"),
        ]);
        let (mut agent, scratch) = command_agent(endpoint, "after-report", CLOSED);
        let skill = scratch.0.join("skills/rs-check/SKILL.md");
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::write(
            &skill,
            "---\nname: rs-check\ndescription: Checks.\n---\nCheck it.\n",
        )
        .unwrap();
        agent.skills = super::super::Skills::in_directory(&scratch.0.join("skills"));
        // A steering message that arrives after the report: the model gets it, and the
        // completion is routed once, when the turn ends.
        let mut ui = Recorder {
            steering: vec![vec![], vec!["also this".into()]],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        assert!(agent.take_completed().is_some());
        assert_eq!(agent.lifecycle.active(), None);
        // A skill is the user's reply: it resumes the waiting command, whose turn gets the
        // status check. An unknown slash command leaves the command.
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        agent.turn("/skill:rs-check", &mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), Some("rs:work"));
        agent.turn("/nothing here", &mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), None);
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 6);
        assert_eq!(last_input(&bodies[1]), "also this");
        assert!(last_input(&bodies[3]).starts_with("<skill name=\"rs-check\""));
        let checks: Vec<_> = bodies.iter().map(is_check).collect();
        assert_eq!(checks, [false, false, false, false, true, false]);
        assert_eq!(last_input(&bodies[5]), "/nothing here");
    }
}
