//! Scramjet's command lifecycle: a command a prompt runs stays active until the model reports
//! it with `report_scramjet_command_status`; a turn that ends without a report gets Scramjet's
//! status check; a completed command offers its next steps, which its `next:` policy and the
//! `autonomy.yaml` edges route. As Scramjet's `lifecycle.ts`, `command-status.ts`,
//! `next-step.ts`, `commands/validator.ts` and `auto-continue.ts` do.

use super::autonomy::{Edge, Edges};
use super::commands::{Candidate, Commands, Policy};
use serde_json::{Value, json};

pub const NAME: &str = "report_scramjet_command_status";

/// Scramjet's harness-only tool, never offered to the model: a call of it and its result record
/// a next-step choice in the conversation, which the model sees.
pub(super) const RECORD: &str = "scramjet_next_step_selection";

/// Scramjet's `promptSnippet`, listed under the available tools.
pub(super) const SNIPPET: &str = "You have access to `report_scramjet_command_status` for \
reporting the status of an active Scramjet slash command. Summarize the work you performed first, \
then give the status — the summary is your evidence, the status is your assessment of it. Once the \
command's work is done and your final user-facing answer has been delivered, you may report a \
terminal status (`completed`, `blocked`, or `incomplete`) inline — always deliver the complete \
answer first, because reporting ends the turn. If you do not report inline, Scramjet sends a \
status-check message as a fallback; respond to it with this tool. Do not call this tool for \
ordinary user tasks.";

/// How often the model may say `continuing` to status checks before it must report.
const CONTINUE_LIMIT: u32 = 3;

const NO_ACTIVE_COMMAND_ERROR: &str = "report_scramjet_command_status is not active right now — \
no Scramjet command is running. Use it only during an active Scramjet slash command: report a \
terminal status inline once the command's work is done and the final answer is delivered, or in \
response to Scramjet's status-check message. Do not call this tool for ordinary tasks.";
const PARKED_ERROR: &str = "report_scramjet_command_status cannot accept a report right now: the \
command is parked waiting for user input. Wait for the user's reply before reporting.";
const ALREADY_REPORTED_ERROR: &str = "report_scramjet_command_status cannot accept a report right \
now: a status report was already filed for this command. Do not call this tool again.";
const CONTINUING_OUT_OF_PHASE_ERROR: &str = "report_scramjet_command_status cannot accept \
\"continuing\" right now — it is only valid in response to Scramjet's status-check message or \
while the command is dormant. Keep working, or report a terminal status (completed, blocked, \
incomplete) once the work is done.";
const CONTINUING: &str = "Continuing. Proceed with your work.";

pub(super) fn definition() -> Value {
    let status = |value: &str| json!({"const": value, "type": "string"});
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Report the status of an active Scramjet slash command. Summarize the work \
performed first, then give the status. Terminal statuses (completed, blocked, incomplete) may be \
reported inline once the command's work is done and the final user-facing answer has been \
delivered, or in response to Scramjet's status-check message. Do not call this tool for ordinary \
user tasks.",
        "parameters":{"type":"object","properties":{
            "summary":{"type":"string","minLength":1,"pattern":"\\S",
                "description":"A summary of the work you completed. On your first report, \
summarize the work done so far; on each later report, summarize only the work completed since \
your previous report."},
            "status":{"anyOf":[status("completed"),status("blocked"),status("incomplete"),
                    status("continuing")],
                "description":"completed = the command's work is done and your final \
user-facing answer was already delivered; continuing = you have more work to do right now (not \
blocked, not waiting for input, not finished); blocked = the command cannot proceed (error, \
missing dependency, authorization); incomplete = none of the above (stopped without a clean \
completion/question/blocker)."},
            "next_steps":{"type":"array",
                "description":"Ordered next-step candidates for completed commands. Omit \
entirely to stop the chain. Each entry is a suggested next message; entries may reuse the same \
slash command with different arguments to offer meaningful variants.",
                "items":{"type":"object","properties":{
                    "message":{"type":"string","description":"The suggested next message, shown \
to the user verbatim and dispatched on selection. For a slash command, start with '/' and include \
any arguments, e.g. '/mach12:issue-plan 55'. For a non-command follow-up, write the message text \
directly."},
                    "fresh_session":{"type":"boolean","description":"Whether to start a fresh \
session first (true if instructions say '/clear then ...' or 'in a fresh session'). Only \
meaningful for slash commands; defaults to false."},
                    "reason":{"type":"string","description":"Brief explanation of why this next \
step fits."}},
                  "required":["message","fresh_session"]}},
            "recommended_next_step":{"type":"integer","minimum":0,
                "description":"Zero-based index into next_steps for the recommended selector \
option."}},
          "required":["summary","status"]}})
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Status {
    Completed,
    Blocked,
    Incomplete,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Blocked => "blocked",
            Self::Incomplete => "incomplete",
        }
    }
}

/// A next step the model offers: a message to send, a slash command or text.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Offered {
    message: String,
    fresh_session: bool,
    reason: Option<String>,
}

/// A terminal report.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Report {
    status: Status,
    summary: String,
    next_steps: Vec<Offered>,
    recommended: Option<usize>,
}

/// Where an active command is, as Scramjet's lifecycle modes.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Working; the status check is due when the turn ends.
    Running {
        continues: u32,
    },
    /// The status check was sent.
    Probing {
        continues: u32,
    },
    /// Waiting for the user's reply to a freetext question.
    Parked,
    /// Started but not running: nothing is checked until the model reports. `resumable`: a
    /// question was cancelled, so the user's next reply resumes it.
    Dormant {
        resumable: bool,
    },
    Reported(Report),
}

/// The active command and where it is.
#[derive(Debug, Default)]
pub(super) struct Lifecycle {
    active: Option<(String, Phase)>,
}

/// What follows a turn.
#[derive(Debug, PartialEq)]
pub(super) enum Ended {
    Nothing,
    /// Send this status check and run another turn.
    Probe(String),
    /// The command completed; its next steps are routed.
    Completed(Completed),
    /// Something to tell the user.
    Notice(String),
}

/// A command reported completed.
#[derive(Clone, Debug, PartialEq)]
pub struct Completed {
    command: String,
    report: Report,
}

impl Lifecycle {
    /// The session line that keeps the active command for a resume, as Scramjet's journal keeps
    /// what it rebuilds its lifecycle from: the command (`null`: none) and, when the user's next
    /// reply resumes it, `state`: `parked` (a freetext question waits for the reply) or
    /// `resumable` (a question was cancelled). A completion is saved when its turn ends, as
    /// Scramjet records a report then.
    pub fn saved(&self) -> Value {
        let Some((name, phase)) = &self.active else {
            return json!({"type":"command_state","command":null});
        };
        let mut line = json!({"type":"command_state","command":name});
        match phase {
            Phase::Parked => line["state"] = json!("parked"),
            Phase::Dormant { resumable: true } => line["state"] = json!("resumable"),
            _ => {}
        }
        line
    }

