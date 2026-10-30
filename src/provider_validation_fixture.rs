use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) fn metadata() -> [serde_json::Value; 3] {
    [
        json!({"startup_ready":true,"transition_state":"ready","version":"0.15.0-beta.1","model_catalog":{"source":"agentic","stale":false}}),
        json!({"data":[{"id":"gpt-6-luna"}]}),
        json!({"revision":"2026-10-26-001","models":[{"requestId":"gpt-6-luna","provider":"openai","primaryProtocol":"openai-responses","supportedProtocols":["openai-responses"],"limits":{"context":1050000,"output":128000}}]}),
    ]
}
pub(super) fn seed(storage: &Path, endpoint: SocketAddr) -> Result<[u8; 32], String> {
    let directory = provider_history::directory(storage)
        .map_err(|_| refused("owned fixture storage refused"))?;
    let file = create(&directory, "provider-attempts.jsonl")?;
    let mut bytes = Vec::new();
    let mut records = vec![
        json!({"kind":"Header","endpoint":endpoint.to_string(),"model":"gpt-6-luna","post_budget":32,"max_output_tokens":1024}),
    ];
    for ordinal in 1..=4 {
        records.push(json!({"kind":"Reserved","ordinal":ordinal,"request_id":ordinal}));
        records.push(json!({"kind":"Joined","ordinal":ordinal,"terminal":null,"usage":null}));
    }
    for record in records {
        bytes.extend_from_slice(&encode(&record, crate::PROVIDER_FRAME_BYTES + 1024)?);
        bytes.push(b'\n');
    }
    let digest = hash(&bytes);
    persist(file, &bytes)?;
    directory
        .sync_all()
        .map_err(|_| refused("fixture baseline sync failed"))?;
    Ok(digest)
}
pub(super) struct ServerResult {
    pub requests: Vec<serde_json::Value>,
    pub dispatched: Vec<Instant>,
}
pub(super) async fn server(
    bodies: [Vec<u8>; 2],
    metadata_second: Option<[serde_json::Value; 3]>,
) -> (SocketAddr, tokio::task::JoinHandle<ServerResult>) {
    server_metadata(bodies, metadata(), metadata_second).await
}
pub(super) async fn server_metadata(
    bodies: [Vec<u8>; 2],
    first: [serde_json::Value; 3],
    metadata_second: Option<[serde_json::Value; 3]>,
) -> (SocketAddr, tokio::task::JoinHandle<ServerResult>) {
    server_controlled(bodies, first, metadata_second, false, None, false).await
}
pub(super) async fn server_controlled(
    bodies: [Vec<u8>; 2],
    first: [serde_json::Value; 3],
    metadata_second: Option<[serde_json::Value; 3]>,
    hold: bool,
    stop: Option<Arc<AtomicBool>>,
    truncate: bool,
) -> (SocketAddr, tokio::task::JoinHandle<ServerResult>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut result = ServerResult {
            requests: Vec::new(),
            dispatched: Vec::new(),
        };
        'connections: loop {
            let accepted = tokio::time::timeout(
                Duration::from_secs(if result.requests.is_empty() { 10 } else { 3 }),
                listener.accept(),
            )
            .await;
            let Ok(Ok((mut socket, _))) = accepted else {
                break;
            };
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                if let Err(error) = socket.read_exact(&mut byte).await {
                    assert!(head.is_empty());
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                    ));
                    continue 'connections;
                }
                head.push(byte[0]);
                assert!(head.len() <= 4096);
            }
            let head = std::str::from_utf8(&head).unwrap().to_ascii_lowercase();
            let (content_type, body) = if head.starts_with("get ") {
                let index = if head.starts_with("get /api/status ") {
                    0
                } else if head.starts_with("get /v1/models ") {
                    1
                } else {
                    assert!(head.starts_with("get /api/model-cards "));
                    2
                };
                let selected = if result.requests.is_empty() {
                    &first
                } else {
                    metadata_second.as_ref().unwrap_or(&first)
                };
                (
                    "application/json",
                    serde_json::to_vec(&selected[index]).unwrap(),
                )
            } else {
                assert!(head.starts_with("post /v1/responses http/1.1\r\n"));
                assert!(head.contains("authorization: bearer managed-by-proxy\r\n"));
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!(length <= 16384);
                let mut bytes = vec![0; length];
                socket.read_exact(&mut bytes).await.unwrap();
                result
                    .requests
                    .push(serde_json::from_slice(&bytes).unwrap());
                result.dispatched.push(Instant::now());
                assert!(result.requests.len() <= 2, "third POST is forbidden");
                (
                    "text/event-stream",
                    bodies[result.requests.len() - 1].clone(),
                )
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()+usize::from(content_type=="text/event-stream" && result.requests.len()==1 && truncate)).as_bytes()).await.unwrap();
            for chunk in body.chunks(127) {
                if socket.write_all(chunk).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            if content_type == "text/event-stream" && result.requests.len() == 1 && hold {
                if let Some(stop) = &stop {
                    stop.store(true, Ordering::SeqCst);
                }
                let mut byte = [0];
                match tokio::time::timeout(Duration::from_secs(34), socket.read(&mut byte))
                    .await
                    .unwrap()
                {
                    Ok(0) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    result => panic!("provider transport was not joined: {result:?}"),
                };
            }
        }
        result
    });
    (endpoint, task)
}
pub(super) fn first_events() -> Vec<serde_json::Value> {
    vec![
        json!({"type":"response.created","response":{"id":"response-first","model":"gpt-6-luna","status":"in_progress","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"reasoning-first","summary":[]}}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"message-first","role":"assistant","status":"in_progress","content":[]}}),
        json!({"type":"response.content_part.added","output_index":1,"item_id":"message-first","content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
        json!({"type":"response.output_text.delta","output_index":1,"item_id":"message-first","content_index":0,"delta":"synthetic-smoke"}),
        json!({"type":"response.output_text.done","output_index":1,"item_id":"message-first","content_index":0,"text":"synthetic-smoke"}),
        json!({"type":"response.content_part.done","output_index":1,"item_id":"message-first","content_index":0,"part":{"type":"output_text","text":"synthetic-smoke","annotations":[]}}),
        json!({"type":"response.output_item.done","output_index":1,"item":{"type":"message","id":"message-first","role":"assistant","status":"completed","phase":"final_answer","content":[{"type":"output_text","text":"synthetic-smoke","annotations":[]}]}}),
        json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"reasoning-first","summary":[],"encrypted_content":"synthetic-opaque:/é🙂\\\""}}),
        json!({"type":"response.completed","response":{"id":"response-first","model":"gpt-6-luna","status":"completed","output":[{"type":"reasoning","id":"reasoning-first","summary":[],"encrypted_content":"synthetic-opaque:/é🙂\\\""},{"type":"message","id":"message-first","role":"assistant","status":"completed","phase":"final_answer","content":[{"type":"output_text","text":"synthetic-smoke","annotations":[]}]}],"usage":fixture::known_usage()}}),
    ]
}
pub(super) async fn run(
    storage: &Path,
    evidence: &Path,
    stopped: Arc<AtomicBool>,
) -> Result<Vec<u8>, String> {
    let first = first_events();
    let second = fixture::call_events("{\"message\":\"synthetic-smoke\"}", fixture::known_usage());
    let (endpoint, task) = server(
        [fixture::wire(&first, true), fixture::wire(&second, false)],
        None,
    )
    .await;
    let expected = seed(storage, endpoint)?;
    let result = run_pair(
        endpoint,
        storage.into(),
        expected,
        evidence.into(),
        stopped,
        false,
    )
    .await;
    let observed = task
        .await
        .map_err(|_| refused("pair fixture server failed"))?;
    if observed.requests.len() != 2
        && result.as_ref().is_ok_and(|v| {
            serde_json::from_slice::<serde_json::Value>(v).is_ok_and(|v| v["success"] == true)
        })
    {
        return Err(refused("pair fixture observation contradicts success"));
    }
    if observed.dispatched.len() == 2
        && observed.dispatched[1].duration_since(observed.dispatched[0]) < Duration::from_secs(1)
    {
        return Err(refused("pair fixture spacing violated"));
    }
    result.map(|bytes| {
        let mut report: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        report["fixture_posts"] = json!(observed.requests.len());
        serde_json::to_vec(&report).unwrap()
    })
}
