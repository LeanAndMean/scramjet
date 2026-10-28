use crate::cooldown_fixture as fixture;
use scramjet_rs::{Outcome, RequestHandle, RequestRate, Runtime, SubmitError};
use std::time::{Duration, Instant};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(25), journey()).await??;
    Ok(())
}
async fn terminal(handle: &mut RequestHandle, outcome: Outcome) {
    let terminal = handle.terminal().await.unwrap();
    assert_eq!(terminal.id, handle.id());
    assert_eq!(terminal.outcome, outcome);
    assert!(handle.terminal().await.is_none());
}
async fn journey() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let fixture = fixture::Fixture::new().await;
    let origin = tokio::time::Instant::now();
    let runtime = Runtime::new(
        fixture.address,
        2,
        RequestRate::new(2, Duration::from_millis(250)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut initial: Vec<_> = (0..2)
        .map(|i| {
            runtime
                .submit(
                    &format!("{i}:ok"),
                    scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
                )
                .unwrap()
        })
        .collect();
    let mut queued: Vec<_> = (2..10)
        .map(|i| {
            runtime
                .submit(
                    &format!("{i}:ok"),
                    scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
                )
                .unwrap()
        })
        .collect();
    assert!(matches!(
        runtime.submit("10:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap()),
        Err(SubmitError::Full)
    ));
    fixture.wait_started(2).await;
    assert_eq!((runtime.snapshot().active, fixture.metrics().0), (2, 2));
    queued[0].cancel();
    queued[3].cancel();
    terminal(&mut queued[0], Outcome::Cancelled).await;
    terminal(&mut queued[3], Outcome::Cancelled).await;
    for (i, h) in initial.iter_mut().enumerate() {
        fixture.release(i);
        terminal(h, Outcome::Success).await;
    }
    drop(initial);
    assert_eq!(
        (runtime.snapshot().active, runtime.snapshot().commitments),
        (0, 2)
    );
    let mut commitment_ms = Vec::with_capacity(6);
    for (n, index) in [1, 2, 4, 5, 6, 7].into_iter().enumerate() {
        fixture.wait_started(3 + n).await;
        let state = runtime.snapshot();
        assert_eq!(state.commitments, 3 + n);
        assert_eq!(state.last_committed_id, Some(queued[index].id()));
        let elapsed = state.last_commitment.unwrap().duration_since(origin);
        assert!(elapsed >= Duration::from_millis(250 * (n as u64 + 1)));
        commitment_ms.push(elapsed.as_millis());
        let receipt = fixture.receipts()[2 + n];
        assert_eq!(receipt.id, queued[index].id());
        fixture.release(index + 2);
        terminal(&mut queued[index], Outcome::Success).await;
    }
    let receipts = fixture.receipts();
    for receipt in &receipts {
        fixture.wait_settled(receipt.id).await;
    }
    assert_eq!((receipts.len(), runtime.snapshot().attempts), (8, 8));
    let receipt_ms: Vec<_> = receipts
        .iter()
        .map(|r| r.received.duration_since(start).as_millis())
        .collect();
    drop(queued);
    let state = runtime.shutdown().await;
    let (peak, fd_peak) = fixture.metrics();
    fixture.shutdown().await;
    assert_eq!(
        (
            state.active,
            state.pending,
            state.handles,
            state.input_bytes,
            state.delta_bytes,
            state.unresolved,
            state.potential
        ),
        (0, 0, 0, 0, 0, 0, 0)
    );
    println!(
        "pacing burst=2 interval_ms=250 commitments=8 attempts=8 receipts=8 completed=8 cancelled_queued=2 retries=0 overlap={}/{} pending_peak={} handles_peak={} input_peak={} delta_peak={} captured_bytes={} commitment_ms={:?} receipt_ms={:?} elapsed_ms={} fd_peak={} joined=true local_final=0 unresolved=0",
        state.active_peak,
        peak,
        state.pending_peak,
        state.handles_peak,
        state.input_peak,
        state.delta_bytes_peak,
        receipts.iter().map(|r| r.input).sum::<usize>(),
        commitment_ms,
        receipt_ms,
        start.elapsed().as_millis(),
        fd_peak
    );
    Ok(())
}
