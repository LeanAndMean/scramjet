#[allow(dead_code)]
#[path = "support/project_fixture.rs"]
mod fixture;
#[path = "support/coding.rs"]
mod roots;
use scramjet_rs::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
struct Meter;
fn allocated(bytes: usize) {
    TOTAL.fetch_add(bytes, Ordering::Relaxed);
    let n = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(n, Ordering::Relaxed);
    LARGEST.fetch_max(bytes, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            allocated(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            allocated(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        allocated(n);
        let q = unsafe { System.realloc(p, l, n) };
        LIVE.fetch_sub(if q.is_null() { n } else { l.size() }, Ordering::Relaxed);
        q
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;
#[test]
fn maximum_admissible_complete_contexts_two_journeys_live_snapshots_and_recovery_have_bounded_all_thread_backing()
 {
    let root = roots::OwnedRoot::new();
    let storage = roots::OwnedRoot::new();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();
    let names = ["t".repeat(64), "a".repeat(64), "b".repeat(64)];
    let before = "\t".repeat(384) + &"n".repeat(256) + &"\t".repeat(384);
    for (name, text) in names
        .iter()
        .zip([before.as_str(), &"\t".repeat(1024), &"界".repeat(341)])
    {
        fs::write(root.path.join(name), text).unwrap();
        fs::set_permissions(root.path.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    let requests=[serde_json::json!({"operation":"replace_once","target":names[0],"supporting":[names[1],names[2]],"needle":"n".repeat(256),"replacement":"r".repeat(256)}).to_string(),serde_json::json!({"operation":"replace_once","target":names[0],"supporting":[names[1],names[2]],"needle":"r".repeat(256),"replacement":"q".repeat(256)}).to_string()];
    let oversized = "x".repeat(1024 * 1024);
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async{
        tokio::task::spawn_blocking(||()).await.unwrap();
        let baseline=LIVE.load(Ordering::Relaxed);let total=TOTAL.load(Ordering::Relaxed);PEAK.store(baseline,Ordering::Relaxed);LARGEST.store(0,Ordering::Relaxed);
        let mut longest=0;
        {
            let f=fixture::Fixture::start(fixture::Mode::SizedSummary(240)).await;
            let runtime=Runtime::new(f.endpoint,2,RequestRate::new(4,Duration::from_millis(1)).unwrap(),SyntheticTokenBudget::new(65536,65536,Duration::from_millis(1)).unwrap(),ConcurrencyMode::Fixed).unwrap();
            let (mut session,settlement)=ProjectSession::start(runtime,&root.path,&storage.path,&"s".repeat(64));
            let observer=session.observer();let control=session.control();
            for (index,request) in requests.iter().enumerate(){
                let decide=async{loop{if let Some(coding)=&observer.snapshot().journeys[index].coding && let Some(p)=&coding.proposal{control.decide(p,CodingDecision::Approve).unwrap();break;}tokio::time::sleep(Duration::from_millis(1)).await;}};
                let (receipt,())=tokio::join!(session.submit(request),decide);let mut receipt=receipt.unwrap();
                let summary=observer.snapshot().journeys[index].coding.as_ref().unwrap().final_summary.as_deref().unwrap().to_owned();
                let handle=session.report_source(&mut receipt,CommandCompletion::Completed,&summary).await.unwrap().unwrap();
                let ready=session.resume(handle).await.unwrap();let mut receipt=session.verify(ready).await.unwrap();
                session.report_verification(&mut receipt,CommandCompletion::Completed,"full metadata/content matched").await.unwrap();
            }
            for _ in 0..32 { assert!(session.submit(&oversized).await.is_err()); }
            let snapshots:Vec<_>=(0..80).map(|_|observer.snapshot()).collect();
            assert!(snapshots.iter().all(|s| std::sync::Arc::ptr_eq(&snapshots[0].records[0],&s.records[0])));
            for request in f.requests(){longest=longest.max(request.input.len());assert!(request.input.len()<=4096);}
            let finish=settlement.finish().await.unwrap();assert!(finish.logical_settled&&finish.runtime_joined&&finish.storage_joined&&finish.history_acknowledged);assert_eq!(finish.runtime.attempts,14);
            let report=f.shutdown().await;assert!(report.joined&&report.peak==2);report.attest(&observer.snapshot()).unwrap();
            for _ in 0..30{let history=reconstruct_project_history(&storage.path,&"s".repeat(64)).unwrap();assert!(history.closure_recorded);assert_eq!(history.coding.iter().flatten().count(),2);}
            let path=storage.path.join(".project-journal"); let original=fs::read(&path).unwrap();
            fs::write(&path,b"invalid\n").unwrap();
            for _ in 0..32 { assert!(reconstruct_project_history(&storage.path,&"s".repeat(64)).is_err()); }
            assert_eq!(fs::read(&path).unwrap(),b"invalid\n"); fs::write(path,original).unwrap();
            drop(snapshots);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        let peak=PEAK.load(Ordering::Relaxed).saturating_sub(baseline);let cumulative=TOTAL.load(Ordering::Relaxed)-total;let largest=LARGEST.load(Ordering::Relaxed);let residual=LIVE.load(Ordering::Relaxed).saturating_sub(baseline);
        eprintln!("all-thread retained project: peak={peak} cumulative={cumulative} largest={largest} residual={residual}; longest_complete_context={longest}; two journeys, 14 actual attempts, 80 immutable snapshots, 30 reconstructions, 32 oversized borrowed requests and 32 malformed journal rejections; allocator backing, not RSS/backend capacity");
        assert!(peak<=1024*1024&&cumulative<=24*1024*1024&&largest<=131072&&residual<=8192);
        assert!(longest>3600&&longest<=4096);
    });
}
