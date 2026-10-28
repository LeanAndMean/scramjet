use super::*;
use crate::*;
#[allow(dead_code)]
#[path = "../../tests/support/project_fixture.rs"]
mod fixture;
use crate::coding::tests::support as roots;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::atomic::AtomicBool,
    time::Instant,
};
const BEFORE: &str = "head\nold fragment\ntail\n";
fn root() -> roots::OwnedRoot {
    let root = roots::OwnedRoot::new();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.path.join("sample.txt"), BEFORE).unwrap();
    for name in ["rules.txt", "notes.txt"] {
        fs::write(root.path.join(name), format!("complete support {name}\n")).unwrap();
    }
    for name in ["sample.txt", "rules.txt", "notes.txt"] {
        fs::set_permissions(root.path.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    root
}
fn request() -> String {
    serde_json::json!({"operation":"replace_once","target":"sample.txt","supporting":["rules.txt","notes.txt"],"needle":"old","replacement":"new"}).to_string()
}
fn runtime(endpoint: std::net::SocketAddr) -> Runtime {
    Runtime::new(
        endpoint,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap()
}
async fn exercise(session: &mut ProjectSession) -> Result<(), CodingError> {
    let observer = session.observer();
    let control = session.control();
    let decide = async {
        loop {
            let snapshot = observer.snapshot();
            if snapshot.error.is_some() || snapshot.cancelled {
                break;
            }
            if let Some(coding) = &snapshot.journeys[0].coding {
                if let Some(proposal) = &coding.proposal {
                    let _ = control.decide(proposal, CodingDecision::Approve);
                    break;
                }
                if coding.run_ended {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    let json = request();
    let (result, ()) = tokio::join!(session.submit(&json), decide);
    let mut receipt = result?;
    let snapshot = observer.snapshot();
    let coding = snapshot.journeys[0].coding.as_ref().unwrap();
    let summary = coding
        .final_summary
        .as_deref()
        .unwrap_or("source failed")
        .to_owned();
    let completion = if coding.final_summary.is_some() {
        CommandCompletion::Completed
    } else {
        CommandCompletion::Failed
    };
    if let Some(handle) = session
        .report_source(&mut receipt, completion, &summary)
        .await?
    {
        let ready = session.resume(handle).await?;
        let mut receipt = session.verify(ready).await?;
        session
            .report_verification(
                &mut receipt,
                CommandCompletion::Completed,
                "full edited file matched",
            )
            .await?;
    }
    Ok(())
}
fn unit(test: impl std::future::Future<Output = ()>) {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let _owner = crate::runtime::tests::TEST_OWNER.lock().await;
            test.await;
        });
}
async fn wait(entered: &AtomicBool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !entered.load(Ordering::Acquire) {
        assert!(Instant::now() < end, "checkpoint not reached");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
#[test]
fn blocked_setup_capture_reports_reads_and_final_joins_leave_stop_responsive_and_preserve_host_release_order()
 {
    unit(async {
        for point in [
            "Setup",
            "Requested",
            "Capture",
            "Captured",
            "Readers",
            "CodingStart",
            "Coding",
            "SourceReport",
            "Ready",
            "Verify",
            "Read",
            "VerificationReport",
            "ThreadExit",
            "Closure",
        ] {
            let root = root();
            let storage = roots::OwnedRoot::new();
            let fixture = fixture::Fixture::start(fixture::Mode::Golden).await;
            let entered = Arc::new(AtomicBool::new(false));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let callback_entered = entered.clone();
            let callback_gate = gate.clone();
            let hooks = Hooks {
                callback: Some(Arc::new(move |at, after| {
                    if at == point && !after {
                        callback_entered.store(true, Ordering::Release);
                        let mut open = callback_gate.0.lock().unwrap();
                        while !*open {
                            open = callback_gate.1.wait(open).unwrap();
                        }
                    }
                    Ok(())
                })),
                ..Default::default()
            };
            let (mut session, settlement) = ProjectSession::start_inner(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "gated-project",
                hooks,
            );
            let observer = session.observer();
            let control = session.control();
            if matches!(point, "ThreadExit" | "Closure") {
                exercise(&mut session).await.unwrap();
                let finish = settlement.finish();
                tokio::pin!(finish);
                tokio::select! { _=wait(&entered)=>{}, result=&mut finish=>panic!("finalization bypassed gate: {}",result.is_ok()) };
                let start = Instant::now();
                control.stop();
                let snapshot = observer.snapshot();
                assert!(start.elapsed() < Duration::from_millis(200));
                assert!(!snapshot.settled);
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
                let finish = finish.await.unwrap();
                assert!(finish.storage_joined && finish.runtime_joined);
            } else {
                let work = exercise(&mut session);
                tokio::pin!(work);
                tokio::select! { _=wait(&entered)=>{}, result=&mut work=>panic!("execution bypassed gate {point}: {}",result.is_ok()) };
                let start = Instant::now();
                control.stop();
                let snapshot = observer.snapshot();
                assert!(start.elapsed() < Duration::from_millis(200));
                assert!(snapshot.cancelled && !snapshot.settled);
                settlement.release_host();
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
                let _ = work.await;
                let finish = settlement.finish().await.unwrap();
                assert!(
                    finish.readers_joined
                        && finish.coding_joined
                        && finish.storage_joined
                        && finish.runtime_joined,
                    "{point}"
                );
            }
            let snapshot = observer.snapshot();
            let fixture = fixture.shutdown().await;
            assert!(fixture.joined, "{point}");
            fixture.attest(&snapshot).unwrap();
            assert_eq!(Runtime::process_snapshot().unknown_usage, 0);
            assert_eq!(Runtime::process_snapshot().unresolved, 0);
        }
    });
}
#[test]
fn persistence_failure_before_or_after_ack_never_mints_a_companion_or_erases_earlier_accepted_reports()
 {
    unit(async {
        for (point, after) in [
            ("Requested", false),
            ("Captured", false),
            ("Readers", false),
            ("Coding", false),
            ("SourceReport", false),
            ("SourceReport", true),
            ("Ready", false),
            ("Ready", true),
            ("Read", false),
            ("VerificationReport", false),
            ("Closure", false),
        ] {
            let root = root();
            let storage = roots::OwnedRoot::new();
            let fixture = fixture::Fixture::start(fixture::Mode::Golden).await;
            let hooks = Hooks {
                callback: Some(Arc::new(move |at, ack| {
                    if at == point && ack == after {
                        Err(CodingError::Io(std::io::Error::other(
                            "injected project persistence failure",
                        )))
                    } else {
                        Ok(())
                    }
                })),
                ..Default::default()
            };
            let (mut session, settlement) = ProjectSession::start_inner(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "failure-project",
                hooks,
            );
            settlement.release_host();
            let result = exercise(&mut session).await;
            if point == "Closure" {
                result.unwrap();
            } else {
                assert!(result.is_err(), "{point}/{after}");
            }
            let before = session.observer().snapshot();
            let prior_reports = before
                .records
                .iter()
                .filter(|r| {
                    matches!(
                        r.as_ref(),
                        ProjectRecord::SourceReport(_) | ProjectRecord::VerificationReport(_)
                    )
                })
                .count();
            let finish = settlement.finish().await.unwrap();
            assert!(finish.storage_joined && finish.runtime_joined);
            assert!(finish.error.is_some());
            let snapshot = session.observer().snapshot();
            assert_eq!(
                prior_reports,
                snapshot
                    .records
                    .iter()
                    .filter(|r| matches!(
                        r.as_ref(),
                        ProjectRecord::SourceReport(_) | ProjectRecord::VerificationReport(_)
                    ))
                    .count()
            );
            let fixture = fixture.shutdown().await;
            assert!(fixture.joined);
            fixture.attest(&snapshot).unwrap();
            assert!(fixture.requests.len() <= 7);
            assert!(root.path.exists());
            if matches!(point, "Requested" | "Captured" | "Readers") {
                assert_eq!(
                    fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                    BEFORE
                );
            }
        }
    });
}
#[test]
fn late_storage_owner_panic_keeps_prior_reports_and_retains_history_without_clean_settlement() {
    unit(async {
        let root = root();
        let storage = roots::OwnedRoot::new();
        let fixture = fixture::Fixture::start(fixture::Mode::Golden).await;
        let hooks = Hooks {
            callback: Some(Arc::new(|at, _| {
                if at == "ThreadExit" {
                    panic!("injected storage owner exit panic");
                }
                Ok(())
            })),
            ..Default::default()
        };
        let (mut session, settlement) = ProjectSession::start_inner(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "late-panic",
            hooks,
        );
        exercise(&mut session).await.unwrap();
        let reports = session.observer().snapshot().records.len();
        let finish = settlement.finish().await.unwrap();
        assert!(finish.storage_joined && finish.runtime_joined);
        assert!(!finish.logical_settled && !finish.history_acknowledged && finish.error.is_some());
        assert_eq!(session.observer().snapshot().records.len(), reports);
        let snapshot = session.observer().snapshot();
        let fixture = fixture.shutdown().await;
        assert!(fixture.joined);
        fixture.attest(&snapshot).unwrap();
        let history = reconstruct_project_history(&storage.path, "late-panic").unwrap();
        assert!(!history.closure_recorded);
        assert!(history.records.iter().any(|r| matches!(r.as_ref(),ProjectRecord::VerificationReport(r) if r.completion=="Completed")));
    });
}
#[test]
#[ignore = "explicit fresh-process integrated post-rename writer/read-only recovery helper"]
fn process_helper() {
    let Some(project) = std::env::var_os("SCRAMJET_PROJECT_TEST_ROOT") else {
        panic!("synthetic helper root missing");
    };
    let storage = PathBuf::from(std::env::var_os("SCRAMJET_PROJECT_TEST_STORAGE").unwrap());
    if std::env::var("SCRAMJET_PROJECT_TEST_MODE").unwrap() == "recover" {
        let history = reconstruct_project_history(&storage, "process-project").unwrap();
        assert_eq!(
            history.coding[0].as_ref().unwrap().operations()[0].state,
            OperationState::Uncertain
        );
        assert!(history.records.iter().all(|r| !matches!(
            r.as_ref(),
            ProjectRecord::SourceReport(_) | ProjectRecord::VerificationReady(_)
        )));
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = fixture::Fixture::start(fixture::Mode::Golden).await;
            let hooks = Hooks {
                coding: Some(Arc::new(Mutex::new(Some(
                    crate::coding::Checkpoint::ExitAfterRename,
                )))),
                ..Default::default()
            };
            let (mut session, settlement) = ProjectSession::start_inner(
                runtime(fixture.endpoint),
                Path::new(&project),
                &storage,
                "process-project",
                hooks,
            );
            settlement.release_host();
            exercise(&mut session).await.unwrap();
            panic!("post-rename exit checkpoint not reached");
        });
}
#[test]
fn actual_integrated_post_rename_exit_and_three_fresh_recoveries_preserve_uncertain_effect_without_replay()
 {
    let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
    let root = root();
    let storage = roots::OwnedRoot::new();
    let child = |mode| {
        std::process::Command::new("timeout")
            .arg("15")
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "project::session::tests::process_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("SCRAMJET_PROJECT_TEST_ROOT", &root.path)
            .env("SCRAMJET_PROJECT_TEST_STORAGE", &storage.path)
            .env("SCRAMJET_PROJECT_TEST_MODE", mode)
            .output()
            .unwrap()
    };
    let output = child("interrupt");
    assert_eq!(output.status.code(), Some(17), "{output:?}");
    let original = fs::read(storage.path.join(".project-journal")).unwrap();
    let coding = fs::read(storage.path.join("coding-1/.coding-journal")).unwrap();
    let target = fs::read(root.path.join("sample.txt")).unwrap();
    assert_eq!(&target, BEFORE.replace("old", "new").as_bytes());
    let inode = fs::metadata(root.path.join("sample.txt")).unwrap().ino();
    let moved = root.path.with_extension("moved");
    fs::rename(&root.path, &moved).unwrap();
    for _ in 0..3 {
        let output = child("recover");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            fs::read(storage.path.join(".project-journal")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(storage.path.join("coding-1/.coding-journal")).unwrap(),
            coding
        );
        assert_eq!(fs::read(moved.join("sample.txt")).unwrap(), target);
        assert_eq!(fs::metadata(moved.join("sample.txt")).unwrap().ino(), inode);
    }
    fs::rename(moved, &root.path).unwrap();
}
