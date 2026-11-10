//! Scramjet's `subagent` tool (`subagent/index.ts`): the model hands tasks to agents defined in
//! markdown files, each run with a context of its own and its final reply as the result. Here
//! the children run in this process with the parent's endpoint, and get read-only tools only:
//! `read`, `grep`, `find` and `ls`, and bash in the read-only sandbox, none of which shows the
//! user's credential stores.

use super::yaml;
use super::{
    AgentError, AgentUi, ChildItem, ChildRun, ChildState, ChildUsage, Children, Delegation,
    MAX_STEPS, Problem, Skills, ToolDetail, Usage,
    approvals::{self, Approvals},
    commands::{self, Sets},
    instructions,
    providers::{ModelInfo, Providers},
    rates,
    request::{self, Sending},
    sandbox,
    tools::{self, Call, Output},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};
use yaml_rust2::Yaml;

mod slots;

pub const NAME: &str = "subagent";

/// Scramjet's `promptSnippet`, listed under the available tools.
pub(super) const SNIPPET: &str = "effort: optional thinking level \
(off/minimal/low/medium/high/xhigh). Guidance: low for exploration and information gathering; \
medium for structured review and analysis; high for architecture, design, assessment, and code \
tracing tasks requiring judgment; xhigh for complex multi-constraint planning or deep root-cause \
analysis. Capped at your current session level.";

const MAX_PARALLEL_TASKS: usize = 8;
const MAX_CONCURRENCY: usize = 4;
/// Children running at once in this process, whatever calls and threads started them.
static CHILDREN: slots::Slots = slots::Slots::new(8);
/// Scramjet's thinking levels, lowest first.
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
/// The tools a child can have; none of them changes a file.
const READ_ONLY: [&str; 5] = ["read", "grep", "find", "ls", "bash"];

pub(super) fn definition() -> Value {
    let effort = json!({"type":"string","enum":LEVELS,
        "description":"Thinking effort level for the agent subprocess. Capped at the main session's level."});
    let item = |task: &str| {
        json!({"type":"object","properties":{
            "agent":{"type":"string","description":"Name of the agent to invoke"},
            "task":{"type":"string","description":task},
            "cwd":{"type":"string","description":"Working directory for the agent process"},
            "effort":effort},
          "required":["agent","task"]})
    };
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Delegate tasks to specialized subagents with isolated context. Modes: \
single (agent + task), parallel (tasks array), chain (sequential with {previous} placeholder). \
Default loose-agent scope is \"user\" (from ~/.scramjet/agent/agents). To enable loose \
project-local agents in .scramjet/agents, set agentScope: \"both\" (or \"project\"). Registered \
command-set agents remain available under every scope; project-provenance confirmation still \
applies.",
        "parameters":{"type":"object","properties":{
            "agent":{"type":"string","description":"Name of the agent to invoke (for single mode)"},
            "task":{"type":"string","description":"Task to delegate (for single mode)"},
            "tasks":{"type":"array","description":"Array of {agent, task} for parallel execution",
                "items":item("Task to delegate to the agent")},
            "chain":{"type":"array","description":"Array of {agent, task} for sequential execution",
                "items":item("Task with optional {previous} placeholder for prior output")},
            "agentScope":{"type":"string","enum":["user","project","both"],"default":"user",
                "description":"Which loose agent directories to use. Default: \"user\". Use \
\"both\" to include project-local loose agents. Registered command-set agents remain available \
under every scope."},
            "cwd":{"type":"string","description":"Working directory for the agent process (single mode)"},
            "effort":effort}}})
}

/// An agent a task can be given to, from its markdown file.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Definition {
    pub name: String,
    pub description: String,
    /// The tools its frontmatter lists; `None` for the default ones.
    pub tools: Option<Vec<String>>,
    /// The tools its `disallowedTools` lists, as Claude Code's, left out of `tools` or the
    /// default ones.
    pub disallowed: Vec<String>,
    pub model: Option<String>,
    /// The file's body, added to the child's instructions.
    pub prompt: String,
    /// `user` or `project` for a loose agent; a command set's scope (`global` or `project`).
    pub source: &'static str,
    pub path: PathBuf,
    /// The SHA-256 of the file's text, which the user's approval of a project agent records.
    pub digest: String,
    /// What Scramjet says about frontmatter fields it ignored.
    pub diagnostics: Vec<String>,
}

impl Definition {
    fn problems(&self) -> impl Iterator<Item = Problem> {
        (self.diagnostics.iter()).map(|text| Problem::in_file(text.clone(), &self.path))
    }
}

/// Which loose agent directories a call uses, as Scramjet's `agentScope`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Scope {
    User,
    Project,
    Both,
}

/// Where agents are found, besides the project.
pub(super) struct Sources<'a> {
    /// The user's loose agents: `$SCRAMJET_CODING_AGENT_DIR/agents`, else
    /// `~/.scramjet/agent/agents`.
    pub user: Option<&'a Path>,
    /// The session's command sets, whose agents are registered.
    pub sets: &'a Sets,
}

/// Scramjet's agent directory, as its `getAgentDir()`: `$SCRAMJET_CODING_AGENT_DIR`, else
/// `~/.scramjet/agent`.
pub fn agent_directory() -> Option<PathBuf> {
    match std::env::var_os("SCRAMJET_CODING_AGENT_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(super::tools::resolve(
            Path::new("/"),
            &dir.to_string_lossy(),
        )),
        None => std::env::home_dir().map(|home| crate::os::join(&home, ".scramjet/agent")),
    }
}

/// The user's loose agent directory.
pub(super) fn user_directory() -> Option<PathBuf> {
    agent_directory().map(|dir| dir.join("agents"))
}

/// The agents a call can use, and what was wrong with the files skipped.
#[derive(Debug, Default)]
pub(super) struct Discovery {
    pub agents: Vec<Definition>,
    pub diagnostics: Vec<Problem>,
}

/// As Scramjet's `discoverAgents`: the user's loose agents and those of the nearest
/// `.scramjet/agents` above `cwd`, by `scope` (with `Both`, a project agent replaces a user one of
/// its name), then the agents of the command sets, which replace loose ones. Unlike Scramjet, a
/// loose agent whose name is in a set's namespace (`<set>:`) is skipped with a warning: such
/// names belong to the set.
pub(super) fn discover(sources: &Sources, cwd: &Path, scope: Scope) -> Discovery {
    let mut found = Discovery::default();
    let mut project = project_directory(cwd);
    if let Some(directory) = &project
        && let Some(owner) = crate::os::stranger(directory)
    {
        let text = crate::os::left_out(directory, owner);
        found.diagnostics.push(Problem::in_file(text, directory));
        project = None;
    }
    let mut agents: Vec<Definition> = Vec::new();
    if scope != Scope::Project
        && let Some(user) = sources.user
    {
        agents = loose(user, "user", sources.sets, &mut found.diagnostics);
    }
    if scope != Scope::User
        && let Some(project) = &project
    {
        for agent in loose(project, "project", sources.sets, &mut found.diagnostics) {
            match agents.iter_mut().find(|known| known.name == agent.name) {
                Some(known) => *known = agent,
                None => agents.push(agent),
            }
        }
    }
    for agent in registered(sources.sets, &mut found.diagnostics) {
        agents.retain(|known| known.name != agent.name);
        agents.push(agent);
    }
    found.agents = agents;
    found
}

