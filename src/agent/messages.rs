//! Anthropic Messages (`/v1/messages`), which Proxy serves Claude models on. Requests are converted
//! from the Responses request the agent builds, and replies back into Responses items, so a
//! session keeps one item format whichever protocol produced its items.
use super::StreamProgress;
use super::client::{Failure, LIMITS, Reply};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Used without a model card to say how much a reply may have.
pub(super) const DEFAULT_MAX_TOKENS: u64 = 32_000;

/// The beta header budget thinking needs; adaptive models have interleaved thinking built in.
pub(super) const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// Least output space left for a reply when thinking takes a budget, as Scramjet keeps it.
const MIN_OUTPUT_TOKENS: u64 = 1024;

/// Whether `model` takes adaptive thinking: Claude 4.6 and later (Proxy rewrites budget thinking
/// for 4.7 and later, which reject it).
pub(super) fn adaptive_thinking(model: &str) -> bool {
    let Some(rest) = model.strip_prefix("claude-") else {
        return false;
    };
    let mut parts = rest.split('-');
    if !parts
        .next()
        .is_some_and(|family| family.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return false;
    }
    let Some(major) = parts.next().and_then(|part| part.parse::<u32>().ok()) else {
        return false;
    };
    // A date suffix (`claude-sonnet-4-20250514`) is not a minor version.
    let minor = parts
        .next()
        .filter(|part| part.len() <= 2)
        .and_then(|part| part.parse::<u32>().ok())
        .unwrap_or(0);
    major > 4 || (major == 4 && minor >= 6)
}

/// Whether `body` asks for budget thinking, which needs the interleaved-thinking beta header.
pub(super) fn budget_thinking(body: &Value) -> bool {
    body["thinking"]["type"] == "enabled"
}

/// The thinking budget for an effort level, as Scramjet's default budgets; `xhigh` and `max`
/// count as `high`, and no effort as `medium`.
fn budget(effort: Option<&str>) -> u64 {
    match effort {
        Some("minimal") => 1024,
        Some("low") => 2048,
        Some("high" | "xhigh" | "max") => 16384,
        _ => 8192,
    }
}

