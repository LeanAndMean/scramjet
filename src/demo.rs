#[path = "../tests/support/mod.rs"]
mod support;
use scramjet_rs::{Outcome, RequestHandle, RequestId, Runtime, Snapshot};
use std::time::{Duration, Instant};
use support::{Fixture, fd_count};
use tokio::task::JoinSet;

pub async fn run() -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(25), demonstration())
        .await
        .map_err(|_| "stream-demo exceeded its local guard".to_owned())?
}

async fn consume(mut handle: RequestHandle) -> Result<(Outcome, usize), String> {
    let mut bytes = 0;
    while let Some(delta) = handle.next_delta().await {
        bytes += delta.len();
    }
    let terminal = handle.terminal().await.ok_or("missing terminal result")?;
    Ok((terminal.outcome, bytes))
}

fn reclaimed(state: Snapshot) -> Result<(), String> {
    if (
        state.active,
        state.pending,
        state.handles,
        state.input_bytes,
        state.delta_bytes,
    ) != (0, 0, 0, 0, 0)
    {
        return Err(format!("local state not reclaimed: {state:?}"));
    }
    Ok(())
}

async fn demonstration() -> Result<(), String> {
    println!(
        "Owned loopback HTTP/1 fixture; process-local limits, provisional deltas; no retries or provider capacity claim."
    );
    println!(
        "ceilings: active=1/2/4 pending=8 leases=12 input=4096B record=1024B output=8x1024B/lease wire=131072B body=65536B attempt=5s exposure=4"
    );
    let baseline_fd = fd_count();
    let started = Instant::now();
    for ceiling in [1, 2, 4] {
        let fixture = Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            ceiling,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .map_err(|_| "fixture runtime still owns process obligations")?;
        let start = Instant::now();
        let mut consumers = JoinSet::new();
        // Submit only the bounded active+pending window, then refill from completed consumers.
        let initial = (ceiling + 8).min(12);
        for _ in 0..initial {
            let handle = runtime
                .submit("hold", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .map_err(|e| format!("submission: {e:?}"))?;
            consumers.spawn(consume(handle));
        }
        fixture.wait_started(ceiling).await;
        if fixture.started().len() != ceiling || fixture.peak() != ceiling {
            return Err("fixture overlap not demonstrated".into());
        }
        fixture.release(12);
        let mut submitted = initial;
        let mut completed = 0;
        let mut bytes = 0;
        while let Some(result) = consumers.join_next().await {
            let (outcome, count) = result.map_err(|e| e.to_string())??;
            if outcome != Outcome::Success {
                return Err(format!("batch failure: {outcome:?}"));
            }
            completed += 1;
            bytes += count;
            if submitted < 12 {
                let handle = runtime
                    .submit("hold", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                    .map_err(|e| format!("refill: {e:?}"))?;
                consumers.spawn(consume(handle));
                submitted += 1;
            }
        }
        let state = runtime.shutdown().await;
        reclaimed(state)?;
        let elapsed = start.elapsed();
        println!(
            "active_ceiling={ceiling} requests=12 elapsed_ms={:.2} requests_per_s={:.2} client_peak={} fixture_peak={} completed={completed} errors=0 cancelled=0 text_bytes={bytes} pending_peak={} leases_peak={} input_peak={} delta_items_peak={} delta_bytes_peak={} unresolved={} retries=0 fd_peak={} local_final=0",
            elapsed.as_secs_f64() * 1000.0,
            12.0 / elapsed.as_secs_f64(),
            state.active_peak,
            fixture.peak(),
            state.pending_peak,
            state.handles_peak,
            state.input_peak,
            state.delta_items_peak,
            state.delta_bytes_peak,
            state.unresolved,
            fixture.fd_peak()
        );
        fixture.shutdown().await;
        tokio::time::sleep(state.rate_full_refill).await;
    }
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .map_err(|_| "fixture runtime still owns process obligations")?;
    let phase = Instant::now();
    let failure = runtime
        .submit("incomplete", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .map_err(|e| format!("failure phase: {e:?}"))?;
    let failure_id = failure.id();
    let failure = tokio::spawn(consume(failure));
    let mut slow = runtime
        .submit("slow", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .map_err(|e| format!("slow phase: {e:?}"))?;
    while runtime.snapshot().delta_items < 8 {
        tokio::task::yield_now().await;
    }
    slow.cancel();
    let terminal = slow.terminal().await.ok_or("missing cancellation result")?;
    if terminal.outcome != Outcome::Cancelled {
        return Err("slow-consumer cancellation failed".into());
    }
    let retained = runtime.snapshot().delta_bytes;
    fixture.wait_settled(slow.id()).await;
    Runtime::reconcile_fixture(slow.id()).map_err(str::to_owned)?;
    drop(slow);
    let (outcome, _) = failure.await.map_err(|e| e.to_string())??;
    if outcome != Outcome::Incomplete {
        return Err("failure phase did not fail truthfully".into());
    }
    fixture.wait_settled(failure_id).await;
    let unresolved_before = runtime.snapshot().unresolved;
    Runtime::reconcile_fixture(failure_id).map_err(str::to_owned)?;
    let state = runtime.shutdown().await;
    reclaimed(state)?;
    println!(
        "pressure requests=2 elapsed_ms={:.2} client_peak={} fixture_peak={} completed=0 errors=1 cancelled=1 slow_retained_bytes={retained} delta_bytes_peak={} unresolved_before_reconciliation={unresolved_before} unresolved_final={} retries=0 fd_peak={} local_final=0",
        phase.elapsed().as_secs_f64() * 1000.0,
        state.active_peak,
        fixture.peak(),
        state.delta_bytes_peak,
        state.unresolved,
        fixture.fd_peak()
    );
    fixture.shutdown().await;
    println!(
        "total_elapsed_ms={:.2} fd_baseline={baseline_fd} fd_post_shutdown={} (executor/fixture descriptors included; RSS measured separately)",
        started.elapsed().as_secs_f64() * 1000.0,
        fd_count()
    );
    Ok(())
}
