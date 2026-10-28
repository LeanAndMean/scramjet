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
#[path = "support/readers_fixture.rs"]
mod fixture;
#[path = "support/readers_root.rs"]
mod roots;
use scramjet_rs::{ReaderInputs, ReaderParent, RequestId, Runtime, reconstruct_readers};
#[test]
fn maximum_escaping_and_reconstruction_have_bounded_allocation_work() {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let root = roots::OwnedRoot::new();
    executor.block_on(async {
        let mut replies = fixture::golden("meter", u64::MAX);
        for (child, reply) in replies.iter_mut().enumerate() {
            reply.text = format!(
                r#"{{"session":"meter","generation":{},"child":{},"summary":"{}"}}"#,
                u64::MAX,
                child,
                "\\u0041".repeat(300)
            );
        }
        let fixture = fixture::ReadersFixture::start(replies).await;
        let runtime = Runtime::new(fixture.endpoint, 2, scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::ConcurrencyMode::Fixed,
).unwrap();
        let content = "\t".repeat(1024);
        ENABLED.store(true, Ordering::Relaxed);
        let (parent, cleanup) = ReaderParent::start(
            runtime,
            &root.path,
            ReaderInputs::new("meter", u64::MAX, [&content; 3]).unwrap(),
        )
        .unwrap();
        fixture.wait_child(0).await;
        fixture.wait_child(1).await;
        fixture.release(1);
        fixture.wait_child(2).await;
        fixture.release_all();
        assert!(fixture.peak() <= 2);
        assert!(fixture.contexts().iter().all(Option::is_some));
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        for _ in 0..3 {
            assert_eq!(
                reconstruct_readers(&root.path, "meter", u64::MAX)
                    .unwrap()
                    .children,
                run.tree.children
            );
        }
        fixture.shutdown().await;
        ENABLED.store(false, Ordering::Relaxed);
        let total = TOTAL.load(Ordering::Relaxed);
        let largest = LARGEST.load(Ordering::Relaxed);
        eprintln!("reader workload: cumulative_requested={total} largest={largest}");
        assert!(total < 2 * 1024 * 1024);
        assert!(largest <= 65536);
        assert!(run.snapshot.input_peak <= 12288);
        assert!(run.snapshot.delta_bytes_peak <= 24576);

        let cases = [
            format!(r#"{{"unknown":[{}]}}"#, "0,".repeat(990) + "0"),
            format!(r#"{{"summary":[{}]}}"#, "0,".repeat(990) + "0"),
            format!(r#"{{"unknown":{{{}}}}}"#, "\"x\":0,".repeat(240) + "\"x\":0"),
            format!(r#"{{"summary":{}0{}}}"#, "[".repeat(500), "]".repeat(500)),
            format!(r#"{{"summary":"ok",{}}}"#, "\"summary\":\"ok\",".repeat(110) + "\"child\":0"),
            format!(r#"{{"summary":"{}\\u0041"}}"#, "x".repeat(1950)),
            format!(r#"{{"{}\\u0041":0}}"#, "x".repeat(1950)),
            r#"{"summary":"ok","child":0,"generation":1,"session":"foreign"}"#.to_owned(),
            r#"["meter",1,0,"positional"]"#.to_owned(),
        ];
        let path = root.path.join(".reader-journal");
        let original = std::fs::read(&path).unwrap();
        let header_len = 4 + u32::from_le_bytes(original[..4].try_into().unwrap()) as usize;
        let inode = rustix::fs::fstat(std::fs::File::open(&path).unwrap()).unwrap().st_ino;
        let mut malformed_total = 0;
        let mut malformed_largest = 0;
        for text in cases {
            assert!(text.len() <= 2048);
            let case_root = roots::OwnedRoot::new();
            let mut replies = fixture::golden("malformed", 1);
            replies[0] = fixture::Reply::success(text.clone());
            let fixture = fixture::ReadersFixture::start(replies).await;
            let runtime = Runtime::new(fixture.endpoint, 2, scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1)).unwrap(), scramjet_rs::ConcurrencyMode::Fixed,
).unwrap();
            TOTAL.store(0, Ordering::Relaxed);
            LARGEST.store(0, Ordering::Relaxed);
            ENABLED.store(true, Ordering::Relaxed);
            let (parent, cleanup) = ReaderParent::start(runtime, &case_root.path,
                ReaderInputs::new("malformed", 1, [""; 3]).unwrap()).unwrap();
            fixture.release_all();
            let failed = parent.finish().await.unwrap();
            cleanup.wait().await.unwrap();
            assert_eq!(failed.tree.children[0].state,
                scramjet_rs::ReaderState::Failed(scramjet_rs::ReaderFailure::InvalidResponse));
            fixture.shutdown().await;
            ENABLED.store(false, Ordering::Relaxed);
            let total = TOTAL.load(Ordering::Relaxed);
            let largest = LARGEST.load(Ordering::Relaxed);
            malformed_total = malformed_total.max(total);
            malformed_largest = malformed_largest.max(largest);
            assert!(total < 2 * 1024 * 1024);
            assert!(largest <= 65536);

            let mut history = original[..header_len].to_vec();
            history.extend_from_slice(&[4, 0, 0, 0, 1, 1, 1, 0]);
            let mut terminal = vec![1, 2, 2, 0];
            terminal.extend_from_slice(&42u64.to_le_bytes());
            terminal.push(0);
            terminal.extend_from_slice(&(text.len() as u16).to_le_bytes());
            terminal.extend_from_slice(text.as_bytes());
            terminal.extend_from_slice(&[0, 0]);
            history.extend_from_slice(&(terminal.len() as u32).to_le_bytes());
            history.extend_from_slice(&terminal);
            std::fs::write(&path, &history).unwrap();
            TOTAL.store(0, Ordering::Relaxed);
            LARGEST.store(0, Ordering::Relaxed);
            ENABLED.store(true, Ordering::Relaxed);
            for _ in 0..3 {
                assert!(reconstruct_readers(&root.path, "meter", u64::MAX).is_err());
            }
            ENABLED.store(false, Ordering::Relaxed);
            let total = TOTAL.load(Ordering::Relaxed);
            let largest = LARGEST.load(Ordering::Relaxed);
            eprintln!("malformed reconstruction: wire={} cumulative_requested={total} largest={largest}", text.len());
            assert!(total < 65536);
            assert!(largest <= 8192);
            assert_eq!(std::fs::read(&path).unwrap(), history);
            assert_eq!(rustix::fs::fstat(std::fs::File::open(&path).unwrap()).unwrap().st_ino, inode);
        }
        for suffix in [vec![255; 4], vec![4, 0, 0, 0, 1, 1, 99, 0], vec![4, 0, 0]] {
            let history = [original[..header_len].to_vec(), suffix].concat();
            std::fs::write(&path, &history).unwrap();
            TOTAL.store(0, Ordering::Relaxed);
            LARGEST.store(0, Ordering::Relaxed);
            ENABLED.store(true, Ordering::Relaxed);
            assert!(reconstruct_readers(&root.path, "meter", u64::MAX).is_err());
            ENABLED.store(false, Ordering::Relaxed);
            assert!(TOTAL.load(Ordering::Relaxed) < 65536);
            assert!(LARGEST.load(Ordering::Relaxed) <= 8192);
            assert_eq!(std::fs::read(&path).unwrap(), history);
        }
        std::fs::write(&path, original).unwrap();
        assert_eq!(reconstruct_readers(&root.path, "meter", u64::MAX).unwrap().children, run.tree.children);
        eprintln!("malformed HTTP maximum: cumulative_requested={malformed_total} largest={malformed_largest}");
    });
}
