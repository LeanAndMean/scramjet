use scramjet_rs::{
    Outcome, RequestId, RequestRate, Runtime, SyntheticCost, SyntheticTokenBudget, SyntheticUsage,
};
use std::{sync::Mutex, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
static OWNER: Mutex<()> = Mutex::new(());
fn execute(future: impl std::future::Future<Output = ()>) {
    let _owner = OWNER.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            tokio::time::timeout(Duration::from_secs(25), future)
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        });
}
async fn response_case(
    metadata: &str,
    status: u16,
    body: &str,
    expected: Outcome,
    usage: SyntheticUsage,
) {
    response_delivery_case(metadata, status, body, expected, usage, "", false).await;
}
async fn response_delivery_case(
    metadata: &str,
    status: u16,
    body: &str,
    expected: Outcome,
    usage: SyntheticUsage,
    prefix: &str,
    fragmented: bool,
) {
    let prefix = prefix.to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let fixture_metadata = metadata.to_owned();
    let body = body.to_owned();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut input = Vec::with_capacity(4096);
        loop {
            let mut b = [0];
            socket.read_exact(&mut b).await.unwrap();
            input.push(b[0]);
            assert!(input.len() <= 4096);
            if input.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = std::str::from_utf8(&input).unwrap();
        let id = head
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("x-fixture-id:")
                    .map(|v| v.trim().parse::<u64>().unwrap())
            })
            .unwrap();
        let mut request_body = [0];
        socket.read_exact(&mut request_body).await.unwrap();
        let metadata = fixture_metadata.replace("{id}", &id.to_string());
        let interim = if status == 100 || status == 103 {
            format!("HTTP/1.1 {status} Interim\r\n\r\n")
        } else {
            String::new()
        };
        let final_status = if interim.is_empty() { status } else { 200 };
        let reply = format!(
            "{prefix}{interim}HTTP/1.1 {final_status} fixture\r\n{metadata}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if fragmented {
            for byte in reply.as_bytes() {
                if socket.write_all(&[*byte]).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        } else {
            let _ = socket.write_all(reply.as_bytes()).await;
        }
        let _ = socket.shutdown().await;
    });
    let runtime = Runtime::new(
        endpoint,
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65_536, 65_536, Duration::from_millis(1)).unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .unwrap();
    let mut h = runtime
        .submit("x", SyntheticCost::new(1, 1).unwrap())
        .unwrap();
    while h.next_delta().await.is_some() {}
    let t = h.terminal().await.unwrap();
    assert_eq!(t.outcome, expected, "{metadata}");
    assert_eq!(t.synthetic_usage, usage, "{metadata}");
    assert!(h.terminal().await.is_none());
    server.await.unwrap();
    let s = runtime.snapshot();
    if let SyntheticUsage::Known(actual) = usage {
        assert_eq!(s.synthetic_measured, Some(u64::from(actual)));
        assert_eq!(s.synthetic_charges, Some(u64::from(actual.max(2))));
        assert_eq!(s.usage_obligations, 0);
        assert!(Runtime::reconcile_usage_fixture(h.id(), actual).is_err());
    } else {
        assert_eq!(
            (s.synthetic_measured, s.synthetic_charges, s.unknown_usage),
            (Some(0), Some(2), 1)
        );
        Runtime::reconcile_usage_fixture(h.id(), 1).unwrap();
    }
    let _ = Runtime::reconcile_fixture(h.id());
    drop(h);
    runtime.shutdown().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
}
#[test]
fn real_http_usage_qualification_and_failed_usage_accounting() {
    execute(async {
        let valid = "X-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Tokens: 1\r\n";
        for status in [100, 103] {
            response_case(
                valid,
                status,
                "data: [DONE]\n\n",
                Outcome::Framing,
                SyntheticUsage::Unknown,
            )
            .await;
            response_case(
                "X-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Tokens:  1 \r\n",
                status,
                "data: [DONE]\n\n",
                Outcome::Framing,
                SyntheticUsage::Unknown,
            )
            .await;
        }
        for (value, expected) in [
            ("0", Outcome::Success),
            ("1", Outcome::Success),
            ("65536", Outcome::ResourceLimit),
        ] {
            let metadata =
                format!("X-Fixture-Usage-Id: {{id}}\r\nX-Fixture-Usage-Tokens: {value}\r\n");
            response_case(
                &metadata,
                200,
                "data: [DONE]\n\n",
                expected,
                SyntheticUsage::Known(value.parse().unwrap()),
            )
            .await;
        }
        response_case(
            "",
            200,
            "data: [DONE]\n\n",
            Outcome::Success,
            SyntheticUsage::Unknown,
        )
        .await;
        for metadata in [
            "X-Fixture-Usage-Id: {id}\r\n",
            "X-Fixture-Usage-Tokens: 1\r\n",
            "X-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Tokens: 1\r\n",
            "X-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Tokens: 1\r\nX-Fixture-Usage-Tokens: 1\r\n",
            "X-Fixture-Usage-Id: 0\r\nX-Fixture-Usage-Tokens: 1\r\n",
            "X-Fixture-Usage-Id: 18446744073709551616\r\nX-Fixture-Usage-Tokens: 1\r\n",
        ] {
            response_case(
                metadata,
                200,
                "data: [DONE]\n\n",
                Outcome::Framing,
                SyntheticUsage::Unknown,
            )
            .await;
        }
        for value in [
            " 1",
            "1 ",
            "\t1",
            "1\t",
            "",
            "+1",
            "-1",
            "1,2",
            "65537",
            "000001",
            "18446744073709551616",
            "1 2",
        ] {
            let metadata =
                format!("X-Fixture-Usage-Id: {{id}}\r\nX-Fixture-Usage-Tokens: {value}\r\n");
            response_case(
                &metadata,
                200,
                "data: [DONE]\n\n",
                Outcome::Framing,
                SyntheticUsage::Unknown,
            )
            .await;
        }
        for (body, expected) in [
            ("data: partial\n\n", Outcome::Incomplete),
            ("data: [DONE]\n\nx", Outcome::Framing),
            ("event: x\n\n", Outcome::Framing),
        ] {
            response_case(valid, 200, body, expected, SyntheticUsage::Unknown).await;
        }
        response_case(
            valid,
            429,
            "",
            Outcome::HttpRejected(429),
            SyntheticUsage::Known(1),
        )
        .await;
        response_case(
            "X-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Tokens: 65536\r\nRetry-After: 0\r\n",
            429,
            "",
            Outcome::HttpRejected(429),
            SyntheticUsage::Known(65_536),
        )
        .await;
        response_case(
            "X-Fixture-Usage-Id: 0\r\nX-Fixture-Usage-Tokens: 1\r\n",
            502,
            "",
            Outcome::HttpRejected(502),
            SyntheticUsage::Unknown,
        )
        .await;
    });
}

