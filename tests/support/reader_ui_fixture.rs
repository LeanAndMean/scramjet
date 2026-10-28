use scramjet_rs::RequestId;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    pub child: u8,
    pub request_id: RequestId,
    pub generation: u64,
    pub joined: bool,
    pub cost: u32,
}
#[derive(Clone, Debug)]
pub struct Report {
    pub session: String,
    pub receipts: [Option<Receipt>; 3],
    pub accepts: usize,
    pub peak: usize,
    pub error: Option<&'static str>,
}
struct Shared {
    report: Mutex<Report>,
    active: Mutex<usize>,
    gates: [Semaphore; 3],
}
pub struct Fixture {
    pub address: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    driver: JoinHandle<()>,
}
impl Fixture {
    pub async fn new(session: &str, generation: u64) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            report: Mutex::new(Report {
                session: session.into(),
                receipts: [None; 3],
                accepts: 0,
                peak: 0,
                error: None,
            }),
            active: Mutex::new(0),
            gates: std::array::from_fn(|_| Semaphore::new(0)),
        });
        let (stop, mut stopped) = watch::channel(false);
        let owner = shared.clone();
        let driver = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let mut accepts = 0;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    joined = handlers.join_next(), if !handlers.is_empty() => {
                        if !matches!(joined, Some(Ok(()))) { owner.report.lock().unwrap().error = Some("fixture handler failed"); }
                    },
                    socket = listener.accept(), if accepts < 3 && handlers.len() < 2 => {
                        let Ok((socket, _)) = socket else { owner.report.lock().unwrap().error = Some("fixture accept failed"); break; };
                        accepts += 1;
                        owner.report.lock().unwrap().accepts = accepts;
                        let shared = owner.clone();
                        let mut stop = stopped.clone();
                        handlers.spawn(async move {
                            tokio::select! {
                                result = serve(socket, shared.clone(), generation) => {
                                    if let Err(error) = result { shared.report.lock().unwrap().error = Some(error); }
                                },
                                _ = stop.changed() => {},
                            }
                        });
                    }
                }
            }
            drop(listener);
            while let Some(joined) = handlers.join_next().await {
                if joined.is_err() {
                    owner.report.lock().unwrap().error = Some("fixture handler failed");
                }
            }
        });
        Ok(Self {
            address,
            shared,
            stop,
            driver,
        })
    }
    pub fn release(&self, child: usize) {
        self.shared.gates[child].add_permits(1);
    }
    pub fn captured(&self, child: usize) -> bool {
        self.shared.report.lock().unwrap().receipts[child].is_some()
    }
    pub async fn shutdown(self) -> Result<Report, &'static str> {
        self.stop.send_replace(true);
        self.driver.await.map_err(|_| "fixture supervisor failed")?;
        Ok(self.shared.report.lock().unwrap().clone())
    }
}
struct Active {
    shared: Arc<Shared>,
    child: usize,
}
impl Drop for Active {
    fn drop(&mut self) {
        *self.shared.active.lock().unwrap() -= 1;
        self.shared.report.lock().unwrap().receipts[self.child]
            .as_mut()
            .unwrap()
            .joined = true;
    }
}
async fn serve(
    mut socket: TcpStream,
    shared: Arc<Shared>,
    generation: u64,
) -> Result<(), &'static str> {
    let mut request = Vec::with_capacity(8192);
    loop {
        let mut byte = [0];
        if socket
            .read(&mut byte)
            .await
            .map_err(|_| "fixture read failed")?
            == 0
        {
            return Ok(());
        }
        if request.len() == 8192 {
            return Err("fixture capture ceiling");
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header = std::str::from_utf8(&request).map_err(|_| "fixture header UTF8")?;
    let value = |name: &str| {
        header.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
    };
    let len: usize = value("Content-Length")
        .and_then(|v| v.parse().ok())
        .ok_or("fixture content length")?;
    let id = RequestId(
        value("X-Fixture-Id")
            .and_then(|v| v.parse().ok())
            .ok_or("fixture request identity")?,
    );
    if len > 4096 || request.len() + len > 8192 {
        return Err("fixture input ceiling");
    }
    let start = request.len();
    request.resize(start + len, 0);
    if socket.read_exact(&mut request[start..]).await.is_err() {
        return Ok(());
    }
    let context: serde_json::Value =
        serde_json::from_slice(&request[start..]).map_err(|_| "fixture context JSON")?;
    let child = context["child"]
        .as_u64()
        .filter(|c| *c < 3)
        .ok_or("fixture child identity")? as usize;
    let session = shared.report.lock().unwrap().session.clone();
    if context["session"].as_str() != Some(&session)
        || context["generation"].as_u64() != Some(generation)
    {
        return Err("fixture foreign invocation");
    }
    {
        let mut active = shared.active.lock().unwrap();
        let mut report = shared.report.lock().unwrap();
        if report.receipts[child].is_some()
            || report.receipts.iter().flatten().any(|r| r.request_id == id)
        {
            return Err("fixture duplicate identity");
        }
        *active += 1;
        report.peak = report.peak.max(*active);
        report.receipts[child] = Some(Receipt {
            child: child as u8,
            request_id: id,
            generation,
            joined: false,
            cost: 1,
        });
    }
    let _active = Active {
        shared: shared.clone(),
        child,
    };
    let mut byte = [0];
    let released = tokio::select! {
        permit = shared.gates[child].acquire() => { permit.map_err(|_| "fixture gate closed")?.forget(); true },
        _ = socket.read(&mut byte) => false,
    };
    if released {
        let response = serde_json::json!({"session": session, "generation": generation, "child": child, "summary": format!("reader-{child}: immutable synthetic content summarized")}).to_string();
        let body = format!("data: {response}\n\ndata: [DONE]\n\n");
        let header = format!(
            "HTTP/1.1 200 fixture\r\nX-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            id.0,
            body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(body.as_bytes()).await;
        let _ = socket.shutdown().await;
    }
    Ok(())
}
