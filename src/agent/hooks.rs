//! Claude Code's hooks: the user's commands that its `hooks` setting runs before and after the
//! agent's tools, given the call as JSON on their input, and able to block it, change its input,
//! ask the user or tell the model more.

use super::tools::{self, Call, Output};
use super::{AgentUi, Answer, mcp, subagent};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The events run, as Claude Code names them.
pub const EVENTS: [&str; 3] = ["PreToolUse", "PostToolUse", "PostToolUseFailure"];

/// Claude Code's time limit for a command hook.
const TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
}

impl Event {
    fn name(self) -> &'static str {
        EVENTS[self as usize]
    }
}

#[derive(Clone, Debug)]
enum Matcher {
    All,
    /// `Bash|Edit`: names compared whole.
    Names(Vec<String>),
    Pattern(regex::Regex),
}

impl Matcher {
    /// As Claude Code reads one: none, `""` or `*` matches every tool, names (letters, digits,
    /// `_` and `-`) between `|` or `,` are names, anything else a regular expression found
    /// anywhere in the name.
    fn read(matcher: &str) -> Result<Self, String> {
        let names = |c: char| c.is_ascii_alphanumeric() || "_|, -".contains(c);
        match matcher.trim() {
            "" | "*" => Ok(Self::All),
            names_only if names_only.chars().all(names) => Ok(Self::Names(
                (names_only.split(['|', ',']).map(str::trim))
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect(),
            )),
            _ => regex::Regex::new(matcher)
                .map(Self::Pattern)
                .map_err(|_| "is not a regular expression".to_owned()),
        }
    }
    fn matches(&self, names: &[String]) -> bool {
        match self {
            Self::All => true,
            Self::Names(given) => given.iter().any(|name| names.contains(name)),
            Self::Pattern(pattern) => names.iter().any(|name| pattern.is_match(name)),
        }
    }
}

#[derive(Clone, Debug)]
struct Group {
    event: Event,
    matcher: Matcher,
    commands: Vec<Command>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub command: String,
    pub timeout: Duration,
}

/// The settings' hooks, in the order the files and their lists give them.
#[derive(Clone, Debug, Default)]
pub struct Hooks {
    groups: Vec<Group>,
}

impl Hooks {
    /// Reads Claude Code's `hooks` setting from the file `path`, warning of what it leaves out.
    pub fn read(value: &Value, path: &str, warnings: &mut Vec<String>) -> Self {
        let mut hooks = Self::default();
        let events = match value {
            Value::Null => return hooks,
            Value::Object(events) => events,
            _ => {
                warnings.push(format!(
                    "{path}: hooks must be an object of events, each a list of matchers with \
                     their hooks; ignored"
                ));
                return hooks;
            }
        };
        for (name, groups) in events {
            let Some(event) = EVENTS.iter().position(|known| known == name) else {
                warnings.push(format!(
                    "{path}: hooks.{name} is not run: Scramjet runs {} hooks; ignored",
                    EVENTS.join(", ")
                ));
                continue;
            };
            let event = [
                Event::PreToolUse,
                Event::PostToolUse,
                Event::PostToolUseFailure,
            ][event];
            let Some(groups) = groups.as_array() else {
                warnings.push(format!(
                    "{path}: hooks.{name} must be a list of matchers with their hooks; ignored"
                ));
                continue;
            };
            for (at, group) in groups.iter().enumerate() {
                let place = format!("{path}: hooks.{name}[{at}]");
                if let Some(group) = read_group(event, group, &place, warnings) {
                    hooks.groups.push(group);
                }
            }
        }
        hooks
    }

