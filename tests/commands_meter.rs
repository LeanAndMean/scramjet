#[path = "support/commands.rs"]
mod fixture;
use fixture::{AUTOMATIC_CATALOG, Roots};
use scramjet_rs::*;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
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
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, n: usize) -> *mut u8 {
        let p = unsafe { System.realloc(p, layout, n) };
        if !p.is_null() && ENABLED.load(Ordering::Relaxed) {
            TOTAL.fetch_add(n, Ordering::Relaxed);
            LARGEST.fetch_max(n, Ordering::Relaxed);
        }
        p
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;
fn start() {
    TOTAL.store(0, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::Relaxed);
}
fn stop(label: &str, total_ceiling: usize, largest_ceiling: usize) {
    ENABLED.store(false, Ordering::Relaxed);
    let total = TOTAL.load(Ordering::Relaxed);
    let largest = LARGEST.load(Ordering::Relaxed);
    eprintln!("{label}: cumulative_requested={total} largest={largest}");
    assert!(total <= total_ceiling);
    assert!(largest <= largest_ceiling);
}

#[test]
fn maximal_valid_and_malformed_catalogs_and_oversized_caller_inputs_have_bounded_backing_work() {
    let catalog = AUTOMATIC_CATALOG
        .replace(
            "\"Readable\",\"steps\":[{\"read\":\"left.txt\"}]",
            "\"Equal\",\"steps\":[{\"read\":\"left.txt\"},{\"read\":\"right.txt\"}]",
        )
        .replace("compare", &"c".repeat(32))
        .replace("inspect", &"i".repeat(32))
        .replace("left.txt", &"l".repeat(64))
        .replace("right.txt", &"r".repeat(64));
    let roots = Roots::new(&catalog, b"", b"").unwrap();
    std::fs::write(roots.inputs.join("l".repeat(64)), vec![b'x'; 1024]).unwrap();
    std::fs::write(roots.inputs.join("r".repeat(64)), vec![b'x'; 1024]).unwrap();
    let primary_name = "c".repeat(32);
    let companion_name = "i".repeat(32);
    let summary = "s".repeat(256);
    start();
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).unwrap();
    let id = session.begin(&primary_name).unwrap();
    session.execute(id).unwrap();
    session
        .report(
            id,
            CommandCompletion::Completed,
            &summary,
            Some(&companion_name),
        )
        .unwrap();
    let handle = session.paused_continuation().unwrap();
    let companion = session.resume(handle).unwrap().unwrap();
    session.execute(companion).unwrap();
    session
        .report(companion, CommandCompletion::Completed, &summary, None)
        .unwrap();
    for _ in 0..3 {
        assert_eq!(session.snapshot().retained_bytes, 4096);
    }
    let state = session.finalize();
    assert_eq!(state.reads, 4);
    assert_eq!(state.retained_text_bytes, 512);
    drop(state);
    stop(
        "maximal valid four-read session + three snapshots",
        65536,
        16384,
    );

    let cases = [
        format!(r#"{{"unknown":[{}]}}"#, "0,".repeat(1900) + "0"),
        format!(
            r#"{{"commands":[{}0{}]}}"#,
            "[".repeat(1000),
            "]".repeat(1000)
        ),
        format!(r#"{{"commands":[{{"name":"{}"}}]}}"#, "\\u0041".repeat(650)),
        format!(r#"{{"{}":0}}"#, "\\u0041".repeat(650)),
        format!(
            r#"{{"commands":[{{"name":"compare",{}}}]}}"#,
            "\"name\":\"compare\",".repeat(100) + "\"outcome\":\"Equal\""
        ),
        format!(
            r#"{{"commands":[{{"name":"c","outcome":"Equal","steps":[{{"read":"left.txt"}},{{"read":"right.txt"}},{}]}}]}}"#,
            "[".repeat(1000) + "0" + &"]".repeat(1000)
        ),
        r#"[["compare","Equal",[["left.txt"],["right.txt"]]]]"#.into(),
    ];
    for text in cases {
        assert!(text.len() <= COMMAND_CATALOG_BYTES);
        std::fs::write(roots.definitions.join("commands.json"), &text).unwrap();
        start();
        assert!(
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic)
                .is_err()
        );
        stop("malformed bounded catalog", 65536, 16384);
    }
    std::fs::write(roots.definitions.join("commands.json"), &catalog).unwrap();
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic).unwrap();
    let id = session.begin(&primary_name).unwrap();
    session.execute(id).unwrap();
    start();
    assert!(session.resume(handle).is_err());
    stop("foreign or closed-source resume handle", 0, 0);
    let oversized = "x".repeat(1024 * 1024);
    let before = session.snapshot();
    start();
    assert!(session.discover(&oversized).is_err());
    assert!(session.begin(&oversized).is_err());
    assert!(
        session
            .report(id, CommandCompletion::Completed, &oversized, None)
            .is_err()
    );
    assert!(
        session
            .report(id, CommandCompletion::Completed, "ok", Some(&oversized))
            .is_err()
    );
    stop("oversized borrowed caller fields", 0, 0);
    assert_eq!(session.snapshot(), before);
}
