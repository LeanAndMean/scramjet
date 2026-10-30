//! A coding agent that talks to a Responses endpoint (such as Proxy) and works in a project.

mod client;
mod session;
mod tools;

pub use client::StreamProgress;
pub use session::Session;

use client::ClientError;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tools::{Approval, Call};

/// Guards against a model that never stops calling tools.
const MAX_STEPS: usize = 200;
const CONTEXT_FILE_BYTES: u64 = 256 << 10;
/// Scramjet's retry backoff: 2s, 4s, 8s, ...
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Like Scramjet, a longer wait (backoff or the provider's Retry-After) fails the request instead.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

pub struct AgentConfig {
    pub endpoint: SocketAddr,
    pub model: String,
    pub root: PathBuf,
    pub effort: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub approve_all: bool,
    /// Extra attempts for a model request that failed transiently (rate limit, overload, network).
    pub retries: u32,
}

pub enum SessionChoice {
    New,
    Continue,
    Open(PathBuf),
}

/// How the agent presents progress and asks for approval.
pub trait AgentUi {
    fn waiting(&mut self);
    /// Text or reasoning summary as it streams; may never be called for a response.
    fn progress(&mut self, update: &StreamProgress);
    /// A model request failed transiently and will be sent again after `delay`.
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration);
    /// The complete text of an assistant message, after any streamed progress for it.
    fn assistant(&mut self, text: &str);
    fn tool_call(&mut self, name: &str, summary: &str);
    /// The tool call announced last starts running, after any approval.
    fn tool_running(&mut self);
    /// The end of the running tool's output so far.
    fn tool_output(&mut self, _output: &str) {}
    /// `output` is what the model receives; `detail`, if any, is better shown to the user.
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool);
    /// Asks whether a tool call may run; returns `Deny` without an answer once `cancel` is set.
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Deny,
    Allow,
    /// Allow this call and every later one until the agent exits.
    AllowAll,
}

/// What the user sees of a successful tool call instead of the text the model receives.
#[derive(Debug, PartialEq)]
pub enum ToolDetail {
    /// The lines an edit changed, each marked ` ` (unchanged), `-` or `+` and numbered, with
    /// `...` for skipped lines.
    Diff(String),
    /// The content a write put in the file.
    Written(String),
}

