mod support;
use scramjet_rs::{Outcome, RequestHandle, RequestId, Runtime, SubmitError};
use std::time::Duration;
use support::Fixture;

static TEST_OWNER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn result(handle: &mut RequestHandle) -> scramjet_rs::Terminal {
    tokio::time::timeout(Duration::from_secs(8), handle.terminal())
        .await
        .unwrap()
        .unwrap()
}
async fn drain(handle: &mut RequestHandle) -> usize {
    let mut bytes = 0;
    while let Some(delta) = handle.next_delta().await {
        bytes += delta.len();
    }
    bytes
}
async fn wait(runtime: &Runtime, predicate: impl Fn(scramjet_rs::Snapshot) -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !predicate(runtime.snapshot()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn actual_http_success_and_typed_failures() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    for (scenario, outcome) in [
        ("fragmented", Outcome::Success),
        ("ok", Outcome::Success),
        ("reject", Outcome::HttpRejected(429)),
        ("invalid", Outcome::Framing),
        ("field", Outcome::Framing),
        ("incomplete", Outcome::Incomplete),
        ("truncated", Outcome::Incomplete),
        ("chunked-incomplete", Outcome::Incomplete),
        ("after-marker", Outcome::Framing),
        ("oversized-record", Outcome::ResourceLimit),
        ("oversized-head", Outcome::ResourceLimit),
        ("many-headers", Outcome::ResourceLimit),
        ("trailer", Outcome::Framing),
        ("oversized-trailer", Outcome::ResourceLimit),
        ("many-trailers", Outcome::ResourceLimit),
    ] {
        let fixture = Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut handle = runtime
            .submit(scenario, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        let mut deltas = Vec::new();
        while let Some(delta) = handle.next_delta().await {
            deltas.push(delta);
        }
        let terminal = result(&mut handle).await;
        assert_eq!(terminal.outcome, outcome, "{scenario}");
        assert!(handle.terminal().await.is_none());
        if outcome == Outcome::Success {
            assert_eq!(deltas, ["hé🙂", "two"]);
        }
        fixture.wait_settled(handle.id()).await;
        let expected_unresolved = usize::from(!matches!(
            outcome,
            Outcome::Success | Outcome::HttpRejected(_)
        ));
        assert_eq!(
            runtime.snapshot().unresolved,
            expected_unresolved,
            "{scenario}"
        );
        if expected_unresolved != 0 {
            Runtime::reconcile_fixture(handle.id()).unwrap();
        }
        drop(handle);
        let final_state = runtime.shutdown().await;
        assert_eq!(
            (
                final_state.active,
                final_state.pending,
                final_state.handles,
                final_state.input_bytes,
                final_state.delta_bytes
            ),
            (0, 0, 0, 0, 0)
        );
        fixture.shutdown().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn fixed_admission_fifo_queue_cancel_and_retained_leases() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    assert!(matches!(
        runtime.submit(
            &"é".repeat(2049),
            scramjet_rs::SyntheticCost::new(1, 1).unwrap()
        ),
        Err(SubmitError::InputTooLarge)
    ));
    let payload = format!("hold{}", "x".repeat(4092));
    let mut handles: Vec<_> = (0..12)
        .map(|_| {
            runtime
                .submit(&payload, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(runtime.snapshot().input_bytes, 49_152);
    let ids: Vec<_> = handles.iter().map(RequestHandle::id).collect();
    assert!(matches!(
        runtime.submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap()),
        Err(SubmitError::Full)
    ));
    fixture.wait_started(4).await;
    assert_eq!(runtime.snapshot().pending, 8);
    assert_eq!(fixture.peak(), 4);
    handles[4].cancel();
    assert_eq!(result(&mut handles[4]).await.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().attempts, 4);
    fixture.release(12);
    for (index, handle) in handles.iter_mut().enumerate() {
        if index == 4 {
            continue;
        }
        drain(handle).await;
        assert_eq!(result(handle).await.outcome, Outcome::Success);
    }
    let mut started = fixture.started();
    let mut expected: Vec<_> = ids
        .into_iter()
        .enumerate()
        .filter_map(|(index, id)| (index != 4).then_some(id))
        .collect();
    started.sort_by_key(|id| id.0);
    expected.sort_by_key(|id| id.0);
    assert_eq!(started, expected);
    assert!(fixture.fd_peak() > 0);
    assert_eq!(runtime.snapshot().handles, 12);
    assert!(matches!(
        runtime.submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap()),
        Err(SubmitError::Full)
    ));
    assert_eq!(runtime.snapshot().input_bytes, 0);
    drop(handles);
    let state = runtime.shutdown().await;
    assert_eq!(state.active_peak, 4);
    assert_eq!(state.pending_peak, 8);
    assert_eq!(state.handles_peak, 12);
    assert_eq!(state.handles, 0);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn pending_bound_is_independent_of_handle_bound() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        1,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handles: Vec<_> = (0..9)
        .map(|_| {
            runtime
                .submit("hold", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    assert!(matches!(
        runtime.submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap()),
        Err(SubmitError::Full)
    ));
    fixture.wait_started(1).await;
    for handle in &handles {
        handle.cancel();
    }
    for handle in &mut handles {
        assert_eq!(result(handle).await.outcome, Outcome::Cancelled);
    }
    assert_eq!(fixture.started().len(), 1);
    fixture.wait_settled(handles[0].id()).await;
    Runtime::reconcile_fixture(handles[0].id()).unwrap();
    drop(handles);
    assert_eq!(runtime.shutdown().await.active, 0);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_does_not_depend_on_draining_full_output_and_drop_cancels() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut cancelled = runtime
        .submit("slow", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    wait(&runtime, |state| state.delta_items == 8).await;
    assert_eq!(runtime.snapshot().delta_bytes, 8192);
    cancelled.cancel();
    assert_eq!(result(&mut cancelled).await.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().active, 0);
    assert_eq!(runtime.snapshot().delta_bytes, 8192);
    assert_eq!(drain(&mut cancelled).await, 8192);
    fixture.wait_settled(cancelled.id()).await;
    Runtime::reconcile_fixture(cancelled.id()).unwrap();
    drop(cancelled);
    let lost = runtime
        .submit("slow", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let id = lost.id();
    wait(&runtime, |state| state.delta_items == 8).await;
    drop(lost);
    wait(&runtime, |state| state.active == 0).await;
    fixture.wait_settled(id).await;
    assert_eq!(runtime.snapshot().unresolved, 1);
    assert_eq!(
        (runtime.snapshot().handles, runtime.snapshot().delta_bytes),
        (0, 0)
    );
    Runtime::reconcile_fixture(id).unwrap();
    let final_state = runtime.shutdown().await;
    assert_eq!(final_state.unresolved, 0);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn four_uncertain_attempts_pause_dispatch_until_correlated_reconciliation() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut failures: Vec<_> = (0..4)
        .map(|_| {
            runtime
                .submit("header", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    let mut queued = runtime
        .submit("hold", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    let mut cancelled = runtime
        .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    fixture.wait_started(4).await;
    for handle in &failures {
        handle.cancel();
    }
    for handle in &mut failures {
        assert_eq!(result(handle).await.outcome, Outcome::Cancelled);
        fixture.wait_settled(handle.id()).await;
    }
    assert_eq!(
        (
            runtime.snapshot().active,
            runtime.snapshot().unresolved,
            runtime.snapshot().pending
        ),
        (0, 4, 2)
    );
    assert_eq!(fixture.started().len(), 4);
    cancelled.cancel();
    assert_eq!(result(&mut cancelled).await.outcome, Outcome::Cancelled);
    let ids: Vec<_> = failures.iter().map(RequestHandle::id).collect();
    let id = ids[0];
    drop(failures);
    assert_eq!(runtime.snapshot().unresolved, 4);
    assert!(Runtime::reconcile_fixture(queued.id()).is_err());
    assert!(Runtime::reconcile_fixture(scramjet_rs::RequestId(9999)).is_err());
    Runtime::reconcile_fixture(id).unwrap();
    assert!(Runtime::reconcile_fixture(id).is_err());
    fixture.wait_started(5).await;
    fixture.release(1);
    drain(&mut queued).await;
    assert_eq!(result(&mut queued).await.outcome, Outcome::Success);
    assert_eq!(runtime.snapshot().attempts, 5);
    drop(queued);
    drop(cancelled);
    assert_eq!(runtime.shutdown().await.unresolved, 3);
    for id in &ids[1..] {
        Runtime::reconcile_fixture(*id).unwrap();
    }
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn body_wire_and_large_frame_limits_reclaim_resources() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    for (scenario, outcome) in [
        ("huge-chunk", Outcome::Success),
        ("oversized-body", Outcome::ResourceLimit),
        ("wire", Outcome::ResourceLimit),
    ] {
        let fixture = Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut handle = runtime
            .submit(scenario, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        drain(&mut handle).await;
        let terminal = result(&mut handle).await;
        assert_eq!(terminal.outcome, outcome, "{scenario}");
        if scenario == "wire" {
            assert_eq!(terminal.wire_bytes, 131_072);
            assert!(terminal.body_bytes < 65_536);
        }
        assert!(runtime.snapshot().delta_items_peak <= 8);
        fixture.wait_settled(handle.id()).await;
        if outcome != Outcome::Success {
            Runtime::reconcile_fixture(handle.id()).unwrap();
        }
        drop(handle);
        assert_eq!(runtime.shutdown().await.delta_bytes, 0);
        fixture.shutdown().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn deadline_on_stalled_consumer_and_shutdown_tree() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut deadline = runtime
        .submit("deadline", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    wait(&runtime, |state| state.delta_items == 8).await;
    assert_eq!(result(&mut deadline).await.outcome, Outcome::Deadline);
    assert_eq!(runtime.snapshot().unresolved, 1);
    let deadline_id = deadline.id();
    drop(deadline);
    let mut handles: Vec<_> = (0..8)
        .map(|_| {
            runtime
                .submit("header", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    fixture.wait_started(4).await;
    let state = runtime.shutdown().await;
    assert_eq!((state.active, state.pending, state.unresolved), (0, 0, 4));
    for handle in &mut handles {
        assert_eq!(result(handle).await.outcome, Outcome::Cancelled);
    }
    fixture.wait_settled(deadline_id).await;
    Runtime::reconcile_fixture(deadline_id).unwrap();
    for handle in &handles[..3] {
        fixture.wait_settled(handle.id()).await;
        Runtime::reconcile_fixture(handle.id()).unwrap();
    }
    drop(handles);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn before_dispatch_and_body_cancellation_have_different_exposure() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut before = runtime
        .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    before.cancel();
    assert_eq!(result(&mut before).await.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().attempts, 0);
    assert_eq!(runtime.snapshot().unresolved, 0);
    assert!(fixture.started().is_empty());
    let mut body = runtime
        .submit("body", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    assert_eq!(body.next_delta().await.unwrap(), "waiting");
    body.cancel();
    assert_eq!(result(&mut body).await.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().unresolved, 1);
    fixture.wait_settled(body.id()).await;
    Runtime::reconcile_fixture(body.id()).unwrap();
    drop(before);
    drop(body);
    runtime.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn completed_success_wins_late_cancel_and_results_are_independent() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handles: Vec<_> = (0..4)
        .map(|_| {
            runtime
                .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    wait(&runtime, |state| state.active == 0).await;
    for handle in &mut handles {
        handle.cancel();
        let terminal = result(handle).await;
        assert_eq!(terminal.id, handle.id());
        assert_eq!(terminal.outcome, Outcome::Success);
        assert_eq!(drain(handle).await, "hé🙂two".len());
    }
    assert_eq!(runtime.snapshot().unresolved, 0);
    drop(handles);
    runtime.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn connection_failure_is_known_not_dispatched() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let runtime = Runtime::new(
        address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handle = runtime
        .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    assert_eq!(result(&mut handle).await.outcome, Outcome::Transport);
    assert_eq!(
        (runtime.snapshot().attempts, runtime.snapshot().unresolved),
        (0, 0)
    );
    drop(handle);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn fifo_progress_and_maximum_input_backing_are_bounded() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        1,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let payload = format!("hold{}", "x".repeat(4092));
    let mut handles: Vec<_> = (0..9)
        .map(|_| {
            runtime
                .submit(&payload, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    let expected: Vec<_> = handles.iter().map(RequestHandle::id).collect();
    assert_eq!(runtime.snapshot().input_bytes, 9 * 4096);
    for (index, handle) in handles.iter_mut().enumerate() {
        fixture.wait_started(index + 1).await;
        assert_eq!(fixture.started()[index], expected[index]);
        fixture.release(1);
        drain(handle).await;
        assert_eq!(result(handle).await.outcome, Outcome::Success);
    }
    drop(handles);
    assert_eq!(runtime.shutdown().await.input_bytes, 0);
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn marker_without_http_completion_does_not_settle_exposure() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handle = runtime
        .submit(
            "marker-stall",
            scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
        )
        .unwrap();
    fixture.wait_started(1).await;
    handle.cancel();
    assert_eq!(result(&mut handle).await.outcome, Outcome::Cancelled);
    assert_eq!(runtime.snapshot().unresolved, 1);
    fixture.wait_settled(handle.id()).await;
    Runtime::reconcile_fixture(handle.id()).unwrap();
    drop(handle);
    runtime.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn simultaneous_protocol_failure_exhausts_reserved_credits() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut failures: Vec<_> = (0..4)
        .map(|_| {
            runtime
                .submit("hold-fail", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    let mut queued = runtime
        .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    fixture.wait_started(4).await;
    fixture.release(4);
    for handle in &mut failures {
        drain(handle).await;
        assert_eq!(result(handle).await.outcome, Outcome::Incomplete);
    }
    assert_eq!(
        (runtime.snapshot().active, runtime.snapshot().unresolved),
        (0, 4)
    );
    assert_eq!(fixture.started().len(), 4);
    queued.cancel();
    assert_eq!(result(&mut queued).await.outcome, Outcome::Cancelled);
    let ids: Vec<_> = failures.iter().map(RequestHandle::id).collect();
    drop(failures);
    drop(queued);
    assert_eq!(runtime.shutdown().await.unresolved, 4);
    for id in ids {
        fixture.wait_settled(id).await;
        Runtime::reconcile_fixture(id).unwrap();
    }
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn reconciliation_rejects_stale_identity_after_clean_replacement() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let first = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut one = first
        .submit("incomplete", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    drain(&mut one).await;
    result(&mut one).await;
    let old_id = one.id();
    fixture.wait_settled(old_id).await;
    Runtime::reconcile_fixture(old_id).unwrap();
    drop(one);
    let stopped = first.shutdown().await;
    tokio::time::sleep(stopped.rate_full_refill).await;
    let second = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut two = second
        .submit("incomplete", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    drain(&mut two).await;
    result(&mut two).await;
    assert_ne!(old_id, two.id());
    assert!(Runtime::reconcile_fixture(old_id).is_err());
    fixture.wait_settled(two.id()).await;
    Runtime::reconcile_fixture(two.id()).unwrap();
    assert!(Runtime::reconcile_fixture(two.id()).is_err());
    drop(two);
    second.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn independent_construction_and_shutdown_cannot_reset_process_budgets() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let other_fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let payload = format!("hold{}", "x".repeat(4092));
    let mut handles: Vec<_> = (0..12)
        .map(|_| {
            runtime
                .submit(&payload, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    let ids: Vec<_> = handles.iter().map(RequestHandle::id).collect();
    fixture.wait_started(4).await;
    let state = Runtime::process_snapshot();
    assert_eq!(
        (
            state.active,
            state.pending,
            state.handles,
            state.input_bytes,
            state.potential
        ),
        (4, 8, 12, 49_152, 4)
    );
    assert_eq!(fixture.peak(), 4);
    for endpoint in [fixture.address, other_fixture.address] {
        assert!(matches!(
            Runtime::new(
                endpoint,
                4,
                scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                scramjet_rs::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1)
                )
                .unwrap(),
                scramjet_rs::ConcurrencyMode::Fixed,
            ),
            Err(scramjet_rs::RuntimeBusy)
        ));
        std::thread::spawn(move || {
            assert!(matches!(
                Runtime::new(
                    endpoint,
                    1,
                    scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                    scramjet_rs::SyntheticTokenBudget::new(
                        65_536,
                        65_536,
                        std::time::Duration::from_millis(1)
                    )
                    .unwrap(),
                    scramjet_rs::ConcurrencyMode::Fixed,
                ),
                Err(scramjet_rs::RuntimeBusy)
            ));
        })
        .join()
        .unwrap();
    }
    assert!(other_fixture.started().is_empty());
    let stopped = runtime.shutdown().await;
    assert_eq!(
        (
            stopped.active,
            stopped.pending,
            stopped.handles,
            stopped.unresolved
        ),
        (0, 0, 12, 4)
    );
    for handle in &mut handles {
        assert_eq!(result(handle).await.outcome, Outcome::Cancelled);
        assert!(handle.terminal().await.is_none());
    }
    assert!(
        Runtime::new(
            other_fixture.address,
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
    for id in &ids[..4] {
        fixture.wait_settled(*id).await;
        Runtime::reconcile_fixture(*id).unwrap();
    }
    assert_eq!(Runtime::process_snapshot().unresolved, 0);
    assert!(
        Runtime::new(
            other_fixture.address,
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
    drop(handles);
    tokio::time::sleep(Runtime::process_snapshot().rate_full_refill).await;
    let next = Runtime::new(
        other_fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    assert_eq!(Runtime::process_snapshot().handles, 0);
    assert!(Runtime::reconcile_fixture(ids[0]).is_err());
    next.shutdown().await;
    fixture.shutdown().await;
    other_fixture.shutdown().await;
}

#[test]
fn concurrent_first_construction_elects_only_one_process_owner() {
    let _owner = TEST_OWNER.blocking_lock();
    std::thread::sleep(Duration::from_millis(5));
    let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
    let attempts: Vec<_> = (0..8)
        .map(|_| {
            let gate = gate.clone();
            std::thread::spawn(move || {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                executor.block_on(async {
                    gate.wait();
                    let owner = Runtime::new(
                        "127.0.0.1:1".parse().unwrap(),
                        4,
                        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1))
                            .unwrap(),
                        scramjet_rs::SyntheticTokenBudget::new(
                            65_536,
                            65_536,
                            std::time::Duration::from_millis(1),
                        )
                        .unwrap(),
                        scramjet_rs::ConcurrencyMode::Fixed,
                    );
                    gate.wait();
                    match owner {
                        Ok(runtime) => {
                            runtime.shutdown().await;
                            true
                        }
                        Err(scramjet_rs::RuntimeBusy) => false,
                    }
                })
            })
        })
        .collect();
    assert_eq!(
        attempts
            .into_iter()
            .map(|task| usize::from(task.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert_eq!(Runtime::process_snapshot().handles, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn completed_output_leases_survive_shutdown_until_actually_freed() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handles: Vec<_> = (0..12)
        .map(|_| {
            runtime
                .submit(
                    "full-output",
                    scramjet_rs::SyntheticCost::new(1, 1).unwrap(),
                )
                .unwrap()
        })
        .collect();
    wait(&runtime, |state| state.settled == 12).await;
    let state = runtime.shutdown().await;
    assert_eq!(
        (
            state.active,
            state.handles,
            state.delta_items,
            state.delta_bytes,
            state.unresolved
        ),
        (0, 12, 96, 98_304, 0)
    );
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
    for handle in &mut handles {
        assert_eq!(result(handle).await.outcome, Outcome::Success);
        assert_eq!(drain(handle).await, 8192);
    }
    assert_eq!(Runtime::process_snapshot().delta_bytes, 0);
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
    drop(handles);
    assert_eq!(Runtime::process_snapshot().handles, 0);
    tokio::time::sleep(Runtime::process_snapshot().rate_full_refill).await;
    Runtime::new(
        fixture.address,
        4,
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

#[tokio::test(flavor = "current_thread")]
async fn runtime_and_handle_drop_preserve_recoverable_exposure() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            runtime
                .submit("header", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
                .unwrap()
        })
        .collect();
    let ids: Vec<_> = handles.iter().map(RequestHandle::id).collect();
    fixture.wait_started(4).await;
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
    drop(handles);
    tokio::time::timeout(Duration::from_secs(2), async {
        while Runtime::process_snapshot().active != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let state = Runtime::process_snapshot();
    assert_eq!(
        (
            state.active,
            state.pending,
            state.handles,
            state.input_bytes,
            state.delta_bytes,
            state.unresolved
        ),
        (0, 0, 0, 0, 0, 4)
    );
    for (index, id) in ids.iter().enumerate() {
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
        fixture.wait_settled(*id).await;
        Runtime::reconcile_fixture(*id).unwrap();
        assert!(Runtime::reconcile_fixture(*id).is_err());
        assert_eq!(Runtime::process_snapshot().unresolved, 3 - index);
    }
    assert_eq!(fixture.started().len(), 4);
    tokio::time::sleep(Runtime::process_snapshot().rate_full_refill).await;
    Runtime::new(
        fixture.address,
        4,
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

#[tokio::test(flavor = "current_thread")]
async fn drop_before_dispatch_requires_local_settlement_but_no_reconciliation() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let fixture = Fixture::new().await;
    let runtime = Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut handle = runtime
        .submit("ok", scramjet_rs::SyntheticCost::new(1, 1).unwrap())
        .unwrap();
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
    assert_eq!(result(&mut handle).await.outcome, Outcome::Cancelled);
    assert_eq!(Runtime::process_snapshot().unresolved, 0);
    assert_eq!(Runtime::process_snapshot().attempts, 0);
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
    drop(handle);
    tokio::time::sleep(Runtime::process_snapshot().rate_full_refill).await;
    Runtime::new(
        fixture.address,
        4,
        scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65_536, 65_536, std::time::Duration::from_millis(1))
            .unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap()
    .shutdown()
    .await;
    assert!(fixture.started().is_empty());
    fixture.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn response_head_limits_are_typed_independently_of_arrival_boundaries() {
    let _owner = TEST_OWNER.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    for (scenario, outcome) in [
        ("head-header-start", Outcome::ResourceLimit),
        ("head-prefix", Outcome::ResourceLimit),
        ("head-complete", Outcome::ResourceLimit),
        ("head-fragmented", Outcome::ResourceLimit),
        ("head-size-prefix", Outcome::ResourceLimit),
        ("head-valid-lf", Outcome::Success),
        ("head-valid-count", Outcome::Success),
        ("head-valid-size", Outcome::Success),
        ("head-valid-mixed", Outcome::Success),
        ("head-valid-mixed-fragmented", Outcome::Success),
        ("head-malformed-ending", Outcome::Framing),
        ("head-malformed-ending-fragmented", Outcome::Framing),
        ("head-invalid-start-nul", Outcome::Framing),
        ("head-invalid-start-nul-fragmented", Outcome::Framing),
        ("head-invalid-start-colon", Outcome::Framing),
        ("head-invalid-start-colon-fragmented", Outcome::Framing),
        ("head-invalid-start-space", Outcome::Framing),
        ("head-invalid-start-space-fragmented", Outcome::Framing),
        ("head-invalid", Outcome::Framing),
    ] {
        let fixture = Fixture::new().await;
        let runtime = Runtime::new(
            fixture.address,
            4,
            scramjet_rs::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
            scramjet_rs::SyntheticTokenBudget::new(
                65_536,
                65_536,
                std::time::Duration::from_millis(1),
            )
            .unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut handle = runtime
            .submit(scenario, scramjet_rs::SyntheticCost::new(1, 1).unwrap())
            .unwrap();
        assert_eq!(drain(&mut handle).await, 0);
        let terminal = result(&mut handle).await;
        assert_eq!(terminal.outcome, outcome, "{scenario}");
        assert!(handle.terminal().await.is_none());
        match scenario {
            "head-prefix" => assert_eq!(terminal.wire_bytes, 119),
            "head-complete" => assert_eq!(terminal.wire_bytes, 121),
            "head-size-prefix" => assert_eq!(terminal.wire_bytes, 4096),
            _ => {}
        }
        assert_eq!(runtime.snapshot().active, 0);
        fixture.wait_settled(handle.id()).await;
        assert_eq!(
            runtime.snapshot().unresolved,
            usize::from(outcome != Outcome::Success)
        );
        if outcome != Outcome::Success {
            Runtime::reconcile_fixture(handle.id()).unwrap();
        }
        drop(handle);
        let state = runtime.shutdown().await;
        assert_eq!(
            (
                state.active,
                state.pending,
                state.handles,
                state.input_bytes,
                state.delta_bytes,
                state.unresolved
            ),
            (0, 0, 0, 0, 0, 0)
        );
        fixture.shutdown().await;
    }
}