/// The Messages request for a Responses `request` built by the agent. As in Scramjet, the system
/// prompt (with the tools before it) and the conversation up to its last block are marked for
/// caching; Foundry rejects `cache_control` on tools. Claude models before 4.6 that reason get
/// budget thinking, which takes room from `max_tokens`; without room for it the request is an
/// error. Other models, such as `deepseek-v4-pro`, get no thinking parameter.
pub(super) fn request(request: &Value, max_tokens: u64, thinking: bool) -> Result<Value, String> {
    let items = request["input"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let max_tokens = request["max_output_tokens"].as_u64().unwrap_or(max_tokens);
    let mut body = json!({
        "model": request["model"],
        "max_tokens": max_tokens,
        "stream": true,
        "system": [{"type":"text","text":request["instructions"],"cache_control":{"type":"ephemeral"}}],
        "messages": messages(items),
    });
    if let Some(tools) = request["tools"].as_array() {
        body["tools"] = tools
            .iter()
            .map(|tool| {
                json!({"name":tool["name"],"description":tool["description"],"input_schema":tool["parameters"]})
            })
            .collect();
    }
    let effort = request["reasoning"]["effort"].as_str();
    let model = request["model"].as_str().unwrap_or("");
    if thinking && effort != Some("none") && model.starts_with("claude-") {
        if adaptive_thinking(model) {
            // Opus 4.7 and later omit the thinking text unless asked; Scramjet asks for summaries.
            body["thinking"] = json!({"type":"adaptive","display":"summarized"});
            if let Some(effort) = effort {
                let effort = if effort == "minimal" { "low" } else { effort };
                body["output_config"] = json!({ "effort": effort });
            }
        } else {
            let mut budget = budget(effort);
            if max_tokens <= budget {
                budget = max_tokens.saturating_sub(MIN_OUTPUT_TOKENS);
            }
            if budget < MIN_OUTPUT_TOKENS {
                return Err("Insufficient output space for Anthropic thinking; reduce input or request more output space.".into());
            }
            body["thinking"] =
                json!({"type":"enabled","budget_tokens":budget,"display":"summarized"});
        }
    }
    Ok(body)
}

/// Responses items as Messages: consecutive items of one role share a message, and as in
/// Scramjet, a tool call without a result (a turn cancelled between calls) gets an error result.
fn messages(items: &[Value]) -> Vec<Value> {
    let mut messages: Vec<(&str, Vec<Value>)> = Vec::new();
    for (role, block) in items.iter().filter_map(block) {
        match messages.last_mut() {
            Some((last, blocks)) if *last == role => blocks.push(block),
            _ => messages.push((role, vec![block])),
        }
    }
    let mut i = 0;
    while i < messages.len() {
        let calls: Vec<Value> = messages[i]
            .1
            .iter()
            .filter(|block| block["type"] == "tool_use")
            .map(|block| block["id"].clone())
            .collect();
        if messages[i].0 == "assistant" && !calls.is_empty() {
            if messages.get(i + 1).is_none_or(|(role, _)| *role != "user") {
                messages.insert(i + 1, ("user", Vec::new()));
            }
            let next = &mut messages[i + 1].1;
            let answered: HashSet<_> = next
                .iter()
                .filter(|block| block["type"] == "tool_result")
                .map(|block| block["tool_use_id"].clone())
                .collect();
            let at = next
                .iter()
                .take_while(|block| block["type"] == "tool_result")
                .count();
            let missing = calls.into_iter().filter(|id| !answered.contains(id)).map(
                |id| json!({"type":"tool_result","tool_use_id":id,"content":"No result provided","is_error":true}),
            );
            next.splice(at..at, missing.collect::<Vec<_>>());
        }
        i += 1;
    }
    if let Some(("user", blocks)) = messages.last_mut()
        && let Some(last) = blocks.last_mut()
    {
        last["cache_control"] = json!({"type":"ephemeral"});
    }
    messages
        .into_iter()
        .map(|(role, content)| json!({"role":role,"content":content}))
        .collect()
}

/// A conversation item as a Messages content block, with the role of the message it belongs to.
/// Reasoning without a Claude signature (another model's) has no Messages form.
fn block(item: &Value) -> Option<(&'static str, Value)> {
    let text = |item: &Value| -> String {
        match &item["content"] {
            Value::String(text) => text.clone(),
            content => content
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|part| part["text"].as_str().or(part["refusal"].as_str()))
                .collect(),
        }
    };
    let role = if item["role"] == "user" {
        "user"
    } else {
        "assistant"
    };
    let block = match (item["role"].as_str(), item["type"].as_str()) {
        (Some("user"), _) | (_, Some("message")) => {
            let text = text(item);
            if text.trim().is_empty() {
                return None;
            }
            json!({"type":"text","text":text})
        }
        (_, Some("reasoning")) => {
            if let Some(data) = item["redacted"].as_str() {
                json!({"type":"redacted_thinking","data":data})
            } else {
                let signature = item["signature"].as_str().filter(|s| !s.is_empty())?;
                let thinking: String = item["summary"]
                    .as_array()?
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .collect();
                if thinking.trim().is_empty() {
                    return None;
                }
                json!({"type":"thinking","thinking":thinking,"signature":signature})
            }
        }
        (_, Some("function_call")) => {
            let input = item["arguments"]
                .as_str()
                .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            json!({"type":"tool_use","id":tool_id(&item["call_id"]),"name":item["name"],"input":input})
        }
        (_, Some("function_call_output")) => {
            return Some((
                "user",
                json!({"type":"tool_result","tool_use_id":tool_id(&item["call_id"]),"content":item["output"]}),
            ));
        }
        _ => return None,
    };
    Some((role, block))
}

/// Anthropic accepts tool ids of at most 64 letters, digits, `_` and `-`; other models' ids are
/// mapped as Scramjet maps them, the same way for a call and its result.
fn tool_id(id: &Value) -> String {
    id.as_str()
        .unwrap_or("")
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            _ => '_',
        })
        .take(64)
        .collect()
}

#[derive(Debug)]
enum Kind {
    Text,
    Thinking {
        signature: String,
    },
    Redacted(String),
    Tool {
        id: String,
        name: String,
        input: Value,
    },
    /// A block type this client does not use, such as a server tool's.
    Other,
}

