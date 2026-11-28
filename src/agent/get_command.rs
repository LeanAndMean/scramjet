//! `get_command`: another command's instructions, with its arguments, for the running command to
//! follow. Nothing starts: the running command keeps its lifecycle. It replaced Scramjet's
//! `delegate`, whose calls still run it.

use super::{commands::Commands, tools::Output};
use serde_json::{Value, json};

pub const NAME: &str = "get_command";

/// The name Scramjet 1 gave the tool, which still runs it.
pub(super) const OLD_NAME: &str = "delegate";

/// The tag its instructions come in, apart from `<scramjet-command>`, which starts a command.
pub const FRAME: &str = "scramjet-command-instructions";

/// Scramjet's call line cuts the arguments after this many characters.
const SHOWN_ARGS: usize = 60;

pub(super) fn definition() -> Value {
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Get another command's instructions, with its arguments, to follow as part \
of the command now running. Nothing starts: the running command keeps going, and you still hand \
off when its part is over. Only while a command runs: to run a command, hand it off (`handoff`); \
to run an agent, use `subagent`. The `args` string is substituted into $ARGUMENTS, $@, and $1-$N \
within the instructions (plus bash-style slicing). Getting the same command with the same \
arguments again in a turn is refused: its instructions are in the earlier result.",
        "parameters":{"type":"object","properties":{
            "command":{"type":"string",
                "description":"The command's qualified name, e.g. 'mach12:gh-issue-read'"},
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

/// Runs a `get_command` call while `active` is the running command. `retrieved` holds the
/// commands retrieved earlier in the turn with their arguments: one of them is refused again, but
/// only with the same arguments, so that siblings such as an issue read for each linked issue
/// run. `is_agent` tells whether a name that is no command is an agent's.
pub(super) fn run(
    arguments: &Value,
    commands: &Commands,
    active: Option<&str>,
    retrieved: &mut Vec<(String, String)>,
    is_agent: &dyn Fn(&str) -> bool,
) -> Output {
    let refused = |text: String| Output {
        text,
        detail: None,
        failed: true,
        images: Vec::new(),
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
        if is_agent(name) {
            return refused(format!(
                "ERROR: '{name}' is an agent, not a command: give it a task with `subagent`."
            ));
        }
        return refused(format!(
            "ERROR: unknown command '{name}'. Check the registry or fix the name."
        ));
    };
    let Some(active) = active else {
        return refused(format!(
            "ERROR: no command is running, so there is none to get instructions for. To run \
/{name}, hand it off: `handoff` with it as a next step, which the user accepts. To read it, its \
file is {}.",
            command.path.display()
        ));
    };
    if retrieved
        .iter()
        .any(|(earlier, with)| earlier == name && with == args)
    {
        return refused(format!(
            "ERROR: '{name}' was already retrieved with these arguments in this turn; its \
instructions are in that earlier result."
        ));
    }
    retrieved.push((name.to_owned(), args.to_owned()));
    let current = command.current();
    let command = current.as_ref().unwrap_or(command);
    Output {
        text: format!(
            "<{FRAME} name=\"{name}\" for=\"{active}\">\n{}\n</{FRAME}>",
            command.body_with(args)
        ),
        detail: None,
        failed: false,
        images: Vec::new(),
    }
}

/// The command and arguments of a call, or what is wrong with them as Pi's validation says it.
pub(super) fn given(arguments: &Value) -> Result<(&str, &str), Vec<String>> {
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
                "---\nallowed-tools:\n  - bash\n  - get_command\n---\nTop",
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
        ])
    }

    fn call(command: &str, args: &str) -> Value {
        json!({"command":command,"args":args})
    }

    fn no_agent(_: &str) -> bool {
        false
    }

    #[test]
    fn a_commands_instructions_come_framed_for_the_running_one_with_the_arguments() {
        let commands = commands();
        let mut retrieved = Vec::new();
        let run = |arguments: Value, retrieved: &mut Vec<_>| {
            let output = run(&arguments, &commands, Some("rs:top"), retrieved, &no_agent);
            (output.text, output.failed)
        };
        // Delegate-only commands are got here, which is what they are for.
        assert_eq!(
            run(call("rs:read", "'a b' c d"), &mut retrieved),
            (
                "<scramjet-command-instructions name=\"rs:read\" for=\"rs:top\">\nRead \
<caller-context>a b then c d</caller-context>\n</scramjet-command-instructions>"
                    .into(),
                false
            )
        );
        // A sibling with other arguments runs; the same arguments again are refused.
        assert!(!run(call("rs:read", "other"), &mut retrieved).1);
        assert_eq!(
            run(call("rs:read", "'a b' c d"), &mut retrieved),
            (
                "ERROR: 'rs:read' was already retrieved with these arguments in this turn; its \
instructions are in that earlier result."
                    .into(),
                true
            )
        );
        assert_eq!(retrieved.len(), 2);
        // Scopes that share no tool are not warned about: `allowed-tools` is the running
        // command's, for publications.
        assert!(
            !run(call("rs:edit", "x"), &mut retrieved)
                .0
                .contains("WARNING")
        );
        assert_eq!(
            run(call("rs:nope", ""), &mut retrieved),
            (
                "ERROR: unknown command 'rs:nope'. Check the registry or fix the name.".into(),
                true
            )
        );
        let received = serde_json::to_string_pretty(&json!({"command":3})).unwrap();
        assert_eq!(
            run(json!({"command":3}), &mut retrieved),
            (
                format!(
                    "Validation failed for tool \"get_command\":\n  - command: must be string\n  \
- args: must have required property\n\nReceived arguments:\n{received}"
                ),
                true
            )
        );
        assert_eq!(retrieved.len(), 3);
    }

    #[test]
    fn with_no_command_running_it_points_to_handoff_and_the_file() {
        let commands = commands();
        let mut retrieved = Vec::new();
        let output = run(
            &call("rs:edit", "x"),
            &commands,
            None,
            &mut retrieved,
            &no_agent,
        );
        assert!(output.failed);
        assert_eq!(
            output.text,
            "ERROR: no command is running, so there is none to get instructions for. To run \
/rs:edit, hand it off: `handoff` with it as a next step, which the user accepts. To read it, its \
file is rs:edit.md."
        );
        assert!(retrieved.is_empty());
    }

    #[test]
    fn an_agents_name_points_to_subagent() {
        let output = run(
            &call("mach12:code-reviewer", ""),
            &commands(),
            Some("rs:top"),
            &mut Vec::new(),
            &|name| name == "mach12:code-reviewer",
        );
        assert!(output.failed);
        assert_eq!(
            output.text,
            "ERROR: 'mach12:code-reviewer' is an agent, not a command: give it a task with \
`subagent`."
        );
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
