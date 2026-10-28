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
pub(crate) struct Receipt {
    pub id: RequestId,
    pub ordinal: usize,
    pub joined: bool,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Report {
    pub receipts: [Option<Receipt>; 4],
    pub accepts: usize,
    pub active: usize,
    pub peak: usize,
    pub error: Option<String>,
}
struct Shared {
    report: Mutex<Report>,
    changed: Notify,
}
pub(crate) struct Fixture {
    pub address: SocketAddr,
    #[cfg(test)]
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    release: watch::Sender<bool>,
    driver: JoinHandle<Report>,
}
impl Fixture {
    pub async fn new(paced: bool, released: bool) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let shared = Arc::new(Shared {
            report: Mutex::new(Report::default()),
            changed: Notify::new(),
        });
        let owner = shared.clone();
        let (stop, mut stopped) = watch::channel(false);
        let (release, released) = watch::channel(released);
        let driver = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            let mut accepts = 0;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    joined = handlers.join_next(), if !handlers.is_empty() => {
                        if let Some(Err(error)) = joined { owner.report.lock().unwrap().error = Some(format!("fixture handler: {error}")); }
                    }
                    accepted = listener.accept(), if accepts < 4 && handlers.len() < 2 => {
                        match accepted {
                            Ok((socket, _)) => {
                                accepts += 1;
                                owner.report.lock().unwrap().accepts = accepts;
                                let shared = owner.clone();
                                let mut stop = stopped.clone();
                                let release = released.clone();
                                handlers.spawn(async move {
                                    let mut socket = socket;
                                    let id = tokio::select! {
                                        result = capture(&mut socket, &shared) => result,
                                        _ = stop.changed() => Ok(None),
                                    };
                                    match id {
                                        Ok(Some((id, ordinal))) => {
                                            let result = tokio::select! {
                                                result = response(&mut socket, id, ordinal, paced, release) => result,
                                                _ = stop.changed() => Ok(()),
                                            };
                                            drop(socket);
                                            let mut report = shared.report.lock().unwrap();
                                            report.active -= 1;
                                            report.receipts[ordinal].as_mut().unwrap().joined = true;
                                            if let Err(error) = result && !matches!(error.kind(), io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof) {
                                                report.error = Some(format!("fixture response: {error}"));
                                            }
                                            drop(report);
                                            shared.changed.notify_waiters();
                                        }
                                        Ok(None) => {},
                                        Err(error) => shared.report.lock().unwrap().error = Some(format!("fixture capture: {error}")),
                                    }
                                });
                            }
                            Err(error) => { owner.report.lock().unwrap().error = Some(format!("fixture accept: {error}")); handlers.abort_all(); break; }
                        }
                    }
                }
            }
            drop(listener);
            while let Some(joined) = handlers.join_next().await {
                if let Err(error) = joined {
                    owner.report.lock().unwrap().error = Some(format!("fixture handler: {error}"));
                }
            }
            owner.report.lock().unwrap().clone()
        });
        Ok(Self {
            address,
            #[cfg(test)]
            shared,
            stop,
            release,
            driver,
        })
    }
    #[cfg(test)]
    pub fn snapshot(&self) -> Report {
        self.shared.report.lock().unwrap().clone()
    }
    #[cfg(test)]
    pub fn release(&self) {
        self.release.send_replace(true);
    }
    #[cfg(test)]
    pub async fn wait_received(&self, ordinal: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let changed = self.shared.changed.notified();
                if self.snapshot().receipts[ordinal].is_some() {
                    return;
                }
                changed.await;
            }
        })
        .await
        .expect("fixture receipt");
    }
    pub async fn shutdown(self) -> Result<Report, String> {
        self.stop.send_replace(true);
        drop(self.release);
        self.driver
            .await
            .map_err(|error| format!("fixture supervisor: {error}"))
    }
}

