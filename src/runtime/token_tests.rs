use super::*;
fn budget(c: u32, q: u32, t: Duration) -> SyntheticTokenBudget {
    SyntheticTokenBudget::new(c, q, t).unwrap()
}
fn cost(n: u32) -> SyntheticCost {
    SyntheticCost::new(n, 0).unwrap()
}
async fn spin() {
    for _ in 0..40 {
        tokio::task::yield_now().await;
    }
}
#[test]
fn validated_bounds_and_explicit_costs() {
    for (c, q, t) in [
        (0, 1, 1),
        (65_537, 1, 1),
        (1, 0, 1),
        (1, 2, 1),
        (1, 1, 0),
        (1, 1, 86_400_001),
    ] {
        assert!(SyntheticTokenBudget::new(c, q, Duration::from_millis(t)).is_err());
    }
    assert!(SyntheticCost::new(0, 0).is_err());
    assert!(SyntheticCost::new(u32::MAX, u32::MAX).is_err());
    assert_eq!(SyntheticCost::new(65_536, 65_536).unwrap().total(), 131_072);
    for t in [Duration::from_nanos(1_000_001), Duration::from_secs(86400)] {
        assert!(SyntheticTokenBudget::new(65_536, 65_536, t).is_ok());
    }
}
#[tokio::test(start_paused = true)]
async fn exact_fractional_boundaries_idle_cap_debt_first_and_huge_elapsed() {
    let t = Duration::from_nanos(1_000_001);
    let mut b = TokenBucket::new(budget(8, 2, t));
    let start = tokio::time::Instant::now();
    b.reserve(8);
    b.refresh(start + t - Duration::from_nanos(1));
    assert_eq!(b.credit, 0);
    assert_eq!(
        b.wait(start + t - Duration::from_nanos(1), 2),
        Duration::from_nanos(1)
    );
    b.refresh(start + t);
    assert_eq!(b.credit, 2);
    b.refresh(start + t * 2 + Duration::from_nanos(123));
    assert_eq!(b.credit, 4);
    assert_eq!(
        b.wait(start + t * 2 + Duration::from_nanos(123), 8),
        t * 2 - Duration::from_nanos(123)
    );
    let far = start + Duration::from_secs(1_000_000_000);
    b.refresh(far);
    assert!(b.full());
    b.refresh(far + t / 2);
    b.reserve(8);
    b.refresh(far + t / 2 + t - Duration::from_nanos(1));
    assert_eq!(b.credit, 0);
    b.refresh(far + t / 2 + t);
    assert_eq!(b.credit, 2);
    let mut b = TokenBucket::new(budget(8, 2, Duration::from_secs(1)));
    for _ in 0..4 {
        b.reserve(2);
    }
    for _ in 0..4 {
        assert!(b.actual(2, 65_536));
    }
    assert_eq!(b.debt, 4 * (65_536 - 2));
    assert_eq!(b.charges, Some(4 * 65_536));
    assert_eq!(b.measured, Some(4 * 65_536));
    b.refresh(start + Duration::from_secs(1));
    assert_eq!((b.credit, b.debt), (0, 4 * (65_536 - 2) - 2));
    b.refresh(far);
    assert!(b.full());
    b.charges = Some(u64::MAX);
    b.measured = Some(u64::MAX);
    b.reserve(1);
    b.actual(1, 1);
    assert_eq!((b.charges, b.measured), (None, None));
}
#[tokio::test(start_paused = true)]
async fn exhaustive_small_refill_and_wait_arithmetic() {
    let start = tokio::time::Instant::now();
    for capacity in 1..=8 {
        for quantum in 1..=capacity {
            for credit in 0..=capacity {
                for debt in 0..=16 {
                    for ticks in 0..=24 {
                        let mut b =
                            TokenBucket::new(budget(capacity, quantum, Duration::from_millis(1)));
                        b.credit = credit;
                        b.debt = debt;
                        let now = start + Duration::from_micros(ticks * 1000 + 123);
                        b.refresh(now);
                        let earned = ticks as u32 * quantum;
                        assert_eq!(b.debt, debt.saturating_sub(earned));
                        assert_eq!(
                            b.credit,
                            (credit + earned.saturating_sub(debt)).min(capacity)
                        );
                        for cost in 1..=capacity {
                            let missing = b.debt + cost.saturating_sub(b.credit);
                            let expected = if missing == 0 {
                                Duration::ZERO
                            } else {
                                let full =
                                    Duration::from_millis(u64::from(missing.div_ceil(quantum)));
                                if b.full() {
                                    full
                                } else {
                                    full - Duration::from_micros(123)
                                }
                            };
                            assert_eq!(b.wait(now, cost), expected);
                        }
                    }
                }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn heterogeneous_fifo_charge_cancel_and_combined_timer_policy() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let r = Runtime::new(
        endpoint,
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        budget(4, 1, Duration::from_secs(1)),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut first = r.submit("ok", cost(4)).unwrap();
    let mut head = r.submit("ok", cost(3)).unwrap();
    let mut tail = r.submit("ok", cost(1)).unwrap();
    let mut cancelled = r.submit("ok", cost(2)).unwrap();
    assert!(matches!(
        r.submit("ok", cost(5)),
        Err(SubmitError::CostTooLarge)
    ));
    assert_eq!(r.snapshot().commitments, 1);
    let initial = first.commitment().unwrap();
    assert_eq!(initial, (1, r.snapshot().last_commitment.unwrap()));
    assert!(head.commitment().is_none());
    assert!(tail.commitment().is_none());
    assert!(cancelled.commitment().is_none());
    first.cancel();
    cancelled.cancel();
    spin().await;
    assert_eq!(
        first.terminal().await.unwrap().synthetic_usage,
        SyntheticUsage::NotAttempted
    );
    assert_eq!(
        cancelled.terminal().await.unwrap().synthetic_usage,
        SyntheticUsage::NotAttempted
    );
    assert_eq!(r.snapshot().synthetic_charges, Some(4));
    {
        let mut s = r.shared.state.lock().unwrap();
        s.bucket.available = 0;
        s.bucket.checkpoint = tokio::time::Instant::now();
        s.cooldown = Some((
            RetryWait::Delay(Duration::from_secs(2)),
            tokio::time::Instant::now(),
        ));
        assert_eq!(s.timing_wait(), Some(Duration::from_secs(3)));
        s.cooldown = Some((RetryWait::Unrepresentable, tokio::time::Instant::now()));
        assert_eq!(s.timing_wait(), None);
        s.cooldown = None;
    }
    for _ in 0..2 {
        tokio::time::advance(Duration::from_secs(1)).await;
        spin().await;
        assert_eq!(r.snapshot().commitments, 1);
    }
    tokio::time::advance(Duration::from_millis(1001)).await;
    spin().await;
    assert_eq!(r.snapshot().last_committed_id, Some(head.id()));
    let promoted = head.commitment().unwrap();
    assert_eq!(promoted, (2, r.snapshot().last_commitment.unwrap()));
    head.cancel();
    spin().await;
    assert_eq!(
        head.terminal().await.unwrap().synthetic_usage,
        SyntheticUsage::NotAttempted
    );
    assert_eq!(r.snapshot().synthetic_charges, Some(7));
    tokio::time::advance(Duration::from_secs(1)).await;
    spin().await;
    assert_eq!(r.snapshot().last_committed_id, Some(tail.id()));
    tail.cancel();
    spin().await;
    tail.terminal().await.unwrap();
    assert_eq!(first.commitment(), Some(initial));
    assert_eq!(head.commitment(), Some(promoted));
    assert_eq!(
        tail.commitment(),
        Some((3, r.snapshot().last_commitment.unwrap()))
    );
    assert!(cancelled.commitment().is_none());
    drop((first, head, tail, cancelled));
    let s = r.shutdown().await;
    assert_eq!(
        (s.usage_obligations, s.attempts, s.synthetic_charges),
        (0, 0, Some(8))
    );
    assert!(
        Runtime::new(
            endpoint,
            1,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            budget(8, 8, Duration::from_millis(1)),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    tokio::time::advance(Duration::from_secs(4)).await;
    Runtime::new(
        endpoint,
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        budget(8, 8, Duration::from_millis(1)),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}
#[tokio::test(flavor = "current_thread")]
async fn unknown_usage_is_independent_bounded_retained_and_one_shot() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = support::Fixture::new().await;
    let r = Runtime::new(
        fixture.address,
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        budget(8, 8, Duration::from_millis(1)),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut hs: Vec<_> = (0..4)
        .map(|_| r.submit("header", cost(2)).unwrap())
        .collect();
    fixture.wait_started(4).await;
    assert!(Runtime::reconcile_usage_fixture(hs[0].id(), 1).is_err());
    let ids: Vec<_> = hs.iter().map(RequestHandle::id).collect();
    for h in &hs {
        h.cancel();
    }
    for h in &mut hs {
        assert_eq!(
            h.terminal().await.unwrap().synthetic_usage,
            SyntheticUsage::Unknown
        );
    }
    drop(hs);
    assert_eq!(r.snapshot().unknown_usage, 4);
    Runtime::reconcile_usage_fixture(ids[0], 0).unwrap();
    assert_eq!(r.snapshot().unresolved, 4);
    assert_eq!(r.snapshot().unknown_usage, 3);
    assert!(Runtime::reconcile_usage_fixture(ids[0], 0).is_err());
    for id in &ids {
        Runtime::reconcile_fixture(*id).unwrap();
    }
    let mut fourth = r.submit("header", cost(2)).unwrap();
    fixture.wait_started(5).await;
    fourth.cancel();
    assert_eq!(
        fourth.terminal().await.unwrap().synthetic_usage,
        SyntheticUsage::Unknown
    );
    let fourth_id = fourth.id();
    drop(fourth);
    Runtime::reconcile_fixture(fourth_id).unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        (
            r.snapshot().token_credit,
            r.snapshot().unresolved,
            r.snapshot().usage_obligations
        ),
        (8, 0, 4)
    );
    let mut queued = r.submit("ok", cost(2)).unwrap();
    spin().await;
    assert_eq!(r.snapshot().commitments, 5);
    assert_eq!(r.shared.state.lock().unwrap().timing_wait(), None);
    assert!(Runtime::reconcile_usage_fixture(fourth_id, 65_537).is_err());
    Runtime::reconcile_usage_fixture(fourth_id, 0).unwrap();
    assert!(Runtime::reconcile_usage_fixture(fourth_id, 0).is_err());
    while queued.next_delta().await.is_some() {}
    assert_eq!(
        queued.terminal().await.unwrap().synthetic_usage,
        SyntheticUsage::Known(1)
    );
    drop(queued);
    r.shutdown().await;
    assert!(
        Runtime::new(
            fixture.address,
            1,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            budget(8, 8, Duration::from_millis(1)),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    for id in &ids[1..] {
        Runtime::reconcile_usage_fixture(*id, 1).unwrap();
    }
    fixture.shutdown().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    Runtime::new(
        "127.0.0.1:1".parse().unwrap(),
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        budget(8, 8, Duration::from_millis(1)),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}
