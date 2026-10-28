use crate::runtime::{MAX_BODY, MAX_RECORD, MAX_WIRE, Outcome, RequestId, Shared, Terminal};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, client::conn::http1};
use hyper_util::rt::TokioIo;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::{mpsc, watch},
};

pub(crate) mod provider;
pub(crate) mod provider_capture;
pub(crate) mod provider_diagnostic;
pub(crate) mod provider_metadata;
pub(crate) mod responses;
pub(crate) mod retry_after;
mod usage;

pub(crate) const RESPONSE_HEAD_BYTES: usize = 4096;
// A header needs at least a token byte, colon and CRLF within the byte budget.
pub(crate) const PROVIDER_HEADERS: usize = RESPONSE_HEAD_BYTES / 4;

struct Meter {
    wire: AtomicUsize,
    limited: AtomicBool,
    malformed: AtomicBool,
    invalid_usage: AtomicBool,
}
struct LimitedSocket {
    socket: TcpStream,
    meter: Arc<Meter>,
    head_bytes: usize,
    max_headers: usize,
    head_lines: usize,
    head_line_empty: bool,
    head_cr: bool,
    in_head: bool,
    line: [u8; 128],
    line_len: usize,
}

impl AsyncRead for LimitedSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let remaining = MAX_WIRE - self.meter.wire.load(Ordering::Relaxed);
        if remaining == 0 {
            self.meter.limited.store(true, Ordering::Relaxed);
            return Poll::Ready(Err(io::Error::other("inbound wire allowance exhausted")));
        }
        let cap = remaining.min(buf.remaining()).min(8192);
        let mut small = ReadBuf::new(buf.initialize_unfilled_to(cap));
        match Pin::new(&mut self.socket).poll_read(cx, &mut small) {
            Poll::Ready(Ok(())) => {
                let len = small.filled().len();
                self.meter.wire.fetch_add(len, Ordering::Relaxed);
                for byte in small.filled() {
                    if self.in_head {
                        self.head_bytes += 1;
                        if *byte == b'\n' {
                            let line = &self.line[..self.line_len.min(self.line.len())];
                            if self.head_lines == 0 && self.head_line_empty {
                                self.meter.malformed.store(true, Ordering::Relaxed);
                                return Poll::Ready(Err(io::Error::other(
                                    "blank line before response status",
                                )));
                            }
                            if self.head_lines == 0
                                && (line.starts_with(b"HTTP/1.1 1")
                                    || line.starts_with(b"HTTP/1.0 1"))
                            {
                                self.meter.malformed.store(true, Ordering::Relaxed);
                                return Poll::Ready(Err(io::Error::other(
                                    "unsupported informational response",
                                )));
                            }
                            if let Some(colon) = line.iter().position(|b| *b == b':') {
                                let name = &line[..colon];
                                if name.eq_ignore_ascii_case(b"x-fixture-usage-id")
                                    || name.eq_ignore_ascii_case(b"x-fixture-usage-tokens")
                                {
                                    let value = &line[colon + 1..];
                                    let value = value.strip_prefix(b" ").unwrap_or(value);
                                    let value = value.strip_suffix(b"\r").unwrap_or(value);
                                    if self.line_len > self.line.len()
                                        || value.is_empty()
                                        || !value.iter().all(u8::is_ascii_digit)
                                    {
                                        self.meter.invalid_usage.store(true, Ordering::Relaxed);
                                    }
                                }
                            }
                            self.line_len = 0;
                        } else {
                            let index = self.line_len;
                            if index < self.line.len() {
                                self.line[index] = *byte;
                            }
                            self.line_len += 1;
                        }
                        if self.head_cr && *byte != b'\n' {
                            self.meter.malformed.store(true, Ordering::Relaxed);
                            return Poll::Ready(Err(io::Error::other(
                                "malformed response newline",
                            )));
                        }
                        let header_start = self.head_lines != 0
                            && self.head_line_empty
                            && !matches!(*byte, b'\r' | b'\n');
                        if header_start
                            && !matches!(*byte, b'!' | b'#'..=b'\'' | b'*' | b'+' | b'-' | b'.'
                                | b'0'..=b'9' | b'A'..=b'Z' | b'^'..=b'z' | b'|' | b'~')
                        {
                            self.meter.malformed.store(true, Ordering::Relaxed);
                            return Poll::Ready(Err(io::Error::other("malformed response header")));
                        }
                        let excess_header = self.head_lines == self.max_headers + 1 && header_start;
                        match *byte {
                            b'\r' => self.head_cr = true,
                            b'\n' => {
                                if self.head_line_empty {
                                    self.in_head = false;
                                }
                                self.head_lines += 1;
                                self.head_line_empty = true;
                                self.head_cr = false;
                            }
                            _ => self.head_line_empty = false,
                        }
                        if excess_header
                            || self.head_bytes > 4096
                            || (self.in_head
                                && (self.head_bytes == 4096
                                    || self.head_lines > self.max_headers + 1))
                        {
                            self.meter.limited.store(true, Ordering::Relaxed);
                            return Poll::Ready(Err(io::Error::other(
                                "response head allowance exhausted",
                            )));
                        }
                    }
                }
                buf.advance(len);
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}
impl AsyncWrite for LimitedSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.socket).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}

