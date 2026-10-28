use super::{ProjectRequest, ProjectSnapshot, RequestId, Runtime};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Copy)]
pub enum Mode {
    Golden,
    ReaderFailure(u8),
    ModelFailure(u64),
    WrongGoal,
    LargeSummary,
    SizedSummary(usize),
    ChangedSummary,
    UnknownUsage,
}
#[derive(Clone)]
pub struct ObservedRequest {
    pub id: RequestId,
    pub input: String,
}
struct State {
    requests: Vec<ObservedRequest>,
    readers: Vec<(String, u64, u8, RequestId, String)>,
    settled: Vec<RequestId>,
    active: usize,
    peak: usize,
}
pub struct Fixture {
    pub endpoint: SocketAddr,
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<bool>>,
}
pub struct FixtureReport {
    pub requests: Vec<ObservedRequest>,
    pub settled: Vec<RequestId>,
    pub peak: usize,
    pub joined: bool,
}
impl FixtureReport {
    pub fn attest(&self, snapshot: &ProjectSnapshot) -> Result<(), &'static str> {
        if !self.joined {
            return Err("fixture handlers not joined; no settlement attestation");
        }
        let mut receipts = Vec::with_capacity(14);
        for journey in &snapshot.journeys {
            if let Some(readers) = &journey.readers {
                for slot in &readers.children {
                    if let Some(receipt) = slot.accounting {
                        receipts.push((
                            receipt.request_id,
                            receipt.unknown_usage,
                            receipt.unresolved_exposure,
                        ));
                    }
                }
            }
            if let Some(coding) = &journey.coding {
                for slot in &coding.attempts {
                    if let Some(receipt) = slot.accounting {
                        receipts.push((
                            receipt.request_id,
                            receipt.unknown_usage,
                            receipt.unresolved_exposure,
                        ));
                    }
                }
            }
        }
        if receipts.len() > 14 {
            return Err("fixture receipt bound");
        }
        for (id, unknown, exposure) in receipts {
            if exposure {
                Runtime::reconcile_fixture(id)?;
            }
            if unknown {
                // Joined connections without a complete request never started this fixture's synthetic model work.
                let actual = u32::from(self.requests.iter().any(|request| request.id == id));
                Runtime::reconcile_usage_fixture(id, actual)?;
            }
        }
        Ok(())
    }
}
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |n, b| {
        (n ^ b as u64).wrapping_mul(0x100000001b3)
    })
}
impl Fixture {
    pub async fn start(mode: Mode) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            requests: Vec::with_capacity(14),
            readers: Vec::with_capacity(6),
            settled: Vec::with_capacity(14),
            active: 0,
            peak: 0,
        }));
        let changed = Arc::new(Notify::new());
        let (stop, mut stopped) = oneshot::channel();
        let shared = state.clone();
        let notify = changed.clone();
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let mut valid = true;
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => {
                        let Ok((socket,_)) = accepted else { valid = false; break; };
                        let state = shared.clone(); let changed = notify.clone();
                        handlers.spawn(async move { serve(socket,state,changed,mode).await });
                    }
                    result = handlers.join_next(), if !handlers.is_empty() => { valid &= matches!(result,Some(Ok(true))); }
                }
            }
            while let Some(result) = handlers.join_next().await {
                valid &= matches!(result, Ok(true));
            }
            valid
        });
        Self {
            endpoint,
            state,
            changed,
            stop: Some(stop),
            task: Some(task),
        }
    }
    pub fn requests(&self) -> Vec<ObservedRequest> {
        self.state.lock().unwrap().requests.clone()
    }
    pub async fn settle(&self, attempts: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let wait = self.changed.notified();
                if self.state.lock().unwrap().settled.len() >= attempts {
                    break;
                }
                wait.await;
            }
        })
        .await
        .unwrap();
        for id in self.state.lock().unwrap().settled.iter().copied() {
            let _ = Runtime::reconcile_fixture(id);
            let _ = Runtime::reconcile_usage_fixture(id, 1);
        }
    }
    pub async fn shutdown(mut self) -> FixtureReport {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let mut task = self.task.take().unwrap();
        let joined = match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
            Ok(Ok(joined)) => joined,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        };
        let state = self.state.lock().unwrap();
        FixtureReport {
            requests: state.requests.clone(),
            settled: state.settled.clone(),
            peak: state.peak,
            joined,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
async fn input(socket: &mut TcpStream) -> Option<(RequestId, String)> {
    let mut bytes = Vec::with_capacity(8192);
    let mut chunk = [0u8; 1024];
    let start = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 || bytes.len() + n > 8192 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(at) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let header = std::str::from_utf8(&bytes[..start])
        .ok()?
        .to_ascii_lowercase();
    let get = |key: &str| {
        header
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
    };
    let id = RequestId(get("x-fixture-id:")?.parse().ok()?);
    let length: usize = get("content-length:")?.parse().ok()?;
    if length > 4096 {
        return None;
    }
    while bytes.len() < start + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 || bytes.len() + n > 8192 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    Some((
        id,
        std::str::from_utf8(&bytes[start..start + length])
            .ok()?
            .to_owned(),
    ))
}
async fn serve(
    mut socket: TcpStream,
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    mode: Mode,
) -> bool {
    let input = tokio::time::timeout(Duration::from_secs(5), input(&mut socket)).await;
    let Ok(Some((id, input))) = input else {
        return true;
    };
    let value: serde_json::Value = match serde_json::from_str(&input) {
        Ok(value) => value,
        Err(_) => return false,
    };
    {
        let mut shared = state.lock().unwrap();
        if shared.requests.len() == 14 || shared.requests.iter().any(|r| r.id == id) {
            return false;
        }
        shared.requests.push(ObservedRequest {
            id,
            input: input.clone(),
        });
        shared.active += 1;
        shared.peak = shared.peak.max(shared.active);
    }
    changed.notify_waiters();
    let (status, text) = if value.get("child").is_some() {
        let child = value["child"].as_u64().unwrap() as u8;
        let session = value["session"].as_str().unwrap();
        let generation = value["generation"].as_u64().unwrap();
        let content = value["content"].as_str().unwrap();
        let mut summary = format!(
            "child{child}: complete_bytes={} checksum={:016x}",
            content.len(),
            hash(content)
        );
        if matches!(mode, Mode::ChangedSummary) {
            summary.push_str("; alternate observation");
        }
        if matches!(mode, Mode::LargeSummary) {
            summary = "\"".repeat(512);
        }
        if let Mode::SizedSummary(bytes) = mode {
            assert!((summary.len()..=512).contains(&bytes));
            summary.push_str(&"s".repeat(bytes - summary.len()));
        }
        state.lock().unwrap().readers.push((
            session.into(),
            generation,
            child,
            id,
            summary.clone(),
        ));
        tokio::time::sleep(Duration::from_millis(if child == 0 { 30 } else { 5 })).await;
        let status = if matches!(mode,Mode::ReaderFailure(failed) if failed==child) {
            500
        } else {
            200
        };
        (status,serde_json::json!({"session":session,"generation":generation,"child":child,"summary":summary}).to_string())
    } else {
        let project = &value["project"];
        let turn = value["turn"].as_u64().unwrap();
        let previous = value["previous"].as_str().unwrap();
        let request = ProjectRequest::parse(&project["request"].to_string()).unwrap();
        let mut combined = String::with_capacity(1536);
        {
            let shared = state.lock().unwrap();
            let evidence = project["readers"].as_array().unwrap();
            assert_eq!(evidence.len(), 3);
            for (child, evidence) in evidence.iter().enumerate() {
                let summary = evidence["summary"].as_str().unwrap();
                assert!(
                    shared
                        .readers
                        .iter()
                        .any(|(session, generation, c, id, actual)| session
                            == project["reader_session"].as_str().unwrap()
                            && *generation == project["generation"].as_u64().unwrap()
                            && *c as usize == child
                            && id.0 == evidence["request"].as_u64().unwrap()
                            && actual == summary)
                );
                combined.push_str(summary);
            }
        }
        let action = match turn {
            1 => serde_json::json!({"kind":"Read","op":1,"target":request.target()}),
            2 => {
                serde_json::json!({"kind":"Replace","op":2,"target":request.target(),"expected":previous,
                "replacement":if matches!(mode,Mode::WrongGoal) { "wrong goal".into() } else { previous.replacen(request.needle(),request.replacement(),1) }})
            }
            3 => {
                serde_json::json!({"kind":"Verify","op":3,"target":request.target(),"expected":previous})
            }
            4 => {
                serde_json::json!({"kind":"Final","summary":format!("replace_once verified using three actual readers; evidence={:016x}",hash(&combined))})
            }
            _ => panic!("unexpected project turn"),
        };
        (
            if matches!(mode,Mode::ModelFailure(failed) if failed==turn) {
                500
            } else {
                200
            },
            action.to_string(),
        )
    };
    let mut body = String::with_capacity(text.len() + 256);
    let mut at = 0;
    while at < text.len() {
        let mut end = (at + 900).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        body.push_str("data: ");
        body.push_str(&text[at..end]);
        body.push_str("\n\n");
        at = end;
    }
    body.push_str("data: [DONE]\n\n");
    let usage = if matches!(mode, Mode::UnknownUsage) {
        String::new()
    } else {
        format!(
            "X-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\n",
            id.0
        )
    };
    let response = format!(
        "HTTP/1.1 {status} fixture\r\n{usage}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
    drop(socket);
    {
        let mut shared = state.lock().unwrap();
        shared.active -= 1;
        shared.settled.push(id);
    }
    changed.notify_waiters();
    true
}
