//! `scramjet acp`: the agent as an Agent Client Protocol (v1) agent over stdio, for editors and
//! other clients (`docs/RPC.md`). JSON-RPC 2.0, one message per line; diagnostics go to stderr.
//! Each session runs on a thread of its own, driven as the full screen drives its own
//! (`drive::run_turn`), while this thread reads the client's messages.
use super::args::{Options, Start as Launch};
use super::drive::{self, Change, Front, Messages, Offer, Picked, Routed, Session, Start};
use super::settings::Settings;
use super::slash::Builtin;
use scramjet::agent::{
    Agent, AgentError, AgentUi, Answer, Attached, Children, Choice, CompactRead, Compaction,
    McpServer, McpTransport, ModelOption, Purpose, SessionChoice, StreamProgress, Tokens,
    ToolDetail, UserCommand,
};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::HashMap,
    ffi::OsString,
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
const NEXT_STEP: &str = "_io.github.leanandmean.scramjet/next_step";
const HOLD: &str = "_io.github.leanandmean.scramjet/hold";
/// How often a wait looks again at what may end it.
const POLL: Duration = Duration::from_millis(50);

/// Runs `scramjet acp` with the options in `arguments` until the client closes stdin or SIGTERM
/// comes.
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
    let saved = match options.start {
        Launch::Unsaved => Saved::Nowhere,
        _ => match options.session_dir.or_else(|| settings.session_dir.clone()) {
            Some(directory) => Saved::In(directory),
            None => Saved::ByProject,
        },
    };
    drop(settings);
    let mut connection = Connection {
        out: Out::default(),
        arguments: arguments.to_vec(),
        saved,
        closed: Vec::new(),
        initialized: false,
        notices: false,
        compaction: false,
        forms: false,
        booleans: false,
        scramjet: false,
        sessions: HashMap::new(),
        renamed: Arc::default(),
    };
    let (sent, inputs) = mpsc::channel();
    on_sigterm(sent.clone()).map_err(|e| format!("acp: SIGTERM: {e}"))?;
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let ended = line.is_err();
            if sent.send(Input::Line(line)).is_err() || ended {
                return;
            }
        }
        let _ = sent.send(Input::Ended);
    });
    let mut failed = None;
    for input in inputs {
        match input {
            Input::Line(Ok(line)) => {
                let line = line.strip_suffix('\r').unwrap_or(&line);
                if !line.trim().is_empty() {
                    connection.receive(line);
                }
            }
            Input::Line(Err(error)) => {
                failed = Some(error.to_string());
                break;
            }
            Input::Ended => break,
            Input::Terminated => {
                // A turn that does not stop when cancelled does not hold the exit up.
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_secs(5));
                    scramjet::agent::end();
                    std::process::exit(TERMINATED);
                });
                connection.end();
                std::process::exit(TERMINATED);
            }
        }
    }
    connection.end();
    failed.map_or(Ok(()), Err)
}

/// The status a shell gives a process killed by SIGTERM.
const TERMINATED: i32 = 128 + signal_hook::consts::SIGTERM;

/// What the main thread waits for.
enum Input {
    Line(std::io::Result<String>),
    /// The client closed stdin.
    Ended,
    Terminated,
}

