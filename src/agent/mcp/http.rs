//! MCP's streamable HTTP transport, as rmcp's `StreamableHttpClientTransport` runs it, over the
//! harness's own HTTP and TLS client (`client::connect_for`): one connection per request, as
//! rmcp's own clients make for this transport.
use super::super::{client, providers};
use bytes::Bytes;
use futures_util::{StreamExt, stream::BoxStream};
use http_body_util::{BodyExt, Full};
use hyper::{
    Method, Request, Response, StatusCode,
    body::Incoming,
    header::{self, HeaderName, HeaderValue},
};
use rmcp::{
    model::{ClientJsonRpcMessage, ClientRequest, ErrorData, JsonRpcMessage, ServerJsonRpcMessage},
    transport::streamable_http_client::{
        SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    },
};
use sse_stream::{Sse, SseStream};
use std::{borrow::Cow, collections::HashMap, sync::Arc};

const SESSION: &str = "mcp-session-id";
const LAST_EVENT: &str = "last-event-id";
const EVENTS: &str = "text/event-stream";
const JSON: &str = "application/json";
/// What is read of an answer in JSON, and of a failure's, as rmcp's own clients read them.
const JSON_BYTES: usize = 16 << 20;
const FAILURE_BYTES: usize = 64 << 10;
/// What a failure's message shows of its body.
const SHOWN_CHARS: usize = 300;
/// Redirects followed for one request, as rmcp's own client follows.
const REDIRECTS: usize = 10;

/// The harness's HTTP client for MCP servers.
#[derive(Clone)]
pub(super) struct Https;

/// Why a request failed, in words.
#[derive(Debug)]
pub(super) struct Failed(String);

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failed {}

type Error = StreamableHttpError<Failed>;

fn failed(message: impl Into<String>) -> Error {
    StreamableHttpError::Client(Failed(message.into()))
}

/// The route to `url`'s host and the request target: its path and query as written.
pub(super) fn target(url: &str) -> Result<(providers::Route, String), String> {
    let url = url.split('#').next().unwrap_or(url);
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let parts = providers::base_url(base)?;
    let scheme = if parts.tls { "https://" } else { "http://" }.len();
    let path = match &base[scheme + parts.authority.len()..] {
        "" => "/",
        path => path,
    };
    let target = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };
    let route = providers::Route {
        provider: String::new(),
        tls: parts.tls,
        host: parts.host,
        port: parts.port,
        authority: parts.authority,
        path: String::new(),
        headers: Vec::new(),
        login: None,
    };
    Ok((route, target))
}

/// Sends a request to `url` with `headers`, the server's and the transport's, then `own`. A 307
/// or 308 to the same scheme, host and port is followed, as rmcp's own client follows them: they
/// keep the method and body. Other redirects are answers: the headers and session must not reach
/// another host.
async fn send(
    method: Method,
    url: &str,
    headers: HashMap<HeaderName, HeaderValue>,
    own: Vec<(HeaderName, String)>,
    body: Vec<u8>,
) -> Result<Response<Incoming>, Error> {
    let (route, mut target) = target(url).map_err(failed)?;
    let body = Bytes::from(body);
    for _ in 0..=REDIRECTS {
        let response = send_to(&route, &target, &method, &headers, &own, body.clone()).await?;
        if !matches!(
            response.status(),
            StatusCode::TEMPORARY_REDIRECT | StatusCode::PERMANENT_REDIRECT
        ) {
            return Ok(response);
        }
        match (response.headers().get(header::LOCATION)).and_then(|to| same_origin(&route, to)) {
            Some(next) => target = next,
            None => return Ok(response),
        }
    }
    Err(failed(format!(
        "{} redirected it more than {REDIRECTS} times",
        route.authority
    )))
}

async fn send_to(
    route: &providers::Route,
    target: &str,
    method: &Method,
    headers: &HashMap<HeaderName, HeaderValue>,
    own: &[(HeaderName, String)],
    body: Bytes,
) -> Result<Response<Incoming>, Error> {
    // The connection's task ends with it, once the answer was read or dropped.
    let (mut sender, _driver) =
        (client::connect_for(route, "").await).map_err(|failure| failed(failure.message))?;
    let mut request = Request::builder()
        .method(method)
        .uri(target)
        .header(header::HOST, &route.authority);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    for (name, value) in own {
        request = request.header(name, value);
    }
    let request = (request.body(Full::new(body))).map_err(|e| failed(e.to_string()))?;
    (sender.send_request(request).await)
        .map_err(|e| failed(format!("request to {} failed: {e}", route.authority)))
}

/// The request target a redirect's `location` names, when it is a path or a URL on `route`'s
/// scheme, host and port.
fn same_origin(route: &providers::Route, location: &HeaderValue) -> Option<String> {
    let location = location.to_str().ok()?;
    if location.starts_with('/') && !location.starts_with("//") {
        let path = location.split('#').next().unwrap_or(location);
        return Some(path.to_owned());
    }
    let (to, target) = target(location).ok()?;
    let same =
        to.tls == route.tls && to.port == route.port && to.host.eq_ignore_ascii_case(&route.host);
    same.then_some(target)
}