#[derive(Debug)]
struct Block {
    index: u64,
    kind: Kind,
    /// The text, thinking or tool input JSON received so far.
    text: String,
    done: bool,
}

/// Decodes a streamed Messages reply as it arrives.
#[derive(Debug, Default)]
pub(super) struct Stream {
    line: Vec<u8>,
    data: String,
    id: String,
    blocks: Vec<Block>,
    /// Input, cache read, cache write and output tokens.
    usage: Option<[u64; 4]>,
    stop_reason: Option<String>,
    stopped: bool,
}

fn malformed(what: &str) -> Failure {
    Failure::new(format!("unexpected response stream ({what})"), false)
}

impl Stream {
    pub(super) fn feed(
        &mut self,
        data: &[u8],
        progress: &mut dyn FnMut(StreamProgress),
    ) -> Result<(), Failure> {
        for &byte in data {
            if byte != b'\n' {
                if self.line.len() + self.data.len() >= LIMITS.frame {
                    return Err(Failure::new(
                        "a response event exceeded the size limit",
                        false,
                    ));
                }
                self.line.push(byte);
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let line = line.strip_suffix(b"\r").unwrap_or(&line);
            let line = std::str::from_utf8(line).map_err(|_| malformed("invalid UTF-8"))?;
            if line.is_empty() {
                let data = std::mem::take(&mut self.data);
                if !data.is_empty() {
                    self.event(&data, progress)?;
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        Ok(())
    }

    fn block(&mut self, event: &Value) -> Result<&mut Block, Failure> {
        let index = event["index"].as_u64();
        self.blocks
            .iter_mut()
            .find(|block| Some(block.index) == index && !block.done)
            .ok_or_else(|| malformed("event for an unknown content block"))
    }

    fn event(
        &mut self,
        data: &str,
        progress: &mut dyn FnMut(StreamProgress),
    ) -> Result<(), Failure> {
        let event: Value = serde_json::from_str(data).map_err(|_| malformed("invalid JSON"))?;
        match event["type"].as_str().unwrap_or("") {
            "message_start" => {
                let message = &event["message"];
                self.id = message["id"].as_str().unwrap_or("").to_owned();
                self.usage(&message["usage"]);
            }
            "content_block_start" => {
                let index = event["index"]
                    .as_u64()
                    .ok_or_else(|| malformed("content block without an index"))?;
                let start = &event["content_block"];
                let string = |field: &str| start[field].as_str().unwrap_or("").to_owned();
                let (kind, text) = match start["type"].as_str().unwrap_or("") {
                    "text" => (Kind::Text, string("text")),
                    "thinking" => (
                        Kind::Thinking {
                            signature: string("signature"),
                        },
                        string("thinking"),
                    ),
                    "redacted_thinking" => (Kind::Redacted(string("data")), String::new()),
                    "tool_use" => {
                        progress(StreamProgress::Call(string("name")));
                        let input = match &start["input"] {
                            input @ Value::Object(_) => input.clone(),
                            _ => json!({}),
                        };
                        let kind = Kind::Tool {
                            id: string("id"),
                            name: string("name"),
                            input,
                        };
                        (kind, String::new())
                    }
                    _ => (Kind::Other, String::new()),
                };
                self.blocks.push(Block {
                    index,
                    kind,
                    text,
                    done: false,
                });
            }
            "content_block_delta" => {
                let delta = &event["delta"];
                let block = self.block(&event)?;
                let field = match (delta["type"].as_str().unwrap_or(""), &mut block.kind) {
                    ("text_delta", Kind::Text) => "text",
                    ("thinking_delta", Kind::Thinking { .. }) => "thinking",
                    ("input_json_delta", Kind::Tool { .. }) => "partial_json",
                    ("signature_delta", Kind::Thinking { signature }) => {
                        signature.push_str(delta["signature"].as_str().unwrap_or(""));
                        return Ok(());
                    }
                    _ => return Ok(()),
                };
                let text = delta[field].as_str().unwrap_or("");
                block.text.push_str(text);
                match block.kind {
                    Kind::Text => progress(StreamProgress::Text(text.to_owned())),
                    Kind::Thinking { .. } => progress(StreamProgress::Reasoning(text.to_owned())),
                    _ => {}
                }
            }
            "content_block_stop" => {
                let block = self.block(&event)?;
                block.done = true;
                // Like a Responses summary part, a thinking block is a paragraph of its own.
                if matches!(block.kind, Kind::Thinking { .. }) && !block.text.is_empty() {
                    progress(StreamProgress::Reasoning("\n\n".into()));
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_owned());
                }
                self.usage(&event["usage"]);
            }
            "message_stop" => self.stopped = true,
            "error" => {
                let error = &event["error"];
                let message = error["message"].as_str().unwrap_or("no message");
                return Err(Failure::provider(
                    error["type"].as_str(),
                    format!("provider error: {message}"),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    /// As Scramjet, a later event's counts replace earlier ones only where it gives them.
    fn usage(&mut self, usage: &Value) {
        if !usage.is_object() {
            return;
        }
        let counts = self.usage.get_or_insert([0; 4]);
        let fields = [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "output_tokens",
        ];
        for (count, field) in counts.iter_mut().zip(fields) {
            if let Some(n) = usage[field].as_u64() {
                *count = n;
            }
        }
    }

    /// The reply once the stream ended; a reply cut short or stopped early is a failure, so its
    /// tool calls never run.
    pub(super) fn finish(self) -> Result<Reply, Failure> {
        if !self.stopped {
            return Err(Failure::new(
                "stream ended before the response completed",
                true,
            ));
        }
        if self.blocks.iter().any(|block| !block.done) {
            return Err(malformed("unfinished content block"));
        }
        match self.stop_reason.as_deref() {
            None | Some("end_turn" | "tool_use" | "stop_sequence" | "pause_turn") => {}
            Some("max_tokens") => {
                return Err(Failure::new(
                    "response incomplete: it reached the output token limit (max_tokens)",
                    false,
                ));
            }
            Some(reason) => {
                return Err(Failure::provider(
                    None,
                    format!("response stopped: {reason}"),
                ));
            }
        }
        let id = match self.id.as_str() {
            "" => "msg".to_owned(),
            id => tool_id(&json!(id)),
        };
        let items = self
            .blocks
            .into_iter()
            .enumerate()
            .filter_map(|(i, block)| match block.kind {
                Kind::Text if !block.text.is_empty() => Some(json!({
                    "type": "message",
                    "id": format!("{}_{i}", &id[..id.len().min(56)]),
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type":"output_text","text":block.text,"annotations":[]}],
                })),
                Kind::Thinking { signature } if !block.text.is_empty() || !signature.is_empty() => {
                    Some(json!({
                        "type": "reasoning",
                        "summary": [{"type":"summary_text","text":block.text}],
                        "signature": signature,
                    }))
                }
                Kind::Redacted(data) => {
                    Some(json!({"type":"reasoning","summary":[],"redacted":data}))
                }
                Kind::Tool { id, name, input } => Some(json!({
                    "type": "function_call",
                    "call_id": id,
                    "name": name,
                    "arguments": if block.text.is_empty() { input.to_string() } else { block.text },
                    "status": "completed",
                })),
                _ => None,
            })
            .collect();
        let usage = self.usage.map(|[input, read, write, output]| {
            let input = input + read + write;
            crate::http::responses::ProviderUsage {
                input_tokens: Some(input),
                output_tokens: Some(output),
                total_tokens: Some(input + output),
                cached_input_tokens: Some(read),
                reasoning_tokens: None,
            }
        });
        Ok(Reply { items, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_thinking_is_for_claude_4_6_and_later() {
        for model in [
            "claude-opus-5-5",
            "claude-haiku-5-5",
            "claude-sonnet-4-6",
            "claude-opus-5",
        ] {
            assert!(adaptive_thinking(model), "{model}");
        }
        for model in [
            "claude-sonnet-4-5",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-20250514",
            "claude-3-7-sonnet-20250219",
            "gpt-6-luna",
            "deepseek-v4-pro",
        ] {
            assert!(!adaptive_thinking(model), "{model}");
        }
    }

    #[test]
    fn other_models_history_becomes_alternating_messages() {
        let request = json!({
            "model": "claude-opus-5-5",
            "instructions": "Be brief.",
            "max_output_tokens": 500,
            "reasoning": {"summary": "auto", "effort": "minimal"},
            "input": [
                {"role":"user","content":"first"},
                {"type":"reasoning","encrypted_content":"opaque","summary":[]},
                {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Looking"}]},
                {"type":"function_call","call_id":"call|1","name":"bash","arguments":"{\"command\":\"ls\"}"},
                {"type":"function_call_output","call_id":"call|1","output":"a.txt"},
                {"type":"function_call","call_id":"call_2","name":"read","arguments":"not json"},
                {"type":"function_call","call_id":"call_3","name":"read","arguments":"{}"},
                {"type":"function_call_output","call_id":"call_2","output":"Error: bad"},
                {"role":"user","content":"  "},
                {"role":"user","content":"second"},
            ],
        });
        let body = super::request(&request, 9, true).unwrap();
        assert_eq!(body["max_tokens"], 500);
        assert_eq!(body["output_config"], json!({"effort":"low"}));
        assert!(body.get("tools").is_none());
        assert_eq!(
            body["messages"],
            json!([
                {"role":"user","content":[{"type":"text","text":"first"}]},
                {"role":"assistant","content":[
                    {"type":"text","text":"Looking"},
                    {"type":"tool_use","id":"call_1","name":"bash","input":{"command":"ls"}},
                ]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"a.txt"}]},
                {"role":"assistant","content":[
                    {"type":"tool_use","id":"call_2","name":"read","input":{}},
                    {"type":"tool_use","id":"call_3","name":"read","input":{}},
                ]},
                {"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"call_2","content":"Error: bad"},
                    {"type":"tool_result","tool_use_id":"call_3","content":"No result provided","is_error":true},
                    {"type":"text","text":"second","cache_control":{"type":"ephemeral"}},
                ]},
            ])
        );
        let plain = super::request(&json!({"model":"claude-sonnet-4-5","input":[]}), 9, false);
        let plain = plain.unwrap();
        assert_eq!(plain["max_tokens"], 9);
        assert!(plain.get("thinking").is_none());
        let deepseek = json!({"model":"deepseek-v4-pro","input":[]});
        assert!(
            super::request(&deepseek, 9, true)
                .unwrap()
                .get("thinking")
                .is_none()
        );
        let none = json!({"model":"m","input":[],"reasoning":{"effort":"none"}});
        assert!(
            super::request(&none, 9, true)
                .unwrap()
                .get("thinking")
                .is_none()
        );
    }

    #[test]
    fn older_claude_models_get_budget_thinking_by_effort() {
        let budgets = [
            (None, 8192),
            (Some("minimal"), 1024),
            (Some("low"), 2048),
            (Some("medium"), 8192),
            (Some("high"), 16384),
            (Some("xhigh"), 16384),
            (Some("max"), 16384),
        ];
        for (effort, expected) in budgets {
            let mut request =
                json!({"model":"claude-sonnet-4-5","input":[],"max_output_tokens":40000});
            if let Some(effort) = effort {
                request["reasoning"] = json!({"effort":effort});
            }
            let body = super::request(&request, DEFAULT_MAX_TOKENS, true).unwrap();
            assert_eq!(body["max_tokens"], 40000);
            assert_eq!(
                body["thinking"],
                json!({"type":"enabled","budget_tokens":expected,"display":"summarized"}),
                "{effort:?}"
            );
            assert!(budget_thinking(&body));
            assert!(body.get("output_config").is_none());
        }
        let card = json!({"model":"claude-haiku-4-5-20251001","input":[]});
        let body = super::request(&card, 4096, true).unwrap();
        assert_eq!(body["max_tokens"], 4096);
        // No effort is medium, 8192, more than the 4096 limit: the budget becomes 4096 - 1024.
        assert_eq!(body["thinking"]["budget_tokens"], 3072);
    }

    #[test]
    fn budget_thinking_leaves_room_for_the_reply_or_fails_before_sending() {
        let request = |max: u64, effort: &str| {
            json!({"model":"claude-sonnet-4-5","input":[],"max_output_tokens":max,
                "reasoning":{"effort":effort}})
        };
        // A limit at or below the budget gives the budget the limit less 1024 tokens.
        let body = super::request(&request(8192, "medium"), 0, true).unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 7168);
        let body = super::request(&request(2048, "high"), 0, true).unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 1024);
        let error = super::request(&request(2047, "high"), 0, true).unwrap_err();
        assert_eq!(
            error,
            "Insufficient output space for Anthropic thinking; reduce input or request more output space."
        );
        assert!(super::request(&request(1024, "low"), 0, true).is_err());
        // Without thinking, the limit alone decides.
        let body = super::request(&request(1000, "low"), 0, false).unwrap();
        assert!(body.get("thinking").is_none());
    }

    fn sse(events: &[Value]) -> Vec<u8> {
        events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\r\ndata: {event}\r\n\r\n",
                    event["type"].as_str().unwrap()
                )
            })
            .collect::<String>()
            .into_bytes()
    }

    fn decode(events: &[Value]) -> (Result<Reply, Failure>, Vec<StreamProgress>) {
        let mut stream = Stream::default();
        let mut updates = Vec::new();
        for chunk in sse(events).chunks(7) {
            if let Err(failure) = stream.feed(chunk, &mut |update| updates.push(update)) {
                return (Err(failure), updates);
            }
        }
        (stream.finish(), updates)
    }

    fn start() -> Value {
        json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}})
    }

    fn block_start(index: u64, block: Value) -> Value {
        json!({"type":"content_block_start","index":index,"content_block":block})
    }

    fn block_stop(index: u64) -> Value {
        json!({"type":"content_block_stop","index":index})
    }

    fn end(reason: &str) -> [Value; 2] {
        [
            json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":9}}),
            json!({"type":"message_stop"}),
        ]
    }

    #[test]
    fn a_streamed_reply_becomes_responses_items() {
        let events = [
            vec![
                start(),
                block_start(0, json!({"type":"redacted_thinking","data":"opaque"})),
                block_stop(0),
                block_start(1, json!({"type":"thinking","thinking":""})),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"Plan é"}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"s"}}),
                block_stop(1),
                block_start(2, json!({"type":"text","text":""})),
                json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"Done"}}),
                block_stop(2),
                block_start(3, json!({"type":"tool_use","id":"toolu_9","name":"bash","input":{}})),
                block_stop(3),
                block_start(4, json!({"type":"server_tool_use","id":"x"})),
                block_stop(4),
            ],
            end("tool_use").to_vec(),
        ]
        .concat();
        let (reply, updates) = decode(&events);
        let reply = reply.ok().unwrap();
        assert_eq!(
            reply.items,
            [
                json!({"type":"reasoning","summary":[],"redacted":"opaque"}),
                json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Plan é"}],"signature":"s"}),
                json!({"type":"message","id":"msg_1_2","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"Done","annotations":[]}]}),
                json!({"type":"function_call","call_id":"toolu_9","name":"bash","arguments":"{}","status":"completed"}),
            ]
        );
        assert_eq!(
            updates,
            [
                StreamProgress::Reasoning("Plan é".into()),
                StreamProgress::Reasoning("\n\n".into()),
                StreamProgress::Text("Done".into()),
                StreamProgress::Call("bash".into()),
            ]
        );
        let usage = reply.usage.unwrap();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.total_tokens),
            (Some(3), Some(9), Some(12))
        );
    }

    #[test]
    fn replies_cut_short_stopped_early_or_failed_are_failures() {
        let text = [
            start(),
            block_start(0, json!({"type":"text","text":"partial"})),
            block_stop(0),
        ];
        let fail = |events: Vec<Value>| decode(&events).0.err().unwrap();
        let cut = fail(text.to_vec());
        assert!(cut.transient && cut.message.contains("ended before"));
        let limit = fail([&text[..], &end("max_tokens")].concat());
        assert!(!limit.transient && limit.message.contains("max_tokens"));
        let refusal = fail([&text[..], &end("refusal")].concat());
        assert!(!refusal.transient && !refusal.overflow);
        assert!(fail([&text[..], &end("model_context_window_exceeded")].concat()).overflow);
        let unfinished = [&text[..2], &end("end_turn")].concat();
        assert!(
            fail(unfinished)
                .message
                .contains("unfinished content block")
        );
        let overloaded =
            json!({"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}});
        let failure = fail(vec![start(), overloaded]);
        assert!(failure.transient);
        assert_eq!(failure.message, "provider error: Overloaded");
        let invalid = json!({"type":"error","error":{"type":"invalid_request_error","message":"retry later"}});
        assert!(!fail(vec![invalid]).transient);
    }
}
