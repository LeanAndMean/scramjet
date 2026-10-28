#[path = "support/coding.rs"]
mod support;
use scramjet_rs::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
struct Meter;
fn allocated(bytes: usize) {
    TOTAL.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
    LARGEST.fetch_max(bytes, Ordering::Relaxed);
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
        let new = unsafe { System.realloc(pointer, layout, bytes) };
        LIVE.fetch_sub(
            if new.is_null() { bytes } else { layout.size() },
            Ordering::Relaxed,
        );
        new
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;
fn start() -> (usize, usize) {
    let live = LIVE.load(Ordering::Relaxed);
    PEAK.store(live, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    (live, TOTAL.load(Ordering::Relaxed))
}
fn stop(label: &str, baseline: (usize, usize), cumulative_ceiling: usize) {
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline.0);
    let residual = LIVE.load(Ordering::Relaxed).saturating_sub(baseline.0);
    let cumulative = TOTAL.load(Ordering::Relaxed) - baseline.1;
    let largest = LARGEST.load(Ordering::Relaxed);
    eprintln!("{label}: peak={peak} cumulative={cumulative} largest={largest} residual={residual}");
    assert!(
        peak <= 49152 && largest <= 16384 && residual <= 1024,
        "{label}"
    );
    assert!(cumulative <= cumulative_ceiling, "{label}");
}

#[test]
fn maximum_capture_two_exact_edits_recovery_and_malformed_requests_have_fixed_backing_work() {
    let root = support::OwnedRoot::new();
    let storage = support::OwnedRoot::new();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();
    let names = ["t".repeat(64), "a".repeat(64), "b".repeat(64)];
    let before = "h".repeat(384) + &"n".repeat(256) + &"f".repeat(384);
    for (name, contents) in
        names
            .iter()
            .zip([before.as_str(), &"\t".repeat(1024), &"é".repeat(512)])
    {
        fs::write(root.path.join(name), contents).unwrap();
        fs::set_permissions(root.path.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    let requests = [
        serde_json::json!({"operation":"replace_once", "target":names[0], "supporting":[names[1],names[2]], "needle":"n".repeat(256), "replacement":"r".repeat(256)}).to_string(),
        serde_json::json!({"operation":"replace_once", "target":names[0], "supporting":[names[1],names[2]], "needle":"r".repeat(256), "replacement":"q".repeat(256)}).to_string(),
        serde_json::json!({"operation":"replace_once", "target":names[0], "supporting":[names[1],names[2]], "needle":"q".repeat(256), "replacement":"z".repeat(256)}).to_string(),
    ];
    assert!(requests.iter().all(|s| s.len() <= PROJECT_REQUEST_BYTES));
    let warmup = ProjectWorkspace::open(&root.path, &storage.path).unwrap();
    drop(warmup);
    let sessions = ["s".repeat(64), "j".repeat(64)];
    let baseline = start();
    {
        let workspace = ProjectWorkspace::open(&root.path, &storage.path).unwrap();
        for journey in 1..=2 {
            let request = ProjectRequest::parse(&requests[journey - 1]).unwrap();
            let capture = workspace.capture(request).unwrap();
            let expected = capture.files()[0].content.clone();
            let desired = capture.desired_target().to_owned();
            let mut coding = workspace
                .coding_session(capture, &sessions[journey - 1], journey as u8)
                .unwrap();
            let intent = coding.intent(&names[0], &expected, &desired).unwrap();
            let approval = coding.approve_exact(&intent).unwrap();
            coding
                .authorize(intent, approval, &EditCancellation::default())
                .unwrap();
            assert_eq!(
                coding.dispatch(&EditCancellation::default()).unwrap().state,
                OperationState::Completed
            );
            assert_eq!(coding.read(&names[0]).unwrap(), desired);
            drop(coding);
            for _ in 0..20 {
                let history = ProjectWorkspace::coding_history(
                    &storage.path,
                    journey as u8,
                    &sessions[journey - 1],
                    workspace.root_identity(),
                )
                .unwrap();
                assert_eq!(history.operations()[0].state, OperationState::Completed);
            }
        }
        let third = workspace
            .capture(ProjectRequest::parse(&requests[2]).unwrap())
            .unwrap();
        assert!(workspace.coding_session(third, "third", 3).is_err());
    }
    stop(
        "maximum filesystem foundation: two edits + 40 recoveries",
        baseline,
        1_572_864,
    );
    let malformed = [
        format!(r#"{{"unknown":[{}]}}"#, "0,".repeat(400) + "0"),
        format!(r#"{{"target":"{}"}}"#, "\\u0041".repeat(150)),
        format!(
            r#"{{"supporting":[{}0{}]}}"#,
            "[".repeat(200),
            "]".repeat(200)
        ),
        r#"["replace_once","t",["a","b"],"n","r"]"#.into(),
        "x".repeat(1024 * 1024),
    ];
    for json in malformed {
        let baseline = start();
        for _ in 0..32 {
            assert!(ProjectRequest::parse(&json).is_err());
        }
        stop(
            "32 malformed/oversized borrowed requests",
            baseline,
            32 * 1024,
        );
    }
}