/// Scramjet's `# Available subagents` section of the instructions, empty without agents.
pub(super) fn catalog(agents: &[Definition]) -> String {
    if agents.is_empty() {
        return String::new();
    }
    let mut entries: Vec<String> = agents
        .iter()
        .map(|agent| format!("- {}: {}", agent.name, agent.description))
        .collect();
    entries.sort();
    format!(
        "# Available subagents\n\nThe following agents can be dispatched via the `subagent` \
tool:\n\n{}",
        entries.join("\n")
    )
}

/// The project's agent directory: the nearest `.scramjet/agents` from `cwd` up.
pub(super) fn project_directory(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .map(|directory| crate::os::join(directory, ".scramjet/agents"))
        .find(|candidate| candidate.is_dir())
}

/// The agents in `directory`'s `.md` files, by file name, as Scramjet's `loadAgentsFromDir`,
/// without those in the namespace of one of `sets`; a warning names them unless they link into
/// the set's own directory.
fn loose(
    directory: &Path,
    source: &'static str,
    sets: &Sets,
    diagnostics: &mut Vec<Problem>,
) -> Vec<Definition> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            diagnostics.push(Problem::new(format!(
                "{}: failed to read agent directory ({error})",
                directory.display()
            )));
            return Vec::new();
        }
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".md"))
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_file() || kind.is_symlink())
        })
        .map(|entry| entry.path())
        .collect();
    files.sort();
    let mut agents = Vec::new();
    let claimed = |name: &str, path: &Path| {
        let set = sets.owner(name)?;
        let target = std::fs::canonicalize(path).ok();
        let own = std::fs::canonicalize(&set.directory).ok();
        if let (Some(target), Some(own)) = (target, own)
            && target.starts_with(own)
        {
            return Some(None);
        }
        // A stale link's target says why it is there and that it can go.
        let link = match std::fs::read_link(path) {
            Ok(link) if path.exists() => format!(" (a link to {})", link.display()),
            Ok(link) => format!(" (a broken link to {})", link.display()),
            Err(_) => String::new(),
        };
        let text = format!(
            "skipping {source} agent {}{link}: names starting with \"{}:\" belong to the {} \
command set {}",
            path.display(),
            set.name,
            set.origin.name(),
            set.directory.display()
        );
        Some(Some(Problem::in_file(text, path)))
    };
    for path in files {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        if let Some(skipped) = claimed(&stem, &path) {
            diagnostics.extend(skipped);
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                let text = format!("{}: failed to read agent file ({error})", path.display());
                diagnostics.push(Problem::in_file(text, &path));
                continue;
            }
        };
        match parse(&path, &content, source) {
            Ok(agent) => match claimed(&agent.name, &path) {
                Some(skipped) => diagnostics.extend(skipped),
                None => {
                    diagnostics.extend(agent.problems());
                    agents.push(agent);
                }
            },
            Err(error) => diagnostics.push(Problem::in_file(error, &path)),
        }
    }
    agents
}

/// The agents of `sets`, as Scramjet's `buildAgentRegistry`: `<set>/agents/<name>.md` or
/// `<set>:<name>.md`, a bare `name` in either taken as `<set>:<name>`; the first of a name kept.
pub(super) fn registered(sets: &Sets, diagnostics: &mut Vec<Problem>) -> Vec<Definition> {
    let mut agents: Vec<Definition> = Vec::new();
    for set in sets.list() {
        let scope = set.origin.name();
        let directory = set.directory.join("agents");
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        for path in files {
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            let skipping = |error: String| {
                Problem::in_file(format!("skipping agent {}: {error}", path.display()), &path)
            };
            let Some(stem) = file.strip_suffix(".md") else {
                diagnostics.push(skipping(format!("{file}: not a markdown file")));
                continue;
            };
            if commands::qualified(&set.name, stem).is_none() {
                diagnostics.push(skipping(format!(
                    "{file}: filename must not name another set than \"{}\"",
                    set.name
                )));
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let agent = match parse(&path, &content, scope) {
                Ok(agent) => match commands::qualified(&set.name, &agent.name) {
                    Some(name) => Definition { name, ..agent },
                    None => {
                        diagnostics.push(skipping(format!(
                            "{file}: name must not name another set than \"{}\"",
                            set.name
                        )));
                        continue;
                    }
                },
                Err(error) => {
                    diagnostics.push(skipping(error));
                    continue;
                }
            };
            if let Some(existing) = agents.iter().find(|known| known.name == agent.name) {
                let text = format!(
                    "skipping {scope} agent {} at {}: name already registered from {}",
                    agent.name,
                    path.display(),
                    existing.path.display()
                );
                diagnostics.push(Problem::in_file(text, &path));
                continue;
            }
            diagnostics.extend(agent.problems());
            agents.push(agent);
        }
    }
    agents
}

/// An agent file as Scramjet's `parseExecutableAgent` reads it, or why it rejects it.
fn parse(path: &Path, content: &str, source: &'static str) -> Result<Definition, String> {
    let shown = path.display();
    let digest = approvals::digest(content);
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (frontmatter, body) = commands::split_frontmatter(content);
    let frontmatter = yaml::first(&frontmatter)
        .map_err(|error| format!("{shown}: invalid YAML frontmatter ({error})"))?;
    let text = |key: &str| frontmatter[key].as_str().map(str::trim).unwrap_or_default();
    let (name, description) = (text("name"), text("description"));
    if name.is_empty() || description.is_empty() {
        return Err(format!(
            "{shown}: frontmatter must include string name and description"
        ));
    }
    let mut diagnostics = Vec::new();
    let mut list = |key: &str| match &frontmatter[key] {
        Yaml::String(list) => Some(
            list.split(',')
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
        .filter(|tools| !tools.is_empty()),
        Yaml::BadValue => None,
        _ => {
            diagnostics.push(format!("{shown}: ignoring non-string {key} frontmatter"));
            None
        }
    };
    let tools = list("tools");
    let disallowed = list("disallowedTools").unwrap_or_default();
    let model = match &frontmatter["model"] {
        Yaml::String(model) => Some(model.trim().to_owned()).filter(|model| !model.is_empty()),
        Yaml::BadValue => None,
        _ => {
            diagnostics.push(format!("{shown}: ignoring non-string model frontmatter"));
            None
        }
    };
    Ok(Definition {
        name: name.to_owned(),
        description: description.to_owned(),
        tools,
        disallowed,
        model,
        prompt: body,
        source,
        path: path.to_owned(),
        digest,
        diagnostics,
    })
}

/// One task as a call gives it.
#[derive(Debug)]
struct Task {
    agent: String,
    task: String,
    cwd: Option<String>,
    effort: Option<String>,
}

enum Mode {
    Single(Task),
    Parallel(Vec<Task>),
    Chain(Vec<Task>),
}

impl Mode {
    fn tasks(&self) -> Vec<&Task> {
        match self {
            Self::Single(task) => vec![task],
            Self::Parallel(tasks) | Self::Chain(tasks) => tasks.iter().collect(),
        }
    }
}

struct Arguments {
    /// `None` when the call gives no mode, or more than one.
    mode: Option<Mode>,
    scope: Scope,
}

/// What is wrong with a call's arguments, in the form of Pi's validation errors.
#[derive(Default)]
struct Checked {
    errors: Vec<String>,
}

impl Checked {
    fn string(&mut self, value: &Value, path: &str, required: bool) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Null if !required => None,
            Value::Null => {
                self.errors
                    .push(format!("  - {path}: must have required property"));
                None
            }
            _ => {
                self.errors.push(format!("  - {path}: must be string"));
                None
            }
        }
    }

    fn effort(&mut self, value: &Value, path: &str) -> Option<String> {
        let effort = self.string(value, path, false)?;
        if !LEVELS.contains(&effort.as_str()) {
            let allowed = LEVELS.join(", ");
            self.errors
                .push(format!("  - {path}: must be one of {allowed}"));
        }
        Some(effort)
    }

    fn task(&mut self, item: &Value, path: &str) -> Option<Task> {
        let agent = self.string(&item["agent"], &format!("{path}/agent"), true);
        let task = self.string(&item["task"], &format!("{path}/task"), true);
        let cwd = self.string(&item["cwd"], &format!("{path}/cwd"), false);
        let effort = self.effort(&item["effort"], &format!("{path}/effort"));
        Some(Task {
            agent: agent?,
            task: task?,
            cwd,
            effort,
        })
    }

    fn tasks(&mut self, value: &Value, path: &str) -> Vec<Task> {
        match value {
            Value::Null => Vec::new(),
            Value::Array(items) => items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| self.task(item, &format!("{path}/{index}")))
                .collect(),
            _ => {
                self.errors.push(format!("  - {path}: must be an array"));
                Vec::new()
            }
        }
    }
}

