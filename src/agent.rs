//! A coding agent that talks to Proxy (Responses for GPT models, Messages for Claude) and works in
//! a project.

mod autonomy;
mod client;
mod commands;
mod compaction;
mod delegate;
mod forge;
mod instructions;
mod messages;
mod publish;
mod question;
mod request;
mod sandbox;
mod session;
mod skills;
mod status;
mod subagent;
mod tools;
mod tree;
mod turn;
mod ui;

pub use autonomy::{
    Approval, Approvals, Edge, NextStep, NextSteps, Publication, Scope, Setting, Suggestion,
};
pub use client::StreamProgress;
pub use commands::{Command, Commands, Origin, Set, Sets};
pub use compaction::{AutoCompaction, Compaction};
pub use delegate::{NAME as DELEGATE, SCOPE_WARNING as DELEGATE_SCOPE_WARNING};
pub use publish::TOOLS as PUBLICATION_TOOLS;
pub use question::Choice;
pub use session::{Removal, Session, SessionInfo};
pub use skills::{Skill, Skills};
pub use status::{Completed, Next, Routing, Step};
pub use tools::{
    CODING_TOOLS, DEFAULT_TOOLS, search_program, sequence_diff, shortened, terminal_line,
    write_replacing,
};
pub use tree::{Navigated, SessionTree, TreeEntry, TreeItem};
pub use turn::Shell;
pub use ui::{
    AgentUi, Answer, ChildItem, ChildRun, ChildState, ChildUsage, Children, Delegation, Purpose,
    ToolDetail, publication_choices, show_items,
};

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
/// Stored with a `subagent` call's result: its children (`Children`), as they ended.
const SUBAGENTS: &str = "_scramjet_subagents";
/// Stored, as `true`, with Scramjet's status check, a user message the user never typed, so it
/// is neither shown nor recalled.
pub(super) const PROBE: &str = "_scramjet_probe";
/// Items of the conversation that are only shown, never sent to a model, as Scramjet keeps its
/// failed replies and `!!` output out of the context: how a turn stopped (`turn_stopped`, with
/// the `error` unless it was cancelled) and a `!!` command (`bang_excluded`, with its `command`
/// and the `output` shown).
const SHOWN_ONLY: [&str; 2] = ["turn_stopped", "bang_excluded"];

fn shown_only(item: &Value) -> bool {
    (item["type"].as_str()).is_some_and(|kind| SHOWN_ONLY.contains(&kind))
}

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
    /// The directories of the installed command sets, in their order.
    pub command_sets: Vec<PathBuf>,
    /// The skill files and directories the settings list, besides those found by default.
    pub skills: Vec<PathBuf>,
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
    skills: Skills,
    /// The command the last command prompt ran, until it is reported or left.
    lifecycle: status::Lifecycle,
    /// The command the last turn reported completed, until its next step is routed.
    completed: Option<Completed>,
    /// The commands delegated in this turn, with their arguments.
    delegated: Vec<(String, String)>,
    /// Scramjet's global `autonomy.yaml`, and what was last said about the files.
    autonomy: Option<PathBuf>,
    autonomy_diagnostics: Vec<String>,
    /// Where subagents are found: the user's agent directory and the command sets.
    agents_directory: Option<PathBuf>,
    sets: Sets,
    /// The context files the instructions have, outermost first.
    context_files: Vec<PathBuf>,
    session: Session,
    executor: tokio::runtime::Runtime,
    /// The current model's protocol and limits, from its name until `load_model_card`.
    info: ModelInfo,
    /// Whether the coding tools keep out of the credential stores (`sandbox::Hidden`).
    hide_credentials: bool,
    /// Whether a completed command's recommended next step runs without the user choosing it.
    autopilot: bool,
    pub usage: Usage,
}

/// What a session in `root` with the `installed` command sets and the `listed` skills would load
/// from them and from the agent and skill directories, and what is wrong there, loose project
/// agents included.
pub fn inventory(installed: &[PathBuf], listed: &[PathBuf], root: &Path) -> Inventory {
    let sets = Sets::for_project(installed, root);
    let commands = Commands::discover(&sets);
    let user = subagent::user_directory();
    let sources = subagent::Sources {
        user: user.as_deref(),
        sets: &sets,
    };
    let discovery = subagent::discover(&sources, root, subagent::Scope::Both);
    let mut warnings = commands.warnings().to_vec();
    warnings.extend(discovery.diagnostics);
    let agents = (discovery.agents.into_iter())
        .map(|agent| (agent.name, agent.path))
        .collect();
    Inventory {
        sets,
        commands,
        agents,
        warnings,
        skills: Skills::discover(listed, root),
    }
}

/// What a session has loaded, as `/settings` shows it, besides its commands and skills.
pub struct Installed {
    pub sets: Vec<Set>,
    /// The agents a subagent call can name, found as it finds them.
    pub agents: Vec<AgentFile>,
    /// The context files the instructions have, outermost first.
    pub context_files: Vec<PathBuf>,
    /// Scramjet's `autonomy.yaml` of the project, and the one for all projects.
    pub autonomy: (PathBuf, Option<PathBuf>),
    /// What is wrong in the sets, commands, agents, skills and `autonomy.yaml` files, each
    /// saying which: `scramjet: `, `agents: ` or `skills: ` first.
    pub problems: Vec<String>,
}

