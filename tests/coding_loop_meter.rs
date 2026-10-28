use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static ENABLED: AtomicBool = AtomicBool::new(false);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
struct Meter;
fn add(bytes: usize) {
    if ENABLED.load(Ordering::Relaxed) {
        TOTAL.fetch_add(bytes, Ordering::Relaxed);
        LARGEST.fetch_max(bytes, Ordering::Relaxed);
    }
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            add(layout.size());
        }
        result
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        let result = unsafe { System.realloc(ptr, layout, bytes) };
        if !result.is_null() {
            add(bytes);
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

#[path = "support/coding_fixture.rs"]
mod fixture;
#[allow(dead_code)]
#[path = "support/coding_ui_fixture.rs"]
mod follow_fixture;
#[path = "support/coding.rs"]
mod roots;
use scramjet_rs::{
    CodingAgent, CodingCapability, CodingSession, EditCancellation, RequestId, Runtime,
    reconstruct_coding, reconstruct_coding_history,
};

#[test]
fn saturated_large_fragmented_and_escaped_material_work_has_bounded_allocation_requests() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let oversized = "x".repeat(1_048_576);
    TOTAL.store(0, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    assert!(CodingAgent::validate_prompt("meter-loop", &oversized).is_err());
    ENABLED.store(false, Ordering::Relaxed);
    assert_eq!(TOTAL.load(Ordering::Relaxed), 0);
    executor.block_on(async {
        let root = roots::OwnedRoot::new();
        std::fs::write(root.path.join("sample.txt"), "\n".repeat(1024)).unwrap();
        let session = CodingSession::create(&root.path, "meter-loop", CodingCapability::ReadOnly).unwrap();
        assert_eq!(fixture::golden().len(), 4);
        let replies = (1..=8).map(|op| {
            let base = format!(r#"{{"kind":"Read","op":{op},"target":"sample.txt"}}"#);
            fixture::Reply::success(&format!("{}{}", base, "\t".repeat(16_384 - base.len())))
        }).collect();
        let fixture = fixture::ModelFixture::start(replies).await;
        let runtime = Runtime::new(fixture.endpoint, 1, scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::ConcurrencyMode::Fixed,
).unwrap();
        let agent = CodingAgent::start(session, None, EditCancellation::default(), &"x".repeat(1024)).unwrap();
        TOTAL.store(0, Ordering::Relaxed); LARGEST.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        assert!(agent.run(&runtime).await.is_err());
        let attempts = runtime.snapshot().attempts;
        runtime.shutdown().await;
        fixture.settle(attempts).await;
        let requests = fixture.requests();
        fixture.stop().await;
        for _ in 0..3 { assert!(reconstruct_coding(&root.path, "meter-loop").unwrap().is_empty()); }
        ENABLED.store(false, Ordering::Relaxed);
        let total = TOTAL.load(Ordering::Relaxed);
        let largest = LARGEST.load(Ordering::Relaxed);
        println!("coding saturated allocation work: total_requested={total} largest={largest} attempts={attempts}");
        assert!(attempts <= 8 && requests.len() == attempts);
        for (index, text) in requests.iter().enumerate() {
            assert!(text.len() <= 4096);
            let context: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(context["prompt"], "x".repeat(1024));
            assert_eq!(context["previous"], if index == 0 { String::new() } else { "\n".repeat(1024) });
        }
        assert!(total < 8_388_608);
        assert!(largest <= 65_536);
        let journal_path = root.path.join(".coding-journal");
        let original = std::fs::read_to_string(&journal_path).unwrap();
        let mut maximal = format!("{}\n",original.lines().next().unwrap());
        let mut sequence = 1;
        for turn in 1..=7 {
            let action = serde_json::json!({"kind":"Read","op":turn,"target":"sample.txt"}).to_string();
            let response = format!("{}{}",action,"\t".repeat(12288-action.len()));
            let mut entries = vec![(0,0,"Request","Pending","\x7f".repeat(4096))];
            entries.extend(response.as_bytes().chunks(2048).map(|p| (turn,0,"ResponsePart","Pending",std::str::from_utf8(p).unwrap().into())));
            entries.push((turn,0,"Response","Success",String::new()));
            entries.push((turn,turn,"Tool","Read","\u{202e}".repeat(341)));
            for (request,operation,event,status,text) in entries {
                let entry = serde_json::json!({"turn":turn,"request":request,"operation":operation,"event":event,"status":status,"text":text});
                maximal.push_str(&serde_json::json!({"sequence":sequence,"record":{"kind":"Material","entry":entry}}).to_string());
                maximal.push('\n'); sequence += 1;
            }
        }
        assert_eq!(sequence,64);
        std::fs::write(&journal_path,&maximal).unwrap();
        TOTAL.store(0, Ordering::Relaxed); LARGEST.store(0, Ordering::Relaxed);
        ENABLED.store(true, Ordering::Relaxed);
        let mut retained = 0;
        for _ in 0..3 {
            let h = std::sync::Arc::new(reconstruct_coding_history(&root.path, "meter-loop").unwrap());
            retained = retained.max(h.payload_bytes());
            for _ in 0..1000 { assert!(std::sync::Arc::ptr_eq(&h, &h.clone())); }
        }
        ENABLED.store(false, Ordering::Relaxed);
        println!("history loading: payload={retained} total_requested={} largest={}", TOTAL.load(Ordering::Relaxed), LARGEST.load(Ordering::Relaxed));
        assert!(TOTAL.load(Ordering::Relaxed) <= 2_097_152 && LARGEST.load(Ordering::Relaxed) <= 65_536);
        assert_eq!(std::fs::read_to_string(&journal_path).unwrap(), maximal);
        let malformed = format!("{}{{\"sequence\":1,\"record\":{{\"kind\":\"Material\",\"entry\":{{\"turn\":1,\"request\":0,\"operation\":0,\"event\":\"Request\",\"status\":\"Pending\",\"text\":\"{}\"}}}}}}\n",format_args!("{}\n",original.lines().next().unwrap()), "x".repeat(500_000));
        std::fs::write(&journal_path,&malformed).unwrap();
        TOTAL.store(0, Ordering::Relaxed); LARGEST.store(0, Ordering::Relaxed); ENABLED.store(true, Ordering::Relaxed);
        assert!(reconstruct_coding_history(&root.path,"meter-loop").is_err());
        ENABLED.store(false, Ordering::Relaxed);
        println!("malformed history loading: total_requested={} largest={}",TOTAL.load(Ordering::Relaxed),LARGEST.load(Ordering::Relaxed));
        assert!(TOTAL.load(Ordering::Relaxed) <= 65536 && LARGEST.load(Ordering::Relaxed) <= 16384);
        std::fs::write(&journal_path,&original).unwrap();
        let journal = std::fs::read(root.path.join(".coding-journal")).unwrap();
        assert!(journal.len() <= 1_048_576);
        assert!(journal.split(|b| *b == b'\n').all(|r| r.len() < 16_384));
        follow_up_work_meter(&oversized).await;
    });
}

async fn follow_up_work_meter(oversized: &str) {
    use scramjet_rs::{CodingDecision, ConcurrencyMode, RequestRate, SyntheticTokenBudget};
    use std::{sync::Arc, time::Duration};
    let root = roots::OwnedRoot::new();
    let fixture = follow_fixture::Fixture::consuming_follow_up(Arc::from("meter-follow"))
        .await
        .unwrap();
    let runtime = Runtime::new(
        fixture.address,
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap();
    let session =
        CodingSession::create(&root.path, "meter-follow", CodingCapability::ControlledEdit)
            .unwrap();
    let first = "x".repeat(1024);
    let mut agent =
        CodingAgent::start_follow_up(session, EditCancellation::default(), &first).unwrap();
    TOTAL.store(0, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    for invocation in 1..=2 {
        let control = agent.control();
        let observer = agent.observer();
        let approve = async {
            loop {
                if let Some(p) = &observer.snapshot().proposal
                    && p.invocation == invocation
                {
                    control.decide(p, CodingDecision::Approve).unwrap();
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(agent.run_unjoined(&runtime), approve).0
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.turns, invocation * 4);
        if invocation == 1 {
            tokio::time::timeout(Duration::from_secs(5), fixture.checkpoint())
                .await
                .unwrap()
                .unwrap();
            let before = TOTAL.load(Ordering::Relaxed);
            assert!(agent.validate_follow_up(oversized).is_err());
            assert_eq!(TOTAL.load(Ordering::Relaxed), before);
            agent
                .follow_up(&runtime, "second 界\ncomplete prompt")
                .await
                .unwrap();
        }
    }
    let observer = agent.observer();
    agent.finish().unwrap();
    runtime.shutdown().await;
    let report = fixture.shutdown().await.unwrap();
    follow_fixture::reconcile(&observer.snapshot(), &report).unwrap();
    let total = TOTAL.load(Ordering::Relaxed);
    let largest = LARGEST.load(Ordering::Relaxed);
    ENABLED.store(false, Ordering::Relaxed);
    assert_eq!(report.accepts, 8);
    assert!(report.supervisor_joined.iter().all(|v| *v));
    println!("same-owner follow-up work: total_requested={total} largest={largest} attempts=8");
    assert!(total <= 2_097_152 && largest <= 65_536);
    TOTAL.store(0, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
    let h = Arc::new(reconstruct_coding_history(&root.path, "meter-follow").unwrap());
    for _ in 0..1000 {
        assert!(Arc::ptr_eq(&h, &h.clone()));
    }
    ENABLED.store(false, Ordering::Relaxed);
    println!(
        "same-owner history: records={} payload={} total_requested={} largest={}",
        h.records().len(),
        h.payload_bytes(),
        TOTAL.load(Ordering::Relaxed),
        LARGEST.load(Ordering::Relaxed)
    );
    assert!(h.records().len() < 64 && h.payload_bytes() <= 524_288);
    assert!(TOTAL.load(Ordering::Relaxed) <= 262_144 && LARGEST.load(Ordering::Relaxed) <= 65_536);
}
