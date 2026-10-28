#[allow(dead_code)]
#[path = "support/commands.rs"]
mod definitions;
#[path = "support/readers_fixture.rs"]
mod fixture;
#[path = "support/readers_root.rs"]
mod roots;
use scramjet_rs::*;
use std::{sync::Mutex, time::Duration};
static GATE: Mutex<()> = Mutex::new(());
const CATALOG: &str = r#"{"commands":[{"name":"summarize","outcome":"AllReaders"}]}"#;
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
fn runtime(fixture: &fixture::ReadersFixture) -> Runtime {
    Runtime::new(
        fixture.endpoint,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap()
}
fn session(command: &CommandSession) -> String {
    command.snapshot().invocations[0]
        .as_ref()
        .unwrap()
        .readers
        .as_ref()
        .unwrap()
        .session
        .to_string()
}
async fn release(fixture: &fixture::ReadersFixture, observer: &ReaderObserver) {
    fixture.wait_child(0).await;
    fixture.wait_child(1).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while Runtime::process_snapshot().pending != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(Runtime::process_snapshot().active, 2);
    fixture.release(1);
    tokio::time::timeout(Duration::from_secs(3), async {
        while observer.snapshot().children[1].acceptance_ordinal != Some(1) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.wait_child(2).await;
    fixture.release_all();
}
async fn cleanup(
    settlement: CommandReaderSettlement,
    fixture: fixture::ReadersFixture,
    observer: &ReaderObserver,
) {
    let cleaned = tokio::time::timeout(Duration::from_secs(4), settlement.wait())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleaned.active + cleaned.pending + cleaned.handles, 0);
    assert!(observer.snapshot().storage_joined && observer.snapshot().runtime_joined);
    fixture.shutdown().await;
    let post = Runtime::process_snapshot();
    assert_eq!(
        post.unresolved + post.potential + post.usage_obligations + post.token_debt as usize,
        0
    );
}
#[test]
fn golden_complete_inputs_logical_receipt_report_and_join_are_separate() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for content in ["alpha\n\t界🙂\\", &"\t\\界".repeat(204)[..1020]] {
            let (_definitions, mut command) = command();
            assert!(command.begin("summarize").is_err());
            let contents = [content, "beta", "gamma"];
            let id = command.begin_readers("summarize", contents).unwrap();
            assert!(command.begin_readers("summarize", contents).is_err());
            assert!(command.execute(id).is_err());
            let session = session(&command);
            let root = roots::OwnedRoot::new();
            let fixture = fixture::ReadersFixture::start(fixture::golden(&session, 1)).await;
            let (mut adapter, settlement) = command
                .start_readers(id, runtime(&fixture), &root.path)
                .unwrap();
            let observer = adapter.observer();
            let (receipt, ()) = tokio::join!(adapter.logical(), release(&fixture, &observer));
            let mut receipt = receipt.unwrap();
            assert!(adapter.logical().await.is_err());
            assert!(!observer.snapshot().storage_joined && !observer.snapshot().runtime_joined);
            assert!(
                command
                    .report(id, CommandCompletion::Completed, "too soon", None)
                    .is_err()
            );
            command.accept_readers(&mut receipt).unwrap();
            assert!(command.accept_readers(&mut receipt).is_err());
            let before = command.snapshot();
            let fact = before.invocations[0].as_ref().unwrap();
            assert_eq!(fact.phase, CommandPhase::AwaitingCompletion);
            assert!(fact.report.is_none());
            let execution = fact.readers.as_ref().unwrap().execution.as_ref().unwrap();
            assert_eq!(execution.disposition, CommandCompletion::Completed);
            assert_eq!(execution.usage_tokens, Some(3));
            assert_eq!(
                execution.observation.children[1].acceptance_ordinal,
                Some(1)
            );
            assert!(!execution.observation.storage_joined && !before.settled);
            for (child, context) in fixture.contexts().iter().enumerate() {
                let value: serde_json::Value =
                    serde_json::from_str(context.as_ref().unwrap()).unwrap();
                assert_eq!(value["content"], contents[child]);
                assert_eq!(value["session"], session);
                assert_eq!(value["child"], child);
            }
            assert_eq!(fixture.peak(), 2);
            command
                .report(
                    id,
                    CommandCompletion::Completed,
                    "Three summaries durably accepted; not independently verified.",
                    None,
                )
                .unwrap();
            assert!(
                command
                    .report(id, CommandCompletion::Completed, "duplicate", None)
                    .is_err()
            );
            let final_observer = command.observer();
            assert!(!command.finalize().settled);
            assert!(!observer.snapshot().storage_joined);
            cleanup(settlement, fixture, &observer).await;
            assert!(final_observer.snapshot().settled);
            let tree = reconstruct_readers(&root.path, &session, 1).unwrap();
            assert!(tree.parent_finished);
            for (child, f) in tree.children.iter().enumerate() {
                assert_eq!(execution.observation.children[child].fact.as_ref(), Some(f));
            }
            assert_eq!(
                final_observer.snapshot().invocations[0]
                    .as_ref()
                    .unwrap()
                    .phase,
                CommandPhase::Completed
            );
        }
    });
}
#[test]
fn partial_failure_child_cancel_and_stop_report_order_retain_siblings() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for mode in 0..7 {
            let (_definitions, mut command) = command();
            let id = command.begin_readers("summarize", ["a", "b", "c"]).unwrap();
            let session = session(&command);
            let root = roots::OwnedRoot::new();
            let mut replies = fixture::golden(&session, 1);
            if mode == 0 || mode == 3 || mode == 4 || mode == 6 {
                replies[0].text = "not JSON".into();
            }
            let fixture = fixture::ReadersFixture::start(replies).await;
            let (mut adapter, settlement) = command
                .start_readers(id, runtime(&fixture), &root.path)
                .unwrap();
            let observer = adapter.observer();
            fixture.wait_child(0).await;
            fixture.wait_child(1).await;
            if mode == 1 {
                adapter.control().cancel_child(&session, 1, 0).unwrap();
            }
            fixture.release_all();
            let mut receipt = adapter.logical().await.unwrap();
            command.accept_readers(&mut receipt).unwrap();
            let execution = command.snapshot().invocations[0]
                .as_ref()
                .unwrap()
                .readers
                .as_ref()
                .unwrap()
                .execution
                .clone()
                .unwrap();
            assert!(matches!(
                execution.observation.children[1]
                    .fact
                    .as_ref()
                    .unwrap()
                    .state,
                ReaderState::Completed { .. }
            ));
            let expected = if mode == 2 || mode == 5 {
                CommandCompletion::Completed
            } else {
                CommandCompletion::Failed
            };
            assert_eq!(execution.disposition, expected);
            if mode == 1 {
                assert!(execution.usage_tokens.is_none());
            }
            if mode == 3 || mode == 2 || mode == 5 || mode == 6 {
                if mode >= 5 {
                    assert!(adapter.control().cancel_tree("foreign", 1).is_err());
                    assert!(!command.snapshot().cancellation_requested);
                    adapter.control().cancel_tree(&session, 1).unwrap();
                } else {
                    command.control().cancel();
                }
                assert!(command.report(id, expected, "Stop wins", None).is_err());
                command
                    .report(
                        id,
                        CommandCompletion::Cancelled,
                        "Cancelled; child evidence retained.",
                        None,
                    )
                    .unwrap();
            } else {
                command
                    .report(id, expected, "Actual logical outcome retained.", None)
                    .unwrap();
                command.control().cancel();
            }
            let phase = command.snapshot().invocations[0].as_ref().unwrap().phase;
            assert_eq!(
                phase,
                if mode == 3 || mode == 2 || mode == 5 || mode == 6 {
                    CommandPhase::Cancelled
                } else {
                    CommandPhase::Failed
                }
            );
            drop(adapter);
            cleanup(settlement, fixture, &observer).await;
            assert_eq!(
                command.finalize().invocations[0].as_ref().unwrap().phase,
                phase
            );
        }
    });
}
#[test]
fn aborted_borrowed_wait_foreign_receipt_and_finalize_never_mint_completion() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (_other_roots, mut other) = command();
        let (_definitions, mut command) = command();
        let id = command.begin_readers("summarize", ["a", "b", "c"]).unwrap();
        other.begin_readers("summarize", ["a", "b", "c"]).unwrap();
        let session = session(&command);
        let root = roots::OwnedRoot::new();
        let fixture = fixture::ReadersFixture::start(fixture::golden(&session, 1)).await;
        let (mut adapter, settlement) = command
            .start_readers(id, runtime(&fixture), &root.path)
            .unwrap();
        let observer = adapter.observer();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), adapter.logical())
                .await
                .is_err()
        );
        fixture.release_all();
        let mut receipt = adapter.logical().await.unwrap();
        assert!(other.accept_readers(&mut receipt).is_err());
        let retained = command.observer();
        assert!(!command.finalize().settled);
        drop(receipt);
        cleanup(settlement, fixture, &observer).await;
        assert!(retained.snapshot().settled);
        assert_eq!(
            retained.snapshot().invocations[0].as_ref().unwrap().phase,
            CommandPhase::Incomplete
        );
    });
}
#[test]
fn setup_failure_is_logical_incomplete_and_cleanup_waits_for_release() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (_definitions, mut command) = command();
        let id = command.begin_readers("summarize", ["a", "b", "c"]).unwrap();
        let root = roots::OwnedRoot::new();
        std::fs::write(root.path.join(".reader-journal"), b"occupied").unwrap();
        let fixture = fixture::ReadersFixture::start(fixture::golden(&session(&command), 1)).await;
        let (mut adapter, settlement) = command
            .start_readers(id, runtime(&fixture), &root.path)
            .unwrap();
        let observer = adapter.observer();
        let mut receipt = adapter.logical().await.unwrap();
        assert!(!observer.snapshot().storage_joined);
        command.accept_readers(&mut receipt).unwrap();
        assert!(
            command
                .report(id, CommandCompletion::Failed, "no End", None)
                .is_err()
        );
        assert!(
            command
                .report(id, CommandCompletion::Completed, "no End", None)
                .is_err()
        );
        command
            .report(
                id,
                CommandCompletion::Incomplete,
                "Setup failed before End.",
                None,
            )
            .unwrap();
        cleanup(settlement, fixture, &observer).await;
        assert_eq!(
            command.finalize().invocations[0].as_ref().unwrap().phase,
            CommandPhase::Incomplete
        );
    });
}
#[test]
fn dropping_adapter_and_settlement_reclaims_owner_without_report() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (_definitions, mut command) = command();
        let id = command.begin_readers("summarize", ["a", "b", "c"]).unwrap();
        let root = roots::OwnedRoot::new();
        let fixture = fixture::ReadersFixture::start(fixture::golden(&session(&command), 1)).await;
        let (adapter, settlement) = command
            .start_readers(id, runtime(&fixture), &root.path)
            .unwrap();
        let observer = adapter.observer();
        fixture.wait_child(0).await;
        drop(adapter);
        let retained = command.observer();
        assert!(!command.finalize().settled);
        drop(settlement);
        tokio::time::timeout(Duration::from_secs(4), async {
            while !retained.snapshot().settled {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        fixture.shutdown().await;
        assert!(observer.snapshot().storage_joined && observer.snapshot().runtime_joined);
        assert_eq!(
            retained.snapshot().invocations[0].as_ref().unwrap().phase,
            CommandPhase::Incomplete
        );
    });
}
#[test]
fn strict_schema_inputs_and_cancelled_unlaunched_admission() {
    for catalog in [
        r#"{"commands":[{"name":"other","outcome":"AllReaders"}]}"#,
        r#"{"commands":[{"name":"summarize","outcome":"AllReaders","target":"sample.txt"}]}"#,
        r#"{"commands":[{"name":"summarize","outcome":"AllReaders","steps":[{"read":"left.txt"}]}]}"#,
        r#"{"commands":[["summarize","AllReaders"]]}"#,
        r#"{"commands":[{"name":"summarize","outcome":"AllReaders","outcome":"AllReaders"}]}"#,
    ] {
        let roots = definitions::Roots::new(catalog, b"", b"").unwrap();
        assert!(
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause).is_err()
        );
    }
    for policy in ["Pause", "Automatic"] {
        let catalog = format!(
            r#"{{"commands":[{{"name":"compare","outcome":"Equal","steps":[{{"read":"left.txt"}},{{"read":"right.txt"}}],"next":{{"command":"summarize","policy":"{policy}"}}}},{{"name":"summarize","outcome":"AllReaders"}}]}}"#
        );
        let roots = definitions::Roots::new(&catalog, b"", b"").unwrap();
        assert!(
            CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Automatic)
                .is_err()
        );
    }
    let (_definitions, mut command) = command();
    for text in [&"x".repeat(1025), "\x1b", "\0", "\u{85}"] {
        assert!(
            command
                .begin_readers("summarize", [text, "b", "c"])
                .is_err()
        );
        assert!(command.snapshot().invocations[0].is_none());
    }
    let id = command
        .begin_readers("summarize", [&"x".repeat(1024), "\t\n界", ""])
        .unwrap();
    command.control().cancel();
    command
        .report(
            id,
            CommandCompletion::Cancelled,
            "Stopped before launch.",
            None,
        )
        .unwrap();
    let final_state = command.finalize();
    assert!(final_state.settled && final_state.reads == 0);
}
