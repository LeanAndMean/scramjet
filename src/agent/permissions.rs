//! `request_permissions`, after Codex's: the agent asks the user to make directories writable
//! in the sandbox, with a reason, rather than working around it. Offered only with a sandbox.

use super::sandbox::Sandbox;
use super::tools::Output;
use super::{Agent, AgentUi, Choice, status};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const NAME: &str = "request_permissions";

pub(super) fn definition() -> Value {
    json!({"type":"function","name":NAME,"strict":false,
        "description":"Ask the user to make directories writable in the sandbox, for the rest \
of the session. Use it when the task needs to change files outside what the sandbox lets you \
write, instead of working around the sandbox. The user sees the directories and your reason, and \
may decline.",
        "parameters":{"type":"object","properties":{
            "write":{"type":"array","items":{"type":"string"},"minItems":1,
                "description":"Directories to make writable, absolute or relative to the \
project; each covers what is under it."},
            "reason":{"type":"string",
                "description":"Why the task needs to write there, in one sentence for the user."}},
          "required":["write","reason"]}})
}

/// The call line's text.
pub(super) fn summary(arguments: &Value) -> String {
    let listed = (arguments["write"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    format!("write {}", listed.join(", "))
}

/// A call's directories and reason, or what is wrong with them.
fn given(arguments: &Value) -> Result<(Vec<&str>, &str), Vec<String>> {
    let mut errors = Vec::new();
    let write: Vec<&str> = (arguments["write"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .collect();
    let all_strings = (arguments["write"].as_array())
        .is_some_and(|listed| listed.len() == write.len() && !listed.is_empty());
    if !all_strings {
        errors.push("write: must be a non-empty array of directory paths".to_owned());
    }
    let reason = arguments["reason"].as_str().map(str::trim).unwrap_or("");
    if reason.is_empty() {
        errors.push("reason: must be a non-empty string".to_owned());
    }
    match errors.is_empty() {
        true => Ok((write, reason)),
        false => Err(errors),
    }
}

/// What the agent's instructions say of the sandbox while one confines its tools: what is
/// writable, what stays read-only in it, the network, and that it can ask for more.
pub(super) fn section(sandbox: &Sandbox, root: &Path) -> String {
    let listed = |paths: &[PathBuf]| {
        (paths.iter().map(|path| path.display().to_string()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let roots = sandbox.writable_roots(root);
    let mut text = match roots.is_empty() {
        true => format!(
            "\n\n# Sandbox\n\nYour tools run in a sandbox ({}): write and edit, and the commands \
you run, can change no file but in a private /tmp discarded after each command.",
            sandbox.mode.name()
        ),
        false => format!(
            "\n\n# Sandbox\n\nYour tools run in a sandbox ({}): write and edit, and the commands \
you run, can change files only in these directories (and a private /tmp discarded after each \
command): {}.",
            sandbox.mode.name(),
            listed(&roots)
        ),
    };
    let kept = sandbox.protected_paths(root);
    if !kept.is_empty() {
        text += &format!(" Kept read-only within them: {}.", listed(&kept));
    }
    if let Some(project) = (sandbox.read_only_project(root)).filter(|_| !roots.is_empty()) {
        text += &format!(
            " The project ({}) is read-only outside them.",
            project.display()
        );
    }
    text += match sandbox.network {
        true => " Commands reach the network.",
        false => " Commands have no network.",
    };
    text + &format!(
        "\nWhen the task needs to write elsewhere, ask the user with `{NAME}`, naming the \
directories and why; do not work around the sandbox."
    )
}

impl Agent {
    /// Runs a `request_permissions` call: the directories not yet writable are granted for the
    /// rest of the session if the user allows it. Refused when no one can answer: an unattended
    /// turn, or no sandbox.
    pub(super) fn request_permissions(
        &mut self,
        arguments: &Value,
        ui: &mut dyn AgentUi,
    ) -> Output {
        let output = |text: String, failed: bool| Output {
            text,
            detail: None,
            failed,
            image: None,
        };
        let (write, reason) = match given(arguments) {
            Ok(given) => given,
            Err(errors) => {
                let errors: Vec<&str> = errors.iter().map(String::as_str).collect();
                return output(status::validation_failed(NAME, &errors, arguments), true);
            }
        };
        let Some(sandbox) = self.config.sandbox.clone() else {
            let text = "No sandbox confines your tools: they can already write wherever the user \
can.";
            return output(text.to_owned(), false);
        };
        let root = self.config.root.clone();
        let mut with = sandbox.clone();
        let mut asked = Vec::new();
        let mut notes = Vec::new();
        for path in &write {
            match with.grant(&root, path) {
                Ok(real) => asked.push(real),
                Err(error) => notes.push(error),
            }
        }
        let notes = notes.join("\n");
        if asked.is_empty() {
            return output(format!("Nothing to ask the user for:\n{notes}"), true);
        }
        let listed = (asked.iter().map(|path| path.display().to_string()))
            .collect::<Vec<_>>()
            .join(", ");
        if self.unattended {
            return output(
                format!(
                    "Not asked: this turn runs unattended (a step run by autopilot, or -p), so \
no one can answer. {listed} stay read-only. Tell the user which directories you need writable \
and why; they can grant them with /add-dir DIR, or start with --add-dir DIR."
                ),
                true,
            );
        }
        let mut message = "**The agent asks to write in:**\n\n".to_owned();
        for path in &asked {
            message += &format!("- `{}`\n", path.display());
        }
        message += &format!("\n**Why:** {reason}");
        if !notes.is_empty() {
            message += &format!("\n\nNot asked: {}", notes.replace('\n', "; "));
        }
        ui.question(&message);
        let choices = [
            (
                "Allow for this session",
                "/sandbox remove DIR takes it back",
            ),
            ("Deny", "they stay read-only"),
        ]
        .map(|(label, description)| Choice {
            label: label.into(),
            description: Some(description.into()),
            value: label.into(),
        });
        let allowed = ui.choose("Allow writing there?", &choices, None) == Some(0);
        let mut text = match allowed {
            true => {
                self.set_sandbox(Some(with));
                format!("The user allowed writing in {listed} for the rest of the session.")
            }
            false => format!(
                "The user declined: {listed} stay read-only. Do not write there another way; \
work within the writable directories or ask the user how to go on."
            ),
        };
        if !notes.is_empty() {
            text += &format!("\nNot asked:\n{notes}");
        }
        output(text, !allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_needs_directories_and_a_reason() {
        let ok = json!({"write":["../other"],"reason":"The fix spans both repositories."});
        assert_eq!(
            given(&ok).unwrap(),
            (vec!["../other"], "The fix spans both repositories.")
        );
        assert_eq!(summary(&ok), "write ../other");
        let errors = given(&json!({"write":[],"reason":" "})).unwrap_err();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(given(&json!({"write":["a", 3],"reason":"r"})).is_err());
        assert!(given(&json!({"write":"a","reason":"r"})).is_err());
    }

    #[test]
    fn the_instructions_say_what_is_writable() {
        let root =
            std::env::temp_dir().join(format!("scramjet-permissions-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let mut sandbox = Sandbox {
            mode: super::super::SandboxMode::ReadOnly,
            network: false,
            writable: Vec::new(),
        };
        let text = section(&sandbox, &root);
        assert!(
            text.contains(
                "(read-only): write and edit, and the commands you run, can change \
no file but in a private /tmp"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "Commands have no network.\nWhen the task needs to write elsewhere, \
ask the user with `request_permissions`"
            ),
            "{text}"
        );
        sandbox.grant(&root, "docs").unwrap();
        let text = section(&sandbox, &root);
        let docs = root.join("docs");
        let project = root.display();
        assert!(
            text.contains(&format!(
                "command): {}. The project ({project}) is read-only outside \
them.",
                docs.display()
            )),
            "{text}"
        );
        sandbox.mode = super::super::SandboxMode::WorkspaceWrite;
        sandbox.network = true;
        let text = section(&sandbox, &root);
        let kept = root.join(".git").display().to_string();
        assert!(
            text.contains(&format!(
                ": {project}, {}. Kept read-only within them: {kept}. \
Commands reach the network.",
                docs.display()
            )),
            "{text}"
        );
    }
}