struct Parser {
    record: Vec<u8>,
    done: bool,
}
impl Parser {
    fn new() -> Self {
        Self {
            record: Vec::with_capacity(1036),
            done: false,
        }
    }
    fn byte(&mut self, byte: u8) -> Result<Option<String>, Outcome> {
        if self.done {
            return Err(Outcome::Framing);
        }
        if self.record.len() == 1036 {
            return Err(Outcome::ResourceLimit);
        }
        self.record.push(byte);
        let ending = if self.record.ends_with(b"\r\n\r\n") {
            4
        } else if self.record.ends_with(b"\n\n") {
            2
        } else {
            return Ok(None);
        };
        let record = &self.record[..self.record.len() - ending];
        let Some(text) = record.strip_prefix(b"data: ") else {
            return Err(Outcome::Framing);
        };
        if text.contains(&b'\n') || text.contains(&b'\r') {
            return Err(Outcome::Framing);
        }
        if text.len() > MAX_RECORD {
            return Err(Outcome::ResourceLimit);
        }
        let text = std::str::from_utf8(text).map_err(|_| Outcome::Framing)?;
        if text == "[DONE]" {
            self.done = true;
            self.record.clear();
            return Ok(None);
        }
        let delta = text.to_owned();
        self.record.clear();
        Ok(Some(delta))
    }
    fn complete(&self) -> Result<(), Outcome> {
        if self.done && self.record.is_empty() {
            Ok(())
        } else {
            Err(Outcome::Incomplete)
        }
    }
}

