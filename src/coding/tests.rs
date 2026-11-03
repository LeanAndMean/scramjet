use super::*;
use crate::{RequestId, Runtime};
#[path = "../../tests/support/coding_fixture.rs"]
pub(crate) mod model_fixture;
#[path = "../../tests/support/coding.rs"]
pub(crate) mod support;
use std::fs;
use std::process::Command;
use support::OwnedRoot;

// fork may temporarily retain another test's flock until exec closes its descriptors.
// Not reentrant, and the isolated runner takes it: never take it in a re-executed child test.
pub(crate) static PROCESS_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

fn authorized(root: &OwnedRoot) -> (CodingSession, EditCancellation) {
    let mut session = CodingSession::create(
        &root.path,
        "fault-session",
        CodingCapability::ControlledEdit,
    )
    .unwrap();
    let cancel = EditCancellation::default();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    session.authorize(intent, approval, &cancel).unwrap();
    (session, cancel)
}

#[test]
fn caller_backing_is_not_retained_and_validation_boundary_cancellation_does_not_dispatch() {
    let _gate = PROCESS_GATE.read().unwrap();
    let root = OwnedRoot::new();
    let mut session = CodingSession::create(
        &root.path,
        "fault-session",
        CodingCapability::ControlledEdit,
    )
    .unwrap();
    let mut intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    intent.replacement.reserve_exact(1_048_576);
    let approval = session.approve_exact(&intent).unwrap();
    let cancel = EditCancellation::default();
    session.authorize(intent, approval, &cancel).unwrap();
    assert!(session.current.as_ref().unwrap().replacement.capacity() <= CONTENT_BYTES);
    session.checkpoint = Some(Checkpoint::Cancel(
        CheckpointPhase::BeforeDispatch,
        cancel.clone(),
    ));
    assert_eq!(
        session.dispatch(&cancel).unwrap().state,
        OperationState::NotDispatched
    );
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "before\n"
    );
}

#[test]
fn effect_errors_are_uncertain_poisoned_and_never_retried() {
    let _gate = PROCESS_GATE.read().unwrap();
    for phase in [
        CheckpointPhase::StageWrite,
        CheckpointPhase::StageSync,
        CheckpointPhase::Rename,
        CheckpointPhase::AfterRename,
    ] {
        let root = OwnedRoot::new();
        let (mut session, cancel) = authorized(&root);
        session.checkpoint = Some(Checkpoint::Fail(phase));
        let result = session.dispatch(&cancel).unwrap();
        assert_eq!(result.state, OperationState::Uncertain);
        assert!(result.acknowledgement_error.is_some());
        assert!(matches!(
            session.dispatch(&cancel),
            Err(CodingError::Poisoned)
        ));
        let expected = if phase == CheckpointPhase::AfterRename {
            "after\n"
        } else {
            "before\n"
        };
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            expected
        );
        drop(session);
        assert_eq!(
            reconstruct_coding(&root.path, "fault-session").unwrap()[0].state,
            OperationState::Uncertain
        );
    }
}

#[test]
fn partial_stage_write_and_failed_dispatch_record_preserve_truthful_uncertainty() {
    let _gate = PROCESS_GATE.read().unwrap();
    let root = OwnedRoot::new();
    let (mut session, cancel) = authorized(&root);
    session.checkpoint = Some(Checkpoint::ShortStageWrite);
    assert_eq!(
        session.dispatch(&cancel).unwrap().state,
        OperationState::Uncertain
    );
    assert_eq!(
        fs::read_to_string(root.path.join(STAGE_NAME)).unwrap(),
        "aft"
    );
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "before\n"
    );
    drop(session);
    assert_eq!(
        reconstruct_coding(&root.path, "fault-session").unwrap()[0].state,
        OperationState::Uncertain
    );

    let root = OwnedRoot::new();
    let (mut session, cancel) = authorized(&root);
    session.journal.fault = Some(journal::JournalFault::Sync);
    assert!(session.dispatch(&cancel).is_err());
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "before\n"
    );
    drop(session);
    assert_eq!(
        reconstruct_coding(&root.path, "fault-session").unwrap()[0].state,
        OperationState::Uncertain
    );
}

