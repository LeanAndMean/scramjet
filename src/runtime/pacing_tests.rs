use super::*;

fn rate(burst: u8, interval: Duration) -> RequestRate {
    RequestRate::new(burst, interval).unwrap()
}
async fn spin() {
    for _ in 0..40 {
        tokio::task::yield_now().await;
    }
}

#[test]
fn configuration_bounds_and_exact_fractional_interval() {
    for burst in [0, 5, u8::MAX] {
        assert_eq!(
            RequestRate::new(burst, Duration::from_secs(1)),
            Err(InvalidRequestRate)
        );
    }
    for interval in [
        Duration::ZERO,
        Duration::from_nanos(999_999),
        Duration::from_secs(86400) + Duration::from_nanos(1),
    ] {
        assert_eq!(RequestRate::new(1, interval), Err(InvalidRequestRate));
    }
    for burst in [1, 4] {
        for interval in [
            Duration::from_millis(1),
            Duration::from_secs(86400),
            Duration::from_nanos(1_000_001),
        ] {
            assert_eq!(rate(burst, interval).interval, interval);
        }
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn bucket_boundaries_fractional_remainder_idle_cap_and_huge_elapsed() {
    let interval = Duration::from_nanos(1_000_001);
    let mut bucket = Bucket::new(rate(4, interval));
    let start = bucket.checkpoint;
    bucket.available = 0;
    bucket.refresh(start + interval - Duration::from_nanos(1));
    assert_eq!(bucket.available, 0);
    assert_eq!(
        bucket.wait(start + interval - Duration::from_nanos(1), 1),
        Duration::from_nanos(1)
    );
    bucket.refresh(start + interval);
    assert_eq!(bucket.available, 1);
    bucket.available -= 1;
    bucket.refresh(start + interval * 2 + Duration::from_nanos(123));
    assert_eq!(bucket.available, 1);
    assert_eq!(
        bucket.wait(start + interval * 2 + Duration::from_nanos(123), 4),
        interval * 3 - Duration::from_nanos(123)
    );
    let far = start + Duration::from_secs(1_000_000_000);
    bucket.refresh(far);
    assert_eq!(bucket.available, 4);
    assert_eq!(bucket.checkpoint, far);
    bucket.refresh(far + Duration::from_nanos(999_999));
    bucket.available = 0;
    let drained = bucket.checkpoint;
    bucket.refresh(drained + interval - Duration::from_nanos(1));
    assert_eq!(bucket.available, 0);
    bucket.refresh(drained + interval);
    assert_eq!(bucket.available, 1);
}

#[tokio::test(flavor = "current_thread")]
async fn fifo_idle_timer_saturation_and_cancellation_charge_only_commitments() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let runtime = Runtime::new(
        endpoint,
        2,
        rate(2, Duration::from_millis(250)),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let origin = tokio::time::Instant::now();
    let mut handles: Vec<_> = (0..10)
        .map(|_| {
            runtime
                .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(
        (runtime.snapshot().commitments, runtime.snapshot().pending),
        (2, 8)
    );
    assert!(matches!(
        runtime.submit("ok", crate::SyntheticCost::new(1, 1).unwrap()),
        Err(SubmitError::Full)
    ));
    assert!(matches!(
        runtime.submit(
            &"x".repeat(MAX_INPUT + 1),
            crate::SyntheticCost::new(1, 1).unwrap()
        ),
        Err(SubmitError::InputTooLarge)
    ));
    for i in [0, 1, 2, 5] {
        handles[i].cancel();
    }
    spin().await;
    for i in [0, 1, 2, 5] {
        assert_eq!(
            handles[i].terminal().await.unwrap().outcome,
            Outcome::Cancelled
        );
    }
    assert_eq!(
        (runtime.snapshot().commitments, runtime.snapshot().active),
        (2, 0)
    );
    assert_eq!(runtime.snapshot().rate_available, 0);
    for (index, i) in [3, 4, 6, 7, 8, 9].into_iter().enumerate() {
        let before = origin + Duration::from_millis(250 * (index as u64 + 1) - 1);
        tokio::time::advance(before - tokio::time::Instant::now()).await;
        spin().await;
        assert_eq!(runtime.snapshot().commitments, 2 + index);
        let (mut reached, release) = tests::probe(&runtime, Checkpoint::BeforeTransport);
        tokio::time::advance(Duration::from_millis(2)).await;
        reached.recv().await.unwrap();
        assert_eq!(runtime.snapshot().commitments, 3 + index);
        assert_eq!(runtime.snapshot().last_committed_id, Some(handles[i].id()));
        handles[i].cancel();
        release.notify_one();
        spin().await;
        assert_eq!(
            handles[i].terminal().await.unwrap().outcome,
            Outcome::Cancelled
        );
    }
    drop(handles);
    spin().await;
    let state = runtime.shutdown().await;
    assert_eq!(
        (
            state.active,
            state.pending,
            state.handles,
            state.unresolved,
            state.attempts
        ),
        (0, 0, 0, 0, 0)
    );
    assert!(
        Runtime::new(
            endpoint,
            4,
            rate(4, Duration::from_millis(1)),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    Runtime::new(
        endpoint,
        4,
        rate(4, Duration::from_millis(1)),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn timing_max_cooldown_burst_cap_and_exposure_never_refills() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = Runtime::new(
        listener.local_addr().unwrap(),
        4,
        rate(2, Duration::from_secs(2)),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    {
        let mut state = runtime.shared.state.lock().unwrap();
        state.bucket.available = 0;
        for delay in [1, 2, 3] {
            state.cooldown = None;
            state.feedback(
                RetryAfter {
                    wait: Some(RetryWait::Delay(Duration::from_secs(delay))),
                    ..RetryAfter::default()
                },
                tokio::time::Instant::now(),
            );
            assert_eq!(state.timing_wait(), Some(Duration::from_secs(delay.max(2))));
        }
        state.feedback(
            RetryAfter {
                wait: Some(RetryWait::Unrepresentable),
                ..RetryAfter::default()
            },
            tokio::time::Instant::now(),
        );
        assert_eq!(state.timing_wait(), None);
        state.cooldown = None;
        for id in 0..4 {
            state.exposure.push(Exposure {
                id: RequestId(id),
                dispatched: true,
                unresolved: true,
            });
        }
        state.refresh();
    }
    tokio::time::advance(Duration::from_secs(86400)).await;
    {
        let mut state = runtime.shared.state.lock().unwrap();
        assert!(!state.can_start(crate::SyntheticCost::new(1, 1).unwrap()));
        assert_eq!(state.bucket.available, 2);
        assert_eq!(state.timing_wait(), None);
        assert_eq!(state.stats.unresolved, 4);
    }
    for id in 0..4 {
        Runtime::reconcile_fixture(RequestId(id)).unwrap();
    }
    assert!(
        runtime
            .shared
            .state
            .lock()
            .unwrap()
            .can_start(crate::SyntheticCost::new(1, 1).unwrap())
    );
    runtime.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_requires_full_refill_even_with_one_token_and_changed_owner() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let runtime = Runtime::new(
        endpoint,
        2,
        rate(2, Duration::from_secs(1)),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handles: Vec<_> = (0..2)
        .map(|_| {
            runtime
                .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    for h in &handles {
        h.cancel();
    }
    spin().await;
    for h in &mut handles {
        h.terminal().await.unwrap();
    }
    drop(handles);
    runtime.shutdown().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(Runtime::process_snapshot().rate_available, 1);
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    assert!(
        Runtime::new(
            other.local_addr().unwrap(),
            4,
            rate(4, Duration::from_millis(1)),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    tokio::time::advance(Duration::from_millis(999)).await;
    assert!(
        Runtime::new(
            endpoint,
            1,
            rate(1, Duration::from_secs(86400)),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    tokio::time::advance(Duration::from_millis(1)).await;
    Runtime::new(
        other.local_addr().unwrap(),
        4,
        rate(4, Duration::from_millis(1)),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_submit_burst_and_drop_shutdown_do_not_wait_for_debt() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let runtime = Arc::new(
        Runtime::new(
            endpoint,
            4,
            rate(2, Duration::from_secs(86400)),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .unwrap(),
    );
    let barrier = Arc::new(std::sync::Barrier::new(11));
    let executor = tokio::runtime::Handle::current();
    let threads: Vec<_> = (0..10)
        .map(|_| {
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            let executor = executor.clone();
            std::thread::spawn(move || {
                let _entered = executor.enter();
                barrier.wait();
                runtime
                    .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
                    .unwrap()
            })
        })
        .collect();
    barrier.wait();
    let mut handles: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        (runtime.snapshot().commitments, runtime.snapshot().pending),
        (2, 8)
    );
    let now = tokio::time::Instant::now();
    drop(runtime);
    spin().await;
    for h in &mut handles {
        assert_eq!(h.terminal().await.unwrap().outcome, Outcome::Cancelled);
    }
    drop(handles);
    spin().await;
    assert_eq!(tokio::time::Instant::now(), now);
    assert_eq!(
        (
            Runtime::process_snapshot().active,
            Runtime::process_snapshot().handles,
            Runtime::process_snapshot().attempts
        ),
        (0, 0, 0)
    );
    assert!(
        Runtime::new(
            endpoint,
            4,
            rate(4, Duration::from_millis(1)),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    tokio::time::advance(Duration::from_secs(172800)).await;
    Runtime::new(
        endpoint,
        4,
        rate(4, Duration::from_millis(1)),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn combined_timing_idle_wake_and_long_cooldown_earn_only_burst() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    for cooldown_ms in [100, 250, 500, 10_000] {
        let runtime = Runtime::new(
            endpoint,
            4,
            rate(2, Duration::from_millis(250)),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut initial: Vec<_> = (0..2)
            .map(|_| {
                runtime
                    .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
                    .unwrap()
            })
            .collect();
        for h in &initial {
            h.cancel();
        }
        spin().await;
        for h in &mut initial {
            h.terminal().await.unwrap();
        }
        drop(initial);
        let origin = tokio::time::Instant::now();
        runtime.shared.feedback(
            RetryAfter {
                wait: Some(RetryWait::Delay(Duration::from_millis(cooldown_ms))),
                ..RetryAfter::default()
            },
            origin,
        );
        let mut pending: Vec<_> = (0..3)
            .map(|_| {
                runtime
                    .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
                    .unwrap()
            })
            .collect();
        spin().await;
        tokio::time::advance(Duration::from_millis(cooldown_ms.max(250) - 1)).await;
        spin().await;
        assert_eq!(runtime.snapshot().commitments, 2);
        tokio::time::advance(Duration::from_millis(2)).await;
        spin().await;
        let expected = if cooldown_ms >= 500 { 4 } else { 3 };
        assert_eq!(runtime.snapshot().commitments, expected);
        assert_eq!(runtime.snapshot().pending, 5 - expected);
        for h in &pending {
            h.cancel();
        }
        spin().await;
        for h in &mut pending {
            assert_eq!(h.terminal().await.unwrap().outcome, Outcome::Cancelled);
        }
        drop(pending);
        let stopped = runtime.shutdown().await;
        assert_eq!(
            (
                stopped.active,
                stopped.pending,
                stopped.unresolved,
                stopped.attempts
            ),
            (0, 0, 0, 0)
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(Runtime::process_snapshot().rate_full_refill, Duration::ZERO);
    }
}
