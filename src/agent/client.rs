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
    /// Nothing listened at the provider's address.
    pub refused: bool,
    /// The model or the provider's filter declined to answer.
    pub refusal: bool,
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
            refused: false,
            refusal: false,
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
                | "content_policy_violation"
                | "refusal",
            ) => false,
            _ => !mentions(&message, PERMANENT_WORDS) && mentions(&message, TRANSIENT_WORDS),
        };
        Self {
            overflow: code == Some("context_length_exceeded") || overflows(&message),
            refusal: matches!(
                code,
                Some("content_filter" | "content_policy_violation" | "refusal")
            ),
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
    /// The route's headers, its commands' run for this request.
    pub given: &'a [(String, String)],
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
        given,
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
    for (name, value) in given {
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
        let sent = (given
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())))
        .chain(headers.iter().map(|(name, value)| (*name, value.as_str())));
        // Redacted before the cut, so a secret across it leaves no prefix.
        let mut text = super::redact::redact(
            &String::from_utf8_lossy(&text),
            &super::redact::secrets(sent),
        );
        text.truncate(text.floor_char_boundary(ERROR_BODY_BYTES));
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
                refused: false,
                refusal: false,
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
    pub location: Option<String>,
    /// What of the request's headers may be credentials, for `said`.
    pub secrets: Vec<String>,
}

impl Fetched {
    /// The body as text, without the request's credentials, for a failure to show.
    pub fn said(&self) -> String {
        super::redact::redact(&String::from_utf8_lossy(&self.body), &self.secrets)
    }
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
    fetch_within(
        what,
        method,
        url,
        headers,
        body,
        FETCHED_BYTES,
        FETCH_TIMEOUT,
    )
    .await
}

/// As `fetch`, with an answer of at most `limit` bytes within `timeout`.
pub(super) async fn fetch_within(
    what: &str,
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<(&str, Vec<u8>)>,
    limit: usize,
    timeout: Duration,
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
        commanded: Vec::new(),
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
        let mut secrets = super::redact::secrets(headers.iter().copied());
        if kind == "application/x-www-form-urlencoded" {
            secrets.extend(super::redact::form_secrets(&String::from_utf8_lossy(&body)));
            secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        }
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
        let location = (response.headers().get(hyper::header::LOCATION))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| {
                Failure::new(format!("{}'s answer failed: {e}", route.authority), true)
            })?;
            if let Ok(data) = frame.into_data() {
                bytes.extend_from_slice(&data);
            }
            if bytes.len() > limit {
                let megabytes = limit >> 20;
                return Err(format!("{}'s answer is over {megabytes} MB", route.authority).into());
            }
        }
        driver.abort();
        Ok(Fetched {
            status,
            body: bytes,
            location,
            secrets,
        })
    };
    match tokio::time::timeout(timeout, exchange).await {
        Ok(fetched) => fetched,
        Err(_) => Err(Failure::new(
            format!(
                "{} gave no answer within {}s",
                route.authority,
                timeout.as_secs()
            ),
            true,
        )),
    }
}

const REDIRECTS: usize = 5;

/// The body `url` answers a GET with, `what` naming it in failures: at most `limit` bytes, each
/// request within `timeout`, following at most 5 redirects and none from HTTPS to HTTP. An
/// answer other than 2xx fails with its status and the start of its body, if not a web page.
pub fn download(
    what: &str,
    url: &str,
    headers: &[(&str, &str)],
    limit: usize,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut url = url.to_owned();
    for _ in 0..=REDIRECTS {
        let fetching = fetch_within(what, "GET", &url, headers, None, limit, timeout);
        // On a thread of its own, as a runtime may be running where this is called.
        let fetched = std::thread::scope(|scope| {
            let fetched = scope.spawn(|| super::login::blocking(fetching));
            (fetched.join()).unwrap_or_else(|_| Err(Failure::new(format!("{what} failed"), false)))
        });
        let fetched = fetched.map_err(|failure| failure.message)?;
        match fetched.status {
            200..=299 => return Ok(fetched.body),
            301 | 302 | 303 | 307 | 308 => url = redirected(what, &url, fetched.location)?,
            status => {
                let said = fetched.said();
                let said: String = match said.trim() {
                    page if page.is_empty() || page.starts_with('<') => String::new(),
                    said => format!(": {}", said.chars().take(400).collect::<String>()),
                };
                return Err(format!("{what} answered HTTP {status}{said}"));
            }
        }
    }
    Err(format!("{what} redirected more than {REDIRECTS} times"))
}

