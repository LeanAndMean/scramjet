use super::*;

#[test]
#[ignore = "owned PTY presentation panic helper"]
fn panic_child() {
    if std::env::var_os("S413B_UI_PANIC").is_none() {
        return;
    }
    let args = [
        OsString::from("--project"),
        std::env::var_os("S413B_PROJECT").unwrap(),
        OsString::from("--storage"),
        std::env::var_os("S413B_STORAGE").unwrap(),
        OsString::from("--session"),
        OsString::from("pty"),
    ];
    assert!(execute(args.into_iter()).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn presentation_panic_restores_before_gated_stage_settlement() {
    let output = std::process::Command::new("timeout")
        .args([
            "25",
            "python3",
            "tests/project_ui_pty.py",
            "unused",
            "panic",
        ])
        .env("S413B_TEST_BINARY", std::env::current_exe().unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);
const REQUEST: &str = r#"{"operation":"replace_once","target":"sample.txt","supporting":["guide.txt","notes.txt"],"needle":"old","replacement":"new"}"#;
struct Roots {
    base: PathBuf,
    project: PathBuf,
    storage: PathBuf,
}
impl Roots {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "s413b-unit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&base).unwrap();
        let project = base.join("project");
        let storage = base.join("storage");
        std::fs::create_dir(&project).unwrap();
        std::fs::create_dir(&storage).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&storage, std::fs::Permissions::from_mode(0o700)).unwrap();
        for (name, text) in [
            ("sample.txt", "before old after\n"),
            ("guide.txt", "guide\n"),
            ("notes.txt", "notes\n"),
        ] {
            std::fs::write(project.join(name), text).unwrap();
        }
        Self {
            base,
            project,
            storage,
        }
    }
}
impl Drop for Roots {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}
async fn await_offer(bridge: &Bridge, journey: u8, intent: Intent) -> Offer {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let p = bridge.snapshot();
            assert!(p.error.is_none(), "{:?}", p.error);
            if let Some(offer) = p
                .offer
                .filter(|o| o.journey == journey && o.intent == intent)
            {
                return offer;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn consumer_two_requests_actual_reports_history_fresh_intent_and_one_runtime() {
    let _lock = crate::RUNTIME_TEST_LOCK.lock().await;
    let roots = Roots::new();
    let options = Options {
        project: roots.project.clone(),
        storage: roots.storage.clone(),
        session: "ui-consumer".into(),
        mode: Mode::Golden,
    };
    let (ready, receive) = std::sync::mpsc::sync_channel(1);
    let (returned, notice) = oneshot::channel();
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(host(options, ready, notice))
    });
    let bridge = receive.recv().unwrap();
    let mut old_verify = None;
    for journey in 1..=2 {
        let mut offer = await_offer(&bridge, journey, Intent::Request).await;
        offer.revision = 7;
        bridge.invoke(Some(offer), 7, REQUEST);
        assert!(matches!(
            bridge.snapshot().rejection,
            Some("StaleIntentOrRevision")
        ));
        bridge.painted(Some(offer));
        bridge.invoke(Some(offer), 8, REQUEST);
        assert!(matches!(
            bridge.snapshot().rejection,
            Some("StaleIntentOrRevision")
        ));
        bridge.invoke(Some(offer), 7, "unsupported");
        assert!(matches!(
            bridge.snapshot().rejection,
            Some("UnsupportedRequest")
        ));
        let request = if journey == 1 {
            REQUEST.to_owned()
        } else {
            REQUEST
                .replace("\"old\"", "\"new\"")
                .replace("\"replacement\":\"new\"", "\"replacement\":\"final\"")
        };
        bridge.invoke(Some(offer), 7, &request);
        bridge.invoke(Some(offer), 7, &request);
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let p = bridge.snapshot();
                if let Some(c) = p.snapshot.journeys[journey as usize - 1].coding.as_ref()
                    && let Some(proposal) = c.proposal.as_ref()
                {
                    bridge.decide(proposal, CodingDecision::Approve);
                    break;
                }
                assert!(p.error.is_none(), "{:?}", p.error);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        let verify = await_offer(&bridge, journey, Intent::Verify).await;
        let p = bridge.snapshot();
        assert_eq!(p.snapshot.phase, ProjectPhase::Paused);
        assert!(p.history.as_ref().unwrap().coding[journey as usize - 1].is_some());
        assert!(!p.snapshot.records.iter().any(
            |r| matches!(r.as_ref(),ProjectRecord::VerificationRead(r) if r.journey==journey)
        ));
        if let Some(old) = old_verify {
            bridge.painted(Some(old));
            bridge.invoke(Some(old), 0, "");
            assert_eq!(bridge.snapshot().snapshot.phase, ProjectPhase::Paused);
        }
        bridge.painted(Some(verify));
        bridge.invoke(Some(verify), 99, "");
        old_verify = Some(verify);
        tokio::time::timeout(Duration::from_secs(10), async {
            while bridge.snapshot().snapshot.phase != ProjectPhase::Verified {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        if journey == 1 {
            crate::ui::check_project_history(&p, &bridge.snapshot());
        }
    }
    assert!(bridge.snapshot().offer.is_none());
    assert_eq!(
        std::fs::read_to_string(roots.project.join("sample.txt")).unwrap(),
        "before final after\n"
    );
    bridge.stop();
    bridge.release_host();
    let _ = returned.send(());
    let receipt = worker.join().unwrap().unwrap();
    assert!(
        receipt.contains("attempts=14 fixture_requests=14"),
        "{receipt}"
    );
    assert!(receipt.contains("runtime=true history=true"));
    assert!(
        receipt.contains(
            "unknown_after_fixture=0 exposure_after_fixture=0 handles=0 active=0 pending=0"
        )
    );
    assert!(
        reconstruct_project_history(&roots.storage, "ui-consumer")
            .unwrap()
            .closure_recorded
    );
}
#[tokio::test]
async fn consumer_stop_rejects_painted_request_without_dispatch() {
    let _lock = crate::RUNTIME_TEST_LOCK.lock().await;
    let roots = Roots::new();
    let (ready, receive) = std::sync::mpsc::sync_channel(1);
    let (returned, notice) = oneshot::channel();
    let options = Options {
        project: roots.project.clone(),
        storage: roots.storage.clone(),
        session: "ui-stop".into(),
        mode: Mode::Golden,
    };
    let worker = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(host(options, ready, notice))
    });
    let bridge = receive.recv().unwrap();
    let offer = await_offer(&bridge, 1, Intent::Request).await;
    bridge.painted(Some(offer));
    bridge.stop();
    bridge.invoke(Some(offer), 0, REQUEST);
    bridge.release_host();
    let _ = returned.send(());
    let receipt = worker.join().unwrap().unwrap();
    assert!(
        receipt.contains("attempts=0 fixture_requests=0"),
        "{receipt}"
    );
    assert_eq!(
        std::fs::read_to_string(roots.project.join("sample.txt")).unwrap(),
        "before old after\n"
    );
}
