//! The system instructions of the agent and of its subagents.
use super::{
    Commands, Problem, Skills, SystemPrompt, publish, question, status, subagent, suggest, switch,
};
use std::path::{Path, PathBuf};

const CONTEXT_FILE_BYTES: u64 = 256 << 10;
/// Scramjet's base directives, from its command framing to its release timing.
const DIRECTIVES: &str = include_str!("directives.md");
/// This build's README, which the instructions point to when it is there.
pub(super) const README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");

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
) -> String {
    let has = |tool: &str| tools.contains(&tool);
    let agents = if has(subagent::NAME) { agents } else { &[] };
    let catalog: String = [subagent::catalog(agents), commands.catalog()]
        .into_iter()
        .filter(|catalog| !catalog.is_empty())
        .map(|catalog| catalog + "\n\n")
        .collect();
    let mut listed: Vec<String> = super::tools::CODING_TOOLS
        .into_iter()
        .filter(|tool| has(tool))
        .map(|tool| match tool {
            "read" => "- read: Read file contents",
            "bash" => "- bash: Execute bash commands (ls, grep, find, etc.)",
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
        ]);
    listed.extend(
        (snippets.filter(|(tool, _)| has(tool))).map(|(tool, text)| format!("- {tool}: {text}")),
    );
    let mut guidelines = Vec::new();
    if has("bash") {
        guidelines.push(match ["grep", "find", "ls"].iter().any(|tool| has(tool)) {
            true => {
                "- Prefer grep/find/ls tools over bash for file exploration (faster, respects \
.gitignore)"
            }
            false => {
                "- Use bash for file operations like ls, rg, find, and for git, builds and tests"
            }
        });
    }
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
            "You are scramjet-rs, a coding agent. You help the user with software engineering \
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
    let text = text
        + &project_context(&loaded.context)
        + &skills_section(has("read"), skills)
        + &format!("\n{}\n{DIRECTIVES}\n{catalog}", orientation());
    text.trim_end().to_owned()
}

