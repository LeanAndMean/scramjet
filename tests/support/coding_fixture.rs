use super::{RequestId, Runtime};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub struct Reply {
    pub text: String,
    pub done: bool,
    pub status: u16,
    pub delay_ms: u64,
}
impl Reply {
    pub fn success(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            done: true,
            status: 200,
            delay_ms: 0,
        }
    }
}
pub struct ModelFixture {
    pub endpoint: SocketAddr,
    settled: Arc<Mutex<Vec<(RequestId, u32)>>>,
    requests: Arc<Mutex<Vec<String>>>,
    notify: Arc<Notify>,
    task: Option<JoinHandle<()>>,
}
impl ModelFixture {
    pub async fn start(replies: Vec<Reply>) -> Self {
        Self::start_with_retry_after(replies, None).await
    }
    pub async fn start_with_retry_after(replies: Vec<Reply>, retry_after: Option<u64>) -> Self {
        Self::start_with_usage(replies, retry_after, [Some(1); 8]).await
    }
    pub async fn start_with_usage(
        replies: Vec<Reply>,
        retry_after: Option<u64>,
        usage: [Option<u32>; 8],
    ) -> Self {
        assert!(usage.iter().flatten().all(|n| *n <= 65_536));
        assert!(replies.len() <= 8);
        assert!(
            replies
                .iter()
                .all(|r| r.text.len() <= 20_000 && r.delay_ms <= 1000)
        );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let settled = Arc::new(Mutex::new(Vec::with_capacity(8)));
        let requests = Arc::new(Mutex::new(Vec::with_capacity(8)));
        let notify = Arc::new(Notify::new());
        let (s, r, n) = (settled.clone(), requests.clone(), notify.clone());
        let task = tokio::spawn(async move {
            for (index, reply) in replies.into_iter().enumerate() {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut input = Vec::with_capacity(8192);
                let (header, body_start) = loop {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count != 0 && input.len() + count <= 8192);
                    input.extend_from_slice(&chunk[..count]);
                    if let Some(pos) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (String::from_utf8(input[..pos].to_vec()).unwrap(), pos + 4);
                    }
                };
                let header = header.to_ascii_lowercase();
                let value = |key: &str| {
                    header
                        .lines()
                        .find_map(|l| l.strip_prefix(key))
                        .unwrap()
                        .trim()
                };
                let id = RequestId(value("x-fixture-id:").parse().unwrap());
                let length: usize = value("content-length:").parse().unwrap();
                assert!(length <= 4096);
                while input.len() < body_start + length {
                    let mut chunk = [0; 1024];
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count != 0 && input.len() + count <= 8192);
                    input.extend_from_slice(&chunk[..count]);
                }
                r.lock().unwrap().push(
                    String::from_utf8(input[body_start..body_start + length].to_vec()).unwrap(),
                );
                tokio::time::sleep(std::time::Duration::from_millis(reply.delay_ms)).await;
                let mut body = String::new();
                let mut start = 0;
                while start < reply.text.len() {
                    let mut end = (start + 900).min(reply.text.len());
                    while !reply.text.is_char_boundary(end) {
                        end -= 1;
                    }
                    body.push_str("data: ");
                    body.push_str(&reply.text[start..end]);
                    body.push_str("\n\n");
                    start = end;
                }
                if reply.done {
                    body.push_str("data: [DONE]\n\n");
                }
                let retry = if reply.status == 429 {
                    retry_after
                        .map(|seconds| format!("Retry-After: {seconds}\r\n"))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let metadata = usage[index]
                    .map(|n| {
                        format!(
                            "X-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: {n}\r\n",
                            id.0
                        )
                    })
                    .unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 {} fixture\r\n{retry}{metadata}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
                drop(socket);
                s.lock().unwrap().push((id, usage[index].unwrap_or(1)));
                n.notify_one();
            }
        });
        Self {
            endpoint,
            settled,
            requests,
            notify,
            task: Some(task),
        }
    }
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
    pub async fn settle(&self, attempts: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let ready = self.notify.notified();
                if self.settled.lock().unwrap().len() >= attempts {
                    break;
                }
                ready.await;
            }
        })
        .await
        .unwrap();
        for (id, actual) in self.settled.lock().unwrap().iter().copied() {
            let _ = Runtime::reconcile_fixture(id);
            let _ = Runtime::reconcile_usage_fixture(id, actual);
        }
        assert_eq!(
            Runtime::process_snapshot().unresolved,
            0,
            "fixture attestation did not settle every correlated exposure"
        );
    }
    pub async fn stop(mut self) {
        let task = self.task.take().unwrap();
        task.abort();
        let _ = task.await;
    }
}
impl Drop for ModelFixture {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub fn golden() -> Vec<Reply> {
    [
        r#"{"kind":"Read","op":1,"target":"sample.txt"}"#,
        r#"{"kind":"Replace","op":2,"target":"sample.txt","expected":"before\n","replacement":"after\n"}"#,
        r#"{"kind":"Verify","op":3,"target":"sample.txt","expected":"after\n"}"#,
        r#"{"kind":"Final","summary":"Exact disposable edit completed and verified."}"#,
    ].into_iter().map(Reply::success).collect()
}
