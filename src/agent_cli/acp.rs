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
    Ending, McpServer, McpTransport, ModelOption, Navigated, Purpose, Reopened, SessionChoice,
    Shell, StreamProgress, Tokens, ToolDetail, TreeEntry, TreeItem, UserCommand,
};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::{HashMap, VecDeque},
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

mod sign_in;

const PROTOCOL_VERSION: u64 = 1;
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const NEXT_STEP: &str = "_io.github.leanandmean.scramjet/next_step";
const HOLD: &str = "_io.github.leanandmean.scramjet/hold";
const COMPACT: &str = "_io.github.leanandmean.scramjet/compact";
const RENAME: &str = "_io.github.leanandmean.scramjet/rename";
const EXPORT: &str = "_io.github.leanandmean.scramjet/export_html";
const SPENT: &str = "_io.github.leanandmean.scramjet/usage";
const FORK: &str = "_io.github.leanandmean.scramjet/fork";
const BRANCH: &str = "_io.github.leanandmean.scramjet/branch";
const TREE: &str = "_io.github.leanandmean.scramjet/tree";
const NAVIGATE: &str = "_io.github.leanandmean.scramjet/navigate_tree";
const LABEL: &str = "_io.github.leanandmean.scramjet/set_label";
const RELOAD: &str = "_io.github.leanandmean.scramjet/reload";
const BASH: &str = "_io.github.leanandmean.scramjet/bash";
const ABORT_BASH: &str = "_io.github.leanandmean.scramjet/abort_bash";
const USER_COMMAND: &str = "_io.github.leanandmean.scramjet/user_command";
const STATUS: &str = "_io.github.leanandmean.scramjet/status";
const QUESTION: &str = "_io.github.leanandmean.scramjet/question";
const ENTRY: &str = "_io.github.leanandmean.scramjet/entry";
const QUEUED: &str = "_io.github.leanandmean.scramjet/queued";
const SET_API_KEY: &str = "_io.github.leanandmean.scramjet/set_api_key";
const STATE: &str = "_io.github.leanandmean.scramjet/state";
const EVENTS: &str = "_io.github.leanandmean.scramjet/events";
/// The extension methods the client may call, as `initialize` lists them.
const EXTENSIONS: [&str; 16] = [
    HOLD,
    COMPACT,
    RENAME,
    EXPORT,
    SPENT,
    FORK,
    BRANCH,
    TREE,
    NAVIGATE,
    LABEL,
    RELOAD,
    BASH,
    ABORT_BASH,
    SET_API_KEY,
    STATE,
    EVENTS,
];
/// JSON-RPC's error for a request the client cancelled, as ACP's `$/cancel_request` answers.
const REQUEST_CANCELLED: i64 = -32800;
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
        sign_in: sign_in::SignIn::default(),
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
        self.write(&Self::line(message));
    }
    fn line(message: &Value) -> String {
        // Inside JSON these appear only in strings, where escaping them is the same text; some
        // readers split lines at them.
        (message.to_string())
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029")
    }
    fn write(&self, line: &str) {
        let _held = lock(&self.written);
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}").and_then(|()| stdout.flush());
    }
    fn respond(&self, id: &Value, result: Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
    fn error(&self, id: &Value, code: i64, message: &str) {
        // Again in `data`, where clients such as Toad 0.6.20 look for what to show.
        let error = json!({"code": code, "message": message, "data": {"details": message}});
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": error}));
    }
    /// ACP's error for a session not known, naming it in `data` as ACP's error page asks.
    fn unknown(&self, id: &Value, session: &str, message: &str) {
        let data = json!({"details": message, "sessionId": session});
        let error = json!({"code": UNKNOWN_SESSION, "message": message, "data": data});
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": error}));
    }
    /// Sends request `method`; its response comes through the receiver, unless `forget` drops
    /// it first.
    fn request(&self, method: &str, params: Value) -> (Value, mpsc::Receiver<Value>) {
        let (id, answered) = self.register();
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        (id, answered)
    }
    /// A new request's id, and where its response will come.
    fn register(&self) -> (Value, mpsc::Receiver<Value>) {
        let id = json!(self.ids.fetch_add(1, Ordering::Relaxed) + 1);
        let (sent, answered) = mpsc::channel();
        lock(&self.pending).insert(id.to_string(), sent);
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
}

/// How much of what a session sent is held for `events`.
const HELD: usize = 8 << 20;

/// What a session sent the client since its turn began, numbered for a client that comes back
/// (`state`, `events`), and what `state` says of it.
struct Log {
    /// The client knows Scramjet's extensions, so each message carries its `seq`.
    numbered: bool,
    /// The last message's number.
    seq: u64,
    /// The messages held, each with its number and size on the wire, at most `limit` bytes.
    held: VecDeque<(u64, Value, usize)>,
    bytes: usize,
    limit: usize,
    /// The request waiting for the client's answer, as it was sent.
    open: Option<Value>,
    /// What a question asked that nothing asked the client since: the next prompt answers it.
    question: Option<String>,
    /// The config options and usage the client was last given.
    options: Value,
    usage: Value,
    /// `--ask`, and the sandbox's mode.
    asks: bool,
    sandbox: &'static str,
}

type Shared = Arc<Mutex<Log>>;

impl Log {
    fn new(numbered: bool) -> Self {
        Self {
            numbered,
            seq: 0,
            held: VecDeque::new(),
            bytes: 0,
            limit: HELD,
            open: None,
            question: None,
            options: Value::Null,
            usage: Value::Null,
            asks: false,
            sandbox: "danger-full-access",
        }
    }
    fn number(&mut self, message: &mut Value) {
        self.seq += 1;
        if self.numbered {
            add_meta(&mut message["params"], json!({"seq": self.seq}));
        }
    }
    fn hold(&mut self, out: &Out, message: Value) {
        let line = Out::line(&message);
        // Written while held, so that the numbers go out in order.
        out.write(&line);
        self.bytes += line.len();
        self.held.push_back((self.seq, message, line.len()));
        while self.bytes > self.limit
            && let Some((.., size)) = self.held.pop_front()
        {
            self.bytes -= size;
        }
    }
    fn send(&mut self, out: &Out, mut message: Value) {
        self.number(&mut message);
        self.hold(out, message);
    }
    fn update(&mut self, out: &Out, session: &str, update: Value) {
        let params = json!({"sessionId": session, "update": update});
        self.send(
            out,
            json!({"jsonrpc": "2.0", "method": "session/update", "params": params}),
        );
    }
    fn notify(&mut self, out: &Out, method: &str, params: Value) {
        self.send(
            out,
            json!({"jsonrpc": "2.0", "method": method, "params": params}),
        );
    }
    /// Sends request `method`, open until `settled` or `withdraw`.
    fn request(
        &mut self,
        out: &Out,
        method: &str,
        params: Value,
    ) -> (Value, mpsc::Receiver<Value>) {
        let (id, answered) = out.register();
        let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.number(&mut message);
        self.open = Some(message.clone());
        self.hold(out, message);
        (id, answered)
    }
    fn settled(&mut self) {
        self.open = None;
    }
    /// Withdraws request `id`, whose answer is no longer waited for.
    fn withdraw(&mut self, out: &Out, id: &Value) {
        out.forget(id);
        self.open = None;
        self.notify(out, "$/cancel_request", json!({"requestId": id}));
    }
    /// A turn begins: what was held before it goes.
    fn turn_starts(&mut self) {
        self.held.clear();
        self.bytes = 0;
        self.question = None;
    }
    /// `events`' result: the messages after `since`, and whether none of them was dropped.
    fn events(&self, since: u64) -> Value {
        let earliest = self.held.front().map_or(self.seq + 1, |(seq, ..)| *seq);
        let events: Vec<&Value> = (self.held.iter())
            .filter(|(seq, ..)| *seq > since)
            .map(|(_, message, _)| message)
            .collect();
        json!({"events": events, "complete": since + 1 >= earliest, "seq": self.seq})
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
    sign_in: sign_in::SignIn,
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
/// ACP's error for a request that needs a sign-in first, which `authMethods` offer.
const AUTH_REQUIRED: i64 = -32000;

/// `AUTH_REQUIRED`'s message, saying why.
fn auth_required(why: &str) -> String {
    format!("authentication required: {why}")
}

/// What this thread holds of a session that runs on its own.
struct Handle {
    jobs: mpsc::Sender<Job>,
    cancel: Arc<AtomicBool>,
    turn: Arc<Mutex<Turn>>,
    thread: JoinHandle<()>,
    /// The project, where `bash` runs its commands.
    root: PathBuf,
    log: Shared,
}

/// A prompt sent during a turn, as text, with the id of its request.
struct Queued {
    text: String,
    id: Value,
}

/// The texts of `queued`.
fn texts(queued: Vec<Queued>) -> Vec<String> {
    queued.into_iter().map(|queued| queued.text).collect()
}

/// A session's turn, as both threads see it.
#[derive(Default)]
struct Turn {
    running: bool,
    /// Prompts sent while it runs, for the agent to take: `messages.steering` before its next
    /// request, `messages.follow_ups` when it would end.
    messages: Messages<Queued>,
    /// The requests of those prompts, answered when the turn ends.
    waiting: Vec<Value>,
    /// The client held the countdown of the next-step list open (`hold`).
    held: bool,
    /// Config options and names the client set while it runs, which the session takes at its
    /// next pause.
    configuring: Vec<Configure>,
    /// What stops the `bash` command running, if one runs.
    bash: Option<Arc<AtomicBool>>,
    /// The `bash` commands that ended while it ran, added to the conversation when it ends.
    shells: Vec<Shell>,
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
    /// Request `id`'s compaction, with its instructions.
    Compact(Value, String),
    /// Request `id`'s fork before the prompt at that line of the session file.
    Fork(Value, usize),
    /// Request `id`'s branch, with the name it is given, if any.
    Branch(Value, String),
    /// Request `id`'s move to the entry on that line, with the branch left summarized when
    /// a focus is given (empty for none).
    Navigate(Value, usize, Option<String>),
    /// `reload`, done when the sender is dropped.
    Reload(mpsc::Sender<()>),
    /// A `bash` command that ended between turns, to add to the conversation.
    Shell(Shell),
    /// A login or an API key saved or removed for that provider: its models are read again.
    Credentials(String),
    /// Request `id`'s `session/load` (true) or `resume` of the session open already.
    Reopen(Value, bool),
}

/// A `session/set_config_option` or `rename` request, answered once the session takes it.
struct Configure {
    id: Value,
    setting: Setting,
}

enum Setting {
    Option {
        option: String,
        value: Value,
    },
    Rename(String),
    /// `export_html`'s path, if given.
    Export(Option<String>),
    Tree,
    /// `set_label`'s entry and its label, `None` to take it away.
    Label(usize, Option<String>),
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
                false => self.unknown(&id, &params),
            },
            COMPACT => self.compact(id, &params),
            RENAME => self.rename(&id, &params),
            EXPORT => self.export(&id, &params),
            SPENT => self.usage(id),
            FORK => self.fork(&id, &params),
            BRANCH => self.branch(&id, &params),
            TREE => match self.handle(&params) {
                Some(handle) => Self::settle(handle, &id, Setting::Tree),
                None => self.unknown(&id, &params),
            },
            NAVIGATE => self.navigate(id, &params),
            LABEL => self.label(&id, &params),
            RELOAD => self.reload(id),
            SET_API_KEY => self.set_api_key(id, &params),
            "authenticate" => (self.sign_in).authenticate(&self.out, id, &params, self.jobs()),
            "logout" => (self.sign_in).logout(&self.out, &id, &params, self.jobs()),
            BASH => self.bash(id, &params),
            STATE => self.state(&id, &params),
            EVENTS => self.events(&id, &params),
            ABORT_BASH => match self.abort_bash(&params) {
                true => self.out.respond(&id, json!({})),
                false => self.unknown(&id, &params),
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
        } else if method == ABORT_BASH {
            self.abort_bash(params);
        } else if method == "$/cancel_request" {
            self.sign_in.cancel(&params["requestId"]);
        }
    }

    /// Where each session is sent its jobs.
    fn jobs(&self) -> Vec<mpsc::Sender<Job>> {
        self.sessions.values().map(|h| h.jobs.clone()).collect()
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
            return self.unknown(id, params);
        };
        let Some(option) = params["configId"].as_str() else {
            return self.out.error(id, INVALID_PARAMS, "configId is required");
        };
        let setting = Setting::Option {
            option: option.to_owned(),
            value: params["value"].clone(),
        };
        Self::settle(handle, id, setting);
    }

    /// Names the session, as `/rename` does, at once between turns, else at the turn's next
    /// pause.
    fn rename(&self, id: &Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        let name = params["name"].as_str().unwrap_or_default().trim();
        if name.is_empty() {
            return self.out.error(id, INVALID_PARAMS, "name is required");
        }
        Self::settle(handle, id, Setting::Rename(name.to_owned()));
    }

    /// Writes the session's current branch as `/export` does, at once between turns, else at the
    /// turn's next pause.
    fn export(&self, id: &Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        let path = params["path"].as_str().filter(|path| !path.is_empty());
        Self::settle(handle, id, Setting::Export(path.map(str::to_owned)));
    }

    /// Answers with `/usage`'s summary as data, from a thread of its own, as it reads every saved
    /// session.
    fn usage(&self, id: Value) {
        let out = self.out.clone();
        std::thread::spawn(move || {
            let sessions = match scramjet::agent::state_directory() {
                Ok(state) => state.join("sessions"),
                Err(error) => return out.error(&id, INTERNAL_ERROR, &error.to_string()),
            };
            let history = scramjet::agent::usage::History::read(&sessions, Path::new(""));
            let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
                .map_or(0, |since| since.as_millis() as u64);
            out.respond(&id, super::usage::data(&history, now));
        });
    }

    /// Hands `setting` to the session, which takes it at once between turns, else at the turn's
    /// next pause, and answers request `id` then.
    fn settle(handle: &Handle, id: &Value, setting: Setting) {
        let configure = Configure {
            id: id.clone(),
            setting,
        };
        let mut turn = lock(&handle.turn);
        if turn.running {
            return turn.configuring.push(configure);
        }
        drop(turn);
        let _ = handle.jobs.send(Job::Configure(configure));
    }

    /// Compacts the session's conversation, as `/compact` does; refused during a turn, as the
    /// full screen refuses it. Prompts sent meanwhile wait for it, then run as a turn.
    fn compact(&self, id: Value, params: &Value) {
        let instructions = params["instructions"].as_str().unwrap_or_default();
        let job = Job::Compact(id.clone(), instructions.to_owned());
        self.busy(&id, params, job);
    }

    /// Moves the session's conversation to the entry `entryId`, as `/tree` does; with
    /// `summarize` (`true`, or the focus as a string) the branch left is summarized first.
    /// Refused during a turn; prompts sent meanwhile wait for it.
    fn navigate(&self, id: Value, params: &Value) {
        let Some(line) = params["entryId"].as_u64() else {
            return self.out.error(&id, INVALID_PARAMS, "entryId is required");
        };
        let focus = match &params["summarize"] {
            Value::Bool(true) => Some(String::new()),
            Value::String(focus) => Some(focus.trim().to_owned()),
            _ => None,
        };
        self.busy(&id, params, Job::Navigate(id.clone(), line as usize, focus));
    }

    /// Labels the entry `entryId`, or takes its label away (no `label`, or an empty one), at
    /// the turn's next pause.
    fn label(&self, id: &Value, params: &Value) {
        let Some(line) = params["entryId"].as_u64() else {
            return self.out.error(id, INVALID_PARAMS, "entryId is required");
        };
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        let label = (params["label"].as_str().map(str::trim))
            .filter(|label| !label.is_empty())
            .map(str::to_owned);
        Self::settle(handle, id, Setting::Label(line as usize, label));
    }

    /// Hands `job` to the session `params` names as its turn, unless one runs there:
    /// `session/cancel` stops it, and prompts sent meanwhile wait for it.
    fn busy(&self, id: &Value, params: &Value, job: Job) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        let mut turn = lock(&handle.turn);
        if turn.running {
            drop(turn);
            return self.out.error(id, INVALID_PARAMS, "a turn is running");
        }
        turn.running = true;
        lock(&handle.log).turn_starts();
        handle.cancel.store(false, Ordering::Relaxed);
        drop(turn);
        let _ = handle.jobs.send(job);
    }

    /// Continues in a new session before the prompt at `entryId`, as `/fork` does; refused
    /// during a turn, as the full screen refuses it.
    fn fork(&self, id: &Value, params: &Value) {
        let Some(line) = params["entryId"].as_u64() else {
            return self.out.error(id, INVALID_PARAMS, "entryId is required");
        };
        self.busy(id, params, Job::Fork(id.clone(), line as usize));
    }

    /// Continues in a new session with the whole conversation, as `/branch` does; refused during
    /// a turn.
    fn branch(&self, id: &Value, params: &Value) {
        let name = params["name"].as_str().unwrap_or_default().trim();
        self.between_turns(id, params, Job::Branch(id.clone(), name.to_owned()));
    }

    /// Reads the settings and what they name again in every session, as `/reload` does, and
    /// answers once all have; refused while a turn runs in any, as the full screen refuses it.
    fn reload(&self, id: Value) {
        let busy = |handle: &Handle| lock(&handle.turn).running;
        if self.sessions.values().any(busy) {
            return self.out.error(&id, INVALID_PARAMS, "a turn is running");
        }
        let (done, finished) = mpsc::channel();
        for handle in self.sessions.values() {
            let _ = handle.jobs.send(Job::Reload(done.clone()));
        }
        drop(done);
        let out = self.out.clone();
        std::thread::spawn(move || {
            while finished.recv().is_ok() {}
            out.respond(&id, json!({}));
        });
    }

    /// Saves the API key `params` gives in `auth.json` for its provider, one `/login` takes a key
    /// for, as `/login`'s key entry does; every session then reads its providers again, after
    /// its turn if one runs. The key is never echoed.
    fn set_api_key(&self, id: Value, params: &Value) {
        let (Some(provider), Some(key)) = (params["provider"].as_str(), params["key"].as_str())
        else {
            return self
                .out
                .error(&id, INVALID_PARAMS, "provider and key are required");
        };
        let (provider, key) = (provider.to_owned(), key.to_owned());
        let sessions = self.jobs();
        let out = self.out.clone();
        // Reading the providers may run their programs.
        std::thread::spawn(move || {
            let Some(logins) = scramjet::agent::login::Logins::user() else {
                let message = "neither XDG_CONFIG_HOME nor HOME is set";
                return out.error(&id, INTERNAL_ERROR, message);
            };
            let providers = scramjet::agent::Providers::read(
                Settings::load().models_files(),
                Some(logins.clone()),
            );
            if !(providers.keyed().iter()).any(|(name, _)| *name == provider) {
                let message = format!("no provider {provider:?} takes an API key");
                return out.error(&id, INVALID_PARAMS, &message);
            }
            let origins = providers.origins(&provider);
            if let Err(error) =
                scramjet::agent::login::save_key(&logins.auth, &provider, &key, origins)
            {
                let message = format!("the API key for {provider} was not saved: {error}");
                return out.error(&id, INVALID_PARAMS, &message);
            }
            for jobs in sessions {
                let _ = jobs.send(Job::Credentials(provider.clone()));
            }
            out.respond(&id, json!({}));
        });
    }

    /// Runs `command` in the session's project as `!` does, or `!!` with `exclude`, alongside
    /// its turn if one runs: its output goes into the conversation at once between turns, else
    /// when the turn ends, as the full screen adds a `!` typed during a turn. Answered when it
    /// ends, with its output, exit code and whether it was aborted; one at a time per session.
    fn bash(&self, id: Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(&id, params);
        };
        let command = params["command"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        if command.is_empty() {
            return self.out.error(&id, INVALID_PARAMS, "command is required");
        }
        let include = params["exclude"] != true;
        let cancel = Arc::<AtomicBool>::default();
        let mut turn = lock(&handle.turn);
        if turn.bash.is_some() {
            drop(turn);
            return self.out.error(&id, INVALID_PARAMS, "a command is running");
        }
        turn.bash = Some(cancel.clone());
        drop(turn);
        let (out, turn, jobs) = (self.out.clone(), handle.turn.clone(), handle.jobs.clone());
        let (root, scramjet, session) = (handle.root.clone(), self.scramjet, &params["sessionId"]);
        let log = handle.log.clone();
        let mut said = json!({"sessionId": session, "command": command, "include": include});
        std::thread::spawn(move || {
            let tell = |params: &Value| {
                if scramjet {
                    lock(&log).notify(&out, USER_COMMAND, params.clone());
                }
            };
            tell(&said);
            let shell = Shell::run(&root, &command, include, &cancel, &mut |_| {});
            let result = ran(&shell.shown());
            let mut held = lock(&turn);
            held.bash = None;
            match held.running {
                true => held.shells.push(shell),
                false => {
                    let _ = jobs.send(Job::Shell(shell));
                }
            }
            drop(held);
            said["result"] = result.clone();
            tell(&said);
            out.respond(&id, result);
        });
    }

    /// Stops the `bash` command running in the session `params` names, if one runs; whether
    /// there is that session.
    fn abort_bash(&self, params: &Value) -> bool {
        let Some(handle) = self.handle(params) else {
            return false;
        };
        if let Some(cancel) = &lock(&handle.turn).bash {
            cancel.store(true, Ordering::Relaxed);
        }
        true
    }

    /// What the session shows now, for a client that comes back.
    fn state(&self, id: &Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        let turn = lock(&handle.turn);
        let log = lock(&handle.log);
        let question = log.question.as_ref().filter(|_| !turn.running);
        let result = json!({
            "running": turn.running,
            "request": log.open,
            "question": question,
            "queue": {
                "steering": turn.messages.steering.iter().map(|queued| &queued.text).collect::<Vec<_>>(),
                "followUp": turn.messages.follow_ups.iter().map(|queued| &queued.text).collect::<Vec<_>>(),
            },
            "bash": turn.bash.is_some(),
            "configOptions": log.options,
            "ask": log.asks,
            "sandbox": log.sandbox,
            "usage": log.usage,
            "seq": log.seq,
        });
        drop((log, turn));
        self.out.respond(id, result);
    }

    /// The messages the session sent after `since` that are held, since its turn began.
    fn events(&self, id: &Value, params: &Value) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        let Some(since) = params["since"].as_u64() else {
            return self.out.error(id, INVALID_PARAMS, "since is required");
        };
        let result = lock(&handle.log).events(since);
        self.out.respond(id, result);
    }

    /// Hands `job` to the session `params` names unless a turn runs there.
    fn between_turns(&self, id: &Value, params: &Value, job: Job) {
        let Some(handle) = self.handle(params) else {
            return self.unknown(id, params);
        };
        if lock(&handle.turn).running {
            return self.out.error(id, INVALID_PARAMS, "a turn is running");
        }
        let _ = handle.jobs.send(job);
    }

    /// The error for the session `params` names, which this connection does not have open.
    fn unknown(&self, id: &Value, params: &Value) {
        let session = params["sessionId"].as_str().unwrap_or_default();
        self.out.unknown(id, session, "no such session");
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
        self.sign_in = sign_in::SignIn::read(capabilities["elicitation"]["url"].is_object());
        let result = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"list": {}, "resume": {}, "close": {}, "delete": {}},
                "promptCapabilities": {"image": true, "audio": false, "embeddedContext": true},
                "mcpCapabilities": {"http": true, "sse": false},
                "auth": {"logout": {}},
            },
            "agentInfo": {
                "name": "scramjet",
                "title": "Scramjet",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "authMethods": self.sign_in.advertised(),
            "_meta": {"io.github.leanandmean.scramjet": {"version": 1, "methods": EXTENSIONS}},
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
        if opening != Opening::New
            && let Some(handle) = self.handle(params)
        {
            return self.reopen(id, handle, &cwd, opening == Opening::Load);
        }
        let file = match opening {
            Opening::New => None,
            Opening::Resume | Opening::Load => match self.resumable(params, &cwd) {
                Ok(file) => Some(file),
                Err((UNKNOWN_SESSION, message)) => {
                    let session = params["sessionId"].as_str().unwrap_or_default();
                    return self.out.unknown(id, session, &message);
                }
                Err((code, message)) => return self.out.error(id, code, &message),
            },
        };
        let (started, starts) = mpsc::channel::<Result<(String, PathBuf), (i64, String)>>();
        let (jobs, given) = mpsc::channel();
        let (cancel, turn) = (Arc::<AtomicBool>::default(), Arc::<Mutex<Turn>>::default());
        let log = Arc::new(Mutex::new(Log::new(self.scramjet)));
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
            log: log.clone(),
        };
        let (arguments, out, answered) = (self.arguments.clone(), self.out.clone(), id.clone());
        let thread = std::thread::spawn(move || {
            let session = match start_session(&arguments, &cwd, &servers, file) {
                Ok(session) => session,
                Err(error) => {
                    let _ = started.send(Err((INTERNAL_ERROR, error)));
                    return;
                }
            };
            if opening == Opening::New
                && let Some(why) = session.agent.login_needed()
            {
                let _ = started.send(Err((AUTH_REQUIRED, auth_required(&why))));
                return;
            }
            let mut front = worker.front(session, servers);
            if opening == Opening::Load {
                front.replay();
            }
            let shown = front.options();
            front.show(shown);
            // Answered here, so that the commands' update follows it.
            let result = json!({"sessionId": front.ui.session, "configOptions": front.shown});
            out.respond(&answered, result);
            let root = front.session.agent.root().to_path_buf();
            let _ = started.send(Ok((front.ui.session.clone(), root)));
            front.commands();
            for job in given {
                let (id, Prompt { text, images }) = match job {
                    Job::Prompt(id, prompt) => (id, prompt),
                    Job::Configure(configure) => {
                        front.configure_with(configure);
                        continue;
                    }
                    Job::Compact(id, instructions) => {
                        front.compact(&id, &instructions);
                        continue;
                    }
                    Job::Fork(id, line) => {
                        front.fork(&id, line);
                        continue;
                    }
                    Job::Branch(id, name) => {
                        front.branch(&id, &name);
                        continue;
                    }
                    Job::Navigate(id, line, focus) => {
                        front.navigate(id, line, focus.as_deref());
                        continue;
                    }
                    Job::Reload(_done) => {
                        front.reload();
                        continue;
                    }
                    Job::Shell(shell) => {
                        front.add_shells(vec![shell]);
                        continue;
                    }
                    Job::Credentials(provider) => {
                        front.credentials_changed(&provider);
                        continue;
                    }
                    Job::Reopen(id, load) => {
                        if load {
                            front.replay();
                        }
                        let shown = front.options();
                        front.show(shown);
                        let result =
                            json!({"sessionId": front.ui.session, "configOptions": front.shown});
                        out.respond(&id, result);
                        continue;
                    }
                };
                if let Some(why) = front.session.agent.login_needed() {
                    front.end_turn((&id, json!({})), Err((AUTH_REQUIRED, &auth_required(&why))));
                    continue;
                }
                if !images.is_empty() {
                    match Attached::images(&images) {
                        Ok(attached) => front.session.agent.attach(attached),
                        Err(error) => {
                            front.end_turn((&id, json!({})), Err((INVALID_PARAMS, &error)));
                            continue;
                        }
                    }
                }
                front.run((id, json!({})), Some(text), Start::Typed);
            }
        });
        match starts.recv() {
            Ok(Ok((session, root))) => {
                let handle = Handle {
                    jobs,
                    cancel,
                    turn,
                    thread,
                    root,
                    log,
                };
                self.sessions.insert(session, handle);
            }
            Ok(Err((code, error))) => self.out.error(id, code, &error),
            Err(_) => self
                .out
                .error(id, INTERNAL_ERROR, "the session failed to start"),
        }
    }

    /// `session/load` or `resume` of a session open on this connection: between turns it goes on
    /// as it is, its branch shown again when loaded; its MCP servers stay those it has.
    fn reopen(&self, id: &Value, handle: &Handle, cwd: &Path, load: bool) {
        let root = super::project_root(Some(cwd.to_owned())).unwrap_or_else(|_| cwd.to_owned());
        if root != handle.root {
            let message = format!(
                "the session is in {}, not {}",
                handle.root.display(),
                root.display()
            );
            return self.out.error(id, INVALID_PARAMS, &message);
        }
        if lock(&handle.turn).running {
            return self.out.error(id, INVALID_PARAMS, "a turn is running");
        }
        let _ = handle.jobs.send(Job::Reopen(id.clone(), load));
    }

    /// The file of the saved session `params` names, to resume in `cwd`: one of its project's
    /// sessions with a conversation, not open on this connection.
    fn resumable(&self, params: &Value, cwd: &Path) -> Result<PathBuf, (i64, String)> {
        let invalid = |message: String| (INVALID_PARAMS, message);
        let wanted = params["sessionId"].as_str().unwrap_or_default();
        // An id names a file, so it never holds a path.
        let valid = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if wanted.is_empty() || !wanted.chars().all(valid) {
            return Err((UNKNOWN_SESSION, format!("no session {wanted:?}")));
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
            if let Some(bash) = &turn.bash {
                bash.store(true, Ordering::Relaxed);
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
            return self.unknown(&id, params);
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
                        .to_owned()
                }
                Builtin::Compact(_) => format!("; {COMPACT} compacts"),
                Builtin::Rename(_) => format!("; {RENAME} renames"),
                Builtin::Export(_) => format!("; {EXPORT} exports"),
                Builtin::Usage(_) => format!("; {SPENT} gives the usage"),
                Builtin::Fork(_) => format!("; {FORK} forks"),
                Builtin::Branch(_) => format!("; {BRANCH} branches"),
                Builtin::Tree(_) => format!("; {TREE} lists the entries, {NAVIGATE} moves"),
                _ => String::new(),
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
            // Told under the turn's lock, so before the agent can take it.
            if self.scramjet {
                let queue = if follow_up { "followUp" } else { "steering" };
                let told = json!({"sessionId": params["sessionId"], "promptId": id,
                    "queue": queue});
                lock(&handle.log).notify(&self.out, QUEUED, told);
            }
            turn.messages.push(
                Queued {
                    text,
                    id: id.clone(),
                },
                follow_up,
            );
            turn.waiting.push(id);
            return;
        }
        turn.running = true;
        lock(&handle.log).turn_starts();
        handle.cancel.store(false, Ordering::Relaxed);
        drop(turn);
        let _ = handle.jobs.send(Job::Prompt(id, prompt));
    }

    /// At the end of input or SIGTERM: every turn cancelled, and each session's thread waited for, which
    /// writes its session file as it goes.
    fn end(self) {
        for handle in self.sessions.values() {
            handle.cancel.store(true, Ordering::Relaxed);
            if let Some(bash) = &lock(&handle.turn).bash {
                bash.store(true, Ordering::Relaxed);
            }
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

/// `bash`'s result for the command `shown`, as Scramjet 1's RPC gives it: what it printed, its
/// exit code (null when it did not exit), whether it was aborted, and why it failed otherwise.
fn ran(shown: &UserCommand) -> Value {
    let mut result = json!({"output": shown.output, "exitCode": null, "cancelled": false});
    match &shown.ending {
        Ending::Done => result["exitCode"] = json!(0),
        Ending::Exited(code) => result["exitCode"] = json!(code),
        Ending::Cancelled => result["cancelled"] = json!(true),
        Ending::Failed(why) => result["error"] = json!(why),
    }
    result
}

/// The JSON-RPC error for `error`: the client's when it named no entry of the session.
fn code(error: &AgentError) -> i64 {
    match error {
        AgentError::Session(error) if error.kind() == std::io::ErrorKind::InvalidInput => {
            INVALID_PARAMS
        }
        _ => INTERNAL_ERROR,
    }
}

/// An entry of the session tree as `tree` gives it: `entryId` its line in the session file, which
/// `fork` takes, `parentId` the listed entry it continues, `kind` and what that kind shows.
fn tree_entry(entry: &TreeEntry) -> Value {
    let mut listed = match &entry.item {
        TreeItem::Prompt(text) => json!({"kind": "prompt", "text": text}),
        TreeItem::Command(text) => json!({"kind": "command", "text": text}),
        TreeItem::Reply(text) => json!({"kind": "reply", "text": text}),
        TreeItem::Stopped(error) => json!({"kind": "stopped", "error": error}),
        TreeItem::Tool(name, summary) => {
            json!({"kind": "toolResult", "name": name, "summary": summary})
        }
        TreeItem::Call(name, summary) => {
            json!({"kind": "toolCall", "name": name, "summary": summary})
        }
        TreeItem::Compaction(tokens) => json!({"kind": "compaction", "tokensBefore": tokens}),
        TreeItem::BranchSummary(text) => json!({"kind": "branchSummary", "text": text}),
        TreeItem::Reasoning => json!({"kind": "reasoning"}),
        TreeItem::Model(model) => json!({"kind": "model", "model": model}),
        TreeItem::Effort(level) => json!({"kind": "thoughtLevel", "level": level}),
    };
    listed["entryId"] = json!(entry.line);
    listed["parentId"] = json!(entry.parent);
    listed["active"] = json!(entry.active);
    if let Some(label) = &entry.label {
        listed["label"] = json!(label.text);
    }
    listed
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
    // A model with no levels keeps the option, with the model's own only: a client keeps every
    // option a session showed (ACP's "complete" state, as ACP's TCK reads it).
    let levels = agent.effort_levels();
    let (effort, mut values) = match agent.reasons() && !levels.is_empty() {
        true => (
            agent.effort().unwrap_or(DEFAULT_LEVEL),
            levels.iter().map(String::as_str).collect(),
        ),
        false => (DEFAULT_LEVEL, Vec::new()),
    };
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
    servers: &[McpServer],
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
    drive::with_servers(&mut settings, servers);
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
    log: Shared,
}

impl Worker {
    fn front(self, session: Session, servers: Vec<McpServer>) -> Acp {
        let mut log = lock(&self.log);
        log.asks = session.agent.asks();
        if let Some(sandbox) = session.agent.sandbox() {
            log.sandbox = sandbox.mode.name();
        }
        drop(log);
        let ui = Updates {
            out: self.out,
            log: self.log,
            first: session.agent.session_id(),
            session: session.agent.session_id(),
            notices: self.notices,
            scramjet: self.scramjet,
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
            call: Call::default(),
            together: Vec::new(),
            of: None,
            window: None,
            used: None,
            stopped: None,
            asked: None,
            cancel: self.cancel.clone(),
            replaying: false,
            held: Cell::new(None),
            thinking: false,
            entry: None,
            call_ids: HashMap::new(),
            prompt_shown: None,
            taken_ids: Default::default(),
            title: None,
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
            servers,
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
    /// The MCP servers the client gave the session.
    servers: Vec<McpServer>,
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
        self.session.agent.show_conversation_entries(&mut self.ui);
        self.ui.flush();
        (self.ui.replaying, self.ui.entry) = (false, None);
        (self.ui.asked, self.ui.stopped, self.ui.thinking) = (None, None, false);
        lock(&self.ui.log).question = None;
    }

    /// `reload` in this session: the settings and what they name read again, what is wrong in
    /// them said, and the commands and options listed again.
    fn reload(&mut self) {
        self.session.reload(&self.servers);
        let settings = &self.session.settings;
        (self.ui.steering, self.ui.follow_up) = (settings.steering_mode, settings.follow_up_mode);
        let installed = self.session.agent.installed();
        let problems = (settings.warnings.iter())
            .map(|warning| format!("settings: {warning}"))
            .chain(
                installed
                    .problems
                    .iter()
                    .map(|problem| problem.text.clone()),
            )
            .chain(self.session.agent.providers().problems().iter().cloned());
        for problem in problems.collect::<Vec<_>>() {
            self.ui.say("warning", &problem);
        }
        self.commands();
        self.models.clear();
        self.options_changed();
    }

    /// Reads the providers again after a login or an API key for `provider` was saved or
    /// removed, as `/login` and `/logout` do.
    fn credentials_changed(&mut self, provider: &str) {
        self.session.agent.forget_list(provider);
        self.session.agent.load_model_info();
        self.models.clear();
        self.options_changed();
    }

    /// Adds the `bash` commands that ended to the conversation, for the model (`!`) or only to
    /// be shown again (`!!`).
    fn add_shells(&mut self, shells: Vec<Shell>) {
        for shell in shells {
            if let Err(error) = self.session.agent.add_shell(shell, &mut self.ui) {
                self.ui.say("warning", &format!("Error: {error}"));
            }
        }
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
        self.ui.update(update);
    }

    /// Runs `prompt`, or with none goes on with the turn the session holds, and the next steps
    /// that follow on their own, then the prompts sent meanwhile that the agent did not take;
    /// answers request `id`, with `result`'s fields beside the stop reason, and theirs when it
    /// ends.
    fn run(&mut self, (id, result): (Value, Value), prompt: Option<String>, start: Start) {
        self.configure();
        let (mut prompt, mut start) = (prompt, start);
        let stopped = loop {
            self.ui.title = prompt.as_deref().and_then(|prompt| self.titles(prompt));
            let next = drive::run_turn(self, prompt.as_deref(), start);
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
                (prompt, start) = (Some(step), how);
                continue;
            }
            let mut turn = lock(&self.turn);
            let queued = turn.messages.take();
            if queued.is_empty() {
                break Ended::Reason("end_turn");
            }
            drop(turn);
            let ids = queued.iter().map(|queued| queued.id.clone()).collect();
            let text = texts(queued).join("\n\n");
            self.ui.taken(&text, ids);
            (prompt, start) = (Some(text), Start::Typed);
        };
        match stopped {
            Ended::Reason(reason) => self.end_turn((&id, result), Ok(reason)),
            Ended::Failed(error) => {
                // Signed out since the turn began, or the login's renewal was refused.
                let code = match self.session.agent.login_needed() {
                    Some(_) => AUTH_REQUIRED,
                    None => INTERNAL_ERROR,
                };
                self.end_turn((&id, result), Err((code, &error)));
            }
        }
    }

    /// The title `prompt` gives the session, as `session/list` shows it, if the session has no
    /// name and no prompt yet.
    fn titles(&self, prompt: &str) -> Option<String> {
        let agent = &self.session.agent;
        if agent.session_name().is_some() || !agent.fork_prompts().ok()?.is_empty() {
            return None;
        }
        let title = prompt.lines().next()?.trim();
        (!title.is_empty()).then(|| title.to_owned())
    }

    /// Takes the config options the client set during the turn.
    fn configure(&mut self) {
        let configuring = std::mem::take(&mut lock(&self.turn).configuring);
        for configure in configuring {
            self.configure_with(configure);
        }
    }

    /// Compacts the conversation for request `id`, answered with whether there was anything to
    /// compact; then runs the prompts sent meanwhile as a turn.
    fn compact(&mut self, id: &Value, instructions: &str) {
        let agent = &mut self.session.agent;
        match agent.compact(instructions, None, &mut self.ui, &self.cancel) {
            Ok(Some(compaction)) => {
                self.ui.compacted(&compaction, None);
                self.ui.out.respond(id, json!({"compacted": true}));
            }
            Ok(None) => self.ui.out.respond(id, json!({"compacted": false})),
            Err(AgentError::Cancelled) => {
                if self.ui.compacting {
                    self.ui.compaction_update("cancelled", json!({}), None);
                }
                self.ui.out.error(id, REQUEST_CANCELLED, "cancelled");
            }
            Err(error) => {
                self.ui.compaction_failed(&error);
                self.ui.out.error(id, INTERNAL_ERROR, &error.to_string());
            }
        }
        self.after_job();
    }

    /// Moves the conversation to the entry on `line` for request `id`, with the branch left
    /// summarized when `focus` is given, as `/tree` does. Answered with whether it moved and the
    /// prompt gone back to, to edit; a question gone back to is asked again, and the answer
    /// comes when its turn ends, as a prompt's. A next-step list offered there is offered again.
    fn navigate(&mut self, id: Value, line: usize, focus: Option<&str>) {
        let agent = &mut self.session.agent;
        let navigated = agent.navigate(line, focus, &mut self.ui, &self.cancel);
        let summarizing = self.ui.compacting;
        let branch = json!({"io.github.leanandmean.scramjet": {"branch": true}});
        match navigated {
            Ok(Navigated::AlreadyThere) => self.ui.out.respond(&id, json!({"moved": false})),
            Ok(Navigated::Moved {
                prompt,
                offer,
                question,
            }) => {
                if summarizing {
                    let mut fields = json!({"_meta": branch});
                    if let Some(summary) = self.branch_summary() {
                        fields["summary"] = json!([{"type": "text", "text": summary}]);
                    }
                    let said = "Earlier branch summarized.";
                    self.ui.compaction_update("completed", fields, Some(said));
                }
                self.session_changed("✓ Navigated to selected point", Change::Kept);
                self.switched();
                let result = json!({"moved": true, "prompt": prompt});
                if question {
                    return self.run((id, result), None, Start::Typed);
                }
                return self.reopen(&id, result, offer);
            }
            Err(AgentError::Cancelled) => {
                if summarizing {
                    let fields = json!({"_meta": branch});
                    self.ui.compaction_update("cancelled", fields, None);
                }
                self.ui.out.error(&id, REQUEST_CANCELLED, "cancelled");
            }
            Err(error) => {
                if summarizing {
                    let said = format!("Branch summary failed: {error}");
                    let fields = json!({"error": error.to_string(), "_meta": branch});
                    self.ui.compaction_update("failed", fields, Some(&said));
                }
                self.ui.out.error(&id, code(&error), &error.to_string());
            }
        }
        self.after_job();
    }

    /// The branch summary the conversation now ends with, if any.
    fn branch_summary(&self) -> Option<String> {
        let tree = self.session.agent.tree().ok()?;
        let current = tree
            .entries
            .iter()
            .find(|entry| Some(entry.line) == tree.current)?;
        match &current.item {
            TreeItem::BranchSummary(summary) => Some(summary.clone()),
            _ => None,
        }
    }

    /// After a compaction or a move the client waited for: the usage, then the prompts sent
    /// meanwhile as a turn, or the session left idle.
    fn after_job(&mut self) {
        self.ui.usage_of(&self.session.agent);
        let mut turn = lock(&self.turn);
        let queued = match self.cancel.load(Ordering::Relaxed) {
            true => Vec::new(),
            false => turn.messages.take(),
        };
        if queued.is_empty() {
            turn.running = false;
            let waiting = std::mem::take(&mut turn.waiting);
            let shells = std::mem::take(&mut turn.shells);
            drop(turn);
            self.add_shells(shells);
            self.configure();
            // Prompts the client cancelled with the job.
            for id in &waiting {
                self.ui.out.respond(id, json!({"stopReason": "cancelled"}));
            }
            return;
        }
        let first = turn.waiting.remove(0);
        drop(turn);
        let ids = queued.iter().map(|queued| queued.id.clone()).collect();
        let text = texts(queued).join("\n\n");
        self.ui.taken(&text, ids);
        self.run((first, json!({})), Some(text), Start::Typed);
    }

    /// Names the session for request `id`, as `/rename`, answered with `result`, and tells the
    /// client its new title.
    fn rename(&mut self, id: &Value, name: &str, result: Value) {
        if let Err(error) = self.session.agent.set_session_name(name) {
            return self.ui.out.error(id, INTERNAL_ERROR, &error.to_string());
        }
        self.ui.out.respond(id, result);
        let now = utc_time(std::time::SystemTime::now());
        self.ui.update(
            json!({"sessionUpdate": "session_info_update", "title": name,
            "updatedAt": now}),
        );
    }

    /// Writes the current branch for request `id` as `/export` does, a relative path within the
    /// project; answered with the file written, null before any conversation.
    fn export(&mut self, id: &Value, path: Option<&str>) {
        let agent = &self.session.agent;
        match super::slash::export_branch(agent, path, agent.root()) {
            Ok(path) => {
                let path = path.map(|path| path.display().to_string());
                self.ui.out.respond(id, json!({"path": path}));
            }
            Err(error) => self.ui.out.error(id, INTERNAL_ERROR, &error.to_string()),
        }
    }

    /// Continues in a new session before the prompt at `line`, for request `id`, answered with
    /// the session's id and that prompt to edit; a next-step list offered there is offered again.
    fn fork(&mut self, id: &Value, line: usize) {
        let agent = &mut self.session.agent;
        let forked = match agent.fork_index(line) {
            Ok(Some(index)) => agent.fork(index),
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        match forked {
            Ok(Some((prompt, offer))) => {
                drive::changed(self, "Forked to new session", Change::Kept);
                self.switched();
                let result = json!({"sessionId": self.ui.session, "prompt": prompt});
                return self.reopen(id, result, offer);
            }
            Ok(None) => {
                let message = format!("no prompt at entry {line}");
                self.ui.out.error(id, INVALID_PARAMS, &message);
            }
            Err(error) => self.ui.out.error(id, INTERNAL_ERROR, &error.to_string()),
        }
        self.after_job();
    }

    /// After a fork or a move: the next-step list `offer` gone back to offered again, and
    /// request `id` answered with `result` once it closes; a step chosen runs first as a turn,
    /// and `result` then comes with its stop reason, as a prompt's.
    fn reopen(&mut self, id: &Value, result: Value, offer: Option<Reopened>) {
        if let Some(offer) = offer {
            drive::reopen(self, offer, None);
            if let Some((step, start)) = self.session.chosen.take() {
                self.ui.user_message(&step);
                return self.run((id.clone(), result), Some(step), start);
            }
        }
        self.ui.out.respond(id, result);
        self.after_job();
    }

    /// Continues in a new session with the whole conversation for request `id`, named `name`
    /// when given; answered with the session's id.
    fn branch(&mut self, id: &Value, name: &str) {
        match self.session.agent.clone_session() {
            Ok(true) => {
                drive::changed(self, "Cloned to new session", Change::Kept);
                self.switched();
                if !name.is_empty() {
                    return self.rename(id, name, json!({"sessionId": self.ui.session}));
                }
                self.ui
                    .out
                    .respond(id, json!({"sessionId": self.ui.session}));
            }
            Ok(false) => self
                .ui
                .out
                .error(id, INVALID_PARAMS, "nothing to branch yet"),
            Err(error) => self.ui.out.error(id, INTERNAL_ERROR, &error.to_string()),
        }
    }

    /// Tells the client what changed with the session: its config options and usage.
    fn switched(&mut self) {
        self.options_changed();
        self.ui.usage_of(&self.session.agent);
    }

    /// Labels the entry on `line` for request `id`, or takes its label away; answered with the
    /// label given.
    fn label(&mut self, id: &Value, line: usize, label: Option<&str>) {
        match self.session.agent.set_label(line, label) {
            Ok(label) => {
                let label = label.map(|label| label.text);
                self.ui.out.respond(id, json!({"label": label}));
            }
            Err(error) => self.ui.out.error(id, code(&error), &error.to_string()),
        }
    }

    /// The session's entries as `/tree` lists them, for request `id`.
    fn tree(&mut self, id: &Value) {
        match self.session.agent.tree() {
            Ok(tree) => {
                let entries: Vec<Value> = tree.entries.iter().map(tree_entry).collect();
                let result = json!({"entries": entries, "currentId": tree.current});
                self.ui.out.respond(id, result);
            }
            Err(error) => self.ui.out.error(id, INTERNAL_ERROR, &error.to_string()),
        }
    }

    /// Sets what `configure` names and answers its request, or says why it was not set.
    fn configure_with(&mut self, configure: Configure) {
        let Configure { id, setting } = configure;
        let (option, value) = match setting {
            Setting::Option { option, value } => (option, value),
            Setting::Rename(name) => return self.rename(&id, &name, json!({})),
            Setting::Export(path) => return self.export(&id, path.as_deref()),
            Setting::Tree => return self.tree(&id),
            Setting::Label(line, label) => return self.label(&id, line, label.as_deref()),
        };
        match self.set_option(&option, &value) {
            Ok(()) => {
                let shown = self.options();
                self.show(shown);
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
                let levels = agent.effort_levels();
                let leveled = agent.reasons() && !levels.is_empty();
                if level == current || (!leveled && level == DEFAULT_LEVEL) {
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
            self.show(options);
            let update =
                json!({"sessionUpdate": "config_option_update", "configOptions": self.shown});
            self.ui.update(update);
        }
    }

    /// Keeps `options` as those the client was given.
    fn show(&mut self, options: Value) {
        lock(&self.ui.log).options = options.clone();
        self.shown = options;
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

    /// Answers request `id` with its result's fields beside the stop reason, and those
    /// of the prompts sent during its turn, the turn having ended for `ended`'s stop reason or
    /// failed with its error; cancelled, it says so.
    fn end_turn(&mut self, id: (&Value, Value), ended: Result<&str, (i64, &str)>) {
        let mut turn = lock(&self.turn);
        turn.running = false;
        let waiting = std::mem::take(&mut turn.waiting);
        let shells = std::mem::take(&mut turn.shells);
        drop(turn);
        self.add_shells(shells);
        // Those set after the turn's last look, which no later turn would take.
        self.configure();
        let ended = match self.cancel.load(Ordering::Relaxed) {
            true => Ok("cancelled"),
            false => ended,
        };
        let mut log = lock(&self.ui.log);
        if let (true, Ok("end_turn"), Some(question)) = (self.scramjet, ended, log.question.clone())
        {
            let params = json!({"sessionId": self.ui.session, "question": question});
            log.notify(&self.ui.out, QUESTION, params);
        }
        drop(log);
        let empty = || json!({});
        let answered = std::iter::once(id).chain(waiting.iter().map(|id| (id, empty())));
        for (id, mut result) in answered {
            match ended {
                Err((code, error)) => self.ui.out.error(id, code, error),
                Ok(reason) => {
                    result["stopReason"] = json!(reason);
                    self.ui.out.respond(id, result);
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
        self.ui.usage_of(&self.session.agent);
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
            true => {
                let params = self.next_step_params(&offer);
                Some(lock(&self.ui.log).request(&self.ui.out, NEXT_STEP, params))
            }
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
        let mut log = lock(&self.ui.log);
        match (&request, &answer) {
            (Some((id, _)), None | Some(None)) => log.withdraw(&self.ui.out, id),
            _ => log.settled(),
        }
        drop(log);
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
            let method = "_io.github.leanandmean.scramjet/session_changed";
            lock(&self.ui.log).notify(&self.ui.out, method, params);
            lock(&self.renamed).insert(id.clone(), self.ui.first.clone());
            self.ui.session = id;
        }
        self.ui.tell(message);
    }
}

/// The agent's interface in a session: `session/update` notifications to the client.
struct Updates {
    out: Out,
    /// What the session sent, numbered and held for `state` and `events`.
    log: Shared,
    /// The session's id at its start, and the one its updates carry.
    first: String,
    session: String,
    notices: bool,
    /// The client knows Scramjet's extensions.
    scramjet: bool,
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
    /// The call shown now, and those of the calls running together (`running_calls`) that
    /// are not, by their index, with the one shown now's (`of`).
    call: Call,
    together: Vec<Option<Call>>,
    of: Option<usize>,
    window: Option<u64>,
    /// The usage last sent, which is not sent again.
    used: Option<Value>,
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
    /// The session file's line of the item replayed now, which its updates carry as `entryId`.
    entry: Option<usize>,
    /// The ids given to the calls whose results are not saved yet, by the model's.
    call_ids: HashMap<String, String>,
    /// The message that showed the prompt saved next, if one did.
    prompt_shown: Option<String>,
    /// The requests of the queued prompts the turn took, in order, not yet shown as taken.
    taken_ids: std::collections::VecDeque<Value>,
    /// The title the prompt saved next gives a session with no name and no prompt yet.
    title: Option<String>,
}

/// A tool call as ACP shows it.
#[derive(Default)]
struct Call {
    /// Its number among the session's calls, which its id carries.
    number: usize,
    name: String,
    /// Its arguments, and the model's id of it until it is announced; given before (`tool_input`).
    arguments: Value,
    model_id: String,
    /// What it was announced with (`title`, `kind`, `rawInput`, `locations`), which a
    /// permission request about it repeats.
    shown: Value,
}

/// Puts `fields` into `update`'s `_meta["io.github.leanandmean.scramjet"]`, beside what is there.
fn add_meta(update: &mut Value, fields: Value) {
    let ours = &mut update["_meta"]["io.github.leanandmean.scramjet"];
    if !ours.is_object() {
        *ours = json!({});
    }
    if let (Some(ours), Value::Object(fields)) = (ours.as_object_mut(), fields) {
        ours.extend(fields);
    }
}

impl Updates {
    fn update(&self, mut update: Value) {
        self.flush();
        self.stamp(&mut update);
        lock(&self.log).update(&self.out, &self.session, update);
    }
    /// A replayed message, reasoning, tool call or compaction carries its entry's line.
    fn stamp(&self, update: &mut Value) {
        let kinds = [
            "user_message_chunk",
            "agent_message_chunk",
            "agent_thought_chunk",
            "tool_call",
            "compaction_update",
        ];
        if let (true, Some(line)) = (self.replaying, self.entry)
            && kinds.contains(&update["sessionUpdate"].as_str().unwrap_or(""))
        {
            add_meta(update, json!({"entryId": line}));
        }
    }
    /// Sends the replayed tool call held, if any.
    fn flush(&self) {
        if let Some(held) = self.held.take() {
            lock(&self.log).update(&self.out, &self.session, held);
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
        self.prompt_shown = Some(format!("response-{}", self.responses));
        self.chunk("user_message_chunk", text);
    }
    /// The queued prompts the turn took, kept to be shown as they are added (`take_queued`).
    fn taking(&mut self, taken: Vec<Queued>) -> Vec<String> {
        self.taken_ids
            .extend(taken.iter().map(|queued| queued.id.clone()));
        texts(taken)
    }
    /// The queued prompt the turn adds now, `text`, shown where it goes in.
    fn take_queued(&mut self, text: &str) {
        if let Some(id) = self.taken_ids.pop_front() {
            self.taken(text, vec![id]);
        }
    }
    /// The prompts of requests `ids`, sent during a turn and given to the agent as `text`: a
    /// client that knows Scramjet is told where they go into the conversation; others showed
    /// them when they sent them.
    fn taken(&mut self, text: &str, ids: Vec<Value>) {
        if !self.scramjet {
            return;
        }
        self.responses += 1;
        self.prompt_shown = Some(format!("response-{}", self.responses));
        let mut update = self.chunked("user_message_chunk", text);
        add_meta(&mut update, json!({"promptIds": ids}));
        self.update(update);
    }
    /// What the command or skill `typed` sent in its place, as a message of its own.
    fn expansion(&mut self, typed: &str, sent: &str) {
        self.responses += 1;
        self.prompt_shown = Some(format!("response-{}", self.responses));
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
        format!("call-{}", self.call.number)
    }
    fn call_update(&self, fields: Value) {
        let mut held = self.held.take();
        if let (Some(held), Value::Object(fields)) = (&mut held, &fields) {
            for (key, value) in fields {
                match key.as_str() {
                    "_meta" => add_meta(held, value["io.github.leanandmean.scramjet"].clone()),
                    _ => held[key] = value.clone(),
                }
            }
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
    /// `agent`'s window and usage, as `usage_update` sends it.
    fn usage_of(&mut self, agent: &scramjet::agent::Agent) {
        self.window = agent.context_window();
        let usage = (
            agent.session_tokens(),
            agent.context_tokens(),
            agent.output_rate(),
        );
        self.usage_update(usage);
    }
    /// The session's tokens, its context and the model's output rate, with the tokens and the
    /// rate in `_meta`; not sent again unchanged.
    fn usage_update(&mut self, (tokens, context, rate): (Tokens, Option<u64>, Option<f64>)) {
        let (Some(used), Some(size)) = (context, self.window) else {
            return;
        };
        let cost = json!({"amount": tokens.cost, "currency": "USD"});
        let mut update =
            json!({"sessionUpdate": "usage_update", "used": used, "size": size, "cost": cost});
        let mut meta = json!({"tokens": {"input": tokens.input, "output": tokens.output,
            "cacheRead": tokens.cache_read, "cacheWrite": tokens.cache_write}});
        if let Some(rate) = rate {
            meta["outputRate"] = json!(rate);
        }
        add_meta(&mut update, meta);
        if self.used.as_ref() == Some(&update) {
            return;
        }
        self.used = Some(update.clone());
        lock(&self.log).usage = json!({"used": used, "size": size, "cost": update["cost"]});
        self.update(update);
    }
    /// Where the tool call announced last reads or changes files, as absolute paths.
    fn locations(&self) -> Vec<Value> {
        let Call {
            name, arguments, ..
        } = &self.call;
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
    /// What the edit or write shown now changed, as ACP's diffs: for an edit the text each
    /// replacement replaced, for a write that made its file (`detail`) its content.
    fn diffs(&self, detail: Option<&ToolDetail>) -> Vec<Value> {
        let Call {
            name, arguments, ..
        } = &self.call;
        let Some(path) = arguments["path"].as_str().map(|path| self.path(path)) else {
            return Vec::new();
        };
        match (name.as_str(), detail) {
            ("edit", _) => {}
            (
                "write",
                Some(ToolDetail::Written {
                    content,
                    created: true,
                    ..
                }),
            ) => {
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
        let (id, answered) = lock(&self.log).request(&self.out, method, params);
        let answer = self.answer(method, &id, &answered, cancel);
        if answer.is_some() {
            lock(&self.log).settled();
        }
        answer
    }
    /// `ask`'s wait for the answer to request `id`.
    fn answer(
        &self,
        method: &str,
        id: &Value,
        answered: &mpsc::Receiver<Value>,
        cancel: &AtomicBool,
    ) -> Option<Value> {
        loop {
            match answered.recv_timeout(POLL) {
                Ok(mut message) => match message.get_mut("result") {
                    Some(result) => return Some(result.take()),
                    None => {
                        lock(&self.log).settled();
                        let error = message["error"]["message"].as_str().unwrap_or("no reason");
                        self.say("warning", &format!("The client failed {method}: {error}"));
                        return None;
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) if !cancel.load(Ordering::Relaxed) => {}
                Err(_) => {
                    lock(&self.log).withdraw(&self.out, id);
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
        let mut call = match &self.call.shown {
            Value::Object(_) => self.call.shown.clone(),
            _ => json!({}),
        };
        call["toolCallId"] = json!(self.call_id());
        call["content"] = content;
        let params = json!({"sessionId": self.session, "toolCall": call, "options": offered});
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
        lock(&self.log).question = None;
        match self.asked.take() {
            Some(shown) => format!("{title}\n\n{shown}"),
            None => title.to_owned(),
        }
    }
    /// The answer the client's form gives for `property`, asked with what `question` showed;
    /// `None` when declined or cancelled.
    fn elicit(&mut self, title: &str, property: Value) -> Option<Value> {
        lock(&self.log).question = None;
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
    /// What the full screen's working line says now, for a client that knows the extension.
    fn status(&self, state: &str, text: &str, delay: Option<Duration>) {
        if !self.scramjet || self.replaying {
            return;
        }
        let mut params = json!({"sessionId": self.session, "state": state, "text": text});
        if let Some(delay) = delay {
            params["delayMs"] = json!(delay.as_millis() as u64);
        }
        lock(&self.log).notify(&self.out, STATUS, params);
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
        let state = match purpose {
            Purpose::Reply => "waiting",
            Purpose::Compaction(_) => "compacting",
            Purpose::BranchSummary => "summarizing",
        };
        self.status(state, super::terminal::waiting_for(purpose), None);
        match purpose {
            Purpose::Reply => {
                self.responses += 1;
                self.streamed = false;
            }
            // A compaction may ask for two summaries.
            Purpose::Compaction(_) if !self.compacting => {
                self.compaction_update("in_progress", json!({}), None);
            }
            Purpose::BranchSummary if !self.compacting => {
                let branch = json!({"io.github.leanandmean.scramjet": {"branch": true}});
                self.compaction_update("in_progress", json!({"_meta": branch}), None);
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
        let shown = format!("{} {seconds}s", super::terminal::retrying(attempt, retries));
        self.status("retrying", &shown, Some(delay));
    }
    fn cooling(&mut self, delay: Duration) {
        let seconds = delay.as_secs_f64().ceil();
        let text = format!("{} {seconds}s", super::terminal::COOLING);
        self.say("info", &text);
        self.status("cooling", &text, Some(delay));
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
        let model_id = std::mem::take(&mut self.call.model_id);
        self.call = Call {
            number: self.calls,
            name: name.to_owned(),
            arguments: std::mem::take(&mut self.call.arguments),
            ..Call::default()
        };
        if !self.replaying && !model_id.is_empty() {
            self.call_ids.insert(model_id, self.call_id());
        }
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
        if !self.call.arguments.is_null() {
            update["rawInput"] = self.call.arguments.clone();
        }
        let locations = self.locations();
        if !locations.is_empty() {
            update["locations"] = json!(locations);
        }
        self.call.shown = json!({});
        for key in ["title", "kind", "rawInput", "locations"] {
            if let Some(value) = update.get(key) {
                self.call.shown[key] = value.clone();
            }
        }
        if self.replaying {
            self.thinking = false;
            self.flush();
            self.stamp(&mut update);
            return self.held.set(Some(update));
        }
        self.update(update);
    }
    fn running_calls(&mut self, count: usize) -> bool {
        self.together = (0..count).map(|_| None).collect();
        self.of = None;
        true
    }
    fn of_call(&mut self, index: Option<usize>) {
        if let Some(shown) = self.of.take() {
            self.together[shown] = Some(std::mem::take(&mut self.call));
        }
        if let Some(index) = index {
            self.call = self.together[index].take().unwrap_or_default();
            self.of = Some(index);
        }
    }
    fn tool_input(&mut self, call_id: &str, arguments: &Value) {
        self.call.model_id = call_id.to_owned();
        self.call.arguments = arguments.clone();
    }
    fn entry(&mut self, line: Option<usize>) {
        self.entry = line;
    }
    /// Names the update that showed what was saved, for a client that knows the extension.
    fn saved(&mut self, saved: scramjet::agent::Saved, line: usize) {
        use scramjet::agent::Saved as Kept;
        let prompt = self.prompt_shown.take();
        if let (Kept::Prompt | Kept::Command, Some(title)) = (&saved, self.title.take()) {
            let now = utc_time(std::time::SystemTime::now());
            self.update(
                json!({"sessionUpdate": "session_info_update", "title": title, "updatedAt": now}),
            );
        }
        if !self.scramjet || self.replaying {
            return;
        }
        let response = format!("response-{}", self.responses);
        let compaction = format!("compaction-{}", self.compactions);
        let (kind, shown) = match saved {
            Kept::Prompt => ("prompt", prompt.map(|id| ("messageId", id))),
            Kept::Command => ("command", prompt.map(|id| ("messageId", id))),
            Kept::Reply => ("reply", Some(("messageId", response))),
            Kept::Reasoning => ("reasoning", Some(("messageId", response))),
            Kept::ToolResult(id) => {
                let shown = self.call_ids.remove(id);
                ("toolResult", shown.map(|id| ("toolCallId", id)))
            }
            Kept::Compaction if self.compaction => {
                ("compaction", Some(("compactionId", compaction)))
            }
            Kept::Compaction => ("compaction", Some(("messageId", compaction))),
        };
        let mut params = json!({"sessionId": self.session, "entryId": line, "kind": kind});
        if let Some((field, id)) = shown {
            params[field] = json!(id);
        }
        lock(&self.log).notify(&self.out, ENTRY, params);
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
            content.extend(self.diffs(detail));
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
        match self.replaying {
            true => self.user_message(typed),
            false => self.take_queued(typed),
        }
        self.expansion(typed, sent);
    }
    /// Replayed; live, a prompt queued during the turn, taken.
    fn user_prompt(&mut self, prompt: &str) {
        match self.replaying {
            true => {
                self.thinking = false;
                self.user_message(prompt);
            }
            false => self.take_queued(prompt),
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
        lock(&self.log).question = Some(message.to_owned());
        self.agent_message(message);
    }
    fn notice(&mut self, text: &str) {
        self.say("info", text);
    }
    fn next_step(&mut self, record: &str) {
        self.say("info", record);
    }
    fn steering(&mut self) -> Vec<String> {
        let taken = lock(&self.turn).messages.steer(self.steering);
        self.taking(taken)
    }
    fn follow_up(&mut self) -> Vec<String> {
        let taken = lock(&self.turn).messages.follow_up(self.follow_up);
        self.taking(taken)
    }
    fn usage(&mut self, tokens: Tokens, context: Option<u64>, rate: Option<f64>) {
        self.usage_update((tokens, context, rate));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_drops_its_oldest_messages_past_its_limit_and_says_so() {
        let (out, mut log) = (Out::default(), Log::new(true));
        for n in 0..5 {
            log.notify(&out, "test", json!({"text": "x".repeat(100), "n": n}));
            // Room for two messages.
            log.limit = 2 * log.held[0].2;
        }
        let held: Vec<u64> = log.held.iter().map(|(seq, ..)| *seq).collect();
        assert_eq!(held, [4, 5]);
        assert_eq!(
            log.held[0].1["params"]["_meta"]["io.github.leanandmean.scramjet"]["seq"],
            4
        );
        assert_eq!(log.events(3)["complete"], true);
        assert_eq!(log.events(2)["complete"], false);
        assert_eq!(log.events(4)["events"].as_array().unwrap().len(), 1);
        log.turn_starts();
        assert_eq!(log.events(5)["complete"], true);
        assert_eq!(log.events(4)["complete"], false);
    }
}
