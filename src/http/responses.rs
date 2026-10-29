use super::provider_diagnostic::{
    ProviderFailure, ProviderFailureStage, ProviderFinalDifference, ProviderIdentityProblem,
};
use crate::Outcome;
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, fmt, io};

pub const PROVIDER_ENVELOPE_BYTES: usize = 16_384;
pub const PROVIDER_FRAME_BYTES: usize = 16_384;
pub const PROVIDER_OUTPUT_ITEMS: usize = 8;
pub const PROVIDER_MODEL: &str = "gpt-6-luna";
// Eight selected journey turns can each retain eight items, a result and user input.
const INPUT_ITEMS: usize = 8 * (PROVIDER_OUTPUT_ITEMS + 2);

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
    pub fn complete(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some() && self.total_tokens.is_some()
    }
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
    pub(crate) fn raw(&self) -> &Value {
        &self.wire
    }
    pub fn id(&self) -> &str {
        self.wire["id"].as_str().expect("validated item identity")
    }
    pub fn kind(&self) -> &str {
        self.wire["type"].as_str().expect("validated item type")
    }
    pub fn function_call(&self) -> Option<ProviderFunctionCall<'_>> {
        (self.kind() == "function_call").then(|| ProviderFunctionCall {
            call_id: self.wire["call_id"].as_str().unwrap(),
            name: self.wire["name"].as_str().unwrap(),
            arguments: self.wire["arguments"].as_str().unwrap(),
        })
    }
    pub fn text(&self) -> Option<String> {
        (self.kind() == "message").then(|| {
            self.wire["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|part| part["text"].as_str().unwrap())
                .collect()
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ProviderFunctionCall<'a> {
    pub call_id: &'a str,
    pub name: &'a str,
    pub arguments: &'a str,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderEnvelope {
    pub response_id: String,
    pub items: Vec<ProviderItem>,
}
#[derive(Clone, Debug)]
pub enum ProviderInput {
    User(String),
    Item(ProviderItem),
    ToolResult { call_id: String, output: String },
}
#[derive(Clone, Debug)]
pub struct ProviderTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}
#[derive(Clone, Debug)]
pub struct ProviderRequest {
    pub(crate) encoded: String,
}
impl ProviderRequest {
    pub fn new(input: &[ProviderInput], tools: &[ProviderTool]) -> Result<Self, Outcome> {
        if input.is_empty() || input.len() > INPUT_ITEMS || tools.len() > 3 {
            return Err(Outcome::ResourceLimit);
        }
        let mut admitted_bytes = 0usize;
        for item in input {
            let bytes = match item {
                ProviderInput::User(text) => text.len(),
                ProviderInput::Item(item) => {
                    bounded_json(&item.wire, PROVIDER_ENVELOPE_BYTES)?.len()
                }
                ProviderInput::ToolResult { call_id, output } => call_id
                    .len()
                    .checked_add(output.len())
                    .ok_or(Outcome::ResourceLimit)?,
            };
            admitted_bytes = admitted_bytes
                .checked_add(bytes)
                .ok_or(Outcome::ResourceLimit)?;
            if admitted_bytes > PROVIDER_ENVELOPE_BYTES {
                return Err(Outcome::ResourceLimit);
            }
        }
        for tool in tools {
            let schema = bounded_json(&tool.parameters, PROVIDER_ENVELOPE_BYTES)?;
            for bytes in [tool.name.len(), tool.description.len(), schema.len()] {
                admitted_bytes = admitted_bytes
                    .checked_add(bytes)
                    .ok_or(Outcome::ResourceLimit)?;
                if admitted_bytes > PROVIDER_ENVELOPE_BYTES {
                    return Err(Outcome::ResourceLimit);
                }
            }
        }
        let mut ids = BTreeSet::new();
        let mut calls = BTreeSet::new();
        let mut pending = None;
        let mut wire_input = Vec::with_capacity(input.len());
        for item in input {
            match item {
                ProviderInput::User(text) => {
                    if pending.is_some() {
                        return Err(Outcome::Framing);
                    }
                    if text.len() > PROVIDER_ENVELOPE_BYTES {
                        return Err(Outcome::ResourceLimit);
                    }
                    wire_input.push(json!({"role":"user","content":text}));
                }
                ProviderInput::Item(item) => {
                    if !ids.insert(item.id()) {
                        return Err(Outcome::Framing);
                    }
                    if let Some(call) = item.function_call() {
                        if pending.is_some() || !calls.insert(call.call_id) {
                            return Err(Outcome::Framing);
                        }
                        pending = Some(call.call_id);
                    }
                    wire_input.push(item.wire.clone());
                }
                ProviderInput::ToolResult { call_id, output } => {
                    if pending != Some(call_id.as_str()) {
                        return Err(Outcome::Framing);
                    }
                    if output.len() > PROVIDER_ENVELOPE_BYTES {
                        return Err(Outcome::ResourceLimit);
                    }
                    pending = None;
                    wire_input.push(
                        json!({"type":"function_call_output","call_id":call_id,"output":output}),
                    );
                }
            }
        }
        if pending.is_some() {
            return Err(Outcome::Framing);
        }
        let mut names = BTreeSet::new();
        let mut wire_tools = Vec::with_capacity(tools.len());
        for tool in tools {
            if !name(&tool.name) || !names.insert(&tool.name) || !tool.parameters.is_object() {
                return Err(Outcome::Framing);
            }
            if tool.description.len() > PROVIDER_ENVELOPE_BYTES {
                return Err(Outcome::ResourceLimit);
            }
            wire_tools.push(
                json!({"type":"function","name":tool.name,"description":tool.description,
                "parameters":tool.parameters,"strict":true}),
            );
        }
        let wire = json!({"model":PROVIDER_MODEL,"input":wire_input,"tools":wire_tools,
            "stream":true,"store":false,"include":["reasoning.encrypted_content"],
            "max_output_tokens":1024,"parallel_tool_calls":false});
        let encoded = bounded_json(&wire, PROVIDER_ENVELOPE_BYTES)?;
        Ok(Self { encoded })
    }
    pub fn encoded_bytes(&self) -> usize {
        self.encoded.len()
    }
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
        bytes: Vec::with_capacity(limit),
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
pub(crate) fn strict_json(bytes: &[u8]) -> Result<Value, Outcome> {
    bounded_json_value(bytes, PROVIDER_FRAME_BYTES)
}
pub(crate) fn metadata_json(bytes: &[u8]) -> Result<Value, Outcome> {
    bounded_json_value(bytes, crate::runtime::MAX_WIRE)
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
pub(crate) fn validate_item(item: &Value, complete: bool) -> Result<(), Outcome> {
    if !identity(string(item, "id")?) {
        return Err(Outcome::Framing);
    }
    match string(item, "type")? {
        "message" => {
            if string(item, "role")? != "assistant"
                || string(item, "status")? != if complete { "completed" } else { "in_progress" }
            {
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
            if complete && !strict_json(arguments.as_bytes())?.is_object() {
                return Err(Outcome::Framing);
            }
        }
        "reasoning" => {
            if item.get("status").is_some_and(|status| {
                status.as_str() != Some(if complete { "completed" } else { "in_progress" })
            }) {
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

fn compare_parts(done: &Value, terminal: &Value, text: bool) -> Result<bool, Outcome> {
    let done = done.as_array().ok_or(Outcome::Framing)?;
    let terminal = terminal.as_array().ok_or(Outcome::Framing)?;
    if done.len() != terminal.len() {
        return Err(Outcome::Framing);
    }
    let mut replay_refused = false;
    for (done, terminal) in done.iter().zip(terminal) {
        for key in ["type", "text"] {
            if done[key] != terminal[key] {
                return Err(Outcome::Framing);
            }
        }
        for key in done
            .as_object()
            .unwrap()
            .keys()
            .chain(terminal.as_object().unwrap().keys())
        {
            if matches!(key.as_str(), "type" | "text")
                || (text && matches!(key.as_str(), "annotations" | "logprobs"))
            {
                continue;
            }
            if done.get(key) != terminal.get(key) {
                replay_refused = true;
            }
        }
    }
    Ok(replay_refused)
}

fn select_replay(done: &Value, terminal: &Value) -> Result<(bool, bool), Outcome> {
    validate_item(done, true)?;
    validate_item(terminal, true)?;
    let kind = string(done, "type")?;
    let material: &[&str] = match kind {
        "message" => &["id", "type", "role"],
        "function_call" => &["id", "type", "name", "call_id", "arguments"],
        "reasoning" => &["id", "type"],
        _ => unreachable!(),
    };
    for key in material {
        if done.get(*key) != terminal.get(*key) {
            return Err(Outcome::Framing);
        }
    }
    let mut replay_refused = false;
    let continuation = match kind {
        "message" => {
            replay_refused |= compare_parts(&done["content"], &terminal["content"], true)?;
            Some("phase")
        }
        "reasoning" => {
            replay_refused |= compare_parts(&done["summary"], &terminal["summary"], false)?;
            Some("encrypted_content")
        }
        "function_call" => None,
        _ => unreachable!(),
    };
    for key in done
        .as_object()
        .unwrap()
        .keys()
        .chain(terminal.as_object().unwrap().keys())
    {
        if material.contains(&key.as_str())
            || Some(key.as_str()) == continuation
            || (key == "status" && matches!(kind, "function_call" | "reasoning"))
            || (key == "content" && kind == "message")
            || (key == "summary" && kind == "reasoning")
        {
            continue;
        }
        if done.get(key) != terminal.get(key) {
            replay_refused = true;
        }
    }
    let mut use_terminal = false;
    if let Some(key) = continuation {
        let done_value = done.get(key).filter(|v| !v.is_null());
        let terminal_value = terminal.get(key).filter(|v| !v.is_null());
        match (done_value, terminal_value) {
            (Some(a), Some(b)) if a != b => return Err(Outcome::Framing),
            (None, Some(_)) if kind == "reasoning" => replay_refused = true,
            (None, Some(_)) => use_terminal = true,
            _ => {}
        }
        if kind == "reasoning" && done_value.is_none() {
            replay_refused = true;
        }
    }
    Ok((use_terminal, replay_refused))
}

struct Part {
    text: String,
    done: bool,
    closed: bool,
}
struct ItemState {
    added: Value,
    done: Option<Value>,
    replay_terminal: bool,
    parts: Vec<Part>,
    arguments: String,
    arguments_done: bool,
}
pub(crate) struct ParsedResponse {
    pub(crate) refusal: Option<ProviderFailure>,
    pub(crate) envelope: Option<ProviderEnvelope>,
    pub(crate) usage: Option<ProviderUsage>,
    pub(crate) response_id: String,
    pub(crate) reported_model: Option<String>,
    pub(crate) status: String,
}
pub(crate) struct ResponsesParser {
    pub(crate) failure: ProviderFailure,
    frame: Vec<u8>,
    response_id: Option<String>,
    reported_model: Option<String>,
    sequence: Option<usize>,
    items: Vec<ItemState>,
    terminal: Option<Value>,
    replay_refusal: Option<ProviderFailure>,
    done_marker: bool,
    output_bytes: usize,
    provisional_bytes: usize,
    in_progress: bool,
}
impl ResponsesParser {
    pub(crate) fn new() -> Self {
        Self {
            failure: ProviderFailure::at(ProviderFailureStage::SseFrame),
            frame: Vec::with_capacity(PROVIDER_FRAME_BYTES),
            response_id: None,
            reported_model: None,
            sequence: None,
            items: Vec::with_capacity(PROVIDER_OUTPUT_ITEMS),
            terminal: None,
            replay_refusal: None,
            done_marker: false,
            output_bytes: 0,
            provisional_bytes: 0,
            in_progress: false,
        }
    }
    pub(crate) fn byte(&mut self, byte: u8) -> Result<(), Outcome> {
        if self.done_marker {
            self.failure = ProviderFailure::at(ProviderFailureStage::TrailingData);
            return Err(Outcome::Framing);
        }
        if self.frame.is_empty() {
            self.failure = ProviderFailure::at(ProviderFailureStage::SseFrame);
        }
        if self.frame.len() == PROVIDER_FRAME_BYTES {
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
            } else {
                self.failure.field = Some("SSE unknown line");
                return Err(Outcome::Framing);
            }
        }
        if !has_data {
            self.frame.clear();
            return Ok(());
        }
        if data == "[DONE]" {
            if self.terminal.is_none() || event.is_some() {
                return Err(Outcome::Incomplete);
            }
            self.done_marker = true;
            self.frame.clear();
            return Ok(());
        }
        if self.terminal.is_some() {
            self.failure = ProviderFailure::at(ProviderFailureStage::TrailingData);
            return Err(Outcome::Framing);
        }
        self.failure.stage = ProviderFailureStage::Json;
        let value = strict_json(data.as_bytes())?;
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
        self.failure.field = Some("sequence_number");
        let sequence = index(&event, "sequence_number")?;
        if self.sequence.is_some_and(|previous| sequence <= previous) {
            return Err(Outcome::Framing);
        }
        self.sequence = Some(sequence);
        let kind = string(&event, "type")?;
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
            if self.provisional_bytes > PROVIDER_ENVELOPE_BYTES {
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
                self.failure.field = Some("item/output_index");
                if index(&event, "output_index")? != self.items.len() {
                    return Err(Outcome::Framing);
                }
                if self.items.len() == PROVIDER_OUTPUT_ITEMS {
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
                    string(&item, "arguments")?.to_owned()
                } else {
                    String::new()
                };
                self.provisional_bytes = self
                    .provisional_bytes
                    .checked_add(arguments.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if self.provisional_bytes > PROVIDER_ENVELOPE_BYTES {
                    return Err(Outcome::ResourceLimit);
                }
                self.items.push(ItemState {
                    added: item,
                    done: None,
                    replay_terminal: false,
                    parts: Vec::new(),
                    arguments,
                    arguments_done: false,
                });
            }
            "response.output_item.done" => {
                self.failure.stage = ProviderFailureStage::ItemDone;
                self.failure.field = Some("item/output_index");
                let i = index(&event, "output_index")?;
                let item = &event["item"];
                validate_item(item, true)?;
                let state = self.items.get_mut(i).ok_or(Outcome::Framing)?;
                if state.done.is_some()
                    || item["id"] != state.added["id"]
                    || item["type"] != state.added["type"]
                {
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
                let encoded = bounded_json(item, PROVIDER_ENVELOPE_BYTES)?;
                self.output_bytes = self
                    .output_bytes
                    .checked_add(encoded.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if self.output_bytes > PROVIDER_ENVELOPE_BYTES {
                    return Err(Outcome::ResourceLimit);
                }
                state.done = Some(item.clone());
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                self.failure.stage = ProviderFailureStage::Arguments;
                self.failure.field = Some("arguments/item_id");
                let state = self.item(&event, "function_call")?;
                if state.arguments_done {
                    return Err(Outcome::Framing);
                }
                if kind.ends_with(".delta") {
                    let delta = string(&event, "delta")?;
                    if delta.len() > PROVIDER_ENVELOPE_BYTES - state.arguments.len() {
                        return Err(Outcome::ResourceLimit);
                    }
                    state.arguments.push_str(delta);
                } else {
                    if string(&event, "arguments")? != state.arguments
                        || !strict_json(state.arguments.as_bytes())?.is_object()
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
                if self.provisional_bytes > PROVIDER_ENVELOPE_BYTES {
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
                    if delta.len() > PROVIDER_ENVELOPE_BYTES - part.text.len() {
                        return Err(Outcome::ResourceLimit);
                    }
                    part.text.push_str(delta);
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
                } else {
                    if part.done || string(&event, "text")? != part.text {
                        return Err(Outcome::Framing);
                    }
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
                    self.failure.field = Some("response.output");
                    let mut difference =
                        ProviderFinalDifference::new(self.items.len(), response.get("output"));
                    let Some(output) = response["output"].as_array() else {
                        self.failure.final_difference = Some(Box::new(difference));
                        return Err(Outcome::Framing);
                    };
                    self.failure.field = Some("response.output.count");
                    if output.len() != self.items.len() || output.is_empty() {
                        for i in 0..self
                            .items
                            .len()
                            .max(output.len())
                            .min(PROVIDER_OUTPUT_ITEMS)
                        {
                            difference.item(
                                i,
                                self.items.get(i).and_then(|s| s.done.as_ref()),
                                output.get(i),
                            );
                        }
                        difference.additional_items = output.len() > PROVIDER_OUTPUT_ITEMS;
                        self.failure.final_difference = Some(Box::new(difference));
                        return Err(Outcome::Framing);
                    }
                    self.failure.field = Some("response.output.item");
                    let selection = self
                        .items
                        .iter()
                        .zip(output)
                        .map(|(state, item)| {
                            let done = state.done.as_ref().ok_or(Outcome::Framing)?;
                            select_replay(done, item)
                        })
                        .collect::<Result<Vec<_>, _>>();
                    let selection = match selection {
                        Ok(selection) => selection,
                        Err(outcome) => {
                            for (i, (state, item)) in self.items.iter().zip(output).enumerate() {
                                difference.item(i, state.done.as_ref(), Some(item));
                            }
                            self.failure.final_difference = Some(Box::new(difference));
                            return Err(outcome);
                        }
                    };
                    let selected: Vec<_> = self
                        .items
                        .iter()
                        .zip(output)
                        .zip(&selection)
                        .map(|((state, item), (terminal, _))| {
                            if *terminal {
                                item
                            } else {
                                state.done.as_ref().unwrap()
                            }
                        })
                        .collect();
                    bounded_json_bytes(
                        &json!({"response_id":self.response_id,"items":selected}),
                        PROVIDER_ENVELOPE_BYTES,
                    )?;
                    if selection.iter().any(|(_, refused)| *refused) {
                        for (i, (state, item)) in self.items.iter().zip(output).enumerate() {
                            difference.item(i, state.done.as_ref(), Some(item));
                        }
                        self.failure.final_difference = Some(Box::new(difference));
                        self.replay_refusal = Some(self.failure.clone());
                    }
                    for (state, (terminal, _)) in self.items.iter_mut().zip(selection) {
                        state.replay_terminal = terminal;
                    }
                }
                self.terminal = Some(response.clone());
            }
            _ => return Err(Outcome::Unsupported),
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
        let i = index(event, "output_index")?;
        let state = self.items.get_mut(i).ok_or(Outcome::Framing)?;
        if state.done.is_some()
            || state.added["type"] != kind
            || string(event, "item_id")? != string(&state.added, "id")?
        {
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
        let response_id = self.response_id.unwrap();
        let usage = usage(&response)?;
        let envelope =
            (status == "completed" && self.replay_refusal.is_none()).then(|| ProviderEnvelope {
                response_id: response_id.clone(),
                items: self
                    .items
                    .into_iter()
                    .enumerate()
                    .map(|(index, state)| ProviderItem {
                        wire: if state.replay_terminal {
                            response["output"][index].clone()
                        } else {
                            state.done.unwrap()
                        },
                    })
                    .collect(),
            });
        Ok(ParsedResponse {
            refusal: self.replay_refusal,
            envelope,
            usage,
            response_id,
            reported_model: self.reported_model,
            status,
        })
    }
}

#[cfg(test)]
#[path = "responses_tests.rs"]
mod responses_tests;