#[test]
fn leading_blank_heads_never_authorize_success_or_known_usage() {
    execute(async {
        for fragmented in [false, true] {
            response_delivery_case(
                "X-Fixture-Usage-Id: {id}\r\nX-Fixture-Usage-Tokens: 1\r\n",
                200,
                "data: [DONE]\n\n",
                Outcome::Success,
                SyntheticUsage::Known(1),
                "",
                fragmented,
            )
            .await;
            for prefix in ["\r\n", "\n"] {
                for status in [200, 100, 103] {
                    for value in ["1", " 1 "] {
                        response_delivery_case(
                            &format!(
                                "X-Fixture-Usage-Id: {{id}}\r\nX-Fixture-Usage-Tokens: {value}\r\n"
                            ),
                            status,
                            "data: [DONE]\n\n",
                            Outcome::Framing,
                            SyntheticUsage::Unknown,
                            prefix,
                            fragmented,
                        )
                        .await;
                    }
                }
            }
        }
    });
}

#[test]
fn actual_unknown_usage_pause_exposure_independence_and_debt_recovery() {
    execute(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (receipts, mut received) = tokio::sync::mpsc::channel(5);
        let server = tokio::spawn(async move {
            for i in 0..5 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = Vec::with_capacity(4096);
                loop {
                    let mut b = [0];
                    socket.read_exact(&mut b).await.unwrap();
                    input.push(b[0]);
                    assert!(input.len() <= 4096);
                    if input.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let head = std::str::from_utf8(&input).unwrap();
                let id = RequestId(
                    head.lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("x-fixture-id:")
                                .map(|v| v.trim().parse::<u64>().unwrap())
                        })
                        .unwrap(),
                );
                let mut b = [0];
                socket.read_exact(&mut b).await.unwrap();
                receipts.send(id).await.unwrap();
                let metadata = if i == 4 {
                    format!(
                        "X-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\n",
                        id.0
                    )
                } else {
                    String::new()
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\n{metadata}Content-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        let r = Runtime::new(
            endpoint,
            2,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            SyntheticTokenBudget::new(8, 2, Duration::from_millis(100)).unwrap(),
            scramjet_rs::ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut ids = Vec::with_capacity(4);
        for _ in 0..4 {
            let mut h = r.submit("x", SyntheticCost::new(1, 1).unwrap()).unwrap();
            ids.push(h.id());
            assert_eq!(received.recv().await, Some(h.id()));
            assert_eq!(
                h.terminal().await.unwrap().synthetic_usage,
                SyntheticUsage::Unknown
            );
            drop(h);
        }
        tokio::time::sleep(Duration::from_millis(410)).await;
        assert_eq!(
            (
                r.snapshot().unknown_usage,
                r.snapshot().token_credit,
                r.snapshot().unresolved
            ),
            (4, 8, 0)
        );
        let mut h = r.submit("x", SyntheticCost::new(1, 1).unwrap()).unwrap();
        assert!(received.try_recv().is_err());
        Runtime::reconcile_usage_fixture(ids[0], 14).unwrap();
        assert_eq!((r.snapshot().token_credit, r.snapshot().token_debt), (0, 4));
        assert_eq!(r.snapshot().synthetic_charges, Some(20));
        assert!(Runtime::reconcile_usage_fixture(ids[0], 14).is_err());
        assert!(received.try_recv().is_err());
        assert_eq!(received.recv().await, Some(h.id()));
        assert_eq!(
            h.terminal().await.unwrap().synthetic_usage,
            SyntheticUsage::Known(1)
        );
        drop(h);
        for id in &ids[1..] {
            Runtime::reconcile_usage_fixture(*id, 1).unwrap();
        }
        assert_eq!(r.snapshot().synthetic_measured, Some(18));
        assert_eq!(r.snapshot().synthetic_charges, Some(22));
        server.await.unwrap();
        let s = r.shutdown().await;
        assert_eq!(
            (
                s.active,
                s.pending,
                s.handles,
                s.usage_obligations,
                s.unresolved
            ),
            (0, 0, 0, 0, 0)
        );
        tokio::time::sleep(Duration::from_millis(410)).await;
    });
}
