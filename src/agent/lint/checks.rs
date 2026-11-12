//! The checks of `scramjet lint` beyond what loading a set says: names Windows or Claude Code
//! cannot use, tools no harness knows, statuses the harness refuses, and next steps, edges and
//! mentions that lead nowhere.

use super::{Diagnostic, Severity};
use crate::agent::commands::{self, Commands, Policy, Set};
use crate::agent::{autonomy, tools, yaml};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use yaml_rust2::Yaml;

fn warning(code: &'static str, path: &Path, line: Option<usize>, message: String) -> Diagnostic {
    Diagnostic {
        code,
        severity: Severity::Warning,
        path: path.to_path_buf(),
        line,
        message,
    }
}

/// Command and agent files named `<set>:<name>.md`, which Windows cannot check out.
pub(super) fn file_names(files: &[PathBuf]) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for path in files {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some((_, bare)) = file.split_once(':') {
            let text = format!(
                "Windows cannot check out a file name with ':'; name it {bare}, as set.yaml \
                 names the set"
            );
            found.push(warning("file-name-colon", path, None, text));
        }
    }
    found
}

/// A set without `set.yaml`, and a Claude Code `plugin.json` naming the plugin otherwise.
pub(super) fn manifest(set: &Set) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    let declared = set.directory.join("set.yaml");
    if !declared.is_file() {
        let text = format!(
            "No set.yaml: the set takes its directory's name, \"{0}\", which a clone or copy can \
             change; add a set.yaml with `name: {0}`",
            set.name
        );
        found.push(warning("set-yaml-missing", &set.directory, None, text));
    }
    let plugin = set.directory.join(".claude-plugin").join("plugin.json");
    let Ok(text) = std::fs::read_to_string(&plugin) else {
        return found;
    };
    let name = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(value) => value["name"].as_str().map(str::to_owned),
        Err(error) => {
            let text = format!("Claude Code cannot read this plugin.json: {error}");
            found.push(warning("plugin-json", &plugin, Some(error.line()), text));
            return found;
        }
    };
    if let Some(name) = name.filter(|name| declared.is_file() && *name != set.name) {
        let line = text.lines().position(|line| line.contains("\"name\""));
        let text = format!(
            "plugin.json names the plugin \"{name}\" and set.yaml the set \"{0}\": Claude Code \
             would run /{name}:… and Scramjet /{0}:…; give both one name",
            set.name
        );
        found.push(warning("plugin-name", &plugin, line.map(|at| at + 1), text));
    }
    found
}

/// Claude Code's tools (`code.claude.com/docs/en/tools-reference.md`, read 2026-11-11), with
/// older names sets still use.
const CLAUDE_CODE: &[&str] = &[
    "Agent",
    "Artifact",
    "AskUserQuestion",
    "Bash",
    "BashOutput",
    "CronCreate",
    "CronDelete",
    "CronList",
    "Edit",
    "EndConversation",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "Glob",
    "Grep",
    "KillShell",
    "LS",
    "LSP",
    "ListAgents",
    "ListMcpResourcesTool",
    "Monitor",
    "MultiEdit",
    "NotebookEdit",
    "NotebookRead",
    "PowerShell",
    "PushNotification",
    "Read",
    "ReadMcpResourceTool",
    "RemoteTrigger",
    "ReportFindings",
    "ScheduleWakeup",
    "SendFeedback",
    "SendMessage",
    "SendUserFile",
    "ShareOnboardingGuide",
    "Skill",
    "SlashCommand",
    "SubagentHandback",
    "Task",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    "TodoWrite",
    "ToolSearch",
    "WaitForMcpServers",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

/// Claude Code's name for one of Scramjet's coding tools; Scramjet reads both.
fn claude_name(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "bash" => "Bash",
        "grep" => "Grep",
        "find" => "Glob",
        "ls" => "LS",
        "powershell" => "PowerShell",
        _ => return None,
    })
}