/// Where a refused redirect points, after ` to `, without its query.
fn redirected_to(response: &Response<Incoming>) -> String {
    if !response.status().is_redirection() {
        return String::new();
    }
    match (response.headers().get(header::LOCATION)).and_then(|to| to.to_str().ok()) {
        Some(to) => match to.split_once('?') {
            Some((to, _)) => format!(" to {to}?…"),
            None => format!(" to {to}"),
        },
        None => String::new(),
    }
}

/// The headers a request carries besides the server's: its session's, its token and `more`.
fn own(
    session: Option<&str>,
    token: Option<String>,
    more: &[(HeaderName, &str)],
) -> Vec<(HeaderName, String)> {
    let mut own: Vec<(HeaderName, String)> = (more.iter())
        .map(|(name, value)| (name.clone(), (*value).to_owned()))
        .collect();
    if let Some(session) = session {
        own.push((HeaderName::from_static(SESSION), session.to_owned()));
    }
    if let Some(token) = token {
        own.push((header::AUTHORIZATION, format!("Bearer {token}")));
    }
    own
}

fn length(response: &Response<Incoming>) -> Option<u64> {
    let value = response.headers().get(header::CONTENT_LENGTH)?;
    value.to_str().ok()?.parse().ok()
}

fn has_type(response: &Response<Incoming>, kind: &str) -> bool {
    (response.headers().get(header::CONTENT_TYPE))
        .is_some_and(|value| value.as_bytes().starts_with(kind.as_bytes()))
}

/// The body, refused once it is over `max` bytes.
async fn read(response: Response<Incoming>, max: usize) -> Result<Vec<u8>, Error> {
    if length(&response).is_some_and(|length| length > max as u64) {
        return Err(StreamableHttpError::ResponseBodyTooLarge { max_size: max });
    }
    let mut body = response.into_body();
    let mut bytes = Vec::new();
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|e| failed(format!("its answer failed: {e}")))?;
        if let Ok(data) = frame.into_data() {
            if data.len() > max - bytes.len() {
                return Err(StreamableHttpError::ResponseBodyTooLarge { max_size: max });
            }
            bytes.extend_from_slice(&data);
        }
    }
    Ok(bytes)
}

/// The start of a failure's body, on one line.
fn shown(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

/// A refused request's failure: one asking for authorization says how to give it.
fn unauthorized(status: StatusCode, body: &[u8]) -> Error {
    let said = match shown(body) {
        said if said.is_empty() => String::new(),
        said => format!(" ({said})"),
    };
    failed(format!(
        "HTTP {status}{said}: it needs authorization, which scramjet-rs gives it only as the \
headers of its entry in mcpServers (OAuth sign-in is not supported yet)"
    ))
}

/// A server of the revisions before 2026-07-28 refusing `server/discover` outside a session, as
/// an answer rmcp's lifecycle takes to fall back to `initialize`, as its own clients give it.
fn discover_refused(
    message: &ClientJsonRpcMessage,
    session: bool,
    status: StatusCode,
    body: &[u8],
) -> Option<StreamableHttpPostResponse> {
    let ClientJsonRpcMessage::Request(request) = message else {
        return None;
    };
    if session
        || !status.is_client_error()
        || !matches!(request.request, ClientRequest::DiscoverRequest(_))
    {
        return None;
    }
    let error = match serde_json::from_slice::<ServerJsonRpcMessage>(body) {
        Ok(ServerJsonRpcMessage::Error(error)) => error.error,
        _ => ErrorData::invalid_request(
            format!(
                "server/discover refused with HTTP {status}: {}",
                shown(body)
            ),
            None,
        ),
    };
    Some(StreamableHttpPostResponse::Json(
        ServerJsonRpcMessage::error(error, Some(request.id.clone())),
        None,
    ))
}

/// The server-sent events of `response`, each refused once it is over `max` bytes.
fn events(response: Response<Incoming>, max: usize) -> BoxStream<'static, Result<Sse, SseError>> {
    let bytes = (response.into_body().into_data_stream()).scan(Limit::new(max), |limit, chunk| {
        let item = match (limit.failed, chunk) {
            (true, _) => None,
            (false, Ok(chunk)) => Some(limit.observe(&chunk).map(|()| chunk)),
            (false, Err(error)) => Some(Err(Failed(format!("the stream failed: {error}")))),
        };
        limit.failed |= matches!(item, Some(Err(_)));
        std::future::ready(item)
    });
    SseStream::from_bytes_stream(bytes).boxed()
}

/// Counts an event's bytes, from its first line to the empty line that ends it.
struct Limit {
    max: usize,
    event: usize,
    line: usize,
    after_cr: bool,
    failed: bool,
}

impl Limit {
    fn new(max: usize) -> Self {
        Self {
            max,
            event: 0,
            line: 0,
            after_cr: false,
            failed: false,
        }
    }