/// The call's arguments, or what is wrong with them.
fn arguments(arguments: &Value) -> Result<Arguments, Vec<String>> {
    let mut checked = Checked::default();
    let tasks = checked.tasks(&arguments["tasks"], "tasks");
    let chain = checked.tasks(&arguments["chain"], "chain");
    let agent = checked.string(&arguments["agent"], "agent", false);
    let task = checked.string(&arguments["task"], "task", false);
    let cwd = checked.string(&arguments["cwd"], "cwd", false);
    let effort = checked.effort(&arguments["effort"], "effort");
    let scope = match checked
        .string(&arguments["agentScope"], "agentScope", false)
        .as_deref()
    {
        None | Some("user") => Scope::User,
        Some("project") => Scope::Project,
        Some("both") => Scope::Both,
        Some(_) => {
            let error = "  - agentScope: must be one of user, project, both";
            checked.errors.push(error.into());
            Scope::User
        }
    };
    if !checked.errors.is_empty() {
        return Err(checked.errors);
    }
    // As in Scramjet, an empty agent or task does not make a single call.
    let single = match (agent, task) {
        (Some(agent), Some(task)) if !agent.is_empty() && !task.is_empty() => Some(Task {
            agent,
            task,
            cwd,
            effort,
        }),
        _ => None,
    };
    let modes = usize::from(!chain.is_empty())
        + usize::from(!tasks.is_empty())
        + usize::from(single.is_some());
    let mode = match single {
        _ if modes != 1 => None,
        Some(single) => Some(Mode::Single(single)),
        None if !tasks.is_empty() => Some(Mode::Parallel(tasks)),
        None => Some(Mode::Chain(chain)),
    };
    Ok(Arguments { mode, scope })
}

/// The call line's text, as Scramjet's `renderCall` while the call runs: the agents, the scope
/// and an effort given, then each task's first line.
pub(super) fn summary(arguments: &Value) -> String {
    let scope = arguments["agentScope"].as_str().unwrap_or("user");
    let effort = |value: &Value| match value["effort"].as_str() {
        Some(effort) => format!(" [Effort:{effort}]"),
        None => String::new(),
    };
    let first_line = |value: &Value| {
        let task = value["task"].as_str().unwrap_or("...").trim();
        let task = task.replace("{previous}", "");
        let mut lines = task.trim().lines();
        let first = lines.next().unwrap_or_default().to_owned();
        match lines.next() {
            Some(_) => format!("{first} …"),
            None => first,
        }
    };
    let list = |key: &str, title: &str| -> Option<String> {
        let items = arguments[key]
            .as_array()
            .filter(|items| !items.is_empty())?;
        let mut text = format!(
            "{title} ({} {}) [{scope}]",
            items.len(),
            match key {
                "chain" => "steps",
                _ => "tasks",
            }
        );
        for (index, item) in items.iter().enumerate() {
            let agent = item["agent"].as_str().unwrap_or("...");
            let number = match key {
                "chain" => format!("{}. ", index + 1),
                _ => String::new(),
            };
            text += &format!("\n{number}{agent}{} {}", effort(item), first_line(item));
        }
        Some(text)
    };
    list("chain", "chain")
        .or_else(|| list("tasks", "parallel"))
        .unwrap_or_else(|| {
            let agent = arguments["agent"].as_str().unwrap_or("...");
            let task = match first_line(arguments) {
                task if task.is_empty() => task,
                task => format!("\n{task}"),
            };
            format!("{agent}{} [{scope}]{task}", effort(arguments))
        })
}

/// What children share with the agent that starts them.
pub(super) struct Parent<'a> {
    pub providers: &'a Providers,
    pub retries: super::Retries,
    pub model: &'a str,
    pub info: &'a ModelInfo,
    /// The level the agent's own requests ask for.
    pub effort: Option<&'a str>,
    pub max_output_tokens: Option<u64>,
    pub root: &'a Path,
    /// Shared by children of an agent, so that their requests can reuse cached prefixes.
    pub cache_key: String,
    pub sources: Sources<'a>,
    /// The agent's skills, which children with `read` are told of, as Scramjet's children load
    /// them.
    pub skills: &'a Skills,
    /// Scramjet's agent directory, whose context files children have, as Scramjet's do.
    pub agent_directory: Option<&'a Path>,
    /// Where the user's approvals of project agents are remembered (`approvals::path`).
    pub approvals: Option<&'a Path>,
    /// Where replies' output rates are kept (`rates::path`).
    pub rates: Option<&'a Path>,
    /// The agent's coding tools, whose read-only ones a child whose file lists none has.
    pub tools: &'a [String],
}