async fn capture(
    socket: &mut TcpStream,
    shared: &Shared,
) -> io::Result<Option<(RequestId, usize)>> {
    let mut bytes = Vec::with_capacity(8192);
    loop {
        if bytes.len() == 8192 {
            return Err(io::Error::other("fixture head limit"));
        }
        let mut byte = [0];
        if socket.read(&mut byte).await? == 0 {
            return Ok(None);
        }
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = std::str::from_utf8(&bytes).map_err(io::Error::other)?;
    if !head.starts_with("POST /stream HTTP/1.1\r\n") {
        return Err(io::Error::other("fixture route"));
    }
    let header = |name: &str| {
        head.lines().find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
    };
    let len = header("Content-Length")
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| io::Error::other("fixture length"))?;
    let id = RequestId(
        header("X-Fixture-Id")
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| io::Error::other("fixture identity"))?,
    );
    let end = bytes.len();
    if len > 4096 || end + len > 8192 {
        return Err(io::Error::other("fixture input limit"));
    }
    bytes.resize(end + len, 0);
    socket.read_exact(&mut bytes[end..]).await?;
    let ordinal = match &bytes[end..] {
        b"unicode" => 0,
        b"incomplete" => 1,
        b"dense" => 2,
        b"stalled" => 3,
        _ => return Err(io::Error::other("fixture scenario")),
    };
    let mut report = shared.report.lock().unwrap();
    if report.receipts[ordinal].is_some()
        || report
            .receipts
            .iter()
            .flatten()
            .any(|receipt| receipt.id == id)
    {
        return Err(io::Error::other("duplicate fixture receipt"));
    }
    report.receipts[ordinal] = Some(Receipt {
        id,
        ordinal,
        joined: false,
    });
    report.active += 1;
    report.peak = report.peak.max(report.active);
    drop(report);
    shared.changed.notify_waiters();
    Ok(Some((id, ordinal)))
}
async fn response(
    socket: &mut TcpStream,
    id: RequestId,
    ordinal: usize,
    paced: bool,
    mut release: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut byte = [0];
    while !*release.borrow_and_update() {
        tokio::select! {
            changed = release.changed() => { if changed.is_err() { return Ok(()); } },
            read = socket.read(&mut byte) => { read?; return Ok(()); },
        }
    }
    let records: Vec<String> = match ordinal {
        0 => [
            "e",
            "\u{301}",
            "界",
            "👩",
            "\u{200d}",
            "💻",
            " ESC:",
            "\x1b]52;c;external\x07",
            " C1:\u{85}",
            " bidi:\u{202e}",
            " end",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        1 => vec!["partial".into()],
        2 => vec!["x".repeat(1024); 40],
        _ => vec!["waiting".into()],
    };
    let body_len: usize = records.iter().map(|record| record.len() + 8).sum::<usize>()
        + if matches!(ordinal, 0 | 2) {
            14
        } else if ordinal == 3 {
            10
        } else {
            0
        };
    socket.write_all(format!("HTTP/1.1 200 OK\r\nX-Fixture-Usage-Id: {}\r\nX-Fixture-Usage-Tokens: 1\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n", id.0).as_bytes()).await?;
    for record in records {
        // The protocol record boundary is independent of both UTF-8 and display boundaries.
        let wire = format!("data: {record}\n\n");
        for fragment in wire.as_bytes().chunks(3) {
            socket.write_all(fragment).await?;
        }
        if paced && ordinal < 2 {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tokio::task::yield_now().await;
    }
    if matches!(ordinal, 0 | 2) {
        socket.write_all(b"data: [DONE]\n\n").await?;
    }
    if ordinal == 3 {
        let _closed = socket.read(&mut [0]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn fixture_gate_has_explicit_receipt_and_joined_scalar_attestation() {
        let fixture = Fixture::new(false, false).await.unwrap();
        let mut socket = TcpStream::connect(fixture.address).await.unwrap();
        socket
            .write_all(
                b"POST /stream HTTP/1.1\r\nX-Fixture-Id: 999\r\nContent-Length: 7\r\n\r\nunicode",
            )
            .await
            .unwrap();
        fixture.wait_received(0).await;
        assert!(!fixture.snapshot().receipts[0].unwrap().joined);
        fixture.release();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        assert!(response.ends_with(b"data: [DONE]\n\n"));
        drop(socket);
        let report = fixture.shutdown().await.unwrap();
        assert!(report.error.is_none());
        assert_eq!(report.active, 0);
        assert_eq!(report.peak, 1);
        assert_eq!(report.receipts[0].unwrap().id, RequestId(999));
        assert!(report.receipts[0].unwrap().joined);
    }
}
