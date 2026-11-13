//! A coding agent that talks to its models' providers (Responses or Messages) and works in
//! a project.

mod approvals;
mod attached;
mod autonomy;
mod batch;
mod builtin;
mod calls;
mod client;
pub mod codex;
mod commands;
mod compaction;
mod completions;
mod configured;
mod cooldown;
pub mod copilot;
mod delegate;
mod forge;
mod image;
mod instructions;
mod lint;
mod listed;
pub mod local;
pub mod login;
mod mcp;
mod messages;
mod permissions;
mod pricing;
mod problem;
mod providers;
mod proxy;
mod publish;
mod question;
pub mod quota;
mod rates;
mod redact;
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
pub use turn::ShownPaused;
mod ui;
pub mod usage;
mod yaml;

pub use attached::Attached;
pub use autonomy::{
    Approval, Approvals, Edge, NextStep, NextSteps, Publication, Scope, Setting, Suggestion,
};
pub use client::{StreamProgress, download};
pub use commands::{Command, Commands, Origin, Set, Sets, scramjet_1_directory, sets_in};
pub use compaction::{AutoCompaction, Compaction};
pub use delegate::{NAME as DELEGATE, SCOPE_WARNING as DELEGATE_SCOPE_WARNING};
pub use image::{decode_base64, mime as image_type, set_images};
pub use lint::{Diagnostic, Severity, lint};
pub use listed::Listing;
pub use login::config_directory;
pub use mcp::{
    CALL as MCP_CALL, SEARCH as MCP_SEARCH, Server as McpServer, State as McpState,
    Transport as McpTransport,
};
pub use problem::{Problem, ProblemFile};
pub use providers::{KeyFrom, Providers, unmet_engine};
pub use publish::TOOLS as PUBLICATION_TOOLS;
pub use question::Choice;
pub use request::{CacheRetention, set_cache_retention};
pub use sandbox::{Remembered, Sandbox, SandboxMode};
pub use session::{
    Branch, Found, Label, Removal, Session, SessionInfo, directory as session_directory,
    projects_directory, read_branch, state_directory,
};
pub use shell::Shell;
pub use skills::{Skill, SkillOptions, Skills};
pub use status::{Completed, FRESH_MARK, Next, Reopened, Routing, Source, Step};
pub use suggest::Suggested;
pub use tools::{
    CODING_TOOLS, DEFAULT_TOOLS, HARNESS_TOOLS, READ_MORE, UserShell, found, is_shell,
    missing_shells, offline, read_line_too_long, sequence_diff, set_shell, set_user_shell,
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
    time::Duration,
};

/// Guards against a model that never stops calling tools.
const MAX_STEPS: usize = 300;
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
/// Stored with a response's last item: when its request was sent (`started`, milliseconds since
/// 1970), the milliseconds until its response's headers, its first streamed text, reasoning or
/// call, and its end, and the attempts that failed before it (`failed`). Never sent.
const TIMING: &str = "_scramjet_timing";
/// Stored with a user message whose text is not its prompt as typed (a command's or skill's
/// expansion, a `!command`'s output, `@` files' contents): the prompt as typed, which is shown and
/// recalled. Never sent to the provider.
pub(super) const TYPED: &str = "_scramjet_typed";
/// Stored, as `true`, with a tool result or `!` command that failed, so a resumed conversation
/// shows it as it ran. Never sent, as no `_scramjet_` field is.
const FAILED: &str = "_scramjet_failed";
/// The result of a call the session left without one.
const INTERRUPTED: &str = "Error: the previous session ended before this tool call reported a result. It may or may not have taken effect, and it was not re-run.";

/// A session before the current one in its chain (`Agent::earlier_sessions`).
pub struct EarlierSession {
    read: session::Earlier,
    /// What its command reported when it completed.
    pub summary: Option<String>,
}

impl EarlierSession {
    /// Its first prompt as typed, such as the command that ran in it.
    pub fn first_prompt(&self) -> Option<&str> {
        self.read.first_prompt.as_deref()
    }

    /// When it started, in milliseconds since 1970.
    pub fn created(&self) -> Option<u64> {
        self.read.created
    }

