//! A coding agent that talks to Proxy (Responses for GPT models, Messages for Claude) and works in
//! a project.

mod autonomy;
mod client;
mod commands;
mod compaction;
mod instructions;
mod messages;
mod question;
mod request;
mod sandbox;
mod session;
mod status;
mod subagent;
mod tools;
mod tree;
mod turn;
mod ui;

pub use client::StreamProgress;
pub use commands::{Command, Commands};
pub use compaction::{AutoCompaction, Compaction};
pub use question::Choice;
pub use session::{Session, SessionInfo};
pub use status::{Completed, Next, Routing, Step, record as next_step_record};
pub use tools::{
    CODING_TOOLS, DEFAULT_TOOLS, search_program, sequence_diff, shortened, terminal_line,
};
pub use tree::{Navigated, SessionTree, TreeEntry, TreeItem};
pub use turn::Shell;
pub use ui::{AgentUi, Answer, Purpose, ToolDetail, show_items};

use client::ModelInfo;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

/// Guards against a model that never stops calling tools.
const MAX_STEPS: usize = 200;
/// Stored with each response item: the model that produced it. Never sent to the endpoint.
const PRODUCER: &str = "_scramjet_model";
/// Stored with a response's last item when the response reported it: the context size, so a
/// resumed session knows how full the context is before its first reply, as Scramjet reads it
/// from saved usage. Never sent.
const CONTEXT: &str = "_scramjet_context_tokens";
/// Stored with a response's last item when the response reported its usage: the tokens it counted
/// (`input` not read from the cache, `output`, `cache_read`), which `/session` sums over the
/// conversation as Scramjet sums its assistant messages' usage. Never sent.
const USAGE: &str = "_scramjet_usage";
/// Stored with a user message whose text is not its prompt: the `!command` it ran, which is recalled
/// as typed. Never sent to the endpoint.
pub(super) const TYPED: &str = "_scramjet_typed";
/// Stored, as `true`, with a tool result or `!` command that failed, so a resumed conversation
/// shows it as it ran. Never sent, as no `_scramjet_` field is.
const FAILED: &str = "_scramjet_failed";
/// Stored with an edit's result: the diff shown for it.
const DIFF: &str = "_scramjet_diff";
/// Stored, as `true`, with Scramjet's status check, a user message the user never typed, so it
/// is neither shown nor recalled.
pub(super) const PROBE: &str = "_scramjet_probe";

pub struct AgentConfig {
    pub endpoint: SocketAddr,
    pub model: String,
    pub root: PathBuf,
    pub effort: Option<String>,
    pub max_output_tokens: Option<u64>,
    /// Ask before each bash command and each write or edit outside the project.
    pub ask: bool,
    /// Extra attempts for a model request that failed transiently (rate limit, overload, network).
    pub retries: u32,
    /// The coding tools the model has, among `CODING_TOOLS`.
    pub tools: Vec<String>,
}

/// The reasoning effort levels a model's card lists, and the current one, as Shift+Tab cycles
/// through them.
#[derive(Clone, Debug, Default)]
pub struct EffortCycle {
    levels: Vec<String>,
    current: Option<String>,
}

impl EffortCycle {
    /// Moves to the level after the current one, wrapping around as Scramjet's thinking level
    /// cycle does, and returns it; `None` when the card lists no levels.
    pub fn advance(&mut self) -> Option<String> {
        let levels = &self.levels;
        let next = (self.current.as_ref())
            .and_then(|current| levels.iter().position(|level| level == current))
            .map_or(0, |index| (index + 1) % levels.len());
        let level = levels.get(next)?.clone();
        self.current = Some(level.clone());
        Some(level)
    }
}

pub enum SessionChoice {
    New,
    Continue,
    Open(PathBuf),
}

#[derive(Debug)]
pub enum AgentError {
    Cancelled,
    /// The turn stopped before a request for the session to use the agent (`AgentUi::pause`).
    Paused,
    Provider(String),
    /// A request failed because its input exceeded the model's context window.
    ContextOverflow(String),
    /// A compaction the agent started by itself failed.
    CompactionFailed(Box<AgentError>),
    Session(std::io::Error),
    TooManySteps,
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Paused => f.write_str("paused"),
            Self::Provider(message) => write!(f, "provider: {message}"),
            Self::ContextOverflow(message) => write!(f, "context overflow: {message}"),
            Self::CompactionFailed(error) => write!(f, "automatic compaction failed: {error}"),
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
    /// The input and output tokens of the last response, when it reported them: how full the
    /// context was, as Scramjet's footer shows it.
    pub context_tokens: Option<u64>,
}

