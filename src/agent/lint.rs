//! `scramjet lint`: what the harness would say about a command set when loading it, as
//! errors for files it skips and warnings for what it ignores, then the shape Scramjet's
//! `scramjet-command-lint` asks of each command: an early `## Goals` list, and its arguments in
//! one context block. The set's skills are checked as they load.

use super::commands::{Commands, Set, Sets};
use super::{Problem, ProblemFile, Skills, subagent};
use regex::Regex;
use std::path::PathBuf;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub severity: Severity,
    pub path: PathBuf,
    /// 1-based, when one line is at fault.
    pub line: Option<usize>,
    pub message: String,
}

mod checks;

/// The lint of `set`, and the command, agent and skill files it read. `mcp_servers` are the
/// names of the MCP servers the user's settings list, whose tools a set may name.
pub fn lint(set: &Set, mcp_servers: &[String]) -> (Vec<Diagnostic>, Vec<PathBuf>) {
    let sets = Sets::only(set.clone());
    let commands = Commands::discover(&sets);
    let mut problems = commands.warnings().to_vec();
    subagent::registered(&sets, &mut problems);
    let skills = Skills::of_set(&set.directory);
    problems.extend_from_slice(skills.warnings());
    let mut diagnostics: Vec<Diagnostic> = problems
        .into_iter()
        .map(|problem| loaded(problem, set))
        .collect();
    diagnostics.extend(checks::manifest(set));
    let mut files = markdown_files(&set.directory.join("commands"));
    let agents = markdown_files(&set.directory.join("agents"));
    diagnostics.extend(checks::file_names(&files));
    diagnostics.extend(checks::file_names(&agents));
    for path in &files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let lines = visible_lines(&content);
        let mut found = goals(&lines);
        let command = commands.list().iter().find(|command| &command.path == path);
        if let Some(command) = command {
            found.extend(framing(
                &lines,
                command.delegate_only,
                command.argument_hint.is_some(),
            ));
        }
        diagnostics.extend(found.into_iter().map(|(code, line, message)| Diagnostic {
            code,
            severity: Severity::Warning,
            path: path.clone(),
            line,
            message,
        }));
        let tools = checks::tool_names(path, &content, &["allowed-tools"], mcp_servers);
        diagnostics.extend(tools);
        diagnostics.extend(checks::statuses(path, &lines));
        diagnostics.extend(checks::references(set, &commands, path, &content, &lines));
    }
    for path in &agents {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let keys = ["tools", "disallowedTools"];
        diagnostics.extend(checks::tool_names(path, &content, &keys, mcp_servers));
    }
    diagnostics.extend(checks::edges(set, &commands));
    files.extend(agents);
    files.extend(skills.list().iter().map(|skill| skill.path.clone()));
    (diagnostics, files)
}

/// The `.md` files in `directory`, by name.
fn markdown_files(directory: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = (std::fs::read_dir(directory).into_iter().flatten())
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|e| e == "md"))
        .collect();
    files.sort();
    files
}

/// A loading problem: an error when the harness skips what it names, else a warning. The
/// message loses the file's path and name the diagnostic already shows; one about
/// `autonomy-defaults.yaml` points at the line of the edge or command it names.
fn loaded(problem: Problem, set: &Set) -> Diagnostic {
    let skipped = problem.text.starts_with("skipping ") || problem.text.starts_with("could not ");
    let path = match problem.file {
        Some(ProblemFile::Path(path)) => path,
        _ => set.directory.clone(),
    };
    let (shown, file) = (
        path.display().to_string(),
        path.file_name().unwrap_or_default().to_string_lossy(),
    );
    let mut message = problem.text;
    for repeated in [
        format!(" {shown}: {shown}: "),
        format!(" {shown}: {file}: "),
    ] {
        message = message.replacen(&repeated, ": ", 1);
    }
    for prefix in [
        format!("skill {shown}: "),
        format!("{shown}: "),
        format!("{file}: "),
    ] {
        if let Some(rest) = message.strip_prefix(&prefix) {
            message = rest.to_owned();
        }
    }
    let line = match file == "autonomy-defaults.yaml" {
        true => (std::fs::read_to_string(&path).ok())
            .and_then(|text| checks::notice_line(&text, &message)),
        false => None,
    };
    Diagnostic {
        code: if skipped {
            "runtime-unrecognized"
        } else {
            "runtime-notice"
        },
        severity: if skipped {
            Severity::Error
        } else {
            Severity::Warning
        },
        path,
        line,
        message,
    }
}

