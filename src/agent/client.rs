use super::providers::Route;
pub use crate::http::responses::StreamProgress;
use crate::http::responses::{ParserLimits, ResponsesParser};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, body::Incoming, client::conn::http1};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    task::JoinHandle,
};

/// Large enough for real coding turns; the model's own context limit applies first.
pub(super) const LIMITS: ParserLimits = ParserLimits {
    frame: 32 << 20,
    envelope: 32 << 20,
    items: 256,
};
const WIRE_BYTES: usize = 256 << 20;
const ERROR_BODY_BYTES: usize = 64 << 10;
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

#[derive(Default)]
pub(super) struct Reply {
    pub items: Vec<Value>,
    pub usage: Option<crate::http::responses::ProviderUsage>,
    /// Why the reply stopped before it was complete, when it reached the output token limit:
    /// `items` then keep its text, but not its tool calls.
    pub incomplete: Option<String>,
    /// The value of the response header its model's prices are chosen by, in lower case.
    pub priced_by: Option<String>,
    /// What its response's headers said of the account's quota (ChatGPT's, Copilot's).
    pub quota: Option<super::quota::Quota>,
    /// What it cost in dollars, when its model's prices say (`request::send`).
    pub cost: Option<f64>,
    /// From its request to its end, retries aside (`request::send`).
    pub elapsed: Duration,
    /// When its response's headers came, and its first text, reasoning or call streamed.
    pub headers_at: Option<Instant>,
    pub first_output_at: Option<Instant>,
    /// Set by `request::send`.
    pub timing: Option<Timing>,
}

/// When a reply's request was sent and how long its parts took, with the attempts that failed
/// before it.
#[derive(Clone, Debug)]
pub(super) struct Timing {
    pub started_at: SystemTime,
    pub headers: Option<Duration>,
    pub first_output: Option<Duration>,
    pub elapsed: Duration,
    pub failed: Vec<FailedAttempt>,
}

/// An attempt that failed before a reply came, retried.
#[derive(Clone, Debug)]
pub(super) struct FailedAttempt {
    pub started_at: SystemTime,
    pub elapsed: Duration,
    pub error: String,
}

pub(super) enum ClientError {
    Cancelled,
    Failed(Failure),
}

#[derive(Debug)]
pub(super) struct Failure {
    pub message: String,
    /// Whether another attempt may succeed: rate limits, overload, server and connection failures.
    pub transient: bool,
    /// The input exceeded the model's context window; compacting may let it through.
    pub overflow: bool,
    /// The provider's Retry-After wait; `Duration::MAX` when it is unrepresentable.
    pub wait: Option<Duration>,
    /// What its response's headers said of the account's quota, as a reply's.
    pub quota: Option<super::quota::Quota>,
}

/// A failure nothing says another attempt may cure, such as a file's.
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new(message, false)
    }
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        failure.message
    }
}

impl Failure {
    pub(super) fn new(message: impl Into<String>, transient: bool) -> Self {
        Self {
            message: message.into(),
            transient,
            overflow: false,
            wait: None,
            quota: None,
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

/// Keeps of a reply stopped at the output token limit its text, with the reasoning before it, but
/// not its tool calls, whose input may be cut off; says so, naming `limit`, the request's field.
pub(super) fn cut_at_limit(items: &mut Vec<Value>, limit: &str) -> String {
    let calls = (items.iter())
        .filter(|item| item["type"] == "function_call")
        .count();
    items.retain(|item| item["type"] != "function_call");
    if !items.iter().any(|item| item["type"] == "message") {
        items.clear();
    }
    let mut message = format!("response incomplete: it reached the output token limit ({limit})");
    match calls {
        0 => {}
        1 => message.push_str("; the tool call it was making did not run"),
        n => message.push_str(&format!("; the {n} tool calls in it did not run")),
    }
    message
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

/// The protocol a model's requests use.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Protocol {
    Responses,
    /// ChatGPT's Codex backend: Responses, with what it takes (`codex`).
    Codex,
    Messages,
    /// Chat Completions, with what the server accepts.
    Completions,
}

/// What a request sends besides its body: `headers` are those of this request (a Messages
/// request's `anthropic-beta`), after the route's, and `priced_by` names the response header
/// `Reply::priced_by` keeps.
pub(super) struct Sent<'a> {
    pub route: &'a Route,
    pub protocol: Protocol,
    pub headers: &'a [(&'static str, String)],
    pub priced_by: Option<&'a str>,
    /// What a Chat Completions server accepts.
    pub compat: &'a super::completions::Compat,
}