impl Usage {
    /// Counts a response's tokens.
    fn count(&mut self, usage: &crate::http::responses::ProviderUsage) {
        self.input_tokens += usage.input_tokens.unwrap_or(0);
        self.cached_input_tokens += usage.cached_input_tokens.unwrap_or(0);
        self.output_tokens += usage.output_tokens.unwrap_or(0);
    }

    /// Adds another agent's requests and tokens, not its context.
    fn add(&mut self, other: &Usage) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

/// `/session`'s counts over the current conversation, as Scramjet's session stats count them.
#[derive(Debug, Default, PartialEq)]
pub struct Stats {
    pub user: usize,
    pub assistant: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    /// Every message: the counts above, and the `!` commands and summaries.
    pub total: usize,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
}

pub struct Agent {
    config: AgentConfig,
    instructions: String,
    conversation: Vec<Value>,
    prompts: Vec<String>,
    commands: Commands,
    /// The command the last command prompt ran, until it is reported or left.
    lifecycle: status::Lifecycle,
    /// The command the last turn reported completed, until its next step is routed.
    completed: Option<Completed>,
    /// Scramjet's global `autonomy.yaml`, and what was last said about the files.
    autonomy: Option<PathBuf>,
    autonomy_diagnostics: Vec<String>,
    /// Where subagents are found: the user's agent directory and Scramjet's global root of
    /// command sets.
    agents_directory: Option<PathBuf>,
    sets_root: Option<PathBuf>,
    session: Session,
    executor: tokio::runtime::Runtime,
    /// The current model's protocol and limits, from its name until `load_model_card`.
    info: ModelInfo,
    /// Whether the coding tools keep out of the credential stores (`sandbox::Hidden`).
    hide_credentials: bool,
    pub usage: Usage,
}

impl Agent {
    pub fn start(config: AgentConfig, choice: SessionChoice) -> Result<Self, AgentError> {
        let opened = match choice {
            SessionChoice::New => None,
            SessionChoice::Continue => Session::latest(&session::directory(&config.root)?)?,
            SessionChoice::Open(path) => Some(path),
        };
        let (session, conversation, prompts) = match opened {
            Some(path) => Session::open(&path)?,
            None => (
                Session::create(&session::directory(&config.root)?, &config.root)?,
                Vec::new(),
                Vec::new(),
            ),
        };
        let context_tokens = session.context_tokens();
        let lifecycle = status::Lifecycle::restored(session.command());
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let sets_root = commands::global_root();
        let commands = Commands::discover(sets_root.as_deref(), &config.root.join(".scramjet"));
        let agents_directory = subagent::user_directory();
        let sources = subagent::Sources {
            user: agents_directory.as_deref(),
            sets: sets_root.as_deref(),
        };
        let agents =
            subagent::discover(&sources, &config.root, &config.root, subagent::Scope::User).agents;
        let mut agent = Self {
            instructions: instructions::agent(&config.root, &config.tools, &commands, &agents),
            agents_directory,
            sets_root,
            commands,
            lifecycle,
            completed: None,
            autonomy: autonomy::global_path(),
            autonomy_diagnostics: Vec::new(),
            info: ModelInfo::guess(&config.model),
            hide_credentials: false,
            config,
            conversation,
            prompts,
            session,
            executor,
            usage: Usage {
                context_tokens,
                ..Usage::default()
            },
        };
        agent.answer_interrupted_calls()?;
        Ok(agent)
    }

    /// The sessions saved next to the current one that have a conversation, and the current one,
    /// newest first. Scramjet writes no session file before the first reply, so it lists no
    /// empty ones.
    pub fn sessions(&self) -> Result<Vec<SessionInfo>, AgentError> {
        let directory = self.session.path().parent().unwrap_or(Path::new("."));
        let mut sessions = Session::list(directory)?;
        sessions.retain(|session| session.item_count > 0 || self.session.is_file(&session.path));
        Ok(sessions)
    }

    /// Whether `path` is the current session's file.
    pub fn is_session(&self, path: &Path) -> bool {
        self.session.is_file(path)
    }

    /// Continues in the saved session `path` with the model it last used, as starting with
    /// `--session` does; on a failure to open it nothing changes. Approvals given with `a` still
    /// apply.
    pub fn resume_session(&mut self, path: &Path) -> Result<(), AgentError> {
        if self.session.is_file(path) {
            return Err(AgentError::Session(std::io::Error::other(
                "that is the current session",
            )));
        }
        self.switch_session(Session::open(path)?)
    }