fn scramjet_tool(tool: &str) -> bool {
    tool == "cmd" || tools::HARNESS_TOOLS.contains(&tool)
}

/// The tools the frontmatter's `keys` list (a YAML list or comma-separated text) that Claude
/// Code would deny for their case, or that neither harness nor a configured MCP server has.
pub(super) fn tool_names(
    path: &Path,
    content: &str,
    keys: &[&str],
    mcp_servers: &[String],
) -> Vec<Diagnostic> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (frontmatter, _) = commands::split_frontmatter(content);
    let Ok(frontmatter) = yaml::first(&frontmatter) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for key in keys {
        let listed: Vec<String> = match &frontmatter[*key] {
            Yaml::Array(items) => (items.iter().filter_map(Yaml::as_str))
                .map(str::to_owned)
                .collect(),
            Yaml::String(text) => text.split(',').map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        for tool in listed.iter().map(|tool| tool.trim()) {
            let base = tool.split('(').next().unwrap_or(tool).trim();
            if base.is_empty() {
                continue;
            }
            let line = frontmatter_line(content, tool);
            let message = if let Some(name) = claude_name(base) {
                format!(
                    "Claude Code matches tool names by case and denies \"{base}\": write \
                     \"{name}\", which Scramjet reads as its {base}"
                )
            } else if scramjet_tool(base) || CLAUDE_CODE.contains(&base) {
                continue;
            } else if let Some(rest) = base.strip_prefix("mcp__") {
                let server = rest.split("__").next().unwrap_or(rest);
                if mcp_servers.iter().any(|known| known == server) {
                    continue;
                }
                let text = format!(
                    "\"{base}\" is a tool of MCP server \"{server}\", which your settings do not \
                     list, so it cannot be checked"
                );
                found.push(warning("tool-unchecked", path, line, text));
                continue;
            } else {
                let known = (CLAUDE_CODE.iter().copied())
                    .chain(tools::HARNESS_TOOLS)
                    .chain(["cmd"]);
                match closest(base, known) {
                    Some(near) => format!(
                        "\"{base}\" is not a tool of Scramjet, Claude Code or an MCP server \
                         (mcp__<server>__<tool>); did you mean \"{near}\"?"
                    ),
                    None => format!(
                        "\"{base}\" is not a tool of Scramjet, Claude Code or an MCP server \
                         (mcp__<server>__<tool>)"
                    ),
                }
            };
            let code = match claude_name(base) {
                Some(_) => "tool-case",
                None => "tool-unknown",
            };
            found.push(warning(code, path, line, message));
        }
    }
    found
}

/// The name in `known` nearest `name` by edits, ignoring case, if near enough to be a slip.
fn closest<'a>(name: &str, known: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let name = name.to_lowercase();
    let (distance, near) = known
        .map(|candidate| (edits(&name, &candidate.to_lowercase()), candidate))
        .min_by_key(|(distance, _)| *distance)?;
    (distance <= 2.max(name.chars().count() / 3)).then_some(near)
}

/// Levenshtein distance.
fn edits(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            let substituted = previous[j] + usize::from(x != *y);
            current.push(substituted.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// The 1-based line of the frontmatter that holds `name` as a whole name.
fn frontmatter_line(content: &str, name: &str) -> Option<usize> {
    let mut lines = content.lines().enumerate();
    lines.next().filter(|(_, line)| *line == "---")?;
    (lines.take_while(|(_, line)| !line.starts_with("---")))
        .find(|(_, line)| holds(line, name))
        .map(|(index, _)| index + 1)
}

/// Whether `name` is in `line` with no other name characters on either side.
fn holds(line: &str, name: &str) -> bool {
    let part = |c: char| c.is_alphanumeric() || "_:.-".contains(c);
    (line.match_indices(name)).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + name.len()..].chars().next();
        !before.is_some_and(part) && !after.is_some_and(|c| part(c) && c != ':' && c != '.')
    })
}

