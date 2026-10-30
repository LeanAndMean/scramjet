pub use crate::http::responses::StreamProgress;
use crate::http::responses::{ParserLimits, ResponsesParser};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, client::conn::http1};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};
use tokio::net::TcpStream;

/// Large enough for real coding turns; the model's own context limit applies first.
pub(super) const LIMITS: ParserLimits = ParserLimits {
    frame: 32 << 20,
    envelope: 32 << 20,
    items: 256,
};
const WIRE_BYTES: usize = 256 << 20;
const ERROR_BODY_BYTES: usize = 64 << 10;
/// Proxy's model card document is about 73KB.
const CARDS_BYTES: usize = 4 << 20;
/// The lookup runs before the first prompt; Proxy answers in milliseconds.
const CARDS_TIMEOUT: Duration = Duration::from_millis(500);
/// Like Scramjet's provider timeout: reset by every received chunk, so long generations continue.
const INACTIVITY: Duration = Duration::from_secs(600);
/// Failure wording that another attempt cannot fix, checked before `TRANSIENT_WORDS`.
const PERMANENT_WORDS: &[&str] = &[
    "context length",
    "context window",
    "maximum context",
    "quota",
    "billing",
];
const TRANSIENT_WORDS: &[&str] = &[
    "rate limit",
    "too many requests",
    "overload",
    "capacity",
    "timeout",
    "timed out",
    "server error",
    "internal error",
    "service unavailable",
    "bad gateway",
    "upstream",
    "connection",
    "network",
    "stream ended",
    "retry",
];

pub(super) struct Reply {
    pub items: Vec<Value>,
    pub usage: Option<crate::ProviderUsage>,
}

pub(super) enum ClientError {
    Cancelled,
    Failed(Failure),
}

pub(super) struct Failure {
    pub message: String,
    /// Whether another attempt may succeed: rate limits, overload, server and connection failures.
    pub transient: bool,
    /// The provider's Retry-After wait; `Duration::MAX` when it is unrepresentable.
    pub wait: Option<Duration>,
}

impl Failure {
    fn new(message: impl Into<String>, transient: bool) -> Self {
        Self {
            message: message.into(),
            transient,
            wait: None,
        }
    }
    /// A failure the provider explained, classified by its error code, then by its message.
    pub(super) fn provider(code: Option<&str>, message: String) -> Self {
        let transient = match code {
            Some("rate_limit_exceeded" | "server_error" | "overloaded_error" | "timeout") => true,
            Some(
                "context_length_exceeded"
                | "insufficient_quota"
                | "billing_hard_limit_reached"
                | "invalid_request_error"
                | "authentication_error"
                | "permission_denied"
                | "not_found"
                | "content_filter"
                | "content_policy_violation",
            ) => false,
            _ => !mentions(&message, PERMANENT_WORDS) && mentions(&message, TRANSIENT_WORDS),
        };
        Self::new(message, transient)
    }
    /// An `error` event; its fields may be at the top level or under `error`.
    fn event(event: &Value) -> Self {
        let field = |name: &str| event[name].as_str().or(event["error"][name].as_str());
        let message = field("message").unwrap_or("no message");
        Self::provider(field("code"), format!("provider error: {message}"))
    }
}

fn mentions(message: &str, words: &[&str]) -> bool {
    let message = message.to_lowercase();
    words.iter().any(|word| message.contains(word))
}

pub(super) async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The endpoint's models (`/v1/models`) except those whose Proxy model card says they do not
/// accept Responses requests; `None` when the list is not served in time.
pub(super) async fn models(endpoint: SocketAddr) -> Option<Vec<String>> {
    let list = tokio::time::timeout(CARDS_TIMEOUT, get(endpoint, "/v1/models"))
        .await
        .ok()??;
    let list: Value = serde_json::from_slice(&list).ok()?;
    let cards = cards(endpoint).await.unwrap_or_default();
    let responses = |id: &str| {
        let card = cards.iter().find(|card| card["requestId"] == id);
        match card.and_then(|card| card["supportedProtocols"].as_array()) {
            Some(protocols) => protocols.iter().any(|p| p == "openai-responses"),
            None => true,
        }
    };
    Some(
        list["data"]
            .as_array()?
            .iter()
            .filter_map(|model| model["id"].as_str())
            .filter(|id| responses(id))
            .map(str::to_owned)
            .collect(),
    )
}

/// The model's context window (input and output tokens) from Proxy's model cards; `None` when
/// the endpoint does not serve them in time or does not list the model.
pub(super) async fn context_window(endpoint: SocketAddr, model: &str) -> Option<u64> {
    cards(endpoint)
        .await?
        .iter()
        .find(|card| card["requestId"] == model)?["limits"]["context"]
        .as_u64()
}

async fn cards(endpoint: SocketAddr) -> Option<Vec<Value>> {
    let cards = tokio::time::timeout(CARDS_TIMEOUT, get(endpoint, "/api/model-cards"))
        .await
        .ok()??;
    let mut cards: Value = serde_json::from_slice(&cards).ok()?;
    match cards["models"].take() {
        Value::Array(cards) => Some(cards),
        _ => None,
    }
}

