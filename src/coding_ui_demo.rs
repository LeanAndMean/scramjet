use crate::{
    coding_ui_fixture::{self as fixture, Fixture, Report},
    ui,
};
#[cfg(test)]
use scramjet_rs::reconstruct_coding;
use scramjet_rs::{
    CodingAgent, CodingCapability, CodingControl, CodingDecision, CodingFollowUpAdmission,
    CodingHistory, CodingObservation, CodingObserver, CodingProposal, CodingSession,
    CommandCodingAdapter, CommandCodingReceipt, CommandCompletion, CommandControl, CommandObserver,
    CommandPolicy, CommandSession, CommandSnapshot, EditCancellation, OperationState, Runtime,
    Snapshot, reconstruct_coding_history,
};
use std::{
    io::{self, IsTerminal, Write},
    os::unix::fs::DirBuilderExt,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;
#[cfg(test)]
#[path = "coding_ui_demo_tests.rs"]
mod tests;

#[derive(Clone, Copy, Default)]
pub(crate) struct Intent {
    submitted: bool,
    stop: bool,
    restored: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Offer {
    pub id: u64,
    pub revision: u64,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct SubmissionReceipt {
    pub offer: Offer,
    pub bytes: usize,
}
#[derive(Default)]
struct Submission {
    painted: Option<Offer>,
    receipt: Option<SubmissionReceipt>,
    prompt: Option<Arc<str>>,
    stop: bool,
    control: Option<CodingControl>,
    lifecycle: Option<(Arc<std::sync::atomic::AtomicBool>, std::time::Instant)>,
    admission: Option<CodingFollowUpAdmission>,
    retired: bool,
    root_cancellation: Option<EditCancellation>,
    command: Option<Arc<CommandContext>>,
    #[cfg(test)]
    fail_after_handoff: bool,
}
static OFFERS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
#[derive(Clone, Debug, Default)]
pub(crate) struct Projection {
    pub offer: Option<u64>,
    pub journey: u64,
    pub session: Option<Arc<str>>,
    pub submission: Option<SubmissionReceipt>,
    pub rejection: Option<(Option<Offer>, &'static str)>,
    pub observation: Option<Arc<CodingObservation>>,
    pub pre: Option<Snapshot>,
    pub post: Option<Snapshot>,
    pub fixture: Option<Report>,
    pub ended: bool,
    pub needs_settlement: bool,
    pub needs_checkpoint: bool,
    pub follow_up: bool,
    pub invocation: u64,
    pub checkpoint: Option<Arc<CodingObservation>>,
    pub first_submission: Option<SubmissionReceipt>,
    pub previous: Option<Arc<Projection>>,
    pub reconstructed: bool,
    pub durable_effect: Option<OperationState>,
    pub history: Option<Arc<CodingHistory>>,
    pub history_error: Option<&'static str>,
    pub settlement_failed: bool,
    pub error: Option<String>,
    pub retained_root: Option<std::path::PathBuf>,
    pub command: Option<CommandSnapshot>,
}
impl Projection {
    pub(crate) fn settled(&self) -> bool {
        self.ended
            && !self.settlement_failed
            && self.reconstructed
            && self.history.is_some()
            && self.retained_root.is_none()
            && self.fixture.is_some()
            && self.post.is_some()
            && self
                .observation
                .as_ref()
                .is_some_and(|o| o.worker_terminated && o.worker_joined)
    }
    fn error(&mut self, message: &str) {
        self.settlement_failed = true;
        self.execution_error(message);
    }
    fn execution_error(&mut self, message: &str) {
        if self.error.is_some() {
            return;
        }
        let mut end = message.len().min(4096);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        self.error = Some(message[..end].into());
    }
}
#[derive(Clone)]
pub(crate) struct Bridge {
    store: Arc<Mutex<Projection>>,
    intent: watch::Sender<Intent>,
    observer: Arc<Mutex<Option<CodingObserver>>>,
    submission: Arc<Mutex<Submission>>,
    offer: u64,
    session: Arc<str>,
    current: Arc<Mutex<Option<Execution>>>,
}
#[derive(Clone)]
struct Execution {
    intent: watch::Receiver<Intent>,
    store: Arc<Mutex<Projection>>,
    observer: Arc<Mutex<Option<CodingObserver>>>,
    submission: Arc<Mutex<Submission>>,
    session: Arc<str>,
    command: Option<Arc<CommandContext>>,
}
struct CommandContext {
    session: Mutex<Option<CommandSession>>,
    observer: CommandObserver,
    control: CommandControl,
    roots: Mutex<crate::command_fixture::Roots>,
}
impl Execution {
    fn live(&self) -> bool {
        let s = self.submission.lock().unwrap();
        !s.stop
            && s.lifecycle.as_ref().is_none_or(|(flag, deadline)| {
                !flag.load(std::sync::atomic::Ordering::Relaxed)
                    && std::time::Instant::now() < *deadline
            })
    }
    fn attach(&self, control: CodingControl) {
        let mut state = self.submission.lock().unwrap();
        if state.stop {
            control.stop();
        }
        state.control = Some(control);
    }
    fn snapshot(&self) -> Projection {
        self.store.lock().unwrap().clone()
    }
}
impl Bridge {
    fn execution(&self) -> Execution {
        Execution {
            intent: self.intent.subscribe(),
            store: self.store.clone(),
            submission: self.submission.clone(),
            observer: self.observer.clone(),
            session: self.session.clone(),
            command: self.submission.lock().unwrap().command.clone(),
        }
    }
    pub(crate) fn new() -> (Self, watch::Receiver<Intent>) {
        let (intent, receive) = watch::channel(Intent::default());
        let offer = OFFERS
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_add(1).filter(|n| *n <= u64::MAX / 256),
            )
            .expect("submission identity exhausted");
        let session: Arc<str> = format!("coding-ui-{offer}").into();
        let bridge = Self {
            store: Arc::new(Mutex::new(Projection {
                offer: Some(offer),
                journey: offer,
                invocation: 1,
                session: Some(session.clone()),
                ..Projection::default()
            })),
            intent,
            submission: Arc::new(Mutex::new(Submission::default())),
            offer,
            session,
            observer: Arc::new(Mutex::new(None)),
            current: Arc::new(Mutex::new(None)),
        };
        *bridge.current.lock().unwrap() = Some(bridge.execution());
        (bridge, receive)
    }
    fn new_command() -> io::Result<(Self, watch::Receiver<Intent>)> {
        let roots = crate::command_fixture::Roots::new(
            r#"{"commands":[{"name":"replace","outcome":"ReplaceVerified","target":"sample.txt"}]}"#,
            b"",
            b"",
        )?;
        let session = CommandSession::load(&roots.definitions, &roots.inputs, CommandPolicy::Pause)
            .map_err(io::Error::other)?;
        let observer = session.observer();
        let context = Arc::new(CommandContext {
            control: session.control(),
            observer,
            session: Mutex::new(Some(session)),
            roots: Mutex::new(roots),
        });
        let (mut bridge, intent) = Self::new();
        bridge.session = Arc::from(format!("command-{}-1", context.observer.session()));
        {
            let mut store = bridge.store.lock().unwrap();
            store.session = Some(bridge.session.clone());
            store.command = Some(context.observer.snapshot());
        }
        bridge.submission.lock().unwrap().command = Some(context);
        *bridge.current.lock().unwrap() = Some(bridge.execution());
        Ok((bridge, intent))
    }
    pub(crate) fn command_mode(&self) -> bool {
        self.submission.lock().unwrap().command.is_some()
    }
    fn continuation(
        &self,
        admission: CodingFollowUpAdmission,
        observer: CodingObserver,
        control: CodingControl,
    ) -> (Self, watch::Receiver<Intent>) {
        let (mut next, intent) = Self::new();
        next.session = self.session.clone();
        {
            let mut old = self.submission.lock().unwrap();
            let mut new = next.submission.lock().unwrap();
            old.retired = true;
            old.painted = None;
            old.control = None;
            new.lifecycle = old.lifecycle.clone();
            new.admission = Some(admission);
            new.control = Some(control);
            new.root_cancellation = old.root_cancellation.take();
        }
        *self.observer.lock().unwrap() = None;
        *next.observer.lock().unwrap() = Some(observer.clone());
        {
            let mut old = self.store.lock().unwrap();
            let mut new = next.store.lock().unwrap();
            new.session = Some(self.session.clone());
            new.follow_up = true;
            new.invocation = 2;
            new.checkpoint = Some(observer.snapshot());
            new.observation = new.checkpoint.clone();
            new.first_submission = old.submission;
            new.retained_root = old.retained_root.take();
        }
        next.current = self.current.clone();
        *next.current.lock().unwrap() = Some(next.execution());
        (next, intent)
    }
    pub fn restored(&self) {
        self.intent.send_modify(|i| i.restored = true);
    }
    pub fn restoration_failed(&self) {
        self.store
            .lock()
            .unwrap()
            .error("terminal interval/restoration failed; replacement suppressed");
        self.restored();
    }
    pub fn bind_lifecycle(
        &self,
        stopped: Arc<std::sync::atomic::AtomicBool>,
        deadline: std::time::Instant,
    ) {
        self.submission.lock().unwrap().lifecycle = Some((stopped, deadline));
    }
    pub fn painted(&self, offer: Option<Offer>) {
        self.submission.lock().unwrap().painted = offer;
    }
    pub fn reject(&self, offer: Option<Offer>, reason: &'static str) {
        self.store.lock().unwrap().rejection = Some((offer, reason));
    }
    pub fn submit(&self, offered: Option<Offer>, revision: u64, prompt: &str) -> bool {
        let mut state = self.submission.lock().unwrap();
        let reason = if state.lifecycle.as_ref().is_some_and(|(stopped, deadline)| {
            stopped.load(std::sync::atomic::Ordering::Relaxed)
                || std::time::Instant::now() >= *deadline
        }) {
            Some("LifecycleStoppedOrExpired")
        } else if state.stop || state.retired {
            Some("StoppedOrRetired")
        } else if state.receipt.is_some() {
            Some("AlreadyQueued")
        } else if offered.is_none()
            || offered != state.painted
            || offered
                != Some(Offer {
                    id: self.offer,
                    revision,
                })
        {
            Some("StaleOrUnpainted")
        } else if prompt.len() > 1024 {
            Some("PromptByteLimit")
        } else if prompt.trim().is_empty() {
            Some("EmptyDraft")
        } else if state
            .admission
            .as_ref()
            .map_or_else(
                || CodingAgent::validate_prompt(&self.session, prompt),
                |admission| admission.validate(prompt),
            )
            .is_err()
        {
            Some("PromptControlOrContextLimit")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.reject(offered, reason);
            return false;
        }
        let receipt = SubmissionReceipt {
            offer: offered.unwrap(),
            bytes: prompt.len(),
        };
        let frozen = if let Some(command) = &state.command {
            let mut session = command.session.lock().unwrap();
            let Some(session) = session.as_mut() else {
                self.reject(offered, "CommandFinalized");
                return false;
            };
            if session.begin_coding("replace", prompt).is_err() {
                self.reject(offered, "CommandAdmissionRejected");
                return false;
            }
            command.observer.snapshot().invocations[0]
                .as_ref()
                .unwrap()
                .coding
                .as_ref()
                .unwrap()
                .input
                .clone()
        } else {
            Arc::from(prompt)
        };
        state.receipt = Some(receipt);
        state.prompt = Some(frozen);
        {
            let mut store = self.store.lock().unwrap();
            store.submission = Some(receipt);
            store.offer = None;
        }
        self.intent.send_modify(|i| i.submitted = true);
        true
    }
    pub fn stop(&self) {
        let mut state = self.submission.lock().unwrap();
        if state.retired {
            return;
        }
        state.stop = true;
        if let Some(command) = &state.command {
            command.control.cancel();
        }
        if let Some(root) = &state.root_cancellation {
            root.cancel();
        }
        if let Some(control) = state.control.as_ref() {
            control.stop();
        }
        self.intent.send_modify(|i| i.stop = true);
    }
    pub fn decide(&self, proposal: &CodingProposal, decision: CodingDecision) {
        let state = self.submission.lock().unwrap();
        if let Some(control) = state.control.as_ref() {
            if state.retired {
                return;
            }
            if state.stop
                || state.lifecycle.as_ref().is_some_and(|(stopped, deadline)| {
                    stopped.load(std::sync::atomic::Ordering::Relaxed)
                        || std::time::Instant::now() >= *deadline
                })
            {
                if let Some(command) = &state.command {
                    command.control.cancel();
                }
                control.stop();
            } else {
                let _ = control.decide(proposal, decision);
            }
        }
    }
    pub fn snapshot(&self) -> Projection {
        let mut projection = self.store.lock().unwrap().clone();
        if let Some(observer) = self.observer.lock().unwrap().clone() {
            projection.observation = Some(observer.snapshot());
        }
        if let Some(command) = &self.submission.lock().unwrap().command {
            projection.command = Some(command.observer.snapshot());
        }
        projection
    }
}
async fn wait_restoration(bridge: &Execution, intent: &mut watch::Receiver<Intent>) {
    loop {
        if intent.borrow_and_update().restored {
            return;
        }
        if intent.changed().await.is_err() {
            bridge
                .store
                .lock()
                .unwrap()
                .error("terminal restoration acknowledgement unavailable");
            return;
        }
    }
}
async fn protected<F: std::future::Future>(future: F) -> Result<F::Output, ()> {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Ok(std::task::Poll::Ready(value)) => std::task::Poll::Ready(Ok(value)),
            Err(_) => std::task::Poll::Ready(Err(())),
        }
    })
    .await
}
async fn journey(
    bridge: Execution,
    intent: watch::Receiver<Intent>,
    replies: Option<Vec<fixture::Reply>>,
) {
    execute_journey(bridge, intent, replies, None).await;
}
fn new_runtime(fixture: &Fixture) -> io::Result<Runtime> {
    Runtime::new(
        fixture.address,
        1,
        scramjet_rs::RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        scramjet_rs::SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        scramjet_rs::ConcurrencyMode::Fixed,
    )
    .map_err(|_| io::Error::other("runtime busy; second submission unavailable; no retry"))
}
async fn execute_journey(
    bridge: Execution,
    intent: watch::Receiver<Intent>,
    replies: Option<Vec<fixture::Reply>>,
    prepared: Option<(Fixture, Runtime)>,
) {
    execute_owned(bridge, intent, replies, prepared, None).await;
}
enum CodingOwner {
    Agent(CodingAgent),
    Command(CommandCodingAdapter),
}
impl CodingOwner {
    fn observer(&self) -> CodingObserver {
        match self {
            Self::Agent(a) => a.observer(),
            Self::Command(a) => a.observer(),
        }
    }
    fn control(&self) -> CodingControl {
        match self {
            Self::Agent(a) => a.control(),
            Self::Command(a) => a.control(),
        }
    }
    async fn run_unjoined(&mut self, runtime: &Runtime) -> Result<(), scramjet_rs::CodingError> {
        match self {
            Self::Agent(a) => a.run_unjoined(runtime).await.map(|_| ()),
            Self::Command(a) => {
                a.run_unjoined(runtime)
                    .await
                    .map_err(|_| scramjet_rs::CodingError::Stale)?;
                match a.result().unwrap() {
                    Ok(_) => Ok(()),
                    Err(scramjet_rs::CodingError::Denied) => Err(scramjet_rs::CodingError::Denied),
                    Err(scramjet_rs::CodingError::Cancelled) => {
                        Err(scramjet_rs::CodingError::Cancelled)
                    }
                    Err(_) => Err(scramjet_rs::CodingError::Invalid(
                        "command execution failed; actual reason retained in receipt",
                    )),
                }
            }
        }
    }
    fn validate_follow_up(&self, input: &str) -> Result<(), scramjet_rs::CodingError> {
        match self {
            Self::Agent(a) => a.validate_follow_up(input),
            Self::Command(_) => Err(scramjet_rs::CodingError::Stale),
        }
    }
    async fn follow_up_admission(
        &self,
    ) -> Result<CodingFollowUpAdmission, scramjet_rs::CodingError> {
        match self {
            Self::Agent(a) => a.follow_up_admission().await,
            Self::Command(_) => Err(scramjet_rs::CodingError::Stale),
        }
    }
    async fn follow_up(
        &mut self,
        runtime: &Runtime,
        input: &str,
    ) -> Result<(), scramjet_rs::CodingError> {
        match self {
            Self::Agent(a) => a.follow_up(runtime, input).await,
            Self::Command(_) => Err(scramjet_rs::CodingError::Stale),
        }
    }
    fn finish(self) -> Result<Option<CommandCodingReceipt>, scramjet_rs::CodingError> {
        match self {
            Self::Agent(a) => a.finish().map(|()| None),
            Self::Command(a) => Ok(Some(a.finish())),
        }
    }
}
fn finish_command(bridge: &Execution, mut receipt: Option<CommandCodingReceipt>) {
    let Some(context) = &bridge.command else {
        return;
    };
    if !bridge.live() {
        context.control.cancel();
    }
    let mut owner = context.session.lock().unwrap();
    let Some(mut session) = owner.take() else {
        return;
    };
    if context.roots.lock().unwrap().cleanup().is_err() {
        bridge
            .store
            .lock()
            .unwrap()
            .error("command definitions cleanup failed");
    }
    if let Some(receipt) = receipt.as_mut() {
        if session.accept_coding(receipt).is_err() {
            bridge
                .store
                .lock()
                .unwrap()
                .error("command receipt rejected; no accepted success");
        } else {
            let snapshot = session.snapshot();
            let fact = snapshot.invocations[0].as_ref().unwrap();
            let disposition = if snapshot.cancellation_requested {
                CommandCompletion::Cancelled
            } else if bridge.snapshot().settlement_failed {
                CommandCompletion::Incomplete
            } else {
                fact.coding
                    .as_ref()
                    .unwrap()
                    .execution
                    .as_ref()
                    .unwrap()
                    .disposition
            };
            if session.report(fact.id, disposition, match disposition {
                CommandCompletion::Completed => "exact submitted replacement durably verified and recorded Final",
                CommandCompletion::Failed => "coding goal, approval or execution failed; see retained execution evidence",
                CommandCompletion::Cancelled => "explicit command cancellation; actual effect retained independently",
                CommandCompletion::Incomplete => "execution or settlement evidence incomplete; no replay or success inferred",
            }, None).is_err() {
                bridge.store.lock().unwrap().error("command report rejected; no accepted success");
            }
        }
    } else if let Some(fact) = session.snapshot().invocations[0].as_ref()
        && context.observer.snapshot().cancellation_requested
        && fact.coding.as_ref().is_some_and(|c| c.owner.is_none())
    {
        let _ = session.report(
            fact.id,
            CommandCompletion::Cancelled,
            "cancelled before coding launch; no worker or effect",
            None,
        );
    }
    drop(receipt);
    bridge.store.lock().unwrap().command = Some(session.finalize());
}
async fn execute_owned(
    mut bridge: Execution,
    mut intent: watch::Receiver<Intent>,
    replies: Option<Vec<fixture::Reply>>,
    prepared: Option<(Fixture, Runtime)>,
    mut continuation: Option<(Bridge, std::sync::mpsc::SyncSender<Bridge>)>,
) {
    let follows = continuation.is_some();
    let mut prepared = prepared;
    let submitted = loop {
        let state = *intent.borrow_and_update();
        if state.stop || !bridge.live() {
            break false;
        }
        if state.submitted {
            break true;
        }
        if intent.changed().await.is_err() {
            break false;
        }
    };
    if !submitted {
        if bridge.command.is_some() {
            wait_restoration(&bridge, &mut intent).await;
        }
        if let Some((fixture, runtime)) = prepared.take() {
            wait_restoration(&bridge, &mut intent).await;
            bridge.store.lock().unwrap().pre = Some(runtime.shutdown().await);
            match fixture.shutdown().await {
                Ok(report) => bridge.store.lock().unwrap().fixture = Some(report),
                Err(e) => bridge.store.lock().unwrap().error(e),
            }
            bridge.store.lock().unwrap().post = Some(Runtime::process_snapshot());
        }
        finish_command(&bridge, None);
        bridge.store.lock().unwrap().ended = true;
        return;
    }
    let prompt = bridge.submission.lock().unwrap().prompt.clone().unwrap();
    let (fixture, mut runtime) = if let Some((fixture, runtime)) = prepared.take() {
        (fixture, Some(runtime))
    } else {
        let fixture = match if follows {
            if let Some(replies) = replies {
                Fixture::consuming_follow_up_replies(bridge.session.clone(), replies).await
            } else {
                Fixture::consuming_follow_up(bridge.session.clone()).await
            }
        } else if let Some(replies) = replies {
            Fixture::replies_session(replies, bridge.session.clone()).await
        } else {
            Fixture::consuming_session(bridge.session.clone()).await
        } {
            Ok(f) => f,
            Err(_) => {
                let mut store = bridge.store.lock().unwrap();
                store.error("fixture init failed");
                store.ended = true;
                return;
            }
        };
        (fixture, None)
    };
    let root = std::env::temp_dir().join(format!(
        "scramjet-coding-ui-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut created = false;
    let setup = (|| -> io::Result<CodingOwner> {
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        created = true;
        bridge.store.lock().unwrap().retained_root = Some(root.clone());
        std::fs::write(root.join("sample.txt"), fixture::BEFORE)?;
        if runtime.is_none() {
            runtime = Some(new_runtime(&fixture)?);
        }
        let session =
            CodingSession::create(&root, &bridge.session, CodingCapability::ControlledEdit)
                .map_err(io::Error::other)?;
        if let Some(command) = &bridge.command {
            let mut owner = command.session.lock().unwrap();
            let owner = owner.as_mut().unwrap();
            let id = command.observer.snapshot().invocations[0]
                .as_ref()
                .unwrap()
                .id;
            owner
                .start_coding(id, session)
                .map(CodingOwner::Command)
                .map_err(io::Error::other)
        } else {
            let cancellation = EditCancellation::default();
            bridge.submission.lock().unwrap().root_cancellation = Some(cancellation.clone());
            if follows {
                CodingAgent::start_follow_up(session, cancellation, &prompt)
            } else {
                CodingAgent::start_interactive(session, cancellation, &prompt)
            }
            .map(CodingOwner::Agent)
            .map_err(io::Error::other)
        }
    })();
    let mut owner_terminated = true;
    let mut command_receipt = None;
    match setup {
        Ok(mut agent) => {
            let observer = agent.observer();
            let phases = async {
                *bridge.observer.lock().unwrap() = Some(observer.clone());
                let control = agent.control();
                bridge.attach(control.clone());
                let mut run = Box::pin(agent.run_unjoined(runtime.as_ref().unwrap()));
                let mut sample = tokio::time::interval(Duration::from_millis(10));
                sample.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut open = true;
                let operation = async {
                    loop {
                        let state = *intent.borrow_and_update();
                        if state.stop || !open || !bridge.live() {
                            if let Some(command) = &bridge.command {
                                command.control.cancel();
                            }
                            control.stop();
                        }
                        bridge.store.lock().unwrap().observation = Some(observer.snapshot());
                        tokio::select! {
                            biased;
                            changed = intent.changed(), if open => { if changed.is_err() { open = false; } },
                            result = &mut run => break result,
                            _ = sample.tick() => {},
                        }
                    }
                };
                let result = protected(operation).await.unwrap_or_else(|_| {
                    control.stop();
                    Err(scramjet_rs::CodingError::Invalid(
                        "coding execution panicked; restoration required",
                    ))
                });
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(run))).is_err() {
                    bridge
                        .store
                        .lock()
                        .unwrap()
                        .execution_error("coding future cleanup panicked; restoration required");
                }
                let eligible = follows
                    && result.is_ok()
                    && bridge.live()
                    && bridge.snapshot().error.is_none()
                    && agent.validate_follow_up("").is_ok();
                {
                    let mut p = bridge.store.lock().unwrap();
                    p.needs_checkpoint = eligible;
                    p.needs_settlement = true;
                }
                wait_restoration(&bridge, &mut intent).await;
                if eligible && bridge.live() && !bridge.snapshot().settlement_failed {
                    let remaining = bridge.submission.lock().unwrap().lifecycle.as_ref().map_or(
                        Duration::from_secs(5),
                        |(_, end)| {
                            end.saturating_duration_since(std::time::Instant::now())
                                .min(Duration::from_secs(5))
                        },
                    );
                    let checkpoint_end = tokio::time::Instant::now() + remaining;
                    let mut acknowledgement = Box::pin(tokio::time::timeout_at(
                        checkpoint_end,
                        fixture.checkpoint(),
                    ));
                    let checkpoint = loop {
                        tokio::select! {
                            value = &mut acknowledgement => break value.map_err(|_| "fixture checkpoint timeout").and_then(|r| r),
                            _ = sample.tick() => {
                                if !bridge.live() { control.stop(); break Err("checkpoint lifecycle stopped/expired"); }
                            }
                        }
                    };
                    drop(acknowledgement);
                    let admission = match checkpoint.and_then(|report| {
                        fixture::correlate_checkpoint(&observer.snapshot(), &report)?;
                        if !bridge.live() {
                            return Err("checkpoint lifecycle stopped/expired");
                        }
                        let s = runtime.as_ref().unwrap().snapshot();
                        if !work_clear(&s) {
                            return Err("checkpoint retains runtime obligations");
                        }
                        Ok(())
                    }) {
                        Ok(()) => {
                            tokio::time::timeout_at(checkpoint_end, agent.follow_up_admission())
                                .await
                                .map_err(|_| "coding checkpoint acknowledgement timeout")
                                .and_then(|r| {
                                    r.map_err(|_| "coding checkpoint capacity/eligibility refused")
                                })
                                .and_then(|a| {
                                    if bridge.live() {
                                        Ok(a)
                                    } else {
                                        Err("checkpoint lifecycle stopped/expired")
                                    }
                                })
                        }
                        Err(error) => Err(error),
                    };
                    match admission {
                        Ok(admission) => {
                            let (old, send) = continuation.take().unwrap();
                            let (next, receive) =
                                old.continuation(admission, observer.clone(), control.clone());
                            let owner = next.execution();
                            if send.send(next.clone()).is_err() {
                                next.stop();
                            }
                            bridge = owner;
                            intent = receive;
                            let submitted = loop {
                                let state = *intent.borrow_and_update();
                                if state.stop || !bridge.live() {
                                    control.stop();
                                    break false;
                                }
                                if state.submitted {
                                    break true;
                                }
                                tokio::select! {
                                    changed = intent.changed() => { if changed.is_err() { control.stop(); break false; } },
                                    _ = sample.tick() => {},
                                }
                            };
                            if submitted {
                                #[cfg(test)]
                                if bridge.submission.lock().unwrap().fail_after_handoff {
                                    panic!("injected executor panic after handoff");
                                }
                                let new_prompt =
                                    bridge.submission.lock().unwrap().prompt.clone().unwrap();
                                let transition = protected(
                                    agent.follow_up(runtime.as_ref().unwrap(), &new_prompt),
                                )
                                .await;
                                bridge.attach(agent.control());
                                let second = if matches!(transition, Ok(Ok(()))) {
                                    let control = agent.control();
                                    let mut run =
                                        Box::pin(agent.run_unjoined(runtime.as_ref().unwrap()));
                                    let operation = async {
                                        loop {
                                            if !bridge.live() {
                                                control.stop();
                                            }
                                            bridge.store.lock().unwrap().observation =
                                                Some(observer.snapshot());
                                            tokio::select! {
                                                biased;
                                                changed = intent.changed() => { if changed.is_err() { control.stop(); } },
                                                result = &mut run => break result,
                                                _ = sample.tick() => {},
                                            }
                                        }
                                    };
                                    let result = protected(operation).await.unwrap_or_else(|_| {
                                        control.stop();
                                        Err(scramjet_rs::CodingError::Invalid(
                                            "follow-up execution panicked",
                                        ))
                                    });
                                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                        || drop(run),
                                    ))
                                    .is_err()
                                    {
                                        bridge
                                            .store
                                            .lock()
                                            .unwrap()
                                            .execution_error("follow-up future cleanup panicked");
                                    }
                                    result
                                } else {
                                    agent.control().stop();
                                    Err(scramjet_rs::CodingError::Invalid(
                                        "follow-up transition failed; no retry",
                                    ))
                                };
                                if let Err(e) = second
                                    && !matches!(
                                        e,
                                        scramjet_rs::CodingError::Denied
                                            | scramjet_rs::CodingError::Cancelled
                                    )
                                {
                                    bridge.store.lock().unwrap().execution_error(&e.to_string());
                                }
                            }
                            bridge.store.lock().unwrap().needs_settlement = true;
                            wait_restoration(&bridge, &mut intent).await;
                        }
                        Err(e) => {
                            control.stop();
                            bridge.store.lock().unwrap().execution_error(e);
                        }
                    }
                }
                result
            };
            let result = protected(phases).await.unwrap_or_else(|_| {
                if let Some(root) = &bridge
                    .submission
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .root_cancellation
                {
                    root.cancel();
                }
                let mut p = bridge.store.lock().unwrap_or_else(|e| e.into_inner());
                p.execution_error("coding phase panicked; restoration required; no retry");
                p.needs_checkpoint = false;
                p.needs_settlement = true;
                Err(scramjet_rs::CodingError::Invalid("coding phase panicked"))
            });
            wait_restoration(&bridge, &mut intent).await;
            match agent.finish() {
                Ok(receipt) => command_receipt = receipt,
                Err(_) => bridge
                    .store
                    .lock()
                    .unwrap()
                    .error("coding worker cleanup unavailable"),
            }
            let observation = observer.snapshot();
            owner_terminated = observation.worker_terminated;
            let run_error = result
                .err()
                .filter(|e| {
                    !matches!(
                        e,
                        scramjet_rs::CodingError::Denied | scramjet_rs::CodingError::Cancelled
                    )
                })
                .map(|e| e.to_string());
            {
                let mut store = bridge.store.lock().unwrap();
                store.observation = Some(observation.clone());
                if !observation.worker_joined {
                    store.error("coding worker cleanup unavailable");
                }
                if let Some(error) = run_error {
                    store.execution_error(&error);
                }
            }
        }
        Err(error) => {
            let message = error.to_string();
            bridge.store.lock().unwrap().error(&message);
        }
    }
    if bridge.snapshot().error.is_some() || intent.borrow().stop {
        wait_restoration(&bridge, &mut intent).await;
    }
    bridge.submission.lock().unwrap().control = None;
    if let Some(runtime) = runtime {
        match tokio::spawn(runtime.shutdown()).await {
            Ok(pre) => bridge.store.lock().unwrap().pre = Some(pre),
            Err(_) => bridge
                .store
                .lock()
                .unwrap()
                .error("runtime cleanup unavailable/unjoined"),
        }
    }
    if bridge.snapshot().error.is_some() {
        wait_restoration(&bridge, &mut intent).await;
    }
    match fixture.shutdown().await {
        Ok(report) => {
            if report.session != bridge.session {
                bridge
                    .store
                    .lock()
                    .unwrap()
                    .error("foreign fixture session");
            }
            if report
                .prompt
                .as_ref()
                .is_some_and(|captured| captured.as_ref() != prompt.as_ref())
            {
                bridge
                    .store
                    .lock()
                    .unwrap()
                    .error("submitted/consumed prompt mismatch");
            }
            if report.follow_prompt.as_ref().is_some_and(|p| {
                bridge.submission.lock().unwrap().prompt.as_deref() != Some(p.as_ref())
            }) {
                bridge
                    .store
                    .lock()
                    .unwrap()
                    .error("submitted/consumed follow-up mismatch");
            }
            if let Some(observation) = bridge.snapshot().observation
                && let Err(error) = fixture::reconcile(&observation, &report)
            {
                bridge.store.lock().unwrap().error(error);
            }
            bridge.store.lock().unwrap().fixture = Some(report);
        }
        Err(error) => bridge.store.lock().unwrap().error(error),
    }
    let post = Runtime::process_snapshot();
    {
        let mut store = bridge.store.lock().unwrap();
        store.post = Some(post);
        if !work_clear(&post) {
            store.error("joined cleanup retains work/accounting obligations");
        }
    }
    if bridge.snapshot().error.is_some() {
        wait_restoration(&bridge, &mut intent).await;
    }
    if created && owner_terminated {
        if let Some(observation) = bridge.snapshot().observation {
            let source = root.clone();
            let session = bridge.session.clone();
            let history =
                tokio::task::spawn_blocking(move || reconstruct_coding_history(&source, &session))
                    .await;
            let history = history.ok().and_then(Result::ok);
            let facts = history.as_ref().map(|h| h.operations());
            let content = std::fs::read_to_string(root.join("sample.txt"));
            bridge.store.lock().unwrap().durable_effect =
                facts.and_then(|f| f.last().map(|f| f.state));
            let agrees = facts.is_some_and(|facts| {
                let p = bridge.snapshot();
                let current = facts.iter().find(|f| f.operation == observation.invocation);
                let first = p.checkpoint.as_ref().is_none_or(|o| {
                    o.effect == Some(OperationState::Completed)
                        && o.verified
                        && facts.first().is_some_and(|f| {
                            f.operation == 1 && f.state == OperationState::Completed
                        })
                });
                let latest = facts
                    .iter()
                    .rev()
                    .find(|f| f.state == OperationState::Completed);
                let expected = latest.map_or(Some(fixture::BEFORE), |f| {
                    p.fixture.as_ref().and_then(|r| {
                        if f.operation == 1 {
                            r.prompt.as_deref().or(Some(fixture::AFTER))
                        } else {
                            r.follow_prompt.as_deref()
                        }
                    })
                });
                let uncertain = observation.dispatched
                    && current.is_some_and(|f| f.state == OperationState::Uncertain);
                first
                    && observation.effect == current.map(|f| f.state)
                    && if uncertain {
                        content.as_ref().is_ok_and(|c| c.len() <= 1024)
                    } else {
                        expected.is_some() && content.as_deref().ok() == expected
                    }
            });
            {
                let mut store = bridge.store.lock().unwrap();
                store.history = history.map(Arc::new);
                if store.history.is_none() {
                    store.history_error =
                        Some("validated history unavailable; no partial substitute");
                }
            }
            if agrees {
                bridge.store.lock().unwrap().reconstructed = true;
            } else {
                bridge
                    .store
                    .lock()
                    .unwrap()
                    .error("live/durable facts mismatch or reconstruction unavailable");
            }
        }
        if !bridge.snapshot().reconstructed {
            bridge
                .store
                .lock()
                .unwrap()
                .error("validated attestation unavailable: root retained");
        } else if (follows || bridge.command.is_some())
            && bridge.snapshot().history.as_ref().is_some_and(|h| {
                h.operations()
                    .iter()
                    .any(|f| f.state == OperationState::Uncertain)
            })
        {
            bridge
                .store
                .lock()
                .unwrap()
                .error("uncertain effect: disposable source retained; no replay");
        } else if std::fs::remove_dir_all(&root).is_ok() {
            bridge.store.lock().unwrap().retained_root = None;
        } else {
            bridge
                .store
                .lock()
                .unwrap()
                .error("disposable root cleanup failed");
        }
    } else if created {
        bridge
            .store
            .lock()
            .unwrap()
            .error("coding owner termination unavailable: root retained");
    }
    finish_command(&bridge, command_receipt);
    bridge.store.lock().unwrap().ended = true;
}
fn work_clear(s: &Snapshot) -> bool {
    s.active
        + s.pending
        + s.handles
        + s.input_bytes
        + s.delta_items
        + s.delta_bytes
        + s.potential
        + s.unresolved
        + s.usage_obligations
        + s.token_debt as usize
        == 0
}
fn spawn_executor(
    bridge: &Bridge,
    intent: watch::Receiver<Intent>,
    prepare: bool,
) -> io::Result<(std::thread::JoinHandle<()>, std::sync::mpsc::Receiver<bool>)> {
    let owner = bridge.execution();
    let offer = bridge.offer;
    let (ready, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("coding-ui-executor".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
                    let executor = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    executor.block_on(async {
                        if !prepare {
                            let _ = ready.send(true);
                            journey(owner.clone(), intent, None).await;
                            return;
                        }
                        let allowed = || {
                            let s = owner.submission.lock().unwrap();
                            !s.stop
                                && s.lifecycle.as_ref().is_none_or(|(stopped, deadline)| {
                                    !stopped.load(std::sync::atomic::Ordering::Relaxed)
                                        && std::time::Instant::now() < *deadline
                                })
                        };
                        if !allowed() {
                            owner.store.lock().unwrap().ended = true;
                            let _ = ready.send(false);
                            return;
                        }
                        let fixture = match Fixture::consuming_session(owner.session.clone()).await
                        {
                            Ok(f) => f,
                            Err(_) => {
                                let mut p = owner.store.lock().unwrap();
                                p.error("second fixture preparation failed");
                                p.ended = true;
                                let _ = ready.send(false);
                                return;
                            }
                        };
                        let runtime = match new_runtime(&fixture) {
                            Ok(r) => r,
                            Err(e) => {
                                owner.store.lock().unwrap().error(&e.to_string());
                                match fixture.shutdown().await {
                                    Ok(r) => owner.store.lock().unwrap().fixture = Some(r),
                                    Err(e) => owner.store.lock().unwrap().error(e),
                                }
                                let mut p = owner.store.lock().unwrap();
                                p.post = Some(Runtime::process_snapshot());
                                p.ended = true;
                                let _ = ready.send(false);
                                return;
                            }
                        };
                        if allowed() {
                            owner.store.lock().unwrap().offer = Some(offer);
                        }
                        let _ = ready.send(true);
                        execute_journey(owner.clone(), intent, None, Some((fixture, runtime)))
                            .await;
                    });
                    Ok(())
                }));
            if !matches!(result, Ok(Ok(()))) {
                let mut p = owner.store.lock().unwrap();
                p.error("executor failure; subordinate cleanup unavailable; root retained");
                p.ended = true;
            }
        })?;
    Ok((worker, receive))
}
fn interval(
    bridge: &Bridge,
    state: &mut ui::CodingUi,
    display: &mut impl FnMut(&Bridge, &mut ui::CodingUi) -> io::Result<ui::CodingReturn>,
    errors: &mut Vec<String>,
) -> ui::CodingReturn {
    if !state.live() {
        bridge.restored();
        bridge.stop();
        return ui::CodingReturn::Exit;
    }
    let result = display(bridge, state);
    if result.is_ok() {
        bridge.restored();
    } else {
        bridge.restoration_failed();
    }
    if (!matches!(result, Ok(ui::CodingReturn::Checkpoint))
        && !(bridge.command_mode() && matches!(result, Ok(ui::CodingReturn::Settlement))))
        || !state.live()
    {
        bridge.stop();
    }
    match result {
        Ok(ui::CodingReturn::Checkpoint) if !state.live() => ui::CodingReturn::Exit,
        Ok(r) => r,
        Err(e) => {
            errors.push(format!("terminal: {e}"));
            ui::CodingReturn::Exit
        }
    }
}
fn coordinate(
    state: &mut ui::CodingUi,
    mut display: impl FnMut(&Bridge, &mut ui::CodingUi) -> io::Result<ui::CodingReturn>,
) -> io::Result<(Bridge, Vec<String>)> {
    let mut errors = Vec::new();
    let (first, intent) = Bridge::new();
    state.bind(&first);
    let (worker, _) = spawn_executor(&first, intent, false)?;
    let disposition = interval(&first, state, &mut display, &mut errors);
    if worker.join().is_err() {
        first.store.lock().unwrap().error("executor join failed");
    }
    let p = first.snapshot();
    if disposition != ui::CodingReturn::Settlement || !state.live() || !p.settled() {
        return Ok((first, errors));
    }
    let (second, intent) = Bridge::new();
    state.bind(&second);
    {
        let mut s = second.store.lock().unwrap();
        s.offer = None;
        s.previous = Some(Arc::new(p));
    }
    let (worker, ready) = match spawn_executor(&second, intent, true) {
        Ok(w) => w,
        Err(e) => {
            second
                .store
                .lock()
                .unwrap()
                .error(&format!("second executor preparation failed: {e}"));
            state.mode(true);
            interval(&second, state, &mut display, &mut errors);
            return Ok((second, errors));
        }
    };
    let prepared = match ready.recv() {
        Ok(prepared) => prepared,
        Err(_) => {
            second.restored();
            second.stop();
            if worker.join().is_err() {
                second.store.lock().unwrap().error("executor join failed");
            }
            return Ok((second, errors));
        }
    };
    state.mode(!prepared);
    let disposition = interval(&second, state, &mut display, &mut errors);
    if worker.join().is_err() {
        second.store.lock().unwrap().error("executor join failed");
    }
    if prepared
        && disposition == ui::CodingReturn::Settlement
        && state.live()
        && second.snapshot().settled()
    {
        state.mode(true);
        interval(&second, state, &mut display, &mut errors);
    }
    Ok((second, errors))
}
async fn supervised_follow_up(
    initial: Bridge,
    intent: watch::Receiver<Intent>,
    send: std::sync::mpsc::SyncSender<Bridge>,
    replies: Option<Vec<fixture::Reply>>,
    prepared: Option<(Fixture, Runtime)>,
) {
    let current = initial.current.clone();
    let mut work = Box::pin(execute_owned(
        initial.execution(),
        intent,
        replies,
        prepared,
        Some((initial, send)),
    ));
    if protected(&mut work).await.is_err() {
        let active = current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .unwrap()
            .clone();
        {
            let state = active.submission.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(root) = &state.root_cancellation {
                root.cancel();
            }
        }
        {
            let mut p = active.store.lock().unwrap_or_else(|e| e.into_inner());
            p.error("executor failure; subordinate cleanup unavailable; source retained");
            p.needs_settlement = true;
            p.needs_checkpoint = false;
        }
        let mut intent = active.intent.clone();
        wait_restoration(&active, &mut intent).await;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(work)));
        active.store.lock().unwrap_or_else(|e| e.into_inner()).ended = true;
    }
}
pub(crate) fn coordinate_follow_up(
    state: &mut ui::CodingUi,
    mut display: impl FnMut(&Bridge, &mut ui::CodingUi) -> io::Result<ui::CodingReturn>,
) -> io::Result<(Bridge, Vec<String>)> {
    let mut errors = Vec::new();
    let (first, intent) = Bridge::new();
    first.store.lock().unwrap().follow_up = true;
    state.bind(&first);
    let current = first.current.clone();
    let initial = first.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("coding-ui-executor".into())
        .spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
                    let executor = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    executor.block_on(supervised_follow_up(initial, intent, send, None, None));
                    Ok(())
                }));
            if !matches!(result, Ok(Ok(()))) {
                let active = current
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .as_ref()
                    .unwrap()
                    .clone();
                let mut p = active.store.lock().unwrap_or_else(|e| e.into_inner());
                p.error("executor failure; subordinate cleanup unavailable; root retained");
                p.ended = true;
            }
        })?;
    let first_return = interval(&first, state, &mut display, &mut errors);
    let (active, disposition) = if first_return == ui::CodingReturn::Checkpoint {
        match receive.recv() {
            Ok(next) => {
                state.bind(&next);
                state.mode(false);
                let disposition = interval(&next, state, &mut display, &mut errors);
                (next, disposition)
            }
            Err(_) => (first, ui::CodingReturn::Exit),
        }
    } else {
        (first, first_return)
    };
    if worker.join().is_err() {
        active.store.lock().unwrap().error("executor join failed");
    }
    if disposition == ui::CodingReturn::Settlement && state.live() && active.snapshot().settled() {
        state.mode(true);
        interval(&active, state, &mut display, &mut errors);
    }
    Ok((active, errors))
}
pub(crate) fn coordinate_command(
    state: &mut ui::CodingUi,
    mut display: impl FnMut(&Bridge, &mut ui::CodingUi) -> io::Result<ui::CodingReturn>,
) -> io::Result<(Bridge, Vec<String>)> {
    let mut errors = Vec::new();
    let (bridge, intent) = Bridge::new_command()?;
    state.bind(&bridge);
    let owner = bridge.execution();
    let worker = std::thread::Builder::new()
        .name("command-coding-ui-executor".into())
        .spawn(move || {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match executor {
                Ok(executor) => executor.block_on(async {
                    let mut work = Box::pin(execute_owned(owner.clone(), intent, None, None, None));
                    if protected(&mut work).await.is_err() {
                        owner.command.as_ref().unwrap().control.cancel();
                        {
                            let mut p = owner.store.lock().unwrap_or_else(|e| e.into_inner());
                            p.error("command executor panicked; restoration required; no replay");
                            p.needs_settlement = true;
                        }
                        let mut intent = owner.intent.clone();
                        wait_restoration(&owner, &mut intent).await;
                        let _ =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(work)));
                    }
                    let mut restoration = owner.intent.clone();
                    wait_restoration(&owner, &mut restoration).await;
                    finish_command(&owner, None);
                    owner.store.lock().unwrap().ended = true;
                }),
                Err(_) => {
                    owner
                        .store
                        .lock()
                        .unwrap()
                        .error("command executor unavailable");
                    owner.store.lock().unwrap().ended = true;
                }
            }
        })?;
    let disposition = interval(&bridge, state, &mut display, &mut errors);
    if worker.join().is_err() {
        bridge
            .store
            .lock()
            .unwrap()
            .error("command executor join unavailable");
    }
    finish_command(&bridge.execution(), None);
    if disposition == ui::CodingReturn::Settlement && state.live() && bridge.snapshot().settled() {
        state.mode(true);
        interval(&bridge, state, &mut display, &mut errors);
    }
    Ok((bridge, errors))
}

