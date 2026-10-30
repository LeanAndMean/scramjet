use super::*;
use crate::provider_fixture as fixture;
use crate::{ProviderInput, ProviderRequest};
use std::{os::unix::fs::PermissionsExt, process::Command};
#[path = "../../tests/support/provider_server.rs"]
mod server;

#[test]
fn provider_authority_boundaries() {
    let _fork_gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
    for case in [
        "cancel-settlement",
        "accepted-control",
        "fail-reserve",
        "fail-joined",
        "post-rate",
        "pending-cap",
        "freeze-unknown",
        "retained-unknown",
        "retained-known",
        "partial-freeze-unknown",
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::provider_tests::provider_boundary_child",
                "--nocapture",
            ])
            .env("SCRAMJET_PROVIDER_BOUNDARY", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
#[test]
fn consumer_deadline_reaches_transport_without_changing_default_attempt_limit() {
    let _fork_gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::provider_tests::consumer_deadline_child",
            "--nocapture",
        ])
        .env("HAIKU_CONSUMER_DEADLINE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn consumer_deadline_child() {
    if std::env::var_os("HAIKU_CONSUMER_DEADLINE").is_none() {
        return;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            use tokio::io::AsyncReadExt;
            let path = std::env::temp_dir().join(format!(
                "scramjet-consumer-deadline-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                if let Ok(Ok((mut socket, _))) =
                    tokio::time::timeout(Duration::from_millis(200), listener.accept()).await
                {
                    let mut byte = [0];
                    let result = socket.read(&mut byte).await;
                    assert!(
                        matches!(result, Ok(0))
                            || result.as_ref().is_err_and(
                                |error| error.kind() == std::io::ErrorKind::ConnectionReset
                            ),
                        "expired request wrote a POST byte: {result:?}"
                    );
                }
            });
            let runtime = Runtime::new_provider(endpoint, &path).unwrap();
            let shared = runtime.ownership();
            let (reached, mut reached_rx) = mpsc::channel(1);
            let release = Arc::new(Notify::new());
            *shared.probe.lock().unwrap() = Some(Probe {
                checkpoint: Checkpoint::BeforeTransport,
                reached,
                release: release.clone(),
            });
            let request =
                ProviderRequest::new(&[ProviderInput::User("synthetic".into())], &[]).unwrap();
            let mut handle = runtime
                .submit_provider_until(
                    request,
                    tokio::time::Instant::now() + Duration::from_millis(50),
                )
                .unwrap();
            reached_rx.recv().await.unwrap();
            tokio::time::sleep(Duration::from_millis(80)).await;
            release.notify_waiters();
            let response = handle.result().await.unwrap();
            assert_eq!(response.outcome, Outcome::Deadline);
            assert!(response.envelope.is_none());
            drop(handle);
            let local = runtime.shutdown().await;
            assert_eq!(local.active, 0);
            server.await.unwrap();
        });
}