    /// The lifecycle a session's `saved` line leaves: as Scramjet rebuilds it, a command that
    /// was working is dormant.
    pub fn restored(saved: Option<&Value>) -> Self {
        let active = saved.and_then(|line| {
            let name = line["command"].as_str().filter(|name| !name.is_empty())?;
            let phase = match line["state"].as_str() {
                Some("parked") => Phase::Parked,
                Some("resumable") => Phase::Dormant { resumable: true },
                _ => Phase::Dormant { resumable: false },
            };
            Some((name.to_owned(), phase))
        });
        Self { active }
    }

    /// The active command's name, whatever its phase.
    pub fn active(&self) -> Option<&str> {
        self.active.as_ref().map(|(name, _)| name.as_str())
    }

    /// Before a prompt is sent: a prompt running `command` starts it; as in Scramjet, a reply
    /// resumes a command parked for it or whose question was cancelled, and a `/` prompt that
    /// names no command ends the active one.
    pub fn prompted(&mut self, prompt: &str, command: Option<&str>) {
        if let Some(name) = command {
            self.active = Some((name.to_owned(), Phase::Running { continues: 0 }));
            return;
        }
        if prompt.starts_with('/') {
            self.active = None;
            return;
        }
        if let Some((_, phase)) = &mut self.active
            && matches!(phase, Phase::Parked | Phase::Dormant { resumable: true })
        {
            *phase = Phase::Running { continues: 0 };
        }
    }

    /// A freetext question parks the active command for the user's reply; whether one did.
    pub fn park(&mut self) -> bool {
        match &mut self.active {
            Some((_, phase)) => {
                *phase = Phase::Parked;
                true
            }
            None => false,
        }
    }

    /// A cancelled question leaves the active command for the user's next reply to resume.
    pub fn question_cancelled(&mut self) {
        if let Some((_, phase)) = &mut self.active
            && !matches!(phase, Phase::Reported(_))
        {
            *phase = Phase::Dormant { resumable: true };
        }
    }

    /// The turn was cancelled: the command waits, and a report it made is dropped.
    pub fn cancelled(&mut self) -> Option<String> {
        let (name, phase) = self.active.as_mut()?;
        let notice = match phase {
            Phase::Reported(report) => Some(format!(
                "scramjet: aborted — discarding \"{}\" status report for {name}",
                report.status.name()
            )),
            _ => None,
        };
        *phase = Phase::Dormant { resumable: false };
        notice
    }

    /// Runs a `report_scramjet_command_status` call: what the model receives, whether it
    /// failed, and whether the turn ends with it.
    pub fn report(&mut self, arguments: &Value) -> (String, bool, bool) {
        let (summary, status) = match validate(arguments) {
            Ok(valid) => valid,
            Err(errors) => {
                let received = serde_json::to_string_pretty(arguments).unwrap_or_default();
                let text = format!(
                    "Validation failed for tool \"{NAME}\":\n{errors}\n\nReceived arguments:\n{received}"
                );
                return (text, true, false);
            }
        };
        let Some((_, phase)) = &mut self.active else {
            return (NO_ACTIVE_COMMAND_ERROR.into(), false, false);
        };
        let Some(status) = status else {
            let text = match phase {
                Phase::Probing { continues } if *continues >= CONTINUE_LIMIT => format!(
                    "You have reported \"continuing\" {CONTINUE_LIMIT} times without completing \
the command. Report your actual status: completed, blocked, or incomplete."
                ),
                Phase::Probing { continues } => {
                    *phase = Phase::Running {
                        continues: *continues + 1,
                    };
                    CONTINUING.into()
                }
                Phase::Dormant { .. } => {
                    *phase = Phase::Running { continues: 0 };
                    CONTINUING.into()
                }
                _ => CONTINUING_OUT_OF_PHASE_ERROR.into(),
            };
            return (text, false, false);
        };
        match phase {
            Phase::Parked => return (PARKED_ERROR.into(), false, false),
            Phase::Reported(_) => return (ALREADY_REPORTED_ERROR.into(), false, false),
            _ => {}
        }
        let next_steps: Vec<Offered> = arguments["next_steps"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|step| Offered {
                message: step["message"].as_str().unwrap_or_default().to_owned(),
                fresh_session: step["fresh_session"].as_bool().unwrap_or(false),
                reason: step["reason"].as_str().map(str::to_owned),
            })
            .collect();
        let recommended = arguments["recommended_next_step"]
            .as_u64()
            .map(|index| index as usize);
        let next = match recommended {
            Some(index) => next_steps.get(index),
            None => next_steps.first(),
        };
        let next = next.filter(|step| parse_slash(&step.message).is_some());
        let text = match next.filter(|_| status == Status::Completed) {
            Some(step) => format!("→ {}", step.message.trim()),
            None => format!("status: {}", status.name()),
        };
        *phase = Phase::Reported(Report {
            status,
            summary: summary.to_owned(),
            next_steps,
            recommended,
        });
        (text, false, true)
    }

    /// After a turn: what follows it; `autopilot` as `/autopilot` is, which the status check
    /// tells the model.
    pub fn turn_ended(&mut self, commands: &Commands, autopilot: bool) -> Ended {
        let Some((name, phase)) = &mut self.active else {
            return Ended::Nothing;
        };
        match phase {
            Phase::Running { continues } => {
                let policy = commands.get(name).and_then(|command| command.next.as_ref());
                let probe = probe_message(policy, name, autopilot);
                *phase = Phase::Probing {
                    continues: *continues,
                };
                Ended::Probe(probe)
            }
            // As in Scramjet, a status check left unanswered pauses the command.
            Phase::Probing { .. } => {
                *phase = Phase::Dormant { resumable: false };
                Ended::Nothing
            }
            Phase::Reported(report) if report.status == Status::Completed => {
                let completed = Completed {
                    command: name.clone(),
                    report: report.clone(),
                };
                self.active = None;
                Ended::Completed(completed)
            }
            Phase::Reported(report) => {
                let notice = (report.status == Status::Blocked).then(|| {
                    format!(
                        "scramjet: command blocked — {}",
                        clean_notice(&report.summary)
                    )
                });
                *phase = Phase::Dormant { resumable: false };
                notice.map_or(Ended::Nothing, Ended::Notice)
            }
            Phase::Parked | Phase::Dormant { .. } => Ended::Nothing,
        }
    }

