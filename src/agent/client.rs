pub use crate::http::responses::StreamProgress;
use crate::http::responses::{ParserLimits, ResponsesParser};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, body::Incoming, client::conn::http1};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
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
/// Scramjet's patterns for a failure saying the input exceeded the model's context window
/// (`packages/ai/src/utils/overflow.ts`), matched in lower case: `#` is a number and `*` any
/// text; its optional words are spelled out.
const OVERFLOW_PATTERNS: &[&str] = &[
    "prompt is too long",
    "request_too_large",
    "input is too long for requested model",
    "exceeds the context window",
    "exceeds maximum context length of # token",
    "exceeds the maximum context length of # token",
    "exceeds model's maximum context length of # token",
    "exceeds models maximum context length of # token",
    "exceeds the model's maximum context length of # token",
    "exceeds the models maximum context length of # token",
    "input token count*exceeds the maximum",
    "maximum prompt length is #",
    "reduce the length of the messages",
    "maximum context length is # tokens",
    "input (# tokens) is longer than the model's context length (# tokens)",
    "input (# tokens) is longer than the models context length (# tokens)",
    "exceeds the limit of #",
    "exceeds the available context size",
    "greater than the context length",
    "context window exceeds limit",
    "exceeded model token limit",
    "too large for model with # maximum context length",
    "model_context_window_exceeded",
    "prompt too long; exceeded context length",
    "prompt too long; exceeded max context length",
    "context_length_exceeded",
    "context_length exceeded",
    "context length_exceeded",
    "context length exceeded",
    "too many tokens",
    "token limit exceeded",
];
/// Wording that mentions tokens or limits for another reason, such as throttling.
const NON_OVERFLOW_WORDS: &[&str] = &["rate limit", "too many requests", "throttl"];

pub(super) struct Reply {
    pub items: Vec<Value>,
    pub usage: Option<crate::http::responses::ProviderUsage>,
    /// Why the reply stopped before it was complete, when it reached the output token limit:
    /// `items` then keep its text, but not its tool calls.
    pub incomplete: Option<String>,
}

pub(super) enum ClientError {
    Cancelled,
    Failed(Failure),
}

pub(super) struct Failure {
    pub message: String,
    /// Whether another attempt may succeed: rate limits, overload, server and connection failures.
    pub transient: bool,
    /// The input exceeded the model's context window; compacting may let it through.
    pub overflow: bool,
    /// The provider's Retry-After wait; `Duration::MAX` when it is unrepresentable.
    pub wait: Option<Duration>,
}

impl Failure {
    pub(super) fn new(message: impl Into<String>, transient: bool) -> Self {
        Self {
            message: message.into(),
            transient,
            overflow: false,
            wait: None,
        }
    }
    /// A failure the provider explained, classified by its error code, then by its message.
    pub(super) fn provider(code: Option<&str>, message: String) -> Self {
        let transient = match code {
            Some(
                "rate_limit_exceeded"
                | "rate_limit_error"
                | "server_error"
                | "api_error"
                | "overloaded_error"
                | "timeout",
            ) => true,
            Some(
                "context_length_exceeded"
                | "insufficient_quota"
                | "billing_hard_limit_reached"
                | "invalid_request_error"
                | "authentication_error"
                | "permission_denied"
                | "permission_error"
                | "not_found"
                | "not_found_error"
                | "content_filter"
                | "content_policy_violation",
            ) => false,
            _ => !mentions(&message, PERMANENT_WORDS) && mentions(&message, TRANSIENT_WORDS),
        };
        Self {
            overflow: code == Some("context_length_exceeded") || overflows(&message),
            ..Self::new(message, transient)
        }
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

/// Whether a failure's message says the input exceeded the model's context window.
fn overflows(message: &str) -> bool {
    let message = message.to_lowercase();
    !mentions(&message, NON_OVERFLOW_WORDS)
        && OVERFLOW_PATTERNS
            .iter()
            .any(|pattern| contains_pattern(&message, pattern))
}

/// Whether `text` contains `pattern`, where `#` is a number (digits and commas) and `*` any text.
fn contains_pattern(text: &str, pattern: &str) -> bool {
    text.char_indices()
        .any(|(at, _)| matches_from(&text[at..], pattern))
}

fn matches_from(text: &str, pattern: &str) -> bool {
    let mut rest = pattern.chars();
    match rest.next() {
        None => true,
        Some('#') => {
            let number = text.len()
                - text
                    .trim_start_matches(|c: char| c.is_ascii_digit() || c == ',')
                    .len();
            (1..=number).any(|end| matches_from(&text[end..], rest.as_str()))
        }
        Some('*') => text
            .char_indices()
            .map(|(at, _)| at)
            .chain([text.len()])
            .any(|at| matches_from(&text[at..], rest.as_str())),
        Some(c) => text
            .strip_prefix(c)
            .is_some_and(|text| matches_from(text, rest.as_str())),
    }
}

pub(super) async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The endpoint's models (`/v1/models`); `None` when the list is not served in time.
pub(super) async fn models(endpoint: SocketAddr) -> Option<Vec<String>> {
    let list = tokio::time::timeout(CARDS_TIMEOUT, get(endpoint, "/v1/models"))
        .await
        .ok()??;
    let list: Value = serde_json::from_slice(&list).ok()?;
    Some(
        list["data"]
            .as_array()?
            .iter()
            .filter_map(|model| model["id"].as_str())
            .map(str::to_owned)
            .collect(),
    )
}

/// The protocol a model's requests use: Proxy serves Claude on Messages only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Protocol {
    Responses,
    Messages,
}

/// What the agent needs to know about a model to send it requests.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelInfo {
    pub protocol: Protocol,
    /// The context window (input and output tokens).
    pub window: Option<u64>,
    /// The most output tokens a response may have.
    pub output: Option<u64>,
    /// Whether the model reasons; Proxy's cards give `reasoning: null` for those that do not.
    pub reasoning: bool,
    /// The reasoning effort levels the card lists, in order; empty when it lists none.
    pub levels: Vec<String>,
    /// The card's default reasoning effort.
    pub default_level: Option<String>,
}

