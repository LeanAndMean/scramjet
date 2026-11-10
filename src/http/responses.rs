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
    /// Input tokens read from the cache, part of `input_tokens`.
    pub cached_input_tokens: Option<u64>,
    /// Input tokens written to the cache, part of `input_tokens`.
    pub cache_write_tokens: Option<u64>,
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
        let cache = [self.cached_input_tokens, self.cache_write_tokens];
        let cached = (cache.iter().flatten()).try_fold(0u64, |sum, n| sum.checked_add(*n));
        if cache.iter().any(Option::is_some)
            && cached.is_none_or(|n| self.input_tokens.is_none_or(|input| n > input))
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
        validate_item(&wire)?;
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
        cache_write_tokens: number(&value["input_tokens_details"], "cache_write_tokens")?,
        reasoning_tokens: number(&value["output_tokens_details"], "reasoning_tokens")?,
    };
    usage.validate()?;
    Ok(Some(usage))
}
/// Whether a final item's status fits: `completed`, or `incomplete` when the response stopped at
/// its output limit. Scramjet's package does not require one.
fn finished(item: &Value) -> bool {
    match item.get("status") {
        None | Some(Value::Null) => true,
        Some(status) => matches!(status.as_str(), Some("completed" | "incomplete")),
    }
}
/// A final item, as it is kept and sent again.
pub(crate) fn validate_item(item: &Value) -> Result<(), Outcome> {
    if !identity(string(item, "id")?) {
        return Err(Outcome::Framing);
    }
    match string(item, "type")? {
        "message" => {
            if string(item, "role")? != "assistant" || !finished(item) {
                return Err(Outcome::Framing);
            }
            // Replayed as it came; Scramjet's package keeps phases it does not know.
            if item
                .get("phase")
                .is_some_and(|phase| !phase.is_null() && !phase.is_string())
            {
                return Err(Outcome::Framing);
            }
            for part in item["content"].as_array().ok_or(Outcome::Framing)? {
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
            // A namespaced tool is not one the request offered.
            if item.get("namespace").is_some() {
                return Err(Outcome::Unsupported);
            }
            if !identity(string(item, "call_id")?)
                || !name(string(item, "name")?)
                || item
                    .get("status")
                    .is_some_and(|status| !status.is_null() && status != "completed")
            {
                return Err(Outcome::Framing);
            }
            let arguments = string(item, "arguments")?;
            if !bounded_json_value(arguments.as_bytes(), arguments.len())?.is_object() {
                return Err(Outcome::Framing);
            }
        }
        "reasoning" => {
            if !finished(item) {
                return Err(Outcome::Framing);
            }
            for part in item["summary"].as_array().ok_or(Outcome::Framing)? {
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
/// Drops a namespace that is not a text, as a gateway's normalizer does since Foundry sent
/// invalid ones.
fn without_bad_namespace(mut item: Value) -> Value {
    if let Some(object) = item.as_object_mut()
        && object
            .get("namespace")
            .is_some_and(|namespace| !namespace.is_string())
    {
        object.remove("namespace");
    }
    item
}
fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Message,
    Reasoning,
    Call,
}
impl Kind {
    fn of(item: &Value) -> Option<Self> {
        match item["type"].as_str()? {
            "message" => Some(Self::Message),
            "reasoning" => Some(Self::Reasoning),
            "function_call" => Some(Self::Call),
            _ => None,
        }
    }
}
/// A message's content part or a reasoning item's summary part as it streams, for display: the
/// item's done copy says what it holds.
struct Part {
    /// Its `content_index` or `summary_index`, when an event gave one.
    key: Option<u64>,
    text: String,
    /// Its final text came; later deltas are ignored.
    done: bool,
    closed: bool,
}
struct ItemState {
    kind: Kind,
    id: Option<String>,
    index: Option<u64>,
    /// Its `output_item.added` came.
    added: bool,
    call_id: Option<String>,
    name: Option<String>,
    /// Its call was reported as progress.
    announced: bool,
    done: Option<Value>,
    parts: Vec<Part>,
    arguments: String,
}
impl ItemState {
    /// The part an event names, made when it is new: by its index, else the last one open.
    fn part(&mut self, key: Option<u64>) -> &mut Part {
        let found = match key {
            Some(key) => (self.parts.iter().position(|p| p.key == Some(key))).or_else(|| {
                self.parts
                    .iter()
                    .rposition(|p| p.key.is_none() && !p.closed)
            }),
            None => self.parts.iter().rposition(|p| !p.closed),
        };
        let i = match found {
            Some(i) => {
                self.parts[i].key = self.parts[i].key.or(key);
                i
            }
            None => {
                self.parts.push(Part {
                    key,
                    text: String::new(),
                    done: false,
                    closed: false,
                });
                self.parts.len() - 1
            }
        };
        &mut self.parts[i]
    }
}
pub(crate) struct ParsedResponse {
    /// Provider explanation for a failed or incomplete response, when it gave one.
    pub(crate) error: Option<String>,
    pub(crate) error_code: Option<String>,
    pub(crate) envelope: Option<ProviderEnvelope>,
    pub(crate) usage: Option<ProviderUsage>,
    pub(crate) status: String,
}
/// Reads a Responses stream as Scramjet's provider package reads the company's raw ones,
/// tolerating what a gateway's normalizer repairs for stricter clients: items' events before or
/// without their `output_item.added`, parts without their `.added`, and events without ids.
pub(crate) struct ResponsesParser {
    pub(crate) failure: ProviderFailure,
    frame: Vec<u8>,
    response_id: Option<String>,
    reported_model: Option<String>,
    items: Vec<ItemState>,
    /// Output indexes of items of other types, whose events are ignored.
    ignored: Vec<u64>,
    terminal: Option<Value>,
    output_bytes: usize,
    provisional_bytes: usize,
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
            ignored: Vec::new(),
            terminal: None,
            output_bytes: 0,
            provisional_bytes: 0,
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
            // Azure's background responses are queued before they are created.
            if kind == "response.queued" {
                return Ok(());
            }
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
        if kind.ends_with(".delta") {
            // As in Scramjet's package, a delta that is not a text is ignored.
            let Some(delta) = event["delta"].as_str() else {
                return Ok(());
            };
            self.provisional(delta.len())?;
        }
        match kind {
            "response.in_progress" => self.check_response(&event["response"], "in_progress")?,
            "response.output_item.added" => self.item_added(&event)?,
            "response.output_item.done" => self.item_done(&event)?,
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                self.arguments(&event, kind.ends_with(".delta"))?
            }
            "response.content_part.added"
            | "response.output_text.delta"
            | "response.output_text.done"
            | "response.content_part.done"
            | "response.reasoning_summary_part.added"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_summary_part.done" => self.part_event(&event, kind)?,
            "response.completed" | "response.failed" | "response.incomplete" => {
                self.terminal_event(&event, kind)?
            }
            _ => {}
        }
        Ok(())
    }
    fn provisional(&mut self, bytes: usize) -> Result<(), Outcome> {
        self.provisional_bytes = self
            .provisional_bytes
            .checked_add(bytes)
            .ok_or(Outcome::ResourceLimit)?;
        if self.provisional_bytes > self.limits.envelope {
            return Err(Outcome::ResourceLimit);
        }
        Ok(())
    }
    fn position(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|s| s.id.as_deref() == Some(id))
    }
    fn make(&mut self, kind: Kind, id: Option<&str>, index: Option<u64>) -> Result<usize, Outcome> {
        if self.items.len() == self.limits.items {
            return Err(Outcome::ResourceLimit);
        }
        self.items.push(ItemState {
            kind,
            id: id.map(str::to_owned),
            index,
            added: false,
            call_id: None,
            name: None,
            announced: false,
            done: None,
            parts: Vec::new(),
            arguments: String::new(),
        });
        Ok(self.items.len() - 1)
    }
    /// The item an event is for, found as Scramjet's package finds it: by the event's id, else
    /// one of its kind at its output index (GitHub Copilot gives every event ids of its own),
    /// else, without an index, the last one of its kind still open. One is made when there is
    /// none. `None`: the index is an ignored item's.
    fn locate(
        &mut self,
        id: Option<&str>,
        index: Option<u64>,
        kind: Kind,
    ) -> Result<Option<usize>, Outcome> {
        if let Some(i) = id.and_then(|id| self.position(id)) {
            return match self.items[i].kind == kind {
                true => Ok(Some(i)),
                false => Err(Outcome::Framing),
            };
        }
        if index.is_some_and(|n| self.ignored.contains(&n)) {
            return Ok(None);
        }
        let at = |s: &ItemState| s.kind == kind && (index.is_none() || s.index == index);
        let found = (self.items.iter().rposition(|s| at(s) && s.done.is_none()))
            .or_else(|| index.and_then(|_| self.items.iter().rposition(at)));
        let i = match found {
            Some(i) => i,
            None => self.make(kind, id, index)?,
        };
        let state = &mut self.items[i];
        if state.id.is_none() {
            state.id = id.map(str::to_owned);
        }
        Ok(Some(i))
    }
    fn item_added(&mut self, event: &Value) -> Result<(), Outcome> {
        self.failure.stage = ProviderFailureStage::ItemAdded;
        self.failure.field = Some("item/id");
        let item = &event["item"];
        let index = event["output_index"].as_u64();
        let Some(kind) = Kind::of(item) else {
            // As in Scramjet's package, items of other types (a web search, say) are ignored.
            self.ignored.extend(index);
            return Ok(());
        };
        let id = nonempty(&item["id"]);
        // An item added with an id of its own is a new one even at a shared output index (a
        // gateway's DeepSeek route gave a message and a reasoning item the same one), unless one
        // made for its earlier events waits there.
        let waiting = id.and_then(|id| self.position(id)).or_else(|| {
            self.items.iter().rposition(|s| {
                s.kind == kind
                    && !s.added
                    && s.done.is_none()
                    && s.id.is_none()
                    && (s.index.is_none() || s.index == index)
            })
        });
        let i = match waiting {
            Some(i) if self.items[i].kind != kind => return Err(Outcome::Framing),
            Some(i) => i,
            None => self.make(kind, None, index)?,
        };
        // A repeated one changes nothing.
        if self.items[i].added || self.items[i].done.is_some() {
            return Ok(());
        }
        let initial = match kind {
            Kind::Call if self.items[i].arguments.is_empty() => {
                item["arguments"].as_str().unwrap_or_default()
            }
            _ => "",
        };
        self.provisional(initial.len())?;
        let state = &mut self.items[i];
        state.added = true;
        state.index = state.index.or(index);
        if state.id.is_none() {
            state.id = id.map(str::to_owned);
        }
        if kind == Kind::Call {
            state.arguments.push_str(initial);
            state.call_id = nonempty(&item["call_id"]).map(str::to_owned);
            state.name = nonempty(&item["name"]).map(str::to_owned);
            if let Some(name) = &state.name {
                state.announced = true;
                if let Some(progress) = &mut self.progress {
                    progress.push(StreamProgress::Call(name.clone()));
                }
            }
        }
        Ok(())
    }
    fn item_done(&mut self, event: &Value) -> Result<(), Outcome> {
        self.failure.stage = ProviderFailureStage::ItemDone;
        self.failure.field = Some("item/id");
        let index = event["output_index"].as_u64();
        let Some(kind) = Kind::of(&event["item"]) else {
            self.ignored.extend(index);
            return Ok(());
        };
        let mut item = without_bad_namespace(event["item"].clone());
        let Some(i) = self.locate(nonempty(&item["id"]), index, kind)? else {
            return Ok(());
        };
        let state = &self.items[i];
        if kind == Kind::Call {
            // As in Scramjet's package, the done copy's arguments count, or the streamed ones
            // when it has none.
            if item["arguments"]
                .as_str()
                .is_none_or(|arguments| arguments.trim().is_empty())
            {
                item["arguments"] = json!(match state.arguments.trim().is_empty() {
                    true => "{}",
                    false => &state.arguments,
                });
            }
            if (state.call_id.as_ref()).is_some_and(|id| item["call_id"] != id.as_str())
                || (state.name.as_ref()).is_some_and(|name| item["name"] != name.as_str())
            {
                return Err(Outcome::Framing);
            }
        }
        if let Some(done) = &state.done {
            // A call completed again must be the same call, or both would run.
            if kind == Kind::Call
                && ["call_id", "name", "arguments"]
                    .iter()
                    .any(|key| done[key] != item[key])
            {
                return Err(Outcome::Framing);
            }
            return Ok(());
        }
        validate_item(&item)?;
        if kind == Kind::Call
            && (self.items.iter().filter_map(|s| s.done.as_ref()))
                .any(|done| done["call_id"] == item["call_id"])
        {
            return Err(Outcome::Framing);
        }
        let encoded = bounded_json(&item, self.limits.envelope)?;
        self.output_bytes = self
            .output_bytes
            .checked_add(encoded.len())
            .ok_or(Outcome::ResourceLimit)?;
        if self.output_bytes > self.limits.envelope {
            return Err(Outcome::ResourceLimit);
        }
        let state = &mut self.items[i];
        if kind == Kind::Call && !std::mem::replace(&mut state.announced, true) {
            let name = string(&item, "name")?.to_owned();
            if let Some(progress) = &mut self.progress {
                progress.push(StreamProgress::Call(name));
            }
        }
        state.done = Some(item);
        Ok(())
    }
    fn arguments(&mut self, event: &Value, delta: bool) -> Result<(), Outcome> {
        self.failure.stage = ProviderFailureStage::Arguments;
        self.failure.field = Some("arguments/item_id");
        let index = event["output_index"].as_u64();
        let Some(i) = self.locate(nonempty(&event["item_id"]), index, Kind::Call)? else {
            return Ok(());
        };
        let envelope = self.limits.envelope;
        let state = &mut self.items[i];
        if state.done.is_some() {
            return Ok(());
        }
        if delta {
            let delta = event["delta"].as_str().unwrap_or_default();
            if delta.len() > envelope.saturating_sub(state.arguments.len()) {
                return Err(Outcome::ResourceLimit);
            }
            state.arguments.push_str(delta);
        } else if let Some(arguments) = nonempty(&event["arguments"]) {
            if arguments.len() > envelope {
                return Err(Outcome::ResourceLimit);
            }
            state.arguments = arguments.to_owned();
        }
        Ok(())
    }
    /// A content or summary part's event. Parts are made as their events come, as Foundry's
    /// reasoning summaries can lack `reasoning_summary_part.added`, and its `.done` events their
    /// ids, indexes and text (a gateway's normalizer repairs these for stricter clients).
    fn part_event(&mut self, event: &Value, kind: &str) -> Result<(), Outcome> {
        self.failure.stage = match kind.ends_with("_part.added") {
            true => ProviderFailureStage::PartAdded,
            false => ProviderFailureStage::PartText,
        };
        self.failure.field = Some("part/text/index/item_id");
        let reasoning = kind.starts_with("response.reasoning_summary_");
        let (item_kind, part_type, key) = match reasoning {
            true => (Kind::Reasoning, "summary_text", "summary_index"),
            false => (Kind::Message, "output_text", "content_index"),
        };
        // A part of another type (a refusal, raw reasoning text) is not shown as it streams; the
        // done copy says what the item holds.
        if event
            .get("part")
            .is_some_and(|part| part["type"] != part_type)
        {
            return Ok(());
        }
        let index = event["output_index"].as_u64();
        let Some(i) = self.locate(nonempty(&event["item_id"]), index, item_kind)? else {
            return Ok(());
        };
        let envelope = self.limits.envelope;
        let state = &mut self.items[i];
        if state.done.is_some() {
            return Ok(());
        }
        let part = state.part(event[key].as_u64());
        let (mut shown, mut grown) = (None, 0);
        if kind.ends_with(".delta") {
            if !part.done {
                let delta = event["delta"].as_str().unwrap_or_default();
                if delta.len() > envelope.saturating_sub(part.text.len()) {
                    return Err(Outcome::ResourceLimit);
                }
                part.text.push_str(delta);
                shown = Some(delta.to_owned());
            }
        } else {
            // A final text replaces what streamed, and what extends it is shown: a gateway's
            // DeepSeek route sent a reply's text only in its done event.
            let text = match kind {
                _ if kind.ends_with("_text.done") => event["text"].as_str(),
                _ if kind.ends_with("_part.added") => event["part"]["text"]
                    .as_str()
                    .filter(|_| part.text.is_empty()),
                _ => event["part"]["text"].as_str().filter(|_| !part.done),
            };
            if let Some(text) = text {
                if text.len() > envelope {
                    return Err(Outcome::ResourceLimit);
                }
                grown = text.len().saturating_sub(part.text.len());
                shown = (text.strip_prefix(part.text.as_str()))
                    .filter(|rest| !rest.is_empty())
                    .map(str::to_owned);
                part.text = text.to_owned();
            }
            part.done |= kind.ends_with(".done");
        }
        // Like Scramjet, summary parts are shown as separate paragraphs.
        let closing = kind.ends_with("_part.done") && !std::mem::replace(&mut part.closed, true);
        self.provisional(grown)?;
        if let Some(progress) = &mut self.progress {
            let update = match reasoning {
                true => StreamProgress::Reasoning,
                false => StreamProgress::Text,
            };
            progress.extend(shown.map(update));
            if reasoning && closing {
                progress.push(StreamProgress::Reasoning("\n\n".into()));
            }
        }
        Ok(())
    }
    fn terminal_event(&mut self, event: &Value, kind: &str) -> Result<(), Outcome> {
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
            // As in Scramjet's clients, each streamed `output_item.done` item is authoritative;
            // the terminal summary copy only fills an item whose done event never arrived. The
            // two copies need not agree.
            let output = response.get("output").and_then(Value::as_array);
            for i in 0..self.items.len() {
                if self.items[i].done.is_some() {
                    continue;
                }
                match self.terminal_copy(output, i) {
                    Some(item) => {
                        validate_item(&item)?;
                        self.items[i].done = Some(item);
                    }
                    // Reasoning without its final copy is left out, as a replay needs that
                    // copy's encrypted content. Scramjet's package fails a call without its done
                    // event.
                    None if self.items[i].kind == Kind::Call
                        || self.items[i].kind == Kind::Message
                            && self.items[i].parts.iter().any(|p| !p.text.is_empty()) =>
                    {
                        return Err(Outcome::Incomplete);
                    }
                    None => {}
                }
            }
            let selected: Vec<_> = (self.items.iter().filter_map(|s| s.done.as_ref())).collect();
            if selected.iter().any(|item| item["status"] == "incomplete") {
                return Err(Outcome::Framing);
            }
            bounded_json_bytes(
                &json!({"response_id":self.response_id,"items":selected}),
                self.limits.envelope,
            )?;
        }
        self.terminal = Some(response.clone());
        Ok(())
    }
    /// The terminal copy of an item whose done event never came: the output item with its id,
    /// else the one at its output index or its position, if of its kind and no other item's.
    /// Calls have none: each needs its own done event.
    fn terminal_copy(&self, output: Option<&Vec<Value>>, i: usize) -> Option<Value> {
        let state = &self.items[i];
        let output = output.filter(|_| state.kind != Kind::Call)?;
        let fits = |item: &&Value| Kind::of(item) == Some(state.kind);
        let others = |item: &&Value| {
            item["id"].as_str().is_some_and(|id| {
                self.items.iter().any(|other| {
                    other.id.as_deref() == Some(id)
                        || other.done.as_ref().is_some_and(|done| done["id"] == id)
                })
            })
        };
        let free = |item: &&Value| fits(item) && !others(item);
        let at = |n: u64| usize::try_from(n).ok().and_then(|n| output.get(n));
        (state.id.as_ref())
            .and_then(|id| {
                output
                    .iter()
                    .find(|item| fits(item) && item["id"] == id.as_str())
            })
            .or_else(|| state.index.and_then(at).filter(free))
            .or_else(|| output.get(i).filter(free))
            .map(|item| without_bad_namespace(item.clone()))
    }
    /// The response's status and model; its id may change from event to event, as GitHub
    /// Copilot's does, and as Scramjet's client allows.
    fn check_response(&mut self, response: &Value, status: &str) -> Result<(), Outcome> {
        self.failure.stage = ProviderFailureStage::ResponseMetadata;
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
            && !self.items.iter().any(|state| state.kind == Kind::Call)
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