    /// Scramjet's system prompt section for a dormant command.
    pub fn dormant_notice(&self) -> Option<String> {
        let Some((name, Phase::Dormant { resumable })) = &self.active else {
            return None;
        };
        let terminal = "If the work is already done, report a terminal status directly \
(`status: \"completed\"`, `\"blocked\"`, or `\"incomplete\"`).";
        let notice = if *resumable {
            format!(
                "The command `{name}` is dormant — it started but is not currently active.\n\
The next interactive non-slash user reply will resume it because structured input was cancelled. \
Continue the command work after that reply without first reporting `continuing`.\n{terminal}"
            )
        } else {
            format!(
                "The command `{name}` is dormant — it started but is not currently active.\n\
Ordinary user replies do NOT auto-resume a dormant command.\n\
You have two options:\n\
- Before resuming work, first call `report_scramjet_command_status` with `status: \"continuing\"`.\n\
- {terminal}\n\
Both paths are accepted from dormant state."
            )
        };
        Some(format!("\n\n# Dormant Scramjet Command\n\n{notice}"))
    }
}

/// The summary and the terminal status (`None`: continuing), or the schema errors, as Pi lists
/// them.
fn validate(arguments: &Value) -> Result<(&str, Option<Status>), String> {
    let mut errors = Vec::new();
    let summary = arguments["summary"].as_str().unwrap_or_default();
    if summary.trim().is_empty() {
        errors.push("  - summary: must be a non-empty string with a non-blank character");
    }
    let status = match arguments["status"].as_str() {
        Some("completed") => Some(Some(Status::Completed)),
        Some("blocked") => Some(Some(Status::Blocked)),
        Some("incomplete") => Some(Some(Status::Incomplete)),
        Some("continuing") => Some(None),
        _ => {
            errors.push("  - status: must be one of completed, blocked, incomplete or continuing");
            None
        }
    };
    match &arguments["next_steps"] {
        Value::Null => {}
        Value::Array(steps) => {
            let valid = |step: &Value| {
                step["message"].is_string()
                    && step["fresh_session"].is_boolean()
                    && (step["reason"].is_null() || step["reason"].is_string())
            };
            if !steps.iter().all(valid) {
                errors.push(
                    "  - next_steps: each entry must have a string message, a boolean \
fresh_session and an optional string reason",
                );
            }
        }
        _ => errors.push("  - next_steps: must be an array"),
    }
    let recommended = &arguments["recommended_next_step"];
    if !recommended.is_null() && !recommended.is_u64() {
        errors.push("  - recommended_next_step: must be an integer of at least 0");
    }
    match (errors.is_empty(), status) {
        (true, Some(status)) => Ok((summary, status)),
        _ => Err(errors.join("\n")),
    }
}

/// A slash command's name and arguments, as Scramjet's `parseSlashCommand` reads a message.
fn parse_slash(message: &str) -> Option<(&str, Option<&str>)> {
    let rest = message.trim().strip_prefix('/')?;
    let (name, args) = match rest.split_once(' ') {
        Some((name, args)) => (
            name,
            Some(args.trim_start()).filter(|args| !args.is_empty()),
        ),
        None => (rest, None),
    };
    (!name.is_empty()).then_some((name, args))
}

/// Text from the model shown on one line: controls as spaces, blanks collapsed.
fn clean_display(text: &str) -> String {
    let spaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// As Scramjet's `cleanForNotify`: one line, at most 200 characters.
fn clean_notice(text: &str) -> String {
    let clean = clean_display(text);
    match clean.char_indices().nth(199) {
        Some((at, _)) if clean.chars().count() > 200 => format!("{}…", &clean[..at]),
        _ => clean,
    }
}

/// Scramjet's `<scramjet-next-step>` block, which describes a policy to the model and what
/// `autopilot` does with the recommendation; `</` of the closing tag inside the text is escaped.
fn next_step_block(policy: &Policy, command: &str, autopilot: bool) -> String {
    let safe = |text: &str| escape_close_tag(text);
    let candidates = |candidates: &[Candidate]| {
        let rows = candidates.iter().enumerate().map(|(index, candidate)| {
            let hint = candidate.hint.as_deref().map_or(String::new(), |hint| {
                format!(
                    " — {}",
                    safe(&hint.split_whitespace().collect::<Vec<_>>().join(" "))
                )
            });
            format!("  - [{index}] {}{hint}", safe(&candidate.name))
        });
        rows.collect::<Vec<_>>().join("\n")
    };
    let choose = "Choose the best-supported continuation from the completed work and current \
context, construct `next_steps`, and derive any `recommended_next_step` from the selected entry's \
zero-based runtime index.";
    let recommendation = match autopilot {
        true => format!(
            "{choose} With `/autopilot on`, set it to highlight the selected entry; Scramjet \
auto-dispatches it only when its message is a slash command."
        ),
        false => format!(
            "{choose} With `/autopilot off`, set it to highlight the best option for the user; \
Scramjet will not auto-dispatch it."
        ),
    };
    let id = safe(command);
    let body = match policy {
        Policy::Forced { target } => format!(
            "The command `{id}` declares a `forced` next-step: `{target}` will run after this \
command completes. You may add a single next_steps entry only to pass arguments or fresh_session \
to that declared target; its message must be `/{target}` followed by any arguments. Omit \
next_steps if no runtime arguments need to be passed.",
            target = safe(target)
        ),
        Policy::Closed { candidates: list } => format!(
            "The command `{id}` declares a `closed` next-step policy.\n\
Each next_steps entry has a `message` field containing the next action as a slash command: start \
with `/`, use one of these zero-based candidates, and include any arguments (e.g., `/<candidate> \
<args>`):\n{}\n\
Entries may reuse the same command with different arguments to offer meaningful variants.\n\
Set `fresh_session: true` when the command should run in a clean session.\n\
Each entry must include reason before you set recommended_next_step.\n\
{recommendation}\n\
If none apply, omit next_steps and recommended_next_step entirely to stop the chain.",
            candidates(list)
        ),
        Policy::Open {
            candidates: list,
            blacklist,
        } => {
            let listed = match list.is_empty() {
                true => "No suggested commands are listed.\n".to_owned(),
                false => format!(
                    "Suggested commands with zero-based indexes:\n{}\n",
                    candidates(list)
                ),
            };
            let blacklisted = match blacklist.is_empty() {
                true => String::new(),
                false => {
                    let names: Vec<_> = blacklist.iter().map(|name| safe(name)).collect();
                    format!("\nDo not pick slash commands for: {}.", names.join(", "))
                }
            };
            format!(
                "The command `{id}` declares an `open` next-step policy.\n{listed}\
You may pick any slash command if it fits the work.{blacklisted}\n\
Each next_steps entry has a `message` field containing the next action.\n\
For slash commands, start with `/` and include any arguments (e.g., `/mach12:pr-merge 113`).\n\
For non-command follow-ups, write the message text directly.\n\
Set `fresh_session: true` when the command should run in a clean session.\n\
Each entry must include reason before you set recommended_next_step.\n\
{recommendation}\n\
Omit next_steps and recommended_next_step entirely to stop the chain."
            )
        }
        Policy::Ask { hint } => {
            let hint = hint
                .as_deref()
                .map_or(String::new(), |hint| format!("\n{}", safe(hint.trim())));
            format!(
                "The command `{id}` declares an `ask` next-step policy. The chain will pause for \
the user after this turn. Do not include next_steps.{hint}"
            )
        }
    };
    format!("<scramjet-next-step>\n{body}\n</scramjet-next-step>")
}

/// `</scramjet-next-step>` in any case becomes `<\/scramjet-next-step>`.
fn escape_close_tag(text: &str) -> String {
    const TAG: &str = "</scramjet-next-step>";
    let lower = text.to_ascii_lowercase();
    let mut out = String::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find(TAG) {
        let start = at + found;
        out.push_str(&text[at..start]);
        out.push_str("<\\/scramjet-next-step>");
        at = start + TAG.len();
    }
    out.push_str(&text[at..]);
    out
}

/// Scramjet's status check, sent as a hidden message when a command's turn ends without a report.
fn probe_message(policy: Option<&Policy>, command: &str, autopilot: bool) -> String {
    let id = escape_close_tag(command);
    let preamble = format!(
        "Scramjet status check for `{id}`.\n\n\
Choose one route — do not write prose unless you continue command work after user input.\n\n\
`report_scramjet_command_status` — summarize the work you performed, then report your status, \
then stop the probe turn. The `summary` is your evidence and comes first; the `status` is your \
assessment of it. Summarize only the work completed since your previous report (or all of it on \
your first report):\n\
- `continuing` — you have more work to do (not blocked, not waiting for input, not finished)\n\
- `completed` — the command's work is done and your answer was already delivered\n\
- `blocked` — cannot proceed (error, missing dependency, authorization)\n\
- `incomplete` — stopped without clean completion\n\n\
`get_scramjet_user_input` — if you need user input before continuing:\n\
- successful `confirm`/`select` responses return in this probe turn; continue command work in \
this turn\n\
- `freetext` parks the command for the user's standard-editor reply; do not try to continue \
same-turn\n\
- cancelled `confirm`/`select` puts the command dormant; do not try to continue same-turn"
    );
    match policy {
        None => format!(
            "{preamble}\n\nThis command has no next-step policy. When reporting `completed`, omit \
`next_steps` entirely — no chaining will occur."
        ),
        Some(policy) => format!(
            "{preamble}\n\n{}",
            next_step_block(policy, command, autopilot)
        ),
    }
}

/// A next step to take: a prompt to send, in a new session or not.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// Its position in the model's `next_steps`.
    pub index: usize,
    /// As the model wrote it, shown in the list.
    pub message: String,
    pub reason: String,
    pub fresh_session: bool,
    /// What runs: `/<name> args` for a command; `None` for text, which goes into the editor.
    pub command: Option<String>,
}

