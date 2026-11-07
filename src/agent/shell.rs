//! The `!` and `!!` commands the user runs without a model turn.
use super::tools::{self, Output};
use super::{Agent, AgentError, AgentUi, Ending, FAILED, TYPED, UserCommand};
use serde_json::json;
use std::sync::atomic::AtomicBool;

/// A `!` or `!!` command the user ran, which `Agent::add_shell` adds to the conversation: run
/// while a turn runs, it waits for the turn to end, as Scramjet keeps it from coming between a
/// tool call and its result.
pub struct Shell {
    command: String,
    /// Added to the conversation (`!`), or only shown (`!!`).
    include: bool,
    ran: Result<tools::Ran, String>,
}

impl Shell {
    /// Runs `command` with bash in `root` until it ends or `cancel` is set; `progress` receives
    /// the end of its output as it grows.
    pub fn run(
        root: &std::path::Path,
        command: &str,
        include: bool,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Self {
        Self {
            command: command.to_owned(),
            include,
            ran: tools::run_command(root, command, None, cancel, progress),
        }
    }

    /// How it ran, as its block shows it, live and on a resume: from the text kept for it.
    pub fn shown(&self) -> UserCommand {
        match &self.ran {
            Ok(ran) if self.include => {
                let failed = ran.tool_output().failed;
                UserCommand::included(&self.command, &ran.message(&self.command), failed)
            }
            Ok(ran) => {
                let shown = ran.tool_output();
                UserCommand::excluded(&self.command, &shown.text, shown.failed)
            }
            Err(error) => UserCommand::new(
                &self.command,
                self.include,
                "",
                Ending::Failed(error.clone()),
            ),
        }
    }
}

impl Agent {
    /// A `!` or `!!` command the user runs without a model turn: `include` adds it to the
    /// conversation as a user message, as Scramjet's `!` does, and `!!` leaves it out.
    pub fn shell(
        &mut self,
        command: &str,
        include: bool,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        ui.command_started(command, include);
        let shell = Shell::run(&self.config.root, command, include, cancel, &mut |output| {
            ui.tool_output(output)
        });
        self.add_shell(shell, ui)
    }

    /// Shows how `shell`, run apart from the agent, ended, and adds it to the conversation as
    /// `shell` says: for the model (`!`), or only to be shown again on a resume (`!!`).
    pub fn add_shell(&mut self, shell: Shell, ui: &mut dyn AgentUi) -> Result<(), AgentError> {
        ui.command_result(&shell.shown());
        let Ok(ran) = &shell.ran else {
            return Ok(());
        };
        let Output { text, failed, .. } = ran.tool_output();
        let prompt = format!("!{}", shell.command);
        let mut item = match shell.include {
            true => json!({"role":"user","content":ran.message(&shell.command),TYPED:prompt}),
            false => json!({"type":"bang_excluded","command":shell.command,"output":text}),
        };
        if failed {
            item[FAILED] = json!(true);
        }
        self.add(item)?;
        if shell.include {
            self.prompts.push(prompt);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{Session, TreeItem, show_items};
    use crate::provider_fixture as fixture;
    use std::sync::{Arc, atomic::Ordering};
    use std::time::Duration;

    #[test]
    fn bang_commands_run_without_a_turn_and_are_recalled_as_typed() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "ok",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "bang");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        // As Scramjet's `!` gives the model its output, without escape sequences.
        let command = r"printf '\e[1mhi\e[0m\n10%%\r100%%\n'; exit 3";
        let typed = format!("!{command}");
        agent.shell(command, true, &mut ui, &cancel).unwrap();
        agent.shell("echo secret", false, &mut ui, &cancel).unwrap();
        let blocks = [
            format!("!{command} \"hi\\n100%\" Exited(3)"),
            "!!echo secret \"secret\\n\" Done".into(),
        ];
        assert_eq!(
            ui.shown,
            [
                format!("started {command} true"),
                blocks[0].clone(),
                "started echo secret false".into(),
                blocks[1].clone(),
            ]
        );
        assert_eq!(agent.usage.requests, 0);
        let message = format!("Ran `{command}`\n```\nhi\n100%\n```\n\nCommand exited with code 3");
        assert_eq!(
            agent.conversation(),
            [
                json!({"role":"user","content":message,"_scramjet_typed":typed,
                "_scramjet_failed":true}),
                json!({"type":"bang_excluded","command":"echo secret","output":"secret\n"}),
            ]
        );
        assert_eq!(agent.prompts(), [typed.as_str()]);
        agent.turn("next", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        // As Scramjet's `!!`, saved and shown again on a resume, but never sent.
        assert_eq!(
            bodies[0]["input"],
            json!([{"role":"user","content":message},{"role":"user","content":"next"}])
        );
        assert!(!bodies[0].to_string().contains("\"_scramjet_"));
        assert!(!bodies[0].to_string().contains("secret"));
        let path = agent.session_path().to_owned();
        let (_, conversation, prompts) = Session::open(&path).unwrap();
        assert_eq!(prompts, [typed.as_str(), "next"]);
        let mut shown = Recorder::default();
        show_items(&conversation, None, &mut shown);
        // A resume shows them as they ended.
        assert_eq!(shown.shown[..2], blocks);
        let tree = agent.tree().unwrap();
        let items: Vec<_> = tree.entries.iter().map(|entry| &entry.item).collect();
        assert_eq!(items[1], &TreeItem::Command("echo secret".into()));
        assert_eq!((agent.stats().user, agent.stats().total), (1, 3));
        // As Scramjet's session list names it by its first message, not a `!` command.
        let listed = agent.sessions().unwrap();
        assert_eq!(listed[0].first_prompt.as_deref(), Some("next"));
    }

    #[test]
    fn a_cancelled_bang_command_is_recorded_as_cancelled() {
        if crate::agent::tests::missing("bash") {
            return;
        }
        let (endpoint, _server) = serve(vec![]);
        let (mut agent, _scratch) = agent(endpoint, "bang-cancel");
        let mut ui = Recorder::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            setter.store(true, Ordering::Relaxed);
        });
        agent.shell("sleep 30", true, &mut ui, &cancel).unwrap();
        assert_eq!(
            agent.conversation()[0]["content"],
            "Ran `sleep 30`\n(no output)\n\n(command cancelled)"
        );
    }
}
