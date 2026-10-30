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
    time::Duration,
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
/// Like Scramjet's provider timeout: reset by every received chunk, so long generations continue.
const INACTIVITY: Duration = Duration::from_secs(600);

pub(super) struct Reply {
    pub items: Vec<Value>,
    pub usage: Option<crate::ProviderUsage>,
}

#[derive(Debug)]
pub enum ClientError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

async fn cancelled(flag: &AtomicBool) {
    while !flag.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(50)).await;
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
) -> Result<Reply, String> {
    let socket = TcpStream::connect(endpoint)
        .await
        .map_err(|e| format!("cannot connect to {endpoint}: {e} (is Proxy running?)"))?;
    let (mut sender, driver) = http1::handshake(TokioIo::new(socket))
        .await
        .map_err(|e| format!("HTTP handshake failed: {e}"))?;
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
        .map_err(|e| e.to_string())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
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
        Err(format!(
            "HTTP {status}: {}",
            String::from_utf8_lossy(&text).trim()
        ))
    } else {
        let mut parser = ResponsesParser::with_limits(LIMITS);
        parser.progress = Some(Vec::new());
        let mut received = 0usize;
        let mut failure = None;
        loop {
            let frame = match tokio::time::timeout(INACTIVITY, body.frame()).await {
                Err(_) => {
                    failure = Some(format!("no data for {}s", INACTIVITY.as_secs()));
                    break;
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    failure = Some(format!("stream failed: {e}"));
                    break;
                }
                Ok(Some(Ok(frame))) => frame,
            };
            let Ok(data) = frame.into_data() else {
                continue;
            };
            received += data.len();
            if received > WIRE_BYTES {
                failure = Some("response exceeded the client size limit".into());
                break;
            }
            let error = data.iter().find_map(|b| parser.byte(*b).err());
            for update in parser.progress.as_mut().unwrap().drain(..) {
                progress(update);
            }
            if let Some(outcome) = error {
                failure = Some(describe(&parser, outcome));
                break;
            }
        }
        match failure {
            Some(failure) => Err(failure),
            None => match parser.complete() {
                Err(outcome) => Err(format!("stream ended early ({outcome:?})")),
                Ok(parsed) => match parsed.envelope {
                    Some(envelope) => Ok(Reply {
                        items: envelope
                            .items
                            .into_iter()
                            .map(|i| i.raw().clone())
                            .collect(),
                        usage: parsed.usage,
                    }),
                    None => Err(format!(
                        "response {}{}",
                        parsed.status,
                        parsed.error.map(|e| format!(": {e}")).unwrap_or_default()
                    )),
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
