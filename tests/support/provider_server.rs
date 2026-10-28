use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn start(body: Vec<u8>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
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
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        for chunk in body.chunks(127) {
            if socket.write_all(chunk).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    });
    (endpoint, task)
}