pub(super) async fn respond(
    sent: Sent<'_>,
    body: Vec<u8>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(StreamProgress),
) -> Result<Reply, ClientError> {
    tokio::select! {
        biased;
        () = cancelled(cancel) => Err(ClientError::Cancelled),
        result = exchange(sent, body, progress) => result.map_err(ClientError::Failed),
    }
}

async fn exchange(
    Sent {
        route,
        protocol,
        headers,
        priced_by,
        compat,
    }: Sent<'_>,
    body: Vec<u8>,
    shown: &mut dyn FnMut(StreamProgress),
) -> Result<Reply, Failure> {
    let mut first_output_at = None;
    let progress = &mut |update| {
        first_output_at.get_or_insert_with(Instant::now);
        shown(update)
    };
    let authority = &route.authority;
    let (mut sender, driver) = connect(route).await?;
    let mut request = match protocol {
        Protocol::Responses => Request::builder().uri(format!("{}/responses", route.path)),
        Protocol::Codex => Request::builder().uri(super::codex::path(&route.path)),
        Protocol::Completions => Request::builder().uri(format!("{}/chat/completions", route.path)),
        Protocol::Messages => Request::builder()
            .uri(format!("{}/v1/messages", route.path))
            .header("anthropic-version", "2023-06-01"),
    };
    for (name, value) in &route.headers {
        request = request.header(name, value);
    }
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let request = request
        .method("POST")
        .header("Host", authority)
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .header("Content-Length", body.len())
        .body(Full::new(Bytes::from(body)))
        .map_err(|e| Failure::new(e.to_string(), false))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| Failure::new(format!("request failed: {e}"), true))?;
    let headers_at = Some(Instant::now());
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
    let priced_by = (priced_by.and_then(|header| response.headers().get(header)))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    let quota = {
        let headers: Vec<(&str, &str)> = (response.headers().iter())
            .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?)))
            .collect();
        super::quota::from_headers(&headers, super::login::now_ms() as u64)
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
        let limit = (protocol == Protocol::Codex)
            .then(|| super::codex::usage_limit(&text))
            .flatten();
        match limit {
            Some(limit) => Err(Failure::new(format!("HTTP {status}: {limit}"), false)),
            None => Err(Failure {
                message: format!("HTTP {status}: {}", text.trim()),
                transient: matches!(status.as_u16(), 408 | 429 | 500..)
                    && !mentions(&text, PERMANENT_WORDS),
                // Cerebras reports an oversized request as a 400 or 413 without a body.
                overflow: overflows(&text)
                    || (matches!(status.as_u16(), 400 | 413) && text.trim().is_empty()),
                wait,
                quota: None,
            }),
        }
    } else {
        match protocol {
            Protocol::Responses | Protocol::Codex => responses_reply(&mut body, progress).await,
            Protocol::Messages => {
                let mut stream = super::messages::Stream::default();
                match read_body(&mut body, |data| stream.feed(data, progress)).await {
                    Ok(()) => stream.finish(),
                    Err(failure) => Err(failure),
                }
            }
            Protocol::Completions => {
                let mut stream = super::completions::Stream::new(compat);
                match read_body(&mut body, |data| stream.feed(data, progress)).await {
                    Ok(()) => stream.finish(),
                    Err(failure) => Err(failure),
                }
            }
        }
    };
    driver.abort();
    match result {
        Ok(reply) => Ok(Reply {
            priced_by,
            quota,
            headers_at,
            first_output_at,
            ..reply
        }),
        // A response refused at the limit reports the quota too.
        Err(failure) => Err(Failure { quota, ..failure }),
    }
}

/// What a request other than a model's (a login's) was answered: its status and body.
pub(super) struct Fetched {
    pub status: u16,
    pub body: Vec<u8>,
}

