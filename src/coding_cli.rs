use scramjet_rs::{
    CodingError, CodingInvocation, CodingInvocationIntent, CodingInvocationOptions,
    EditCancellation,
};
use serde::Deserialize;
use std::{
    ffi::OsString,
    fs::File,
    io::{Read, Write},
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
fn decide(
    intent: &CodingInvocationIntent,
    automatic: bool,
    cancel: &EditCancellation,
    deadline: Instant,
) -> Result<bool, CodingError> {
    let bytes = serde_json::to_vec(intent)
        .map_err(|_| CodingError::Invalid("proposal serialization failed"))?;
    let displayed = safe_json(&bytes)?;
    let mut stderr = std::io::stderr().lock();
    writeln!(stderr, "proposal {displayed}")?;
    writeln!(
        stderr,
        "Reply with one JSON line containing approve and this exact intent; EOF denies."
    )?;
    stderr.flush()?;
    drop(stderr);
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
    let mut owned = false;
    let mut automatic = false;
    let mut args = args.iter();
    while let Some(option) = args.next() {
        match option.to_str(){
            Some("--owned-fixture")if !owned=>owned=true,
            Some("--approve-effects")if !automatic=>automatic=true,
            Some("--project")if project.is_none()=>project=Some(PathBuf::from(args.next().ok_or("missing project")?)),
            Some("--storage")if storage.is_none()=>storage=Some(PathBuf::from(args.next().ok_or("missing storage")?)),
            Some("--run")if run.is_none()=>run=Some(args.next().and_then(|s|s.to_str()).ok_or("missing UTF-8 run ID")?.to_owned()),
            Some("--endpoint")if endpoint.is_none()=>endpoint=Some(args.next().and_then(|s|s.to_str()).ok_or("missing endpoint")?.parse::<SocketAddr>().map_err(|_|"invalid loopback endpoint")?),
            Some("--file")if files.len()<2=>files.push(args.next().and_then(|s|s.to_str()).ok_or("missing UTF-8 basename")?.to_owned()),
            Some("--prompt-file")if prompt.is_none()=>prompt=Some(PathBuf::from(args.next().ok_or("missing prompt input")?)),
            Some("--record-file")if previous.is_none()=>previous=Some(PathBuf::from(args.next().ok_or("missing record input")?)),
            _=>return Err("requires --owned-fixture --endpoint ADDR --project PATH --storage PATH --run ID --file NAME --file NAME --prompt-file PATH [- for one stdin line] [--record-file PATH] [--approve-effects]; live execution is stopped".into()),
        }
    }
    if !owned {
        return Err("caller must explicitly assert ownership of the loopback fixture; live execution is stopped".into());
    }
    let endpoint = endpoint.ok_or("missing endpoint")?;
    if !endpoint.ip().is_loopback() || endpoint.port() == 18080 {
        return Err("fixture route refused; live execution is stopped".into());
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
        owned_fixture: owned,
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
        let term_cancel = cancel;
        let int = unsafe {
            signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
                int_cancel.cancel()
            })
        }
        .map_err(|_| "signal owner unavailable")?;
        let term = unsafe {
            signal_hook::low_level::register(signal_hook::consts::SIGTERM, move || {
                term_cancel.cancel()
            })
        }
        .map_err(|_| "signal owner unavailable")?;
        let reported = match invocation.run(runtime).await {
            Ok(result) => tokio::task::spawn_blocking(move || {
                let mut stdout = std::io::stdout().lock();
                let failure = result.failure.clone();
                result.deliver(&mut stdout).map_err(|error| {
                    failure
                        .unwrap_or_else(|| format!("result publication failed; no retry: {error}"))
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
}