/// A task given to an agent, ready to run.
struct Job<'a> {
    name: &'a str,
    /// `None` when no agent has that name.
    agent: Option<&'a Definition>,
    task: String,
    cwd: PathBuf,
    /// The model's reference (`provider/id`), or the name an agent gives that none offers.
    model: String,
    info: ModelInfo,
    /// The level its requests ask for, at most the parent's (`child_effort`).
    effort: Option<String>,
    tools: Vec<&'static str>,
    warnings: Vec<String>,
}

impl<'a> Job<'a> {
    fn new(
        task: &'a Task,
        text: String,
        agents: &'a [Definition],
        parent: &Parent,
        sandbox: &Result<(), String>,
    ) -> Self {
        let agent = agents.iter().find(|agent| agent.name == task.agent);
        let (tools, warnings) = agent
            .map(|agent| child_tools(agent, parent.tools, sandbox))
            .unwrap_or_default();
        let model = agent.map_or("", |agent| agent.model.as_deref().unwrap_or(parent.model));
        let info = match model == parent.model {
            true => parent.info.clone(),
            false => parent.providers.info(model),
        };
        Self {
            name: &task.agent,
            agent,
            task: text,
            cwd: match &task.cwd {
                Some(cwd) => tools::resolve(parent.root, cwd),
                None => parent.root.to_owned(),
            },
            model: parent.providers.reference(model),
            info,
            effort: agent.and(child_effort(task.effort.as_deref(), parent.effort)),
            tools,
            warnings,
        }
    }

    /// The job as the user sees it before it runs.
    fn shown(&self) -> ChildRun {
        ChildRun {
            agent: self.name.to_owned(),
            source: self
                .agent
                .map_or("unknown", |agent| agent.source)
                .to_owned(),
            model: self.info.id.clone(),
            // Scramjet's `off` is the endpoint's `none`.
            effort: self.effort.as_deref().map(|effort| match effort {
                "none" => "off".to_owned(),
                effort => effort.to_owned(),
            }),
            task: self.task.clone(),
            state: ChildState::Running,
            items: Vec::new(),
            usage: ChildUsage::default(),
            responses: Vec::new(),
            warnings: self.warnings.clone(),
        }
    }
}

