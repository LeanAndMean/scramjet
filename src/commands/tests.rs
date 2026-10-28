use super::*;
#[path = "../../tests/support/commands.rs"]
mod fixture;
use fixture::{AUTOMATIC_CATALOG, Roots};
use std::sync::mpsc;
use std::time::Duration;

fn checkpoint_session(
    roots: &Roots,
    at: Checkpoint,
) -> (
    CommandSession,
    CommandControl,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
) {
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic).unwrap();
    let control = session.control();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    session.checkpoint = Some(Box::new(move |point| {
        if point == at {
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }));
    (session, control, ready_rx, release_tx)
}

#[test]
fn resume_handles_are_absent_outside_live_pause_and_closed_or_forged_handles_reject() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    for completion in [
        CommandCompletion::Completed,
        CommandCompletion::Cancelled,
        CommandCompletion::Incomplete,
    ] {
        let mut session =
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).unwrap();
        let id = session.begin("compare").unwrap();
        session.execute(id).unwrap();
        if completion == CommandCompletion::Cancelled {
            session.control().cancel();
        }
        session.report(id, completion, "no edge", None).unwrap();
        assert!(session.paused_continuation().is_none());
    }
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic).unwrap();
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    session
        .report(id, CommandCompletion::Completed, "equal", Some("inspect"))
        .unwrap();
    assert!(session.paused_continuation().is_none());
    session.close();
    assert!(session.paused_continuation().is_none());
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).unwrap();
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    session
        .report(id, CommandCompletion::Completed, "equal", Some("inspect"))
        .unwrap();
    let handle = session.paused_continuation().unwrap();
    let before = session.snapshot();
    assert!(
        session
            .resume(CommandResume {
                source: CommandInvocation { ordinal: 2, ..id }
            })
            .is_err()
    );
    assert_eq!(session.snapshot(), before);
    session.close();
    assert!(session.paused_continuation().is_none());
    let before = session.snapshot();
    assert!(session.resume(handle).is_err());
    assert_eq!(session.snapshot(), before);
}

