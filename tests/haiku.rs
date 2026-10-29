#[path = "support/responses_fixture.rs"]
mod fixture;
use fixture as responses;
#[path = "support/haiku_fixture.rs"]
mod journey;
#[path = "support/provider_server.rs"]
mod server_fixture;
use scramjet_rs::{
    HAIKU_COMMAND, HAIKU_SCRIPT, HaikuAgent, HaikuProcessResult, HaikuSession, Runtime,
    haiku_prompt, haiku_tools,
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
                ("bash", json!({"command":HAIKU_COMMAND}), serde_json::to_string(&HaikuProcessResult { exit_code: Some(0), signal: None, stdout: poem.into(), stderr: String::new(), stopped: None, launcher_reaped: true, namespace_dead: true }).unwrap()),
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
    let session = HaikuSession::create(&project, &storage, "http").unwrap();
    let agent = HaikuAgent::start(session).unwrap();
    let observer = agent.observer();
    let control = agent.control();
    let run = executor.block_on(async {
        let approvals = tokio::spawn(async move {
            let mut snapshots = Vec::with_capacity(80);
            loop {
                let snapshot = observer.snapshot();
                if let Some(proposal) = &snapshot.proposal {
                    control.decide(proposal, true).unwrap();
                    assert!(control.decide(proposal, true).is_err());
                }
                let ended = snapshot.run_ended;
                if snapshots.len() < 80 {
                    snapshots.push(snapshot);
                }
                if ended {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            snapshots
        });
        let run = agent.run(&runtime).await.unwrap();
        let retained = approvals.await.unwrap();
        assert!(retained.len() <= 80);
        assert!(run.snapshot.worker_joined && run.snapshot.worker_terminated);
        run
    });
    assert_eq!(run.turns, 5);
    assert_eq!(run.request_ids.len(), 5);
    assert!(run.snapshot.history.finished());
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    println!(
        "common driver actual HTTP: incremental_peak={peak} largest={} total_requested={}",
        LARGEST.load(Ordering::Relaxed),
        REQUESTED.load(Ordering::Relaxed)
    );
    assert!(peak <= 2_097_152);
    assert_eq!(run.snapshot.history.process().unwrap().stdout, poem);
    let journal_path = storage.join("http/haiku.jsonl");
    let original = std::fs::read(&journal_path).unwrap();
    let recovery_base = LIVE.load(Ordering::Relaxed);
    PEAK.store(recovery_base, Ordering::Relaxed);
    let recovered = scramjet_rs::reconstruct_haiku_history(&storage, "http").unwrap();
    assert!(recovered.ending_seen());
    assert!(!recovered.finished());
    assert_eq!(recovered.effects().len(), 3);
    assert_eq!(recovered.attempts().len(), 5);
    assert_eq!(recovered.process().unwrap().stdout, poem);
    assert_eq!(original, std::fs::read(&journal_path).unwrap());
    let recovery_peak = PEAK.load(Ordering::Relaxed).saturating_sub(recovery_base);
    println!("common driver recovery incremental_peak={recovery_peak}");
    assert!(recovery_peak <= 4_194_304);
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
    drop(run);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_and_refused_journeys_run_in_independent_owners() {
    for case in [
        "maximum",
        "context",
        "nodes",
        "refused",
        "cancelled",
        "malformed",
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "bounded_journey_child", "--nocapture"])
            .env("HAIKU_BOUNDED_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
}
fn measure_selected_recovery(
    storage: &std::path::Path,
    journal: &std::path::Path,
    original: &[u8],
) {
    let mut records: Vec<Value> = std::str::from_utf8(original)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let result = records
        .iter()
        .position(|entry| entry["record"]["kind"] == "Result")
        .unwrap();
    records.truncate(result + 1);
    records.push(json!({"sequence":records.len(),"record":{"kind":"Stopped","turn":1,"reason":"synthetic resource probe"}}));
    let selected = records
        .iter()
        .position(|entry| entry["record"]["kind"] == "Selected")
        .unwrap();
    for (index, item) in records[selected]["record"]["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        let kind = item["type"].as_str().unwrap().to_owned();
        let map = item.as_object_mut().unwrap();
        map.insert("id".into(), json!(format!("opaque é🙂 /\u{0}\r\n {index}")));
        if kind == "function_call" || kind == "reasoning" {
            map.remove("status");
        }
        if kind == "function_call" {
            map.insert("namespace".into(), Value::Null);
        }
    }
    for (kind, field, value) in [
        ("", "", Value::Null),
        ("function_call", "status", json!("in_progress")),
        ("function_call", "namespace", json!("foreign")),
        ("message", "content", json!({})),
        ("reasoning", "summary", json!({})),
    ] {
        let mut changed = records.clone();
        if !kind.is_empty() {
            let item = changed[selected]["record"]["items"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|item| item["type"] == kind)
                .unwrap();
            item[field] = value;
        }
        let bytes: Vec<u8> = changed
            .iter()
            .flat_map(|entry| format!("{entry}\n").into_bytes())
            .collect();
        std::fs::write(journal, &bytes).unwrap();
        let base = LIVE.load(Ordering::Relaxed);
        PEAK.store(base, Ordering::Relaxed);
        let history = scramjet_rs::reconstruct_haiku_history(storage, "bounded");
        let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
        println!("selected {kind}/{field} recovery incremental_peak={peak}");
        assert!(peak <= 4_194_304);
        if kind.is_empty() {
            let history = history.unwrap();
            assert_eq!(
                history.effects()[0].state,
                scramjet_rs::HaikuCertainty::Completed
            );
            assert!(!history.finished() && !history.ending_seen());
            assert!(!history.entries.last().unwrap().acknowledged);
        } else {
            assert!(matches!(history, Err(scramjet_rs::CodingError::History(_))));
        }
        assert_eq!(bytes, std::fs::read(journal).unwrap());
    }
    std::fs::write(journal, original).unwrap();
}
#[test]
fn bounded_journey_child() {
    let Ok(case) = std::env::var("HAIKU_BOUNDED_CASE") else {
        return;
    };
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
    if case == "malformed" {
        let session = HaikuSession::create(&project, &storage, "bounded").unwrap();
        drop(session);
        let path = storage.join("bounded/haiku.jsonl");
        let mut bytes = std::fs::read(&path).unwrap();
        let record = json!({"sequence":1,"record":{"kind":"Selected","turn":1,"request_id":1,"ordinal":1,"response_id":"opaque","items":[{"id":"r","type":"reasoning","encrypted_content":"e","metadata":vec!["";40000]}],"usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15,"cached_input_tokens":null,"reasoning_tokens":null}}});
        bytes.extend_from_slice(record.to_string().as_bytes());
        bytes.push(b'\n');
        std::fs::write(&path, &bytes).unwrap();
        let base = LIVE.load(Ordering::Relaxed);
        PEAK.store(base, Ordering::Relaxed);
        assert!(matches!(
            scramjet_rs::reconstruct_haiku_history(&storage, "bounded"),
            Err(scramjet_rs::CodingError::ResourceLimit(_))
        ));
        let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
        println!("malformed selected recovery incremental_peak={peak}");
        assert!(peak <= 4_194_304);
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        assert!(!project.join("haiku.md").exists());
        std::fs::remove_dir_all(root).unwrap();
        return;
    }
    let poem = if case == "context" {
        "\0".repeat(512)
    } else {
        "x".repeat(512)
    };
    let mut sequence = if case == "maximum" {
        let mut sequence = journey::sequence(&poem, 2, &"opaque é🙂".repeat(20));
        let mut final_events = fixture::text_events(&"x".repeat(15700), fixture::known_usage());
        fixture::qualify_events(&mut final_events, 7);
        sequence.last_mut().unwrap().1 = fixture::wire(&final_events, true);
        sequence
    } else if case == "nodes" {
        let mut sequence = journey::sequence(&poem, 0, "");
        let metadata = json!(vec![Vec::<Value>::new(); 4000]);
        let body = std::str::from_utf8(&sequence[0].1).unwrap();
        let mut events: Vec<Value> = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        fn dense(value: &mut Value, metadata: &Value) {
            match value {
                Value::Object(map) => {
                    if map.get("id").is_some_and(|id| id == "msg_1.1")
                        && map.get("type").is_some_and(|kind| kind == "message")
                    {
                        map.insert("metadata".into(), metadata.clone());
                    }
                    for (key, value) in map.iter_mut() {
                        if key != "metadata" {
                            dense(value, metadata);
                        }
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        dense(value, metadata);
                    }
                }
                _ => {}
            }
        }
        for event in &mut events {
            dense(event, &metadata);
        }
        sequence[0].1 = fixture::wire(&events, true);
        for (request, _) in sequence.iter_mut().skip(1) {
            dense(request.as_mut().unwrap(), &metadata);
        }
        let first_refused = sequence
            .iter()
            .position(|(request, _)| request.as_ref().unwrap().to_string().len() > 16384)
            .expect("dense retained context must refuse intact");
        assert!(first_refused > 0);
        sequence.truncate(first_refused);
        sequence
    } else if case == "context" {
        let mut found = None;
        for bytes in (0..=768).step_by(16) {
            let sequence = journey::sequence(&poem, 0, &"e".repeat(bytes));
            let sizes: Vec<_> = sequence
                .iter()
                .map(|(request, _)| request.as_ref().unwrap().to_string().len())
                .collect();
            if sizes[..4].iter().all(|size| *size <= 16384) && sizes[4] > 16384 {
                found = Some(sequence);
                break;
            }
        }
        let mut sequence =
            found.expect("a complete process can settle before intact next-context refusal");
        sequence.truncate(4);
        sequence
    } else if case == "cancelled" {
        Vec::new()
    } else {
        let request = journey::sequence("poem", 0, "e").remove(0).0;
        vec![(
            request,
            fixture::wire(&fixture::disagreement_events(), true),
        )]
    };
    let expected_posts = if case == "cancelled" {
        1
    } else {
        sequence.len()
    };
    let expected_size = sequence
        .iter()
        .map(|(request, _)| request.as_ref().unwrap().to_string().len())
        .max()
        .unwrap_or(0);
    assert!(expected_size <= 16384);
    let reached = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server_reached = reached.clone();
    let (runtime, server) = executor.block_on(async {
        let (endpoint, server) = if case == "cancelled" {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let _ = request(&mut socket).await;
                server_reached.store(true, Ordering::Release);
                let mut byte = [0];
                let received = socket.read(&mut byte).await;
                assert!(
                        matches!(received, Ok(0))
                            || received.as_ref().is_err_and(
                                |error| error.kind() == std::io::ErrorKind::ConnectionReset
                            )
                    );
            });
            (endpoint, server)
        } else if case == "refused" {
            server_fixture::start(sequence.remove(0).1).await
        } else {
            server_fixture::start_sequence(std::mem::take(&mut sequence)).await
        };
        (
            Runtime::new_provider(endpoint, &provider_storage).unwrap(),
            server,
        )
    });
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    let agent =
        HaikuAgent::start(HaikuSession::create(&project, &storage, "bounded").unwrap()).unwrap();
    let observer = agent.observer();
    let control = agent.control();
    let final_observer = observer.clone();
    let result = executor.block_on(async {
        let approvals = tokio::spawn(async move {
            let mut retained = Vec::with_capacity(80);
            let mut previous = 0;
            loop {
                let snapshot = observer.snapshot();
                if reached.load(Ordering::Acquire) {
                    control.cancel();
                }
                if let Some(proposal) = &snapshot.proposal {
                    control.decide(proposal, true).unwrap();
                }
                if previous != snapshot.history.entries.len() && retained.len() < 80 {
                    previous = snapshot.history.entries.len();
                    retained.push(snapshot.clone());
                }
                if snapshot.run_ended {
                    while retained.len() < 80 {
                        retained.push(snapshot.clone());
                    }
                    return retained;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let result = agent.run(&runtime).await;
        let retained = approvals.await.unwrap();
        assert_eq!(retained.len(), 80);
        result
    });
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    println!(
        "{case} complete driver incremental_peak={peak} largest={} posts={expected_posts} max_request={expected_size}",
        LARGEST.load(Ordering::Relaxed)
    );
    assert!(peak <= 2_097_152);
    let snapshot = final_observer.snapshot();
    assert!(snapshot.worker_joined && snapshot.worker_terminated);
    if case == "maximum" {
        assert_eq!(result.unwrap().turns, 7);
        assert!(snapshot.history.finished());
        assert_eq!(snapshot.history.effects().len(), 3);
    } else if case == "nodes" {
        assert!(
            matches!(result, Err(scramjet_rs::CodingError::ResourceLimit(_))),
            "{result:?}"
        );
        assert_eq!(
            std::fs::read(project.join("haiku.md")).unwrap(),
            poem.as_bytes()
        );
    } else if case == "context" {
        assert!(
            matches!(result, Err(scramjet_rs::CodingError::ResourceLimit(_))),
            "{result:?}"
        );
        assert_eq!(snapshot.history.process().unwrap().stdout, poem);
        assert!(snapshot.process_acknowledged);
        assert!(
            snapshot
                .history
                .effects()
                .iter()
                .all(|fact| fact.state == scramjet_rs::HaikuCertainty::Completed)
        );
    } else if case == "cancelled" {
        assert!(
            matches!(result, Err(scramjet_rs::CodingError::Cancelled)),
            "{result:?}"
        );
        assert_eq!(
            snapshot.history.attempts()[0].outcome,
            scramjet_rs::HaikuOutcome::Cancelled
        );
        assert!(!project.join("haiku.md").exists());
    } else {
        assert!(
            matches!(result, Err(scramjet_rs::CodingError::Unsupported(_))),
            "{result:?}"
        );
        assert!(!project.join("haiku.md").exists());
    }
    let accounting = runtime.provider_history().unwrap();
    assert_eq!(accounting.consumed(), expected_posts);
    assert_eq!(
        (accounting.unknown_usage(), accounting.unresolved_remote()),
        if case == "cancelled" { (1, 1) } else { (0, 0) }
    );
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let journal = storage.join("bounded/haiku.jsonl");
    let original = std::fs::read(&journal).unwrap();
    let recovered = scramjet_rs::reconstruct_haiku_history(&storage, "bounded").unwrap();
    assert_eq!(recovered.attempts().len(), expected_posts);
    assert!(!recovered.finished());
    assert_eq!(original, std::fs::read(&journal).unwrap());
    let recovery_peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    println!("{case} recovery incremental_peak={recovery_peak}");
    assert!(recovery_peak <= 4_194_304);
    if case == "nodes" {
        measure_selected_recovery(&storage, &journal, &original);
    }
    executor.block_on(async {
        runtime.shutdown().await;
        server.await.unwrap();
    });
    std::fs::remove_dir_all(root).unwrap();
}
