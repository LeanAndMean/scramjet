use scramjet_rs::{
    CodingError, CodingInvocation, CodingInvocationIntent, CodingInvocationOptions,
    EditCancellation,
};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::File,
    io::Read,
    net::SocketAddr,
    os::{
        fd::BorrowedFd,
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const INPUT_BYTES: usize = 16_384;
const RECORD_BYTES: usize = 131_072;
fn safe_json(bytes: &[u8]) -> Result<String, CodingError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| CodingError::Invalid("non-UTF-8 proposal"))?;
    let mut out = String::new();
    for c in text.chars() {
        let n = c as u32;
        if (32..127).contains(&n) {
            out.push(c);
        } else if n <= 0xffff {
            out.push_str(&format!("\\u{n:04x}"));
        } else {
            let n = n - 0x10000;
            out.push_str(&format!(
                "\\u{:04x}\\u{:04x}",
                0xd800 + (n >> 10),
                0xdc00 + (n & 1023)
            ));
        }
    }
    Ok(out)
}
fn acquire(path: &Path, limit: usize) -> Result<Vec<u8>, CodingError> {
    if path == Path::new("-") {
        return stdin_line(
            limit,
            &EditCancellation::default(),
            Instant::now() + Duration::from_secs(300),
        );
    }
    let file = File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(CodingError::Unsupported(
            "prompt/record input must be a regular file or stdin",
        ));
    }
    if before.len() > limit as u64 {
        return Err(CodingError::ResourceLimit(
            "prompt/record input resource limit",
        ));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize + 1);
    (&file).take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(CodingError::ResourceLimit(
            "prompt/record input resource limit",
        ));
    }
    let after = file.metadata()?;
    if (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        return Err(CodingError::Stale);
    }
    Ok(bytes)
}
fn stdin_line(
    limit: usize,
    cancel: &EditCancellation,
    deadline: Instant,
) -> Result<Vec<u8>, CodingError> {
    let fd = unsafe { BorrowedFd::borrow_raw(0) };
    let mut bytes = Vec::new();
    let mut byte = [0];
    loop {
        if cancel.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(CodingError::Deadline);
        }
        let mut fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        match rustix::event::poll(
            &mut fds,
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 10_000_000,
            }),
        ) {
            Ok(0) => continue,
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
            _ => {}
        }
        match rustix::io::read(fd, &mut byte) {
            Ok(0) => return Ok(bytes),
            Ok(_) => {
                if bytes.len() == limit {
                    return Err(CodingError::ResourceLimit(
                        "stdin decision/input resource limit",
                    ));
                }
                bytes.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(bytes);
                }
            }
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(e.into()),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    approve: bool,
    intent: CodingInvocationIntent,
}
fn present(
    fd: impl std::os::fd::AsFd,
    displayed: &str,
    cancel: &EditCancellation,
    deadline: Instant,
) -> Result<(), CodingError> {
    use rustix::{
        event::{PollFd, PollFlags, Timespec, poll},
        fs::{FileType, OFlags, fcntl_getfl, fcntl_setfl, fstat},
        io::Errno,
    };
    let kind = FileType::from_raw_mode(fstat(&fd)?.st_mode);
    if !matches!(kind, FileType::Fifo | FileType::Socket) && !rustix::termios::isatty(&fd) {
        return Err(CodingError::Unsupported(
            "approval stderr requires a pipe, socket or terminal",
        ));
    }
    let mut output = File::from(fd.as_fd().try_clone_to_owned()?);
    let flags = fcntl_getfl(&fd)?;
    fcntl_setfl(&fd, flags | OFlags::NONBLOCK)?;
    let delivered =
        (|| {
            for bytes in [
            b"proposal ".as_slice(),
            displayed.as_bytes(),
            b"\nReply with one JSON line containing approve and this exact intent; EOF denies.\n",
        ] {
            let mut remaining = bytes;
            while !remaining.is_empty() {
                if cancel.is_cancelled() {
                    return Err(CodingError::Cancelled);
                }
                if Instant::now() >= deadline {
                    return Err(CodingError::Deadline);
                }
                match std::io::Write::write(&mut output, &remaining[..remaining.len().min(4096)]) {
                    Ok(0) => {
                        return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
                    }
                    Ok(count) => remaining = &remaining[count..],
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        let mut descriptors = [PollFd::new(&fd, PollFlags::OUT)];
                        match poll(
                            &mut descriptors,
                            Some(&Timespec { tv_sec: 0, tv_nsec: 10_000_000 }),
                        ) {
                            Ok(_) | Err(Errno::INTR) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
            if cancel.is_cancelled() {
                return Err(CodingError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(CodingError::Deadline);
            }
            Ok(())
        })();
    let restored = fcntl_setfl(&fd, flags).map_err(CodingError::from);
    delivered.and(restored)
}
fn decide(
    intent: &CodingInvocationIntent,
    automatic: bool,
    cancel: &EditCancellation,
    deadline: Instant,
) -> Result<bool, CodingError> {
    let bytes = serde_json::to_vec(intent)
        .map_err(|_| CodingError::Invalid("proposal serialization failed"))?;
    let displayed = safe_json(&bytes)?;
    present(std::io::stderr(), &displayed, cancel, deadline)?;
    if automatic {
        return Ok(true);
    }
    let bytes = stdin_line(RECORD_BYTES, cancel, deadline)?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(false);
    }
    let decision: Decision = serde_json::from_slice(&bytes).map_err(|_| CodingError::Denied)?;
    if decision.intent != *intent {
        return Err(CodingError::Stale);
    }
    Ok(decision.approve)
}
pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let args = arguments.take(25).collect::<Vec<_>>();
    if args.len() > 24 || args.iter().map(|s| s.as_bytes().len()).sum::<usize>() > INPUT_BYTES {
        return Err("CLI argument resource limit".into());
    }
    let (mut project, mut storage, mut run, mut endpoint, mut prompt, mut previous) =
        (None, None, None, None, None, None);
    let mut files = Vec::new();
    let mut automatic = false;
    let mut args = args.iter();
    while let Some(option) = args.next() {
        match option.to_str(){
            Some("--approve-effects")if !automatic=>automatic=true,
            Some("--project")if project.is_none()=>project=Some(PathBuf::from(args.next().ok_or("missing project")?)),
            Some("--storage")if storage.is_none()=>storage=Some(PathBuf::from(args.next().ok_or("missing storage")?)),
            Some("--run")if run.is_none()=>run=Some(args.next().and_then(|s|s.to_str()).ok_or("missing UTF-8 run ID")?.to_owned()),
            Some("--endpoint")if endpoint.is_none()=>endpoint=Some(args.next().and_then(|s|s.to_str()).ok_or("missing endpoint")?.parse::<SocketAddr>().map_err(|_|"invalid loopback endpoint")?),
            Some("--file")if files.len()<2=>files.push(args.next().and_then(|s|s.to_str()).ok_or("missing UTF-8 basename")?.to_owned()),
            Some("--prompt-file")if prompt.is_none()=>prompt=Some(PathBuf::from(args.next().ok_or("missing prompt input")?)),
            Some("--record-file")if previous.is_none()=>previous=Some(PathBuf::from(args.next().ok_or("missing record input")?)),
            _=>return Err("requires --project PATH --storage PATH --run ID --file NAME --file NAME --prompt-file PATH [- for one stdin line] [--endpoint ADDR] [--record-file PATH] [--approve-effects]".into()),
        }
    }
    let endpoint = endpoint.unwrap_or(SocketAddr::from(([127, 0, 0, 1], 18080)));
    if !endpoint.ip().is_loopback() {
        return Err("only a loopback Responses endpoint (such as Proxy) is supported".into());
    }
    let prompt =
        acquire(&prompt.ok_or("missing prompt input")?, INPUT_BYTES).map_err(|e| e.to_string())?;
    let prompt = String::from_utf8(prompt).map_err(|_| "prompt must be UTF-8")?;
    let previous = previous
        .map(|p| acquire(&p, RECORD_BYTES))
        .transpose()
        .map_err(|e| e.to_string())?;
    let files: [String; 2] = files
        .try_into()
        .map_err(|_| "exactly two selected file basenames required")?;
    let options = CodingInvocationOptions {
        project: project.ok_or("missing project")?,
        storage: storage.ok_or("missing storage")?,
        run: run.ok_or("missing run ID")?,
        files,
        prompt,
        previous,
        endpoint,
    };
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "executor unavailable")?;
    executor.block_on(async move {
        let (invocation, runtime) = tokio::task::spawn_blocking(move || {
            CodingInvocation::start(options, move |intent, cancel, deadline| {
                decide(intent, automatic, cancel, deadline)
            })
        })
        .await
        .map_err(|_| "invocation owner failed")?
        .map_err(|e| e.to_string())?;
        let cancel = invocation.cancellation();
        let int_cancel = cancel.clone();
        let delivery_cancel = EditCancellation::default();
        let int_delivery_cancel = delivery_cancel.clone();
        let term_delivery_cancel = delivery_cancel.clone();
        let term_cancel = cancel;
        let int = unsafe {
            signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
                int_delivery_cancel.cancel();
                int_cancel.cancel()
            })
        }
        .map_err(|_| "signal owner unavailable")?;
        let term = unsafe {
            signal_hook::low_level::register(signal_hook::consts::SIGTERM, move || {
                term_delivery_cancel.cancel();
                term_cancel.cancel()
            })
        }
        .map_err(|_| "signal owner unavailable")?;
        let reported = match invocation.run(runtime).await {
            Ok(result) => tokio::task::spawn_blocking(move || {
                let stdout = std::io::stdout();
                let failure = result.failure.clone();
                result
                    .deliver_fd_with_cancellation(&stdout, &delivery_cancel)
                    .map_err(|error| {
                        failure.unwrap_or_else(|| {
                            format!("result publication failed; no retry: {error}")
                        })
                    })
            })
            .await
            .map_err(|_| "stdout owner failed; no retry".to_owned())
            .and_then(|result| result),
            Err(error) => Err(error.to_string()),
        };
        signal_hook::low_level::unregister(int);
        signal_hook::low_level::unregister(term);
        reported
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_proposal_display_is_terminal_safe_and_json_reversible() {
        let value = serde_json::json!({"text":"hé🙂\u{009b}\u{202e}\u{007f}\n\u{001b}"});
        let display = safe_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(display.bytes().all(|b| (32..127).contains(&b)));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&display).unwrap(),
            value
        );
    }

    #[allow(dead_code)]
    mod fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/responses_fixture.rs"
        ));
    }

    #[test]
    fn saturated_proposal_original_consumer_deadline_stops_and_refuses_reopen() {
        use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
        use std::{
            io::Write,
            os::unix::fs::{DirBuilderExt, OpenOptionsExt},
            sync::atomic::{AtomicUsize, Ordering},
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let Ok(kind) = std::env::var("SCRAMJET_APPROVAL_DEADLINE") else {
            for kind in ["pipe", "socket"] {
                assert!(std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "coding_cli::tests::saturated_proposal_original_consumer_deadline_stops_and_refuses_reopen", "--nocapture"])
                    .env("SCRAMJET_APPROVAL_DEADLINE", kind)
                    .status().unwrap().success());
            }
            return;
        };
        for kind in [kind.as_str()] {
            let base = std::env::temp_dir().join(format!(
                "scramjet-approval-deadline-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let project = base.join("project");
            let storage = base.join("storage");
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&base)
                .unwrap();
            for path in [&project, &storage] {
                std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
            }
            let (reader, output): (File, File) = if kind == "pipe" {
                let path = base.join("output");
                rustix::fs::mkfifoat(
                    rustix::fs::CWD,
                    &path,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                )
                .unwrap();
                let reader = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(OFlags::NONBLOCK.bits() as i32)
                    .open(&path)
                    .unwrap();
                (
                    reader,
                    std::fs::OpenOptions::new().write(true).open(path).unwrap(),
                )
            } else {
                let (reader, output) = std::os::unix::net::UnixStream::pair().unwrap();
                (
                    std::os::fd::OwnedFd::from(reader).into(),
                    std::os::fd::OwnedFd::from(output).into(),
                )
            };
            let flags = fcntl_getfl(&output).unwrap();
            fcntl_setfl(&output, flags | OFlags::NONBLOCK).unwrap();
            loop {
                match rustix::io::write(&output, &[b'x'; 4096]) {
                    Ok(_) => {}
                    Err(rustix::io::Errno::AGAIN) => break,
                    Err(e) => panic!("{e}"),
                }
            }
            fcntl_setfl(&output, flags).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut head = Vec::new();
                let mut byte = [0];
                while !head.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    head.push(byte[0]);
                }
                let length: usize = std::str::from_utf8(&head)
                    .unwrap()
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|s| s.trim().parse().unwrap())
                    })
                    .unwrap();
                assert!(length <= INPUT_BYTES);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let mut events = fixture::call_events(
                    r#"{"path":"input.md","content":"new bytes"}"#,
                    fixture::known_usage(),
                );
                for event in &mut events {
                    if event["item"]["type"] == "function_call" {
                        event["item"]["name"] = serde_json::json!("write");
                    }
                }
                events.last_mut().unwrap()["response"]["output"][0]["name"] =
                    serde_json::json!("write");
                let wire = fixture::wire(&events, true);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", wire.len()).unwrap();
                stream.write_all(&wire).unwrap();
                listener.set_nonblocking(true).unwrap();
                listener
            });
            let checked = output.try_clone().unwrap();
            let options = CodingInvocationOptions {
                project: project.clone(),
                storage: storage.clone(),
                run: "deadline".into(),
                files: ["input.md".into(), "reader.py".into()],
                prompt: "Create the selected file.".into(),
                previous: None,
                endpoint,
            };
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let entered = executor.enter();
            let (invocation, runtime) =
                CodingInvocation::start(options, move |intent, cancel, deadline| {
                    // Narrow only this test owner's original consumer deadline, never the production run allowance.
                    let deadline = deadline.min(Instant::now() + Duration::from_millis(100));
                    let displayed = safe_json(&serde_json::to_vec(intent).unwrap())?;
                    present(&output, &displayed, cancel, deadline)?;
                    Ok(true)
                })
                .unwrap();
            drop(entered);
            let started = Instant::now();
            let result = executor.block_on(invocation.run(runtime)).unwrap();
            assert!(!result.success);
            assert!(result.failure.as_ref().unwrap().contains("deadline"));
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(fcntl_getfl(&checked).unwrap(), flags);
            assert!(!project.join("input.md").exists());
            let journal = storage.join("deadline/invocation.jsonl");
            let before = std::fs::read(&journal).unwrap();
            assert!(
                std::str::from_utf8(&before)
                    .unwrap()
                    .lines()
                    .last()
                    .unwrap()
                    .contains("\"kind\":\"Stopped\"")
            );
            let record = base.join("record");
            std::fs::write(&record, &result.record).unwrap();
            if let Ok(cli) = std::env::var("SCRAMJET_CONTROL_CLI") {
                let prompt = base.join("prompt");
                std::fs::write(&prompt, "Try again.").unwrap();
                let retry = std::process::Command::new(cli)
                    .args([
                        "coding",
                        "--endpoint",
                        &endpoint.to_string(),
                        "--run",
                        "deadline",
                        "--file",
                        "input.md",
                        "--file",
                        "reader.py",
                        "--project",
                    ])
                    .arg(&project)
                    .arg("--storage")
                    .arg(&storage)
                    .arg("--prompt-file")
                    .arg(prompt)
                    .arg("--record-file")
                    .arg(record)
                    .output()
                    .unwrap();
                assert!(!retry.status.success());
                assert_eq!(std::fs::read(&journal).unwrap(), before);
            }
            let listener = server.join().unwrap();
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
            drop(result);
            drop(reader);
            std::fs::remove_dir_all(base).unwrap();
            println!(
                "approval {kind}: narrowed original test-owner deadline stopped without drainage or subsequent POST/effect"
            );
        }
    }
}