pub(crate) async fn attempt(
    endpoint: SocketAddr,
    id: RequestId,
    input: String,
    output: mpsc::Sender<String>,
    mut cancel: watch::Receiver<Option<Outcome>>,
    shared: &Shared,
    deadline: tokio::time::Instant,
) -> (Terminal, bool) {
    let meter = Arc::new(Meter {
        wire: AtomicUsize::new(0),
        limited: AtomicBool::new(false),
        malformed: AtomicBool::new(false),
        invalid_usage: AtomicBool::new(false),
    });
    let mut connection = None;
    let mut body_bytes: usize = 0;
    let mut text_bytes = 0;
    let mut observed = false;
    let mut retry_after = None;
    let mut dispatch_to_head = None;
    let mut output_backpressure = false;
    let mut synthetic_usage = crate::SyntheticUsage::Unknown;
    let work = async {
        let connecting = TcpStream::connect(endpoint);
        tokio::pin!(connecting);
        #[cfg(test)]
        let socket = {
            let first = std::future::poll_fn(|cx| {
                Poll::Ready(std::future::Future::poll(connecting.as_mut(), cx))
            })
            .await;
            match first {
                Poll::Ready(result) => result,
                Poll::Pending => {
                    shared
                        .checkpoint(crate::runtime::Checkpoint::ConnectPending)
                        .await;
                    connecting.await
                }
            }
        }
        .map_err(|_| Outcome::Transport)?;
        #[cfg(not(test))]
        let socket = connecting.await.map_err(|_| Outcome::Transport)?;
        let io = TokioIo::new(LimitedSocket {
            socket,
            meter: meter.clone(),
            head_bytes: 0,
            max_headers: 16,
            head_lines: 0,
            head_line_empty: true,
            head_cr: false,
            in_head: true,
            line: [0; 128],
            line_len: 0,
        });
        let (mut sender, driver) = http1::Builder::new()
            .max_headers(16)
            .max_header_size(4096)
            .max_buf_size(8192)
            .handshake(io)
            .await
            .map_err(|_| Outcome::Transport)?;
        connection = Some(tokio::spawn(driver));
        let request = Request::builder()
            .method("POST")
            .uri("/stream")
            .header("Host", endpoint.to_string())
            .header("Connection", "close")
            .header("Content-Length", input.len())
            .header("X-Fixture-Id", id.0.to_string())
            .body(Full::new(Bytes::from(input)))
            .expect("fixed request metadata");
        if tokio::time::Instant::now() >= deadline {
            return Err(Outcome::Deadline);
        }
        shared.dispatched(id);
        let dispatched = tokio::time::Instant::now();
        let response = sender.send_request(request).await.map_err(|error| {
            classify(
                &error,
                &meter,
                if error.is_parse() {
                    Outcome::Framing
                } else {
                    Outcome::Transport
                },
            )
        })?;
        let head_time = tokio::time::Instant::now();
        dispatch_to_head = Some(head_time.saturating_duration_since(dispatched));
        if head_time >= deadline {
            return Err(Outcome::Deadline);
        }
        #[cfg(test)]
        shared
            .checkpoint(crate::runtime::Checkpoint::ResponseHeaders)
            .await;
        let observed_head = tokio::time::Instant::now();
        if response.status() == 429 {
            retry_after = Some(retry_after::parse(
                response
                    .headers()
                    .get_all("retry-after")
                    .iter()
                    .map(|v| v.as_bytes()),
                std::time::SystemTime::now(),
            ));
        }
        shared.head(id, response.status().as_u16(), retry_after, observed_head);
        let usage = usage::parse(
            response
                .headers()
                .get_all("x-fixture-usage-id")
                .iter()
                .map(|v| v.as_bytes()),
            response
                .headers()
                .get_all("x-fixture-usage-tokens")
                .iter()
                .map(|v| v.as_bytes()),
            id,
        )
        .and_then(|usage| {
            if meter.invalid_usage.load(Ordering::Relaxed) {
                Err(())
            } else {
                Ok(usage)
            }
        });
        if response.status() != 200 {
            if let Ok(Some(actual)) = usage {
                synthetic_usage = crate::SyntheticUsage::Known(actual);
            }
            observed = true;
            return Ok(Outcome::HttpRejected(response.status().as_u16()));
        }
        let usage = usage.map_err(|_| Outcome::Framing)?;
        let mut body = response.into_body();
        let mut parser = Parser::new();
        while let Some(frame) = body.frame().await {
            if tokio::time::Instant::now() >= deadline {
                return Err(Outcome::Deadline);
            }
            let frame = frame.map_err(|error| classify(&error, &meter, Outcome::Incomplete))?;
            if let Some(trailers) = frame.trailers_ref() {
                let bytes: usize = trailers
                    .iter()
                    .map(|(name, value)| name.as_str().len() + value.len() + 4)
                    .sum();
                if trailers.len() > 16 || bytes > 4096 {
                    return Err(Outcome::ResourceLimit);
                }
                return Err(Outcome::Framing);
            }
            if let Ok(data) = frame.into_data() {
                body_bytes = body_bytes
                    .checked_add(data.len())
                    .ok_or(Outcome::ResourceLimit)?;
                if body_bytes > MAX_BODY {
                    return Err(Outcome::ResourceLimit);
                }
                for byte in data.iter() {
                    if let Some(text) = parser.byte(*byte)? {
                        let permit = match output.try_reserve() {
                            Ok(permit) => permit,
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                return Err(Outcome::ConsumerLost);
                            }
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                output_backpressure = true;
                                #[cfg(test)]
                                shared
                                    .checkpoint(crate::runtime::Checkpoint::OutputBackpressure)
                                    .await;
                                output.reserve().await.map_err(|_| Outcome::ConsumerLost)?
                            }
                        };
                        if tokio::time::Instant::now() >= deadline {
                            return Err(Outcome::Deadline);
                        }
                        text_bytes += text.len();
                        shared.emit(id, permit, text)?;
                    }
                }
            }
        }
        #[cfg(test)]
        shared
            .checkpoint(crate::runtime::Checkpoint::BodyComplete)
            .await;
        parser.complete()?;
        if tokio::time::Instant::now() >= deadline {
            return Err(Outcome::Deadline);
        }
        if let Some(actual) = usage {
            synthetic_usage = crate::SyntheticUsage::Known(actual);
        }
        observed = true;
        Ok(Outcome::Success)
    };
    let outcome = {
        tokio::pin!(work);
        tokio::select! {
            biased;
            reason = cancelled(&mut cancel) => reason,
            _ = tokio::time::sleep_until(deadline) => Outcome::Deadline,
            result = &mut work => result.unwrap_or_else(|reason| reason),
        }
    };
    #[cfg(test)]
    shared.checkpoint(crate::runtime::Checkpoint::Settled).await;
    if let Some(driver) = connection {
        driver.abort();
        let _ = driver.await;
    }
    (
        Terminal {
            id,
            outcome,
            body_bytes,
            wire_bytes: meter.wire.load(Ordering::Relaxed),
            text_bytes,
            retry_after,
            synthetic_usage,
            dispatch_to_head,
            commitment_to_terminal: None,
            output_backpressure,
        },
        observed,
    )
}

