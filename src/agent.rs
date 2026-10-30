//! A coding agent that talks to a Responses endpoint (such as Proxy) and works in a project.

mod client;
mod session;
mod tools;

pub use client::{ClientError, StreamProgress};
pub use session::Session;

use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tools::{Approval, Call};

/// Guards against a model that never stops calling tools.
const MAX_STEPS: usize = 200;
const CONTEXT_FILE_BYTES: u64 = 256 << 10;

pub struct AgentConfig {
    pub endpoint: SocketAddr,
    pub model: String,
    pub root: PathBuf,
    pub effort: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub approve_all: bool,
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
    /// The complete text of an assistant message, after any streamed progress for it.
    fn assistant(&mut self, text: &str);
    fn tool_call(&mut self, name: &str, summary: &str);
    fn tool_result(&mut self, output: &str, failed: bool);
    fn approve(&mut self, question: &str) -> bool;
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
            ui.waiting();
            let body = serde_json::to_vec(&self.request()).map_err(std::io::Error::from)?;
            let reply = self
                .executor
                .block_on(client::respond(
                    self.config.endpoint,
                    body,
                    cancel,
                    &mut |update| ui.progress(&update),
                ))
                .map_err(|error| match error {
                    ClientError::Cancelled => AgentError::Cancelled,
                    ClientError::Failed(message) => AgentError::Provider(message),
                })?;
            self.usage.requests += 1;
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

    fn call(&self, item: &Value, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> String {
        let call = match Call::parse(item) {
            Ok(call) => call,
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(&output, true);
                return output;
            }
        };
        ui.tool_call(call.name, &call.summary());
        if let Approval::Ask(question) = call.approval(&self.config.root)
            && !self.config.approve_all
            && !ui.approve(&question)
        {
            let output = "The user denied this tool call.".to_owned();
            ui.tool_result(&output, true);
            return output;
        }
        let (output, failed) = match call.run(&self.config.root, cancel) {
            Ok(output) => (output, false),
            Err(error) => (format!("Error: {error}"), true),
        };
        ui.tool_result(&output, failed);
        output
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

    #[test]
    fn requests_share_the_session_cache_key_across_resumes() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-cache-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
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
        };
        let agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        assert_eq!(agent.request()["prompt_cache_key"], "1700000000000-42");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