/// The file's lines outside its frontmatter and fenced code, with their numbers.
fn visible_lines(content: &str) -> Vec<(usize, &str)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let lines: Vec<&str> = content.lines().collect();
    let mut start = 0;
    if lines.first() == Some(&"---")
        && let Some(end) = lines.iter().skip(1).position(|line| *line == "---")
    {
        start = end + 2;
    }
    let mut visible = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, &text) in lines.iter().enumerate().skip(start) {
        let indent = text.len() - text.trim_start_matches(' ').len();
        let rest = &text[indent..];
        let run = |marker: char| rest.chars().take_while(|c| *c == marker).count();
        if let Some((marker, length)) = fence {
            let closes = indent <= 3
                && run(marker) >= length
                && rest.trim_start_matches(marker).trim().is_empty();
            if closes {
                fence = None;
            }
            continue;
        }
        let opening = rest.chars().next().filter(|c| *c == '`' || *c == '~');
        if let Some(marker) = opening
            && indent <= 3
            && run(marker) >= 3
        {
            fence = Some((marker, run(marker)));
            continue;
        }
        visible.push((index + 1, text));
    }
    visible
}

static GOALS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}##[ \t]+Goals(?:[ \t]+#+)?[ \t]*$").unwrap());
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}#{1,2}(?:[ \t]+|$)").unwrap());
static LIST_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(?:[-+*]|\d+[.)])[ \t]+").unwrap());
static SUBSTITUTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$(?:ARGUMENTS|@|\d+|\{@:\d+(?::\d+)?\})").unwrap());
static CONTEXT_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?(?:user|caller)-context>").unwrap());
static PROCEDURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^ {0,3}##[ \t]+(?:Steps?|Actions?|Process|Procedure|Workflow|Method|Implementation|Verification|Review|Summary)\b",
    )
    .unwrap()
});

