//! OpenAI Chat Completions (`/chat/completions`), Scramjet's `openai-completions` api, which open
//! servers (vLLM, Ollama, llama.cpp) and many hosted providers serve. As for Messages, requests
//! are converted from the Responses request the agent builds, and replies back into Responses
//! items. What a server accepts differs: a model's `compat` says, else its provider's name and
//! base URL do, as Scramjet's `detectCompat` tells them apart.
use super::StreamProgress;
use super::client::{self, Failure, LIMITS, Reply};
use super::providers::ModelInfo;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// The delta fields reasoning may come in (llama.cpp's first), the first found taken, as in
/// Scramjet: some servers send it in two of them.
const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

/// Put between tool results and a user message for servers that need an assistant there.
const BRIDGE: &str = "I have processed the tool results.";

/// How a request asks for reasoning, Scramjet's `thinkingFormat`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Thinking {
    /// `reasoning_effort`.
    OpenAi,
    /// `reasoning: {effort}`.
    OpenRouter,
    /// `thinking: {type}`, with `reasoning_effort` when on.
    DeepSeek,
    /// `reasoning: {enabled}`, with `reasoning_effort` when on and taken.
    Together,
    /// `enable_thinking` (z.ai and Qwen).
    Enable,
    /// `chat_template_kwargs.enable_thinking`.
    QwenChatTemplate,
}

/// What a server accepts: Scramjet's `OpenAICompletionsCompat`, resolved.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Compat {
    store: bool,
    developer_role: bool,
    reasoning_effort: bool,
    usage_in_streaming: bool,
    /// `max_completion_tokens` or `max_tokens`.
    max_tokens_field: &'static str,
    tool_result_name: bool,
    assistant_after_tool_result: bool,
    thinking_as_text: bool,
    reasoning_content_on_assistant: bool,
    thinking: Thinking,
    /// OpenRouter's `provider` preferences, for an OpenRouter base URL.
    open_router_routing: Option<Value>,
    /// Vercel AI Gateway's `providerOptions.gateway`, for its base URL.
    vercel_gateway_routing: Option<Value>,
    zai_tool_stream: bool,
    strict_mode: bool,
    anthropic_cache_control: bool,
    pub(super) session_affinity: bool,
    pub(super) long_cache_retention: bool,
    /// OpenAI's own servers, which take `prompt_cache_key` whatever the retention.
    at_openai: bool,
    /// The model's `thinkingLevelMap` names a level for `off`, which is then sent.
    off_named: bool,
}

impl Default for Compat {
    /// What an unknown OpenAI-compatible server is taken to accept.
    fn default() -> Self {
        Self::detect("", "", "", false)
    }
}

impl Compat {
    /// Scramjet's defaults for a provider `name` at `base_url`, for the model `model`.
    fn detect(name: &str, base_url: &str, model: &str, off_named: bool) -> Self {
        let at = |host: &str| base_url.contains(host);
        let zai = name == "zai" || at("api.z.ai");
        let together = name == "together" || at("api.together.ai") || at("api.together.xyz");
        let moonshot = name == "moonshotai" || name == "moonshotai-cn" || at("api.moonshot.");
        let workers_ai = name == "cloudflare-workers-ai" || at("api.cloudflare.com");
        let gateway = name == "cloudflare-ai-gateway" || at("gateway.ai.cloudflare.com");
        let grok = name == "xai" || at("api.x.ai");
        let deepseek = name == "deepseek" || at("deepseek.com");
        let open_router = name == "openrouter" || at("openrouter.ai");
        let non_standard = name == "cerebras"
            || at("cerebras.ai")
            || grok
            || together
            || at("chutes.ai")
            || at("deepseek.com")
            || zai
            || moonshot
            || name == "opencode"
            || at("opencode.ai")
            || workers_ai
            || gateway;
        let max_tokens = at("chutes.ai") || moonshot || gateway || together;
        let thinking = match () {
            _ if deepseek => Thinking::DeepSeek,
            _ if zai => Thinking::Enable,
            _ if together => Thinking::Together,
            _ if open_router => Thinking::OpenRouter,
            _ => Thinking::OpenAi,
        };
        let long_cache_retention = !(together || workers_ai || gateway);
        Self {
            store: !non_standard,
            developer_role: !non_standard,
            reasoning_effort: !grok && !zai && !moonshot && !together && !gateway,
            usage_in_streaming: true,
            max_tokens_field: match max_tokens {
                true => "max_tokens",
                false => "max_completion_tokens",
            },
            tool_result_name: false,
            assistant_after_tool_result: false,
            thinking_as_text: false,
            reasoning_content_on_assistant: deepseek,
            thinking,
            open_router_routing: None,
            vercel_gateway_routing: None,
            zai_tool_stream: false,
            strict_mode: !moonshot && !together && !gateway,
            anthropic_cache_control: name == "openrouter" && model.starts_with("anthropic/"),
            session_affinity: false,
            long_cache_retention,
            at_openai: at("api.openai.com"),
            off_named,
        }
    }

    /// A Responses model's: of its provider's `compat` then its own, only
    /// `supportsLongCacheRetention` (default true), as Scramjet 1 reads; other keys are ignored.
    pub(super) fn read_responses(given: [Option<&Value>; 2]) -> Result<Self, String> {
        let mut compat = Self::default();
        for given in given.into_iter().flatten() {
            let given = given.as_object().ok_or("compat must be an object")?;
            if let Some(value) = given.get("supportsLongCacheRetention") {
                compat.long_cache_retention = (value.as_bool())
                    .ok_or("compat.supportsLongCacheRetention must be true or false")?;
            }
        }
        Ok(compat)
    }

