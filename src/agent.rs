//! A coding agent that talks to its models' providers (Responses or Messages) and works in
//! a project.

mod approvals;
mod attached;
mod autonomy;
mod batch;
mod calls;
mod client;
pub mod codex;
mod commands;
mod compaction;
mod completions;
pub mod copilot;
mod delegate;
mod forge;
mod image;
mod instructions;
mod listed;
pub mod login;
mod mcp;
mod messages;
mod pricing;
mod problem;
mod providers;
mod publish;
mod question;
mod rates;
mod request;
mod sandbox;
mod session;
mod shell;
mod skills;
mod status;
mod subagent;
mod suggest;
mod switch;
mod tools;
mod tree;
mod turn;
mod ui;

pub use attached::Attached;
pub use autonomy::{
    Approval, Approvals, Edge, NextStep, NextSteps, Publication, Scope, Setting, Suggestion,
};
pub use client::StreamProgress;
pub use commands::{Command, Commands, Origin, Set, Sets, sets_in};
pub use compaction::{AutoCompaction, Compaction};
pub use delegate::{NAME as DELEGATE, SCOPE_WARNING as DELEGATE_SCOPE_WARNING};
pub use listed::Listing;
pub use mcp::{
    CALL as MCP_CALL, SEARCH as MCP_SEARCH, Server as McpServer, State as McpState,
    Transport as McpTransport,
};
pub use problem::{Problem, ProblemFile};
pub use providers::{KeyFrom, Providers};
pub use publish::TOOLS as PUBLICATION_TOOLS;
pub use question::Choice;
pub use session::{
    Branch, Found, Label, Removal, Session, SessionInfo, directory as session_directory,
    read_branch, state_directory,
};
pub use shell::Shell;
pub use skills::{Skill, SkillOptions, Skills};
pub use status::{Completed, Next, Routing, Source, Step};
pub use subagent::agent_directory;
pub use suggest::Suggested;
pub use tools::{
    CODING_TOOLS, DEFAULT_TOOLS, HARNESS_TOOLS, READ_MORE, search_program, sequence_diff,
    shortened, terminal_line, write_replacing,
};
pub use tree::{Navigated, SessionTree, TreeEntry, TreeItem};
pub use ui::{
    AgentUi, Answer, ChildItem, ChildRun, ChildState, ChildUsage, Children, CompactRead,
    Delegation, Ending, Purpose, ToolDetail, UserCommand, publication_choices, show_items,
};

use providers::ModelInfo;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Guards against a model that never stops calling tools.
const MAX_STEPS: usize = 200;
/// Stored with each response item: the model that produced it. Never sent to the provider.
const PRODUCER: &str = "_scramjet_model";
/// Stored with a response's last item when the response reported it: the context size, so a
/// resumed session knows how full the context is before its first reply, as Scramjet reads it
/// from saved usage. Never sent.
const CONTEXT: &str = "_scramjet_context_tokens";
/// Stored with a response's last item when the response reported its usage: the tokens it counted
/// (`input` neither read from the cache nor written to it, `output`, `cache_read`, `cache_write`)
/// and, when its model's prices say, its `cost` in dollars, which `/session` and the footer
/// sum as Scramjet sums its assistant messages' usage. Never sent.
const USAGE: &str = "_scramjet_usage";
/// Stored with a user message whose text is not its prompt as typed (a command's or skill's
/// expansion, a `!command`'s output, `@` files' contents): the prompt as typed, which is shown and
/// recalled. Never sent to the provider.
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
/// The response items a model is sent, besides user messages; replies hold only these. Other
/// items of the conversation are only shown, as Scramjet keeps its failed replies and `!!` output
/// out of the context: how a turn stopped (`turn_stopped`, with the `error` unless it was
/// cancelled) and a `!!` command (`bang_excluded`, with its `command` and the `output` shown).
/// So is an entry of a type this build does not know, as Scramjet's `convertToLlm` drops one.
const SENT: [&str; 4] = [
    "message",
    "reasoning",
    "function_call",
    "function_call_output",
];

fn sent(item: &Value) -> bool {
    match item["type"].as_str() {
        Some(kind) => SENT.contains(&kind),
        // A prompt, `!` command, summary or status check: a user message without a type.
        None => item.get("type").is_none() && item["role"] == "user",
    }
}

/// Ends what must not go on, or be cut short, when the process exits on a signal: kills the
/// commands running and waits for lines being written to a session file. Neither starts again.
pub fn end() {
    tools::end_commands();
    session::end_writes();
}

/// The credential files and directories that exist and are hidden from the tools that hide them:
/// the common ones and those `providers`' files name, by their real paths.
pub fn credential_stores(providers: &Providers) -> Vec<PathBuf> {
    sandbox::Hidden::listed(providers.credential_files())
}