impl ModelInfo {
    /// What the model's name says, for a model without a card.
    pub(super) fn guess(model: &str) -> Self {
        Self {
            protocol: match model.starts_with("claude-") {
                true => Protocol::Messages,
                false => Protocol::Responses,
            },
            window: None,
            output: None,
            reasoning: true,
            levels: Vec::new(),
            default_level: None,
        }
    }

    /// `effort`, or when the card lists levels without it, the nearest higher level it lists, else
    /// the nearest lower one, as Scramjet clamps a thinking level. An effort not in Scramjet's
    /// order is left for the endpoint to judge.
    pub(super) fn fit<'a>(&'a self, effort: &'a str) -> &'a str {
        const ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
        let listed = |level: &str| self.levels.iter().any(|listed| listed == level);
        match ORDER.iter().position(|level| *level == effort) {
            Some(wanted) if !self.levels.is_empty() && !listed(effort) => ORDER[wanted..]
                .iter()
                .chain(ORDER[..wanted].iter().rev())
                .copied()
                .find(|level| listed(level))
                .unwrap_or(effort),
            _ => effort,
        }
    }

    /// From a Proxy model card: Responses when the card lists it, else Messages when it lists that.
    fn from_card(card: &Value) -> Self {
        let lists = |protocol: &str| {
            card["supportedProtocols"]
                .as_array()
                .is_some_and(|protocols| protocols.iter().any(|p| p == protocol))
        };
        Self {
            protocol: match !lists("openai-responses") && lists("anthropic-messages") {
                true => Protocol::Messages,
                false => Protocol::Responses,
            },
            window: card["limits"]["context"].as_u64(),
            output: card["limits"]["output"].as_u64(),
            reasoning: !card["reasoning"].is_null(),
            levels: card["reasoning"]["levels"]
                .as_array()
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|l| l.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            default_level: card["reasoning"]["default"].as_str().map(str::to_owned),
        }
    }
}

