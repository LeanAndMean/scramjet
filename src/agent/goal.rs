//! `/goal`, as Claude Code's: a condition checked after each turn, the agent going on until a
//! check finds it met. The check is a side request: the turn's own request, so the
//! prompt cache holds, with the condition after it, asking for a verdict in JSON; no call its
//! reply makes runs. Each check is saved in a `goal_check` line with what it spent: never sent,
//! but counted.
use super::compaction::Spent;
use super::{Agent, AgentError, AgentUi, Attached, Purpose, message_text};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

/// What the model is asked, before the condition.
const CHECK: &str = "<system-reminder>Stop working for a moment: judge whether the session's \
goal below is met now, from what this conversation shows. Do not call any tool and do not go on \
with the work; neither this question nor your answer stays in the conversation. Answer with one \
JSON object and nothing else:\n\
- {\"ok\": true, \"reason\": \"...\"} when the conversation shows the goal met, quoting what \
shows it;\n\
- {\"ok\": false, \"reason\": \"...\"} when it does not, saying what is missing or in the way;\n\
- {\"ok\": false, \"impossible\": true, \"reason\": \"...\"} only when the goal can never be met \
in this session: it contradicts itself, it needs what is not available, or every reasonable way \
has been tried. Saying it cannot be done is not proof: judge it yourself. Slow progress is not \
impossible.\n\
Without clear evidence in the conversation, the goal is not met.</system-reminder>\n\nThe goal: ";

/// What a goal's first prompt sends before the prompt as typed.
const SET: &str = "<system-reminder>The user set this session's goal with /goal. Say briefly \
that you have it, then start, or go on, working toward it: the condition is your task, so do not \
stop to ask the user what to do. After each of your turns a check reads the conversation and \
decides whether it is met; until it is, you are asked to go on. Once met, the goal ends by \
itself: the user does not need to clear it.</system-reminder>\n\n";

/// What the prompt that goes on after a check sends before the prompt as typed.
const NOT_MET: &str = "<system-reminder>A check after your turn found the session's goal not \
met yet; the reason it gave follows. Keep working toward the goal without asking the user what \
to do. If it truly cannot be met, say why plainly.</system-reminder>\n\n";

/// A check's verdict.
#[derive(Debug, Deserialize, PartialEq)]
pub struct Verdict {
    #[serde(rename = "ok")]
    pub met: bool,
    #[serde(default)]
    pub impossible: bool,
    pub reason: String,
}

/// A goal checked, and its record to keep (`Agent::keep_goal_check`).
#[derive(Debug)]
pub struct GoalCheck {
    pub verdict: Verdict,
    record: Value,
}

