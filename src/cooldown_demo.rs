use crate::cooldown_fixture as fixture;
use scramjet_rs::{Outcome, RequestHandle, Runtime};
use std::time::{Duration, Instant};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(25), journey()).await??;
    Ok(())
}
async fn terminal(handle: &mut RequestHandle, expected: Outcome) {
    let terminal = handle.terminal().await.unwrap();
    assert_eq!(terminal.id, handle.id());
    assert_eq!(terminal.outcome, expected);
    assert!(handle.terminal().await.is_none());
}
async fn journey() -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let fixture = fixture::Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        2,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut refusal = runtime
        .submit("0:1", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut grandfathered = runtime
        .submit("1:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    fixture.wait_started(2).await;
    assert_eq!(runtime.snapshot().active, 2);
    fixture.release(0);
    terminal(&mut refusal, Outcome::HttpRejected(429)).await;
    let observed = Instant::now();
    let remaining = runtime.snapshot().cooldown_remaining;
    assert!(remaining > Duration::from_millis(500));
    fixture.release(1);
    terminal(&mut grandfathered, Outcome::Success).await;
    drop(refusal);
    drop(grandfathered);
    let mut head = runtime
        .submit("2:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut middle = runtime
        .submit("3:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut survivors: Vec<_> = (4..10)
        .map(|i| {
            runtime
                .submit(
                    &format!("{i}:ok"),
                    scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
                )
                .unwrap()
        })
        .collect();
    assert_eq!(runtime.snapshot().pending, 8);
    assert!(matches!(
        runtime.submit("10:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap()),
        Err(scramjet_rs::SubmitError::Full)
    ));
    head.cancel();
    middle.cancel();
    terminal(&mut head, Outcome::Cancelled).await;
    terminal(&mut middle, Outcome::Cancelled).await;
    drop(head);
    drop(middle);
    assert_eq!(runtime.snapshot().active, 0);
    assert_eq!(runtime.snapshot().commitments, 2);
    tokio::time::sleep(remaining.saturating_sub(Duration::from_millis(50))).await;
    assert_eq!(runtime.snapshot().commitments, 2);
    assert_eq!(fixture.receipts().len(), 2);
    // Hold each pair independently: FIFO refers to commitments, not concurrent socket arrival.
    for pair in 0..3 {
        fixture.wait_started(4 + pair * 2).await;
        let receipts = fixture.receipts();
        let mut actual: Vec<_> = receipts[2 + pair * 2..4 + pair * 2]
            .iter()
            .map(|r| r.id.0)
            .collect();
        actual.sort_unstable();
        let mut expected = vec![survivors[pair * 2].id().0, survivors[pair * 2 + 1].id().0];
        expected.sort_unstable();
        assert_eq!(actual, expected);
        for i in 0..2 {
            let child = pair * 2 + i;
            fixture.release(4 + child);
            terminal(&mut survivors[child], Outcome::Success).await;
        }
    }
    let receipts = fixture.receipts();
    let recovered_after = receipts[2..]
        .iter()
        .map(|r| r.received.duration_since(observed))
        .min()
        .unwrap();
    assert!(recovered_after >= remaining);
    assert_eq!(receipts.len(), 8);
    assert_eq!(runtime.snapshot().attempts, 8);
    for receipt in &receipts {
        fixture.wait_settled(receipt.id).await;
    }
    drop(survivors);
    let state = runtime.shutdown().await;
    let (peak, fd_peak) = fixture.metrics();
    fixture.shutdown().await;
    assert_eq!(
        (state.active, state.pending, state.handles, state.unresolved),
        (0, 0, 0, 0)
    );
    println!(
        "cooldown attempts=8 refused=1 completed=7 cancelled_queued=2 retries=0 overlap={}/{} pending_peak={} commitments={} input_peak={} captured_bytes={} recovery_ms={} measured_remaining_ms={} elapsed_ms={} fd_peak={} joined=true local_final=0 unresolved=0",
        state.active_peak,
        peak,
        state.pending_peak,
        state.commitments,
        state.input_peak,
        receipts.iter().map(|r| r.input).sum::<usize>(),
        recovered_after.as_millis(),
        remaining.as_millis(),
        start.elapsed().as_millis(),
        fd_peak
    );
    Ok(())
}