#[derive(Debug)]
pub enum AgentError {
    Cancelled,
    Provider(String),
    Session(std::io::Error),
    TooManySteps,
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Provider(message) => write!(f, "provider: {message}"),
            Self::Session(error) => write!(f, "session: {error}"),
            Self::TooManySteps => write!(f, "stopped after {MAX_STEPS} model requests in one turn"),
        }
    }
}
impl From<std::io::Error> for AgentError {
    fn from(error: std::io::Error) -> Self {
        Self::Session(error)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

pub struct Agent {
    config: AgentConfig,
    instructions: String,
    conversation: Vec<Value>,
    session: Session,
    executor: tokio::runtime::Runtime,
    pub usage: Usage,
}

impl Agent {
    pub fn start(config: AgentConfig, choice: SessionChoice) -> Result<Self, AgentError> {
        let opened = match choice {
            SessionChoice::New => None,
            SessionChoice::Continue => Session::latest(&config.root)?,
            SessionChoice::Open(path) => Some(path),
        };
        let (session, conversation) = match opened {
            Some(path) => Session::open(&path)?,
            None => (Session::create(&config.root)?, Vec::new()),
        };
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut agent = Self {
            instructions: instructions(&config.root),
            config,
            conversation,
            session,
            executor,
            usage: Usage::default(),
        };
        agent.answer_interrupted_calls()?;
        Ok(agent)
    }

    pub fn session_path(&self) -> &Path {
        self.session.path()
    }

    pub fn root(&self) -> &Path {
        &self.config.root
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub fn conversation(&self) -> &[Value] {
        &self.conversation
    }

    /// A tool call left without a result (the process stopped mid-call) is answered, never re-run.
    fn answer_interrupted_calls(&mut self) -> Result<(), AgentError> {
        let answered: std::collections::HashSet<_> = self
            .conversation
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .filter_map(|item| item["call_id"].as_str().map(str::to_owned))
            .collect();
        let missing: Vec<_> = self
            .conversation
            .iter()
            .filter(|item| item["type"] == "function_call")
            .filter_map(|item| item["call_id"].as_str())
            .filter(|id| !answered.contains(*id))
            .map(str::to_owned)
            .collect();
        for call_id in missing {
            self.add(json!({"type":"function_call_output","call_id":call_id,
                "output":"Error: the previous session ended before this tool call reported a result. It may or may not have taken effect, and it was not re-run."}))?;
        }
        Ok(())
    }

    fn add(&mut self, item: Value) -> Result<(), AgentError> {
        self.session.append(&item)?;
        self.conversation.push(item);
        Ok(())
    }

    fn request(&self) -> Value {
        let mut request = json!({
            "model": self.config.model,
            "instructions": self.instructions,
            "input": self.conversation,
            "tools": tools::definitions(),
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            // GPT-5.6 and later only reuse cached prefixes reliably when requests share a key.
            "prompt_cache_key": self.session.id(),
        });
        request["reasoning"] = json!({"summary": "auto"});
        if let Some(effort) = &self.config.effort {
            request["reasoning"]["effort"] = json!(effort);
        }
        if let Some(tokens) = self.config.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        request
    }

    /// Runs one user prompt to completion: model requests and tool calls until a final answer.
    pub fn turn(
        &mut self,
        prompt: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        self.add(json!({"role":"user","content":prompt}))?;
        for _ in 0..MAX_STEPS {
            if cancel.load(Ordering::Relaxed) {
                return Err(AgentError::Cancelled);
            }
            let reply = self.respond(ui, cancel)?;
            if let Some(usage) = &reply.usage {
                self.usage.input_tokens += usage.input_tokens.unwrap_or(0);
                self.usage.cached_input_tokens += usage.cached_input_tokens.unwrap_or(0);
                self.usage.output_tokens += usage.output_tokens.unwrap_or(0);
            }
            let mut calls = Vec::new();
            for item in reply.items {
                match item["type"].as_str() {
                    Some("message") => ui.assistant(&message_text(&item)),
                    Some("function_call") => calls.push(item.clone()),
                    _ => {}
                }
                self.add(item)?;
            }
            if calls.is_empty() {
                return Ok(());
            }
            for item in calls {
                let output = self.call(&item, ui, cancel);
                self.add(json!({"type":"function_call_output","call_id":item["call_id"],"output":output}))?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(AgentError::Cancelled);
                }
            }
        }
        Err(AgentError::TooManySteps)
    }

    /// Sends the conversation, sending it again after transient failures. A failed response's
    /// items are discarded, so a retry never runs a tool call from it.
    fn respond(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<client::Reply, AgentError> {
        let body = serde_json::to_vec(&self.request()).map_err(std::io::Error::from)?;
        let mut attempt = 0;
        loop {
            ui.waiting();
            self.usage.requests += 1;
            let failure = match self.executor.block_on(client::respond(
                self.config.endpoint,
                body.clone(),
                cancel,
                &mut |update| ui.progress(&update),
            )) {
                Ok(reply) => return Ok(reply),
                Err(ClientError::Cancelled) => return Err(AgentError::Cancelled),
                Err(ClientError::Failed(failure)) => failure,
            };
            let backoff = RETRY_DELAY * 2u32.pow(attempt.min(5));
            let delay = backoff
                .min(MAX_RETRY_WAIT)
                .max(failure.wait.unwrap_or_default());
            if !failure.transient || attempt == self.config.retries || delay > MAX_RETRY_WAIT {
                return Err(AgentError::Provider(match attempt {
                    0 => failure.message,
                    n => format!("{} (after {n} retries)", failure.message),
                }));
            }
            attempt += 1;
            ui.retrying(&failure.message, attempt, self.config.retries, delay);
            let interrupted = self.executor.block_on(async {
                tokio::select! {
                    biased;
                    () = client::cancelled(cancel) => true,
                    () = tokio::time::sleep(delay) => false,
                }
            });
            if interrupted {
                return Err(AgentError::Cancelled);
            }
        }
    }

    fn call(&mut self, item: &Value, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> String {
        let call = match Call::parse(item) {
            Ok(call) => call,
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(item["name"].as_str().unwrap_or(""), &output, None, true);
                return output;
            }
        };
        ui.tool_call(call.name, &call.summary());
        let answer = match call.approval(&self.config.root) {
            Approval::Ask(question) if !self.config.approve_all => ui.approve(&question, cancel),
            _ => Answer::Allow,
        };
        if answer == Answer::AllowAll {
            self.config.approve_all = true;
        }
        if answer == Answer::Deny {
            let output = if cancel.load(Ordering::Relaxed) {
                "The user cancelled the turn before this tool call ran."
            } else {
                "The user denied this tool call."
            };
            ui.tool_result(call.name, output, None, true);
            return output.to_owned();
        }
        ui.tool_running();
        match call.run(&self.config.root, cancel, &mut |output| {
            ui.tool_output(output)
        }) {
            Ok(output) => {
                ui.tool_result(
                    call.name,
                    &output.text,
                    output.detail.as_ref(),
                    output.failed,
                );
                output.text
            }
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(call.name, &output, None, true);
                output
            }
        }
    }
}

fn message_text(item: &Value) -> String {
    item["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str().or(part["refusal"].as_str()))
        .collect()
}

fn instructions(root: &Path) -> String {
    let mut text = format!(
        "You are scramjet-rs, a coding agent. You help the user with software engineering tasks \
in the project directory {root}, using tools to read, write and edit files and to run bash commands.

Guidelines:
- Use read to examine files, edit for precise changes, write for new files or complete rewrites, \
and bash for everything else (ls, rg, find, git, builds, tests).
- Prefer editing existing files. Keep changes focused on the task, and verify them with relevant checks.
- Be concise. Show file paths clearly when referring to files.

Current date: {date}
Current working directory: {root}
",
        root = root.display(),
        date = today(),
    );
    let mut files = Vec::new();
    for directory in root.ancestors() {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = directory.join(name);
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= CONTEXT_FILE_BYTES)
                && let Ok(contents) = std::fs::read_to_string(&path)
            {
                files.push((path, contents));
            }
        }
    }
    if !files.is_empty() {
        text.push_str("\n# Project Context\n\nProject-specific instructions and guidelines:\n");
        for (path, contents) in files.iter().rev() {
            text.push_str(&format!(
                "\n## {}\n\n{}\n",
                path.display(),
                contents.trim_end()
            ));
        }
    }
    text
}

/// UTC date as YYYY-MM-DD, from days since the Unix epoch.
fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        / 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_fixture as fixture;
    use std::io::{BufRead, BufReader, Read, Write};

