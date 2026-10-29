use super::*;
use std::os::unix::fs::DirBuilderExt;

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
                },
                sender,
                host_fence,
            )
        });
        let bridge = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        bridge.stop();
        std::thread::sleep(Duration::from_millis(40));
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
