use super::{LimitedSocket, Meter};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, client::conn::http1};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tokio::net::TcpStream;

fn invalid() -> io::Error {
    io::Error::other("Proxy route readiness or compatibility check failed")
}
async fn get(
    endpoint: SocketAddr,
    path: &'static str,
    stopped: Option<&AtomicBool>,
) -> io::Result<Value> {
    let meter = Arc::new(Meter {
        wire: AtomicUsize::new(0),
        limited: AtomicBool::new(false),
        malformed: AtomicBool::new(false),
        invalid_usage: AtomicBool::new(false),
    });
    let mut connection = None;
    let work = async {
        let socket = TcpStream::connect(endpoint).await?;
        let io = TokioIo::new(LimitedSocket {
            socket,
            meter,
            head_bytes: 0,
            max_headers: super::PROVIDER_HEADERS,
            head_lines: 0,
            head_line_empty: true,
            head_cr: false,
            in_head: true,
            line: [0; 128],
            line_len: 0,
        });
        let (mut sender, driver) = http1::Builder::new()
            .max_headers(super::PROVIDER_HEADERS)
            .max_header_size(super::RESPONSE_HEAD_BYTES)
            .max_buf_size(8192)
            .handshake(io)
            .await
            .map_err(|_| invalid())?;
        connection = Some(tokio::spawn(driver));
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .header("Host", endpoint.to_string())
            .header("Connection", "close")
            .body(Full::new(Bytes::new()))
            .expect("fixed metadata request");
        let response = sender.send_request(request).await.map_err(|_| invalid())?;
        if response.status() != 200 {
            return Err(invalid());
        }
        let mut body = response.into_body();
        let mut bytes = Vec::with_capacity(crate::runtime::MAX_WIRE);
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| invalid())?;
            if frame.trailers_ref().is_some() {
                return Err(invalid());
            }
            if let Ok(data) = frame.into_data() {
                if data.len() > crate::runtime::MAX_WIRE - bytes.len() {
                    return Err(invalid());
                }
                bytes.extend_from_slice(&data);
            }
        }
        super::responses::metadata_json(&bytes).map_err(|_| invalid())
    };
    let result = tokio::select! {
        result=tokio::time::timeout(std::time::Duration::from_secs(5),work)=>result.map_err(|_|invalid()).and_then(|r|r),
        ()=async {
            if let Some(stopped)=stopped {
                while !stopped.load(std::sync::atomic::Ordering::SeqCst){tokio::time::sleep(std::time::Duration::from_millis(10)).await;}
            } else {std::future::pending::<()>().await;}
        }=>Err(invalid()),
    };
    if let Some(driver) = connection {
        driver.abort();
        let _ = driver.await;
    }
    result
}
pub(crate) fn compatible(status: &Value, models: &Value, cards: &Value) -> bool {
    status["startup_ready"] == true
        && status["transition_state"] == "ready"
        && status["version"] == "0.15.0-beta.1"
        && status["model_catalog"]["source"] == "agentic"
        && status["model_catalog"]["stale"] == false
        && cards["revision"] == "2026-10-26-001"
        && models["data"]
            .as_array()
            .is_some_and(|entries| entries.iter().filter(|e| e["id"] == "gpt-6-luna").count() == 1)
        && cards["models"].as_array().is_some_and(|entries| {
            let matching: Vec<_> = entries
                .iter()
                .filter(|e| e["requestId"] == "gpt-6-luna")
                .collect();
            matching.len() == 1
                && matching[0]["provider"] == "openai"
                && matching[0]["primaryProtocol"] == "openai-responses"
                && matching[0]["supportedProtocols"]
                    .as_array()
                    .is_some_and(|p| p.iter().any(|v| v == "openai-responses"))
                && matching[0]["limits"]["context"] == 1050000
                && matching[0]["limits"]["output"] == 128000
        })
}
#[cfg(test)]
#[path = "provider_metadata_tests.rs"]
mod provider_metadata_tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataEvidence {
    pub status: Value,
    pub models: Value,
    pub cards: Value,
    pub observed_unix_ms: [u64; 3],
}
impl MetadataEvidence {
    pub(crate) fn predicates(&self) -> Value {
        let card = self.cards["models"]
            .as_array()
            .and_then(|a| a.iter().find(|v| v["requestId"] == "gpt-6-luna"));
        json!({"startup_ready":self.status["startup_ready"],"transition_state":self.status["transition_state"],
            "version":self.status["version"],"catalog":self.status["model_catalog"],"revision":self.cards["revision"],
            "model_count":self.models["data"].as_array().map(|a|a.iter().filter(|v|v["id"]=="gpt-6-luna").count()),
            "card":card.map(|v|json!({"requestId":v["requestId"],"provider":v["provider"],"primaryProtocol":v["primaryProtocol"],
                "supportedProtocols":v["supportedProtocols"],"limits":v["limits"]}))})
    }
}
fn observed() -> io::Result<u64> {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| invalid())?
            .as_millis(),
    )
    .map_err(|_| invalid())
}
pub(crate) async fn observe(endpoint: SocketAddr) -> io::Result<MetadataEvidence> {
    observe_controlled(endpoint, None).await
}
pub(crate) async fn observe_controlled(
    endpoint: SocketAddr,
    stopped: Option<&AtomicBool>,
) -> io::Result<MetadataEvidence> {
    let status = get(endpoint, "/api/status", stopped).await?;
    let first = observed()?;
    let models = get(endpoint, "/v1/models", stopped).await?;
    let second = observed()?;
    let cards = get(endpoint, "/api/model-cards", stopped).await?;
    let third = observed()?;
    if !compatible(&status, &models, &cards) {
        return Err(invalid());
    }
    Ok(MetadataEvidence {
        status,
        models,
        cards,
        observed_unix_ms: [first, second, third],
    })
}
pub(crate) async fn check() -> io::Result<SocketAddr> {
    let endpoint = "127.0.0.1:18080"
        .parse()
        .expect("fixed numeric Proxy endpoint");
    observe(endpoint).await?;
    Ok(endpoint)
}