pub struct AgentConfig {
    /// The providers requests go to, by their models.
    pub providers: Providers,
    /// The model's reference (`provider/id`), or its id, which the providers resolve.
    pub model: String,
    pub root: PathBuf,
    pub effort: Option<String>,
    pub max_output_tokens: Option<u64>,
    /// Ask before each bash command and each write or edit outside the project.
    pub ask: bool,
    /// Extra attempts for a model request that failed transiently (rate limit, overload, network).
    pub retries: u32,
    /// The coding tools the model has, among `CODING_TOOLS`: the settings' `tools`.
    pub tools: Vec<String>,
    /// `--tools`, `--no-tools` or `--no-builtin-tools`: the only tools the model has in this run,
    /// coding tools in place of `tools` and the harness's own, as Scramjet's allowlist.
    pub only_tools: Option<Vec<String>>,
    /// The directories of the installed command sets, in their order.
    pub command_sets: Vec<PathBuf>,
    /// The skill files and directories the settings list, besides those found by default.
    pub skills: Vec<PathBuf>,
    /// `--skill` and `--no-skills`.
    pub skill_options: SkillOptions,
    pub prompt: SystemPrompt,
    /// The MCP servers the settings list (`mcpServers`).
    pub mcp_servers: Vec<McpServer>,
}

/// What the command line changes in the agent's instructions, as Scramjet's options do. A text
/// that is a file's path stands for the file's contents, read again by `/reload`.
#[derive(Clone, Debug, Default)]
pub struct SystemPrompt {
    /// `--system-prompt`: in place of the opening (the tools and guidelines), and of the
    /// `SYSTEM.md` files that would replace it.
    pub system: Option<String>,
    /// `--append-system-prompt`: after it, in place of the `APPEND_SYSTEM.md` files.
    pub append: Vec<String>,
    /// `--no-context-files`: without the `AGENTS.md` and `CLAUDE.md` files.
    pub no_context_files: bool,
}

/// The reasoning effort levels a model takes, and the current one, as Shift+Tab cycles
/// through them.
#[derive(Clone, Debug, Default)]
pub struct EffortCycle {
    levels: Vec<String>,
    current: Option<String>,
}

impl EffortCycle {
    /// Moves to the level after the current one, wrapping around as Scramjet's thinking level
    /// cycle does, and returns it; `None` when it takes no levels.
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

/// A model to run a next step with, as its provider describes it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelOption {
    pub id: String,
    /// Its display name.
    pub name: Option<String>,
    /// The provider offering it; `None` for a model none offers.
    pub provider: Option<String>,
    pub reasoning: bool,
    /// The effort levels it takes, in order; empty: any.
    pub levels: Vec<String>,
    /// The context window, input and output tokens.
    pub window: Option<u64>,
    /// The most output tokens a response may have.
    pub output: Option<u64>,
    pub images: bool,
    /// The median rate its last replies were written at, in tokens a second (`rates`).
    pub rate: Option<f64>,
}

impl ModelOption {
    fn new(
        id: String,
        name: Option<String>,
        provider: Option<String>,
        info: &ModelInfo,
        rate: Option<f64>,
    ) -> Self {
        Self {
            id,
            name,
            provider,
            reasoning: info.reasoning,
            levels: info.levels.clone(),
            window: info.window,
            output: info.output,
            images: info.images,
            rate,
        }
    }

    /// `provider/id`, as the agent, its sessions and settings name it; its id for a model no
    /// provider offers.
    pub fn reference(&self) -> String {
        match &self.provider {
            Some(provider) => providers::reference(provider, &self.id),
            None => self.id.clone(),
        }
    }

    /// The effort its requests have with `set` set, as `Agent::effort` fits it; `None` when it
    /// does not reason or none is set.
    pub fn effort(&self, set: Option<&str>) -> Option<String> {
        let effort = providers::fit(&self.levels, set?);
        self.reasoning.then(|| effort.to_owned())
    }

    /// Shift+Tab's: the level after `effort(set)` among its levels, wrapping around; `None` when
    /// it takes fewer than two.
    pub fn next_effort(&self, set: Option<&str>) -> Option<String> {
        if self.levels.len() < 2 {
            return None;
        }
        EffortCycle {
            levels: self.levels.clone(),
            current: self.effort(set),
        }
        .advance()
    }
}

