use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn start(body: Vec<u8>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    start_sequence(vec![(None, body)]).await
}

pub async fn start_sequence(
    responses: Vec<(Option<serde_json::Value>, Vec<u8>)>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    start_sequence_delayed(responses, std::time::Duration::ZERO, None).await
}

pub async fn start_sequence_delayed(
    responses: Vec<(Option<serde_json::Value>, Vec<u8>)>,
    delay: std::time::Duration,
    reached: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let continuation = responses.len() > 1;
        for (index, (expected, body)) in responses.into_iter().enumerate() {
            let chunk_bytes = if expected.is_some() { 1 } else { 127 };
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0; 1];
                socket.read_exact(&mut byte).await.unwrap();
                head.push(byte[0]);
                assert!(head.len() <= 4096);
            }
            let head = std::str::from_utf8(&head).unwrap().to_ascii_lowercase();
            assert!(head.starts_with("post /v1/responses http/1.1\r\n"));
            assert!(head.contains("authorization: bearer managed-by-proxy\r\n"));
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse()
                .unwrap();
            assert!(length <= 16384);
            let mut request = vec![0; length];
            socket.read_exact(&mut request).await.unwrap();
            let request: serde_json::Value = serde_json::from_slice(&request).unwrap();
            assert_eq!(request["model"], "gpt-6-luna");
            assert_eq!(request["max_output_tokens"], 1024);
            assert_eq!(
                request["include"],
                serde_json::json!(["reasoning.encrypted_content"])
            );
            if let Some(expected) = expected {
                assert_eq!(request, expected);
            }
            if index == 0 && !delay.is_zero() {
                if let Some(reached) = &reached {
                    reached.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                tokio::time::sleep(delay).await;
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            for chunk in body.chunks(chunk_bytes) {
                if socket.write_all(chunk).await.is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        if continuation {
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        }
    });
    (endpoint, task)
}
