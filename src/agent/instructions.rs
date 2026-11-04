//! The system instructions of the agent and of its subagents.
use super::{Commands, Skills, publish, question, status, subagent, suggest, switch};
use std::path::{Path, PathBuf};

const CONTEXT_FILE_BYTES: u64 = 256 << 10;
/// Scramjet's base directives, from its command framing to its release timing.
const DIRECTIVES: &str = include_str!("directives.md");
/// This build's README, which the instructions point to when it is there.
pub(super) const README: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/README.md");

/// The agent's instructions, in Scramjet's order: Pi's list of its coding `tools` and the
/// guidelines for them, the project's context files, the `skills`, Scramjet's directives, then
/// the subagents and commands. Each request adds `identity` and `environment`.
pub(super) fn agent(
    root: &Path,
    tools: &[String],
    commands: &Commands,
    agents: &[subagent::Definition],
    skills: &Skills,
) -> String {
    let catalog: String = [subagent::catalog(agents), commands.catalog()]
        .into_iter()
        .filter(|catalog| !catalog.is_empty())
        .map(|catalog| catalog + "\n\n")
        .collect();
    let has = |tool: &str| tools.iter().any(|name| name == tool);
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
    listed.push(format!("- {}: {}", status::NAME, status::SNIPPET));
    listed.extend(publish::TOOLS.map(|tool| format!("- {tool}: {}", publish::snippet(tool))));
    listed.extend([
        format!("- {}: {}", question::NAME, question::SNIPPET),
        format!("- {}: {}", subagent::NAME, subagent::SNIPPET),
        format!("- {}: {}", switch::NAME, switch::SNIPPET),
        format!("- {}: {}", suggest::NAME, suggest::SNIPPET),
    ]);
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
    guidelines.extend([
        suggest::GUIDELINE,
        "- Be concise in your responses",
        "- Show file paths clearly when working with files",
    ]);
    let text = format!(
        "You are scramjet-rs, a coding agent. You help the user with software engineering tasks \
in the project directory {root}, using the tools below.

Available tools:
{tools}

Guidelines:
{guidelines}
",
        root = root.display(),
        tools = listed.join("\n"),
        guidelines = guidelines.join("\n"),
    );
    let text = text
        + &project_context(root)
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
    if Path::new(README).is_file() {
        text += &format!(
            "\nscramjet-rs documentation (read only when the user asks about scramjet-rs itself — \
its commands, command sets, next steps, delegation, settings, keys or the /autopilot on/off \
flag): {README}\n"
        );
    }
    text
}

/// A model as the Model Identity names it, from its Proxy card.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Identity {
    pub name: Option<String>,
    pub provider: Option<String>,
}

/// Scramjet's `# Model Identity` for a request to `current`, after replies of `contributors` on
/// the session's branch: the model attributes published work with it, as Mach 12's reviews ask.
/// The current model is listed among them before it has replied, so that the text stays the
/// same after its first reply, and the cached prefix with it.
pub(super) fn identity(
    current: &str,
    contributors: &[String],
    known: &std::collections::HashMap<String, Identity>,
) -> String {
    let unknown = Identity::default();
    let name = |model: &str| -> String {
        let identity = known.get(model).unwrap_or(&unknown);
        let name = identity.name.as_deref().unwrap_or(model);
        match &identity.provider {
            Some(provider) => format!("{name} (ID: {model}, provider: {provider})"),
            None => format!("{name} (ID: {model})"),
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
    let short = (known
        .get(current)
        .and_then(|identity| identity.name.as_deref()))
    .unwrap_or(current);
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
/// session's file.
pub(super) fn environment(root: &Path, journal: &Path) -> String {
    format!(
        "\nCurrent date: {}\nCurrent working directory: {}\nCurrent session journal: {}",
        today(),
        root.display(),
        journal.display()
    )
}

/// Pi's list of skills, only for a model that has `read` to load them.
fn skills_section(read: bool, skills: &Skills) -> String {
    match (read, skills.catalog()) {
        (true, catalog) if !catalog.is_empty() => format!("\n{catalog}"),
        _ => String::new(),
    }
}

/// A subagent's instructions, as Pi builds a child's: its tools and guidelines, the agent's own
/// text, the project's context files from `cwd` and the `skills`, then the date and directory.
pub(super) fn child(cwd: &Path, tools: &[&str], prompt: &str, skills: &Skills) -> String {
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
    text += &project_context(cwd);
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

/// The `AGENTS.md` and `CLAUDE.md` files of `root` and the directories above it, outermost
/// first, with their contents.
pub(super) fn context_files(root: &Path) -> Vec<(PathBuf, String)> {
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
    files.reverse();
    files
}

/// The context files of `root`, as Pi's `# Project Context`; empty without any.
fn project_context(root: &Path) -> String {
    let mut text = String::new();
    let files = context_files(root);
    if !files.is_empty() {
        text.push_str("\n# Project Context\n\nProject-specific instructions and guidelines:\n");
        for (path, contents) in &files {
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
    fn the_tools_and_guidelines_listed_are_the_configured_ones() {
        let root = Path::new("/nonexistent/scramjet-rs-instructions");
        let commands = Commands::default();
        let tools = super::super::tools::DEFAULT_TOOLS.map(String::from);
        let text = agent(root, &tools, &commands, &[], &Skills::default());
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

        let text = agent(root, &[], &commands, &[], &Skills::default());
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
        let all = super::super::tools::CODING_TOOLS.map(String::from);
        let text = agent(root, &all, &commands, &[], &Skills::default());
        assert!(text.contains(
            "Guidelines:\n- Prefer grep/find/ls tools over bash for file exploration (faster, \
respects .gitignore)\n- Use read"
        ));
    }

    #[test]
    fn the_model_identity_names_models_as_their_cards_do_and_by_id_without_one() {
        let known = std::collections::HashMap::from([(
            "c".to_owned(),
            Identity {
                name: Some("Claude C".into()),
                provider: Some("anthropic".into()),
            },
        )]);
        let text = identity("x", &["c".to_owned()], &known);
        let expected = "\n\n# Model Identity\nYour model for the imminent response is: x (ID: x).\n\
Models with material responses in this session branch, the imminent one included:\n- Claude C (ID: \
c, provider: anthropic)\n- x (ID: x)\nWhen posting to GitHub, use honest contribution-aware \
attribution:\n- Credit the current model for work in the imminent response.\n- Credit prior \
models only when their material responses contributed to the published work.\n- Single credited \
model: \"Reviewed by x\"\n- Multiple credited models: describe each model's contribution (e.g., \
\"Reviewed by X (analysis) and Y (posting)\")";
        assert_eq!(text, expected);
        assert!(identity("c", &[], &known).contains("is: Claude C (ID: c, provider: anthropic)."));
    }

    #[test]
    fn the_directives_follow_the_context_and_skills_and_the_catalogs_follow_them() {
        let root = Path::new("/nonexistent/scramjet-rs-directives");
        let text = agent(root, &[], &Commands::default(), &[], &Skills::default());
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
next steps, delegation, settings, keys or the /autopilot on/off flag): {README}\n"
        )));
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
        let text = child(cwd, &["read", "bash"], "Be brief.", &skills);
        let listed = "Be brief.\n\nThe following skills provide specialized instructions for \
specific tasks.\n";
        assert!(text.contains(listed), "{text}");
        let end = format!(
            "<location>{}</location>\n  </skill>\n</available_skills>\n\nCurrent date: ",
            file.display()
        );
        assert!(text.contains(&end), "{text}");
        assert!(!child(cwd, &["bash"], "", &skills).contains("<available_skills>"));
    }
}