/// Scramjet's `# Scramjet` section, for scramjet-rs.
fn orientation() -> String {
    let mut text = "# scramjet-rs

scramjet-rs is the harness you are running under: a coding agent that loads command sets —
directories of user-defined slash commands and their agents — and wires them into emergent
workflows through declared next-step policies (forced / closed / open / ask) and command
delegation, as Scramjet does. Mach 12 is one such command set (the issue → plan → review →
implement → PR → ship methodology); other sets can sit alongside it.
"
    .to_owned();
    let readme = Path::new(README).is_file().then(|| README.to_owned());
    let help = std::env::current_exe().ok().map(|exe| {
        format!(
            "`{} --help` (every option, key and built-in command)",
            exe.display()
        )
    });
    let docs: Vec<String> = [readme, help].into_iter().flatten().collect();
    if !docs.is_empty() {
        text += &format!(
            "\nscramjet-rs documentation (read only when the user asks about scramjet-rs itself — \
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
    let name = |model: &str| -> String {
        let Some(identity) = known.get(model) else {
            return format!("{model} (ID: {model})");
        };
        let id = &identity.id;
        let name = identity.name.as_deref().unwrap_or(id);
        match &identity.provider {
            Some(provider) => format!("{name} (ID: {id}, provider: {provider})"),
            None => format!("{name} (ID: {id})"),
        }
    };
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
/// text, the context files from `cwd` (and the `agent_directory`) and the `skills`, then the date
/// and directory.
pub(super) fn child(
    cwd: &Path,
    tools: &[&str],
    prompt: &str,
    skills: &Skills,
    agent_directory: Option<&Path>,
) -> String {
    let listed: Vec<&str> = tools
        .iter()
        .map(|tool| match *tool {
            "read" => "- read: Read file contents",
            "grep" => "- grep: Search file contents for patterns (respects .gitignore)",
            "find" => "- find: Find files by glob pattern (respects .gitignore)",
            "ls" => "- ls: List directory contents",
            _ => "- bash: Execute bash commands (ls, grep, find, etc.) in a read-only sandbox",
        })
        .collect();
    let mut guidelines = Vec::new();
    let explores = ["grep", "find", "ls"]
        .iter()
        .any(|tool| tools.contains(tool));
    if tools.contains(&"bash") {
        guidelines.push(match explores {
            true => {
                "- Prefer grep/find/ls tools over bash for file exploration (faster, respects \
.gitignore)"
            }
            false => "- Use bash for file operations like ls, rg, find",
        });
        guidelines.push(
            "- bash runs in a read-only sandbox without network, whose only writable directory \
is a private /tmp discarded after each command",
        );
    }
    guidelines.push("- No file can be changed: you only read and report");
    guidelines.push("- Be concise in your responses");
    guidelines.push("- Show file paths clearly when working with files");
    let mut text = format!(
        "You are a subagent of scramjet-rs, a coding agent: you are given a task, and your final \
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
    text += &project_context(&context_files(cwd, agent_directory));
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
    /// The files `opening` and `appended` were read from, each with whether it is the opening.
    pub files: Vec<(PathBuf, bool)>,
    /// The files among them that could not be read.
    pub problems: Vec<Problem>,
}

impl Loaded {
    /// The text `source` gives: the contents of the file it names, else itself.
    fn read(&mut self, source: &str, opening: bool) -> Option<String> {
        let path = Path::new(source);
        if !path.exists() {
            return Some(source.to_owned());
        }
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
                self.files.push((path, opening));
                Some(text)
            }
            Err(error) => {
                let text = format!("could not read {}: {error}", path.display());
                self.problems.push(Problem::in_file(text, path));
                None
            }
        }
    }
}

/// Reads what `prompt` names for the instructions of an agent in `root`, as Scramjet does: its
/// opening's text or file, else the project's `.scramjet/SYSTEM.md`, else the agent directory's;
/// the texts or files it appends, else an `APPEND_SYSTEM.md` found the same way; and, unless it
/// leaves them out, the context files.
pub(super) fn load(prompt: &SystemPrompt, root: &Path, agent_directory: Option<&Path>) -> Loaded {
    let mut loaded = Loaded::default();
    if !prompt.no_context_files {
        loaded.context = context_files(root, agent_directory);
    }
    let found = |name: &str| {
        let directories = [
            Some(root.join(".scramjet")),
            agent_directory.map(Path::to_path_buf),
        ];
        (directories.into_iter().flatten())
            .map(|directory| directory.join(name))
            .find(|path| path.exists())
            .map(|path| path.to_string_lossy().into_owned())
    };
    let opening = (prompt.system.clone()).or_else(|| found("SYSTEM.md"));
    if let Some(source) = opening {
        loaded.opening = (loaded.read(&source, true)).filter(|text| !text.trim().is_empty());
    }
    let appended = match prompt.append.is_empty() {
        true => found("APPEND_SYSTEM.md").into_iter().collect(),
        false => prompt.append.clone(),
    };
    for source in appended {
        if let Some(text) = loaded.read(&source, false) {
            let text = text.trim_end();
            if !text.is_empty() {
                loaded.appended.push(text.to_owned());
            }
        }
    }
    loaded
}

/// The context files, as Scramjet's: those of the `agent_directory`, then those of `root` and
/// the directories above it, outermost first, with their contents. A directory's `CLAUDE.md` comes
/// before its `AGENTS.md`, which is left out when it says the same.
pub(super) fn context_files(root: &Path, agent_directory: Option<&Path>) -> Vec<(PathBuf, String)> {
    let mut files = agent_directory.map(directory_context).unwrap_or_default();
    let mut above = Vec::new();
    for directory in root.ancestors() {
        let found = directory_context(directory).into_iter();
        let found: Vec<_> = found
            .filter(|(path, _)| !files.iter().any(|(seen, _)| seen == path))
            .collect();
        above.splice(0..0, found);
    }
    files.extend(above);
    files
}

/// The `CLAUDE.md` and `AGENTS.md` (or `.MD`) of `directory`, as Scramjet reads them.
fn directory_context(directory: &Path) -> Vec<(PathBuf, String)> {
    let first = |names: [&str; 2]| {
        names.into_iter().find_map(|name| {
            let path = directory.join(name);
            let fits = std::fs::metadata(&path)
                .is_ok_and(|m| m.is_file() && m.len() <= CONTEXT_FILE_BYTES);
            let contents = fits
                .then(|| std::fs::read_to_string(&path).ok())
                .flatten()?;
            Some((path, contents))
        })
    };
    let claude = first(["CLAUDE.md", "CLAUDE.MD"]);
    let agents = first(["AGENTS.md", "AGENTS.MD"]);
    match (claude, agents) {
        (Some(claude), Some(agents)) if claude.1 != agents.1 => vec![claude, agents],
        (Some(claude), _) => vec![claude],
        (None, agents) => agents.into_iter().collect(),
    }
}

/// The context `files`, as Pi's `# Project Context`; empty without any.
fn project_context(files: &[(PathBuf, String)]) -> String {
    let mut text = String::new();
    if !files.is_empty() {
        text.push_str("\n# Project Context\n\nProject-specific instructions and guidelines:\n");
        for (path, contents) in files {
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
    use super::super::ProblemFile;
    use super::*;

    #[test]
    fn the_tools_and_guidelines_listed_are_the_configured_ones() {
        let root = Path::new("/nonexistent/scramjet-rs-instructions");
        let commands = Commands::default();
        let tools = super::super::tools::offered(
            &super::super::tools::DEFAULT_TOOLS.map(String::from),
            None,
        );
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
            "- {}: {}\n- {}: {}\n\nGuidelines:",
            switch::NAME,
            switch::SNIPPET,
            suggest::NAME,
            suggest::SNIPPET
        );
        assert!(text.contains(&listed), "{text}");
        let all = super::super::tools::offered(
            &super::super::tools::CODING_TOOLS.map(String::from),
            None,
        );
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
        let text = agent(
            root,
            &[],
            &Commands::default(),
            &[],
            &Skills::default(),
            &Loaded::default(),
        );
        let at = |heading: &str| text.find(&format!("\n# {heading}\n")).unwrap();
        assert!(at("scramjet-rs") < at("Prior-session fallback"));
        assert!(at("Command framing") < at("Executing actions with care"));
        assert!(text.ends_with(
            "\n# Release preparation timing\n - Version bumps, changelog \
entries, and release-preparation tasks belong at the end of a PR's lifecycle — after feature \
work is complete, after review, and after the default branch has been merged in. Never include \
them in implementation plans, never flag their absence in reviews, and never perform them during \
feature implementation. A dedicated pre-merge step handles them at the correct time; doing them \
earlier causes version conflicts between parallel PRs."
        ));
        assert!(text.contains(&format!(
            "about scramjet-rs itself — its commands, command sets, \
next steps, delegation, settings, keys or the /autopilot on/off flag): {README}; `{} --help` \
(every option, key and built-in command)\n",
            std::env::current_exe().unwrap().display()
        )));
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
        let loaded = load(&prompt, &root, Some(&home));
        assert_eq!(
            (loaded.opening, loaded.appended),
            (None, Vec::<String>::new())
        );
        write(&home.join("SYSTEM.md"), "Global.");
        write(&home.join("APPEND_SYSTEM.md"), "Global after.");
        let loaded = load(&prompt, &root, Some(&home));
        assert_eq!(loaded.opening.as_deref(), Some("Global."));
        assert_eq!(loaded.appended, ["Global after."]);
        write(&root.join(".scramjet/SYSTEM.md"), "Project.");
        let loaded = load(&prompt, &root, Some(&home));
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
        };
        let loaded = load(&prompt, &root, Some(&home));
        assert_eq!(loaded.opening.as_deref(), Some("Be terse."));
        assert_eq!(loaded.appended, ["From a file.", "Inline."]);
        assert_eq!(loaded.files, [(given, false)]);
        let prompt = SystemPrompt {
            system: Some(String::new()),
            ..SystemPrompt::default()
        };
        assert_eq!(load(&prompt, &root, Some(&home)).opening, None);
        // A file that cannot be read is a problem, and left out.
        std::fs::create_dir(root.join(".scramjet/APPEND_SYSTEM.md")).unwrap();
        let loaded = load(&SystemPrompt::default(), &root, Some(&home));
        assert!(loaded.appended.is_empty());
        let unread = root.join(".scramjet/APPEND_SYSTEM.md");
        assert_eq!(loaded.problems.len(), 1);
        assert_eq!(
            loaded.problems[0].file,
            Some(ProblemFile::Path(unread.clone()))
        );
        assert!(
            (loaded.problems[0].text)
                .starts_with(&format!("could not read {}: ", unread.display())),
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
        };
        let loaded = load(&prompt, &scratch, None);
        let tools = super::super::tools::offered(
            &super::super::tools::DEFAULT_TOOLS.map(String::from),
            None,
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
instructions and guidelines:\n\n## {}\n\nBe careful.\n\n# scramjet-rs\n",
            scratch.join("AGENTS.md").display()
        );
        assert!(text.starts_with(&context), "{text}");
        assert!(!text.contains("Available tools:"));
        let prompt = SystemPrompt {
            append: vec!["After.".into()],
            no_context_files: true,
            ..SystemPrompt::default()
        };
        let loaded = load(&prompt, &scratch, None);
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
            "- Show file paths clearly when working with files\n\nAfter.\n\n# scramjet-rs\n"
        ));
        assert!(!text.contains("# Project Context"));
    }

    #[test]
    fn context_files_are_the_agent_directory_s_then_outermost_first_claude_md_before_agents_md() {
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
        let found: Vec<(PathBuf, String)> = context_files(&root, Some(&home))
            .into_iter()
            .filter(|(path, _)| path.starts_with(&scratch))
            .collect();
        let expected = [
            (home.join("AGENTS.md"), "Home."),
            (outer.join("CLAUDE.MD"), "Outer claude."),
            (outer.join("AGENTS.md"), "Outer agents."),
            (root.join("CLAUDE.md"), "Same."),
        ]
        .map(|(path, text)| (path, text.to_owned()));
        assert_eq!(found, expected);
    }

    #[test]
    fn children_that_can_read_are_told_of_the_skills() {
        let scratch =
            std::env::temp_dir().join(format!("sjrs-child-skills-{}", std::process::id()));
        let _scratch = super::super::tests::ScratchDir::new(scratch.clone());
        let file = scratch.join("pdf/SKILL.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "---\ndescription: PDFs.\n---\nBody").unwrap();
        let skills = Skills::in_directory(&scratch);
        let cwd = Path::new("/nonexistent/scramjet-rs-child");
        let text = child(cwd, &["read", "bash"], "Be brief.", &skills, None);
        let listed = "Be brief.\n\nThe following skills provide specialized instructions for \
specific tasks.\n";
        assert!(text.contains(listed), "{text}");
        let end = format!(
            "<location>{}</location>\n  </skill>\n</available_skills>\n\nCurrent date: ",
            file.display()
        );
        assert!(text.contains(&end), "{text}");
        assert!(!child(cwd, &["bash"], "", &skills, None).contains("<available_skills>"));
    }
}
