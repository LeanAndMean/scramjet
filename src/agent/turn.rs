//! A turn: model requests and tool calls from a prompt to the final answer, the command it
//! runs, and the `!` commands the user runs without one.
use super::tools::{self, Approval, Call, Output};
use super::{
    Agent, AgentError, AgentUi, Answer, AutoCompaction, CONTEXT, Completed, DIFF, Ending, FAILED,
    MAX_STEPS, PROBE, PRODUCER, Purpose, Routing, SUBAGENTS, Source, Step, Suggested, TYPED,
    ToolDetail, USAGE, Usage, UserCommand, autonomy, batch, delegate, forge, message_text, publish,
    question, sandbox, status, subagent, suggest, switch,
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

    /// How it ran, as its block shows it, live and on a resume: from the text kept for it.
    pub fn shown(&self) -> UserCommand {
        match &self.ran {
            Ok(ran) if self.include => {
                let failed = ran.tool_output().failed;
                UserCommand::included(&self.command, &ran.message(&self.command), failed)
            }
            Ok(ran) => {
                let shown = ran.tool_output();
                UserCommand::excluded(&self.command, &shown.text, shown.failed)
            }
            Err(error) => UserCommand::new(
                &self.command,
                self.include,
                "",
                Ending::Failed(error.clone()),
            ),
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
        self.suggestion = suggest::Pending::Nothing;
        self.delegated.clear();
        // With files the text starts with them, so it is a reply even after a `/`, as in Scramjet.
        let (message, typed) = match self.attached.take() {
            Some(files) => {
                self.lifecycle.prompted(status::Prompt::Reply);
                (files.message(prompt), files.typed(prompt))
            }
            None => (self.user_message(prompt, ui), prompt.to_owned()),
        };
        self.add(message)?;
        self.save_command()?;
        self.prompts.push(typed);
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
                    self.suggestion = suggest::Pending::Nothing;
                    if let Some(notice) = self.lifecycle.interrupted() {
                        ui.notice(&notice);
                    }
                    self.save_command()?;
                    // After the command's state, which `/tree` takes as the state there.
                    let stopped = match &error {
                        AgentError::Cancelled => json!({"type":"turn_stopped"}),
                        error => json!({"type":"turn_stopped","error":error.to_string()}),
                    };
                    // Not saving it leaves the error to report as it is.
                    let _ = self.add(stopped);
                    return Err(error);
                }
                Ok(()) => {}
            }
            let mut ended = self.lifecycle.turn_ended(&self.commands, self.autopilot);
            if let status::Ended::Probe(_) = ended
                && !self.offers(status::NAME)
                && let Some(name) = self.lifecycle.active().map(str::to_owned)
            {
                self.lifecycle.wait();
                ended = status::Ended::Notice(format!(
                    "/{name} is waiting for you: {} is not among this run's tools, so it cannot \
finish and is not checked on. Reply to continue.",
                    status::NAME
                ));
            }
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

    /// The steps the agent suggested in the last turn, which ended as a turn ends without a
    /// command, for the user to choose from.
    pub fn take_suggestion(&mut self) -> Option<Suggested> {
        match std::mem::take(&mut self.suggestion) {
            suggest::Pending::Steps(suggested) => Some(suggested),
            _ => None,
        }
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
        source: Source,
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
            let saved = saved_usage(&reply);
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
            ui.usage(
                self.session_tokens(),
                self.context_tokens(),
                self.output_rate(),
            );
            if let Some(message) = reply.incomplete {
                return Err(AgentError::Provider(message));
            }
            if calls.is_empty() {
                self.compact_when_full(ui, cancel)?;
                if self.continue_queued(ui)? {
                    continue;
                }
                return Ok(());
            }
            // As in Pi, the turn ends without another request when every call ends it.
            let ends = match self.together(&calls) {
                Some(together) => {
                    let tools = self.tools();
                    let root = &self.config.root;
                    let run = |call: &Call, ui: &mut dyn AgentUi| {
                        ui.tool_call(call.name, &call.summary(), call.compact(root).as_ref());
                        tools.run(call, ui, cancel)
                    };
                    let ran = batch::run(&together, &self.config.root, &run, ui, cancel);
                    for (item, (output, usage)) in calls.iter().zip(ran) {
                        self.usage.add(&usage);
                        self.add_result(item, &output, ui)?;
                    }
                    if cancel.load(Ordering::Relaxed) {
                        return Err(AgentError::Cancelled);
                    }
                    false
                }
                None => {
                    let mut ends = true;
                    let mut calls = calls.iter();
                    while let Some(item) = calls.next() {
                        let (output, ends_turn) = self.call(item, ui, cancel);
                        ends &= ends_turn;
                        self.add_result(item, &output, ui)?;
                        if cancel.load(Ordering::Relaxed) {
                            // Every call gets a result, as in Pi, or the next request is refused.
                            for item in calls {
                                self.not_run(item, ui)?;
                            }
                            return Err(AgentError::Cancelled);
                        }
                    }
                    ends
                }
            };
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
        // The user went on, as Scramjet drops a suggestion at the user's input.
        self.suggestion = suggest::Pending::Nothing;
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
        ui.command_started(command, include);
        let shell = Shell::run(&self.config.root, command, include, cancel, &mut |output| {
            ui.tool_output(output)
        });
        self.add_shell(shell, ui)
    }

    /// Shows how `shell`, run apart from the agent, ended, and adds it to the conversation as
    /// `shell` says: for the model (`!`), or only to be shown again on a resume (`!!`).
    pub fn add_shell(&mut self, shell: Shell, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
        ui.command_result(&shell.shown());
        let Ok(ran) = &shell.ran else {
            return Ok(());
        };
        let Output { text, failed, .. } = ran.tool_output();
        let prompt = format!("!{}", shell.command);
        let mut item = match shell.include {
            true => json!({"role":"user","content":ran.message(&shell.command),TYPED:prompt}),
            false => json!({"type":"bang_excluded","command":shell.command,"output":text}),
        };
        if failed {
            item[FAILED] = json!(true);
        }
        self.add(item)?;
        if shell.include {
            self.prompts.push(prompt);
        }
        Ok(())
    }

    /// The calls of a reply that run at the same time, as Pi runs them: when there are several
    /// and each is a coding tool or `subagent` that asks no approval question. Otherwise they
    /// run one after another, as the user's questions, the command's reports and delegations and
    /// the publications (sequential in Scramjet too) need.
    fn together<'a>(&self, calls: &'a [Value]) -> Option<Vec<Call<'a>>> {
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
    fn tools(&self) -> Tools<'_> {
        Tools {
            parent: subagent::Parent {
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
                agent_directory: self.agent_directory.as_deref(),
                approvals: self.approvals.as_deref(),
                rates: self.rates.as_deref(),
            },
            hide_credentials: self.hide_credentials,
        }
    }

    /// Adds `output` as the result of the call `item`, with what is shown of it on a resume.
    fn add_result(
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
    fn not_run(&mut self, item: &Value, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
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
    fn call(&mut self, item: &Value, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> (Output, bool) {
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
struct Tools<'a> {
    parent: subagent::Parent<'a>,
    hide_credentials: bool,
}

impl Tools<'_> {
    /// Runs `call` and shows its result; with the usage of its children, if any.
    fn run(&self, call: &Call, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> (Output, Usage) {
        let (output, usage) = match call.name {
            subagent::NAME => subagent::run(&call.arguments, &self.parent, ui, cancel),
            _ => {
                ui.tool_running(std::time::Instant::now());
                let hidden = self.hide_credentials.then(sandbox::Hidden::current);
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

/// A response's usage as saved with its last item (`USAGE`), as Scramjet's usage counts it:
/// input tokens neither read from the cache nor written to it, output tokens, cache reads and
/// writes, and the cost when it is known.
fn saved_usage(reply: &super::client::Reply) -> Option<Value> {
    let usage = reply.usage.as_ref()?;
    let read = usage.cached_input_tokens.unwrap_or(0);
    let written = usage.cache_write_tokens.unwrap_or(0);
    let mut saved = json!({
        "input": usage.input_tokens.unwrap_or(0).saturating_sub(read + written),
        "output": usage.output_tokens.unwrap_or(0),
        "cache_read": read,
        "cache_write": written,
    });
    if let Some(cost) = reply.cost {
        saved["cost"] = json!(cost);
    }
    Some(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{
        AgentConfig, ChildItem, ChildState, Delegation, Navigated, Next, Session, SessionChoice,
        TreeItem, approvals, instructions, messages, show_items,
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
        show_items(&conversation, None, &mut replay);
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
        let blocks = [
            format!("!{command} \"hi\\n100%\" Exited(3)"),
            "!!echo secret \"secret\\n\" Done".into(),
        ];
        assert_eq!(
            ui.shown,
            [
                format!("started {command} true"),
                blocks[0].clone(),
                "started echo secret false".into(),
                blocks[1].clone(),
            ]
        );
        assert_eq!(agent.usage.requests, 0);
        let message = format!("Ran `{command}`\n```\nhi\n100%\n```\n\nCommand exited with code 3");
        assert_eq!(
            agent.conversation(),
            [
                json!({"role":"user","content":message,"_scramjet_typed":typed,
                "_scramjet_failed":true}),
                json!({"type":"bang_excluded","command":"echo secret","output":"secret\n"}),
            ]
        );
        assert_eq!(agent.prompts(), [typed.as_str()]);
        agent.turn("next", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        // As Scramjet's `!!`, saved and shown again on a resume, but never sent.
        assert_eq!(
            bodies[0]["input"],
            json!([{"role":"user","content":message},{"role":"user","content":"next"}])
        );
        assert!(!bodies[0].to_string().contains("\"_scramjet_"));
        assert!(!bodies[0].to_string().contains("secret"));
        let path = agent.session_path().to_owned();
        let (_, conversation, prompts) = Session::open(&path).unwrap();
        assert_eq!(prompts, [typed.as_str(), "next"]);
        let mut shown = Recorder::default();
        show_items(&conversation, None, &mut shown);
        // A resume shows them as they ended.
        assert_eq!(shown.shown[..2], blocks);
        let tree = agent.tree().unwrap();
        let items: Vec<_> = tree.entries.iter().map(|entry| &entry.item).collect();
        assert_eq!(items[1], &TreeItem::Command("echo secret".into()));
        assert_eq!((agent.stats().user, agent.stats().total), (1, 3));
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
            only_tools: None,
            command_sets: Vec::new(),
            skills: Vec::new(),
            prompt: Default::default(),
        };
        let mut agent = Agent::start(config, SessionChoice::Open(path.clone())).unwrap();
        agent.instructions = instructions::agent(
            &root,
            &agent.offered(),
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
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
        show_items(&conversation, None, &mut shown);
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
            &agent.offered(),
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
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
        assert!(instructions.contains(&format!(
            "<available_skills>\n  <skill>\n    <name>pdf</name>\n    <description>Fills PDF \
forms.</description>\n    <location>{}</location>\n  </skill>\n</available_skills>\n\n# \
scramjet-rs\n",
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
        let text = instructions::agent(
            agent.root(),
            &[],
            &agent.commands,
            &[],
            &agent.skills,
            &Default::default(),
        );
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
    fn a_failed_or_cancelled_turn_is_saved_to_be_shown_and_listed_but_never_sent() {
        let failed = b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let (endpoint, server) = serve(vec![
            failed.to_vec(),
            stream(&fixture::text_events("ok", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "stopped");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let error = agent.turn("one", &mut ui, &cancel).unwrap_err().to_string();
        cancel.store(true, Ordering::Relaxed);
        let cancelled = agent.turn("two", &mut ui, &cancel);
        assert!(matches!(cancelled, Err(AgentError::Cancelled)));
        cancel.store(false, Ordering::Relaxed);
        agent.turn("three", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let prompts = ["one", "two", "three"].map(|text| json!({"role":"user","content":text}));
        assert_eq!(bodies[1]["input"], json!(prompts));
        let (_, conversation, _) = Session::open(agent.session_path()).unwrap();
        let mut shown = Recorder::default();
        show_items(&conversation, None, &mut shown);
        let stopped = format!("stopped {error}");
        assert_eq!(
            shown.shown,
            [
                "prompt one",
                &stopped,
                "prompt two",
                "stopped (cancelled)",
                "prompt three",
                "reply ok"
            ]
        );
        let tree = agent.tree().unwrap();
        let items: Vec<_> = tree.entries.into_iter().map(|entry| entry.item).collect();
        assert_eq!(
            items[..4],
            [
                TreeItem::Prompt("one".into()),
                TreeItem::Stopped(Some(error)),
                TreeItem::Prompt("two".into()),
                TreeItem::Stopped(None),
            ]
        );
        // Not counted among the context's messages either.
        assert_eq!((agent.stats().user, agent.stats().total), (3, 4));
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
                only_tools: None,
                command_sets: Vec::new(),
                skills: Vec::new(),
                prompt: Default::default(),
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
            let input = body["input"].to_string();
            !input.contains("command_state") && !input.contains("Dormant")
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
        let shown = agent
            .record_next_step(Source::Completion("rs:loop"), &steps, Some(1))
            .unwrap();
        assert_eq!(
            shown,
            "Next step (from rs:loop):\n  /rs:review — risky\n→ /rs:loop"
        );
        let dismissed = agent
            .record_next_step(Source::Completion("rs:loop"), &steps, None)
            .unwrap();
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
        show_items(&resumed, None, &mut replay);
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

    #[test]
    fn a_suggestion_waits_for_the_turns_end_unless_the_user_goes_on_or_it_stops() {
        let suggest = |id: &str| {
            let step = json!({"message":"/rs:work 5","fresh_session":false,"reason":"Go on."});
            tool_call(
                id,
                suggest::NAME,
                json!({"next_steps":[step],"recommended_next_step":0}),
            )
        };
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let failed = b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let step = json!({"message":"/rs:work 5","fresh_session":false,"reason":"Go on."});
        let (endpoint, server) = serve(vec![
            suggest("call_1"),
            reply("Try it."),
            suggest("call_2"),
            reply("Waiting."),
            tool_calls(&[
                (suggest::NAME, json!({"next_steps":[step]})),
                (
                    question::NAME,
                    json!({"type":"freetext","message":"Which?"}),
                ),
            ]),
            suggest("call_3"),
            reply("Asked."),
            suggest("call_4"),
            failed.to_vec(),
            suggest("call_5"),
            reply("Unseen."),
            reply("Plain."),
            suggest("call_6"),
            tool_call(
                "call_7",
                status::NAME,
                json!({"summary":"Done.","status":"completed"}),
            ),
        ]);
        let (mut agent, _scratch) = command_agent(endpoint, "suggest", CLOSED);
        let mut ui = Recorder {
            suggests: true,
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);

        // Kept for the end of the turn, which goes on after it.
        agent.turn("what next?", &mut ui, &cancel).unwrap();
        let suggested = agent.take_suggestion().unwrap();
        assert_eq!(suggested.recommended, Some(0));
        let commands: Vec<_> = (suggested.steps.iter())
            .map(|step| step.command.as_deref())
            .collect();
        assert_eq!(commands, [Some("/rs:work 5")]);
        assert_eq!(agent.take_suggestion(), None);

        // Dropped when the user sends a message meanwhile, a question waits for their reply (and
        // none is taken until then), the turn fails or the next one starts.
        ui.steering = vec![Vec::new(), vec!["no, wait".into()]];
        agent.turn("again", &mut ui, &cancel).unwrap();
        assert_eq!(agent.take_suggestion(), None);
        agent.turn("and?", &mut ui, &cancel).unwrap();
        assert!(
            ui.shown.contains(&"question Which?".to_owned()),
            "{:?}",
            ui.shown
        );
        assert_eq!(agent.take_suggestion(), None);
        assert!(agent.turn("fails", &mut ui, &cancel).is_err());
        assert_eq!(agent.take_suggestion(), None);
        agent.turn("unseen", &mut ui, &cancel).unwrap();
        agent.turn("plain", &mut ui, &cancel).unwrap();
        assert_eq!(agent.take_suggestion(), None);

        // A running command offers its steps with its report instead.
        agent.turn("/rs:work 1", &mut ui, &cancel).unwrap();
        assert_eq!(agent.take_suggestion(), None);
        assert!(agent.take_completed().is_some());

        let bodies = server.join().unwrap();
        let accepted = "Suggestion accepted — the user will see a selector popup with: /rs:work 5";
        assert_eq!(outputs(&bodies[1]), [("call_1".into(), accepted.into())]);
        let result = |body: &Value, id: &str| {
            let found = outputs(body).into_iter().find(|(call, _)| call == id);
            found.unwrap().1
        };
        assert!(result(&bodies[6], "call_3").starts_with("A freetext reply is pending"));
        let refused = result(&bodies[13], "call_6");
        assert!(
            refused.contains("only when no command is active (/rs:work is)"),
            "{refused}"
        );
        // The model sees the tool and its guideline.
        let tools = bodies[0]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == suggest::NAME));
        let instructions = bodies[0]["instructions"].as_str().unwrap();
        assert!(instructions.contains(suggest::GUIDELINE));
    }

    #[test]
    fn the_agent_switches_the_model_for_its_next_requests_as_scramjets_tool_does() {
        let switch = |provider: &str, model: &str| {
            (switch::NAME, json!({"provider":provider,"model":model}))
        };
        let got = |body: Value| {
            let body = body.to_string();
            let head = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len());
            (head + &body).into_bytes()
        };
        let listed = got(json!({"data":[{"id":"m"},{"id":"gpt-x"},{"id":"claude-y"}]}));
        let cards = got(json!({"models":[
            {"requestId":"m","provider":"openai","displayName":"M"},
            {"requestId":"gpt-x","provider":"openai","displayName":"GPT X",
                "supportedProtocols":["openai-responses"],"limits":{"context":400_000}},
            {"requestId":"claude-y","provider":"anthropic","displayName":"Claude Y"},
        ]}));
        let (endpoint, server) = serve_requests(vec![
            tool_calls(&[
                (switch::NAME, json!({"model":3})),
                switch("anthropic", "gpt-x"),
                switch("openai", "m"),
                switch("openai", "gpt-x"),
            ]),
            // Each call lists the models with their cards; a switch reads the cards again.
            listed.clone(),
            cards.clone(),
            listed.clone(),
            cards.clone(),
            listed,
            cards.clone(),
            cards,
            stream(&fixture::text_events("Done.", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "switch-model");
        agent.set_model_scope(vec!["m".into(), "claude-y".into()]);
        let mut ui = Recorder::default();
        (agent.turn("switch", &mut ui, &AtomicBool::new(false))).unwrap();
        let requests = server.join().unwrap();
        let lines: Vec<_> = (requests.iter())
            .map(|(head, _)| head.lines().next().unwrap())
            .collect();
        let (models, cards) = ("get /v1/models http/1.1", "get /api/model-cards http/1.1");
        let gets = [models, cards, models, cards, models, cards, cards];
        assert_eq!(lines[1..8], gets);

        let (first, last) = (&requests[0].1, &requests[8].1);
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
        // the session keeps for a resume, with its card's window.
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
    fn a_command_without_the_status_tool_waits_for_the_user_unchecked() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let (endpoint, server) = serve(vec![reply("worked"), reply("more")]);
        let (mut agent, _scratch) = command_agent(endpoint, "unchecked", CLOSED);
        agent.config.only_tools = Some(vec!["read".into()]);
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work a", &mut ui, &cancel).unwrap();
        let waiting = "notice /rs:work is waiting for you: report_scramjet_command_status is not \
among this run's tools, so it cannot finish and is not checked on. Reply to continue.";
        assert_eq!(ui.shown.last().map(String::as_str), Some(waiting));
        assert_eq!(agent.lifecycle.saved()["command"], "rs:work");
        agent.turn("go on", &mut ui, &cancel).unwrap();
        assert_eq!(ui.shown.last().map(String::as_str), Some(waiting));
        let bodies = server.join().unwrap();
        assert_eq!(
            bodies.iter().map(is_check).collect::<Vec<_>>(),
            [false, false]
        );
    }

    #[test]
    fn a_command_no_longer_installed_ends_with_its_turn() {
        let reply = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let freetext = json!({"type":"freetext","message":"Which file?"});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", question::NAME, freetext),
            reply("used a.txt"),
        ]);
        let (mut agent, _scratch) = command_agent(endpoint, "gone", CLOSED);
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("/rs:work", &mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), Some("rs:work"));
        std::fs::remove_file(agent.root().join(".scramjet/rs/commands/rs:work.md")).unwrap();
        project_sets(&mut agent);
        agent.turn("a.txt", &mut ui, &cancel).unwrap();
        assert_eq!(agent.lifecycle.active(), None);
        assert!(agent.take_completed().is_none());
        // No status check followed the reply.
        assert_eq!(server.join().unwrap().len(), 2);
        let notice = "notice /rs:work has ended: no command of that name is installed now.";
        assert_eq!(ui.shown.last().map(String::as_str), Some(notice));
        let saved = std::fs::read_to_string(agent.session_path()).unwrap();
        assert!(
            saved.ends_with("{\"command\":null,\"type\":\"command_state\"}\n"),
            "{saved}"
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
