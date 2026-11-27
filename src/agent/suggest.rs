//! A `handoff` with no command running, as Scramjet's `suggest_scramjet_next_steps` tool
//! (`suggest-next-steps.ts`): the agent hands off what to run next, which the user chooses from the
//! next-step list once the turn ends, or dismisses.

use super::commands::Commands;
use super::status::{self, Step};
use serde_json::Value;

/// Among the guidelines.
pub(super) const GUIDELINE: &str = "- When idle in an interactive session, prefer `handoff` \
with `next_steps` when the user explicitly asks for the next Scramjet command or when you \
independently recommend a concrete registered command at an idle natural pause. Concise \
supporting prose is allowed, but do not leave an otherwise eligible actionable command as \
prose-only text. Treat the available command catalog as the authority for command names and \
required arguments.";

/// Scramjet's schema allows at most this many steps.
const MAX_STEPS: usize = 3;

/// Steps the agent suggested, for the user to choose from when the turn ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggested {
    pub steps: Vec<Step>,
    /// A position in `steps`.
    pub recommended: Option<usize>,
}

/// What the agent suggested in the turn running or last run.
#[derive(Debug, Default)]
pub(super) enum Pending {
    #[default]
    Nothing,
    /// The latest suggestion, to offer when the turn ends.
    Steps(Suggested),
    /// A freetext question waits for the user's reply: nothing is offered until they give one,
    /// as in Scramjet.
    Reply,
}

/// The call line's text: the step recommended, else the first.
pub(super) fn summary(arguments: &Value) -> String {
    preview(arguments).to_owned()
}

fn preview(arguments: &Value) -> &str {
    shown(arguments)
        .and_then(|step| step["message"].as_str())
        .unwrap_or_default()
}

fn shown(arguments: &Value) -> Option<&Value> {
    let steps = arguments["next_steps"].as_array();
    let at = arguments["recommended_next_step"].as_u64().unwrap_or(0) as usize;
    steps.and_then(|steps| steps.get(at).or(steps.first()))
}

