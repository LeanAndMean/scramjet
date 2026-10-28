use super::*;
use adaptive::{Adaptive, Witness};
use tokio::time::{Instant, advance};

fn terminal(outcome: Outcome) -> Terminal {
    Terminal {
        id: RequestId(0),
        outcome,
        body_bytes: 0,
        wire_bytes: 0,
        text_bytes: 0,
        retry_after: None,
        synthetic_usage: SyntheticUsage::Known(1),
        dispatch_to_head: Some(Duration::from_millis(1)),
        commitment_to_terminal: None,
        output_backpressure: false,
    }
}
fn sample(a: &mut Adaptive) {
    let now = Instant::now();
    a.finish(
        &terminal(Outcome::Success),
        Some(Witness {
            generation: a.snapshot.generation,
            committed: now,
            demand: true,
        }),
        now,
    );
}

#[tokio::test(start_paused = true)]
async fn ceilings_and_fixed_diagnostics_never_change_target() {
    for ceiling in 1..=4 {
        for mode in [ConcurrencyMode::Fixed, ConcurrencyMode::Adaptive] {
            let mut a = Adaptive::new(mode, ceiling);
            assert_eq!(a.snapshot.target, ceiling);
            a.head(429, Instant::now());
            assert_eq!(
                a.snapshot.target,
                if mode == ConcurrencyMode::Fixed {
                    ceiling
                } else {
                    ceiling.div_ceil(2)
                }
            );
            assert_eq!(a.snapshot.refusals, Some(1));
            advance(Duration::from_millis(250)).await;
            for _ in 0..8 {
                sample(&mut a);
            }
            assert!(a.snapshot.target <= ceiling);
            if mode == ConcurrencyMode::Fixed {
                assert_eq!(a.snapshot.increases, Some(0));
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn decrease_guard_is_independent_of_repeated_pressure_and_floors() {
    let mut a = Adaptive::new(ConcurrencyMode::Adaptive, 4);
    a.head(429, Instant::now());
    assert_eq!(a.snapshot.target, 2);
    advance(Duration::from_millis(249)).await;
    a.head(502, Instant::now());
    assert_eq!(a.snapshot.target, 2);
    advance(Duration::from_millis(1)).await;
    a.head(504, Instant::now());
    assert_eq!(a.snapshot.target, 1);
    advance(Duration::from_secs(1)).await;
    a.head(429, Instant::now());
    assert_eq!((a.snapshot.target, a.snapshot.decreases), (1, Some(2)));
    assert_eq!(
        (a.snapshot.refusals, a.snapshot.proxy_errors),
        (Some(2), Some(2))
    );
    advance(Duration::from_millis(249)).await;
    sample(&mut a);
    assert_eq!(a.snapshot.recovery_samples, 0);
    advance(Duration::from_millis(1)).await;
    for n in 1..=3 {
        sample(&mut a);
        assert_eq!(a.snapshot.recovery_samples, n);
    }
    sample(&mut a);
    assert_eq!((a.snapshot.target, a.snapshot.recovery_samples), (2, 0));
}

#[tokio::test(start_paused = true)]
async fn freshness_gap_stale_generation_and_missing_demand_reset_streak() {
    let mut a = Adaptive::new(ConcurrencyMode::Adaptive, 4);
    let old = a.snapshot.generation;
    a.head(502, Instant::now());
    advance(Duration::from_millis(250)).await;
    for (generation, demand, age) in [
        (old, true, Duration::ZERO),
        (a.snapshot.generation, false, Duration::ZERO),
        (
            a.snapshot.generation,
            true,
            Duration::from_secs(2) + Duration::from_nanos(1),
        ),
    ] {
        sample(&mut a);
        a.finish(
            &terminal(Outcome::Success),
            Some(Witness {
                generation,
                demand,
                committed: Instant::now() - age,
            }),
            Instant::now(),
        );
        assert_eq!(a.snapshot.recovery_samples, 0);
    }
    sample(&mut a);
    advance(Duration::from_secs(2)).await;
    sample(&mut a);
    assert_eq!(a.snapshot.recovery_samples, 2);
    advance(Duration::from_secs(2) + Duration::from_nanos(1)).await;
    sample(&mut a);
    assert_eq!(a.snapshot.recovery_samples, 1);
    let now = Instant::now();
    a.finish(
        &terminal(Outcome::Success),
        Some(Witness {
            generation: a.snapshot.generation,
            demand: true,
            committed: now - Duration::from_secs(2),
        }),
        now,
    );
    assert_eq!(a.snapshot.recovery_samples, 2);
}

#[tokio::test(start_paused = true)]
async fn excluded_completions_unknown_usage_and_backpressure_never_recover() {
    let mut a = Adaptive::new(ConcurrencyMode::Adaptive, 4);
    a.head(502, Instant::now());
    advance(Duration::from_millis(250)).await;
    for outcome in [
        Outcome::Deadline,
        Outcome::Transport,
        Outcome::Cancelled,
        Outcome::Framing,
        Outcome::ResourceLimit,
        Outcome::ConsumerLost,
        Outcome::Incomplete,
        Outcome::HttpRejected(500),
    ] {
        sample(&mut a);
        a.finish(&terminal(outcome), None, Instant::now());
        assert_eq!((a.snapshot.target, a.snapshot.recovery_samples), (2, 0));
    }
    assert_eq!(a.snapshot.deadlines, Some(1));
    assert_eq!(a.snapshot.transport_errors, Some(1));
    assert_eq!(a.snapshot.cancellations, Some(1));
    assert_eq!(a.snapshot.framing_errors, Some(1));
    assert_eq!(a.snapshot.incomplete_responses, Some(1));
    assert_eq!(a.snapshot.resource_failures, Some(1));
    assert_eq!(a.snapshot.consumer_losses, Some(1));
    assert_eq!(a.snapshot.other_statuses, Some(1));
    for backpressure in [false, true] {
        sample(&mut a);
        let mut t = terminal(Outcome::Success);
        t.output_backpressure = backpressure;
        if !backpressure {
            t.synthetic_usage = SyntheticUsage::Unknown;
        }
        let now = Instant::now();
        a.finish(
            &t,
            Some(Witness {
                generation: a.snapshot.generation,
                demand: true,
                committed: now,
            }),
            now,
        );
        assert_eq!(a.snapshot.recovery_samples, 0);
    }
    advance(Duration::from_secs(86400)).await;
    assert_eq!(a.snapshot.target, 2);
}

#[tokio::test(start_paused = true)]
async fn overflow_is_unavailable_and_disables_growth_without_wrapping() {
    let mut a = Adaptive::new(ConcurrencyMode::Adaptive, 4);
    a.snapshot.generation = Some(u64::MAX);
    a.snapshot.refusals = Some(u64::MAX);
    a.head(429, Instant::now());
    assert_eq!((a.snapshot.generation, a.snapshot.refusals), (None, None));
    advance(Duration::from_secs(86400)).await;
    for _ in 0..20 {
        sample(&mut a);
    }
    assert_eq!(a.snapshot.target, 2);
    a.head(429, Instant::now());
    assert_eq!(a.snapshot.refusals, None);
    assert_eq!(a.snapshot.target, 1);
}

async fn unpolled_runtime() -> Runtime {
    let mut r = Runtime::new(
        "127.0.0.1:9".parse().unwrap(),
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    // Keep transport unpolled while exercising correlated feedback ordering.
    let driver = r.driver.take().unwrap();
    driver.abort();
    assert!(driver.await.unwrap_err().is_cancelled());
    r
}

async fn shutdown_unpolled(mut r: Runtime, mut hs: Vec<RequestHandle>) {
    for h in &hs {
        h.cancel();
    }
    r.driver = Some(tokio::spawn(schedule(
        r.shared.clone(),
        "127.0.0.1:9".parse().unwrap(),
        4,
    )));
    let s = r.shutdown().await;
    for h in &mut hs {
        assert_eq!(h.terminal().await.unwrap().outcome, Outcome::Cancelled);
    }
    drop(hs);
    advance(Duration::from_secs(1)).await;
    let settled = Runtime::process_snapshot();
    assert_eq!(settled.cooldown_remaining, Duration::ZERO);
    assert_eq!(settled.handles, 0);
    assert_eq!(
        (
            s.active,
            s.pending,
            s.unresolved,
            s.usage_obligations,
            s.attempts
        ),
        (0, 0, 0, 0, 0)
    );
}

#[tokio::test]
async fn shared_head_delayed_acceptance_uses_mutex_time_for_decrease_spacing() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let r = unpolled_runtime().await;
    let hs: Vec<_> = (0..4)
        .map(|_| r.submit("hold", SyntheticCost::new(1, 1).unwrap()).unwrap())
        .collect();
    let commitments: Vec<_> = hs.iter().map(RequestHandle::commitment).collect();
    let observed = Instant::now();
    advance(Duration::from_millis(300)).await;
    let accepted = Instant::now();
    r.shared.head(
        hs[0].id(),
        429,
        Some(RetryAfter {
            wait: Some(RetryWait::Delay(Duration::from_secs(1))),
            ..RetryAfter::default()
        }),
        observed,
    );
    let s = r.snapshot();
    assert_eq!(s.concurrency.target, 2);
    assert_eq!(
        s.concurrency.last_change,
        Some((accepted, ConcurrencyChange::Refusal))
    );
    assert_eq!(s.cooldown_remaining, Duration::from_millis(700));
    advance(Duration::from_millis(1)).await;
    r.shared.head(hs[1].id(), 502, None, Instant::now());
    assert_eq!(r.snapshot().concurrency.target, 2);
    advance(Duration::from_millis(248)).await;
    r.shared.head(hs[2].id(), 504, None, Instant::now());
    assert_eq!(r.snapshot().concurrency.decreases, Some(1));
    advance(Duration::from_millis(1)).await;
    r.shared.head(hs[3].id(), 502, None, observed);
    let s = r.snapshot();
    assert_eq!(
        (s.active, s.concurrency.target, s.concurrency.decreases),
        (4, 1, Some(2))
    );
    assert_eq!(
        s.concurrency.last_change,
        Some((Instant::now(), ConcurrencyChange::Proxy))
    );
    assert_eq!(
        hs.iter().map(RequestHandle::commitment).collect::<Vec<_>>(),
        commitments
    );
    shutdown_unpolled(r, hs).await;
}

#[tokio::test]
async fn shared_head_reversed_feedback_cannot_backdate_recovery_hold() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let r = unpolled_runtime().await;
    let hs: Vec<_> = (0..4)
        .map(|_| r.submit("hold", SyntheticCost::new(1, 1).unwrap()).unwrap())
        .collect();
    let older = Instant::now();
    r.shared.head(hs[0].id(), 502, None, older);
    advance(Duration::from_millis(300)).await;
    r.shared.head(hs[1].id(), 504, None, Instant::now());
    advance(Duration::from_millis(10)).await;
    r.shared.head(
        hs[2].id(),
        429,
        Some(RetryAfter {
            wait: Some(RetryWait::Delay(Duration::from_secs(1))),
            ..RetryAfter::default()
        }),
        older,
    );
    assert_eq!(r.snapshot().cooldown_remaining, Duration::from_millis(690));
    {
        let mut s = r.shared.state.lock().unwrap();
        s.adaptive
            .finish(&terminal(Outcome::HttpRejected(429)), None, Instant::now());
        for _ in 0..4 {
            sample(&mut s.adaptive);
        }
    }
    assert_eq!(
        (
            r.snapshot().concurrency.target,
            r.snapshot().concurrency.recovery_samples
        ),
        (1, 0)
    );
    advance(Duration::from_millis(249)).await;
    r.shared.head(
        hs[2].id(),
        429,
        Some(RetryAfter {
            wait: Some(RetryWait::Delay(Duration::from_secs(10))),
            ..RetryAfter::default()
        }),
        Instant::now(),
    );
    assert_eq!(r.snapshot().cooldown_remaining, Duration::from_millis(441));
    assert_eq!(r.snapshot().concurrency.refusals, Some(1));
    {
        let mut s = r.shared.state.lock().unwrap();
        for _ in 0..4 {
            sample(&mut s.adaptive);
        }
    }
    assert_eq!(
        (
            r.snapshot().concurrency.target,
            r.snapshot().concurrency.recovery_samples
        ),
        (1, 0)
    );
    advance(Duration::from_millis(1)).await;
    for n in 1..=3 {
        sample(&mut r.shared.state.lock().unwrap().adaptive);
        assert_eq!(
            (
                r.snapshot().concurrency.target,
                r.snapshot().concurrency.recovery_samples
            ),
            (1, n)
        );
    }
    sample(&mut r.shared.state.lock().unwrap().adaptive);
    assert_eq!(
        (
            r.snapshot().concurrency.target,
            r.snapshot().concurrency.increases
        ),
        (2, Some(1))
    );
    shutdown_unpolled(r, hs).await;
}

#[tokio::test]
async fn head_feedback_precedes_teardown_and_grandfathers_committed_work() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let f = support::Fixture::new().await;
    let r = Runtime::new(
        f.address,
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    let (mut reached, release) = tests::probe(&r, Checkpoint::Settled);
    let mut hs: Vec<_> = (0..8)
        .map(|i| {
            r.submit(
                if i == 0 { "reject" } else { "hold" },
                SyntheticCost::new(1, 1).unwrap(),
            )
            .unwrap()
        })
        .collect();
    f.wait_started(4).await;
    let commitments: Vec<_> = hs[..4].iter().map(RequestHandle::commitment).collect();
    reached.recv().await.unwrap();
    assert_eq!(
        (r.snapshot().active, r.snapshot().concurrency.target),
        (4, 2)
    );
    assert_eq!(
        hs[..4]
            .iter()
            .map(RequestHandle::commitment)
            .collect::<Vec<_>>(),
        commitments
    );
    assert!(hs[4].commitment().is_none());
    hs[4].cancel();
    assert_eq!(hs[4].terminal().await.unwrap().outcome, Outcome::Cancelled);
    release.notify_one();
    assert_eq!(
        hs[0].terminal().await.unwrap().outcome,
        Outcome::HttpRejected(429)
    );
    f.wait_settled(hs[0].id()).await;
    f.release(8);
    for (i, h) in hs.iter_mut().enumerate().skip(1) {
        if i == 4 {
            continue;
        }
        while h.next_delta().await.is_some() {}
        assert_eq!(h.terminal().await.unwrap().outcome, Outcome::Success);
    }
    let orders: Vec<_> = hs
        .iter()
        .filter_map(RequestHandle::commitment)
        .map(|v| v.0)
        .collect();
    assert_eq!(orders, (1..=7).collect::<Vec<_>>());
    drop(hs);
    tokio::time::sleep(Duration::from_millis(2)).await;
    let s = r.shutdown().await;
    assert_eq!(
        (s.active, s.pending, s.unresolved, s.usage_obligations),
        (0, 0, 0, 0)
    );
    f.shutdown().await;
}

#[tokio::test]
async fn slow_output_is_explicitly_ineligible_even_with_known_clean_usage() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let f = support::Fixture::new().await;
    let r = Runtime::new(
        f.address,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    let (mut reached, release) = tests::probe(&r, Checkpoint::OutputBackpressure);
    let mut h = r.submit("slow", SyntheticCost::new(1, 1).unwrap()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.snapshot().delta_items, 8);
    f.release(1);
    release.notify_one();
    while h.next_delta().await.is_some() {}
    let t = h.terminal().await.unwrap();
    assert_eq!(
        (t.outcome, t.synthetic_usage),
        (Outcome::Success, SyntheticUsage::Known(1))
    );
    assert!(t.output_backpressure);
    assert_eq!(r.snapshot().concurrency.ineligible_successes, Some(1));
    drop(h);
    r.shutdown().await;
    f.shutdown().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
}

#[tokio::test]
async fn demand_and_effective_slot_gate_never_bypass_independent_fixed_gates() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let r = Runtime::new(
        listener.local_addr().unwrap(),
        4,
        RequestRate::new(4, Duration::from_secs(1)).unwrap(),
        SyntheticTokenBudget::new(100, 100, Duration::from_secs(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    r.shared
        .state
        .lock()
        .unwrap()
        .adaptive
        .head(502, Instant::now());
    r.shared
        .state
        .lock()
        .unwrap()
        .adaptive
        .head(502, Instant::now() + Duration::from_millis(250));
    let mut hs: Vec<_> = (0..3)
        .map(|_| r.submit("hold", SyntheticCost::new(1, 1).unwrap()).unwrap())
        .collect();
    {
        let mut s = r.shared.state.lock().unwrap();
        assert_eq!(s.adaptive.snapshot.target, 1);
        assert!(s.demand_eligible());
        assert!(!s.slots[0].witness.unwrap().demand);
        assert!(!s.can_start(SyntheticCost::new(1, 1).unwrap()));
        assert_eq!(s.timing_wait(), None);
        s.stats.active = 0;
        assert!(s.can_start(SyntheticCost::new(1, 1).unwrap()));
        s.stats.active = 1;
        s.bucket.available = 0;
        assert!(!s.demand_eligible());
        s.bucket.available = 3;
        s.tokens.credit = 1;
        assert!(!s.demand_eligible());
        s.tokens.credit = 98;
        s.tokens.debt = 1;
        assert!(!s.demand_eligible());
        s.tokens.debt = 0;
        s.cooldown = Some((RetryWait::Unrepresentable, Instant::now()));
        assert!(!s.demand_eligible());
        s.stats.active = 0;
        assert_eq!(s.timing_wait(), None);
        s.stats.active = 1;
        s.cooldown = None;
        for n in 0..3 {
            s.exposure.push(Exposure {
                id: RequestId(u64::MAX - n),
                dispatched: true,
                unresolved: true,
            });
        }
        assert!(!s.demand_eligible());
        s.exposure.truncate(1);
        for n in 0..3 {
            s.usage.push(UsageObligation {
                id: RequestId(u64::MAX - n),
                reserved: 1,
                terminal_unknown: true,
            });
        }
        assert!(!s.demand_eligible());
        s.usage.truncate(1);
        assert!(s.demand_eligible());
    }
    for h in &hs {
        h.cancel();
    }
    for h in &mut hs {
        assert_eq!(h.terminal().await.unwrap().outcome, Outcome::Cancelled);
    }
    drop(hs);
    let s = r.shutdown().await;
    assert_eq!(s.attempts, 0);
    assert_eq!(s.concurrency.target, 1);
    assert!(
        Runtime::new(
            listener.local_addr().unwrap(),
            4,
            RequestRate::new(4, Duration::from_secs(1)).unwrap(),
            SyntheticTokenBudget::new(100, 100, Duration::from_secs(1)).unwrap(),
            ConcurrencyMode::Adaptive
        )
        .is_err()
    );
    advance(Duration::from_secs(1)).await;
    let next = Runtime::new(
        listener.local_addr().unwrap(),
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(100, 100, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    assert_eq!(next.snapshot().concurrency.target, 4);
    next.shutdown().await;
}

#[tokio::test]
async fn lowered_target_does_not_clear_unknown_exposure_usage_or_replacement_guards() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let f = support::Fixture::new().await;
    let r = Runtime::new(
        f.address,
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    for target in [2, 1] {
        let mut h = r
            .submit("reject", SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        assert_eq!(
            h.terminal().await.unwrap().outcome,
            Outcome::HttpRejected(429)
        );
        assert_eq!(r.snapshot().concurrency.target, target);
        f.wait_settled(h.id()).await;
        drop(h);
        if target == 2 {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    let mut ids = Vec::new();
    for n in 0..4 {
        let mut h = r
            .submit("header", SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        f.wait_started(n + 3).await;
        h.cancel();
        let t = h.terminal().await.unwrap();
        assert_eq!(
            (t.outcome, t.synthetic_usage),
            (Outcome::Cancelled, SyntheticUsage::Unknown)
        );
        assert!(t.dispatch_to_head.is_none());
        ids.push(h.id());
        drop(h);
    }
    assert_eq!(
        (
            r.snapshot().concurrency.target,
            r.snapshot().active,
            r.snapshot().unresolved,
            r.snapshot().unknown_usage
        ),
        (1, 0, 4, 4)
    );
    let mut queued = r.submit("ok", SyntheticCost::new(1, 1).unwrap()).unwrap();
    assert!(queued.commitment().is_none());
    assert_eq!(r.shared.state.lock().unwrap().timing_wait(), None);
    for id in &ids {
        Runtime::reconcile_fixture(*id).unwrap();
    }
    assert_eq!(
        (r.snapshot().unresolved, r.snapshot().unknown_usage),
        (0, 4)
    );
    assert!(queued.commitment().is_none());
    assert_eq!(r.shared.state.lock().unwrap().timing_wait(), None);
    f.wait_settled(ids[0]).await;
    while queued.next_delta().await.is_some() {}
    assert_eq!(queued.terminal().await.unwrap().outcome, Outcome::Success);
    assert_eq!(r.snapshot().concurrency.target, 1);
    drop(queued);
    let s = r.shutdown().await;
    assert_eq!((s.active, s.unresolved, s.unknown_usage), (0, 0, 3));
    assert!(
        Runtime::new(
            f.address,
            4,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
            ConcurrencyMode::Adaptive
        )
        .is_err()
    );
    for id in &ids[1..] {
        f.wait_settled(*id).await;
    }
    f.shutdown().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let next = Runtime::new(
        "127.0.0.1:9".parse().unwrap(),
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    assert_eq!(next.snapshot().concurrency.target, 4);
    next.shutdown().await;
}

#[tokio::test]
async fn adaptive_expired_pre_send_deadline_is_excluded_and_has_no_head_timing() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let r = Runtime::new(
        listener.local_addr().unwrap(),
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Adaptive,
    )
    .unwrap();
    let mut h = r.submit("ok", SyntheticCost::new(1, 1).unwrap()).unwrap();
    r.shared.state.lock().unwrap().slots[0].admitted =
        Some(Instant::now() - Duration::from_secs(6));
    let t = h.terminal().await.unwrap();
    assert_eq!(
        (t.outcome, t.synthetic_usage),
        (Outcome::Deadline, SyntheticUsage::NotAttempted)
    );
    assert!(t.dispatch_to_head.is_none());
    assert_eq!(
        (
            r.snapshot().attempts,
            r.snapshot().concurrency.target,
            r.snapshot().concurrency.deadlines
        ),
        (0, 4, Some(1))
    );
    drop(h);
    r.shutdown().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
}

#[test]
fn reader_parent_accepts_hard_ceiling_two_with_effective_target_one() {
    let _process = crate::coding::tests::PROCESS_GATE.read().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let _owner = tests::TEST_OWNER.lock().await;
            tokio::time::sleep(Duration::from_millis(5)).await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let r = Runtime::new(
                listener.local_addr().unwrap(),
                2,
                RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
                ConcurrencyMode::Adaptive,
            )
            .unwrap();
            r.shared
                .state
                .lock()
                .unwrap()
                .adaptive
                .head(502, Instant::now());
            assert_eq!((r.active_limit(), r.snapshot().concurrency.target), (2, 1));
            let path = std::env::temp_dir()
                .join(format!("scramjet-rs-adaptive-start-{}", std::process::id()));
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .unwrap();
            let (parent, cleanup) = crate::ReaderParent::start(
                r,
                &path,
                crate::ReaderInputs::new("adaptive-start", 1, ["a", "b", "c"]).unwrap(),
            )
            .unwrap();
            parent.cancel_tree();
            let run = parent.finish().await.unwrap();
            cleanup.wait().await.unwrap();
            assert_eq!(
                (
                    run.snapshot.attempts,
                    run.snapshot.concurrency.ceiling,
                    run.snapshot.concurrency.target
                ),
                (0, 2, 1)
            );
            assert!(
                run.tree
                    .children
                    .iter()
                    .all(|c| c.state == crate::ReaderState::NotDispatched)
            );
            assert_eq!(
                crate::reconstruct_readers(&path, "adaptive-start", 1)
                    .unwrap()
                    .children,
                run.tree.children
            );
            std::fs::remove_dir_all(path).unwrap();
        });
}
