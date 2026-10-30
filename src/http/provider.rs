use super::provider_diagnostic::{ProviderFailure, ProviderFailureStage};
use super::{LimitedSocket, Meter, cancelled, classify, responses::ResponsesParser};
use crate::runtime::{
    Shared,
    provider_history::{self, ProviderLedger, ProviderTerminalEvidence},
};
use crate::{Outcome, ProviderEnvelope, ProviderUsage, RequestId, SyntheticUsage, Terminal};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, client::conn::http1};
use hyper_util::rt::TokioIo;
use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use tokio::{net::TcpStream, sync::watch};

#[cfg(test)]
pub(crate) struct FrameProbe {
    pub(crate) observed_at: tokio::time::Instant,
    pub(crate) frames: usize,
    pub(crate) data: Vec<u8>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static FRAME_PROBE: std::cell::RefCell<Option<FrameProbe>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone, Debug)]
pub struct ProviderResponse {
    pub id: RequestId,
    pub reservation: Option<usize>,
    pub failure: Option<ProviderFailure>,
    pub outcome: Outcome,
    pub envelope: Option<ProviderEnvelope>,
    pub usage: Option<ProviderUsage>,
    pub remote_terminal: Option<ProviderTerminalEvidence>,
    pub durable: bool,
    pub wire_bytes: usize,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn attempt(
    endpoint: SocketAddr,
    id: RequestId,
    input: String,
    mut cancel: watch::Receiver<Option<Outcome>>,
    shared: &Shared,
    deadline: tokio::time::Instant,
    ledger: Arc<Mutex<ProviderLedger>>,
    capture_sender: Option<tokio::sync::oneshot::Sender<crate::ProviderCapture>>,
) -> (Terminal, bool, Option<ProviderResponse>) {
    let mut response = ProviderResponse {
        id,
        reservation: None,
        failure: None,
        outcome: Outcome::Transport,
        envelope: None,
        usage: None,
        remote_terminal: None,
        durable: false,
        wire_bytes: 0,
    };
    let ordinal = provider_history::reserve(ledger.clone(), id.0).await;
    response.reservation = ordinal.as_ref().ok().copied();
    let mut capture = capture_sender
        .as_ref()
        .map(|_| crate::ProviderCapture::new(id, response.reservation));
    let meter = Arc::new(Meter {
        wire: AtomicUsize::new(0),
        limited: AtomicBool::new(false),
        malformed: AtomicBool::new(false),
        invalid_usage: AtomicBool::new(false),
    });
    let mut connection = None;
    let mut body_bytes = 0usize;
    let mut retry_after = None;
    let mut dispatch_to_head = None;
    let mut parsed = None;
    let mut failure = ProviderFailure::at(ProviderFailureStage::Reservation);
    let work = async {
        ordinal.as_ref().map_err(Clone::clone)?;
        if ledger.lock().unwrap().validation_cancelled() {
            return Err(Outcome::Cancelled);
        }
        if ledger.lock().unwrap().validation_metadata_expired() {
            return Err(Outcome::Deadline);
        }
        failure.stage = ProviderFailureStage::Connect;
        let socket = TcpStream::connect(endpoint)
            .await
            .map_err(|_| Outcome::Transport)?;
        let io = TokioIo::new(LimitedSocket {
            socket,
            meter: meter.clone(),
            head_bytes: 0,
            max_headers: super::PROVIDER_HEADERS,
            head_lines: 0,
            head_line_empty: true,
            head_cr: false,
            in_head: true,
            line: [0; 128],
            line_len: 0,
        });
        failure.stage = ProviderFailureStage::Handshake;
        let (mut sender, driver) = http1::Builder::new()
            .max_headers(super::PROVIDER_HEADERS)
            .max_header_size(super::RESPONSE_HEAD_BYTES)
            .max_buf_size(8192)
            .handshake(io)
            .await
            .map_err(|_| Outcome::Transport)?;
        connection = Some(tokio::spawn(driver));
        let request = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("Host", endpoint.to_string())
            .header("Connection", "close")
            .header("Content-Type", "application/json")
            .header("Authorization", "Bearer managed-by-proxy")
            .header("Content-Length", input.len())
            .body(Full::new(Bytes::from(input)))
            .expect("fixed provider metadata");
        if tokio::time::Instant::now() >= deadline {
            return Err(Outcome::Deadline);
        }
        if ledger.lock().unwrap().validation_cancelled() {
            return Err(Outcome::Cancelled);
        }
        #[cfg(test)]
        shared
            .checkpoint(crate::runtime::Checkpoint::ProviderBeforeDispatch)
            .await;
        loop {
            let wait = shared.provider_dispatch_delay();
            if wait.is_zero() {
                break;
            }
            tokio::time::sleep(wait).await;
        }
        {
            let owner = ledger.lock().unwrap();
            if owner.validation_cancelled() {
                return Err(Outcome::Cancelled);
            }
            if owner.validation_metadata_expired() {
                return Err(Outcome::Deadline);
            }
        }
        shared.dispatched(id);
        let dispatched = tokio::time::Instant::now();
        failure.stage = ProviderFailureStage::HttpHead;
        let received = sender.send_request(request).await.map_err(|error| {
            classify(
                &error,
                &meter,
                if error.is_parse() {
                    Outcome::Framing
                } else {
                    Outcome::Transport
                },
            )
        })?;
        let head_time = tokio::time::Instant::now();
        dispatch_to_head = Some(head_time.saturating_duration_since(dispatched));
        if head_time >= deadline {
            return Err(Outcome::Deadline);
        }
        if received.status() == 429 {
            retry_after = Some(super::retry_after::parse(
                received
                    .headers()
                    .get_all("retry-after")
                    .iter()
                    .map(|v| v.as_bytes()),
                std::time::SystemTime::now(),
            ));
        }
        shared.head(id, received.status().as_u16(), retry_after, head_time);
        if received.status() != 200 {
            return Ok(Outcome::HttpRejected(received.status().as_u16()));
        }
        failure.stage = ProviderFailureStage::ContentType;
        if received.headers().get_all("content-type").iter().count() != 1
            || received
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_none_or(|v| v.split(';').next().unwrap().trim() != "text/event-stream")
        {
            return Err(Outcome::Framing);
        }
        let mut body = received.into_body();
        let mut parser = ResponsesParser::new();
        failure.stage = ProviderFailureStage::Body;
        while let Some(frame) = body.frame().await {
            // DATA already yielded by the body remains evidence even if its parser deadline elapsed.
            if let Some(capture) = &mut capture
                && let Ok(frame) = &frame
                && let Some(bytes) = frame.data_ref()
            {
                capture.receive(bytes);
            }
            let observed_at = tokio::time::Instant::now();
            #[cfg(test)]
            let observed_at = FRAME_PROBE.with(|probe| {
                let mut probe = probe.borrow_mut();
                if let Some(probe) = probe.as_mut() {
                    probe.frames += 1;
                    if let Ok(frame) = &frame
                        && let Some(bytes) = frame.data_ref()
                    {
                        probe.data.extend_from_slice(bytes);
                    }
                    probe.observed_at
                } else {
                    observed_at
                }
            });
            if observed_at >= deadline {
                return Err(Outcome::Deadline);
            }
            let frame = frame.map_err(|error| classify(&error, &meter, Outcome::Incomplete))?;
            if frame.trailers_ref().is_some() {
                return Err(Outcome::Framing);
            }
            if let Ok(bytes) = frame.into_data() {
                body_bytes = body_bytes
                    .checked_add(bytes.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if body_bytes > crate::runtime::MAX_WIRE {
                    return Err(Outcome::ResourceLimit);
                }
                for byte in bytes {
                    if let Some(capture) = &mut capture {
                        capture.parser_fed_bytes += 1;
                    }
                    if let Err(outcome) = parser.byte(byte) {
                        failure = parser.failure.clone();
                        return Err(outcome);
                    }
                }
            }
        }
        if let Some(capture) = &mut capture {
            capture.body_eof = true;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Outcome::Deadline);
        }
        failure = parser.failure.clone();
        parsed = Some(parser.complete().inspect_err(|_| {
            failure.stage = ProviderFailureStage::MissingTerminal;
        })?);
        let completed = parsed.as_ref().unwrap();
        if let Some(capture) = &mut capture {
            capture.parser_accepted = completed.refusal.is_none();
        }
        Ok(if let Some(refusal) = &completed.refusal {
            failure = refusal.clone();
            Outcome::Unsupported
        } else if completed.envelope.is_some() {
            Outcome::Success
        } else {
            Outcome::Incomplete
        })
    };
    let outcome = {
        tokio::pin!(work);
        tokio::select! {
            biased;
            reason = cancelled(&mut cancel) => reason,
            _ = tokio::time::sleep_until(deadline) => Outcome::Deadline,
            result = &mut work => result.unwrap_or_else(|reason| reason),
        }
    };
    if let Some(driver) = connection {
        driver.abort();
        let _ = driver.await;
    }
    if let Some(parsed) = parsed {
        response.envelope = parsed.envelope;
        response.usage = parsed.usage;
        response.remote_terminal = Some(ProviderTerminalEvidence {
            response_id: parsed.response_id,
            reported_model: parsed.reported_model,
            status: parsed.status,
        });
    }
    response.outcome = outcome;
    if let Ok(ordinal) = ordinal {
        #[cfg(test)]
        shared
            .checkpoint(crate::runtime::Checkpoint::ProviderBeforeAcknowledgement)
            .await;
        response.durable = provider_history::joined(
            ledger,
            ordinal,
            response.remote_terminal.clone(),
            response.usage.clone(),
        )
        .await;
        if !response.durable {
            response.outcome = Outcome::ResourceLimit;
            failure = ProviderFailure::at(ProviderFailureStage::Acknowledgement);
        }
    }
    if response.outcome != Outcome::Success {
        response.failure = Some(failure);
    }
    if response.outcome != Outcome::Success || !response.durable {
        response.envelope = None;
    }
    response.wire_bytes = meter.wire.load(Ordering::Relaxed);
    let observed = response.durable && response.remote_terminal.is_some();
    let terminal = Terminal {
        id,
        outcome: response.outcome.clone(),
        body_bytes,
        wire_bytes: response.wire_bytes,
        text_bytes: 0,
        retry_after,
        synthetic_usage: SyntheticUsage::Unknown,
        dispatch_to_head,
        commitment_to_terminal: None,
        output_backpressure: false,
    };
    if let (Some(sender), Some(mut capture)) = (capture_sender, capture) {
        capture.local_joined = true;
        let _ = sender.send(capture);
    }
    (terminal, observed, Some(response))
}