#[test]
fn cancellation_after_dispatch_finishes_owned_sync_work() {
    let _gate = PROCESS_GATE.read().unwrap();
    for phase in [CheckpointPhase::StageWrite, CheckpointPhase::AfterRename] {
        let root = OwnedRoot::new();
        let (mut session, cancel) = authorized(&root);
        session.checkpoint = Some(Checkpoint::Cancel(phase, cancel.clone()));
        let result = session.dispatch(&cancel).unwrap();
        assert_eq!(result.state, OperationState::Completed);
        assert!(result.cancellation_observed);
        drop(session);
        assert_eq!(
            reconstruct_coding(&root.path, "fault-session").unwrap()[0].state,
            OperationState::Completed
        );
    }
}

#[test]
fn failed_authorization_or_dispatch_acknowledgement_prevents_effect() {
    let _gate = PROCESS_GATE.read().unwrap();
    for dispatch in [false, true] {
        for fault in [
            journal::JournalFault::ShortWrite,
            journal::JournalFault::Append,
            journal::JournalFault::Sync,
        ] {
            let root = OwnedRoot::new();
            let mut session = CodingSession::create(
                &root.path,
                "fault-session",
                CodingCapability::ControlledEdit,
            )
            .unwrap();
            let cancel = EditCancellation::default();
            let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
            let approval = session.approve_exact(&intent).unwrap();
            if dispatch {
                session.authorize(intent, approval, &cancel).unwrap();
                session.journal.fault = Some(fault);
                assert!(session.dispatch(&cancel).is_err());
            } else {
                session.journal.fault = Some(fault);
                assert!(session.authorize(intent, approval, &cancel).is_err());
            }
            assert!(matches!(
                session
                    .intent("sample.txt", "before\n", "after\n")
                    .and_then(|intent| session.approve_exact(&intent)),
                Err(CodingError::Poisoned)
            ));
            assert_eq!(
                fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                "before\n"
            );
            assert!(!root.path.join(STAGE_NAME).exists());
        }
    }
}

#[test]
fn failed_completion_acknowledgement_is_not_success() {
    let _gate = PROCESS_GATE.read().unwrap();
    for fault in [
        journal::JournalFault::ShortWrite,
        journal::JournalFault::Append,
        journal::JournalFault::Sync,
    ] {
        let root = OwnedRoot::new();
        let (mut session, cancel) = authorized(&root);
        session.checkpoint = Some(Checkpoint::Journal(
            CheckpointPhase::BeforeCompletion,
            fault,
        ));
        let result = session.dispatch(&cancel).unwrap();
        assert_eq!(result.state, OperationState::Uncertain);
        assert!(result.acknowledgement_error.is_some());
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            "after\n"
        );
        assert!(matches!(
            session.dispatch(&cancel),
            Err(CodingError::Poisoned)
        ));
    }
}

#[test]
fn terminal_record_capacity_is_reserved_before_authorization() {
    let _gate = PROCESS_GATE.read().unwrap();
    for limits in [
        (3, journal::JOURNAL_BYTES),
        (journal::RECORDS, 3 * journal::RECORD_BYTES),
    ] {
        let root = OwnedRoot::new();
        let mut session = CodingSession::create(
            &root.path,
            "fault-session",
            CodingCapability::ControlledEdit,
        )
        .unwrap();
        session.journal.limits = limits;
        let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
        let approval = session.approve_exact(&intent).unwrap();
        let before = fs::read(root.path.join(journal::JOURNAL_NAME)).unwrap();
        assert!(matches!(
            session.authorize(intent, approval, &EditCancellation::default()),
            Err(CodingError::History(_))
        ));
        assert_eq!(
            fs::read(root.path.join(journal::JOURNAL_NAME)).unwrap(),
            before
        );
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            "before\n"
        );
    }
    let root = OwnedRoot::new();
    let (mut session, cancel) = authorized(&root);
    session.journal.limits = (3, 1);
    assert_eq!(
        session.dispatch(&cancel).unwrap().state,
        OperationState::Completed
    );
}