#[test]
fn obtained_data_survives_elapsed_deadline_without_parser_or_settlement_authority() {
    let _fork_gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
    for case in ["captured", "uncaptured"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::provider_tests::frame_deadline_child",
                "--nocapture",
            ])
            .env("SCRAMJET_FRAME_DEADLINE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn frame_deadline_child() {
    let Ok(case) = std::env::var("SCRAMJET_FRAME_DEADLINE") else {
        return;
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        use crate::http::provider::{FRAME_PROBE, FrameProbe};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = std::env::temp_dir().join(format!("scramjet-frame-deadline-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let body = fixture::wire(&fixture::text_events("hé🙂", fixture::known_usage()), true);
        let expected = body.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                head.push(byte[0]);
                assert!(head.len() <= 4096);
            }
            let head = std::str::from_utf8(&head).unwrap().to_ascii_lowercase();
            let length: usize = head.lines().find_map(|line| line.strip_prefix("content-length: ")).unwrap().parse().unwrap();
            assert!(length <= 4096);
            socket.read_exact(&mut vec![0; length]).await.unwrap();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len() + 20).as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
            let mut byte = [0];
            match socket.read(&mut byte).await {
                Ok(0) => {},
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {},
                result => panic!("transport remained open: {result:?}"),
            }
            closed_tx.send(()).unwrap();
        });
        let runtime = Runtime::new_provider(endpoint, &path).unwrap();
        let shared = runtime.ownership();
        let (reached, mut reached_rx) = mpsc::channel(1);
        let release = Arc::new(Notify::new());
        *shared.probe.lock().unwrap() = Some(Probe { checkpoint: Checkpoint::ProviderBeforeAcknowledgement, reached, release: release.clone() });
        FRAME_PROBE.with(|probe| *probe.borrow_mut() = Some(FrameProbe { observed_at: tokio::time::Instant::now() + Duration::from_secs(60), frames: 0, data: Vec::new() }));
        let request = ProviderRequest::new(&[ProviderInput::User("synthetic".into())], &[]).unwrap();
        let (mut handle, mut receiver) = if case == "captured" {
            let (handle, receiver) = runtime.submit_provider_captured(request).unwrap();
            (handle, Some(receiver))
        } else { (runtime.submit_provider(request).unwrap(), None) };
        let id = handle.id();
        tokio::time::timeout(Duration::from_secs(5), reached_rx.recv()).await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(5), closed_rx).await.unwrap().unwrap();
        if let Some(receiver) = &mut receiver { assert!(matches!(receiver.0.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty))); }
        let before = runtime.provider_history().unwrap();
        assert_eq!(before.consumed(), 1);
        assert!(!before.attempts[0].locally_joined);
        release.notify_one();
        let response = tokio::time::timeout(Duration::from_secs(5), handle.result()).await.unwrap().unwrap();
        let probe = FRAME_PROBE.with(|probe| probe.borrow_mut().take().unwrap());
        assert_eq!(probe.frames, 1);
        assert!(!probe.data.is_empty());
        assert!(expected.starts_with(&probe.data));
        assert_eq!(response.outcome, Outcome::Deadline);
        assert_eq!(response.reservation, Some(1));
        assert!(response.durable);
        assert!(response.remote_terminal.is_none());
        assert!(response.usage.is_none());
        assert!(response.envelope.is_none());
        if let Some(receiver) = receiver {
            let capture = receiver.receive().await.unwrap();
            assert_eq!(capture.id, id);
            assert_eq!(capture.reservation, Some(1));
            assert_eq!(capture.bytes, probe.data);
            assert_eq!(capture.received_bytes, probe.data.len());
            assert_eq!(capture.parser_fed_bytes, 0);
            assert!(!capture.body_eof);
            assert!(!capture.parser_accepted);
            assert!(!capture.overflow);
            assert!(capture.local_joined);
        }
        let history = runtime.provider_history().unwrap();
        assert_eq!(history.consumed(), 1);
        assert!(history.attempts[0].locally_joined);
        assert_eq!(history.unresolved_remote(), 1);
        assert_eq!(history.unknown_usage(), 1);
        assert_eq!(runtime.snapshot().attempts, 1);
        drop(handle);
        let stats = runtime.shutdown().await;
        assert_eq!(stats.active, 0);
        server.await.unwrap();
    });
}

