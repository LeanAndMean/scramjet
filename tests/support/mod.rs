use super::RequestId;
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

struct State {
    started: Vec<RequestId>,
    settled: Vec<RequestId>,
    active: usize,
    peak: usize,
    fd_peak: usize,
}
struct Shared {
    state: Mutex<State>,
    changed: Notify,
    gate: Semaphore,
}
pub struct Fixture {
    pub address: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    driver: JoinHandle<io::Result<()>>,
}

pub fn fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|entries| entries.count())
        .unwrap_or(0)
}

impl Fixture {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                started: Vec::with_capacity(12),
                settled: Vec::with_capacity(12),
                active: 0,
                peak: 0,
                fd_peak: fd_count(),
            }),
            changed: Notify::new(),
            gate: Semaphore::new(0),
        });
        let (stop, mut stopped) = watch::channel(false);
        let owner = shared.clone();
        let driver = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    joined = handlers.join_next(), if !handlers.is_empty() => { joined.unwrap().unwrap()?; }
                    accepted = listener.accept(), if handlers.len() < 12 => {
                        let (socket, _) = accepted?;
                        let shared = owner.clone();
                        let mut stop = stopped.clone();
                        handlers.spawn(async move {
                            tokio::select! {
                                result = serve(socket, &shared) => result,
                                _ = stop.changed() => Ok(()),
                            }
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
    pub fn release(&self, count: usize) {
        self.shared.gate.add_permits(count);
    }
    pub fn started(&self) -> Vec<RequestId> {
        self.shared.state.lock().unwrap().started.clone()
    }
    pub fn peak(&self) -> usize {
        self.shared.state.lock().unwrap().peak
    }
    pub fn fd_peak(&self) -> usize {
        self.shared.state.lock().unwrap().fd_peak
    }
    pub async fn wait_started(&self, count: usize) {
        self.wait_for(|state| state.started.len() >= count).await;
    }
    pub async fn wait_settled(&self, id: RequestId) {
        self.wait_for(|state| state.settled.contains(&id)).await;
        let _ = super::Runtime::reconcile_usage_fixture(id, 1);
    }
    async fn wait_for(&self, predicate: impl Fn(&State) -> bool) {
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
        .expect("fixture observation timed out");
    }
    pub async fn shutdown(self) {
        self.stop.send_replace(true);
        self.driver.await.unwrap().unwrap();
        for id in &self.shared.state.lock().unwrap().settled {
            let _ = super::Runtime::reconcile_usage_fixture(*id, 1);
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
            return Err(io::Error::other("fixture request too large"));
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
    if !head.starts_with("POST /stream HTTP/1.1\r\n") {
        return Err(io::Error::other("unexpected fixture request"));
    }
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
        return Err(io::Error::other("fixture input too large"));
    }
    request.resize(end + len, 0);
    socket.read_exact(&mut request[end..]).await?;
    let scenario = std::str::from_utf8(&request[end..]).map_err(io::Error::other)?;
    {
        let mut state = shared.state.lock().unwrap();
        if state.started.len() == 12 {
            return Err(io::Error::other("fixture workload exceeds 12 requests"));
        }
        state.started.push(id);
        state.active += 1;
        state.peak = state.peak.max(state.active);
        state.fd_peak = state.fd_peak.max(fd_count());
    }
    let active = Active { shared, id };
    shared.changed.notify_one();
    let result = response(&mut socket, scenario, shared, id).await;
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
        other => other,
    }
}

async fn gate_or_close(socket: &mut TcpStream, shared: &Shared) -> io::Result<bool> {
    let mut byte = [0];
    tokio::select! {
        permit = shared.gate.acquire() => { permit.unwrap().forget(); Ok(true) }
        read = socket.read(&mut byte) => { read?; Ok(false) }
    }
}

async fn response(
    socket: &mut TcpStream,
    scenario: &str,
    shared: &Shared,
    id: RequestId,
) -> io::Result<()> {
    if scenario == "header" {
        gate_or_close(socket, shared).await?;
        return Ok(());
    }
    if scenario.starts_with("hold") && !gate_or_close(socket, shared).await? {
        return Ok(());
    }
    if scenario == "cooldown-one" {
        socket.write_all(b"HTTP/1.1 429 Refused\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(());
    }
    if scenario == "reject" {
        socket
            .write_all(
                b"HTTP/1.1 429 Refused\r\nContent-Length: 999999\r\nConnection: close\r\n\r\n",
            )
            .await?;
        return Ok(());
    }
    if scenario == "oversized-head" {
        socket.write_all(b"HTTP/1.1 200 OK\r\nX-Large: ").await?;
        socket.write_all(&[b'x'; 5000]).await?;
        return Ok(());
    }
    if scenario == "many-headers" {
        socket.write_all(b"HTTP/1.1 200 OK\r\n").await?;
        for _ in 0..17 {
            socket.write_all(b"X: y\r\n").await?;
        }
        socket.write_all(b"Content-Length: 0\r\n\r\n").await?;
        return Ok(());
    }
    if scenario.starts_with("head-") {
        let body = b"data: [DONE]\n\n";
        let mut head = b"HTTP/1.1 200 OK\r\n".to_vec();
        match scenario {
            "head-prefix" | "head-complete" | "head-fragmented" | "head-header-start" => {
                for _ in 0..17 {
                    head.extend_from_slice(b"X: y\r\n");
                }
                match scenario {
                    "head-header-start" => {
                        head.truncate(head.len() - 6);
                        head.push(b'X');
                    }
                    "head-prefix" => {}
                    _ => head.extend_from_slice(b"\r\n"),
                }
            }
            "head-size-prefix" => {
                head.extend_from_slice(b"X: ");
                head.resize(4096, b'x');
            }
            "head-valid-lf" => {
                head = b"HTTP/1.1 200 OK\n".to_vec();
                for _ in 0..15 {
                    head.extend_from_slice(b"X: y\n");
                }
                head.extend_from_slice(format!("Content-Length: {}\n\n", body.len()).as_bytes());
            }
            "head-valid-count" => {
                for _ in 0..15 {
                    head.extend_from_slice(b"X: y\r\n");
                }
                head.extend_from_slice(
                    format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes(),
                );
            }
            "head-valid-mixed" | "head-valid-mixed-fragmented" => {
                for _ in 0..15 {
                    head.extend_from_slice(b"X: y\r\n");
                }
                head.extend_from_slice(format!("Content-Length: {}\n\r\n", body.len()).as_bytes());
                assert_eq!(head.len(), 128);
            }
            "head-malformed-ending" | "head-malformed-ending-fragmented" => {
                for _ in 0..15 {
                    head.extend_from_slice(b"X: y\r\n");
                }
                head.extend_from_slice(format!("Content-Length: {}\r\n\rX", body.len()).as_bytes());
            }
            "head-valid-size" => {
                let ending = format!("\r\nContent-Length: {}\r\n\r\n", body.len());
                head.extend_from_slice(b"X: ");
                head.resize(4096 - ending.len(), b'x');
                head.extend_from_slice(ending.as_bytes());
            }
            name if name.starts_with("head-invalid-start-") => {
                for _ in 0..16 {
                    head.extend_from_slice(b"X: y\r\n");
                }
                let name = name.strip_suffix("-fragmented").unwrap_or(name);
                head.push(match name {
                    "head-invalid-start-nul" => 0,
                    "head-invalid-start-colon" => b':',
                    "head-invalid-start-space" => b' ',
                    _ => return Err(io::Error::other("unknown invalid header fixture")),
                });
                head.extend_from_slice(b"X: y\r\n\r\n");
            }
            "head-invalid" => head.extend_from_slice(b"Bad Header: x\r\n\r\n"),
            _ => return Err(io::Error::other("unknown head fixture")),
        }
        if scenario.ends_with("fragmented") {
            socket.set_nodelay(true)?;
            let width = if scenario == "head-fragmented" { 3 } else { 1 };
            for (index, fragment) in head.chunks(width).enumerate() {
                socket.write_all(fragment).await?;
                if index % 2 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        } else {
            socket.write_all(&head).await?;
        }
        if scenario.starts_with("head-valid") {
            socket.write_all(body).await?;
        } else {
            let mut byte = [0];
            if socket.read(&mut byte).await? != 0 {
                return Err(io::Error::other("unexpected data after fixture request"));
            }
        }
        return Ok(());
    }
    if scenario == "wire" {
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await?;
        for index in 0..25_000 {
            socket
                .write_all(&[b'1', b'\r', b'\n', b"data: x\n\n"[index % 9], b'\r', b'\n'])
                .await?;
        }
        return Ok(());
    }
    let body = match scenario {
        "invalid" => b"data: \xff\n\n".to_vec(),
        "field" => b"event: x\n\n".to_vec(),
        "incomplete" | "hold-fail" => b"data: partial\n\n".to_vec(),
        "after-marker" => b"data: [DONE]\n\nx".to_vec(),
        "oversized-record" => [b"data: ".as_slice(), &[b'x'; 1025], b"\n\n"].concat(),
        "slow" | "deadline" | "oversized-body" | "huge-chunk" | "fragmented-long"
        | "coalesced-head" | "full-output" => {
            let count = match scenario {
                "oversized-body" => 64,
                "full-output" => 8,
                _ => 40,
            };
            let mut bytes = Vec::with_capacity(65_536 + 8192);
            for _ in 0..count {
                bytes.extend_from_slice(b"data: ");
                bytes.extend_from_slice(&[b'x'; 1024]);
                bytes.extend_from_slice(b"\n\n");
            }
            bytes.extend_from_slice(b"data: [DONE]\n\n");
            bytes
        }
        "marker-stall" => b"data: [DONE]\n\n".to_vec(),
        "body" => b"data: waiting\n\n".to_vec(),
        _ => "data: hé🙂\r\n\r\ndata: two\n\ndata: [DONE]\n\n"
            .as_bytes()
            .to_vec(),
    };
    if matches!(
        scenario,
        "chunked-incomplete" | "trailer" | "huge-chunk" | "oversized-trailer" | "many-trailers"
    ) {
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await?;
        socket
            .write_all(format!("{:x}\r\n", body.len()).as_bytes())
            .await?;
        socket.write_all(&body).await?;
        socket.write_all(b"\r\n").await?;
        if scenario == "trailer" {
            socket.write_all(b"0\r\nX: y\r\n\r\n").await?;
        }
        if scenario == "oversized-trailer" {
            socket.write_all(b"0\r\nX: ").await?;
            socket.write_all(&[b'x'; 5000]).await?;
            socket.write_all(b"\r\n\r\n").await?;
        }
        if scenario == "many-trailers" {
            socket.write_all(b"0\r\n").await?;
            for _ in 0..17 {
                socket.write_all(b"X: y\r\n").await?;
            }
            socket.write_all(b"\r\n").await?;
        }
        if scenario == "huge-chunk" {
            socket.write_all(b"0\r\n\r\n").await?;
        }
        return Ok(());
    }
    let extra = if matches!(scenario, "truncated" | "body" | "marker-stall") {
        10
    } else {
        0
    };
    if scenario == "coalesced-head" {
        socket.write_all(b"HTTP/1.1 200 OK\r\nX-Padding: ").await?;
        socket.write_all(&[b'x'; 3800]).await?;
        socket
            .write_all(
                format!(
                    "\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await?;
    } else {
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nX-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    id.0,
                    body.len() + extra
                )
                .as_bytes(),
            )
            .await?;
    }
    if scenario == "fragmented-long" {
        for fragment in body.chunks(17) {
            socket.write_all(fragment).await?;
            tokio::task::yield_now().await;
        }
    } else if scenario == "fragmented" {
        for byte in &body {
            socket.write_all(&[*byte]).await?;
            tokio::task::yield_now().await;
        }
    } else {
        socket.write_all(&body).await?;
    }
    if matches!(scenario, "body" | "slow" | "deadline" | "marker-stall") {
        gate_or_close(socket, shared).await?;
    }
    Ok(())
}