    /// The compatibility of `model`, a model of the provider `name` at `base_url`: `given`, its
    /// provider's `compat` then its own, each field over what was detected.
    pub(super) fn read(
        name: &str,
        base_url: &str,
        model: &Value,
        given: [Option<&Value>; 2],
    ) -> Result<Self, String> {
        let id = model["id"].as_str().unwrap_or_default();
        let off_named = model["thinkingLevelMap"]["off"].is_string();
        let mut compat = Self::detect(name, base_url, id, off_named);
        let mut fields = Map::new();
        for given in given.into_iter().flatten() {
            let given = given.as_object().ok_or("compat must be an object")?;
            fields.extend(given.clone());
        }
        for (key, value) in &fields {
            let flag = || {
                value
                    .as_bool()
                    .ok_or_else(|| format!("compat.{key} must be true or false"))
            };
            let one_of = |values: &[&'static str]| {
                let found = values.iter().find(|v| value.as_str() == Some(**v));
                found
                    .copied()
                    .ok_or_else(|| format!("compat.{key} must be one of {}", values.join(", ")))
            };
            let object = || match value {
                Value::Object(_) => Ok(Some(value.clone())),
                _ => Err(format!("compat.{key} must be an object")),
            };
            match key.as_str() {
                "supportsStore" => compat.store = flag()?,
                "supportsDeveloperRole" => compat.developer_role = flag()?,
                "supportsReasoningEffort" => compat.reasoning_effort = flag()?,
                "supportsUsageInStreaming" => compat.usage_in_streaming = flag()?,
                "maxTokensField" => {
                    compat.max_tokens_field = one_of(&["max_completion_tokens", "max_tokens"])?
                }
                "requiresToolResultName" => compat.tool_result_name = flag()?,
                "requiresAssistantAfterToolResult" => compat.assistant_after_tool_result = flag()?,
                "requiresThinkingAsText" => compat.thinking_as_text = flag()?,
                "requiresReasoningContentOnAssistantMessages" => {
                    compat.reasoning_content_on_assistant = flag()?
                }
                "thinkingFormat" => {
                    let formats = [
                        "openai",
                        "openrouter",
                        "deepseek",
                        "together",
                        "zai",
                        "qwen",
                        "qwen-chat-template",
                    ];
                    compat.thinking = match one_of(&formats)? {
                        "openrouter" => Thinking::OpenRouter,
                        "deepseek" => Thinking::DeepSeek,
                        "together" => Thinking::Together,
                        "zai" | "qwen" => Thinking::Enable,
                        "qwen-chat-template" => Thinking::QwenChatTemplate,
                        _ => Thinking::OpenAi,
                    }
                }
                "openRouterRouting" => {
                    let routing = object()?;
                    compat.open_router_routing =
                        routing.filter(|_| base_url.contains("openrouter.ai"))
                }
                "vercelGatewayRouting" => {
                    let routing = object()?.filter(|_| base_url.contains("ai-gateway.vercel.sh"));
                    let gateway: Map<String, Value> = (routing.iter())
                        .flat_map(|routing| ["only", "order"].map(|k| (k, routing[k].clone())))
                        .filter(|(_, value)| !value.is_null())
                        .map(|(k, value)| (k.to_owned(), value))
                        .collect();
                    compat.vercel_gateway_routing = (!gateway.is_empty()).then(|| json!(gateway))
                }
                "zaiToolStream" => compat.zai_tool_stream = flag()?,
                "supportsStrictMode" => compat.strict_mode = flag()?,
                "cacheControlFormat" => {
                    one_of(&["anthropic"])?;
                    compat.anthropic_cache_control = true
                }
                "sendSessionAffinityHeaders" => compat.session_affinity = flag()?,
                "supportsLongCacheRetention" => compat.long_cache_retention = flag()?,
                _ => return Err(format!("compat.{key} is not a Chat Completions setting")),
            }
        }
        Ok(compat)
    }
}

/// The Chat Completions request for a Responses `request` built by the agent, to the model
/// `info` describes, as Scramjet's `buildParams` makes it.
#[cfg(test)]
pub(super) fn request(request: &Value, info: &ModelInfo) -> Value {
    request_retaining(request, info, super::CacheRetention::Long)
}

/// `request`'s, its prefix cached as `retention` asks: a `prompt_cache_key` and 24 hours, and
/// Anthropic's markers for an hour, for long where the server takes it; none of the markers for
/// none, whose request carries no key.
pub(super) fn request_retaining(
    request: &Value,
    info: &ModelInfo,
    retention: super::CacheRetention,
) -> Value {
    let compat = &info.compat;
    let long = retention == super::CacheRetention::Long && compat.long_cache_retention;
    let items = request["input"].as_array().map_or(&[][..], Vec::as_slice);
    let mut messages = Vec::new();
    if let Some(instructions) = request["instructions"]
        .as_str()
        .filter(|text| !text.is_empty())
    {
        let role = match info.reasoning && compat.developer_role {
            true => "developer",
            false => "system",
        };
        messages.push(json!({"role":role,"content":instructions}));
    }
    messages.extend(Conversation::new(compat, info.reasoning).messages(items));
    let mut body = json!({"model":request["model"],"messages":messages,"stream":true});
    if compat.usage_in_streaming {
        body["stream_options"] = json!({"include_usage":true});
    }
    if compat.store {
        body["store"] = json!(false);
    }
    if (compat.at_openai || long)
        && let Some(key) = request["prompt_cache_key"].as_str()
    {
        body["prompt_cache_key"] = json!(key);
    }
    if long {
        body["prompt_cache_retention"] = json!("24h");
    }
    if let Some(tokens) = request["max_output_tokens"].as_u64().or(info.output) {
        body[compat.max_tokens_field] = json!(tokens);
    }
    match request["tools"]
        .as_array()
        .filter(|tools| !tools.is_empty())
    {
        Some(tools) => {
            body["tools"] = (tools.iter())
                .map(|tool| {
                    let mut function = json!({"name":tool["name"],"description":tool["description"],
                        "parameters":tool["parameters"]});
                    if compat.strict_mode {
                        function["strict"] = json!(false);
                    }
                    json!({"type":"function","function":function})
                })
                .collect();
            if compat.zai_tool_stream {
                body["tool_stream"] = json!(true);
            }
        }
        // Proxies to Anthropic need `tools` when the conversation has calls or results.
        None if (items.iter()).any(|item| {
            item["type"] == "function_call" || item["type"] == "function_call_output"
        }) =>
        {
            body["tools"] = json!([]);
        }
        None => {}
    }
    if compat.anthropic_cache_control && retention != super::CacheRetention::None {
        let mut control = json!({"type":"ephemeral"});
        if long {
            control["ttl"] = json!("1h");
        }
        cache_control(&mut body, &control);
    }
    let effort = request["reasoning"]["effort"].as_str();
    if info.reasoning
        && let Some(effort) = effort
    {
        reasoning(&mut body, compat, effort, info.sent(effort));
    }
    if let Some(routing) = &compat.open_router_routing {
        body["provider"] = routing.clone();
    }
    if let Some(gateway) = &compat.vercel_gateway_routing {
        body["providerOptions"] = json!({"gateway":gateway});
    }
    body
}