type Found = (&'static str, Option<usize>, String);

fn goals(lines: &[(usize, &str)]) -> Vec<Found> {
    let headings: Vec<usize> = (lines.iter())
        .filter(|(_, text)| GOALS.is_match(text))
        .map(|(line, _)| *line)
        .collect();
    let Some(_) = headings.first() else {
        let text = "Add an early \"## Goals\" section describing durable outcomes.";
        return vec![("goals-missing", None, text.to_owned())];
    };
    let mut found = Vec::new();
    for &duplicate in &headings[1..] {
        let text = "Keep exactly one \"## Goals\" section.".to_owned();
        found.push(("goals-duplicate", Some(duplicate), text));
    }
    for &heading in &headings {
        let end = (lines.iter())
            .find(|(line, text)| *line > heading && HEADING.is_match(text))
            .map_or(usize::MAX, |(line, _)| *line);
        let section: Vec<&(usize, &str)> = (lines.iter())
            .filter(|(line, _)| *line > heading && *line < end)
            .collect();
        let content: Vec<_> = (section.iter())
            .filter(|(_, text)| !text.trim().is_empty())
            .collect();
        if content.is_empty() {
            let text = "Add durable outcomes beneath \"## Goals\".".to_owned();
            found.push(("goals-empty", Some(heading), text));
        }
        if (lines.iter()).any(|(line, text)| *line < heading && PROCEDURE.is_match(text)) {
            let text = "Move \"## Goals\" before procedural sections.".to_owned();
            found.push(("goals-order", Some(heading), text));
        }
        if !content.is_empty() && !content.iter().any(|(_, text)| LIST_ITEM.is_match(text)) {
            let text = "Express Goals as Markdown list items.".to_owned();
            found.push(("goals-list", Some(heading), text));
        }
        let context = (section.iter())
            .find(|(_, text)| SUBSTITUTION.is_match(text) || CONTEXT_TAG.is_match(text));
        if let Some((line, _)) = context {
            let text = "Keep argument substitution and user/caller context outside Goals.";
            found.push(("goals-context", Some(*line), text.to_owned()));
        }
    }
    found
}

/// Whether the arguments are substituted once, inside the one context block a top-level
/// (`<user-context>`) or delegate-only (`<caller-context>`) command frames them in.
fn framing(lines: &[(usize, &str)], delegate_only: bool, takes_arguments: bool) -> Option<Found> {
    let text = (lines.iter())
        .map(|(_, text)| *text)
        .collect::<Vec<_>>()
        .join("\n");
    let (expected, unexpected) = match delegate_only {
        true => ("caller-context", "user-context"),
        false => ("user-context", "caller-context"),
    };
    let (open, close) = (format!("<{expected}>"), format!("</{expected}>"));
    let stray =
        text.contains(&format!("<{unexpected}>")) || text.contains(&format!("</{unexpected}>"));
    let substitutions: Vec<usize> = SUBSTITUTION.find_iter(&text).map(|m| m.start()).collect();
    if !takes_arguments
        && substitutions.is_empty()
        && !text.contains(&open)
        && !text.contains(&close)
        && !stray
    {
        return None;
    }
    let inside = match (text.find(&open), substitutions.as_slice()) {
        (Some(start), [at]) => {
            let body = start + open.len();
            text[body..]
                .find(&close)
                .is_some_and(|end| *at >= body && *at < body + end)
        }
        _ => false,
    };
    if text.matches(&open).count() == 1 && text.matches(&close).count() == 1 && !stray && inside {
        return None;
    }
    let line = (lines.iter())
        .find(|(_, text)| text.contains("-context>") || SUBSTITUTION.is_match(text))
        .map(|(line, _)| *line);
    let kind = match delegate_only {
        true => "delegate-only",
        false => "top-level",
    };
    let message = format!(
        "Use exactly one argument substitution inside one <{expected}> block for this {kind} \
command."
    );
    Some(("context-framing", line, message))
}

#[cfg(test)]
mod tests {
    use super::super::tests::ScratchDir;
    use super::*;
    use crate::agent::Origin;
    use std::fs;

    fn scratch(name: &str) -> ScratchDir {
        let path =
            std::env::temp_dir().join(format!("scramjet-lint-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        ScratchDir::new(path)
    }

    const GOOD: &str = "---\ndescription: Does it\nargument-hint: \"[focus]\"\n---\n# Do\n\n\
## Goals\n\n- One outcome.\n\n## Focus\n\n<user-context>\n$ARGUMENTS\n</user-context>\n\n\
## Steps\n\n```md\n## Goals\n$1\n```\n";

    fn lint_files(
        name: &str,
        files: &[(&str, &str)],
    ) -> Vec<(String, &'static str, Option<usize>)> {
        lint_with(name, files, &[])
            .into_iter()
            .map(|(file, code, line, _)| (file, code, line))
            .collect()
    }

    /// The lint of a set of `files`, with the MCP servers `servers`: each diagnostic's file
    /// within the set, code, line and message.
    fn lint_with(
        name: &str,
        files: &[(&str, &str)],
        servers: &[String],
    ) -> Vec<(String, &'static str, Option<usize>, String)> {
        let root = scratch(name);
        let directory = root.0.join("set");
        let declared = [("set.yaml", "name: set\n")];
        let declared = declared
            .iter()
            .filter(|_| files.iter().all(|(n, _)| *n != "set.yaml"));
        for (name, content) in files.iter().chain(declared) {
            let path = directory.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        let set = Set::at(&directory, Origin::Installed).unwrap();
        let (diagnostics, _) = lint(&set, servers);
        (diagnostics.into_iter())
            .map(|d| {
                let relative = d.path.strip_prefix(&directory).unwrap();
                let parts: Vec<_> = (relative.iter())
                    .map(|part| part.to_string_lossy())
                    .collect();
                let file = parts.join("/");
                (file, d.code, d.line, d.message)
            })
            .collect()
    }

    #[test]
    fn this_repositorys_scramjet_set_is_clean() {
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR").unwrap();
        let directory = crate::os::join(std::path::Path::new(&manifest), "sets/scramjet");
        let set = Set::at(&directory, Origin::Installed).unwrap();
        let (diagnostics, files) = lint(&set, &[]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let names: Vec<_> = (files.iter())
            .map(|file| file.strip_prefix(&directory).unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "commands/troubleshoot.md",
                "agents/command-architect.md",
                "agents/command-failure-analyst.md",
                "agents/command-reviewer.md",
                "agents/command-set-explorer.md",
                "agents/independent-command-assessor.md",
                "agents/instruction-semantics-analyzer.md",
                "skills/setting-up-command-sets/SKILL.md",
                "skills/writing-scramjet-commands/SKILL.md"
            ]
            .map(std::path::PathBuf::from)
        );
    }

    #[test]
    fn the_sets_skills_are_checked_as_they_load() {
        let named = "---\nname: Named\ndescription: D\n---\nBody\n";
        let found = lint_files(
            "skills",
            &[
                ("commands/do.md", GOOD),
                ("skills/bare/SKILL.md", "---\nname: bare\n---\nBody\n"),
                ("skills/named/SKILL.md", named),
                ("skills/top.md", "Not a skill: only directories are.\n"),
            ],
        );
        let shown: Vec<_> = found
            .iter()
            .map(|(file, code, _)| (file.as_str(), *code))
            .collect();
        assert_eq!(
            shown,
            [
                ("skills/bare/SKILL.md", "runtime-unrecognized"),
                ("skills/named/SKILL.md", "runtime-notice"),
            ]
        );
    }

    #[test]
    fn a_command_with_early_goals_and_framed_arguments_is_clean() {
        assert_eq!(lint_files("clean", &[("commands/do.md", GOOD)]), []);
    }

    #[test]
    fn goals_missing_late_unlisted_or_holding_arguments_are_warned_about_at_their_line() {
        let late = "# X\n\n## Steps\n\nGo.\n\n## Goals\n\nProse only, $ARGUMENTS.\n";
        let twice = "## Goals\n\n- A\n\n## Goals\n\n";
        let found = lint_files(
            "goals",
            &[
                ("commands/late.md", late),
                ("commands/none.md", "# Nothing\n"),
                ("commands/twice.md", twice),
            ],
        );
        let expected = [
            ("commands/late.md", "goals-order", Some(7)),
            ("commands/late.md", "goals-list", Some(7)),
            ("commands/late.md", "goals-context", Some(9)),
            ("commands/late.md", "context-framing", Some(9)),
            ("commands/none.md", "goals-missing", None),
            ("commands/twice.md", "goals-duplicate", Some(5)),
            ("commands/twice.md", "goals-empty", Some(5)),
        ];
        let expected: Vec<_> = (expected.iter())
            .map(|(file, code, line)| (file.to_string(), *code, *line))
            .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn a_delegate_only_command_frames_its_arguments_as_its_callers() {
        let body = "---\ndelegate-only: true\n---\n## Goals\n\n- A\n\n## Input\n\n<user-context>\n$ARGUMENTS\n\
</user-context>\n";
        let fixed = body.replace("user-context", "caller-context");
        let hinted = "---\nargument-hint: X\n---\n## Goals\n\n- A\n";
        let found = lint_files(
            "framing",
            &[
                ("commands/callee.md", body),
                ("commands/fixed.md", &fixed),
                ("commands/hinted.md", hinted),
            ],
        );
        let expected = [
            ("commands/callee.md".to_owned(), "context-framing", Some(10)),
            ("commands/hinted.md".to_owned(), "context-framing", None),
        ];
        assert_eq!(found, expected);
    }

    #[test]
    fn files_the_harness_skips_are_errors_and_what_it_ignores_warnings() {
        let root = scratch("loaded");
        let directory = root.0.join("set");
        fs::create_dir_all(directory.join("commands")).unwrap();
        fs::create_dir_all(directory.join("agents")).unwrap();
        fs::write(directory.join("set.yaml"), "name: set\n").unwrap();
        let broken = directory.join("commands/broken.md");
        fs::write(&broken, "---\nnext: [\n---\n## Goals\n\n- A\n").unwrap();
        let ignored = "---\ndelegate-only: yes\n---\n## Goals\n\n- A\n";
        fs::write(directory.join("commands/odd.md"), ignored).unwrap();
        let agent = directory.join("agents/nameless.md");
        fs::write(&agent, "---\ndescription: D\n---\nBody\n").unwrap();
        let set = Set::at(&directory, Origin::Installed).unwrap();
        let (diagnostics, files) = lint(&set, &[]);
        assert_eq!(files.len(), 3);
        let shown: Vec<_> = (diagnostics.iter())
            .map(|d| (d.path.clone(), d.severity, d.code))
            .collect();
        assert_eq!(
            shown,
            [
                (broken, Severity::Error, "runtime-unrecognized"),
                (
                    directory.join("commands/odd.md"),
                    Severity::Warning,
                    "runtime-notice"
                ),
                (agent, Severity::Error, "runtime-unrecognized"),
            ]
        );
    }

    fn owned(
        expected: &[(&str, &'static str, Option<usize>)],
    ) -> Vec<(String, &'static str, Option<usize>)> {
        (expected.iter())
            .map(|(file, code, line)| (file.to_string(), *code, *line))
            .collect()
    }

    #[test]
    fn names_windows_cannot_check_out_a_missing_set_yaml_and_another_plugin_name_are_warned_about()
    {
        let plugin = "{\n  \"name\": \"other\",\n  \"license\": \"MIT\"\n}\n";
        let found = lint_files(
            "names",
            &[
                ("set.yaml", "name: set\n"),
                ("commands/set:do.md", GOOD),
                (
                    "agents/set:helper.md",
                    "---\nname: helper\ndescription: D\n---\nBody\n",
                ),
                (".claude-plugin/plugin.json", plugin),
            ],
        );
        let expected = [
            (".claude-plugin/plugin.json", "plugin-name", Some(2)),
            ("commands/set:do.md", "file-name-colon", None),
            ("agents/set:helper.md", "file-name-colon", None),
        ];
        assert_eq!(found, owned(&expected));
        let root = scratch("undeclared");
        let directory = root.0.join("loose");
        fs::create_dir_all(directory.join("commands")).unwrap();
        fs::write(directory.join("commands/do.md"), GOOD).unwrap();
        fs::create_dir_all(directory.join(".claude-plugin")).unwrap();
        fs::write(
            directory.join(".claude-plugin/plugin.json"),
            "{\"name\": \"x\"}",
        )
        .unwrap();
        let set = Set::at(&directory, Origin::Installed).unwrap();
        let codes: Vec<_> = lint(&set, &[])
            .0
            .into_iter()
            .map(|d| (d.path, d.code))
            .collect();
        assert_eq!(codes, [(directory, "set-yaml-missing")]);
    }

    #[test]
    fn a_plugin_version_without_a_versioning_section_and_tags_without_a_version_are_warned_about() {
        let versioned = "{\n  \"name\": \"set\",\n  \"version\": \"0.1.0\"\n}\n";
        let plugin = [
            (".claude-plugin/plugin.json", versioned),
            ("commands/do.md", GOOD),
        ];
        let expected = [(
            ".claude-plugin/plugin.json",
            "plugin-version-unrecorded",
            Some(3),
        )];
        assert_eq!(lint_files("unrecorded", &plugin), owned(&expected));
        let claude = "# Set\n\n## Versioning\n\nSemVer, tagged vX.Y.Z.\n";
        let recorded = [plugin[0], plugin[1], ("CLAUDE.md", claude)];
        assert_eq!(lint_files("recorded", &recorded), []);

        let root = scratch("tagged");
        let directory = root.0.join("set");
        fs::create_dir_all(directory.join(".claude-plugin")).unwrap();
        fs::create_dir_all(directory.join("commands")).unwrap();
        fs::write(directory.join("commands/do.md"), GOOD).unwrap();
        fs::write(directory.join("set.yaml"), "name: set\n").unwrap();
        fs::write(
            directory.join(".claude-plugin/plugin.json"),
            "{\"name\": \"set\"}",
        )
        .unwrap();
        let codes = || -> Vec<_> {
            let set = Set::at(&directory, Origin::Installed).unwrap();
            (lint(&set, &[]).0.into_iter())
                .map(|d| (d.code, d.message))
                .collect()
        };
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
                .arg(&root.0)
                .args(args)
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "x"]);
        git(&["tag", "v0.9.0-rc1"]);
        assert_eq!(codes(), []);
        git(&["tag", "v0.2.0"]);
        git(&["tag", "v0.10.0"]);
        let found = codes();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "plugin-version-missing");
        assert!(found[0].1.contains("(v0.10.0)"), "{}", found[0].1);
    }

    #[test]
    fn tool_names_claude_code_denies_or_no_harness_has_are_warned_about_with_the_right_one() {
        let command = "---\nallowed-tools:\n  - Read\n  - bash\n  - Bash(git log *)\n  - Reed\n  \
- mcp__tavily__search\n  - mcp__nowhere__x\n  - delegate\n  - report_scramjet_command_status\n---\n\
## Goals\n\n- A\n";
        let agent = "---\nname: helper\ndescription: D\ntools: Read, find, WebFetch, Agent\n\
disallowedTools: Wrote\n---\nBody\n";
        let found = lint_with(
            "tools",
            &[("commands/do.md", command), ("agents/helper.md", agent)],
            &["tavily".to_owned()],
        );
        let shown: Vec<_> = (found.iter())
            .map(|(file, code, line, message)| (file.as_str(), *code, *line, message.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                (
                    "commands/do.md",
                    "tool-case",
                    Some(4),
                    "Claude Code matches tool names by case and denies \"bash\": write \"Bash\", \
which Scramjet reads as its bash"
                ),
                (
                    "commands/do.md",
                    "tool-unknown",
                    Some(6),
                    "\"Reed\" is not a tool of Scramjet, Claude Code or an MCP server \
(mcp__<server>__<tool>); did you mean \"Read\"?"
                ),
                (
                    "commands/do.md",
                    "tool-unchecked",
                    Some(8),
                    "\"mcp__nowhere__x\" is a tool of MCP server \"nowhere\", which your settings \
do not list, so it cannot be checked"
                ),
                (
                    "agents/helper.md",
                    "tool-case",
                    Some(4),
                    "Claude Code matches tool names by case and denies \"find\": write \"Glob\", \
which Scramjet reads as its find"
                ),
                (
                    "agents/helper.md",
                    "tool-unknown",
                    Some(5),
                    "\"Wrote\" is not a tool of Scramjet, Claude Code or an MCP server \
(mcp__<server>__<tool>); did you mean \"Write\"?"
                ),
            ]
        );
    }

    #[test]
    fn statuses_the_harness_refuses_are_warned_about_where_a_report_names_them() {
        let body = "## Goals\n\n- A\n\n## Report\n\n\
Call `report_scramjet_command_status` with `status: \"completed\"`.\n\
If it stopped, report the matching `status` (`blocked` / `incomplete`) instead.\n\
Report `completed`, `continuing` or `abandoned`.\n\
Set status: dormant when idle.\n\
This command has a `forced` next step; report `completed`.\n\
A `function_call` line named `report_scramjet_command_status` whose `arguments` holds its `status`.\n\
\n```\nreport status: parked\n```\n";
        let found = lint_with("statuses", &[("commands/do.md", body)], &[]);
        let shown: Vec<_> = (found.iter())
            .map(|(_, code, line, message)| (*code, *line, message.split('`').nth(5).unwrap()))
            .collect();
        assert_eq!(
            shown,
            [
                ("status-unknown", Some(8), "blocked"),
                ("status-unknown", Some(8), "incomplete"),
                ("status-unknown", Some(9), "continuing"),
                ("status-unknown", Some(10), "dormant"),
            ]
        );
    }

    #[test]
    fn next_steps_mentions_and_edges_that_lead_nowhere_are_warned_about_at_their_line() {
        let first = "---\nnext:\n  mode: closed\n  candidates:\n    - name: set:second\n    \
- name: second\n    - name: set:gone\n    - name: other:thing\n    - name: set:helper\n---\n\
## Goals\n\n- A\n\nThen /set:second, (/set:gone) or /other:thing; see `/set:second`.\n";
        let second = "---\nnext:\n  mode: open\n  candidates: []\n  blacklist: [set:first]\n---\n\
## Goals\n\n- B\n";
        let helper = "---\ndelegate-only: true\n---\n## Goals\n\n- C\n";
        let edges = "edges:\n  set:first:\n    set:second: chain\n    set:third: pause\n  \
set:second:\n    set:first: pause\n    set:third: chain\n  set:helper:\n    \"*\": pause\n";
        let found = lint_files(
            "references",
            &[
                ("commands/first.md", first),
                ("commands/second.md", second),
                ("commands/third.md", "## Goals\n\n- D\n"),
                ("commands/helper.md", helper),
                ("autonomy-defaults.yaml", edges),
            ],
        );
        let expected = [
            ("commands/first.md", "next-unknown", Some(6)),
            ("commands/first.md", "next-unknown", Some(7)),
            ("commands/first.md", "next-unknown", Some(9)),
            ("commands/first.md", "command-unknown", Some(15)),
            ("autonomy-defaults.yaml", "edge-unreachable", Some(4)),
            ("autonomy-defaults.yaml", "edge-unreachable", Some(6)),
        ];
        assert_eq!(found, owned(&expected));
    }

    #[test]
    fn notices_lose_the_path_they_repeat_and_point_at_the_edge_they_name() {
        let edges = "edges:\n  set:do:\n    set:gone: chain\n    set:do: sometimes\n";
        let found = lint_with(
            "notices",
            &[
                ("commands/do.md", GOOD),
                (
                    "commands/odd.md",
                    "---\ndelegate-only: yes\n---\n## Goals\n\n- A\n",
                ),
                ("commands/broken.md", "---\nnext: [\n---\n## Goals\n\n- A\n"),
                ("autonomy-defaults.yaml", edges),
            ],
            &[],
        );
        let shown: Vec<_> = (found.iter())
            .map(|(file, _, line, message)| (file.as_str(), *line, message.as_str()))
            .collect();
        assert_eq!(shown[0].0, "commands/broken.md");
        assert!(
            shown[0].2.starts_with("skipping: malformed frontmatter"),
            "{shown:?}"
        );
        assert_eq!(
            &shown[1..],
            [
                (
                    "commands/odd.md",
                    None,
                    "\"delegate-only\" must be exactly true when present (got \"yes\"; treated as \
absent)"
                ),
                (
                    "autonomy-defaults.yaml",
                    Some(4),
                    "unknown autonomy recommendation value \"sometimes\" for edge set:do → set:do \
(expected: chain, pause, default)"
                ),
                (
                    "autonomy-defaults.yaml",
                    Some(3),
                    "unknown target command \"set:gone\" (in set:do)"
                ),
            ]
        );
    }
}