#[test]
fn explicit_resume_cancellation_checkpoints_preserve_actual_companion_facts() {
    for (point, reads) in [
        (Checkpoint::BeforeResume, 2),
        (Checkpoint::BeforeRead(0), 2),
        (Checkpoint::Dispatched(0), 3),
        (Checkpoint::Returned(0), 3),
        (Checkpoint::BeforeReport, 3),
    ] {
        let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
        let (mut session, control, ready, release) = checkpoint_session(&roots, point);
        session.policy = CommandPolicy::Pause;
        let hook = session.checkpoint.take();
        let first = session.begin("compare").unwrap();
        session.execute(first).unwrap();
        session
            .report(
                first,
                CommandCompletion::Completed,
                "equal",
                Some("inspect"),
            )
            .unwrap();
        let first_fact = session.snapshot().invocations[0].clone();
        let handle = session.paused_continuation().unwrap();
        session.checkpoint = hook;
        let worker = std::thread::spawn(move || {
            if let Some(id) = session.resume(handle).unwrap() {
                session.execute(id).unwrap();
                if point == Checkpoint::BeforeReport {
                    assert!(
                        session
                            .report(id, CommandCompletion::Completed, "readable", None)
                            .is_err()
                    );
                    session.checkpoint = None;
                }
                session
                    .report(id, CommandCompletion::Cancelled, "cancelled", None)
                    .unwrap();
            }
            session.finalize()
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        control.cancel();
        release.send(()).unwrap();
        let state = worker.join().unwrap();
        assert_eq!(state.reads, reads, "{point:?}");
        assert_eq!(state.invocations[0], first_fact);
        if point == Checkpoint::BeforeResume {
            assert!(state.invocations[1].is_none());
        } else {
            let fact = state.invocations[1].as_ref().unwrap();
            assert_eq!(fact.phase, CommandPhase::Cancelled);
            assert_eq!(fact.steps.iter().flatten().count(), reads - 2);
        }
    }
}

#[test]
fn observer_is_read_only_detached_responsive_during_dispatch_and_retained_after_finalize() {
    let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
    let (mut session, control, ready, release) =
        checkpoint_session(&roots, Checkpoint::Dispatched(0));
    let observer = session.observer();
    let identity = observer.session();
    drop(observer.clone());
    assert!(!observer.snapshot().cancellation_requested);
    let worker = std::thread::spawn(move || {
        let id = session.begin("compare").unwrap();
        session.execute(id).unwrap();
        session
            .report(id, CommandCompletion::Cancelled, "cancelled", None)
            .unwrap();
        session.finalize()
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut detached = observer.snapshot();
    assert_eq!(
        detached.invocations[0].as_ref().unwrap().id.session(),
        identity
    );
    assert_eq!(detached.reads, 1);
    assert_eq!(
        detached.invocations[0].as_ref().unwrap().phase,
        CommandPhase::Running
    );
    detached.reads = 999;
    detached.invocations = [None, None];
    assert_eq!(observer.snapshot().reads, 1);
    control.cancel();
    assert!(!observer.snapshot().settled);
    release.send(()).unwrap();
    let final_state = worker.join().unwrap();
    assert_eq!(observer.snapshot(), final_state);
    assert_eq!(observer.session(), identity);
    assert!(final_state.settled);
    assert_eq!(final_state.reads, 1);
}

#[test]
fn concurrent_cancellation_before_dispatch_between_reads_and_at_result_acceptance_settles_actual_facts()
 {
    for (point, reads) in [
        (Checkpoint::BeforeRead(0), 0),
        (Checkpoint::Dispatched(0), 1),
        (Checkpoint::Returned(0), 1),
        (Checkpoint::BeforeRead(1), 1),
        (Checkpoint::Dispatched(1), 2),
        (Checkpoint::Returned(1), 2),
    ] {
        let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
        let (mut session, control, ready, release) = checkpoint_session(&roots, point);
        let worker = std::thread::spawn(move || {
            let id = session.begin("compare").unwrap();
            session.execute(id).unwrap();
            assert!(
                session
                    .report(id, CommandCompletion::Completed, "no", Some("inspect"))
                    .is_err()
            );
            session
                .report(id, CommandCompletion::Cancelled, "cancelled", None)
                .unwrap();
            session.finalize()
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(control.cancel());
        release.send(()).unwrap();
        let state = worker.join().unwrap();
        assert!(state.settled);
        assert_eq!(state.reads, reads, "{point:?}");
        let fact = state.invocations[0].as_ref().unwrap();
        assert_eq!(fact.phase, CommandPhase::Cancelled);
        assert_eq!(fact.steps.iter().flatten().count(), reads);
        assert!(
            fact.steps
                .iter()
                .flatten()
                .all(|s| s.result == CommandReadResult::Read("same".into()))
        );
        assert!(state.invocations[1].is_none());
        assert!(!control.cancel());
    }
}

#[test]
fn concurrent_cancellation_linearizes_completion_and_next_admission() {
    for point in [
        Checkpoint::BeforeReport,
        Checkpoint::BeforeFollow,
        Checkpoint::AdmittedFollow,
    ] {
        let roots = Roots::new(AUTOMATIC_CATALOG, b"same", b"same").unwrap();
        let (mut session, control, ready, release) = checkpoint_session(&roots, point);
        let worker = std::thread::spawn(move || {
            let id = session.begin("compare").unwrap();
            session.execute(id).unwrap();
            let report = session.report(id, CommandCompletion::Completed, "equal", Some("inspect"));
            if point == Checkpoint::BeforeReport {
                assert!(report.is_err());
                session.checkpoint = None;
                session
                    .report(id, CommandCompletion::Cancelled, "cancelled", None)
                    .unwrap();
            } else {
                report.unwrap();
                let follow = session.follow().unwrap();
                if point == Checkpoint::AdmittedFollow {
                    let companion = follow.unwrap();
                    session
                        .report(companion, CommandCompletion::Cancelled, "cancelled", None)
                        .unwrap();
                } else {
                    assert!(follow.is_none());
                }
            }
            session.finalize()
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        control.cancel();
        release.send(()).unwrap();
        let state = worker.join().unwrap();
        assert!(state.settled);
        assert_eq!(state.reads, 2);
        assert_eq!(
            state.invocations[0].as_ref().unwrap().phase,
            if point == Checkpoint::BeforeReport {
                CommandPhase::Cancelled
            } else {
                CommandPhase::Completed
            }
        );
        if point == Checkpoint::AdmittedFollow {
            let companion = state.invocations[1].as_ref().unwrap();
            assert_eq!(companion.phase, CommandPhase::Cancelled);
            assert!(companion.steps.iter().all(Option::is_none));
        } else {
            assert!(state.invocations[1].is_none());
        }
    }
}

#[test]
fn schema_bounds_fixed_slots_and_all_four_reads_and_retained_payloads() {
    let catalog = AUTOMATIC_CATALOG.replace(
        "\"Readable\",\"steps\":[{\"read\":\"left.txt\"}]",
        "\"Equal\",\"steps\":[{\"read\":\"left.txt\"},{\"read\":\"right.txt\"}]",
    );
    let payload = "x".repeat(COMMAND_INPUT_BYTES);
    let roots = Roots::new(&catalog, payload.as_bytes(), payload.as_bytes()).unwrap();
    let mut session =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic).unwrap();
    let id = session.begin("compare").unwrap();
    session.execute(id).unwrap();
    session
        .report(id, CommandCompletion::Completed, "equal", Some("inspect"))
        .unwrap();
    let companion = session.follow().unwrap().unwrap();
    session
        .report(companion, CommandCompletion::Completed, "also equal", None)
        .unwrap();
    let state = session.finalize();
    assert_eq!(state.reads, COMMAND_READS);
    assert_eq!(state.retained_bytes, 4096);
    assert_eq!(
        state.invocations.iter().flatten().count(),
        COMMAND_INVOCATIONS
    );
    for name in [
        "",
        ".",
        "..",
        "...",
        "a/b",
        "a\\b",
        "a\0b",
        "\u{202e}",
        &"x".repeat(65),
    ] {
        let text = AUTOMATIC_CATALOG.replace("left.txt", &name.escape_default().to_string());
        assert!(schema::decode(&text).is_err(), "{name:?}");
    }
    for n in [32, 33] {
        let text = AUTOMATIC_CATALOG.replace("compare", &"c".repeat(n));
        assert_eq!(schema::decode(&text).is_ok(), n == 32);
    }
    for n in [64, 65] {
        let text = AUTOMATIC_CATALOG.replace("left.txt", &"f".repeat(n));
        assert_eq!(schema::decode(&text).is_ok(), n == 64);
    }
    let third = r#",{"name":"third","outcome":"Readable","steps":[{"read":"left.txt"}]}"#;
    let text = catalog.strip_suffix("]}").unwrap().to_owned() + third + "]}";
    assert!(schema::decode(&text).is_err());
}

#[test]
fn failed_coding_report_and_cancellation_preserve_the_winning_boundary_and_execution() {
    use crate::{CodingCapability, CodingDecision, CodingSession, OperationState, Runtime};
    let _spawn = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let catalog =
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt"}]}"#;
    for failure in [1, 2, 3] {
        for cancel_first in [true, false] {
            let roots = Roots::new(catalog, b"", b"").unwrap();
            let (mut command, control, ready, release) =
                checkpoint_session(&roots, Checkpoint::BeforeReport);
            let observer = command.observer();
            let id = command.begin_coding("replace", "after\n").unwrap();
            let name = command.snapshot().invocations[0]
                .as_ref()
                .unwrap()
                .coding
                .as_ref()
                .unwrap()
                .session
                .clone();
            let project = crate::coding::tests::support::OwnedRoot::new();
            let session =
                CodingSession::create(&project.path, &name, CodingCapability::ControlledEdit)
                    .unwrap();
            let (reported_tx, reported_rx) = mpsc::channel();
            let (cancelled_tx, cancelled_rx) = mpsc::channel();
            let worker = std::thread::spawn(move || {
                let mut adapter = command.start_coding(id, session).unwrap();
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                executor.block_on(async {
                    let _owner = crate::runtime::tests::TEST_OWNER.lock().await;
                    let mut replies = crate::coding::tests::model_fixture::golden();
                    if failure > 1 {
                        replies[failure].text = "{}".into();
                    }
                    let fixture =
                        crate::coding::tests::model_fixture::ModelFixture::start(replies).await;
                    let runtime = Runtime::new(
                        fixture.endpoint,
                        1,
                        crate::RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                        crate::SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1))
                            .unwrap(),
                        crate::ConcurrencyMode::Fixed,
                    )
                    .unwrap();
                    let coding_observer = adapter.observer();
                    let coding_control = adapter.control();
                    let decision = async {
                        loop {
                            if let Some(p) = &coding_observer.snapshot().proposal {
                                coding_control
                                    .decide(
                                        p,
                                        if failure == 1 {
                                            CodingDecision::Deny
                                        } else {
                                            CodingDecision::Approve
                                        },
                                    )
                                    .unwrap();
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    };
                    tokio::join!(adapter.run_unjoined(&runtime), decision)
                        .0
                        .unwrap();
                    assert!(adapter.result().unwrap().is_err());
                    let count = runtime.snapshot().attempts;
                    assert_eq!(count, failure + 1);
                    runtime.shutdown().await;
                    fixture.settle(count).await;
                    fixture.stop().await;
                });
                command.accept_coding(&mut adapter.finish()).unwrap();
                let pending = command.snapshot();
                let execution = pending.invocations[0]
                    .as_ref()
                    .unwrap()
                    .coding
                    .as_ref()
                    .unwrap()
                    .execution
                    .clone()
                    .unwrap();
                assert_eq!(execution.disposition, CommandCompletion::Failed);
                assert!(
                    execution
                        .reason
                        .as_ref()
                        .is_some_and(|reason| !reason.is_empty())
                );
                assert!(execution.observation.worker_joined);
                assert_eq!(
                    execution.observation.effect,
                    if failure == 1 {
                        None
                    } else {
                        Some(OperationState::Completed)
                    }
                );
                assert_eq!(
                    std::fs::read_to_string(project.path.join("sample.txt")).unwrap(),
                    if failure == 1 { "before\n" } else { "after\n" }
                );
                let report = command.report(id, CommandCompletion::Failed, "actual failure", None);
                command.checkpoint = None;
                if cancel_first {
                    assert!(report.is_err());
                    let mut expected = pending;
                    expected.cancellation_requested = true;
                    assert_eq!(command.snapshot(), expected);
                    command
                        .report(
                            id,
                            CommandCompletion::Cancelled,
                            "explicit cancellation",
                            None,
                        )
                        .unwrap();
                } else {
                    report.unwrap();
                    let accepted = command.snapshot();
                    reported_tx.send(()).unwrap();
                    cancelled_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    let mut expected = accepted;
                    expected.cancellation_requested = true;
                    assert_eq!(command.snapshot(), expected);
                    assert!(
                        command
                            .report(id, CommandCompletion::Cancelled, "late", None)
                            .is_err()
                    );
                }
                let result = command.finalize();
                assert!(result.settled && result.cancellation_requested);
                let fact = result.invocations[0].as_ref().unwrap();
                assert_eq!(
                    fact.phase,
                    if cancel_first {
                        CommandPhase::Cancelled
                    } else {
                        CommandPhase::Failed
                    }
                );
                assert_eq!(
                    fact.coding.as_ref().unwrap().execution.as_ref().unwrap(),
                    &execution
                );
            });
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
            let pending = observer.snapshot();
            assert_eq!(
                pending.invocations[0].as_ref().unwrap().phase,
                CommandPhase::AwaitingCompletion
            );
            assert!(pending.invocations[0].as_ref().unwrap().report.is_none());
            if cancel_first {
                assert!(control.cancel());
                release.send(()).unwrap();
            } else {
                release.send(()).unwrap();
                reported_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(control.cancel());
                cancelled_tx.send(()).unwrap();
            }
            worker.join().unwrap();
        }
    }
}

#[test]
fn coding_launch_cancellation_and_success_report_races_have_synchronized_boundaries() {
    use crate::{CodingCapability, CodingDecision, CodingSession, Runtime};
    let _spawn = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let catalog =
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt"}]}"#;
    for point in [
        Checkpoint::BeforeCodingLaunch,
        Checkpoint::AfterCodingLaunch,
        Checkpoint::BeforeReport,
    ] {
        let roots = Roots::new(catalog, b"", b"").unwrap();
        let (mut command, control, ready, release) = checkpoint_session(&roots, point);
        let id = command.begin_coding("replace", "after\n").unwrap();
        let name = command.snapshot().invocations[0]
            .as_ref()
            .unwrap()
            .coding
            .as_ref()
            .unwrap()
            .session
            .clone();
        let project = crate::coding::tests::support::OwnedRoot::new();
        let session =
            CodingSession::create(&project.path, &name, CodingCapability::ControlledEdit).unwrap();
        let worker = std::thread::spawn(move || {
            let started = command.start_coding(id, session);
            if point == Checkpoint::BeforeCodingLaunch {
                assert!(started.is_err());
                command
                    .report(id, CommandCompletion::Cancelled, "no coding launch", None)
                    .unwrap();
            } else {
                let mut adapter = started.unwrap();
                if point == Checkpoint::BeforeReport {
                    let executor = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    executor.block_on(async {
                        let _owner = crate::runtime::tests::TEST_OWNER.lock().await;
                        let fixture = crate::coding::tests::model_fixture::ModelFixture::start(
                            crate::coding::tests::model_fixture::golden(),
                        )
                        .await;
                        let runtime = Runtime::new(
                            fixture.endpoint,
                            1,
                            crate::RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                            crate::SyntheticTokenBudget::new(
                                65536,
                                65536,
                                Duration::from_millis(1),
                            )
                            .unwrap(),
                            crate::ConcurrencyMode::Fixed,
                        )
                        .unwrap();
                        let observer = adapter.observer();
                        let coding_control = adapter.control();
                        let decision = async {
                            loop {
                                if let Some(p) = &observer.snapshot().proposal {
                                    coding_control.decide(p, CodingDecision::Approve).unwrap();
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(1)).await;
                            }
                        };
                        tokio::join!(adapter.run_unjoined(&runtime), decision)
                            .0
                            .unwrap();
                        assert!(adapter.result().unwrap().is_ok());
                        let count = runtime.snapshot().attempts;
                        runtime.shutdown().await;
                        fixture.settle(count).await;
                        fixture.stop().await;
                    });
                }
                command.accept_coding(&mut adapter.finish()).unwrap();
                if point == Checkpoint::BeforeReport {
                    assert!(
                        command
                            .report(
                                id,
                                CommandCompletion::Completed,
                                "cancelled before report",
                                None
                            )
                            .is_err()
                    );
                    command.checkpoint = None;
                }
                command
                    .report(
                        id,
                        CommandCompletion::Cancelled,
                        "explicit command cancellation",
                        None,
                    )
                    .unwrap();
            }
            command.finalize()
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        control.cancel();
        release.send(()).unwrap();
        let result = worker.join().unwrap();
        assert!(result.settled && result.cancellation_requested);
        let fact = result.invocations[0].as_ref().unwrap();
        assert_eq!(fact.phase, CommandPhase::Cancelled);
        if point == Checkpoint::BeforeReport {
            assert_eq!(
                fact.coding
                    .as_ref()
                    .unwrap()
                    .execution
                    .as_ref()
                    .unwrap()
                    .observation
                    .effect,
                Some(crate::OperationState::Completed)
            );
        }
    }
}
