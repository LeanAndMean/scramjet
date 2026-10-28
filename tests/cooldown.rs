#[path = "support/cooldown_fixture.rs"]
mod fixture;
use scramjet_rs::{Outcome, RequestHandle, RetryWait, Runtime};
use std::time::{Duration, Instant};
static OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
async fn outcome(handle: &mut RequestHandle) -> scramjet_rs::Terminal {
    let result = handle.terminal().await.unwrap();
    assert_eq!(result.id, handle.id());
    assert!(handle.terminal().await.is_none());
    result
}

#[tokio::test(flavor = "current_thread")]
async fn refusals_and_proxy_errors_keep_exact_identity_without_fabricated_feedback() {
    let _owner = OWNER.lock().await;
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
    for (index, scenario) in ["missing", "invalid", "502", "504", "metadata-max"]
        .into_iter()
        .enumerate()
    {
        let mut handle = runtime
            .submit(
                &format!("{index}:{scenario}"),
                scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
            )
            .unwrap();
        fixture.wait_started(index + 1).await;
        fixture.release(index);
        let result = outcome(&mut handle).await;
        let status = match scenario {
            "502" => 502,
            "504" => 504,
            _ => 429,
        };
        assert_eq!(result.outcome, Outcome::HttpRejected(status));
        if status == 429 {
            let feedback = result.retry_after.unwrap();
            assert_eq!(feedback.wait, None);
            assert_eq!(feedback.missing, scenario == "missing");
            if scenario == "metadata-max" {
                assert_eq!(feedback.invalid, 14);
                assert!(feedback.duplicate);
            }
        } else {
            assert!(result.retry_after.is_none());
        }
        assert_eq!(runtime.snapshot().cooldown_remaining, Duration::ZERO);
        assert!(!runtime.snapshot().cooldown_indefinite);
        fixture.wait_settled(handle.id()).await;
        assert_eq!(runtime.snapshot().unresolved, 0);
        drop(handle);
    }
    let state = runtime.shutdown().await;
    let metrics = fixture.metrics();
    assert_eq!(state.attempts, 5);
    assert_eq!(state.cooldown_signals, 0);
    assert_eq!(
        (
            state.retry_after_missing,
            state.retry_after_invalid,
            state.retry_after_duplicate
        ),
        (1, 15, 1)
    );
    fixture.shutdown().await;
    println!(
        "negative_refusals=3 proxy_errors=2 attempts=5 fixture_peak={} fd_peak={} no_retry=true",
        metrics.0, metrics.1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cooldown_does_not_reconcile_four_unrelated_exposures() {
    let _owner = OWNER.lock().await;
    let fixture = fixture::Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut old: Vec<_> = (0..3)
        .map(|i| {
            runtime
                .submit(
                    &format!("{i}:stall"),
                    scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
                )
                .unwrap()
        })
        .collect();
    let mut refusal = runtime
        .submit("3:1", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    fixture.wait_started(4).await;
    fixture.release(3);
    assert_eq!(
        outcome(&mut refusal).await.outcome,
        Outcome::HttpRejected(429)
    );
    drop(refusal);
    for handle in &mut old {
        handle.cancel();
        assert_eq!(outcome(handle).await.outcome, Outcome::Cancelled);
    }
    assert_eq!(runtime.snapshot().unresolved, 3);
    let mut fourth = runtime
        .submit("4:stall", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let remaining = runtime.snapshot().cooldown_remaining;
    assert_eq!(runtime.snapshot().commitments, 4);
    tokio::time::sleep(remaining + Duration::from_millis(10)).await;
    fixture.wait_started(5).await;
    fourth.cancel();
    assert_eq!(outcome(&mut fourth).await.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().unresolved, 4);
    assert_eq!(runtime.snapshot().cooldown_remaining, Duration::ZERO);
    let mut blocked = runtime
        .submit("5:ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(fixture.receipts().len(), 5);
    assert_eq!(
        (runtime.snapshot().pending, runtime.snapshot().commitments),
        (1, 5)
    );
    for handle in &old {
        fixture.wait_settled(handle.id()).await;
    }
    fixture.wait_settled(fourth.id()).await;
    Runtime::reconcile_fixture(old[0].id()).unwrap();
    fixture.wait_started(6).await;
    fixture.release(5);
    assert_eq!(outcome(&mut blocked).await.outcome, Outcome::Success);
    for handle in &old[1..] {
        Runtime::reconcile_fixture(handle.id()).unwrap();
    }
    Runtime::reconcile_fixture(fourth.id()).unwrap();
    let receipts = fixture.receipts();
    assert_eq!(receipts.iter().map(|r| r.input).sum::<usize>(), 35);
    assert_eq!(receipts.last().unwrap().id, blocked.id());
    drop(blocked);
    drop(fourth);
    old.clear();
    let state = runtime.shutdown().await;
    assert_eq!((state.handles, state.unresolved, state.attempts), (0, 0, 6));
    let metrics = fixture.metrics();
    fixture.shutdown().await;
    println!(
        "exposure_preserved=4 expiry_grants_credits=false correlated_recovery=true overlap={} fd_peak={}",
        metrics.0, metrics.1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_owner_during_cooldown_drives_cleanup_without_waiting_for_expiry() {
    let _owner = OWNER.lock().await;
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
        .submit("0:2", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    fixture.wait_started(1).await;
    fixture.release(0);
    let result = outcome(&mut refusal).await;
    assert_eq!(
        result.retry_after.unwrap().wait,
        Some(RetryWait::Delay(Duration::from_secs(2)))
    );
    drop(refusal);
    let mut queued: Vec<_> = (1..=8)
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
    let start = Instant::now();
    drop(runtime);
    assert!(
        Runtime::new(
            fixture.address,
            4,
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
    for handle in &mut queued {
        assert_eq!(outcome(handle).await.outcome, Outcome::Cancelled);
    }
    drop(queued);
    tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            if Runtime::process_snapshot().handles == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(start.elapsed() < Duration::from_millis(500));
    assert_eq!(fixture.receipts().len(), 1);
    assert!(
        Runtime::new(
            fixture.address,
            1,
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
    let remaining = Runtime::process_snapshot().cooldown_remaining;
    assert!(remaining > Duration::from_secs(1));
    tokio::time::sleep(remaining + Duration::from_millis(10)).await;
    Runtime::new(
        fixture.address,
        1,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
    fixture.wait_settled(result.id).await;
    let receipts = fixture.receipts();
    let metrics = fixture.metrics();
    fixture.shutdown().await;
    println!(
        "drop_cancelled=8 attempts=1 retained_wait_ms={} first_receipt_age_ms={} fixture_peak={} fd_peak={} replacement_after_expiry=true",
        remaining.as_millis(),
        receipts[0].received.elapsed().as_millis(),
        metrics.0,
        metrics.1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn overlapping_feedback_takes_maximum_remaining_not_sum_or_last() {
    let _owner = OWNER.lock().await;
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
    let mut long = runtime
        .submit("0:2", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut short = runtime
        .submit("1:1", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    fixture.wait_started(2).await;
    fixture.release(0);
    outcome(&mut long).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture.release(1);
    outcome(&mut short).await;
    let state = runtime.snapshot();
    assert_eq!(state.cooldown_signals, 2);
    assert!(state.cooldown_remaining > Duration::from_millis(1500));
    assert!(state.cooldown_remaining <= Duration::from_secs(2));
    drop(long);
    drop(short);
    let start = Instant::now();
    let state = runtime.shutdown().await;
    assert!(start.elapsed() < Duration::from_millis(500));
    let remaining = state.cooldown_remaining;
    tokio::time::sleep(remaining + Duration::from_millis(10)).await;
    Runtime::new(
        fixture.address,
        2,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
    fixture.shutdown().await;
}