/// Asks for `level` (`none` is off), sent as `effort`, the model's `thinkingLevelMap`'s text.
fn reasoning(body: &mut Value, compat: &Compat, level: &str, effort: &str) {
    let on = (level != "none").then_some(effort);
    match compat.thinking {
        Thinking::Enable => body["enable_thinking"] = json!(on.is_some()),
        Thinking::QwenChatTemplate => {
            body["chat_template_kwargs"] =
                json!({"enable_thinking":on.is_some(),"preserve_thinking":true})
        }
        Thinking::DeepSeek => {
            let kind = if on.is_some() { "enabled" } else { "disabled" };
            body["thinking"] = json!({"type":kind});
            if let Some(effort) = on {
                body["reasoning_effort"] = json!(effort);
            }
        }
        Thinking::OpenRouter => body["reasoning"] = json!({"effort":effort}),
        Thinking::Together => {
            body["reasoning"] = json!({"enabled":on.is_some()});
            if let Some(effort) = on.filter(|_| compat.reasoning_effort) {
                body["reasoning_effort"] = json!(effort);
            }
        }
        Thinking::OpenAi if compat.reasoning_effort && (on.is_some() || compat.off_named) => {
            body["reasoning_effort"] = json!(effort)
        }
        Thinking::OpenAi => {}
    }
}

/// Anthropic's cache markers, for servers that pass them on: on the system prompt, the last
/// tool, and the last user or assistant message with text.
fn cache_control(body: &mut Value, control: &Value) {
    let Some(messages) = body["messages"].as_array_mut() else {
        return;
    };
    if let Some(system) = (messages.iter_mut())
        .find(|message| message["role"] == "system" || message["role"] == "developer")
    {
        mark(system, control);
    }
    (messages.iter_mut().rev())
        .filter(|message| message["role"] == "user" || message["role"] == "assistant")
        .any(|message| mark(message, control));
    if let Some(last) = body["tools"]
        .as_array_mut()
        .and_then(|tools| tools.last_mut())
    {
        last["cache_control"] = control.clone();
    }
}

/// Marks `message`'s last text for caching; whether it had any.
fn mark(message: &mut Value, control: &Value) -> bool {
    if let Some(text) = message["content"].as_str().filter(|text| !text.is_empty()) {
        message["content"] = json!([{"type":"text","text":text,"cache_control":control}]);
        return true;
    }
    let parts = message["content"].as_array_mut().into_iter().flatten();
    match parts.rev().find(|part| part["type"] == "text") {
        Some(part) => {
            part["cache_control"] = control.clone();
            true
        }
        None => false,
    }
}

/// An assistant message being gathered from a reply's items.
#[derive(Default)]
struct Draft {
    text: String,
    thinking: Vec<String>,
    /// The delta field its reasoning came in.
    field: Option<&'static str>,
    calls: Vec<Value>,
}

/// Responses items as Chat Completions messages, as Scramjet's `convertMessages`: a reply's
/// items share an assistant message, and a tool call without a result (a turn cancelled between
/// calls) gets an error result.
struct Conversation<'a> {
    compat: &'a Compat,
    reasoning: bool,
    messages: Vec<Value>,
    draft: Option<Draft>,
    /// The calls of the last assistant message that have no result yet.
    unanswered: Vec<String>,
    names: HashMap<String, String>,
    /// The images of the tool results since the last assistant message, sent after them.
    images: Vec<Value>,
    after_results: bool,
}

impl<'a> Conversation<'a> {
    fn new(compat: &'a Compat, reasoning: bool) -> Self {
        Self {
            compat,
            reasoning,
            messages: Vec::new(),
            draft: None,
            unanswered: Vec::new(),
            names: HashMap::new(),
            images: Vec::new(),
            after_results: false,
        }
    }

    fn messages(mut self, items: &[Value]) -> Vec<Value> {
        for item in items {
            self.item(item);
        }
        self.end_reply();
        self.end_results();
        self.messages
    }

    fn item(&mut self, item: &Value) {
        match (item["role"].as_str(), item["type"].as_str()) {
            (Some("user"), _) => self.user(item),
            (_, Some("message")) => {
                let text = message_text(item);
                if !text.trim().is_empty() {
                    self.draft().text.push_str(&text);
                }
            }
            (_, Some("reasoning")) => {
                let parts = item["summary"].as_array().into_iter().flatten();
                let text: String = parts.filter_map(|part| part["text"].as_str()).collect();
                if text.trim().is_empty() {
                    return;
                }
                let field = item["reasoning_field"].as_str();
                let field = REASONING_FIELDS
                    .into_iter()
                    .find(|known| Some(*known) == field);
                let draft = self.draft();
                draft.thinking.push(text);
                draft.field = draft.field.or(field);
            }
            (_, Some("function_call")) => {
                let id = item["call_id"].as_str().unwrap_or_default().to_owned();
                let name = item["name"].as_str().unwrap_or_default().to_owned();
                let arguments = (item["arguments"].as_str())
                    .filter(|text| serde_json::from_str::<Value>(text).is_ok_and(|v| v.is_object()))
                    .unwrap_or("{}");
                let call = json!({"id":id,"type":"function",
                    "function":{"name":name,"arguments":arguments}});
                self.names.insert(id, name);
                self.draft().calls.push(call);
            }
            (_, Some("function_call_output")) => self.result(item),
            _ => {}
        }
    }