pub enum SessionChoice {
    New,
    Continue,
    Open(PathBuf),
    /// A new session of the project with all of this session file's (`Session::fork_from`).
    Fork(PathBuf),
    /// A new session saved nowhere (`--no-session`): it and those made from it (`/new`,
    /// `/fork`, …) are kept in a directory of their own until the agent ends.
    Unsaved,
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
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    /// Dollars, of the responses whose cost is known.
    pub cost: f64,
    /// Whether a response that reported its usage has no known cost: no prices apply.
    pub unpriced: bool,
    /// The input and output tokens of the last response, when it reported them: how full the
    /// context was, as Scramjet's footer shows it.
    pub context_tokens: Option<u64>,
}

impl Usage {
    /// Counts a response's tokens and cost.
    fn count(&mut self, reply: &client::Reply) {
        let Some(usage) = &reply.usage else {
            return;
        };
        self.input_tokens += usage.input_tokens.unwrap_or(0);
        self.cached_input_tokens += usage.cached_input_tokens.unwrap_or(0);
        self.cache_write_tokens += usage.cache_write_tokens.unwrap_or(0);
        self.output_tokens += usage.output_tokens.unwrap_or(0);
        match reply.cost {
            Some(cost) => self.cost += cost,
            None => self.unpriced = true,
        }
    }

    /// Adds another agent's requests, tokens and cost, not its context.
    fn add(&mut self, other: &Usage) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.output_tokens += other.output_tokens;
        self.cost += other.cost;
        self.unpriced |= other.unpriced;
    }
}

/// The tokens responses saved with their items (`USAGE`), added up, and what they and the
/// subagents saved with their calls' results (`SUBAGENTS`) cost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tokens {
    /// Input tokens neither read from the cache nor written to it.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Dollars, of the responses whose cost is known.
    pub cost: f64,
    /// Whether some response's cost is not known (saved without one, as before costs were).
    pub unpriced: bool,
}

impl Tokens {
    /// Adds what `item` saved, if it is a response's last item or a `subagent` call's result.
    fn add(&mut self, item: &Value) {
        if let Some(usage) = item.get(USAGE) {
            self.input += usage["input"].as_u64().unwrap_or(0);
            self.output += usage["output"].as_u64().unwrap_or(0);
            self.cache_read += usage["cache_read"].as_u64().unwrap_or(0);
            self.cache_write += usage["cache_write"].as_u64().unwrap_or(0);
            self.cost(&usage["cost"]);
        }
        let runs = item[SUBAGENTS]["runs"].as_array().into_iter().flatten();
        for usage in runs.map(|run| &run["usage"]) {
            if usage["turns"].as_u64().unwrap_or(0) > 0 {
                self.cost(&usage["cost"]);
            }
        }
    }

    fn cost(&mut self, cost: &Value) {
        match cost.as_f64() {
            Some(cost) => self.cost += cost,
            None => self.unpriced = true,
        }
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
    pub tokens: Tokens,
}

impl Stats {
    /// The counts over `items`, as Scramjet's session stats count messages: assistant messages
    /// are the runs of model output between prompts and tool results, as Scramjet has one message
    /// per response; the usage is what each response saved. Entries that are not conversation
    /// items, such as model changes, are not counted.
    pub fn of(items: &[Value]) -> Self {
        let mut stats = Stats::default();
        let mut other = 0;
        let mut in_reply = false;
        for item in items {
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
            stats.tokens.add(item);
        }
        stats.total = stats.user + stats.assistant + stats.tool_results + other;
        stats
    }
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
    /// The steps the agent suggested in the last turn, outside a command, for the user to choose
    /// from once it ends.
    suggestion: suggest::Pending,
    /// The commands delegated in this turn, with their arguments.
    delegated: Vec<(String, String)>,
    /// Files the next prompt sends.
    attached: Option<Attached>,
    /// Scramjet's global `autonomy.yaml`, and what was last said about the files.
    autonomy: Option<PathBuf>,
    autonomy_diagnostics: Vec<Problem>,
    /// Where subagents are found: the user's agent directory and the command sets.
    agents_directory: Option<PathBuf>,
    /// Where the user's approvals of project agents are remembered.
    approvals: Option<PathBuf>,
    /// Where replies' output rates are kept (`rates::path`).
    rates: Option<PathBuf>,
    sets: Sets,
    /// Scramjet's agent directory, whose context files, `SYSTEM.md` and `APPEND_SYSTEM.md` the
    /// instructions take.
    agent_directory: Option<PathBuf>,
    /// The context files the instructions have, outermost first.
    context_files: Vec<PathBuf>,
    /// The files read in place of their opening (`true`) or after it, and those that could not
    /// be read.
    prompt_files: (Vec<(PathBuf, bool)>, Vec<Problem>),
    session: Session,
    /// With `SessionChoice::Unsaved`, where every session of the run is, removed with the agent.
    unsaved: Option<session::Unsaved>,
    executor: tokio::runtime::Runtime,
    /// What its provider says of the current model, and where its requests go; as its name
    /// suggests when no provider offers it.
    info: ModelInfo,
    /// Its provider, when the models are those of several (`provider`).
    provider: Option<String>,
    /// The names and providers of the providers' models, for the Model Identity; kept when they
    /// cannot be listed again.
    identities: HashMap<String, instructions::Identity>,
    /// The models of the user's scope (`set_model_scope`); empty: all.
    model_scope: Vec<String>,
    /// Whether the coding tools keep out of the credential stores (`sandbox::Hidden`).
    hide_credentials: bool,
    /// Whether a completed command's recommended next step runs without the user choosing it.
    autopilot: bool,
    /// The MCP servers, while the model is offered their tools.
    mcp: Option<Arc<mcp::Servers>>,
    /// A reply's calls that a pause of the turn left running (`AgentError::Paused`).
    calls: Option<calls::Calls>,
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
        skills: Skills::discover(listed, &SkillOptions::default(), root),
    }
}