    /// When it was last written.
    pub fn modified(&self) -> std::time::SystemTime {
        self.read.modified
    }
}
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

/// How a model request that failed transiently is sent again: Scramjet's `retry` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retries {
    /// How many more times it is sent (`maxRetries`); none when `enabled` is false.
    pub attempts: u32,
    /// The first wait, doubled at each further one (`baseDelayMs`).
    pub delay: Duration,
    /// A longer wait, backoff or the provider's Retry-After, fails the request instead
    /// (`provider.maxRetryDelayMs`); `None` for no limit (0 there).
    pub max_wait: Option<Duration>,
}

impl Default for Retries {
    fn default() -> Self {
        Self {
            attempts: 3,
            delay: Duration::from_secs(2),
            max_wait: Some(Duration::from_secs(60)),
        }
    }
}

/// When and how far the conversation is compacted: Scramjet's `compaction` settings and its
/// `branchSummary.reserveTokens`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compacting {
    /// Whether it compacts by itself, near the window and after an overflow (`enabled`); `/compact`
    /// runs either way.
    pub automatic: bool,
    /// What is kept of the window for the next response (`reserveTokens`): automatic compaction
    /// runs once the context passes the rest, and summaries take at most 0.8 of it.
    pub reserve: u64,
    /// About how much of the latest conversation is kept whole (`keepRecentTokens`).
    pub keep_recent: u64,
    /// What a branch summary keeps of the window (`branchSummary.reserveTokens`).
    pub branch_reserve: u64,
}

