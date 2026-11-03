//! Scramjet's `delegate` tool (`delegate.ts`): a command runs another command as a subroutine.
//! The result is the other command's body with the arguments substituted, which the model
//! follows in the same conversation.

use super::{commands::Commands, tools::Output};
use serde_json::{Value, json};

pub const NAME: &str = "delegate";

/// How a result starts when the caller's and the command's `allowed-tools` share no tool.
pub const SCOPE_WARNING: &str = "[scramjet/delegate] WARNING:";

/// Scramjet's call line cuts the arguments after this many characters.
const SHOWN_ARGS: usize = 60;

pub(super) fn definition() -> Value {
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Invoke another scramjet-registered command as a subroutine. The command's \
body is returned as text in this tool's result; read it and follow its instructions inside the \
same conversation context. The `args` string is substituted into $ARGUMENTS, $@, and $1-$N \
within the body (plus bash-style slicing), mirroring Pi's slash-command argument expansion. \
Cycle detection rejects delegating a command again with the same arguments in this turn.",
        "parameters":{"type":"object","properties":{
            "command":{"type":"string",
                "description":"The qualified command name to invoke, e.g. 'mach12:push'"},
            "args":{"type":"string",
                "description":"Argument string (bash-style: whitespace-split, single/double \
quotes group). Pass \"\" for no arguments."}},
          "required":["command","args"]}})
}

/// The call line's text, as Scramjet's: `/<command> <args>`, the arguments on one line and cut.
pub(super) fn summary(arguments: &Value) -> String {
    let args = arguments["args"].as_str().unwrap_or_default();
    let mut args = args.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((at, _)) = args.char_indices().nth(SHOWN_ARGS) {
        args.truncate(at);
        args.push('…');
    }
    match arguments["command"]
        .as_str()
        .filter(|name| !name.is_empty())
    {
        Some(name) if args.is_empty() => format!("/{name}"),
        Some(name) => format!("/{name} {args}"),
        None => args,
    }
}

/// Runs a `delegate` call while `active` is the active command. `delegated` holds the commands
/// delegated earlier in the turn with their arguments: as Scramjet's cycle detection, one of
/// them is refused again, but only with the same arguments, so that siblings such as an issue
/// read for each linked issue run.
pub(super) fn run(
    arguments: &Value,
    commands: &Commands,
    active: Option<&str>,
    delegated: &mut Vec<(String, String)>,
) -> Output {
    let refused = |text: String| Output {
        text,
        detail: None,
        failed: true,
    };
    let (name, args) = match given(arguments) {
        Ok(given) => given,
        Err(errors) => {
            let received = serde_json::to_string_pretty(arguments).unwrap_or_default();
            return refused(format!(
                "Validation failed for tool \"{NAME}\":\n{}\n\nReceived arguments:\n{received}",
                errors.join("\n")
            ));
        }
    };
    let Some(command) = commands.get(name) else {
        return refused(format!(
            "ERROR: unknown command '{name}'. Check the registry or fix the name."
        ));
    };
    if delegated
        .iter()
        .any(|(earlier, with)| earlier == name && with == args)
    {
        let chain: Vec<&str> = (delegated.iter().map(|(earlier, _)| earlier.as_str()))
            .chain([name])
            .collect();
        return refused(format!(
            "ERROR: cycle detected in delegation chain {}: '{name}' was already delegated with \
these arguments in this turn. Refusing to recurse; its instructions are in that earlier result.",
            chain.join(" -> ")
        ));
    }
    delegated.push((name.to_owned(), args.to_owned()));
    let caller = active.and_then(|active| commands.get(active));
    let scopes = (caller.and_then(|caller| caller.allowed_tools.as_ref()))
        .zip(command.allowed_tools.as_ref());
    let body = command.body_with(args);
    let text = match scopes {
        Some((caller, own)) if !own.iter().any(|tool| caller.contains(tool)) => format!(
            "{SCOPE_WARNING} effective allowed-tools scope for '{name}' is empty (caller and \
callee scopes do not overlap). Tool calls are not blocked; consider widening the caller's scope \
or aborting the delegation.\n\n{body}"
        ),
        _ => body,
    };
    Output {
        text: command.framed(text),
        detail: None,
        failed: false,
    }
}