    #[derive(Default)]
    struct Recorder {
        retries: Vec<(String, u32, Duration)>,
        replies: Vec<String>,
        calls: Vec<String>,
        /// Answers to give, in order; questions beyond them are denied.
        answers: Vec<Answer>,
        questions: usize,
    }
    impl AgentUi for Recorder {
        fn waiting(&mut self) {}
        fn progress(&mut self, _: &StreamProgress) {}
        fn retrying(&mut self, error: &str, attempt: u32, _: u32, delay: Duration) {
            self.retries.push((error.to_owned(), attempt, delay));
        }
        fn assistant(&mut self, text: &str) {
            self.replies.push(text.to_owned());
        }
        fn tool_call(&mut self, name: &str, _: &str) {
            self.calls.push(name.to_owned());
        }
        fn tool_running(&mut self) {}
        fn tool_result(&mut self, _: &str, _: &str, _: Option<&ToolDetail>, _: bool) {}
        fn approve(&mut self, _: &str, _: &AtomicBool) -> Answer {
            self.questions += 1;
            self.answers
                .get(self.questions - 1)
                .copied()
                .unwrap_or(Answer::Deny)
        }
    }

    /// Creates a scratch directory and removes it when dropped, also when a test panics.
    pub(super) struct ScratchDir(PathBuf);

    impl ScratchDir {
        pub(super) fn new(path: PathBuf) -> Self {
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn stream(events: &[Value]) -> Vec<u8> {
        let body = fixture::wire(events, false);
        let mut response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend(body);
        response
    }

    /// Answers each connection with the next scripted response; returns the request bodies.
    fn serve(responses: Vec<Vec<u8>>) -> (SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for response in responses {
                let (connection, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(connection);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                bodies.push(serde_json::from_slice(&body).unwrap());
                reader.get_mut().write_all(&response).unwrap();
            }
            bodies
        });
        (address, server)
    }