const FETCHED_BYTES: usize = 4 << 20;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Sends a request other than a model's, such as a login's, to `url` for `what` (a provider's
/// name, in failures), `body` with its content type, and waits at most 30 seconds for its answer.
/// A failure is transient when no answer came, as a model request's.
pub(super) async fn fetch(
    what: &str,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<(&str, Vec<u8>)>,
) -> Result<Fetched, Failure> {
    let (url, query) = match url.split_once('?') {
        Some((url, query)) => (url, format!("?{query}")),
        None => (url, String::new()),
    };
    let parts = super::providers::base_url(url)?;
    let route = Route {
        provider: what.to_owned(),
        tls: parts.tls,
        host: parts.host,
        port: parts.port,
        authority: parts.authority,
        path: parts.path,
        headers: Vec::new(),
        login: None,
    };
    let exchange = async {
        let (mut sender, driver) = connect(&route).await?;
        let path = match route.path.as_str() {
            "" => "/",
            path => path,
        };
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{path}{query}"))
            .header("Host", &route.authority);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let (kind, body) = body.unwrap_or_default();
        if !kind.is_empty() {
            request = request.header("Content-Type", kind);
        }
        if method != "GET" {
            request = request.header("Content-Length", body.len());
        }
        let request = (request.body(Full::new(Bytes::from(body)))).map_err(|e| e.to_string())?;
        let response = (sender.send_request(request).await).map_err(|e| {
            Failure::new(format!("request to {} failed: {e}", route.authority), true)
        })?;
        let status = response.status().as_u16();
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| {
                Failure::new(format!("{}'s answer failed: {e}", route.authority), true)
            })?;
            if let Ok(data) = frame.into_data() {
                bytes.extend_from_slice(&data);
            }
            if bytes.len() > FETCHED_BYTES {
                return Err(format!("{}'s answer is over 4 MB", route.authority).into());
            }
        }
        driver.abort();
        Ok(Fetched {
            status,
            body: bytes,
        })
    };
    match tokio::time::timeout(FETCH_TIMEOUT, exchange).await {
        Ok(fetched) => fetched,
        Err(_) => Err(Failure::new(
            format!(
                "{} gave no answer within {}s",
                route.authority,
                FETCH_TIMEOUT.as_secs()
            ),
            true,
        )),
    }
}

pub(super) type Connection = (
    http1::SendRequest<Full<Bytes>>,
    JoinHandle<hyper::Result<()>>,
);

/// An HTTP/1.1 connection to `route`'s host, in TLS for an `https://` base URL, with the task
/// that drives it.
async fn connect(route: &Route) -> Result<Connection, Failure> {
    connect_for(route, &format!(" (provider {})", route.provider)).await
}

/// As `connect`, its failures naming the host followed by `who`.
pub(super) async fn connect_for(route: &Route, who: &str) -> Result<Connection, Failure> {
    let authority = &route.authority;
    let socket = (TcpStream::connect((route.host.as_str(), route.port)).await)
        .map_err(|e| Failure::new(format!("cannot connect to {authority}{who}: {e}"), true))?;
    if !route.tls {
        return handshake(socket).await;
    }
    let config = tls_config().map_err(|problem| Failure::new(problem, false))?;
    let name = rustls::pki_types::ServerName::try_from(route.host.clone()).map_err(|_| {
        Failure::new(
            format!("{} is not a name TLS can verify", route.host),
            false,
        )
    })?;
    let stream = (tokio_rustls::TlsConnector::from(config)
        .connect(name, socket)
        .await)
        .map_err(|e| tls_failure(route, who, &e))?;
    handshake(stream).await
}

async fn handshake(
    io: impl AsyncRead + AsyncWrite + Unpin + Send + 'static,
) -> Result<Connection, Failure> {
    let (sender, driver) = http1::handshake(TokioIo::new(io))
        .await
        .map_err(|e| Failure::new(format!("HTTP handshake failed: {e}"), true))?;
    Ok((sender, tokio::spawn(driver)))
}

/// TLS's settings, with the system's trusted certificates (or `SSL_CERT_FILE`'s and
/// `SSL_CERT_DIR`'s), read at the first `https://` request and kept once read.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    if let Some(config) = CONFIG.get() {
        return Ok(config.clone());
    }
    let found = rustls_native_certs::load_native_certs();
    let mut roots = rustls::RootCertStore::empty();
    let (added, _) = roots.add_parsable_certificates(found.certs);
    if added == 0 {
        let why = (found.errors.first()).map_or(String::new(), |error| format!(": {error}"));
        return Err(format!(
            "no trusted certificates found for HTTPS in the system's store (or SSL_CERT_FILE, \
             SSL_CERT_DIR){why}"
        ));
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(CONFIG.get_or_init(|| Arc::new(config)).clone())
}

/// A failed TLS handshake: TLS's own errors (an untrusted or invalid certificate, an alert)
/// would fail again, unlike a connection that broke.
fn tls_failure(route: &Route, who: &str, error: &std::io::Error) -> Failure {
    let authority = &route.authority;
    let untrusted = (error.get_ref())
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        .is_some_and(|error| {
            *error == rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)
        });
    let hint = match untrusted {
        true => {
            "; HTTPS trusts the system's certificates, or those SSL_CERT_FILE and SSL_CERT_DIR name"
        }
        false => "",
    };
    Failure::new(
        format!("TLS handshake with {authority}{who} failed: {error}{hint}"),
        error.kind() != std::io::ErrorKind::InvalidData,
    )
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
                    ..Reply::default()
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