#[test]
fn provider_boundary_child() {
    let Ok(case) = std::env::var("SCRAMJET_PROVIDER_BOUNDARY") else {
        return;
    };
    let partial = case.starts_with("partial-");
    let case = case.strip_prefix("partial-").unwrap_or(&case);
    let case = case.to_owned();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let path=std::env::temp_dir().join(format!("scramjet-provider-boundary-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&path).unwrap();std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o700)).unwrap();
        let usage=if partial {serde_json::json!({"input_tokens":10})}else if matches!(case.as_str(),"freeze-unknown"|"retained-unknown") {serde_json::Value::Null}else{fixture::known_usage()};
        let events=if case=="accepted-control" {fixture::call_events("{}",usage)}else{fixture::text_events("synthetic",usage)};
        let body=fixture::wire(&events,false);
        let (endpoint, fixture_task)=if matches!(case.as_str(),"post-rate"|"retained-known") {
            use tokio::io::{AsyncReadExt,AsyncWriteExt};
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let endpoint=listener.local_addr().unwrap();
            (endpoint,tokio::spawn(async move {
                for _ in 0..2 {
                    let (mut socket,_)=listener.accept().await.unwrap();let mut head=Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {let mut byte=[0;1];socket.read_exact(&mut byte).await.unwrap();head.push(byte[0]);}
                    let head=std::str::from_utf8(&head).unwrap().to_ascii_lowercase();
                    let length:usize=head.lines().find_map(|l|l.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                    socket.read_exact(&mut vec![0;length]).await.unwrap();
                    socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
                    socket.write_all(&body).await.unwrap();
                }
            }))
        }else{server::start(body).await};
        let runtime=if matches!(case.as_str(),"freeze-unknown"|"retained-unknown"|"retained-known") {
            if case!="freeze-unknown" {let seed=ProviderLedger::open(&path,endpoint).unwrap();seed.lock().unwrap().reserve(100).unwrap();drop(seed);}
            let prefix=if case=="freeze-unknown" {0}else{1};
            assert!(Runtime::provider_with_policy(endpoint,&path,Some(prefix+1)).is_err());
            Runtime::provider_with_policy(endpoint,&path,Some(prefix)).unwrap()
        }else{Runtime::new_provider(endpoint,&path).unwrap()};
        let shared=runtime.ownership();
        let ledger=shared.state.lock().unwrap().provider.as_ref().unwrap().clone();
        if case=="fail-reserve" {ledger.lock().unwrap().force_write_error();}
        let (reached,mut reached_rx)=mpsc::channel(1);let release=Arc::new(Notify::new());
        if matches!(case.as_str(),"cancel-settlement"|"fail-joined"|"post-rate") {
            *shared.probe.lock().unwrap()=Some(Probe {checkpoint:if case=="post-rate" {Checkpoint::ProviderBeforeDispatch}else{Checkpoint::ProviderBeforeAcknowledgement},reached,release:release.clone()});
        }
        let make_request=||ProviderRequest::new(&[ProviderInput::User("synthetic".into())],&[]).unwrap();
        let mut handle=runtime.submit_provider(make_request()).unwrap();
        if case=="pending-cap" {
            let (mut captured_pending, receiver)=runtime.submit_provider_captured(make_request()).unwrap();
            let mut pending=Vec::new();for _ in 1..MAX_PENDING {pending.push(runtime.submit_provider(make_request()).unwrap());}
            assert_eq!(runtime.snapshot().pending,MAX_PENDING);
            assert!(matches!(runtime.submit_provider(make_request()),Err(SubmitError::Full)));
            captured_pending.cancel();assert_eq!(captured_pending.result().await.unwrap().outcome,Outcome::Cancelled);
            assert!(tokio::time::timeout(Duration::from_secs(1),receiver.receive()).await.unwrap().is_none());
            drop(captured_pending);drop(pending);
        }
        if matches!(case.as_str(),"cancel-settlement"|"fail-joined"|"post-rate") {
            tokio::time::timeout(Duration::from_secs(5),reached_rx.recv()).await.unwrap().unwrap();
            match case.as_str() {
                "cancel-settlement"=>handle.cancel(),
                "fail-joined"=>ledger.lock().unwrap().force_write_error(),
                "post-rate"=>tokio::time::sleep(Duration::from_millis(1200)).await,
                _=>unreachable!(),
            }
            release.notify_one();
        }
        let response=tokio::time::timeout(Duration::from_secs(5),handle.result()).await.unwrap().unwrap();
        drop(handle);
        match case.as_str() {
            "cancel-settlement"=>{assert_eq!(response.outcome,Outcome::Cancelled);assert!(response.envelope.is_none());assert!(response.durable);assert!(response.remote_terminal.is_some());assert!(response.usage.unwrap().complete());},
            "fail-reserve"=>{assert_eq!(response.outcome,Outcome::ResourceLimit);assert!(response.reservation.is_none());assert!(!response.durable);assert_eq!(runtime.snapshot().attempts,0);},
            "fail-joined"=>{assert_eq!(response.outcome,Outcome::ResourceLimit);assert!(response.envelope.is_none());assert!(!response.durable);assert!(response.remote_terminal.is_some());assert_eq!(runtime.provider_history().unwrap().unresolved_remote(),1);},
            "accepted-control"|"pending-cap"=>{assert_eq!(response.outcome,Outcome::Success);assert!(response.envelope.is_some());},
            "freeze-unknown"|"retained-unknown"=>{
                assert_eq!(response.outcome,Outcome::Success);if partial {assert_eq!(response.usage.as_ref().unwrap().input_tokens,Some(10));assert!(!response.usage.as_ref().unwrap().complete());}else{assert!(response.usage.is_none());}
                let mut pending=runtime.submit_provider(make_request()).unwrap();
                tokio::time::sleep(Duration::from_millis(1200)).await;
                assert_eq!(runtime.snapshot().attempts,1);assert_eq!(runtime.snapshot().pending,1);
                pending.cancel();assert_eq!(pending.result().await.unwrap().outcome,Outcome::Cancelled);drop(pending);
                assert_eq!(runtime.provider_history().unwrap().consumed(),if case=="freeze-unknown" {1}else{2});
            },
            "post-rate"|"retained-known"=>{
                assert_eq!(response.outcome,Outcome::Success);
                let first=shared.state.lock().unwrap().provider_dispatch.unwrap();
                let mut handle=runtime.submit_provider(make_request()).unwrap();
                assert_eq!(handle.result().await.unwrap().outcome,Outcome::Success);drop(handle);
                let second=shared.state.lock().unwrap().provider_dispatch.unwrap();
                assert!(second.saturating_duration_since(first)>=Duration::from_secs(1));
                assert_eq!(runtime.snapshot().active_peak,1);
                if case=="retained-known" {
                    let history=runtime.provider_history().unwrap();assert_eq!(history.consumed(),3);
                    assert_eq!(history.unresolved_remote(),1);assert_eq!(history.unknown_usage(),1);
                    assert!(history.attempts[0].terminal.is_none());assert!(!history.has_unknown_after(1));
                }
            },
            _=>unreachable!(),
        }
        if matches!(case.as_str(),"fail-reserve"|"fail-joined") {
            let attempts=runtime.snapshot().attempts;
            let mut handle=runtime.submit_provider(make_request()).unwrap();handle.cancel();assert_eq!(handle.result().await.unwrap().outcome,Outcome::Cancelled);drop(handle);
            assert_eq!(runtime.snapshot().attempts,attempts);
        }
        runtime.shutdown().await;
        drop(shared);
        drop(ledger);
        if case=="fail-reserve" {fixture_task.abort();let _=fixture_task.await;}else{fixture_task.await.unwrap();}
    });
}