/// How one task ran.
struct Run {
    agent: String,
    /// The text of its last reply that had text.
    output: String,
    /// Why it failed, as Scramjet's stop reason (`error`, `aborted`, or none), with the message.
    error: Option<(Option<&'static str>, String)>,
    usage: Usage,
    /// Responses received.
    turns: u64,
    /// Each response's usage and timing (`ChildRun::responses`).
    responses: Vec<Value>,
    /// What was said about the agent's file and the tools it did not get.
    warnings: Vec<String>,
}

impl Run {
    fn new(name: &str) -> Self {
        Self {
            agent: name.to_owned(),
            output: String::new(),
            error: None,
            usage: Usage::default(),
            turns: 0,
            responses: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn shown_usage(&self) -> ChildUsage {
        ChildUsage {
            turns: self.turns,
            input_tokens: self.usage.input_tokens,
            cached_input_tokens: self.usage.cached_input_tokens,
            cache_write_tokens: self.usage.cache_write_tokens,
            output_tokens: self.usage.output_tokens,
            cost: (!self.usage.unpriced).then_some(self.usage.cost),
        }
    }

    fn shown_state(&self) -> ChildState {
        match &self.error {
            None => ChildState::Done,
            Some((reason, message)) => ChildState::Failed {
                reason: reason.map(str::to_owned),
                message: message.clone(),
            },
        }
    }

    fn failed(name: &str, reason: Option<&'static str>, message: String) -> Self {
        Self {
            error: Some((reason, message)),
            ..Self::new(name)
        }
    }

    /// As Scramjet's `getResultOutput`, after the invocation warnings.
    fn result(&self) -> String {
        let text = match &self.error {
            Some((_, message)) => message.clone(),
            None if self.output.is_empty() => "(no output)".into(),
            None => self.output.clone(),
        };
        with_warnings(text, std::slice::from_ref(self))
    }
}

fn with_warnings(text: String, runs: &[Run]) -> String {
    let mut warnings: Vec<&String> = Vec::new();
    for warning in runs.iter().flat_map(|run| &run.warnings) {
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    match warnings.is_empty() {
        true => text,
        false => {
            let list: Vec<String> = warnings.iter().map(|w| format!("- {w}")).collect();
            format!("Agent invocation warnings:\n{}\n\n{text}", list.join("\n"))
        }
    }
}

/// Runs a `subagent` call: what the model receives, and the children's usage to count.
pub(super) fn run(
    call_arguments: &Value,
    parent: &Parent,
    ui: &mut dyn AgentUi,
    cancel: &AtomicBool,
) -> (Output, Usage) {
    let plain = |text: String, failed: bool| Output {
        text,
        detail: None,
        failed,
        image: None,
    };
    let given = match arguments(call_arguments) {
        Ok(given) => given,
        Err(errors) => {
            let received = serde_json::to_string_pretty(call_arguments).unwrap_or_default();
            let text = format!(
                "Validation failed for tool \"{NAME}\":\n{}\n\nReceived arguments:\n{received}",
                errors.join("\n")
            );
            return (plain(text, true), Usage::default());
        }
    };
    let discovery = discover(&parent.sources, parent.root, given.scope);
    let warnings = match discovery.diagnostics.is_empty() {
        true => String::new(),
        false => {
            let list: Vec<String> = discovery
                .diagnostics
                .iter()
                .map(|d| format!("- {d}"))
                .collect();
            format!("\n\nAgent discovery warnings:\n{}", list.join("\n"))
        }
    };
    let Some(mode) = given.mode else {
        let available: Vec<String> = discovery
            .agents
            .iter()
            .map(|agent| format!("{} ({})", agent.name, agent.source))
            .collect();
        let available = match available.is_empty() {
            true => "none".into(),
            false => available.join(", "),
        };
        let text = format!(
            "Invalid parameters. Provide exactly one mode.\nAvailable agents: {available}{warnings}"
        );
        return (plain(text, false), Usage::default());
    };
    let find = |name: &str| discovery.agents.iter().find(|agent| agent.name == name);
    // Unlike Scramjet, approval is remembered for the project's agents as they are then, and the
    // model cannot skip it (Scramjet's `confirmProjectAgents` is not offered and is ignored).
    let (mut names, mut directories): (Vec<&str>, Vec<String>) = (Vec::new(), Vec::new());
    for task in mode.tasks() {
        let Some(agent) = find(&task.agent).filter(|agent| agent.source == "project") else {
            continue;
        };
        if !names.contains(&agent.name.as_str()) {
            names.push(&agent.name);
        }
        let directory = agent
            .path
            .parent()
            .unwrap_or(Path::new(""))
            .display()
            .to_string();
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    let project: Vec<&Definition> = (discovery.agents.iter())
        .filter(|agent| agent.source == "project")
        .collect();
    let approved = Approvals::read(parent.approvals.filter(|_| !names.is_empty()));
    let unapproved: Vec<&str> = (project.iter())
        .filter(|agent| !approved.approved(agent))
        .map(|agent| agent.name.as_str())
        .collect();
    if !names.is_empty() && !unapproved.is_empty() {
        let changed = match project.iter().any(|agent| approved.known(agent)) {
            true => format!(
                "\nAdded or changed since you approved them: {}",
                unapproved.join(", ")
            ),
            false => String::new(),
        };
        ui.question(&format!(
            "Agents: {}\nSource: {}{changed}\n\nProject agents are repo-controlled. Only \
continue for trusted repositories. Yes is remembered for this project until one of its agent files \
is added or changed.",
            names.join(", "),
            directories.join(", ")
        ));
        let options = ["Yes", "No"].map(super::Choice::plain);
        if ui.choose("Run project-local agents?", &options, None) != Some(0) {
            let text = "Canceled: project-local agents not approved.".to_owned();
            return (plain(text, false), Usage::default());
        }
        if let Some(path) = parent.approvals
            && let Err(error) = approvals::remember(path, &project)
        {
            ui.notice(&format!(
                "The approval is not remembered: {}: {error}",
                path.display()
            ));
        }
    }
    if let Mode::Parallel(tasks) = &mode
        && tasks.len() > MAX_PARALLEL_TASKS
    {
        let text = format!(
            "Too many parallel tasks ({}). Max is {MAX_PARALLEL_TASKS}.",
            tasks.len()
        );
        return (plain(text, false), Usage::default());
    }
    let unknown = |name: &str| {
        let available: Vec<String> = discovery
            .agents
            .iter()
            .map(|agent| format!("\"{}\"", agent.name))
            .collect();
        let available = match available.is_empty() {
            true => "none".into(),
            false => available.join(", "),
        };
        format!("Unknown agent: \"{name}\". Available agents: {available}.{warnings}")
    };
    let sandbox = sandbox::available();
    let job = |task, text| Job::new(task, text, &discovery.agents, parent, &sandbox);
    ui.tool_running(std::time::Instant::now());
    let mut shown = Children {
        mode: match mode {
            Mode::Single(_) => Delegation::Single,
            Mode::Parallel(_) => Delegation::Parallel,
            Mode::Chain(_) => Delegation::Chain,
        },
        runs: Vec::new(),
    };
    let mut runner =
        |jobs: &[Job], shown: &mut Children| run_jobs(jobs, parent, &unknown, ui, shown, cancel);
    let (text, failed, runs) = match &mode {
        Mode::Single(task) => {
            let runs = runner(&[job(task, task.task.clone())], &mut shown);
            let run = &runs[0];
            let text = match &run.error {
                Some((reason, _)) => {
                    format!("Agent {}: {}", reason.unwrap_or("failed"), run.result())
                }
                None => run.result(),
            };
            (text, run.error.is_some(), runs)
        }
        Mode::Chain(steps) => {
            let mut runs: Vec<Run> = Vec::new();
            let mut previous = String::new();
            let mut ended = None;
            for (index, step) in steps.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    ended = Some("Chain was aborted before the next step".to_owned());
                    break;
                }
                let task = step.task.replace("{previous}", &previous);
                let run = runner(&[job(step, task)], &mut shown).remove(0);
                if run.error.is_some() {
                    let text = format!(
                        "Chain stopped at step {} ({}): {}",
                        index + 1,
                        step.agent,
                        run.result()
                    );
                    runs.push(run);
                    ended = Some(text);
                    break;
                }
                previous = run.output.clone();
                runs.push(run);
            }
            match ended {
                Some(text) => (text, true, runs),
                None => {
                    let last = runs
                        .last()
                        .map(|run| run.output.clone())
                        .unwrap_or_default();
                    let last = if last.is_empty() {
                        "(no output)".into()
                    } else {
                        last
                    };
                    (with_warnings(last, &runs), false, runs)
                }
            }
        }
        Mode::Parallel(tasks) => {
            let jobs: Vec<Job> = tasks
                .iter()
                .map(|task| job(task, task.task.clone()))
                .collect();
            let runs = runner(&jobs, &mut shown);
            let succeeded = runs.iter().filter(|run| run.error.is_none()).count();
            let summaries: Vec<String> = runs
                .iter()
                .map(|run| {
                    let ended = if run.error.is_some() {
                        "failed"
                    } else {
                        "completed"
                    };
                    format!("[{}] {ended}: {}", run.agent, run.result())
                })
                .collect();
            let aborted = cancel.load(Ordering::Relaxed);
            let title = if aborted {
                "Parallel aborted"
            } else {
                "Parallel"
            };
            let text = format!(
                "{title}: {succeeded}/{} succeeded\n\n{}",
                runs.len(),
                summaries.join("\n\n")
            );
            (text, aborted || succeeded < runs.len(), runs)
        }
    };
    let mut usage = Usage::default();
    for run in &runs {
        usage.add(&run.usage);
    }
    let output = Output {
        text,
        detail: Some(ToolDetail::Subagents(shown)),
        failed,
        image: None,
    };
    (output, usage)
}

/// What a running child reports.
enum Event {
    Item(ChildItem),
    /// Its usage so far, after each response.
    Usage(ChildUsage),
    Done(Run),
}

/// Runs `jobs`, at most `MAX_CONCURRENCY` at a time and within the process's `CHILDREN`, each
/// on a thread of its own; this thread adds them to `shown` and shows what they do. A job not
/// started before a cancellation is reported aborted.
fn run_jobs(
    jobs: &[Job],
    parent: &Parent,
    unknown: &(dyn Fn(&str) -> String + Sync),
    ui: &mut dyn AgentUi,
    shown: &mut Children,
    cancel: &AtomicBool,
) -> Vec<Run> {
    let first = shown.runs.len();
    shown.runs.extend(jobs.iter().map(Job::shown));
    ui.subagents(shown);
    let mut runs: Vec<Option<Run>> = jobs.iter().map(|_| None).collect();
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel::<(usize, Event)>();
    std::thread::scope(|scope| {
        for _ in 0..jobs.len().min(MAX_CONCURRENCY) {
            let (sender, next) = (sender.clone(), &next);
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= jobs.len() || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let job = &jobs[index];
                    let run = match job.agent {
                        Some(agent) => {
                            let Some(_slot) = CHILDREN.take(cancel) else {
                                break;
                            };
                            run_child(job, agent, parent, cancel, &mut |event| {
                                let _ = sender.send((index, event));
                            })
                        }
                        None => Run::failed(job.name, None, unknown(job.name)),
                    };
                    let _ = sender.send((index, Event::Done(run)));
                }
            });
        }
        drop(sender);
        for (index, event) in receiver {
            let child = &mut shown.runs[first + index];
            match event {
                Event::Item(item) => child.items.push(item),
                Event::Usage(usage) => child.usage = usage,
                Event::Done(run) => {
                    child.state = run.shown_state();
                    child.usage = run.shown_usage();
                    child.responses = run.responses.clone();
                    runs[index] = Some(run);
                }
            }
            ui.subagents(shown);
        }
    });
    let runs: Vec<Run> = runs
        .into_iter()
        .zip(jobs)
        .map(|(run, job)| {
            run.unwrap_or_else(|| {
                Run::failed(
                    job.name,
                    Some("aborted"),
                    "Subagent was aborted before launch".into(),
                )
            })
        })
        .collect();
    for (child, run) in shown.runs[first..].iter_mut().zip(&runs) {
        child.state = run.shown_state();
    }
    runs
}