    fn agent(endpoint: SocketAddr, name: &str) -> (Agent, ScratchDir) {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-{name}-{}", std::process::id()));
        let scratch = ScratchDir::new(root.clone());
        let path = root.join("session.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"version\":1}\n").unwrap();
        let config = AgentConfig {
            endpoint,
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            approve_all: false,
            retries: 3,
        };
        (
            Agent::start(config, SessionChoice::Open(path)).unwrap(),
            scratch,
        )
    }

    #[test]
    fn transient_failures_are_sent_again_without_running_their_tool_calls() {
        let mut failed = fixture::call_events("{}", fixture::known_usage());
        let terminal = failed.last_mut().unwrap();
        terminal["type"] = json!("response.failed");
        terminal["response"]["status"] = json!("failed");
        // Observed through Proxy on 2026-10-30.
        terminal["response"]["error"] = json!({"message": "Upstream model provider failed before \
the response completed (ReadError: ConnectionResetError(104, 'Connection reset by peer')). The \
response is incomplete; retry the request."});
        let (endpoint, server) = serve(vec![
            stream(&failed),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "retry");
        let mut ui = Recorder::default();
        agent.turn("hi", &mut ui, &AtomicBool::new(false)).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(ui.retries.len(), 1);
        assert!(ui.retries[0].0.contains("retry the request"));
        assert_eq!((ui.retries[0].1, ui.retries[0].2), (1, RETRY_DELAY));
        assert_eq!(ui.replies, ["done"]);
        assert!(ui.calls.is_empty());
        assert_eq!(agent.usage.requests, 2);
        let kinds: Vec<_> = agent
            .conversation()
            .iter()
            .map(|item| item["type"].as_str().or(item["role"].as_str()).unwrap())
            .collect();
        assert_eq!(kinds, ["user", "message"]);
    }

    #[test]
    fn permanent_failures_and_long_retry_waits_end_the_turn_at_once() {
        let mut overflow = fixture::text_events("partial", fixture::known_usage());
        overflow.truncate(4);
        overflow.push(json!({"type":"error","code":"context_length_exceeded",
            "message":"Your input exceeds the context window of this model."}));
        let (endpoint, server) = serve(vec![
            stream(&overflow),
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 15\r\n\r\ninvalid request".to_vec(),
            b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 120\r\ncontent-length: 4\r\n\r\nslow"
                .to_vec(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "permanent");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let errors: Vec<_> = (0..3)
            .map(|_| match agent.turn("hi", &mut ui, &cancel) {
                Err(AgentError::Provider(message)) => message,
                other => panic!("{:?}", other.err()),
            })
            .collect();
        assert_eq!(
            errors,
            [
                "provider error: Your input exceeds the context window of this model.",
                "HTTP 400 Bad Request: invalid request",
                "HTTP 429 Too Many Requests: slow",
            ]
        );
        assert!(ui.retries.is_empty());
        assert_eq!(server.join().unwrap().len(), 3);
    }

    #[test]
    fn failures_are_classified_by_code_then_wording() {
        let transient =
            |code, message: &str| client::Failure::provider(code, message.into()).transient;
        assert!(transient(Some("server_error"), "anything"));
        assert!(!transient(Some("insufficient_quota"), "rate limit"));
        assert!(transient(None, "Rate limit reached for requests"));
        assert!(transient(Some("unknown_code"), "The server is overloaded"));
        assert!(!transient(
            None,
            "This model's maximum context length is 128000 tokens; timeout"
        ));
        assert!(!transient(None, "Invalid value for 'input'"));
    }

    #[test]
    fn allowing_all_stops_later_approval_questions() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
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
    fn requests_share_the_session_cache_key_across_resumes() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-cache-{}", std::process::id()));
        let _scratch = ScratchDir::new(root.clone());
        let path = root.join("1700000000000-42.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"session\",\"version\":1}\n{\"role\":\"user\",\"content\":\"hi\"}\n",
        )
        .unwrap();
        let config = AgentConfig {
            endpoint: ([127, 0, 0, 1], 9).into(),
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            approve_all: false,
            retries: 3,
        };
        let agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        assert_eq!(agent.request()["prompt_cache_key"], "1700000000000-42");
    }
}
