//! `scramjet acp`: the agent as an Agent Client Protocol (v1) agent over stdio, for editors and
//! other clients (`docs/RPC.md`). JSON-RPC 2.0, one message per line; diagnostics go to stderr.
//! Each session runs on a thread of its own, driven as the full screen drives its own
//! (`drive::run_turn`), while this thread reads the client's messages.
use super::args::{Options, Start as Launch};
use super::drive::{self, Change, Front, Messages, Offer, Picked, Session, Start};
use super::settings::Settings;
use super::slash::Builtin;
use scramjet::agent::{
    AgentError, AgentUi, Answer, Choice, CompactRead, Compaction, Purpose, SessionChoice,
    StreamProgress, Tokens, ToolDetail, UserCommand,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::OsString,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const PROTOCOL_VERSION: u64 = 1;
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// Runs `scramjet acp` with the options in `arguments` until the client closes stdin.
pub(super) fn run(arguments: &[String]) -> Result<(), String> {
    let parse = || Options::parse(arguments.iter().map(OsString::from));
    let Some(options) = parse()? else {
        outln!("{USAGE}");
        return Ok(());
    };
    refuse_unfit(&options)?;
    let settings = Settings::load_cloning();
    scramjet::agent::set_shell(
        settings.shell_path.clone(),
        settings.shell_command_prefix.clone(),
    );
    scramjet::agent::set_user_shell(settings.default_shell);
    scramjet::agent::set_images(settings.resize_images, settings.block_images);
    for warning in &settings.warnings {
        eprintln!("scramjet: settings: {warning}");
    }
    drop(settings);
    let mut connection = Connection {
        out: Out::default(),
        arguments: arguments.to_vec(),
        initialized: false,
        notices: false,
        sessions: HashMap::new(),
    };
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let line = line.strip_suffix('\r').unwrap_or(&line);
        if !line.trim().is_empty() {
            connection.receive(line);
        }
    }
    connection.end();
    Ok(())
}

pub(super) const USAGE: &str = "Usage: scramjet acp [options]

Runs Scramjet as an Agent Client Protocol (v1) agent: the client starts it and speaks JSON-RPC on
its stdin and stdout, one message per line. The options are those of the full screen (--model,
--thinking, --sandbox, --tools, --no-session, ...; see scramjet --help); each session the client
opens starts with them in the folder it names.";

/// What the full screen's options ask that no ACP session can do yet.
fn refuse_unfit(options: &Options) -> Result<(), String> {
    let unfit = [
        (options.print, "-p"),
        (options.ask, "--ask (permission requests are not built yet)"),
        (!options.messages.is_empty(), "prompts"),
        (!options.files.is_empty(), "@FILE"),
        (options.export.is_some(), "--export"),
        (options.list_models.is_some(), "--list-models"),
        (options.list_credential_stores, "--list-credential-stores"),
        (options.version, "--version"),
        (
            !matches!(options.start, Launch::New | Launch::Unsaved),
            "--continue, --resume, --session and --fork (sessions are opened by the client)",
        ),
    ];
    match unfit.iter().find(|(given, _)| *given) {
        Some((_, what)) => Err(format!("acp: {what} cannot be used here")),
        None => Ok(()),
    }
}

/// The client's stdout: each message written whole, as one line.
#[derive(Clone, Default)]
struct Out(Arc<Mutex<()>>);

impl Out {
    fn send(&self, message: &Value) {
        // Inside JSON these appear only in strings, where escaping them is the same text; some
        // readers split lines at them.
        let line = (message.to_string())
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let _held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}").and_then(|()| stdout.flush());
    }
    fn respond(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
    fn error(&self, id: &Value, code: i64, message: &str) {
        let error = json!({"code": code, "message": message});
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": error}));
    }
    fn update(&self, session: &str, update: Value) {
        let params = json!({"sessionId": session, "update": update});
        let notification = json!({"jsonrpc": "2.0", "method": "session/update", "params": params});
        self.send(&notification);
    }
}

