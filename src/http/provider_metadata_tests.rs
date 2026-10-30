use super::*;
use serde_json::json;
#[test]
fn complete_metadata_wait_is_cancelled_or_deadlined_and_driver_is_joined() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            use tokio::io::AsyncReadExt;
            for cancel in [true, false] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = listener.local_addr().unwrap();
                let stopped = Arc::new(AtomicBool::new(false));
                let signal = stopped.clone();
                let server = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut head = Vec::new();
                    while !head.ends_with(b"\r\n\r\n") {
                        let mut b = [0];
                        socket.read_exact(&mut b).await.unwrap();
                        head.push(b[0]);
                        assert!(head.len() < 4096);
                    }
                    if cancel {
                        signal.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    let mut byte = [0];
                    assert_eq!(
                        tokio::time::timeout(
                            std::time::Duration::from_secs(7),
                            socket.read(&mut byte)
                        )
                        .await
                        .unwrap()
                        .unwrap(),
                        0
                    );
                });
                let start = std::time::Instant::now();
                assert!(get(endpoint, "/api/status", Some(&stopped)).await.is_err());
                if cancel {
                    assert!(start.elapsed() < std::time::Duration::from_secs(2));
                } else {
                    assert!(start.elapsed() >= std::time::Duration::from_secs(5));
                    assert!(start.elapsed() < std::time::Duration::from_secs(7));
                }
                server.await.unwrap();
            }
        });
}
#[test]
fn startup_refuses_changed_or_unavailable_route_without_fallback() {
    let status = json!({"startup_ready":true,"transition_state":"ready","version":"0.15.0-beta.1","model_catalog":{"source":"agentic","stale":false}});
    let models = json!({"data":[{"id":"gpt-6-luna"}]});
    let cards = json!({"revision":"2026-10-26-001","models":[{"requestId":"gpt-6-luna","provider":"openai","primaryProtocol":"openai-responses","supportedProtocols":["openai-responses"],"limits":{"context":1050000,"output":128000}}]});
    assert!(compatible(&status, &models, &cards));
    for key in [
        "startup_ready",
        "transition_state",
        "version",
        "model_catalog",
    ] {
        let mut status = status.clone();
        status[key] = Value::Null;
        assert!(!compatible(&status, &models, &cards));
    }
    for key in ["source", "stale"] {
        let mut status = status.clone();
        status["model_catalog"][key] = Value::Null;
        assert!(!compatible(&status, &models, &cards));
    }
    for key in [
        "requestId",
        "provider",
        "primaryProtocol",
        "supportedProtocols",
        "limits",
    ] {
        let mut cards = cards.clone();
        cards["models"][0][key] = Value::Null;
        assert!(!compatible(&status, &models, &cards));
    }
    let mut models = models.clone();
    models["data"] = json!([]);
    assert!(!compatible(&status, &models, &cards));
    let mut cards = cards.clone();
    cards["revision"] = json!("changed");
    assert!(!compatible(&status, &models, &cards));
}