    /// The current session's name, as `/name` sets it.
    pub fn session_name(&self) -> Option<&str> {
        self.session.name()
    }

    /// Names the current session `name`, saved at once; empty clears the name.
    pub fn set_session_name(&mut self, name: &str) -> Result<(), AgentError> {
        Ok(self.session.set_name(name)?)
    }

    /// The current session's id, its file name without the extension.
    pub fn session_id(&self) -> String {
        self.session.id()
    }

    /// The counts over the current conversation, as `/session` shows them. Assistant messages
    /// are the runs of model output between prompts and tool results, as Scramjet has one message
    /// per response; the usage is what each response saved.
    pub fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        let mut other = 0;
        let mut in_reply = false;
        for item in &self.conversation {
            let reply = item["role"] == "assistant"
                || matches!(item["type"].as_str(), Some("reasoning" | "function_call"));
            if reply && !in_reply {
                stats.assistant += 1;
            }
            in_reply = reply;
            match item["type"].as_str() {
                Some("function_call") => stats.tool_calls += 1,
                Some("function_call_output") => stats.tool_results += 1,
                _ if item["role"] == "user"
                    && !session::is_bang(item)
                    && !compaction::is_summary(item)
                    && item.get(PROBE).is_none() =>
                {
                    stats.user += 1
                }
                _ if item["role"] == "user" => other += 1,
                _ => {}
            }
            if let Some(usage) = item.get(USAGE) {
                stats.input += usage["input"].as_u64().unwrap_or(0);
                stats.output += usage["output"].as_u64().unwrap_or(0);
                stats.cache_read += usage["cache_read"].as_u64().unwrap_or(0);
            }
        }
        stats.total = stats.user + stats.assistant + stats.tool_results + other;
        stats
    }

    /// The prompts the current session can be forked before, oldest first, compacted ones too.
    pub fn fork_prompts(&self) -> Result<Vec<String>, AgentError> {
        Ok(self.session.fork_prompts()?)
    }

