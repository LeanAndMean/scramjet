use crate::cooldown_fixture as fixture;
use scramjet_rs::{
    Outcome, RequestHandle, RequestRate, Runtime, SyntheticCost, SyntheticTokenBudget,
    SyntheticUsage,
};
use std::time::{Duration, Instant};
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(25), journey(Duration::ZERO)).await??;
    Ok(())
}
async fn finish(h: &mut RequestHandle, outcome: Outcome) {
    let t = h.terminal().await.unwrap();
    assert_eq!(t.id, h.id());
    assert_eq!(t.outcome, outcome);
    assert_eq!(
        t.synthetic_usage,
        if outcome == Outcome::Success {
            SyntheticUsage::Known(1)
        } else {
            SyntheticUsage::NotAttempted
        }
    );
    assert!(h.terminal().await.is_none());
}
async fn journey(observer_delay: Duration) -> Result<(), Box<dyn std::error::Error>> {
    let start = Instant::now();
    let fixture = fixture::Fixture::new().await;
    let origin = tokio::time::Instant::now();
    let r = Runtime::new(
        fixture.address,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(8, 2, Duration::from_millis(250)).unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let cost = SyntheticCost::new(1, 1).unwrap();
    let mut hs = Vec::with_capacity(10);
    for i in 0..10 {
        hs.push(r.submit(&format!("{i}:ok"), cost).unwrap());
    }
    fixture.wait_started(2).await;
    assert_eq!((r.snapshot().active, fixture.metrics().0), (2, 2));
    hs[2].cancel();
    hs[5].cancel();
    finish(&mut hs[2], Outcome::Cancelled).await;
    finish(&mut hs[5], Outcome::Cancelled).await;
    let mut latency = Vec::with_capacity(8);
    let mut token_wait_observed = false;
    fixture.release(0);
    finish(&mut hs[0], Outcome::Success).await;
    latency.push(start.elapsed().as_millis());
    for (n, i) in [3, 4, 6, 7, 8, 9].into_iter().enumerate() {
        fixture.wait_started(n + 3).await;
        assert!(hs[i].commitment().is_some());
        if i == 4 {
            tokio::time::sleep(observer_delay).await;
        }
        fixture.release(i);
        finish(&mut hs[i], Outcome::Success).await;
        latency.push(start.elapsed().as_millis());
        let s = r.snapshot();
        token_wait_observed |= s.active == 0 && s.pending > 0 && s.token_credit < 2;
        if i == 4 {
            fixture.release(1);
            finish(&mut hs[1], Outcome::Success).await;
            latency.push(start.elapsed().as_millis());
            let s = r.snapshot();
            token_wait_observed |= s.active == 0 && s.pending > 0 && s.token_credit < 2;
            if !observer_delay.is_zero() {
                fixture.wait_started(6).await;
                assert_eq!(r.snapshot().commitments, 6);
            }
        }
    }
    assert!(
        token_wait_observed,
        "missing active-zero token wait observation"
    );
    let survivors = [0, 1, 3, 4, 6, 7, 8, 9];
    let commitments: Vec<_> = survivors
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            let (order, time) = hs[i].commitment().unwrap();
            assert_eq!(order, n + 1);
            let elapsed = time.duration_since(origin);
            if n >= 4 {
                assert!(elapsed >= Duration::from_millis(250 * (n as u64 - 3)));
            }
            (hs[i].id().0, elapsed.as_millis())
        })
        .collect();
    assert!(hs[2].commitment().is_none());
    assert!(hs[5].commitment().is_none());
    let receipts = fixture.receipts();
    for receipt in &receipts {
        fixture.wait_settled(receipt.id).await;
    }
    let receipt_ms: Vec<_> = receipts
        .iter()
        .map(|v| (v.id.0, v.received.duration_since(start).as_millis()))
        .collect();
    assert_eq!(receipts.len(), survivors.len());
    let mut receipt_ids: Vec<_> = receipts.iter().map(|v| v.id).collect();
    receipt_ids.sort_by_key(|id| id.0);
    assert_eq!(receipt_ids, survivors.map(|i| hs[i].id()));
    drop(hs);
    let s = r.shutdown().await;
    let metrics = fixture.metrics();
    fixture.shutdown().await;
    assert_eq!(
        (
            s.active,
            s.pending,
            s.handles,
            s.input_bytes,
            s.delta_bytes,
            s.potential,
            s.unresolved,
            s.usage_obligations,
            s.token_debt
        ),
        (0, 0, 0, 0, 0, 0, 0, 0, 0)
    );
    assert_eq!(
        (
            s.commitments,
            s.attempts,
            s.synthetic_charges,
            s.synthetic_measured
        ),
        (8, 8, Some(16), Some(8))
    );
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "token-demo: capacity=8 quantum=2 interval_ms=250 request_burst=4 commitments=8 attempts=8 receipts=8 completed=8 cancelled_queued=2 retries=0 synthetic_measured=8 conservative_charges=16 overlap={}/{} pending_peak={} handles_peak={} input_peak={} delta_peak={} captured_bytes={} commitment_id_ms={:?} receipt_id_ms={:?} completion_latency_ms={:?} elapsed_ms={} throughput_per_s={:.2} fd_peak={} active_zero_token_wait=true joined=true local_final=0 unresolved=0 usage_obligations=0 debt=0",
        s.active_peak,
        metrics.0,
        s.pending_peak,
        s.handles_peak,
        s.input_peak,
        s.delta_bytes_peak,
        receipts.iter().map(|v| v.input).sum::<usize>(),
        commitments,
        receipt_ms,
        latency,
        start.elapsed().as_millis(),
        8.0 / seconds,
        metrics.1
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn delayed_response_and_observer_preserve_batched_commitment_evidence() {
        let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
        tokio::time::timeout(Duration::from_secs(25), journey(Duration::from_millis(650)))
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while Runtime::process_snapshot().token_credit < 8 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
