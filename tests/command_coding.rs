#[allow(dead_code)]
#[path = "support/commands.rs"]
mod definitions;
#[allow(dead_code)]
#[path = "support/coding_ui_fixture.rs"]
mod fixture;
#[path = "support/coding.rs"]
mod projects;
use scramjet_rs::*;
use std::{sync::Mutex, time::Duration};
static GATE: Mutex<()> = Mutex::new(());
const CATALOG: &str =
    r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt"}]}"#;
fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn command() -> (definitions::Roots, CommandSession) {
    let roots = definitions::Roots::new(CATALOG, b"", b"").unwrap();
    let command =
        CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic).unwrap();
    (roots, command)
}
async fn setup(
    command: &mut CommandSession,
    id: CommandInvocation,
    replies: Option<Vec<fixture::Reply>>,
) -> (
    projects::OwnedRoot,
    fixture::Fixture,
    Runtime,
    CommandCodingAdapter,
) {
    let name = command.snapshot().invocations[0]
        .as_ref()
        .unwrap()
        .coding
        .as_ref()
        .unwrap()
        .session
        .clone();
    let project = projects::OwnedRoot::new();
    let fixture = if let Some(replies) = replies {
        fixture::Fixture::replies_session(replies, name.clone()).await
    } else {
        fixture::Fixture::consuming_session(name.clone()).await
    }
    .unwrap();
    let runtime = Runtime::new(
        fixture.address,
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap();
    let session =
        CodingSession::create(&project.path, &name, CodingCapability::ControlledEdit).unwrap();
    let adapter = command.start_coding(id, session).unwrap();
    (project, fixture, runtime, adapter)
}
async fn decision(observer: &CodingObserver, control: &CodingControl, approve: bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(proposal) = &observer.snapshot().proposal {
                control
                    .decide(
                        proposal,
                        if approve {
                            CodingDecision::Approve
                        } else {
                            CodingDecision::Deny
                        },
                    )
                    .unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
async fn settle(runtime: Runtime, fixture: fixture::Fixture, observer: &CodingObserver) {
    let pre = runtime.shutdown().await;
    assert_eq!(pre.active + pre.pending + pre.handles, 0);
    let report = fixture.shutdown().await.unwrap();
    fixture::reconcile(&observer.snapshot(), &report).unwrap();
    let post = Runtime::process_snapshot();
    assert_eq!(post.unresolved + post.usage_obligations + post.potential, 0);
}
fn replies(input: &str) -> Vec<fixture::Reply> {
    [serde_json::json!({"kind":"Read","op":1,"target":"sample.txt"}),
    serde_json::json!({"kind":"Replace","op":2,"target":"sample.txt","expected":"before\n","replacement":input}),
    serde_json::json!({"kind":"Verify","op":3,"target":"sample.txt","expected":input}),
    serde_json::json!({"kind":"Final","summary":"done"})].into_iter().map(|value| fixture::Reply { text: value.to_string(), done: true, status: 200, usage: true }).collect()
}
#[test]
fn complete_input_controls_actual_execution_and_explicit_report_is_separate() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for input in ["first\n界 e\u{301}", "second 🙂 \\\n", &"a".repeat(1024)] {
            let (_roots, mut command) = command();
            assert!(command.begin("replace").is_err());
            let id = command.begin_coding("replace", input).unwrap();
            assert!(command.execute(id).is_err());
            assert_eq!(
                command.snapshot().invocations[0].as_ref().unwrap().phase,
                CommandPhase::Ready
            );
            let (project, fixture, runtime, mut adapter) = setup(&mut command, id, None).await;
            let observer = adapter.observer();
            let control = adapter.control();
            let (run, ()) = tokio::join!(
                adapter.run_unjoined(&runtime),
                decision(&observer, &control, true)
            );
            run.unwrap();
            let result = adapter.result().unwrap().as_ref().unwrap();
            assert_eq!(result.turns, 4);
            assert_eq!(result.usage_tokens, Some(4));
            assert_eq!(
                std::fs::read_to_string(project.path.join("sample.txt")).unwrap(),
                input
            );
            assert!(!observer.snapshot().worker_joined);
            assert!(adapter.run_unjoined(&runtime).await.is_err());
            assert!(
                command
                    .report(id, CommandCompletion::Completed, "not joined", None)
                    .is_err()
            );
            let mut receipt = adapter.finish();
            assert!(observer.snapshot().worker_joined);
            command.accept_coding(&mut receipt).unwrap();
            assert!(command.accept_coding(&mut receipt).is_err());
            let snapshot = command.snapshot();
            let fact = snapshot.invocations[0].as_ref().unwrap();
            assert_eq!(fact.phase, CommandPhase::AwaitingCompletion);
            assert!(fact.report.is_none());
            assert!(fact.coding.as_ref().unwrap().receipt_accepted);
            assert_eq!(
                fact.coding
                    .as_ref()
                    .unwrap()
                    .execution
                    .as_ref()
                    .unwrap()
                    .disposition,
                CommandCompletion::Completed
            );
            settle(runtime, fixture, &observer).await;
            let name = fact.coding.as_ref().unwrap().session.clone();
            let source = std::fs::read(project.path.join(".coding-journal")).unwrap();
            for _ in 0..3 {
                let history = reconstruct_coding_history(&project.path, &name).unwrap();
                assert_eq!(history.operations()[0].state, OperationState::Completed);
                let requests: Vec<_> = history
                    .records()
                    .iter()
                    .filter_map(|r| match &r.entry {
                        CodingHistoryEntry::Material(m) if m.event == "Request" => {
                            Some(serde_json::from_str::<serde_json::Value>(&m.text).unwrap())
                        }
                        _ => None,
                    })
                    .collect();
                assert_eq!(requests.len(), 4);
                assert!(
                    requests
                        .iter()
                        .all(|r| r["prompt"] == input && r["session"] == name.as_ref())
                );
                assert_eq!(
                    std::fs::read(project.path.join(".coding-journal")).unwrap(),
                    source
                );
            }
            command
                .report(
                    id,
                    CommandCompletion::Completed,
                    "exact replacement verified",
                    None,
                )
                .unwrap();
            let accepted = command.snapshot().invocations[0].clone();
            assert!(
                command
                    .report(id, CommandCompletion::Completed, "duplicate", None)
                    .is_err()
            );
            command.control().cancel();
            assert_eq!(command.snapshot().invocations[0], accepted);
            assert!(command.finalize().settled);
        }
    });
}
#[test]
fn strict_coding_definition_and_borrowed_input_rejections_preserve_admission() {
    for value in [
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified"}]}"#,
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"other.txt"}]}"#,
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt","steps":[{"read":"sample.txt"}]}]}"#,
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt","target":"sample.txt"}]}"#,
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt","next":{"command":"replace","policy":"Automatic"}}]}"#,
        r#"{"commands":[{"name":"read","outcome":"Readable","target":"sample.txt","steps":[{"read":"sample.txt"}]}]}"#,
        r#"{"commands":[["replace","ReplaceVerified","sample.txt"]]}"#,
        r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt","unknown":{}}]}"#,
    ] {
        let roots = definitions::Roots::new(value, b"", b"").unwrap();
        assert!(
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).is_err(),
            "{value}"
        );
    }
    let (_roots, mut command) = command();
    let before = command.snapshot();
    for input in [
        "",
        " \n ",
        "x\t",
        "x\0",
        &"a".repeat(1 << 20),
        &format!("x{}", "\n".repeat(1023)),
    ] {
        assert!(command.begin_coding("replace", input).is_err());
        assert_eq!(command.snapshot(), before);
    }
    command.begin_coding("replace", "valid").unwrap();
}
#[test]
fn unrelated_and_reordered_actions_fail_before_authority_or_extra_attempts() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for (turn, value) in [
            (0, serde_json::json!({"kind":"Read","op":1,"target":"other.txt"})),
            (1, serde_json::json!({"kind":"Replace","op":2,"target":"sample.txt","expected":"before\n","replacement":"unrelated"})),
            (1, serde_json::json!({"kind":"Read","op":2,"target":"sample.txt"})),
            (1, serde_json::json!({"kind":"Final","summary":"false success"})),
        ] {
            let (_roots, mut command) = command();
            let id = command.begin_coding("replace", "intended").unwrap();
            let mut sequence = replies("intended");
            sequence[turn].text = value.to_string();
            let (project, fixture, runtime, mut adapter) = setup(&mut command, id, Some(sequence)).await;
            adapter.run_unjoined(&runtime).await.unwrap();
            assert!(adapter.result().unwrap().is_err());
            let observer = adapter.observer();
            assert!(observer.snapshot().proposal.is_none());
            assert!(!observer.snapshot().authorized);
            assert_eq!(std::fs::read(project.path.join("sample.txt")).unwrap(), b"before\n");
            let mut receipt = adapter.finish();
            command.accept_coding(&mut receipt).unwrap();
            assert!(command.report(id, CommandCompletion::Completed, "false", None).is_err());
            command.report(id, CommandCompletion::Failed, "goal rejected", None).unwrap();
            assert_eq!(observer.snapshot().attempts.iter().filter(|a| a.request_id.is_some()).count(), turn + 1);
            settle(runtime, fixture, &observer).await;
        }
    });
}
#[test]
fn denial_and_later_verify_final_failure_preserve_actual_effect() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for failure in [1, 2, 3] {
            let (_roots, mut command) = command();
            let id = command.begin_coding("replace", "intended").unwrap();
            let mut sequence = replies("intended");
            if failure > 1 {
                sequence[failure].text = "{}".into();
            }
            let (project, fixture, runtime, mut adapter) =
                setup(&mut command, id, Some(sequence)).await;
            let observer = adapter.observer();
            let control = adapter.control();
            let (run, ()) = tokio::join!(
                adapter.run_unjoined(&runtime),
                decision(&observer, &control, failure > 1)
            );
            run.unwrap();
            assert!(adapter.result().unwrap().is_err());
            let mut receipt = adapter.finish();
            command.accept_coding(&mut receipt).unwrap();
            let fact = command.snapshot().invocations[0].clone().unwrap();
            let execution = fact.coding.unwrap().execution.unwrap();
            assert_eq!(execution.disposition, CommandCompletion::Failed);
            assert_eq!(
                execution.observation.effect,
                if failure > 1 {
                    Some(OperationState::Completed)
                } else {
                    None
                }
            );
            assert!(
                command
                    .report(id, CommandCompletion::Completed, "false", None)
                    .is_err()
            );
            command
                .report(id, CommandCompletion::Failed, "denied or failed", None)
                .unwrap();
            assert_eq!(
                std::fs::read_to_string(project.path.join("sample.txt")).unwrap(),
                if failure > 1 { "intended" } else { "before\n" }
            );
            settle(runtime, fixture, &observer).await;
        }
    });
}
#[test]
fn cancellation_before_launch_or_approval_cannot_be_lost_or_supply_approval() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (_roots, mut command) = command();
        let id = command.begin_coding("replace", "intended").unwrap();
        command.control().cancel();
        let name = command.snapshot().invocations[0]
            .as_ref()
            .unwrap()
            .coding
            .as_ref()
            .unwrap()
            .session
            .clone();
        let project = projects::OwnedRoot::new();
        let session =
            CodingSession::create(&project.path, &name, CodingCapability::ControlledEdit).unwrap();
        assert!(command.start_coding(id, session).is_err());
        assert_eq!(
            command.snapshot().invocations[0]
                .as_ref()
                .unwrap()
                .coding
                .as_ref()
                .unwrap()
                .owner,
            None
        );
        assert_eq!(
            command.finalize().invocations[0].as_ref().unwrap().phase,
            CommandPhase::Incomplete
        );
        let (_roots, mut command) = self::command();
        let id = command.begin_coding("replace", "intended").unwrap();
        let (project, fixture, runtime, mut adapter) = setup(&mut command, id, None).await;
        let observer = adapter.observer();
        let cancel = command.control();
        let cancellation = async {
            loop {
                if observer.snapshot().proposal.is_some() {
                    cancel.cancel();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        let (run, ()) = tokio::join!(adapter.run_unjoined(&runtime), cancellation);
        run.unwrap();
        assert!(matches!(
            adapter.result(),
            Some(Err(CodingError::Cancelled))
        ));
        let mut receipt = adapter.finish();
        command.accept_coding(&mut receipt).unwrap();
        command
            .report(
                id,
                CommandCompletion::Cancelled,
                "explicit cancellation",
                None,
            )
            .unwrap();
        assert!(!observer.snapshot().authorized);
        assert_eq!(
            std::fs::read(project.path.join("sample.txt")).unwrap(),
            b"before\n"
        );
        settle(runtime, fixture, &observer).await;
    });
}
#[test]
fn foreign_session_receipt_and_report_reject_without_consuming_valid_evidence() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (_roots, mut command) = command();
        let id = command.begin_coding("replace", "intended").unwrap();
        let foreign = projects::OwnedRoot::new();
        let session =
            CodingSession::create(&foreign.path, "foreign", CodingCapability::ControlledEdit)
                .unwrap();
        assert!(command.start_coding(id, session).is_err());
        let (_project, fixture, runtime, mut adapter) = setup(&mut command, id, None).await;
        let observer = adapter.observer();
        let control = adapter.control();
        let (run, ()) = tokio::join!(
            adapter.run_unjoined(&runtime),
            decision(&observer, &control, true)
        );
        run.unwrap();
        let mut receipt = adapter.finish();
        let (_other_roots, mut other) = self::command();
        let other_id = other.begin_coding("replace", "other").unwrap();
        let before = other.snapshot();
        assert!(other.accept_coding(&mut receipt).is_err());
        assert_eq!(other.snapshot(), before);
        command.accept_coding(&mut receipt).unwrap();
        assert!(
            command
                .report(other_id, CommandCompletion::Completed, "foreign", None)
                .is_err()
        );
        command
            .report(id, CommandCompletion::Completed, "valid", None)
            .unwrap();
        settle(runtime, fixture, &observer).await;
    });
}
#[test]
fn omitted_run_receipt_or_report_retains_incomplete_and_joined_owner_evidence() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for omission in [0, 1, 2] {
            let (_roots, mut command) = command();
            let id = command.begin_coding("replace", "intended").unwrap();
            let (_project, fixture, runtime, mut adapter) = setup(&mut command, id, None).await;
            let observer = adapter.observer();
            if omission > 0 {
                let control = adapter.control();
                let (run, ()) = tokio::join!(
                    adapter.run_unjoined(&runtime),
                    decision(&observer, &control, true)
                );
                run.unwrap();
            }
            if omission == 0 {
                drop(adapter);
            } else if omission == 1 {
                drop(adapter.finish());
            } else {
                command.accept_coding(&mut adapter.finish()).unwrap();
            }
            let snapshot = command.finalize();
            let fact = snapshot.invocations[0].as_ref().unwrap();
            assert_eq!(fact.phase, CommandPhase::Incomplete);
            assert!(fact.report.is_none());
            assert!(snapshot.settled);
            assert!(
                fact.coding
                    .as_ref()
                    .unwrap()
                    .execution
                    .as_ref()
                    .unwrap()
                    .observation
                    .worker_joined
            );
            settle(runtime, fixture, &observer).await;
        }
    });
}
