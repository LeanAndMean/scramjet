use serde::{Serialize, Serializer};
use serde_json::Value;

pub const PROVIDER_DIAGNOSTIC_BYTES: usize = 4096;
pub const PROVIDER_ITEM_FIELDS: [&str; 11] = [
    "id",
    "type",
    "role",
    "status",
    "call_id",
    "name",
    "arguments",
    "content",
    "summary",
    "phase",
    "encrypted_content",
];
pub const PROVIDER_PART_FIELDS: [&str; 4] = ["type", "text", "annotations", "logprobs"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ProviderFailureStage {
    Reservation,
    Connect,
    Handshake,
    HttpHead,
    ContentType,
    Body,
    SseFrame,
    Json,
    Sequence,
    CreatedResponse,
    ResponseMetadata,
    ItemAdded,
    ItemDone,
    Arguments,
    PartAdded,
    PartText,
    Terminal,
    Usage,
    FinalItems,
    MissingTerminal,
    TrailingData,
    Acknowledgement,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ProviderIdentityProblem {
    Missing,
    NotString,
    Empty,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum ProviderJsonKind {
    #[default]
    Missing = 0,
    Null = 1,
    Bool = 2,
    Number = 3,
    String = 4,
    Array = 5,
    Object = 6,
}
impl Serialize for ProviderJsonKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}
impl ProviderJsonKind {
    fn of(value: Option<&Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(Value::Null) => Self::Null,
            Some(Value::Bool(_)) => Self::Bool,
            Some(Value::Number(_)) => Self::Number,
            Some(Value::String(_)) => Self::String,
            Some(Value::Array(_)) => Self::Array,
            Some(Value::Object(_)) => Self::Object,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderPartDifference {
    pub index: usize,
    pub fields: u8,
    pub done_kinds: [ProviderJsonKind; 4],
    pub final_kinds: [ProviderJsonKind; 4],
    pub other_fields: bool,
    pub additional_parts: bool,
}
impl Serialize for ProviderPartDifference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (
            self.index,
            self.fields,
            self.done_kinds,
            self.final_kinds,
            self.other_fields,
            self.additional_parts,
        )
            .serialize(serializer)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderArrayDifference {
    pub done_count: Option<usize>,
    pub final_count: Option<usize>,
    pub first_part: Option<ProviderPartDifference>,
}
impl Serialize for ProviderArrayDifference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.done_count, self.final_count, self.first_part).serialize(serializer)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderItemDifference {
    pub index: usize,
    pub done_kind: ProviderJsonKind,
    pub final_kind: ProviderJsonKind,
    pub fields: u16,
    pub done_kinds: [ProviderJsonKind; 11],
    pub final_kinds: [ProviderJsonKind; 11],
    pub other_fields: bool,
    pub content: Option<ProviderArrayDifference>,
    pub summary: Option<ProviderArrayDifference>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderFinalDifference {
    pub done_count: usize,
    pub final_kind: ProviderJsonKind,
    pub final_count: Option<usize>,
    pub items: [Option<ProviderItemDifference>; 8],
    pub additional_items: bool,
}
impl ProviderFinalDifference {
    pub(crate) fn new(done_count: usize, output: Option<&Value>) -> Self {
        Self {
            done_count,
            final_kind: ProviderJsonKind::of(output),
            final_count: output.and_then(Value::as_array).map(Vec::len),
            items: [None; 8],
            additional_items: false,
        }
    }
    pub(crate) fn item(&mut self, index: usize, done: Option<&Value>, final_item: Option<&Value>) {
        if done == final_item {
            return;
        }
        let Some(slot) = self.items.get_mut(index) else {
            self.additional_items = true;
            return;
        };
        let (fields, done_kinds, final_kinds) = fields(done, final_item, &PROVIDER_ITEM_FIELDS);
        *slot = Some(ProviderItemDifference {
            index,
            done_kind: ProviderJsonKind::of(done),
            final_kind: ProviderJsonKind::of(final_item),
            fields,
            done_kinds,
            final_kinds,
            other_fields: other_fields(done, final_item, &PROVIDER_ITEM_FIELDS)
                || (done.is_some_and(|v| !v.is_object()) && done != final_item),
            content: array_difference(
                done.and_then(|v| v.get("content")),
                final_item.and_then(|v| v.get("content")),
            ),
            summary: array_difference(
                done.and_then(|v| v.get("summary")),
                final_item.and_then(|v| v.get("summary")),
            ),
        });
    }
}
fn fields<const N: usize>(
    done: Option<&Value>,
    final_value: Option<&Value>,
    names: &[&str; N],
) -> (u16, [ProviderJsonKind; N], [ProviderJsonKind; N]) {
    let mut mask = 0;
    let mut done_kinds = [ProviderJsonKind::Missing; N];
    let mut final_kinds = [ProviderJsonKind::Missing; N];
    for (index, name) in names.iter().enumerate() {
        let a = done.and_then(|v| v.get(*name));
        let b = final_value.and_then(|v| v.get(*name));
        done_kinds[index] = ProviderJsonKind::of(a);
        final_kinds[index] = ProviderJsonKind::of(b);
        if a != b {
            mask |= 1 << index;
        }
    }
    (mask, done_kinds, final_kinds)
}
fn other_fields(done: Option<&Value>, final_value: Option<&Value>, names: &[&str]) -> bool {
    let a = done.and_then(Value::as_object);
    let b = final_value.and_then(Value::as_object);
    a.into_iter().flatten().any(|(key, value)| {
        !names.contains(&key.as_str()) && b.and_then(|m| m.get(key)) != Some(value)
    }) || b.into_iter().flatten().any(|(key, value)| {
        !names.contains(&key.as_str()) && a.and_then(|m| m.get(key)) != Some(value)
    })
}
fn array_difference(
    done: Option<&Value>,
    final_value: Option<&Value>,
) -> Option<ProviderArrayDifference> {
    if done == final_value {
        return None;
    }
    let a = done.and_then(Value::as_array);
    let b = final_value.and_then(Value::as_array);
    let mut first_part: Option<ProviderPartDifference> = None;
    for index in 0..a.map_or(0, Vec::len).max(b.map_or(0, Vec::len)) {
        let left = a.and_then(|v| v.get(index));
        let right = b.and_then(|v| v.get(index));
        if left == right {
            continue;
        }
        if let Some(first) = &mut first_part {
            first.additional_parts = true;
            continue;
        }
        let (fields, done_kinds, final_kinds) = fields(left, right, &PROVIDER_PART_FIELDS);
        first_part = Some(ProviderPartDifference {
            index,
            fields: fields as u8,
            done_kinds,
            final_kinds,
            other_fields: other_fields(left, right, &PROVIDER_PART_FIELDS)
                || ProviderJsonKind::of(left) != ProviderJsonKind::of(right)
                || (left.is_some_and(|v| !v.is_object()) && left != right),
            additional_parts: false,
        });
    }
    Some(ProviderArrayDifference {
        done_count: a.map(Vec::len),
        final_count: b.map(Vec::len),
        first_part,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderFailure {
    pub stage: ProviderFailureStage,
    pub event: Option<&'static str>,
    pub field: Option<&'static str>,
    pub identity_problem: Option<ProviderIdentityProblem>,
    pub identity_bytes: Option<usize>,
    pub final_difference: Option<Box<ProviderFinalDifference>>,
}
impl ProviderFailure {
    pub(crate) fn at(stage: ProviderFailureStage) -> Self {
        Self {
            stage,
            event: None,
            field: None,
            identity_problem: None,
            identity_bytes: None,
            final_difference: None,
        }
    }
    pub(crate) fn event(kind: &str) -> &'static str {
        match kind {
            "response.created" => "response.created",
            "response.in_progress" => "response.in_progress",
            "response.output_item.added" => "response.output_item.added",
            "response.output_item.done" => "response.output_item.done",
            "response.function_call_arguments.delta" => "response.function_call_arguments.delta",
            "response.function_call_arguments.done" => "response.function_call_arguments.done",
            "response.content_part.added" => "response.content_part.added",
            "response.content_part.done" => "response.content_part.done",
            "response.output_text.delta" => "response.output_text.delta",
            "response.output_text.done" => "response.output_text.done",
            "response.reasoning_summary_part.added" => "response.reasoning_summary_part.added",
            "response.reasoning_summary_part.done" => "response.reasoning_summary_part.done",
            "response.reasoning_summary_text.delta" => "response.reasoning_summary_text.delta",
            "response.reasoning_summary_text.done" => "response.reasoning_summary_text.done",
            "response.completed" => "response.completed",
            "response.failed" => "response.failed",
            "response.incomplete" => "response.incomplete",
            _ => "unsupported event",
        }
    }
}
