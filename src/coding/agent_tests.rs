use super::super::tests::support as roots;
use super::*;

#[test]
fn failed_request_sync_does_not_publish_recorded_or_admitted_receipt() {
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let root = roots::OwnedRoot::new();
    let mut session = super::super::CodingSession::create(
        &root.path,
        "request-sync",
        super::super::CodingCapability::ControlledEdit,
    )
    .unwrap();
    session.journal.fault = Some(super::super::journal::JournalFault::Sync);
    let agent =
        CodingAgent::start_interactive(session, EditCancellation::default(), "prompt\n界").unwrap();
    let observer = agent.observer();
    let executor = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    executor.block_on(async {
        let (reply, receive) = oneshot::channel();
        agent
            .sender
            .as_ref()
            .unwrap()
            .try_send(Work::Request {
                turn: 1,
                text: serialize_context("request-sync", 1, "prompt\n界", "").unwrap(),
                reply,
            })
            .unwrap();
        assert!(receive.await.unwrap().is_err());
    });
    assert!(!observer.snapshot().attempts[0].recorded);
    assert!(observer.snapshot().attempts[0].request_id.is_none());
    agent.finish().unwrap();
    assert!(observer.snapshot().worker_joined);
    assert_eq!(
        std::fs::read(root.path.join("sample.txt")).unwrap(),
        b"before\n"
    );
}

#[test]
fn prompt_preflight_and_every_turn_reserve_actual_complete_context() {
    let session = "s".repeat(64);
    let previous = "\n".repeat(1024);
    let overhead = serialize_context(&session, 8, "", &previous).unwrap().len();
    let budget = 4096 - overhead;
    let prompt = format!(
        "{}{}",
        "\"".repeat(budget / 2),
        if budget % 2 == 1 { "x" } else { "" }
    );
    assert!(prompt.len() <= 1024);
    CodingAgent::validate_prompt(&session, &prompt).unwrap();
    assert_eq!(
        serialize_context(&session, 8, &prompt, &previous)
            .unwrap()
            .len(),
        4096
    );
    assert!(CodingAgent::validate_prompt(&session, &format!("{prompt}x")).is_err());
    for name in ["s", session.as_str()] {
        for prompt in [
            "",
            "界 e\u{301}🙂\n",
            &"x".repeat(1024),
            &"\\".repeat(967),
            &"\n".repeat(967),
        ] {
            CodingAgent::validate_prompt(name, prompt).unwrap();
            for turn in 1..=8 {
                for previous in [
                    "",
                    previous.as_str(),
                    &"\t".repeat(1024),
                    &"\"".repeat(1024),
                    &"\\".repeat(1024),
                ] {
                    let text = serialize_context(name, turn, prompt, previous).unwrap();
                    assert!(text.len() <= 4096);
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    assert_eq!(value["prompt"], prompt);
                    assert_eq!(value["previous"], previous);
                }
            }
        }
    }
    for bad in ["\t", "\r", "\u{1b}", "\u{7f}", "\u{85}"] {
        assert!(CodingAgent::validate_prompt("s", bad).is_err());
    }
    assert!(CodingAgent::validate_prompt("s", &"x".repeat(1025)).is_err());
}

#[test]
fn input_serialization_and_material_capacity_reject_before_effect() {
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let root = roots::OwnedRoot::new();
    let mut session = CodingSession::create(
        &root.path,
        "quota-test",
        super::super::CodingCapability::ControlledEdit,
    )
    .unwrap();
    for _ in 0..49 {
        session
            .journal
            .append(Record::Cancelled { operation: 1 })
            .unwrap();
    }
    assert!(session.journal.ensure_turn_capacity().is_err());
    assert_eq!(session.read("sample.txt").unwrap(), "before\n");
    let mut buffer = InputBuffer(Vec::with_capacity(4096));
    let previous = "\u{1}".repeat(1024);
    assert!(
        serde_json::to_writer(
            &mut buffer,
            &Context {
                session: "quota-test",
                turn: 1,
                prompt: "",
                previous: &previous
            }
        )
        .is_err()
    );
    assert_eq!(buffer.0.capacity(), 4096);
    assert!(buffer.0.len() <= 4096);
}