impl Agent {
    /// Whether the conversation meets `condition`, in the model's judgment from the conversation
    /// so far with the turn's instructions and tools. Nothing is added to the conversation until
    /// `keep_goal_check`.
    pub fn check_goal(
        &mut self,
        condition: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<GoalCheck, AgentError> {
        let mut request = self.request_with(&self.instruction_parts());
        let input = request["input"].as_array_mut().unwrap();
        input.push(json!({"role":"user","content":format!("{CHECK}{condition}")}));
        let reply = self.respond(&request, Purpose::GoalCheck, ui, cancel)?;
        let mut spent = Spent::default();
        spent.add(&self.config.model, &reply);
        let answer = (reply.items.iter())
            .filter(|item| item["type"] == "message")
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n");
        let verdict = verdict(&answer).ok_or_else(|| {
            AgentError::Provider(match answer.trim() {
                "" => {
                    "the goal check gave no answer: the model may have tried to use a tool".into()
                }
                answer => format!("the goal check's answer is not a verdict: {answer}"),
            })
        })?;
        let mut record = json!({"type":"goal_check","condition":condition,
            "met":verdict.met,"reason":verdict.reason});
        if verdict.impossible {
            record["impossible"] = json!(true);
        }
        spent.save(&mut record);
        Ok(GoalCheck { verdict, record })
    }

    /// Sends the next prompt with what tells the model that the goal is set (`set`), or that a
    /// check found it not met, before the prompt as typed.
    pub fn attach_goal(&mut self, set: bool) {
        self.attach(Attached::before(if set { SET } else { NOT_MET }));
    }

    /// Saves `check` in the conversation, which never sends it, and in the session.
    pub fn keep_goal_check(&mut self, check: GoalCheck) -> Result<(), AgentError> {
        self.add(check.record)
    }
}

/// The verdict in `answer`: its JSON object, also inside a code fence or after other words.
fn verdict(answer: &str) -> Option<Verdict> {
    let (start, end) = (answer.find('{')?, answer.rfind('}')?);
    let mut verdict: Verdict = serde_json::from_str(answer.get(start..=end)?).ok()?;
    verdict.impossible &= !verdict.met;
    Some(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{Recorder, agent, serve, stream, tool_call};
    use crate::provider_fixture as fixture;

    #[test]
    fn the_verdict_is_read_from_the_json_in_the_answer() {
        let met = Verdict {
            met: true,
            impossible: false,
            reason: "tests pass".into(),
        };
        assert_eq!(
            verdict(r#"{"ok": true, "reason": "tests pass"}"#),
            Some(met)
        );
        let fenced =
            "Checked.\n```json\n{\"ok\": false, \"impossible\": true, \"reason\": \"x\"}\n```";
        let fenced = verdict(fenced).unwrap();
        assert!(!fenced.met && fenced.impossible);
        // A met goal is never impossible.
        let both = verdict(r#"{"ok": true, "impossible": true, "reason": "y"}"#).unwrap();
        assert!(both.met && !both.impossible);
        assert_eq!(verdict("It is met."), None);
        assert_eq!(verdict(r#"{"ok": true}"#), None);
    }

    #[test]
    fn a_goal_check_sends_the_turns_request_and_keeps_nothing_until_kept() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events(
                r#"{"ok": false, "reason": "the tests were not run"}"#,
                fixture::known_usage(),
            )),
            tool_call("call_9", "bash", json!({"command":"cargo test"})),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "goal-check");
        agent
            .add(json!({"role":"user","content":"fix the build"}))
            .unwrap();
        let conversation = agent.conversation().to_vec();
        let turn = agent.request();
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));

        let check = agent
            .check_goal("cargo test passes", &mut ui, &cancel)
            .unwrap();
        assert!(!check.verdict.met);
        assert_eq!(check.verdict.reason, "the tests were not run");
        // A reply that only calls a tool is no verdict, and its call does not run.
        let error = agent
            .check_goal("cargo test passes", &mut ui, &cancel)
            .unwrap_err();
        assert!(error.to_string().contains("no answer"), "{error}");
        assert_eq!(agent.conversation(), conversation);

        agent.keep_goal_check(check).unwrap();
        let kept = agent.conversation().last().unwrap();
        assert_eq!(
            (&kept["type"], &kept["met"], &kept["condition"]),
            (
                &json!("goal_check"),
                &json!(false),
                &json!("cargo test passes")
            )
        );
        assert_eq!(agent.session_tokens().output, 5);
        assert_eq!(agent.request()["input"], turn["input"]);

        let requests = server.join().unwrap();
        let sent = &requests[0];
        for key in ["instructions", "tools", "tool_choice", "prompt_cache_key"] {
            assert_eq!(sent[key], turn[key], "{key}");
        }
        let input = sent["input"].as_array().unwrap();
        assert_eq!(input[..1], turn["input"].as_array().unwrap()[..]);
        let asked = input[1]["content"].as_str().unwrap();
        assert!(asked.starts_with("<system-reminder>"), "{asked}");
        assert!(asked.ends_with("The goal: cargo test passes"), "{asked}");
        assert_eq!(input.len(), 2);
    }
}
