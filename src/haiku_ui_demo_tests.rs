use super::*;
use std::os::unix::fs::DirBuilderExt;

pub(super) fn large_sequence() -> Vec<(Option<serde_json::Value>, Vec<u8>)> {
    let poem = "😀".repeat(128);
    let mut sequence = fixture::sequence(&poem, 0, "opaque fixture reasoning");
    fn replace(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) if text == "resp_1.1" => *text = "😀".repeat(1600),
            serde_json::Value::String(text) if text == "call_1.1" => *text = "界".repeat(2000),
            serde_json::Value::Array(values) => {
                for v in values {
                    replace(v);
                }
            }
            serde_json::Value::Object(values) => {
                for v in values.values_mut() {
                    replace(v);
                }
            }
            _ => {}
        }
    }
    for (expected, _) in &mut sequence {
        if let Some(expected) = expected {
            replace(expected);
        }
    }
    let text = std::str::from_utf8(&sequence[0].1).unwrap();
    let mut events: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for event in &mut events {
        replace(event);
    }
    sequence[0].1 = crate::provider_demo::fixture::wire(&events, true);
    assert!(sequence[0].1.len() <= 131072);
    sequence
}

#[test]
#[ignore = "owned PTY application/meter/panic helper; invoked in an isolated fresh process"]
fn application_child() {
    let project =
        PathBuf::from(std::env::var_os("S414C_PROJECT").expect("synthetic owned project"));
    let storage =
        PathBuf::from(std::env::var_os("S414C_STORAGE").expect("synthetic owned storage"));
    let mode = std::env::var("S414C_APPLICATION_MODE").unwrap();
    crate::ui::start_haiku_meter();
    let result = execute_options(Options {
        project: (!matches!(mode.as_str(), "history" | "history-trace")).then_some(project),
        storage,
        session: "pty".into(),
        history: matches!(mode.as_str(), "history" | "history-trace"),
        panic_after_ready: mode == "host-panic",
        large_approval: matches!(mode.as_str(), "large-approval" | "denial-paint"),
        model_delay: mode == "model-delay",
    });
    crate::ui::finish_haiku_meter();
    if mode == "history-trace" {
        if let Err(error) = &result {
            assert!(
                error.contains("terminal operation exceeded 100ms"),
                "unexpected traced-history refusal: {error}"
            );
            println!("HISTORY_TRACE_PAINT_RESOURCE_REFUSAL: {error}");
        }
    } else if matches!(mode.as_str(), "golden" | "history") {
        assert!(result.is_ok(), "{result:?}");
    } else {
        assert!(result.is_err(), "{result:?}");
    }
}

#[test]
fn actual_application_composition_large_approval_model_stop_and_panic_restore() {
    let executable = std::env::current_exe().unwrap();
    let output = std::process::Command::new("timeout")
        .args(["150", "python3", "tests/haiku_ui_pty.py"])
        .arg(&executable)
        .args([
            "meter-golden",
            "meter-deny-create",
            "meter-deny-process",
            "large-approval",
            "denial-paint",
            "host-panic",
            "host-panic-close",
            "model-delay",
        ])
        .env("S414C_TEST_BINARY", &executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

#[test]
fn real_host_owners_wait_for_return_fence_on_stop_and_caught_panic() {
    let _serial = crate::RUNTIME_TEST_LOCK.blocking_lock();
    for panic_after_ready in [false, true] {
        let base = std::env::temp_dir().join(format!(
            "s414c-host-{}-{panic_after_ready}",
            std::process::id()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&base)
            .unwrap();
        let project = base.join("project");
        let storage = base.join("storage");
        for path in [&project, &storage] {
            std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        let fence = Arc::new(ReturnFence::default());
        let (sender, receiver) = mpsc::sync_channel(0);
        let host_fence = fence.clone();
        let worker = std::thread::spawn(move || {
            host(
                Options {
                    project: Some(project),
                    storage,
                    session: "test".into(),
                    history: false,
                    panic_after_ready,
                    large_approval: false,
                    model_delay: false,
                },
                sender,
                host_fence,
            )
        });
        let bridge = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        if panic_after_ready {
            bridge.stop();
        } else {
            bridge.cancellation().unwrap().cancel();
        }
        assert!(!bridge.snapshot().offered);
        std::thread::sleep(Duration::from_millis(60));
        assert!(!worker.is_finished());
        assert!(!bridge.snapshot().snapshot.worker_joined);
        assert!(!bridge.snapshot().snapshot.worker_terminated);
        assert!(!bridge.snapshot().runtime_joined);
        assert!(
            bridge
                .snapshot()
                .snapshot
                .history
                .entries
                .iter()
                .all(|e| !matches!(e.record.as_ref(), scramjet_rs::HaikuRecord::Attempt { .. }))
        );
        fence.release();
        assert!(worker.join().unwrap().is_err());
        let projection = bridge.snapshot();
        assert!(
            projection.snapshot.worker_joined
                && projection.snapshot.worker_terminated
                && projection.runtime_joined
        );
        if panic_after_ready {
            assert!(projection.error.unwrap().contains("host execution panic"));
        }
        drop(bridge);
        std::fs::remove_dir_all(base).unwrap();
    }
}

#[test]
fn fixture_options_refuse_live_conflicts_and_auto_approval() {
    for args in [
        vec!["--live"],
        vec!["--fixture", "--history", "--storage", "/synthetic"],
        vec![
            "--fixture",
            "--project",
            "/p",
            "--storage",
            "/s",
            "--approve-effects",
        ],
        vec!["--history", "--project", "/p", "--storage", "/s"],
    ] {
        assert!(options(args.into_iter().map(OsString::from)).is_err());
    }
}
