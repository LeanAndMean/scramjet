//! The system instructions of the agent and of its subagents.
use super::{
    Commands, Problem, Skills, SystemPrompt, mcp, publish, question,
    sandbox::{Hidden, Sandbox},
    status, subagent, suggest, switch,
};
use std::io::Read;
use std::path::{Path, PathBuf};

const CONTEXT_FILE_BYTES: u64 = 256 << 10;
/// Scramjet's base directives, from its command framing to its release timing.
const DIRECTIVES: &str = include_str!("directives.md");

/// What a part of the instructions holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// Pi's list of tools and guidelines, or the prompt file in their place, with what is
    /// appended to it.
    Opening,
    ContextFile,
    Skills,
    /// The `# Scramjet` orientation and Scramjet's directives.
    Directives,
    Agents,
    Commands,
    Mcp,
    Sandbox,
    Identity,
    Environment,
}

/// A part of the instructions: what it holds, its name (a context file's path), and its text.
/// The parts' texts, in order, are the instructions.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub section: Section,
    pub name: String,
    pub text: String,
}

impl Part {
    pub(super) fn new(section: Section, name: &str, text: String) -> Self {
        Self {
            section,
            name: name.to_owned(),
            text,
        }
    }
}

/// The agent's instructions, in Scramjet's order: Pi's list of the `tools` offered and the
/// guidelines for them, or the opening `loaded` has in their place, the texts it appends, the
/// context files, the `skills`, Scramjet's directives, then the subagents and commands. Each
/// request adds `identity` and `environment`.
pub(super) fn agent(
    root: &Path,
    tools: &[&str],
    commands: &Commands,
    agents: &[subagent::Definition],
    skills: &Skills,
    loaded: &Loaded,
) -> Vec<Part> {
    let has = |tool: &str| tools.contains(&tool);
    let agents = if has(subagent::NAME) { agents } else { &[] };
    let mut listed: Vec<String> = super::tools::CODING_TOOLS
        .into_iter()
        .filter(|tool| has(tool))
        .map(|tool| match tool {
            "read" => "- read: Read file contents",
            "bash" => "- bash: Execute bash commands (ls, grep, find, etc.)",
            "powershell" => "- powershell: Execute PowerShell commands",
            "cmd" => "- cmd: Execute cmd.exe command lines (batch files, tools that expect cmd)",
            "edit" => {
                "- edit: Make precise file edits with exact text replacement, including multiple \
disjoint edits in one call"
            }
            "write" => "- write: Create or overwrite files",
            "grep" => "- grep: Search file contents for patterns (respects .gitignore)",
            "find" => "- find: Find files by glob pattern (respects .gitignore)",
            _ => "- ls: List directory contents",
        })
        .map(str::to_owned)
        .collect();
    let snippets = [(status::NAME, status::SNIPPET.to_owned())].into_iter();
    let snippets = snippets
        .chain(publish::TOOLS.map(|tool| (tool, publish::snippet(tool))))
        .chain([
            (question::NAME, question::SNIPPET.to_owned()),
            (subagent::NAME, subagent::SNIPPET.to_owned()),
            (switch::NAME, switch::SNIPPET.to_owned()),
            (suggest::NAME, suggest::SNIPPET.to_owned()),
            (mcp::SEARCH, mcp::SEARCH_SNIPPET.to_owned()),
            (mcp::CALL, mcp::CALL_SNIPPET.to_owned()),
        ]);
    listed.extend(
        (snippets.filter(|(tool, _)| has(tool))).map(|(tool, text)| format!("- {tool}: {text}")),
    );
    let shells: Vec<&str> = (["bash", "powershell", "cmd"].into_iter())
        .filter(|shell| has(shell))
        .collect();
    let shell_guideline = shells.first().map(|shell| {
        match ["grep", "find", "ls"].iter().any(|tool| has(tool)) {
            true => format!(
                "- Prefer grep/find/ls tools over {} for file exploration (faster, respects \
.gitignore)",
                shells.join(" or ")
            ),
            false => format!(
                "- Use {shell} for file operations like ls, rg, find, and for git, builds and tests"
            ),
        }
    });
    let mut guidelines: Vec<&str> = shell_guideline.as_deref().into_iter().collect();
    if has("read") {
        guidelines.push("- Use read to examine files instead of cat or sed.");
    }
    if has("edit") {
        guidelines.extend([
            "- Use edit for precise changes (edits[].oldText must match exactly)",
            "- When changing multiple separate locations in one file, use one edit call with \
multiple entries in edits[] instead of multiple edit calls",
            "- Each edits[].oldText is matched against the original file, not after earlier edits \
are applied. Do not emit overlapping or nested edits. Merge nearby changes into one edit.",
            "- Keep edits[].oldText as small as possible while still being unique in the file. Do \
not pad with large unchanged regions.",
        ]);
    }
    if has("write") {
        guidelines.push("- Use write only for new files or complete rewrites.");
    }
    if has("edit") || has("write") {
        guidelines.push(
            "- Prefer editing existing files. Keep changes focused on the task, and verify them \
with relevant checks.",
        );
    }
    if has(suggest::NAME) {
        guidelines.push(suggest::GUIDELINE);
    }
    guidelines.extend([
        "- Be concise in your responses",
        "- Show file paths clearly when working with files",
    ]);
    let mut text = match &loaded.opening {
        Some(opening) => opening.clone(),
        None => format!(
            "You are Scramjet, a coding agent. You help the user with software engineering \
tasks in the project directory {root}, using the tools below.

Available tools:
{tools}

Guidelines:
{guidelines}
",
            root = root.display(),
            tools = listed.join("\n"),
            guidelines = guidelines.join("\n"),
        ),
    };
    if !loaded.appended.is_empty() {
        text = format!("{}\n\n{}", text.trim_end(), loaded.appended.join("\n\n"));
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let mut parts = vec![Part::new(Section::Opening, "Opening", text)];
    parts.extend(project_context(&loaded.context));
    parts.push(Part::new(
        Section::Skills,
        "Skills",
        skills_section(has("read"), skills),
    ));
    if !loaded.no_directives {
        let directives = directives(commands, agents, has(question::NAME));
        parts.push(Part::new(
            Section::Directives,
            "Scramjet's directives",
            format!("\n{}\n{directives}\n", orientation()),
        ));
    }
    let catalogs = [
        (Section::Agents, "Subagents", subagent::catalog(agents)),
        (Section::Commands, "Commands", commands.catalog()),
    ];
    for (section, name, catalog) in catalogs {
        if !catalog.is_empty() {
            parts.push(Part::new(section, name, catalog + "\n\n"));
        }
    }
    parts.retain(|part| !part.text.is_empty());
    while let Some(last) = parts.last_mut() {
        last.text.truncate(last.text.trim_end().len());
        match last.text.is_empty() {
            true => parts.pop(),
            false => break,
        };
    }
    parts
}

/// The text of `parts`, in order.
pub(super) fn joined(parts: &[Part]) -> String {
    parts.iter().map(|part| part.text.as_str()).collect()
}

/// Scramjet's directives, each section and guideline only where it applies: command framing
/// with commands loaded, release timing with a pre-merge command to hand it to, the structural
/// mapper's guideline with that agent offered, and asking through a tool with the tool offered.
fn directives(commands: &Commands, agents: &[subagent::Definition], asks: bool) -> String {
    let command =
        |suffix: &str| (commands.list().iter()).any(|command| command.name.ends_with(suffix));
    let applies = |heading: &str| match heading {
        "# Command framing" => !commands.list().is_empty(),
        "# Release preparation timing" => command(":pr-pre-merge"),
        _ => true,
    };
    let mapper = agents
        .iter()
        .any(|agent| agent.name == "mach12:structural-mapper");
    let starts = std::iter::once(0).chain(DIRECTIVES.match_indices("\n# ").map(|(at, _)| at + 1));
    let starts: Vec<usize> = starts.chain([DIRECTIVES.len()]).collect();
    let mut text = String::new();
    for section in starts.windows(2).map(|at| &DIRECTIVES[at[0]..at[1]]) {
        if applies(section.lines().next().unwrap_or_default()) {
            text += section;
        }
    }
    if !mapper {
        let at = text.find(" - Before producing a non-trivial file-level implementation plan");
        if let Some(at) = at {
            let end = text[at..].find('\n').map_or(text.len(), |end| at + end + 1);
            text.replace_range(at..end, "");
        }
    }
    if !asks {
        text = text.replace(
            " When you ask through a tool, say it in the question itself.",
            "",
        );
    }
    text.truncate(text.trim_end().len());
    text.push('\n');
    text
}

/// The `# Scramjet` section, as Scramjet 1's.
fn orientation() -> String {
    let mut text = "# Scramjet

Scramjet is the harness you are running under: a coding agent that loads command sets —
directories of user-defined slash commands and their agents — and wires them into emergent
workflows through declared next-step policies (forced / closed / open / ask) and command
delegation. Mach 12 is one such command set (the issue → plan → review →
implement → PR → ship methodology); other sets can sit alongside it.
"
    .to_owned();
    let readme = super::builtin::readme().map(|readme| readme.display().to_string());
    let help = std::env::current_exe().ok().map(|exe| {
        format!(
            "`{} --help-all` (every option, key and built-in command)",
            exe.display()
        )
    });
    let docs: Vec<String> = [readme, help].into_iter().flatten().collect();
    if !docs.is_empty() {
        text += &format!(
            "\nScramjet documentation (read only when the user asks about Scramjet itself — \
its commands, command sets, next steps, delegation, settings, keys or the /autopilot on/off \
flag): {}\n",
            docs.join("; ")
        );
    }
    text
}

/// A model as the Model Identity names it, as its provider describes it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Identity {
    pub id: String,
    pub name: Option<String>,
    pub provider: Option<String>,
}

