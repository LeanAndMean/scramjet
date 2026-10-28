use super::{provider_diagnostic::ProviderFailure, responses::ResponsesParser};
use crate::{Outcome, RequestId};
use tokio::sync::oneshot;
#[cfg(test)]
#[path = "provider_capture_tests.rs"]
mod tests;

pub const PROVIDER_CAPTURE_BYTES: usize = crate::runtime::MAX_WIRE;

pub struct ProviderCapture {
    pub id: RequestId,
    pub reservation: Option<usize>,
    pub received_bytes: usize,
    pub parser_fed_bytes: usize,
    pub body_eof: bool,
    pub overflow: bool,
    pub parser_accepted: bool,
    pub local_joined: bool,
    pub bytes: Vec<u8>,
}

impl ProviderCapture {
    pub(crate) fn new(id: RequestId, reservation: Option<usize>) -> Self {
        Self {
            id,
            reservation,
            received_bytes: 0,
            parser_fed_bytes: 0,
            body_eof: false,
            overflow: false,
            parser_accepted: false,
            local_joined: false,
            bytes: Vec::with_capacity(PROVIDER_CAPTURE_BYTES),
        }
    }
    pub(crate) fn receive(&mut self, bytes: &[u8]) {
        self.received_bytes += bytes.len();
        if bytes.len() > PROVIDER_CAPTURE_BYTES - self.bytes.len() {
            self.overflow = true;
        } else if !self.overflow {
            self.bytes.extend_from_slice(bytes);
        }
    }
}

pub struct ProviderCaptureReceiver(pub(crate) oneshot::Receiver<ProviderCapture>);
impl ProviderCaptureReceiver {
    pub async fn receive(self) -> Option<ProviderCapture> {
        self.0.await.ok()
    }
}

pub struct ProviderReplay {
    pub outcome: Outcome,
    pub failure: Option<ProviderFailure>,
    pub parser_accepted: bool,
}

pub fn replay_provider_data(bytes: &[u8]) -> ProviderReplay {
    let mut parser = ResponsesParser::new();
    let result = if bytes.len() > PROVIDER_CAPTURE_BYTES {
        Err(Outcome::ResourceLimit)
    } else {
        bytes.iter().try_for_each(|byte| parser.byte(*byte))
    };
    let mut failure = parser.failure.clone();
    let result = result.and_then(|()| {
        parser.complete().inspect_err(|_| {
            failure.stage = super::provider_diagnostic::ProviderFailureStage::MissingTerminal;
        })
    });
    match result {
        Ok(parsed) => ProviderReplay {
            outcome: if parsed.refusal.is_some() {
                Outcome::Unsupported
            } else if parsed.envelope.is_some() {
                Outcome::Success
            } else {
                Outcome::Incomplete
            },
            parser_accepted: parsed.refusal.is_none(),
            failure: parsed.refusal,
        },
        Err(outcome) => ProviderReplay {
            outcome,
            failure: Some(failure),
            parser_accepted: false,
        },
    }
}