/// Where a redirect from `from` to `location` leads: an absolute URL, or a path on the same
/// host; never from HTTPS to plain HTTP.
fn redirected(what: &str, from: &str, location: Option<String>) -> Result<String, String> {
    let location = location.ok_or_else(|| format!("{what} redirected nowhere"))?;
    let refused = || {
        Err(format!(
            "{what} redirected to {location}, which is not followed"
        ))
    };
    if location.starts_with("https://") {
        return Ok(location);
    }
    if location.starts_with("http://") {
        return match from.starts_with("http://") {
            true => Ok(location),
            false => refused(),
        };
    }
    if location.starts_with('/') && !location.starts_with("//") {
        let scheme = from.find("://").map_or(0, |at| at + 3);
        let origin = from[scheme..]
            .find('/')
            .map_or(from, |at| &from[..scheme + at]);
        return Ok(format!("{origin}{location}"));
    }
    refused()
}

pub(super) type Connection = (
    http1::SendRequest<Full<Bytes>>,
    JoinHandle<hyper::Result<()>>,
);

/// An HTTP/1.1 connection to `route`'s host, in TLS for an `https://` base URL, through the
/// proxy the environment names for it if any (`proxy`), with the task that drives it.
async fn connect(route: &Route) -> Result<Connection, Failure> {
    connect_for(route, &format!(" (provider {})", route.provider)).await
}

/// As `connect`, its failures naming the host followed by `who`.
pub(super) async fn connect_for(route: &Route, who: &str) -> Result<Connection, Failure> {
    let authority = &route.authority;
    let proxy = super::proxy::for_host(route.tls, &route.host, route.port)
        .map_err(|problem| Failure::new(problem, false))?;
    let socket = match &proxy {
        None => (tcp_connect(&route.host, route.port).await).map_err(|e| Failure {
            refused: e.kind() == std::io::ErrorKind::ConnectionRefused,
            ..Failure::new(format!("cannot connect to {authority}{who}: {e}"), true)
        })?,
        Some(proxy) => {
            let whom = format!("{authority}{who}");
            (super::proxy::tunnel(proxy, &route.host, route.port, &whom).await)
                .map_err(|(problem, transient)| Failure::new(problem, transient))?
        }
    };
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

/// A connection to the first of `host`'s addresses to answer, the families alternating and each
/// attempt started `STAGGER` after the last unless it failed sooner (RFC 8305, as Node's
/// `autoSelectFamily`): Windows resolves `localhost` to `::1` first, where a runtime listening on
/// `127.0.0.1` leaves the attempt hanging under WSL's mirrored networking.
pub(super) async fn tcp_connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    const STAGGER: Duration = Duration::from_millis(250);
    let resolved: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((host, port)).await?.collect();
    connect_first(&resolved, STAGGER).await.map_err(|error| {
        error.unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{host} has no address"),
            )
        })
    })
}

