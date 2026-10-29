use super::{
    CodingError, EditCancellation, HAIKU_COMMAND, HAIKU_SCRIPT, HaikuCapture, HaikuProcessResult,
};

pub(super) const PROFILE: &str = "linux-x86_64-v1;/usr/bin/bwrap;unshare-user,pid,net,ipc,uts;default-pid1-reaper;die-with-parent;new-session;cap-drop=ALL;ro-bind=/bin/bash,/usr/bin/python3.8,/usr/lib/python3.8,/lib/x86_64-linux-gnu,/lib64;symlink=/usr/bin/python3->python3.8;proc=/proc;minimal-dev=/dev;tmpfs=/tmp;workspace=/workspace;ro-memfd=haiku.md,print_haiku.py;supervisor-info-and-block-fds-consumed-before-command;fork-fds=close_range-CLOEXEC;stdin=/dev/null;cwd=/workspace;argv=/bin/bash,--noprofile,--norc,-c,exec /usr/bin/python3 -I print_haiku.py;environment=PATH=/usr/bin:/bin;wall=5s,cpu=2s,as=256MiB,fd=32,core=0,combined-retained-output=16384";

pub(super) fn check_host() -> Result<(), CodingError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(CodingError::Invalid(
            "installed Linux x86_64 isolation profile unsupported",
        ));
    }
    for path in [
        "/usr/bin/bwrap",
        "/bin/bash",
        "/usr/bin/python3.8",
        "/usr/lib/python3.8",
        "/lib/x86_64-linux-gnu",
        "/lib64",
    ] {
        let metadata = std::fs::metadata(path)?;
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != 0 || metadata.mode() & 0o6022 != 0 {
            return Err(CodingError::Invalid(
                "untrusted installed isolation runtime",
            ));
        }
    }
    if std::fs::read_link("/usr/bin/python3")? != std::path::Path::new("python3.8") {
        return Err(CodingError::Invalid("unsupported installed Python symlink"));
    }
    Ok(())
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use super::*;
    use rustix::{
        event::{PollFd, PollFlags, poll},
        fs::{
            MemfdFlags, OFlags, SealFlags, fcntl_add_seals, fcntl_getfl, fcntl_setfl, memfd_create,
        },
        io::{FdFlags, fcntl_setfd},
        process::{
            Pid, PidfdFlags, Resource, Rlimit, Signal, pidfd_open, pidfd_send_signal, setrlimit,
        },
    };
    use std::{
        fs::File,
        io::{Read, Seek, SeekFrom, Write},
        os::{
            fd::{AsRawFd, OwnedFd},
            unix::{net::UnixStream, process::CommandExt},
        },
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    const OUTPUT_BYTES: usize = 16_384;
    const INFO_BYTES: usize = 1_024;
    const SYS_CLOSE_RANGE: std::ffi::c_long = 436;
    const CLOSE_RANGE_CLOEXEC: u32 = 4;
    unsafe extern "C" {
        fn syscall(number: std::ffi::c_long, ...) -> std::ffi::c_long;
    }

    fn sealed(bytes: &[u8]) -> Result<File, CodingError> {
        let mut file = File::from(memfd_create(
            "haiku-capture",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?);
        file.write_all(bytes)?;
        file.seek(SeekFrom::Start(0))?;
        fcntl_add_seals(
            &file,
            SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
        )?;
        Ok(file)
    }
    struct OwnedChild {
        child: Child,
        init: Option<OwnedFd>,
        reaped: bool,
    }
    impl OwnedChild {
        fn settle(&mut self, kill: bool) -> Result<std::process::ExitStatus, CodingError> {
            if kill {
                if let Some(init) = &self.init {
                    if !dead(init)? {
                        match pidfd_send_signal(init, Signal::KILL) {
                            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                } else if self.child.try_wait()?.is_none() {
                    self.child.kill()?;
                }
            }
            let status = self.child.wait()?;
            let init = self.init.as_ref().ok_or(CodingError::Invalid(
                "sandbox namespace ownership unavailable; settlement unverified",
            ))?;
            let mut fds = [PollFd::new(init, PollFlags::IN)];
            poll(
                &mut fds,
                Some(&rustix::event::Timespec {
                    tv_sec: 1,
                    tv_nsec: 0,
                }),
            )?;
            if !fds[0].revents().contains(PollFlags::IN) {
                return Err(CodingError::Invalid(
                    "sandbox namespace death not established",
                ));
            }
            self.reaped = true;
            Ok(status)
        }
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !self.reaped {
                let _ = self.settle(true);
            }
        }
    }
    enum Event {
        Started {
            launcher: u32,
            namespace_init: u32,
            acknowledged: std::sync::mpsc::SyncSender<bool>,
        },
        Finished(Result<HaikuProcessResult, CodingError>),
    }
    struct Supervisor {
        worker: Option<std::thread::JoinHandle<()>>,
        cancellation: EditCancellation,
    }
    impl Drop for Supervisor {
        fn drop(&mut self) {
            if let Some(worker) = self.worker.take() {
                self.cancellation.cancel();
                let _ = worker.join();
            }
        }
    }
    fn drain(
        reader: &mut impl Read,
        bytes: &mut Vec<u8>,
        total: &mut usize,
        limit: usize,
    ) -> Result<bool, CodingError> {
        let mut chunk = [0; 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => return Ok(true),
                Ok(count) => {
                    if count > limit - *total {
                        return Err(CodingError::Invalid("isolated output resource limit"));
                    }
                    *total += count;
                    bytes.extend_from_slice(&chunk[..count]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn init_handle(info: &[u8], launcher: u32) -> Result<Option<(OwnedFd, u32)>, CodingError> {
        let Ok(info) = serde_json::from_slice::<serde_json::Value>(info) else {
            return Ok(None);
        };
        let raw = info["child-pid"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or(CodingError::Invalid("missing sandbox init identity"))?;
        let pid = Pid::from_raw(raw as i32)
            .ok_or(CodingError::Invalid("invalid sandbox init identity"))?;
        let path = format!("/proc/{raw}/status");
        let belongs = |text: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix("PPid:\t"))
                .and_then(|id| id.parse::<u32>().ok())
                == Some(launcher)
        };
        let before = std::fs::read_to_string(&path)?;
        if !belongs(&before) {
            return Err(CodingError::Invalid(
                "sandbox init is not owned by launcher",
            ));
        }
        let fd = pidfd_open(pid, PidfdFlags::empty())?;
        if !belongs(&std::fs::read_to_string(path)?) {
            return Err(CodingError::Invalid(
                "sandbox init changed before supervision",
            ));
        }
        if std::fs::read_link(format!("/proc/{raw}/ns/pid"))?
            == std::fs::read_link("/proc/self/ns/pid")?
        {
            return Err(CodingError::Invalid(
                "sandbox PID namespace was not isolated",
            ));
        }
        Ok(Some((fd, raw)))
    }
    fn dead(fd: &OwnedFd) -> Result<bool, CodingError> {
        let mut fds = [PollFd::new(fd, PollFlags::IN)];
        Ok(poll(
            &mut fds,
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        )? == 1
            && fds[0].revents().contains(PollFlags::IN))
    }
    fn run(
        captures: &[HaikuCapture; 2],
        cancellation: &EditCancellation,
        script: &str,
        startup_delay: Duration,
        events: &std::sync::mpsc::SyncSender<Event>,
    ) -> Result<HaikuProcessResult, CodingError> {
        check_host()?;
        if captures[0].path != "haiku.md"
            || captures[1].path != "print_haiku.py"
            || captures.iter().any(|file| file.content.len() > 512)
            || captures[1].content != HAIKU_SCRIPT
        {
            return Err(CodingError::Invalid("unsupported isolated capture"));
        }
        #[cfg(test)]
        let fork_gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let started = Instant::now();
        let captures = [
            sealed(captures[0].content.as_bytes())?,
            sealed(script.as_bytes())?,
        ];
        let (mut info_reader, info_writer) = UnixStream::pair()?;
        let (gate_reader, mut gate_writer) = UnixStream::pair()?;
        info_reader.set_nonblocking(true)?;
        let info_fd = info_writer.as_raw_fd().to_string();
        let gate_fd = gate_reader.as_raw_fd().to_string();
        let first = captures[0].as_raw_fd().to_string();
        let second = captures[1].as_raw_fd().to_string();
        let mut command = Command::new("/usr/bin/bwrap");
        command
            .args([
                "--unshare-user",
                "--unshare-pid",
                "--unshare-net",
                "--unshare-ipc",
                "--unshare-uts",
                "--die-with-parent",
                "--new-session",
                "--cap-drop",
                "ALL",
                "--info-fd",
                &info_fd,
                "--block-fd",
                &gate_fd,
            ])
            .args([
                "--ro-bind",
                "/bin/bash",
                "/bin/bash",
                "--ro-bind",
                "/usr/bin/python3.8",
                "/usr/bin/python3.8",
                "--symlink",
                "python3.8",
                "/usr/bin/python3",
                "--ro-bind",
                "/usr/lib/python3.8",
                "/usr/lib/python3.8",
                "--ro-bind",
                "/lib/x86_64-linux-gnu",
                "/lib/x86_64-linux-gnu",
                "--ro-bind",
                "/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--dir",
                "/workspace",
            ])
            .args([
                "--ro-bind-data",
                &first,
                "/workspace/haiku.md",
                "--ro-bind-data",
                &second,
                "/workspace/print_haiku.py",
                "--chdir",
                "/workspace",
                "/bin/bash",
                "--noprofile",
                "--norc",
                "-c",
                HAIKU_COMMAND,
            ])
            .current_dir("/")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        unsafe {
            command.pre_exec(move || {
                // Linux close_range CLOEXEC closes the fork-time descriptor race without closing Rust's exec-error pipe.
                if syscall(SYS_CLOSE_RANGE, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for file in &captures {
                    fcntl_setfd(file, FdFlags::empty())?;
                }
                fcntl_setfd(&info_writer, FdFlags::empty())?;
                fcntl_setfd(&gate_reader, FdFlags::empty())?;
                for (resource, value) in [
                    (Resource::Cpu, 2),
                    (Resource::As, 268_435_456),
                    (Resource::Nofile, 32),
                    (Resource::Core, 0),
                ] {
                    setrlimit(
                        resource,
                        Rlimit {
                            current: Some(value),
                            maximum: Some(value),
                        },
                    )?;
                }
                Ok(())
            });
        }
        let mut owned = OwnedChild {
            child: command.spawn()?,
            init: None,
            reaped: false,
        };
        #[cfg(test)]
        drop(fork_gate);
        let result = (|| {
            drop(command);
            std::thread::sleep(startup_delay);
            let mut stdout = owned.child.stdout.take().unwrap();
            let mut stderr = owned.child.stderr.take().unwrap();
            fcntl_setfl(&stdout, fcntl_getfl(&stdout)? | OFlags::NONBLOCK)?;
            fcntl_setfl(&stderr, fcntl_getfl(&stderr)? | OFlags::NONBLOCK)?;
            let (mut out, mut err, mut info) = (Vec::new(), Vec::new(), Vec::new());
            let (mut total, mut info_total) = (0, 0);
            let (mut out_eof, mut err_eof) = (false, false);
            let mut stopped = None;
            let mut status = None;
            loop {
                let _ = drain(&mut info_reader, &mut info, &mut info_total, INFO_BYTES)?;
                if owned.init.is_none() {
                    match init_handle(&info, owned.child.id()) {
                        Ok(Some((fd, namespace_init))) => {
                            owned.init = Some(fd);
                            let (acknowledged, receive) = std::sync::mpsc::sync_channel(1);
                            events
                                .send(Event::Started {
                                    launcher: owned.child.id(),
                                    namespace_init,
                                    acknowledged,
                                })
                                .map_err(|_| CodingError::Cancelled)?;
                            loop {
                                if cancellation.is_cancelled() {
                                    stopped = Some("cancelled before command release".into());
                                    break;
                                }
                                if started.elapsed() >= Duration::from_secs(5) {
                                    stopped = Some("isolated wall-time resource limit".into());
                                    break;
                                }
                                match receive.recv_timeout(Duration::from_millis(1)) {
                                    Ok(true) => {
                                        if cancellation.is_cancelled() {
                                            stopped =
                                                Some("cancelled before command release".into());
                                        } else if started.elapsed() >= Duration::from_secs(5) {
                                            stopped =
                                                Some("isolated wall-time resource limit".into());
                                        } else {
                                            gate_writer.write_all(b"1")?;
                                        }
                                        break;
                                    }
                                    Ok(false)
                                    | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                        stopped = Some("start acknowledgement refused".into());
                                        break;
                                    }
                                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            stopped.get_or_insert_with(|| error.to_string());
                        }
                    }
                }
                if !out_eof {
                    match drain(&mut stdout, &mut out, &mut total, OUTPUT_BYTES) {
                        Ok(eof) => out_eof = eof,
                        Err(error) => {
                            stopped.get_or_insert_with(|| error.to_string());
                        }
                    }
                }
                if !err_eof {
                    match drain(&mut stderr, &mut err, &mut total, OUTPUT_BYTES) {
                        Ok(eof) => err_eof = eof,
                        Err(error) => {
                            stopped.get_or_insert_with(|| error.to_string());
                        }
                    }
                }
                if cancellation.is_cancelled() {
                    stopped.get_or_insert_with(|| "cancelled".into());
                }
                if started.elapsed() >= Duration::from_secs(5) {
                    stopped.get_or_insert_with(|| "isolated wall-time resource limit".into());
                }
                if stopped.is_some() {
                    if let Some(init) = &owned.init {
                        if !dead(init)? {
                            match pidfd_send_signal(init, Signal::KILL) {
                                Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                    } else if status.is_none() {
                        owned.child.kill()?;
                    }
                }
                if status.is_none() {
                    status = owned.child.try_wait()?;
                }
                if status.is_some() && out_eof && err_eof {
                    break;
                }
                if stopped.is_some() && started.elapsed() > Duration::from_secs(6) {
                    return Err(CodingError::Invalid(
                        "isolated tree/pipe settlement unestablished",
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let status = owned.settle(false)?;
            let namespace_dead = true;
            Ok(HaikuProcessResult {
                exit_code: status.code(),
                signal: std::os::unix::process::ExitStatusExt::signal(&status),
                stdout: String::from_utf8(out)
                    .map_err(|_| CodingError::Invalid("isolated stdout is not UTF-8"))?,
                stderr: String::from_utf8(err)
                    .map_err(|_| CodingError::Invalid("isolated stderr is not UTF-8"))?,
                stopped,
                launcher_reaped: true,
                namespace_dead,
            })
        })();
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                owned.settle(true)?;
                Err(error)
            }
        }
    }
    pub(super) fn execute(
        captures: &[HaikuCapture; 2],
        cancellation: &EditCancellation,
        start: &mut impl FnMut(u32, u32) -> Result<(), CodingError>,
        observed: &mut impl FnMut(&HaikuProcessResult),
    ) -> Result<HaikuProcessResult, CodingError> {
        supervised(
            captures,
            cancellation,
            HAIKU_SCRIPT,
            Duration::ZERO,
            start,
            observed,
        )
    }
    fn supervised(
        captures: &[HaikuCapture; 2],
        cancellation: &EditCancellation,
        script: &str,
        delay: Duration,
        start: &mut impl FnMut(u32, u32) -> Result<(), CodingError>,
        observed: &mut impl FnMut(&HaikuProcessResult),
    ) -> Result<HaikuProcessResult, CodingError> {
        let captures = captures.clone();
        let script = script.to_owned();
        let cancel = cancellation.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("haiku-process".into())
            .spawn(move || {
                let result = run(&captures, &cancel, &script, delay, &send);
                let _ = send.send(Event::Finished(result));
            })?;
        let mut supervisor = Supervisor {
            worker: Some(worker),
            cancellation: cancellation.clone(),
        };
        let mut acknowledgement_error = None;
        loop {
            match receive
                .recv()
                .map_err(|_| CodingError::Invalid("process supervisor acknowledgement lost"))?
            {
                Event::Started {
                    launcher,
                    namespace_init,
                    acknowledged,
                } => {
                    let result = start(launcher, namespace_init);
                    let _ = acknowledged.send(result.is_ok());
                    acknowledgement_error = result.err();
                }
                Event::Finished(result) => {
                    supervisor
                        .worker
                        .take()
                        .unwrap()
                        .join()
                        .map_err(|_| CodingError::Invalid("process supervisor panicked"))?;
                    let result = result?;
                    observed(&result);
                    if let Some(error) = acknowledgement_error {
                        return Err(error);
                    }
                    return Ok(result);
                }
            }
        }
    }
    #[cfg(test)]
    pub(super) fn control_started(
        captures: &[HaikuCapture; 2],
        cancellation: &EditCancellation,
        script: &str,
        start: &mut impl FnMut(u32, u32) -> Result<(), CodingError>,
    ) -> Result<HaikuProcessResult, CodingError> {
        supervised(
            captures,
            cancellation,
            script,
            Duration::from_millis(100),
            start,
            &mut |_| {},
        )
    }
    #[cfg(test)]
    pub(super) fn control(
        captures: &[HaikuCapture; 2],
        cancellation: &EditCancellation,
        script: &str,
    ) -> Result<HaikuProcessResult, CodingError> {
        supervised(
            captures,
            cancellation,
            script,
            Duration::from_millis(100),
            &mut |_, _| Ok(()),
            &mut |_| {},
        )
    }
}

pub(super) fn execute(
    captures: &[HaikuCapture; 2],
    cancellation: &EditCancellation,
    mut start: impl FnMut(u32, u32) -> Result<(), CodingError>,
    mut observed: impl FnMut(&HaikuProcessResult),
) -> Result<HaikuProcessResult, CodingError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        linux::execute(captures, cancellation, &mut start, &mut observed)
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (captures, cancellation, start, observed);
        Err(CodingError::Invalid("isolation unsupported"))
    }
}
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(super) fn control_started(
    captures: &[HaikuCapture; 2],
    cancellation: &EditCancellation,
    script: &str,
    mut start: impl FnMut(u32, u32) -> Result<(), CodingError>,
) -> Result<HaikuProcessResult, CodingError> {
    linux::control_started(captures, cancellation, script, &mut start)
}
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(super) fn control(
    captures: &[HaikuCapture; 2],
    cancellation: &EditCancellation,
    script: &str,
) -> Result<HaikuProcessResult, CodingError> {
    linux::control(captures, cancellation, script)
}
