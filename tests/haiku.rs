#[path = "support/responses_fixture.rs"]
mod fixture;
use scramjet_rs::{
    EditCancellation, HAIKU_COMMAND, HAIKU_SCRIPT, HaikuProcessResult, HaikuSession, HaikuStep,
    Runtime, haiku_prompt, haiku_tools,
};
use serde_json::{Value, json};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};
use std::{os::unix::fs::DirBuilderExt, path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
struct Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
fn allocated(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
    LARGEST.fetch_max(bytes, Ordering::Relaxed);
    REQUESTED.fetch_add(bytes, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        allocated(bytes);
        let result = unsafe { System.realloc(pointer, layout, bytes) };
        LIVE.fetch_sub(
            if result.is_null() {
                bytes
            } else {
                layout.size()
            },
            Ordering::Relaxed,
        );
        result
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn owned() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "scramjet-haiku-http-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}
fn rename(value: &mut Value, name: &str) {
    match value {
        Value::Object(map) => {
            if map.get("name").is_some_and(|value| value == "inert") {
                map.insert("name".into(), json!(name));
            }
            for value in map.values_mut() {
                rename(value, name);
            }
        }
        Value::Array(values) => {
            for value in values {
                rename(value, name);
            }
        }
        _ => {}
    }
}
async fn request(socket: &mut tokio::net::TcpStream) -> Value {
    let mut bytes = Vec::new();
    let length;
    loop {
        let mut byte = [0];
        socket.read_exact(&mut byte).await.unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() <= 4096);
        if bytes.ends_with(b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes).unwrap();
            assert!(headers.starts_with("POST /v1/responses HTTP/1.1\r\n"));
            length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|length| length.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            break;
        }
    }
    assert!(length <= 16384);
    let mut body = vec![0; length];
    socket.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn actual_http_retains_complete_ordered_context_and_actual_effect_results() {
    let root = owned();
    let project = root.join("project");
    let storage = root.join("storage");
    let provider_storage = root.join("provider");
    for path in [&project, &storage, &provider_storage] {
        std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let poem = "Rust unfolds\r\nStreams answer\rFiles remember\0é🙂\n";
    let (runtime, server) = executor.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let runtime = Runtime::new_provider(listener.local_addr().unwrap(), &provider_storage).unwrap();
        let task = tokio::spawn(async move {
            let mut expected = vec![json!({"role":"user","content":haiku_prompt()})];
            let tools: Vec<_> = haiku_tools().into_iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":true})).collect();
            for (turn, (name, args, output)) in [
                ("write", json!({"path":"haiku.md","content":poem}), serde_json::to_string(&json!({"status":"created","path":"haiku.md","mode":384})).unwrap()),
                ("read", json!({"path":"haiku.md"}), serde_json::to_string(&json!({"status":"read","path":"haiku.md","content":poem})).unwrap()),
                ("write", json!({"path":"print_haiku.py","content":HAIKU_SCRIPT}), serde_json::to_string(&json!({"status":"created","path":"print_haiku.py","mode":384})).unwrap()),
                ("bash", json!({"command":HAIKU_COMMAND}), serde_json::to_string(&HaikuProcessResult { exit_code: Some(0), stdout: poem.into(), stderr: String::new(), stopped: None, launcher_reaped: true, namespace_dead: true }).unwrap()),
                ("", json!({}), String::new()),
            ].into_iter().enumerate() {
                let (mut socket, _) = listener.accept().await.unwrap();
                let actual = request(&mut socket).await;
                assert_eq!(actual, json!({"model":"gpt-6-luna","input":expected,"tools":tools,"stream":true,"store":false,"include":["reasoning.encrypted_content"],"max_output_tokens":1024,"parallel_tool_calls":false}));
                let mut events = if name.is_empty() { fixture::text_events("done", fixture::known_usage()) } else { fixture::continuation_events("working hé🙂", &args.to_string(), "opaque encrypted é🙂") };
                for event in &mut events { rename(event, name); }
                fixture::qualify_events(&mut events, turn + 1);
                expected.extend(events.last().unwrap()["response"]["output"].as_array().unwrap().iter().cloned());
                if !name.is_empty() { expected.push(json!({"type":"function_call_output","call_id":format!("call_1.{}", turn + 1),"output":output})); }
                let body = fixture::wire(&events, turn % 2 == 0);
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                for chunk in body.chunks(37) { socket.write_all(chunk).await.unwrap(); }
                socket.shutdown().await.unwrap();
            }
            assert!(tokio::time::timeout(Duration::from_millis(30), listener.accept()).await.is_err());
        });
        (runtime, task)
    });
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    REQUESTED.store(0, Ordering::Relaxed);
    let mut session = HaikuSession::create(&project, &storage, "http").unwrap();
    for turn in 1..=5 {
        let request = session.prepare_request().unwrap();
        let response = executor.block_on(async {
            let mut handle = runtime.submit_provider(request).unwrap();
            let id = handle.id();
            let response = handle.result().await.unwrap();
            assert!(handle.result().await.is_none());
            drop(handle);
            (id, response)
        });
        match session.select_response(response.0, response.1).unwrap() {
            HaikuStep::Approval(proposal) => session
                .decide(proposal, true, &EditCancellation::default())
                .unwrap(),
            HaikuStep::Read { content, .. } => assert_eq!(content, poem),
            HaikuStep::Finished => assert_eq!(turn, 5),
        }
    }
    assert!(session.successful());
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    println!(
        "partial effect-owner actual HTTP: incremental_peak={peak} largest={} total_requested={}",
        LARGEST.load(Ordering::Relaxed),
        REQUESTED.load(Ordering::Relaxed)
    );
    assert!(peak <= 2_097_152);
    assert_eq!(session.process_result().unwrap().stdout, poem);
    let history = runtime.provider_history().unwrap();
    assert_eq!(history.consumed(), 5);
    assert_eq!(
        (history.unresolved_remote(), history.unknown_usage()),
        (0, 0)
    );
    let snapshot = executor.block_on(async {
        let snapshot = runtime.shutdown().await;
        server.await.unwrap();
        snapshot
    });
    assert_eq!(snapshot.active, 0);
    drop(session);
    std::fs::remove_dir_all(root).unwrap();
}
