use crate::haiku_demo::fixture;
use crate::provider_demo::server;
use scramjet_rs::{
    HaikuAgent, HaikuControl, HaikuHistory, HaikuObserver, HaikuProposal, HaikuSession,
    HaikuSnapshot, ProviderHistory, Runtime, haiku_prompt, reconstruct_haiku_history,
};
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Offer {
    pub revision: u64,
}
struct State {
    observer: Option<HaikuObserver>,
    recovered: Option<Arc<HaikuSnapshot>>,
    control: Option<HaikuControl>,
    sender: Option<mpsc::SyncSender<()>>,
    offered: bool,
    painted: Option<Offer>,
    stopped: bool,
    rejection: Option<&'static str>,
    error: Option<String>,
    runtime_joined: bool,
    accounting: Option<Arc<ProviderHistory>>,
}
impl State {
    fn cancelled(&self) -> bool {
        self.stopped
            || self
                .control
                .as_ref()
                .is_some_and(|control| control.cancellation().is_cancelled())
    }
}
pub(crate) struct Bridge {
    state: Mutex<State>,
    pub deadline: Instant,
}
pub(crate) struct Projection {
    pub snapshot: Arc<HaikuSnapshot>,
    pub offered: bool,
    pub rejection: Option<&'static str>,
    pub error: Option<String>,
    pub runtime_joined: bool,
    pub accounting: Option<Arc<ProviderHistory>>,
    pub recovered: bool,
}
impl Bridge {
    pub fn snapshot(&self) -> Projection {
        let s = self.state.lock().unwrap();
        Projection {
            snapshot: s.observer.as_ref().map_or_else(
                || s.recovered.as_ref().unwrap().clone(),
                HaikuObserver::snapshot,
            ),
            offered: s.offered && !s.cancelled() && Instant::now() < self.deadline,
            rejection: s.rejection,
            error: s.error.clone(),
            runtime_joined: s.runtime_joined,
            accounting: s.accounting.clone(),
            recovered: s.recovered.is_some(),
        }
    }
    pub fn cancellation(&self) -> Option<scramjet_rs::EditCancellation> {
        self.state
            .lock()
            .unwrap()
            .control
            .as_ref()
            .map(HaikuControl::cancellation)
    }
    pub fn painted(&self, offer: Option<Offer>) {
        let mut s = self.state.lock().unwrap();
        s.painted = offer.filter(|_| s.offered && !s.cancelled() && Instant::now() < self.deadline);
    }
    pub fn submit(&self, offer: Option<Offer>, revision: u64, request: &str) {
        let mut s = self.state.lock().unwrap();
        if !s.offered
            || s.cancelled()
            || Instant::now() >= self.deadline
            || offer.is_none()
            || s.painted != offer
            || offer.unwrap().revision != revision
        {
            s.rejection = Some("StaleUnpaintedStoppedOrExpiredSubmission");
        } else if request != haiku_prompt() {
            s.rejection = Some(
                "UnsupportedRequest: draft retained; only the fixed displayed request is supported",
            );
        } else if s
            .sender
            .as_ref()
            .is_none_or(|sender| sender.try_send(()).is_err())
        {
            s.rejection = Some("SubmissionOwnerUnavailable");
        } else {
            s.offered = false;
            s.painted = None;
            s.rejection = None;
        }
    }
    pub fn reject(&self) {
        self.state.lock().unwrap().rejection = Some("FocusBrowseHistoryApprovalOrViewport");
    }
    pub fn unsupported(&self) {
        self.state.lock().unwrap().rejection = Some(
            "UnsupportedRequest: draft retained; only the fixed displayed request is supported",
        );
    }
    pub fn decide(&self, proposal: &HaikuProposal, approve: bool) {
        let mut s = self.state.lock().unwrap();
        if !s.cancelled()
            && Instant::now() < self.deadline
            && let Some(control) = &s.control
            && let Err(error) = control.decide(proposal, approve)
        {
            s.error = Some(error.to_string());
        }
    }
    pub fn stop(&self) {
        let mut s = self.state.lock().unwrap();
        s.stopped = true;
        s.offered = false;
        s.painted = None;
        if let Some(control) = &s.control {
            control.cancel();
        }
    }
    fn fail(&self, error: impl ToString) {
        self.stop();
        self.state.lock().unwrap().error = Some(error.to_string());
    }
}
#[derive(Default)]
struct ReturnFence {
    returned: Mutex<bool>,
    changed: Condvar,
}
impl ReturnFence {
    fn release(&self) {
        *self.returned.lock().unwrap() = true;
        self.changed.notify_all();
    }
    fn wait(&self) {
        let mut returned = self.returned.lock().unwrap();
        while !*returned {
            returned = self.changed.wait(returned).unwrap();
        }
    }
}
struct HostFence(Arc<ReturnFence>);
impl Drop for HostFence {
    fn drop(&mut self) {
        self.0.wait();
    }
}
struct PanicNotice<'a> {
    bridge: &'a Bridge,
    message: &'static str,
}
impl Drop for PanicNotice<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.bridge.fail(self.message);
        }
    }
}
struct TerminalReturn {
    fence: Arc<ReturnFence>,
    bridge: Option<Arc<Bridge>>,
}
impl Drop for TerminalReturn {
    fn drop(&mut self) {
        if let Some(bridge) = &self.bridge {
            bridge.stop();
        }
        self.fence.release();
    }
}
struct Options {
    project: Option<PathBuf>,
    storage: PathBuf,
    session: String,
    history: bool,
    #[cfg(test)]
    panic_after_ready: bool,
    #[cfg(test)]
    large_approval: bool,
    #[cfg(test)]
    model_delay: bool,
}
fn options(arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut arguments = arguments;
    let (mut fixture, mut history, mut project, mut storage, mut session) =
        (false, false, None, None, None);
    for _ in 0..8 {
        let Some(name) = arguments.next() else {
            break;
        };
        match name.to_str() {
            Some("--fixture") if !fixture => fixture = true,
            Some("--history") if !history => history = true,
            Some("--project") if project.is_none() => project = Some(PathBuf::from(arguments.next().ok_or("missing project")?)),
            Some("--storage") if storage.is_none() => storage = Some(PathBuf::from(arguments.next().ok_or("missing storage")?)),
            Some("--session") if session.is_none() => session = Some(arguments.next().ok_or("missing session")?.into_string().map_err(|_| "session must be UTF8")?),
            _ => return Err("requires explicit --fixture --project PATH --storage PATH, or --history --storage PATH [--session ID]; live and automatic approvals are not eligible".into()),
        }
    }
    if arguments.next().is_some()
        || fixture == history
        || (history && project.is_some())
        || (fixture && project.is_none())
    {
        return Err("conflicting or incomplete haiku UI options".into());
    }
    Ok(Options {
        project,
        storage: storage.ok_or("missing owned storage")?,
        session: session.unwrap_or_else(|| "haiku-ui".into()),
        history,
        #[cfg(test)]
        panic_after_ready: false,
        #[cfg(test)]
        large_approval: false,
        #[cfg(test)]
        model_delay: false,
    })
}
fn recovered(history: Arc<HaikuHistory>, accounting: Option<Arc<ProviderHistory>>) -> Arc<Bridge> {
    Arc::new(Bridge {
        deadline: Instant::now() + Duration::from_secs(300),
        state: Mutex::new(State {
            observer: None,
            control: None,
            sender: None,
            offered: false,
            painted: None,
            stopped: false,
            rejection: None,
            error: None,
            runtime_joined: false,
            accounting,
            recovered: Some(Arc::new(HaikuSnapshot {
                phase: scramjet_rs::HaikuPhase::Ready,
                proposal: None,
                history,
                observed_attempt: None,
                attempt_acknowledged: false,
                observed_process: None,
                process_acknowledged: false,
                failure: None,
                request_id: None,
                run_ended: false,
                worker_terminated: false,
                worker_joined: false,
            })),
        }),
    })
}
fn host(
    options: Options,
    ready: mpsc::SyncSender<Arc<Bridge>>,
    fence: Arc<ReturnFence>,
) -> Result<String, String> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    // Conservative deadline includes initialization and never renews the session's 300s allowance.
    let deadline = Instant::now() + Duration::from_secs(300);
    let session = HaikuSession::create(
        options.project.as_ref().unwrap(),
        &options.storage,
        &options.session,
    )
    .map_err(|e| e.to_string())?;
    let provider_path = options
        .storage
        .join(format!("{}-provider", options.session));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&provider_path)
        .map_err(|e| e.to_string())?;
    let mut agent = HaikuAgent::start(session).map_err(|e| e.to_string())?;
    let poem =
        "# Haiku\nRust unfolds softly\nStreams answer in quiet files\nA small verse returns\n";
    let sequence = fixture::sequence(poem, 0, "opaque fixture reasoning");
    #[cfg(test)]
    let sequence = if options.large_approval {
        tests::large_sequence()
    } else {
        sequence
    };
    let delay_reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(test)]
    let delay = if options.model_delay {
        Duration::from_secs(60)
    } else {
        Duration::ZERO
    };
    #[cfg(not(test))]
    let delay = Duration::ZERO;
    let (endpoint, server) = executor.block_on(server::start_sequence_delayed(
        sequence,
        delay,
        Some(delay_reached.clone()),
    ));
    let runtime = executor
        .block_on(async { Runtime::new_provider(endpoint, &provider_path) })
        .map_err(|e| e.to_string())?;
    let (sender, actions) = mpsc::sync_channel(1);
    let bridge = Arc::new(Bridge {
        deadline,
        state: Mutex::new(State {
            observer: Some(agent.observer()),
            recovered: None,
            control: Some(agent.control()),
            sender: Some(sender),
            offered: true,
            painted: None,
            stopped: false,
            rejection: None,
            error: None,
            runtime_joined: false,
            accounting: runtime.provider_history().map(Arc::new),
        }),
    });
    let sampling_stopped = std::sync::atomic::AtomicBool::new(false);
    let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        std::thread::scope(|scope| {
            let sampler = std::thread::Builder::new()
                .name("haiku-accounting".into())
                .spawn_scoped(scope, || {
                    let _notice = PanicNotice {
                        bridge: &bridge,
                        message: "provider accounting observer panicked; accounting incomplete",
                    };
                    while !sampling_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                        let history = match runtime.try_provider_history() {
                            Ok(history) => history,
                            Err(error) => {
                                bridge.fail(error);
                                break;
                            }
                        };
                        if let Some(history) = history {
                            let mut state = bridge.state.lock().unwrap();
                            if state
                                .accounting
                                .as_ref()
                                .is_none_or(|current| **current != history)
                            {
                                state.accounting = Some(Arc::new(history));
                            }
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                })
                .map_err(|error| error.to_string())?;
            struct StopSampling<'a>(&'a std::sync::atomic::AtomicBool);
            impl Drop for StopSampling<'_> {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let stop_sampling = StopSampling(&sampling_stopped);
            // This inner fence also precedes the scope's implicit sampler join on unwind.
            let sampler_fence = HostFence(fence.clone());
            let _notice = PanicNotice {
                bridge: &bridge,
                message: "haiku host owner panicked; acknowledgement uncertain",
            };
            let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if ready.send(bridge.clone()).is_err() {
                    bridge.stop();
                    return Err("terminal readiness abandoned".to_owned());
                }
                #[cfg(test)]
                if options.panic_after_ready {
                    panic!("synthetic borrowed host interval panic");
                }
                loop {
                    if bridge.state.lock().unwrap().cancelled() || Instant::now() >= deadline {
                        return Err("stopped before submission".to_owned());
                    }
                    match actions.try_recv() {
                        Ok(()) => break,
                        Err(mpsc::TryRecvError::Empty) => {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            return Err("input owner lost".to_owned());
                        }
                    }
                }
                executor
                    .block_on(agent.run_unjoined(&runtime))
                    .map_err(|e| e.to_string())
            }));
            let execution = match execution {
                Ok(result) => result.map(Some),
                Err(_) => Err("haiku host execution panic; acknowledgement uncertain".into()),
            };
            if let Err(error) = &execution {
                bridge.fail(error);
            }
            fence.wait();
            drop(sampler_fence);
            drop(stop_sampling);
            if sampler.join().is_err() {
                return Err("provider accounting observer panicked".to_owned());
            }
            execution
        })
    }))
    .unwrap_or_else(|_| {
        Err("haiku host owner panic after restoration gate; acknowledgement uncertain".into())
    });
    let snapshot = agent.finish().map_err(|e| e.to_string());
    let accounting = runtime.provider_history();
    let local = executor.block_on(runtime.shutdown());
    let fixture_joined = if execution.is_ok() {
        executor.block_on(server).is_ok()
    } else {
        server.abort();
        let _ = executor.block_on(server);
        true
    };
    {
        let mut state = bridge.state.lock().unwrap();
        state.runtime_joined = local.active == 0;
        state.accounting = accounting.clone().map(Arc::new);
    }
    let snapshot = snapshot?;
    let accounting = accounting.ok_or("provider accounting unavailable")?;
    let receipt = format!(
        "haiku-ui settlement: worker_terminated={} worker_joined={} runtime_joined={} fixture_joined={} ending_seen={} ending_acknowledged={} local_active={} consumed={} unknown_usage={} unresolved_remote={} process_acknowledged={} namespace_dead={:?} project_retained=true storage_retained=true fixture_delay_entered={}",
        snapshot.worker_terminated,
        snapshot.worker_joined,
        local.active == 0,
        fixture_joined,
        snapshot.history.ending_seen(),
        snapshot.history.finished(),
        local.active,
        accounting.consumed(),
        accounting.unknown_usage(),
        accounting.unresolved_remote(),
        snapshot.process_acknowledged,
        snapshot.observed_process.as_ref().map(|p| p.namespace_dead),
        delay_reached.load(std::sync::atomic::Ordering::Relaxed)
    );
    execution.map_err(|error| format!("{receipt}; {error}"))?;
    if !snapshot.worker_joined
        || !snapshot.worker_terminated
        || local.active != 0
        || !fixture_joined
        || !snapshot.history.finished()
    {
        return Err(format!("{receipt}; settlement incomplete"));
    }
    Ok(receipt)
}
fn recovered_accounting(path: &std::path::Path) -> Result<Option<Arc<ProviderHistory>>, String> {
    use rustix::fs::{FileType, Mode, OFlags};
    use std::{io::Read, path::Component};
    if !path.is_absolute() {
        return Err("accounting path must be absolute".into());
    }
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| e.to_string())?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = match rustix::fs::openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => fd,
                    Err(rustix::io::Errno::NOENT) => return Ok(None),
                    Err(error) => return Err(error.to_string()),
                };
            }
            _ => return Err("unsupported accounting path component".into()),
        }
    }
    let directory_stat = rustix::fs::fstat(&directory).map_err(|e| e.to_string())?;
    if directory_stat.st_uid != rustix::process::getuid().as_raw()
        || directory_stat.st_mode & 0o777 != 0o700
    {
        return Err("unsupported owned accounting directory".into());
    }
    let descriptor = match rustix::fs::openat(
        &directory,
        "provider-attempts.jsonl",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let stat = rustix::fs::fstat(&descriptor).map_err(|e| e.to_string())?;
    let record_budget = scramjet_rs::PROVIDER_FRAME_BYTES + 1024;
    let history_budget = (1 + 2 * scramjet_rs::PROVIDER_POST_BUDGET) * (record_budget + 1);
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != rustix::process::getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
        || stat.st_size < 0
        || stat.st_size as u64 > history_budget as u64
    {
        return Err("unsupported accounting header metadata or ledger budget".into());
    }
    rustix::fs::flock(
        &descriptor,
        rustix::fs::FlockOperation::NonBlockingLockShared,
    )
    .map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(record_budget + 1);
    std::fs::File::from(descriptor)
        .take((record_budget + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let end = bytes
        .iter()
        .position(|b| *b == b'\n')
        .ok_or("accounting header exceeds record budget or is incomplete")?;
    let header: serde_json::Value =
        serde_json::from_slice(&bytes[..end]).map_err(|_| "invalid accounting header")?;
    let endpoint = header["endpoint"]
        .as_str()
        .ok_or("accounting header lacks route")?
        .parse()
        .map_err(|_| "invalid recorded accounting route")?;
    // The recorded endpoint is data for the existing inert validator, never dispatch authority.
    scramjet_rs::reconstruct_provider_history(path, endpoint)
        .map(Arc::new)
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("requires TTY stdin and stdout".into());
    }
    execute_options(options(arguments)?)
}
fn execute_options(options: Options) -> Result<(), String> {
    if options.history {
        let history = reconstruct_haiku_history(&options.storage, &options.session)
            .map_err(|e| e.to_string())?;
        let accounting = recovered_accounting(
            &options
                .storage
                .join(format!("{}-provider", options.session)),
        )?;
        if let Some(accounting) = &accounting {
            for entry in &history.entries {
                if let scramjet_rs::HaikuRecord::Receipt { attempt } = entry.record.as_ref()
                    && let Some(ordinal) = attempt.ordinal
                    && accounting
                        .attempts
                        .iter()
                        .find(|fact| fact.ordinal == ordinal)
                        .is_none_or(|fact| fact.request_id != attempt.request_id)
                {
                    return Err("haiku/provider history request identity mismatch".into());
                }
            }
        }
        let bridge = recovered(history, accounting);
        let mut state = crate::ui::project_state(bridge.deadline).map_err(|e| e.to_string())?;
        state.mode(true);
        return crate::ui::run_haiku(&bridge, &mut state)
            .map(|_| ())
            .map_err(|e| e.to_string());
    }
    let fence = Arc::new(ReturnFence::default());
    let mut returned = TerminalReturn {
        fence: fence.clone(),
        bridge: None,
    };
    let (ready, receive) = mpsc::sync_channel(0);
    let worker = std::thread::Builder::new()
        .name("haiku-ui-host".into())
        .spawn(move || host(options, ready, fence))
        .map_err(|e| e.to_string())?;
    let bridge = match receive.recv() {
        Ok(bridge) => bridge,
        Err(_) => {
            drop(returned);
            return worker
                .join()
                .map_err(|_| "haiku initialization panic")?
                .map(|_| ());
        }
    };
    returned.bridge = Some(bridge.clone());
    let mut state = crate::ui::project_state(bridge.deadline);
    let terminal = state
        .as_mut()
        .map_err(|e| io::Error::other(e.to_string()))
        .and_then(|state| crate::ui::run_haiku(&bridge, state));
    let terminal_facts = state.as_ref().map_or_else(
        |_| crate::ui::TerminalFacts::default(),
        |state| state.terminal,
    );
    let browse = matches!(terminal, Ok(crate::ui::CodingReturn::Settlement));
    if !browse {
        bridge.stop();
    }
    let pending = (|| {
        let mut output = crate::ui::runtime_output()?;
        writeln!(
            output,
            "haiku-ui: terminal_returned=true terminal_result_ok={} settlement_pending=true terminal_entered={} restoration_attempted={} restoration_ok={:?} cursor_restoration_ok={:?}{}",
            terminal.is_ok(),
            terminal_facts.entered,
            terminal_facts.restoration_attempted,
            terminal_facts.restoration_ok,
            terminal_facts.cursor_restoration_ok,
            terminal
                .as_ref()
                .err()
                .map_or_else(String::new, |e| format!(" error={e}"))
        )?;
        output.flush()
    })();
    returned.bridge = None;
    drop(returned);
    while !worker.is_finished() {
        if state.as_ref().is_ok_and(|state| !state.live()) {
            bridge.stop();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = worker
        .join()
        .map_err(|_| "haiku host panic after terminal return")?;
    let mut output = crate::ui::runtime_output().map_err(|e| e.to_string())?;
    writeln!(
        output,
        "terminal_returned=true terminal_result_ok={} {}",
        terminal.is_ok(),
        result.as_ref().map_or_else(|e| e.as_str(), |r| r.as_str())
    )
    .map_err(|e| e.to_string())?;
    crate::ui::write_haiku_facts(&bridge, &mut output)
        .map_err(|e| format!("complete observed fact report failed; no retry: {e}"))?;
    output.flush().map_err(|e| e.to_string())?;
    pending.map_err(|e| format!("restoration pending receipt write failed; no retry: {e}"))?;
    result?;
    terminal.map_err(|e| e.to_string())?;
    if browse
        && let Ok(state) = &mut state
        && state.live()
    {
        state.mode(true);
        crate::ui::run_haiku(&bridge, state).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "haiku_ui_demo_tests.rs"]
mod tests;