/// The first of `resolved` to accept, as `tcp_connect` races them; `None` when it is empty.
async fn connect_first(
    resolved: &[std::net::SocketAddr],
    stagger: Duration,
) -> Result<TcpStream, Option<std::io::Error>> {
    let (first, second): (Vec<_>, Vec<_>) = (resolved.iter())
        .partition(|address| Some(address.is_ipv6()) == resolved.first().map(|a| a.is_ipv6()));
    let mut first = first.into_iter();
    let mut second = second.into_iter();
    let mut pending = std::iter::from_fn(|| {
        let next = first.next().or_else(|| second.next());
        std::mem::swap(&mut first, &mut second);
        next
    })
    .copied()
    .peekable();
    let mut attempts = tokio::task::JoinSet::new();
    let mut last = None;
    loop {
        match pending.next() {
            Some(address) => drop(attempts.spawn(TcpStream::connect(address))),
            None if attempts.is_empty() => return Err(last),
            None => {}
        }
        let more = pending.peek().is_some();
        tokio::select! {
            Some(joined) = attempts.join_next() => match joined {
                Ok(Ok(stream)) => return Ok(stream),
                Ok(Err(error)) => last = Some(error),
                Err(error) => last = Some(std::io::Error::other(error)),
            },
            () = tokio::time::sleep(stagger), if more => {}
        }
    }
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
                    parsed
                        .error
                        .as_deref()
                        .map(|e| format!(": {e}"))
                        .unwrap_or_default()
                );
                // An incomplete response (output limit, content filter) would end the same way again.
                Err(match parsed.status.as_str() {
                    "failed" => Failure::provider(parsed.error_code.as_deref(), message),
                    "incomplete" if parsed.error.as_deref() == Some("content_filter") => {
                        Failure::provider(Some("content_filter"), message)
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn redirects_lead_anywhere_but_from_https_to_http() {
        let to = |from: &str, location: &str| redirected("it", from, Some(location.to_owned()));
        let https = "https://github.com/o/r/releases/download/v1/a.tar.gz";
        let asset = "https://objects.example/a?x=1";
        assert_eq!(to(https, asset).unwrap(), asset);
        assert_eq!(to(https, "/o/r/a").unwrap(), "https://github.com/o/r/a");
        assert_eq!(
            to("http://127.0.0.1:9/x", "/y").unwrap(),
            "http://127.0.0.1:9/y"
        );
        assert_eq!(
            to("http://127.0.0.1:9/x", "http://localhost/y").unwrap(),
            "http://localhost/y"
        );
        for location in ["http://objects.example/a", "//objects.example/a", "a"] {
            assert_eq!(
                to(https, location).unwrap_err(),
                format!("it redirected to {location}, which is not followed")
            );
        }
        assert_eq!(
            redirected("it", https, None).unwrap_err(),
            "it redirected nowhere"
        );
    }

    /// Answers each connection's request with `answer(path)`'s status, extra header and body.
    fn serve(answer: fn(&str) -> (u16, String, &'static str)) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
                let request = lines.next().unwrap().unwrap();
                while lines.next().is_some_and(|line| !line.unwrap().is_empty()) {}
                let path = request.split(' ').nth(1).unwrap().to_owned();
                let (status, header, body) = answer(&path);
                let reply = format!(
                    "HTTP/1.1 {status} X\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        format!("http://{address}")
    }

    #[test]
    fn a_download_follows_redirects_and_says_what_failed() {
        let base = serve(|path| match path {
            "/start" => (302, "Location: /next\r\n".to_owned(), ""),
            "/next" => (307, "Location: /body?signed=1\r\n".to_owned(), ""),
            "/body?signed=1" => (200, String::new(), "the bytes"),
            "/loop" => (301, "Location: /loop\r\n".to_owned(), ""),
            _ => (404, String::new(), "{\"message\":\"Not Found\"}"),
        });
        let get = |path: &str| {
            let url = format!("{base}{path}");
            download("the file", &url, &[], 1 << 20, Duration::from_secs(5))
        };
        assert_eq!(get("/start").unwrap(), b"the bytes");
        assert_eq!(
            get("/gone").unwrap_err(),
            "the file answered HTTP 404: {\"message\":\"Not Found\"}"
        );
        assert_eq!(
            get("/loop").unwrap_err(),
            "the file redirected more than 5 times"
        );
    }

    #[test]
    fn a_connection_races_past_an_address_that_does_not_answer() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let open = listener.local_addr().unwrap();
            // TEST-NET-1: never answers, or fails at once where there is no route.
            let silent: std::net::SocketAddr = "192.0.2.1:9".parse().unwrap();
            let started = Instant::now();
            let stream = connect_first(&[silent, open], Duration::from_millis(250)).await;
            assert_eq!(stream.unwrap().peer_addr().unwrap(), open);
            assert!(started.elapsed() < Duration::from_secs(2));
            assert!(
                connect_first(&[], Duration::ZERO)
                    .await
                    .unwrap_err()
                    .is_none()
            );
        });
    }
}