pub async fn command_demo() -> io::Result<()> {
    let (bridge, intent) = Bridge::new_command()?;
    let prompt = "command-driven complete input\n界 e\u{301} 🙂 \\";
    let offer = Some(Offer {
        id: bridge.offer,
        revision: 1,
    });
    bridge.painted(offer);
    if !bridge.submit(offer, 1, prompt) {
        return Err(io::Error::other("command input admission failed"));
    }
    bridge.restored();
    let owner = bridge.execution();
    let decide = async {
        loop {
            if let Some(proposal) = bridge
                .snapshot()
                .observation
                .and_then(|o| o.proposal.clone())
            {
                bridge.decide(&proposal, CodingDecision::Approve);
                break;
            }
            if bridge.snapshot().ended {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::join!(execute_owned(owner, intent, None, None, None), decide);
    let p = bridge.snapshot();
    write_report(&p, &mut io::stdout())?;
    if !p.settled()
        || p.command.as_ref().is_none_or(|s| {
            !s.settled
                || s.invocations[0].as_ref().is_none_or(|f| {
                    f.report
                        .as_ref()
                        .is_none_or(|r| r.completion != CommandCompletion::Completed)
                })
        })
    {
        return Err(io::Error::other(
            "command coding demonstration did not establish completion",
        ));
    }
    Ok(())
}

pub fn run(follow_up: bool, command: bool) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other("requires TTY stdin and stdout"));
    }
    let mut state = ui::CodingUi::new()?;
    let panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = if command {
        coordinate_command(&mut state, ui::run_coding)
    } else if follow_up {
        coordinate_follow_up(&mut state, ui::run_coding)
    } else {
        coordinate(&mut state, ui::run_coding)
    };
    std::panic::set_hook(panic_hook);
    let (bridge, mut errors) = result?;
    let p = bridge.snapshot();
    let report = ui::runtime_output().and_then(|mut writer| {
        if let Some(first) = &p.previous {
            writeln!(writer, "journey=1 retained independent history")?;
            write_report(first, &mut writer)?;
            writeln!(writer, "journey=2 independent history")?;
        }
        write_report(&p, &mut writer)
    });
    for projection in p
        .previous
        .iter()
        .map(|p| p.as_ref())
        .chain(std::iter::once(&p))
    {
        if let Some(e) = &projection.error {
            errors.push(e.clone());
        }
    }
    if let Err(e) = report {
        errors.push(format!("report: {e}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(errors.join("; ")))
    }
}
pub fn report_error(error: &io::Error) {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        if let Ok(mut writer) = ui::runtime_output() {
            let _ = writeln!(writer, "coding-ui-demo: {error}");
        }
    } else {
        let _ = writeln!(io::stderr(), "coding-ui-demo: {error}");
    }
}
fn write_report(p: &Projection, writer: &mut impl Write) -> io::Result<()> {
    writeln!(
        writer,
        "coding-ui-demo: workload={} ceiling=1 retries=0 ended={} fixture_joined={} reconstructed={} error={}",
        if p.follow_up { 8 } else { 4 },
        p.ended,
        p.fixture.is_some(),
        p.reconstructed,
        p.error.is_some()
    )?;
    writeln!(
        writer,
        "session={:?} Queued(local-only)={:?} rejection={:?}",
        p.session, p.submission, p.rejection
    )?;
    if let Some(command) = &p.command {
        if let Some(fact) = &command.invocations[0] {
            let coding = fact.coding.as_ref().unwrap();
            writeln!(
                writer,
                "command=replace outcome=ReplaceVerified target={} input_bytes={} invocation={:?} coding_session={} owner={:?} phase={:?} report={:?} command_local_settled={} cancellation_requested={}",
                coding.target,
                coding.input.len(),
                fact.id,
                coding.session,
                coding.owner,
                fact.phase,
                fact.report,
                command.settled,
                command.cancellation_requested
            )?;
            writeln!(
                writer,
                "AwaitingCompletion_reached={} execution_disposition={:?}; accepted command report is separate from coding evidence",
                coding.receipt_accepted,
                coding.execution.as_ref().map(|e| e.disposition)
            )?;
        } else {
            writeln!(
                writer,
                "command=replace outcome=ReplaceVerified target=sample.txt not_admitted=true command_local_settled={}",
                command.settled
            )?;
        }
    }
    if p.follow_up {
        writeln!(
            writer,
            "same_session=true offered_invocation={} first_submission={:?} checkpoint_verified={}",
            p.invocation,
            p.first_submission,
            p.checkpoint
                .as_ref()
                .is_some_and(|o| o.verified && o.final_summary.is_some())
        )?;
    }
    if let Some(o) = &p.observation {
        writeln!(
            writer,
            "owner={} invocation={} decision={:?} authorized={} dispatched={} effect={:?} verified={} final={} worker_terminated={} worker_joined={}",
            o.owner,
            o.invocation,
            o.decision,
            o.authorized,
            o.dispatched,
            o.effect,
            o.verified,
            o.final_summary.is_some(),
            o.worker_terminated,
            o.worker_joined
        )?;
        for (index, slot) in o
            .attempts
            .iter()
            .enumerate()
            .filter(|(_, s)| s.recorded || s.request_id.is_some())
        {
            writeln!(
                writer,
                "turn={} recorded={} id={:?} outcome={:?} material={:?} accounting={:?}",
                index + 1,
                slot.recorded,
                slot.request_id,
                slot.outcome,
                slot.material,
                slot.accounting
            )?;
        }
    }
    writeln!(
        writer,
        "read_only_durable_effect={:?}; live publication receipts are separate; history={:?} history_error={:?}",
        p.durable_effect,
        p.history
            .as_ref()
            .map(|h| (h.records().len(), h.ending(), h.payload_bytes())),
        p.history_error
    )?;
    for (label, snapshot) in [("pre_attestation", p.pre), ("post_attestation", p.post)] {
        if let Some(s) = snapshot {
            writeln!(
                writer,
                "{label}: active={} pending={} handles={} exposure={} unknown_usage={} obligations={} debt={} measured={:?}",
                s.active,
                s.pending,
                s.handles,
                s.potential + s.unresolved,
                s.unknown_usage,
                s.usage_obligations,
                s.token_debt,
                s.synthetic_measured
            )?;
        } else {
            writeln!(writer, "{label}: unavailable")?;
        }
    }
    if let Some(root) = &p.retained_root {
        writeln!(writer, "disposable root retained: {}", root.display())?;
    }
    writer.flush()
}