impl Default for Compacting {
    fn default() -> Self {
        Self {
            automatic: true,
            reserve: 16_384,
            keep_recent: 20_000,
            branch_reserve: 16_384,
        }
    }
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
    /// How a model request that failed transiently (rate limit, overload, network) is sent again.
    pub retries: Retries,
    /// When and how far the conversation is compacted.
    pub compacting: Compacting,
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
    /// The names of those not started (`disabledMcpServers`, `/mcp disable`).
    pub mcp_disabled: Vec<String>,
    /// How the coding tools are confined (MCP servers never are); `None`
    /// leaves them as `Agent::set_hide_credentials` says.
    pub sandbox: Option<Sandbox>,
    /// `--session-dir` or the settings' `sessionDir`: where this run's sessions are saved and
    /// found, in place of the project's directory under `session::projects_directory`.
    pub session_dir: Option<PathBuf>,
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
    /// The model's response stopped at its output token limit; what it finished is kept.
    OutputLimit(String),
    /// The model or the provider's content filter declined to answer.
    Refusal(String),
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
            Self::Provider(message) | Self::OutputLimit(message) | Self::Refusal(message) => {
                write!(f, "provider: {message}")
            }
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
    /// What to tell the user of the command a failed or cancelled turn left waiting, shown after
    /// the turn's stop.
    interrupted: Option<String>,
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
    /// The session the next step just recorded starts, which `new_session` continues in.
    next_session: Option<Session>,
    /// Sessions switched away from while a full disk kept some of their lines in memory, tried
    /// again with the current one's until written.
    left: Vec<Session>,
    /// Whether lines runs that ended kept for want of room still wait for it
    /// (`session::write_kept`).
    kept_waiting: bool,
    /// What to say of those lines, once.
    kept_notes: Vec<String>,
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
    /// What local runtimes said of their models' windows, by model reference (`follow_runtime`).
    runtime_windows: HashMap<String, turn::RuntimeWindow>,
    /// The models of the user's scope (`set_model_scope`); empty: all.
    model_scope: Vec<String>,
    /// Whether the coding tools keep out of the credential stores (`sandbox::Hidden`).
    hide_credentials: bool,
    /// Whether the turn runs with no one to answer a question (`set_unattended`).
    unattended: bool,
    /// Harness tools this run cannot use, not offered (`leave_out_idle_tools`).
    left_out: Vec<&'static str>,
    /// Whether a completed command's recommended next step runs without the user choosing it.
    autopilot: bool,
    /// The MCP servers, while the model is offered their tools.
    mcp: Option<Arc<mcp::Servers>>,
    /// A reply's calls that a pause of the turn left running (`AgentError::Paused`).
    calls: Option<calls::Calls>,
    /// A step's request that a pause of the turn left streaming.
    replying: Option<request::Replying>,
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
    let skills = Skills::discover(listed, sets.list(), &SkillOptions::default(), root);
    Inventory {
        sets,
        commands,
        agents,
        warnings,
        skills,
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
        let directory = || match &config.session_dir {
            Some(directory) => Ok(directory.clone()),
            None => session::directory(&config.root),
        };
        // Before a session is opened, which may be one they belong to.
        let (kept_notes, kept_waiting) = kept_notes(session::write_kept(), true);
        let mut unsaved = None;
        let latest = match choice {
            SessionChoice::Continue => Session::latest(&directory()?)?,
            _ => None,
        };
        let (session, conversation, prompts) = match (choice, latest) {
            (_, Some(path)) | (SessionChoice::Open(path), _) => Session::open(&path)?,
            (SessionChoice::Fork(source), _) => {
                Session::fork_from(&directory()?, &config.root, &source)?
            }
            (choice, None) => {
                let directory = match choice {
                    SessionChoice::Unsaved => unsaved
                        .insert(session::Unsaved::create()?)
                        .path()
                        .to_owned(),
                    _ => directory()?,
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
            interrupted: None,
            suggestion: suggest::Pending::Nothing,
            delegated: Vec::new(),
            attached: None,
            autonomy: autonomy::global_path(),
            autonomy_diagnostics: Vec::new(),
            info: config.providers.info(&config.model),
            provider: None,
            identities: HashMap::new(),
            runtime_windows: HashMap::new(),
            model_scope: Vec::new(),
            hide_credentials: false,
            unattended: false,
            left_out: Vec::new(),
            autopilot,
            mcp: None,
            calls: None,
            replying: None,
            config,
            conversation,
            prompts,
            session,
            next_session: None,
            left: Vec::new(),
            kept_waiting,
            kept_notes,
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
        mcp_disabled: Vec<String>,
    ) {
        self.config.tools = tools;
        self.config.command_sets = command_sets;
        self.config.skills = skills;
        self.config.mcp_servers = mcp_servers;
        self.config.mcp_disabled = mcp_disabled;
        self.autonomy_diagnostics.clear();
        self.load();
        self.start_mcp();
    }

    /// The retry settings, read again at `/reload`; a request already waiting keeps its own.
    pub fn set_retries(&mut self, retries: Retries) {
        self.config.retries = retries;
    }

    pub fn compacting(&self) -> Compacting {
        self.config.compacting
    }

    /// The compaction settings, changed in `/settings` or read again at `/reload`.
    pub fn set_compacting(&mut self, compacting: Compacting) {
        self.config.compacting = compacting;
    }

    /// Starts the MCP servers when the model is offered their tools; servers already running as
    /// configured keep running, the others, also those that ended, start again.
    fn start_mcp(&mut self) {
        let offered = self.offered();
        let (wanted, root) = (&self.enabled_mcp(), &self.config.root);
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

    /// The MCP servers the settings list that are not disabled.
    fn enabled_mcp(&self) -> Vec<McpServer> {
        (self.config.mcp_servers.iter())
            .filter(|server| !self.config.mcp_disabled.contains(&server.name))
            .cloned()
            .collect()
    }

    /// The MCP servers the settings list that are disabled.
    pub fn disabled_mcp(&self) -> Vec<McpServer> {
        (self.config.mcp_servers.iter())
            .filter(|server| self.config.mcp_disabled.contains(&server.name))
            .cloned()
            .collect()
    }

    pub fn mcp_disabled(&self) -> &[String] {
        &self.config.mcp_disabled
    }

    /// `/mcp enable` and `disable`: the servers named in `names` stop, the others start; the
    /// model's tools and instructions follow from the next request.
    pub fn set_mcp_disabled(&mut self, names: Vec<String>) {
        self.config.mcp_disabled = names;
        self.instruct();
        self.start_mcp();
    }

    /// The names of the MCP servers the settings list, disabled ones too.
    pub fn mcp_names(&self) -> Vec<String> {
        (self.config.mcp_servers.iter())
            .map(|server| server.name.clone())
            .collect()
    }

    /// The MCP servers and how each is, in the settings' order.
    pub fn mcp_servers(&self) -> Vec<(McpServer, McpState)> {
        let states = (self.mcp.as_deref()).map(|servers| {
            // Settles which are offered directly, as the next request would, once none starts.
            servers.section(self.info.window);
            servers.states()
        });
        let states = states.unwrap_or_default();
        (states.into_iter())
            .map(|(server, state)| (server.clone(), state))
            .collect()
    }

    /// `/mcp reconnect`: starts the server named `name` again, or every one with `None`, and waits
    /// until they started or `cancel` is set. Returns each with how it is then.
    pub fn reconnect_mcp(
        &mut self,
        name: Option<&str>,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Vec<(McpServer, McpState)>, String> {
        let root = self.config.root.clone();
        let Some(servers) = self.mcp.as_mut() else {
            return Err("no MCP servers are running".into());
        };
        let Some(servers) = Arc::get_mut(servers) else {
            return Err("a tool call still uses the MCP servers; try again once it ended".into());
        };
        let names = servers.reconnect(name, &root)?;
        ui.notice(&format!(
            "Reconnecting to MCP servers: {}",
            names.join(", ")
        ));
        servers.settle(cancel);
        let states = (self.mcp_servers().into_iter())
            .filter(|(server, _)| names.contains(&server.name))
            .collect::<Vec<_>>();
        if let Some(servers) = &self.mcp {
            for (server, state) in &states {
                if matches!(state, McpState::Failed(_)) {
                    servers.reported(&server.name);
                }
            }
        }
        Ok(states)
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
            ui.notice(&format!(
                "MCP server {name} is not available: {why}\nIt starts again when used a minute \
                 after it stopped, or now with /mcp reconnect."
            ));
        }
        Ok(())
    }

    fn load(&mut self) {
        let config = &self.config;
        self.sets = Sets::for_project(&config.command_sets, &config.root);
        self.commands = Commands::discover(&self.sets);
        self.skills = Skills::discover(
            &config.skills,
            self.sets.list(),
            &config.skill_options,
            &config.root,
        );
        self.agents_directory = subagent::user_directory();
        self.agent_directory = login::config_directory();
        self.instruct();
    }

    fn instruct(&mut self) {
        let config = &self.config;
        let directory = self.agent_directory.as_deref();
        let hidden = sandbox::Hidden::stores(config.providers.credential_files());
        let loaded = instructions::load(&config.prompt, &config.root, directory, &hidden);
        let agents = subagent::registered(&self.sets, &mut Vec::new());
        let mcp = !self.enabled_mcp().is_empty();
        self.instructions = instructions::agent(
            &config.root,
            &self.without_left_out(tools::offered(
                &config.tools,
                config.only_tools.as_deref(),
                mcp,
                false,
            )),
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

    /// The sessions `sessions` lists, with every other project's saved sessions, newest first,
    /// as Scramjet's `/resume` lists all folders' with Tab. As in Scramjet's, a project whose
    /// sessions cannot be read is passed over.
    pub fn all_sessions(&self) -> Result<Vec<SessionInfo>, AgentError> {
        let mut sessions = self.sessions()?;
        let here = self.session_directory();
        // A saved session's directory is one of the projects'; an unsaved run's, or one
        // `session_dir` names, is elsewhere.
        let projects = match (
            self.unsaved.is_none() && self.config.session_dir.is_none(),
            here.parent(),
        ) {
            (true, Some(projects)) => Ok(projects.to_owned()),
            _ => session::projects_directory(),
        };
        let projects = projects.and_then(std::fs::read_dir);
        for entry in projects.into_iter().flatten().flatten() {
            let other = entry.path();
            if other != here {
                sessions.extend(Session::list(&other).unwrap_or_default());
            }
        }
        sessions.retain(|session| session.item_count > 0);
        sessions.sort_by(|a, b| (b.modified, &b.path).cmp(&(a.modified, &a.path)));
        Ok(sessions)
    }

    /// Whether the saved session `path` is one `sessions` lists, which `resume_session` continues
    /// in; another project's continues in its folder (`resume_session_in`), or is forked here
    /// (`fork_session_file`).
    pub fn session_here(&self, path: &Path) -> bool {
        path.parent() == Some(self.session_directory())
    }

    /// Continues in a new session of this project with every entry of the saved session `path`,
    /// another project's, as `--fork` copies one; that session stays as it is. On failure nothing
    /// changes.
    pub fn fork_session_file(&mut self, path: &Path) -> Result<(), AgentError> {
        let directory = self.session_directory().to_owned();
        let opened = Session::fork_from(&directory, &self.config.root, path)?;
        self.switch_session(opened)
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

    fn session_directory(&self) -> &Path {
        self.session.path().parent().unwrap_or(Path::new("."))
    }

    /// Continues in `session`; the one before goes, kept only while its file cannot take lines it
    /// holds. One left without a conversation was never written, as Scramjet would not have.
    fn replace_session(&mut self, session: Session) {
        let left = std::mem::replace(&mut self.session, session);
        if left.holding() {
            self.left.push(left);
        }
    }

    /// Whether the session is saved: not with `SessionChoice::Unsaved`.
    pub fn session_saved(&self) -> bool {
        self.unsaved.is_none()
    }

    /// The lines of the session's file that could not be read when it was opened (numbered
    /// from 0, the header's), which it goes on without.
    pub fn session_skipped(&self) -> &[usize] {
        self.session.skipped()
    }

    /// What to say, once, of a change in whether the session's lines reach its file: a full disk
    /// keeps them in memory (`Session::write`).
    pub fn saving_notice(&mut self) -> Option<String> {
        let mut said = std::mem::take(&mut self.kept_notes);
        let change = self.session.take_change().filter(|_| self.session_saved());
        said.extend(change.map(|change| match change {
            session::Saving::Held(error) => format!(
                "Session not written: {error}. It is kept in memory and written once there is room."
            ),
            session::Saving::Saved => {
                "Session written again, with what was kept in memory.".to_owned()
            }
        }));
        (!said.is_empty()).then(|| said.join("\n"))
    }

    /// Whether the session, or one switched away from, keeps lines in memory that its file could
    /// not take.
    pub fn session_holding(&self) -> bool {
        self.session.holding() || !self.left.is_empty() || self.kept_waiting
    }

    /// The directory where every line held in memory for want of room is copied, so that a later
    /// run writes it once there is room; `None` while some are only in memory.
    pub fn held_kept_in(&self) -> Option<&Path> {
        let holding = std::iter::once(&self.session).chain(&self.left);
        let mut kept = holding.filter(|session| session.held_len() > 0);
        let first = kept.next()?.kept_path()?;
        kept.all(|session| session.kept_path().is_some())
            .then(|| first.parent())
            .flatten()
    }

    /// How many bytes of lines the session and those switched away from keep in memory, for want
    /// of room, that would be lost if the agent ended now: none for an unsaved session's, which
    /// go with their directory, or for a session without a conversation, never written.
    pub fn held_len(&self) -> usize {
        if !self.session_saved() {
            return 0;
        }
        self.session.held_len() + self.left.iter().map(Session::held_len).sum::<usize>()
    }

    /// Tries again to write what the session and those switched away from keep in memory, with
    /// what to say of it, as a quiet session has no next write to take it.
    pub fn write_held(&mut self) -> Option<String> {
        let mut said = self.write_left();
        if self.kept_waiting {
            let (notes, waiting) = kept_notes(session::write_kept(), false);
            said.extend(notes);
            self.kept_waiting = waiting;
        }
        if let Err(error) = self.session.write_held() {
            said.push(format!("Error: session not written: {error}"));
        }
        said.extend(self.saving_notice());
        (!said.is_empty()).then(|| said.join("\n"))
    }

    /// Writes what the sessions switched away from keep in memory where there is room now, and
    /// says which were written; one that still cannot be written stays, said already.
    fn write_left(&mut self) -> Vec<String> {
        let mut said = Vec::new();
        self.left.retain_mut(|session| {
            let _ = session.write_held();
            session.take_change();
            if session.holding() {
                return true;
            }
            said.push(format!(
                "Session {} written, with what was kept in memory.",
                session.path().display()
            ));
            false
        });
        said
    }

    fn say_saving(&mut self, ui: &mut dyn AgentUi) {
        for notice in self.write_left() {
            ui.notice(&notice);
        }
        if let Some(notice) = self.saving_notice() {
            ui.notice(&notice);
        }
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
    /// apply. A session left here that still holds lines in memory is taken back with them.
    pub fn resume_session(&mut self, path: &Path) -> Result<(), AgentError> {
        let opened = self.reopened(path)?;
        self.switch_session(opened)
    }

    /// Continues in the saved session `path` as `resume_session` does, with the project `root`
    /// the session was in: the coding tools work there, and the command sets, skills, context
    /// files and MCP servers are those of `root` from then on, as Scramjet's `/resume` moves to
    /// a session's folder. The process's own directory stays.
    pub fn resume_session_in(&mut self, path: &Path, root: &Path) -> Result<(), AgentError> {
        let opened = self.reopened(path)?;
        if root != self.config.root {
            self.config.root = root.to_owned();
            self.autonomy_diagnostics.clear();
            self.load();
            // Servers started as programs run in the project's directory.
            self.mcp = None;
            self.start_mcp();
        }
        self.switch_session(opened)
    }

    /// The saved session `path`, opened to continue in, or taken back from those left here.
    fn reopened(&mut self, path: &Path) -> Result<session::Opened, AgentError> {
        if self.session.is_file(path) {
            return Err(AgentError::Session(std::io::Error::other(
                "that is the current session",
            )));
        }
        let left = (self.left.iter()).position(|left| left.path() == path || left.is_file(path));
        if let Some(index) = left {
            let (conversation, prompts) = self.left[index].resumed()?;
            let session = self.left.remove(index);
            return Ok((session, conversation, prompts));
        }
        Ok(Session::open(path)?)
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

    /// The index `fork` takes of the prompt at line `line` of the session file, as `tree` lists
    /// its entries; `None`: no prompt there.
    pub fn fork_index(&self, line: usize) -> Result<Option<usize>, AgentError> {
        Ok(self.session.fork_index(line)?)
    }

    /// Continues in a new session saved next to the current one, with the conversation before
    /// the current session's prompt `index` (0-based, as `fork_prompts` lists them) and the model
    /// it used then, as Scramjet's `/fork` does; the current session stays as it is. Returns the
    /// prompt, for editing before it is sent again, and the next-step list offered before it,
    /// to be offered again: a prompt sent by a choice from a list is forked before the list's
    /// record. `None`: no such prompt. On failure nothing changes. Approvals given with `a`
    /// still apply.
    pub fn fork(&mut self, index: usize) -> Result<Option<(String, Option<Reopened>)>, AgentError> {
        let mut offer = None;
        let before = |entries: &[session::Entry], leaf| {
            let (leaf, offered) = tree::before_offer(entries, leaf);
            offer = offered;
            leaf
        };
        let Some(((mut session, conversation, prompts), prompt)) =
            self.session.fork(&self.config.root, index, before)?
        else {
            return Ok(None);
        };
        // As in Scramjet, a fork without a conversation keeps the current model, and is not
        // written before the first reply (its lines are the current session's).
        if conversation.is_empty() {
            session.record_model(&self.config.model)?;
        }
        self.switch_session((session, conversation, prompts))?;
        Ok(Some((prompt, offer)))
    }

    /// Continues in a new session saved next to the current one with the conversation up to
    /// where it is and the session's name, as Scramjet's `/clone` duplicates a session; the
    /// current session stays as it is. Returns false, changing nothing, before any conversation
    /// (Scramjet's clones its model change then). On failure nothing changes.
    pub fn clone_session(&mut self) -> Result<bool, AgentError> {
        if self.conversation.is_empty() {
            return Ok(false);
        }
        let opened = self.session.clone_at_leaf(&self.config.root)?;
        self.switch_session(opened)?;
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
        let opened = Session::import(directory, path)?;
        self.switch_session(opened)
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
        let window = self.info.window;
        let servers = (self.mcp.as_deref()).map(|servers| servers.section(window));
        let sandbox = (self.config.sandbox.as_ref())
            .filter(|_| self.offers(permissions::NAME))
            .map(|sandbox| permissions::section(sandbox, &self.config.root));
        self.instructions.clone()
            + &servers.unwrap_or_default()
            + &sandbox.unwrap_or_default()
            + &instructions::identity(model, &contributors, &self.identities)
            + &instructions::environment(
                &self.config.root,
                self.session_saved().then(|| self.session.path()),
            )
    }

    /// The definitions of the tools the model's requests offer.
    pub fn tool_definitions(&self) -> Value {
        let offered = self.offered();
        let mut definitions = tools::definitions(&offered);
        if let (Some(servers), Value::Array(listed)) = (&self.mcp, &mut definitions)
            && offered.contains(&mcp::CALL)
        {
            listed.extend(servers.direct(self.info.window));
        }
        definitions
    }

    /// The tools the model has, coding tools first.
    fn offered(&self) -> Vec<&'static str> {
        let config = &self.config;
        let mcp = !self.enabled_mcp().is_empty();
        let sandbox = config.sandbox.is_some();
        self.without_left_out(tools::offered(
            &config.tools,
            config.only_tools.as_deref(),
            mcp,
            sandbox,
        ))
    }

    fn without_left_out(&self, mut offered: Vec<&'static str>) -> Vec<&'static str> {
        offered.retain(|tool| !self.left_out.contains(tool));
        offered
    }

    /// For a run whose only prompts are given at its start (`-p`): leaves out
    /// `suggest_scramjet_next_steps`, as no one can choose a suggestion, and, unless `commands`
    /// (a prompt may start a command) or the session left a command waiting,
    /// `report_scramjet_command_status`, which only a command takes. Small models call them
    /// anyway when offered.
    pub fn leave_out_idle_tools(&mut self, commands: bool) {
        self.left_out = vec![suggest::NAME];
        if !commands && self.lifecycle.active().is_none() {
            self.left_out.push(status::NAME);
        }
        self.instruct();
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
            self.keep_runtime_window();
        }
        if let Some(level) = effort {
            self.config.effort = Some(level);
        }
        self.answer_interrupted_calls()?;
        Ok(())
    }

    /// Continues in an empty session saved next to the current one, or in the one the next step
    /// just recorded starts; on failure nothing changes. Approvals given with `a` still apply.
    pub fn new_session(&mut self) -> Result<(), AgentError> {
        let mut session = match self.next_session.take() {
            Some(session) => session,
            None => Session::create(self.session_directory(), &self.config.root, None)?,
        };
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
    pub fn expand(&mut self, prompt: &str) -> Option<String> {
        self.commands.refresh(prompt);
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

    /// Why the current model cannot be asked until a sign-in, if it cannot
    /// (`Providers::login_needed`).
    pub fn login_needed(&self) -> Option<String> {
        let route = self.info.route.as_ref();
        self.config
            .providers
            .login_needed(&self.config.model, route)
    }

    /// The id the current model's requests name it by.
    pub fn model_id(&self) -> &str {
        &self.info.id
    }

    /// `--api-key`: `key`, as it is, for the current model's provider's requests from now on.
    pub fn give_key(&mut self, key: &str) -> Result<(), String> {
        let Some(provider) = self.model_provider().map(str::to_owned) else {
            return Err(format!(
                "--api-key: no provider offers {}, the model it would be for",
                self.config.model
            ));
        };
        self.config.providers.give_key(&provider, key);
        self.take_model_info();
        Ok(())
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
        self.keep_runtime_window();
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

    /// Whether the turns that follow run with no one to answer (`-p`, a step autopilot runs):
    /// a request for write access is then refused rather than asked.
    pub fn set_unattended(&mut self, unattended: bool) {
        self.unattended = unattended;
    }

    /// Confines the coding tools as `sandbox` says, the credential stores hidden too, or, with
    /// `None`, no more than `set_hide_credentials` says. Where bubblewrap cannot run, their
    /// commands fail (`Sandbox::check` says so first). A `!` command and the MCP servers stay
    /// outside.
    pub fn set_sandbox(&mut self, sandbox: Option<Sandbox>) {
        self.config.sandbox = sandbox;
    }

    pub fn sandbox(&self) -> Option<&Sandbox> {
        self.config.sandbox.as_ref()
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
        let files = match files.is_empty() {
            true => "models.json files".to_owned(),
            false => files.join(", "),
        };
        let mut message = match providers.options().is_empty() {
            true => format!("no provider offers any model: /login, or add providers to {files}"),
            false => format!(
                "no provider offers the model {}; providers come from {files}",
                self.config.model
            ),
        };
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

    /// The sessions before this one in its chain, the first first, as they show above its
    /// conversation; reading stops at one that cannot be read, with what went wrong.
    pub fn earlier_sessions(&self) -> (Vec<EarlierSession>, Option<String>) {
        let mut earlier = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut next = self.session.continues().map(Path::to_owned);
        let mut problem = None;
        while let Some(path) = next.take() {
            if !seen.insert(path.clone()) {
                break;
            }
            match session::read_earlier(&path) {
                Ok(read) => {
                    next = read.continues.clone();
                    earlier.push(EarlierSession {
                        summary: status::completed_summary(&read.items),
                        read,
                    });
                }
                Err(error) => problem = Some(format!("{}: {error}", path.display())),
            }
        }
        earlier.reverse();
        (earlier, problem)
    }

    /// Shows an earlier session's conversation as `show_conversation` shows this one's.
    pub fn show_earlier(&self, earlier: &EarlierSession, ui: &mut dyn AgentUi) {
        show_items(&earlier.read.items, Some(&self.config.root), ui);
    }

    /// Shows the conversation as a resumed session has it, as its turns showed when they ran;
    /// calls left to run again (a question gone back to) show as they run.
    pub fn show_conversation(&self, ui: &mut dyn AgentUi) {
        self.show(ui, false);
    }

    /// `show_conversation`, telling `ui` each item's line in the session file (`AgentUi::entry`);
    /// with the file unreadable, none.
    pub fn show_conversation_entries(&self, ui: &mut dyn AgentUi) {
        self.show(ui, true);
    }

    fn show(&self, ui: &mut dyn AgentUi, entries: bool) {
        let root = Some(self.config.root.as_path());
        let shown: std::borrow::Cow<[Value]> = match &self.calls {
            None => self.conversation.as_slice().into(),
            Some(calls) => (self.conversation.iter())
                .filter(|item| {
                    item["type"] != "function_call"
                        || !calls.has(item["call_id"].as_str().unwrap_or(""))
                })
                .cloned()
                .collect::<Vec<_>>()
                .into(),
        };
        let lines = match entries {
            true => self.session.lines_of(&shown).unwrap_or_default(),
            false => Vec::new(),
        };
        ui::show_entries(&shown, &lines, root, ui);
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
        providers.keep_given_key(&self.config.providers);
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
        self.keep_runtime_window();
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
            // A question gone back to, and the calls after it, are answered as they run again.
            .filter(|id| !self.calls.as_ref().is_some_and(|calls| calls.has(id)))
            .map(str::to_owned)
            .collect();
        for call_id in missing {
            self.add(json!({"type":"function_call_output","call_id":call_id,
                "output":INTERRUPTED,FAILED:true}))?;
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

/// What to say of the lines `written` (`session::write_kept`), its problems only when `problems`,
/// and whether some still wait for room.
fn kept_notes(written: session::Written, problems: bool) -> (Vec<String>, bool) {
    let mut notes: Vec<String> = (written.sessions.iter())
        .map(|(path, len)| {
            format!(
                "Session {} written, with the {} an earlier run kept in memory for want of room.",
                path.display(),
                byte_size(*len)
            )
        })
        .collect();
    if problems {
        notes.extend(written.problems);
    }
    (notes, written.waiting > 0)
}

/// `bytes` for people: `512 bytes`, `1.5 KB`, `2.0 MB`.
pub fn byte_size(bytes: usize) -> String {
    match bytes {
        ..1024 => format!("{bytes} bytes"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
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