static STATUS_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bstatus\b["'`]?[ \t]*[:=][ \t]*["'`]?([A-Za-z][A-Za-z_-]*)"#).unwrap()
});
static LIST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"`[a-z]+`(?:[ \t]*(?:,[ \t]*(?:or|and)|,|/|\||or|and)[ \t]*`[a-z]+`)*").unwrap()
});
static AFTER_STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)status(?:es)?[`"']?[ \t]*(?:[:(=]|\bis\b|\bas\b|\bof\b)?[ \t]*$"#).unwrap()
});
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([a-z]+)`").unwrap());
static REPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)report").unwrap());

const STATUSES: [&str; 2] = ["completed", "abandoned"];

/// Statuses the harness refuses: `status: x`, and on a line about reporting, backticked words
/// listed right after "status" or with `completed` or `abandoned`.
pub(super) fn statuses(path: &Path, lines: &[(usize, &str)]) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for &(line, text) in lines {
        let mut values: Vec<&str> = (STATUS_VALUE.captures_iter(text))
            .map(|captures| captures.get(1).expect("group").as_str())
            .collect();
        if REPORT.is_match(text) {
            for list in LIST.find_iter(text) {
                let words: Vec<&str> = (WORD.captures_iter(list.as_str()))
                    .map(|captures| captures.get(1).expect("group").as_str())
                    .collect();
                let with_status = words.len() > 1 && words.iter().any(|w| STATUSES.contains(w));
                if with_status || AFTER_STATUS.is_match(&text[..list.start()]) {
                    values.extend(words);
                }
            }
        }
        let mut seen = Vec::new();
        for value in values {
            if STATUSES.contains(&value) || seen.contains(&value) {
                continue;
            }
            seen.push(value);
            let text = format!(
                "The harness takes only `completed` or `abandoned` as a status and refuses \
                 `{value}`; for work that cannot go on, report `completed` saying so, or ask the \
                 user with get_scramjet_user_input"
            );
            found.push(warning("status-unknown", path, Some(line), text));
        }
    }
    found
}

/// What a `next:` name is wrong with, `None` when it names a command that can follow or one of
/// another set, which this lint does not see.
fn unusable(name: &str, set: &Set, commands: &Commands) -> Option<String> {
    match commands.get(name) {
        Some(command) if command.delegate_only => {
            return Some(format!(
                "{name} is delegate-only, so it cannot be a next step"
            ));
        }
        Some(_) => return None,
        None => {}
    }
    match name.split_once(':') {
        None if commands.get(&format!("{}:{name}", set.name)).is_some() => Some(format!(
            "\"{name}\" is matched as written: write \"{}:{name}\"",
            set.name
        )),
        None => Some(format!(
            "\"{name}\" is not a command: write <set>:<command>"
        )),
        Some((prefix, _)) if prefix == set.name => Some(format!("{name} is not a command")),
        Some(_) => None,
    }
}

/// The names a `next:` policy gives, those it allows to follow and those it blacklists.
fn named(policy: &Policy) -> Vec<&str> {
    match policy {
        Policy::Forced { target } => vec![target.as_str()],
        Policy::Closed { candidates } => candidates.iter().map(|c| c.name.as_str()).collect(),
        Policy::Open {
            candidates,
            blacklist,
        } => (candidates.iter().map(|c| c.name.as_str()))
            .chain(blacklist.iter().map(String::as_str))
            .collect(),
        Policy::Ask { .. } => Vec::new(),
    }
}