/// Runs a call, as Scramjet's tool does: the steps to offer and what the model receives, or what
/// it receives instead and whether that is a failure. `shows`: the interface can offer them;
/// `pending`: what was suggested earlier in the turn.
pub(super) fn run(
    arguments: &Value,
    commands: &Commands,
    shows: bool,
    pending: &Pending,
) -> Result<(Suggested, String), (String, bool)> {
    if arguments
        .get("status")
        .is_some_and(|status| !status.is_null())
    {
        return Err((status::NO_ACTIVE_COMMAND.into(), false));
    }
    let mut errors = Vec::new();
    match &arguments["next_steps"] {
        Value::Null => errors.push("  - next_steps: is required"),
        Value::Array(steps) if steps.is_empty() => {
            errors.push("  - next_steps: must have at least 1 item")
        }
        Value::Array(steps) if steps.len() > MAX_STEPS => {
            errors.push("  - next_steps: must have at most 3 items")
        }
        _ => {}
    }
    status::next_steps_errors(arguments, &mut errors);
    if !errors.is_empty() {
        return Err((
            status::validation_failed(status::NAME, &errors, arguments),
            true,
        ));
    }
    if !shows {
        let text = "With no command running, a handoff needs an interactive session to offer its \
steps in, and this one is not: write them as text instead.";
        return Err((text.into(), false));
    }
    if let Pending::Reply = pending {
        let text = "A freetext reply is pending — wait for the user to respond before suggesting \
next steps.";
        return Err((text.into(), false));
    }
    let (steps, recommended) = status::suggested(arguments, commands)
        .map_err(|reason| (format!("Suggestion rejected: {reason}"), false))?;
    let fresh = shown(arguments).is_some_and(|step| step["fresh_session"] == true);
    let runs = status::runs_in(fresh);
    let text = format!(
        "Handed off: the user sees the list of next steps with {}{runs} recommended, and accepts \
or dismisses it. The turn ends here.",
        preview(arguments)
    );
    Ok((Suggested { steps, recommended }, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn commands() -> Commands {
        Commands::parsed(&[
            ("rs:loop.md", "Loop"),
            ("rs:steer.md", "Steer"),
            ("rs:helper.md", "---\ndelegate-only: true\n---\nHelp"),
        ])
    }

    fn step(message: &str, reason: Option<&str>) -> Value {
        let mut step = json!({"message":message,"fresh_session":false});
        if let Some(reason) = reason {
            step["reason"] = json!(reason);
        }
        step
    }

    #[test]
    fn a_suggestion_is_checked_as_scramjets_tool_checks_it() {
        let commands = commands();
        let run = |arguments: &Value, shows| run(arguments, &commands, shows, &Pending::Nothing);
        let arguments = json!({"next_steps":[
            step("/rs:helper", Some("Help.")),
            step("/rs:loop 5", Some("Go on.")),
            step("Tell me more", Some("Explain.")),
        ],"recommended_next_step":1});
        let (suggested, text) = run(&arguments, true).unwrap();
        assert_eq!(
            text,
            "Handed off: the user sees the list of next steps with /rs:loop 5, in this session \
recommended, and accepts or dismisses it. The turn ends here."
        );
        // A delegate-only command is left out; a text step goes into the editor when chosen.
        let shown: Vec<_> = (suggested.steps.iter())
            .map(|step| (step.index, step.command.as_deref()))
            .collect();
        assert_eq!(shown, [(1, Some("/rs:loop 5")), (2, None)]);
        assert_eq!(suggested.recommended, Some(0));
        assert_eq!(summary(&arguments), "/rs:loop 5");

        let fresh = json!({"next_steps":[{"message":"/rs:loop","fresh_session":true,
            "reason":"Go on."}]});
        assert!((run(&fresh, true).unwrap().1).contains("/rs:loop, in a new session recommended"),);

        // Printed output cannot show the list; with no command, there is no status to give.
        let (text, failed) = run(&arguments, false).unwrap_err();
        assert!(
            !failed && text.contains("needs an interactive session"),
            "{text}"
        );
        let mut completed = arguments.clone();
        completed["status"] = json!("completed");
        assert_eq!(
            run(&completed, true).unwrap_err(),
            (status::NO_ACTIVE_COMMAND.into(), false)
        );

        // With no step to offer, the first reason; an unknown command lists those there are.
        let unknown =
            json!({"next_steps":[step("/rs:nope", Some("Why.")), step("/rs:loop", None)]});
        assert_eq!(
            run(&unknown, true).unwrap_err(),
            (
                "Suggestion rejected: unknown command \"rs:nope\" — available: /rs:loop, /rs:steer"
                    .into(),
                false
            )
        );
        let reasonless = json!({"next_steps":[step("/rs:loop", Some(" "))]});
        assert_eq!(
            run(&reasonless, true).unwrap_err().0,
            "Suggestion rejected: selector-visible next steps must include reason"
        );

        // Arguments outside the schema fail whatever the session is.
        for (arguments, error) in [
            (json!({}), "next_steps: is required"),
            (
                json!({"next_steps":[]}),
                "next_steps: must have at least 1 item",
            ),
            (
                json!({"next_steps":vec![step("/rs:loop", Some("a")); 4]}),
                "next_steps: must have at most 3 items",
            ),
            (
                json!({"next_steps":[{"message":"/rs:loop"}]}),
                "each entry must have a string message, a boolean fresh_session",
            ),
            (
                json!({"next_steps":[step("/rs:loop", Some("a"))],"recommended_next_step":-1}),
                "recommended_next_step: must be an integer of at least 0",
            ),
        ] {
            let (text, failed) = run(&arguments, false).unwrap_err();
            assert!(failed && text.contains(error), "{text}");
            assert!(text.starts_with("Validation failed for tool \"handoff\""));
        }
    }
}
