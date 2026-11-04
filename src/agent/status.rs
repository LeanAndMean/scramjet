//! The command lifecycle (`docs/DESIGN.md`): a command a prompt starts keeps going until the
//! model reports it `completed`, the user confirms abandoning it, or the user starts another
//! command or types an unknown slash command. Any other stop leaves it waiting for the user, whose
//! next message resumes it. A reply that ends without a report gets a status check, at most
//! `CHECK_LIMIT` in a row without the user's input. A completed command offers its next steps,
//! which its `next:` policy and the `autonomy.yaml` edges route, as Scramjet's `next-step.ts`,
//! `commands/validator.ts` and `auto-continue.ts` do.

use super::autonomy::{Edge, Edges};
use super::commands::{Candidate, Commands, Policy};
use super::question::Choice;
use serde_json::{Value, json};

pub const NAME: &str = "report_scramjet_command_status";

/// Scramjet's harness-only tool, never offered to the model: a call of it and its result record
/// a next-step choice in the conversation, which the model sees.
pub(super) const RECORD: &str = "scramjet_next_step_selection";

/// Listed under the available tools.
pub(super) const SNIPPET: &str = "You have access to `report_scramjet_command_status` for ending \
an active Scramjet command. A command keeps running until its part is over: keep working while \
there is more you can do, and ask the user with `get_scramjet_user_input` when you need something \
from them. When its part is over, deliver your complete answer, then report `completed` with a \
summary of the work; reporting ends the turn. If the user wants to stop before then, report \
`abandoned`, as the only tool call in its response: the user reads your summary and confirms. A \
response that ends without either gets a status check. Do not call this tool for ordinary user \
tasks.";

/// How many status checks in a row, without the user's input, before the command waits for them.
const CHECK_LIMIT: u32 = 3;

const NO_ACTIVE_COMMAND: &str = "No Scramjet command is running, so there is nothing to report. \
Use this tool only to end an active Scramjet command.";
const ALREADY_REPORTED: &str = "This command was already reported completed. Do not call this \
tool again.";

