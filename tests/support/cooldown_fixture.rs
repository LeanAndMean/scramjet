use scramjet_rs::RequestId;
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Copy, Debug)]
pub struct Receipt {
    pub id: RequestId,
    pub received: Instant,
    pub input: usize,
}
struct State {
    receipts: Vec<Receipt>,
    settled: Vec<RequestId>,
    active: usize,
    peak: usize,
    fd_peak: usize,
}
struct Shared {
    state: Mutex<State>,
    changed: Notify,
    gates: [Semaphore; 12],
    receipt_gate: Option<(usize, Arc<Semaphore>)>,
}
pub struct Fixture {
    pub address: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    driver: JoinHandle<io::Result<()>>,
}
fn fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, |it| it.count())
}
impl Fixture {
    pub async fn new() -> Self {
        Self::new_with_receipt_gate(None).await
    }
    pub async fn new_with_receipt_gate(receipt_gate: Option<(usize, Arc<Semaphore>)>) -> Self {
        assert!(receipt_gate.as_ref().is_none_or(|(index, _)| *index < 12));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                receipts: Vec::with_capacity(12),
                settled: Vec::with_capacity(12),
                active: 0,
                peak: 0,
                fd_peak: fd_count(),
            }),
            changed: Notify::new(),
            gates: std::array::from_fn(|_| Semaphore::new(0)),
            receipt_gate,
        });
        let owner = shared.clone();
        let (stop, mut stopped) = watch::channel(false);
        let driver = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let mut accepted = 0;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    joined = handlers.join_next(), if !handlers.is_empty() => { joined.unwrap().unwrap()?; }
                    result = listener.accept(), if accepted < 12 && handlers.len() < 12 => {
                        let (socket, _) = result?;
                        accepted += 1;
                        let shared = owner.clone();
                        let mut stop = stopped.clone();
                        handlers.spawn(async move {
                            tokio::select! { result = serve(socket, &shared) => result, _ = stop.changed() => Ok(()) }
                        });
                    }
                }
            }
            drop(listener);
            while let Some(joined) = handlers.join_next().await {
                joined.unwrap()?;
            }
            Ok(())
        });
        Self {
            address,
            shared,
            stop,
            driver,
        }
    }
    pub fn release(&self, index: usize) {
        self.shared.gates[index].add_permits(1);
    }
    pub fn receipts(&self) -> Vec<Receipt> {
        self.shared.state.lock().unwrap().receipts.clone()
    }
    pub fn metrics(&self) -> (usize, usize) {
        let state = self.shared.state.lock().unwrap();
        (state.peak, state.fd_peak)
    }
    pub async fn wait_started(&self, count: usize) {
        self.wait(|s| s.receipts.len() >= count).await;
    }
    pub async fn wait_settled(&self, id: RequestId) {
        self.wait(|s| s.settled.contains(&id)).await;
        let _ = scramjet_rs::Runtime::reconcile_usage_fixture(id, 1);
    }
    async fn wait(&self, predicate: impl Fn(&State) -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let changed = self.shared.changed.notified();
                if predicate(&self.shared.state.lock().unwrap()) {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("cooldown fixture observation timeout");
    }
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        self.driver.await.unwrap().unwrap();
        for id in &self.shared.state.lock().unwrap().settled {
            let _ = scramjet_rs::Runtime::reconcile_usage_fixture(*id, 1);
        }
    }
}
struct Active<'a> {
    shared: &'a Shared,
    id: RequestId,
}
impl Drop for Active<'_> {
    fn drop(&mut self) {
        let mut state = self.shared.state.lock().unwrap();
        state.active -= 1;
        state.settled.push(self.id);
        drop(state);
        self.shared.changed.notify_one();
    }
}
async fn serve(mut socket: TcpStream, shared: &Shared) -> io::Result<()> {
    let mut request = Vec::with_capacity(8192);
    let end = loop {
        if request.len() == 8192 {
            return Err(io::Error::other("fixture head limit"));
        }
        let mut byte = [0];
        if socket.read(&mut byte).await? == 0 {
            return Ok(());
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break request.len();
        }
    };
    let head = std::str::from_utf8(&request).map_err(io::Error::other)?;
    let header = |name: &str| {
        head.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
    };
    let len: usize = header("Content-Length").unwrap().parse().unwrap();
    let id = RequestId(header("X-Fixture-Id").unwrap().parse().unwrap());
    if len > 4096 || end + len > 8192 {
        return Err(io::Error::other("fixture request limit"));
    }
    request.resize(end + len, 0);
    socket.read_exact(&mut request[end..]).await?;
    let input = std::str::from_utf8(&request[end..]).map_err(io::Error::other)?;
    let (index, scenario) = input
        .split_once(':')
        .ok_or_else(|| io::Error::other("fixture scenario"))?;
    let index: usize = index.parse().map_err(io::Error::other)?;
    if index >= 12 {
        return Err(io::Error::other("fixture gate limit"));
    }
    if let Some((gated_index, gate)) = &shared.receipt_gate
        && index == *gated_index
    {
        gate.acquire().await.unwrap().forget();
    }
    {
        let mut state = shared.state.lock().unwrap();
        if state.receipts.len() == 12 {
            return Err(io::Error::other("fixture capture limit"));
        }
        state.receipts.push(Receipt {
            id,
            received: Instant::now(),
            input: len,
        });
        state.active += 1;
        state.peak = state.peak.max(state.active);
        state.fd_peak = state.fd_peak.max(fd_count());
    }
    let active = Active { shared, id };
    shared.changed.notify_one();
    let result = async {
        let mut byte = [0];
        tokio::select! {
            permit = shared.gates[index].acquire() => permit.unwrap().forget(),
            result = socket.read(&mut byte) => { result?; return Ok(()); }
        }
        if scenario == "stall" { let _ = socket.read(&mut byte).await?; return Ok(()); }
        let response = match scenario {
            "ok" => "HTTP/1.1 200 OK\r\nContent-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n".to_owned(),
            "missing" => "HTTP/1.1 429 Refused\r\nContent-Length: 0\r\n\r\n".to_owned(),
            "invalid" => "HTTP/1.1 429 Refused\r\nRetry-After: +1\r\nContent-Length: 0\r\n\r\n".to_owned(),
            "502" | "504" => format!("HTTP/1.1 {scenario} Proxy\r\nRetry-After: 99\r\nContent-Length: 0\r\n\r\n"),
            "duplicate" => "HTTP/1.1 429 Refused\r\nRetry-After: bad\r\nRetry-After: 1\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n".to_owned(),
            "metadata-max" => {
                let mut response = "HTTP/1.1 429 Refused\r\n".to_owned();
                for _ in 0..14 { response.push_str("Retry-After: "); response.push_str(&"x".repeat(260)); response.push_str("\r\n"); }
                response.push_str("Content-Length: 0\r\n\r\n"); assert!(response.len() <= 4096); response
            }
            seconds => format!("HTTP/1.1 429 Refused\r\nRetry-After: {seconds}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        };
        let response = if scenario == "metadata-max" { response } else {
            response.replacen("\r\n", &format!("\r\nX-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\n", id.0), 1)
        };
        socket.write_all(response.as_bytes()).await
    }.await;
    drop(socket);
    drop(active);
    match result {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(())
        }
        result => result,
    }
}
