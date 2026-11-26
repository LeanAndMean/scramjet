//! The instructions a file the agent works on brings in, as Claude Code adds them: the rules
//! whose `paths` match it and the `CLAUDE.md` files of the directories between the project and
//! it (`instructions::guidance`).

use super::tools::{self, Call};
use super::{Agent, AgentError, AgentUi, FAILED, PROBE, instructions};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Stored with the message that adds them: the files whose instructions it holds, so none is
/// added twice in a conversation.
const GUIDANCE: &str = "_scramjet_guidance";

/// The tools whose file brings its instructions in, as Claude Code's Read, Edit and Write.
const TOUCHING: [&str; 3] = ["read", "write", "edit"];

impl Agent {
    /// Adds, for the model's next request, the instructions the files that `calls` read, wrote or
    /// edited bring in, each file once in the conversation, the user told which.
    pub(super) fn add_guidance(
        &mut self,
        calls: &[Value],
        ui: &mut dyn AgentUi,
    ) -> Result<(), AgentError> {
        if self.config.prompt.no_context_files {
            return Ok(());
        }
        let succeeded = |call: &Value| {
            (self.conversation.iter().rev()).any(|item| {
                item["type"] == "function_call_output"
                    && item["call_id"] == call["call_id"]
                    && item.get(FAILED).is_none()
            })
        };
        let touched: Vec<PathBuf> = (calls.iter())
            .filter(|call| TOUCHING.contains(&call["name"].as_str().unwrap_or_default()))
            .filter(|call| succeeded(call))
            .filter_map(|call| {
                let call = Call::parse(call).ok()?;
                let path = call.arguments["path"].as_str()?.to_owned();
                Some(tools::resolve(&self.config.root, &path))
            })
            .collect();
        if touched.is_empty() {
            return Ok(());
        }
        let real = |path: &Path| crate::os::canonicalize(path).unwrap_or_else(|_| path.into());
        let mut loaded: Vec<PathBuf> = (self.context_files.iter().map(|path| real(path)))
            .chain(
                (self.conversation.iter())
                    .filter_map(|item| item.get(GUIDANCE)?.as_array())
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|path| real(Path::new(path))),
            )
            .collect();
        let hidden = self.instruction_hidden();
        let (mut files, mut problems) = (Vec::new(), Vec::new());
        for file in touched {
            for (path, text) in
                instructions::guidance(&self.config.root, &file, &hidden, &mut problems)
            {
                let path_real = real(&path);
                if !loaded.contains(&path_real) {
                    loaded.push(path_real);
                    files.push((path, text));
                }
            }
        }
        for problem in problems {
            ui.notice(&problem.to_string());
        }
        if files.is_empty() {
            return Ok(());
        }
        let shown = |path: &Path| match path.strip_prefix(&self.config.root) {
            Ok(inside) => inside.display().to_string(),
            Err(_) => tools::shortened(&path.display().to_string()),
        };
        let names: Vec<String> = files.iter().map(|(path, _)| shown(path)).collect();
        ui.notice(&format!("Loaded {}", names.join(", ")));
        let contents: Vec<String> = (files.iter())
            .map(|(path, text)| format!("Contents of {}:\n\n{}", path.display(), text.trim()))
            .collect();
        let note = format!(
            "<system-reminder>\n{}\n</system-reminder>",
            contents.join("\n\n")
        );
        let paths: Vec<String> = (files.iter())
            .map(|(path, _)| path.display().to_string())
            .collect();
        self.add(json!({"role": "user", "content": note, PROBE: true, GUIDANCE: paths}))
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::tests::{Recorder, agent, serve, stream, tool_call};
    use crate::provider_fixture as fixture;
    use serde_json::{Value, json};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn a_file_read_brings_in_its_rules_and_claude_md_once_and_a_failed_read_none() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "read", json!({"path": "sub/a.rs"})),
            tool_call("call_2", "read", json!({"path": "sub/a.rs"})),
            tool_call("call_3", "read", json!({"path": "other/missing.rs"})),
            stream(&fixture::text_events("Done.", fixture::known_usage())),
        ]);
        let (mut agent, scratch) = agent(endpoint, "guidance");
        let root = scratch.0.clone();
        for directory in ["sub", "other", ".claude/rules"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join("sub/a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("sub/CLAUDE.md"), "Sub rules.\n").unwrap();
        std::fs::write(root.join("other/CLAUDE.md"), "Other rules.\n").unwrap();
        std::fs::write(
            root.join(".claude/rules/rust.md"),
            "---\npaths: \"**/*.rs\"\n---\nRust rule.\n",
        )
        .unwrap();
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.turn("look", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        let last = |at: usize| -> Value {
            let input = requests[at]["input"].as_array().unwrap();
            input.last().unwrap().clone()
        };
        let told = last(1)["content"].as_str().unwrap().to_owned();
        // The user's own `~/.claude/rules` that match would come first.
        let expected = format!(
            "Contents of {}:\n\nSub rules.\n\nContents of {}:\n\nRust rule.\n</system-reminder>",
            root.join("sub/CLAUDE.md").display(),
            root.join(".claude/rules/rust.md").display()
        );
        assert!(told.starts_with("<system-reminder>\n"), "{told}");
        assert!(told.ends_with(&expected), "{told}");
        assert_eq!(last(1)["role"], "user");
        // Read again, and read where it failed: nothing more is added.
        assert_eq!(last(2)["type"], "function_call_output");
        assert_eq!(last(3)["type"], "function_call_output");
        let notices: Vec<&String> = (ui.shown.iter())
            .filter(|shown| shown.starts_with("notice Loaded"))
            .collect();
        assert_eq!(notices.len(), 1, "{:?}", ui.shown);
        assert!(
            notices[0].ends_with("sub/CLAUDE.md, .claude/rules/rust.md"),
            "{notices:?}"
        );
    }
}
