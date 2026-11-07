//! A turn: model requests and tool calls from a prompt to the final answer, the messages
//! queued during it, and the command it runs: its status checks and what follows it.
use super::calls::Calls;
use super::request::Replying;
use super::{
    Agent, AgentError, AgentUi, AutoCompaction, CONTEXT, Completed, MAX_STEPS, PROBE, PRODUCER,
    Routing, Source, Step, Suggested, TIMING, TYPED, USAGE, autonomy, message_text, status,
    suggest,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// What a paused turn left running, shown in a thread of its own (`Agent::show_paused`).
pub struct ShownPaused {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<(Option<Calls>, Option<Replying>)>,
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
        self.say_saving(ui);
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

    /// Shows on `ui`, from a thread of its own, what the paused turn left running (its calls or
    /// its streaming reply) while the session does something else, until `take_paused`; `None`
    /// when nothing runs. Their questions to the user and their ends wait for `resume_turn`.
    pub fn show_paused(&mut self, mut ui: Box<dyn AgentUi + Send>) -> Option<ShownPaused> {
        let (mut calls, mut replying) = (self.calls.take(), self.replying.take());
        if calls.is_none() && replying.is_none() {
            return None;
        }
        let retries = self.config.retries;
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            if let Some(batch) = calls.as_mut().and_then(Calls::running) {
                batch.pump(&mut *ui, &stopped);
            }
            if let Some(replying) = &mut replying {
                replying.pump(&mut *ui, retries, &stopped);
            }
            (calls, replying)
        });
        Some(ShownPaused { stop, thread })
    }

    /// Takes back what `show_paused` showed, for `resume_turn`.
    pub fn take_paused(&mut self, shown: ShownPaused) {
        shown.stop.store(true, Ordering::Relaxed);
        let (calls, replying) = (shown.thread.join()).unwrap_or_else(|panic| {
            std::panic::resume_unwind(panic);
        });
        (self.calls, self.replying) = (calls, replying);
    }

    /// Goes on with the turn that `turn` or this returned `AgentError::Paused` from: with the
    /// calls the pause left running, else from its next request.
    pub fn resume_turn(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let mut ran = self.run(ui, cancel);
        // A command's reply that ends without a report gets a status check.
        loop {
            self.say_saving(ui);
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
        let mut arguments = status::record_arguments(source, steps, chosen);
        self.next_session = None;
        // A fresh session's name is taken now, so that the record names its file.
        if chosen.is_some_and(|at| steps[at].fresh()) {
            let directory = self.session_directory();
            let from = self.session.path();
            if let Ok(next) = super::Session::continuing(directory, &self.config.root, from) {
                arguments["nextSession"] = json!(
                    std::path::absolute(next.path()).unwrap_or_else(|_| next.path().to_owned())
                );
                self.next_session = Some(next);
            }
        }
        let arguments = arguments.to_string();
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
            // The calls a pause left running come first.
            if let Some(calls) = self.calls.take()
                && self.after_calls(calls, ui, cancel)?
            {
                return Ok(());
            }
            // As is a request a pause left streaming.
            let replying = match self.replying.take() {
                Some(replying) => replying,
                None => {
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
                    self.settle_mcp(ui, cancel)?;
                    let request = self.request();
                    self.start_reply(request)?
                }
            };
            let model = replying.model.clone();
            let reply = match self.wait_reply(replying, ui, cancel) {
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
            let timing = reply.timing.as_ref().map(saved_timing);
            for (index, mut item) in reply.items.into_iter().enumerate() {
                if let Some(object) = item.as_object_mut() {
                    object.insert(PRODUCER.into(), json!(model));
                    if let Some(tokens) = self.usage.context_tokens.filter(|_| index == last) {
                        object.insert(CONTEXT.into(), json!(tokens));
                    }
                    if let Some(usage) = saved.as_ref().filter(|_| index == last) {
                        object.insert(USAGE.into(), usage.clone());
                    }
                    if let Some(timing) = timing.as_ref().filter(|_| index == last) {
                        object.insert(TIMING.into(), timing.clone());
                    }
                }
                match item["type"].as_str() {
                    Some("message") => ui.assistant(&message_text(&item)),
                    Some("function_call") => calls.push(item.clone()),
                    _ => {}
                }
                self.add(item)?;
            }
            self.say_saving(ui);
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
            let calls = self.start_calls(calls, ui);
            if self.after_calls(calls, ui, cancel)? {
                return Ok(());
            }
        }
        Err(AgentError::TooManySteps)
    }

    /// Runs a reply's `calls` on, then returns whether the turn ends.
    fn after_calls(
        &mut self,
        calls: Calls,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<bool, AgentError> {
        let ends = self.run_calls(calls, ui, cancel)?;
        // After the results, so one reply's results stay a chain (see `destination`).
        self.save_command()?;
        if ends {
            self.compact_when_full(ui, cancel)?;
            return Ok(!self.continue_queued(ui)?);
        }
        Ok(false)
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
}

/// A response's usage as saved with its last item (`USAGE`), as Scramjet's usage counts it:
/// input tokens neither read from the cache nor written to it, output tokens, cache reads and
/// writes, and the cost when it is known.
pub(super) fn saved_usage(reply: &super::client::Reply) -> Option<Value> {
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

/// A response's timing as saved with its last item (`TIMING`).
pub(super) fn saved_timing(timing: &super::client::Timing) -> Value {
    let since = |at: std::time::SystemTime| {
        (at.duration_since(std::time::UNIX_EPOCH)).map_or(0, |since| since.as_millis() as u64)
    };
    let millis = |duration: std::time::Duration| duration.as_millis() as u64;
    let mut saved = json!({
        "started": since(timing.started_at),
        "duration_ms": millis(timing.elapsed),
    });
    if let Some(headers) = timing.headers {
        saved["headers_ms"] = json!(millis(headers));
    }
    if let Some(first) = timing.first_output {
        saved["first_output_ms"] = json!(millis(first));
    }
    if !timing.failed.is_empty() {
        saved["failed"] = (timing.failed.iter())
            .map(|attempt| {
                json!({
                    "started": since(attempt.started_at),
                    "duration_ms": millis(attempt.elapsed),
                    "error": attempt.error,
                })
            })
            .collect();
    }
    saved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{
        AgentConfig, Navigated, Next, Session, SessionChoice, TreeItem, instructions, messages,
        question, show_items,
    };
    use crate::provider_fixture as fixture;

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
        // While the call runs, and before the status check's request.
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
    fn a_turn_pauses_while_a_call_runs_which_goes_on_meanwhile() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", json!({"command":"sleep 1; echo slept"})),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "paused-call");
        let mut ui = Recorder {
            pauses: vec![false, true],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let paused = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        assert!(started.elapsed() < std::time::Duration::from_millis(800));
        let last = agent.conversation().last().unwrap().clone();
        assert_eq!(last["type"], "function_call");
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let resumed = std::time::Instant::now();
        agent.resume_turn(&mut ui, &cancel).unwrap();
        // It ran on during the pause.
        assert!(resumed.elapsed() < std::time::Duration::from_millis(800));
        let bodies = server.join().unwrap();
        assert_eq!(last_output(&bodies[1]), "slept\n");
        assert_eq!(ui.replies, ["done"]);
    }

    #[test]
    fn a_turn_pauses_while_a_reply_streams_which_goes_on_meanwhile() {
        let response = stream(&fixture::text_events("streamed", fixture::known_usage()));
        let half = response.len() / 2;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 65536];
            while !String::from_utf8_lossy(&request).contains("\"stream\"") {
                let read = std::io::Read::read(&mut connection, &mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            std::io::Write::write_all(&mut connection, &response[..half]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(600));
            std::io::Write::write_all(&mut connection, &response[half..]).unwrap();
        });
        let (mut agent, _scratch) = agent(endpoint, "paused-reply");
        let mut ui = Recorder {
            pauses: vec![false, true],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let paused = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        assert_eq!(agent.conversation().last().unwrap()["role"], "user");
        // A model chosen meanwhile takes the next request; the reply is the first model's.
        agent.set_model("other").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let resumed = std::time::Instant::now();
        agent.resume_turn(&mut ui, &cancel).unwrap();
        assert!(resumed.elapsed() < std::time::Duration::from_millis(400));
        server.join().unwrap();
        assert_eq!(ui.replies, ["streamed"]);
        assert_eq!(agent.conversation().last().unwrap()[PRODUCER], "test/m");
        assert_eq!(agent.usage.requests, 1);
    }

    #[test]
    fn what_a_paused_turn_left_running_shows_from_another_thread_meanwhile() {
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                "bash",
                json!({"command":"echo one; sleep 0.6; echo two"}),
            ),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "shown-paused");
        let mut ui = Recorder {
            pauses: vec![false, true],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        let paused = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        // The pause may come while the call's request streams; then it comes again at the call.
        let mut outputs: Vec<String> = Vec::new();
        for _ in 0..3 {
            let (to, seen) = std::sync::mpsc::channel();
            let shown = agent.show_paused(Box::new(Watching(to))).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !outputs.iter().any(|seen| seen == "result bash") {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                match seen.recv_timeout(left) {
                    Ok(seen) => outputs.push(seen),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(timeout) => panic!("{timeout:?}: {outputs:?}"),
                }
            }
            agent.take_paused(shown);
            if outputs.iter().any(|seen| seen == "result bash") {
                break;
            }
            ui.pauses = vec![true];
            let paused = agent.resume_turn(&mut ui, &cancel);
            assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        }
        // The output so far may show once more with `two` before the result does.
        let expected = ["output one\n", "output one\ntwo\n", "result bash"];
        assert!(
            outputs == [expected[0], expected[2]] || outputs == expected,
            "{outputs:?}"
        );
        agent.resume_turn(&mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(last_output(&bodies[1]), "one\ntwo\n");
        assert_eq!(ui.replies, ["done"]);
    }

    #[test]
    fn a_reply_a_paused_turn_left_streaming_shows_from_another_thread_meanwhile() {
        let response = stream(&fixture::text_events("streamed", fixture::known_usage()));
        // Held back from the text's delta on, which then streams during the pause.
        let text = String::from_utf8_lossy(&response);
        let delta = text.find("response.output_text.delta").unwrap();
        let half = text[..delta].rfind("\n\n").unwrap() + 2;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 65536];
            while !String::from_utf8_lossy(&request).contains("\"stream\"") {
                let read = std::io::Read::read(&mut connection, &mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
            }
            std::io::Write::write_all(&mut connection, &response[..half]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(600));
            std::io::Write::write_all(&mut connection, &response[half..]).unwrap();
        });
        let (mut agent, _scratch) = agent(endpoint, "shown-paused-reply");
        let mut ui = Recorder {
            pauses: vec![false, true],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        let paused = agent.turn("go", &mut ui, &cancel);
        assert!(matches!(paused, Err(AgentError::Paused)), "{paused:?}");
        let (to, seen) = std::sync::mpsc::channel();
        let shown = agent.show_paused(Box::new(Watching(to))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        agent.take_paused(shown);
        let resumed = std::time::Instant::now();
        agent.resume_turn(&mut ui, &cancel).unwrap();
        assert!(resumed.elapsed() < std::time::Duration::from_millis(400));
        server.join().unwrap();
        let seen: Vec<_> = seen.try_iter().collect();
        assert!(seen.contains(&"progress".to_owned()), "{seen:?}");
        assert_eq!(ui.replies, ["streamed"]);
        assert_eq!(agent.usage.requests, 1);
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
            providers: crate::agent::tests::serving(endpoint),
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
            skill_options: Default::default(),
            prompt: Default::default(),
            mcp_servers: Vec::new(),
            mcp_disabled: Vec::new(),
            sandbox: None,
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
                providers: crate::agent::tests::serving(endpoint),
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
                skill_options: Default::default(),
                prompt: Default::default(),
                mcp_servers: Vec::new(),
                mcp_disabled: Vec::new(),
                sandbox: None,
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
        let saved = agent.session.text();
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
    fn a_fresh_session_next_step_links_the_two_sessions_as_a_chain() {
        let (mut agent, _scratch) = agent(([127, 0, 0, 1], 9).into(), "next-step-chain");
        agent.add(json!({"role":"user","content":"first"})).unwrap();
        let first = agent.session_path().to_owned();
        let step = |fresh_session, command: Option<&str>| Step {
            index: 0,
            message: "/rs:loop".into(),
            reason: String::new(),
            fresh_session,
            command: command.map(str::to_owned),
        };
        let record = |agent: &Agent| -> Value {
            let call = agent.conversation().iter().rev().nth(1).unwrap();
            serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap()
        };

        // A step in the same session, or a text step, names no other.
        let source = Source::Completion("rs:loop");
        agent
            .record_next_step(source, &[step(false, Some("/rs:loop"))], Some(0))
            .unwrap();
        assert_eq!(record(&agent).get("nextSession"), None);
        agent
            .record_next_step(source, &[step(true, None)], Some(0))
            .unwrap();
        assert_eq!(record(&agent).get("nextSession"), None);

        // A fresh session's record names the file the session continues in, and that file's
        // header names the session before it, marked as a next step's.
        agent
            .record_next_step(source, &[step(true, Some("/rs:loop"))], Some(0))
            .unwrap();
        let named = std::path::PathBuf::from(record(&agent)["nextSession"].as_str().unwrap());
        agent.new_session().unwrap();
        assert_eq!(agent.session_path(), named);
        agent
            .add(json!({"role":"user","content":"second"}))
            .unwrap();
        let header: Value = serde_json::from_str(
            std::fs::read_to_string(&named)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(header["parentSession"], json!(first));
        assert_eq!(header["nextStep"], true);
        let listed = Session::list(first.parent().unwrap()).unwrap();
        let second = listed.iter().find(|info| info.path == named).unwrap();
        assert_eq!(second.continues.as_deref(), Some(first.as_path()));
        assert_eq!(second.parent, None);

        // The chain's earlier sessions, the first first, each with what its command reported.
        let report = json!({"status":"completed","summary":"Did the second."});
        let call = json!({"type":"function_call","call_id":"c1","name":status::NAME,
            "arguments":report.to_string()});
        agent.add(call).unwrap();
        agent
            .record_next_step(source, &[step(true, Some("/rs:loop"))], Some(0))
            .unwrap();
        agent.new_session().unwrap();
        let third = agent.session_path().to_owned();
        let shown = |agent: &Agent| {
            let (earlier, problem) = agent.earlier_sessions();
            assert_eq!(problem, None);
            (earlier.iter())
                .map(|session| {
                    (
                        session.first_prompt().unwrap().to_owned(),
                        session.summary.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let chain = vec![
            ("first".to_owned(), None),
            ("second".to_owned(), Some("Did the second.".to_owned())),
        ];
        assert_eq!(shown(&agent), chain);
        // As a resume finds them, and up to a session that cannot be read.
        agent.add(json!({"role":"user","content":"third"})).unwrap();
        agent.new_session().unwrap();
        agent.resume_session(&third).unwrap();
        assert_eq!(shown(&agent), chain);
        std::fs::remove_file(&first).unwrap();
        let (earlier, problem) = agent.earlier_sessions();
        assert_eq!(earlier.len(), 1);
        assert!(problem.unwrap().starts_with(&first.display().to_string()));

        // `/clear` starts a session of its own, and a dismissed list leaves none behind.
        let dir_len = || std::fs::read_dir(third.parent().unwrap()).unwrap().count();
        let before = dir_len();
        agent
            .record_next_step(source, &[step(true, Some("/rs:loop"))], None)
            .unwrap();
        assert_eq!(dir_len(), before);
        agent.new_session().unwrap();
        let cleared = agent.session.text();
        assert!(!cleared.contains("parentSession"), "{cleared}");
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