    /// The reply being gathered, started after the results of the one before.
    fn draft(&mut self) -> &mut Draft {
        if self.draft.is_none() {
            self.end_results();
        }
        self.draft.get_or_insert_default()
    }

    fn user(&mut self, item: &Value) {
        let content = match &item["content"] {
            Value::String(text) if !text.is_empty() => json!(text),
            Value::Array(parts) => {
                let parts: Vec<Value> = parts.iter().filter_map(content_part).collect();
                if parts.is_empty() {
                    return;
                }
                json!(parts)
            }
            _ => return,
        };
        self.end_reply();
        self.end_results();
        self.bridge();
        self.messages.push(json!({"role":"user","content":content}));
    }

    fn result(&mut self, item: &Value) {
        self.end_reply();
        let id = item["call_id"].as_str().unwrap_or_default();
        let (text, images) = match &item["output"] {
            Value::String(text) => (text.clone(), Vec::new()),
            Value::Array(parts) => {
                let texts = parts.iter().filter(|part| part["type"] == "input_text");
                let texts: Vec<&str> = texts.filter_map(|part| part["text"].as_str()).collect();
                let images = parts.iter().filter(|part| part["type"] == "input_image");
                (texts.join("\n"), images.filter_map(content_part).collect())
            }
            output => (output.to_string(), Vec::new()),
        };
        let text = match text.is_empty() && !images.is_empty() {
            true => "(see attached image)".to_owned(),
            false => text,
        };
        self.images.extend(images);
        self.unanswered.retain(|unanswered| unanswered != id);
        self.tool(id, &text);
    }

    fn tool(&mut self, id: &str, text: &str) {
        let mut message = json!({"role":"tool","content":text,"tool_call_id":id});
        if self.compat.tool_result_name
            && let Some(name) = self.names.get(id)
        {
            message["name"] = json!(name);
        }
        self.messages.push(message);
        self.after_results = true;
    }

    /// An assistant message between tool results and a user message, for servers that need one.
    fn bridge(&mut self) {
        if self.after_results && self.compat.assistant_after_tool_result {
            self.messages
                .push(json!({"role":"assistant","content":BRIDGE}));
        }
        self.after_results = false;
    }

    /// Results for the calls left without one, then the results' images in a user message.
    fn end_results(&mut self) {
        for id in std::mem::take(&mut self.unanswered) {
            self.tool(&id, "No result provided");
        }
        if !self.images.is_empty() {
            self.bridge();
            let mut content =
                vec![json!({"type":"text","text":"Attached image(s) from tool result:"})];
            content.append(&mut self.images);
            self.messages.push(json!({"role":"user","content":content}));
        }
    }

    /// The gathered reply as an assistant message, unless it has nothing to say.
    fn end_reply(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let compat = self.compat;
        let mut message = json!({"role":"assistant","content":null});
        if compat.assistant_after_tool_result {
            message["content"] = json!("");
        }
        if !draft.thinking.is_empty() && compat.thinking_as_text {
            let mut parts = vec![json!({"type":"text","text":draft.thinking.join("\n\n")})];
            if !draft.text.is_empty() {
                parts.push(json!({"type":"text","text":draft.text}));
            }
            message["content"] = json!(parts);
        } else {
            if !draft.text.is_empty() {
                message["content"] = json!(draft.text);
            }
            if let Some(field) = draft.field {
                message[field] = json!(draft.thinking.join("\n"));
            }
        }
        if compat.reasoning_content_on_assistant
            && self.reasoning
            && message.get("reasoning_content").is_none()
        {
            message["reasoning_content"] = json!("");
        }
        let content = &message["content"];
        let said = content.as_str().is_some_and(|text| !text.is_empty())
            || content.as_array().is_some_and(|parts| !parts.is_empty());
        if !said && draft.calls.is_empty() {
            return;
        }
        if !draft.calls.is_empty() {
            let ids = draft.calls.iter().filter_map(|call| call["id"].as_str());
            self.unanswered = ids.map(str::to_owned).collect();
            message["tool_calls"] = json!(draft.calls);
        }
        self.after_results = false;
        self.messages.push(message);
    }
}

/// An assistant message item's text, its refusals included.
fn message_text(item: &Value) -> String {
    match &item["content"] {
        Value::String(text) => text.clone(),
        content => (content.as_array().into_iter().flatten())
            .filter_map(|part| part["text"].as_str().or(part["refusal"].as_str()))
            .collect(),
    }
}

/// A Responses content part of a user message or tool result as a Chat Completions part.
fn content_part(part: &Value) -> Option<Value> {
    match part["type"].as_str()? {
        "input_text" => Some(json!({"type":"text","text":part["text"].as_str()?})),
        "input_image" => {
            Some(json!({"type":"image_url","image_url":{"url":part["image_url"].as_str()?}}))
        }
        _ => None,
    }
}

#[derive(Debug, Default)]
struct Call {
    /// The `index` deltas name it by.
    index: Option<u64>,
    id: String,
    name: String,
    arguments: String,
}

/// Decodes a streamed Chat Completions reply as it arrives.
#[derive(Debug)]
pub(super) struct Stream {
    /// The request's output token limit, as named in the request.
    limit: &'static str,
    line: Vec<u8>,
    data: String,
    text: String,
    thinking: String,
    field: Option<&'static str>,
    calls: Vec<Call>,
    usage: Option<Value>,
    finish: Option<String>,
}

fn malformed(what: &str) -> Failure {
    Failure::new(format!("unexpected response stream ({what})"), false)
}

/// Whether a `finish_reason` ends a reply that may be used.
fn usable(reason: &str) -> bool {
    matches!(
        reason,
        "stop" | "end" | "tool_calls" | "function_call" | "length"
    )
}

