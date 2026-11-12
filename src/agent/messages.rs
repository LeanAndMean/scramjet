//! Anthropic Messages (`/v1/messages`), which gateways serve Claude models on. Requests are
//! converted from the Responses request the agent builds, and replies back into Responses items,
//! so a session keeps one item format whichever protocol produced its items.
use super::StreamProgress;
use super::client::{Failure, LIMITS, Reply};
use serde_json::{Value, json};
use std::collections::HashSet;

/// Used without a model card to say how much a reply may have.
pub(super) const DEFAULT_MAX_TOKENS: u64 = 32_000;

/// The beta budget thinking needs; adaptive models have interleaved thinking built in.
const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";

/// The beta that streams tool input as it is written instead of buffering it, which Scramjet's
/// gateway configuration asks for in place of `eager_input_streaming` on each tool.
const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";

/// Least output space left for a reply when thinking takes a budget, as Scramjet keeps it.
const MIN_OUTPUT_TOKENS: u64 = 1024;

/// Whether `model` takes adaptive thinking: Claude 4.6 and later (gateways rewrite budget thinking
/// for 4.7 and later, which reject it), also as GitHub Copilot names them (`claude-opus-4.7`).
pub(super) fn adaptive_thinking(model: &str) -> bool {
    let Some(rest) = model.strip_prefix("claude-") else {
        return false;
    };
    let mut parts = rest.split(['-', '.']);
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

/// The `anthropic-beta` header `body` needs, if any, in Scramjet's order.
pub(super) fn betas(body: &Value) -> Option<String> {
    let mut betas = Vec::new();
    if body["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty())
    {
        betas.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if body["thinking"]["type"] == "enabled" {
        betas.push(INTERLEAVED_THINKING_BETA);
    }
    (!betas.is_empty()).then(|| betas.join(","))
}

/// Scramjet's long cache retention, which its coding agent uses by default: written prefixes stay
/// cached for an hour instead of five minutes.
fn cache_control() -> Value {
    json!({"type":"ephemeral","ttl":"1h"})
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
/// error. Other models, such as `deepseek-v4-pro`, get no thinking parameter. `renamed`: the
/// text the model's `thinkingLevelMap` sends for the effort, as Scramjet sends it for adaptive
/// thinking.
pub(super) fn request(
    request: &Value,
    max_tokens: u64,
    thinking: bool,
    renamed: Option<&str>,
) -> Result<Value, String> {
    let items = request["input"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let max_tokens = request["max_output_tokens"].as_u64().unwrap_or(max_tokens);
    let mut body = json!({
        "model": request["model"],
        "max_tokens": max_tokens,
        "stream": true,
        "system": [{"type":"text","text":request["instructions"],"cache_control":cache_control()}],
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
                let effort = match renamed {
                    Some(text) => text,
                    None if effort == "minimal" => "low",
                    None => effort,
                };
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

/// `body` with the cache markers `request` puts on the system prompt and the last message as
/// `retention` asks: an hour's for long, Anthropic's default five minutes for short, none for none.
pub(super) fn retained(mut body: Value, retention: super::CacheRetention) -> Value {
    if retention == super::CacheRetention::Long {
        return body;
    }
    let retain = |block: &mut Value| match retention {
        super::CacheRetention::None => {
            (block.as_object_mut()).map(|block| block.remove("cache_control"));
        }
        _ => {
            let control = block
                .get_mut("cache_control")
                .and_then(Value::as_object_mut);
            control.map(|control| control.remove("ttl"));
        }
    };
    body["system"]
        .as_array_mut()
        .into_iter()
        .flatten()
        .for_each(retain);
    let messages = body["messages"].as_array_mut().into_iter().flatten();
    (messages.flat_map(|message| message["content"].as_array_mut().into_iter().flatten()))
        .for_each(retain);
    body
}

/// Responses items as Messages: consecutive items of one role share a message, and as in
/// Scramjet, a tool call without a result (a turn cancelled between calls) gets an error result.
fn messages(items: &[Value]) -> Vec<Value> {
    let mut messages: Vec<(&str, Vec<Value>)> = Vec::new();
    for (role, added) in items.iter().filter_map(blocks) {
        match messages.last_mut() {
            Some((last, blocks)) if *last == role => blocks.extend(added),
            _ => messages.push((role, added)),
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
        last["cache_control"] = cache_control();
    }
    messages
        .into_iter()
        .map(|(role, content)| json!({"role":role,"content":content}))
        .collect()
}

/// A conversation item as Messages content blocks, with the role of the message they belong to:
/// one, or a user message's text and images.
fn blocks(item: &Value) -> Option<(&'static str, Vec<Value>)> {
    if item["role"] == "user"
        && let Some(parts) = item["content"].as_array()
    {
        let blocks: Vec<Value> = parts.iter().filter_map(content_block).collect();
        return (!blocks.is_empty()).then_some(("user", blocks));
    }
    let (role, block) = block(item)?;
    Some((role, vec![block]))
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
            let content = match &item["output"] {
                Value::Array(parts) => parts.iter().filter_map(content_block).collect(),
                output => output.clone(),
            };
            return Some((
                "user",
                json!({"type":"tool_result","tool_use_id":tool_id(&item["call_id"]),"content":content}),
            ));
        }
        _ => return None,
    };
    Some((role, block))
}

/// A Responses content part of a tool result or user message as a Messages content block: text,
/// or an image given as a data URL.
fn content_block(part: &Value) -> Option<Value> {
    match part["type"].as_str()? {
        "input_text" => {
            let text = part["text"].as_str().filter(|text| !text.is_empty())?;
            Some(json!({"type":"text","text":text}))
        }
        "input_image" => {
            let url = part["image_url"].as_str()?.strip_prefix("data:")?;
            let (mime, data) = url.split_once(";base64,")?;
            Some(json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}}))
        }
        _ => None,
    }
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
        // As Anthropic's SDK, which reads only the events it knows: a proxy may end with `[DONE]`.
        if data.trim() == "[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_str(data).map_err(|_| {
            let shown: String = data.chars().take(200).collect();
            malformed(&format!("invalid JSON: {shown}"))
        })?;
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
    /// tool calls never run. One stopped at the output token limit keeps its text (and the
    /// reasoning with it) without its tool calls, whose input may be cut off.
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
        let limited = match self.stop_reason.as_deref() {
            None | Some("end_turn" | "tool_use" | "stop_sequence" | "pause_turn") => false,
            Some("max_tokens") => true,
            Some(reason) => {
                return Err(Failure::provider(
                    Some(reason),
                    format!("response stopped: {reason}"),
                ));
            }
        };
        let id = match self.id.as_str() {
            "" => "msg".to_owned(),
            id => tool_id(&json!(id)),
        };
        let mut items: Vec<Value> = self
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
        let incomplete = limited.then(|| super::client::cut_at_limit(&mut items, "max_tokens"));
        let usage = self.usage.map(|[input, read, write, output]| {
            let input = input + read + write;
            crate::http::responses::ProviderUsage {
                input_tokens: Some(input),
                output_tokens: Some(output),
                total_tokens: Some(input + output),
                cached_input_tokens: Some(read),
                cache_write_tokens: Some(write),
                reasoning_tokens: None,
            }
        });
        Ok(Reply {
            items,
            usage,
            incomplete,
            ..Reply::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::CacheRetention;

    #[test]
    fn adaptive_thinking_is_for_claude_4_6_and_later() {
        for model in [
            "claude-opus-5-5",
            "claude-haiku-5-5",
            "claude-sonnet-4-6",
            "claude-opus-5",
            "claude-opus-4.7",
            "claude-sonnet-5.5",
        ] {
            assert!(adaptive_thinking(model), "{model}");
        }
        for model in [
            "claude-sonnet-4-5",
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-20250514",
            "claude-3-7-sonnet-20250219",
            "claude-haiku-4.5",
            "gpt-6-luna",
            "deepseek-v4-pro",
        ] {
            assert!(!adaptive_thinking(model), "{model}");
        }
    }

    #[test]
    fn an_image_in_a_tool_result_is_sent_as_an_image_block() {
        let output = json!([
            {"type":"input_text","text":"Read image file [image/png]"},
            {"type":"input_image","detail":"auto","image_url":"data:image/png;base64,iVBORw0K"},
        ]);
        let request = json!({"model":"claude-opus-5-5","input":[
            {"type":"function_call","call_id":"c","name":"read","arguments":"{\"path\":\"a.png\"}"},
            {"type":"function_call_output","call_id":"c","output":output},
        ]});
        let body = super::request(&request, 9, false, None).unwrap();
        assert_eq!(
            body["messages"][1],
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"c","content":[
                {"type":"text","text":"Read image file [image/png]"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0K"}},
            ],"cache_control":{"type":"ephemeral","ttl":"1h"}}]})
        );
    }

    #[test]
    fn a_user_messages_images_follow_its_text() {
        let request = json!({"model":"claude-opus-5-5","input":[
            {"role":"user","content":[
                {"type":"input_text","text":"<file name=\"/p/a.png\"></file>\nWhat is it?"},
                {"type":"input_image","detail":"auto","image_url":"data:image/png;base64,iVBORw0K"},
            ],"_scramjet_typed":"@a.png What is it?"},
        ]});
        let body = super::request(&request, 9, false, None).unwrap();
        assert_eq!(
            body["messages"],
            json!([{"role":"user","content":[
                {"type":"text","text":"<file name=\"/p/a.png\"></file>\nWhat is it?"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0K"},
                 "cache_control":{"type":"ephemeral","ttl":"1h"}},
            ]}])
        );
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
        let body = super::request(&request, 9, true, None).unwrap();
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
                    {"type":"text","text":"second","cache_control":{"type":"ephemeral","ttl":"1h"}},
                ]},
            ])
        );
        let plain = super::request(
            &json!({"model":"claude-sonnet-4-5","input":[]}),
            9,
            false,
            None,
        );
        let plain = plain.unwrap();
        assert_eq!(plain["max_tokens"], 9);
        assert!(plain.get("thinking").is_none());
        assert_eq!(betas(&plain), None);
        let deepseek = json!({"model":"deepseek-v4-pro","input":[]});
        assert!(
            super::request(&deepseek, 9, true, None)
                .unwrap()
                .get("thinking")
                .is_none()
        );
        let none = json!({"model":"m","input":[],"reasoning":{"effort":"none"}});
        assert!(
            super::request(&none, 9, true, None)
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
            let body = super::request(&request, DEFAULT_MAX_TOKENS, true, None).unwrap();
            assert_eq!(body["max_tokens"], 40000);
            assert_eq!(
                body["thinking"],
                json!({"type":"enabled","budget_tokens":expected,"display":"summarized"}),
                "{effort:?}"
            );
            assert_eq!(betas(&body).as_deref(), Some(INTERLEAVED_THINKING_BETA));
            assert!(body.get("output_config").is_none());
        }
        let card = json!({"model":"claude-haiku-4-5-20251001","input":[]});
        let body = super::request(&card, 4096, true, None).unwrap();
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
        let body = super::request(&request(8192, "medium"), 0, true, None).unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 7168);
        let body = super::request(&request(2048, "high"), 0, true, None).unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 1024);
        let error = super::request(&request(2047, "high"), 0, true, None).unwrap_err();
        assert_eq!(
            error,
            "Insufficient output space for Anthropic thinking; reduce input or request more output space."
        );
        assert!(super::request(&request(1024, "low"), 0, true, None).is_err());
        // Without thinking, the limit alone decides.
        let body = super::request(&request(1000, "low"), 0, false, None).unwrap();
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
        let refusal = fail([&text[..], &end("refusal")].concat());
        assert!(!refusal.transient && !refusal.overflow && refusal.refusal);
        let overflow = fail([&text[..], &end("model_context_window_exceeded")].concat());
        assert!(overflow.overflow && !overflow.refusal);
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

    #[test]
    fn a_reply_at_the_output_limit_keeps_its_text_but_not_its_tool_calls() {
        let thinking = [
            block_start(0, json!({"type":"thinking","thinking":"Plan"})),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"s"}}),
            block_stop(0),
        ];
        let text = [
            block_start(1, json!({"type":"text","text":"Writing it"})),
            block_stop(1),
        ];
        let call = |index| {
            [
                block_start(
                    index,
                    json!({"type":"tool_use","id":"toolu_1","name":"write","input":{}}),
                ),
                json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a"}}),
                block_stop(index),
            ]
        };
        let decoded = |blocks: &[&[Value]]| {
            let events = [&[start()][..], &blocks.concat(), &end("max_tokens")].concat();
            decode(&events).0.ok().unwrap()
        };
        let reply = decoded(&[&thinking, &text, &call(2)]);
        assert_eq!(
            reply.items,
            [
                json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Plan"}],"signature":"s"}),
                json!({"type":"message","id":"msg_1_1","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"Writing it","annotations":[]}]}),
            ]
        );
        assert_eq!(
            reply.incomplete.as_deref(),
            Some(
                "response incomplete: it reached the output token limit (max_tokens); the tool \
call it was making did not run"
            )
        );
        assert_eq!(reply.usage.unwrap().output_tokens, Some(9));
        let text_only = decoded(&[&text]);
        assert_eq!(text_only.items.len(), 1);
        assert_eq!(
            text_only.incomplete.as_deref(),
            Some("response incomplete: it reached the output token limit (max_tokens)")
        );
        let mut second = call(3);
        second[0]["content_block"]["id"] = json!("toolu_2");
        let calls = decoded(&[&text, &call(2), &second]);
        assert!(
            calls
                .incomplete
                .unwrap()
                .ends_with("the 2 tool calls in it did not run")
        );
        // Without text there is nothing to keep, not even the reasoning before a cut-off call.
        let no_text = decoded(&[&thinking, &call(1)]);
        assert!(no_text.items.is_empty());
        assert!(no_text.incomplete.is_some());
        let complete = decode(&[&[start()][..], &text, &end("end_turn")].concat()).0;
        assert!(complete.ok().unwrap().incomplete.is_none());
    }

    #[test]
    fn requests_with_tools_ask_for_fine_grained_tool_streaming() {
        let tool = json!({"name":"read","description":"Read","parameters":{"type":"object"}});
        let request = |model: &str, tools: Value| {
            super::request(
                &json!({"model":model,"input":[],"tools":tools,"max_output_tokens":40000}),
                0,
                true,
                None,
            )
            .unwrap()
        };
        let adaptive = request("claude-opus-5-5", json!([tool]));
        assert_eq!(
            betas(&adaptive).as_deref(),
            Some("fine-grained-tool-streaming-2025-05-14")
        );
        assert_eq!(
            adaptive["system"][0]["cache_control"],
            json!({"type":"ephemeral","ttl":"1h"})
        );
        assert!(adaptive["tools"][0].get("cache_control").is_none());
        let budget = request("claude-sonnet-4-5", json!([tool]));
        assert_eq!(
            betas(&budget).as_deref(),
            Some("fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14")
        );
        assert_eq!(betas(&request("claude-opus-5-5", json!([]))), None);
    }

    #[test]
    fn short_retention_drops_the_hour_and_none_the_markers() {
        let request = json!({"model":"claude-test","instructions":"Be brief.","input":[
            {"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}],
            "tools":[{"name":"t","description":"d","parameters":{"type":"object",
                "properties":{"cache_control":{"type":"string"}}}}]});
        let body = super::request(&request, 1000, false, None).unwrap();
        let hour = json!({"type":"ephemeral","ttl":"1h"});
        let marks = |body: &Value| {
            let last = &body["messages"][0]["content"][0];
            (
                body["system"][0].get("cache_control").cloned(),
                last.get("cache_control").cloned(),
            )
        };
        assert_eq!(marks(&body), (Some(hour.clone()), Some(hour)));
        let schema = body["tools"].clone();
        let short = retained(body.clone(), CacheRetention::Short);
        let default = json!({"type":"ephemeral"});
        assert_eq!(marks(&short), (Some(default.clone()), Some(default)));
        let none = retained(body.clone(), CacheRetention::None);
        assert_eq!(marks(&none), (None, None));
        assert_eq!(none["tools"], schema);
        assert_eq!(retained(body.clone(), CacheRetention::Long), body);
    }
}
