//! Claude Code's hooks: the user's commands that its `hooks` setting runs before and after the
//! agent's tools, given the call as JSON on their input, and able to block it, change its input,
//! ask the user or tell the model more; and as a prompt is sent, as a turn would end and as a
//! session starts.

use super::tools::{self, Call, Output};
use super::{Agent, AgentError, AgentUi, Answer, PROBE, mcp, message_text, subagent};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The events run, as Claude Code names them.
pub const EVENTS: [&str; 6] = [
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "UserPromptSubmit",
    "Stop",
    "SessionStart",
];

/// How many times in a row `Stop` hooks may keep a turn going, as Claude Code's
/// `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP` by default.
const STOP_BLOCKS: usize = 8;

/// Claude Code's time limit for a command hook.
const TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    UserPromptSubmit,
    Stop,
    /// Its matcher names the source: `startup`, `resume`, `clear` or `compact`.
    SessionStart,
}

impl Event {
    const ALL: [Self; 6] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::UserPromptSubmit,
        Self::Stop,
        Self::SessionStart,
    ];

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
            let event = Event::ALL[event];
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

    /// The commands run for `event` on a tool, or a session's source, with `names`, each once;
    /// `None`: an event whose matcher is not read, as Claude Code reads none for
    /// `UserPromptSubmit` and `Stop`.
    fn matching(&self, event: Event, names: Option<&[String]>) -> Vec<&Command> {
        let mut found: Vec<&Command> = Vec::new();
        for group in self.groups.iter().filter(|group| group.event == event) {
            if names.is_none_or(|names| group.matcher.matches(names)) {
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
    /// That error output or reason alone, for the user.
    why: Option<String>,
    decision: Option<&'static str>,
    reason: Option<String>,
    updated: Option<Value>,
    context: Option<String>,
    /// What it printed, when not JSON: for the model after `UserPromptSubmit` and
    /// `SessionStart`.
    plain: Option<String>,
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
        let commands = self.hooks.matching(Event::PreToolUse, Some(&names));
        if commands.is_empty() {
            return None;
        }
        let input = json!({"tool_name": names[0], "tool_input": tool_input(call, root)});
        let said = self.run(
            Event::PreToolUse,
            &format!("PreToolUse:{}", names[0]),
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
        let commands = self.hooks.matching(event, Some(&names));
        if !commands.is_empty() && !cancel.load(Ordering::Relaxed) {
            let mut input = json!({"tool_name": names[0], "tool_input": tool_input(call, root)});
            match event {
                Event::PostToolUseFailure => {
                    input["error"] = json!(output.text);
                    input["is_interrupted"] = json!(false);
                }
                _ => input["tool_response"] = tool_response(call, output, root),
            }
            for said in self.run(event, &label(event), &commands, input, root, ui, cancel) {
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

    /// Runs the `UserPromptSubmit` hooks on `prompt`, as typed: what they tell the model, or
    /// `Err` with why, for the user, the prompt is not to be sent.
    pub fn prompt_submitted(
        &self,
        prompt: &str,
        root: &Path,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Vec<String>, String> {
        let event = Event::UserPromptSubmit;
        let commands = self.hooks.matching(event, None);
        if commands.is_empty() {
            return Ok(Vec::new());
        }
        let input = json!({"prompt": prompt});
        let said = self.run(event, event.name(), &commands, input, root, ui, cancel);
        let blocked: Vec<String> = said.iter().filter_map(|said| said.why.clone()).collect();
        match blocked.is_empty() {
            true => Ok(told(event.name(), said)),
            false => Err(blocked.join("\n")),
        }
    }

    /// Runs the `Stop` hooks as a turn would end after `last`, the model's last text;
    /// `active` when a `Stop` hook kept this turn going before. If any hook blocks its end, what
    /// has the turn go on, for the model and for the user.
    pub fn stopping(
        &self,
        active: bool,
        last: Option<String>,
        root: &Path,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Option<(String, String)> {
        let event = Event::Stop;
        let commands = self.hooks.matching(event, None);
        if commands.is_empty() {
            return None;
        }
        let mut input = json!({"stop_hook_active": active});
        if let Some(last) = last {
            input["last_assistant_message"] = json!(last);
        }
        let said = self.run(event, event.name(), &commands, input, root, ui, cancel);
        let (blocked, why): (Vec<_>, Vec<_>) = (said.into_iter())
            .filter_map(|said| said.blocking.zip(said.why))
            .unzip();
        (!blocked.is_empty()).then(|| (blocked.join("\n"), why.join("\n")))
    }

    /// Runs the `SessionStart` hooks matching `source` for a session on `model`: what they
    /// tell the model.
    pub fn session_started(
        &self,
        source: &str,
        model: &str,
        root: &Path,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Vec<String> {
        let event = Event::SessionStart;
        let commands = self.hooks.matching(event, Some(&[source.to_owned()]));
        if commands.is_empty() {
            return Vec::new();
        }
        let input = json!({"source": source, "model": model});
        let label = format!("{}:{source}", event.name());
        let said = self.run(event, &label, &commands, input, root, ui, cancel);
        told(&label, said)
    }

    /// Runs `commands` at once, each given `input` with the session's fields; what each said.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        event: Event,
        label: &str,
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
        (ran.into_iter())
            .map(|(command, ran)| said(event, label, command, ran, ui))
            .collect()
    }
}

/// What hooks that ran as a prompt was sent or a session started tell the model: what they
/// printed, and their `additionalContext`.
fn told(label: &str, said: Vec<Said>) -> Vec<String> {
    let mut told = Vec::new();
    for said in said {
        told.extend(
            said.plain
                .map(|plain| format!("{label} hook success: {plain}")),
        );
        told.extend(said.context.map(|context| additional(label, &context)));
    }
    told
}

impl Agent {
    /// What `UserPromptSubmit` hooks tell the model of `prompt`; `None`, the user told why, when
    /// one blocks it, so it is not sent.
    pub(super) fn prompt_hooks(
        &self,
        prompt: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Option<Vec<String>> {
        let Some(hooks) = self.bound_hooks() else {
            return Some(Vec::new());
        };
        match hooks.prompt_submitted(prompt, &self.config.root, ui, cancel) {
            Ok(told) => Some(told),
            Err(why) => {
                ui.notice(&format!(
                    "A UserPromptSubmit hook blocked the prompt, which was not sent: {why}"
                ));
                None
            }
        }
    }

    /// Adds what hooks `told` the model, as a message the user never typed.
    pub(super) fn add_hook_context(&mut self, told: Vec<String>) -> Result<(), AgentError> {
        if told.is_empty() {
            return Ok(());
        }
        let note = format!("<system-reminder>\n{}\n</system-reminder>", told.join("\n"));
        self.add(json!({"role": "user", "content": note, PROBE: true}))
    }

    /// Runs the `SessionStart` hooks for the start, resume, clear or compaction since the last
    /// request, adding what they tell the model.
    pub(super) fn session_start_hooks(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let (Some(source), Some(hooks)) = (self.session_start.take(), self.bound_hooks()) else {
            return Ok(());
        };
        let told = hooks.session_started(source, self.model_id(), &self.config.root, ui, cancel);
        self.add_hook_context(told)
    }

    /// Where the turn would end: whether `Stop` hooks have it go on, with what they said added
    /// for the model. Past `STOP_BLOCKS` times in a turn it ends anyway, the user told so.
    pub(super) fn stop_hooks_go_on(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<bool, AgentError> {
        let Some(hooks) = self.bound_hooks() else {
            return Ok(false);
        };
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let last = (self.conversation.iter().rev())
            .find(|item| item["type"] == "message" && item["role"] == "assistant")
            .map(message_text);
        let active = self.stop_blocks > 0;
        let Some((why, shown)) = hooks.stopping(active, last, &self.config.root, ui, cancel) else {
            return Ok(false);
        };
        self.stop_blocks += 1;
        if self.stop_blocks > STOP_BLOCKS {
            ui.notice(&format!(
                "A Stop hook blocked the turn from ending {} times in a row; it ends now. A \
                 Stop hook can read stop_hook_active in its input and let the turn end while it \
                 is true.",
                self.stop_blocks
            ));
            return Ok(false);
        }
        ui.notice(&format!("Stop hook feedback: {shown}"));
        self.add(json!({"role": "user", "content": why, PROBE: true}))?;
        Ok(true)
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
            // As Claude Code's, a session's start cannot be blocked: the user is told.
            match event {
                Event::SessionStart => ui.notice(&format!("{label} hook error: {why}")),
                _ => {
                    said.blocking = Some(blocking(label, command, why));
                    said.why = Some(why.to_owned());
                }
            }
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
        said.plain = Some(exited.stdout.trim().to_owned()).filter(|plain| !plain.is_empty());
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
                said.why = Some(why.to_owned());
            }
        }
        Event::PostToolUse | Event::PostToolUseFailure | Event::UserPromptSubmit | Event::Stop => {
            if answer.get("decision").and_then(Value::as_str) == Some("block") {
                let why = reason.unwrap_or("Blocked by hook");
                said.blocking = Some(blocking(label, command, why));
                said.why = Some(why.to_owned());
            }
        }
        Event::SessionStart => {}
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
            "Notification": [{"hooks": [{"type": "command", "command": "d"}]}],
            "PostToolUse": {"matcher": "Write"},
        }));
        assert_eq!(
            warnings,
            [
                "s.json: hooks.Notification is not run: Scramjet runs PreToolUse, PostToolUse, \
                 PostToolUseFailure, UserPromptSubmit, Stop, SessionStart hooks; ignored",
                "s.json: hooks.PostToolUse must be a list of matchers with their hooks; ignored",
                "s.json: hooks.PreToolUse[0].hooks[1]: Scramjet runs only \"command\" hooks, \
                 not \"prompt\"; ignored",
                "s.json: hooks.PreToolUse[1].matcher is not a regular expression; ignored",
                "s.json: hooks.PreToolUse[2].hooks[0].timeout must be seconds, above 0; ignored",
            ]
        );
        let bash = names(&call("bash", json!({})));
        let run = hooks.matching(Event::PreToolUse, Some(&bash));
        assert_eq!(
            run,
            [&Command {
                command: "a".into(),
                timeout: Duration::from_secs(5)
            }]
        );
        assert!(hooks.matching(Event::PostToolUse, Some(&bash)).is_empty());
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
    fn prompts_turns_ends_and_session_starts_run_claude_codes_hooks() {
        use crate::agent::tests::{Recorder, agent, missing, serve, stream};
        use crate::provider_fixture as fixture;
        if missing("bash") {
            return;
        }
        let (endpoint, server) = serve(
            ["done", "tested", "fine"]
                .map(|text| stream(&fixture::text_events(text, fixture::known_usage())))
                .to_vec(),
        );
        let (mut agent, _scratch) = agent(endpoint, "hook-events");
        let root = agent.root().to_owned();
        let prompt = r#"input=$(cat); echo "$input" >> prompts.jsonl; case $input in *secret*) echo 'no secrets' >&2; exit 2;; esac; echo '{"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": "the date is today"}}'"#;
        let stop = r#"input=$(cat); echo "$input" >> stops.jsonl; case $input in *'"last_assistant_message":"done"'*) echo 'run the tests first' >&2; exit 2;; esac"#;
        let settings = json!({
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": prompt}]}],
            "Stop": [{"matcher": "never read", "hooks": [{"type": "command", "command": stop}]}],
            "SessionStart": [
                {"matcher": "startup", "hooks": [{"type": "command", "command": "cat > start.json; echo from the start"}]},
                {"matcher": "clear|compact", "hooks": [{"type": "command", "command": "echo cleared"}]},
            ],
        });
        let (hooks, warnings) = read(settings);
        assert!(warnings.is_empty(), "{warnings:?}");
        agent.set_hooks(std::sync::Arc::new(hooks));
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("a secret plan", &mut ui, &cancel).unwrap();
        assert!(agent.conversation().is_empty());
        let blocked = "notice A UserPromptSubmit hook blocked the prompt, which was not sent: no \
                       secrets";
        assert!(ui.shown.contains(&blocked.to_owned()), "{:?}", ui.shown);
        agent.turn("go", &mut ui, &cancel).unwrap();
        agent.new_session().unwrap();
        agent.turn("again", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        let users = |at: usize| -> Vec<String> {
            (requests[at]["input"].as_array().unwrap().iter())
                .filter(|item| item["role"] == "user")
                .map(|item| item["content"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(
            users(0),
            [
                "go",
                "<system-reminder>\nUserPromptSubmit hook additional context: the date is today\n\
                 </system-reminder>",
                "<system-reminder>\nSessionStart:startup hook success: from the start\n\
                 </system-reminder>",
            ]
        );
        assert_eq!(
            users(1).last().unwrap(),
            &format!("Stop hook blocking error from command: \"{stop}\": run the tests first")
        );
        let feedback = "notice Stop hook feedback: run the tests first".to_owned();
        assert!(ui.shown.contains(&feedback), "{:?}", ui.shown);
        let again = users(2);
        assert_eq!(again[0], "again");
        assert!(
            again[2].contains("SessionStart:clear hook success: cleared"),
            "{again:?}"
        );
        let lines = |file: &str| -> Vec<Value> {
            (std::fs::read_to_string(root.join(file)).unwrap().lines())
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        };
        let prompts = lines("prompts.jsonl");
        assert_eq!(prompts.len(), 3);
        assert_eq!(prompts[1]["prompt"], "go");
        assert_eq!(prompts[1]["hook_event_name"], "UserPromptSubmit");
        let stops = lines("stops.jsonl");
        let stopped: Vec<_> = (stops.iter())
            .map(|stop| {
                (
                    stop["stop_hook_active"].clone(),
                    stop["last_assistant_message"].clone(),
                )
            })
            .collect();
        assert_eq!(
            stopped,
            [
                (json!(false), json!("done")),
                (json!(true), json!("tested")),
                (json!(false), json!("fine")),
            ]
        );
        let start: Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("start.json")).unwrap())
                .unwrap();
        assert_eq!(start["hook_event_name"], "SessionStart");
        assert_eq!(start["source"], "startup");
        assert_eq!(start["model"], agent.model_id());
    }

    #[test]
    fn a_stop_hook_that_always_blocks_lets_the_turn_end_after_eight_times() {
        use crate::agent::tests::{Recorder, agent, missing, serve, stream};
        use crate::provider_fixture as fixture;
        if missing("bash") {
            return;
        }
        let replies = (0..=STOP_BLOCKS)
            .map(|_| stream(&fixture::text_events("idle", fixture::known_usage())))
            .collect();
        let (endpoint, server) = serve(replies);
        let (mut agent, _scratch) = agent(endpoint, "hook-stops");
        let stop = "echo '{\"decision\": \"block\", \"reason\": \"keep going\"}'";
        let (hooks, _) = read(json!({"Stop": [{"hooks": [{"type": "command", "command": stop}]}]}));
        agent.set_hooks(std::sync::Arc::new(hooks));
        let mut ui = Recorder::default();
        agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
        assert_eq!(server.join().unwrap().len(), STOP_BLOCKS + 1);
        assert!(
            (ui.shown.iter()).any(|shown| shown.starts_with(
                "notice A Stop hook blocked the turn from ending 9 times in a row; it ends now."
            )),
            "{:?}",
            ui.shown
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
