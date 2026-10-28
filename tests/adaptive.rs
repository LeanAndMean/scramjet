use scramjet_rs::{
    ConcurrencyMode, Outcome, RequestId, RequestRate, Runtime, SyntheticCost, SyntheticTokenBudget,
    SyntheticUsage,
};
use std::{sync::Mutex, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
static OWNER: Mutex<()> = Mutex::new(());

fn execute(f: impl std::future::Future<Output = ()>) {
    let _owner = OWNER.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f);
}

#[test]
fn correlated_http_matrix_keeps_pressure_distinct_from_failure_and_usage() {
    execute(async {
        for (status, retry, usage, body, expected) in [
            (
                429,
                "Retry-After: 1\r\n",
                "1",
                "",
                Outcome::HttpRejected(429),
            ),
            (429, "", "", "", Outcome::HttpRejected(429)),
            (
                429,
                "Retry-After: nonsense\r\n",
                "bad",
                "",
                Outcome::HttpRejected(429),
            ),
            (
                502,
                "Retry-After: 10\r\n",
                "",
                "",
                Outcome::HttpRejected(502),
            ),
            (504, "", "1", "", Outcome::HttpRejected(504)),
            (
                500,
                "Retry-After: 10\r\n",
                "1",
                "",
                Outcome::HttpRejected(500),
            ),
            (200, "", "bad", "data: [DONE]\n\n", Outcome::Framing),
            (200, "", "3", "data: [DONE]\n\n", Outcome::ResourceLimit),
            (200, "", "1", "data: partial\n\n", Outcome::Incomplete),
            (200, "", "1", "event: x\n\n", Outcome::Framing),
            (200, "", "", "data: [DONE]\n\n", Outcome::Success),
            (200, "", "1", "data: [DONE]\n\n", Outcome::Success),
        ] {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let worker = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = Vec::with_capacity(8192);
                loop {
                    let mut b = [0];
                    socket.read_exact(&mut b).await.unwrap();
                    input.push(b[0]);
                    assert!(input.len() <= 8192);
                    if input.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let text = std::str::from_utf8(&input).unwrap();
                let value = |name: &str| {
                    text.lines()
                        .find_map(|l| {
                            l.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                                .map(|(_, v)| v.trim())
                        })
                        .unwrap()
                };
                let id = RequestId(value("X-Fixture-Id").parse().unwrap());
                let length: usize = value("Content-Length").parse().unwrap();
                assert!(length <= 4096 && input.len() + length <= 8192);
                let mut body_input = vec![0; length];
                socket.read_exact(&mut body_input).await.unwrap();
                let metadata = if usage.is_empty() {
                    String::new()
                } else {
                    format!(
                        "X-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: {usage}\r\n",
                        id.0
                    )
                };
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\n{retry}{metadata}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                drop(socket);
                id
            });
            let r = Runtime::new(
                address,
                4,
                RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
                ConcurrencyMode::Adaptive,
            )
            .unwrap();
            let mut h = r.submit("one", SyntheticCost::new(1, 1).unwrap()).unwrap();
            while h.next_delta().await.is_some() {}
            let terminal = h.terminal().await.unwrap();
            assert_eq!(terminal.id, h.id());
            assert_eq!(terminal.outcome, expected);
            assert!(terminal.dispatch_to_head.is_some());
            assert!(terminal.commitment_to_terminal.unwrap() >= terminal.dispatch_to_head.unwrap());
            assert!(!terminal.output_backpressure);
            assert!(h.terminal().await.is_none());
            let s = r.snapshot();
            let pressure = matches!(status, 429 | 502 | 504);
            assert_eq!(s.concurrency.target, if pressure { 2 } else { 4 });
            assert_eq!(s.concurrency.decreases, Some(u64::from(pressure)));
            assert_eq!(
                s.cooldown_signals,
                usize::from(status == 429 && retry.contains(": 1"))
            );
            if status == 429 {
                assert_eq!(terminal.retry_after.unwrap().missing, retry.is_empty());
            }
            assert_eq!(s.concurrency.increases, Some(0));
            assert_eq!(worker.await.unwrap(), h.id());
            if s.unresolved > 0 {
                Runtime::reconcile_fixture(h.id()).unwrap();
            }
            if s.unknown_usage > 0 {
                assert_eq!(terminal.synthetic_usage, SyntheticUsage::Unknown);
                Runtime::reconcile_usage_fixture(h.id(), 1).unwrap();
            }
            drop(h);
            let s = r.shutdown().await;
            assert_eq!(
                (
                    s.active,
                    s.pending,
                    s.handles,
                    s.unresolved,
                    s.usage_obligations
                ),
                (0, 0, 0, 0, 0)
            );
            tokio::time::sleep(s.cooldown_remaining + Duration::from_millis(5)).await;
        }
    });
}

#[test]
fn missing_response_head_is_absent_timing_not_zero_or_pressure() {
    execute(async {
        for partial in [false, true] {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let r = Runtime::new(
                listener.local_addr().unwrap(),
                4,
                RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
                ConcurrencyMode::Adaptive,
            )
            .unwrap();
            let mut h = r.submit("one", SyntheticCost::new(1, 1).unwrap()).unwrap();
            let worker = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut b = [0; 1024];
                assert!(socket.read(&mut b).await.unwrap() > 0);
                if partial {
                    socket
                        .write_all(b"HTTP/1.1 502 Fixture\r\nX:")
                        .await
                        .unwrap();
                }
                drop(socket);
            });
            let t = h.terminal().await.unwrap();
            worker.await.unwrap();
            assert!(matches!(t.outcome, Outcome::Transport | Outcome::Framing));
            assert!(t.dispatch_to_head.is_none());
            assert!(t.commitment_to_terminal.is_some());
            assert_eq!(r.snapshot().concurrency.target, 4);
            assert_eq!(r.snapshot().concurrency.proxy_errors, Some(0));
            Runtime::reconcile_fixture(h.id()).unwrap();
            Runtime::reconcile_usage_fixture(h.id(), 1).unwrap();
            drop(h);
            r.shutdown().await;
        }
    });
}
