use scramjet_rs::{ReaderInputs, ReaderParent, ReaderState, RequestId, Runtime};
#[path = "support/readers_root.rs"]
mod roots;
#[expect(
    dead_code,
    reason = "retained transport fixture has scenarios beyond this focused exposure test"
)]
mod support;
#[test]
fn exhausted_inherited_exposure_pauses_children_and_tree_cancel_never_clears_it() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = support::Fixture::new().await;
            let runtime = Runtime::new(
                fixture.address,
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
            let mut ids = Vec::with_capacity(4);
            for i in 0..4 {
                let mut handle = runtime
                    .submit("hold", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                    .unwrap();
                fixture.wait_started(i + 1).await;
                let id = handle.id();
                ids.push(id);
                handle.cancel();
                while handle.next_delta().await.is_some() {}
                handle.terminal().await.unwrap();
                drop(handle);
                fixture.wait_settled(id).await;
            }
            assert_eq!(runtime.snapshot().unresolved, 4);
            let root = roots::OwnedRoot::new();
            let (parent, cleanup) = ReaderParent::start(
                runtime,
                &root.path,
                ReaderInputs::new("exposure", 1, ["a", "b", "c"]).unwrap(),
            )
            .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while Runtime::process_snapshot().pending != 3 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(Runtime::process_snapshot().attempts, 4);
            parent.cancel_tree();
            let run = parent.finish().await.unwrap();
            let snapshot = cleanup.wait().await.unwrap();
            assert!(
                run.tree
                    .children
                    .iter()
                    .all(|f| f.state == ReaderState::Cancelled)
            );
            assert_eq!(
                (
                    snapshot.attempts,
                    snapshot.unresolved,
                    snapshot.active,
                    snapshot.handles
                ),
                (4, 4, 0, 0)
            );
            assert!(
                Runtime::new(
                    fixture.address,
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
            fixture.shutdown().await;
            for id in ids {
                Runtime::reconcile_fixture(id).unwrap();
            }
            let replacement = Runtime::new(
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
            replacement.shutdown().await;
        });
}