/// What follows a completed command.
#[derive(Debug, PartialEq)]
pub enum Next {
    Nothing,
    /// Runs `steps[chosen]` at once: a forced next step, or the recommended one when an edge
    /// chains it or, without a list, `/autopilot on` chooses it. `steps` are those offered.
    Run {
        steps: Vec<Step>,
        chosen: usize,
    },
    /// For the user to choose; `recommended` is a position in `steps`, which `/autopilot on`
    /// chooses after a countdown when `autoselect`.
    Offer {
        steps: Vec<Step>,
        recommended: Option<usize>,
        autoselect: bool,
    },
}

/// The next step of a completed command and what to tell the user about it.
#[derive(Debug, PartialEq)]
pub struct Routing {
    pub source: String,
    pub notices: Vec<String>,
    pub next: Next,
}

/// Routes a completed command's next steps as Scramjet's `routeCompleted` does: `interactive`
/// when the user can choose from a list, `autopilot` as `/autopilot` is.
pub(super) fn route(
    completed: &Completed,
    commands: &Commands,
    edges: &Edges,
    interactive: bool,
    autopilot: bool,
) -> Routing {
    let source = completed.command.clone();
    let report = &completed.report;
    let mut notices = edges.diagnostics.clone();
    let policy = commands
        .get(&source)
        .and_then(|command| command.next.as_ref());
    let next = match policy {
        None => Next::Nothing,
        Some(Policy::Forced { target }) => forced(target, report, commands, &mut notices),
        Some(Policy::Ask { .. }) => {
            if !report.next_steps.is_empty() {
                notices.push(
                    "scramjet: ask-mode command; agent proposed next steps — ignored, waiting \
for user"
                        .into(),
                );
            }
            Next::Nothing
        }
        Some(policy) => {
            let checked = check(report, policy, commands);
            offered(
                checked,
                &source,
                edges,
                (interactive, autopilot),
                &mut notices,
            )
        }
    };
    Routing {
        source,
        notices,
        next,
    }
}

fn forced(target: &str, report: &Report, commands: &Commands, notices: &mut Vec<String>) -> Next {
    let Some(command) = commands.get(target) else {
        notices.push(format!(
            "scramjet: forced target \"{target}\" not in registry; auto-continue skipped"
        ));
        return Next::Nothing;
    };
    if command.delegate_only {
        notices.push(format!(
            "scramjet: forced target \"{target}\" is delegate-only; auto-continue skipped"
        ));
        return Next::Nothing;
    }
    let mut step = Step {
        index: 0,
        message: format!("/{target}"),
        reason: String::new(),
        fresh_session: false,
        command: Some(format!("/{target}")),
    };
    let handoff = report.next_steps.first();
    if let Some((offered, (name, args))) =
        handoff.and_then(|offered| Some((offered, parse_slash(&offered.message)?)))
    {
        if name == target {
            let wire = format!(
                "/{target}{}",
                args.map_or(String::new(), |a| format!(" {a}"))
            );
            step = Step {
                message: offered.message.clone(),
                reason: offered.reason.clone().unwrap_or_default(),
                fresh_session: offered.fresh_session,
                command: Some(wire),
                ..step
            };
        } else {
            notices.push(format!(
                "scramjet: forced target is \"{target}\"; agent supplied next_steps name \
\"{name}\" — ignoring supplied forced handoff"
            ));
        }
    }
    Next::Run {
        steps: vec![step],
        chosen: 0,
    }
}

/// The steps a policy allows, with why others were left out, as Scramjet's `validateNextSteps`.
struct Checked {
    valid: Vec<Step>,
    skipped: Vec<(usize, String, String)>,
    recommended: Option<usize>,
    recommended_reason: Option<String>,
    reason: Option<String>,
}

