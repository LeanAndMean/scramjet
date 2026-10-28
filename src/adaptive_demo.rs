use crate::adaptive_fixture::Fixture;
use scramjet_rs::{
    ConcurrencyMode, Outcome, RequestId, RequestRate, Runtime, SyntheticCost, SyntheticTokenBudget,
    SyntheticUsage,
};
use std::time::Duration;
use tokio::task::JoinSet;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(60), async {
        compare(ConcurrencyMode::Fixed).await;
        compare(ConcurrencyMode::Adaptive).await;
    })
    .await?;
    Ok(())
}
async fn compare(mode: ConcurrencyMode) {
    tokio::time::timeout(Duration::from_secs(30), journey(mode))
        .await
        .expect("comparison guard");
}
async fn journey(mode: ConcurrencyMode) {
    let start = tokio::time::Instant::now();
    let f = Fixture::new().await;
    let r = Runtime::new(
        f.address,
        4,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        mode,
    )
    .unwrap();
    let mut commitments = Vec::with_capacity(48);
    let mut heads = Vec::with_capacity(48);
    let mut completions = Vec::with_capacity(48);
    let mut queue_waits = Vec::with_capacity(48);
    let mut outcomes = [0usize; 4];
    let mut transitions = Vec::with_capacity(48);
    let mut last_change = None;
    let mut phase_targets = Vec::with_capacity(6);
    for batch in 0..6 {
        f.phase(batch);
        let mut tasks = JoinSet::new();
        for logical in batch * 8..batch * 8 + 8 {
            let submitted = tokio::time::Instant::now();
            let mut h = r
                .submit(&logical.to_string(), SyntheticCost::new(1, 1).unwrap())
                .unwrap();
            tasks.spawn(async move {
                let mut text = String::new();
                while let Some(delta) = h.next_delta().await {
                    text.push_str(&delta);
                }
                let t = h.terminal().await.unwrap();
                assert_eq!(t.id, h.id());
                assert!(h.terminal().await.is_none());
                assert_eq!(t.synthetic_usage, SyntheticUsage::Known(1));
                assert_eq!(
                    text,
                    if t.outcome == Outcome::Success {
                        "ok"
                    } else {
                        ""
                    }
                );
                (logical, h.id(), h.commitment().unwrap(), submitted, t)
            });
        }
        let mut ticker = tokio::time::interval(Duration::from_millis(1));
        while !tasks.is_empty() {
            tokio::select! {
                result = tasks.join_next() => {
                    let (logical,id,(order,time),submitted,t) = result.unwrap().unwrap();
                    queue_waits.push(time.saturating_duration_since(submitted).as_micros());
                    commitments.push((logical,id,order,time.duration_since(start).as_millis()));
                    heads.push(t.dispatch_to_head.expect("missing head observation").as_micros());
                    completions.push(t.commitment_to_terminal.expect("missing completion observation").as_micros());
                    outcomes[match t.outcome { Outcome::Success => 0, Outcome::HttpRejected(429) => 1,
                        Outcome::HttpRejected(502 | 504) => 2, _ => 3 }] += 1;
                }
                _ = ticker.tick() => {},
            }
            let s = r.snapshot();
            assert!((1..=4).contains(&s.concurrency.target));
            assert!(
                s.pending <= 8
                    && s.handles <= 12
                    && s.active <= 4
                    && s.usage_obligations <= 4
                    && s.potential + s.unresolved <= 4
            );
            if s.concurrency.last_change != last_change {
                last_change = s.concurrency.last_change;
                assert!(transitions.len() < 48);
                transitions.push((batch, s.concurrency.target, last_change));
            }
        }
        f.wait_batch((batch + 1) * 8).await;
        phase_targets.push(r.snapshot().concurrency.target);
    }
    commitments.sort_by_key(|v| v.2);
    assert_eq!(
        commitments.iter().map(|v| v.0).collect::<Vec<_>>(),
        (0..48).collect::<Vec<_>>()
    );
    assert_eq!(
        commitments.iter().map(|v| v.2).collect::<Vec<_>>(),
        (1..=48).collect::<Vec<_>>()
    );
    let metrics = f.metrics();
    assert_eq!(metrics.receipts.len(), 48);
    for receipt in &metrics.receipts {
        let (_, id, _, _) = commitments.iter().find(|v| v.0 == receipt.logical).unwrap();
        assert_eq!(*id, receipt.id);
        assert!(receipt.settled);
        assert!(matches!(receipt.status, 200 | 429 | 502 | 504));
    }
    assert_eq!(
        metrics
            .receipts
            .iter()
            .find(|v| v.logical == 8)
            .unwrap()
            .status,
        429
    );
    assert_eq!(
        metrics
            .receipts
            .iter()
            .find(|v| v.logical == 9)
            .unwrap()
            .status,
        504
    );
    assert_eq!(outcomes.iter().sum::<usize>(), 48);
    assert_eq!(outcomes[3], 0);
    let s = r.shutdown().await;
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
            s.unknown_usage,
            s.token_debt
        ),
        (0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
    );
    assert_eq!(
        (
            s.commitments,
            s.attempts,
            s.synthetic_measured,
            s.synthetic_charges
        ),
        (48, 48, Some(48), Some(96))
    );
    if mode == ConcurrencyMode::Adaptive {
        assert!(
            s.concurrency.decreases.unwrap() > 0,
            "missing pressure decrease"
        );
        assert!(
            s.concurrency.increases.unwrap() > 0,
            "missing demand-qualified recovery"
        );
    } else {
        assert_eq!(
            (s.concurrency.decreases, s.concurrency.increases),
            (Some(0), Some(0))
        );
    }
    let metrics = f.shutdown().await;
    heads.sort_unstable();
    completions.sort_unstable();
    queue_waits.sort_unstable();
    let elapsed = start.elapsed();
    let ids: Vec<RequestId> = commitments.iter().map(|v| v.1).collect();
    println!(
        "adaptive-demo: mode={mode:?} workload=48 batches=6 batch_size=8 capacity=[4,1,1,4,4,4] service_ms=100 hard_ceiling=4 host={} candidate=working-tree elapsed_ms={} throughput_per_s={:.2} commitments={} attempts={} receipts={} success={} refusal={} proxy={} other={} retries=0 client_peak={} handler_peak={} service_peak={} service_phase_peaks={:?} pending_peak={} handles_peak={} input_peak={} delta_items_peak={} delta_bytes_peak={} usage_peak={} exposure_peak={} captured_input={} fd_peak={:?} rss_peak_kib={:?} memory_method=sampled_proc_VmRSS fd_method=sampled_proc_fd head_us_p50_p95_max={:?} completion_us_p50_p95_max={:?} queue_wait_us_p50_p95_max={:?} phase_targets={:?} transitions={:?} controller={:?} commitment_logical_id_order_ms={:?} receipt_logical_id_status={:?} joined=true local_final=0 exposure_final=0 usage_final=0 debt_final=0",
        std::env::consts::OS,
        elapsed.as_millis(),
        48.0 / elapsed.as_secs_f64(),
        s.commitments,
        s.attempts,
        ids.len(),
        outcomes[0],
        outcomes[1],
        outcomes[2],
        outcomes[3],
        s.active_peak,
        metrics.handler_peak,
        metrics.service_peak,
        metrics.service_phase_peaks,
        s.pending_peak,
        s.handles_peak,
        s.input_peak,
        s.delta_items_peak,
        s.delta_bytes_peak,
        s.usage_peak,
        s.exposure_peak,
        metrics.captured_input,
        metrics.fd_peak,
        metrics.rss_peak_kib,
        (heads[23], heads[45], heads[47]),
        (completions[23], completions[45], completions[47]),
        (queue_waits[23], queue_waits[45], queue_waits[47]),
        phase_targets,
        transitions,
        s.concurrency,
        commitments,
        metrics
            .receipts
            .iter()
            .map(|v| (v.logical, v.id, v.status))
            .collect::<Vec<_>>()
    );
}
