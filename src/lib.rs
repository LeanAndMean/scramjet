mod coding;
mod commands;

pub use commands::coding::{
    CommandCodingAdapter, CommandCodingExecution, CommandCodingFact, CommandCodingReceipt,
};
pub use commands::project::{
    ProjectContinuation, ProjectControl, ProjectInvocation, ProjectLiveJourney, ProjectObserver,
    ProjectPhase, ProjectReady, ProjectSnapshot, ProjectSourceReceipt, ProjectVerificationReceipt,
};
pub use commands::readers::{
    CommandReaderAdapter, CommandReaderControl, CommandReaderExecution, CommandReaderFact,
    CommandReaderReceipt, CommandReaderSettlement,
};
pub use commands::{
    COMMAND_CATALOG_BYTES, COMMAND_INPUT_BYTES, COMMAND_INVOCATIONS, COMMAND_READS,
    COMMAND_TEXT_BYTES, CommandCompletion, CommandContinuation, CommandControl, CommandError,
    CommandInvocation, CommandInvocationFact, CommandObserver, CommandPhase, CommandPolicy,
    CommandReadFailure, CommandReadResult, CommandReportFact, CommandResume, CommandSession,
    CommandSnapshot, CommandStepFact,
};
mod http;
pub use http::provider::ProviderResponse;
pub use http::provider_capture::{
    PROVIDER_CAPTURE_BYTES, ProviderCapture, ProviderCaptureReceiver, ProviderReplay,
    replay_provider_data,
};
pub use http::provider_diagnostic::{
    PROVIDER_DIAGNOSTIC_BYTES, ProviderFailure, ProviderFailureStage, ProviderIdentityProblem,
};
pub use http::responses::{
    PROVIDER_ENVELOPE_BYTES, PROVIDER_FRAME_BYTES, PROVIDER_MODEL, PROVIDER_OUTPUT_ITEMS,
    ProviderEnvelope, ProviderFunctionCall, ProviderInput, ProviderItem, ProviderRequest,
    ProviderTool, ProviderUsage,
};
mod provider_validation;
pub use provider_validation::{
    provider_validation_fixture_observe, provider_validation_fixture_pair,
    provider_validation_live_pair, provider_validation_observe,
};
mod project;
mod readers;
mod runtime;
pub use project::{
    PROJECT_JOURNAL_BYTES, PROJECT_JOURNAL_RECORD_BYTES, PROJECT_JOURNAL_RECORDS,
    PROJECT_LOCK_NAME, PROJECT_REQUEST_BYTES, ProjectCapture, ProjectCaptured, ProjectClosure,
    ProjectCoding, ProjectFile, ProjectFinish, ProjectHeader, ProjectHistory, ProjectRead,
    ProjectReaderFact, ProjectReaders, ProjectReadyRecord, ProjectRecord, ProjectReport,
    ProjectRequest, ProjectRequested, ProjectSession, ProjectSettlement, ProjectWorkspace,
    reconstruct_project_history,
};

pub use readers::{
    READER_INSTRUCTION, ReaderAccounting, ReaderCleanup, ReaderControl, ReaderError, ReaderFact,
    ReaderFailure, ReaderInputs, ReaderObservation, ReaderObserver, ReaderParent, ReaderPhase,
    ReaderRun, ReaderSlot, ReaderState, ReaderTree, reconstruct_readers,
};

pub use coding::agent::{CodingAgent, CodingFollowUpAdmission, CodingRun};
pub use coding::haiku::agent::{HaikuAgent, HaikuRun};
pub use coding::haiku::{
    HAIKU_COMMAND, HAIKU_SCRIPT, HaikuAttempt, HaikuCapture, HaikuCertainty, HaikuControl,
    HaikuEffect, HaikuFact, HaikuHistory, HaikuHistoryEntry, HaikuIntent, HaikuObserver,
    HaikuOutcome, HaikuPhase, HaikuProcessResult, HaikuProposal, HaikuRecord, HaikuSession,
    HaikuSnapshot, HaikuStep, haiku_prompt, haiku_tools, reconstruct_haiku_history,
};
pub use coding::invocation::{
    CodingInvocation, CodingInvocationEffect, CodingInvocationIntent, CodingInvocationOptions,
    CodingInvocationResult,
};
pub use coding::observation::{
    CodingAccounting, CodingAttempt, CodingControl, CodingDecision, CodingObservation,
    CodingObserver, CodingProposal,
};
pub use coding::{
    CODING_HISTORY_PAYLOAD_BYTES, CodingCapability, CodingError, CodingHistory,
    CodingHistoryEnding, CodingHistoryEntry, CodingHistoryRecord, CodingMaterial, CodingSession,
    EditApproval, EditCancellation, EditIntent, EditResult, OperationFact, OperationState,
    RootIdentity, reconstruct_coding, reconstruct_coding_history,
};
pub use runtime::{
    ConcurrencyChange, ConcurrencyMode, ConcurrencySnapshot, InvalidRequestRate,
    InvalidSyntheticTokens, MAX_SYNTHETIC_TOKENS, Outcome, PROVIDER_POST_BUDGET,
    ProviderAttemptFact, ProviderHandle, ProviderHistory, ProviderTerminalEvidence, RequestHandle,
    RequestId, RequestRate, RetryAfter, RetryWait, Runtime, RuntimeBusy, Snapshot, SubmitError,
    SyntheticCost, SyntheticTokenBudget, SyntheticUsage, Terminal, reconstruct_provider_history,
};

pub fn smoke_message() -> &'static str {
    "scramjet-rs: offline bootstrap ready"
}

#[path = "../tests/support/responses_fixture.rs"]
#[allow(dead_code)]
mod provider_fixture;

#[cfg(test)]
mod tests {
    use super::smoke_message;

    #[test]
    fn smoke_message_identifies_the_offline_bootstrap() {
        assert_eq!(smoke_message(), "scramjet-rs: offline bootstrap ready");
    }
}