    fn observe(&mut self, chunk: &[u8]) -> Result<(), Failed> {
        for &byte in chunk {
            let after_cr = std::mem::replace(&mut self.after_cr, byte == b'\r');
            match byte {
                b'\n' if after_cr => {}
                b'\r' | b'\n' if self.line == 0 => self.event = 0,
                b'\r' | b'\n' => self.event += std::mem::take(&mut self.line) + 1,
                _ => self.line += 1,
            }
            if self.event + self.line > self.max {
                return Err(Failed(format!(
                    "an event of its stream is over {} MB",
                    self.max >> 20
                )));
            }
        }
        Ok(())
    }
}

fn expects_no_answer(message: &ClientJsonRpcMessage) -> bool {
    matches!(
        message,
        JsonRpcMessage::Notification(_) | JsonRpcMessage::Response(_) | JsonRpcMessage::Error(_)
    )
}

impl StreamableHttpClient for Https {
    type Error = Failed;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        self.post_message_with_max_sse_event_size(uri, message, session, token, headers, JSON_BYTES)
            .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session: Option<Arc<str>>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
        max_event: usize,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let body = serde_json::to_vec(&message).map_err(|e| failed(e.to_string()))?;
        let own = own(
            session.as_deref(),
            token,
            &[
                (header::CONTENT_TYPE, JSON),
                (header::ACCEPT, "application/json, text/event-stream"),
            ],
        );
        let response = send(Method::POST, &uri, headers, own, body).await?;
        let status = response.status();
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        let new_session = (response.headers().get(SESSION))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if !status.is_success() {
            let json = has_type(&response, JSON);
            let to = redirected_to(&response);
            let body = read(response, FAILURE_BYTES).await.unwrap_or_default();
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
                return Err(unauthorized(status, &body));
            }
            if let Some(answer) = discover_refused(&message, session.is_some(), status, &body) {
                return Ok(answer);
            }
            if json
                && let Ok(error @ JsonRpcMessage::Error(_)) =
                    serde_json::from_slice::<ServerJsonRpcMessage>(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(error, new_session));
            }
            let said = match shown(&body) {
                said if said.is_empty() => String::new(),
                said => format!(": {said}"),
            };
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}{to}{said}"),
            )));
        }
        if length(&response) == Some(0) && expects_no_answer(&message) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if has_type(&response, EVENTS) {
            return Ok(StreamableHttpPostResponse::Sse(
                events(response, max_event),
                new_session,
            ));
        }
        if !has_type(&response, JSON) {
            let kind = (response.headers().get(header::CONTENT_TYPE))
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
            return Err(StreamableHttpError::UnexpectedContentType(kind));
        }
        let body = read(response, JSON_BYTES).await?;
        match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
            Ok(answer) => Ok(StreamableHttpPostResponse::Json(answer, new_session)),
            // A notification or reply needs no answer: what came is not one.
            Err(_) if expects_no_answer(&message) => Ok(StreamableHttpPostResponse::Accepted),
            Err(error) => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("its answer is not JSON-RPC ({error}): {}", shown(&body)),
            ))),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session: Arc<str>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Error> {
        let own = own(Some(&session), token, &[]);
        let response = send(Method::DELETE, &uri, headers, own, Vec::new()).await?;
        match response.status() {
            StatusCode::METHOD_NOT_ALLOWED => {
                Err(StreamableHttpError::ServerDoesNotSupportDeleteSession)
            }
            status if status.is_success() => Ok(()),
            status => Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("ending its session: HTTP {status}"),
            ))),
        }
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last_event: Option<String>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        self.get_stream_with_max_sse_event_size(
            uri, session, last_event, token, headers, JSON_BYTES,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session: Option<Arc<str>>,
        last_event: Option<String>,
        token: Option<String>,
        headers: HashMap<HeaderName, HeaderValue>,
        max_event: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let mut more = vec![(header::ACCEPT, "text/event-stream, application/json")];
        if let Some(last_event) = &last_event {
            more.push((HeaderName::from_static(LAST_EVENT), last_event));
        }
        let own = own(session.as_deref(), token, &more);
        let response = send(Method::GET, &uri, headers, own, Vec::new()).await?;
        let status = response.status();
        if status == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        // So that the transport stops asking, as it does on these.
        if status == StatusCode::UNAUTHORIZED {
            let challenge = (response.headers().get(header::WWW_AUTHENTICATE))
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
            return Err(StreamableHttpError::AuthRequired(
                rmcp::transport::streamable_http_client::AuthRequiredError::new(
                    challenge.unwrap_or_default(),
                ),
            ));
        }
        if !status.is_success() {
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("its stream: HTTP {status}"),
            )));
        }
        if !has_type(&response, EVENTS) && !has_type(&response, JSON) {
            let kind = (response.headers().get(header::CONTENT_TYPE))
                .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
            return Err(StreamableHttpError::UnexpectedContentType(kind));
        }
        Ok(events(response, max_event))
    }
}