fn classify(error: &hyper::Error, meter: &Meter, fallback: Outcome) -> Outcome {
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<io::Error>() {
            // These decoder errors are verified against the pinned Hyper version and fixtures.
            if matches!(
                io.to_string().as_str(),
                "chunk trailers bytes over limit"
                    | "chunk trailers count overflow"
                    | "chunk extensions over limit"
            ) {
                return Outcome::ResourceLimit;
            }
        }
        source = cause.source();
    }
    if meter.limited.load(Ordering::Relaxed) {
        Outcome::ResourceLimit
    } else if meter.malformed.load(Ordering::Relaxed) {
        Outcome::Framing
    } else {
        fallback
    }
}

async fn cancelled(cancel: &mut watch::Receiver<Option<Outcome>>) -> Outcome {
    loop {
        if let Some(reason) = cancel.borrow().clone() {
            return reason;
        }
        if cancel.changed().await.is_err() {
            return Outcome::Cancelled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_accepts_fragmented_unicode_and_both_delimiters() {
        let mut parser = Parser::new();
        let output: Vec<_> = "data: hé🙂\r\n\r\ndata: two\n\ndata: [DONE]\n\n"
            .bytes()
            .filter_map(|b| parser.byte(b).unwrap())
            .collect();
        assert_eq!(output, ["hé🙂", "two"]);
        assert_eq!(parser.complete(), Ok(()));
        assert_eq!(parser.byte(b'x'), Err(Outcome::Framing));
    }
    #[test]
    fn parser_rejects_invalid_records_and_incomplete_marker() {
        for bytes in [
            b"event: x\n\n".as_slice(),
            b"data: x\ndata: y\n\n",
            b"data: \xff\n\n",
        ] {
            let mut parser = Parser::new();
            assert!(bytes.iter().any(|b| parser.byte(*b).is_err()));
        }
        assert_eq!(Parser::new().complete(), Err(Outcome::Incomplete));
        let mut parser = Parser::new();
        for _ in 0..1036 {
            parser.byte(b'x').unwrap();
        }
        assert_eq!(parser.byte(b'x'), Err(Outcome::ResourceLimit));
    }
}