async fn get(endpoint: SocketAddr, path: &str) -> Option<Bytes> {
    let socket = TcpStream::connect(endpoint).await.ok()?;
    let (mut sender, connection) = http1::handshake(TokioIo::new(socket)).await.ok()?;
    let request = Request::builder()
        .uri(path)
        .header("Host", endpoint.to_string())
        .body(Full::new(Bytes::new()))
        .ok()?;
    let mut exchange = std::pin::pin!(async {
        let response = sender.send_request(request).await.ok()?;
        if response.status() != 200 {
            return None;
        }
        let body = http_body_util::Limited::new(response.into_body(), CARDS_BYTES);
        Some(body.collect().await.ok()?.to_bytes())
    });
    // A connection closed after the response may end first; the response is still complete.
    tokio::select! {
        body = &mut exchange => body,
        _ = connection => exchange.await,
    }
}

pub(super) async fn respond(
    endpoint: SocketAddr,
    body: Vec<u8>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(StreamProgress),
) -> Result<Reply, ClientError> {
    tokio::select! {
        biased;
        () = cancelled(cancel) => Err(ClientError::Cancelled),
        result = exchange(endpoint, body, progress) => result.map_err(ClientError::Failed),
    }
}

async fn exchange(
    endpoint: SocketAddr,
    body: Vec<u8>,
    progress: &mut dyn FnMut(StreamProgress),
) -> Result<Reply, Failure> {
    let socket = TcpStream::connect(endpoint).await.map_err(|e| {
        Failure::new(
            format!("cannot connect to {endpoint}: {e} (is Proxy running?)"),
            true,
        )
    })?;
    let (mut sender, driver) = http1::handshake(TokioIo::new(socket))
        .await
        .map_err(|e| Failure::new(format!("HTTP handshake failed: {e}"), true))?;
    let driver = tokio::spawn(driver);
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("Host", endpoint.to_string())
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("Authorization", "Bearer managed-by-proxy")
        .header("Content-Length", body.len())
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| Failure::new(e.to_string(), false))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| Failure::new(format!("request failed: {e}"), true))?;
    let status = response.status();
    let wait = match crate::http::retry_after::parse(
        response
            .headers()
            .get_all(hyper::header::RETRY_AFTER)
            .iter()
            .map(|value| value.as_bytes()),
        SystemTime::now(),
    )
    .wait
    {
        Some(crate::RetryWait::Delay(delay)) => Some(delay),
        Some(crate::RetryWait::Unrepresentable) => Some(Duration::MAX),
        None => None,
    };
    let mut body = response.into_body();
    let result = if status != 200 {
        let mut text = Vec::new();
        while text.len() < ERROR_BODY_BYTES {
            match tokio::time::timeout(INACTIVITY, body.frame()).await {
                Ok(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        text.extend_from_slice(&data);
                    }
                }
                _ => break,
            }
        }
        text.truncate(ERROR_BODY_BYTES);
        let text = String::from_utf8_lossy(&text);
        Err(Failure {
            message: format!("HTTP {status}: {}", text.trim()),
            transient: matches!(status.as_u16(), 408 | 429 | 500..)
                && !mentions(&text, PERMANENT_WORDS),
            wait,
        })
    } else {
        let mut parser = ResponsesParser::with_limits(LIMITS);
        parser.progress = Some(Vec::new());
        let mut received = 0usize;
        let mut failure = None;
        loop {
            let frame = match tokio::time::timeout(INACTIVITY, body.frame()).await {
                Err(_) => {
                    failure = Some(Failure::new(
                        format!("no data for {}s", INACTIVITY.as_secs()),
                        true,
                    ));
                    break;
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    failure = Some(Failure::new(format!("stream failed: {e}"), true));
                    break;
                }
                Ok(Some(Ok(frame))) => frame,
            };
            let Ok(data) = frame.into_data() else {
                continue;
            };
            received += data.len();
            if received > WIRE_BYTES {
                failure = Some(Failure::new(
                    "response exceeded the client size limit",
                    false,
                ));
                break;
            }
            let error = data.iter().find_map(|b| parser.byte(*b).err());
            for update in parser.progress.as_mut().unwrap().drain(..) {
                progress(update);
            }
            if let Some(outcome) = error {
                // Without an error event, `Incomplete` means `[DONE]` before the terminal event.
                let cut_short = outcome == crate::Outcome::Incomplete;
                failure = Some(match &parser.stream_error {
                    Some(event) => Failure::event(event),
                    None => Failure::new(describe(&parser, outcome), cut_short),
                });
                break;
            }
        }
        match failure {
            Some(failure) => Err(failure),
            None => match parser.complete() {
                Err(outcome) => Err(Failure::new(
                    format!("stream ended before the response completed ({outcome:?})"),
                    outcome == crate::Outcome::Incomplete,
                )),
                Ok(parsed) => match parsed.envelope {
                    Some(envelope) => Ok(Reply {
                        items: envelope
                            .items
                            .into_iter()
                            .map(|i| i.raw().clone())
                            .collect(),
                        usage: parsed.usage,
                    }),
                    None => {
                        let message = format!(
                            "response {}{}",
                            parsed.status,
                            parsed.error.map(|e| format!(": {e}")).unwrap_or_default()
                        );
                        // An incomplete response (output limit, content filter) would end the same way again.
                        Err(match parsed.status.as_str() {
                            "failed" => Failure::provider(parsed.error_code.as_deref(), message),
                            _ => Failure::new(message, false),
                        })
                    }
                },
            },
        }
    };
    driver.abort();
    result
}

fn describe(parser: &ResponsesParser, outcome: crate::Outcome) -> String {
    let failure = &parser.failure;
    format!(
        "unexpected response stream ({outcome:?} at {:?}{}{})",
        failure.stage,
        failure
            .event
            .map(|e| format!(", event {e}"))
            .unwrap_or_default(),
        failure
            .field
            .map(|f| format!(", field {f}"))
            .unwrap_or_default(),
    )
}