/// What a session has loaded, as `/settings` shows it, besides its commands and skills.
pub struct Installed {
    pub sets: Vec<Set>,
    /// The agents a subagent call can name, found as it finds them.
    pub agents: Vec<AgentFile>,
    /// The context files the instructions have, outermost first.
    pub context_files: Vec<PathBuf>,
    /// The files read for the instructions in place of their opening (`true`) or after it.
    pub prompt_files: Vec<(PathBuf, bool)>,
    /// Scramjet's `autonomy.yaml` of the project, and the one for all projects.
    pub autonomy: (PathBuf, Option<PathBuf>),
    /// What is wrong in the sets, commands, agents, skills, instructions files and
    /// `autonomy.yaml` files, each saying which: `scramjet: `, `agents: `, `skills: ` or
    /// `instructions: ` first.
    pub problems: Vec<Problem>,
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
    /// The project's `.scramjet/agents`, with what the user's approval of them says of it.
    Project(ProjectApproval),
}

/// Whether the user's approval of running the project's agents covers an agent's file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProjectApproval {
    /// As the file is now.
    Approved,
    /// As the file was before it changed.
    Changed,
    NotApproved,
}

/// What `inventory` found.
pub struct Inventory {
    pub sets: Sets,
    pub commands: Commands,
    /// Every agent a subagent call could name, with its file.
    pub agents: Vec<(String, PathBuf)>,
    /// Why sets, commands and agent files were skipped.
    pub warnings: Vec<Problem>,
    pub skills: Skills,
}

impl Agent {
    pub fn start(mut config: AgentConfig, choice: SessionChoice) -> Result<Self, AgentError> {
        config.model = config.providers.reference(&config.model);
        let directory = || session::directory(&config.root);
        let mut unsaved = None;
        let opened = match choice {
            SessionChoice::New => None,
            SessionChoice::Unsaved => {
                unsaved = Some(session::Unsaved::create()?);
                None
            }
            SessionChoice::Continue => Session::latest(&directory()?)?.map(|path| (path, false)),
            SessionChoice::Open(path) => Some((path, false)),
            SessionChoice::Fork(source) => Some((
                Session::fork_from(&directory()?, &config.root, &source)?,
                true,
            )),
        };
        let (session, conversation, prompts) = match opened {
            Some((path, forked)) => Session::open(&path).inspect_err(|_| {
                if forked {
                    let _ = std::fs::remove_file(&path);
                }
            })?,
            None => {
                let directory = match &unsaved {
                    Some(unsaved) => unsaved.path().to_owned(),
                    None => directory()?,
                };
                let created = Session::create(&directory, &config.root, None)?;
                (created, Vec::new(), Vec::new())
            }
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
            approvals: approvals::path(),
            rates: rates::path(),
            sets: Sets::default(),
            agent_directory: None,
            context_files: Vec::new(),
            prompt_files: Default::default(),
            commands: Commands::default(),
            skills: Skills::default(),
            lifecycle,
            completed: None,
            suggestion: suggest::Pending::Nothing,
            delegated: Vec::new(),
            attached: None,
            autonomy: autonomy::global_path(),
            autonomy_diagnostics: Vec::new(),
            info: config.providers.info(&config.model),
            provider: None,
            identities: HashMap::new(),
            model_scope: Vec::new(),
            hide_credentials: false,
            autopilot,
            mcp: None,
            calls: None,
            config,
            conversation,
            prompts,
            session,
            unsaved,
            executor,
            usage: Usage {
                context_tokens,
                ..Usage::default()
            },
        };
        agent.load();
        agent.start_mcp();
        agent.answer_interrupted_calls()?;
        Ok(agent)
    }

