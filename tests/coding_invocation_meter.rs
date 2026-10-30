#[path = "support/coding_invocation.rs"]
#[allow(dead_code)]
mod support;
use scramjet_rs::{CodingInvocation, CodingInvocationOptions};
use serde_json::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};
use support::{Dirs, server};
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
        let result = unsafe { System.realloc(p, layout, bytes) };
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
#[test]
fn complete_export_import_and_adversarial_import_have_bounded_requested_heap() {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let poem = "\0".repeat(512);
    let script = "import sys\nwith open('input.md','rb') as source:\n sys.stdout.buffer.write(source.read())\n";
    let prompts = [
        "Create the selected file and finish this message.".to_owned(),
        "Read the file, create your own Python reader, and execute it.".to_owned(),
    ];
    dirs.input("prompt-1", prompts[0].as_bytes());
    dirs.input("prompt-2", prompts[1].as_bytes());
    let calls = vec![
        Some(("write", json!({"path":names[0],"content":poem}))),
        None,
        Some(("read", json!({"path":names[0]}))),
        Some(("write", json!({"path":names[1],"content":script}))),
        Some(("python", json!({"path":names[1]}))),
        None,
    ];
    let (endpoint, server) = server(
        calls,
        prompts,
        2,
        poem,
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    for case in ["first", "second"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "meter_child", "--nocapture"])
            .env("SCRAMJET_METER_CASE", case)
            .env("SCRAMJET_METER_BASE", &dirs.base)
            .env("SCRAMJET_METER_ENDPOINT", endpoint.to_string())
            .status()
            .unwrap();
        assert!(status.success(), "{case}");
    }
    server.join().unwrap();
    let bad = format!("[{}null]", "null,".repeat(26000));
    dirs.input("bad-record", bad.as_bytes());
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "meter_child", "--nocapture"])
        .env("SCRAMJET_METER_CASE", "bad")
        .env("SCRAMJET_METER_BASE", &dirs.base)
        .env("SCRAMJET_METER_ENDPOINT", endpoint.to_string())
        .status()
        .unwrap();
    assert!(status.success());
}
#[test]
fn meter_child() {
    let Ok(case) = std::env::var("SCRAMJET_METER_CASE") else {
        return;
    };
    let base = std::path::PathBuf::from(std::env::var_os("SCRAMJET_METER_BASE").unwrap());
    let endpoint = std::env::var("SCRAMJET_METER_ENDPOINT")
        .unwrap()
        .parse()
        .unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    executor.block_on(async {
        let prompt = if case == "first" {
            "prompt-1"
        } else {
            "prompt-2"
        };
        let prior = match case.as_str() {
            "first" => None,
            "second" => Some(fs::read(base.join("record-1")).unwrap()),
            _ => Some(fs::read(base.join("bad-record")).unwrap()),
        };
        let options = CodingInvocationOptions {
            project: base.join("project"),
            storage: base.join("storage"),
            run: "ordinary-run".into(),
            files: ["input.md".into(), "reader.py".into()],
            prompt: String::from_utf8(fs::read(base.join(prompt)).unwrap()).unwrap(),
            previous: prior,
            endpoint,
        };
        let started = CodingInvocation::start(options, |intent, _, _| {
            eprintln!("proposal {}", serde_json::to_string(intent).unwrap());
            Ok(true)
        });
        if case == "bad" {
            assert!(started.is_err());
        } else {
            let (invocation, runtime) = started.unwrap();
            let import_peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
            assert!(import_peak <= 4_194_304, "{case} import: {import_peak}");
            PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
            let result = invocation.run(runtime).await.unwrap();
            assert!(result.success, "{:?}", result.failure);
            let (mut reader, output) = std::os::unix::net::UnixStream::pair().unwrap();
            let collector = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut reader, &mut bytes).unwrap();
                bytes
            });
            result.deliver_fd(&output).unwrap();
            drop(output);
            let record = collector.join().unwrap();
            let running_peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
            assert!(
                running_peak <= 2_097_152,
                "{case} running/delivery: {running_peak}"
            );
            println!("{case} import_peak={import_peak} running_delivery_peak={running_peak}");
            fs::write(
                base.join(if case == "first" {
                    "record-1"
                } else {
                    "record-2"
                }),
                record,
            )
            .unwrap();
        }
    });
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    let largest = LARGEST.load(Ordering::Relaxed);
    let ceiling = if case == "first" {
        2_097_152
    } else {
        4_194_304
    };
    println!(
        "invocation {case}: all-thread incremental requested peak={peak} largest={largest}; client/export/import, not RSS or child AS"
    );
    assert!(peak <= ceiling, "{case}: {peak}");
}
