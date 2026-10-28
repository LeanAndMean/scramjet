use super::*;

fn signal(seconds: u64) -> RetryAfter {
    RetryAfter {
        wait: Some(RetryWait::Delay(Duration::from_secs(seconds))),
        valid: 1,
        ..RetryAfter::default()
    }
}
#[tokio::test(flavor = "current_thread")]
async fn simultaneous_feedback_and_absolute_date_snapshot_use_max_remaining() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = Runtime::new(
        listener.local_addr().unwrap(),
        1,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let threads: Vec<_> = [2, 9]
        .into_iter()
        .map(|seconds| {
            let shared = runtime.shared.clone();
            let barrier = barrier.clone();
            let observation = tokio::time::Instant::now();
            let executor = tokio::runtime::Handle::current();
            std::thread::spawn(move || {
                let _entered = executor.enter();
                barrier.wait();
                // The scalar merge is tested with one captured monotonic checkpoint on both writers.
                let mut state = shared.state.lock().unwrap();
                state.feedback(signal(seconds), observation);
            })
        })
        .collect();
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(runtime.snapshot().cooldown_remaining <= Duration::from_secs(9));
    assert!(runtime.snapshot().cooldown_remaining > Duration::from_secs(8));
    assert_eq!(runtime.snapshot().cooldown_signals, 2);
    {
        let mut state = runtime.shared.state.lock().unwrap();
        state.cooldown = None;
        state.refresh_cooldown();
    }
    let utc = httpdate::parse_http_date("Sun, 06 Nov 1994 08:49:36 GMT").unwrap()
        + Duration::from_millis(250);
    let feedback = crate::http::retry_after::parse(
        [b"Sun, 06 Nov 1994 08:49:37 GMT".as_slice()].into_iter(),
        utc,
    );
    runtime
        .shared
        .feedback(feedback, tokio::time::Instant::now());
    assert_eq!(
        runtime.snapshot().cooldown_remaining,
        Duration::from_millis(750)
    );
    tokio::time::advance(Duration::from_millis(500)).await;
    assert_eq!(
        runtime.snapshot().cooldown_remaining,
        Duration::from_millis(250)
    );
    // A later wall-clock conversion is not consulted by an accepted countdown.
    let _jumped_wall_clock = utc + Duration::from_secs(86400);
    tokio::time::advance(Duration::from_millis(250)).await;
    assert_eq!(runtime.snapshot().cooldown_remaining, Duration::ZERO);
    runtime.shutdown().await;
}