    /// `/reload`: finds the command sets, their commands and agents, the skills and the context
    /// files again, as at start, with the settings' `tools`, `command_sets` and `skills`, as
    /// Scramjet's reload does, and starts the MCP servers whose entries changed or that ended. The
    /// conversation and the command running stay; what is wrong in `autonomy.yaml` is said again
    /// at the next routing.
    pub fn reload(
        &mut self,
        tools: Vec<String>,
        command_sets: Vec<PathBuf>,
        skills: Vec<PathBuf>,
        mcp_servers: Vec<McpServer>,
    ) {
        self.config.tools = tools;
        self.config.command_sets = command_sets;
        self.config.skills = skills;
        self.config.mcp_servers = mcp_servers;
        self.autonomy_diagnostics.clear();
        self.load();
        self.start_mcp();
    }

    /// Starts the MCP servers when the model is offered their tools; servers already running as
    /// configured keep running, the others, also those that ended, start again.
    fn start_mcp(&mut self) {
        let offered = self.offered();
        let (wanted, root) = (&self.config.mcp_servers, &self.config.root);
        if wanted.is_empty() || !mcp::TOOLS.iter().any(|tool| offered.contains(tool)) {
            self.mcp = None;
            return;
        }
        // Calls still running keep the servers they had.
        match self.mcp.as_mut().and_then(Arc::get_mut) {
            Some(running) => running.update(wanted, root),
            None => self.mcp = Some(Arc::new(mcp::Servers::start(wanted, root))),
        }
    }

    /// The MCP servers and how each is, in the settings' order.
    pub fn mcp_servers(&self) -> Vec<(McpServer, McpState)> {
        let states = self
            .mcp
            .as_deref()
            .map(mcp::Servers::states)
            .unwrap_or_default();
        (states.into_iter())
            .map(|(server, state)| (server.clone(), state))
            .collect()
    }