/// Scramjet's `# Model Identity` for a request to `current`, after replies of `contributors` on
/// the session's branch, all named by reference as `known` is keyed (a model without an entry,
/// by its name alone): the model attributes published work with it, as Mach 12's reviews ask.
/// The current model is listed among them before it has replied, so that the text stays the
/// same after its first reply, and the cached prefix with it.
pub(super) fn identity(
    current: &str,
    contributors: &[String],
    known: &std::collections::HashMap<String, Identity>,
) -> String {
    let name = |model: &str| named(model, known);
    let mut models: Vec<&str> = contributors.iter().map(String::as_str).collect();
    if !models.contains(&current) {
        models.push(current);
    }
    let listed: Vec<String> = models
        .iter()
        .map(|model| format!("- {}", name(model)))
        .collect();
    let short = (known.get(current)).map_or(current, |identity| {
        identity.name.as_deref().unwrap_or(&identity.id)
    });
    format!(
        "\n\n# Model Identity
Your model for the imminent response is: {current}.
Models with material responses in this session branch, the imminent one included:
{listed}
When posting to GitHub, use honest contribution-aware attribution:
- Credit the current model for work in the imminent response.
- Credit prior models only when their material responses contributed to the published work.
- Single credited model: \"Reviewed by {short}\"
- Multiple credited models: describe each model's contribution (e.g., \"Reviewed by X \
(analysis) and Y (posting)\")",
        current = name(current),
        listed = listed.join("\n"),
    )
}

/// A model as the Model Identity and model change notes name it.
fn named(model: &str, known: &std::collections::HashMap<String, Identity>) -> String {
    let Some(identity) = known.get(model) else {
        return format!("{model} (ID: {model})");
    };
    let id = &identity.id;
    let name = identity.name.as_deref().unwrap_or(id);
    match &identity.provider {
        Some(provider) => format!("{name} (ID: {id}, provider: {provider})"),
        None => format!("{name} (ID: {id})"),
    }
}

/// The note that marks where the model changed other than by the agent's own switch: for
/// OpenAI's protocols a developer message tagged as Codex tags it, else a `<system-reminder>`
/// on the user's side, as Claude Code gives its reminders.
pub(super) fn model_change_note(
    from: &str,
    to: &str,
    known: &std::collections::HashMap<String, Identity>,
    developer: bool,
) -> serde_json::Value {
    let (from, to) = (named(from, known), named(to, known));
    let text = format!(
        "The session's model changed here from {from} to {to}: the replies before this point \
were written by {from}, those after it by {to} until a later change."
    );
    match developer {
        true => serde_json::json!({"role":"developer",
            "content":format!("<model_switch>\n{text}\n</model_switch>")}),
        false => serde_json::json!({"role":"user",
            "content":format!("<system-reminder>\n{text}\n</system-reminder>")}),
    }
}

/// The facts that end the agent's instructions, as Scramjet's: the date, the directory and the
/// session's file, when it is saved.
pub(super) fn environment(root: &Path, journal: Option<&Path>) -> String {
    let mut facts = format!(
        "\nCurrent date: {}\nCurrent working directory: {}",
        today(),
        root.display()
    );
    if let Some(journal) = journal {
        facts += &format!("\nCurrent session journal: {}", journal.display());
    }
    facts
}

/// Pi's list of skills, only for a model that has `read` to load them.
fn skills_section(read: bool, skills: &Skills) -> String {
    match (read, skills.catalog()) {
        (true, catalog) if !catalog.is_empty() => format!("\n{catalog}"),
        _ => String::new(),
    }
}