/// The model's Proxy model card; `None` when the endpoint does not serve the cards in time or does
/// not list the model.
pub(super) async fn model_info(endpoint: SocketAddr, model: &str) -> Option<ModelInfo> {
    cards(endpoint)
        .await?
        .iter()
        .find(|card| card["requestId"] == model)
        .map(ModelInfo::from_card)
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

/// `betas` is a Messages request's `anthropic-beta` header.
pub(super) async fn respond(
    endpoint: SocketAddr,
    protocol: Protocol,
    body: Vec<u8>,
    betas: Option<&str>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(StreamProgress),
) -> Result<Reply, ClientError> {
    tokio::select! {
        biased;
        () = cancelled(cancel) => Err(ClientError::Cancelled),
        result = exchange(endpoint, protocol, body, betas, progress) => result.map_err(ClientError::Failed),
    }
}

async fn exchange(
    endpoint: SocketAddr,
    protocol: Protocol,
    body: Vec<u8>,
    betas: Option<&str>,
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
    // Proxy replaces client credentials with its own.
    let request = match protocol {
        Protocol::Responses => Request::builder()
            .uri("/v1/responses")
            .header("Authorization", "Bearer managed-by-proxy"),
        Protocol::Messages => {
            let request = Request::builder()
                .uri("/v1/messages")
                .header("x-api-key", "managed-by-proxy")
                .header("anthropic-version", "2023-06-01");
            match betas {
                Some(betas) => request.header("anthropic-beta", betas),
                None => request,
            }
        }
    };
    let request = request
        .method("POST")
        .header("Host", endpoint.to_string())
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
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
    ) {
        Some(crate::http::retry_after::RetryWait::Delay(delay)) => Some(delay),
        Some(crate::http::retry_after::RetryWait::Unrepresentable) => Some(Duration::MAX),
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
            // Cerebras reports an oversized request as a 400 or 413 without a body.
            overflow: overflows(&text)
                || (matches!(status.as_u16(), 400 | 413) && text.trim().is_empty()),
            wait,
        })
    } else {
        match protocol {
            Protocol::Responses => responses_reply(&mut body, progress).await,
            Protocol::Messages => {
                let mut stream = super::messages::Stream::default();
                match read_body(&mut body, |data| stream.feed(data, progress)).await {
                    Ok(()) => stream.finish(),
                    Err(failure) => Err(failure),
                }
            }
        }
    };
    driver.abort();
    result
}

/// Passes each received chunk to `feed` until the body ends, `feed` fails or the stream stalls.
async fn read_body(
    body: &mut Incoming,
    mut feed: impl FnMut(&[u8]) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let mut received = 0usize;
    loop {
        let frame = match tokio::time::timeout(INACTIVITY, body.frame()).await {
            Err(_) => {
                return Err(Failure::new(
                    format!("no data for {}s", INACTIVITY.as_secs()),
                    true,
                ));
            }
            Ok(None) => return Ok(()),
            Ok(Some(Err(e))) => return Err(Failure::new(format!("stream failed: {e}"), true)),
            Ok(Some(Ok(frame))) => frame,
        };
        let Ok(data) = frame.into_data() else {
            continue;
        };
        received += data.len();
        if received > WIRE_BYTES {
            return Err(Failure::new(
                "response exceeded the client size limit",
                false,
            ));
        }
        feed(&data)?;
    }
}

async fn responses_reply(
    body: &mut Incoming,
    progress: &mut dyn FnMut(StreamProgress),
) -> Result<Reply, Failure> {
    let mut parser = ResponsesParser::with_limits(LIMITS);
    parser.progress = Some(Vec::new());
    read_body(body, |data| {
        let error = data.iter().find_map(|b| parser.byte(*b).err());
        for update in parser.progress.as_mut().unwrap().drain(..) {
            progress(update);
        }
        match error {
            // Without an error event, `Incomplete` means `[DONE]` before the terminal event.
            Some(outcome) => Err(match &parser.stream_error {
                Some(event) => Failure::event(event),
                None => {
                    let cut_short = outcome == crate::http::responses::Outcome::Incomplete;
                    Failure::new(describe(&parser, outcome), cut_short)
                }
            }),
            None => Ok(()),
        }
    })
    .await?;
    match parser.complete() {
        Err(outcome) => Err(Failure::new(
            format!("stream ended before the response completed ({outcome:?})"),
            outcome == crate::http::responses::Outcome::Incomplete,
        )),
        Ok(parsed) => match parsed.envelope {
            Some(envelope) => {
                let mut items: Vec<Value> = (envelope.items.into_iter())
                    .map(|item| item.raw().clone())
                    .collect();
                let incomplete = (parsed.status == "incomplete").then(|| {
                    // Kept as the Claude reply cut at its limit is: its messages, and the
                    // reasoning before them, which a replay needs followed by an item.
                    let last = items.iter().rposition(|item| item["type"] == "message");
                    items.truncate(last.map_or(0, |last| last + 1));
                    // Sent again as Scramjet replays a message, completed.
                    for item in items.iter_mut().filter(|item| item["type"] == "message") {
                        item["status"] = json!("completed");
                    }
                    "response incomplete: it reached the output token limit (max_output_tokens)"
                        .to_owned()
                });
                Ok(Reply {
                    items,
                    usage: parsed.usage,
                    incomplete,
                })
            }
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
    }
}

fn describe(parser: &ResponsesParser, outcome: crate::http::responses::Outcome) -> String {
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
