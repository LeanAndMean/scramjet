use scramjet_rs::{Outcome, ProviderInput, ProviderRequest, Runtime};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    os::unix::fs::DirBuilderExt,
    sync::atomic::{AtomicUsize, Ordering},
};
#[allow(dead_code)]
#[path = "../src/provider_demo.rs"]
mod demo;
use demo::{fixture, server};
struct Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
fn allocated(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
    LARGEST.fetch_max(bytes, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        allocated(bytes);
        let new = unsafe { System.realloc(p, layout, bytes) };
        LIVE.fetch_sub(
            if new.is_null() { bytes } else { layout.size() },
            Ordering::Relaxed,
        );
        new
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;
fn reset() -> usize {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    live
}
fn storage(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "{label}-{}-{}",
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
fn measure(label: &str, base: usize, ceiling: usize) {
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(base);
    let largest = LARGEST.load(Ordering::Relaxed);
    println!("{label}: incremental_peak={peak} largest={largest}");
    assert!(peak <= ceiling, "{label}: {peak}");
    assert!(largest <= 2_097_152, "{label}: {largest}");
}
#[test]
fn bounded_maximum_request_frame_and_history_allocations() {
    demo::check_report_writer_budget();
    let max: usize = 16384;
    let mut low = 0;
    let mut high = max;
    while low < high {
        let mid = (low + high).div_ceil(2);
        if ProviderRequest::new(&[ProviderInput::User("x".repeat(mid))], &[]).is_ok() {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let input = vec![ProviderInput::User("x".repeat(low))];
    let base = reset();
    let request = ProviderRequest::new(&input, &[]).unwrap();
    assert_eq!(request.encoded_bytes(), max);
    measure("maximum-request", base, 262144);
    assert!(ProviderRequest::new(&[ProviderInput::User("x".repeat(low + 1))], &[]).is_err());
    let events = fixture::text_events(&"x".repeat(15800), fixture::known_usage());
    let body = fixture::wire(&events, false);
    assert!(body.len() < 131072);
    let call = fixture::call_events("{}", fixture::known_usage());
    assert!(!fixture::wire(&call, true).is_empty());
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let history_path = storage("provider-history-meter");
    let mut lines = vec![
        serde_json::json!({"kind":"Header","endpoint":"127.0.0.1:18080","model":"gpt-6-luna","post_budget":32,"max_output_tokens":1024}),
    ];
    for ordinal in 1..=32 {
        lines.push(serde_json::json!({"kind":"Reserved","ordinal":ordinal,"request_id":ordinal}));
        lines.push(serde_json::json!({"kind":"Joined","ordinal":ordinal,"terminal":{"response_id":"r".repeat(scramjet_rs::PROVIDER_FRAME_BYTES-1024),"status":"completed"},
            "usage":{"input_tokens":u64::MAX-1,"output_tokens":1,"total_tokens":u64::MAX,"cached_input_tokens":u64::MAX-1,"reasoning_tokens":1}}));
    }
    let wire: String = lines.iter().map(|line| format!("{line}\n")).collect();
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let history_file = history_path.join("provider-attempts.jsonl");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&history_file)
        .unwrap()
        .write_all(wire.as_bytes())
        .unwrap();
    let base = reset();
    let history = scramjet_rs::reconstruct_provider_history(
        &history_path,
        "127.0.0.1:18080".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(history.consumed(), 32);
    measure("maximum-valid-history", base, 2_097_152);
    drop(history);
    let history_budget = (1 + 2 * scramjet_rs::PROVIDER_POST_BUDGET)
        * (scramjet_rs::PROVIDER_FRAME_BYTES + 1024 + 1);
    let mut oversized = vec![b'x'; history_budget];
    *oversized.last_mut().unwrap() = b'\n';
    std::fs::write(&history_file, &oversized).unwrap();
    let base = reset();
    assert!(
        scramjet_rs::reconstruct_provider_history(
            &history_path,
            "127.0.0.1:18080".parse().unwrap()
        )
        .is_err()
    );
    measure("maximum-malformed-history", base, 2_097_152);
    executor.block_on(async {
        let path = storage("provider-owned-history-meter");
        let events = fixture::text_events("checked", fixture::known_usage());
        let (endpoint, task) = server::start(fixture::wire(&events, false)).await;
        let mut records = lines[..63].to_vec();
        records[0]["endpoint"] = serde_json::json!(endpoint.to_string());
        for record in &mut records {
            if record["kind"] == "Joined" {
                record["terminal"]["response_id"] = serde_json::json!(format!("{}\u{0000}é", "r".repeat(16300)));
            }
        }
        let record_budget = scramjet_rs::PROVIDER_FRAME_BYTES + 1024;
        let mut baseline = Vec::new();
        for record in &records {
            let encoded = serde_json::to_vec(record).unwrap();
            assert!(encoded.len() <= record_budget);
            baseline.extend(std::iter::repeat_n(b' ', record_budget - encoded.len()));
            baseline.extend_from_slice(&encoded);
            baseline.push(b'\n');
        }
        let file = path.join("provider-attempts.jsonl");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .unwrap()
            .write_all(&baseline)
            .unwrap();
        let probe_request = ProviderRequest::new(&[ProviderInput::User("synthetic".into())], &[]).unwrap();
        let base = reset();
        let runtime = Runtime::new_provider(endpoint, &path).unwrap();
        let mut handle = runtime.submit_provider(probe_request).unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), handle.result())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.outcome, Outcome::Success);
        assert_eq!(response.reservation, Some(32));
        assert!(response.durable);
        measure("maximum-noncanonical-owned-history-append", base, 2097152);
        let history = runtime.provider_history().unwrap();
        assert_eq!(history.consumed(), 32);
        assert_eq!(history.unresolved_remote(), 0);
        assert_eq!(history.unknown_usage(), 0);
        measure("maximum-owned-history-with-caller-snapshot", base, 4194304);
        assert!(std::fs::read(&file).unwrap().starts_with(&baseline));
        drop(history);
        drop(response);
        drop(handle);
        runtime.shutdown().await;
        task.await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let refused_path = storage("provider-owned-history-refusal-meter");
        let refused_file = refused_path.join("provider-attempts.jsonl");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&refused_file)
            .unwrap()
            .write_all(&baseline)
            .unwrap();
        let mut changed = baseline.clone();
        let index = changed.iter().position(|byte| *byte == b'3').unwrap();
        changed[index] = b'2';
        let base = reset();
        let runtime = Runtime::new_provider(endpoint, &refused_path).unwrap();
        std::fs::write(&refused_file, &changed).unwrap();
        let mut handle = runtime
            .submit_provider(ProviderRequest::new(&[ProviderInput::User("refused".into())], &[]).unwrap())
            .unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), handle.result())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.outcome, Outcome::ResourceLimit);
        assert!(response.reservation.is_none());
        assert_eq!(runtime.snapshot().attempts, 0);
        measure("maximum-owned-history-content-refusal", base, 2097152);
        drop(response);
        drop(handle);
        runtime.shutdown().await;
        assert_eq!(std::fs::read(&refused_file).unwrap(), changed);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let path = storage("provider-meter");
        let (endpoint, task) = server::start(body).await;
        let runtime = Runtime::new_provider(endpoint, &path).unwrap();
        let base = reset();
        let mut handle = runtime.submit_provider(request).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let response = handle.result().await.unwrap();
        assert_eq!(response.outcome, Outcome::Success);
        assert_eq!(
            response.envelope.as_ref().unwrap().items[0]
                .text()
                .unwrap()
                .len(),
            15800
        );
        measure("maximum-frame-slow-consumer", base, 1048576);
        drop(response);
        drop(handle);
        runtime.shutdown().await;
        task.await.unwrap();

        for (encrypted_bytes, id_bytes, refused, expected) in [(14000,1000,false,Outcome::Success),(15000,1500,false,Outcome::ResourceLimit),(14000,1000,true,Outcome::Unsupported),(15000,1500,true,Outcome::ResourceLimit)] {
            let id = "r".repeat(id_bytes);
            let done = serde_json::json!({"type":"reasoning","id":"rs","summary":[],"encrypted_content":"x".repeat(encrypted_bytes)});
            let mut terminal = serde_json::json!({"type":"reasoning","id":"rs","summary":[]});
            if refused {terminal["opaque"]=serde_json::json!(true);}
            let events = vec![
                serde_json::json!({"type":"response.created","response":{"id":id,"status":"in_progress","output":[]}}),
                serde_json::json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs","summary":[]}}),
                serde_json::json!({"type":"response.output_item.done","output_index":0,"item":done}),
                serde_json::json!({"type":"response.completed","response":{"id":id,"status":"completed","output":[terminal]}}),
            ];
            let body = fixture::wire(&events,true);
            let base = reset();
            let replay = scramjet_rs::replay_provider_data(&body);
            assert_eq!(replay.outcome,expected);
            measure(if refused {"canonical-done-with-replay-refusal"}else{"canonical-done-enrichment-success-or-refusal"},base,1048576);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let path = storage("provider-replay-meter");
        let events = fixture::continuation_events("checked", "{}", &"opaque:/é".repeat(64));
        let (endpoint, task) = server::start(fixture::wire(&events,true)).await;
        let runtime = Runtime::new_provider(endpoint,&path).unwrap();
        let base = reset();
        let mut handle = runtime.submit_provider(ProviderRequest::new(&[ProviderInput::User("synthetic".into())],&[]).unwrap()).unwrap();
        let response = handle.result().await.unwrap();
        assert_eq!(response.outcome,Outcome::Success);
        let mut context = vec![ProviderInput::User("\u{0000}".repeat(512))];
        context.extend(response.envelope.unwrap().items.into_iter().map(ProviderInput::Item));
        context.push(ProviderInput::ToolResult{call_id:"call_1".into(),output:"\u{0000}".repeat(512)});
        measure("mixed-replay-transport",base,1048576);
        let tools:Vec<_>=["inert","read","bash"].into_iter().map(|name|scramjet_rs::ProviderTool{name:name.into(),description:"synthetic data only".into(),parameters:serde_json::json!({"type":"object","properties":{"payload":{"type":"string"}},"required":["payload"],"additionalProperties":false})}).collect();
        let mut low: usize = 0;
        let mut high: usize = 16384;
        while low < high {
            let mid = (low+high).div_ceil(2);
            let mut probe = context.clone();
            probe.push(ProviderInput::User("x".repeat(mid)));
            if ProviderRequest::new(&probe,&tools).is_ok() { low=mid; } else { high=mid-1; }
        }
        context.push(ProviderInput::User("x".repeat(low)));
        let base = reset();
        let request = ProviderRequest::new(&context,&tools).unwrap();
        assert_eq!(request.encoded_bytes(),16384);
        measure("maximum-complete-mixed-replay-request",base,262144);
        if let ProviderInput::User(text) = context.last_mut().unwrap() { text.push('x'); }
        let base = reset();
        assert!(matches!(ProviderRequest::new(&context,&tools),Err(Outcome::ResourceLimit)));
        measure("complete-mixed-replay-request-refusal",base,262144);
        assert_eq!(runtime.provider_history().unwrap().consumed(),1);
        drop(handle);
        runtime.shutdown().await;
        task.await.unwrap();

        let path = storage("provider-diagnostic-meter");
        let mut nested = serde_json::json!("synthetic-private");
        for _ in 0..100 { nested = serde_json::json!({"synthetic-private":nested}); }
        let mut events = vec![fixture::text_events("x", serde_json::Value::Null)[0].clone()];
        let mut output = Vec::new();
        for index in 0..8 {
            let added = serde_json::json!({"type":"reasoning","id":format!("rs_{index}"),"summary":[]});
            let content = vec![serde_json::json!({"type":"reasoning_text","text":"hé🙂","annotations":[]}); 16];
            let done = serde_json::json!({"type":"reasoning","id":format!("rs_{index}"),"summary":[],"content":content,"phase":null,"encrypted_content":"opaque"});
            events.push(serde_json::json!({"type":"response.output_item.added","output_index":index,"item":added}));
            events.push(serde_json::json!({"type":"response.output_item.done","output_index":index,"item":done}));
            let mut final_item = done;
            final_item["phase"] = serde_json::json!("final_answer");
            for part in final_item["content"].as_array_mut().unwrap() { part["text"] = serde_json::json!("changed"); }
            if index == 0 { final_item["synthetic-private"] = nested.clone(); }
            output.push(final_item);
        }
        events.push(serde_json::json!({"type":"response.completed","response":{"id":"resp_1","status":"completed","output":output,"usage":fixture::known_usage()}}));
        for event in &events { assert!(serde_json::to_vec(event).unwrap().len() < scramjet_rs::PROVIDER_FRAME_BYTES - 128); }
        events.last_mut().unwrap()["response"]["output"][7]["synthetic-private-padding"]=serde_json::json!("");
        let initial=fixture::wire(&events,true);
        let terminal_bytes=std::str::from_utf8(&initial).unwrap().rsplit("\r\n\r\n").nth(1).unwrap().len()+4;
        events.last_mut().unwrap()["response"]["output"][7]["synthetic-private-padding"]=serde_json::json!("x".repeat(scramjet_rs::PROVIDER_FRAME_BYTES-terminal_bytes));
        let body = fixture::wire(&events, true);
        assert_eq!(std::str::from_utf8(&body).unwrap().rsplit("\r\n\r\n").nth(1).unwrap().len()+4,scramjet_rs::PROVIDER_FRAME_BYTES);
        assert!(body.len() < 131072);
        let mut padded = Vec::with_capacity(130000);
        padded.extend_from_slice(&body);
        let mut remaining = 130000 - body.len();
        while remaining > 0 {
            let size = remaining.min(scramjet_rs::PROVIDER_FRAME_BYTES);
            let size = if remaining > size && remaining - size < 3 { size - 3 } else { size };
            padded.push(b':'); padded.extend(std::iter::repeat_n(b'x',size-3)); padded.extend_from_slice(b"\n\n"); remaining -= size;
        }
        let prepared = demo::capture::prepare(&path.join("capture"),false).unwrap();
        let (endpoint, task) = server::start(padded).await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let runtime = Runtime::new_provider(endpoint, &path).unwrap();
        let base = reset();
        let started = std::time::Instant::now();
        let (mut handle, capture_receiver) = runtime.submit_provider_captured(ProviderRequest::new(&[ProviderInput::User("synthetic".into())], &[]).unwrap()).unwrap();
        let mut response = handle.result().await.unwrap();
        assert_eq!(response.outcome, Outcome::Unsupported);
        assert!(response.envelope.is_none());
        assert_eq!(response.remote_terminal.as_ref().unwrap().response_id,"resp_1");
        assert!(response.usage.as_ref().unwrap().complete());
        let history = runtime.provider_history().unwrap();
        assert_eq!(history.unknown_usage(), 0);
        assert_eq!(history.unresolved_remote(), 0);
        for item in response.failure.as_ref().unwrap().final_difference.as_ref().unwrap().items.iter().flatten() {
            let part = item.content.unwrap().first_part.unwrap();
            assert_eq!(part.fields, 2);
            assert!(part.additional_parts);
        }
        let report = demo::encode_report(&response, &history, 0).unwrap();
        let text = std::str::from_utf8(&report).unwrap();
        assert!(!text.contains("private"));
        assert!(serde_json::from_slice::<serde_json::Value>(&report).unwrap()["diagnostics_complete"].as_bool().unwrap());
        println!("eight-item-128-part-depth100-diagnostic: elapsed_us={} report_bytes={}", started.elapsed().as_micros(), report.len());
        let capture = capture_receiver.receive().await.unwrap();
        assert_eq!(capture.bytes.len(),130000);
        assert_eq!(capture.bytes.capacity(),scramjet_rs::PROVIDER_CAPTURE_BYTES);
        let pointer = capture.bytes.as_ptr() as usize;
        assert!(capture.body_eof); assert!(!capture.parser_accepted);
        tokio::task::spawn_blocking(move || {assert_eq!(capture.bytes.as_ptr() as usize,pointer);demo::capture::persist(prepared,capture,report).unwrap();}).await.unwrap();
        println!("capture-storage: raw={} report={} provenance={}",std::fs::metadata(path.join("capture/response.data")).unwrap().len(),std::fs::metadata(path.join("capture/diagnostic.json")).unwrap().len(),std::fs::metadata(path.join("capture/provenance.json")).unwrap().len());
        measure("maximum-layout-diagnostic-transport", base, 1048576);
        let replay_base = reset();
        let replay = tokio::task::spawn_blocking({let artifact=path.join("capture");move || demo::capture::replay(&artifact).unwrap()}).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&replay).unwrap()["full_stream"],true);
        measure("maximum-capture-read-replay",replay_base,1048576);
        drop(replay);
        let failure = response.failure.as_mut().unwrap();
        let difference = failure.final_difference.as_mut().unwrap();
        difference.done_count = usize::MAX;
        difference.final_count = Some(usize::MAX);
        difference.additional_items = true;
        for item in difference.items.iter_mut().flatten() {
            item.index = usize::MAX;
            item.fields = 2047;
            item.other_fields = true;
            item.done_kinds = [scramjet_rs::ProviderJsonKind::Object; 11];
            item.final_kinds = [scramjet_rs::ProviderJsonKind::Object; 11];
            let part = scramjet_rs::ProviderPartDifference { index: usize::MAX, fields: 15, done_kinds: [scramjet_rs::ProviderJsonKind::Object;4], final_kinds:[scramjet_rs::ProviderJsonKind::Object;4], other_fields:true, additional_parts:true };
            let array = scramjet_rs::ProviderArrayDifference { done_count:Some(usize::MAX), final_count:Some(usize::MAX), first_part:Some(part) };
            item.content=Some(array);item.summary=Some(array);
        }
        failure.identity_bytes=Some(usize::MAX);
        failure.identity_problem=Some(scramjet_rs::ProviderIdentityProblem::NotString);
        failure.event=Some("response.reasoning_summary_text.done");
        failure.field=Some("response.incomplete_details");
        failure.stage=scramjet_rs::ProviderFailureStage::Acknowledgement;
        response.wire_bytes=usize::MAX;
        response.outcome=Outcome::ResourceLimit;
        response.durable=false;
        response.usage=Some(scramjet_rs::ProviderUsage {
            input_tokens:Some(u64::MAX/2), output_tokens:Some(u64::MAX/2), total_tokens:Some(u64::MAX-1),
            cached_input_tokens:Some(u64::MAX/2), reasoning_tokens:Some(u64::MAX/2),
        });
        let base = reset();
        let report = demo::encode_report(&response,&history,usize::MAX).unwrap();
        println!("worst-fixed-diagnostic-report: bytes={}",report.len());
        assert!(serde_json::from_slice::<serde_json::Value>(&report).unwrap()["diagnostics_complete"].as_bool().unwrap());
        assert!(report.len() <= scramjet_rs::PROVIDER_DIAGNOSTIC_BYTES);
        measure("worst-fixed-diagnostic-report",base,65536);
        drop(response);drop(handle);
        runtime.shutdown().await;
        task.await.unwrap();
    });
}
