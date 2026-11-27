//! Scramjet's `subagent` tool (`subagent/index.ts`): the model hands tasks to agents defined in
//! markdown files, each run with a context of its own and its final reply as the result. Here
//! the children run in this process with the parent's endpoint, and get read-only tools:
//! `read`, `grep`, `find` and `ls`, and bash in the read-only sandbox, none of which shows the
//! user's credential stores; `write` and `edit` too, for an agent that lists them, where
//! `sandbox.subagents` lets children write, bash then in a sandbox of their own.

use super::yaml;
use super::{
    AgentError, AgentUi, ChildItem, ChildRun, ChildState, ChildUsage, Children, Delegation,
    MAX_STEPS, Problem, Skills, ToolDetail, Usage,
    approvals::{self, Approvals},
    commands::{self, Sets},
    guidance, instructions,
    providers::{Model, ModelInfo, Providers},
    rates,
    request::{self, Sending},
    sandbox,
    tools::{self, Call, Output},
};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};
use yaml_rust2::Yaml;

mod slots;
pub(super) mod worktree;

pub const NAME: &str = "subagent";

/// What a child's command that missed a file the user's `/tmp` holds is told it can do
/// (`sandbox::unseen_in_tmp`): it cannot ask for the directory, the agent that started it can
/// move the file.
fn unseen_way_out(it: &str) -> String {
    format!(
        "say so in your result, so that {it} can be put where commands see {it}, as in the project."
    )
}

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
/// The tools a child can have; none of them changes a file, the shells being in the read-only
/// sandbox.
#[cfg(not(windows))]
const READ_ONLY: [&str; 5] = ["read", "grep", "find", "ls", "bash"];
#[cfg(windows)]
const READ_ONLY: [&str; 7] = ["read", "grep", "find", "ls", "bash", "powershell", "cmd"];

pub(super) fn definition() -> Value {
    let effort = json!({"type":"string","enum":LEVELS,
        "description":"Thinking effort level for the agent subprocess. Capped at the main session's level."});
    let isolation = json!({"type":"string","enum":[worktree::ISOLATION],
        "description":"\"worktree\" runs the agent in a git worktree of its own, on a new branch \
from HEAD (uncommitted changes are not in it): removed afterwards if the agent changed nothing, \
else its path and branch are in the result. A chain's steps share one, each seeing what the \
earlier changed."});
    let item = |task: &str| {
        json!({"type":"object","properties":{
            "agent":{"type":"string","description":"Name of the agent to invoke"},
            "task":{"type":"string","description":task},
            "cwd":{"type":"string","description":"Working directory for the agent process"},
            "effort":effort,
            "isolation":isolation},
          "required":["agent","task"]})
    };
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Delegate tasks to specialized subagents with isolated context. Modes: \
single (agent + task), parallel (tasks array), chain (sequential with {previous} placeholder). \
Default loose-agent scope is \"user\" (from ~/.config/scramjet/agents). To enable loose \
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
            "effort":effort,
            "isolation":isolation}}})
}

/// Where children's worktrees are made: the state directory; nowhere in unit tests, which must
/// leave the user's alone.
pub(super) fn worktrees() -> Option<PathBuf> {
    match cfg!(test) {
        true => None,
        false => super::session::state_directory().ok(),
    }
}

/// Finishes the worktrees of the project `root` that children of runs that ended before them
/// left (`worktree::sweep`), saying what became of each.
pub(super) fn sweep_worktrees(root: &Path) -> Vec<String> {
    (worktrees().iter())
        .flat_map(|state| worktree::sweep(state, root))
        .collect()
}

/// The name here of a tool an agent file names as Claude Code does, so that one file serves both.
pub fn ours(tool: &str) -> &str {
    match tool {
        "Read" => "read",
        "Write" => "write",
        "Edit" | "MultiEdit" => "edit",
        "Bash" => "bash",
        "PowerShell" => "powershell",
        "Grep" => "grep",
        "Glob" => "find",
        "LS" => "ls",
        other => other,
    }
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
    /// Its `isolation: worktree`, as Claude Code's: a git worktree of its own for each task.
    pub worktree: bool,
    /// The file's body, added to the child's instructions.
    pub prompt: String,
    /// `user` or `project` for a loose agent; a command set's origin (`installed`, `project` or `built in`).
    pub source: &'static str,
    pub path: PathBuf,
    /// The SHA-256 of the file's text, which the user's approval of a project agent records.
    pub digest: String,
    /// What Scramjet says about frontmatter fields it ignored.
    pub diagnostics: Vec<String>,
}

impl Definition {
    /// Whether its file lists `write` or `edit` and does not disallow it.
    fn writes(&self) -> bool {
        (self.tools.iter().flatten()).any(|tool| {
            matches!(tool.as_str(), "write" | "edit") && !self.disallowed.contains(tool)
        })
    }

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
    /// The user's loose agent directories, a later one's agent replacing an earlier one's of its
    /// name: Claude Code's `~/.claude/agents`, then `~/.config/scramjet/agents` (`user_directories`).
    pub user: &'a [PathBuf],
    /// The session's command sets, whose agents are registered.
    pub sets: &'a Sets,
}