#[test]
fn restart_process_helper() {
    let _gate = PROCESS_GATE.read().unwrap();
    let Some(path) = std::env::var_os("SCRAMJET_CODING_TEST_ROOT") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    match std::env::var("SCRAMJET_CODING_TEST_MODE").unwrap().as_str() {
        "integrated-interrupt" | "integrated-cancel" | "integrated-complete" => {
            let mode = std::env::var("SCRAMJET_CODING_TEST_MODE").unwrap();
            let mut session =
                CodingSession::create(&path, "restart-session", CodingCapability::ControlledEdit)
                    .unwrap();
            let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
            let approval = session.approve_exact(&intent).unwrap();
            let cancel = EditCancellation::default();
            session.checkpoint = Some(if mode == "integrated-interrupt" {
                Checkpoint::ExitAfterRename
            } else if mode == "integrated-cancel" {
                Checkpoint::Cancel(CheckpointPhase::AfterRename, cancel.clone())
            } else {
                Checkpoint::Cancel(CheckpointPhase::BeforeDispatch, cancel.clone())
            });
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            executor.block_on(async {
                let fixture = model_fixture::ModelFixture::start(model_fixture::golden()).await;
                let runtime = crate::Runtime::new(
                    fixture.endpoint,
                    1,
                    crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                    crate::SyntheticTokenBudget::new(
                        65_536,
                        65_536,
                        std::time::Duration::from_millis(1),
                    )
                    .unwrap(),
                    crate::ConcurrencyMode::Fixed,
                )
                .unwrap();
                let agent =
                    agent::CodingAgent::start(session, Some(approval), cancel, "after\n").unwrap();
                assert!(agent.run(&runtime).await.is_err());
                let attempts = runtime.snapshot().attempts;
                runtime.shutdown().await;
                fixture.settle(attempts).await;
                assert_eq!(fixture.requests().len(), 2);
                assert!(fixture.requests().iter().all(|text| {
                    serde_json::from_str::<serde_json::Value>(text).unwrap()["prompt"] == "after\n"
                }));
                fixture.stop().await;
            });
            let facts = reconstruct_coding(&path, "restart-session").unwrap();
            assert_eq!(
                facts[0].state,
                if mode == "integrated-cancel" {
                    OperationState::Completed
                } else {
                    OperationState::NotDispatched
                }
            );
        }
        "selected-interrupt" | "selected-recover" => {
            let storage =
                std::path::PathBuf::from(std::env::var_os("SCRAMJET_SELECTED_STORAGE").unwrap());
            if std::env::var("SCRAMJET_CODING_TEST_MODE").unwrap() == "selected-interrupt" {
                let workspace = crate::ProjectWorkspace::open(&path, &storage).unwrap();
                let request = crate::ProjectRequest::parse(r#"{"operation":"replace_once","target":"sample.txt","supporting":["rules.txt","notes.txt"],"needle":"before","replacement":"after"}"#).unwrap();
                let capture = workspace.capture(request).unwrap();
                let mut session = workspace
                    .coding_session(capture, "selected-restart", 1)
                    .unwrap();
                let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
                let approval = session.approve_exact(&intent).unwrap();
                let cancel = EditCancellation::default();
                session.authorize(intent, approval, &cancel).unwrap();
                session.checkpoint = Some(Checkpoint::ExitAfterRename);
                session.dispatch(&cancel).unwrap();
                panic!("selected interruption not reached");
            }
            let identity = RootIdentity {
                device: std::env::var("SCRAMJET_SELECTED_DEVICE")
                    .unwrap()
                    .parse()
                    .unwrap(),
                inode: std::env::var("SCRAMJET_SELECTED_INODE")
                    .unwrap()
                    .parse()
                    .unwrap(),
            };
            let history =
                crate::ProjectWorkspace::coding_history(&storage, 1, "selected-restart", identity)
                    .unwrap();
            assert_eq!(history.operations()[0].state, OperationState::Uncertain);
        }
        "interrupt" => {
            let mut session =
                CodingSession::create(&path, "restart-session", CodingCapability::ControlledEdit)
                    .unwrap();
            let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
            let approval = session.approve_exact(&intent).unwrap();
            let cancel = EditCancellation::default();
            session.authorize(intent, approval, &cancel).unwrap();
            session.checkpoint = Some(Checkpoint::ExitAfterRename);
            session.dispatch(&cancel).unwrap();
            panic!("interruption checkpoint not reached");
        }
        mode => {
            let facts = reconstruct_coding(&path, "restart-session").unwrap();
            let expected = match mode {
                "uncertain" => OperationState::Uncertain,
                "completed" => OperationState::Completed,
                "authorized" => OperationState::Authorized,
                "not-dispatched" => OperationState::NotDispatched,
                _ => panic!("unknown test mode"),
            };
            assert_eq!(facts.len(), 1);
            assert_eq!(facts[0].state, expected);
        }
    }
}

fn child(root: &OwnedRoot, mode: &str) -> std::process::Output {
    Command::new("timeout")
        .args(["10"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "coding::tests::restart_process_helper",
            "--nocapture",
        ])
        .env("SCRAMJET_CODING_TEST_ROOT", &root.path)
        .env("SCRAMJET_CODING_TEST_MODE", mode)
        .output()
        .unwrap()
}

#[test]
fn actual_post_rename_exit_and_three_fresh_reconstructions_never_replay() {
    let _gate = PROCESS_GATE.write().unwrap();
    for mode in [
        "uncertain",
        "completed",
        "authorized",
        "integrated-interrupt",
        "integrated-cancel",
        "integrated-complete",
    ] {
        let root = OwnedRoot::new();
        if mode.starts_with("integrated-") {
            let output = child(&root, mode);
            assert_eq!(
                output.status.code(),
                Some(if mode == "integrated-interrupt" {
                    17
                } else {
                    0
                }),
                "{output:?}"
            );
        } else if mode == "uncertain" {
            let output = child(&root, "interrupt");
            assert_eq!(output.status.code(), Some(17), "{output:?}");
        } else {
            let mut session = CodingSession::create(
                &root.path,
                "restart-session",
                CodingCapability::ControlledEdit,
            )
            .unwrap();
            let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
            let approval = session.approve_exact(&intent).unwrap();
            let cancel = EditCancellation::default();
            session.authorize(intent, approval, &cancel).unwrap();
            if mode == "completed" {
                session.dispatch(&cancel).unwrap();
            }
        }
        let before_journal = fs::read(root.path.join(journal::JOURNAL_NAME)).unwrap();
        if mode.starts_with("integrated-") {
            let requests: Vec<_> = std::str::from_utf8(&before_journal)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .filter(|v| v["record"]["entry"]["event"] == "Request")
                .collect();
            assert_eq!(requests.len(), 2);
            for request in requests {
                let context: serde_json::Value =
                    serde_json::from_str(request["record"]["entry"]["text"].as_str().unwrap())
                        .unwrap();
                assert_eq!(context["prompt"], "after\n");
            }
        }
        let before_target = fs::read(root.path.join("sample.txt")).unwrap();
        use std::os::unix::fs::MetadataExt;
        let before_inode = fs::metadata(root.path.join("sample.txt")).unwrap().ino();
        for _ in 0..3 {
            let recovery_mode = match mode {
                "integrated-interrupt" => "uncertain",
                "integrated-cancel" => "completed",
                "integrated-complete" => "not-dispatched",
                mode => mode,
            };
            let output = child(&root, recovery_mode);
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                fs::read(root.path.join(journal::JOURNAL_NAME)).unwrap(),
                before_journal
            );
            assert_eq!(
                fs::read(root.path.join("sample.txt")).unwrap(),
                before_target
            );
            assert_eq!(
                fs::metadata(root.path.join("sample.txt")).unwrap().ino(),
                before_inode
            );
            assert!(!root.path.join(STAGE_NAME).exists());
        }
    }
}