/// The command and arguments of a call, or what is wrong with them as Pi's validation says it.
fn given(arguments: &Value) -> Result<(&str, &str), Vec<String>> {
    let field = |key: &str| match arguments.get(key) {
        None => Err(format!("  - {key}: must have required property")),
        Some(Value::String(text)) => Ok(text.as_str()),
        Some(_) => Err(format!("  - {key}: must be string")),
    };
    match (field("command"), field("args")) {
        (Ok(name), Ok(args)) => Ok((name, args)),
        (name, args) => Err([name.err(), args.err()].into_iter().flatten().collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> Commands {
        Commands::parsed(&[
            (
                "rs:top.md",
                "---\nallowed-tools:\n  - bash\n  - delegate\n---\nTop",
            ),
            (
                "rs:read.md",
                "---\ndelegate-only: true\nallowed-tools: bash, read\n---\nRead <caller-context>$1 \
then ${@:2}</caller-context>",
            ),
            (
                "rs:edit.md",
                "---\nallowed-tools: [edit]\n---\nEdit $ARGUMENTS",
            ),
            (
                "rs:framed.md",
                "<scramjet-command name=\"x\">\nAs is\n</scramjet-command>",
            ),
        ])
    }

    fn call(command: &str, args: &str) -> Value {
        json!({"command":command,"args":args})
    }

    #[test]
    fn a_delegated_command_returns_its_body_framed_with_the_arguments() {
        let commands = commands();
        let mut delegated = Vec::new();
        let run = |arguments: Value, delegated: &mut Vec<_>| {
            let output = run(&arguments, &commands, Some("rs:top"), delegated);
            (output.text, output.failed)
        };
        // Delegate-only commands run here, which is what they are for.
        assert_eq!(
            run(call("rs:read", "'a b' c d"), &mut delegated),
            (
                "<scramjet-command name=\"rs:read\">\nRead <caller-context>a b then c \
d</caller-context>\n</scramjet-command>"
                    .into(),
                false
            )
        );
        // A sibling with other arguments runs; the same arguments again are a cycle.
        assert!(!run(call("rs:read", "other"), &mut delegated).1);
        assert_eq!(
            run(call("rs:read", "'a b' c d"), &mut delegated),
            (
                "ERROR: cycle detected in delegation chain rs:read -> rs:read -> rs:read: \
'rs:read' was already delegated with these arguments in this turn. Refusing to recurse; its \
instructions are in that earlier result."
                    .into(),
                true
            )
        );
        assert_eq!(delegated.len(), 2);
        // A body that is already framed is returned as it is.
        assert_eq!(
            run(call("rs:framed", ""), &mut delegated).0,
            "<scramjet-command name=\"x\">\nAs is\n</scramjet-command>"
        );
        assert_eq!(
            run(call("rs:nope", ""), &mut delegated),
            (
                "ERROR: unknown command 'rs:nope'. Check the registry or fix the name.".into(),
                true
            )
        );
        let received = serde_json::to_string_pretty(&json!({"command":3})).unwrap();
        assert_eq!(
            run(json!({"command":3}), &mut delegated),
            (
                format!(
                    "Validation failed for tool \"delegate\":\n  - command: must be string\n  - \
args: must have required property\n\nReceived arguments:\n{received}"
                ),
                true
            )
        );
        assert_eq!(delegated.len(), 3);
    }

    #[test]
    fn scopes_that_share_no_tool_are_warned_about() {
        let commands = commands();
        let text = |active: Option<&str>| {
            let output = run(&call("rs:edit", "x"), &commands, active, &mut Vec::new());
            output.text
        };
        assert_eq!(
            text(Some("rs:top")),
            "<scramjet-command name=\"rs:edit\">\n[scramjet/delegate] WARNING: effective \
allowed-tools scope for 'rs:edit' is empty (caller and callee scopes do not overlap). Tool calls \
are not blocked; consider widening the caller's scope or aborting the delegation.\n\nEdit \
x\n</scramjet-command>"
        );
        // Without a caller's scope, or with one that overlaps, there is nothing to warn about.
        let plain = "<scramjet-command name=\"rs:edit\">\nEdit x\n</scramjet-command>";
        assert_eq!(text(None), plain);
        assert_eq!(text(Some("rs:edit")), plain);
        let output = run(
            &call("rs:read", ""),
            &commands,
            Some("rs:top"),
            &mut Vec::new(),
        );
        assert!(!output.text.contains(SCOPE_WARNING));
    }

    #[test]
    fn the_call_line_shows_the_command_and_its_arguments_on_one_line() {
        assert_eq!(summary(&call("mach12:push", "")), "/mach12:push");
        assert_eq!(
            summary(&call("mach12:gh-issue-read", " 42\n  --repo\to/r ")),
            "/mach12:gh-issue-read 42 --repo o/r"
        );
        let long = "é".repeat(70);
        assert_eq!(
            summary(&call("a:b", &long)),
            format!("/a:b {}…", "é".repeat(60))
        );
        assert_eq!(summary(&json!({"args":"x"})), "x");
    }
}