/// The effort a child asks for: the one requested, at most the parent's, else the parent's.
fn child_effort(requested: Option<&str>, parent: Option<&str>) -> Option<String> {
    let rank = |level: &str| {
        let level = if level == "none" { "off" } else { level };
        LEVELS.iter().position(|known| *known == level)
    };
    let effort = match (requested, parent) {
        (Some(requested), Some(parent)) => match (rank(requested), rank(parent)) {
            (Some(wanted), Some(allowed)) if wanted > allowed => parent,
            _ => requested,
        },
        (requested, parent) => requested.or(parent)?,
    };
    // Scramjet's `off` is the endpoint's `none`.
    Some(if effort == "off" { "none" } else { effort }.to_owned())
}

/// The child's tools: those its file lists, or else the read-only ones of the parent's `inherited`,
/// that are read-only and available, but those it disallows, with warnings about the others.
fn child_tools(
    agent: &Definition,
    inherited: &[String],
    sandbox: &Result<(), String>,
) -> (Vec<&'static str>, Vec<String>) {
    let inherited: Vec<String> = (inherited.iter())
        .filter(|tool| READ_ONLY.contains(&tool.as_str()))
        .cloned()
        .collect();
    let listed = agent.tools.as_deref().unwrap_or(&inherited);
    let listed = listed
        .iter()
        .filter(|tool| !agent.disallowed.contains(tool));
    let mut tools = Vec::new();
    let (mut writers, mut unconfined, mut missing) = (Vec::new(), Vec::new(), Vec::new());
    for tool in listed {
        match READ_ONLY.iter().find(|known| *known == tool) {
            Some(&"bash") if sandbox.is_err() => unconfined.push(tool.as_str()),
            Some(known) if !tools.contains(known) => tools.push(*known),
            Some(_) => {}
            None if matches!(tool.as_str(), "write" | "edit") => writers.push(tool.as_str()),
            None if cfg!(windows) && sandbox.is_err() && tools::is_shell(tool) => {
                unconfined.push(tool.as_str())
            }
            None => missing.push(tool.as_str()),
        }
    }
    let mut warnings = agent.diagnostics.clone();
    if !writers.is_empty() {
        warnings.push(format!(
            "{}: left out, as subagents run read-only here",
            writers.join(", ")
        ));
    }
    if let Err(error) = sandbox
        && !unconfined.is_empty()
    {
        warnings.push(format!(
            "{}: left out, as the read-only sandbox is unavailable ({error})",
            unconfined.join(", ")
        ));
    }
    if !missing.is_empty() {
        warnings.push(format!(
            "{}: not available to subagents here",
            missing.join(", ")
        ));
    }
    (tools, warnings)
}