#[test]
fn failed_material_acknowledgement_refuses_tools_and_agent_drop_joins_session_owner() {
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let root = roots::OwnedRoot::new();
    let mut session = CodingSession::create(
        &root.path,
        "material-fault",
        super::super::CodingCapability::ControlledEdit,
    )
    .unwrap();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    session.journal.fault = Some(super::super::journal::JournalFault::Sync);
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let mut agent =
            CodingAgent::start(session, Some(approval), EditCancellation::default(), "").unwrap();
        let (reply, receive) = oneshot::channel();
        agent
            .sender
            .as_ref()
            .unwrap()
            .try_send(Work::Request {
                turn: 1,
                text: "{}".to_owned(),
                reply,
            })
            .unwrap();
        assert!(receive.await.unwrap().is_err());
        agent.join().unwrap();
    });
    assert_eq!(
        std::fs::read(root.path.join("sample.txt")).unwrap(),
        b"before\n"
    );
    let other = roots::OwnedRoot::new();
    let session = CodingSession::create(
        &other.path,
        "drop-owner",
        super::super::CodingCapability::ReadOnly,
    )
    .unwrap();
    let token = EditCancellation::default();
    let agent = CodingAgent::start(session, None, token.clone(), "").unwrap();
    assert!(super::super::reconstruct_coding(&other.path, "drop-owner").is_err());
    drop(agent);
    assert!(token.is_cancelled());
    assert!(
        super::super::reconstruct_coding(&other.path, "drop-owner")
            .unwrap()
            .is_empty()
    );
}

