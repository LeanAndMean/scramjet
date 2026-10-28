use crate::project_fixture::{Fixture, Mode};
use scramjet_rs::*;
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Intent {
    Request,
    Verify,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Offer {
    pub instance: u64,
    pub journey: u8,
    pub intent: Intent,
    pub revision: u64,
}
enum Action {
    Request(String),
    Verify,
}
pub(crate) struct RetainedHistory {
    pub coding: [Option<Arc<CodingHistory>>; 2],
}
struct State {
    observer: ProjectObserver,
    control: ProjectControl,
    sender: mpsc::Sender<Action>,
    offer: Option<Offer>,
    painted: Option<Offer>,
    stopped: bool,
    error: Option<String>,
    rejection: Option<&'static str>,
    history: Option<Arc<RetainedHistory>>,
}
pub(crate) struct Bridge {
    state: Mutex<State>,
    settlement: Mutex<Option<ProjectSettlement>>,
    pub deadline: Instant,
}
pub(crate) struct Projection {
    pub snapshot: ProjectSnapshot,
    pub offer: Option<Offer>,
    pub error: Option<String>,
    pub rejection: Option<&'static str>,
    pub history: Option<Arc<RetainedHistory>>,
}
impl Bridge {
    pub fn snapshot(&self) -> Projection {
        let state = self.state.lock().unwrap();
        Projection {
            snapshot: state.observer.snapshot(),
            offer: state.offer,
            error: state.error.clone(),
            rejection: state.rejection,
            history: state.history.clone(),
        }
    }
    pub fn painted(&self, offer: Option<Offer>) {
        let mut state = self.state.lock().unwrap();
        state.painted = offer.filter(|offer| {
            state.offer.is_some_and(|current| {
                (current.instance, current.journey, current.intent)
                    == (offer.instance, offer.journey, offer.intent)
            }) && !state.stopped
                && Instant::now() < self.deadline
        });
    }
    pub fn invoke(&self, ingress: Option<Offer>, revision: u64, request: &str) {
        let mut state = self.state.lock().unwrap();
        let Some(offer) = ingress else {
            state.rejection = Some("UnpaintedIntent");
            return;
        };
        if state.stopped || Instant::now() >= self.deadline {
            state.rejection = Some("StoppedOrDeadline");
            return;
        }
        if state.painted != Some(offer)
            || state.offer.is_none_or(|current| {
                (current.instance, current.journey, current.intent)
                    != (offer.instance, offer.journey, offer.intent)
            })
            || (offer.intent == Intent::Request && revision != offer.revision)
        {
            state.rejection = Some("StaleIntentOrRevision");
            return;
        }
        let action = match offer.intent {
            Intent::Request => {
                if ProjectRequest::parse(request).is_err() {
                    state.rejection = Some("UnsupportedRequest");
                    return;
                }
                Action::Request(request.to_owned())
            }
            Intent::Verify => Action::Verify,
        };
        if state.sender.try_send(action).is_err() {
            state.rejection = Some("CommandOwnerUnavailable");
            return;
        }
        state.offer = None;
        state.painted = None;
        state.rejection = None;
    }
    pub fn reject(&self) {
        self.state.lock().unwrap().rejection =
            Some("FocusBrowseHistoryApprovalViewportOrExhaustion");
    }
    pub fn decide(&self, proposal: &CodingProposal, decision: CodingDecision) {
        let state = self.state.lock().unwrap();
        if !state.stopped && Instant::now() < self.deadline {
            let _ = state.control.decide(proposal, decision);
        }
    }
    pub fn cancel_child(&self, child: u8) {
        let state = self.state.lock().unwrap();
        let snapshot = state.observer.snapshot();
        if let Some(id) = snapshot.current
            && let Some(readers) = &snapshot.journeys[id.journey() as usize - 1].readers
        {
            let _ = state
                .control
                .cancel_child(&readers.session, readers.generation, child);
        }
    }
    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.stopped = true;
        state.offer = None;
        state.painted = None;
        state.control.stop();
    }
    fn release_host(&self) {
        if let Some(settlement) = self.settlement.lock().unwrap().as_ref() {
            settlement.release_host();
        }
    }
    fn offer(&self, journey: u8, intent: Intent) {
        let mut state = self.state.lock().unwrap();
        if !state.stopped && Instant::now() < self.deadline {
            state.offer = Some(Offer {
                instance: state.observer.snapshot().instance,
                journey,
                intent,
                revision: 0,
            });
        }
    }
    fn fail(&self, error: impl ToString) {
        let mut state = self.state.lock().unwrap();
        state.error = Some(error.to_string());
        state.offer = None;
        state.painted = None;
    }
}
struct Options {
    project: PathBuf,
    storage: PathBuf,
    session: String,
    mode: Mode,
}
fn options(arguments: impl Iterator<Item = OsString>) -> Result<Options, String> {
    let mut arguments = arguments;
    let (mut project, mut storage, mut session, mut mode) = (None, None, None, None);
    for _ in 0..4 {
        let Some(name) = arguments.next() else {
            break;
        };
        let value = arguments
            .next()
            .ok_or("missing project-ui-demo option value")?;
        if value.len() > 4096 {
            return Err("option byte ceiling".into());
        }
        match name.to_str() {
            Some("--project") if project.is_none() => project = Some(PathBuf::from(value)),
            Some("--storage") if storage.is_none() => storage = Some(PathBuf::from(value)),
            Some("--session") if session.is_none() => {
                session = Some(value.into_string().map_err(|_| "session must be UTF8")?)
            }
            Some("--fixture") if mode.is_none() => {
                mode = Some(match value.to_str() {
                    Some("golden") => Mode::Golden,
                    Some("reader-failure") => Mode::ReaderFailure(1),
                    Some("model-failure") => Mode::ModelFailure(2),
                    Some("unknown-usage") => Mode::UnknownUsage,
                    _ => return Err("unsupported offline fixture".into()),
                })
            }
            _ => return Err("unsupported/duplicate project-ui-demo option".into()),
        }
    }
    if arguments.next().is_some() {
        return Err("option count ceiling".into());
    }
    let session = session.unwrap_or_else(|| "project-ui".into());
    if session.is_empty()
        || session.len() > 64
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err("invalid bounded session".into());
    }
    Ok(Options {
        project: project.ok_or("--project is required")?,
        storage: storage.ok_or("--storage is required")?,
        session,
        mode: mode.unwrap_or(Mode::Golden),
    })
}
async fn source(
    session: &mut ProjectSession,
    request: &str,
) -> Result<Option<ProjectContinuation>, String> {
    let mut receipt = session.submit(request).await.map_err(|e| e.to_string())?;
    let snapshot = session.observer().snapshot();
    let coding = snapshot
        .records
        .iter()
        .rev()
        .find_map(|r| match r.as_ref() {
            ProjectRecord::Coding(c) => Some(c),
            _ => None,
        })
        .ok_or("source receipt without coding fact")?;
    let completion = match coding.completion.as_str() {
        "Completed" => CommandCompletion::Completed,
        "Failed" => CommandCompletion::Failed,
        _ => CommandCompletion::Incomplete,
    };
    session
        .report_source(
            &mut receipt,
            completion,
            coding
                .summary
                .as_deref()
                .unwrap_or("source did not establish success"),
        )
        .await
        .map_err(|e| e.to_string())
}
async fn verify(
    session: &mut ProjectSession,
    continuation: ProjectContinuation,
) -> Result<bool, String> {
    let ready = session
        .resume(continuation)
        .await
        .map_err(|e| e.to_string())?;
    let mut receipt = session.verify(ready).await.map_err(|e| e.to_string())?;
    let snapshot = session.observer().snapshot();
    let matched = snapshot
        .records
        .iter()
        .rev()
        .find_map(|r| match r.as_ref() {
            ProjectRecord::VerificationRead(read) => Some(read.matched),
            _ => None,
        })
        .ok_or("verification receipt without read fact")?;
    session
        .report_verification(
            &mut receipt,
            if matched {
                CommandCompletion::Completed
            } else {
                CommandCompletion::Failed
            },
            if matched {
                "full edited target identity/content matched"
            } else {
                "edited target verification failed"
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(matched)
}
async fn history(bridge: &Bridge, storage: PathBuf) -> Result<(), String> {
    let snapshot = bridge.snapshot().snapshot;
    let journey = snapshot
        .current
        .ok_or("history lacks live invocation")?
        .journey();
    let captured = snapshot
        .records
        .iter()
        .find_map(|r| match r.as_ref() {
            ProjectRecord::Captured(c) if c.journey == journey => Some(c),
            _ => None,
        })
        .ok_or("history lacks acknowledged capture")?;
    let root = snapshot
        .records
        .iter()
        .find_map(|r| match r.as_ref() {
            ProjectRecord::Header(h) => Some(h.project),
            _ => None,
        })
        .ok_or("history lacks project identity")?;
    let session = captured.coding_session.clone();
    let history = tokio::task::spawn_blocking(move || {
        ProjectWorkspace::coding_history(&storage, journey, &session, root)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let mut state = bridge.state.lock().unwrap();
    let mut coding = state
        .history
        .as_ref()
        .map_or_else(|| std::array::from_fn(|_| None), |h| h.coding.clone());
    coding[journey as usize - 1] = Some(Arc::new(history));
    state.history = Some(Arc::new(RetainedHistory { coding }));
    Ok(())
}
async fn host(
    options: Options,
    ready: std::sync::mpsc::SyncSender<Arc<Bridge>>,
    mut returned: oneshot::Receiver<()>,
) -> Result<String, String> {
    let fixture = Fixture::start(options.mode).await;
    let runtime = Runtime::new(
        fixture.endpoint,
        2,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .map_err(|_| "runtime owner unavailable")?;
    let (mut session, settlement) = ProjectSession::start(
        runtime,
        &options.project,
        &options.storage,
        &options.session,
    );
    let (sender, mut actions) = mpsc::channel(1);
    let bridge = Arc::new(Bridge {
        state: Mutex::new(State {
            observer: session.observer(),
            control: session.control(),
            sender,
            offer: None,
            painted: None,
            stopped: false,
            error: None,
            rejection: None,
            history: None,
        }),
        settlement: Mutex::new(Some(settlement)),
        deadline: Instant::now() + Duration::from_secs(120),
    });
    if ready.send(bridge.clone()).is_err() {
        bridge.stop();
        bridge.release_host();
    }
    bridge.offer(1, Intent::Request);
    let mut continuation = None;
    let work = async {
        loop {
            let action = tokio::select! { biased;
                _ = &mut returned => break,
                action = actions.recv() => action.ok_or("input owner lost")?,
            };
            match action {
                Action::Request(request) => {
                    continuation = source(&mut session, &request).await?;
                    history(&bridge, options.storage.clone()).await?;
                    if continuation.is_some() {
                        bridge.offer(
                            session.observer().snapshot().current.unwrap().journey(),
                            Intent::Verify,
                        );
                    }
                }
                Action::Verify => {
                    let matched = verify(
                        &mut session,
                        continuation.take().ok_or("live continuation unavailable")?,
                    )
                    .await?;
                    let journey = session.observer().snapshot().current.unwrap().journey();
                    if matched && journey < 2 {
                        bridge.offer(journey + 1, Intent::Request);
                    }
                }
            }
        }
        Ok::<_, String>(())
    }
    .await;
    if let Err(error) = &work {
        bridge.fail(error);
        let _ = returned.await;
    }
    bridge.stop();
    let settlement = bridge.settlement.lock().unwrap().take().unwrap();
    let finish = settlement.finish().await.map_err(|e| e.to_string())?;
    let snapshot = session.observer().snapshot();
    let fixture = fixture.shutdown().await;
    fixture.attest(&snapshot).map_err(str::to_owned)?;
    let after = Runtime::process_snapshot();
    let receipt = format!(
        "project-ui settlement: logical={} readers={} coding={} storage={} runtime={} history={} phase={:?} attempts={} fixture_requests={} fixture_peak={} fixture_joined={} unknown_before_fixture={} exposure_before_fixture={} unknown_after_fixture={} exposure_after_fixture={} handles={} active={} pending={} project_retained=true storage_retained=true",
        finish.logical_settled,
        finish.readers_joined,
        finish.coding_joined,
        finish.storage_joined,
        finish.runtime_joined,
        finish.history_acknowledged,
        snapshot.phase,
        finish.runtime.attempts,
        fixture.requests.len(),
        fixture.peak,
        fixture.joined,
        finish.runtime.unknown_usage,
        finish.runtime.unresolved,
        after.unknown_usage,
        after.unresolved,
        after.handles,
        after.active,
        after.pending
    );
    if let Some(error) = finish.error {
        return Err(format!("{receipt}; {error}"));
    }
    if !fixture.joined
        || !finish.readers_joined
        || !finish.coding_joined
        || !finish.storage_joined
        || !finish.runtime_joined
        || !finish.history_acknowledged
        || after.handles != 0
        || after.active != 0
        || after.pending != 0
        || after.unknown_usage != 0
        || after.unresolved != 0
    {
        return Err(format!("{receipt}; local settlement incomplete"));
    }
    if let Err(error) = work {
        return Err(format!("{receipt}; {error}"));
    }
    Ok(receipt)
}
pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("requires TTY stdin and stdout".into());
    }
    let options = options(arguments)?;
    let (ready, receive) = std::sync::mpsc::sync_channel(1);
    let (returned, return_notice) = oneshot::channel();
    let worker = std::thread::Builder::new()
        .name("project-ui-host".into())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?
                .block_on(host(options, ready, return_notice))
        })
        .map_err(|e| e.to_string())?;
    let bridge = match receive.recv() {
        Ok(bridge) => bridge,
        Err(_) => return worker.join().map_err(|_| "project host panic")?.map(|_| ()),
    };
    let mut state = crate::ui::project_state(bridge.deadline);
    let terminal = match &mut state {
        Ok(state) => crate::ui::run_project(&bridge, state),
        Err(error) => Err(io::Error::other(error.to_string())),
    };
    let browse_after_settlement = matches!(terminal, Ok(crate::ui::CodingReturn::Settlement));
    if !browse_after_settlement {
        bridge.stop();
    }
    bridge.release_host();
    let _ = returned.send(());
    while !worker.is_finished() {
        if state.as_ref().is_ok_and(|state| !state.live()) {
            bridge.stop();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = worker
        .join()
        .map_err(|_| "project host panic after terminal return")?;
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(error) => {
            let mut out = crate::ui::runtime_output().map_err(|e| e.to_string())?;
            writeln!(
                out,
                "terminal_returned=true terminal_result_ok={} {error}",
                terminal.is_ok()
            )
            .map_err(|e| e.to_string())?;
            return Err(error);
        }
    };
    let mut output = crate::ui::runtime_output().map_err(|e| e.to_string())?;
    writeln!(
        output,
        "terminal_returned=true terminal_result_ok={} {receipt}",
        terminal.is_ok()
    )
    .map_err(|e| e.to_string())?;
    output.flush().map_err(|e| e.to_string())?;
    terminal.map_err(|e| e.to_string())?;
    if browse_after_settlement
        && let Ok(state) = &mut state
        && state.live()
    {
        state.mode(true);
        crate::ui::run_project(&bridge, state).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "project_ui_demo_tests.rs"]
mod tests;
