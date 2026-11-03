use super::provider_diagnostic::{ProviderFailure, ProviderFailureStage, ProviderIdentityProblem};
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};
use std::{fmt, io};

pub const PROVIDER_ENVELOPE_BYTES: usize = 16_384;
pub const PROVIDER_FRAME_BYTES: usize = 16_384;
pub const PROVIDER_OUTPUT_ITEMS: usize = 8;

/// Why a stream or value was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Framing,
    Unsupported,
    Incomplete,
    ResourceLimit,
}

/// Incremental output reported while a response streams, for display only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamProgress {
    Text(String),
    Reasoning(String),
    /// A function call with this name started streaming; its arguments follow.
    Call(String),
}

/// Byte and item budgets for one streamed response.
#[derive(Clone, Copy, Debug)]
pub struct ParserLimits {
    pub frame: usize,
    pub envelope: usize,
    pub items: usize,
}
impl ParserLimits {
    #[cfg(test)]
    pub const DEFAULT: Self = Self {
        frame: PROVIDER_FRAME_BYTES,
        envelope: PROVIDER_ENVELOPE_BYTES,
        items: PROVIDER_OUTPUT_ITEMS,
    };
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}
impl ProviderUsage {
    pub(crate) fn validate(&self) -> Result<(), Outcome> {
        if let (Some(input), Some(output), Some(total)) =
            (self.input_tokens, self.output_tokens, self.total_tokens)
            && input.checked_add(output) != Some(total)
        {
            return Err(Outcome::Framing);
        }
        if self
            .cached_input_tokens
            .is_some_and(|n| self.input_tokens.is_none_or(|input| n > input))
            || self
                .reasoning_tokens
                .is_some_and(|n| self.output_tokens.is_none_or(|output| n > output))
        {
            return Err(Outcome::Framing);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderItem {
    wire: Value,
}
impl ProviderItem {
    fn checked(wire: Value, limit: usize) -> Result<Self, Outcome> {
        bounded_json(&wire, limit)?;
        validate_item(&wire, true)?;
        Ok(Self { wire })
    }
    pub(crate) fn raw(&self) -> &Value {
        &self.wire
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderEnvelope {
    pub items: Vec<ProviderItem>,
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl io::Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(io::Error::other("JSON allowance exhausted"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(crate) fn bounded_json_bytes(
    value: &impl serde::Serialize,
    limit: usize,
) -> Result<Vec<u8>, Outcome> {
    let mut writer = BoundedWriter {
        bytes: Vec::with_capacity(limit.min(PROVIDER_ENVELOPE_BYTES)),
        limit,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| Outcome::ResourceLimit)?;
    Ok(writer.bytes)
}

pub(crate) fn bounded_json(value: &Value, limit: usize) -> Result<String, Outcome> {
    fn finite(
        value: &Value,
        depth: usize,
        nodes: &mut usize,
        maximum_nodes: usize,
    ) -> Result<(), Outcome> {
        *nodes += 1;
        if depth >= 128 || *nodes > maximum_nodes {
            return Err(Outcome::ResourceLimit);
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    finite(value, depth + 1, nodes, maximum_nodes)?;
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    finite(value, depth + 1, nodes, maximum_nodes)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    // A compact JSON tree needs at least two bytes per additional value node.
    finite(value, 0, &mut 0, limit.saturating_add(1) / 2)?;
    Ok(String::from_utf8(bounded_json_bytes(value, limit)?).expect("JSON UTF-8"))
}

struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("duplicate-free JSON")
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Strict, E> {
                serde_json::Number::from_f64(value)
                    .map(|n| Strict(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Strict, E> {
                Ok(Strict(value.into()))
            }
            fn visit_none<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<Strict, A::Error> {
                let mut values = Vec::new();
                while let Some(Strict(value)) = access.next_element()? {
                    values.push(value);
                }
                Ok(Strict(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Strict, A::Error> {
                let mut values = Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate field"));
                    }
                    let Strict(value) = access.next_value()?;
                    values.insert(key, value);
                }
                Ok(Strict(Value::Object(values)))
            }
        }
        d.deserialize_any(JsonVisitor)
    }
}
pub(crate) fn bounded_json_value(bytes: &[u8], limit: usize) -> Result<Value, Outcome> {
    if bytes.len() > limit {
        return Err(Outcome::ResourceLimit);
    }
    serde_json::from_slice::<Strict>(bytes)
        .map(|v| v.0)
        .map_err(|error| {
            if error.to_string().starts_with("recursion limit exceeded") {
                Outcome::ResourceLimit
            } else {
                Outcome::Framing
            }
        })
}
#[cfg(test)]
pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value, Outcome> {
    bounded_json_value(bytes, PROVIDER_FRAME_BYTES)
}
fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, Outcome> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(Outcome::Framing)
}
fn index(value: &Value, field: &str) -> Result<usize, Outcome> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or(Outcome::Framing)
}
pub(crate) fn identity(value: &str) -> bool {
    !value.is_empty()
}
fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn usage(response: &Value) -> Result<Option<ProviderUsage>, Outcome> {
    let Some(value) = response.get("usage").filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    if !value.is_object() {
        return Err(Outcome::Framing);
    }
    let number = |object: &Value, key: &str| -> Result<Option<u64>, Outcome> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(n) => n.as_u64().map(Some).ok_or(Outcome::Framing),
        }
    };
    for field in ["input_tokens_details", "output_tokens_details"] {
        if value
            .get(field)
            .is_some_and(|v| !v.is_null() && !v.is_object())
        {
            return Err(Outcome::Framing);
        }
    }
    let usage = ProviderUsage {
        input_tokens: number(value, "input_tokens")?,
        output_tokens: number(value, "output_tokens")?,
        total_tokens: number(value, "total_tokens")?,
        cached_input_tokens: number(&value["input_tokens_details"], "cached_tokens")?,
        reasoning_tokens: number(&value["output_tokens_details"], "reasoning_tokens")?,
    };
    usage.validate()?;
    Ok(Some(usage))
}
/// Whether a message or reasoning item's status fits: `in_progress` when added, `completed` when
/// done, or `incomplete` when the response stopped at its output limit.
fn status(item: &Value, complete: bool) -> Result<bool, Outcome> {
    Ok(match string(item, "status")? {
        "completed" | "incomplete" => complete,
        "in_progress" => !complete,
        _ => false,
    })
}
pub(crate) fn validate_item(item: &Value, complete: bool) -> Result<(), Outcome> {
    if !identity(string(item, "id")?) {
        return Err(Outcome::Framing);
    }
    match string(item, "type")? {
        "message" => {
            if string(item, "role")? != "assistant" || !status(item, complete)? {
                return Err(Outcome::Framing);
            }
            if item.get("phase").is_some_and(|phase| {
                !phase.is_null() && !matches!(phase.as_str(), Some("commentary" | "final_answer"))
            }) {
                return Err(Outcome::Framing);
            }
            let content = item["content"].as_array().ok_or(Outcome::Framing)?;
            if !complete && !content.is_empty() {
                return Err(Outcome::Unsupported);
            }
            for part in content {
                if string(part, "type")? != "output_text" {
                    return Err(Outcome::Unsupported);
                }
                string(part, "text")?;
                if part.get("annotations").is_some_and(|a| !a.is_array())
                    || part
                        .get("logprobs")
                        .is_some_and(|v| !v.is_null() && !v.is_array())
                {
                    return Err(Outcome::Framing);
                }
            }
        }
        "function_call" => {
            if let Some(namespace) = item
                .get("namespace")
                .filter(|namespace| !namespace.is_null())
            {
                if !namespace.is_string() {
                    return Err(Outcome::Framing);
                }
                return Err(Outcome::Unsupported);
            }
            if !identity(string(item, "call_id")?)
                || !name(string(item, "name")?)
                || item.get("status").is_some_and(|status| {
                    status.as_str() != Some(if complete { "completed" } else { "in_progress" })
                })
            {
                return Err(Outcome::Framing);
            }
            let arguments = string(item, "arguments")?;
            if complete && !bounded_json_value(arguments.as_bytes(), arguments.len())?.is_object() {
                return Err(Outcome::Framing);
            }
        }
        "reasoning" => {
            if item.get("status").is_some() && !status(item, complete)? {
                return Err(Outcome::Framing);
            }
            let summary = item["summary"].as_array().ok_or(Outcome::Framing)?;
            if !complete && !summary.is_empty() {
                return Err(Outcome::Unsupported);
            }
            for part in summary {
                if string(part, "type")? != "summary_text" {
                    return Err(Outcome::Unsupported);
                }
                string(part, "text")?;
            }
            if item
                .get("encrypted_content")
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(Outcome::Framing);
            }
        }
        _ => return Err(Outcome::Unsupported),
    }
    Ok(())
}

struct Part {
    text: String,
    done: bool,
    closed: bool,
}
struct ItemState {
    added: Value,
    done: Option<Value>,
    parts: Vec<Part>,
    arguments: String,
    arguments_done: bool,
}
pub(crate) struct ParsedResponse {
    /// Provider explanation for a failed or incomplete response, when it gave one.
    pub(crate) error: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) envelope: Option<ProviderEnvelope>,
    pub(crate) usage: Option<ProviderUsage>,
    pub(crate) status: String,
}
pub(crate) struct ResponsesParser {
    pub(crate) failure: ProviderFailure,
    frame: Vec<u8>,
    response_id: Option<String>,
    reported_model: Option<String>,
    items: Vec<ItemState>,
    terminal: Option<Value>,
    output_bytes: usize,
    provisional_bytes: usize,
    in_progress: bool,
    limits: ParserLimits,
    /// Collected only when enabled; the consumer drains it as the stream arrives.
    pub(crate) progress: Option<Vec<StreamProgress>>,
    /// An `error` event, which ends the stream as a failure (parsing stops with `Incomplete`).
    pub(crate) stream_error: Option<Value>,
}
impl ResponsesParser {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_limits(ParserLimits::DEFAULT)
    }
    pub(crate) fn with_limits(limits: ParserLimits) -> Self {
        Self {
            failure: ProviderFailure::at(ProviderFailureStage::SseFrame),
            frame: Vec::with_capacity(limits.frame.min(PROVIDER_FRAME_BYTES)),
            response_id: None,
            reported_model: None,
            items: Vec::with_capacity(limits.items.min(PROVIDER_OUTPUT_ITEMS)),
            terminal: None,
            output_bytes: 0,
            provisional_bytes: 0,
            in_progress: false,
            limits,
            progress: None,
            stream_error: None,
        }
    }
    pub(crate) fn byte(&mut self, byte: u8) -> Result<(), Outcome> {
        // Like Scramjet's clients, nothing after the terminal event affects the response.
        if self.terminal.is_some() {
            return Ok(());
        }
        if self.frame.is_empty() {
            self.failure = ProviderFailure::at(ProviderFailureStage::SseFrame);
        }
        if self.frame.len() == self.limits.frame {
            return Err(Outcome::ResourceLimit);
        }
        self.frame.push(byte);
        let ending = if self.frame.ends_with(b"\r\n\r\n") {
            4
        } else if self.frame.ends_with(b"\n\n") {
            2
        } else {
            return Ok(());
        };
        let text = std::str::from_utf8(&self.frame[..self.frame.len() - ending])
            .map_err(|_| Outcome::Framing)?;
        let mut event = None;
        let mut data = String::new();
        let mut has_data = false;
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.starts_with(':')
                || line.is_empty()
                || line.starts_with("id:")
                || line.starts_with("retry:")
            {
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                if event
                    .replace(value.strip_prefix(' ').unwrap_or(value))
                    .is_some()
                {
                    return Err(Outcome::Framing);
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                if has_data {
                    data.push('\n');
                }
                has_data = true;
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
            }
        }
        if !has_data {
            self.frame.clear();
            return Ok(());
        }
        if data == "[DONE]" {
            return Err(Outcome::Incomplete);
        }
        self.failure.stage = ProviderFailureStage::Json;
        let value = bounded_json_value(data.as_bytes(), self.limits.frame)?;
        if event.is_some_and(|event| value["type"].as_str() != Some(event)) {
            return Err(Outcome::Framing);
        }
        self.event(value)?;
        self.frame.clear();
        Ok(())
    }
    fn event(&mut self, event: Value) -> Result<(), Outcome> {
        self.failure.stage = ProviderFailureStage::Sequence;
        self.failure.event = event
            .get("type")
            .and_then(Value::as_str)
            .map(ProviderFailure::event);
        self.failure.field = Some("type");
        let kind = string(&event, "type")?;
        if kind == "error" {
            self.stream_error = Some(event);
            return Err(Outcome::Incomplete);
        }
        if self.response_id.is_none() {
            if kind != "response.created" {
                return Err(Outcome::Framing);
            }
            self.failure.stage = ProviderFailureStage::CreatedResponse;
            let response = &event["response"];
            self.failure.field = Some("response.id");
            let id = match response.get("id") {
                None => {
                    self.failure.identity_problem = Some(ProviderIdentityProblem::Missing);
                    return Err(Outcome::Framing);
                }
                Some(value) => match value.as_str() {
                    Some(id) => id,
                    None => {
                        self.failure.identity_problem = Some(ProviderIdentityProblem::NotString);
                        return Err(Outcome::Framing);
                    }
                },
            };
            self.failure.identity_bytes = Some(id.len());
            if !identity(id) {
                self.failure.identity_problem = Some(ProviderIdentityProblem::Empty);
                return Err(Outcome::Framing);
            }
            self.failure.identity_bytes = None;
            self.failure.field = Some("response.status");
            if string(response, "status")? != "in_progress" {
                return Err(Outcome::Framing);
            }
            self.failure.field = Some("response.output");
            if response["output"].as_array().is_none_or(|a| !a.is_empty()) {
                return Err(Outcome::Framing);
            }
            self.response_id = Some(id.to_owned());
            self.check_response(response, "in_progress")?;
            return Ok(());
        }
        if let Some(response_id) = event.get("response_id")
            && response_id.as_str() != self.response_id.as_deref()
        {
            return Err(Outcome::Framing);
        }
        if kind.ends_with(".delta") {
            let bytes = string(&event, "delta")?.len();
            self.provisional_bytes = self
                .provisional_bytes
                .checked_add(bytes)
                .ok_or(Outcome::ResourceLimit)?;
            if self.provisional_bytes > self.limits.envelope {
                return Err(Outcome::ResourceLimit);
            }
        }
        match kind {
            "response.in_progress" => {
                if self.in_progress || !self.items.is_empty() || self.terminal.is_some() {
                    return Err(Outcome::Framing);
                }
                self.check_response(&event["response"], "in_progress")?;
                if event["response"]["output"]
                    .as_array()
                    .is_none_or(|items| !items.is_empty())
                {
                    return Err(Outcome::Framing);
                }
                self.in_progress = true;
            }
            "response.output_item.added" => {
                self.failure.stage = ProviderFailureStage::ItemAdded;
                self.failure.field = Some("item/id");
                // Items are found by id, not `output_index`: Proxy's DeepSeek route gave a message
                // and a reasoning item the same index.
                if self.items.len() == self.limits.items {
                    return Err(Outcome::ResourceLimit);
                }
                let item = event["item"].clone();
                validate_item(&item, false)?;
                if self.items.iter().any(|i| i.added["id"] == item["id"]) {
                    return Err(Outcome::Framing);
                }
                if item["type"] == "function_call"
                    && self
                        .items
                        .iter()
                        .any(|i| i.added["type"] == "function_call")
                {
                    return Err(Outcome::Unsupported);
                }
                let arguments = if item["type"] == "function_call" {
                    if let Some(progress) = &mut self.progress {
                        progress.push(StreamProgress::Call(string(&item, "name")?.to_owned()));
                    }
                    string(&item, "arguments")?.to_owned()
                } else {
                    String::new()
                };
                self.provisional_bytes = self
                    .provisional_bytes
                    .checked_add(arguments.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if self.provisional_bytes > self.limits.envelope {
                    return Err(Outcome::ResourceLimit);
                }
                self.items.push(ItemState {
                    added: item,
                    done: None,
                    parts: Vec::new(),
                    arguments,
                    arguments_done: false,
                });
            }
            "response.output_item.done" => {
                self.failure.stage = ProviderFailureStage::ItemDone;
                self.failure.field = Some("item/id");
                let item = &event["item"];
                validate_item(item, true)?;
                let state = self
                    .items
                    .iter_mut()
                    .find(|state| state.added["id"] == item["id"])
                    .ok_or(Outcome::Framing)?;
                if state.done.is_some() || item["type"] != state.added["type"] {
                    return Err(Outcome::Framing);
                }
                match item["type"].as_str().unwrap() {
                    "function_call" => {
                        if !state.arguments_done
                            || item["arguments"].as_str() != Some(&state.arguments)
                            || item["name"] != state.added["name"]
                            || item["call_id"] != state.added["call_id"]
                        {
                            return Err(Outcome::Framing);
                        }
                    }
                    "message" | "reasoning" => {
                        let key = if item["type"] == "message" {
                            "content"
                        } else {
                            "summary"
                        };
                        let parts = item[key].as_array().unwrap();
                        if parts.len() != state.parts.len()
                            || state.parts.iter().zip(parts).any(|(part, wire)| {
                                !part.closed || wire["text"].as_str() != Some(&part.text)
                            })
                        {
                            return Err(Outcome::Framing);
                        }
                    }
                    _ => unreachable!(),
                }
                let encoded = bounded_json(item, self.limits.envelope)?;
                self.output_bytes = self
                    .output_bytes
                    .checked_add(encoded.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if self.output_bytes > self.limits.envelope {
                    return Err(Outcome::ResourceLimit);
                }
                state.done = Some(item.clone());
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                self.failure.stage = ProviderFailureStage::Arguments;
                self.failure.field = Some("arguments/item_id");
                let envelope = self.limits.envelope;
                let state = self.item(&event, "function_call")?;
                if state.arguments_done {
                    return Err(Outcome::Framing);
                }
                if kind.ends_with(".delta") {
                    let delta = string(&event, "delta")?;
                    if delta.len() > envelope - state.arguments.len() {
                        return Err(Outcome::ResourceLimit);
                    }
                    state.arguments.push_str(delta);
                } else {
                    if string(&event, "arguments")? != state.arguments
                        || !bounded_json_value(state.arguments.as_bytes(), envelope)?.is_object()
                    {
                        return Err(Outcome::Framing);
                    }
                    state.arguments_done = true;
                }
            }
            "response.content_part.added" | "response.reasoning_summary_part.added" => {
                self.failure.stage = ProviderFailureStage::PartAdded;
                self.failure.field = Some("part/index/item_id");
                let reasoning = kind == "response.reasoning_summary_part.added";
                if string(&event["part"], "type")?
                    != if reasoning {
                        "summary_text"
                    } else {
                        "output_text"
                    }
                {
                    return Err(Outcome::Unsupported);
                }
                let initial = string(&event["part"], "text")?;
                self.provisional_bytes = self
                    .provisional_bytes
                    .checked_add(initial.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if self.provisional_bytes > self.limits.envelope {
                    return Err(Outcome::ResourceLimit);
                }
                let state = self.item(&event, if reasoning { "reasoning" } else { "message" })?;
                let i = index(
                    &event,
                    if reasoning {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?;
                if i != state.parts.len()
                    || string(&event["part"], "type")?
                        != if reasoning {
                            "summary_text"
                        } else {
                            "output_text"
                        }
                {
                    return Err(Outcome::Framing);
                }
                state.parts.push(Part {
                    text: initial.to_owned(),
                    done: false,
                    closed: false,
                });
            }
            "response.output_text.delta"
            | "response.output_text.done"
            | "response.content_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => {
                self.failure.stage = ProviderFailureStage::PartText;
                self.failure.field = Some("part/text/index/item_id");
                let reasoning = kind.starts_with("response.reasoning_");
                let envelope = self.limits.envelope;
                let state = self.item(&event, if reasoning { "reasoning" } else { "message" })?;
                let i = index(
                    &event,
                    if reasoning {
                        "summary_index"
                    } else {
                        "content_index"
                    },
                )?;
                let part = state.parts.get_mut(i).ok_or(Outcome::Framing)?;
                if kind.ends_with(".delta") {
                    if part.done {
                        return Err(Outcome::Framing);
                    }
                    let delta = string(&event, "delta")?;
                    if delta.len() > envelope - part.text.len() {
                        return Err(Outcome::ResourceLimit);
                    }
                    part.text.push_str(delta);
                    if let Some(progress) = &mut self.progress {
                        progress.push(if reasoning {
                            StreamProgress::Reasoning(delta.to_owned())
                        } else {
                            StreamProgress::Text(delta.to_owned())
                        });
                    }
                } else if kind.ends_with("_part.done") {
                    if !part.done
                        || part.closed
                        || string(&event["part"], "text")? != part.text
                        || string(&event["part"], "type")?
                            != if reasoning {
                                "summary_text"
                            } else {
                                "output_text"
                            }
                    {
                        return Err(Outcome::Framing);
                    }
                    part.closed = true;
                    // Like Scramjet, summary parts are shown as separate paragraphs.
                    if reasoning && let Some(progress) = &mut self.progress {
                        progress.push(StreamProgress::Reasoning("\n\n".into()));
                    }
                } else {
                    let text = string(&event, "text")?;
                    if part.done {
                        return Err(Outcome::Framing);
                    }
                    if text.len() > envelope {
                        return Err(Outcome::ResourceLimit);
                    }
                    // As in Scramjet the final text counts, not the deltas: Proxy's DeepSeek route
                    // sent a reply's text only here.
                    part.text = text.to_owned();
                    part.done = true;
                }
            }
            "response.completed" | "response.failed" | "response.incomplete" => {
                self.failure.stage = ProviderFailureStage::Terminal;
                self.failure.field = None;
                let status = kind.strip_prefix("response.").unwrap();
                let response = &event["response"];
                self.check_response(response, status)?;
                self.failure.stage = ProviderFailureStage::Usage;
                self.failure.field = Some("response.usage");
                usage(response)?;
                self.failure.stage = ProviderFailureStage::Terminal;
                self.failure.field = Some("response.status");
                if status == "completed" {
                    self.failure.stage = ProviderFailureStage::FinalItems;
                    self.failure.field = Some("response.error");
                    if response.get("error").is_some_and(|e| !e.is_null()) {
                        return Err(Outcome::Framing);
                    }
                    self.failure.field = Some("response.incomplete_details");
                    if response
                        .get("incomplete_details")
                        .is_some_and(|e| !e.is_null())
                    {
                        return Err(Outcome::Framing);
                    }
                    self.failure.field = Some("response.output.item");
                    // As in Scramjet's clients, each streamed `output_item.done` item is
                    // authoritative; the terminal summary copy only fills an item whose done
                    // event never arrived. The two copies need not agree.
                    let output = response.get("output").and_then(Value::as_array);
                    for (i, state) in self.items.iter_mut().enumerate() {
                        if state.done.is_some() {
                            continue;
                        }
                        let item = output
                            .and_then(|output| output.get(i))
                            .filter(|item| {
                                item["type"] == state.added["type"]
                                    && state.added["type"] != "function_call"
                            })
                            .ok_or(Outcome::Incomplete)?;
                        validate_item(item, true)?;
                        state.done = Some(item.clone());
                    }
                    if self.items.is_empty()
                        || self
                            .items
                            .iter()
                            .any(|state| state.done.as_ref().unwrap()["status"] == "incomplete")
                    {
                        return Err(Outcome::Framing);
                    }
                    let selected: Vec<_> = self.items.iter().map(|s| &s.done).collect();
                    bounded_json_bytes(
                        &json!({"response_id":self.response_id,"items":selected}),
                        self.limits.envelope,
                    )?;
                }
                self.terminal = Some(response.clone());
            }
            _ => {}
        }
        Ok(())
    }
    fn check_response(&mut self, response: &Value, status: &str) -> Result<(), Outcome> {
        self.failure.stage = ProviderFailureStage::ResponseMetadata;
        self.failure.field = Some("response.id");
        if response["id"].as_str() != self.response_id.as_deref() {
            return Err(Outcome::Framing);
        }
        self.failure.field = Some("response.status");
        if string(response, "status")? != status {
            return Err(Outcome::Framing);
        }
        self.failure.field = Some("response.model");
        if let Some(model) = response.get("model") {
            let model = model.as_str().ok_or(Outcome::Framing)?;
            if model.is_empty()
                || self
                    .reported_model
                    .as_deref()
                    .is_some_and(|prior| prior != model)
            {
                return Err(Outcome::Framing);
            }
            self.reported_model = Some(model.to_owned());
        }
        Ok(())
    }
    fn item(&mut self, event: &Value, kind: &str) -> Result<&mut ItemState, Outcome> {
        let id = string(event, "item_id")?;
        let state = self
            .items
            .iter_mut()
            .find(|state| state.added["id"] == id)
            .ok_or(Outcome::Framing)?;
        if state.done.is_some() || state.added["type"] != kind {
            return Err(Outcome::Framing);
        }
        Ok(state)
    }
    pub(crate) fn complete(self) -> Result<ParsedResponse, Outcome> {
        if !self.frame.is_empty() {
            return Err(Outcome::Incomplete);
        }
        let response = self.terminal.ok_or(Outcome::Incomplete)?;
        let status = string(&response, "status")?.to_owned();
        let usage = usage(&response)?;
        let call = |item: &Value| item["type"] == "function_call";
        // As in Scramjet's client, a response stopped at its output limit keeps the items it
        // finished, unless it made a function call.
        let cut = status == "incomplete"
            && response["incomplete_details"]["reason"] == "max_output_tokens"
            && !self.items.iter().any(|state| call(&state.added))
            && !(response["output"].as_array().into_iter().flatten()).any(call);
        let envelope = if status == "completed" || cut {
            Some(ProviderEnvelope {
                items: self
                    .items
                    .into_iter()
                    .filter_map(|state| state.done)
                    .map(|item| ProviderItem::checked(item, self.limits.envelope))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        } else {
            None
        };
        let error = response["error"]["message"]
            .as_str()
            .or(response["incomplete_details"]["reason"].as_str())
            .map(str::to_owned);
        Ok(ParsedResponse {
            error,
            error_code: response["error"]["code"].as_str().map(str::to_owned),
            envelope,
            usage,
            status,
        })
    }
}

#[cfg(test)]
#[path = "responses_tests.rs"]
mod responses_tests;
