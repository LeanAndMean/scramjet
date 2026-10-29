use super::{
    CodingError, EditCancellation,
    captured_files::{CapturedFile, validate_basename},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub(crate) enum ProcessOutput {
    Text(String),
    Bytes { bytes: Vec<u8> },
}
impl ProcessOutput {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn new(bytes: Vec<u8>) -> Self {
        match String::from_utf8(bytes) {
            Ok(s) => Self::Text(s),
            Err(e) => Self::Bytes {
                bytes: e.into_bytes(),
            },
        }
    }
    pub(crate) fn byte_len(&self) -> usize {
        match self {
            Self::Text(s) => s.len(),
            Self::Bytes { bytes } => bytes.len(),
        }
    }
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            Self::Bytes { .. } => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessResult {
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub stdout: ProcessOutput,
    pub stderr: ProcessOutput,
    pub stopped: Option<String>,
    pub launcher_reaped: bool,
    pub namespace_dead: bool,
    pub status_source: String,
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[derive(Clone)]
enum Launch {
    Legacy { script: String, command: String },
    Python { path: String },
}

pub(crate) const PYTHON_PROFILE: &str = "linux-x86_64-python-v1;/usr/bin/bwrap;unshare-user,pid,net,ipc,uts;default-pid1-reaper;die-with-parent;launcher-pdeath=KILL-before-exec;owned-tree-ptrace=TRACEME-before-exec,EXITKILL,FORK,VFORK,CLONE,EXEC;native-tree=launcher,pid1,one-program;new-session;cap-drop=ALL;ro-bind=/usr/bin/python3.8,/usr/lib/python3.8,/lib/x86_64-linux-gnu,/lib64;symlink=/usr/bin/python3->python3.8;proc-ro=/proc;dev-ro=/dev/null,/dev/zero;root-ro;zero-writable-filesystem-scratch;selected-sealed-captures-ro;direct-fixed-python-argv;seccomp-x86_64-allowlist-v1;alternate-ABI=kill;clone,fork,threads,pipe,socket,memfd,sysv-ipc,mount,ptrace,bpf,io_uring=denied;supervisor-and-filter-fds-consumed-before-command;fork-fds=close_range-CLOEXEC;stdin=/dev/null;cwd=/workspace;environment=inherited-cleared,PATH=/usr/bin:/bin,PWD=/workspace;wall=5s,cpu=2s,as=256MiB,fd=32,core=0,combined-output=16384";

pub(crate) const LEGACY_PROFILE: &str = "linux-x86_64-v1;/usr/bin/bwrap;unshare-user,pid,net,ipc,uts;default-pid1-reaper;die-with-parent;new-session;cap-drop=ALL;ro-bind=/bin/bash,/usr/bin/python3.8,/usr/lib/python3.8,/lib/x86_64-linux-gnu,/lib64;symlink=/usr/bin/python3->python3.8;proc=/proc;minimal-dev=/dev;tmpfs=/tmp;workspace=/workspace;ro-memfd=haiku.md,print_haiku.py;supervisor-info-and-block-fds-consumed-before-command;fork-fds=close_range-CLOEXEC;stdin=/dev/null;cwd=/workspace;argv=/bin/bash,--noprofile,--norc,-c,exec /usr/bin/python3 -I print_haiku.py;environment=PATH=/usr/bin:/bin;wall=5s,cpu=2s,as=256MiB,fd=32,core=0,combined-retained-output=16384";

pub(crate) fn check_host() -> Result<(), CodingError> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(CodingError::Invalid(
            "installed Linux x86_64 isolation profile unsupported",
        ));
    }
    for path in [
        "/usr/bin/bwrap",
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
            "coding-capture",
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
    struct TracePid {
        pid: Pid,
        parent: Option<Pid>,
        initial: bool,
    }
    struct Trace {
        launcher: Pid,
        live: Vec<TracePid>,
        protected: bool,
        launcher_status: Option<std::process::ExitStatus>,
        program_status: Option<std::process::ExitStatus>,
        unsupported_stop: bool,
    }
    fn ptrace(request: usize, pid: Pid, data: usize) -> Result<(), CodingError> {
        if unsafe { syscall(101, request, pid.as_raw_pid(), 0_usize, data) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    impl Trace {
        fn new(pid: Pid) -> Self {
            Self {
                launcher: pid,
                live: vec![TracePid {
                    pid,
                    parent: None,
                    initial: false,
                }],
                protected: false,
                launcher_status: None,
                program_status: None,
                unsupported_stop: false,
            }
        }
        fn pump(&mut self) -> Result<(), CodingError> {
            let mut index = 0;
            while index < self.live.len() {
                let pid = self.live[index].pid;
                let Some((_, status)) =
                    rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::NOHANG)?
                else {
                    index += 1;
                    continue;
                };
                if status.exited() || status.signaled() {
                    use std::os::unix::process::ExitStatusExt;
                    let exit = std::process::ExitStatus::from_raw(status.as_raw());
                    if pid == self.launcher {
                        self.launcher_status = Some(exit);
                    } else if self.live[index]
                        .parent
                        .is_some_and(|parent| parent != self.launcher)
                    {
                        self.program_status = Some(exit);
                    }
                    self.live.remove(index);
                    continue;
                }
                let signal = status
                    .stopping_signal()
                    .ok_or(CodingError::History("unexpected native trace status"))?;
                let event = status.as_raw() >> 16;
                if pid == self.launcher && !self.protected {
                    if signal != 5 {
                        return Err(CodingError::Unsupported(
                            "launcher exec-stop protection unavailable",
                        ));
                    }
                    ptrace(0x4200, pid, 0x100000 | 2 | 4 | 8 | 16)?;
                    self.protected = true;
                    ptrace(7, pid, 0)?;
                } else if matches!(event, 1..=3) {
                    let mut raw = 0_usize;
                    ptrace(0x4201, pid, &mut raw as *mut usize as usize)?;
                    let child =
                        Pid::from_raw(i32::try_from(raw).map_err(|_| {
                            CodingError::History("native child identity unavailable")
                        })?)
                        .ok_or(CodingError::History("native child identity unavailable"))?;
                    if self.live.len() >= 3 {
                        return Err(CodingError::ResourceLimit(
                            "fixed native launcher tree exceeded",
                        ));
                    }
                    self.live.push(TracePid {
                        pid: child,
                        parent: Some(pid),
                        initial: true,
                    });
                    ptrace(7, pid, 0)?;
                } else if event == 4 {
                    ptrace(7, pid, 0)?;
                } else if self.live[index].initial && signal == 19 {
                    self.live[index].initial = false;
                    ptrace(7, pid, 0)?;
                } else {
                    if matches!(signal, 19..=22) {
                        let mut info = [0_usize; 16];
                        if unsafe {
                            syscall(
                                101,
                                0x4202_usize,
                                pid.as_raw_pid(),
                                0_usize,
                                info.as_mut_ptr(),
                            )
                        } != 0
                        {
                            let error = std::io::Error::last_os_error();
                            if error.raw_os_error() == Some(22) {
                                self.unsupported_stop = true;
                                index += 1;
                                continue;
                            }
                            return Err(error.into());
                        }
                    }
                    ptrace(7, pid, signal as usize)?;
                }
                index += 1;
            }
            Ok(())
        }
        fn kill(&self) -> Result<(), CodingError> {
            for entry in &self.live {
                match rustix::process::kill_process(entry.pid, Signal::KILL) {
                    Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        }
    }
    struct OwnedChild {
        child: Child,
        init: Option<OwnedFd>,
        reaped: bool,
        trace: Option<Trace>,
    }
    impl OwnedChild {
        fn trace_poll(&mut self) -> Result<(), CodingError> {
            if let Some(trace) = &mut self.trace {
                trace.pump()?;
            }
            Ok(())
        }
        fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, CodingError> {
            self.trace_poll()?;
            if let Some(trace) = &self.trace {
                Ok(trace.launcher_status)
            } else {
                Ok(self.child.try_wait()?)
            }
        }
        fn settle(&mut self, kill: bool) -> Result<std::process::ExitStatus, CodingError> {
            if kill {
                if let Some(init) = &self.init {
                    if !dead(init)? {
                        match pidfd_send_signal(init, Signal::KILL) {
                            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                } else if self.try_wait()?.is_none() {
                    self.child.kill()?;
                }
                if let Some(trace) = &self.trace {
                    trace.kill()?;
                }
            }
            let status = if self.trace.is_some() {
                let deadline = Instant::now() + Duration::from_secs(1);
                loop {
                    self.trace_poll()?;
                    let trace = self.trace.as_ref().unwrap();
                    if trace.live.is_empty() {
                        break trace.launcher_status.ok_or(CodingError::History(
                            "launcher trace settlement unavailable",
                        ))?;
                    }
                    if Instant::now() >= deadline {
                        return Err(CodingError::History(
                            "namespace trace settlement unavailable",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            } else {
                self.child.wait()?
            };
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
        Finished(Result<ProcessResult, CodingError>),
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
        captures: &[CapturedFile],
        cancellation: &EditCancellation,
        launch: &Launch,
        startup_delay: Duration,
        events: &std::sync::mpsc::SyncSender<Event>,
    ) -> Result<ProcessResult, CodingError> {
        check_host()?;
        if captures.is_empty()
            || captures.len() > 2
            || captures.iter().any(|file| file.content.len() > 512)
        {
            return Err(CodingError::ResourceLimit(
                "isolated capture resource limit",
            ));
        }
        for capture in captures {
            validate_basename(&capture.path)?;
        }
        if captures.len() == 2 && captures[0].path == captures[1].path {
            return Err(CodingError::Invalid("duplicate isolated capture"));
        }
        if let Launch::Python { path } = launch
            && !captures.iter().any(|c| c.path == *path)
        {
            return Err(CodingError::Unsupported(
                "selected Python capture unavailable",
            ));
        }
        #[cfg(test)]
        let fork_gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let started = Instant::now();
        let targets = captures
            .iter()
            .map(|c| format!("/workspace/{}", c.path))
            .collect::<Vec<_>>();
        let captures = captures
            .iter()
            .enumerate()
            .map(|(i, c)| {
                sealed(match launch {
                    Launch::Legacy { script, .. } if i == 1 => script.as_bytes(),
                    _ => c.content.as_bytes(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let filter = if matches!(launch, Launch::Python { .. }) {
            Some(sealed(&super::seccomp::filter())?)
        } else {
            None
        };
        let (mut info_reader, info_writer) = UnixStream::pair()?;
        let (gate_reader, mut gate_writer) = UnixStream::pair()?;
        info_reader.set_nonblocking(true)?;
        let info_fd = info_writer.as_raw_fd().to_string();
        let gate_fd = gate_reader.as_raw_fd().to_string();
        let capture_fds = captures
            .iter()
            .map(|f| f.as_raw_fd().to_string())
            .collect::<Vec<_>>();
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
            ]);
        match launch {
            Launch::Legacy { .. } => {
                command.args([
                    "--ro-bind",
                    "/bin/bash",
                    "/bin/bash",
                    "--dev",
                    "/dev",
                    "--tmpfs",
                    "/tmp",
                ]);
            }
            Launch::Python { .. } => {
                command.args([
                    "--remount-ro",
                    "/proc",
                    "--dir",
                    "/dev",
                    "--ro-bind",
                    "/dev/null",
                    "/dev/null",
                    "--ro-bind",
                    "/dev/zero",
                    "/dev/zero",
                    "--dir",
                    "/dev/shm",
                    "--dir",
                    "/tmp",
                ]);
            }
        }
        command.args(["--dir", "/workspace"]);
        for (fd, target) in capture_fds.iter().zip(&targets) {
            command.args(["--ro-bind-data", fd, target]);
        }
        command.args(["--chdir", "/workspace"]);
        match launch {
            Launch::Legacy { command: argv, .. } => {
                command.args(["/bin/bash", "--noprofile", "--norc", "-c", argv]);
            }
            Launch::Python { path } => {
                command.args([
                    "--remount-ro",
                    "/",
                    "--seccomp",
                    &filter.as_ref().unwrap().as_raw_fd().to_string(),
                    "/usr/bin/python3",
                    "-I",
                    &format!("/workspace/{path}"),
                ]);
            }
        }
        command
            .current_dir("/")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        let parent = rustix::process::getpid();
        let bind_parent = matches!(launch, Launch::Python { .. });
        unsafe {
            command.pre_exec(move || {
                if bind_parent {
                    rustix::process::set_parent_process_death_signal(Some(Signal::KILL))?;
                    if syscall(101, 0_usize, 0_usize, 0_usize, 0_usize) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if rustix::process::getppid() != Some(parent) {
                        return Err(std::io::Error::other("process launch owner disappeared"));
                    }
                }
                // Linux close_range CLOEXEC closes the fork-time descriptor race without closing Rust's exec-error pipe.
                if syscall(SYS_CLOSE_RANGE, 3u32, u32::MAX, CLOSE_RANGE_CLOEXEC) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for file in &captures {
                    fcntl_setfd(file, FdFlags::empty())?;
                }
                fcntl_setfd(&info_writer, FdFlags::empty())?;
                fcntl_setfd(&gate_reader, FdFlags::empty())?;
                if let Some(filter) = &filter {
                    fcntl_setfd(filter, FdFlags::empty())?;
                }
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
            trace: None,
        };
        #[cfg(test)]
        drop(fork_gate);
        if bind_parent {
            owned.trace = Some(Trace::new(Pid::from_raw(owned.child.id() as i32).unwrap()));
        }
        let result = (|| {
            drop(command);
            while owned.trace.as_ref().is_some_and(|trace| !trace.protected) {
                owned.trace_poll()?;
                if cancellation.is_cancelled() {
                    return Err(CodingError::Cancelled);
                }
                if started.elapsed() >= Duration::from_secs(5) {
                    return Err(CodingError::ResourceLimit(
                        "native exec-stop setup deadline",
                    ));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
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
                owned.trace_poll()?;
                if owned
                    .trace
                    .as_ref()
                    .is_some_and(|trace| trace.unsupported_stop)
                {
                    stopped.get_or_insert_with(|| "isolated job-control stop unsupported".into());
                }
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
                                owned.trace_poll()?;
                                if owned
                                    .trace
                                    .as_ref()
                                    .is_some_and(|trace| trace.unsupported_stop)
                                {
                                    stopped = Some("isolated job-control stop unsupported".into());
                                    break;
                                }
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
                    status = owned.try_wait()?;
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
            let program_status = owned.trace.as_ref().and_then(|trace| trace.program_status);
            let result_status = program_status.unwrap_or(status);
            Ok(ProcessResult {
                exit_code: result_status.code(),
                signal: std::os::unix::process::ExitStatusExt::signal(&result_status),
                stdout: ProcessOutput::new(out),
                stderr: ProcessOutput::new(err),
                stopped,
                launcher_reaped: true,
                namespace_dead,
                status_source: if program_status.is_some() {
                    "program_wait_status"
                } else {
                    "bubblewrap_launcher"
                }
                .into(),
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
    pub(super) fn supervised(
        captures: &[CapturedFile],
        cancellation: &EditCancellation,
        launch: Launch,
        delay: Duration,
        start: &mut impl FnMut(u32, u32) -> Result<(), CodingError>,
        observed: &mut impl FnMut(&ProcessResult),
    ) -> Result<ProcessResult, CodingError> {
        let captures = captures.to_vec();
        let cancel = cancellation.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("captured-process".into())
            .spawn(move || {
                let result = run(&captures, &cancel, &launch, delay, &send);
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
}

pub(crate) fn legacy(
    captures: &[CapturedFile; 2],
    script: &str,
    command: &str,
    cancellation: &EditCancellation,
    delay: std::time::Duration,
    mut start: impl FnMut(u32, u32) -> Result<(), CodingError>,
    mut observed: impl FnMut(&ProcessResult),
) -> Result<ProcessResult, CodingError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        linux::supervised(
            captures,
            cancellation,
            Launch::Legacy {
                script: script.into(),
                command: command.into(),
            },
            delay,
            &mut start,
            &mut observed,
        )
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (
            captures,
            script,
            command,
            cancellation,
            delay,
            start,
            observed,
        );
        Err(CodingError::Unsupported("isolation unsupported"))
    }
}
pub(crate) fn check_python_host() -> Result<(), CodingError> {
    check_host()
}
pub(crate) fn python(
    captures: &[CapturedFile],
    path: &str,
    cancellation: &EditCancellation,
    mut start: impl FnMut(u32, u32) -> Result<(), CodingError>,
) -> Result<ProcessResult, CodingError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        linux::supervised(
            captures,
            cancellation,
            Launch::Python { path: path.into() },
            std::time::Duration::ZERO,
            &mut start,
            &mut |_| {},
        )
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (captures, path, cancellation, start);
        Err(CodingError::Unsupported("isolation unsupported"))
    }
}
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod seccomp;
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
mod tests;
