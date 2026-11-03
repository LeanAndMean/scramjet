//! What a model request sends, and sending it again after transient failures as Scramjet
//! does.
use super::client::{self, ClientError, ModelInfo, Protocol};
use super::{Agent, AgentError, AgentUi, PRODUCER, Purpose, StreamProgress, messages, tools};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::atomic::AtomicBool, time::Duration};

/// Scramjet's retry backoff: 2s, 4s, 8s, ...
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Like Scramjet, a longer wait (backoff or the provider's Retry-After) fails the request instead.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

impl Agent {
    pub(super) fn request(&self) -> Value {
        let mut request = json!({
            "model": self.config.model,
            "instructions": self.instructions,
            "input": self.replay_conversation(),
            "tools": tools::definitions(&self.config.tools),
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            // GPT-5.6 and later only reuse cached prefixes reliably when requests share a key.
            "prompt_cache_key": self.session.id(),
        });
        request["reasoning"] = json!({"summary": "auto"});
        if let Some(effort) = self.effort() {
            request["reasoning"]["effort"] = json!(effort);
        }
        if let Some(tokens) = self.config.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        request
    }

    /// The conversation as sent to the current model. As in Scramjet, items another model produced
    /// lose what only that model accepts: encrypted reasoning, and `fc_` ids paired with it.
    fn replay_conversation(&self) -> Vec<Value> {
        self.conversation
            .iter()
            .map(|item| {
                let mut item = item.clone();
                let producer = item.as_object_mut().and_then(|object| {
                    let producer = object.remove(PRODUCER);
                    object.retain(|key, _| !key.starts_with("_scramjet_"));
                    producer
                });
                if producer.is_some_and(|producer| producer != self.config.model.as_str()) {
                    if item["type"] == "reasoning" {
                        return Value::Null;
                    }
                    if item["type"] == "function_call"
                        && item["id"].as_str().is_some_and(|id| id.starts_with("fc_"))
                    {
                        item.as_object_mut().unwrap().remove("id");
                    }
                }
                item
            })
            .filter(|item| !item.is_null())
            .collect()
    }

    /// Sends `request` as `send` does, and counts its usage.
    pub(super) fn respond(
        &mut self,
        request: &Value,
        purpose: Purpose,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<client::Reply, AgentError> {
        let retries = self.config.retries;
        let requests = &mut self.usage.requests;
        let reply = send(
            &self.executor,
            self.config.endpoint,
            &self.info,
            retries,
            request,
            cancel,
            &mut |event| match event {
                Sending::Attempt => {
                    ui.waiting(purpose);
                    *requests += 1;
                }
                Sending::Progress(update) if purpose == Purpose::Reply => ui.progress(update),
                Sending::Progress(_) => {}
                Sending::Retrying(error, attempt, delay) => {
                    ui.retrying(error, attempt, retries, delay)
                }
            },
        )?;
        if let Some(usage) = &reply.usage {
            self.usage.count(usage);
        }
        Ok(reply)
    }
}

/// What happens while a request is sent.
pub(super) enum Sending<'a> {
    /// An attempt starts.
    Attempt,
    Progress(&'a StreamProgress),
    /// An attempt failed transiently; the next one, the given attempt, starts after the delay.
    Retrying(&'a str, u32, Duration),
}

/// Sends `request` to the model `info` describes, sending it again after transient failures. A
/// failed response's items are discarded, so a retry never runs a tool call from it.
pub(super) fn send(
    executor: &tokio::runtime::Runtime,
    endpoint: SocketAddr,
    info: &ModelInfo,
    retries: u32,
    request: &Value,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Sending),
) -> Result<client::Reply, AgentError> {
    let (body, betas) = match info.protocol {
        Protocol::Responses => (serde_json::to_vec(request), None),
        Protocol::Messages => {
            let body = messages::request(
                request,
                info.output.unwrap_or(messages::DEFAULT_MAX_TOKENS),
                info.reasoning,
            )
            .map_err(AgentError::Provider)?;
            let betas = messages::betas(&body);
            (serde_json::to_vec(&body), betas)
        }
    };
    let body = body.map_err(std::io::Error::from)?;
    let mut attempt = 0;
    loop {
        on(Sending::Attempt);
        let failure = match executor.block_on(client::respond(
            endpoint,
            info.protocol,
            body.clone(),
            betas.as_deref(),
            cancel,
            &mut |update| on(Sending::Progress(&update)),
        )) {
            Ok(reply) => return Ok(reply),
            Err(ClientError::Cancelled) => return Err(AgentError::Cancelled),
            Err(ClientError::Failed(failure)) => failure,
        };
        let backoff = RETRY_DELAY * 2u32.pow(attempt.min(5));
        let delay = backoff
            .min(MAX_RETRY_WAIT)
            .max(failure.wait.unwrap_or_default());
        // A context overflow is not retried as is: only compaction can let the request through.
        if !failure.transient || failure.overflow || attempt == retries || delay > MAX_RETRY_WAIT {
            let message = match attempt {
                0 => failure.message,
                n => format!("{} (after {n} retries)", failure.message),
            };
            return Err(match failure.overflow {
                true => AgentError::ContextOverflow(message),
                false => AgentError::Provider(message),
            });
        }
        attempt += 1;
        on(Sending::Retrying(&failure.message, attempt, delay));
        let interrupted = executor.block_on(async {
            tokio::select! {
                biased;
                () = client::cancelled(cancel) => true,
                () = tokio::time::sleep(delay) => false,
            }
        });
        if interrupted {
            return Err(AgentError::Cancelled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{AgentConfig, SessionChoice};
    use crate::provider_fixture as fixture;

    #[test]
    fn transient_failures_are_sent_again_without_running_their_tool_calls() {
        let mut failed = fixture::call_events("{}", fixture::known_usage());
        let terminal = failed.last_mut().unwrap();
        terminal["type"] = json!("response.failed");
        terminal["response"]["status"] = json!("failed");
        // Observed through Proxy on 2026-10-30.
        terminal["response"]["error"] = json!({"message": "Upstream model provider failed before \
the response completed (ReadError: ConnectionResetError(104, 'Connection reset by peer')). The \
response is incomplete; retry the request."});
        let (endpoint, server) = serve(vec![
            stream(&failed),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "retry");
        let mut ui = Recorder::default();
        agent.turn("hi", &mut ui, &AtomicBool::new(false)).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(ui.retries.len(), 1);
        assert!(ui.retries[0].0.contains("retry the request"));
        assert_eq!((ui.retries[0].1, ui.retries[0].2), (1, RETRY_DELAY));
        assert_eq!(ui.replies, ["done"]);
        assert!(ui.calls.is_empty());
        assert_eq!(agent.usage.requests, 2);
        assert_eq!(agent.usage.context_tokens, Some(15));
        assert_eq!(agent.conversation()[1]["_scramjet_model"], "m");
        assert!(bodies[1]["input"][0].get("_scramjet_model").is_none());
        let kinds: Vec<_> = agent
            .conversation()
            .iter()
            .map(|item| item["type"].as_str().or(item["role"].as_str()).unwrap())
            .collect();
        assert_eq!(kinds, ["user", "message"]);
    }

    #[test]
    fn permanent_failures_and_long_retry_waits_end_the_turn_at_once() {
        let (endpoint, server) = serve(vec![
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 15\r\n\r\ninvalid request".to_vec(),
            b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 120\r\ncontent-length: 4\r\n\r\nslow"
                .to_vec(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "permanent");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let errors: Vec<_> = (0..2)
            .map(|_| match agent.turn("hi", &mut ui, &cancel) {
                Err(AgentError::Provider(message)) => message,
                other => panic!("{:?}", other.err()),
            })
            .collect();
        assert_eq!(
            errors,
            [
                "HTTP 400 Bad Request: invalid request",
                "HTTP 429 Too Many Requests: slow",
            ]
        );
        assert!(ui.retries.is_empty());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn failures_are_classified_by_code_then_wording() {
        let failure = |code, message: &str| client::Failure::provider(code, message.into());
        let transient = |code, message: &str| failure(code, message).transient;
        assert!(transient(Some("server_error"), "anything"));
        assert!(!transient(Some("insufficient_quota"), "rate limit"));
        assert!(transient(None, "Rate limit reached for requests"));
        assert!(transient(Some("unknown_code"), "The server is overloaded"));
        assert!(!transient(
            None,
            "This model's maximum context length is 128000 tokens; timeout"
        ));
        assert!(!transient(None, "Invalid value for 'input'"));
        assert!(failure(Some("context_length_exceeded"), "anything").overflow);
        assert!(failure(None, "Your input exceeds the context window of this model.").overflow);
        assert!(failure(None, "prompt is too long: 213462 tokens > 200000 maximum").overflow);
        assert!(!failure(None, "Rate limit reached: too many tokens").overflow);
        assert!(!failure(None, "Invalid value for 'input'").overflow);
        // Scramjet's provider examples, and deepseek-v4.1-flash's gateway through Proxy.
        for overflow in [
            "This model's maximum context length is 327680 tokens. However, you requested 330001",
            "Requested token count exceeds the model's maximum context length of 131,072 tokens",
            "The input token count (1196265) exceeds the maximum number of tokens allowed",
            "prompt token count of 140000 exceeds the limit of 128000",
            "The input (140000 tokens) is longer than the model's context length (131072 tokens).",
            "Prompt contains 140000 tokens, too large for model with 131072 maximum context length",
            "prompt too long; exceeded max context length by 12 tokens",
        ] {
            assert!(failure(None, overflow).overflow, "{overflow}");
        }
        // The same words without what makes them an overflow in Scramjet's patterns.
        for other in [
            "The upload exceeds the limit of the plan",
            "max_tokens must be less than the maximum context length",
            "Invalid input token count",
            "ThrottlingException: Too many tokens, please wait before trying again.",
        ] {
            assert!(!failure(None, other).overflow, "{other}");
        }
    }

    #[test]
    fn replay_drops_other_models_reasoning_and_unpairs_function_calls_after_resume() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-model-{}", std::process::id()));
        let _scratch = ScratchDir::new(root.clone());
        let path = root.join("1700000000000-42.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"version\":1}\n",
                "{\"type\":\"reasoning\",\"encrypted_content\":\"opaque\",\"_scramjet_model\":\"old\"}\n",
                "{\"type\":\"function_call\",\"id\":\"fc_item\",\"call_id\":\"call_1\",\"_scramjet_model\":\"old\"}\n",
                "{\"type\":\"function_call\",\"id\":\"item_other\",\"call_id\":\"call_2\",\"_scramjet_model\":\"old\"}\n",
                "{\"type\":\"function_call_output\",\"call_id\":\"call_1\",\"output\":\"ok\"}\n",
                "{\"type\":\"function_call_output\",\"call_id\":\"call_2\",\"output\":\"ok\"}\n"
            ),
        )
        .unwrap();
        let config = AgentConfig {
            endpoint: ([127, 0, 0, 1], 9).into(),
            model: "new".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
            command_sets: Vec::new(),
            skills: Vec::new(),
        };
        let mut agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        let replay = agent.replay_conversation();
        assert_eq!(replay.len(), 4);
        assert_eq!(replay[0]["type"], "function_call");
        assert!(replay[0].get("id").is_none());
        assert_eq!(replay[0]["call_id"], "call_1");
        assert_eq!(replay[1]["id"], "item_other");
        assert_eq!(replay[2]["type"], "function_call_output");
        assert!(
            replay
                .iter()
                .all(|item| item.get("_scramjet_model").is_none())
        );

        assert_eq!(agent.last_model(), Some("old"));
        agent.set_model("old").unwrap();
        let same_model = agent.replay_conversation();
        assert_eq!(same_model.len(), 5);
        assert_eq!(same_model[0]["type"], "reasoning");
        assert_eq!(same_model[1]["id"], "fc_item");
    }

    #[test]
    fn older_claude_models_send_budget_thinking_with_the_interleaved_beta() {
        let (endpoint, server) = serve_requests(vec![messages_stream(&[
            json!({"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":3,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ])]);
        let (mut agent, _scratch) = agent(endpoint, "budget");
        agent.config.effort = Some("low".into());
        agent.set_model("claude-haiku-4-5-20251001").unwrap();
        let mut ui = Recorder::default();
        agent.turn("Hi", &mut ui, &AtomicBool::new(false)).unwrap();
        let requests = server.join().unwrap();
        let (head, body) = &requests[0];
        assert!(
            head.contains(
                "anthropic-beta: fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14\r\n"
            ),
            "{head}"
        );
        assert_eq!(
            body["thinking"],
            json!({"type":"enabled","budget_tokens":2048,"display":"summarized"})
        );
    }

    #[test]
    fn budget_thinking_without_room_fails_before_sending() {
        // Nothing listens on the endpoint: the request must not be sent.
        let endpoint = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let (mut agent, _scratch) = agent(endpoint, "no-room");
        agent.config.effort = Some("high".into());
        agent.config.max_output_tokens = Some(2000);
        agent.set_model("claude-sonnet-4-5").unwrap();
        let mut ui = Recorder::default();
        let error = agent
            .turn("Hi", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert!(
            matches!(&error, AgentError::Provider(message) if message.starts_with("Insufficient output space for Anthropic thinking")),
            "{error}"
        );
    }

    #[test]
    fn claude_models_use_messages_and_replay_their_thinking_calls_and_results() {
        let start = |id: &str, input: u64| {
            json!({"type":"message_start","message":{"id":id,"usage":{"input_tokens":input,
                "cache_read_input_tokens":5,"cache_creation_input_tokens":2,"output_tokens":1}}})
        };
        let stop = |index: u64| json!({"type":"content_block_stop","index":index});
        let delta = |index: u64, delta: Value| json!({"type":"content_block_delta","index":index,"delta":delta});
        let (endpoint, server) = serve_requests(vec![
            messages_stream(&[
                start("msg_1", 10),
                json!({"type":"ping"}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
                delta(
                    0,
                    json!({"type":"thinking_delta","thinking":"Need the file."}),
                ),
                delta(0, json!({"type":"signature_delta","signature":"sig1"})),
                stop(0),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                delta(1, json!({"type":"text_delta","text":"Reading"})),
                stop(1),
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read","input":{}}}),
                delta(
                    2,
                    json!({"type":"input_json_delta","partial_json":"{\"path\":"}),
                ),
                delta(
                    2,
                    json!({"type":"input_json_delta","partial_json":"\"notes.txt\"}"}),
                ),
                stop(2),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}),
                json!({"type":"message_stop"}),
            ]),
            messages_stream(&[
                start("msg_2", 30),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                delta(0, json!({"type":"text_delta","text":"It says hello."})),
                stop(0),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
                json!({"type":"message_stop"}),
            ]),
        ]);
        let (mut agent, scratch) = agent(endpoint, "claude");
        std::fs::write(scratch.0.join("notes.txt"), "hello\n").unwrap();
        agent.set_model("claude-opus-5-5").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("What is in notes.txt?", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["Reading", "It says hello."]);
        assert_eq!(ui.calls, ["read"]);
        let requests = server.join().unwrap();
        for (head, _) in &requests {
            assert!(head.starts_with("post /v1/messages http/1.1\r\n"), "{head}");
            assert!(head.contains("anthropic-version: 2023-06-01\r\n"), "{head}");
            assert!(
                head.contains("anthropic-beta: fine-grained-tool-streaming-2025-05-14\r\n"),
                "{head}"
            );
        }
        let first = &requests[0].1;
        assert_eq!(first["model"], "claude-opus-5-5");
        assert_eq!(first["max_tokens"], messages::DEFAULT_MAX_TOKENS);
        assert_eq!(
            first["thinking"],
            json!({"type":"adaptive","display":"summarized"})
        );
        assert_eq!(first["system"][0]["text"], agent.instructions);
        assert_eq!(first["tools"][0]["name"], "read");
        assert_eq!(
            first["tools"][0]["input_schema"]["required"],
            json!(["path"])
        );
        assert_eq!(
            first["messages"],
            json!([{"role":"user","content":[{"type":"text","text":"What is in notes.txt?",
                "cache_control":{"type":"ephemeral","ttl":"1h"}}]}])
        );
        let second = &requests[1].1["messages"];
        assert_eq!(
            second[1],
            json!({"role":"assistant","content":[
                {"type":"thinking","thinking":"Need the file.","signature":"sig1"},
                {"type":"text","text":"Reading"},
                {"type":"tool_use","id":"toolu_1","name":"read","input":{"path":"notes.txt"}},
            ]})
        );
        let result = &second[2]["content"][0];
        assert_eq!(second[2]["role"], "user");
        assert_eq!(result["tool_use_id"], "toolu_1");
        assert!(result["content"].as_str().unwrap().contains("hello"));
        assert_eq!(
            result["cache_control"],
            json!({"type":"ephemeral","ttl":"1h"})
        );
        assert!(second[0]["content"][0].get("cache_control").is_none());
        // Input counts cache reads and writes, as Responses input does.
        assert_eq!(agent.usage.input_tokens, 17 + 37);
        assert_eq!(agent.usage.cached_input_tokens, 10);
        assert_eq!(agent.usage.output_tokens, 24);
        assert_eq!(agent.usage.context_tokens, Some(41));
        assert!(
            agent.conversation()[1..]
                .iter()
                .filter(|item| item.get("type").is_some())
                .all(|item| item["_scramjet_model"] == "claude-opus-5-5"
                    || item["type"] == "function_call_output")
        );
    }

    #[test]
    fn a_claude_reply_at_the_output_limit_keeps_its_text_and_ends_the_turn_without_its_call() {
        let (endpoint, server) = serve_requests(vec![
            messages_stream(&[
                json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Writing notes.txt"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"write","input":{}}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"notes.txt\",\"content\":\"hel"}}),
                json!({"type":"content_block_stop","index":1}),
                json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":20}}),
                json!({"type":"message_stop"}),
            ]),
            messages_stream(&[
                json!({"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":9,"output_tokens":1}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Shorter then."}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
                json!({"type":"message_stop"}),
            ]),
        ]);
        let (mut agent, scratch) = agent(endpoint, "limit");
        agent.set_model("claude-opus-5-5").unwrap();
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let error = agent.turn("Write notes", &mut ui, &cancel).unwrap_err();
        assert!(
            matches!(&error, AgentError::Provider(message) if message == "response incomplete: it \
reached the output token limit (max_tokens); the tool call it was making did not run"),
            "{error}"
        );
        assert_eq!(ui.replies, ["Writing notes.txt"]);
        assert!(ui.calls.is_empty());
        assert!(!scratch.0.join("notes.txt").exists());
        assert_eq!(agent.usage.output_tokens, 20);
        agent.turn("Continue", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        assert_eq!(
            requests[1].1["messages"],
            json!([
                {"role":"user","content":[{"type":"text","text":"Write notes"}]},
                {"role":"assistant","content":[{"type":"text","text":"Writing notes.txt"}]},
                {"role":"user","content":[{"type":"text","text":"Continue",
                    "cache_control":{"type":"ephemeral","ttl":"1h"}}]},
            ])
        );
    }

    #[test]
    fn requests_share_the_session_cache_key_across_resumes() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-cache-{}", std::process::id()));
        let _scratch = ScratchDir::new(root.clone());
        let path = root.join("1700000000000-42.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"session\",\"version\":1}\n{\"role\":\"user\",\"content\":\"hi\"}\n",
        )
        .unwrap();
        let config = AgentConfig {
            endpoint: ([127, 0, 0, 1], 9).into(),
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
            command_sets: Vec::new(),
            skills: Vec::new(),
        };
        let agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        assert_eq!(agent.request()["prompt_cache_key"], "1700000000000-42");
    }
}
