use super::{RequestId, Runtime};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

pub struct Reply {
    pub text: String,
    pub done: bool,
    pub status: u16,
}
impl Reply {
    pub fn success(text: String) -> Self {
        Self {
            text,
            done: true,
            status: 200,
        }
    }
}
struct State {
    contexts: [Option<String>; 3],
    ids: [Option<RequestId>; 3],
    settled: [bool; 3],
    active: usize,
    peak: usize,
}
struct Shared {
    state: Mutex<State>,
    gates: [Semaphore; 3],
    changed: Notify,
    retry_after: Option<u64>,
    usage: [Option<u32>; 3],
}
pub struct ReadersFixture {
    pub endpoint: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    driver: JoinHandle<()>,
}
impl ReadersFixture {
    pub async fn start(replies: [Reply; 3]) -> Self {
        Self::start_with_retry_after(replies, None).await
    }
    pub async fn start_with_retry_after(replies: [Reply; 3], retry_after: Option<u64>) -> Self {
        Self::start_with_usage(replies, retry_after, [Some(1); 3]).await
    }
    pub async fn start_with_usage(
        replies: [Reply; 3],
        retry_after: Option<u64>,
        usage: [Option<u32>; 3],
    ) -> Self {
        assert!(usage.iter().flatten().all(|n| *n <= 65_536));
        assert!(replies.iter().all(|r| r.text.len() <= 20000));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                contexts: std::array::from_fn(|_| None),
                ids: [None; 3],
                settled: [false; 3],
                active: 0,
                peak: 0,
            }),
            gates: std::array::from_fn(|_| Semaphore::new(0)),
            changed: Notify::new(),
            retry_after,
            usage,
        });
        let (stop, mut stopped) = watch::channel(false);
        let owner = shared.clone();
        let replies = Arc::new(replies);
        let driver = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let mut accepted = 0;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    joined = handlers.join_next(), if !handlers.is_empty() => { joined.unwrap().unwrap(); }
                    socket = listener.accept(), if accepted < 3 => {
                        accepted += 1;
                        let (socket, _) = socket.unwrap();
                        let s = owner.clone(); let r = replies.clone(); let mut stop = stopped.clone();
                        handlers.spawn(async move { tokio::select! { _ = serve(socket, s, r) => {}, _ = stop.changed() => {} } });
                    }
                }
            }
            drop(listener);
            while let Some(joined) = handlers.join_next().await {
                joined.unwrap();
            }
        });
        Self {
            endpoint,
            shared,
            stop,
            driver,
        }
    }
    pub fn release(&self, child: usize) {
        self.shared.gates[child].add_permits(1);
    }
    pub fn release_all(&self) {
        for i in 0..3 {
            self.release(i);
        }
    }
    pub async fn wait_child(&self, child: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let changed = self.shared.changed.notified();
                if self.shared.state.lock().unwrap().contexts[child].is_some() {
                    break;
                }
                changed.await;
            }
        })
        .await
        .unwrap();
    }
    pub fn contexts(&self) -> [Option<String>; 3] {
        self.shared.state.lock().unwrap().contexts.clone()
    }
    pub fn peak(&self) -> usize {
        self.shared.state.lock().unwrap().peak
    }
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        self.driver.await.unwrap();
        let state = self.shared.state.lock().unwrap();
        for (child, id) in state.ids.iter().enumerate() {
            if let Some(id) = id {
                let _ = Runtime::reconcile_fixture(*id);
                let _ =
                    Runtime::reconcile_usage_fixture(*id, self.shared.usage[child].unwrap_or(1));
            }
        }
    }
}
struct Active {
    shared: Arc<Shared>,
    child: usize,
}
impl Drop for Active {
    fn drop(&mut self) {
        let mut s = self.shared.state.lock().unwrap();
        s.active -= 1;
        s.settled[self.child] = true;
        self.shared.changed.notify_waiters();
    }
}
async fn serve(mut socket: TcpStream, shared: Arc<Shared>, replies: Arc<[Reply; 3]>) {
    let mut request = Vec::with_capacity(8192);
    loop {
        let mut byte = [0];
        if socket.read(&mut byte).await.unwrap_or(0) == 0 {
            return;
        }
        assert!(request.len() < 8192);
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = std::str::from_utf8(&request).unwrap();
    let value = |name: &str| {
        header
            .lines()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                    .map(|(_, v)| v.trim())
            })
            .unwrap()
    };
    let len: usize = value("Content-Length").parse().unwrap();
    let id = RequestId(value("X-Fixture-Id").parse().unwrap());
    assert!(len <= 4096 && request.len() + len <= 8192);
    let start = request.len();
    request.resize(start + len, 0);
    if socket.read_exact(&mut request[start..]).await.is_err() {
        return;
    }
    let context = std::str::from_utf8(&request[start..]).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(context).unwrap();
    let child = parsed["child"].as_u64().unwrap() as usize;
    assert!(child < 3);
    {
        let mut s = shared.state.lock().unwrap();
        assert!(s.contexts[child].is_none());
        s.contexts[child] = Some(context.to_owned());
        s.ids[child] = Some(id);
        s.active += 1;
        s.peak = s.peak.max(s.active);
    }
    let active = Active {
        shared: shared.clone(),
        child,
    };
    shared.changed.notify_waiters();
    let mut byte = [0];
    let release = tokio::select! { permit = shared.gates[child].acquire() => { permit.unwrap().forget(); true }, _ = socket.read(&mut byte) => false };
    if release {
        let reply = &replies[child];
        let mut body = String::with_capacity(24000);
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
            shared
                .retry_after
                .map(|seconds| format!("Retry-After: {seconds}\r\n"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let metadata = shared.usage[child]
            .map(|n| {
                format!(
                    "X-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: {n}\r\n",
                    id.0
                )
            })
            .unwrap_or_default();
        let header = format!(
            "HTTP/1.1 {} fixture\r\n{retry}{metadata}Content-Length: {}\r\nConnection: close\r\n\r\n",
            reply.status,
            body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(body.as_bytes()).await;
        let _ = socket.shutdown().await;
    }
    drop(socket);
    drop(active);
}
pub fn golden(session: &str, generation: u64) -> [Reply; 3] {
    std::array::from_fn(|child| {
        Reply::success(format!(
            r#"{{"session":"{session}","generation":{generation},"child":{child},"summary":"reader-{child}"}}"#
        ))
    })
}