/// Runs `job` to its final reply, as Scramjet's child process runs its task: a conversation of
/// its own that starts with `Task: <task>`, the agent's file added to the instructions.
/// `report` receives its replies' texts and tool calls, and its usage after each response.
fn run_child(
    job: &Job,
    agent: &Definition,
    parent: &Parent,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(Event),
) -> Run {
    let tools = &job.tools;
    let mut run = Run::new(job.name);
    run.warnings = job.warnings.clone();
    let executor = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(executor) => executor,
        Err(error) => {
            run.error = Some((Some("error"), error.to_string()));
            return run;
        }
    };
    let info = &job.info;
    let effort = &job.effort;
    let instructions = instructions::child(
        &job.cwd,
        tools,
        &agent.prompt,
        parent.skills,
        parent.agent_directory,
        &sandbox::Hidden::stores(parent.providers.credential_files()),
    );
    let mut input = vec![json!({"role":"user","content":format!("Task: {}", job.task)})];
    for _ in 0..MAX_STEPS {
        if cancel.load(Ordering::Relaxed) {
            run.error = Some((Some("aborted"), "Subagent was aborted".into()));
            return run;
        }
        let mut request = json!({
            "model": info.id,
            "instructions": instructions,
            "input": input,
            "tools": tools::child_definitions(tools),
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": format!("{}:{}", parent.cache_key, agent.name),
            "reasoning": {"summary": "auto"},
        });
        if let Some(effort) = &effort {
            request["reasoning"]["effort"] = json!(info.fit(effort));
        }
        if let Some(tokens) = parent.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        let requests = &mut run.usage.requests;
        let sent = request::send(
            &executor,
            info,
            parent.retries,
            &request,
            false,
            cancel,
            &mut |event| {
                if let Sending::Sent = event {
                    *requests += 1;
                }
            },
        );
        let reply = match sent {
            Ok(reply) => reply,
            Err(AgentError::Cancelled) => {
                run.error = Some((Some("aborted"), "Subagent was aborted".into()));
                return run;
            }
            Err(error) => {
                run.error = Some((Some("error"), error.to_string()));
                return run;
            }
        };
        run.turns += 1;
        run.usage.count(&reply);
        run.responses.push(json!({
            "usage": super::turn::saved_usage(&reply),
            "timing": reply.timing.as_ref().map(super::turn::saved_timing),
        }));
        if let Some(path) = parent.rates {
            // The rates are only shown; a child goes on without them.
            let _ = rates::record(path, &job.model, &reply);
        }
        report(Event::Usage(run.shown_usage()));
        let mut calls = Vec::new();
        for item in reply.items {
            match item["type"].as_str() {
                Some("message") => {
                    let text = super::message_text(&item);
                    if !text.is_empty() {
                        run.output = text.clone();
                        report(Event::Item(ChildItem::Text(text)));
                    }
                }
                Some("function_call") => calls.push(item.clone()),
                _ => {}
            }
            input.push(item);
        }
        if let Some(message) = reply.incomplete {
            run.error = Some((Some("error"), message));
            return run;
        }
        if calls.is_empty() {
            return run;
        }
        let hidden = sandbox::Hidden::current(parent.providers.credential_files());
        for item in calls {
            let output = match Call::parse(&item) {
                Ok(call) => {
                    report(Event::Item(ChildItem::Call {
                        name: call.name.to_owned(),
                        summary: call.summary(),
                    }));
                    match tools.contains(&call.name) {
                        true => call
                            .run_read_only(&job.cwd, &hidden, cancel)
                            .map(|output| output.sent())
                            .unwrap_or_else(|error| json!(format!("Error: {error}"))),
                        false => json!(format!("Tool {} not found", call.name)),
                    }
                }
                Err(error) => json!(format!("Error: {error}")),
            };
            input.push(
                json!({"type":"function_call_output","call_id":item["call_id"],"output":output}),
            );
        }
    }
    run.error = Some((Some("error"), AgentError::TooManySteps.to_string()));
    run
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::agent::{ProblemFile, tests::ScratchDir};

    #[cfg(unix)]
    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[cfg(unix)]
    fn agent_file(name: &str, extra: &str) -> String {
        format!("---\nname: {name}\ndescription: {name} agent\n{extra}---\n\nBe {name}.\n")
    }

    #[test]
    fn agent_files_are_read_as_scramjet_reads_them() {
        let path = Path::new("/a/x.md");
        let agent = parse(
            path,
            "\u{feff}---\nname: x\ndescription:  Does x \ntools: read, , ls ,bash\nmodel: m2\n\
disallowedTools: bash\n---\n\nBody\n",
            "user",
        )
        .unwrap();
        assert_eq!(
            (
                agent.name.as_str(),
                agent.description.as_str(),
                agent.prompt.as_str()
            ),
            ("x", "Does x", "Body")
        );
        assert_eq!(
            agent.tools,
            Some(vec!["read".into(), "ls".into(), "bash".into()])
        );
        assert_eq!(agent.model.as_deref(), Some("m2"));
        assert_eq!(agent.disallowed, ["bash"]);
        assert!(agent.diagnostics.is_empty());
        let odd = parse(
            path,
            "---\nname: x\ndescription: d\ntools: [read]\nmodel: 3\ndisallowedTools: 1\n---\n",
            "user",
        )
        .unwrap();
        assert_eq!((odd.tools, odd.model), (None, None));
        assert!(odd.disallowed.is_empty());
        assert_eq!(
            odd.diagnostics,
            [
                "/a/x.md: ignoring non-string tools frontmatter",
                "/a/x.md: ignoring non-string disallowedTools frontmatter",
                "/a/x.md: ignoring non-string model frontmatter"
            ]
        );
        for content in [
            "---\nname: x\n---\nBody",
            "no frontmatter",
            "---\nname: x\ndescription: \" \"\n---\n",
        ] {
            assert_eq!(
                parse(path, content, "user").unwrap_err(),
                "/a/x.md: frontmatter must include string name and description"
            );
        }
        let error = parse(path, "---\nname: [x\n---\n", "user").unwrap_err();
        assert!(
            error.starts_with("/a/x.md: invalid YAML frontmatter ("),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_follows_the_scope_and_command_sets_replace_loose_agents() {
        let base = std::env::temp_dir().join(format!("scramjet-subagents-{}", std::process::id()));
        let _scratch = ScratchDir::new(base.clone());
        let (user, global, root) = (base.join("user"), base.join("sets"), base.join("project"));
        let installed = base.join("installed/mach12");
        write(&user.join("a.md"), &agent_file("a", ""));
        write(
            &user.join("shared.md"),
            &agent_file("shared", "tools: read\n"),
        );
        write(&user.join("mach12:kept.md"), &agent_file("mach12:kept", ""));
        write(&user.join("mach10:old.md"), &agent_file("mach10:old", ""));
        write(&user.join("renamed.md"), &agent_file("x:renamed", ""));
        write(&user.join("notes.txt"), "not an agent");
        write(&user.join("bad.md"), "---\nname: bad\n---\n");
        std::os::unix::fs::symlink(base.join("missing.md"), user.join("gone.md")).unwrap();
        std::os::unix::fs::symlink(base.join("missing.md"), user.join("mach12:gone.md")).unwrap();
        std::os::unix::fs::symlink(user.join("a.md"), user.join("mach12:linked.md")).unwrap();
        write(
            &root.join(".scramjet/agents/shared.md"),
            &agent_file("shared", "tools: ls\n"),
        );
        write(&root.join(".scramjet/agents/b.md"), &agent_file("b", ""));
        write(
            &installed.join("agents/mach12:reviewer.md"),
            &agent_file("mach12:reviewer", ""),
        );
        // A link to the set's own file is the set's agent: it is left out without a warning.
        std::os::unix::fs::symlink(
            installed.join("agents/mach12:reviewer.md"),
            user.join("mach12:reviewer.md"),
        )
        .unwrap();
        write(&global.join("x/agents/x:c.md"), &agent_file("x:c", ""));
        write(&global.join("x/agents/y:d.md"), &agent_file("y:d", ""));
        write(&global.join("x/agents/x:e.md"), &agent_file("e", ""));
        write(
            &global.join("mach12/agents/mach12:kept.md"),
            &agent_file("mach12:kept", ""),
        );
        write(
            &root.join(".scramjet/x/agents/x:c.md"),
            &agent_file("x:c", ""),
        );
        write(
            &root.join(".scramjet/z/agents/z:a.md"),
            &agent_file("z:a", ""),
        );
        let sets = Sets::find(
            std::slice::from_ref(&installed),
            Some(&global),
            &root.join(".scramjet"),
        );
        let sources = Sources {
            user: Some(&user),
            sets: &sets,
        };
        let cwd = root.join("src/deep");
        std::fs::create_dir_all(&cwd).unwrap();
        let found = |scope| {
            let discovery = discover(&sources, &cwd, scope);
            let agents: Vec<String> = discovery
                .agents
                .iter()
                .map(|agent| format!("{} {}", agent.name, agent.source))
                .collect();
            (agents, discovery.diagnostics)
        };
        let texts = |problems: Vec<Problem>| -> Vec<String> {
            problems.into_iter().map(|problem| problem.text).collect()
        };
        let (user_scope, diagnostics) = found(Scope::User);
        // Each is about its file, the broken link's too, which `/settings` offers to remove.
        let gone = Some(ProblemFile::Path(user.join("mach12:gone.md")));
        assert!(diagnostics.iter().any(|problem| problem.file == gone));
        assert!(diagnostics.iter().all(|problem| problem.file.is_some()));
        let diagnostics = texts(diagnostics);
        assert_eq!(
            user_scope,
            [
                "a user",
                "mach10:old user",
                "shared user",
                "mach12:reviewer installed",
                "x:c global",
                "x:e global",
                "z:a project"
            ]
        );
        let shown = |path: PathBuf| path.display().to_string();
        let claimed = |file: &str, set: &str, origin: &str, directory: PathBuf| {
            format!(
                "skipping user agent {}: names starting with \"{set}:\" belong to the {origin} \
command set {}",
                shown(user.join(file)),
                shown(directory)
            )
        };
        let broken = format!(
            "skipping user agent {} (a broken link to {}): names starting with \"mach12:\" belong \
to the installed command set {}",
            shown(user.join("mach12:gone.md")),
            shown(base.join("missing.md")),
            shown(installed.clone())
        );
        assert_eq!(
            diagnostics,
            [
                format!(
                    "{}: frontmatter must include string name and description",
                    shown(user.join("bad.md"))
                ),
                format!(
                    "{}: failed to read agent file (No such file or directory (os error 2))",
                    shown(user.join("gone.md"))
                ),
                broken,
                claimed("mach12:kept.md", "mach12", "installed", installed.clone()),
                format!(
                    "skipping user agent {} (a link to {}): names starting with \"mach12:\" \
belong to the installed command set {}",
                    shown(user.join("mach12:linked.md")),
                    shown(user.join("a.md")),
                    shown(installed.clone())
                ),
                claimed("renamed.md", "x", "global", global.join("x")),
                format!(
                    "skipping agent {}: y:d.md: filename must not name another set than \"x\"",
                    shown(global.join("x/agents/y:d.md"))
                ),
                format!(
                    "skipping project agent x:c at {}: name already registered from {}",
                    shown(root.join(".scramjet/x/agents/x:c.md")),
                    shown(global.join("x/agents/x:c.md"))
                ),
            ]
        );
        assert_eq!(
            found(Scope::Project).0,
            [
                "b project",
                "shared project",
                "mach12:reviewer installed",
                "x:c global",
                "x:e global",
                "z:a project"
            ]
        );
        let (both, _) = found(Scope::Both);
        assert_eq!(
            both,
            [
                "a user",
                "mach10:old user",
                "shared project",
                "b project",
                "mach12:reviewer installed",
                "x:c global",
                "x:e global",
                "z:a project"
            ]
        );
        // As Scramjet's, the catalog lists the command sets' agents only.
        let agents = registered(&sets, &mut Vec::new());
        assert_eq!(
            catalog(&agents),
            "# Available subagents\n\nThe following agents can be dispatched via the `subagent` \
tool:\n\n- mach12:reviewer: mach12:reviewer agent\n- x:c: x:c agent\n- x:e: e agent\n- z:a: z:a agent"
        );
        assert_eq!(catalog(&[]), "");
    }

    #[test]
    fn a_child_asks_for_its_effort_at_most_at_the_parents() {
        let cases = [
            (Some("high"), Some("low"), Some("low")),
            (Some("minimal"), Some("high"), Some("minimal")),
            (Some("off"), Some("high"), Some("none")),
            (Some("max"), Some("none"), Some("none")),
            (None, Some("medium"), Some("medium")),
            (Some("xhigh"), None, Some("xhigh")),
            (None, None, None),
        ];
        for (requested, parent, expected) in cases {
            assert_eq!(
                child_effort(requested, parent).as_deref(),
                expected,
                "{requested:?} {parent:?}"
            );
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn children_get_only_the_read_only_tools_they_list() {
        let agent = |tools: Option<&str>, disallowed: &str| Definition {
            name: "x".into(),
            description: "d".into(),
            tools: tools.map(|list| list.split(", ").map(str::to_owned).collect()),
            disallowed: (disallowed.split(", ").filter(|tool| !tool.is_empty()))
                .map(str::to_owned)
                .collect(),
            model: None,
            prompt: String::new(),
            source: "user",
            path: PathBuf::from("/x.md"),
            digest: String::new(),
            diagnostics: vec!["noted".into()],
        };
        let sandbox = Ok(());
        let all = tools::DEFAULT_TOOLS.map(str::to_owned);
        // A file listing none takes the parent's read-only tools, without a word of the others.
        let (tools, warnings) = child_tools(&agent(None, ""), &all, &sandbox);
        assert_eq!(tools, ["read", "bash", "grep", "find", "ls"]);
        assert_eq!(warnings, ["noted"]);
        let narrowed = ["read", "edit", "grep"].map(str::to_owned);
        let (tools, _) = child_tools(&agent(None, ""), &narrowed, &sandbox);
        assert_eq!(tools, ["read", "grep"]);
        let (tools, warnings) = child_tools(&agent(None, "bash, grep"), &all, &sandbox);
        assert_eq!(
            (tools, warnings),
            (vec!["read", "find", "ls"], vec!["noted".into()])
        );
        let (tools, warnings) = child_tools(
            &agent(Some("read, grep, find, ls, bash, read, web, write"), "find"),
            &narrowed,
            &sandbox,
        );
        assert_eq!(tools, ["read", "grep", "ls", "bash"]);
        assert_eq!(
            warnings,
            [
                "noted",
                "write: left out, as subagents run read-only here",
                "web: not available to subagents here"
            ]
        );
        let missing = Err("no bwrap".to_owned());
        let (tools, warnings) =
            child_tools(&agent(Some("read, grep, ls, bash"), ""), &all, &missing);
        assert_eq!(tools, ["read", "grep", "ls"]);
        assert_eq!(
            warnings,
            [
                "noted",
                "bash: left out, as the read-only sandbox is unavailable (no bwrap)"
            ]
        );
    }

    #[test]
    fn the_call_line_names_the_agents_and_their_tasks() {
        assert_eq!(
            summary(&json!({"agent":"r","task":"Review\nall of it","effort":"low"})),
            "r [Effort:low] [user]\nReview …"
        );
        assert_eq!(
            summary(
                &json!({"tasks":[{"agent":"a","task":"one"},{"agent":"b","task":"two"}],"agentScope":"both"})
            ),
            "parallel (2 tasks) [both]\na one\nb two"
        );
        assert_eq!(
            summary(
                &json!({"chain":[{"agent":"a","task":"one"},{"agent":"b","task":"use {previous}"}]})
            ),
            "chain (2 steps) [user]\n1. a one\n2. b use"
        );
        assert_eq!(summary(&json!({})), "... [user]\n...");
    }
}
