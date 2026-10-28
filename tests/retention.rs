#[path = "support/cooldown_fixture.rs"]
mod cooldown_fixture;
mod support;
use scramjet_rs::{Outcome, RequestId, Runtime, SubmitError};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

struct MeteredAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
static TEST_OWNER: std::sync::Mutex<()> = std::sync::Mutex::new(());
thread_local! { static DROP_TARGET: Cell<usize> = const { Cell::new(0) }; }
static DROP_REACHED: AtomicBool = AtomicBool::new(false);
static DROP_RELEASE: AtomicBool = AtomicBool::new(false);

fn allocated(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
    LARGEST.fetch_max(bytes, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for MeteredAllocator {
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
        if DROP_TARGET
            .try_with(|target| {
                if target.get() == layout.size() {
                    target.set(0);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
        {
            DROP_REACHED.store(true, Ordering::SeqCst);
            let start = Instant::now();
            while !DROP_RELEASE.load(Ordering::SeqCst) {
                assert!(start.elapsed() < Duration::from_secs(10));
                std::thread::yield_now();
            }
        }
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        // Count old/new overlap conservatively even if the system reallocates in place.
        allocated(bytes);
        let new = unsafe { System.realloc(pointer, layout, bytes) };
        LIVE.fetch_sub(
            if new.is_null() { bytes } else { layout.size() },
            Ordering::Relaxed,
        );
        new
    }
}

#[global_allocator]
static ALLOCATOR: MeteredAllocator = MeteredAllocator;

fn run_test(future: impl std::future::Future<Output = ()>) {
    let _owner = TEST_OWNER.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            future.await;
        });
}

#[test]
fn owner_remains_busy_until_final_channel_backing_is_freed() {
    run_test(async {
        assert_eq!(
            size_of::<usize>(),
            8,
            "pinned channel allocation probe is 64-bit"
        );
        // Tokio's pinned 64-bit mpsc blocks hold 32 values and four header words.
        for item_size in [size_of::<String>(), size_of::<scramjet_rs::Terminal>()] {
            let block_bytes = 32 * item_size + 4 * size_of::<usize>();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let runtime = Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let mut handle = runtime
                .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap();
            handle.cancel();
            assert_eq!(handle.terminal().await.unwrap().outcome, Outcome::Cancelled);
            assert!(handle.terminal().await.is_none());
            let stopped = runtime.shutdown().await;
            assert_eq!(stopped.attempts, 0);
            tokio::time::sleep(stopped.rate_full_refill).await;
            assert!(
                Runtime::new(
                    endpoint,
                    4,
                    scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                    scramjet_rs::SyntheticTokenBudget::new(
                        65_536,
                        65_536,
                        std::time::Duration::from_millis(1)
                    )
                    .unwrap(),
                    scramjet_rs::ConcurrencyMode::Fixed,
                )
                .is_err()
            );
            DROP_REACHED.store(false, Ordering::SeqCst);
            DROP_RELEASE.store(false, Ordering::SeqCst);
            let dropping = std::thread::spawn(move || {
                DROP_TARGET.with(|target| target.set(block_bytes));
                drop(handle);
                assert_eq!(
                    DROP_TARGET.with(Cell::get),
                    0,
                    "channel deallocation was not observed"
                );
            });
            let reached = tokio::time::timeout(Duration::from_secs(3), async {
                while !DROP_REACHED.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            let state = Runtime::process_snapshot();
            let replacement = Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            );
            DROP_RELEASE.store(true, Ordering::SeqCst);
            dropping.join().unwrap();
            if let Ok(runtime) = replacement {
                runtime.shutdown().await;
                panic!("replacement admitted before channel backing was freed");
            }
            reached.unwrap();
            assert_eq!((state.handles, state.active, state.unresolved), (1, 0, 0));
            Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap()
            .shutdown()
            .await;
            println!(
                "channel_block_bytes={block_bytes} owner_busy_until_deallocation=true replacement_after_drop=true"
            );
        }
    });
}

#[test]
fn live_owner_keeps_saturated_leases_until_both_channels_are_freed() {
    run_test(async {
        assert_eq!(size_of::<usize>(), 8, "pinned channel probe is 64-bit");
        for item_size in [size_of::<String>(), size_of::<scramjet_rs::Terminal>()] {
            let block_bytes = 32 * item_size + 4 * size_of::<usize>();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let runtime = Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let mut handles: Vec<_> = (0..12)
                .map(|_| {
                    runtime
                        .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                        .unwrap()
                })
                .collect();
            for handle in &handles {
                handle.cancel();
            }
            for handle in &mut handles {
                assert_eq!(handle.terminal().await.unwrap().outcome, Outcome::Cancelled);
                assert!(handle.terminal().await.is_none());
            }
            assert_eq!(runtime.snapshot().attempts, 0);
            tokio::time::sleep(runtime.snapshot().rate_full_refill).await;
            assert!(matches!(
                runtime.submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap()),
                Err(SubmitError::Full)
            ));
            let handle = handles.pop().unwrap();
            DROP_REACHED.store(false, Ordering::SeqCst);
            DROP_RELEASE.store(false, Ordering::SeqCst);
            let dropping = std::thread::spawn(move || {
                DROP_TARGET.with(|target| target.set(block_bytes));
                drop(handle);
                assert_eq!(DROP_TARGET.with(Cell::get), 0, "channel block not freed");
            });
            let reached = tokio::time::timeout(Duration::from_secs(3), async {
                while !DROP_REACHED.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            let state = runtime.snapshot();
            let submitted = runtime.submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap());
            let replacement = Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            );
            DROP_RELEASE.store(true, Ordering::SeqCst);
            dropping.join().unwrap();
            reached.unwrap();
            assert!(replacement.is_err());
            if let Ok(mut handle) = submitted {
                handle.cancel();
                handle.terminal().await.unwrap();
                drop(handle);
                drop(handles);
                runtime.shutdown().await;
                panic!(
                    "same-owner admission released before channel block {block_bytes} was freed"
                );
            }
            assert!(matches!(submitted, Err(SubmitError::Full)));
            assert_eq!((state.handles, state.active, state.unresolved), (12, 0, 0));
            assert_eq!(runtime.snapshot().handles, 11);
            let mut next = runtime
                .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap();
            assert_eq!(runtime.snapshot().handles, 12);
            next.cancel();
            assert_eq!(next.terminal().await.unwrap().outcome, Outcome::Cancelled);
            assert!(next.terminal().await.is_none());
            drop(next);
            drop(handles);
            let state = runtime.shutdown().await;
            assert_eq!(
                (
                    state.handles,
                    state.active,
                    state.unresolved,
                    state.attempts
                ),
                (0, 0, 0, 0)
            );
            tokio::time::sleep(state.rate_full_refill).await;
            Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap()
            .shutdown()
            .await;
            println!(
                "channel_block_bytes={block_bytes} live_owner_full_until_deallocation=true admission_after_drop=true"
            );
        }
    });
}

#[test]
fn maximum_retry_metadata_has_fixed_terminal_backing_and_reclaims() {
    run_test(async {
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        LARGEST.store(0, Ordering::Relaxed);
        let fixture = cooldown_fixture::Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut handles: Vec<_> = (0..4)
            .map(|i| {
                runtime
                    .submit(
                        &format!("{i}:metadata-max"),
                        scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
                    )
                    .unwrap()
            })
            .collect();
        fixture.wait_started(4).await;
        for i in 0..4 {
            fixture.release(i);
        }
        for handle in &mut handles {
            let terminal = handle.terminal().await.unwrap();
            assert_eq!(terminal.outcome, Outcome::HttpRejected(429));
            let feedback = terminal.retry_after.unwrap();
            assert_eq!(
                (feedback.valid, feedback.invalid, feedback.wait),
                (0, 14, None)
            );
            assert!(feedback.duplicate);
            fixture.wait_settled(handle.id()).await;
        }
        let receipts = fixture.receipts();
        assert_eq!(receipts.len(), 4);
        assert!(
            receipts
                .iter()
                .all(|r| r.input <= 4096 && r.received.elapsed() < Duration::from_secs(5))
        );
        assert!(
            receipts
                .iter()
                .all(|r| handles.iter().any(|h| h.id() == r.id))
        );
        let peak = PEAK.load(Ordering::Relaxed);
        let largest = LARGEST.load(Ordering::Relaxed);
        assert!(peak.saturating_sub(baseline) < 2 * 1024 * 1024);
        assert!(largest <= 131072);
        drop(handles);
        let state = runtime.shutdown().await;
        assert_eq!(
            (state.handles, state.unresolved, state.cooldown_signals),
            (0, 0, 0)
        );
        let metrics = fixture.metrics();
        fixture.shutdown().await;
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        let after = LIVE.load(Ordering::Relaxed);
        assert!(after.saturating_sub(baseline) < 16384);
        println!(
            "retry_metadata_headers=14 attempts=4 overlap={} fd_peak={} baseline={baseline} peak={peak} largest={largest} after={after} terminal_bytes={}",
            metrics.0,
            metrics.1,
            size_of::<scramjet_rs::Terminal>()
        );
    });
}

#[test]
fn allocated_backing_including_fixture_stays_bounded_and_reclaims() {
    run_test(async {
        let baseline = LIVE.load(Ordering::Relaxed);
        for scenario in [
            "slow",
            "fragmented",
            "fragmented-long",
            "coalesced-head",
            "huge-chunk",
            "wire",
            "oversized-body",
            "oversized-trailer",
        ] {
            PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
            LARGEST.store(0, Ordering::Relaxed);
            let fixture = support::Fixture::new().await;
            let runtime = Runtime::new(
                fixture.address,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let mut handles: Vec<_> = (0..4)
                .map(|_| {
                    runtime
                        .submit(scenario, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                        .unwrap()
                })
                .collect();
            fixture.release(0);
            fixture.wait_started(4).await;
            assert_eq!(fixture.started().len(), 4);
            if scenario == "slow" {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while runtime.snapshot().delta_items < 32 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                assert_eq!(runtime.snapshot().delta_bytes, 32_768);
                for handle in &handles {
                    handle.cancel();
                }
            }
            for handle in &mut handles {
                if scenario != "slow" {
                    while handle.next_delta().await.is_some() {}
                }
                let terminal = handle.terminal().await.unwrap();
                assert_eq!(
                    terminal.outcome,
                    match scenario {
                        "slow" => Outcome::Cancelled,
                        "wire" | "oversized-body" | "oversized-trailer" => Outcome::ResourceLimit,
                        _ => Outcome::Success,
                    }
                );
                fixture.wait_settled(handle.id()).await;
                if terminal.outcome != Outcome::Success {
                    Runtime::reconcile_fixture(handle.id()).unwrap();
                }
            }
            let peak = PEAK.load(Ordering::Relaxed);
            let largest = LARGEST.load(Ordering::Relaxed);
            assert!(
                peak - baseline < 2 * 1024 * 1024,
                "allocation envelope exceeded: {peak}"
            );
            assert!(largest <= 131_072, "unexpected large backing: {largest}");
            drop(handles);
            let state = runtime.shutdown().await;
            assert_eq!((state.active, state.handles, state.delta_bytes), (0, 0, 0));
            let fixture_peak = fixture.peak();
            let fd_peak = fixture.fd_peak();
            fixture.shutdown().await;
            for _ in 0..4 {
                tokio::task::yield_now().await;
            }
            let after = LIVE.load(Ordering::Relaxed);
            assert!(
                after.saturating_sub(baseline) < 16_384,
                "retained allocation after shutdown: {after} baseline={baseline}"
            );
            println!(
                "scenario={scenario} fixture_peak={fixture_peak} fd_peak={fd_peak} baseline_allocated={baseline} peak_allocated={peak} largest_allocation={largest} after_shutdown_allocated={after} (includes executor/fixture; realloc overlap conservative)"
            );
        }
    });
}