struct Connection {
    out: Out,
    /// The options each session starts with.
    arguments: Vec<String>,
    initialized: bool,
    /// The client offered `session.notices`.
    notices: bool,
    sessions: HashMap<String, Handle>,
}

/// What this thread holds of a session that runs on its own.
struct Handle {
    prompts: mpsc::Sender<(Value, String)>,
    cancel: Arc<AtomicBool>,
    turn: Arc<Mutex<Turn>>,
    thread: JoinHandle<()>,
}

/// A session's turn, as both threads see it.
#[derive(Default)]
struct Turn {
    running: bool,
    /// Prompts sent while it runs, for the agent to take: `messages.steering` before its next
    /// request, `messages.follow_ups` when it would end.
    messages: Messages,
    /// The requests of those prompts, answered when the turn ends.
    waiting: Vec<Value>,
}

impl Connection {
    fn receive(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return self.out.error(&Value::Null, PARSE_ERROR, "Parse error");
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to a request of ours: none is sent yet.
            if message.get("result").is_some() || message.get("error").is_some() {
                return;
            }
            let id = id.unwrap_or(Value::Null);
            return self.out.error(&id, INVALID_REQUEST, "Invalid request");
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            return self.notified(method, &params);
        };
        if !self.initialized && method != "initialize" {
            return self.out.error(&id, INVALID_REQUEST, "initialize first");
        }
        match method {
            "initialize" => self.initialize(&id, &params),
            "session/new" => self.new_session(&id, &params),
            "session/prompt" => self.prompt(id, &params),
            _ => self.out.error(&id, METHOD_NOT_FOUND, "Method not found"),
        }
    }

    fn notified(&mut self, method: &str, params: &Value) {
        if method == "session/cancel"
            && let Some(handle) =
                (params["sessionId"].as_str()).and_then(|id| self.sessions.get(id))
        {
            let mut turn = lock(&handle.turn);
            if turn.running {
                turn.messages.take();
                handle.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    fn initialize(&mut self, id: &Value, params: &Value) {
        self.initialized = true;
        let capabilities = &params["clientCapabilities"];
        self.notices = capabilities["session"]["notices"].is_object();
        let result = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "agentCapabilities": {
                "loadSession": false,
                "promptCapabilities": {"image": false, "audio": false, "embeddedContext": true},
                "mcpCapabilities": {"http": false, "sse": false},
            },
            "agentInfo": {
                "name": "scramjet",
                "title": "Scramjet",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "authMethods": [],
        });
        self.out.respond(id, result);
    }

    fn new_session(&mut self, id: &Value, params: &Value) {
        let Some(cwd) = params["cwd"].as_str().map(PathBuf::from) else {
            return self.out.error(id, INVALID_PARAMS, "cwd is required");
        };
        if !cwd.is_absolute() || !cwd.is_dir() {
            let message = format!("cwd {} is not an absolute path to a folder", cwd.display());
            return self.out.error(id, INVALID_PARAMS, &message);
        }
        if (params["mcpServers"].as_array()).is_some_and(|servers| !servers.is_empty()) {
            eprintln!("scramjet: acp: the client's MCP servers are not used yet; ignored");
        }
        let (started, starts) = mpsc::channel::<Result<String, String>>();
        let (prompts, prompted) = mpsc::channel();
        let (cancel, turn) = (Arc::<AtomicBool>::default(), Arc::<Mutex<Turn>>::default());
        let worker = Worker {
            out: self.out.clone(),
            notices: self.notices,
            cancel: cancel.clone(),
            turn: turn.clone(),
        };
        let arguments = self.arguments.clone();
        let thread = std::thread::spawn(move || {
            let session = match start_session(&arguments, &cwd) {
                Ok(session) => session,
                Err(error) => {
                    let _ = started.send(Err(error));
                    return;
                }
            };
            let mut front = worker.front(session);
            let _ = started.send(Ok(front.ui.session.clone()));
            front.commands();
            for (id, prompt) in prompted {
                front.run(id, prompt);
            }
        });
        match starts.recv() {
            Ok(Ok(session)) => {
                self.out.respond(id, json!({"sessionId": session}));
                let handle = Handle {
                    prompts,
                    cancel,
                    turn,
                    thread,
                };
                self.sessions.insert(session, handle);
            }
            Ok(Err(error)) => self.out.error(id, INTERNAL_ERROR, &error),
            Err(_) => self
                .out
                .error(id, INTERNAL_ERROR, "the session failed to start"),
        }
    }

    fn prompt(&mut self, id: Value, params: &Value) {
        let Some(handle) = (params["sessionId"].as_str()).and_then(|id| self.sessions.get(id))
        else {
            return self.out.error(&id, INVALID_PARAMS, "no such session");
        };
        let prompt = match prompt_text(&params["prompt"]) {
            Ok(prompt) => prompt,
            Err(error) => return self.out.error(&id, INVALID_PARAMS, &error),
        };
        if Builtin::parse(&prompt).is_some() {
            let name = prompt.split_whitespace().next().unwrap_or_default();
            let message = format!("{name} is a built-in command, which ACP mode does not run");
            return self.out.error(&id, INVALID_PARAMS, &message);
        }
        let follow_up = params["_meta"]["io.github.leanandmean.scramjet"]["queue"] == "followUp";
        let mut turn = lock(&handle.turn);
        if turn.running {
            turn.messages.push(prompt, follow_up);
            turn.waiting.push(id);
            return;
        }
        turn.running = true;
        handle.cancel.store(false, Ordering::Relaxed);
        drop(turn);
        let _ = handle.prompts.send((id, prompt));
    }

    /// At the end of input: every turn cancelled, and each session's thread waited for, which
    /// writes its session file as it goes.
    fn end(self) {
        for handle in self.sessions.values() {
            handle.cancel.store(true, Ordering::Relaxed);
        }
        for (_, handle) in self.sessions {
            drop(handle.prompts);
            let _ = handle.thread.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The text of a prompt's content blocks: text as given, an embedded resource's text under its
/// URI, a link's URI.
fn prompt_text(blocks: &Value) -> Result<String, String> {
    let blocks = blocks
        .as_array()
        .ok_or("prompt must be an array of content blocks")?;
    let mut parts = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => parts.push(block["text"].as_str().unwrap_or_default().to_owned()),
            Some("resource") => {
                let resource = &block["resource"];
                let uri = resource["uri"].as_str().unwrap_or_default();
                let Some(text) = resource["text"].as_str() else {
                    return Err(format!(
                        "resource {uri} has no text; binary ones are not taken"
                    ));
                };
                parts.push(format!("{uri}:\n```\n{text}\n```"));
            }
            Some("resource_link") => {
                parts.push(block["uri"].as_str().unwrap_or_default().to_owned());
            }
            Some(other) => return Err(format!("{other} content is not taken")),
            None => return Err("a content block without a type".to_owned()),
        }
    }
    Ok(parts.join("\n\n"))
}

/// Starts a session in `cwd` with the options `arguments` give, as the full screen starts one.
fn start_session(arguments: &[String], cwd: &Path) -> Result<Session, String> {
    let options = Options::parse(arguments.iter().map(OsString::from))?
        .ok_or("--help is not a session's option")?;
    let choice = match options.start {
        Launch::Unsaved => SessionChoice::Unsaved,
        _ => SessionChoice::New,
    };
    let root = super::project_root(Some(cwd.to_owned()))?;
    let settings = Settings::load();
    let session_dir = (options.session_dir.as_deref())
        .or(settings.session_dir.as_deref())
        .map(|directory| super::given_path(directory, cwd));
    let patterns = (options.models.clone()).unwrap_or_else(|| settings.enabled_models.clone());
    let source = match options.models {
        Some(_) => "--models",
        None => "settings: enabledModels",
    };
    let (agent, warnings) = super::start(
        options,
        (root, session_dir),
        choice,
        &settings,
        (&patterns, source),
    )?;
    for warning in warnings {
        eprintln!("scramjet: {warning}");
    }
    Ok(Session {
        agent,
        settings,
        scope: patterns,
        chosen: None,
    })
}

/// What a session's thread is given.
struct Worker {
    out: Out,
    notices: bool,
    cancel: Arc<AtomicBool>,
    turn: Arc<Mutex<Turn>>,
}

impl Worker {
    fn front(self, session: Session) -> Acp {
        let ui = Updates {
            out: self.out,
            session: session.agent.session_id(),
            notices: self.notices,
            turn: self.turn.clone(),
            steering: session.settings.steering_mode,
            follow_up: session.settings.follow_up_mode,
            responses: 0,
            streamed: false,
            calls: 0,
            window: None,
            used: None,
            stopped: None,
        };
        Acp {
            session,
            ui,
            cancel: self.cancel,
            turn: self.turn,
        }
    }
}

/// A session as ACP drives it.
struct Acp {
    session: Session,
    ui: Updates,
    cancel: Arc<AtomicBool>,
    turn: Arc<Mutex<Turn>>,
}

impl Acp {
    /// Lists the command sets' commands and the skills for the client.
    fn commands(&mut self) {
        let agent = &self.session.agent;
        let commands = (agent.commands().list().iter())
            .filter(|command| !command.delegate_only)
            .map(|command| {
                let mut listed = json!({"name": command.name, "description": command.description});
                if let Some(hint) = &command.argument_hint {
                    listed["input"] = json!({"hint": hint});
                }
                listed
            });
        let skills = (agent.skills().list().iter()).map(|skill| {
            json!({"name": format!("skill:{}", skill.name), "description": skill.description})
        });
        let listed: Vec<Value> = commands.chain(skills).collect();
        let update =
            json!({"sessionUpdate": "available_commands_update", "availableCommands": listed});
        self.ui.out.update(&self.ui.session, update);
    }

    /// Runs `prompt` and the next steps that follow it on their own, then the prompts sent
    /// meanwhile that the agent did not take; answers request `id` and theirs when it ends.
    fn run(&mut self, id: Value, prompt: String) {
        let (mut prompt, mut start) = (prompt, Start::Typed);
        let stopped = loop {
            let next = drive::run_turn(self, Some(&prompt), start);
            let stopped = self.ui.stopped.take();
            if stopped.is_some() || self.cancel.load(Ordering::Relaxed) {
                break stopped.unwrap_or(None);
            }
            if let Some((step, how)) = next {
                self.ui.user_message(&step);
                (prompt, start) = (step, how);
                continue;
            }
            let mut turn = lock(&self.turn);
            let queued = turn.messages.take();
            if queued.is_empty() {
                break None;
            }
            drop(turn);
            (prompt, start) = (queued.join("\n\n"), Start::Typed);
        };
        let mut turn = lock(&self.turn);
        turn.running = false;
        let waiting = std::mem::take(&mut turn.waiting);
        drop(turn);
        let cancelled = self.cancel.load(Ordering::Relaxed);
        for id in Some(id).into_iter().chain(waiting) {
            match &stopped {
                Some(error) if !cancelled => self.ui.out.error(&id, INTERNAL_ERROR, error),
                _ => {
                    let reason = match cancelled {
                        true => "cancelled",
                        false => "end_turn",
                    };
                    self.ui.out.respond(&id, json!({"stopReason": reason}));
                }
            }
        }
    }
}

impl Front for Acp {
    fn session(&mut self) -> &mut Session {
        &mut self.session
    }
    fn ui(&mut self) -> &mut dyn AgentUi {
        &mut self.ui
    }
    fn turn_starts(&mut self) {
        self.ui.window = self.session.agent.context_window();
    }
    fn turn(&mut self, prompt: Option<&str>) -> Result<(), AgentError> {
        let agent = &mut self.session.agent;
        match prompt {
            Some(prompt) => agent.turn(prompt, &mut self.ui, &self.cancel),
            None => agent.resume_turn(&mut self.ui, &self.cancel),
        }
    }
    fn paused(&mut self) {}
    fn turn_stops(&mut self) {}
    fn turn_ended(&mut self) {
        let agent = &self.session.agent;
        let (context, cost) = (agent.context_tokens(), agent.session_tokens().cost);
        self.ui.window = agent.context_window();
        self.ui.usage_update(context, cost);
    }
    fn chooses(&self) -> bool {
        false
    }
    fn offer(&mut self, _offer: Offer) -> Option<Picked> {
        None
    }
    /// A text step, for the user to send.
    fn edit(&mut self, text: String) {
        self.ui.agent_message(&format!("\n\nNext: {text}"));
    }
    fn say(&mut self, text: &str) {
        self.ui.notice(text);
    }
    fn note(&mut self, text: &str) {
        self.ui.notice(text);
    }
    fn notices(&mut self, notices: &[String]) {
        for notice in notices {
            self.ui.notice(notice);
        }
    }
    fn recorded(&mut self, record: &str) {
        self.ui.notice(record);
    }
    fn session_changed(&mut self, message: &str, _change: Change) {
        self.ui.notice(message);
    }
}

/// The agent's interface in a session: `session/update` notifications to the client.
struct Updates {
    out: Out,
    session: String,
    notices: bool,
    turn: Arc<Mutex<Turn>>,
    /// `steeringMode` and `followUpMode`.
    steering: super::settings::Delivery,
    follow_up: super::settings::Delivery,
    /// The model responses so far, which number their messages.
    responses: usize,
    /// Text of the current response was sent as it streamed.
    streamed: bool,
    /// The tool calls so far, which number them.
    calls: usize,
    window: Option<u64>,
    /// The usage last sent, which is not sent again.
    used: Option<(u64, u64, f64)>,
    /// How the last turn stopped before its end: cancelled (`None`), or failed with an error.
    stopped: Option<Option<String>>,
}

impl Updates {
    fn update(&self, update: Value) {
        self.out.update(&self.session, update);
    }
    fn chunk(&self, kind: &str, text: &str) {
        let id = format!("response-{}", self.responses);
        let content = json!({"type": "text", "text": text});
        self.update(json!({"sessionUpdate": kind, "messageId": id, "content": content}));
    }
    fn agent_message(&self, text: &str) {
        self.chunk("agent_message_chunk", text);
    }
    fn user_message(&mut self, text: &str) {
        self.responses += 1;
        self.chunk("user_message_chunk", text);
    }
    fn call_id(&self) -> String {
        format!("call-{}", self.calls)
    }
    fn call_update(&self, fields: Value) {
        let mut update = json!({"sessionUpdate": "tool_call_update", "toolCallId": self.call_id()});
        if let (Some(update), Some(fields)) = (update.as_object_mut(), fields.as_object()) {
            update.extend(fields.clone());
        }
        self.update(update);
    }
    fn usage_update(&mut self, context: Option<u64>, cost: f64) {
        let (Some(used), Some(size)) = (context, self.window) else {
            return;
        };
        if self.used.replace((used, size, cost)) == Some((used, size, cost)) {
            return;
        }
        let cost = json!({"amount": cost, "currency": "USD"});
        self.update(
            json!({"sessionUpdate": "usage_update", "used": used, "size": size, "cost": cost}),
        );
    }
    fn say(&self, severity: &str, text: &str) {
        match self.notices {
            true => {
                self.update(json!({"sessionUpdate": "notice", "severity": severity, "title": text}))
            }
            false => eprintln!("scramjet: {text}"),
        }
    }
}

/// The ACP kind of the tool `name`.
fn kind(name: &str) -> &'static str {
    match name {
        "read" => "read",
        "edit" | "write" => "edit",
        "grep" | "find" | "ls" => "search",
        "bash" | "powershell" | "cmd" => "execute",
        _ => "other",
    }
}

impl AgentUi for Updates {
    fn waiting(&mut self, purpose: Purpose) {
        if purpose == Purpose::Reply {
            self.responses += 1;
            self.streamed = false;
        }
    }
    fn progress(&mut self, update: &StreamProgress) {
        match update {
            StreamProgress::Text(text) => {
                self.streamed = true;
                self.agent_message(text);
            }
            StreamProgress::Reasoning(text) => self.chunk("agent_thought_chunk", text),
            StreamProgress::Call(_) => {}
        }
    }
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration) {
        let seconds = delay.as_secs_f64().ceil();
        let text = format!("Retrying ({attempt}/{retries}) in {seconds} s: {error}");
        self.say("warning", &text);
    }
    fn assistant(&mut self, text: &str) {
        if !self.streamed && !text.is_empty() {
            self.agent_message(text);
        }
        self.streamed = false;
    }
    fn tool_call(&mut self, name: &str, summary: &str, _compact: Option<&CompactRead>) {
        self.calls += 1;
        let title = match summary.trim() {
            "" => name,
            summary => summary,
        };
        self.update(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": self.call_id(),
            "title": title,
            "name": name,
            "kind": kind(name),
            "status": "pending",
        }));
    }
    fn tool_running(&mut self, _since: Instant) {
        self.call_update(json!({"status": "in_progress"}));
    }
    fn tool_output(&mut self, output: &str) {
        let content = json!([{"type": "content", "content": {"type": "text", "text": output}}]);
        self.call_update(json!({"content": content}));
    }
    fn tool_result(
        &mut self,
        _name: &str,
        output: &str,
        detail: Option<&ToolDetail>,
        failed: bool,
    ) {
        let text = match detail {
            Some(ToolDetail::Diff(diff)) if !failed => format!("```diff\n{diff}\n```"),
            _ => output.to_owned(),
        };
        let content = json!([{"type": "content", "content": {"type": "text", "text": text}}]);
        let status = match failed {
            true => "failed",
            false => "completed",
        };
        self.call_update(json!({"status": status, "content": content}));
    }
    fn command_started(&mut self, _command: &str, _include: bool) {}
    fn command_result(&mut self, _ran: &UserCommand) {}
    fn stopped(&mut self, error: Option<&str>) {
        self.stopped = Some(error.map(str::to_owned));
    }
    fn compacted(
        &mut self,
        _compaction: &Compaction,
        _automatic: Option<scramjet::agent::AutoCompaction>,
    ) {
        self.say("info", "Context compacted");
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        self.say("warning", &format!("Compaction failed: {error}"));
    }
    fn user_prompt(&mut self, _prompt: &str) {}
    fn compaction_summary(&mut self, _summary: &str) {}
    fn branch_summary(&mut self, _summary: &str) {}
    /// Permission requests are not built: `--ask` is refused at start, and project agents'
    /// consent is denied.
    fn approve(&mut self, question: &str, _cancel: &AtomicBool) -> Answer {
        self.say(
            "warning",
            &format!("Denied, as ACP mode cannot ask yet: {question}"),
        );
        Answer::Deny
    }
    /// What a question asks, which the next prompt answers.
    fn question(&mut self, message: &str) {
        self.agent_message(message);
    }
    fn notice(&mut self, text: &str) {
        self.say("info", text);
    }
    fn next_step(&mut self, record: &str) {
        self.say("info", record);
    }
    fn steering(&mut self) -> Vec<String> {
        lock(&self.turn).messages.steer(self.steering)
    }
    fn follow_up(&mut self) -> Vec<String> {
        lock(&self.turn).messages.follow_up(self.follow_up)
    }
    fn usage(&mut self, tokens: Tokens, context: Option<u64>, _rate: Option<f64>) {
        self.usage_update(context, tokens.cost);
    }
    fn choose(
        &mut self,
        _title: &str,
        _options: &[Choice],
        _recommended: Option<usize>,
    ) -> Option<usize> {
        None
    }
}