static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s(\[`'\x22])/([A-Za-z0-9_.-]+):([A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*)")
        .unwrap()
});

/// `next:` names of `path`'s command that cannot follow it, and `/<set>:<command>` mentions in
/// its body (`lines`) that name none of the set's commands.
pub(super) fn references(
    set: &Set,
    commands: &Commands,
    path: &Path,
    content: &str,
    lines: &[(usize, &str)],
) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    let command = commands.list().iter().find(|command| command.path == path);
    for name in command
        .and_then(|c| c.next.as_ref())
        .map_or(Vec::new(), named)
    {
        if let Some(why) = unusable(name, set, commands) {
            let line = frontmatter_line(content, name);
            found.push(warning("next-unknown", path, line, why));
        }
    }
    for &(line, text) in lines {
        for captures in MENTION.captures_iter(text) {
            let (prefix, rest) = (&captures[1], &captures[2]);
            let name = format!("{prefix}:{rest}");
            if prefix == set.name && commands.get(&name).is_none() {
                let text = format!("/{name} is not a command of this set");
                found.push(warning("command-unknown", path, Some(line), text));
            }
        }
    }
    found
}

static NAMED: LazyLock<[Regex; 4]> = LazyLock::new(|| {
    [
        r"edge (\S+) → (\S+)",
        r"tool (\S+) → (\S+)",
        r#""([^"]+)" \(in ([^)]+)\)"#,
        r#"command "([^"]+)""#,
    ]
    .map(|pattern| Regex::new(pattern).unwrap())
});

/// The line of `autonomy-defaults.yaml`'s `text` that a loading notice's `message` names.
pub(super) fn notice_line(text: &str, message: &str) -> Option<usize> {
    let [edge, tool, within, command] = &*NAMED;
    let keys = if let Some(found) = edge.captures(message).or_else(|| tool.captures(message)) {
        [found.get(1), found.get(2)]
    } else if let Some(found) = within.captures(message) {
        [found.get(2), found.get(1)]
    } else {
        [
            command.captures(message).and_then(|found| found.get(1)),
            None,
        ]
    };
    let keys: Vec<&str> = keys.into_iter().flatten().map(|key| key.as_str()).collect();
    (!keys.is_empty()).then(|| key_line(text, &keys)).flatten()
}

/// The 1-based line of the YAML key `keys`' last, each key looked for after the one before:
/// where an edge `source → target` is.
fn key_line(text: &str, keys: &[&str]) -> Option<usize> {
    let mut at = None;
    let mut from = 0;
    for key in keys {
        let found = (text.lines().enumerate().skip(from)).find(|(_, line)| {
            let line = line.trim_start().trim_start_matches(['"', '\'']);
            line.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start_matches(['"', '\'']).starts_with(':'))
        });
        if let Some((index, _)) = found {
            at = Some(index + 1);
            from = index + 1;
        }
    }
    at
}

/// `autonomy-defaults.yaml` edges whose source's `next:` block never offers their target.
pub(super) fn edges(set: &Set, commands: &Commands) -> Vec<Diagnostic> {
    let path = set.directory.join("autonomy-defaults.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok((edges, _)) = autonomy::defaults(&text, &mut Vec::new()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (source, targets) in &edges {
        let Some(command) = commands.get(source) else {
            continue;
        };
        let known = |target: &&(String, _)| target.0 != "*" && commands.get(&target.0).is_some();
        for (target, _) in targets.iter().filter(known) {
            let why = match &command.next {
                None => "has no next: block",
                Some(Policy::Ask { .. }) => "leaves the next step to the user (mode: ask)",
                Some(Policy::Forced { target: forced }) if forced != target => {
                    "is always followed by its forced target"
                }
                Some(Policy::Closed { candidates })
                    if candidates.iter().all(|c| c.name != *target) =>
                {
                    "does not list it among its closed candidates"
                }
                Some(Policy::Open { blacklist, .. }) if blacklist.contains(target) => {
                    "blacklists it"
                }
                Some(_) => continue,
            };
            let message = format!("edge {source} → {target} is never taken: {source} {why}");
            let line = key_line(&text, &[source, target]);
            found.push(warning("edge-unreachable", &path, line, message));
        }
    }
    found
}
