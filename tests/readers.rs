#[path = "support/readers_root.rs"]
mod coding_support;
#[path = "support/readers_fixture.rs"]
mod fixture;
use coding_support::OwnedRoot;
use scramjet_rs::{
    ReaderFailure, ReaderInputs, ReaderParent, ReaderState, RequestId, Runtime, reconstruct_readers,
};
use std::sync::Mutex;
static GATE: Mutex<()> = Mutex::new(());
fn execute(f: impl std::future::Future<Output = ()>) {
    let _guard = GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f);
}

#[test]
fn cooldown_refusal_preserves_failed_child_and_delays_pending_sibling() {
    execute(async {
        let root = OwnedRoot::new();
        let mut replies = fixture::golden("cooldown-tree", 1);
        replies[0].status = 429;
        let fixture = fixture::ReadersFixture::start_with_retry_after(replies, Some(1)).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            2,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let (parent, cleanup) = ReaderParent::start(
            runtime,
            &root.path,
            ReaderInputs::new("cooldown-tree", 1, ["a", "b", "c"]).unwrap(),
        )
        .unwrap();
        fixture.wait_child(0).await;
        fixture.wait_child(1).await;
        fixture.release(0);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while Runtime::process_snapshot().cooldown_signals != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(Runtime::process_snapshot().commitments, 2);
        assert!(fixture.contexts()[2].is_none());
        fixture.release(1);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(fixture.contexts()[2].is_none());
        assert_eq!(Runtime::process_snapshot().commitments, 2);
        fixture.wait_child(2).await;
        fixture.release(2);
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        assert_eq!(
            run.tree.children[0].state,
            ReaderState::Failed(ReaderFailure::Transport)
        );
        assert!(run.tree.children.iter().all(|f| f.request_id.is_some()));
        assert_eq!(run.usage_tokens, Some(3));
        assert_eq!(run.snapshot.attempts, 3);
        for child in &run.tree.children[1..] {
            assert!(matches!(child.state, ReaderState::Completed { .. }));
        }
        let before = std::fs::read(root.path.join(".reader-journal")).unwrap();
        for _ in 0..3 {
            assert_eq!(
                reconstruct_readers(&root.path, "cooldown-tree", 1)
                    .unwrap()
                    .children,
                run.tree.children
            );
            assert_eq!(
                std::fs::read(root.path.join(".reader-journal")).unwrap(),
                before
            );
        }
        fixture.shutdown().await;
        assert_eq!(Runtime::process_snapshot().unresolved, 0);
    });
}