    pub fn extend(&mut self, other: Self) {
        self.groups.extend(other.groups);
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The commands run for `event` on a tool with `names`, each once.
    fn matching(&self, event: Event, names: &[String]) -> Vec<&Command> {
        let mut found: Vec<&Command> = Vec::new();
        for group in self.groups.iter().filter(|group| group.event == event) {
            if group.matcher.matches(names) {
                for command in &group.commands {
                    if !found.iter().any(|seen| seen.command == command.command) {
                        found.push(command);
                    }
                }
            }
        }
        found
    }
}

fn read_group(
    event: Event,
    group: &Value,
    place: &str,
    warnings: &mut Vec<String>,
) -> Option<Group> {
    let Some(group) = group.as_object() else {
        warnings.push(format!(
            "{place} must be an object with \"matcher\" and \"hooks\"; ignored"
        ));
        return None;
    };
    let matcher = match group.get("matcher") {
        None | Some(Value::Null) => Matcher::All,
        Some(Value::String(matcher)) => match Matcher::read(matcher) {
            Ok(matcher) => matcher,
            Err(why) => {
                warnings.push(format!("{place}.matcher {why}; ignored"));
                return None;
            }
        },
        Some(_) => {
            warnings.push(format!(
                "{place}.matcher must be a tool's name or a pattern; ignored"
            ));
            return None;
        }
    };
    let Some(listed) = group.get("hooks").and_then(Value::as_array) else {
        warnings.push(format!("{place}.hooks must be a list of hooks; ignored"));
        return None;
    };
    let commands: Vec<Command> = (listed.iter().enumerate())
        .filter_map(|(at, hook)| read_command(hook, &format!("{place}.hooks[{at}]"), warnings))
        .collect();
    (!commands.is_empty()).then_some(Group {
        event,
        matcher,
        commands,
    })
}

fn read_command(hook: &Value, place: &str, warnings: &mut Vec<String>) -> Option<Command> {
    let hook: &Map<String, Value> = match hook.as_object() {
        Some(hook) => hook,
        None => {
            warnings.push(format!("{place} must be an object; ignored"));
            return None;
        }
    };
    match hook.get("type").and_then(Value::as_str) {
        Some("command") => {}
        Some(kind @ ("prompt" | "agent" | "http" | "mcp_tool")) => {
            warnings.push(format!(
                "{place}: Scramjet runs only \"command\" hooks, not \"{kind}\"; ignored"
            ));
            return None;
        }
        _ => {
            warnings.push(format!("{place}.type must be \"command\"; ignored"));
            return None;
        }
    }
    let Some(command) =
        (hook.get("command").and_then(Value::as_str)).filter(|command| !command.trim().is_empty())
    else {
        warnings.push(format!("{place}.command must be a command; ignored"));
        return None;
    };
    let timeout = match hook.get("timeout") {
        None | Some(Value::Null) => TIMEOUT,
        Some(seconds) => match seconds.as_f64().filter(|seconds| *seconds > 0.0) {
            Some(seconds) => Duration::from_secs_f64(seconds.min(86_400.0)),
            None => {
                warnings.push(format!("{place}.timeout must be seconds, above 0; ignored"));
                return None;
            }
        },
    };
    Some(Command {
        command: command.to_owned(),
        timeout,
    })
}

/// The hooks with what their input says of the session.
#[derive(Clone, Debug)]
pub(super) struct Bound {
    pub hooks: std::sync::Arc<Hooks>,
    pub session_id: String,
    pub transcript: PathBuf,
    /// Claude Code's `permission_mode`: `default` asking, `plan`, else `bypassPermissions`.
    pub mode: &'static str,
}

/// What `PreToolUse` hooks let a call do.
#[derive(Debug, PartialEq)]
pub(super) enum Before {
    /// Run, with these arguments in place of the model's if given; after asking the user if
    /// `ask` (with the hook's reason); `context` for the model after its result.
    Run {
        arguments: Option<Value>,
        ask: Option<String>,
        context: Vec<String>,
    },
    /// Not run: this is its result.
    Blocked(String),
}

/// What one hook's run says, as Claude Code reads its exit code and output.
#[derive(Default)]
struct Said {
    /// For the model: exit code 2's error output, a `deny` or `block` with its reason.
    blocking: Option<String>,
    decision: Option<&'static str>,
    reason: Option<String>,
    updated: Option<Value>,
    context: Option<String>,
}

impl Bound {
    /// Runs the `PreToolUse` hooks matching `call`, if any.
    pub fn before(
        &self,
        call: &Call,
        root: &Path,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Option<Before> {
        let names = names(call);
        let commands = self.hooks.matching(Event::PreToolUse, &names);
        if commands.is_empty() {
            return None;
        }
        let input = json!({"tool_name": names[0], "tool_input": tool_input(call, root)});
        let said = self.run(
            Event::PreToolUse,
            &names[0],
            &commands,
            input,
            root,
            ui,
            cancel,
        );
        let blocked: Vec<String> = said
            .iter()
            .filter_map(|said| said.blocking.clone())
            .collect();
        if !blocked.is_empty() {
            return Some(Before::Blocked(blocked.join("\n")));
        }
        let decided = |decision| said.iter().find(|said| said.decision == Some(decision));
        let ask = decided("ask").map(|said| said.reason.clone().unwrap_or_default());
        let arguments = (said.iter().find_map(|said| said.updated.clone()))
            .map(|updated| arguments(call, updated));
        let context = said.into_iter().filter_map(|said| said.context).collect();
        Some(Before::Run {
            arguments,
            ask,
            context,
        })
    }

    /// Runs the `PostToolUse` hooks matching `call`, or `PostToolUseFailure` for a failed
    /// `output`, adding what they tell the model, and `context` the `PreToolUse` hooks gave, to
    /// its text.
    pub fn after(
        &self,
        call: &Call,
        output: &mut Output,
        context: Vec<String>,
        root: &Path,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) {
        let names = names(call);
        let label = |event: Event| format!("{}:{}", event.name(), names[0]);
        let mut told: Vec<String> = (context.into_iter())
            .map(|context| additional(&label(Event::PreToolUse), &context))
            .collect();
        let event = match output.failed {
            true => Event::PostToolUseFailure,
            false => Event::PostToolUse,
        };
        let commands = self.hooks.matching(event, &names);
        if !commands.is_empty() && !cancel.load(Ordering::Relaxed) {
            let mut input = json!({"tool_name": names[0], "tool_input": tool_input(call, root)});
            match event {
                Event::PostToolUseFailure => {
                    input["error"] = json!(output.text);
                    input["is_interrupted"] = json!(false);
                }
                _ => input["tool_response"] = tool_response(call, output, root),
            }
            for said in self.run(event, &names[0], &commands, input, root, ui, cancel) {
                told.extend(said.blocking);
                told.extend(
                    said.context
                        .map(|context| additional(&label(event), &context)),
                );
            }
        }
        if !told.is_empty() {
            output.text = format!("{}\n\n{}", output.text.trim_end(), told.join("\n"));
        }
    }

    /// Runs `commands` at once, each given `input` with the session's fields; what each said.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        event: Event,
        tool: &str,
        commands: &[&Command],
        mut input: Value,
        root: &Path,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Vec<Said> {
        input["session_id"] = json!(self.session_id);
        input["transcript_path"] = json!(self.transcript.to_string_lossy());
        input["cwd"] = json!(root.to_string_lossy());
        input["permission_mode"] = json!(self.mode);
        input["hook_event_name"] = json!(event.name());
        let input = input.to_string().into_bytes();
        let env = [("CLAUDE_PROJECT_DIR", root.as_os_str())];
        let ran: Vec<_> = std::thread::scope(|scope| {
            let running: Vec<_> = (commands.iter())
                .map(|command| {
                    let input = &input;
                    scope.spawn(move || {
                        let ran = tools::run_piped(
                            root,
                            &command.command,
                            input,
                            &env,
                            command.timeout,
                            cancel,
                        );
                        (command.command.as_str(), ran)
                    })
                })
                .collect();
            (running.into_iter())
                .filter_map(|running| running.join().ok())
                .collect()
        });
        let label = format!("{}:{tool}", event.name());
        (ran.into_iter())
            .map(|(command, ran)| said(event, &label, command, ran, ui))
            .collect()
    }
}

fn additional(label: &str, context: &str) -> String {
    format!("{label} hook additional context: {}", context.trim())
}

fn blocking(label: &str, command: &str, why: &str) -> String {
    format!(
        "{label} hook blocking error from command: \"{command}\": {}",
        why.trim()
    )
}

/// What a hook's exit says: 0 with JSON read as Claude Code reads it, 2 blocking with its error
/// output, anything else an error the user is told of, the call going on.
fn said(
    event: Event,
    label: &str,
    command: &str,
    ran: Result<tools::Exited, String>,
    ui: &mut dyn AgentUi,
) -> Said {
    let mut said = Said::default();
    let exited = match ran {
        Ok(exited) => exited,
        Err(why) if why == "cancelled" => return said,
        Err(why) => {
            ui.notice(&format!("{label} hook error: {command}: {why}"));
            return said;
        }
    };
    match exited.code {
        Some(0) => {}
        Some(2) => {
            let why = match exited.stderr.trim() {
                "" => "No stderr output",
                stderr => stderr,
            };
            said.blocking = Some(blocking(label, command, why));
            return said;
        }
        code => {
            let code = code.map_or("a signal".to_owned(), |code| code.to_string());
            let stderr = match exited.stderr.trim() {
                "" => "No stderr output",
                stderr => stderr,
            };
            ui.notice(&format!(
                "{label} hook error: Failed with non-blocking status code {code}: {stderr}"
            ));
            return said;
        }
    }
    let Some(answer) = (exited.stdout.trim_start().starts_with('{'))
        .then(|| serde_json::from_str::<Map<String, Value>>(exited.stdout.trim()).ok())
        .flatten()
    else {
        return said;
    };
    if let Some(message) = answer.get("systemMessage").and_then(Value::as_str) {
        ui.notice(message);
    }
    let specific = answer.get("hookSpecificOutput").and_then(Value::as_object);
    let field = |name: &str| {
        (specific.and_then(|specific| specific.get(name)))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    said.context = field("additionalContext").filter(|context| !context.trim().is_empty());
    let reason = answer.get("reason").and_then(Value::as_str);
    match event {
        Event::PreToolUse => {
            said.decision = match (
                field("permissionDecision").as_deref(),
                answer.get("decision"),
            ) {
                (Some("allow"), _) => Some("allow"),
                (Some("deny"), _) => Some("deny"),
                (Some("ask"), _) => Some("ask"),
                (None, Some(Value::String(legacy))) if legacy == "approve" => Some("allow"),
                (None, Some(Value::String(legacy))) if legacy == "block" => Some("deny"),
                _ => None,
            };
            said.reason = field("permissionDecisionReason").or(reason.map(str::to_owned));
            said.updated = (specific.and_then(|specific| specific.get("updatedInput")))
                .filter(|updated| updated.is_object())
                .cloned();
            if said.decision == Some("deny") {
                let why = said.reason.as_deref().unwrap_or("Blocked by hook");
                said.blocking = Some(blocking(label, command, why));
            }
        }
        Event::PostToolUse | Event::PostToolUseFailure => {
            if answer.get("decision").and_then(Value::as_str) == Some("block") {
                let why = reason.unwrap_or("Blocked by hook");
                said.blocking = Some(blocking(label, command, why));
            }
        }
    }
    said
}

/// The names a matcher may give `call`'s tool: Claude Code's first, as the hook's `tool_name`,
/// then Scramjet's.
fn names(call: &Call) -> Vec<String> {
    let claude = match call.name {
        "bash" => "Bash",
        "powershell" => "PowerShell",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "grep" => "Grep",
        "find" => "Glob",
        "ls" => "LS",
        subagent::NAME => "Agent",
        mcp::CALL => {
            let part = |field: &str| call.arguments[field].as_str().unwrap_or_default();
            let name = format!("mcp__{}__{}", part("server"), part("tool"));
            return vec![name, call.name.to_owned()];
        }
        name => name,
    };
    let mut names = vec![claude.to_owned()];
    if call.name == subagent::NAME {
        names.push("Task".to_owned());
    }
    if claude != call.name {
        names.push(call.name.to_owned());
    }
    names
}

fn is_file_tool(name: &str) -> bool {
    matches!(name, "read" | "write" | "edit")
}

/// The call's arguments as a hook is given them: an MCP tool's own, a file tool's with Claude
/// Code's absolute `file_path` beside its `path`.
fn tool_input(call: &Call, root: &Path) -> Value {
    let mut input = match call.name {
        mcp::CALL => call.arguments["arguments"].clone(),
        _ => call.arguments.clone(),
    };
    if is_file_tool(call.name)
        && let Some(path) = call.arguments["path"].as_str()
    {
        input["file_path"] = json!(tools::resolve(root, path).to_string_lossy());
    }
    if !input.is_object() {
        input = json!({});
    }
    input
}

/// The call's arguments from a hook's `updatedInput`, read back as `tool_input` gave them.
fn arguments(call: &Call, mut updated: Value) -> Value {
    if is_file_tool(call.name)
        && let Some(path) = updated
            .as_object_mut()
            .and_then(|input| input.remove("file_path"))
    {
        updated["path"] = path;
    }
    match call.name {
        mcp::CALL => {
            let mut arguments = call.arguments.clone();
            arguments["arguments"] = updated;
            arguments
        }
        _ => updated,
    }
}

/// What a `PostToolUse` hook is given of the result: its text, and Claude Code's `filePath` for
/// a file tool.
fn tool_response(call: &Call, output: &Output, root: &Path) -> Value {
    let mut response = json!({"output": output.text, "success": !output.failed});
    if is_file_tool(call.name)
        && let Some(path) = call.arguments["path"].as_str()
    {
        response["filePath"] = json!(tools::resolve(root, path).to_string_lossy());
    }
    response
}

/// Asks the user whether `call` may run, as a `PreToolUse` hook's `ask` wants, with its reason.
pub(super) fn ask(
    call: &Call,
    reason: &str,
    root: &Path,
    ui: &mut dyn AgentUi,
    cancel: &AtomicBool,
) -> bool {
    if !reason.trim().is_empty() {
        ui.question(reason);
    }
    let question = match call.approval(root) {
        tools::Approval::Ask(question) => question,
        tools::Approval::None => format!("Run {} (a PreToolUse hook asks)", call.name),
    };
    ui.approve(&question, cancel) != Answer::Deny
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(value: Value) -> (Hooks, Vec<String>) {
        let mut warnings = Vec::new();
        (Hooks::read(&value, "s.json", &mut warnings), warnings)
    }

    fn call(name: &str, arguments: Value) -> Call<'_> {
        Call { name, arguments }
    }

    #[test]
    fn claude_codes_hooks_are_read_and_what_scramjet_does_not_run_is_named() {
        let (hooks, warnings) = read(json!({
            "PreToolUse": [
                {"matcher": "Bash", "hooks": [
                    {"type": "command", "command": "a", "timeout": 5},
                    {"type": "prompt", "prompt": "Safe?"},
                    {"type": "command", "command": "a"},
                ]},
                {"matcher": "(", "hooks": [{"type": "command", "command": "b"}]},
                {"hooks": [{"type": "command", "command": "c", "timeout": 0}]},
            ],
            "Stop": [{"hooks": [{"type": "command", "command": "d"}]}],
            "PostToolUse": {"matcher": "Write"},
        }));
        assert_eq!(
            warnings,
            [
                "s.json: hooks.PostToolUse must be a list of matchers with their hooks; ignored",
                "s.json: hooks.PreToolUse[0].hooks[1]: Scramjet runs only \"command\" hooks, \
                 not \"prompt\"; ignored",
                "s.json: hooks.PreToolUse[1].matcher is not a regular expression; ignored",
                "s.json: hooks.PreToolUse[2].hooks[0].timeout must be seconds, above 0; ignored",
                "s.json: hooks.Stop is not run: Scramjet runs PreToolUse, PostToolUse, \
                 PostToolUseFailure hooks; ignored",
            ]
        );
        let bash = names(&call("bash", json!({})));
        let run = hooks.matching(Event::PreToolUse, &bash);
        assert_eq!(
            run,
            [&Command {
                command: "a".into(),
                timeout: Duration::from_secs(5)
            }]
        );
        assert!(hooks.matching(Event::PostToolUse, &bash).is_empty());
        let (_, warnings) = read(json!(["no setting's type"]));
        assert_eq!(
            warnings,
            [
                "s.json: hooks must be an object of events, each a list of matchers with their \
              hooks; ignored"
            ]
        );
    }

    #[test]
    fn matchers_take_claude_codes_names_scramjets_and_patterns() {
        let matches =
            |matcher: &str, tool: &Call| Matcher::read(matcher).unwrap().matches(&names(tool));
        let find = call("find", json!({"pattern": "*.rs"}));
        let agent = call(subagent::NAME, json!({}));
        let mcp = call(mcp::CALL, json!({"server": "tavily", "tool": "search"}));
        for (matcher, tool, expected) in [
            ("", &find, true),
            ("*", &find, true),
            ("Glob", &find, true),
            ("find", &find, true),
            ("Read | Glob", &find, true),
            ("Grep", &find, false),
            ("Gl", &find, false),
            ("^Gl", &find, true),
            ("Task", &agent, true),
            ("Agent", &agent, true),
            ("mcp__tavily__.*", &mcp, true),
            ("mcp__tavily__search", &mcp, true),
            ("mcp__other__.*", &mcp, false),
        ] {
            assert_eq!(matches(matcher, tool), expected, "{matcher} {}", tool.name);
        }
    }

    #[test]
    fn file_tools_are_given_claude_codes_file_path_and_take_it_back() {
        let root = Path::new("/project");
        let edit = call("edit", json!({"path": "src/a.rs", "edits": []}));
        let input = tool_input(&edit, root);
        assert_eq!(input["file_path"], "/project/src/a.rs");
        assert_eq!(input["path"], "src/a.rs");
        let back = arguments(
            &edit,
            json!({"file_path": "/project/b.rs", "path": "src/a.rs", "edits": []}),
        );
        assert_eq!(back, json!({"path": "/project/b.rs", "edits": []}));
        let mcp = call(
            mcp::CALL,
            json!({"server": "s", "tool": "t", "arguments": {"q": 1}}),
        );
        assert_eq!(tool_input(&mcp, root), json!({"q": 1}));
        assert_eq!(
            arguments(&mcp, json!({"q": 2})),
            json!({"server": "s", "tool": "t", "arguments": {"q": 2}})
        );
    }
}