fn check(report: &Report, policy: &Policy, commands: &Commands) -> Checked {
    let mut valid = Vec::new();
    let mut skipped = Vec::new();
    for (index, offered) in report.next_steps.iter().enumerate() {
        match allowed(offered, policy, commands) {
            Ok(command) => valid.push(Step {
                index,
                message: offered.message.clone(),
                reason: offered.reason.clone().unwrap_or_default(),
                fresh_session: offered.fresh_session,
                command,
            }),
            Err(reason) => skipped.push((index, offered.message.trim().to_owned(), reason)),
        }
    }
    let reason = skipped.first().map(|(_, _, reason)| reason.clone());
    let (recommended, recommended_reason) = match report.recommended {
        Some(index) => match valid.iter().position(|step| step.index == index) {
            Some(position) => (Some(position), None),
            None => match skipped.iter().find(|(skipped, _, _)| *skipped == index) {
                Some((_, label, why)) => (
                    None,
                    Some(format!(
                        "recommended_next_step {index} points to invalid next step {label}: {why}"
                    )),
                ),
                None => (
                    None,
                    Some(format!(
                        "recommended_next_step {index} is outside next_steps"
                    )),
                ),
            },
        },
        None if !valid.is_empty() => (
            None,
            Some("missing recommended_next_step; no automatic next-step dispatch".into()),
        ),
        None => (None, None),
    };
    Checked {
        valid,
        skipped,
        recommended,
        recommended_reason,
        reason,
    }
}

/// The command a step runs (`None`: text), or why the policy does not allow it.
fn allowed(
    offered: &Offered,
    policy: &Policy,
    commands: &Commands,
) -> Result<Option<String>, String> {
    let parsed = parse_slash(&offered.message);
    let open = matches!(policy, Policy::Open { .. });
    if parsed.is_none() && !open {
        return Err("non-command messages are valid only for open policies".into());
    }
    if let Some((name, _)) = parsed {
        match policy {
            Policy::Closed { candidates } if !candidates.iter().any(|c| c.name == name) => {
                let names: Vec<_> = candidates.iter().map(|c| c.name.as_str()).collect();
                return Err(format!(
                    "{name} is not in closed candidates [{}]",
                    names.join(", ")
                ));
            }
            Policy::Open { blacklist, .. } if blacklist.iter().any(|n| n == name) => {
                return Err(format!("{name} is blacklisted"));
            }
            _ => {}
        }
        match commands.get(name) {
            None => return Err(format!("{name} is not registered")),
            Some(command) if command.delegate_only => {
                return Err(format!(
                    "{name} is delegate-only (invoke via delegate, not top-level dispatch)"
                ));
            }
            Some(_) => {}
        }
    }
    if offered
        .reason
        .as_deref()
        .is_none_or(|r| r.trim().is_empty())
    {
        return Err("selector-visible next steps must include reason".into());
    }
    Ok(parsed
        .map(|(name, args)| format!("/{name}{}", args.map_or(String::new(), |a| format!(" {a}")))))
}

fn offered(
    checked: Checked,
    source: &str,
    edges: &Edges,
    (interactive, autopilot): (bool, bool),
    notices: &mut Vec<String>,
) -> Next {
    if checked.valid.is_empty() {
        if let Some(reason) = checked.reason {
            notices.push(format!("scramjet: {reason}"));
        }
        return Next::Nothing;
    }
    if !checked.skipped.is_empty() {
        let skipped: Vec<_> = checked
            .skipped
            .iter()
            .map(|(_, label, why)| format!("{} ({})", clean_notice(label), clean_notice(why)))
            .collect();
        notices.push(format!(
            "scramjet: skipped invalid next step(s): {}",
            clean_notice(&skipped.join(", "))
        ));
    }
    let recommended = checked.recommended.map(|at| &checked.valid[at]);
    let edge = recommended
        .and_then(|step| step.command.as_deref())
        .and_then(|command| parse_slash(command))
        .and_then(|(name, _)| edges.resolve(source, name));
    let run = |chosen| Next::Run {
        steps: checked.valid.clone(),
        chosen,
    };
    if let Some(at) = checked.recommended
        && recommended.is_some_and(|step| step.command.is_some())
        && edge == Some(Edge::Chain)
    {
        return run(at);
    }
    let options = || {
        let messages: Vec<_> = checked
            .valid
            .iter()
            .map(|s| clean_notice(&s.message))
            .collect();
        clean_notice(&messages.join(", "))
    };
    if interactive {
        if let Some(reason) = &checked.recommended_reason {
            notices.push(format!(
                "scramjet: {reason}; valid option(s): {}",
                options()
            ));
        }
        let autoselect = autopilot
            && edge != Some(Edge::Pause)
            && recommended.is_some_and(|step| step.command.is_some());
        return Next::Offer {
            steps: checked.valid,
            recommended: checked.recommended,
            autoselect,
        };
    }
    // Without a list to choose from, only autopilot runs the recommended command.
    let Some(at) = checked.recommended else {
        if let Some(reason) = &checked.recommended_reason {
            notices.push(format!(
                "scramjet: {reason}; valid option(s): {}",
                options()
            ));
        }
        return Next::Nothing;
    };
    let step = &checked.valid[at];
    let message = clean_notice(&step.message);
    let fresh = if step.fresh_session {
        " (fresh session)"
    } else {
        ""
    };
    let notice = match (step.command.is_some(), edge, autopilot) {
        (false, _, true) => format!(
            "scramjet: recommended next step is not a slash command ({message}); automatic \
dispatch skipped"
        ),
        (false, _, false) => format!(
            "scramjet: next would be {message}; /autopilot on only auto-dispatches command next \
steps"
        ),
        (true, Some(Edge::Pause), _) => {
            format!("scramjet: next would be {message}{fresh}; edge setting: pause")
        }
        (true, _, true) => return run(at),
        (true, _, false) => {
            format!("scramjet: next would be {message}{fresh}; /autopilot on to chain")
        }
    };
    notices.push(notice);
    Next::Nothing
}

/// Scramjet's record of a next-step choice: the options offered and the one chosen (`None`:
/// the list was dismissed).
pub fn record(source: &str, steps: &[Step], chosen: Option<usize>) -> String {
    let mut lines = vec![format!("Next step (from {}):", clean_display(source))];
    for (position, step) in steps.iter().enumerate() {
        let mark = match chosen {
            None => "- ",
            Some(at) if at == position => "→ ",
            Some(_) => "  ",
        };
        let reason = clean_display(&step.reason);
        let reason = match reason.is_empty() {
            true => String::new(),
            false => format!(" — {reason}"),
        };
        lines.push(format!("{mark}{}{reason}", clean_display(&step.message)));
    }
    if chosen.is_none() {
        lines.push("Cancelled".into());
    }
    lines.join("\n")
}

/// The arguments of the `RECORD` call for `record`'s choice, as Scramjet's
/// `NextStepRecordParams`.
pub(super) fn record_arguments(source: &str, steps: &[Step], chosen: Option<usize>) -> Value {
    let options: Vec<_> = (steps.iter())
        .map(|step| match step.reason.is_empty() {
            true => json!({"message":step.message}),
            false => json!({"message":step.message,"reason":step.reason}),
        })
        .collect();
    let outcome = match chosen {
        Some(_) => "selected",
        None => "dismissed",
    };
    json!({"outcome":outcome,"options":options,"selectedIndex":chosen,"sourceCommand":source,
        "source":"completion"})
}

