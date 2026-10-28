#[path = "support/cooldown_fixture.rs"]
mod fixture;
use scramjet_rs::{Outcome, RequestRate, Runtime};
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;

#[tokio::test(flavor = "current_thread")]
async fn actual_refusal_and_cancel_consume_rate_without_refund_or_retry() {
    for observation in ["prompt", "delayed-response", "delayed-receipt"] {
        let receipt_gate = Arc::new(Semaphore::new(0));
        let fixture = if observation == "delayed-receipt" {
            fixture::Fixture::new_with_receipt_gate(Some((0, receipt_gate.clone()))).await
        } else {
            fixture::Fixture::new().await
        };
        let runtime = Runtime::new(
            fixture.address,
            2,
            RequestRate::new(1, Duration::from_millis(150)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut first = runtime
            .submit("0:0", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        let origin = runtime.snapshot().last_commitment.unwrap();
        let mut cancelled = runtime
            .submit("1:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        let mut success = runtime
            .submit("2:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        cancelled.cancel();
        assert_eq!(
            cancelled.terminal().await.unwrap().outcome,
            Outcome::Cancelled
        );
        if observation == "prompt" {
            fixture.wait_started(1).await;
            fixture.release(0);
            assert_eq!(
                first.terminal().await.unwrap().outcome,
                Outcome::HttpRejected(429)
            );
        }
        if observation == "delayed-receipt" {
            fixture.wait_started(1).await;
            assert_eq!(fixture.receipts()[0].id, success.id());
            receipt_gate.add_permits(1);
        }
        fixture.wait_started(2).await;
        let state = runtime.snapshot();
        assert_eq!(state.commitments, 2);
        assert_eq!(state.last_committed_id, Some(success.id()));
        assert!(
            state.last_commitment.unwrap().duration_since(origin) >= Duration::from_millis(150)
        );
        if observation != "prompt" {
            fixture.release(0);
            assert_eq!(
                first.terminal().await.unwrap().outcome,
                Outcome::HttpRejected(429)
            );
        }
        fixture.release(2);
        assert_eq!(success.terminal().await.unwrap().outcome, Outcome::Success);
        let receipts = fixture.receipts();
        eprintln!(
            "{observation}: commitment_gap={:?}, receipts={:?}",
            state.last_commitment.unwrap().duration_since(origin),
            receipts
                .iter()
                .map(|r| (r.id, r.received.duration_since(receipts[0].received)))
                .collect::<Vec<_>>()
        );
        let mut receipt_ids = receipts.iter().map(|r| r.id).collect::<Vec<_>>();
        receipt_ids.sort_by_key(|id| id.0);
        assert_eq!(receipt_ids, vec![first.id(), success.id()]);
        assert!(receipts.iter().all(|r| r.input <= 4096));
        for r in &receipts {
            fixture.wait_settled(r.id).await;
        }
        assert!(fixture.metrics().0 <= 2);
        drop(first);
        drop(cancelled);
        drop(success);
        let final_state = runtime.shutdown().await;
        assert_eq!(
            (
                final_state.commitments,
                final_state.attempts,
                final_state.active,
                final_state.pending,
                final_state.handles,
                final_state.input_bytes,
                final_state.delta_bytes,
                final_state.potential,
                final_state.unresolved
            ),
            (2, 2, 0, 0, 0, 0, 0, 0, 0)
        );
        fixture.shutdown().await;
        tokio::time::sleep(Runtime::process_snapshot().rate_full_refill).await;
    }
}