impl Stream {
    pub(super) fn new(compat: &Compat) -> Self {
        Self {
            limit: compat.max_tokens_field,
            line: Vec::new(),
            data: String::new(),
            text: String::new(),
            thinking: String::new(),
            field: None,
            calls: Vec::new(),
            usage: None,
            finish: None,
        }
    }

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
                self.end_event(progress)?;
            } else if let Some(value) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        Ok(())
    }

    fn end_event(&mut self, progress: &mut dyn FnMut(StreamProgress)) -> Result<(), Failure> {
        let data = std::mem::take(&mut self.data);
        match data.trim() {
            "" | "[DONE]" => Ok(()),
            data => self.chunk(data, progress),
        }
    }

    fn chunk(
        &mut self,
        data: &str,
        progress: &mut dyn FnMut(StreamProgress),
    ) -> Result<(), Failure> {
        let chunk: Value = serde_json::from_str(data).map_err(|_| malformed("invalid JSON"))?;
        if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
            let message = error["message"]
                .as_str()
                .or(error.as_str())
                .unwrap_or("no message");
            let code = error["code"].as_str().or(error["type"].as_str());
            return Err(Failure::provider(
                code,
                format!("provider error: {message}"),
            ));
        }
        if chunk["usage"].is_object() {
            self.usage = Some(chunk["usage"].clone());
        }
        let Some(choice) = chunk["choices"].get(0) else {
            return Ok(());
        };
        // Moonshot gives the usage in the choice.
        if !chunk["usage"].is_object() && choice["usage"].is_object() {
            self.usage = Some(choice["usage"].clone());
        }
        if let Some(reason) = choice["finish_reason"].as_str()
            && self.finish.as_deref().is_none_or(usable)
        {
            self.finish = Some(reason.to_owned());
        }
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            self.text.push_str(text);
            progress(StreamProgress::Text(text.to_owned()));
        }
        let reasoning = (REASONING_FIELDS.into_iter())
            .find_map(|field| Some((field, delta[field].as_str().filter(|t| !t.is_empty())?)));
        if let Some((field, text)) = reasoning {
            self.field.get_or_insert(field);
            self.thinking.push_str(text);
            progress(StreamProgress::Reasoning(text.to_owned()));
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = call["index"].as_u64();
            let id = call["id"].as_str().filter(|id| !id.is_empty());
            let found = (self.calls.iter())
                .position(|known| index.is_some() && known.index == index)
                .or_else(|| {
                    self.calls
                        .iter()
                        .position(|known| id.is_some_and(|id| known.id == id))
                });
            let at = found.unwrap_or_else(|| {
                self.calls.push(Call {
                    index,
                    ..Call::default()
                });
                self.calls.len() - 1
            });
            let known = &mut self.calls[at];
            known.index = known.index.or(index);
            if known.id.is_empty()
                && let Some(id) = id
            {
                known.id = id.to_owned();
            }
            let function = &call["function"];
            if known.name.is_empty()
                && let Some(name) = function["name"].as_str().filter(|name| !name.is_empty())
            {
                known.name = name.to_owned();
                progress(StreamProgress::Call(name.to_owned()));
            }
            known
                .arguments
                .push_str(function["arguments"].as_str().unwrap_or_default());
        }
        Ok(())
    }

    /// The reply once the stream ended; one without a `finish_reason` was cut short, and one
    /// stopped for another reason than its end, a tool call or the output token limit failed, so
    /// its tool calls never run. One at the output token limit keeps its text without its calls.
    pub(super) fn finish(mut self) -> Result<Reply, Failure> {
        self.feed(b"\n", &mut |_| {})?;
        self.end_event(&mut |_| {})?;
        let Some(reason) = self.finish.take() else {
            return Err(Failure::new(
                "stream ended before the response completed",
                true,
            ));
        };
        match reason.as_str() {
            reason if usable(reason) => {}
            "network_error" => return Err(Failure::new("response stopped: network_error", true)),
            reason => {
                let code = (reason == "content_filter").then_some(reason);
                return Err(Failure::provider(
                    code,
                    format!("response stopped: {reason}"),
                ));
            }
        }
        let mut items = Vec::new();
        if !self.thinking.is_empty() {
            items.push(json!({"type":"reasoning",
                "summary":[{"type":"summary_text","text":self.thinking}],
                "reasoning_field":self.field}));
        }
        if !self.text.is_empty() {
            items.push(
                json!({"type":"message","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":self.text,"annotations":[]}]}),
            );
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis());
        for (i, call) in self.calls.into_iter().enumerate() {
            // A server that names no call gets ids of its own, which its results pair with.
            let id = match call.id.is_empty() {
                true => format!("call_{stamp:x}_{i}"),
                false => call.id,
            };
            let arguments = match call.arguments.trim().is_empty() {
                true => "{}".to_owned(),
                false => call.arguments,
            };
            items.push(json!({"type":"function_call","call_id":id,"name":call.name,
                "arguments":arguments,"status":"completed"}));
        }
        let incomplete = (reason == "length").then(|| client::cut_at_limit(&mut items, self.limit));
        let usage = self.usage.map(|usage| {
            let count = |value: &Value| value.as_u64();
            let read = count(&usage["prompt_tokens_details"]["cached_tokens"])
                .or(count(&usage["prompt_cache_hit_tokens"]))
                .unwrap_or(0);
            let write = count(&usage["prompt_tokens_details"]["cache_write_tokens"]).unwrap_or(0);
            let input = count(&usage["prompt_tokens"])
                .unwrap_or(0)
                .max(read.saturating_add(write));
            let output = count(&usage["completion_tokens"]).unwrap_or(0);
            crate::http::responses::ProviderUsage {
                input_tokens: Some(input),
                output_tokens: Some(output),
                total_tokens: Some(input.saturating_add(output)),
                cached_input_tokens: Some(read),
                cache_write_tokens: Some(write),
                reasoning_tokens: count(&usage["completion_tokens_details"]["reasoning_tokens"]),
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

    fn model(url: &str, entry: Value) -> ModelInfo {
        let compat = Compat::read("local", url, &entry, [None, entry.get("compat")]).unwrap();
        ModelInfo {
            protocol: super::super::client::Protocol::Completions,
            reasoning: entry["reasoning"].as_bool().unwrap_or(false),
            output: entry["maxTokens"].as_u64(),
            compat,
            ..ModelInfo::guess("m")
        }
    }

    fn tool() -> Value {
        json!({"type":"function","name":"read","description":"Read a file",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}}}})
    }

    /// A reply that reasoned, said something and made two calls, one answered with an image.
    fn history() -> Vec<Value> {
        vec![
            json!({"role":"user","content":[
                {"type":"input_text","text":"What is in a.png?"},
                {"type":"input_image","detail":"auto","image_url":"data:image/png;base64,AAAA"},
            ]}),
            json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Look."}],
                "reasoning_field":"reasoning_content"}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Reading"}]}),
            json!({"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\":\"a.png\"}"}),
            json!({"type":"function_call","call_id":"call_2","name":"read","arguments":"cut {"}),
            json!({"type":"function_call_output","call_id":"call_1","output":[
                {"type":"input_text","text":"Read image file [image/png]"},
                {"type":"input_image","detail":"auto","image_url":"data:image/png;base64,BBBB"},
            ]}),
            json!({"role":"user","content":"And now?"}),
        ]
    }

    fn sent(info: &ModelInfo, effort: Option<&str>) -> Value {
        let mut request = json!({"model":"m","instructions":"Be brief.","input":history(),
            "tools":[tool()],"prompt_cache_key":"session-1"});
        if let Some(effort) = effort {
            request["reasoning"] = json!({"summary":"auto","effort":effort});
        }
        super::request(&request, info)
    }

    #[test]
    fn a_request_is_one_message_a_reply_and_its_results_after_it() {
        let info = model(
            "http://127.0.0.1:8000/v1",
            json!({"id":"m","reasoning":true,"maxTokens":4096}),
        );
        let body = sent(&info, Some("high"));
        assert_eq!(
            body["messages"],
            json!([
                {"role":"developer","content":"Be brief."},
                {"role":"user","content":[
                    {"type":"text","text":"What is in a.png?"},
                    {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}},
                ]},
                {"role":"assistant","content":"Reading","reasoning_content":"Look.","tool_calls":[
                    {"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"path\":\"a.png\"}"}},
                    {"id":"call_2","type":"function","function":{"name":"read","arguments":"{}"}},
                ]},
                {"role":"tool","content":"Read image file [image/png]","tool_call_id":"call_1"},
                {"role":"tool","content":"No result provided","tool_call_id":"call_2"},
                {"role":"user","content":[
                    {"type":"text","text":"Attached image(s) from tool result:"},
                    {"type":"image_url","image_url":{"url":"data:image/png;base64,BBBB"}},
                ]},
                {"role":"user","content":"And now?"},
            ])
        );
        assert_eq!(
            body["tools"],
            json!([{"type":"function","function":{"name":"read","description":"Read a file",
                "parameters":tool()["parameters"],"strict":false}}])
        );
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"], json!({"include_usage":true}));
        assert_eq!(body["store"], false);
        assert_eq!(body["max_completion_tokens"], 4096);
        assert_eq!(body["prompt_cache_key"], "session-1");
        assert_eq!(body["prompt_cache_retention"], "24h");
        assert_eq!(body["reasoning_effort"], "high");
        // Off is the model's default unless its map names a level for it.
        assert!(sent(&info, Some("none")).get("reasoning_effort").is_none());
        assert!(sent(&info, None).get("reasoning_effort").is_none());
        let named = json!({"id":"m","reasoning":true,"thinkingLevelMap":{"off":"none"}});
        let named = model("http://localhost:11434/v1", named);
        assert_eq!(sent(&named, Some("none"))["reasoning_effort"], "none");
        // A model that does not reason gets the system role and nothing of reasoning.
        let plain = model("http://localhost:11434/v1", json!({"id":"m"}));
        let body = sent(&plain, Some("high"));
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("max_completion_tokens").is_none());
        // Without tools, a conversation with calls still says it has none.
        let request = json!({"model":"m","input":history()});
        assert_eq!(super::request(&request, &plain)["tools"], json!([]));
        let request = json!({"model":"m","input":[{"role":"user","content":"hi"}]});
        assert!(super::request(&request, &plain).get("tools").is_none());
    }

    #[test]
    fn compat_says_what_a_server_needs() {
        let entry = json!({"id":"m","reasoning":true,"compat":{
            "requiresToolResultName":true,"requiresAssistantAfterToolResult":true,
            "requiresThinkingAsText":true,"maxTokensField":"max_tokens","supportsStore":false,
            "supportsDeveloperRole":false,"supportsUsageInStreaming":false,
            "supportsStrictMode":false,"supportsLongCacheRetention":false,
            "thinkingFormat":"qwen-chat-template"}});
        let info = model("http://localhost:8080/v1", entry);
        let mut request = json!({"model":"m","input":history(),"tools":[tool()],
            "max_output_tokens":100,"prompt_cache_key":"s","reasoning":{"effort":"low"}});
        let body = super::request(&request, &info);
        let messages = body["messages"].as_array().unwrap();
        let roles: Vec<&str> = messages.iter().filter_map(|m| m["role"].as_str()).collect();
        assert_eq!(
            roles,
            [
                "user",
                "assistant",
                "tool",
                "tool",
                "assistant",
                "user",
                "user"
            ]
        );
        assert_eq!(
            messages[1]["content"],
            json!([{"type":"text","text":"Look."},{"type":"text","text":"Reading"}])
        );
        assert!(messages[1].get("reasoning_content").is_none());
        assert_eq!(messages[2]["name"], "read");
        assert_eq!(messages[4], json!({"role":"assistant","content":BRIDGE}));
        assert_eq!(body["max_tokens"], 100);
        assert_eq!(body["tools"][0]["function"].get("strict"), None);
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking":true,"preserve_thinking":true})
        );
        for absent in [
            "store",
            "stream_options",
            "prompt_cache_key",
            "prompt_cache_retention",
        ] {
            assert!(body.get(absent).is_none(), "{absent}");
        }
        request["instructions"] = json!("Be brief.");
        assert_eq!(
            super::request(&request, &info)["messages"][0]["role"],
            "system"
        );

        // Known hosts are told apart by their URL, as in Scramjet.
        let deepseek = model(
            "https://api.deepseek.com",
            json!({"id":"m","reasoning":true}),
        );
        let body = sent(&deepseek, Some("high"));
        assert_eq!(body["thinking"], json!({"type":"enabled"}));
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body.get("store").is_none());
        let body = sent(&deepseek, Some("none"));
        assert_eq!(body["thinking"], json!({"type":"disabled"}));
        assert!(body.get("reasoning_effort").is_none());
        let message = json!({"type":"message","role":"assistant","content":"Hi"});
        let request = json!({"model":"m","input":[message]});
        assert_eq!(
            super::request(&request, &deepseek)["messages"][0],
            json!({"role":"assistant","content":"Hi","reasoning_content":""})
        );
        let together = model(
            "https://api.together.xyz/v1",
            json!({"id":"m","reasoning":true}),
        );
        let body = sent(&together, Some("low"));
        assert_eq!(body["reasoning"], json!({"enabled":true}));
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("max_tokens").is_none());
        let zai = model(
            "https://api.z.ai/api/paas/v4",
            json!({"id":"m","reasoning":true}),
        );
        assert_eq!(sent(&zai, Some("none"))["enable_thinking"], false);
    }

    #[test]
    fn openrouter_takes_its_routing_and_claude_cache_markers() {
        let entry = json!({"id":"anthropic/claude-opus","reasoning":true,
            "compat":{"openRouterRouting":{"order":["anthropic"]}}});
        let compat = Compat::read(
            "openrouter",
            "https://openrouter.ai/api/v1",
            &entry,
            [None, entry.get("compat")],
        )
        .unwrap();
        let info = ModelInfo {
            reasoning: true,
            compat,
            ..ModelInfo::guess("m")
        };
        let body = sent(&info, Some("medium"));
        assert_eq!(body["reasoning"], json!({"effort":"medium"}));
        assert_eq!(body["provider"], json!({"order":["anthropic"]}));
        let control = json!({"type":"ephemeral","ttl":"1h"});
        assert_eq!(
            body["messages"][0]["content"],
            json!([{"type":"text","text":"Be brief.","cache_control":control}])
        );
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(
            last["content"],
            json!([{"type":"text","text":"And now?","cache_control":control}])
        );
        assert_eq!(body["tools"][0]["cache_control"], control);
        assert_eq!(body["prompt_cache_key"], "session-1");
        assert_eq!(body["prompt_cache_retention"], "24h");

        // Short retention asks for the servers' default; none for no markers.
        let mut request = json!({"model":"m","instructions":"Be brief.","input":history(),
            "tools":[tool()],"prompt_cache_key":"session-1"});
        let body = request_retaining(&request, &info, CacheRetention::Short);
        let control = json!({"type":"ephemeral"});
        assert_eq!(body["tools"][0]["cache_control"], control);
        assert_eq!(body["messages"][0]["content"][0]["cache_control"], control);
        assert!(body.get("prompt_cache_key").is_none());
        assert!(body.get("prompt_cache_retention").is_none());
        // None's request has no key (`request::body`).
        request.as_object_mut().unwrap().remove("prompt_cache_key");
        let body = request_retaining(&request, &info, CacheRetention::None);
        assert!(!body.to_string().contains("cache_control"), "{body}");
        assert_eq!(body["messages"][0]["content"], "Be brief.");
        assert!(body.get("prompt_cache_key").is_none());
    }

    #[test]
    fn openai_takes_the_cache_key_at_any_retention_and_24_hours_at_long() {
        let info = model("https://api.openai.com/v1", json!({"id":"m"}));
        let request = json!({"model":"m","input":history(),"prompt_cache_key":"s"});
        for (retention, kept) in [
            (CacheRetention::Long, Some("24h")),
            (CacheRetention::Short, None),
        ] {
            let body = request_retaining(&request, &info, retention);
            assert_eq!(body["prompt_cache_key"], "s");
            assert_eq!(body["prompt_cache_retention"].as_str(), kept);
        }
    }

    #[test]
    fn a_compat_that_is_not_understood_is_said() {
        let read = |compat: Value| {
            let entry = json!({"id":"m","compat":compat});
            Compat::read(
                "local",
                "http://localhost",
                &entry,
                [None, entry.get("compat")],
            )
        };
        assert_eq!(
            read(json!({"supportsStore":"no"})).unwrap_err(),
            "compat.supportsStore must be true or false"
        );
        assert_eq!(
            read(json!({"thinkingFormat":"mine"})).unwrap_err(),
            "compat.thinkingFormat must be one of openai, openrouter, deepseek, together, zai, \
             qwen, qwen-chat-template"
        );
        assert_eq!(
            read(json!({"supportsFoo":true})).unwrap_err(),
            "compat.supportsFoo is not a Chat Completions setting"
        );
        assert_eq!(read(json!([])).unwrap_err(), "compat must be an object");
        // A model's own fields come over its provider's.
        let entry = json!({"id":"m","compat":{"maxTokensField":"max_tokens"}});
        let provider = json!({"maxTokensField":"max_completion_tokens","supportsStore":false});
        let compat = Compat::read(
            "local",
            "http://localhost",
            &entry,
            [Some(&provider), entry.get("compat")],
        );
        let compat = compat.unwrap();
        assert_eq!(
            (compat.max_tokens_field, compat.store),
            ("max_tokens", false)
        );
    }

    fn sse(chunks: &[Value]) -> Vec<u8> {
        let mut wire: String = (chunks.iter())
            .map(|chunk| format!("data: {chunk}\r\n\r\n"))
            .collect();
        wire.push_str(": keep-alive\n\ndata: [DONE]\n\n");
        wire.into_bytes()
    }

    fn decode(wire: &[u8]) -> (Result<Reply, Failure>, Vec<StreamProgress>) {
        let mut stream = Stream::new(&Compat::default());
        let mut updates = Vec::new();
        for piece in wire.chunks(7) {
            if let Err(failure) = stream.feed(piece, &mut |update| updates.push(update)) {
                return (Err(failure), updates);
            }
        }
        (stream.finish(), updates)
    }

    fn delta(delta: Value) -> Value {
        json!({"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[
            {"index":0,"delta":delta,"finish_reason":null}]})
    }

    fn finish(reason: &str) -> Value {
        json!({"id":"chatcmpl-1","choices":[{"index":0,"delta":{},"finish_reason":reason}]})
    }

    fn usage() -> Value {
        json!({"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":50,"completion_tokens":12,
            "prompt_tokens_details":{"cached_tokens":30},
            "completion_tokens_details":{"reasoning_tokens":4}}})
    }

    #[test]
    fn a_streamed_reply_becomes_responses_items() {
        let wire = sse(&[
            delta(json!({"role":"assistant","content":""})),
            delta(json!({"reasoning_content":"Need é","reasoning":"Need é"})),
            delta(json!({"reasoning_content":" it."})),
            delta(json!({"content":"Reading"})),
            delta(
                json!({"tool_calls":[{"index":0,"id":"call_a","type":"function",
                "function":{"name":"read","arguments":""}}]}),
            ),
            delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]})),
            delta(
                json!({"tool_calls":[{"index":1,"id":"call_b","type":"function",
                "function":{"name":"bash","arguments":"{}"}}]}),
            ),
            delta(json!({"tool_calls":[{"index":0,"function":{"arguments":"\"a\"}"}}]})),
            finish("tool_calls"),
            usage(),
        ]);
        let (reply, updates) = decode(&wire);
        let reply = reply.ok().unwrap();
        assert_eq!(
            reply.items,
            [
                json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Need é it."}],
                    "reasoning_field":"reasoning_content"}),
                json!({"type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"Reading","annotations":[]}]}),
                json!({"type":"function_call","call_id":"call_a","name":"read",
                    "arguments":"{\"path\":\"a\"}","status":"completed"}),
                json!({"type":"function_call","call_id":"call_b","name":"bash","arguments":"{}",
                    "status":"completed"}),
            ]
        );
        assert_eq!(
            updates,
            [
                StreamProgress::Reasoning("Need é".into()),
                StreamProgress::Reasoning(" it.".into()),
                StreamProgress::Text("Reading".into()),
                StreamProgress::Call("read".into()),
                StreamProgress::Call("bash".into()),
            ]
        );
        let usage = reply.usage.unwrap();
        assert_eq!(
            (usage.input_tokens, usage.output_tokens, usage.total_tokens),
            (Some(50), Some(12), Some(62))
        );
        assert_eq!(
            (
                usage.cached_input_tokens,
                usage.cache_write_tokens,
                usage.reasoning_tokens
            ),
            (Some(30), Some(0), Some(4))
        );
        assert!(reply.incomplete.is_none());
        // Replayed to the same model, the reasoning goes back in the field it came in.
        let request = json!({"model":"m","input":reply.items});
        let info = model("http://localhost", json!({"id":"m","reasoning":true}));
        let assistant = &super::request(&request, &info)["messages"][0];
        assert_eq!(assistant["reasoning_content"], "Need é it.");
        assert_eq!(
            assistant["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );
    }

    #[test]
    fn calls_without_ids_get_their_own_and_a_last_event_needs_no_blank_line() {
        let call =
            json!({"tool_calls":[{"function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]});
        let mut wire = sse(&[delta(call), finish("stop")]);
        wire.truncate(wire.len() - "data: [DONE]\n\n".len());
        wire.extend(format!("data: {}", usage()).as_bytes());
        let reply = decode(&wire).0.ok().unwrap();
        let id = reply.items[0]["call_id"].as_str().unwrap();
        assert!(id.starts_with("call_") && id.ends_with("_0"), "{id}");
        assert_eq!(reply.items[0]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(reply.usage.unwrap().input_tokens, Some(50));
    }

    #[test]
    fn replies_cut_short_stopped_early_or_failed_are_failures() {
        let fail = |chunks: &[Value]| decode(&sse(chunks)).0.err().unwrap();
        let text = delta(json!({"content":"partial"}));
        let cut = fail(std::slice::from_ref(&text));
        assert!(
            cut.transient && cut.message.contains("ended before"),
            "{}",
            cut.message
        );
        let filtered = fail(&[text.clone(), finish("content_filter")]);
        assert!(!filtered.transient && filtered.refusal);
        assert_eq!(filtered.message, "response stopped: content_filter");
        assert!(fail(&[text.clone(), finish("network_error")]).transient);
        // The first reason that stops a reply is kept.
        let both = fail(&[finish("content_filter"), finish("stop")]);
        assert_eq!(both.message, "response stopped: content_filter");
        assert!(fail(&[finish("model_context_window_exceeded")]).overflow);
        let error = json!({"error":{"message":"Rate limit reached","type":"rate_limit_error"}});
        let failure = fail(&[text, error]);
        assert!(failure.transient);
        assert_eq!(failure.message, "provider error: Rate limit reached");
        let failure = fail(&[json!({"error":"model not loaded"})]);
        assert_eq!(failure.message, "provider error: model not loaded");
        let invalid = decode(b"data: {not json}\n\n").0.err().unwrap();
        assert!(invalid.message.contains("invalid JSON") && !invalid.transient);
    }

    #[test]
    fn a_reply_at_the_output_limit_keeps_its_text_but_not_its_tool_calls() {
        let call = json!({"tool_calls":[{"index":0,"id":"c","function":{"name":"write",
            "arguments":"{\"path\":\"a"}}]});
        let reply = decode(&sse(&[
            delta(json!({"reasoning":"Plan"})),
            delta(json!({"content":"Writing it"})),
            delta(call.clone()),
            finish("length"),
        ]));
        let reply = reply.0.ok().unwrap();
        let kinds: Vec<&str> = reply
            .items
            .iter()
            .filter_map(|i| i["type"].as_str())
            .collect();
        assert_eq!(kinds, ["reasoning", "message"]);
        assert_eq!(
            reply.incomplete.as_deref(),
            Some(
                "response incomplete: it reached the output token limit \
                 (max_completion_tokens); the tool call it was making did not run"
            )
        );
        let no_text = decode(&sse(&[delta(call), finish("length")]))
            .0
            .ok()
            .unwrap();
        assert!(no_text.items.is_empty() && no_text.incomplete.is_some());
    }
}
