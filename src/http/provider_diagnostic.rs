use serde::Serialize;

pub const PROVIDER_DIAGNOSTIC_BYTES: usize = 4096;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProviderFailure {
    pub stage: ProviderFailureStage,
    pub event: Option<&'static str>,
    pub field: Option<&'static str>,
    pub identity_problem: Option<ProviderIdentityProblem>,
    pub identity_bytes: Option<usize>,
}
impl ProviderFailure {
    pub(crate) fn at(stage: ProviderFailureStage) -> Self {
        Self {
            stage,
            event: None,
            field: None,
            identity_problem: None,
            identity_bytes: None,
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