async fn spin() {
    for _ in 0..40 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cooldown_commitments_fifo_idle_expiry_and_cancellation() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = Runtime::new(
        listener.local_addr().unwrap(),
        2,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    // This synchronous sequence orders admission before feedback, without polling the supervisor.
    let mut grandfathered = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    runtime
        .shared
        .feedback(signal(3), tokio::time::Instant::now());
    let mut head = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut middle = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut survivor = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    assert_eq!(
        (runtime.snapshot().commitments, runtime.snapshot().pending),
        (1, 3)
    );
    head.cancel();
    middle.cancel();
    grandfathered.cancel();
    spin().await;
    assert_eq!(head.terminal().await.unwrap().outcome, Outcome::Cancelled);
    assert_eq!(middle.terminal().await.unwrap().outcome, Outcome::Cancelled);
    assert_eq!(
        grandfathered.terminal().await.unwrap().outcome,
        Outcome::Cancelled
    );
    drop(head);
    drop(middle);
    drop(grandfathered);
    assert_eq!(
        (runtime.snapshot().active, runtime.snapshot().pending),
        (0, 1)
    );
    tokio::time::advance(Duration::from_millis(2999)).await;
    spin().await;
    assert_eq!(runtime.snapshot().commitments, 1);
    tokio::time::advance(Duration::from_millis(2)).await;
    spin().await;
    assert_eq!(runtime.snapshot().commitments, 2);
    survivor.cancel();
    spin().await;
    let terminal = survivor.terminal().await.unwrap();
    assert_eq!(terminal.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().attempts, 0);
    drop(survivor);
    runtime.shutdown().await;
    tokio::time::advance(Duration::from_millis(4)).await;
    assert_eq!(Runtime::process_snapshot().rate_full_refill, Duration::ZERO);
}

#[tokio::test(flavor = "current_thread")]
async fn long_wait_day_chunks_max_merge_and_retained_replacement() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let runtime = Runtime::new(
        endpoint,
        1,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    runtime
        .shared
        .feedback(signal(3 * 86400 + 7), tokio::time::Instant::now());
    let mut queued = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    spin().await;
    for day in 1..=3 {
        tokio::time::advance(Duration::from_secs(86400)).await;
        spin().await;
        assert_eq!(runtime.snapshot().commitments, 0);
        assert_eq!(
            runtime.snapshot().cooldown_remaining,
            Duration::from_secs((3 - day) * 86400 + 7)
        );
    }
    runtime
        .shared
        .feedback(signal(2), tokio::time::Instant::now());
    assert_eq!(
        runtime.snapshot().cooldown_remaining,
        Duration::from_secs(7)
    );
    runtime
        .shared
        .feedback(signal(9), tokio::time::Instant::now());
    assert_eq!(
        runtime.snapshot().cooldown_remaining,
        Duration::from_secs(9)
    );
    queued.cancel();
    spin().await;
    queued.terminal().await.unwrap();
    drop(queued);
    let state = runtime.shutdown().await;
    assert_eq!(state.cooldown_remaining, Duration::from_secs(9));
    assert!(
        Runtime::new(
            endpoint,
            4,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    tokio::time::advance(Duration::from_secs(8)).await;
    assert!(
        Runtime::new(
            endpoint,
            2,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
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
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn countdown_preserves_u64_max_and_indefinite_never_clears() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let runtime = Runtime::new(
        listener.local_addr().unwrap(),
        1,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    {
        let mut state = runtime.shared.state.lock().unwrap();
        state.feedback(signal(u64::MAX), tokio::time::Instant::now());
        assert_eq!(state.stats.cooldown_remaining.as_secs(), u64::MAX);
    }
    let mut queued = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    spin().await;
    tokio::time::advance(Duration::from_secs(86400)).await;
    spin().await;
    assert_eq!(runtime.snapshot().commitments, 0);
    queued.cancel();
    spin().await;
    assert_eq!(queued.terminal().await.unwrap().outcome, Outcome::Cancelled);
    drop(queued);
    {
        let mut state = runtime.shared.state.lock().unwrap();
        state.refresh_cooldown();
        assert_eq!(state.stats.cooldown_remaining.as_secs(), u64::MAX - 86400);
        state.feedback(
            RetryAfter {
                wait: Some(RetryWait::Unrepresentable),
                valid: 1,
                ..RetryAfter::default()
            },
            tokio::time::Instant::now(),
        );
        state.feedback(signal(0), tokio::time::Instant::now());
        assert!(!state.can_start(crate::SyntheticCost::new(1, 1).unwrap()));
        assert!(state.stats.cooldown_indefinite);
        // Restore only this private test's state before releasing the serialized process owner.
        state.cooldown = None;
        state.refresh_cooldown();
    }
    runtime.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_before_refusal_observation_creates_no_feedback() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let fixture = support::Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        2,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let (mut reached, _release) = tests::probe(&runtime, Checkpoint::ResponseHeaders);
    let mut handle = runtime
        .submit("cooldown-one", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runtime.snapshot().cooldown_signals, 0);
    handle.cancel();
    let terminal = handle.terminal().await.unwrap();
    assert_eq!(terminal.outcome, Outcome::Cancelled);
    assert!(terminal.retry_after.is_none());
    assert_eq!(runtime.snapshot().cooldown_signals, 0);
    assert_eq!(runtime.snapshot().unresolved, 1);
    fixture.wait_settled(handle.id()).await;
    Runtime::reconcile_fixture(handle.id()).unwrap();
    drop(handle);
    runtime.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn grandfathered_commitment_sends_after_feedback_but_pending_promotion_waits() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let fixture = support::Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        1,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let (mut reached, release) = tests::probe(&runtime, Checkpoint::ConnectPending);
    let mut first = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (runtime.snapshot().commitments, runtime.snapshot().attempts),
        (1, 0)
    );
    let mut pending = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    runtime
        .shared
        .feedback(signal(1), tokio::time::Instant::now());
    release.notify_one();
    assert_eq!(first.terminal().await.unwrap().outcome, Outcome::Success);
    assert_eq!(
        (
            runtime.snapshot().commitments,
            runtime.snapshot().attempts,
            runtime.snapshot().pending
        ),
        (1, 1, 1)
    );
    assert_eq!(fixture.started(), vec![first.id()]);
    pending.cancel();
    pending.terminal().await.unwrap();
    drop(first);
    drop(pending);
    runtime.shutdown().await;
    fixture.shutdown().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        Runtime::process_snapshot().cooldown_remaining,
        Duration::ZERO
    );
}

#[tokio::test(flavor = "current_thread")]
async fn indefinite_pending_cancel_shutdown_and_replacement_are_fail_closed() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    tokio::time::pause();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let runtime = Runtime::new(
        endpoint,
        2,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    runtime.shared.feedback(
        RetryAfter {
            wait: Some(RetryWait::Unrepresentable),
            valid: 1,
            ..RetryAfter::default()
        },
        tokio::time::Instant::now(),
    );
    let mut handles: Vec<_> = (0..8)
        .map(|_| {
            runtime
                .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    spin().await;
    tokio::time::advance(Duration::from_secs(86400 * 100)).await;
    spin().await;
    assert_eq!(
        (runtime.snapshot().commitments, runtime.snapshot().pending),
        (0, 8)
    );
    handles[0].cancel();
    spin().await;
    assert_eq!(
        handles[0].terminal().await.unwrap().outcome,
        Outcome::Cancelled
    );
    let shared = runtime.shared.clone();
    let state = runtime.shutdown().await;
    assert!(state.cooldown_indefinite);
    for handle in &mut handles[1..] {
        assert_eq!(handle.terminal().await.unwrap().outcome, Outcome::Cancelled);
    }
    drop(handles);
    drop(shared);
    assert!(
        Runtime::new(
            endpoint,
            4,
            crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
                .unwrap(),
            crate::ConcurrencyMode::Fixed,
        )
        .is_err()
    );
    {
        let owner = PROCESS_OWNER.lock().unwrap();
        let shared = owner.as_ref().unwrap();
        let mut state = shared.state.lock().unwrap();
        state.feedback(signal(1), tokio::time::Instant::now());
        assert!(state.stats.cooldown_indefinite);
        assert_eq!(
            (
                state.stats.attempts,
                state.stats.handles,
                state.stats.unresolved
            ),
            (0, 0, 0)
        );
        state.cooldown = None;
        state.refresh_cooldown();
    }
    Runtime::new(
        endpoint,
        4,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn observed_feedback_precedes_teardown_and_later_submit() {
    let _owner = tests::TEST_OWNER.lock().await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let fixture = support::Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        2,
        crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        crate::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        crate::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let (mut reached, release) = tests::probe(&runtime, Checkpoint::Settled);
    let mut refused = runtime
        .submit("cooldown-one", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), reached.recv())
        .await
        .unwrap()
        .unwrap();
    let state = runtime.snapshot();
    assert_eq!(
        (state.active, state.cooldown_signals, state.commitments),
        (1, 1, 1)
    );
    assert!(state.cooldown_remaining > Duration::ZERO);
    let mut later = runtime
        .submit("ok", crate::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    assert_eq!(
        (runtime.snapshot().pending, runtime.snapshot().commitments),
        (1, 1)
    );
    refused.cancel();
    later.cancel();
    release.notify_one();
    let terminal = refused.terminal().await.unwrap();
    assert_eq!(terminal.outcome, Outcome::HttpRejected(429));
    assert_eq!(terminal.retry_after.unwrap().wait, signal(1).wait);
    assert!(refused.terminal().await.is_none());
    assert_eq!(later.terminal().await.unwrap().outcome, Outcome::Cancelled);
    drop(refused);
    drop(later);
    runtime.shutdown().await;
    fixture.shutdown().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        Runtime::process_snapshot().cooldown_remaining,
        Duration::ZERO
    );
}
