use crate::haiku_demo::fixture;
use crate::provider_demo::server;
use scramjet_rs::{
    HaikuAgent, HaikuControl, HaikuHistory, HaikuObserver, HaikuProposal, HaikuSession,
    HaikuSnapshot, Runtime, haiku_prompt, reconstruct_haiku_history,
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
}
impl Bridge {
    pub fn snapshot(&self) -> Projection {
        let s = self.state.lock().unwrap();
        Projection {
            snapshot: s.observer.as_ref().map_or_else(
                || s.recovered.as_ref().unwrap().clone(),
                HaikuObserver::snapshot,
            ),
            offered: s.offered && !s.stopped && Instant::now() < self.deadline,
            rejection: s.rejection,
            error: s.error.clone(),
            runtime_joined: s.runtime_joined,
        }
    }
    pub fn painted(&self, offer: Option<Offer>) {
        let mut s = self.state.lock().unwrap();
        s.painted = offer.filter(|_| s.offered && !s.stopped && Instant::now() < self.deadline);
    }
    pub fn submit(&self, offer: Option<Offer>, revision: u64, request: &str) {
        let mut s = self.state.lock().unwrap();
        if !s.offered
            || s.stopped
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
        if !s.stopped
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
    })
}
fn recovered(history: Arc<HaikuHistory>) -> Arc<Bridge> {
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
    let (endpoint, server) = executor.block_on(server::start_sequence(fixture::sequence(
        poem,
        0,
        "opaque fixture reasoning",
    )));
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
        }),
    });
    let gate = HostFence(fence.clone());
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
            if bridge.state.lock().unwrap().stopped || Instant::now() >= deadline {
                return Err("stopped before submission".to_owned());
            }
            match actions.try_recv() {
                Ok(()) => break,
                Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
                Err(mpsc::TryRecvError::Disconnected) => return Err("input owner lost".to_owned()),
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
    drop(gate);
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
    bridge.state.lock().unwrap().runtime_joined = local.active == 0;
    let snapshot = snapshot?;
    let accounting = accounting.ok_or("provider accounting unavailable")?;
    let receipt = format!(
        "haiku-ui settlement: worker_terminated={} worker_joined={} runtime_joined={} fixture_joined={} ending_seen={} ending_acknowledged={} local_active={} consumed={} unknown_usage={} unresolved_remote={} process_acknowledged={} namespace_dead={:?} project_retained=true storage_retained=true",
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
        snapshot.observed_process.as_ref().map(|p| p.namespace_dead)
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
pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("requires TTY stdin and stdout".into());
    }
    let options = options(arguments)?;
    if options.history {
        let bridge = recovered(
            reconstruct_haiku_history(&options.storage, &options.session)
                .map_err(|e| e.to_string())?,
        );
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
    let browse = matches!(terminal, Ok(crate::ui::CodingReturn::Settlement));
    if !browse {
        bridge.stop();
    }
    let pending = (|| {
        let mut output = crate::ui::runtime_output()?;
        writeln!(
            output,
            "haiku-ui: terminal_returned=true terminal_result_ok={} settlement_pending=true{}",
            terminal.is_ok(),
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