/// A subagent's instructions, as Pi builds a child's: its tools and guidelines, the agent's own
/// text, the `context` files (`context_files` from `cwd`) and the `skills`, then the date and
/// directory. A `writer` is the sandbox in which it may change files.
pub(super) fn child(
    cwd: &Path,
    tools: &[&str],
    prompt: &str,
    skills: &Skills,
    context: &[(PathBuf, String)],
    network: bool,
    writer: Option<&Sandbox>,
) -> String {
    let sandboxed = match writer {
        Some(_) => "in a sandbox",
        None => "in a read-only sandbox",
    };
    let listed: Vec<String> = tools
        .iter()
        .map(|tool| match *tool {
            "read" => "- read: Read file contents".to_owned(),
            "grep" => "- grep: Search file contents for patterns (respects .gitignore)".to_owned(),
            "find" => "- find: Find files by glob pattern (respects .gitignore)".to_owned(),
            "ls" => "- ls: List directory contents".to_owned(),
            "write" => "- write: Create or overwrite files".to_owned(),
            "edit" => "- edit: Make precise file edits with exact text replacement".to_owned(),
            "powershell" => format!("- powershell: Execute PowerShell commands {sandboxed}"),
            "cmd" => format!("- cmd: Execute cmd.exe command lines {sandboxed}"),
            _ => format!("- bash: Execute bash commands (ls, grep, find, etc.) {sandboxed}"),
        })
        .collect();
    let mut guidelines = Vec::new();
    let explores = ["grep", "find", "ls"]
        .iter()
        .any(|tool| tools.contains(tool));
    let shells: Vec<&str> = (tools.iter().copied())
        .filter(|tool| super::tools::is_shell(tool))
        .collect();
    if let Some(shell) = shells.first() {
        guidelines.push(match explores {
            true => format!(
                "- Prefer grep/find/ls tools over {shell} for file exploration (faster, respects \
.gitignore)"
            ),
            false => format!("- Use {shell} for file operations like ls, rg, find"),
        });
        guidelines.push(match cfg!(windows) {
            true => "- Commands run at Windows' low integrity level: no file on this computer's \
disks can be changed, except in a TEMP of their own discarded after each command"
                .to_owned(),
            false => format!(
                "- bash runs {sandboxed} {}{}{}{}",
                super::sandbox::network_said(network),
                match writer {
                    Some(_) => String::new(),
                    None => format!(", writable only in {}", super::sandbox::TEMPORARY),
                },
                if super::sandbox::UNRUNNABLE.is_empty() {
                    ""
                } else {
                    "."
                },
                super::sandbox::UNRUNNABLE
            ),
        });
    }
    guidelines.push(match writer {
        None => "- No file can be changed: you only read and report".to_owned(),
        Some(writer) => {
            let listed = |paths: Vec<PathBuf>| {
                (paths.iter().map(|path| path.display().to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let kept = writer.protected_paths(cwd);
            let kept = match kept.is_empty() {
                true => String::new(),
                false => format!("; kept read-only within them: {}", listed(kept)),
            };
            format!(
                "- Files can be changed only in {} (and in {}){kept}. Your final reply says what \
you changed",
                listed(writer.writable_roots(cwd)),
                super::sandbox::TEMPORARY
            )
        }
    });
    guidelines.push("- Be concise in your responses".to_owned());
    guidelines.push("- Show file paths clearly when working with files".to_owned());
    let mut text = format!(
        "You are a subagent of Scramjet, a coding agent: you are given a task, and your final \
reply is its result.

Available tools:
{}

Guidelines:
{}",
        match listed.is_empty() {
            true => "(none)".to_owned(),
            false => listed.join("\n"),
        },
        guidelines.join("\n")
    );
    if !prompt.trim().is_empty() {
        text += &format!("\n\n{prompt}");
    }
    text += &joined(&project_context(context));
    let listed = skills_section(tools.contains(&"read"), skills);
    if !listed.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text += &listed;
    text + &format!(
        "\nCurrent date: {}\nCurrent working directory: {}",
        today(),
        cwd.display()
    )
}

/// What the agent's instructions take from files and the command line, as Scramjet's resource
/// loader reads it.
#[derive(Debug, Default)]
pub(super) struct Loaded {
    /// In place of the opening (the tools and guidelines): `--system-prompt`, else `SYSTEM.md`.
    pub opening: Option<String>,
    /// After it: the `--append-system-prompt` texts, else `APPEND_SYSTEM.md`.
    pub appended: Vec<String>,
    pub context: Vec<(PathBuf, String)>,
    /// `--no-directives`: without the `# Scramjet` orientation and Scramjet's directives.
    pub no_directives: bool,
    /// The files `opening` and `appended` were read from, each with whether it is the opening.
    pub files: Vec<(PathBuf, bool)>,
    /// The files among them that could not be read.
    pub problems: Vec<Problem>,
}

impl Loaded {
    /// The text `source` gives: the contents of the file it names, else itself; itself when it is
    /// `text`.
    fn read(&mut self, source: &str, opening: bool, text: bool, hidden: &Hidden) -> Option<String> {
        let path = Path::new(source);
        if text || !path.exists() {
            return Some(source.to_owned());
        }
        let read = hidden
            .opened(path, std::fs::File::open(path))
            .and_then(|mut file| {
                let mut text = String::new();
                (file.read_to_string(&mut text).map(|_| text))
                    .map_err(|error| format!("could not read {}: {error}", path.display()))
            });
        match read {
            Ok(text) => {
                let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
                self.files.push((path, opening));
                Some(text)
            }
            Err(text) => {
                self.problems.push(Problem::in_file(text, path));
                None
            }
        }
    }
}

/// Reads what `prompt` names for the instructions of an agent in `root`, as Scramjet does: its
/// opening's text or file, else the project's `.scramjet/SYSTEM.md`, else the agent directory's;
/// the texts or files it appends, else an `APPEND_SYSTEM.md` found the same way; and, unless it
/// leaves them out, the context files, with Claude Code's own when `claude`. A file in one of the `hidden` stores, also through a
/// symlink, is left out with a problem: nobody chose to send it.
pub(super) fn load(
    prompt: &SystemPrompt,
    root: &Path,
    agent_directory: Option<&Path>,
    claude: bool,
    hidden: &Hidden,
) -> Loaded {
    let mut loaded = Loaded {
        no_directives: prompt.no_directives,
        ..Loaded::default()
    };
    if !prompt.no_context_files {
        let problems = &mut loaded.problems;
        loaded.context = context_files(root, agent_directory, claude, hidden, problems);
    }
    let found = |name: &str, problems: &mut Vec<Problem>| {
        let directories = [
            Some(root.join(".scramjet")),
            agent_directory.map(Path::to_path_buf),
        ];
        let path = (directories.into_iter().flatten())
            .map(|directory| directory.join(name))
            .filter(|path| path.exists())
            .find(|path| match crate::os::stranger(path) {
                Some(owner) => {
                    problems.push(Problem::in_file(crate::os::left_out(path, owner), path));
                    false
                }
                None => true,
            })?;
        Some(path.to_string_lossy().into_owned())
    };
    let opening = match &prompt.system {
        Some(given) => Some((given.clone(), prompt.system_text)),
        None => found("SYSTEM.md", &mut loaded.problems).map(|file| (file, false)),
    };
    if let Some((source, text)) = opening {
        loaded.opening =
            (loaded.read(&source, true, text, hidden)).filter(|text| !text.trim().is_empty());
    }
    let (appended, texts) = match prompt.append.is_empty() {
        true => (
            (found("APPEND_SYSTEM.md", &mut loaded.problems))
                .into_iter()
                .collect(),
            false,
        ),
        false => (prompt.append.clone(), prompt.append_text),
    };
    for source in appended {
        if let Some(text) = loaded.read(&source, false, texts, hidden) {
            let text = text.trim_end();
            if !text.is_empty() {
                loaded.appended.push(text.to_owned());
            }
        }
    }
    loaded
}

/// The context files, as Scramjet's: those of the `agent_directory`, then the user's Claude Code
/// `~/.claude/CLAUDE.md` and rules, then those of `root` and the directories above it, outermost
/// first, with their contents. A directory's `CLAUDE.md` and `.claude/CLAUDE.md` come before its
/// `AGENTS.md`, which is left out when it says the same, then its `CLAUDE.local.md` and its
/// `.claude/rules`; with `--bare`, none of the home directory's. Without `claude`, none of Claude
/// Code's own (`~/.claude`'s, `.claude/CLAUDE.md`, the rules), and the `CLAUDE.md` files are read
/// as they are, imports and comments included. One in a `hidden` store is left out, said in
/// `problems`.
pub(super) fn context_files(
    root: &Path,
    agent_directory: Option<&Path>,
    claude: bool,
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Vec<(PathBuf, String)> {
    let mut files = (agent_directory
        .map(|directory| directory_context(directory, (false, claude), hidden, problems)))
    .unwrap_or_default();
    let mut above = Vec::new();
    let home = std::env::home_dir();
    let real_home = (home.as_deref()).and_then(|home| crate::os::canonicalize(home).ok());
    if let Some(own) = super::login::claude_directory().filter(|_| claude && !super::bare()) {
        if let Some(file) = context_file(&own.join("CLAUDE.md"), hidden, problems) {
            files.extend(memory(file, hidden, problems));
        }
        files.extend(rules(&own, hidden, problems));
    }
    for directory in root.ancestors() {
        let at_home = home.as_deref() == Some(directory) || real_home.as_deref() == Some(directory);
        if at_home && super::bare() {
            continue;
        }
        let own = (claude && !at_home, claude);
        above.splice(0..0, directory_context(directory, own, hidden, problems));
    }
    files.extend(above);
    // A file read twice (imported by two files, or a link to one read already) is kept first.
    let mut seen = Vec::new();
    files.retain(|(path, _)| {
        let real = crate::os::canonicalize(path).unwrap_or_else(|_| path.clone());
        let new = !seen.contains(&real);
        seen.push(real);
        new
    });
    files
}

/// The `CLAUDE.md` and `AGENTS.md` (or `.MD`) of `directory`, as Scramjet reads them, then its
/// `CLAUDE.local.md`, the user's own notes kept out of version control, as Claude Code reads it.
/// With `own`, Claude Code's `.claude/CLAUDE.md` after the `CLAUDE.md`, and its `.claude/rules`
/// last; with `imports`, the `CLAUDE.md` files read as Claude Code reads them (`memory`).
fn directory_context(
    directory: &Path,
    (claude, imports): (bool, bool),
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Vec<(PathBuf, String)> {
    let memory = |file, problems: &mut Vec<Problem>| match imports {
        true => memory(file, hidden, problems),
        false => vec![file],
    };
    let mut first = |names: &[&str]| {
        let path = (names.iter().map(|name| directory.join(name))).find(|path| path.is_file())?;
        context_file(&path, hidden, problems)
    };
    let mut claudes: Vec<_> = first(&["CLAUDE.md", "CLAUDE.MD"]).into_iter().collect();
    if claude {
        claudes.extend(first(&[".claude/CLAUDE.md"]));
    }
    let agents = first(&["AGENTS.md", "AGENTS.MD"]);
    let local = first(&["CLAUDE.local.md"]);
    let agents = agents.filter(|agents| !claudes.iter().any(|claude| claude.1 == agents.1));
    let mut files = Vec::new();
    for file in claudes {
        files.extend(memory(file, problems));
    }
    files.extend(agents);
    if let Some(file) = local {
        files.extend(memory(file, problems));
    }
    if claude {
        files.extend(rules(&directory.join(".claude"), hidden, problems));
    }
    files
}

/// The file at `path` with its contents, unless it is over the size kept, another user's or in a
/// `hidden` store (those two said in `problems`).
fn context_file(
    path: &Path,
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Option<(PathBuf, String)> {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= CONTEXT_FILE_BYTES) {
        return None;
    }
    let mut said = |problem: Problem| {
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    };
    if let Some(owner) = crate::os::stranger(path) {
        said(Problem::in_file(crate::os::left_out(path, owner), path));
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut file = match hidden.opened(path, Ok(file)) {
        Ok(file) => file.take(CONTEXT_FILE_BYTES),
        Err(why) => {
            said(Problem::in_file(why, path));
            return None;
        }
    };
    let mut contents = String::new();
    file.read_to_string(&mut contents).ok()?;
    Some((path.to_path_buf(), contents))
}

/// Claude Code's rules in `claude`'s `rules` directory loaded always: each `.md` file under it
/// whose frontmatter gives no `paths`, in the order of their paths, without its frontmatter.
fn rules(claude: &Path, hidden: &Hidden, problems: &mut Vec<Problem>) -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    for (path, globs, body) in rule_files(claude, hidden, problems) {
        if globs.is_none() {
            files.extend(memory((path, body), hidden, problems));
        }
    }
    files
}

/// The rules in `claude`'s `rules` directory whose `paths` match `file`, relative to `base`, as
/// Claude Code loads them once the agent works on that file.
fn matching_rules(
    claude: &Path,
    base: &Path,
    file: &Path,
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Vec<(PathBuf, String)> {
    if !file
        .strip_prefix(base)
        .is_ok_and(|relative| relative.components().next().is_some())
    {
        return Vec::new();
    }
    let mut files = Vec::new();
    for (path, globs, body) in rule_files(claude, hidden, problems) {
        if globs.is_some_and(|globs| matched(base, &globs, file)) {
            files.extend(memory((path, body), hidden, problems));
        }
    }
    files
}

/// Each `.md` file under `claude`'s `rules` directory, in the order of their paths, with the
/// globs its frontmatter's `paths` gives (`None` for a rule loaded always) and its text without
/// the frontmatter.
fn rule_files(
    claude: &Path,
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Vec<(PathBuf, Option<Vec<String>>, String)> {
    let mut paths = Vec::new();
    let mut seen = Vec::new();
    let mut directories = vec![claude.join("rules")];
    while let Some(directory) = directories.pop() {
        // A link back to a directory already walked is walked once.
        match crate::os::canonicalize(&directory) {
            Ok(real) if !seen.contains(&real) => seen.push(real),
            _ => continue,
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "md") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut files = Vec::new();
    for path in paths {
        let Some((path, contents)) = context_file(&path, hidden, problems) else {
            continue;
        };
        let (frontmatter, body) = super::commands::split_frontmatter(&contents);
        files.push((path, globs(&frontmatter), body));
    }
    files
}

/// The instructions Claude Code adds once the agent reads, writes or edits `file` in a session in
/// `root`, outermost first: the rules of the user's `~/.claude/rules` whose `paths` match it,
/// relative to `root`; for each directory below `root` down to the file's, its `CLAUDE.md`,
/// `.claude/CLAUDE.md`, `CLAUDE.local.md` and the rules of its `.claude/rules` loaded always or
/// matching it; and the rules matching it in the `.claude/rules` of `root` and the directories
/// above it, but the home directory's. With `--bare`, none of the user's.
pub(super) fn guidance(
    root: &Path,
    file: &Path,
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Vec<(PathBuf, String)> {
    let real = |path: &Path| crate::os::canonicalize(path).unwrap_or_else(|_| path.into());
    let (root, file) = match file.starts_with(root) {
        true => (root.to_path_buf(), file.to_path_buf()),
        false => (real(root), real(file)),
    };
    let home = std::env::home_dir();
    let at_home = |directory: &Path| {
        (home.as_deref()).is_some_and(|home| directory == home || real(directory) == real(home))
    };
    let mut files = Vec::new();
    if let Some(claude) = super::login::claude_directory().filter(|_| !super::bare()) {
        files.extend(matching_rules(&claude, &root, &file, hidden, problems));
    }
    let below: Vec<&Path> = (file.parent().into_iter())
        .flat_map(Path::ancestors)
        .take_while(|directory| *directory != root && directory.starts_with(&root))
        .collect();
    for directory in below
        .into_iter()
        .rev()
        .filter(|directory| !at_home(directory))
    {
        let mut first = |names: &[&str]| {
            let path =
                (names.iter().map(|name| directory.join(name))).find(|path| path.is_file())?;
            context_file(&path, hidden, problems)
        };
        let found = [
            first(&["CLAUDE.md", "CLAUDE.MD"]),
            first(&[".claude/CLAUDE.md"]),
            first(&["CLAUDE.local.md"]),
        ];
        for file in found.into_iter().flatten() {
            files.extend(memory(file, hidden, problems));
        }
        let claude = directory.join(".claude");
        files.extend(rules(&claude, hidden, problems));
        files.extend(matching_rules(&claude, directory, &file, hidden, problems));
    }
    let above: Vec<&Path> = root.ancestors().collect();
    for directory in above
        .into_iter()
        .rev()
        .filter(|directory| !at_home(directory))
    {
        let claude = directory.join(".claude");
        files.extend(matching_rules(&claude, directory, &file, hidden, problems));
    }
    files
}

/// How deep Claude Code follows `@path` imports: the file itself, then four hops.
const IMPORT_DEPTH: usize = 5;

/// The extensions of the files an import reads, as Claude Code's; one without an extension is
/// read too.
const IMPORTED_EXTENSIONS: &str = "md txt text json yaml yml toml xml csv html htm css scss \
    sass less js ts tsx jsx mjs cjs mts cts py pyi pyw rb erb rake go rs java kt kts scala c cpp \
    cc cxx h hpp hxx cs swift sh bash zsh fish ps1 bat cmd env ini cfg conf config properties \
    sql graphql gql proto vue svelte astro ejs hbs pug jade php pl pm lua r dart ex exs erl hrl \
    clj cljs cljc edn hs lhs elm ml mli f f90 f95 for cmake make makefile gradle sbt rst adoc \
    asciidoc org tex latex lock log diff patch";

/// A Claude Code memory `file` as Claude Code reads it, without its block-level HTML comments,
/// unless that leaves nothing, then each file it imports with `@path`, after it and depth first.
fn memory(
    file: (PathBuf, String),
    hidden: &Hidden,
    problems: &mut Vec<Problem>,
) -> Vec<(PathBuf, String)> {
    fn read(
        (path, contents): (PathBuf, String),
        depth: usize,
        hidden: &Hidden,
        problems: &mut Vec<Problem>,
        files: &mut Vec<(PathBuf, String)>,
    ) {
        let (text, imports) = claude_markdown(&contents);
        if text.trim().is_empty() {
            return;
        }
        let directory = path.parent().map(Path::to_path_buf).unwrap_or_default();
        files.push((path, text));
        if depth + 1 >= IMPORT_DEPTH {
            return;
        }
        let real = |path: &Path| crate::os::canonicalize(path).unwrap_or_else(|_| path.into());
        for import in imports {
            let path = match import.strip_prefix("~/") {
                Some(rest) => match std::env::home_dir() {
                    Some(home) => home.join(rest),
                    None => continue,
                },
                None => directory.join(&import),
            };
            let path: PathBuf = path.components().collect();
            let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            if extension.is_some_and(|e| !IMPORTED_EXTENSIONS.split(' ').any(|known| known == e))
                || files.iter().any(|(seen, _)| real(seen) == real(&path))
            {
                continue;
            }
            if let Some(file) = context_file(&path, hidden, problems) {
                read(file, depth + 1, hidden, problems, files);
            }
        }
    }
    let mut files = Vec::new();
    read(file, 0, hidden, problems, &mut files);
    files
}

/// `text` as Claude Code's lexer reads a memory file: without its block-level HTML comments (those
/// starting a top-level HTML block that closes), and the paths it imports with `@path` outside code
/// and other HTML, in order.
fn claude_markdown(text: &str) -> (String, Vec<String>) {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    // Spans left out of the search for imports, each a comment block's text once its comments
    // are cut, or `None` for code and other HTML, kept as it is.
    let mut cuts: Vec<(std::ops::Range<usize>, Option<String>)> = Vec::new();
    let mut depth = 0usize;
    let comments = regex::Regex::new("(?s)<!--.*?-->").expect("a valid pattern");
    for (event, range) in Parser::new_ext(text, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(Tag::HtmlBlock) => {
                let raw = &text[range.clone()];
                if depth == 0 && raw.trim_start().starts_with("<!--") && raw.contains("-->") {
                    let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
                    let mut end = match text[..range.end].ends_with('\n') {
                        true => range.end,
                        false => text[range.end..]
                            .find('\n')
                            .map_or(text.len(), |i| range.end + i + 1),
                    };
                    // The blank lines after it go with it, as in Claude Code's lexer.
                    while let Some(line) = text[end..].split_inclusive('\n').next()
                        && line.trim().is_empty()
                    {
                        end += line.len();
                    }
                    let rest = comments.replace_all(&text[start..end], "").into_owned();
                    cuts.push((start..end, Some(rest)));
                } else {
                    cuts.push((range, None));
                }
                depth += 1;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                cuts.push((range, None));
                depth += 1;
            }
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            Event::Code(_) | Event::InlineHtml(_) => cuts.push((range, None)),
            _ => {}
        }
    }
    cuts.sort_by_key(|(range, _)| range.start);
    let (mut kept, mut imports, mut at) = (String::new(), Vec::new(), 0);
    for (range, rest) in cuts {
        if range.start < at {
            continue;
        }
        let between = &text[at..range.start];
        kept.push_str(between);
        imports.extend(imported_paths(between));
        match rest {
            Some(rest) if rest.trim().is_empty() => {}
            Some(rest) => {
                kept.push_str(&rest);
                imports.extend(imported_paths(&rest));
            }
            None => kept.push_str(&text[range.clone()]),
        }
        at = range.end;
    }
    kept.push_str(&text[at..]);
    imports.extend(imported_paths(&text[at..]));
    (kept, imports)
}

/// The paths `text` imports, as Claude Code finds them: an `@` at the start or after white space,
/// then the path up to white space (a space escaped with `\` kept) or a `#`, starting with `./`,
/// `~/`, `/` or a letter, digit, `.`, `_` or `-`.
fn imported_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut chars = text.char_indices().peekable();
    let mut after_space = true;
    while let Some((_, c)) = chars.next() {
        if c != '@' || !after_space {
            after_space = c.is_whitespace();
            continue;
        }
        let mut path = String::new();
        while let Some(&(i, c)) = chars.peek() {
            if text[i..].starts_with("\\ ") {
                chars.next();
                chars.next();
                path.push(' ');
            } else if c.is_whitespace() || c == '\\' {
                break;
            } else {
                chars.next();
                path.push(c);
            }
        }
        after_space = false;
        let path = path.split('#').next().unwrap_or_default();
        let starts = path.starts_with("./")
            || path.starts_with("~/")
            || (path.starts_with('/') && path != "/" && !path.starts_with("//"))
            || (path.chars().next())
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if starts {
            paths.push(path.to_owned());
        }
    }
    paths
}

/// The globs a rule's `frontmatter` gives in `paths`, as Claude Code reads them: a list or a
/// comma-separated string, braces expanded, a trailing `/**` dropped. `None`, a rule loaded
/// always, when it gives none or only `**`; one that does not parse gives none.
fn globs(frontmatter: &str) -> Option<Vec<String>> {
    use yaml_rust2::Yaml;
    let Some(Yaml::Hash(fields)) = (yaml_rust2::YamlLoader::load_from_str(frontmatter).ok())
        .and_then(|documents| documents.into_iter().next())
    else {
        return None;
    };
    let given = match fields.get(&Yaml::String("paths".into()))? {
        Yaml::Array(paths) => paths.iter().filter_map(Yaml::as_str).collect(),
        Yaml::String(paths) => vec![paths.as_str()],
        _ => Vec::new(),
    };
    let globs: Vec<String> = (given.into_iter())
        .flat_map(split_globs)
        .flat_map(|glob| expand_braces(&glob))
        .map(|glob| glob.strip_suffix("/**").map(str::to_owned).unwrap_or(glob))
        .filter(|glob| !glob.is_empty())
        .collect();
    match globs.iter().all(|glob| glob == "**") {
        true => None,
        false => Some(globs),
    }
}

/// `paths` split at its commas outside braces, each trimmed.
fn split_globs(paths: &str) -> Vec<String> {
    let (mut globs, mut glob, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in paths.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                globs.push(std::mem::take(&mut glob));
                continue;
            }
            _ => {}
        }
        glob.push(c);
    }
    globs.push(glob);
    (globs.into_iter())
        .map(|glob| glob.trim().to_owned())
        .filter(|glob| !glob.is_empty())
        .collect()
}

/// How many globs one pattern's braces expand to at most, as Claude Code's.
const BRACE_RESULTS: usize = 1000;

/// `glob` with each `{a,b}` expanded, innermost pairs without braces inside first.
fn expand_braces(glob: &str) -> Vec<String> {
    let pattern = regex::Regex::new(r"^([^{]*)\{([^}]+)\}(.*)$").expect("a valid pattern");
    let (mut done, mut pending) = (Vec::new(), vec![glob.to_owned()]);
    while let Some(glob) = pending.pop() {
        let Some(parts) = pattern.captures(&glob) else {
            done.push(glob);
            continue;
        };
        if done.len() + pending.len() >= BRACE_RESULTS {
            done.push(glob);
            continue;
        }
        for choice in parts[2].split(',') {
            pending.push(format!("{}{}{}", &parts[1], choice.trim(), &parts[3]));
        }
    }
    done.reverse();
    done
}

/// Whether `globs`, as gitignore lines in `base`, match `file` under it or one of its
/// directories; one that does not parse is dropped, as Claude Code drops it.
fn matched(base: &Path, globs: &[String], file: &Path) -> bool {
    let mut lines = ignore::gitignore::GitignoreBuilder::new(base);
    for glob in globs {
        let _ = lines.add_line(None, glob);
    }
    (lines.build()).is_ok_and(|lines| lines.matched_path_or_any_parents(file, false).is_ignore())
}

/// The context `files`, as Pi's `# Project Context`, one part each, the first with the heading.
fn project_context(files: &[(PathBuf, String)]) -> Vec<Part> {
    let mut heading =
        "\n# Project Context\n\nProject-specific instructions and guidelines:\n".to_owned();
    (files.iter())
        .map(|(path, contents)| {
            let text = std::mem::take(&mut heading)
                + &format!("\n## {}\n\n{}\n", path.display(), contents.trim_end());
            Part::new(Section::ContextFile, &path.display().to_string(), text)
        })
        .collect()
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
    use super::super::ProblemFile;
    use super::*;

    fn agent(
        root: &Path,
        tools: &[&str],
        commands: &Commands,
        agents: &[subagent::Definition],
        skills: &Skills,
        loaded: &Loaded,
    ) -> String {
        joined(&super::agent(root, tools, commands, agents, skills, loaded))
    }

    #[test]
    fn the_parts_are_the_instructions_in_order_each_marked() {
        let loaded = Loaded {
            context: vec![
                (PathBuf::from("/p/CLAUDE.md"), "one\n".into()),
                (PathBuf::from("/p/AGENTS.md"), "two".into()),
            ],
            ..Loaded::default()
        };
        let commands = Commands::parsed(&[("set:go", "---\ndescription: Go\n---\nGo.")]);
        let parts = super::agent(
            Path::new("/p"),
            &["read"],
            &commands,
            &[],
            &Skills::default(),
            &loaded,
        );
        let sections: Vec<_> = parts
            .iter()
            .map(|part| (part.section, &*part.name))
            .collect();
        assert_eq!(
            sections,
            [
                (Section::Opening, "Opening"),
                (Section::ContextFile, "/p/CLAUDE.md"),
                (Section::ContextFile, "/p/AGENTS.md"),
                (Section::Directives, "Scramjet's directives"),
                (Section::Commands, "Commands"),
            ]
        );
        assert!(parts[1].text.starts_with("\n# Project Context\n"));
        assert_eq!(parts[2].text, "\n## /p/AGENTS.md\n\ntwo\n");
        assert!(
            parts[4].text.ends_with("- /set:go: Go"),
            "{}",
            parts[4].text
        );
    }
    #[test]
    fn the_tools_and_guidelines_listed_are_the_configured_ones() {
        let root = Path::new("/nonexistent/scramjet-rs-instructions");
        let commands = Commands::default();
        let harness = (super::super::tools::HARNESS_TOOLS.into_iter())
            .filter(|tool| !mcp::TOOLS.contains(tool));
        let tools: Vec<&str> = ["read", "bash", "edit", "write"]
            .into_iter()
            .chain(harness)
            .collect();
        let text = agent(
            root,
            &tools,
            &commands,
            &[],
            &Skills::default(),
            &Loaded::default(),
        );
        let listed = "Available tools:\n- read: Read file contents\n- bash: Execute bash commands \
(ls, grep, find, etc.)\n- edit: Make precise file edits with exact text replacement, including \
multiple disjoint edits in one call\n- write: Create or overwrite files\n- report_scramjet_command_status:";
        assert!(text.contains(listed), "{text}");
        let guidelines = "Guidelines:\n- Use bash for file operations like ls, rg, find, and for \
git, builds and tests\n- Use read to examine files instead of cat or sed.\n- Use edit for precise";
        assert!(text.contains(guidelines), "{text}");
        assert!(
            text.contains("- Use write only for new files or complete rewrites.\n- Prefer editing")
        );

        let text = agent(
            root,
            &super::super::tools::HARNESS_TOOLS,
            &commands,
            &[],
            &Skills::default(),
            &Loaded::default(),
        );
        assert!(text.contains("Available tools:\n- report_scramjet_command_status:"));
        let guidelines = format!("Guidelines:\n{}\n- Be concise", suggest::GUIDELINE);
        assert!(text.contains(&guidelines), "{text}");
        let listed = format!(
            "- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}\n\nGuidelines:",
            switch::NAME,
            switch::SNIPPET,
            suggest::NAME,
            suggest::SNIPPET,
            mcp::SEARCH,
            mcp::SEARCH_SNIPPET,
            mcp::CALL,
            mcp::CALL_SNIPPET
        );
        assert!(text.contains(&listed), "{text}");
        let all = ["read", "bash", "edit", "write", "grep", "find", "ls"];
        let text = agent(
            root,
            &all,
            &commands,
            &[],
            &Skills::default(),
            &Loaded::default(),
        );
        assert!(text.contains(
            "Guidelines:\n- Prefer grep/find/ls tools over bash for file exploration (faster, \
respects .gitignore)\n- Use read"
        ));
        let text = |tools: &[&str]| {
            let (skills, loaded) = (Skills::default(), Loaded::default());
            agent(root, tools, &commands, &[], &skills, &loaded)
        };
        let windows = text(&["read", "powershell", "cmd", "grep"]);
        assert!(
            !cfg!(windows)
                || windows.contains("- powershell: Execute PowerShell commands\n- cmd: ")
        );
        assert!(windows.contains("- Prefer grep/find/ls tools over powershell or cmd for file"));
        let alone = text(&["powershell"]);
        assert!(alone.contains("- Use powershell for file operations like ls, rg, find, and for"));
    }

    #[test]
    fn the_model_identity_names_models_as_their_cards_do_and_by_id_without_one() {
        let known = std::collections::HashMap::from([(
            "anthropic/c".to_owned(),
            Identity {
                id: "c".into(),
                name: Some("Claude C".into()),
                provider: Some("anthropic".into()),
            },
        )]);
        let text = identity("x", &["anthropic/c".to_owned()], &known);
        let expected = "\n\n# Model Identity\nYour model for the imminent response is: x (ID: x).\n\
Models with material responses in this session branch, the imminent one included:\n- Claude C (ID: \
c, provider: anthropic)\n- x (ID: x)\nWhen posting to GitHub, use honest contribution-aware \
attribution:\n- Credit the current model for work in the imminent response.\n- Credit prior \
models only when their material responses contributed to the published work.\n- Single credited \
model: \"Reviewed by x\"\n- Multiple credited models: describe each model's contribution (e.g., \
\"Reviewed by X (analysis) and Y (posting)\")";
        assert_eq!(text, expected);
        let current = identity("anthropic/c", &[], &known);
        assert!(current.contains("is: Claude C (ID: c, provider: anthropic)."));
    }

    #[test]
    fn the_directives_follow_the_context_and_skills_and_the_catalogs_follow_them() {
        let root = Path::new("/nonexistent/scramjet-rs-directives");
        let commands = Commands::parsed(&[("set:pr-pre-merge", "Merge.")]);
        let text = agent(
            root,
            &[],
            &commands,
            &[],
            &Skills::default(),
            &Loaded::default(),
        );
        let text = &text[..text.find("\n\n# Available commands").unwrap()];
        let at = |heading: &str| text.find(&format!("\n# {heading}\n")).unwrap();
        assert!(at("Scramjet") < at("Prior-session fallback"));
        assert!(at("Command framing") < at("Executing actions with care"));
        assert!(text.ends_with(
            "\n# Release preparation timing\n - Version bumps, changelog \
entries, and release-preparation tasks belong at the end of a PR's lifecycle — after feature \
work is complete, after review, and after the default branch has been merged in. Never include \
them in implementation plans, never flag their absence in reviews, and never perform them during \
feature implementation. A dedicated pre-merge step handles them at the correct time; doing them \
earlier causes version conflicts between parallel PRs."
        ));
        let readme = match crate::agent::builtin::readme() {
            Some(readme) => format!("{}; ", readme.display()),
            None => String::new(),
        };
        assert!(text.contains(&format!(
            "about Scramjet itself — its commands, command sets, \
next steps, delegation, settings, keys or the /autopilot on/off flag): {readme}`{} --help-all` \
(every option, key and built-in command)\n",
            std::env::current_exe().unwrap().display()
        )));
    }

    #[test]
    fn each_directive_is_given_only_where_it_applies_and_none_without_directives() {
        let root = Path::new("/nonexistent/scramjet-rs-directives");
        let mapper = subagent::Definition {
            name: "mach12:structural-mapper".into(),
            description: "Maps.".into(),
            tools: None,
            disallowed: Vec::new(),
            model: None,
            worktree: false,
            prompt: String::new(),
            source: "installed",
            path: PathBuf::from("/sets/mach12/agents/structural-mapper.md"),
            digest: String::new(),
            diagnostics: Vec::new(),
        };
        let given = |tools: &[&str], commands: &Commands, agents: &[subagent::Definition]| {
            agent(
                root,
                tools,
                commands,
                agents,
                &Skills::default(),
                &Loaded::default(),
            )
        };
        let (framing, release) = ("\n# Command framing\n", "\n# Release preparation timing\n");
        let planning = "dispatch `mach12:structural-mapper`";
        let asking = "When you ask through a tool, say it in the question itself.";
        let bare = given(&[], &Commands::default(), &[]);
        for absent in [framing, release, planning, asking] {
            assert!(!bare.contains(absent), "{absent}");
        }
        assert!(bare.contains("\n# Tool results and external content\n"));
        assert!(bare.contains("IMPORTANT: You must NEVER generate or guess URLs"));
        assert!(bare.contains(" - Prefer editing existing files to creating new ones.\n"));
        assert!(bare.contains("not a gate to clear."), "{bare}");
        let commands = Commands::parsed(&[("set:go", "Go.")]);
        let text = given(
            &[question::NAME, subagent::NAME],
            &commands,
            std::slice::from_ref(&mapper),
        );
        for present in [framing, planning, asking] {
            assert!(text.contains(present), "{present}");
        }
        assert!(!text.contains(release));
        // The mapper offered without the subagent tool is not offered.
        assert!(!given(&[], &commands, &[mapper]).contains(planning));

        let loaded = Loaded {
            no_directives: true,
            ..Loaded::default()
        };
        let parts = super::agent(root, &["read"], &commands, &[], &Skills::default(), &loaded);
        let sections: Vec<_> = parts.iter().map(|part| part.section).collect();
        assert_eq!(sections, [Section::Opening, Section::Commands]);
        assert!(!joined(&parts).contains("# Scramjet\n"));
    }

    #[test]
    fn system_md_and_append_system_md_are_found_and_the_options_replace_them() {
        let scratch = std::env::temp_dir().join(format!("sjrs-system-md-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let (root, home) = (scratch.join("project"), scratch.join("agent"));
        let write = |path: &Path, text: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let prompt = SystemPrompt::default();
        let loaded = load(&prompt, &root, Some(&home), true, &Hidden::default());
        assert_eq!(
            (loaded.opening, loaded.appended),
            (None, Vec::<String>::new())
        );
        write(&home.join("SYSTEM.md"), "Global.");
        write(&home.join("APPEND_SYSTEM.md"), "Global after.");
        let loaded = load(&prompt, &root, Some(&home), true, &Hidden::default());
        assert_eq!(loaded.opening.as_deref(), Some("Global."));
        assert_eq!(loaded.appended, ["Global after."]);
        write(&root.join(".scramjet/SYSTEM.md"), "Project.");
        let loaded = load(&prompt, &root, Some(&home), true, &Hidden::default());
        assert_eq!(loaded.opening.as_deref(), Some("Project."));
        assert_eq!(
            loaded.files,
            [
                (root.join(".scramjet/SYSTEM.md"), true),
                (home.join("APPEND_SYSTEM.md"), false)
            ]
        );
        // The options' texts and files win; an empty one leaves the default opening.
        let given = scratch.join("given.md");
        write(&given, "From a file.");
        let prompt = SystemPrompt {
            system: Some("Be terse.".into()),
            append: vec![given.display().to_string(), "Inline.".into()],
            no_context_files: false,
            ..SystemPrompt::default()
        };
        let loaded = load(&prompt, &root, Some(&home), true, &Hidden::default());
        assert_eq!(loaded.opening.as_deref(), Some("Be terse."));
        assert_eq!(loaded.appended, ["From a file.", "Inline."]);
        assert_eq!(loaded.files, [(given, false)]);
        let prompt = SystemPrompt {
            system: Some(String::new()),
            ..SystemPrompt::default()
        };
        assert_eq!(
            load(&prompt, &root, Some(&home), true, &Hidden::default()).opening,
            None
        );
        // A file that cannot be read is a problem, and left out.
        std::fs::create_dir(root.join(".scramjet/APPEND_SYSTEM.md")).unwrap();
        let loaded = load(
            &SystemPrompt::default(),
            &root,
            Some(&home),
            true,
            &Hidden::default(),
        );
        assert!(loaded.appended.is_empty());
        let unread = crate::os::join(&root, ".scramjet/APPEND_SYSTEM.md");
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(
            loaded.problems[0].file,
            Some(ProblemFile::Path(unread.clone()))
        );
        // Linux opens a directory and fails the read; Windows refuses to open it.
        assert!(
            (loaded.problems[0].text).contains(&format!("{}: ", unread.display())),
            "{:?}",
            loaded.problems
        );
    }

    #[test]
    fn the_opening_given_replaces_the_tools_and_guidelines_and_the_appended_texts_follow_it() {
        let scratch = std::env::temp_dir().join(format!("sjrs-opening-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        std::fs::write(scratch.join("AGENTS.md"), "Be careful.\n").unwrap();
        let prompt = SystemPrompt {
            system: Some("You review code.".into()),
            append: vec!["First.".into(), "Second.\n".into()],
            no_context_files: false,
            ..SystemPrompt::default()
        };
        let loaded = load(&prompt, &scratch, None, true, &Hidden::default());
        let tools = super::super::tools::offered(
            &super::super::tools::DEFAULT_TOOLS.map(String::from),
            &[],
            None,
            false,
            false,
        );
        let text = agent(
            &scratch,
            &tools,
            &Commands::default(),
            &[],
            &Skills::default(),
            &loaded,
        );
        let context = format!(
            "You review code.\n\nFirst.\n\nSecond.\n\n# Project Context\n\nProject-specific \
instructions and guidelines:\n\n## {}\n\nBe careful.\n\n# Scramjet\n",
            scratch.join("AGENTS.md").display()
        );
        assert!(text.starts_with(&context), "{text}");
        assert!(!text.contains("Available tools:"));
        let prompt = SystemPrompt {
            append: vec!["After.".into()],
            no_context_files: true,
            ..SystemPrompt::default()
        };
        let loaded = load(&prompt, &scratch, None, true, &Hidden::default());
        assert!(loaded.context.is_empty());
        let text = agent(
            &scratch,
            &tools,
            &Commands::default(),
            &[],
            &Skills::default(),
            &loaded,
        );
        assert!(text.contains(
            "- Show file paths clearly when working with files\n\nAfter.\n\n# Scramjet\n"
        ));
        assert!(!text.contains("# Project Context"));
    }

    #[test]
    fn context_files_are_the_agent_directory_s_then_outermost_first_claude_md_before_agents_md_and_rules_last()
     {
        let scratch = std::env::temp_dir().join(format!("sjrs-context-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let (outer, root, home) = (
            scratch.join("a"),
            scratch.join("a/b"),
            scratch.join("agent"),
        );
        for directory in [&root, &home] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let write = |path: PathBuf, text: &str| std::fs::write(path, text).unwrap();
        write(home.join("AGENTS.md"), "Home.");
        write(outer.join("AGENTS.md"), "Outer agents.");
        write(outer.join("CLAUDE.MD"), "Outer claude.");
        write(root.join("CLAUDE.md"), "Same.");
        write(root.join("AGENTS.md"), "Same.");
        write(outer.join("CLAUDE.local.md"), "Outer local.");
        write(root.join("CLAUDE.local.md"), "Same.");
        let rules = root.join(".claude/rules");
        std::fs::create_dir_all(rules.join("sub")).unwrap();
        write(root.join(".claude/CLAUDE.md"), "Dot claude.");
        write(rules.join("b.md"), "Rule b.");
        write(
            rules.join("sub/a.md"),
            "---\ndescription: x\n---\n\nRule a.\n",
        );
        write(
            rules.join("scoped.md"),
            "---\npaths:\n  - \"**/*.rs\"\n---\nScoped.",
        );
        write(
            rules.join("listed.md"),
            "---\npaths: \"src/**\"\n---\nScoped.",
        );
        write(rules.join("broken.md"), "---\npaths: [\n---\nBroken.");
        write(rules.join("notes.txt"), "Not a rule.");
        let found: Vec<(PathBuf, String)> = context_files(
            &root,
            Some(&home),
            true,
            &Hidden::default(),
            &mut Vec::new(),
        )
        .into_iter()
        .filter(|(path, _)| path.starts_with(&scratch))
        .collect();
        // A file system that ignores case (Windows', macOS's by default) finds it as CLAUDE.md.
        let claude = match outer.join("CLAUDE.md").is_file() {
            true => "CLAUDE.md",
            false => "CLAUDE.MD",
        };
        let expected = [
            (home.join("AGENTS.md"), "Home."),
            (outer.join(claude), "Outer claude."),
            (outer.join("AGENTS.md"), "Outer agents."),
            (outer.join("CLAUDE.local.md"), "Outer local."),
            (root.join("CLAUDE.md"), "Same."),
            (root.join(".claude/CLAUDE.md"), "Dot claude."),
            (root.join("CLAUDE.local.md"), "Same."),
            (rules.join("b.md"), "Rule b."),
            (rules.join("broken.md"), "Broken."),
            (rules.join("sub/a.md"), "Rule a."),
        ]
        .map(|(path, text)| (path, text.to_owned()));
        assert_eq!(found, expected);
        // Without Claude Code's instructions, none of its own files, and a CLAUDE.md as it is.
        write(root.join("CLAUDE.md"), "Root. <!-- x -->\n\n@notes.md\n");
        write(root.join("notes.md"), "Imported.");
        let found: Vec<(PathBuf, String)> = context_files(
            &root,
            Some(&home),
            false,
            &Hidden::default(),
            &mut Vec::new(),
        )
        .into_iter()
        .filter(|(path, _)| path.starts_with(&scratch))
        .collect();
        let expected = [
            (home.join("AGENTS.md"), "Home."),
            (outer.join(claude), "Outer claude."),
            (outer.join("AGENTS.md"), "Outer agents."),
            (outer.join("CLAUDE.local.md"), "Outer local."),
            (root.join("CLAUDE.md"), "Root. <!-- x -->\n\n@notes.md\n"),
            (root.join("AGENTS.md"), "Same."),
            (root.join("CLAUDE.local.md"), "Same."),
        ]
        .map(|(path, text)| (path, text.to_owned()));
        assert_eq!(found, expected);
    }

    #[test]
    fn memory_files_lose_their_block_comments_and_import_paths_outside_code() {
        let text = "<!-- For maintainers. -->\n\n# Rules\n\nSee @README and @docs/a\\ b.md#part, \
                    `@not` nor \"@quoted\" nor me@mail.\n\n```\n<!-- kept -->\n@fenced\n```\n\n\
                    <!-- a\nb --> after @tail\n\n- item @./item.md @~/home.md @/abs.md @/ @@x\n\n\
                    Inline <!-- kept --> too. <!-- open";
        let (kept, imports) = claude_markdown(text);
        assert_eq!(
            kept,
            "# Rules\n\nSee @README and @docs/a\\ b.md#part, `@not` nor \"@quoted\" nor me@mail.\n\n\
             ```\n<!-- kept -->\n@fenced\n```\n\n after @tail\n\n- item @./item.md @~/home.md \
             @/abs.md @/ @@x\n\nInline <!-- kept --> too. <!-- open"
        );
        let expected = [
            "README",
            "docs/a b.md",
            "tail",
            "./item.md",
            "~/home.md",
            "/abs.md",
        ];
        assert_eq!(imports, expected);
        assert_eq!(claude_markdown("<!-- only -->\n").0, "");
    }

    #[test]
    fn imports_follow_the_file_importing_them_four_hops_deep_each_once() {
        let scratch = std::env::temp_dir().join(format!("sjrs-imports-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let root = scratch.join("project");
        std::fs::create_dir_all(root.join("docs/deep")).unwrap();
        let write = |path: &str, text: &str| std::fs::write(root.join(path), text).unwrap();
        write(
            "CLAUDE.md",
            "@AGENTS.md\n@docs/one.md @missing.md @logo.png @docs/one.md",
        );
        write("AGENTS.md", "Agents.");
        write("logo.png", "Not text.");
        write("docs/one.md", "One @deep/two.md");
        write("docs/deep/two.md", "Two @../../CLAUDE.md @three");
        write("docs/deep/three", "Three @four.md");
        write("docs/deep/four.md", "Four @five.md");
        write("docs/deep/five.md", "Five, too deep.");
        write("CLAUDE.local.md", "<!-- Only a note. -->\n@docs/one.md");
        let found: Vec<(PathBuf, String)> =
            context_files(&root, None, true, &Hidden::default(), &mut Vec::new())
                .into_iter()
                .filter(|(path, _)| path.starts_with(&scratch))
                .collect();
        let expected = [
            (
                "CLAUDE.md",
                "@AGENTS.md\n@docs/one.md @missing.md @logo.png @docs/one.md",
            ),
            ("AGENTS.md", "Agents."),
            ("docs/one.md", "One @deep/two.md"),
            ("docs/deep/two.md", "Two @../../CLAUDE.md @three"),
            ("docs/deep/three", "Three @four.md"),
            ("docs/deep/four.md", "Four @five.md"),
            ("CLAUDE.local.md", "@docs/one.md"),
        ]
        .map(|(path, text)| (root.join(path), text.to_owned()));
        assert_eq!(found, expected);
    }

    #[cfg(unix)]
    #[test]
    fn files_that_lead_into_a_credential_store_are_left_out_and_said() {
        let scratch = std::env::temp_dir().join(format!("sjrs-hidden-md-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let (root, home) = (scratch.join("project"), scratch.join("home"));
        std::fs::create_dir_all(root.join(".scramjet")).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        let key = home.join(".ssh/id_x");
        std::fs::write(&key, "SECRET-key").unwrap();
        std::fs::write(root.join("AGENTS.md"), "Kept.").unwrap();
        let link = |at: &str| std::os::unix::fs::symlink(&key, root.join(at)).unwrap();
        link("CLAUDE.md");
        link(".scramjet/SYSTEM.md");
        let hidden = Hidden::under(&home, Vec::new());
        let loaded = load(&SystemPrompt::default(), &root, None, true, &hidden);
        let context: Vec<&str> = (loaded.context.iter())
            .filter(|(path, _)| path.starts_with(&scratch))
            .map(|(_, text)| text.as_str())
            .collect();
        assert_eq!(context, ["Kept."]);
        assert_eq!(loaded.opening, None);
        let said: Vec<String> = (loaded.problems.iter()).map(|p| p.text.clone()).collect();
        let hidden_at = |at: &str| {
            let path = root.join(at);
            format!("{}: hidden, as it holds credentials", path.display())
        };
        assert_eq!(
            said,
            [hidden_at("CLAUDE.md"), hidden_at(".scramjet/SYSTEM.md")]
        );
        // A subagent's instructions leave it out too.
        let context = context_files(&root, None, true, &hidden, &mut Vec::new());
        let text = child(
            &root,
            &["read"],
            "",
            &Skills::in_directory(&scratch),
            &context,
            false,
            None,
        );
        assert!(text.contains("Kept.") && !text.contains("SECRET"), "{text}");
    }

    #[test]
    fn children_that_can_read_are_told_of_the_skills() {
        let scratch =
            std::env::temp_dir().join(format!("sjrs-child-skills-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let file = crate::os::join(&scratch, "pdf/SKILL.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "---\ndescription: PDFs.\n---\nBody").unwrap();
        let skills = Skills::in_directory(&scratch);
        let cwd = Path::new("/nonexistent/scramjet-rs-child");
        let text = child(
            cwd,
            &["read", "bash"],
            "Be brief.",
            &skills,
            &[],
            false,
            None,
        );
        let listed = "Be brief.\n\nThe following skills provide specialized instructions for \
specific tasks.\n";
        assert!(text.contains(listed), "{text}");
        let end = format!(
            "<location>{}</location>\n  </skill>\n</available_skills>\n\nCurrent date: ",
            file.display()
        );
        assert!(text.contains(&end), "{text}");
        assert!(
            !child(cwd, &["bash"], "", &skills, &[], false, None).contains("<available_skills>")
        );
    }

    #[test]
    fn a_rule_s_paths_are_read_as_claude_code_reads_them() {
        let read = |frontmatter: &str| globs(frontmatter);
        assert_eq!(read("description: x"), None);
        assert_eq!(read("paths: [\n"), None);
        assert_eq!(read("paths: \"**\""), None);
        assert_eq!(read("paths:\n  - \"**/**\"\n  - \"**\""), None);
        assert_eq!(
            read("paths: \"src/**, *.{ts,tsx}\""),
            Some(vec!["src".into(), "*.ts".into(), "*.tsx".into()])
        );
        assert_eq!(
            read("paths:\n  - \"lib/{a,b}/*.rs\"\n  - docs/**"),
            Some(vec![
                "lib/a/*.rs".into(),
                "lib/b/*.rs".into(),
                "docs".into()
            ])
        );
    }

    #[test]
    fn a_file_worked_on_brings_in_the_rules_it_matches_and_the_claude_md_files_down_to_it() {
        let scratch = std::env::temp_dir().join(format!("sjrs-guidance-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let (outer, root) = (scratch.join("a"), scratch.join("a/b"));
        let (sub, deeper) = (root.join("sub"), root.join("sub/deeper"));
        for directory in [
            outer.join(".claude/rules"),
            root.join(".claude/rules"),
            sub.join(".claude/rules"),
            deeper.clone(),
        ] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let write = |path: PathBuf, text: &str| std::fs::write(path, text).unwrap();
        let scoped = |glob: &str, text: &str| format!("---\npaths: \"{glob}\"\n---\n{text}\n");
        write(root.join("CLAUDE.md"), "Root, in the instructions already.");
        write(
            root.join(".claude/rules/always.md"),
            "In the instructions already.",
        );
        write(
            root.join(".claude/rules/rust.md"),
            &scoped("**/*.rs", "Rust."),
        );
        write(
            root.join(".claude/rules/docs.md"),
            &scoped("docs/**", "Docs."),
        );
        write(
            outer.join(".claude/rules/outer.md"),
            &scoped("b/sub/**", "Outer."),
        );
        write(
            outer.join(".claude/rules/other.md"),
            &scoped("c/**", "Other."),
        );
        write(sub.join("CLAUDE.md"), "Sub <!-- unseen -->.\n");
        write(sub.join("CLAUDE.local.md"), "Sub local.");
        write(sub.join(".claude/rules/sub.md"), "Sub rule.");
        write(
            sub.join(".claude/rules/here.md"),
            &scoped("deeper/*.rs", "Here."),
        );
        write(sub.join(".claude/rules/not.md"), &scoped("/*.rs", "Not."));
        write(deeper.join("AGENTS.md"), "Not Claude Code's.");
        let found = |file: &Path| -> Vec<(PathBuf, String)> {
            (guidance(&root, file, &Hidden::default(), &mut Vec::new()).into_iter())
                .filter(|(path, _)| path.starts_with(&scratch))
                .map(|(path, text)| (path, text.trim().to_owned()))
                .collect()
        };
        let expected = [
            (sub.join("CLAUDE.md"), "Sub <!-- unseen -->."),
            (sub.join("CLAUDE.local.md"), "Sub local."),
            (sub.join(".claude/rules/sub.md"), "Sub rule."),
            (sub.join(".claude/rules/here.md"), "Here."),
            (outer.join(".claude/rules/outer.md"), "Outer."),
            (root.join(".claude/rules/rust.md"), "Rust."),
        ];
        let expected: Vec<(PathBuf, String)> = (expected.into_iter())
            .map(|(path, text)| (path, text.to_owned()))
            .collect();
        assert_eq!(found(&deeper.join("x.rs")), expected);
        assert_eq!(
            found(&root.join("docs/guide/a.md")),
            [(root.join(".claude/rules/docs.md"), "Docs.".to_owned())]
        );
        assert!(found(&root.join("README.md")).is_empty());
        // Outside the project, an outer directory's rules still match by their own paths.
        assert_eq!(
            found(&outer.join("c/x.rs")),
            [(outer.join(".claude/rules/other.md"), "Other.".to_owned())]
        );
    }
}