fn state(root: &roots::OwnedRoot) -> WorkerState {
    let cancellation = EditCancellation::default();
    let shared = Shared::new(1, cancellation.clone());
    let mut session = CodingSession::create(
        &root.path,
        "observed-fault",
        super::super::CodingCapability::ControlledEdit,
    )
    .unwrap();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = Some(session.approve_exact(&intent).unwrap());
    session.observation = Some(shared.clone());
    WorkerState {
        session,
        approval,
        cancellation,
        turn: 0,
        read_target: None,
        edited: None,
        verified: false,
        shared,
        interactive: false,
        follow_up: false,
        prompt: Arc::from(""),
        prior: None,
        exact_target: None,
        project: None,
    }
}
fn step(state: &mut WorkerState, turn: u64, text: &str) -> Result<Step, CodingError> {
    state.material(turn, 0, "Request", "Pending", "{}".into())?;
    state.execute(turn, RequestId(turn), text.into(), true)
}
const READ: &str = r#"{"kind":"Read","op":1,"target":"sample.txt"}"#;
const REPLACE: &str = r#"{"kind":"Replace","op":2,"target":"sample.txt","expected":"before\n","replacement":"after\n"}"#;
const VERIFY: &str = r#"{"kind":"Verify","op":3,"target":"sample.txt","expected":"after\n"}"#;
#[test]
fn live_material_checkpoints_preserve_acknowledged_effect_and_verification() {
    use super::super::{Checkpoint, CheckpointPhase as Phase, journal::JournalFault};
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    for phase in [
        Phase::BeforeAuthorization,
        Phase::AfterAuthorization,
        Phase::BeforeCompletion,
        Phase::AfterCompletion,
        Phase::BeforeVerificationMaterial,
        Phase::BeforeFinalMaterial,
    ] {
        let root = roots::OwnedRoot::new();
        let mut state = state(&root);
        step(&mut state, 1, READ).unwrap();
        state.session.checkpoint = Some(Checkpoint::Journal(phase, JournalFault::Sync));
        let replacement = step(&mut state, 2, REPLACE);
        if matches!(
            phase,
            Phase::BeforeVerificationMaterial | Phase::BeforeFinalMaterial
        ) {
            replacement.unwrap();
            let verified = step(&mut state, 3, VERIFY);
            if phase == Phase::BeforeFinalMaterial {
                verified.unwrap();
                assert!(step(&mut state, 4, r#"{"kind":"Final","summary":"done"}"#).is_err());
            } else {
                assert!(verified.is_err());
            }
        } else {
            assert!(replacement.is_err());
        }
        let observed = CodingObserver(state.shared.clone()).snapshot();
        assert_eq!(observed.attempts[0].material, Some("Read"));
        assert_eq!(observed.authorized, phase != Phase::BeforeAuthorization);
        assert_eq!(
            observed.dispatched,
            !matches!(
                phase,
                Phase::BeforeAuthorization | Phase::AfterAuthorization
            )
        );
        assert_eq!(
            observed.effect,
            match phase {
                Phase::BeforeAuthorization => None,
                Phase::AfterAuthorization => Some(OperationState::Authorized),
                Phase::BeforeCompletion => Some(OperationState::Uncertain),
                _ => Some(OperationState::Completed),
            }
        );
        assert_eq!(observed.verified, phase == Phase::BeforeFinalMaterial);
        assert!(observed.final_summary.is_none());
        assert_eq!(
            state.session.read("sample.txt").unwrap(),
            if observed.dispatched {
                "after\n"
            } else {
                "before\n"
            }
        );
        drop(state);
        let history =
            super::super::reconstruct_coding_history(&root.path, "observed-fault").unwrap();
        assert_eq!(
            history.operations(),
            super::super::reconstruct_coding(&root.path, "observed-fault").unwrap()
        );
        if phase == Phase::BeforeFinalMaterial {
            assert_eq!(
                history.ending(),
                super::super::CodingHistoryEnding::RecordedFinal
            );
            assert_eq!(
                history.operations().last().unwrap().state,
                OperationState::Completed
            );
            assert!(observed.final_summary.is_none());
        }
    }
}
#[test]
fn worker_panic_lost_reply_is_explicit_join_and_retains_dispatched_uncertainty() {
    use super::super::{Checkpoint, CheckpointPhase};
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let root = roots::OwnedRoot::new();
    let mut state = state(&root);
    state.session.checkpoint = Some(Checkpoint::Panic(CheckpointPhase::StageWrite));
    let session = state.session;
    let agent =
        CodingAgent::start(session, state.approval, EditCancellation::default(), "").unwrap();
    let observer = agent.observer();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let mut agent = agent;
        for (turn, text) in [(1, READ), (2, REPLACE)] {
            let (reply, receive) = oneshot::channel();
            agent
                .sender
                .as_ref()
                .unwrap()
                .try_send(Work::Request {
                    turn,
                    text: "{}".into(),
                    reply,
                })
                .unwrap();
            receive.await.unwrap().unwrap();
            let (reply, receive) = oneshot::channel();
            agent
                .sender
                .as_ref()
                .unwrap()
                .try_send(Work::Response {
                    turn,
                    id: RequestId(turn),
                    text: text.into(),
                    success: true,
                    reply,
                })
                .unwrap();
            let result = receive.await;
            if turn == 1 {
                result.unwrap().unwrap();
            } else {
                assert!(result.is_err());
            }
        }
        assert!(agent.join().is_err());
    });
    let observed = observer.snapshot();
    assert!(observed.worker_terminated && !observed.worker_joined && observed.error.is_some());
    assert_eq!(observed.effect, Some(OperationState::Uncertain));
    assert_eq!(
        super::super::reconstruct_coding(&root.path, "observed-fault").unwrap()[0].state,
        OperationState::Uncertain
    );
}
#[test]
fn blocked_storage_owner_stop_does_not_make_a_false_join_receipt() {
    use super::super::{Checkpoint, CheckpointPhase};
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let root = roots::OwnedRoot::new();
    let mut state = state(&root);
    let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    state.session.checkpoint = Some(Checkpoint::Wait(
        CheckpointPhase::StageSync,
        gate.clone(),
        entered.clone(),
    ));
    let agent = CodingAgent::start(
        state.session,
        state.approval,
        EditCancellation::default(),
        "",
    )
    .unwrap();
    let observer = agent.observer();
    let control = agent.control();
    let worker = std::thread::spawn(move || {
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor.block_on(async {
            let mut agent = agent;
            for (turn, text) in [(1, READ), (2, REPLACE)] {
                let (reply, receive) = oneshot::channel();
                agent
                    .sender
                    .as_ref()
                    .unwrap()
                    .try_send(Work::Request {
                        turn,
                        text: "{}".into(),
                        reply,
                    })
                    .unwrap();
                receive.await.unwrap().unwrap();
                let (reply, receive) = oneshot::channel();
                agent
                    .sender
                    .as_ref()
                    .unwrap()
                    .try_send(Work::Response {
                        turn,
                        id: RequestId(turn),
                        text: text.into(),
                        success: true,
                        reply,
                    })
                    .unwrap();
                receive.await.unwrap().unwrap();
            }
            agent.join().unwrap();
        });
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !entered.load(Ordering::Acquire) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    control.stop();
    assert!(!observer.snapshot().worker_terminated && !observer.snapshot().worker_joined);
    assert_eq!(observer.snapshot().effect, Some(OperationState::Uncertain));
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    worker.join().unwrap();
    assert!(observer.snapshot().worker_joined);
    assert_eq!(observer.snapshot().effect, Some(OperationState::Completed));
}

#[test]
fn strict_decoder_does_not_accept_positional_nested_duplicate_or_overlong_arguments() {
    for text in [
        r#"["Read",1,"sample.txt"]"#.to_owned(),
        format!(r#"{{"kind":"Read","op":1,"target":"{}"}}"#, "x".repeat(65)),
        format!(
            r#"{{"kind":"Read","op":1,"target":"sample.txt","unknown":[{}0]}}"#,
            "0,".repeat(7000)
        ),
        format!(
            r#"{{"op":1,"target":"sample.txt","kind":"Read","kind":{}0{}}}"#,
            "[".repeat(500),
            "]".repeat(500)
        ),
        r#"{"kind":"Read","op":1,"target":"sample.txt","replacement":"x"}"#.to_owned(),
        r#"{"kind":"Final","summary":"x","op":1}"#.to_owned(),
    ] {
        assert!(serde_json::from_str::<Action>(&text).is_err());
    }
    assert!(
        serde_json::from_str::<Action>(r#"{"target":"sample.txt","op":1,"kind":"Read"}"#).is_ok()
    );
}

async fn completed_follow_agent(
    root: &roots::OwnedRoot,
    checkpoint: Option<super::super::Checkpoint>,
) -> CodingAgent {
    let mut session = CodingSession::create(
        &root.path,
        "follow-checkpoint",
        super::super::CodingCapability::ControlledEdit,
    )
    .unwrap();
    session.checkpoint = checkpoint;
    let mut agent =
        CodingAgent::start_follow_up(session, EditCancellation::default(), "after\n").unwrap();
    for (turn, response) in [
        (1, READ),
        (2, REPLACE),
        (3, VERIFY),
        (4, r#"{"kind":"Final","summary":"done"}"#),
    ] {
        let previous = match turn {
            1 => "",
            2 => "before\n",
            3 => "replacement durably completed",
            _ => "exact content verified",
        };
        let (reply, receive) = oneshot::channel();
        agent
            .sender
            .as_ref()
            .unwrap()
            .try_send(Work::Request {
                turn,
                text: serialize_context("follow-checkpoint", turn, "after\n", previous).unwrap(),
                reply,
            })
            .unwrap();
        receive.await.unwrap().unwrap();
        let (reply, receive) = oneshot::channel();
        agent
            .sender
            .as_ref()
            .unwrap()
            .try_send(Work::Response {
                turn,
                id: RequestId(turn),
                text: response.into(),
                success: true,
                reply,
            })
            .unwrap();
        if turn == 2 {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                if let Some(proposal) = &agent.observer().snapshot().proposal {
                    agent
                        .control()
                        .decide(proposal, CodingDecision::Approve)
                        .unwrap();
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                tokio::task::yield_now().await;
            }
        }
        let step = receive.await.unwrap().unwrap();
        if turn == 4 {
            agent.prior = step.prior;
        }
    }
    agent.ran = true;
    agent.shared.update(|o| o.run_ended = true);
    agent
}

#[test]
fn follow_up_exact_context_boundary_and_nested_schema_are_strict() {
    let prior = PriorContext {
        prompt: Arc::from("x".repeat(1024)),
        summary: Arc::from("done"),
        target: Arc::from("sample.txt"),
        operation: 1,
        verified: true,
    };
    let session = "s".repeat(64);
    let previous = "\n".repeat(1024);
    let overhead = serialize_follow_context(&session, 8, &prior, "", &previous)
        .unwrap()
        .len();
    let prompt = "x".repeat(4096 - overhead);
    assert!(prompt.len() <= 1024);
    assert_eq!(
        serialize_follow_context(&session, 8, &prior, &prompt, &previous)
            .unwrap()
            .len(),
        4096
    );
    validate_follow_prompt(&session, &prior, &prompt).unwrap();
    assert!(validate_follow_prompt(&session, &prior, &format!("{prompt}x")).is_err());
    let text = serialize_follow_context(&session, 8, &prior, &prompt, &previous).unwrap();
    assert!(serde_json::from_str::<OwnedFollowContext>(&text).is_ok());
    for bad in [
        text.replacen("\"invocation\":2", "\"invocation\":2,\"invocation\":2", 1),
        text.replacen("\"operation\":1", "\"operation\":1,\"unknown\":0", 1),
        text.replacen(
            "\"verified\":true",
            "\"verified\":true,\"verified\":true",
            1,
        ),
        "[\"s\",8,2,{},\"p\",\"\"]".into(),
        text.replacen("\"operation\":1", "\"operation\":[]", 1),
    ] {
        assert!(serde_json::from_str::<OwnedFollowContext>(&bad).is_err());
    }
}

#[test]
fn follow_up_marker_faults_and_preexisting_error_never_publish_new_authority() {
    use super::super::{Checkpoint, CheckpointPhase, CodingHistoryEnding, journal::JournalFault};
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    executor.block_on(async {
        for fault in [
            JournalFault::Append,
            JournalFault::ShortWrite,
            JournalFault::Sync,
        ] {
            let root = roots::OwnedRoot::new();
            let mut agent = completed_follow_agent(
                &root,
                Some(Checkpoint::Journal(
                    CheckpointPhase::BeforeFollowUpMaterial,
                    fault,
                )),
            )
            .await;
            assert!(agent.transition("new").await.is_err());
            assert_eq!(agent.observer().snapshot().invocation, 1);
            assert!(agent.cancellation.is_cancelled());
            assert!(agent.validate_follow_up("new").is_err());
            agent.finish().unwrap();
            let h = super::super::reconstruct_coding_history(&root.path, "follow-checkpoint");
            match fault {
                JournalFault::Append => {
                    assert_eq!(h.unwrap().ending(), CodingHistoryEnding::RecordedFinal)
                }
                JournalFault::ShortWrite => assert!(h.is_err()),
                JournalFault::Sync => {
                    assert_eq!(h.unwrap().ending(), CodingHistoryEnding::Incomplete)
                }
            }
        }
        let root = roots::OwnedRoot::new();
        let agent = completed_follow_agent(&root, None).await;
        let before = std::fs::read(root.path.join(".coding-journal")).unwrap();
        agent.shared.failure("known prior failure");
        assert!(agent.validate_follow_up("new").is_err());
        agent.finish().unwrap();
        assert_eq!(
            before,
            std::fs::read(root.path.join(".coding-journal")).unwrap()
        );
    });
}

#[test]
fn dropped_transition_before_or_after_generation_publication_cancels_original_owner() {
    use super::super::{Checkpoint, CheckpointPhase, CodingHistoryEnding};
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    executor.block_on(async {
        for after_send in [false, true] {
            let root = roots::OwnedRoot::new();
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut agent = completed_follow_agent(
                &root,
                Some(Checkpoint::Wait(
                    CheckpointPhase::AfterFollowUpMaterial,
                    gate.clone(),
                    entered.clone(),
                )),
            )
            .await;
            let observer = agent.observer();
            let mut future = Box::pin(agent.transition("new"));
            std::future::poll_fn(|cx| {
                assert!(future.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !entered.load(Ordering::Acquire) {
                assert!(std::time::Instant::now() < deadline);
                tokio::task::yield_now().await;
            }
            if after_send {
                *gate.0.lock().unwrap() = true;
                gate.1.notify_all();
                while observer.snapshot().invocation != 2 {
                    assert!(std::time::Instant::now() < deadline);
                    tokio::task::yield_now().await;
                }
            }
            drop(future);
            let cancelled = agent.cancellation.is_cancelled();
            let failed = observer.snapshot().error.is_some();
            *gate.0.lock().unwrap() = true;
            gate.1.notify_all();
            agent.finish().unwrap();
            assert!(cancelled && failed);
            assert!(observer.snapshot().worker_joined);
            let h =
                super::super::reconstruct_coding_history(&root.path, "follow-checkpoint").unwrap();
            assert_eq!(h.ending(), CodingHistoryEnding::Incomplete);
            assert_eq!(h.operations().len(), 1);
            assert_eq!(
                std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                "after\n"
            );
        }
    });
}

use super::super::tests::model_fixture as command_fixture;
#[test]
fn command_cancellation_and_acknowledgement_faults_preserve_actual_dispatch_boundary() {
    use super::super::{Checkpoint, CheckpointPhase as Phase, CodingCapability};
    use crate::{CommandCompletion, CommandPolicy, CommandSession};
    let _gate = super::super::tests::PROCESS_GATE.read().unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        let _owner = crate::runtime::tests::TEST_OWNER.lock().await;
        for case in 0..7 {
            let definitions = roots::OwnedRoot::new();
            let inputs = roots::OwnedRoot::new();
            let root = roots::OwnedRoot::new();
            std::fs::write(definitions.path.join("commands.json"), r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt"}]}"#).unwrap();
            let mut command = CommandSession::load(&definitions.path, &inputs.path, CommandPolicy::Pause).unwrap();
            let id = command.begin_coding("replace", "after\n").unwrap();
            let name = command.snapshot().invocations[0].as_ref().unwrap().coding.as_ref().unwrap().session.clone();
            let mut session = CodingSession::create(&root.path, &name, CodingCapability::ControlledEdit).unwrap();
            let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
            session.checkpoint = match case {
                0 => Some(Checkpoint::Wait(Phase::BeforeDispatch, gate.clone(), entered.clone())),
                1 => Some(Checkpoint::Wait(Phase::AfterCompletion, gate.clone(), entered.clone())),
                2 => Some(Checkpoint::Fail(Phase::BeforeCompletion)),
                3 => Some(Checkpoint::Panic(Phase::AfterCompletion)),
                4 => Some(Checkpoint::Journal(Phase::BeforeVerificationMaterial, super::super::journal::JournalFault::Sync)),
                5 => Some(Checkpoint::Journal(Phase::BeforeFinalMaterial, super::super::journal::JournalFault::Append)),
                _ => { session.journal.fault = Some(super::super::journal::JournalFault::Sync); None },
            };
            let mut adapter = command.start_coding(id, session).unwrap();
            let observer = adapter.observer();
            let control = adapter.control();
            let cancellation = command.control();
            let model = command_fixture::ModelFixture::start(command_fixture::golden()).await;
            let runtime = Runtime::new(model.endpoint, 1, crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(), crate::SyntheticTokenBudget::new(65536, 65536, std::time::Duration::from_millis(1)).unwrap(), crate::ConcurrencyMode::Fixed).unwrap();
            let decision = async {
                if case == 6 { return; }
                loop {
                    if let Some(proposal) = &observer.snapshot().proposal { control.decide(proposal, CodingDecision::Approve).unwrap(); break; }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
                if case < 2 {
                    while !entered.load(Ordering::Acquire) { tokio::time::sleep(std::time::Duration::from_millis(1)).await; }
                    cancellation.cancel();
                    *gate.0.lock().unwrap() = true;
                    gate.1.notify_all();
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), async { tokio::join!(adapter.run_unjoined(&runtime), decision).0.unwrap(); }).await.unwrap();
            let mut receipt = adapter.finish();
            command.accept_coding(&mut receipt).unwrap();
            let snapshot = command.snapshot();
            let execution = snapshot.invocations[0].as_ref().unwrap().coding.as_ref().unwrap().execution.as_ref().unwrap();
            assert_eq!(execution.disposition, if case < 2 { CommandCompletion::Cancelled } else { CommandCompletion::Incomplete }, "case {case}");
            assert_eq!(execution.observation.effect, match case { 0 => Some(OperationState::NotDispatched), 2 => Some(OperationState::Uncertain), 6 => None, _ => Some(OperationState::Completed) });
            assert!(command.report(id, CommandCompletion::Completed, "false success", None).is_err());
            command.report(id, execution.disposition, "actual settlement retained", None).unwrap();
            let count = runtime.snapshot().attempts;
            runtime.shutdown().await;
            model.settle(count).await;
            let requests = model.requests();
            assert_eq!(requests.len(), count as usize);
            assert!(requests.iter().all(|request| {
                let context: serde_json::Value = serde_json::from_str(request).unwrap();
                context["session"] == name.as_ref() && context["prompt"] == "after\n"
            }));
            model.stop().await;
            let source = std::fs::read(root.path.join(".coding-journal")).unwrap();
            for _ in 0..3 {
                let _ = super::super::reconstruct_coding_history(&root.path, &name);
                assert_eq!(std::fs::read(root.path.join(".coding-journal")).unwrap(), source);
            }
        }
    });
}