#[test]
fn paced_three_children_preserve_success_unknown_usage_and_no_replay() {
    execute(async {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let root = OwnedRoot::new();
        let fixture = fixture::ReadersFixture::start(fixture::golden("paced", 1)).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            2,
            scramjet_rs::RequestRate::new(1, std::time::Duration::from_millis(80)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let origin = std::time::Instant::now();
        let (parent, cleanup) = ReaderParent::start(
            runtime,
            &root.path,
            ReaderInputs::new("paced", 1, ["a", "b", "c"]).unwrap(),
        )
        .unwrap();
        for i in 0..3 {
            fixture.wait_child(i).await;
            assert!(origin.elapsed() >= std::time::Duration::from_millis(80 * i as u64));
            fixture.release(i);
        }
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        assert_eq!(
            (
                run.snapshot.commitments,
                run.snapshot.attempts,
                run.snapshot.unresolved
            ),
            (3, 3, 0)
        );
        assert_eq!(run.usage_tokens, Some(3));
        assert!(
            run.tree
                .children
                .iter()
                .all(|c| matches!(c.state, ReaderState::Completed { .. }))
        );
        let journal = std::fs::read(root.path.join(".reader-journal")).unwrap();
        for _ in 0..3 {
            assert_eq!(
                reconstruct_readers(&root.path, "paced", 1)
                    .unwrap()
                    .children,
                run.tree.children
            );
            assert_eq!(
                std::fs::read(root.path.join(".reader-journal")).unwrap(),
                journal
            );
        }
        fixture.shutdown().await;
        tokio::time::sleep(run.snapshot.rate_full_refill).await;
    });
}

#[test]
fn overlapping_keyed_readers_finish_out_of_order_and_reconstruct() {
    execute(async {
        let root = OwnedRoot::new();
        let fixture = fixture::ReadersFixture::start(fixture::golden("tree", 7)).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            2,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let inputs = ReaderInputs::new("tree", 7, ["zero", "one", "two"]).unwrap();
        let (parent, cleanup) = ReaderParent::start(runtime, &root.path, inputs).unwrap();
        let mut observer = parent.observer();
        let control = parent.control();
        assert!(control.cancel_child("foreign", 7, 0).is_err());
        drop(parent.observer());
        drop(parent.control());
        fixture.wait_child(0).await;
        fixture.wait_child(1).await;
        while Runtime::process_snapshot().pending != 1 {
            tokio::task::yield_now().await;
        }
        assert_eq!(Runtime::process_snapshot().active, 2);
        assert_eq!(Runtime::process_snapshot().pending, 1);
        assert_eq!(fixture.peak(), 2);
        fixture.release(1);
        fixture.wait_child(2).await;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while observer.snapshot().children[1].fact.is_none() {
                observer.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let live = observer.snapshot();
        assert!(!live.tree_ended);
        assert_eq!(live.children[1].acceptance_ordinal, Some(1));
        assert!(live.children[0].fact.is_none() && live.children[2].fact.is_none());
        control.cancel_child("tree", 7, 1).unwrap();
        drop(control);
        assert!(fixture.contexts()[0].as_ref().unwrap().contains("zero"));
        assert!(fixture.contexts()[1].as_ref().unwrap().contains("one"));
        assert!(fixture.contexts()[2].as_ref().unwrap().contains("two"));
        fixture.release(2);
        fixture.release(0);
        let run = parent.finish().await.unwrap();
        let snapshot = cleanup.wait().await.unwrap();
        assert_eq!(
            (
                snapshot.active,
                snapshot.pending,
                snapshot.handles,
                snapshot.unresolved
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(run.snapshot.attempts, 3);
        assert_eq!(run.usage_tokens, Some(3));
        for i in 0..3 {
            assert_eq!(
                run.tree.children[i].state,
                ReaderState::Completed {
                    summary: format!("reader-{i}")
                }
            );
        }
        let observation = observer.snapshot();
        assert!(observation.storage_joined && observation.runtime_joined && observation.tree_ended);
        assert_eq!(observation.children[1].fact, live.children[1].fact);
        assert!(observation.children.iter().all(|slot| {
            slot.accounting
                .is_some_and(|receipt| !receipt.unknown_usage && !receipt.unresolved_exposure)
        }));
        let facts = reconstruct_readers(&root.path, "tree", 7).unwrap();
        assert_eq!(facts.children, run.tree.children);
        assert!(facts.parent_finished);
        assert!(reconstruct_readers(&root.path, "tree", 8).is_err());
        fixture.shutdown().await;
    });
}

#[test]
fn child_cancellation_keeps_siblings_and_attempt_accounting() {
    execute(async {
        let root = OwnedRoot::new();
        let fixture = fixture::ReadersFixture::start(fixture::golden("cancel", 1)).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            2,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let (parent, cleanup) = ReaderParent::start(
            runtime,
            &root.path,
            ReaderInputs::new("cancel", 1, ["a", "b", "c"]).unwrap(),
        )
        .unwrap();
        fixture.wait_child(0).await;
        fixture.wait_child(1).await;
        while Runtime::process_snapshot().pending != 1 {
            tokio::task::yield_now().await;
        }
        parent.cancel_child(2).unwrap();
        parent.cancel_child(0).unwrap();
        fixture.release(1);
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        assert_eq!(run.tree.children[0].state, ReaderState::Cancelled);
        assert_eq!(run.tree.children[2].state, ReaderState::Cancelled);
        assert!(matches!(
            run.tree.children[1].state,
            ReaderState::Completed { .. }
        ));
        assert!(run.tree.children.iter().all(|f| f.request_id.is_some()));
        assert!(Runtime::process_snapshot().unresolved <= 1);
        fixture.shutdown().await;
        assert_eq!(Runtime::process_snapshot().unresolved, 0);
    });
}

#[test]
fn dropped_parent_and_aborted_finish_keep_cleanup_owned() {
    execute(async {
        for abort_finish in [false, true] {
            let root = OwnedRoot::new();
            let fixture = fixture::ReadersFixture::start(fixture::golden("drop", 1)).await;
            let runtime = Runtime::new(
                fixture.endpoint,
                2,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let (parent, cleanup) = ReaderParent::start(
                runtime,
                &root.path,
                ReaderInputs::new("drop", 1, ["a", "b", "c"]).unwrap(),
            )
            .unwrap();
            fixture.wait_child(0).await;
            fixture.wait_child(1).await;
            assert!(
                Runtime::new(
                    fixture.endpoint,
                    2,
                    scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                    scramjet_rs::SyntheticTokenBudget::new(
                        65_536,
                        65_536,
                        std::time::Duration::from_millis(1)
                    )
                    .unwrap(),
                    scramjet_rs::ConcurrencyMode::Fixed,
                )
                .is_err()
            );
            if abort_finish {
                let task = tokio::spawn(parent.finish());
                tokio::task::yield_now().await;
                task.abort();
                let _ = task.await;
            } else {
                drop(parent);
            }
            let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), cleanup.wait())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (snapshot.active, snapshot.pending, snapshot.handles),
                (0, 0, 0)
            );
            if snapshot.unresolved != 0 {
                assert!(
                    Runtime::new(
                        fixture.endpoint,
                        2,
                        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1))
                            .unwrap(),
                        scramjet_rs::SyntheticTokenBudget::new(
                            65_536,
                            65_536,
                            std::time::Duration::from_millis(1)
                        )
                        .unwrap(),
                        scramjet_rs::ConcurrencyMode::Fixed,
                    )
                    .is_err()
                );
            }
            let tree = reconstruct_readers(&root.path, "drop", 1).unwrap();
            assert!(
                tree.children
                    .iter()
                    .all(|f| f.state == ReaderState::Cancelled)
            );
            fixture.shutdown().await;
            let runtime = Runtime::new(
                "127.0.0.1:9".parse().unwrap(),
                2,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            runtime.shutdown().await;
        }
    });
}

#[test]
fn response_failures_never_publish_summaries_or_damage_siblings() {
    execute(async {
        let cases = [
        fixture::Reply::success(r#"["errors",1,0,"positional"]"#.to_owned()),
        fixture::Reply::success(r#"{"session":"errors","generation":2,"child":0,"summary":"stale"}"#.to_owned()),
        fixture::Reply::success(r#"{"session":"errors","generation":1,"child":1,"summary":"foreign"}"#.to_owned()),
        fixture::Reply::success(r#"{"session":"errors","generation":1,"child":0,"summary":"x","summary":"duplicate"}"#.to_owned()),
        fixture::Reply::success("x".repeat(20000)),
        fixture::Reply { text: "partial".to_owned(), done: false, status: 200 },
        fixture::Reply { text: "rejected".to_owned(), done: true, status: 429 },
    ];
        for reply in cases {
            let root = OwnedRoot::new();
            let mut replies = fixture::golden("errors", 1);
            replies[0] = reply;
            let fixture = fixture::ReadersFixture::start(replies).await;
            let runtime = Runtime::new(
                fixture.endpoint,
                2,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let (parent, cleanup) = ReaderParent::start(
                runtime,
                &root.path,
                ReaderInputs::new("errors", 1, ["a", "b", "c"]).unwrap(),
            )
            .unwrap();
            fixture.release_all();
            let run = parent.finish().await.unwrap();
            cleanup.wait().await.unwrap();
            assert!(matches!(run.tree.children[0].state, ReaderState::Failed(_)));
            assert!(matches!(
                run.tree.children[1].state,
                ReaderState::Completed { .. }
            ));
            assert!(matches!(
                run.tree.children[2].state,
                ReaderState::Completed { .. }
            ));
            assert_eq!(
                reconstruct_readers(&root.path, "errors", 1)
                    .unwrap()
                    .children,
                run.tree.children
            );
            fixture.shutdown().await;
        }
    });
}

#[test]
fn pre_start_tree_cancel_and_invalid_input_are_no_dispatch() {
    execute(async {
        assert!(ReaderInputs::new("", 1, [""; 3]).is_err());
        assert!(ReaderInputs::new("valid", 0, [""; 3]).is_err());
        assert!(ReaderInputs::new("valid", 1, ["\u{1}", "", ""]).is_err());
        assert!(ReaderInputs::new("valid", 1, [&"x".repeat(1025), "", ""]).is_err());
        let mut huge = String::with_capacity(4 * 1024 * 1024);
        huge.push('x');
        let inputs = ReaderInputs::new("pre", 1, [&huge, "", ""]).unwrap();
        drop(huge);
        let root = OwnedRoot::new();
        let fixture = fixture::ReadersFixture::start(fixture::golden("pre", 1)).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            2,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let (parent, cleanup) = ReaderParent::start(runtime, &root.path, inputs).unwrap();
        parent.cancel_tree();
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        assert_eq!(run.snapshot.attempts, 0);
        assert!(
            run.tree
                .children
                .iter()
                .all(|f| f.state == ReaderState::NotDispatched)
        );
        assert_eq!(
            reconstruct_readers(&root.path, "pre", 1).unwrap().children,
            run.tree.children
        );
        fixture.shutdown().await;
    });
}

#[test]
fn malformed_response_is_typed() {
    execute(async {
        let root = OwnedRoot::new();
        let mut replies = fixture::golden("typed", 1);
        replies[0] = fixture::Reply::success("{}".to_owned());
        let fixture = fixture::ReadersFixture::start(replies).await;
        fixture.release_all();
        let (parent, cleanup) = ReaderParent::start(
            Runtime::new(
                fixture.endpoint,
                2,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap(),
            &root.path,
            ReaderInputs::new("typed", 1, [""; 3]).unwrap(),
        )
        .unwrap();
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        assert_eq!(
            run.tree.children[0].state,
            ReaderState::Failed(ReaderFailure::InvalidResponse)
        );
        fixture.shutdown().await;
    });
}

#[test]
fn initialization_failure_and_wrong_runtime_bounds_never_dispatch() {
    execute(async {
        let root = OwnedRoot::new();
        std::fs::write(root.path.join(".reader-journal"), "existing").unwrap();
        let fixture = fixture::ReadersFixture::start(fixture::golden("init", 1)).await;
        let (parent, cleanup) = ReaderParent::start(
            Runtime::new(
                fixture.endpoint,
                2,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap(),
            &root.path,
            ReaderInputs::new("init", 1, [""; 3]).unwrap(),
        )
        .unwrap();
        assert!(parent.finish().await.is_err());
        let snapshot = cleanup.wait().await.unwrap();
        assert_eq!(
            (snapshot.attempts, snapshot.active, snapshot.handles),
            (0, 0, 0)
        );
        assert_eq!(
            std::fs::read(root.path.join(".reader-journal")).unwrap(),
            b"existing"
        );
        fixture.shutdown().await;
        let runtime = Runtime::new(
            "127.0.0.1:9".parse().unwrap(),
            1,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let (parent, cleanup) = ReaderParent::start(
            runtime,
            &root.path,
            ReaderInputs::new("init", 1, [""; 3]).unwrap(),
        )
        .unwrap();
        assert!(parent.finish().await.is_err());
        cleanup.wait().await.unwrap();
    });
}

#[test]
fn mixed_and_overrun_child_usage_remains_truthful_and_gates_summary() {
    execute(async {
        for overrun in [false, true] {
            let root = OwnedRoot::new();
            let fixture = fixture::ReadersFixture::start_with_usage(
                fixture::golden("token-read", 1),
                None,
                if overrun {
                    [Some(3), Some(1), Some(1)]
                } else {
                    [Some(1), None, Some(1)]
                },
            )
            .await;
            let runtime = Runtime::new(
                fixture.endpoint,
                2,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let (parent, cleanup) = ReaderParent::start(
                runtime,
                &root.path,
                ReaderInputs::new("token-read", 1, ["a", "b", "c"]).unwrap(),
            )
            .unwrap();
            fixture.release_all();
            let run = parent.finish().await.unwrap();
            cleanup.wait().await.unwrap();
            if overrun {
                assert_eq!(run.usage_tokens, Some(5));
                assert_eq!(
                    run.tree.children[0].state,
                    ReaderState::Failed(ReaderFailure::Transport)
                );
                assert_eq!(run.snapshot.synthetic_charges, Some(7));
            } else {
                assert_eq!(run.usage_tokens, None);
                assert_eq!(run.snapshot.unknown_usage, 1);
            }
            let bytes = std::fs::read(root.path.join(".reader-journal")).unwrap();
            assert_eq!(
                reconstruct_readers(&root.path, "token-read", 1)
                    .unwrap()
                    .children,
                run.tree.children
            );
            assert_eq!(
                std::fs::read(root.path.join(".reader-journal")).unwrap(),
                bytes
            );
            fixture.shutdown().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    });
}

#[test]
fn adaptive_ceiling_two_tree_continues_at_target_one_without_accepting_failure() {
    execute(async {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let root = OwnedRoot::new();
        let mut replies = fixture::golden("adaptive-tree", 1);
        replies[0].status = 502;
        let fixture = fixture::ReadersFixture::start(replies).await;
        let runtime = Runtime::new(
            fixture.endpoint,
            2,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65536,
                65536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Adaptive,
        )
        .unwrap();
        let (parent, cleanup) = ReaderParent::start(
            runtime,
            &root.path,
            ReaderInputs::new("adaptive-tree", 1, ["a", "b", "c"]).unwrap(),
        )
        .unwrap();
        fixture.wait_child(0).await;
        fixture.wait_child(1).await;
        fixture.release(0);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let s = Runtime::process_snapshot();
                if s.concurrency.target == 1 && s.active == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(Runtime::process_snapshot().concurrency.ceiling, 2);
        assert_eq!(Runtime::process_snapshot().cooldown_signals, 0);
        assert!(fixture.contexts()[2].is_none());
        fixture.release(1);
        fixture.wait_child(2).await;
        fixture.release(2);
        let run = parent.finish().await.unwrap();
        cleanup.wait().await.unwrap();
        assert_eq!(
            run.tree.children[0].state,
            ReaderState::Failed(ReaderFailure::Transport)
        );
        assert!(
            run.tree.children[1..]
                .iter()
                .all(|c| matches!(c.state, ReaderState::Completed { .. }))
        );
        assert_eq!(
            (
                run.snapshot.concurrency.ceiling,
                run.snapshot.concurrency.target
            ),
            (2, 1)
        );
        assert_eq!(
            (
                run.snapshot.active,
                run.snapshot.pending,
                run.snapshot.handles,
                run.snapshot.unresolved,
                run.snapshot.usage_obligations
            ),
            (0, 0, 0, 0, 0)
        );
        let before = std::fs::read(root.path.join(".reader-journal")).unwrap();
        for _ in 0..3 {
            assert_eq!(
                reconstruct_readers(&root.path, "adaptive-tree", 1)
                    .unwrap()
                    .children,
                run.tree.children
            );
            assert_eq!(
                std::fs::read(root.path.join(".reader-journal")).unwrap(),
                before
            );
        }
        fixture.shutdown().await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    });
}