/// The user's loose agent directories (none with `--bare`): Claude Code's, then Scramjet's.
pub(super) fn user_directories() -> Vec<PathBuf> {
    if super::bare() {
        return Vec::new();
    }
    let claude = super::login::claude_directory().map(|dir| crate::os::join(&dir, CLAUDE_AGENTS));
    let ours = super::login::config_directory().map(|dir| dir.join("agents"));
    claude.into_iter().chain(ours).collect()
}

/// The directory of Claude Code's agents within its own directory or a project's `.claude`.
const CLAUDE_AGENTS: &str = "agents";

/// Whether `directory` holds Claude Code's agents, whose files are read as Claude Code reads them.
fn claude_code(directory: &Path) -> bool {
    directory
        .file_name()
        .is_some_and(|name| name == CLAUDE_AGENTS)
        && (directory.parent()).is_some_and(|parent| {
            parent.file_name().is_some_and(|name| name == ".claude")
                || super::login::claude_directory().is_some_and(|claude| claude == parent)
        })
}

/// Claude Code's project agent directories, outermost first: the `.claude/agents` of `cwd` and
/// of the directories above it up to its Git root, but those that are the user's.
fn claude_project_directories(cwd: &Path, user: &[PathBuf]) -> Vec<PathBuf> {
    let users: Vec<PathBuf> = (user.iter())
        .flat_map(|path| {
            [path.clone()]
                .into_iter()
                .chain(crate::os::canonicalize(path).ok())
        })
        .collect();
    let mut places = Vec::new();
    for directory in cwd.ancestors() {
        let agents = crate::os::join(directory, ".claude/agents");
        if !users.contains(&agents) && agents.is_dir() {
            places.push(agents);
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    places.reverse();
    places
}

/// The agents a call can use, and what was wrong with the files skipped.
#[derive(Debug, Default)]
pub(super) struct Discovery {
    pub agents: Vec<Definition>,
    pub diagnostics: Vec<Problem>,
}

/// As Scramjet's `discoverAgents`: the user's loose agents and the project's, by `scope`, then
/// the agents of the command sets, which replace loose ones. The project's are those of Claude
/// Code's `.claude/agents` from `cwd`'s Git root down to it, then of the nearest
/// `.scramjet/agents` above `cwd`; each directory's agent replaces an earlier one's of its name,
/// a project's a user's. Unlike Scramjet, a loose agent whose name is in a set's namespace
/// (`<set>:`) is skipped with a warning: such names belong to the set.
pub(super) fn discover(sources: &Sources, cwd: &Path, scope: Scope) -> Discovery {
    let mut found = Discovery::default();
    let mut projects = claude_project_directories(cwd, sources.user);
    projects.extend(project_directory(cwd));
    projects.retain(|directory| match crate::os::stranger(directory) {
        Some(owner) => {
            let text = crate::os::left_out(directory, owner);
            found.diagnostics.push(Problem::in_file(text, directory));
            false
        }
        None => true,
    });
    let mut places: Vec<(&Path, &'static str)> = Vec::new();
    if scope != Scope::Project {
        places.extend(sources.user.iter().map(|user| (user.as_path(), "user")));
    }
    if scope != Scope::User {
        places.extend(
            projects
                .iter()
                .map(|project| (project.as_path(), "project")),
        );
    }
    let mut agents: Vec<Definition> = Vec::new();
    for (directory, source) in places {
        let loaded = match claude_code(directory) {
            true => claude_loose(directory, source, &mut found.diagnostics),
            false => loose(directory, source, sources.sets, &mut found.diagnostics),
        };
        for agent in loaded {
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

/// The agents of Claude Code's agent `directory`, as Claude Code 2.1.259 reads them: its `.md`
/// files and those of its subdirectories, hidden ones included, links followed, each named by its
/// frontmatter's `name` alone, which must not start with `-` or hold `:`; a literal `\n` in
/// `description` is a line break. Of two files of a name, the later in path order wins.
fn claude_loose(
    directory: &Path,
    source: &'static str,
    diagnostics: &mut Vec<Problem>,
) -> Vec<Definition> {
    let mut files = Vec::new();
    markdown_files(directory, &mut HashSet::new(), &mut files);
    let mut agents: Vec<Definition> = Vec::new();
    for path in files {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => {
                let text = format!("{}: failed to read agent file ({error})", path.display());
                diagnostics.push(Problem::in_file(text, &path));
                continue;
            }
        };
        let mut agent = match parse(&path, &content, source) {
            Ok(agent) => agent,
            Err(error) => {
                diagnostics.push(Problem::in_file(error, &path));
                continue;
            }
        };
        if agent.name.starts_with('-') || agent.name.contains(':') {
            let text = format!(
                "skipping {source} agent {}: its name \"{}\" must not start with \"-\" or hold \":\"",
                path.display(),
                agent.name
            );
            diagnostics.push(Problem::in_file(text, &path));
            continue;
        }
        agent.description = agent.description.replace("\\n", "\n");
        diagnostics.extend(agent.problems());
        match agents.iter_mut().find(|known| known.name == agent.name) {
            Some(known) => *known = agent,
            None => agents.push(agent),
        }
    }
    agents
}

/// The `.md` files under `directory`, sorted, each directory and file once.
fn markdown_files(directory: &Path, seen: &mut HashSet<PathBuf>, files: &mut Vec<PathBuf>) {
    if !seen.insert(crate::os::canonicalize(directory).unwrap_or(directory.to_path_buf())) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            markdown_files(&path, seen, files);
        } else if path.extension().is_some_and(|extension| extension == "md")
            && path.is_file()
            && seen.insert(crate::os::canonicalize(&path).unwrap_or(path.clone()))
        {
            files.push(path);
        }
    }
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
    let frontmatter = yaml::frontmatter(&frontmatter)
        .map_err(|error| format!("{shown}: invalid YAML frontmatter ({error})"))?;
    let text = |key: &str| frontmatter[key].as_str().map(str::trim).unwrap_or_default();
    let (name, description) = (text("name"), text("description"));
    if name.is_empty() || description.is_empty() {
        return Err(format!(
            "{shown}: frontmatter must include string name and description"
        ));
    }
    let mut diagnostics = Vec::new();
    // A comma-separated string or, as Claude Code also takes, a list; `*` is all, the default.
    let mut list = |key: &str| {
        let items: Vec<&str> = match &frontmatter[key] {
            Yaml::String(list) => list.split(',').collect(),
            Yaml::Array(items) if items.iter().all(|item| item.as_str().is_some()) => {
                items.iter().filter_map(Yaml::as_str).collect()
            }
            Yaml::BadValue => return None,
            _ => {
                diagnostics.push(format!("{shown}: ignoring non-string {key} frontmatter"));
                return None;
            }
        };
        let tools: Vec<String> = (items.iter().map(|tool| tool.trim()))
            .filter(|tool| !tool.is_empty())
            .map(|tool| ours(tool).to_owned())
            .collect();
        (!tools.is_empty() && !tools.iter().any(|tool| tool == "*")).then_some(tools)
    };
    let tools = list("tools");
    let disallowed = list("disallowedTools").unwrap_or_default();
    let worktree = match &frontmatter["isolation"] {
        Yaml::String(isolation) if isolation.trim() == worktree::ISOLATION => true,
        Yaml::BadValue => false,
        _ => {
            diagnostics.push(format!(
                "{shown}: ignoring isolation frontmatter other than \"worktree\""
            ));
            false
        }
    };
    let model = match &frontmatter["model"] {
        // Claude Code's `inherit`: the parent's model, as without one.
        Yaml::String(model) if model.trim().eq_ignore_ascii_case("inherit") => None,
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
        worktree,
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
    worktree: bool,
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

    fn isolation(&mut self, value: &Value, path: &str) -> bool {
        let isolation = self.string(value, path, false);
        if isolation
            .as_deref()
            .is_some_and(|given| given != worktree::ISOLATION)
        {
            let error = format!("  - {path}: must be equal to one of the allowed values");
            self.errors.push(error);
        }
        isolation.is_some()
    }

    fn task(&mut self, item: &Value, path: &str) -> Option<Task> {
        let agent = self.string(&item["agent"], &format!("{path}/agent"), true);
        let task = self.string(&item["task"], &format!("{path}/task"), true);
        let cwd = self.string(&item["cwd"], &format!("{path}/cwd"), false);
        let effort = self.effort(&item["effort"], &format!("{path}/effort"));
        let worktree = self.isolation(&item["isolation"], &format!("{path}/isolation"));
        Some(Task {
            agent: agent?,
            task: task?,
            cwd,
            effort,
            worktree,
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
    let worktree = checked.isolation(&arguments["isolation"], "isolation");
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
            worktree,
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

/// The call line's text, as Scramjet's `renderCall` while the call runs: the agents, a scope and
/// an effort given, then each task's first line. The default scope is left out: it says nothing
/// of a command set's agents, which every scope finds.
pub(super) fn summary(arguments: &Value) -> String {
    let scope = match arguments["agentScope"].as_str() {
        Some(scope) => format!(" [{scope}]"),
        None => String::new(),
    };
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
            "{title} ({} {}){scope}",
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
            format!("{agent}{}{scope}{task}", effort(arguments))
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
    /// The state directory under which children's worktrees are made; none, none made.
    pub worktrees: Option<&'a Path>,
    /// The agent's coding tools, whose read-only ones a child whose file lists none has.
    pub tools: &'a [String],
    /// The paths the user's settings hide and re-open for the agent's own tools.
    pub reads: &'a sandbox::Reads,
    /// How children's tools are confined: their network, and what is hidden and re-opened.
    pub subagents: &'a sandbox::Subagents,
    /// The agent's writable directories (`Sandbox::writable_roots`, or `Agent::set_writable`'s
    /// without a sandbox), which children's reach keeps.
    pub writable: Vec<PathBuf>,
    /// The agent's own sandbox, outside which no child writes.
    pub sandbox: Option<&'a sandbox::Sandbox>,
}

impl Parent<'_> {
    /// What Scramjet gave the agent to read, which the project-only reach keeps: the command
    /// sets', skills' and the user's agents' directories, this project's sessions, and the Git
    /// directories of a linked worktree it is in.
    pub(super) fn given_to_read(&self) -> Vec<PathBuf> {
        let sets = (self.sources.sets.list().iter()).map(|set| set.directory.clone());
        let skills =
            (self.skills.list().iter()).filter_map(|skill| skill.path.parent().map(Path::to_owned));
        let agents = self.sources.user.iter().cloned();
        let sessions = super::session::directory(self.root).ok();
        let root = crate::os::canonicalize(self.root).unwrap_or_else(|_| self.root.to_owned());
        let worktrees = (self.worktrees).map(|state| worktree::directory(state, &root));
        let git = sandbox::linked_git(&root, &[]);
        (sets
            .chain(skills)
            .chain(agents)
            .chain(sessions)
            .chain(worktrees)
            .chain(git))
        .collect()
    }
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
    /// Where its `write`, `edit` and commands may change files, when it has `write` or `edit`.
    writer: Option<sandbox::Sandbox>,
    warnings: Vec<String>,
    /// The worktree it runs in, with `isolation: worktree`.
    worktree: Option<worktree::Worktree>,
    /// A chain step's: its worktree came from an earlier step, and is finished with the chain.
    shared: Option<bool>,
    /// Why it cannot run, such as a worktree not made.
    failure: Option<String>,
}

impl<'a> Job<'a> {
    /// A chain's step passes the chain's worktree as `shared`, taken if the step asks for one
    /// (made if none is yet) and given back with the job.
    fn new(
        task: &'a Task,
        text: String,
        agents: &'a [Definition],
        parent: &Parent,
        sandbox: &Result<(), String>,
        mut shared: Option<&mut Option<worktree::Worktree>>,
    ) -> Self {
        let agent = agents.iter().find(|agent| agent.name == task.agent);
        let mut cwd = match &task.cwd {
            Some(cwd) => tools::resolve(parent.root, cwd),
            None => parent.root.to_owned(),
        };
        let (mut worktree, mut failure) = (None, None);
        let earlier = shared.as_deref_mut().and_then(Option::take);
        let continued = earlier.is_some();
        if agent.is_some_and(|agent| agent.worktree) || task.worktree && agent.is_some() {
            let made = match (earlier, parent.worktrees) {
                (Some(earlier), _) => match earlier.inside(&cwd) {
                    Ok(inside) => Ok((earlier, inside)),
                    Err(error) => {
                        if let Some(shared) = shared.as_deref_mut() {
                            *shared = Some(earlier);
                        }
                        Err(error)
                    }
                },
                (None, Some(state)) => worktree::Worktree::add(parent.root, &cwd, state),
                (None, None) => Err("isolation \"worktree\": no state directory".to_owned()),
            };
            match made {
                Ok((made, inside)) => (worktree, cwd) = (Some(made), inside),
                Err(error) => failure = Some(error),
            }
        } else if let (Some(earlier), Some(shared)) = (earlier, shared.as_deref_mut()) {
            *shared = Some(earlier);
        }
        let (tools, writer, warnings) = match agent {
            Some(agent) => {
                let (writes, said) = match (agent.writes(), sandbox) {
                    (false, _) => (Err(String::new()), Vec::new()),
                    _ if cfg!(windows) => (Err(WINDOWS.to_owned()), Vec::new()),
                    (true, Err(error)) => (
                        Err(format!("the sandbox is unavailable ({error})")),
                        Vec::new(),
                    ),
                    (true, Ok(())) => (parent.subagents).writer(
                        parent.root,
                        &cwd,
                        parent.sandbox,
                        worktree.is_some(),
                    ),
                };
                let (tools, mut warnings) = child_tools(agent, parent.tools, sandbox, &writes);
                warnings.extend(said);
                (tools, writes.ok(), warnings)
            }
            None => Default::default(),
        };
        let model = match agent.and_then(|agent| agent.model.as_deref()) {
            Some(named) => {
                let provider = parent
                    .providers
                    .find(parent.model)
                    .map(|m| m.provider.as_str());
                (parent.providers.family(named, provider.unwrap_or_default()))
                    .map_or_else(|| named.to_owned(), Model::reference)
            }
            None if agent.is_some() => parent.model.to_owned(),
            None => String::new(),
        };
        let model = model.as_str();
        let info = match model == parent.model {
            true => parent.info.clone(),
            false => parent.providers.info(model),
        };
        Self {
            name: &task.agent,
            agent,
            task: text,
            cwd,
            model: parent.providers.reference(model),
            info,
            effort: agent.and(child_effort(task.effort.as_deref(), parent.effort)),
            tools,
            writer,
            warnings,
            shared: shared.map(|_| continued),
            worktree,
            failure,
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
            worktree: None,
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
    /// What the parent is told of a worktree kept.
    kept: Option<String>,
}

/// What the parent is told of the job's worktree, if kept.
fn kept(job: &Job, parent: &Parent) -> Option<String> {
    let main = parent.sandbox.map(|sandbox| (sandbox, parent.root));
    job.worktree.as_ref()?.finish(main)
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
            kept: None,
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
        let text = match &self.kept {
            Some(kept) => format!("{text}\n\n{kept}"),
            None => text,
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
        images: Vec::new(),
        before: None,
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
        if ui.permit("Run project-local agents?", &options) != Some(0) {
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
    let job = |task, text, shared: Option<&mut Option<worktree::Worktree>>| {
        Job::new(task, text, &discovery.agents, parent, &sandbox, shared)
    };
    ui.tool_running(std::time::Instant::now());
    let mut shown = Children {
        mode: match mode {
            Mode::Single(_) => Delegation::Single,
            Mode::Parallel(_) => Delegation::Parallel,
            Mode::Chain(_) => Delegation::Chain,
        },
        runs: Vec::new(),
    };
    let mut runner = |jobs: &[Job], shown: &mut Children| {
        let shells = jobs.iter().flat_map(|job| &job.tools);
        if cfg!(windows) && shells.into_iter().any(|tool| tools::is_shell(tool)) {
            low_integrity_said(ui);
        }
        run_jobs(jobs, parent, &unknown, ui, shown, cancel)
    };
    let (text, failed, runs) = match &mode {
        Mode::Single(task) => {
            let runs = runner(&[job(task, task.task.clone(), None)], &mut shown);
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
            // The steps that ask for a worktree share one, so each sees what the earlier changed.
            let mut shared = None;
            for (index, step) in steps.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    ended = Some("Chain was aborted before the next step".to_owned());
                    break;
                }
                let task = step.task.replace("{previous}", &previous);
                let mut jobs = [job(step, task, Some(&mut shared))];
                let run = runner(&jobs, &mut shown).remove(0);
                if let Some(worktree) = jobs[0].worktree.take() {
                    shared = Some(worktree);
                }
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
            let main = parent.sandbox.map(|sandbox| (sandbox, parent.root));
            let kept = shared.and_then(|worktree| worktree.finish(main));
            if let Some(last) = shown.runs.last_mut() {
                last.worktree = kept.clone();
            }
            match ended {
                Some(text) => {
                    let text = std::iter::once(text).chain(kept);
                    (text.collect::<Vec<_>>().join("\n\n"), true, runs)
                }
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
                    let last = std::iter::once(last.as_str()).chain(kept.as_deref());
                    let last = last.collect::<Vec<_>>().join("\n\n");
                    (with_warnings(last, &runs), false, runs)
                }
            }
        }
        Mode::Parallel(tasks) => {
            let jobs: Vec<Job> = tasks
                .iter()
                .map(|task| job(task, task.task.clone(), None))
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
        images: Vec::new(),
        before: None,
    };
    (output, usage)
}

/// What a running child reports.
enum Event {
    Item(ChildItem),
    /// Its usage so far, after each response.
    Usage(ChildUsage),
    /// For the user: what its instructions loaded, or a problem with them.
    Notice(String),
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
                    let mut run = match (&job.failure, job.agent) {
                        (Some(failure), _) => Run::failed(job.name, Some("error"), failure.clone()),
                        (None, Some(agent)) => {
                            let Some(_slot) = CHILDREN.take(cancel) else {
                                break;
                            };
                            run_child(job, agent, parent, cancel, &mut |event| {
                                let _ = sender.send((index, event));
                            })
                        }
                        (None, None) => Run::failed(job.name, None, unknown(job.name)),
                    };
                    if job.shared.is_none() {
                        run.kept = kept(job, parent);
                    }
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
                Event::Notice(text) => ui.notice(&text),
                Event::Done(run) => {
                    child.state = run.shown_state();
                    child.usage = run.shown_usage();
                    child.responses = run.responses.clone();
                    child.worktree = run.kept.clone();
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
            run.unwrap_or_else(|| Run {
                kept: job.shared.is_none().then(|| kept(job, parent)).flatten(),
                ..Run::failed(
                    job.name,
                    Some("aborted"),
                    "Subagent was aborted before launch".into(),
                )
            })
        })
        .collect();
    for (child, run) in shown.runs[first..].iter_mut().zip(&runs) {
        child.state = run.shown_state();
        child.worktree = run.kept.clone();
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

/// Says once what the low integrity level that subagents' commands run at on Windows leaves
/// within their reach (user decision, 2026-11-10).
fn low_integrity_said(ui: &mut dyn AgentUi) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, Ordering::Relaxed) {
        ui.notice(
            "Subagents' commands run at Windows' low integrity level: they cannot change files \
             on this computer's disks, but can read the credential stores, use the network, and \
             write WSL's files and network drives",
        );
    }
}

/// Why subagents' `write` and `edit` are left out on Windows, whose low integrity level keeps
/// them off every local disk.
const WINDOWS: &str = "subagents cannot write on Windows";

/// The child's tools: those its file lists, or else the read-only ones of the parent's `inherited`,
/// that are read-only and available, and `write` and `edit` where `writes` lets it, but those it
/// disallows, with warnings about the others.
fn child_tools(
    agent: &Definition,
    inherited: &[String],
    sandbox: &Result<(), String>,
    writes: &Result<sandbox::Sandbox, String>,
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
        // Windows without Git Bash: PowerShell for an agent's shell.
        let tool = match tool.as_str() {
            "bash" if cfg!(windows) && !tools::found("bash") && tools::found("powershell") => {
                "powershell"
            }
            tool => tool,
        };
        match READ_ONLY.iter().find(|known| **known == tool) {
            Some(known) if tools::is_shell(known) && sandbox.is_err() => unconfined.push(tool),
            Some(known) if !tools::found(known) => missing.push(tool),
            Some(known) if !tools.contains(known) => tools.push(*known),
            Some(_) => {}
            None if matches!(tool, "write" | "edit") && writes.is_ok() => {
                let tool = if tool == "write" { "write" } else { "edit" };
                if !tools.contains(&tool) {
                    tools.push(tool);
                }
            }
            None if matches!(tool, "write" | "edit") => writers.push(tool),
            None => missing.push(tool),
        }
    }
    let mut warnings = agent.diagnostics.clone();
    if let Err(why) = writes
        && !writers.is_empty()
    {
        warnings.push(format!("{}: left out, as {why}", writers.join(", ")));
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
    // Its own, apart from its parent's and other children's, removed as it ends.
    let scratch = std::sync::Arc::new(sandbox::Scratch::default());
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
    let network = parent.subagents.network;
    let writer = job.writer.as_ref();
    let writable = writer.map_or_else(Vec::new, |writer| writer.writable_roots(&job.cwd));
    let reads = (parent.subagents.reads).opening(
        [parent.root.to_owned(), job.cwd.clone()]
            .into_iter()
            .chain(parent.writable.iter().cloned())
            .chain(writable.iter().cloned())
            .chain(parent.given_to_read()),
    );
    let instruction_hidden =
        sandbox::Hidden::stores(parent.providers.credential_files(), &reads.without_reach());
    let instructions = instructions::child(
        &job.cwd,
        tools,
        &agent.prompt,
        parent.skills,
        parent.agent_directory,
        &instruction_hidden,
        network,
        writer,
    );
    let directory = parent.agent_directory;
    let mut loaded: Vec<PathBuf> =
        (instructions::context_files(&job.cwd, directory, &instruction_hidden, &mut Vec::new()))
            .into_iter()
            .map(|(path, _)| guidance::real(&path))
            .collect();
    let mut task = format!("Task: {}", job.task);
    if let Some(worktree) = &job.worktree {
        let whose = match job.shared {
            Some(true) => "that this chain's earlier steps worked in, with what they changed",
            _ => "of your own",
        };
        task += &format!(
            "\n\nYou work in a git worktree of {} {whose}, at {}, on the branch {}: what is \
under {0} is at the same place under {1}. Git's own files stay read-only: leave your changes \
uncommitted.",
            worktree.top.display(),
            worktree.path.display(),
            worktree.branch
        );
    }
    let mut input = vec![json!({"role":"user","content":task})];
    for _ in 0..MAX_STEPS {
        if cancel.load(Ordering::Relaxed) {
            run.error = Some((Some("aborted"), "Subagent was aborted".into()));
            return run;
        }
        let mut request = json!({
            "model": info.id,
            "instructions": instructions,
            "input": input,
            "tools": tools::child_definitions(tools, network, &writable),
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
            let tokens = info.output.map_or(tokens, |own| tokens.min(own));
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
        let hidden =
            sandbox::Hidden::current(parent.providers.credential_files(), &reads).scratch(&scratch);
        let mut touched = Vec::new();
        for item in calls {
            let output = match Call::parse(&item) {
                Ok(call) => {
                    report(Event::Item(ChildItem::Call {
                        name: call.name.to_owned(),
                        summary: call.summary(),
                    }));
                    let ran = match (tools.contains(&call.name), writer) {
                        (false, _) => Ok(Output::failure(tools::not_found(call.name, tools))),
                        (true, Some(writer)) => {
                            call.run_in_sandbox(&job.cwd, writer, &hidden, cancel, &mut |_| {})
                        }
                        (true, None) => call.run_read_only(&job.cwd, &hidden, network, cancel),
                    };
                    if let Ok(output) = &ran
                        && !output.failed
                    {
                        touched.extend(guidance::touched(&call, &job.cwd));
                    }
                    let ran = ran.map(|mut output| {
                        if call.name == "bash" && output.failed {
                            let mut bound = writer
                                .map(|writer| writer.writable_roots(&job.cwd))
                                .unwrap_or_default();
                            bound.push(job.cwd.clone());
                            let command = call.arguments["command"].as_str().unwrap_or_default();
                            let unseen = hidden.unseen_in_tmp(command, &bound);
                            if !unseen.is_empty() {
                                output.text += &sandbox::unseen_in_tmp(&unseen, unseen_way_out);
                            }
                        }
                        output
                    });
                    ran.map(|output| output.sent())
                        .unwrap_or_else(|error| json!(format!("Error: {error}")))
                }
                Err(error) => json!(format!("Error: {error}")),
            };
            input.push(
                json!({"type":"function_call_output","call_id":item["call_id"],"output":output}),
            );
        }
        let mut problems = Vec::new();
        let brought = guidance::bring_in(
            &job.cwd,
            &touched,
            &mut loaded,
            &instruction_hidden,
            &mut problems,
        );
        for problem in problems {
            report(Event::Notice(problem.to_string()));
        }
        if let Some((files, note)) = brought {
            let names = guidance::shown(&job.cwd, &files);
            report(Event::Notice(format!("{} loaded {names}", agent.name)));
            input.push(json!({"role": "user", "content": note}));
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
            "---\nname: x\ndescription: d\ntools: [read, 2]\nmodel: 3\ndisallowedTools: 1\n---\n",
            "user",
        )
        .unwrap();
        assert_eq!((odd.tools, odd.model), (None, None));
        assert!(odd.disallowed.is_empty());
        let claude = parse(
            path,
            "---\nname: x\ndescription: d\ntools: Read, Grep, Glob, LS, Bash, WebFetch\n\
disallowedTools: Write, MultiEdit\n---\n",
            "user",
        )
        .unwrap();
        let tools = ["read", "grep", "find", "ls", "bash", "WebFetch"];
        assert_eq!(claude.tools, Some(tools.map(str::to_owned).to_vec()));
        assert_eq!(claude.disallowed, ["write", "edit"]);
        let listed = parse(
            path,
            "---\nname: x\ndescription: d\ntools: [Read, \" Bash \"]\nmodel: Inherit\n\
disallowedTools: [Write]\n---\n",
            "user",
        )
        .unwrap();
        assert_eq!(listed.tools, Some(vec!["read".into(), "bash".into()]));
        assert_eq!(
            (listed.disallowed, listed.model),
            (vec!["write".into()], None)
        );
        let all = parse(
            path,
            "---\nname: x\ndescription: d\ntools: \"*\"\n---\n",
            "user",
        );
        assert_eq!(all.unwrap().tools, None);
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
        let error = parse(path, "---\nname: x\n[\n---\n", "user").unwrap_err();
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
            &root.join(".scramjet/z/agents/z:a.md"),
            &agent_file("z:a", ""),
        );
        let sets = Sets::find(
            &[installed.clone(), global.join("x")],
            &root.join(".scramjet"),
        );
        let users = [user.clone()];
        let sources = Sources {
            user: &users,
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
                "x:c installed",
                "x:e installed",
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
                claimed("renamed.md", "x", "installed", global.join("x")),
                format!(
                    "skipping agent {}: y:d.md: filename must not name another set than \"x\"",
                    shown(global.join("x/agents/y:d.md"))
                ),
            ]
        );
        assert_eq!(
            found(Scope::Project).0,
            [
                "b project",
                "shared project",
                "mach12:reviewer installed",
                "x:c installed",
                "x:e installed",
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
                "x:c installed",
                "x:e installed",
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

    #[cfg(unix)]
    #[test]
    fn claude_codes_agents_are_found_from_the_git_root_down_and_the_users() {
        let base =
            std::env::temp_dir().join(format!("scramjet-claude-agents-{}", std::process::id()));
        let _scratch = ScratchDir::new(base.clone());
        let claude = base.join("home/.claude/agents");
        let ours = base.join("config/agents");
        let (outside, root) = (base.join("work"), base.join("work/repo"));
        let cwd = root.join("apps/api");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let claude_file = |name: &str, extra: &str| {
            format!("---\nname: {name}\ndescription: {name} one\\ntwo\n{extra}---\nBe {name}.\n")
        };
        write(&claude.join("mine.md"), &claude_file("mine", ""));
        write(
            &claude.join("both.md"),
            &claude_file("both", "model: haiku\n"),
        );
        write(&ours.join("both.md"), &agent_file("both", ""));
        write(
            &outside.join(".claude/agents/above.md"),
            &claude_file("above", ""),
        );
        write(
            &root.join(".claude/agents/team/review.md"),
            &claude_file("reviewer", "tools: [Read, Grep, Glob]\nmodel: inherit\n"),
        );
        write(
            &root.join(".claude/agents/.hidden.md"),
            &claude_file("hidden", ""),
        );
        write(
            &root.join(".claude/agents/near.md"),
            &claude_file("near", "tools: Read\n"),
        );
        write(
            &cwd.join(".claude/agents/near.md"),
            &claude_file("near", "tools: Bash\n"),
        );
        write(&root.join(".claude/agents/bad.md"), &claude_file("a:b", ""));
        write(&root.join(".claude/agents/dash.md"), &claude_file("-x", ""));
        write(&root.join(".claude/agents/notes.txt"), "not an agent");
        write(
            &root.join(".scramjet/agents/near.md"),
            &agent_file("near", ""),
        );
        let users = [claude.clone(), ours.clone()];
        let sets = Sets::find(&[], &root.join(".scramjet"));
        let sources = Sources {
            user: &users,
            sets: &sets,
        };
        let discovery = discover(&sources, &cwd, Scope::Both);
        let found: Vec<String> = (discovery.agents.iter())
            .map(|agent| format!("{} {} {}", agent.name, agent.source, agent.path.display()))
            .collect();
        let path = |path: PathBuf| path.display().to_string();
        assert_eq!(
            found,
            [
                format!("both user {}", path(ours.join("both.md"))),
                format!("mine user {}", path(claude.join("mine.md"))),
                format!(
                    "hidden project {}",
                    path(root.join(".claude/agents/.hidden.md"))
                ),
                format!(
                    "near project {}",
                    path(root.join(".scramjet/agents/near.md"))
                ),
                format!(
                    "reviewer project {}",
                    path(root.join(".claude/agents/team/review.md"))
                ),
            ]
        );
        let reviewer = &discovery.agents[4];
        assert_eq!(reviewer.description, "reviewer one\ntwo");
        assert_eq!(
            reviewer.tools,
            Some(vec!["read".into(), "grep".into(), "find".into()])
        );
        assert_eq!(reviewer.model, None);
        let problems: Vec<&str> = (discovery.diagnostics.iter())
            .map(|problem| problem.text.as_str())
            .collect();
        let bad = path(root.join(".claude/agents/bad.md"));
        let dash = path(root.join(".claude/agents/dash.md"));
        assert_eq!(
            problems,
            [
                format!(
                    "skipping project agent {bad}: its name \"a:b\" must not start with \"-\" or hold \":\""
                ),
                format!(
                    "skipping project agent {dash}: its name \"-x\" must not start with \"-\" or hold \":\""
                ),
            ]
        );
        // Without the Scramjet file, the nearest `.claude/agents` wins.
        std::fs::remove_file(root.join(".scramjet/agents/near.md")).unwrap();
        let near = discover(&sources, &cwd, Scope::Project).agents;
        let near = near.iter().find(|agent| agent.name == "near").unwrap();
        assert_eq!(near.tools, Some(vec!["bash".into()]));
        let user = discover(&sources, &cwd, Scope::User).agents;
        assert_eq!(
            user.iter()
                .map(|agent| agent.name.as_str())
                .collect::<Vec<_>>(),
            ["both", "mine"]
        );
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

    /// An agent listing `tools`, if any, but `disallowed`, with a diagnostic.
    fn agent(tools: Option<&str>, disallowed: &str) -> Definition {
        Definition {
            name: "x".into(),
            description: "d".into(),
            tools: tools.map(|list| list.split(", ").map(str::to_owned).collect()),
            disallowed: (disallowed.split(", ").filter(|tool| !tool.is_empty()))
                .map(str::to_owned)
                .collect(),
            model: None,
            worktree: false,
            prompt: String::new(),
            source: "user",
            path: PathBuf::from("/x.md"),
            digest: String::new(),
            diagnostics: vec!["noted".into()],
        }
    }

    #[test]
    #[cfg_attr(windows, ignore = "Windows' shells differ: children_on_windows_…")]
    fn children_get_only_the_read_only_tools_they_list() {
        let sandbox = Ok(());
        let read_only = Err("subagents run read-only here (sandbox.subagents.mode)".to_owned());
        let all = tools::DEFAULT_TOOLS.map(str::to_owned);
        // A file listing none takes the parent's read-only tools, without a word of the others.
        let (tools, warnings) = child_tools(&agent(None, ""), &all, &sandbox, &read_only);
        assert_eq!(tools, ["read", "bash", "grep", "find", "ls"]);
        assert_eq!(warnings, ["noted"]);
        let narrowed = ["read", "edit", "grep"].map(str::to_owned);
        let (tools, _) = child_tools(&agent(None, ""), &narrowed, &sandbox, &read_only);
        assert_eq!(tools, ["read", "grep"]);
        let (tools, warnings) = child_tools(&agent(None, "bash, grep"), &all, &sandbox, &read_only);
        assert_eq!(
            (tools, warnings),
            (vec!["read", "find", "ls"], vec!["noted".into()])
        );
        let (tools, warnings) = child_tools(
            &agent(Some("read, grep, find, ls, bash, read, web, write"), "find"),
            &narrowed,
            &sandbox,
            &read_only,
        );
        assert_eq!(tools, ["read", "grep", "ls", "bash"]);
        assert_eq!(
            warnings,
            [
                "noted",
                "write: left out, as subagents run read-only here (sandbox.subagents.mode)",
                "web: not available to subagents here"
            ]
        );
        // Where children may write, an agent listing write or edit has them; one listing none
        // still has the read-only tools alone.
        let writes = Ok(sandbox::Sandbox {
            mode: sandbox::SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
        });
        let listed = agent(Some("read, edit, write, edit, bash"), "");
        let (tools, warnings) = child_tools(&listed, &all, &sandbox, &writes);
        assert_eq!(
            (tools, warnings),
            (vec!["read", "edit", "write", "bash"], vec!["noted".into()])
        );
        let (tools, _) = child_tools(&agent(None, ""), &all, &sandbox, &writes);
        assert_eq!(tools, ["read", "bash", "grep", "find", "ls"]);
        let (tools, _) = child_tools(
            &agent(Some("read, write"), "write"),
            &all,
            &sandbox,
            &writes,
        );
        assert_eq!(tools, ["read"]);
        let missing = Err("no bwrap".to_owned());
        let (tools, warnings) = child_tools(
            &agent(Some("read, grep, ls, bash"), ""),
            &all,
            &missing,
            &read_only,
        );
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
    #[cfg(windows)]
    fn children_on_windows_get_its_shells_with_powershell_for_a_bash_not_found() {
        let shell = match tools::found("bash") {
            true => "bash",
            false => "powershell",
        };
        let none = Err(WINDOWS.to_owned());
        let (tools, warnings) =
            child_tools(&agent(Some("read, bash, cmd"), ""), &[], &Ok(()), &none);
        assert_eq!(
            (tools, warnings),
            (vec!["read", shell, "cmd"], vec!["noted".into()])
        );
        let missing = Err("x".to_owned());
        let listed = agent(Some("read, powershell, cmd"), "");
        let (tools, warnings) = child_tools(&listed, &[], &missing, &none);
        assert_eq!(tools, ["read"]);
        assert_eq!(
            warnings,
            [
                "noted",
                "powershell, cmd: left out, as the read-only sandbox is unavailable (x)"
            ]
        );
        let described = tools::child_definitions(&["bash", "powershell", "cmd"], false, &[]);
        for tool in described.as_array().unwrap() {
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains(tools::LOW_INTEGRITY), "{description}");
        }
    }

    #[test]
    fn the_call_line_names_the_agents_and_their_tasks() {
        assert_eq!(
            summary(&json!({"agent":"r","task":"Review\nall of it","effort":"low"})),
            "r [Effort:low]\nReview …"
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
            "chain (2 steps)\n1. a one\n2. b use"
        );
        assert_eq!(summary(&json!({})), "...\n...");
        assert_eq!(
            summary(&json!({"agent":"r","task":"go","agentScope":"user"})),
            "r [user]\ngo"
        );
    }
}
