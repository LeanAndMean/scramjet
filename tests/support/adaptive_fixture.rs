use scramjet_rs::RequestId;
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Copy, Debug)]
pub struct Receipt {
    pub logical: usize,
    pub id: RequestId,
    pub status: u16,
    pub settled: bool,
}
#[derive(Clone, Debug)]
pub struct Metrics {
    pub receipts: Vec<Receipt>,
    pub handler_peak: usize,
    pub service_peak: usize,
    pub service_phase_peaks: [usize; 6],
    pub captured_input: usize,
    pub fd_peak: Option<usize>,
    pub rss_peak_kib: Option<usize>,
}
struct State {
    metrics: Metrics,
    handlers: usize,
    service: usize,
    capacity: usize,
    phase: usize,
}
struct Shared {
    state: Mutex<State>,
    changed: Notify,
}
pub struct Fixture {
    pub address: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    driver: JoinHandle<io::Result<()>>,
}
fn fd() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(|v| v.count())
}
fn rss() -> Option<usize> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|l| {
            l.strip_prefix("VmRSS:")
                .and_then(|s| s.split_whitespace().next()?.parse().ok())
        })
}
impl Fixture {
    pub async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                metrics: Metrics {
                    receipts: Vec::with_capacity(48),
                    handler_peak: 0,
                    service_peak: 0,
                    service_phase_peaks: [0; 6],
                    captured_input: 0,
                    fd_peak: fd(),
                    rss_peak_kib: rss(),
                },
                handlers: 0,
                service: 0,
                capacity: 4,
                phase: 0,
            }),
            changed: Notify::new(),
        });
        let (stop, mut stopped) = watch::channel(false);
        let owner = shared.clone();
        let driver = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            let mut accepts = 0;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    result = tasks.join_next(), if !tasks.is_empty() => { result.unwrap().unwrap()?; },
                    accepted = listener.accept(), if tasks.len() < 4 && accepts < 48 => {
                        accepts += 1;
                        let (socket, _) = accepted?;
                        let shared = owner.clone();
                        tasks.spawn(async move { serve(socket, shared).await });
                    }
                }
            }
            drop(listener);
            while let Some(result) = tasks.join_next().await {
                result.unwrap()?;
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
    pub fn phase(&self, phase: usize) {
        let mut s = self.shared.state.lock().unwrap();
        assert_eq!(s.handlers, 0);
        assert_eq!(s.service, 0);
        assert_eq!(s.metrics.receipts.len(), phase * 8);
        assert!(s.metrics.receipts.iter().all(|r| r.settled));
        s.phase = phase;
        s.capacity = if phase == 1 || phase == 2 { 1 } else { 4 };
    }
    pub async fn wait_batch(&self, count: usize) {
        loop {
            let changed = self.shared.changed.notified();
            {
                let s = self.shared.state.lock().unwrap();
                if s.metrics.receipts.len() == count
                    && s.handlers == 0
                    && s.metrics.receipts.iter().all(|r| r.settled)
                {
                    return;
                }
            }
            changed.await;
        }
    }
    pub fn metrics(&self) -> Metrics {
        self.shared.state.lock().unwrap().metrics.clone()
    }
    pub async fn shutdown(self) -> Metrics {
        self.stop.send_replace(true);
        self.driver.await.unwrap().unwrap();
        let s = self.shared.state.lock().unwrap();
        assert_eq!((s.handlers, s.service), (0, 0));
        s.metrics.clone()
    }
}
async fn serve(mut socket: TcpStream, shared: Arc<Shared>) -> io::Result<()> {
    let mut input = Vec::with_capacity(8192);
    let end = loop {
        if input.len() == 8192 {
            return Err(io::Error::other("request bound"));
        }
        let mut b = [0];
        socket.read_exact(&mut b).await?;
        input.push(b[0]);
        if input.ends_with(b"\r\n\r\n") {
            break input.len();
        }
    };
    let head = std::str::from_utf8(&input).map_err(io::Error::other)?;
    let header = |name: &str| {
        head.lines().find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim())
        })
    };
    let len: usize = header("Content-Length")
        .ok_or_else(|| io::Error::other("missing length"))?
        .parse()
        .map_err(io::Error::other)?;
    let id = RequestId(
        header("X-Fixture-Id")
            .ok_or_else(|| io::Error::other("missing id"))?
            .parse()
            .map_err(io::Error::other)?,
    );
    if len > 4096 || end + len > 8192 {
        return Err(io::Error::other("input bound"));
    }
    input.resize(end + len, 0);
    socket.read_exact(&mut input[end..]).await?;
    let logical: usize = std::str::from_utf8(&input[end..])
        .map_err(io::Error::other)?
        .parse()
        .map_err(io::Error::other)?;
    let (index, status, service) = {
        let mut s = shared.state.lock().unwrap();
        if s.metrics.receipts.len() == 48
            || logical / 8 != s.phase
            || s.metrics
                .receipts
                .iter()
                .any(|r| r.logical == logical || r.id == id)
        {
            return Err(io::Error::other("workload correlation"));
        }
        let status = if logical == 8 {
            429
        } else if logical == 9 {
            504
        } else if s.service >= s.capacity {
            502
        } else {
            200
        };
        let service = status == 200;
        s.handlers += 1;
        assert!(s.handlers <= 4);
        s.metrics.handler_peak = s.metrics.handler_peak.max(s.handlers);
        if service {
            s.service += 1;
        }
        s.metrics.service_peak = s.metrics.service_peak.max(s.service);
        let phase = s.phase;
        s.metrics.service_phase_peaks[phase] = s.metrics.service_phase_peaks[phase].max(s.service);
        s.metrics.captured_input += len;
        s.metrics.fd_peak = s.metrics.fd_peak.zip(fd()).map(|(a, b)| a.max(b));
        s.metrics.rss_peak_kib = s.metrics.rss_peak_kib.zip(rss()).map(|(a, b)| a.max(b));
        let index = s.metrics.receipts.len();
        s.metrics.receipts.push(Receipt {
            logical,
            id,
            status,
            settled: false,
        });
        (index, status, service)
    };
    if service {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let body = if status == 200 {
        "data: ok\n\ndata: [DONE]\n\n"
    } else {
        ""
    };
    let retry = if status == 429 {
        "Retry-After: 1\r\n"
    } else {
        ""
    };
    let response = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nX-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\n{retry}Connection: close\r\n\r\n{body}",
        body.len(),
        id.0
    );
    let result = socket.write_all(response.as_bytes()).await;
    drop(socket);
    {
        let mut s = shared.state.lock().unwrap();
        s.handlers -= 1;
        if service {
            s.service -= 1;
        }
        s.metrics.receipts[index].settled = true;
    }
    shared.changed.notify_one();
    result
}