/// A `RECORD` call's line: the step chosen, or that the list was dismissed.
pub(super) fn record_summary(arguments: &Value) -> String {
    let chosen = (arguments["selectedIndex"].as_u64())
        .and_then(|at| arguments["options"].get(at as usize))
        .and_then(|option| option["message"].as_str());
    chosen.map_or_else(|| "dismissed".into(), clean_display)
}

impl Step {
    /// The list row: its position in `next_steps` and its message, as Scramjet's selector
    /// labels it.
    pub fn label(&self) -> String {
        format!("{}: {}", self.index, clean_display(&self.message))
    }

    pub fn description(&self) -> String {
        clean_display(&self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> Commands {
        Commands::parsed(&[
            (
                "rs:loop.md",
                "---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:loop\n    - name: rs:steer\n---\nLoop",
            ),
            ("rs:steer.md", "Steer"),
            (
                "rs:open.md",
                "---\nnext:\n  mode: open\n  candidates: []\n  blacklist: [rs:steer]\n---\nOpen",
            ),
            (
                "rs:forced.md",
                "---\nnext:\n  mode: forced\n  target: rs:steer\n---\nForced",
            ),
            ("rs:ask.md", "---\nnext:\n  mode: ask\n---\nAsk"),
            ("rs:helper.md", "---\ndelegate-only: true\n---\nHelp"),
            (
                "rs:forced-helper.md",
                "---\nnext:\n  mode: forced\n  target: rs:helper\n---\nForced",
            ),
        ])
    }

    #[test]
    fn status_checks_are_scramjets() {
        let reference: Value = serde_json::from_str(include_str!("scramjet_probes.json")).unwrap();
        let candidate = |name: &str, hint: Option<&str>| Candidate {
            name: name.into(),
            hint: hint.map(str::to_owned),
        };
        let closed = Policy::Closed {
            candidates: vec![
                candidate("rs:loop", Some("Continue with the\n  next one.")),
                candidate("rs:steer", None),
            ],
        };
        let open = Policy::Open {
            candidates: Vec::new(),
            blacklist: vec!["rs:x".into(), "a</SCRAMJET-next-step>b".into()],
        };
        let forced = Policy::Forced {
            target: "rs:review".into(),
        };
        let ask = Policy::Ask {
            hint: Some("  Ask them.  ".into()),
        };
        for (key, policy, command) in [
            ("closed", Some(&closed), "rs:loop"),
            ("open", Some(&open), "rs:o"),
            ("forced", Some(&forced), "rs:f"),
            ("ask", Some(&ask), "rs:a"),
            ("none", None, "rs:n"),
        ] {
            assert_eq!(
                probe_message(policy, command, false),
                reference[key],
                "{key}"
            );
        }
    }

    #[test]
    fn a_saved_command_is_restored_as_scramjet_rebuilds_it() {
        let commands = commands();
        let restored = |lifecycle: &Lifecycle| Lifecycle::restored(Some(&lifecycle.saved()));
        let mut lifecycle = Lifecycle::default();
        assert_eq!(
            lifecycle.saved(),
            json!({"type":"command_state","command":null})
        );
        assert_eq!(restored(&lifecycle).active, None);
        assert_eq!(Lifecycle::restored(None).active, None);

        // Working, checked or reported: dormant once resumed.
        lifecycle.prompted("/rs:loop", Some("rs:loop"));
        let started = json!({"type":"command_state","command":"rs:loop"});
        assert_eq!(lifecycle.saved(), started);
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Probe(_)
        ));
        assert_eq!(lifecycle.saved(), started);
        lifecycle.report(&json!({"summary":"Done.","status":"completed"}));
        assert_eq!(lifecycle.saved(), started);
        let mut resumed = restored(&lifecycle);
        assert!(
            resumed
                .dormant_notice()
                .unwrap()
                .contains("do NOT auto-resume")
        );
        resumed.prompted("go on", None);
        assert_eq!(resumed.turn_ended(&commands, false), Ended::Nothing);
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Completed(_)
        ));
        assert_eq!(restored(&lifecycle).active, None);

        // Parked for a reply, or resumable after a cancelled question: the reply resumes it.
        lifecycle.prompted("/rs:loop", Some("rs:loop"));
        lifecycle.park();
        assert_eq!(lifecycle.saved()["state"], "parked");
        let mut resumed = restored(&lifecycle);
        assert_eq!(resumed.dormant_notice(), None);
        resumed.prompted("blue", None);
        assert!(matches!(
            resumed.turn_ended(&commands, false),
            Ended::Probe(_)
        ));
        lifecycle.question_cancelled();
        assert_eq!(lifecycle.saved()["state"], "resumable");
        let mut resumed = restored(&lifecycle);
        assert!(resumed.dormant_notice().unwrap().contains("will resume it"));
        resumed.prompted("blue", None);
        assert!(matches!(
            resumed.turn_ended(&commands, false),
            Ended::Probe(_)
        ));
    }

    #[test]
    fn reports_follow_scramjets_lifecycle() {
        let commands = commands();
        let mut lifecycle = Lifecycle::default();
        let completed = json!({"summary":"Did it.","status":"completed"});
        let continuing = json!({"summary":"More.","status":"continuing"});
        let error = |text: &str| (text.to_owned(), false, false);
        assert_eq!(lifecycle.report(&completed), error(NO_ACTIVE_COMMAND_ERROR));
        lifecycle.prompted("/rs:loop x", Some("rs:loop"));
        assert_eq!(
            lifecycle.report(&continuing),
            error(CONTINUING_OUT_OF_PHASE_ERROR)
        );
        let Ended::Probe(check) = lifecycle.turn_ended(&commands, false) else {
            panic!("no status check");
        };
        assert!(check.starts_with("Scramjet status check for `rs:loop`."));
        assert!(check.contains("declares a `closed` next-step policy"));
        for _ in 0..CONTINUE_LIMIT {
            assert_eq!(lifecycle.report(&continuing), error(CONTINUING));
            assert!(matches!(
                lifecycle.turn_ended(&commands, false),
                Ended::Probe(_)
            ));
        }
        let (limit, _, _) = lifecycle.report(&continuing);
        assert!(limit.starts_with("You have reported \"continuing\" 3 times"));
        let with_next = json!({"summary":"Did it.","status":"completed","recommended_next_step":1,
            "next_steps":[{"message":"text","fresh_session":false,"reason":"r"},
                {"message":" /rs:steer now ","fresh_session":true,"reason":"r"}]});
        assert_eq!(
            lifecycle.report(&with_next),
            ("→ /rs:steer now".into(), false, true)
        );
        assert_eq!(lifecycle.report(&completed), error(ALREADY_REPORTED_ERROR));
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Completed(_)
        ));
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);

        // Blocked: the user is told; the command is dormant until the model says it continues.
        lifecycle.prompted("/rs:steer", Some("rs:steer"));
        let blocked = json!({"summary":"Needs\n a  key.","status":"blocked",
            "next_steps":[{"message":"/rs:loop","fresh_session":false}]});
        assert_eq!(
            lifecycle.report(&blocked),
            ("status: blocked".into(), false, true)
        );
        assert_eq!(
            lifecycle.turn_ended(&commands, false),
            Ended::Notice("scramjet: command blocked — Needs a key.".into())
        );
        let notice = lifecycle.dormant_notice().unwrap();
        assert!(
            notice
                .starts_with("\n\n# Dormant Scramjet Command\n\nThe command `rs:steer` is dormant")
        );
        assert!(notice.contains("Ordinary user replies do NOT auto-resume a dormant command."));
        lifecycle.prompted("why?", None);
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);
        assert_eq!(lifecycle.report(&continuing), error(CONTINUING));
        assert_eq!(lifecycle.dormant_notice(), None);
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Probe(_)
        ));

        // Parked for a reply, which resumes it.
        assert!(lifecycle.park());
        assert_eq!(lifecycle.report(&completed), error(PARKED_ERROR));
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);
        lifecycle.prompted("the answer", None);
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Probe(_)
        ));

        // A cancelled question: the next reply resumes it; a `/` prompt naming nothing ends it.
        lifecycle.question_cancelled();
        assert!(
            lifecycle
                .dormant_notice()
                .unwrap()
                .contains("will resume it because structured input was cancelled")
        );
        lifecycle.prompted("go on", None);
        assert_eq!(lifecycle.dormant_notice(), None);
        lifecycle.prompted("/unknown", None);
        assert_eq!(lifecycle.report(&completed), error(NO_ACTIVE_COMMAND_ERROR));
        assert!(!lifecycle.park());

        // A cancelled turn drops its report.
        lifecycle.prompted("/rs:loop", Some("rs:loop"));
        lifecycle.report(&completed);
        assert_eq!(
            lifecycle.cancelled().as_deref(),
            Some("scramjet: aborted — discarding \"completed\" status report for rs:loop")
        );
        assert!(lifecycle.dormant_notice().is_some());

        let invalid = json!({"summary":" ","status":"done","next_steps":[{"message":1}],
            "recommended_next_step":-1});
        let (text, failed, ends) = lifecycle.report(&invalid);
        assert!(failed && !ends);
        let received = serde_json::to_string_pretty(&invalid).unwrap();
        let errors = [
            "summary: must be a non-empty string with a non-blank character",
            "status: must be one of completed, blocked, incomplete or continuing",
            "next_steps: each entry must have a string message, a boolean fresh_session and an \
optional string reason",
            "recommended_next_step: must be an integer of at least 0",
        ];
        let errors: Vec<_> = errors.iter().map(|error| format!("  - {error}")).collect();
        assert_eq!(
            text,
            format!(
                "Validation failed for tool \"report_scramjet_command_status\":\n{}\n\n\
Received arguments:\n{received}",
                errors.join("\n")
            )
        );
    }

    fn completed(command: &str, report: Value) -> Completed {
        let mut lifecycle = Lifecycle::default();
        lifecycle.prompted(&format!("/{command}"), Some(command));
        let mut report = report;
        report["summary"] = json!("Done.");
        report["status"] = json!("completed");
        assert!(lifecycle.report(&report).2);
        match lifecycle.turn_ended(&commands(), false) {
            Ended::Completed(completed) => completed,
            other => panic!("{other:?}"),
        }
    }

    fn run(step: Step) -> Next {
        Next::Run {
            steps: vec![step],
            chosen: 0,
        }
    }

    fn step(message: &str, reason: Option<&str>, fresh: bool) -> Value {
        let mut step = json!({"message":message,"fresh_session":fresh});
        if let Some(reason) = reason {
            step["reason"] = json!(reason);
        }
        step
    }

    #[test]
    fn next_steps_are_routed_as_scramjet_routes_them_without_autopilot() {
        let commands = commands();
        let none = Edges::default();
        let route = |done: &Completed, edges: &Edges, interactive| {
            let routing = route(done, &commands, edges, interactive, false);
            (routing.notices, routing.next)
        };
        let again = Step {
            index: 0,
            message: "/rs:loop  more".into(),
            reason: "again".into(),
            fresh_session: true,
            command: Some("/rs:loop more".into()),
        };
        let looped = completed(
            "rs:loop",
            json!({"recommended_next_step":0,"next_steps":[
                step("/rs:loop  more", Some("again"), true),
                step("/rs:other", Some("r"), false),
                step("/rs:steer", Some(" "), false),
                step("plain text", Some("r"), false)]}),
        );
        let skipped = "scramjet: skipped invalid next step(s): /rs:other (rs:other is not in \
closed candidates [rs:loop, rs:steer]), /rs:steer (selector-visible next steps must include \
reason), plain text (non-command messages are valid only for open policies)";
        assert_eq!(
            route(&looped, &none, true),
            (
                vec![skipped.to_owned()],
                Next::Offer {
                    steps: vec![again.clone()],
                    recommended: Some(0),
                    autoselect: false,
                }
            )
        );
        let chain = Edges::parse("edges:\n  rs:loop:\n    rs:loop: chain\n");
        assert_eq!(route(&looped, &chain, false).1, run(again.clone()));
        let next = "scramjet: next would be /rs:loop more (fresh session)";
        assert_eq!(
            route(&looped, &none, false),
            (
                vec![
                    skipped.to_owned(),
                    format!("{next}; /autopilot on to chain")
                ],
                Next::Nothing
            )
        );
        let pause = Edges::parse("edges:\n  rs:loop:\n    '*': pause\n");
        assert_eq!(
            route(&looped, &pause, false).0[1],
            format!("{next}; edge setting: pause")
        );

        let misdirected = completed(
            "rs:loop",
            json!({"recommended_next_step":1,"next_steps":[
                step("/rs:loop", Some("again"), false), step("/rs:other", Some("r"), false)]}),
        );
        let (notices, next) = route(&misdirected, &chain, true);
        assert_eq!(
            notices[1],
            "scramjet: recommended_next_step 1 points to invalid next step /rs:other: rs:other is \
not in closed candidates [rs:loop, rs:steer]; valid option(s): /rs:loop"
        );
        assert!(matches!(
            next,
            Next::Offer {
                recommended: None,
                ..
            }
        ));
        let unrecommended = completed(
            "rs:loop",
            json!({"next_steps":[step("/rs:loop", Some("again"), false)]}),
        );
        assert_eq!(
            route(&unrecommended, &chain, false),
            (
                vec![
                    "scramjet: missing recommended_next_step; no automatic next-step dispatch; \
valid option(s): /rs:loop"
                        .to_owned()
                ],
                Next::Nothing
            )
        );
        let invalid = completed(
            "rs:loop",
            json!({"next_steps":[step("/rs:x", None, false)]}),
        );
        assert_eq!(
            route(&invalid, &none, true),
            (
                vec!["scramjet: rs:x is not in closed candidates [rs:loop, rs:steer]".to_owned()],
                Next::Nothing
            )
        );

        // Open: text goes to the editor; blacklisted and unknown commands are left out.
        let opened = completed(
            "rs:open",
            json!({"recommended_next_step":0,"next_steps":[
                step("Write the tests", Some("next"), false),
                step("/rs:steer", Some("r"), false),
                step("/rs:nope", Some("r"), false),
                step("/rs:helper", Some("r"), false)]}),
        );
        let (notices, next) = route(&opened, &none, true);
        assert_eq!(
            notices,
            [
                "scramjet: skipped invalid next step(s): /rs:steer (rs:steer is blacklisted), \
/rs:nope (rs:nope is not registered), /rs:helper (rs:helper is delegate-only (invoke via \
delegate, not top-level dispatch))"
            ]
        );
        let Next::Offer { steps, .. } = next else {
            panic!("{next:?}")
        };
        assert_eq!(steps[0].command, None);
        assert_eq!(
            route(&opened, &none, false).0[1],
            "scramjet: next would be Write the tests; /autopilot on only auto-dispatches command \
next steps"
        );

        // Forced: the target runs, with the arguments of a matching first step.
        let forced = |steps: Value| {
            route(
                &completed("rs:forced", json!({"next_steps":steps})),
                &none,
                false,
            )
        };
        let target = |command: &str, fresh| Step {
            index: 0,
            message: command.into(),
            reason: String::new(),
            fresh_session: fresh,
            command: Some(command.into()),
        };
        assert_eq!(forced(json!([])), (vec![], run(target("/rs:steer", false))));
        assert_eq!(
            forced(json!([step("/rs:steer 5", None, true)])),
            (vec![], run(target("/rs:steer 5", true)))
        );
        assert_eq!(
            forced(json!([step("/rs:loop", None, true)])),
            (
                vec![
                    "scramjet: forced target is \"rs:steer\"; agent supplied next_steps name \
\"rs:loop\" — ignoring supplied forced handoff"
                        .to_owned()
                ],
                run(target("/rs:steer", false))
            )
        );
        assert_eq!(
            route(
                &completed("rs:forced-helper", json!({"next_steps":[]})),
                &none,
                false
            ),
            (
                vec![
                    "scramjet: forced target \"rs:helper\" is delegate-only; auto-continue \
skipped"
                        .to_owned()
                ],
                Next::Nothing
            )
        );
        let asked = completed(
            "rs:ask",
            json!({"next_steps":[step("/rs:loop", Some("r"), false)]}),
        );
        assert_eq!(
            route(&asked, &none, true),
            (
                vec![
                    "scramjet: ask-mode command; agent proposed next steps — ignored, waiting \
for user"
                        .to_owned()
                ],
                Next::Nothing
            )
        );
        let unpoliced = completed(
            "rs:steer",
            json!({"next_steps":[step("/rs:loop", Some("r"), false)]}),
        );
        assert_eq!(route(&unpoliced, &none, true), (vec![], Next::Nothing));
    }

    #[test]
    fn autopilot_runs_the_recommended_command_as_scramjet_does() {
        let commands = commands();
        let none = Edges::default();
        let pause = Edges::parse("edges:\n  rs:loop:\n    rs:loop: pause\n");
        let route = |done: &Completed, edges: &Edges, interactive| {
            let routing = route(done, &commands, edges, interactive, true);
            (routing.notices, routing.next)
        };
        let autoselect = |next: Next| match next {
            Next::Offer { autoselect, .. } => autoselect,
            other => panic!("{other:?}"),
        };
        let looped = completed(
            "rs:loop",
            json!({"recommended_next_step":0,"next_steps":[
                step("/rs:loop 2", Some("again"), false), step("/rs:steer", Some("r"), false)]}),
        );
        // The list chooses the recommended command after a countdown, unless its edge pauses.
        assert!(autoselect(route(&looped, &none, true).1));
        assert!(!autoselect(route(&looped, &pause, true).1));
        let (notices, next) = route(&looped, &none, false);
        assert!(notices.is_empty(), "{notices:?}");
        let Next::Run { steps, chosen: 0 } = next else {
            panic!("{next:?}")
        };
        let offered: Vec<_> = steps.iter().map(|step| step.command.as_deref()).collect();
        assert_eq!(offered, [Some("/rs:loop 2"), Some("/rs:steer")]);
        assert_eq!(
            route(&looped, &pause, false),
            (
                vec!["scramjet: next would be /rs:loop 2; edge setting: pause".to_owned()],
                Next::Nothing
            )
        );
        // Without a recommendation, or with text recommended, nothing is chosen.
        let unrecommended = completed(
            "rs:loop",
            json!({"next_steps":[step("/rs:loop", Some("again"), false)]}),
        );
        assert!(!autoselect(route(&unrecommended, &none, true).1));
        assert_eq!(route(&unrecommended, &none, false).1, Next::Nothing);
        let text = completed(
            "rs:open",
            json!({"recommended_next_step":0,"next_steps":[
                step("Write the tests", Some("next"), false)]}),
        );
        assert!(!autoselect(route(&text, &none, true).1));
        assert_eq!(
            route(&text, &none, false),
            (
                vec![
                    "scramjet: recommended next step is not a slash command (Write the tests); \
automatic dispatch skipped"
                        .to_owned()
                ],
                Next::Nothing
            )
        );
        // The status check tells the model.
        let policy = commands.get("rs:loop").and_then(|c| c.next.as_ref());
        let on = probe_message(policy, "rs:loop", true);
        let off = probe_message(policy, "rs:loop", false);
        assert_eq!(
            on,
            off.replace(
                "With `/autopilot off`, set it to highlight the best option for the user; \
Scramjet will not auto-dispatch it.",
                "With `/autopilot on`, set it to highlight the selected entry; Scramjet \
auto-dispatches it only when its message is a slash command."
            )
        );
        assert_ne!(on, off);
    }

    #[test]
    fn choices_are_recorded_as_scramjet_records_them() {
        let steps = [
            Step {
                index: 0,
                message: "/rs:loop".into(),
                reason: "Continue\nnow.".into(),
                fresh_session: true,
                command: Some("/rs:loop".into()),
            },
            Step {
                index: 2,
                message: "/rs:steer".into(),
                reason: String::new(),
                fresh_session: false,
                command: Some("/rs:steer".into()),
            },
        ];
        assert_eq!(
            record("rs:loop", &steps, Some(1)),
            "Next step (from rs:loop):\n  /rs:loop — Continue now.\n→ /rs:steer"
        );
        assert_eq!(
            record("rs:loop", &steps, None),
            "Next step (from rs:loop):\n- /rs:loop — Continue now.\n- /rs:steer\nCancelled"
        );
        assert_eq!(steps[1].label(), "2: /rs:steer");
        assert_eq!(
            clean_notice(&"x".repeat(250)),
            format!("{}…", "x".repeat(199))
        );
        assert_eq!(clean_notice(&"x".repeat(200)), "x".repeat(200));
    }
}
