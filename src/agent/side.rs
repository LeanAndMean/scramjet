//! `/btw`, as Claude Code's: a side question answered from the conversation so far, apart from
//! it. The request is the turn's own, so the prompt cache holds, with the question after it; no
//! call it makes runs, and the model never sees the question or the answer again. Both are saved
//! in a `side_question` line with what the request spent, as `!!` output is kept: not sent, but
//! counted.
use super::compaction::Spent;
use super::{Agent, AgentError, AgentUi, Purpose, message_text};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

/// What tells the model that the question is an aside, before the question.
const ASIDE: &str = "<system-reminder>The user asks a side question while you work. Answer it \
directly and briefly from what this conversation already shows. Do not call any tool: no call \
runs now, and neither this question nor your answer stays in the conversation. Your work goes on \
afterwards as it was.</system-reminder>\n\n";

/// What a call still running when the question is asked gives the model in place of its result.
const RUNNING: &str = "(Still running: its result is not in yet.)";

/// A side question answered, and its record to keep (`Agent::keep_aside`).
#[derive(Debug)]
pub struct Aside {
    pub answer: String,
    record: Value,
}

impl Agent {
    /// The model's answer to `question`, asked with the conversation so far and the turn's
    /// instructions and tools. Nothing is added to the conversation until `keep_aside`.
    pub fn side_question(
        &mut self,
        question: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Aside, AgentError> {
        let mut request = self.request_with(&self.instruction_parts());
        let input = request["input"].as_array_mut().unwrap();
        // A turn paused while its calls run has sent calls with no results yet.
        let answered: HashSet<Value> = (input.iter())
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| item["call_id"].clone())
            .collect();
        let running: Vec<Value> = (input.iter())
            .filter(|item| item["type"] == "function_call" && !answered.contains(&item["call_id"]))
            .map(|item| {
                json!({"type":"function_call_output","call_id":item["call_id"],"output":RUNNING})
            })
            .collect();
        input.extend(running);
        input.push(json!({"role":"user","content":format!("{ASIDE}{question}")}));
        let reply = self.respond(&request, Purpose::SideQuestion, ui, cancel)?;
        let mut spent = Spent::default();
        spent.add(&self.config.model, &reply);
        let answer = (reply.items.iter())
            .filter(|item| item["type"] == "message")
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n");
        match answer.trim() {
            "" => Err(AgentError::Provider(
                "the model gave no answer: it may have tried to use a tool".into(),
            )),
            answer => {
                let mut record =
                    json!({"type":"side_question","question":question,"answer":answer});
                spent.save(&mut record);
                Ok(Aside {
                    answer: answer.to_owned(),
                    record,
                })
            }
        }
    }

    /// Saves `aside` in the conversation, which shows it but never sends it, and in the session.
    /// Asked while a turn's calls run, it waits for the turn to end, so as not to come between a
    /// call and its result.
    pub fn keep_aside(&mut self, aside: Aside) -> Result<(), AgentError> {
        self.add(aside.record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{Recorder, agent, serve, stream, tool_call};
    use crate::provider_fixture as fixture;

    #[test]
    fn a_side_question_sends_the_turns_request_and_keeps_nothing() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("It is 4.", fixture::known_usage())),
            tool_call("call_9", "read", json!({"path":"a.rs"})),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "side-question");
        agent
            .add(json!({"role":"user","content":"fix the build"}))
            .unwrap();
        let call = json!({"type":"function_call","call_id":"call_1","name":"bash",
            "arguments":"{\"command\":\"cargo test\"}"});
        agent.add(call).unwrap();
        let conversation = agent.conversation().to_vec();
        let session = std::fs::read_to_string(agent.session_path()).unwrap();
        let turn = agent.request();
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));

        let aside = agent
            .side_question("what is 2+2?", &mut ui, &cancel)
            .unwrap();
        assert_eq!(aside.answer, "It is 4.");
        assert_eq!(agent.usage.requests, 1);
        // A reply that only calls a tool is no answer, and its call does not run.
        let error = agent
            .side_question("and now?", &mut ui, &cancel)
            .unwrap_err();
        assert!(error.to_string().contains("no answer"), "{error}");
        assert_eq!(agent.conversation(), conversation);
        assert_eq!(
            std::fs::read_to_string(agent.session_path()).unwrap(),
            session
        );
        // Kept, it is counted and saved, but never sent.
        agent.keep_aside(aside).unwrap();
        assert_eq!(agent.session_tokens().output, 5);
        let kept = agent.conversation().last().unwrap();
        assert_eq!(
            (&kept["type"], &kept["question"]),
            (&json!("side_question"), &json!("what is 2+2?"))
        );
        assert_eq!(kept["model"], agent.config.model);
        assert_eq!(agent.request()["input"], turn["input"]);
        let (resumed, ..) = crate::agent::Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed.tokens().output, 5);

        let requests = server.join().unwrap();
        let sent = &requests[0];
        for key in ["instructions", "tools", "tool_choice", "prompt_cache_key"] {
            assert_eq!(sent[key], turn[key], "{key}");
        }
        let input = sent["input"].as_array().unwrap();
        assert_eq!(input[..2], turn["input"].as_array().unwrap()[..]);
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(
            (&input[2]["call_id"], &input[2]["output"]),
            (&json!("call_1"), &json!(RUNNING))
        );
        let question = input[3]["content"].as_str().unwrap();
        assert!(
            question.starts_with("<system-reminder>") && question.ends_with("\n\nwhat is 2+2?")
        );
        assert_eq!(input.len(), 4);
    }
}
