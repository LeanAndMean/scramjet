use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static ENABLED: AtomicBool = AtomicBool::new(false);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
struct Meter;
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() && ENABLED.load(Ordering::Relaxed) {
            TOTAL.fetch_add(layout.size(), Ordering::Relaxed);
            LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, l, n) };
        if !p.is_null() && ENABLED.load(Ordering::Relaxed) {
            TOTAL.fetch_add(n, Ordering::Relaxed);
            LARGEST.fetch_max(n, Ordering::Relaxed);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;
#[allow(dead_code)]
#[path = "support/commands.rs"]
mod definitions;
#[allow(dead_code)]
#[path = "support/readers_fixture.rs"]
mod fixture;
#[path = "support/readers_root.rs"]
mod roots;
use scramjet_rs::*;
use std::time::Duration;
#[test]
fn complete_maximum_inputs_summaries_and_receipt_snapshots_are_bounded() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let definitions=definitions::Roots::new(r#"{"commands":[{"name":"summarize","outcome":"AllReaders"}]}"#,b"",b"").unwrap();
        let root=roots::OwnedRoot::new();
        let content="\t".repeat(1024);
        ENABLED.store(true,Ordering::Relaxed);
        let mut command=CommandSession::load(&definitions.definitions,&definitions.inputs,CommandPolicy::Automatic).unwrap();
        let id=command.begin_readers("summarize",[&content;3]).unwrap();
        let session=command.snapshot().invocations[0].as_ref().unwrap().readers.as_ref().unwrap().session.clone();
        let replies=std::array::from_fn(|child| fixture::Reply::success(serde_json::json!({"session": session.as_ref(), "generation":1,"child":child,"summary":"\u{a0}".repeat(256)}).to_string()));
        let fixture=fixture::ReadersFixture::start(replies).await;
        let runtime=Runtime::new(fixture.endpoint,2,RequestRate::new(4,Duration::from_millis(1)).unwrap(),SyntheticTokenBudget::new(65536,65536,Duration::from_millis(1)).unwrap(),ConcurrencyMode::Fixed).unwrap();
        let (mut adapter,settlement)=command.start_readers(id,runtime,&root.path).unwrap();
        fixture.release_all();
        let mut receipt=adapter.logical().await.unwrap();
        command.accept_readers(&mut receipt).unwrap();
        command.report(id,CommandCompletion::Completed,&"x".repeat(512),None).unwrap();
        let first=command.snapshot();
        for _ in 0..80 {
            let snapshot=command.snapshot();
            assert!(std::sync::Arc::ptr_eq(&first.invocations[0].as_ref().unwrap().readers.as_ref().unwrap().contents[0],&snapshot.invocations[0].as_ref().unwrap().readers.as_ref().unwrap().contents[0]));
            assert!(std::sync::Arc::ptr_eq(first.invocations[0].as_ref().unwrap().readers.as_ref().unwrap().execution.as_ref().unwrap(),snapshot.invocations[0].as_ref().unwrap().readers.as_ref().unwrap().execution.as_ref().unwrap()));
        }
        settlement.wait().await.unwrap();
        fixture.shutdown().await;
        let final_state=command.finalize();
        assert!(final_state.settled);
        let tree=reconstruct_readers(&root.path,&session,1).unwrap();
        assert!(tree.parent_finished);
        assert!(tree.children.iter().all(|f| matches!(&f.state,ReaderState::Completed {summary} if summary==&"\u{a0}".repeat(256))));
        ENABLED.store(false,Ordering::Relaxed);
        let total=TOTAL.load(Ordering::Relaxed); let largest=LARGEST.load(Ordering::Relaxed);
        assert!(total<2*1024*1024 && largest<=65536,"{total}/{largest}");
        eprintln!("max command reader workload: cumulative={total} largest={largest}; 3072 input bytes, 1536 summary bytes, 512 report bytes, 80 immutable snapshots; includes fixture/HTTP/storage/receipt/reconstruction, not RSS");
    });
}