    /// Waits for the MCP servers still starting, saying so, and says which could not start, each
    /// once, before a request: the instructions list them as they are then.
    fn settle_mcp(&self, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> Result<(), AgentError> {
        let Some(mcp) = &self.mcp else {
            return Ok(());
        };
        let starting = mcp.starting();
        if !starting.is_empty() {
            ui.notice(&format!(
                "Waiting for MCP servers to start: {}",
                starting.join(", ")
            ));
            mcp.settle(cancel);
            if cancel.load(Ordering::Relaxed) {
                return Err(AgentError::Cancelled);
            }
        }
        for (name, why) in mcp.unreported_failures() {
            ui.notice(&format!("MCP server {name} is not available: {why}"));
        }
        Ok(())
    }

    fn load(&mut self) {
        let config = &self.config;
        self.sets = Sets::for_project(&config.command_sets, &config.root);
        self.commands = Commands::discover(&self.sets);
        self.skills = Skills::discover(&config.skills, &config.skill_options, &config.root);
        self.agents_directory = subagent::user_directory();
        self.agent_directory = subagent::agent_directory();
        self.instruct();
    }

    fn instruct(&mut self) {
        let config = &self.config;
        let directory = self.agent_directory.as_deref();
        let loaded = instructions::load(&config.prompt, &config.root, directory);
        let agents = subagent::registered(&self.sets, &mut Vec::new());
        let mcp = !config.mcp_servers.is_empty();
        self.instructions = instructions::agent(
            &config.root,
            &tools::offered(&config.tools, config.only_tools.as_deref(), mcp),
            &self.commands,
            &agents,
            &self.skills,
            &loaded,
        );
        self.context_files = (loaded.context.into_iter()).map(|(path, _)| path).collect();
        self.prompt_files = (loaded.files, loaded.problems);
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
    /// and effort changes went in, as Scramjet writes no session before the first reply.
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

    /// Whether the session is saved: not with `SessionChoice::Unsaved`.
    pub fn session_saved(&self) -> bool {
        self.unsaved.is_none()
    }

    /// With `SessionChoice::Unsaved`, the directory every session of the run is in.
    pub fn unsaved_directory(&self) -> Option<&Path> {
        self.unsaved.as_ref().map(session::Unsaved::path)
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

    /// The counts over the current conversation, as `/session` shows them.
    pub fn stats(&self) -> Stats {
        Stats::of(&self.conversation)
    }

    /// The tokens of every response in the session, on every branch and including those a
    /// compaction summarized, as Scramjet's footer adds them up.
    pub fn session_tokens(&self) -> Tokens {
        self.session.tokens()
    }

    /// How full the context is: the last response's tokens, none for an empty conversation, and
    /// `None` when unknown (no response reported them since the last compaction).
    pub fn context_tokens(&self) -> Option<u64> {
        match self.conversation.is_empty() {
            true => Some(0),
            false => self.usage.context_tokens,
        }
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
            // As in Scramjet, a fork without a conversation keeps the current model, and is not
            // written before the first reply (its lines are the current session's).
            if conversation.is_empty() {
                session.record_model(&self.config.model)?;
                session.forked_unused();
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

    /// Continues in a new session saved next to the current one with the conversation up to
    /// where it is and the session's name, as Scramjet's `/clone` duplicates a session; the
    /// current session stays as it is. Returns false, changing nothing, before any conversation
    /// (Scramjet's clones its model change then). On failure nothing changes.
    pub fn clone_session(&mut self) -> Result<bool, AgentError> {
        if self.conversation.is_empty() {
            return Ok(false);
        }
        let path = self.session.clone_at_leaf(&self.config.root)?;
        match Session::open(&path) {
            Ok(opened) => self.switch_session(opened)?,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error.into());
            }
        }
        Ok(true)
    }

    /// The conversation on the session's current branch, from its start, as `/export` shows it.
    pub fn branch(&self) -> Result<Branch, AgentError> {
        Ok(self.session.branch()?)
    }

    /// Writes the session's current branch to `path` as a session file of its own, as Scramjet's
    /// `/export` writes a `.jsonl` (`Session::export`). Returns false, writing nothing, before any
    /// conversation.
    pub fn export_session(&self, path: &Path) -> Result<bool, AgentError> {
        if self.conversation.is_empty() {
            return Ok(false);
        }
        self.session.export(path)?;
        Ok(true)
    }

    /// Continues in a copy of the session file `path`, saved next to the current session, as
    /// Scramjet's `/import` does; the current session stays as it is. On failure nothing changes.
    pub fn import_session(&mut self, path: &Path) -> Result<(), AgentError> {
        let directory = self.session.path().parent().unwrap_or(Path::new("."));
        let copy = Session::import(directory, path)?;
        match Session::open(&copy) {
            Ok(opened) => self.switch_session(opened),
            Err(error) => {
                let _ = std::fs::remove_file(&copy);
                Err(error.into())
            }
        }
    }

    /// The instructions the model's next request starts with: those of the session, then the
    /// current model's identity and the environment's facts.
    pub fn instructions(&self) -> String {
        let mut contributors: Vec<String> = Vec::new();
        for contributor in self.session.contributors() {
            let contributor = self.config.providers.reference(contributor);
            if !contributors.contains(&contributor) {
                contributors.push(contributor);
            }
        }
        let model = &self.config.model;
        let servers = self.mcp.as_deref().map(mcp::Servers::section);
        self.instructions.clone()
            + &servers.unwrap_or_default()
            + &instructions::identity(model, &contributors, &self.identities)
            + &instructions::environment(
                &self.config.root,
                self.session_saved().then(|| self.session.path()),
            )
    }

    /// The definitions of the tools the model's requests offer.
    pub fn tool_definitions(&self) -> Value {
        tools::definitions(&self.offered())
    }

    /// The tools the model has, coding tools first.
    fn offered(&self) -> Vec<&'static str> {
        let config = &self.config;
        let mcp = !config.mcp_servers.is_empty();
        tools::offered(&config.tools, config.only_tools.as_deref(), mcp)
    }

    fn offers(&self, tool: &str) -> bool {
        self.offered().contains(&tool)
    }

    /// The coding tools of this run when the command line chose them, in place of the settings'.
    pub fn command_line_tools(&self) -> Option<Vec<&'static str>> {
        let only = self.config.only_tools.as_ref()?;
        let named = |tool: &&str| only.iter().any(|name| name == tool);
        Some(CODING_TOOLS.into_iter().filter(named).collect())
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
            self.info = self.config.providers.info(&model);
            self.config.model = self.config.providers.reference(&model);
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
        let approvals = approvals::Approvals::read(self.approvals.as_deref());
        let agents = (discovery.agents.into_iter())
            .map(|agent| AgentFile {
                source: match self.sets.owner(&agent.name) {
                    Some(set) => AgentSource::Set(set.name.clone()),
                    None if agent.source == "user" => AgentSource::User,
                    None if approvals.approved(&agent) => {
                        AgentSource::Project(ProjectApproval::Approved)
                    }
                    None if approvals.known(&agent) => {
                        AgentSource::Project(ProjectApproval::Changed)
                    }
                    None => AgentSource::Project(ProjectApproval::NotApproved),
                },
                name: agent.name,
                description: agent.description,
                path: agent.path,
            })
            .collect();
        let (global, root) = (self.autonomy.as_deref(), &self.config.root);
        let edges = autonomy::Edges::load(global, root, &self.commands);
        let prefixed = |problems: &[Problem], prefix: &str| {
            (problems.iter())
                .map(|problem| problem.clone().prefixed(prefix))
                .collect::<Vec<_>>()
        };
        let mut problems = prefixed(self.commands.warnings(), "scramjet: ");
        problems.extend(prefixed(&discovery.diagnostics, "agents: "));
        problems.extend(prefixed(self.skills.warnings(), "skills: "));
        problems.extend(prefixed(&self.prompt_files.1, "instructions: "));
        problems.extend(edges.diagnostics);
        Installed {
            sets: self.sets.list().to_vec(),
            agents,
            context_files: self.context_files.clone(),
            prompt_files: self.prompt_files.0.clone(),
            autonomy: (autonomy::project_path(root), self.autonomy.clone()),
            problems,
        }
    }

    /// The text `prompt` sends when it runs a command or a skill, `None` when it is sent as typed.
    pub fn expand(&self, prompt: &str) -> Option<String> {
        (self.commands.expand(prompt)).or_else(|| self.skills.expand(prompt)?.ok())
    }

    /// Sends `files` with the next prompt `turn` runs, as Scramjet sends the command line's `@`
    /// files with its first message.
    pub fn attach(&mut self, files: Attached) {
        self.attached = Some(files);
    }

    /// The next prompt as shown and as sent, when files go with it.
    pub fn attached(&self, prompt: &str) -> Option<(String, String)> {
        let files = self.attached.as_ref()?;
        Some((files.typed(prompt), files.sent(prompt)))
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

    /// The current model's reference: `provider/id` when a provider offers it.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// The id the current model's requests name it by.
    pub fn model_id(&self) -> &str {
        &self.info.id
    }

    /// The provider the current model's requests go to; `None` when none offers it.
    pub fn model_provider(&self) -> Option<&str> {
        (self.info.route.as_ref()).map(|route| route.provider.as_str())
    }

    /// Whether a reply of `producer` came from the current model; a reply saved before models
    /// had references names its id, which resolves as `Providers::find` resolves it.
    fn produced(&self, producer: &str) -> bool {
        producer == self.config.model
            || self.config.providers.reference(producer) == self.config.model
    }

    /// Changes the model used for subsequent requests and records it in the session, so a resume
    /// continues with it; on failure nothing changes. `model` is a reference, or an id, which
    /// names the first provider's model of that id.
    pub fn set_model(&mut self, model: impl Into<String>) -> Result<(), AgentError> {
        let model: String = model.into();
        let model = self.config.providers.reference(&model);
        self.session.record_model(&model)?;
        self.info = self.config.providers.info(&model);
        self.provider = None;
        self.config.model = model;
        Ok(())
    }

    /// The reasoning effort requests ask for: the one set, fitted to the levels the model takes
    /// as Scramjet clamps a thinking level; `None` leaves it to the model.
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

    /// Whether the model reasons, so that an effort applies to it, as its provider says.
    pub fn reasons(&self) -> bool {
        self.info.reasoning
    }

    /// The effort levels the model takes, in order; none when it takes any.
    pub fn effort_levels(&self) -> &[String] {
        &self.info.levels
    }

    /// Shift+Tab: moves the reasoning effort to the next level the model takes, after the current
    /// one, wrapping around as Scramjet's thinking level cycle does, and records the change in
    /// the session. Returns the new level; `None` when it lists no levels, which keeps the
    /// effort.
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
            current: self.effort().map(str::to_owned),
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

    /// Forgets the user's approval of running the project's agents, so that the next subagent
    /// call naming one asks again; how many agent files it covered.
    pub fn forget_project_agents(&self) -> std::io::Result<usize> {
        let directory = subagent::project_directory(&self.config.root);
        match (&self.approvals, directory) {
            (Some(path), Some(directory)) => approvals::forget(path, &directory),
            _ => Ok(0),
        }
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

    /// The current model's context window, as its provider says, or `None` when unknown.
    pub fn context_window(&self) -> Option<u64> {
        self.info.window
    }

    /// The model the session last recorded: its latest switch or response item's producer.
    pub fn last_model(&self) -> Option<&str> {
        self.session.model()
    }

    /// The providers' models' references, or `None` when they list none.
    pub fn models(&self) -> Option<Vec<String>> {
        let models = self.config.providers.models();
        let references = models.iter().map(providers::Model::reference);
        (!models.is_empty()).then(|| references.collect())
    }

    /// The reference of the model a provider offers that `name`, a reference or an id, names.
    pub fn offered_model(&self, name: &str) -> Option<String> {
        self.config
            .providers
            .find(name)
            .map(providers::Model::reference)
    }

    /// The providers, as last read.
    pub fn providers(&self) -> &Providers {
        &self.config.providers
    }

    /// Forgets the list `provider` gave itself, so that it is asked again (`Providers::forget_list`).
    pub fn forget_list(&mut self, provider: &str) {
        self.config.providers.forget_list(provider);
    }

    /// Why a request for the current model cannot be sent: no provider offers it.
    fn unoffered(&self) -> String {
        let providers = &self.config.providers;
        let files: Vec<String> = (providers.files().iter())
            .map(|file| file.display().to_string())
            .collect();
        let mut message = format!(
            "no provider offers the model {}; providers come from {}",
            self.config.model,
            match files.is_empty() {
                true => "models.json files".to_owned(),
                false => files.join(", "),
            }
        );
        for problem in providers.problems() {
            message.push_str(&format!("\n{problem}"));
        }
        message
    }

    /// The providers' models, read again, each with its output rate, the current model first
    /// when none offers it.
    pub fn model_options(&mut self) -> Vec<ModelOption> {
        let mut options = self.listed_models().unwrap_or_default();
        if !options
            .iter()
            .any(|option| option.reference() == self.config.model)
        {
            let rate = rates::Rates::read(self.rates.as_deref()).median(&self.config.model);
            let id = self.config.model.clone();
            options.insert(0, ModelOption::new(id, None, None, &self.info, rate));
        }
        options
    }

    /// The models the user chose for Ctrl+P (`--models`, `enabledModels`, `/scoped-models`),
    /// which `switch_scramjet_model` lists after an unknown model as Scramjet's; empty: all.
    pub fn set_model_scope(&mut self, models: Vec<String>) {
        self.model_scope = models;
    }

    /// The providers' models, read again, each with its output rate; `None` when they list none.
    pub fn listed_models(&mut self) -> Option<Vec<ModelOption>> {
        self.config.providers.reload();
        let rates = rates::Rates::read(self.rates.as_deref());
        let mut options = self.config.providers.options();
        for option in &mut options {
            option.rate = rates.median(&option.reference());
        }
        (!options.is_empty()).then_some(options)
    }

    pub fn conversation(&self) -> &[Value] {
        &self.conversation
    }

    /// The text of the conversation's last reply that has any, trimmed, as Scramjet's `/copy`
    /// copies it; reasoning and tool calls are not part of it.
    pub fn last_reply(&self) -> Option<String> {
        (self.conversation.iter().rev())
            .filter(|item| item["type"] == "message")
            .map(|item| message_text(item).trim().to_owned())
            .find(|text| !text.is_empty())
    }

    /// Shows the conversation as a resumed session has it, as its turns showed when they ran.
    pub fn show_conversation(&self, ui: &mut dyn AgentUi) {
        show_items(&self.conversation, Some(&self.config.root), ui);
    }

    /// Reads the providers again for what the current model takes, its limits and where its
    /// requests go; returns its context window. A model no provider offers any longer keeps what
    /// was known of it, and where its requests went while a file or program has problems: one
    /// that failed may offer it again.
    pub fn load_model_info(&mut self) -> Option<u64> {
        self.config.providers.reload();
        self.take_model_info()
    }

    /// `/reload`'s: providers read from other files, as `load_model_info` takes them, with the
    /// stores named before kept hidden as `Providers::reload` keeps them.
    pub fn set_providers(&mut self, mut providers: Providers) {
        providers.keep_credential_files(self.config.providers.credential_files().to_vec());
        self.config.providers = providers;
        self.take_model_info();
    }

    fn take_model_info(&mut self) -> Option<u64> {
        let models = self.config.providers.models();
        let mut providers: Vec<&str> = (models.iter())
            .map(|model| model.provider.as_str())
            .collect();
        providers.sort_unstable();
        providers.dedup();
        let several = providers.len() > 1;
        if !models.is_empty() {
            self.identities = (models.iter())
                .map(|model| {
                    let identity = instructions::Identity {
                        id: model.id.clone(),
                        name: model.name.clone(),
                        provider: Some(model.provider.clone()),
                    };
                    (model.reference(), identity)
                })
                .collect();
        }
        if let Some(model) = self.config.providers.find(&self.config.model) {
            self.provider = several.then(|| model.provider.clone());
            self.info = model.info.clone();
        } else if self.config.providers.problems().is_empty() {
            self.provider = None;
            self.info.route = None;
        }
        self.info.window
    }

    /// The current model's provider, when the models are those of several, as Scramjet's footer
    /// names it.
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }

    /// The median rate the current model's last replies were written at, in tokens a second,
    /// across sessions (`rates`).
    pub fn output_rate(&self) -> Option<f64> {
        rates::Rates::read(self.rates.as_deref()).median(&self.config.model)
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
