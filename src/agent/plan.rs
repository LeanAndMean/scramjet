//! Plan mode, as Claude Code's and Codex's `/plan`: the agent explores and proposes a plan
//! without carrying it out. `write` and `edit` are refused, and in a sandbox commands and
//! subagents run read-only; without one, commands are only asked to change nothing. Each prompt
//! in plan mode says so to the model, and the first after it ends says it is over.
use super::tools::Output;
use super::{Agent, TYPED};
use serde_json::{Value, json};

/// What the first prompt in plan mode sends before the prompt, then what commands may do.
const ON: &str = "<system-reminder>Plan mode is on (the user's /plan): explore, think and \
propose a plan, but do not carry it out. Read files, search and run only commands that change \
nothing; `write` and `edit` are refused. Ask the user about what is unclear. End your turn with \
the plan: the steps in order, the files each changes, how the result will be verified, and what \
you are unsure of. The user then chooses to implement it or to keep planning.";

/// What each later prompt in plan mode sends before the prompt.
const STILL: &str = "<system-reminder>Plan mode is still on: change no files and run only \
commands that change nothing. Answer, refine the plan, and end your turn with the plan as it \
stands.</system-reminder>\n\n";

/// What the first prompt after plan mode sends before the prompt.
const OFF: &str = "<system-reminder>Plan mode is off: you may change files and run commands as \
usual again.</system-reminder>\n\n";

impl Agent {
    /// Starts or ends plan mode; its tools change at once, and the next prompt tells the model.
    pub fn set_planning(&mut self, on: bool) {
        self.planning = on;
    }

    pub fn planning(&self) -> bool {
        self.planning
    }

    /// Puts before `message`'s text what the model has not been told yet of plan mode.
    pub(super) fn tell_plan(&mut self, message: &mut Value) {
        let note = match (self.planning, self.plan_told) {
            (true, false) => {
                let commands = match self.config.sandbox {
                    Some(_) => " Commands and subagents run in a read-only sandbox.",
                    None => " Nothing stops a command from writing: keep to those that only look.",
                };
                format!("{ON}{commands}</system-reminder>\n\n")
            }
            (true, true) => STILL.to_owned(),
            (false, true) => OFF.to_owned(),
            (false, false) => return,
        };
        self.plan_told = self.planning;
        if message.get(TYPED).is_none()
            && let Some(prompt) = message["content"].as_str()
        {
            message[TYPED] = json!(prompt);
        }
        match &mut message["content"] {
            Value::String(text) => text.insert_str(0, &note),
            Value::Array(parts) => match parts.first_mut() {
                Some(part) if part["type"] == "input_text" => {
                    let text = part["text"].as_str().unwrap_or_default();
                    part["text"] = json!(format!("{note}{text}"));
                }
                _ => parts.insert(0, json!({"type":"input_text","text":note})),
            },
            _ => {}
        }
    }
}

/// The result of a `write` or `edit` call in plan mode.
pub(super) fn refused(tool: &str) -> Output {
    Output {
        text: format!(
            "Error: plan mode is on, so {tool} is not run and nothing changed. Put the change in \
             the plan instead; the user ends plan mode to implement it."
        ),
        detail: None,
        failed: true,
        images: Vec::new(),
        before: None,
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::TYPED;
    use crate::agent::tests::{Recorder, agent, serve, stream, tool_call};
    use crate::provider_fixture as fixture;
    use serde_json::{Value, json};
    use std::sync::atomic::AtomicBool;

    /// The text of the last user message request `at` sent.
    fn sent(requests: &[Value], at: usize) -> String {
        let input = requests[at]["input"].as_array().unwrap();
        let user = input.iter().rev().find(|item| item["role"] == "user");
        user.unwrap()["content"].as_str().unwrap().to_owned()
    }

    #[test]
    fn plan_mode_refuses_writes_and_tells_the_model_at_each_prompt() {
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let write = json!({"path":"made.txt","content":"x"});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "write", write.clone()),
            text("Plan: make made.txt."),
            text("Still the plan."),
            tool_call("call_2", "write", write),
            text("Done."),
        ]);
        let (mut agent, scratch) = agent(endpoint, "plan-mode");
        let made = scratch.0.join("made.txt");
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.set_planning(true);
        agent.turn("make a file", &mut ui, &cancel).unwrap();
        assert!(!made.exists());
        agent.turn("and then?", &mut ui, &cancel).unwrap();
        agent.set_planning(false);
        agent.turn("implement it", &mut ui, &cancel).unwrap();
        assert!(made.exists());
        let requests = server.join().unwrap();
        let first = sent(&requests, 0);
        assert!(
            first.starts_with("<system-reminder>Plan mode is on"),
            "{first}"
        );
        assert!(first.contains("Nothing stops a command"), "{first}");
        assert!(
            first.ends_with("</system-reminder>\n\nmake a file"),
            "{first}"
        );
        let input = requests[1]["input"].as_array().unwrap();
        let result = input
            .iter()
            .find(|item| item["type"] == "function_call_output");
        let output = result.unwrap()["output"].to_string();
        assert!(
            output.contains("plan mode is on, so write is not run"),
            "{output}"
        );
        let still = sent(&requests, 2);
        assert!(
            still.starts_with("<system-reminder>Plan mode is still on"),
            "{still}"
        );
        let off = sent(&requests, 3);
        assert!(
            off.starts_with("<system-reminder>Plan mode is off"),
            "{off}"
        );
        assert!(off.ends_with("implement it"), "{off}");
        // Each prompt shows and is recalled as typed.
        let typed: Vec<_> = (agent.conversation().iter())
            .filter(|item| item["role"] == "user")
            .map(|item| item[TYPED].as_str().unwrap_or_default())
            .collect();
        let prompts = ["make a file", "and then?", "implement it"];
        assert_eq!(typed, prompts);
        assert_eq!(agent.prompts(), prompts);
        let mut shown = Recorder::default();
        agent.show_conversation(&mut shown);
        let shown: Vec<_> = (shown.shown.iter())
            .filter(|line| line.starts_with("prompt ") || line.starts_with("command "))
            .collect();
        assert_eq!(
            shown,
            prompts
                .map(|prompt| format!("prompt {prompt}"))
                .iter()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn in_plan_mode_a_writing_sandbox_runs_commands_read_only() {
        use crate::agent::{Sandbox, SandboxMode};
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let touch = json!({"command":"touch made.txt"});
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "bash", touch.clone()),
            text("Refused."),
            tool_call("call_2", "bash", touch),
            text("Made."),
        ]);
        let (mut agent, scratch) = agent(endpoint, "plan-sandbox");
        let made = scratch.0.join("made.txt");
        agent.set_sandbox(Some(Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: false,
            writable: Vec::new(),
        }));
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.set_planning(true);
        agent.turn("plan", &mut ui, &cancel).unwrap();
        assert!(!made.exists());
        agent.set_planning(false);
        agent.turn("go", &mut ui, &cancel).unwrap();
        assert!(made.exists());
        let requests = server.join().unwrap();
        assert!(sent(&requests, 0).contains("run in a read-only sandbox"));
    }
}