/// An agent a subagent call can name.
pub struct AgentFile {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub source: AgentSource,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentSource {
    /// The command set of this name, whose namespace the agent's name is in.
    Set(String),
    /// The user's agent directory.
    User,
    /// The project's `.scramjet/agents`.
    Project,
}

/// What `inventory` found.
pub struct Inventory {
    pub sets: Sets,
    pub commands: Commands,
    /// Every agent a subagent call could name, with its file.
    pub agents: Vec<(String, PathBuf)>,
    /// Why sets, commands and agent files were skipped.
    pub warnings: Vec<String>,
    pub skills: Skills,
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
                Session::create(&session::directory(&config.root)?, &config.root, None)?,
                Vec::new(),
                Vec::new(),
            ),
        };
        let context_tokens = session.context_tokens();
        let lifecycle = status::Lifecycle::restored(session.command());
        let autopilot = session.autopilot().unwrap_or(false);
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut agent = Self {
            instructions: String::new(),
            agents_directory: None,
            sets: Sets::default(),
            context_files: Vec::new(),
            commands: Commands::default(),
            skills: Skills::default(),
            lifecycle,
            completed: None,
            delegated: Vec::new(),
            autonomy: autonomy::global_path(),
            autonomy_diagnostics: Vec::new(),
            info: ModelInfo::guess(&config.model),
            hide_credentials: false,
            autopilot,
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
        agent.load();
        agent.answer_interrupted_calls()?;
        Ok(agent)
    }

    /// `/reload`: finds the command sets, their commands and agents, the skills and the context
    /// files again, as at start, with the settings' `tools`, `command_sets` and `skills`, as
    /// Scramjet's reload does. The conversation and the command running stay; what is wrong in
    /// `autonomy.yaml` is said again at the next routing.
    pub fn reload(&mut self, tools: Vec<String>, command_sets: Vec<PathBuf>, skills: Vec<PathBuf>) {
        self.config.tools = tools;
        self.config.command_sets = command_sets;
        self.config.skills = skills;
        self.autonomy_diagnostics.clear();
        self.load();
    }

    fn load(&mut self) {
        let config = &self.config;
        self.sets = Sets::for_project(&config.command_sets, &config.root);
        self.commands = Commands::discover(&self.sets);
        self.skills = Skills::discover(&config.skills, &config.root);
        self.agents_directory = subagent::user_directory();
        self.instruct();
    }

    fn instruct(&mut self) {
        let context = instructions::context_files(&self.config.root);
        self.context_files = context.into_iter().map(|(path, _)| path).collect();
        let agents = subagent::registered(&self.sets, &mut Vec::new());
        self.instructions = instructions::agent(
            &self.config.root,
            &self.config.tools,
            &self.commands,
            &agents,
            &self.skills,
        );
    }

    /// The coding tools of the requests that follow, as the settings' `tools`; the instructions
    /// are written again for them, the command sets and skills kept as they are.
    pub fn set_tools(&mut self, tools: Vec<String>) {
        self.config.tools = tools;
        self.instruct();
    }

    /// The sessions saved next to the current one that have a conversation, the current one
    /// included, newest first. Scramjet writes no session file before the first reply, so it
    /// lists no empty ones.
    pub fn sessions(&self) -> Result<Vec<SessionInfo>, AgentError> {
        let directory = self.session.path().parent().unwrap_or(Path::new("."));
        let mut sessions = Session::list(directory)?;
        sessions.retain(|session| session.item_count > 0);
        Ok(sessions)
    }

    /// Names the saved session `path`, the current one too, as `set_session_name` does, as
    /// Scramjet's `/resume` renames one.
    pub fn rename_session(&mut self, path: &Path, name: &str) -> Result<(), AgentError> {
        match self.session.is_file(path) {
            true => self.set_session_name(name),
            false => Ok(Session::name_file(path, name)?),
        }
    }

    /// Deletes the saved session `path`, other than the current one, as Scramjet's `/resume`
    /// deletes one: to the trash with the `trash` command when there is one, else for good.
    pub fn delete_session(&self, path: &Path) -> Result<Removal, AgentError> {
        if self.session.is_file(path) {
            return Err(AgentError::Session(std::io::Error::other(
                "cannot delete the current session",
            )));
        }
        Ok(Session::delete(path)?)
    }

    /// Ends the agent, removing its session's file when this process created it and only model
    /// and effort changes went in, as when a start that was to resume a session resumes none.
    pub fn discard_unused_session(self) -> Result<(), AgentError> {
        Ok(self.session.discard_if_unused()?)
    }

    /// Continues in `session`; the one before goes, its file too when it stayed unused
    /// (`Session::discard_if_unused`), as Scramjet would not have written it. That file is left
    /// when it cannot be removed, as all were before.
    fn replace_session(&mut self, session: Session) {
        let left = std::mem::replace(&mut self.session, session);
        let _ = left.discard_if_unused();
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
        self.autopilot = session.autopilot().unwrap_or(self.autopilot);
        self.replace_session(session);
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
        let mut session = Session::create(directory, &self.config.root, None)?;
        session.record_model(&self.config.model)?;
        if let Some(effort) = &self.config.effort {
            session.record_effort(effort)?;
        }
        self.replace_session(session);
        self.conversation.clear();
        self.prompts.clear();
        self.usage = Usage::default();
        self.restore_command();
        Ok(())
    }

    /// The commands this session's prompts can run, as discovered at start or `reload`.
    pub fn commands(&self) -> &Commands {
        &self.commands
    }

    /// The skills found at start or `reload`, which `/skill:<name>` runs.
    pub fn skills(&self) -> &Skills {
        &self.skills
    }

    /// The command sets, agents, context files and `autonomy.yaml` files of the session, and
    /// what is wrong there, its commands and skills included. The agents and the files are read
    /// afresh, as a subagent call and the next steps read them; the rest is what start or
    /// `reload` loaded.
    pub fn installed(&self) -> Installed {
        let sources = subagent::Sources {
            user: self.agents_directory.as_deref(),
            sets: &self.sets,
        };
        let discovery = subagent::discover(&sources, &self.config.root, subagent::Scope::Both);
        let agents = (discovery.agents.into_iter())
            .map(|agent| AgentFile {
                source: match self.sets.owner(&agent.name) {
                    Some(set) => AgentSource::Set(set.name.clone()),
                    None if agent.source == "user" => AgentSource::User,
                    None => AgentSource::Project,
                },
                name: agent.name,
                description: agent.description,
                path: agent.path,
            })
            .collect();
        let (global, root) = (self.autonomy.as_deref(), &self.config.root);
        let edges = autonomy::Edges::load(global, root, &self.commands);
        let mut problems: Vec<String> = (self.commands.warnings().iter())
            .map(|warning| format!("scramjet: {warning}"))
            .collect();
        problems.extend(discovery.diagnostics.iter().map(|d| format!("agents: {d}")));
        problems.extend(
            self.skills
                .warnings()
                .iter()
                .map(|w| format!("skills: {w}")),
        );
        problems.extend(edges.diagnostics);
        Installed {
            sets: self.sets.list().to_vec(),
            agents,
            context_files: self.context_files.clone(),
            autonomy: (autonomy::project_path(root), self.autonomy.clone()),
            problems,
        }
    }

    /// The text `prompt` sends when it runs a command or a skill, `None` when it is sent as typed.
    pub fn expand(&self, prompt: &str) -> Option<String> {
        (self.commands.expand(prompt)).or_else(|| self.skills.expand(prompt)?.ok())
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

    /// The effort levels the model's card lists, in its order; none when it lists none.
    pub fn effort_levels(&self) -> &[String] {
        &self.info.levels
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

    /// Whether `/autopilot` is on. Off at start, it is what the session's latest toggle on the
    /// conversation's path set, as Scramjet restores it; a session or branch without one keeps
    /// the value it had.
    pub fn autopilot(&self) -> bool {
        self.autopilot
    }

    /// The next steps of the commands that have them, with what the `autonomy.yaml` files say
    /// of each, as `/settings` shows them.
    pub fn next_steps(&self) -> NextSteps {
        NextSteps::load(self.autonomy.as_deref(), &self.config.root, &self.commands)
    }

    /// Sets what follows `source` when it recommends `target` (`None`: any other command): it
    /// runs at once or waits for the user (`edge`), for this project or all projects, or, with
    /// no choice, autopilot decides. Used from the next routing on.
    pub fn set_next_step(
        &self,
        source: &str,
        target: Option<&str>,
        choice: Option<(Edge, Scope)>,
    ) -> Result<(), String> {
        let (global, root) = (self.autonomy.as_deref(), &self.config.root);
        autonomy::choose(global, root, (source, target), choice)
    }

    /// Saves what the command sets suggest for the next steps without a setting of their own,
    /// for this project or all projects; how many it saved. Used from the next routing on.
    pub fn apply_suggestions(&self, scope: Scope) -> Result<usize, String> {
        let steps = self.next_steps();
        let (global, root) = (self.autonomy.as_deref(), &self.config.root);
        autonomy::apply(global, root, &steps.suggested(), scope)
    }

    /// The publications of the commands whose policy can approve them, with what the user and
    /// the commands' sets say of each, as `/settings` shows them.
    pub fn approvals(&self) -> Approvals {
        Approvals::load(self.autonomy.as_deref(), &self.commands)
    }

    /// Sets whether `command`'s publications with `tool` in the project's repository ask first,
    /// for all projects; with no choice, its set decides. Used from the next publication on.
    pub fn set_approval(
        &self,
        command: &str,
        tool: &str,
        choice: Option<Publication>,
    ) -> Result<(), String> {
        autonomy::choose_publication(self.autonomy.as_deref(), command, tool, choice)
    }

    /// `/autopilot on` or `off`, saved in the session.
    pub fn set_autopilot(&mut self, on: bool) -> Result<(), AgentError> {
        self.session.record_autopilot(on)?;
        self.autopilot = on;
        Ok(())
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