    /// Continues in a new session saved next to the current one, with the conversation before
    /// the current session's prompt `index` (0-based, as `fork_prompts` lists them) and the model
    /// it used then, as Scramjet's `/fork` does; the current session stays as it is. Returns the
    /// prompt, for editing before it is sent again; `None`: no such prompt. On failure nothing
    /// changes. Approvals given with `a` still apply.
    pub fn fork(&mut self, index: usize) -> Result<Option<String>, AgentError> {
        let Some((path, prompt)) = self.session.fork(&self.config.root, index)? else {
            return Ok(None);
        };
        let opened = Session::open(&path).and_then(|(mut session, conversation, prompts)| {
            // As in Scramjet, a fork without a conversation keeps the current model.
            if conversation.is_empty() {
                session.record_model(&self.config.model)?;
            }
            Ok((session, conversation, prompts))
        });
        match opened {
            Ok(opened) => self.switch_session(opened)?,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error.into());
            }
        }
        Ok(Some(prompt))
    }

    fn switch_session(
        &mut self,
        (session, conversation, prompts): (Session, Vec<Value>, Vec<String>),
    ) -> Result<(), AgentError> {
        let model = session.model().map(str::to_owned);
        let context_tokens = session.context_tokens();
        let effort = session.effort().map(str::to_owned);
        self.session = session;
        self.conversation = conversation;
        self.prompts = prompts;
        self.usage = Usage {
            context_tokens,
            ..Usage::default()
        };
        self.restore_command();
        if let Some(model) = model {
            self.info = ModelInfo::guess(&model);
            self.config.model = model;
        }
        if let Some(level) = effort {
            self.config.effort = Some(level);
        }
        self.answer_interrupted_calls()?;
        Ok(())
    }

    /// Continues in an empty session saved next to the current one; on failure nothing changes.
    /// Approvals given with `a` still apply.
    pub fn new_session(&mut self) -> Result<(), AgentError> {
        let directory = self.session.path().parent().unwrap_or(Path::new("."));
        let mut session = Session::create(directory, &self.config.root)?;
        session.record_model(&self.config.model)?;
        if let Some(effort) = &self.config.effort {
            session.record_effort(effort)?;
        }
        self.session = session;
        self.conversation.clear();
        self.prompts.clear();
        self.usage = Usage::default();
        self.restore_command();
        Ok(())
    }

    /// The commands this session's prompts can run, as discovered when the session started.
    pub fn commands(&self) -> &Commands {
        &self.commands
    }

    /// The user's prompts in this session, oldest first, including those compacted away.
    pub fn prompts(&self) -> &[String] {
        &self.prompts
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

    /// Changes the model used for subsequent requests and records it in the session, so a resume
    /// continues with it; on failure nothing changes.
    pub fn set_model(&mut self, model: impl Into<String>) -> Result<(), AgentError> {
        let model = model.into();
        self.session.record_model(&model)?;
        self.info = ModelInfo::guess(&model);
        self.config.model = model;
        Ok(())
    }

    /// The reasoning effort requests ask for: the one set, fitted to the levels the model's card
    /// lists as Scramjet clamps a thinking level; `None` leaves it to the model.
    pub fn effort(&self) -> Option<&str> {
        let effort = self.config.effort.as_deref()?;
        Some(self.info.fit(effort))
    }

    /// The reasoning effort the session last recorded.
    pub fn last_effort(&self) -> Option<&str> {
        self.session.effort()
    }

    /// Changes the reasoning effort of subsequent requests and records it in the session, so a
    /// resume continues with it; on failure nothing changes.
    pub fn set_effort(&mut self, effort: impl Into<String>) -> Result<(), AgentError> {
        let effort = effort.into();
        self.session.record_effort(&effort)?;
        self.config.effort = Some(effort);
        Ok(())
    }

    /// Shift+Tab: moves the reasoning effort to the next level the model's card lists, after the
    /// current one (the card's default when none is set), wrapping around as Scramjet's thinking
    /// level cycle does, and records the change in the session. Returns the new level; `None`
    /// when the card lists no levels, which keeps the effort.
    pub fn cycle_effort(&mut self) -> Result<Option<String>, AgentError> {
        let Some(level) = self.effort_cycle().advance() else {
            return Ok(None);
        };
        self.set_effort(level.clone())?;
        Ok(Some(level))
    }

    /// Where Shift+Tab's cycle through the model's effort levels is, for an interface that
    /// cycles while a turn runs (`AgentUi::effort`).
    pub fn effort_cycle(&self) -> EffortCycle {
        EffortCycle {
            levels: self.info.levels.clone(),
            current: (self.effort().or(self.info.default_level.as_deref())).map(str::to_owned),
        }
    }

    /// Whether the coding tools of the turns that follow keep out of the user's credential
    /// stores: bash runs with them hidden, and other tools refuse paths in them. Subagents
    /// always do. A `!` command, which the user runs, never does.
    pub fn set_hide_credentials(&mut self, hide: bool) {
        self.hide_credentials = hide;
    }

    /// The window automatic compaction checks; `None` skips the check.
    pub fn set_context_window(&mut self, window: Option<u64>) {
        self.info.window = window;
    }

    /// The current model's context window, from its card, or `None` when unknown.
    pub fn context_window(&self) -> Option<u64> {
        self.info.window
    }

    /// The model the session last recorded: its latest switch or response item's producer.
    pub fn last_model(&self) -> Option<&str> {
        self.session.model()
    }

    /// The endpoint's models, or `None` if they cannot be listed.
    pub fn models(&self) -> Option<Vec<String>> {
        self.executor.block_on(client::models(self.config.endpoint))
    }

    pub fn conversation(&self) -> &[Value] {
        &self.conversation
    }

    /// Shows the conversation as a resumed session has it, as its turns showed when they ran.
    pub fn show_conversation(&self, ui: &mut dyn AgentUi) {
        show_items(&self.conversation, ui);
    }

    /// Reads the current model's Proxy model card: which protocol its requests use, its output
    /// limit and its context window, which is returned. Without a card the model's name decides
    /// the protocol, and the window is unknown (`None`).
    pub fn load_model_card(&mut self) -> Option<u64> {
        if let Some(info) = self
            .executor
            .block_on(client::model_info(self.config.endpoint, &self.config.model))
        {
            self.info = info;
        }
        self.info.window
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
                "output":"Error: the previous session ended before this tool call reported a result. It may or may not have taken effect, and it was not re-run.",
                FAILED:true}))?;
        }
        Ok(())
    }

    fn add(&mut self, item: Value) -> Result<(), AgentError> {
        self.session.append(&item)?;
        self.conversation.push(item);
        Ok(())
    }

    /// A session switch or a move in the session leaves the command that was running for the
    /// one saved there, as Scramjet rebuilds its lifecycle from the session it switches to.
    fn restore_command(&mut self) {
        self.lifecycle = status::Lifecycle::restored(self.session.command());
        self.completed = None;
    }

    /// Saves a change of the active command for a resume.
    fn save_command(&mut self) -> Result<(), AgentError> {
        self.session.record_command(&self.lifecycle.saved())?;
        Ok(())
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

#[cfg(test)]
mod tests;