pub(super) fn definition() -> Value {
    let status = |value: &str| json!({"const": value, "type": "string"});
    json!({"type":"function","name":NAME,"strict":false,
        "description":"End the active Scramjet command with a summary of its work: `completed` \
when this command's part is over and your final answer is delivered, `abandoned` when the user \
wants to stop before then (they confirm first, and no next step runs). Do not call this tool for \
ordinary user tasks.",
        "parameters":{"type":"object","properties":{
            "summary":{"type":"string","minLength":1,"pattern":"\\S",
                "description":"What the command did. For `abandoned`, the user reads it before \
confirming: cover the whole command (what was done, what remains and why stop), the essentials \
first."},
            "status":{"anyOf":[status("completed"),status("abandoned")],
                "description":"completed = this command's part is over and your final answer was \
delivered. It does not mean the overall task succeeded: never claim a success that did not \
happen, and offer only next steps that fit what actually happened. abandoned = the user wants to \
stop before this command's part is over; they confirm after reading your summary, and no next \
step runs, not even a forced one. There is no status for going on: keep working, or ask the user \
with get_scramjet_user_input."},
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

/// A next step the model offers: a message to send, a slash command or text.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Offered {
    message: String,
    fresh_session: bool,
    reason: Option<String>,
}

/// A completion report's next steps.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Report {
    next_steps: Vec<Offered>,
    recommended: Option<usize>,
}

/// Where an active command is.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Working: a reply that ends without a report gets a status check. `checks`: those sent
    /// since the user's last input.
    Running { checks: u32 },
    /// Waiting for the user, whose next message resumes it.
    Waiting,
    /// Reported completed: its next steps are routed when the turn ends.
    Completed(Report),
}

/// The active command and where it is.
#[derive(Debug, Default)]
pub(super) struct Lifecycle {
    active: Option<(String, Phase)>,
}

/// What a prompt is to the active command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Prompt<'a> {
    /// It runs this command, which replaces the active one.
    Command(&'a str),
    /// The user's reply: text, or a skill.
    Reply,
    /// A slash command that is neither a command nor a skill, which leaves the active command.
    Unknown,
}

/// What a `report_scramjet_command_status` call does.
#[derive(Debug, PartialEq)]
pub(super) enum Reported {
    /// What the model receives, whether it failed, and whether the turn ends with it.
    Answer(String, bool, bool),
    /// The model asks to abandon the active command; `Lifecycle::abandon` takes the user's
    /// answer.
    Abandon(Abandon),
}

/// A request to abandon a command, for the user to confirm.
#[derive(Debug, PartialEq)]
pub(super) struct Abandon {
    command: String,
    summary: String,
}

impl Abandon {
    /// The question's title, over `choices`.
    pub fn title(&self) -> String {
        format!("Abandon /{}?", self.command)
    }

    /// Shown before the choices: the model's summary and what abandoning does.
    pub fn message(&self) -> String {
        format!(
            "{}\n\nAbandoning ends `/{}` now. No next step will run.",
            self.summary.trim(),
            self.command
        )
    }

    /// Abandon, then keep working, which is selected at first.
    pub fn choices() -> [Choice; 2] {
        ["Abandon", "Keep working"].map(Choice::plain)
    }
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
    /// The session line that keeps the active command (`null`: none) for a resume. A completion
    /// is saved when its turn ends, as Scramjet records a report then.
    pub fn saved(&self) -> Value {
        json!({"type":"command_state","command":self.active()})
    }

    /// The lifecycle a session's `saved` line leaves: its command waits for the user, as do
    /// those of older lines, whatever `state` they name (`parked`, `resumable` or none, which
    /// was dormant).
    pub fn restored(saved: Option<&Value>) -> Self {
        let active = saved.and_then(|line| {
            let name = line["command"].as_str().filter(|name| !name.is_empty())?;
            Some((name.to_owned(), Phase::Waiting))
        });
        Self { active }
    }

    /// The active command's name, whatever its phase.
    pub fn active(&self) -> Option<&str> {
        self.active.as_ref().map(|(name, _)| name.as_str())
    }

    /// Before a prompt is sent, typed or queued.
    pub fn prompted(&mut self, prompt: Prompt) {
        match prompt {
            Prompt::Command(name) => {
                self.active = Some((name.to_owned(), Phase::Running { checks: 0 }));
            }
            Prompt::Unknown => self.active = None,
            Prompt::Reply => self.answered(),
        }
    }

    /// The user answered: a waiting command resumes, and status checks count from none again.
    /// A completion stands.
    pub fn answered(&mut self) {
        if let Some((_, phase)) = &mut self.active
            && !matches!(phase, Phase::Completed(_))
        {
            *phase = Phase::Running { checks: 0 };
        }
    }

    /// A question ends the turn for the user (a freetext question, or one they dismissed): the
    /// active command waits for them, and a report it made is dropped. Whether one is active.
    pub fn wait(&mut self) -> bool {
        match &mut self.active {
            Some((_, phase)) => {
                *phase = Phase::Waiting;
                true
            }
            None => false,
        }
    }

    /// The turn was cancelled or failed: as `wait`, returning what to tell the user.
    pub fn interrupted(&mut self) -> Option<String> {
        let (name, phase) = self.active.as_mut()?;
        let dropped = match phase {
            Phase::Completed(_) => "; its completed report was dropped",
            _ => "",
        };
        *phase = Phase::Waiting;
        Some(format!(
            "/{name} is waiting for you{dropped}. Reply to continue."
        ))
    }

    /// Runs a `report_scramjet_command_status` call.
    pub fn report(&mut self, arguments: &Value) -> Reported {
        let (summary, abandon) = match validate(arguments) {
            Ok(valid) => valid,
            Err(text) => return Reported::Answer(text, true, false),
        };
        let Some((name, phase)) = &mut self.active else {
            return Reported::Answer(NO_ACTIVE_COMMAND.into(), false, false);
        };
        match phase {
            Phase::Waiting => {
                let text = format!(
                    "/{name} is waiting for the user's reply. Report only once they have answered."
                );
                return Reported::Answer(text, false, false);
            }
            Phase::Completed(_) => {
                return Reported::Answer(ALREADY_REPORTED.into(), false, false);
            }
            Phase::Running { .. } => {}
        }
        if abandon {
            return Reported::Abandon(Abandon {
                command: name.clone(),
                summary: summary.to_owned(),
            });
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
        let text = match next.filter(|step| parse_slash(&step.message).is_some()) {
            Some(step) => format!("→ {}", step.message.trim()),
            None => "status: completed".to_owned(),
        };
        *phase = Phase::Completed(Report {
            next_steps,
            recommended,
        });
        Reported::Answer(text, false, true)
    }

    /// The user's answer to `Abandon` (its `choices`; `None`: dismissed): what the model
    /// receives, whether it failed, and whether the turn ends with it.
    pub fn abandon(&mut self, abandon: &Abandon, chosen: Option<usize>) -> (String, bool, bool) {
        let command = &abandon.command;
        match chosen {
            Some(0) => {
                self.active = None;
                let text = format!(
                    "The user confirmed: /{command} is abandoned, and no next step will run."
                );
                (text, false, true)
            }
            Some(_) => {
                self.answered();
                let text =
                    format!("The user chose to keep /{command} running: go on with its work.");
                (text, false, false)
            }
            None => {
                self.wait();
                let text = format!(
                    "The user dismissed the question: /{command} is waiting for their reply."
                );
                (text, false, true)
            }
        }
    }

    /// After a turn: what follows it; `autopilot` as `/autopilot` is, which the status check
    /// tells the model. A command no longer installed (a resumed session's, or one `/reload`
    /// removed) ends there, as in Scramjet.
    pub fn turn_ended(&mut self, commands: &Commands, autopilot: bool) -> Ended {
        let Some((name, phase)) = &mut self.active else {
            return Ended::Nothing;
        };
        if commands.get(name).is_none() {
            let notice = format!("/{name} has ended: no command of that name is installed now.");
            self.active = None;
            return Ended::Notice(notice);
        }
        match phase {
            Phase::Running { checks } if *checks >= CHECK_LIMIT => {
                *phase = Phase::Waiting;
                Ended::Notice(format!(
                    "/{name} is waiting for you: it was checked on {CHECK_LIMIT} times in a row \
and did not finish or ask you anything. Reply to continue."
                ))
            }
            Phase::Running { checks } => {
                *checks += 1;
                let policy = commands.get(name).and_then(|command| command.next.as_ref());
                Ended::Probe(probe_message(policy, name, autopilot))
            }
            Phase::Completed(report) => {
                let completed = Completed {
                    command: name.clone(),
                    report: report.clone(),
                };
                self.active = None;
                Ended::Completed(completed)
            }
            Phase::Waiting => Ended::Nothing,
        }
    }
}

/// The summary and whether the call abandons the command, or what the model receives: the
/// schema errors, as Pi lists them, or why a status of Scramjet's is not one here.
fn validate(arguments: &Value) -> Result<(&str, bool), String> {
    let refused = |status: &str| {
        format!(
            "`{status}` is not a status a command reports. While the command runs: if there is \
more you can do, keep working; if you need something from the user, ask with \
`get_scramjet_user_input`, explaining the situation; when this command's part is over, report \
`completed`, even if the task did not succeed; if the user wants to stop before then, report \
`abandoned`, and they are asked to confirm."
        )
    };
    let mut errors = Vec::new();
    let summary = arguments["summary"].as_str().unwrap_or_default();
    if summary.trim().is_empty() {
        errors.push("  - summary: must be a non-empty string with a non-blank character");
    }
    let abandon = match arguments["status"].as_str() {
        Some("completed") => Some(false),
        Some("abandoned") => Some(true),
        Some(status @ ("blocked" | "incomplete" | "continuing")) => return Err(refused(status)),
        _ => {
            errors.push("  - status: must be completed or abandoned");
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
    match (errors.is_empty(), abandon) {
        (true, Some(abandon)) => Ok((summary, abandon)),
        _ => {
            let received = serde_json::to_string_pretty(arguments).unwrap_or_default();
            Err(format!(
                "Validation failed for tool \"{NAME}\":\n{}\n\nReceived arguments:\n{received}",
                errors.join("\n")
            ))
        }
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

/// The status check, sent as a hidden message when a command's reply ends without a report:
/// keep working, ask or finish, then Scramjet's description of the command's next-step policy.
fn probe_message(policy: Option<&Policy>, command: &str, autopilot: bool) -> String {
    let id = escape_close_tag(command);
    let preamble = format!(
        "Status check for `/{id}`: your last response ended without finishing this command or \
asking the user anything. Pick one:\n\n\
- Keep working. If there is more you can do, do it now.\n\
- Ask the user. If you need something from them, such as a clarification, a decision, permission \
to change scope or to go on while they are still asking questions, or help with something you \
cannot do yourself (re-authenticating, installing a system package), call \
`get_scramjet_user_input`. Explain the situation so that someone without your context understands \
it, and suggest what they could do, even if you are unsure (say so).\n\
- Finish. If this command has done its job, call `report_scramjet_command_status` with \
`completed`. Completed means this command's part is over, not that the overall task succeeded. If \
you found it cannot succeed and no useful work you are authorized to do would change that, finish \
honestly: say what happened in your answer, and offer next steps only as the guidance below \
allows. If the user wants to stop before this command's part is over, report `abandoned` instead; \
the user is asked to confirm."
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
    fn status_checks_keep_scramjets_next_step_guidance() {
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
        // What follows the choices, as Scramjet's status check ends.
        let guidance = |check: &str| {
            let at = (check.find("\n\n<scramjet-next-step>"))
                .or(check.find("\n\nThis command has no next-step policy."))
                .unwrap();
            check[at..].to_owned()
        };
        for (key, policy, command) in [
            ("closed", Some(&closed), "rs:loop"),
            ("open", Some(&open), "rs:o"),
            ("forced", Some(&forced), "rs:f"),
            ("ask", Some(&ask), "rs:a"),
            ("none", None, "rs:n"),
        ] {
            let check = probe_message(policy, command, false);
            let expected = guidance(reference[key].as_str().unwrap());
            assert_eq!(guidance(&check), expected, "{key}");
            assert!(check.starts_with(&format!("Status check for `/{command}`: ")));
            for choice in ["Keep working.", "Ask the user.", "Finish."] {
                assert!(check.contains(&format!("\n- {choice} ")), "{key}: {choice}");
            }
            let preamble = &check[..check.len() - expected.len()];
            for gone in ["continuing", "blocked", "incomplete", "dormant", "park"] {
                assert!(!preamble.contains(gone), "{key}: {gone}");
            }
        }
    }

    #[test]
    fn a_saved_command_is_restored_waiting_for_the_user() {
        let commands = commands();
        let restored = |line: Value| Lifecycle::restored(Some(&line));
        let mut lifecycle = Lifecycle::default();
        assert_eq!(
            lifecycle.saved(),
            json!({"type":"command_state","command":null})
        );
        assert_eq!(Lifecycle::restored(Some(&lifecycle.saved())).active, None);
        assert_eq!(Lifecycle::restored(None).active, None);
        lifecycle.prompted(Prompt::Command("rs:loop"));
        let started = json!({"type":"command_state","command":"rs:loop"});
        assert_eq!(lifecycle.saved(), started);
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Probe(_)
        ));
        assert_eq!(lifecycle.saved(), started);
        let completed = json!({"summary":"Done.","status":"completed"});
        lifecycle.report(&completed);
        assert_eq!(lifecycle.saved(), started);

        // Working, waiting or reported, and older lines' states: waiting, which a reply resumes.
        let older = ["parked", "resumable", "dormant"]
            .map(|state| json!({"type":"command_state","command":"rs:loop","state":state}));
        for line in [lifecycle.saved()].into_iter().chain(older) {
            let mut resumed = restored(line);
            assert_eq!(resumed.active(), Some("rs:loop"));
            assert_eq!(resumed.turn_ended(&commands, false), Ended::Nothing);
            let Reported::Answer(text, false, false) = resumed.report(&completed) else {
                panic!("a waiting command took a report");
            };
            assert_eq!(
                text,
                "/rs:loop is waiting for the user's reply. Report only once they have answered."
            );
            resumed.prompted(Prompt::Reply);
            assert!(matches!(
                resumed.turn_ended(&commands, false),
                Ended::Probe(_)
            ));
        }
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Completed(_)
        ));
        assert_eq!(restored(lifecycle.saved()).active, None);
    }

    #[test]
    fn a_command_runs_until_it_completes_is_abandoned_or_left() {
        let commands = commands();
        let answer = |text: &str, failed, ends| Reported::Answer(text.to_owned(), failed, ends);
        let completed = json!({"summary":"Did it.","status":"completed"});
        let mut lifecycle = Lifecycle::default();
        assert_eq!(
            lifecycle.report(&completed),
            answer(NO_ACTIVE_COMMAND, false, false)
        );

        // Status checks: at most three in a row without the user's input, then it waits.
        lifecycle.prompted(Prompt::Command("rs:loop"));
        let ended = |lifecycle: &mut Lifecycle, count| {
            (0..count)
                .map(|_| match lifecycle.turn_ended(&commands, false) {
                    Ended::Probe(check) => {
                        let command = lifecycle.active().unwrap();
                        assert!(check.starts_with(&format!("Status check for `/{command}`: ")));
                        "check".to_owned()
                    }
                    Ended::Notice(notice) => notice,
                    other => format!("{other:?}"),
                })
                .collect::<Vec<_>>()
        };
        let waiting = "/rs:loop is waiting for you: it was checked on 3 times in a row and did \
not finish or ask you anything. Reply to continue.";
        assert_eq!(
            ended(&mut lifecycle, 5),
            ["check", "check", "check", waiting, "Nothing"]
        );
        // A reply resumes it; the user's input, a reply or an answer, counts the checks again.
        lifecycle.prompted(Prompt::Reply);
        assert_eq!(ended(&mut lifecycle, 2), ["check", "check"]);
        lifecycle.prompted(Prompt::Reply);
        assert_eq!(ended(&mut lifecycle, 2), ["check", "check"]);
        lifecycle.answered();
        assert_eq!(
            ended(&mut lifecycle, 4),
            ["check", "check", "check", waiting]
        );

        // Scramjet's other statuses are refused, as errors, and change nothing.
        lifecycle.prompted(Prompt::Reply);
        for status in ["blocked", "incomplete", "continuing"] {
            let Reported::Answer(text, true, false) =
                lifecycle.report(&json!({"summary":"Stuck.","status":status}))
            else {
                panic!("{status} was not refused");
            };
            assert!(text.starts_with(&format!("`{status}` is not a status a command reports.")));
            assert!(text.contains("keep working") && text.contains("`abandoned`"));
        }
        assert_eq!(ended(&mut lifecycle, 1), ["check"]);

        // Completed: its next steps are routed when the turn ends; a reply meanwhile keeps it.
        let with_next = json!({"summary":"Did it.","status":"completed","recommended_next_step":1,
            "next_steps":[{"message":"text","fresh_session":false,"reason":"r"},
                {"message":" /rs:steer now ","fresh_session":true,"reason":"r"}]});
        assert_eq!(
            lifecycle.report(&with_next),
            answer("→ /rs:steer now", false, true)
        );
        assert_eq!(
            lifecycle.report(&completed),
            answer(ALREADY_REPORTED, false, false)
        );
        lifecycle.prompted(Prompt::Reply);
        lifecycle.answered();
        assert!(matches!(
            lifecycle.turn_ended(&commands, false),
            Ended::Completed(_)
        ));
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);

        // Abandoned, for the user to confirm: kept running, waiting, or ended without even a
        // forced next step.
        lifecycle.prompted(Prompt::Command("rs:forced"));
        let abandoned = json!({"summary":"Half done.\n","status":"abandoned",
            "next_steps":[{"message":"/rs:steer","fresh_session":false}]});
        let Reported::Abandon(abandon) = lifecycle.report(&abandoned) else {
            panic!("not asked");
        };
        assert_eq!(abandon.title(), "Abandon /rs:forced?");
        assert_eq!(
            abandon.message(),
            "Half done.\n\nAbandoning ends `/rs:forced` now. No next step will run."
        );
        assert_eq!(
            lifecycle.abandon(&abandon, Some(1)),
            (
                "The user chose to keep /rs:forced running: go on with its work.".into(),
                false,
                false
            )
        );
        assert_eq!(ended(&mut lifecycle, 1), ["check"]);
        assert_eq!(
            lifecycle.abandon(&abandon, None),
            (
                "The user dismissed the question: /rs:forced is waiting for their reply.".into(),
                false,
                true
            )
        );
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);
        lifecycle.prompted(Prompt::Reply);
        assert!(matches!(lifecycle.report(&abandoned), Reported::Abandon(_)));
        assert_eq!(
            lifecycle.abandon(&abandon, Some(0)),
            (
                "The user confirmed: /rs:forced is abandoned, and no next step will run.".into(),
                false,
                true
            )
        );
        assert_eq!(lifecycle.active(), None);
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);

        // A question for the user leaves it waiting, dropping a report; an unknown slash
        // command leaves it.
        lifecycle.prompted(Prompt::Command("rs:loop"));
        lifecycle.report(&completed);
        assert!(lifecycle.wait());
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);
        lifecycle.prompted(Prompt::Unknown);
        assert_eq!(lifecycle.active(), None);
        assert!(!lifecycle.wait());

        // A cancelled or failed turn leaves it waiting, dropping a report, and says so.
        assert_eq!(lifecycle.interrupted(), None);
        lifecycle.prompted(Prompt::Command("rs:loop"));
        assert_eq!(
            lifecycle.interrupted().as_deref(),
            Some("/rs:loop is waiting for you. Reply to continue.")
        );
        lifecycle.prompted(Prompt::Reply);
        lifecycle.report(&completed);
        assert_eq!(
            lifecycle.interrupted().as_deref(),
            Some(
                "/rs:loop is waiting for you; its completed report was dropped. Reply to continue."
            )
        );
        assert_eq!(lifecycle.turn_ended(&commands, false), Ended::Nothing);

        let invalid = json!({"summary":" ","status":"done","next_steps":[{"message":1}],
            "recommended_next_step":-1});
        let Reported::Answer(text, true, false) = lifecycle.report(&invalid) else {
            panic!("invalid arguments taken");
        };
        let received = serde_json::to_string_pretty(&invalid).unwrap();
        let errors = [
            "summary: must be a non-empty string with a non-blank character",
            "status: must be completed or abandoned",
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
        lifecycle.prompted(Prompt::Command(command));
        let mut report = report;
        report["summary"] = json!("Done.");
        report["status"] = json!("completed");
        assert!(matches!(
            lifecycle.report(&report),
            Reported::Answer(_, false, true)
        ));
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