/// Has SIGTERM send `Input::Terminated`, which the handler cannot do itself.
fn on_sigterm(sent: mpsc::Sender<Input>) -> std::io::Result<()> {
    let (mut reader, writer) = std::io::pipe()?;
    std::thread::spawn(move || {
        if reader.read_exact(&mut [0]).is_ok() {
            let _ = sent.send(Input::Terminated);
        }
    });
    let wake = move || {
        let _ = (&writer).write(&[0]);
    };
    // SAFETY: a write(2) to a pipe is async-signal-safe.
    unsafe { signal_hook::low_level::register(signal_hook::consts::SIGTERM, wake) }?;
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

/// The client's stdout: each message written whole, as one line; and the requests sent to the
/// client that wait for its response, by id.
#[derive(Clone, Default)]
struct Out {
    written: Arc<Mutex<()>>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
    ids: Arc<AtomicU64>,
}

impl Out {
    fn send(&self, message: &Value) {
        // Inside JSON these appear only in strings, where escaping them is the same text; some
        // readers split lines at them.
        let line = (message.to_string())
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let _held = lock(&self.written);
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
    /// Sends request `method`; its response comes through the receiver, unless `forget` drops
    /// it first.
    fn request(&self, method: &str, params: Value) -> (Value, mpsc::Receiver<Value>) {
        let id = json!(self.ids.fetch_add(1, Ordering::Relaxed) + 1);
        let (sent, answered) = mpsc::channel();
        lock(&self.pending).insert(id.to_string(), sent);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        (id, answered)
    }
    /// Hands a response to the request it answers; one nothing waits for any more is dropped.
    fn answered(&self, message: Value) {
        if let Some(waiting) = lock(&self.pending).remove(&message["id"].to_string()) {
            let _ = waiting.send(message);
        }
    }
    fn forget(&self, id: &Value) {
        lock(&self.pending).remove(&id.to_string());
    }
    /// Withdraws request `id`, whose answer is no longer waited for.
    fn withdraw(&self, id: &Value) {
        self.forget(id);
        let params = json!({"requestId": id});
        self.send(&json!({"jsonrpc": "2.0", "method": "$/cancel_request", "params": params}));
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
    saved: Saved,
    /// The threads of closed sessions, which may still be ending their turns.
    closed: Vec<JoinHandle<()>>,
    initialized: bool,
    /// The client offered `session.notices`.
    notices: bool,
    /// The client offered `session.compaction`.
    compaction: bool,
    /// The client offered `elicitation.form`.
    forms: bool,
    /// The client offered `session.configOptions.boolean`.
    booleans: bool,
    /// The client knows Scramjet's extensions (`_meta["io.github.leanandmean.scramjet"]`).
    scramjet: bool,
    sessions: HashMap<String, Handle>,
    /// The ids sessions took later (a next step's fresh session), each with the one `sessions`
    /// holds it under.
    renamed: Renamed,
}

type Renamed = Arc<Mutex<HashMap<String, String>>>;

/// Where the sessions are saved.
enum Saved {
    /// `--no-session`.
    Nowhere,
    /// `--session-dir` or `sessionDir`, as given: relative to the session's folder.
    In(String),
    /// In each project's directory under the state directory.
    ByProject,
}

/// How many sessions a page of `session/list` holds.
const PAGE: usize = 50;

/// ACP's error for a session the agent does not know.
const UNKNOWN_SESSION: i64 = -32002;

/// What this thread holds of a session that runs on its own.
struct Handle {
    jobs: mpsc::Sender<Job>,
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
    /// The client held the countdown of the next-step list open (`hold`).
    held: bool,
    /// Config options the client set while it runs, which the session takes at its next pause.
    configuring: Vec<Configure>,
}

/// How `session/new`, `resume` and `load` start a session.
#[derive(Clone, Copy, PartialEq)]
enum Opening {
    New,
    Resume,
    /// Resumed, its current branch replayed before the answer.
    Load,
}

/// What a session's thread is sent while it waits between turns.
enum Job {
    /// Request `id`'s prompt.
    Prompt(Value, Prompt),
    Configure(Configure),
}

/// A `session/set_config_option` request, answered once the session takes it.
struct Configure {
    id: Value,
    option: String,
    value: Value,
}

impl Connection {
    fn receive(&mut self, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return self.out.error(&Value::Null, PARSE_ERROR, "Parse error");
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if message.get("result").is_some() || message.get("error").is_some() {
                return self.out.answered(message);
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
            "session/new" => self.open(&id, &params, Opening::New),
            "session/resume" => self.open(&id, &params, Opening::Resume),
            "session/load" => self.open(&id, &params, Opening::Load),
            "session/list" => self.list(&id, &params),
            "session/close" => self.close(&id, &params),
            "session/delete" => self.delete(&id, &params),
            "session/prompt" => self.prompt(id, &params),
            "session/set_config_option" => self.set_config_option(&id, &params),
            HOLD => match self.hold(&params) {
                true => self.out.respond(&id, json!({})),
                false => self.out.error(&id, INVALID_PARAMS, "no such session"),
            },
            _ => self.out.error(&id, METHOD_NOT_FOUND, "Method not found"),
        }
    }

    fn notified(&mut self, method: &str, params: &Value) {
        if method == "session/cancel"
            && let Some(handle) = self.handle(params)
        {
            let mut turn = lock(&handle.turn);
            if turn.running {
                turn.messages.take();
                handle.cancel.store(true, Ordering::Relaxed);
            }
        } else if method == HOLD {
            self.hold(params);
        }
    }

    /// Stops the countdown of the next-step list open in the session `params` names; whether
    /// there is that session.
    fn hold(&self, params: &Value) -> bool {
        let Some(handle) = self.handle(params) else {
            return false;
        };
        lock(&handle.turn).held = true;
        true
    }

    /// Hands the option set to the session, which takes it at once between turns, else at the
    /// turn's next pause, as the full screen takes `/model` typed during a turn, and answers.
    fn set_config_option(&self, id: &Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.out.error(id, INVALID_PARAMS, "no such session");
        };
        let Some(option) = params["configId"].as_str() else {
            return self.out.error(id, INVALID_PARAMS, "configId is required");
        };
        let configure = Configure {
            id: id.clone(),
            option: option.to_owned(),
            value: params["value"].clone(),
        };
        let mut turn = lock(&handle.turn);
        if turn.running {
            return turn.configuring.push(configure);
        }
        drop(turn);
        let _ = handle.jobs.send(Job::Configure(configure));
    }

    /// The session `params` names, by any id it had.
    fn handle(&self, params: &Value) -> Option<&Handle> {
        let id = params["sessionId"].as_str()?;
        match lock(&self.renamed).get(id) {
            Some(first) => self.sessions.get(first),
            None => self.sessions.get(id),
        }
    }

    fn initialize(&mut self, id: &Value, params: &Value) {
        self.initialized = true;
        let capabilities = &params["clientCapabilities"];
        self.notices = capabilities["session"]["notices"].is_object();
        self.compaction = capabilities["session"]["compaction"].is_object();
        self.forms = capabilities["elicitation"]["form"].is_object();
        self.booleans = capabilities["session"]["configOptions"]["boolean"].is_object();
        self.scramjet = capabilities["_meta"]["io.github.leanandmean.scramjet"].is_object();
        let result = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"list": {}, "resume": {}, "close": {}, "delete": {}},
                "promptCapabilities": {"image": true, "audio": false, "embeddedContext": true},
                "mcpCapabilities": {"http": true, "sse": false},
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

    /// Starts a session in the folder `params` names: a new one, or the saved one it names,
    /// which the client already shows when resumed, and is shown again when loaded.
    fn open(&mut self, id: &Value, params: &Value, opening: Opening) {
        let Some(cwd) = params["cwd"].as_str().map(PathBuf::from) else {
            return self.out.error(id, INVALID_PARAMS, "cwd is required");
        };
        if !cwd.is_absolute() || !cwd.is_dir() {
            let message = format!("cwd {} is not an absolute path to a folder", cwd.display());
            return self.out.error(id, INVALID_PARAMS, &message);
        }
        let servers = match mcp_servers(&params["mcpServers"]) {
            Ok(servers) => servers,
            Err(error) => return self.out.error(id, INVALID_PARAMS, &error),
        };
        let file = match opening {
            Opening::New => None,
            Opening::Resume | Opening::Load => match self.resumable(params, &cwd) {
                Ok(file) => Some(file),
                Err((code, message)) => return self.out.error(id, code, &message),
            },
        };
        let (started, starts) = mpsc::channel::<Result<String, String>>();
        let (jobs, given) = mpsc::channel();
        let (cancel, turn) = (Arc::<AtomicBool>::default(), Arc::<Mutex<Turn>>::default());
        let worker = Worker {
            out: self.out.clone(),
            notices: self.notices,
            compaction: self.compaction,
            forms: self.forms,
            booleans: self.booleans,
            scramjet: self.scramjet,
            renamed: self.renamed.clone(),
            cancel: cancel.clone(),
            turn: turn.clone(),
        };
        let (arguments, out, answered) = (self.arguments.clone(), self.out.clone(), id.clone());
        let thread = std::thread::spawn(move || {
            let session = match start_session(&arguments, &cwd, servers, file) {
                Ok(session) => session,
                Err(error) => {
                    let _ = started.send(Err(error));
                    return;
                }
            };
            let mut front = worker.front(session);
            if opening == Opening::Load {
                front.replay();
            }
            front.shown = front.options();
            // Answered here, so that the commands' update follows it.
            let result = json!({"sessionId": front.ui.session, "configOptions": front.shown});
            out.respond(&answered, result);
            let _ = started.send(Ok(front.ui.session.clone()));
            front.commands();
            for job in given {
                let (id, Prompt { text, images }) = match job {
                    Job::Prompt(id, prompt) => (id, prompt),
                    Job::Configure(configure) => {
                        front.configure_with(configure);
                        continue;
                    }
                };
                if !images.is_empty() {
                    match Attached::images(&images) {
                        Ok(attached) => front.session.agent.attach(attached),
                        Err(error) => {
                            front.end_turn(&id, Err((INVALID_PARAMS, &error)));
                            continue;
                        }
                    }
                }
                front.run(id, text);
            }
        });
        match starts.recv() {
            Ok(Ok(session)) => {
                let handle = Handle {
                    jobs,
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

    /// The file of the saved session `params` names, to resume in `cwd`: one of its project's
    /// sessions with a conversation, not open on this connection.
    fn resumable(&self, params: &Value, cwd: &Path) -> Result<PathBuf, (i64, String)> {
        let invalid = |message: String| (INVALID_PARAMS, message);
        let wanted = params["sessionId"].as_str().unwrap_or_default();
        // An id names a file, so it never holds a path.
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if wanted.is_empty() || !wanted.chars().all(valid) {
            return Err(invalid(format!("no session {wanted:?}")));
        }
        if self.handle(params).is_some() {
            return Err(invalid(format!("session {wanted} is open already")));
        }
        let unknown = || (UNKNOWN_SESSION, format!("no saved session {wanted}"));
        let root = super::project_root(Some(cwd.to_owned())).map_err(invalid)?;
        let directories = match &self.saved {
            Saved::Nowhere => {
                return Err(invalid("--no-session: no saved sessions to resume".into()));
            }
            Saved::In(directory) => vec![super::given_path(directory, cwd)],
            Saved::ByProject => {
                let projects =
                    scramjet::agent::projects_directory().map_err(|e| invalid(e.to_string()))?;
                let mut directories = vec![
                    scramjet::agent::session_directory(&root)
                        .map_err(|e| invalid(e.to_string()))?,
                ];
                let others = std::fs::read_dir(projects).into_iter().flatten().flatten();
                directories.extend(others.map(|entry| entry.path()));
                directories
            }
        };
        let file = (directories.iter())
            .map(|directory| directory.join(format!("{wanted}.jsonl")))
            .find(|file| file.is_file())
            .ok_or_else(unknown)?;
        let info = scramjet::agent::Session::info(&file).map_err(|_| unknown())?;
        if info.item_count == 0 {
            return Err(unknown());
        }
        match info.root {
            Some(found) if found != root => Err(invalid(format!(
                "session {wanted} is in {}, not {}",
                found.display(),
                root.display()
            ))),
            _ => Ok(file),
        }
    }

    /// `/resume`'s sessions, newest first, those of the folder `params` names or every
    /// project's, a page at a time.
    fn list(&self, id: &Value, params: &Value) {
        let cwd = params["cwd"].as_str().map(PathBuf::from);
        if cwd.as_ref().is_some_and(|cwd| !cwd.is_absolute()) {
            return self
                .out
                .error(id, INVALID_PARAMS, "cwd is not an absolute path");
        }
        let start = match params["cursor"].as_str().map(str::parse::<usize>) {
            None => 0,
            Some(Ok(start)) => start,
            Some(Err(_)) => return self.out.error(id, INVALID_PARAMS, "invalid cursor"),
        };
        let sessions = self.saved_sessions(cwd.as_deref());
        let page: Vec<Value> = (sessions.iter().skip(start).take(PAGE))
            .filter_map(listed_session)
            .collect();
        let mut result = json!({"sessions": page});
        if sessions.len() > start + PAGE {
            result["nextCursor"] = json!((start + PAGE).to_string());
        }
        self.out.respond(id, result);
    }

    /// The saved sessions with a conversation, newest first: those of the project `cwd`, else
    /// every project's. Unreadable ones are left out, as `/resume` leaves them.
    fn saved_sessions(&self, cwd: Option<&Path>) -> Vec<scramjet::agent::SessionInfo> {
        let root = cwd.map(|cwd| {
            super::project_root(Some(cwd.to_owned())).unwrap_or_else(|_| cwd.to_owned())
        });
        let directories = match &self.saved {
            Saved::Nowhere => Vec::new(),
            Saved::In(directory) => {
                let base = match cwd {
                    Some(cwd) => cwd.to_owned(),
                    None => std::env::current_dir().unwrap_or_default(),
                };
                vec![super::given_path(directory, &base)]
            }
            Saved::ByProject => match &root {
                Some(root) => scramjet::agent::session_directory(root)
                    .into_iter()
                    .collect(),
                None => (scramjet::agent::projects_directory().into_iter())
                    .flat_map(std::fs::read_dir)
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .collect(),
            },
        };
        let mut sessions: Vec<_> = (directories.iter())
            .flat_map(|directory| scramjet::agent::Session::list(directory).unwrap_or_default())
            .filter(|session| session.item_count > 0)
            .filter(|session| root.is_none() || session.root == root)
            .collect();
        sessions.sort_by(|a, b| (b.modified, &b.path).cmp(&(a.modified, &a.path)));
        sessions
    }

    /// Cancels the session's turn, as `session/cancel`, and lets it go; its file stays. A session
    /// not open is closed already.
    fn close(&mut self, id: &Value, params: &Value) {
        let wanted = params["sessionId"].as_str().unwrap_or_default();
        let first = lock(&self.renamed).get(wanted).cloned();
        let first = first.unwrap_or_else(|| wanted.to_owned());
        if let Some(handle) = self.sessions.remove(&first) {
            lock(&self.renamed).retain(|_, held| *held != first);
            let mut turn = lock(&handle.turn);
            if turn.running {
                turn.messages.take();
                handle.cancel.store(true, Ordering::Relaxed);
            }
            drop(turn);
            drop(handle.jobs);
            self.closed.push(handle.thread);
        }
        self.out.respond(id, json!({}));
    }

    /// Removes a saved session as Ctrl+D in `/resume` does: to the trash when a `trash` command
    /// exists. One open on this connection is refused; one not found is deleted already (ACP).
    fn delete(&self, id: &Value, params: &Value) {
        let wanted = params["sessionId"].as_str().unwrap_or_default();
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if wanted.is_empty() || !wanted.chars().all(valid) {
            return self.out.respond(id, json!({}));
        }
        if self.handle(params).is_some() {
            let message = format!("session {wanted} is open: close it first");
            return self.out.error(id, INVALID_PARAMS, &message);
        }
        let directories = match &self.saved {
            Saved::Nowhere => Vec::new(),
            Saved::In(directory) => {
                let base = std::env::current_dir().unwrap_or_default();
                vec![super::given_path(directory, &base)]
            }
            Saved::ByProject => (scramjet::agent::projects_directory().into_iter())
                .flat_map(std::fs::read_dir)
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .collect(),
        };
        let file = (directories.iter())
            .map(|directory| directory.join(format!("{wanted}.jsonl")))
            .find(|file| file.is_file());
        match file.map(|file| scramjet::agent::Session::delete(&file)) {
            None | Some(Ok(_)) => self.out.respond(id, json!({})),
            Some(Err(error)) => {
                let message = format!("Failed to delete: {error}");
                self.out.error(id, INTERNAL_ERROR, &message)
            }
        }
    }

    fn prompt(&mut self, id: Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.out.error(&id, INVALID_PARAMS, "no such session");
        };
        let prompt = match prompt_content(&params["prompt"]) {
            Ok(prompt) => prompt,
            Err(error) => return self.out.error(&id, INVALID_PARAMS, &error),
        };
        // With images the text goes after them, so it is never a command, as with `@` files.
        if prompt.images.is_empty()
            && let Some(builtin) = Builtin::parse(&prompt.text)
        {
            let name = prompt.text.split_whitespace().next().unwrap_or_default();
            let instead = match builtin {
                Builtin::Model(_) | Builtin::Autopilot(_) => {
                    "; session/set_config_option sets the model, thinking level and autopilot"
                }
                _ => "",
            };
            let message =
                format!("{name} is a built-in command, which ACP mode does not run{instead}");
            return self.out.error(&id, INVALID_PARAMS, &message);
        }
        let follow_up = params["_meta"]["io.github.leanandmean.scramjet"]["queue"] == "followUp";
        let mut turn = lock(&handle.turn);
        if turn.running {
            let text = match queued_text(prompt) {
                Ok(text) => text,
                Err(error) => {
                    drop(turn);
                    return self.out.error(&id, INVALID_PARAMS, &error);
                }
            };
            turn.messages.push(text, follow_up);
            turn.waiting.push(id);
            return;
        }
        turn.running = true;
        handle.cancel.store(false, Ordering::Relaxed);
        drop(turn);
        let _ = handle.jobs.send(Job::Prompt(id, prompt));
    }

    /// At the end of input or SIGTERM: every turn cancelled, and each session's thread waited for, which
    /// writes its session file as it goes.
    fn end(self) {
        for handle in self.sessions.values() {
            handle.cancel.store(true, Ordering::Relaxed);
        }
        for (_, handle) in self.sessions {
            drop(handle.jobs);
            let _ = handle.thread.join();
        }
        for thread in self.closed {
            let _ = thread.join();
        }
    }
}

/// A saved session as `session/list` shows it: its title its name, else its first prompt's
/// first line; `_meta` its file and the session before it in its chain. One whose header names
/// no project is left out, as ACP's `cwd` is required.
fn listed_session(session: &scramjet::agent::SessionInfo) -> Option<Value> {
    let id = |path: &Path| Some(path.file_stem()?.to_str()?.to_owned());
    let mut listed = json!({
        "sessionId": id(&session.path)?,
        "cwd": session.root.as_ref()?,
        "updatedAt": utc_time(session.modified),
        "_meta": {"io.github.leanandmean.scramjet": {"path": session.path}},
    });
    let title = (session.name.clone()).or_else(|| {
        Some(
            session
                .first_prompt
                .as_ref()?
                .lines()
                .next()?
                .trim()
                .to_owned(),
        )
    });
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        listed["title"] = json!(title);
    }
    if let Some(before) = session.continues.as_deref().and_then(id) {
        listed["_meta"]["io.github.leanandmean.scramjet"]["parentSession"] = json!(before);
    }
    Some(listed)
}

/// `at` in ISO 8601, in UTC to the second.
fn utc_time(at: std::time::SystemTime) -> String {
    let seconds = (at.duration_since(std::time::UNIX_EPOCH)).map_or(0, |since| since.as_secs());
    let (year, month, day) = super::usage::civil_date((seconds / 86_400) as i64);
    let (hour, minute, second) = (seconds % 86_400 / 3600, seconds % 3600 / 60, seconds % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// A session's config options, as ACP shows them: `model`, the providers' `models` grouped by
/// provider, the current model's first, as `/model` lists them; `thought_level`, the levels
/// Shift+Tab cycles through, for a model that has them; `autopilot`, a boolean for clients that
/// offer those, else `on` or `off`.
fn config_options(agent: &Agent, models: &[ModelOption], booleans: bool) -> Value {
    let current = agent.model();
    let provider = |model: &ModelOption| model.provider.clone().unwrap_or_default();
    let mut providers: Vec<String> = Vec::new();
    for model in models {
        if !providers.contains(&provider(model)) {
            providers.push(provider(model));
        }
    }
    if let Some(at) = (models.iter())
        .find(|model| model.reference() == current)
        .and_then(|model| providers.iter().position(|name| *name == provider(model)))
    {
        let first = providers.remove(at);
        providers.insert(0, first);
    }
    let groups: Vec<Value> = (providers.iter())
        .map(|name| {
            let options: Vec<Value> = (models.iter())
                .filter(|model| provider(model) == *name)
                .map(|model| {
                    let shown = model.name.as_deref().unwrap_or(&model.id);
                    json!({"value": model.reference(), "name": shown})
                })
                .collect();
            let (group, shown) = match name.is_empty() {
                true => ("_none", "Offered by no provider"),
                false => (name.as_str(), name.as_str()),
            };
            json!({"group": group, "name": shown, "options": options})
        })
        .collect();
    let mut options = vec![json!({
        "id": "model",
        "name": "Model",
        "category": "model",
        "type": "select",
        "currentValue": current,
        "options": groups,
    })];
    let levels = agent.effort_levels();
    if agent.reasons() && !levels.is_empty() {
        let effort = agent.effort().unwrap_or(DEFAULT_LEVEL);
        let mut values: Vec<&str> = levels.iter().map(String::as_str).collect();
        if !values.contains(&effort) {
            values.insert(0, effort);
        }
        let values: Vec<Value> = (values.into_iter())
            .map(|level| json!({"value": level, "name": level_name(level)}))
            .collect();
        options.push(json!({
            "id": "thought_level",
            "name": "Thinking",
            "category": "thought_level",
            "type": "select",
            "currentValue": effort,
            "options": values,
        }));
    }
    let mut option = json!({
        "id": "autopilot",
        "name": "Autopilot",
        "description": "When a command finishes, the next step it recommends runs after a \
            3-second countdown, unless you choose.",
        "category": "_autopilot",
    });
    let autopilot = agent.autopilot();
    match booleans {
        true => {
            option["type"] = json!("boolean");
            option["currentValue"] = json!(autopilot);
        }
        false => {
            option["type"] = json!("select");
            option["currentValue"] = json!(if autopilot { "on" } else { "off" });
            option["options"] = json!([{"value": "on", "name": "On"},
                {"value": "off", "name": "Off"}]);
        }
    }
    options.push(option);
    Value::Array(options)
}

/// `thought_level`'s value while no level is set, which leaves it to the model.
const DEFAULT_LEVEL: &str = "default";

/// A thinking level as the client shows it, in Scramjet's words: `none` is off.
fn level_name(level: &str) -> String {
    if level == DEFAULT_LEVEL {
        return "Model's default".to_owned();
    }
    let level = super::settings::scramjet_level(level);
    let mut chars = level.chars();
    (chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect()))
    .unwrap_or_default()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A prompt as the client sent it.
struct Prompt {
    text: String,
    /// The images' bytes, decoded from base64.
    images: Vec<Vec<u8>>,
}

/// The text of a prompt's content blocks (text as given, an embedded resource's text under its
/// URI, a link's URI), and its images.
fn prompt_content(blocks: &Value) -> Result<Prompt, String> {
    let blocks = blocks
        .as_array()
        .ok_or("prompt must be an array of content blocks")?;
    let (mut parts, mut images) = (Vec::new(), Vec::new());
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
            Some("image") => {
                let data = block["data"].as_str().ok_or("an image without data")?;
                let bytes =
                    scramjet::agent::decode_base64(data).ok_or("an image's data is not base64")?;
                images.push(bytes);
            }
            Some(other) => return Err(format!("{other} content is not taken")),
            None => return Err("a content block without a type".to_owned()),
        }
    }
    let text = parts.join("\n\n");
    Ok(Prompt { text, images })
}

/// A prompt queued during a turn, which takes text only: its images saved to temporary files
/// whose paths go before the text, as the full screen pastes an image.
fn queued_text(prompt: Prompt) -> Result<String, String> {
    let mut extensions = Vec::new();
    for (at, bytes) in prompt.images.iter().enumerate() {
        let mime = scramjet::agent::image_type(bytes)
            .ok_or_else(|| format!("image {} is not a PNG, JPEG, GIF or WebP image", at + 1))?;
        extensions.push(mime.trim_start_matches("image/"));
    }
    let directory = std::env::temp_dir().to_string_lossy().into_owned();
    let mut words = Vec::new();
    for (bytes, extension) in prompt.images.iter().zip(extensions) {
        let path = super::clipboard::save(bytes, extension, &directory)?;
        words.push(match path.contains(char::is_whitespace) {
            true => format!("\"{path}\""),
            false => path,
        });
    }
    if !prompt.text.is_empty() {
        words.push(prompt.text);
    }
    Ok(words.join(" "))
}

/// The MCP servers `session/new` gives: stdio ones `{name, command, args, env}`, HTTP ones
/// `{type: "http", name, url, headers}`, their variables and headers lists of `{name, value}`.
fn mcp_servers(servers: &Value) -> Result<Vec<McpServer>, String> {
    let servers = match servers {
        Value::Null => return Ok(Vec::new()),
        Value::Array(servers) => servers,
        _ => return Err("mcpServers must be a list of servers".to_owned()),
    };
    let pairs = |server: &Value, key: &str| -> Result<Vec<(String, String)>, String> {
        let Some(pairs) = server.get(key) else {
            return Ok(Vec::new());
        };
        let wrong = || format!("{key} must be a list of {{name, value}}");
        (pairs.as_array().ok_or_else(wrong)?.iter())
            .map(
                |pair| match (pair["name"].as_str(), pair["value"].as_str()) {
                    (Some(name), Some(value)) => Ok((name.to_owned(), value.to_owned())),
                    _ => Err(wrong()),
                },
            )
            .collect()
    };
    let mut taken: Vec<McpServer> = Vec::new();
    for server in servers {
        let name = (server["name"].as_str())
            .filter(|name| !name.trim().is_empty())
            .ok_or("an MCP server without a name")?;
        let given = |error: String| format!("MCP server {name}: {error}");
        let server = match server["type"].as_str() {
            None | Some("stdio") => {
                let command = (server["command"].as_str())
                    .filter(|command| !command.trim().is_empty())
                    .ok_or_else(|| given("command is required".to_owned()))?;
                let args = match &server["args"] {
                    Value::Null => Vec::new(),
                    Value::Array(args) => (args.iter())
                        .map(|arg| arg.as_str().map(str::to_owned))
                        .collect::<Option<_>>()
                        .ok_or_else(|| given("args must be a list of strings".to_owned()))?,
                    _ => return Err(given("args must be a list of strings".to_owned())),
                };
                let env = pairs(server, "env").map_err(given)?;
                McpServer {
                    name: name.to_owned(),
                    transport: McpTransport::Program {
                        command: command.to_owned(),
                        args,
                        env,
                    },
                }
            }
            Some("http") => {
                let url = server["url"].as_str().unwrap_or_default();
                let headers = pairs(server, "headers").map_err(given)?;
                McpServer::http(name, url, headers).map_err(given)?
            }
            Some(other) => return Err(given(format!("the {other} transport is not supported"))),
        };
        if taken.iter().any(|known| known.name == server.name) {
            return Err(given("given twice".to_owned()));
        }
        taken.push(server);
    }
    Ok(taken)
}

/// Starts a session in `cwd` with the options `arguments` give, as the full screen starts one,
/// or continues the saved session `file`, with the client's MCP `servers` beside the settings' (in place of those of the same name,
/// and started though the settings disable that name).
fn start_session(
    arguments: &[String],
    cwd: &Path,
    servers: Vec<McpServer>,
    file: Option<PathBuf>,
) -> Result<Session, String> {
    let options = Options::parse(arguments.iter().map(OsString::from))?
        .ok_or("--help is not a session's option")?;
    let choice = match (file, &options.start) {
        (Some(file), _) => SessionChoice::Open(file),
        (None, Launch::Unsaved) => SessionChoice::Unsaved,
        (None, _) => SessionChoice::New,
    };
    let root = super::project_root(Some(cwd.to_owned()))?;
    let mut settings = Settings::load();
    settings.retries_given = options.retries;
    let given = |name: &String| servers.iter().any(|server| &server.name == name);
    (settings.mcp_servers).retain(|server| !given(&server.name));
    (settings.disabled_mcp_servers).retain(|name| !given(name));
    settings.mcp_servers.extend(servers);
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
    compaction: bool,
    forms: bool,
    booleans: bool,
    scramjet: bool,
    renamed: Renamed,
    cancel: Arc<AtomicBool>,
    turn: Arc<Mutex<Turn>>,
}

impl Worker {
    fn front(self, session: Session) -> Acp {
        let ui = Updates {
            out: self.out,
            first: session.agent.session_id(),
            session: session.agent.session_id(),
            notices: self.notices,
            compaction: self.compaction,
            forms: self.forms,
            compactions: 0,
            compacting: false,
            turn: self.turn.clone(),
            steering: session.settings.steering_mode,
            follow_up: session.settings.follow_up_mode,
            responses: 0,
            streamed: false,
            calls: 0,
            root: session.agent.root().to_path_buf(),
            input: Value::Null,
            call: (String::new(), Value::Null),
            new_file: false,
            window: None,
            used: None,
            stopped: None,
            asked: None,
            cancel: self.cancel.clone(),
            replaying: false,
            held: Cell::new(None),
            thinking: false,
        };
        Acp {
            session,
            ui,
            cancel: self.cancel,
            turn: self.turn,
            reason: None,
            scramjet: self.scramjet,
            renamed: self.renamed,
            booleans: self.booleans,
            models: Vec::new(),
            shown: Value::Null,
        }
    }
}

/// A session as ACP drives it.
struct Acp {
    session: Session,
    ui: Updates,
    cancel: Arc<AtomicBool>,
    turn: Arc<Mutex<Turn>>,
    /// The stop reason of a model turn that ended without failing though it went no further:
    /// `max_tokens`, `refusal`.
    reason: Option<&'static str>,
    scramjet: bool,
    renamed: Renamed,
    /// The client offered `session.configOptions.boolean`.
    booleans: bool,
    /// The providers' models the `model` option lists, read again when the model is not among
    /// them.
    models: Vec<ModelOption>,
    /// The config options the client was last given.
    shown: Value,
}

/// How a prompt's turns ended.
enum Ended {
    /// ACP's stop reason, `cancelled` aside.
    Reason(&'static str),
    Failed(String),
}

impl Acp {
    /// Shows the session's current branch again, as updates, as the full screen shows a resumed
    /// session.
    fn replay(&mut self) {
        self.ui.replaying = true;
        self.session.agent.show_conversation(&mut self.ui);
        self.ui.flush();
        self.ui.replaying = false;
        (self.ui.asked, self.ui.stopped, self.ui.thinking) = (None, None, false);
    }

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
        self.configure();
        let (mut prompt, mut start) = (prompt, Start::Typed);
        let stopped = loop {
            let next = drive::run_turn(self, Some(&prompt), start);
            let stopped = self.ui.stopped.take();
            if let Some(reason) = self.reason.take() {
                // What the model finished stays shown; the reason says why it went no further.
                if let Some(Some(error)) = &stopped {
                    self.ui.say("warning", error);
                }
                break Ended::Reason(reason);
            }
            if stopped.is_some() || self.cancel.load(Ordering::Relaxed) {
                break stopped
                    .flatten()
                    .map_or(Ended::Reason("end_turn"), Ended::Failed);
            }
            if let Some((step, how)) = next {
                self.ui.user_message(&step);
                (prompt, start) = (step, how);
                continue;
            }
            let mut turn = lock(&self.turn);
            let queued = turn.messages.take();
            if queued.is_empty() {
                break Ended::Reason("end_turn");
            }
            drop(turn);
            (prompt, start) = (queued.join("\n\n"), Start::Typed);
        };
        match stopped {
            Ended::Reason(reason) => self.end_turn(&id, Ok(reason)),
            Ended::Failed(error) => self.end_turn(&id, Err((INTERNAL_ERROR, &error))),
        }
    }

    /// Takes the config options the client set during the turn.
    fn configure(&mut self) {
        let configuring = std::mem::take(&mut lock(&self.turn).configuring);
        for configure in configuring {
            self.configure_with(configure);
        }
    }

    /// Sets the option `configure` names and answers its request with the options, or with why
    /// it was not set.
    fn configure_with(&mut self, configure: Configure) {
        let Configure { id, option, value } = configure;
        match self.set_option(&option, &value) {
            Ok(()) => {
                self.shown = self.options();
                self.ui
                    .out
                    .respond(&id, json!({"configOptions": self.shown}));
            }
            Err(error) => self.ui.out.error(&id, INVALID_PARAMS, &error),
        }
    }

    /// Sets `option` to `value` as the full screen's `/model`, Shift+Tab and `/autopilot` do,
    /// the model remembered and the level saved as the settings' default as they are there.
    fn set_option(&mut self, option: &str, value: &Value) -> Result<(), String> {
        let agent = &mut self.session.agent;
        match option {
            "model" => {
                let model = value.as_str().ok_or("model is a provider/id string")?;
                if model == agent.model() {
                    return Ok(());
                }
                let listed = |models: &[ModelOption]| {
                    (models.iter()).any(|listed| listed.reference() == model)
                };
                if !listed(&self.models) {
                    self.models = agent.model_options();
                }
                if !listed(&self.models) {
                    return Err(format!("no model {model} is offered"));
                }
                agent.set_model(model).map_err(|error| error.to_string())?;
                agent.load_model_info();
                self.ui.window = agent.context_window();
                drive::remember_model(self);
            }
            "thought_level" => {
                let level = value.as_str().ok_or("thought_level is a level's name")?;
                let current = agent.effort().unwrap_or(DEFAULT_LEVEL);
                if level == current {
                    return Ok(());
                }
                if !agent.reasons() || !agent.effort_levels().iter().any(|known| known == level) {
                    return Err(format!("{} takes no thinking level {level}", agent.model()));
                }
                agent.set_effort(level).map_err(|error| error.to_string())?;
                if let Err(error) = self.session.settings.set_default_thinking_level(level) {
                    self.ui
                        .say("warning", &format!("Settings not saved: {error}"));
                }
            }
            "autopilot" => {
                let on = match value {
                    Value::Bool(on) => *on,
                    Value::String(value) if value == "on" => true,
                    Value::String(value) if value == "off" => false,
                    _ => return Err("autopilot is on or off".to_owned()),
                };
                agent.set_autopilot(on).map_err(|error| error.to_string())?;
            }
            other => return Err(format!("no config option {other}")),
        }
        Ok(())
    }

    /// The config options as they are now.
    fn options(&mut self) -> Value {
        let agent = &mut self.session.agent;
        let current = agent.model();
        if !(self.models.iter()).any(|model| model.reference() == current) {
            self.models = agent.model_options();
        }
        config_options(agent, &self.models, self.booleans)
    }

    /// Tells the client of options that changed without its asking: the agent's
    /// `switch_scramjet_model`, a next step's model or level.
    fn options_changed(&mut self) {
        let options = self.options();
        if options != self.shown {
            self.shown = options;
            let update =
                json!({"sessionUpdate": "config_option_update", "configOptions": self.shown});
            self.ui.update(update);
        }
    }

    /// The `next_step` request's parameters for `offer`.
    fn next_step_params(&self, offer: &Offer) -> Value {
        let agent = &self.session.agent;
        let steps: Vec<Value> = (offer.steps.iter())
            .map(|step| {
                json!({"message": step.message, "reason": step.reason,
                    "freshSession": step.fresh()})
            })
            .collect();
        let kind = match offer.suggestion {
            true => "suggestion",
            false => "nextStep",
        };
        let mut params = json!({
            "sessionId": self.ui.session,
            "kind": kind,
            "steps": steps,
            "recommended": offer.recommended,
            "model": agent.model(),
            "thinkingLevel": agent.effort(),
        });
        if let Some(wait) = offer.autoselect {
            params["autoselectMs"] = json!(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX));
        }
        params
    }

    /// The step the client's answer to `next_step` chose, with the model, level and fresh
    /// session it changed; `None` when it cancelled or failed.
    fn picked(&self, offer: &Offer, mut message: Value) -> Option<Picked> {
        let Some(result) = message.get_mut("result").map(Value::take) else {
            let error = message["error"]["message"].as_str().unwrap_or("no reason");
            let text = format!("The client failed {NEXT_STEP}: {error}");
            self.ui.say("warning", &text);
            return None;
        };
        if result["cancelled"] == true {
            return None;
        }
        let at = (result["index"].as_u64())
            .and_then(|at| usize::try_from(at).ok())
            .filter(|at| *at < offer.steps.len());
        let Some(at) = at else {
            self.ui.refused(&result);
            return None;
        };
        let agent = &self.session.agent;
        let model = (result["model"].as_str())
            .filter(|model| *model != agent.model())
            .map(str::to_owned);
        let effort = (result["thinkingLevel"].as_str())
            .filter(|level| Some(*level) != agent.effort())
            .map(str::to_owned);
        let fresh =
            (result["freshSession"].as_bool()).filter(|_| offer.steps[at].command.is_some());
        let routed = Routed {
            model,
            effort,
            fresh,
        };
        Some(Picked {
            at,
            timed: false,
            routed,
        })
    }

    /// Answers request `id` and those of the prompts sent during its turn, the turn having
    /// ended for `ended`'s stop reason or failed with its error; cancelled, it says so.
    fn end_turn(&mut self, id: &Value, ended: Result<&str, (i64, &str)>) {
        let mut turn = lock(&self.turn);
        turn.running = false;
        let waiting = std::mem::take(&mut turn.waiting);
        drop(turn);
        // Those set after the turn's last look, which no later turn would take.
        self.configure();
        let ended = match self.cancel.load(Ordering::Relaxed) {
            true => Ok("cancelled"),
            false => ended,
        };
        for id in Some(id).into_iter().chain(&waiting) {
            match ended {
                Err((code, error)) => self.ui.out.error(id, code, error),
                Ok(reason) => self.ui.out.respond(id, json!({"stopReason": reason})),
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
        self.options_changed();
    }
    fn turn(&mut self, prompt: Option<&str>) -> Result<(), AgentError> {
        let agent = &mut self.session.agent;
        let result = match prompt {
            Some(prompt) => agent.turn(prompt, &mut self.ui, &self.cancel),
            None => agent.resume_turn(&mut self.ui, &self.cancel),
        };
        self.reason = match &result {
            Err(AgentError::OutputLimit(_)) => Some("max_tokens"),
            Err(AgentError::Refusal(_)) => Some("refusal"),
            _ => None,
        };
        result
    }
    fn paused(&mut self) {
        self.configure();
    }
    fn turn_stops(&mut self) {}
    fn turn_ended(&mut self) {
        let agent = &self.session.agent;
        let (context, cost) = (agent.context_tokens(), agent.session_tokens().cost);
        self.ui.window = agent.context_window();
        self.ui.usage_update(context, cost);
        self.configure();
        self.options_changed();
    }
    fn chooses(&self) -> bool {
        true
    }
    /// A client that knows Scramjet is asked with `next_step`; others are shown the list as an
    /// agent message, which a prompt answers. Until the client answers, the session is cancelled
    /// or a prompt comes; with a countdown not held, the recommended step is taken when it ends.
    fn offer(&mut self, offer: Offer) -> Option<Picked> {
        lock(&self.turn).held = false;
        let mut deadline = offer.autoselect.map(|wait| Instant::now() + wait);
        let request = match self.scramjet {
            true => Some(
                self.ui
                    .out
                    .request(NEXT_STEP, self.next_step_params(&offer)),
            ),
            false => {
                self.ui.message(&listed(&offer));
                deadline?;
                None
            }
        };
        let answer = loop {
            let (held, configuring) = {
                let turn = lock(&self.turn);
                if self.cancel.load(Ordering::Relaxed) || !turn.messages.is_empty() {
                    break None;
                }
                (turn.held, !turn.configuring.is_empty())
            };
            if configuring {
                self.configure();
                // Autopilot turned off stops its countdown.
                if !self.session.agent.autopilot() {
                    deadline = None;
                }
            }
            if !held && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                break Some(None);
            }
            match &request {
                Some((_, answered)) => match answered.recv_timeout(POLL) {
                    Ok(message) => break Some(Some(message)),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                },
                None => std::thread::sleep(POLL),
            }
        };
        if let (Some((id, _)), None | Some(None)) = (&request, &answer) {
            self.ui.out.withdraw(id);
        }
        match answer? {
            Some(message) => self.picked(&offer, message),
            None => Some(Picked {
                at: offer.recommended?,
                timed: true,
                routed: Routed::default(),
            }),
        }
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
    /// A client that knows Scramjet follows the new session's id; others keep the thread's,
    /// and are told in it.
    fn session_changed(&mut self, message: &str, _change: Change) {
        let id = self.session.agent.session_id();
        if self.scramjet && id != self.ui.session {
            let params = json!({"sessionId": self.ui.session, "newSessionId": id});
            self.ui.out.send(&json!({"jsonrpc": "2.0",
                "method": "_io.github.leanandmean.scramjet/session_changed", "params": params}));
            lock(&self.renamed).insert(id.clone(), self.ui.first.clone());
            self.ui.session = id;
        }
        self.ui.tell(message);
    }
}

/// The agent's interface in a session: `session/update` notifications to the client.
struct Updates {
    out: Out,
    /// The session's id at its start, and the one its updates carry.
    first: String,
    session: String,
    notices: bool,
    /// The client offered `session.compaction`.
    compaction: bool,
    /// The client offered `elicitation.form`.
    forms: bool,
    /// The compactions so far, which number them, and whether the last one has not ended.
    compactions: usize,
    compacting: bool,
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
    /// The project directory, which relative paths in tool calls are in.
    root: PathBuf,
    /// The arguments of the tool call announced next, then its name and them.
    input: Value,
    call: (String, Value),
    /// The write announced last makes its file.
    new_file: bool,
    window: Option<u64>,
    /// The usage last sent, which is not sent again.
    used: Option<(u64, u64, f64)>,
    /// How the last turn stopped before its end: cancelled (`None`), or failed with an error.
    stopped: Option<Option<String>>,
    /// What `question` showed since the last tool call, which a permission request repeats.
    asked: Option<String>,
    /// The session's cancellation, for what is asked without the turn's.
    cancel: Arc<AtomicBool>,
    /// The session's branch is being shown again (`session/load`).
    replaying: bool,
    /// A replayed tool call not sent yet, which takes its result's status and content, so
    /// that it goes as one `tool_call`.
    held: Cell<Option<Value>>,
    /// The last replayed update was reasoning, whose response the next message is part of.
    thinking: bool,
}

impl Updates {
    fn update(&self, update: Value) {
        self.flush();
        self.out.update(&self.session, update);
    }
    /// Sends the replayed tool call held, if any.
    fn flush(&self) {
        if let Some(held) = self.held.take() {
            self.out.update(&self.session, held);
        }
    }
    /// Replayed, a message or reasoning starts a response of its own unless it follows the
    /// reasoning of its own.
    fn replayed(&mut self, thought: bool) {
        if !self.thinking {
            self.responses += 1;
        }
        self.thinking = thought;
    }
    fn chunk(&self, kind: &str, text: &str) {
        self.update(self.chunked(kind, text));
    }
    fn chunked(&self, kind: &str, text: &str) -> Value {
        let id = format!("response-{}", self.responses);
        let content = json!({"type": "text", "text": text});
        json!({"sessionUpdate": kind, "messageId": id, "content": content})
    }
    fn agent_message(&self, text: &str) {
        self.chunk("agent_message_chunk", text);
    }
    fn user_message(&mut self, text: &str) {
        self.responses += 1;
        self.chunk("user_message_chunk", text);
    }
    /// What the command or skill `typed` sent in its place, as a message of its own.
    fn expansion(&mut self, typed: &str, sent: &str) {
        self.responses += 1;
        let mut update = self.chunked("user_message_chunk", sent);
        update["_meta"] = json!({"io.github.leanandmean.scramjet": {"expands": typed}});
        self.update(update);
    }
    /// The compaction under way, or a new one, has `status`, with `fields` beside it; without
    /// `session.compaction` an ended one is said in `said`, as an agent message of its own.
    fn compaction_update(&mut self, status: &str, fields: Value, said: Option<&str>) {
        if !self.compacting {
            self.compactions += 1;
        }
        self.compacting = status == "in_progress";
        let id = format!("compaction-{}", self.compactions);
        if !self.compaction {
            if let Some(said) = said {
                let content = json!({"type": "text", "text": said});
                let update = json!({"sessionUpdate": "agent_message_chunk", "messageId": id,
                    "content": content});
                self.update(update);
            }
            return;
        }
        let mut update =
            json!({"sessionUpdate": "compaction_update", "compactionId": id, "status": status});
        if let (Some(update), Some(fields)) = (update.as_object_mut(), fields.as_object()) {
            update.extend(fields.clone());
        }
        self.update(update);
    }
    fn call_id(&self) -> String {
        format!("call-{}", self.calls)
    }
    fn call_update(&self, fields: Value) {
        let mut held = self.held.take();
        if let (Some(Value::Object(held)), Value::Object(fields)) = (&mut held, &fields) {
            held.extend(fields.clone());
        }
        if held.is_some() {
            return self.held.set(held);
        }
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
    /// Where the tool call announced last reads or changes files, as absolute paths.
    fn locations(&self) -> Vec<Value> {
        let (name, arguments) = &self.call;
        let path = match (name.as_str(), arguments["path"].as_str()) {
            ("read" | "edit" | "write", Some(path)) => self.path(path),
            _ => return Vec::new(),
        };
        let mut location = json!({"path": path});
        // ACP's lines count from 0, as Zed reads them; `read`'s offset from 1.
        if let (Some(offset), "read") = (arguments["offset"].as_u64(), name.as_str()) {
            location["line"] = json!(offset.saturating_sub(1));
        }
        vec![location]
    }
    /// `path` as the tools resolve it: a leading `~` is the home directory.
    fn path(&self, path: &str) -> PathBuf {
        match scramjet::os::after_tilde(path).zip(std::env::home_dir()) {
            Some((rest, home)) => home.join(rest),
            None => self.root.join(path),
        }
    }
    /// What the edit or write announced last changed, as ACP's diffs: for an edit the text each
    /// replacement replaced, for a write to a new file its content.
    fn diffs(&self, new_file: bool) -> Vec<Value> {
        let (name, arguments) = &self.call;
        let Some(path) = arguments["path"].as_str().map(|path| self.path(path)) else {
            return Vec::new();
        };
        match (name.as_str(), arguments["content"].as_str()) {
            ("edit", _) => {}
            ("write", Some(content)) if new_file => {
                return vec![
                    json!({"type": "diff", "path": path, "oldText": null, "newText": content}),
                ];
            }
            _ => return Vec::new(),
        }
        let parsed;
        let mut edits: Vec<&Value> = match &arguments["edits"] {
            Value::Array(edits) => edits.iter().collect(),
            Value::String(text) => {
                parsed = serde_json::from_str::<Value>(text).ok();
                (parsed.as_ref().and_then(Value::as_array))
                    .map_or_else(Vec::new, |edits| edits.iter().collect())
            }
            _ => Vec::new(),
        };
        edits.push(arguments);
        (edits.into_iter())
            .filter_map(|edit| Some((edit["oldText"].as_str()?, edit["newText"].as_str()?)))
            .map(|(old, new)| json!({"type": "diff", "path": path, "oldText": old, "newText": new}))
            .collect()
    }
    /// A notice that matters, as an agent message of its own without `session.notices`.
    fn tell(&mut self, text: &str) {
        if self.notices {
            return self.say("info", text);
        }
        self.message(text);
    }
    /// An agent message of its own.
    fn message(&mut self, text: &str) {
        self.responses += 1;
        self.agent_message(text);
    }
    /// The client's result for request `method`; `None` once `cancel` is set, or when the
    /// client answered with an error, which a notice says.
    fn ask(&self, method: &str, params: Value, cancel: &AtomicBool) -> Option<Value> {
        let (id, answered) = self.out.request(method, params);
        loop {
            match answered.recv_timeout(POLL) {
                Ok(mut message) => match message.get_mut("result") {
                    Some(result) => return Some(result.take()),
                    None => {
                        let error = message["error"]["message"].as_str().unwrap_or("no reason");
                        self.say("warning", &format!("The client failed {method}: {error}"));
                        return None;
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) if !cancel.load(Ordering::Relaxed) => {}
                Err(_) => {
                    self.out.withdraw(&id);
                    return None;
                }
            }
        }
    }
    /// Asks the client's permission for the tool call announced last, `text` shown with it;
    /// the index of the option of `options` (id, name, kind) chosen, `None` when cancelled or
    /// answered with one not offered.
    fn permission(
        &mut self,
        text: &str,
        options: &[(&str, &str, &str)],
        cancel: &AtomicBool,
    ) -> Option<usize> {
        let content = json!([{"type": "content", "content": {"type": "text", "text": text}}]);
        let offered: Vec<Value> = (options.iter())
            .map(|(id, name, kind)| json!({"optionId": id, "name": name, "kind": kind}))
            .collect();
        let params = json!({
            "sessionId": self.session,
            "toolCall": {"toolCallId": self.call_id(), "content": content},
            "options": offered,
        });
        let outcome = self.ask("session/request_permission", params, cancel)?;
        let outcome = &outcome["outcome"];
        if outcome["outcome"] != "selected" {
            return None;
        }
        let chosen = (options.iter()).position(|(id, ..)| outcome["optionId"] == *id);
        if chosen.is_none() {
            self.say(
                "warning",
                &format!(
                    "Denied: the client chose {}, which was not offered",
                    outcome["optionId"]
                ),
            );
        }
        chosen
    }
    /// The text a request about the tool call announced last shows: what `question` showed
    /// with `title` above it, else `title`.
    fn asking(&mut self, title: &str) -> String {
        match self.asked.take() {
            Some(shown) => format!("{title}\n\n{shown}"),
            None => title.to_owned(),
        }
    }
    /// The answer the client's form gives for `property`, asked with what `question` showed;
    /// `None` when declined or cancelled.
    fn elicit(&mut self, title: &str, property: Value) -> Option<Value> {
        let message = self.asked.take().unwrap_or_else(|| title.to_owned());
        let schema = json!({"type": "object", "properties": {"answer": property},
            "required": ["answer"]});
        let params = json!({
            "sessionId": self.session,
            "toolCallId": self.call_id(),
            "mode": "form",
            "message": message,
            "requestedSchema": schema,
        });
        let mut result = self.ask("elicitation/create", params, &self.cancel.clone())?;
        match result["action"] == "accept" {
            true => Some(result["content"]["answer"].take()),
            false => None,
        }
    }
    /// Without forms: `title` and `options` said after the question, for the next prompt to
    /// answer.
    fn unanswered(&mut self, title: &str, options: &[Choice]) -> Option<usize> {
        let labels: Vec<String> = (options.iter())
            .map(|option| format!("- {}", option.label))
            .collect();
        self.asked = None;
        self.agent_message(&format!(
            "\n\n{title} Reply with one of:\n{}",
            labels.join("\n")
        ));
        None
    }
    fn refused(&self, answer: &Value) {
        self.say(
            "warning",
            &format!("Cancelled: the client answered {answer}, which was not asked"),
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

/// `offer` as text, for a client that does not know `next_step`.
fn listed(offer: &Offer) -> String {
    let mut lines = vec![format!("{}:", offer.title)];
    for (at, step) in offer.steps.iter().enumerate() {
        let recommended = match offer.recommended == Some(at) {
            true => " (recommended)",
            false => "",
        };
        let reason = match step.description() {
            reason if reason.is_empty() => reason,
            reason => format!(" — {reason}"),
        };
        lines.push(format!(
            "- {}{}{recommended}{reason}",
            step.label(),
            step.fresh_mark()
        ));
    }
    lines.push(match offer.autoselect {
        Some(wait) => format!(
            "\nThe recommended step runs in {} s unless you send a prompt.",
            wait.as_secs()
        ),
        None => "\nSend one to run it.".to_owned(),
    });
    lines.join("\n")
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
        match purpose {
            Purpose::Reply => {
                self.responses += 1;
                self.streamed = false;
            }
            // A compaction may ask for two summaries.
            Purpose::Compaction(_) if !self.compacting => {
                self.compaction_update("in_progress", json!({}), None);
            }
            _ => {}
        }
    }
    fn progress(&mut self, update: &StreamProgress) {
        match update {
            StreamProgress::Text(text) => {
                self.streamed = true;
                self.agent_message(text);
            }
            StreamProgress::Reasoning(text) => {
                if self.replaying {
                    self.replayed(true);
                }
                self.chunk("agent_thought_chunk", text)
            }
            StreamProgress::Call(_) => {}
        }
    }
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration) {
        let seconds = delay.as_secs_f64().ceil();
        let text = format!("Retrying ({attempt}/{retries}) in {seconds} s: {error}");
        self.say("warning", &text);
    }
    fn assistant(&mut self, text: &str) {
        if self.replaying {
            self.replayed(false);
        }
        if !self.streamed && !text.is_empty() {
            self.agent_message(text);
        }
        self.streamed = false;
    }
    fn tool_call(&mut self, name: &str, summary: &str, _compact: Option<&CompactRead>) {
        self.calls += 1;
        self.asked = None;
        self.call = (name.to_owned(), std::mem::take(&mut self.input));
        let title = match summary.trim() {
            "" => name,
            summary => summary,
        };
        let mut update = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": self.call_id(),
            "title": title,
            "name": name,
            "kind": kind(name),
            "status": "pending",
        });
        if !self.call.1.is_null() {
            update["rawInput"] = self.call.1.clone();
        }
        let locations = self.locations();
        if !locations.is_empty() {
            update["locations"] = json!(locations);
        }
        if self.replaying {
            // Whether a replayed write made its file is not known.
            self.new_file = false;
            self.thinking = false;
            self.flush();
            return self.held.set(Some(update));
        }
        // Whether a write makes its file is known only before it runs.
        self.new_file = name == "write"
            && (self.call.1["path"].as_str()).is_some_and(|path| !self.path(path).exists());
        self.update(update);
    }
    fn tool_input(&mut self, arguments: &Value) {
        self.input = arguments.clone();
    }
    fn tool_running(&mut self, _since: Instant) {
        self.call_update(json!({"status": "in_progress"}));
    }
    fn tool_output(&mut self, output: &str) {
        let content = json!([{"type": "content", "content": {"type": "text", "text": output}}]);
        self.call_update(json!({"content": content}));
    }
    /// A line per child as the content, for clients that read no `_meta`.
    fn subagents(&mut self, children: &Children) {
        let text = super::children::activity(children, 1000).join("\n");
        let content = json!([{"type": "content", "content": {"type": "text", "text": text}}]);
        let meta = json!({"io.github.leanandmean.scramjet": {"children": children}});
        self.call_update(json!({"content": content, "_meta": meta}));
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
        let mut content =
            vec![json!({"type": "content", "content": {"type": "text", "text": text}})];
        if !failed {
            content.extend(self.diffs(self.new_file));
        }
        let status = match failed {
            true => "failed",
            false => "completed",
        };
        let mut update = json!({"status": status, "content": content});
        if let Some(ToolDetail::Subagents(children)) = detail {
            update["_meta"] = json!({"io.github.leanandmean.scramjet": {"children": children}});
        }
        self.call_update(update);
    }
    fn command_started(&mut self, _command: &str, _include: bool) {}
    fn command_result(&mut self, _ran: &UserCommand) {}
    fn stopped(&mut self, error: Option<&str>) {
        if self.replaying {
            if let Some(error) = error {
                self.message(&format!("The turn failed: {error}"));
            }
            return;
        }
        if self.compacting {
            match error {
                Some(error) => {
                    let said = format!("Compaction failed: {error}");
                    self.compaction_update("failed", json!({"error": error}), Some(&said));
                }
                None => self.compaction_update("cancelled", json!({}), None),
            }
        }
        self.stopped = Some(error.map(str::to_owned));
    }
    fn compacted(
        &mut self,
        compaction: &Compaction,
        automatic: Option<scramjet::agent::AutoCompaction>,
    ) {
        let said = match automatic {
            Some(automatic) => format!("Context compacted. {}", automatic.reason()),
            None => "Context compacted.".to_owned(),
        };
        let summary = json!([{"type": "text", "text": compaction.summary}]);
        let tokens =
            json!({"io.github.leanandmean.scramjet": {"tokensBefore": compaction.tokens_before}});
        let fields = json!({"summary": summary, "_meta": tokens});
        self.compaction_update("completed", fields, Some(&said));
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        let said = format!("Compaction failed: {error}");
        let fields = json!({"error": error.to_string()});
        self.compaction_update("failed", fields, Some(&said));
    }
    fn command_sent(&mut self, typed: &str, sent: &str) {
        self.expansion(typed, sent);
    }
    /// A command queued during the turn: the client sent it as typed. Replayed, as typed too.
    fn command_prompt(&mut self, typed: &str, sent: &str) {
        if self.replaying {
            self.user_message(typed);
        }
        self.expansion(typed, sent);
    }
    /// Replayed only: the client sent the prompts of the turns it shows.
    fn user_prompt(&mut self, prompt: &str) {
        if self.replaying {
            self.thinking = false;
            self.user_message(prompt);
        }
    }
    fn compaction_summary(&mut self, summary: &str) {
        if self.replaying {
            let fields = json!({"summary": [{"type": "text", "text": summary}]});
            self.compaction_update("completed", fields, Some("Context compacted."));
        }
    }
    fn branch_summary(&mut self, summary: &str) {
        if self.replaying {
            let fields = json!({"summary": [{"type": "text", "text": summary}],
                "_meta": {"io.github.leanandmean.scramjet": {"branch": true}}});
            self.compaction_update("completed", fields, Some("Earlier branch summarized."));
        }
    }
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer {
        let text = self.asking(question);
        let options = [
            ("allow_once", "Allow", "allow_once"),
            (
                "allow_always",
                "Allow all calls this session",
                "allow_always",
            ),
            ("reject_once", "Deny", "reject_once"),
        ];
        match self.permission(&text, &options, cancel) {
            Some(0) => Answer::Allow,
            Some(1) => Answer::AllowAll,
            _ => Answer::Deny,
        }
    }
    /// The preview `question` showed, under `title`.
    fn approve_publication(&mut self, title: &str) -> bool {
        let text = self.asking(title);
        let options = [
            ("approve", "Approve publication", "allow_once"),
            ("cancel", "Cancel", "reject_once"),
        ];
        self.permission(&text, &options, &self.cancel.clone()) == Some(0)
    }
    /// What a question asks, which the next prompt answers.
    fn question(&mut self, message: &str) {
        self.asked = Some(message.to_owned());
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
    /// A form with one of `options`, each by its value, the recommended one the default.
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        if !self.forms {
            return self.unanswered(title, options);
        }
        let one_of: Vec<Value> = (options.iter())
            .map(|option| {
                let mut one = json!({"const": option.value, "title": option.label});
                if let Some(description) = &option.description {
                    one["description"] = json!(description);
                }
                one
            })
            .collect();
        let mut property = json!({"type": "string", "title": title, "oneOf": one_of});
        if let Some(option) = recommended.and_then(|index| options.get(index)) {
            property["default"] = json!(option.value);
        }
        let answer = self.elicit(title, property)?;
        let chosen = (options.iter()).position(|option| answer == option.value.as_str());
        if chosen.is_none() {
            self.refused(&answer);
        }
        chosen
    }
    /// A form with a boolean, `true` the first of `options`.
    fn confirm(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        if !self.forms {
            return self.unanswered(title, options);
        }
        let mut property = json!({"type": "boolean", "title": title});
        if let Some(recommended) = recommended {
            property["default"] = json!(recommended == 0);
        }
        match self.elicit(title, property)? {
            Value::Bool(yes) => Some(if yes { 0 } else { 1 }),
            answer => {
                self.refused(&answer);
                None
            }
        }
    }
    /// A permission request: the first option allows once, the last denies, any between
    /// allows for longer.
    fn permit(&mut self, title: &str, options: &[Choice]) -> Option<usize> {
        let text = self.asking(title);
        let last = options.len().saturating_sub(1);
        let kinds: Vec<&str> = (0..options.len())
            .map(|index| match index {
                _ if index == last => "reject_once",
                0 => "allow_once",
                _ => "allow_always",
            })
            .collect();
        let ids: Vec<String> = (kinds.iter().enumerate())
            .map(|(index, kind)| match kinds[..index].contains(kind) {
                true => format!("{kind}-{index}"),
                false => kind.to_string(),
            })
            .collect();
        let offered: Vec<(&str, &str, &str)> = (ids.iter().zip(options).zip(&kinds))
            .map(|((id, option), kind)| (id.as_str(), option.label.as_str(), *kind))
            .collect();
        self.permission(&text, &offered, &self.cancel.clone())
    }
    fn offers_choices(&mut self) -> bool {
        self.forms
    }
    /// A config option set during the turn, which its pause takes (`Acp::paused`).
    fn pause(&mut self) -> bool {
        !lock(&self.turn).configuring.is_empty()
    }
}
