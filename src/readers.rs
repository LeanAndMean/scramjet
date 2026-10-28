use crate::{Outcome, RequestId, Runtime, Snapshot};
use serde::{
    Serialize,
    de::{self, MapAccess, Visitor},
};
use std::{
    fmt, io,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) mod journal;
mod observation;
pub use observation::{
    ReaderAccounting, ReaderControl, ReaderObservation, ReaderObserver, ReaderPhase, ReaderSlot,
};
#[cfg(test)]
mod tests;

pub const READER_INSTRUCTION: &str = "Read only the supplied immutable content and return a short summary with the exact session, generation and child identity.";
const CONTENT: usize = 1024;
const RESPONSE: usize = 2048;
const SUMMARY: usize = 512;

#[derive(Debug)]
pub enum ReaderError {
    Invalid(&'static str),
    Io(io::Error),
    Storage,
    LostOwner,
}
impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ReaderError {}
impl From<io::Error> for ReaderError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rustix::io::Errno> for ReaderError {
    fn from(e: rustix::io::Errno) -> Self {
        Self::Io(e.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderState {
    Completed { summary: String },
    Failed(ReaderFailure),
    Cancelled,
    NotDispatched,
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderFailure {
    Admission,
    Transport,
    InvalidResponse,
    ResponseLimit,
    Storage,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderFact {
    pub child: u8,
    pub request_id: Option<RequestId>,
    pub state: ReaderState,
}
#[derive(Debug)]
pub struct ReaderTree {
    pub session: String,
    pub generation: u64,
    pub children: [ReaderFact; 3],
    pub parent_finished: bool,
}
#[derive(Debug)]
pub struct ReaderRun {
    pub tree: ReaderTree,
    pub snapshot: Snapshot,
    /// Live synthetic fixture usage only; not reconstructed or provider billing.
    pub usage_tokens: Option<u64>,
}

#[derive(Clone)]
pub struct ReaderInputs {
    pub(crate) session: String,
    generation: u64,
    pub(crate) contents: [Arc<str>; 3],
}
impl ReaderInputs {
    pub(crate) fn shared(session: &str, contents: [Arc<str>; 3]) -> Self {
        Self {
            session: session.to_owned(),
            generation: 1,
            contents,
        }
    }
    pub fn new(session: &str, generation: u64, contents: [&str; 3]) -> Result<Self, ReaderError> {
        if session.is_empty()
            || session.len() > 64
            || !session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || generation == 0
        {
            return Err(ReaderError::Invalid("invocation identity"));
        }
        for content in contents {
            validate_text(content, CONTENT, true)?;
        }
        Ok(Self {
            session: session.to_owned(),
            generation,
            contents: contents.map(Arc::from),
        })
    }
}
fn validate_text(text: &str, limit: usize, input: bool) -> Result<(), ReaderError> {
    if text.len() > limit
        || text
            .chars()
            .any(|c| c.is_control() && !(input && matches!(c, '\n' | '\t')))
    {
        return Err(ReaderError::Invalid("text ceiling or controls"));
    }
    Ok(())
}

#[derive(Serialize)]
struct Context<'a> {
    session: &'a str,
    generation: u64,
    child: u8,
    instruction: &'a str,
    content: &'a str,
}
struct InputBuffer(Vec<u8>);
impl io::Write for InputBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > 4096 - self.0.len() {
            return Err(io::Error::other("request ceiling"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn context(inputs: &ReaderInputs, child: u8) -> Result<String, ReaderError> {
    let mut writer = InputBuffer(Vec::with_capacity(4096));
    serde_json::to_writer(
        &mut writer,
        &Context {
            session: &inputs.session,
            generation: inputs.generation,
            child,
            instruction: READER_INSTRUCTION,
            content: &inputs.contents[child as usize],
        },
    )
    .map_err(|_| ReaderError::Invalid("request serialization ceiling"))?;
    Ok(String::from_utf8(writer.0).expect("JSON UTF-8"))
}

struct BoundedText<const N: usize>(String);
impl<'de, const N: usize> serde::Deserialize<'de> for BoundedText<N> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Text<const N: usize>;
        impl<const N: usize> Visitor<'_> for Text<N> {
            type Value = BoundedText<N>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a string of at most {N} bytes")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                if s.len() > N {
                    return Err(E::custom("string ceiling"));
                }
                Ok(BoundedText(s.to_owned()))
            }
        }
        d.deserialize_str(Text::<N>)
    }
}
struct Response {
    session: String,
    generation: u64,
    child: u8,
    summary: String,
}
impl<'de> serde::Deserialize<'de> for Response {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Response;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a fixed reader response object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<Response, M::Error> {
                let (mut session, mut generation, mut child, mut summary) =
                    (None, None, None, None);
                while let Some(BoundedText(key)) = m.next_key::<BoundedText<16>>()? {
                    match key.as_str() {
                        "session" if session.is_none() => {
                            session = Some(m.next_value::<BoundedText<64>>()?.0)
                        }
                        "generation" if generation.is_none() => {
                            generation = Some(m.next_value::<u64>()?)
                        }
                        "child" if child.is_none() => child = Some(m.next_value::<u8>()?),
                        "summary" if summary.is_none() => {
                            summary = Some(m.next_value::<BoundedText<SUMMARY>>()?.0)
                        }
                        _ => return Err(de::Error::custom("unknown or duplicate field")),
                    }
                }
                Ok(Response {
                    session: session.ok_or_else(|| de::Error::missing_field("session"))?,
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    child: child.ok_or_else(|| de::Error::missing_field("child"))?,
                    summary: summary.ok_or_else(|| de::Error::missing_field("summary"))?,
                })
            }
        }
        d.deserialize_map(Object)
    }
}
fn decode(text: &str, session: &str, generation: u64, child: u8) -> Result<String, ReaderError> {
    if text.len() > RESPONSE {
        return Err(ReaderError::Invalid("response ceiling"));
    }
    let response: Response =
        serde_json::from_str(text).map_err(|_| ReaderError::Invalid("strict reader response"))?;
    if response.session != session || response.generation != generation || response.child != child {
        return Err(ReaderError::Invalid("foreign reader result"));
    }
    validate_text(&response.summary, SUMMARY, false)?;
    Ok(response.summary)
}

struct ControlState {
    cancelled: [bool; 3],
    accepted: [bool; 3],
}
struct Control {
    state: Mutex<ControlState>,
    changed: watch::Sender<u64>,
    observation: watch::Sender<ReaderObservation>,
}
pub struct ReaderParent {
    control: Arc<Control>,
    result: Option<oneshot::Receiver<Result<ReaderRun, ReaderError>>>,
}
pub struct ReaderCleanup {
    receipt: oneshot::Receiver<Result<Snapshot, ReaderError>>,
}
impl ReaderCleanup {
    pub async fn wait(self) -> Result<Snapshot, ReaderError> {
        self.receipt.await.map_err(|_| ReaderError::LostOwner)?
    }
}
pub(crate) struct ReaderLogical {
    pub observation: ReaderObservation,
    pub outcome: Result<(ReaderTree, Option<u64>), ReaderError>,
}
pub(crate) struct ReaderSupervision {
    pub logical: oneshot::Sender<ReaderLogical>,
    pub release: oneshot::Receiver<()>,
}
enum ReaderStorage {
    Path(Option<std::path::PathBuf>),
    Descriptor(std::fs::File),
}
impl ReaderStorage {
    fn path(path: &Path) -> Self {
        Self::Path((path.as_os_str().len() <= 4096).then(|| path.to_owned()))
    }
}
enum ReaderRuntime {
    Owned(Runtime),
    Retained(Arc<Runtime>),
}
impl ReaderRuntime {
    fn get(&self) -> &Runtime {
        match self {
            Self::Owned(runtime) => runtime,
            Self::Retained(runtime) => runtime,
        }
    }
}
impl ReaderParent {
    pub fn start(
        runtime: Runtime,
        root: &Path,
        inputs: ReaderInputs,
    ) -> Result<(Self, ReaderCleanup), ReaderError> {
        Self::start_owned(
            ReaderRuntime::Owned(runtime),
            ReaderStorage::path(root),
            inputs,
            None,
        )
    }
    pub(crate) fn start_supervised(
        runtime: Runtime,
        root: &Path,
        inputs: ReaderInputs,
        supervision: ReaderSupervision,
    ) -> Result<(Self, ReaderCleanup), ReaderError> {
        Self::start_owned(
            ReaderRuntime::Owned(runtime),
            ReaderStorage::path(root),
            inputs,
            Some(supervision),
        )
    }
    pub(crate) fn start_retained(
        runtime: Arc<Runtime>,
        root: std::fs::File,
        inputs: ReaderInputs,
        supervision: ReaderSupervision,
    ) -> Result<(Self, ReaderCleanup), ReaderError> {
        Self::start_owned(
            ReaderRuntime::Retained(runtime),
            ReaderStorage::Descriptor(root),
            inputs,
            Some(supervision),
        )
    }
    fn start_owned(
        runtime: ReaderRuntime,
        root: ReaderStorage,
        inputs: ReaderInputs,
        supervision: Option<ReaderSupervision>,
    ) -> Result<(Self, ReaderCleanup), ReaderError> {
        let valid_runtime =
            runtime.get().active_limit() == 2 && runtime.get().snapshot().handles == 0;
        let inputs = Arc::new(inputs);
        let control = Arc::new(Control::new(&inputs.session, inputs.generation));
        let (result, receive) = oneshot::channel();
        let (receipt, cleanup) = oneshot::channel();
        let (sender, receiver) = mpsc::channel::<Work>(1);
        let worker_control = control.clone();
        let (initialized, initialization) = oneshot::channel();
        let worker_inputs = inputs.clone();
        let worker = std::thread::Builder::new()
            .name("reader-journal".to_owned())
            .spawn(move || {
                match (|| {
                    if !valid_runtime {
                        return Err(ReaderError::Invalid(
                            "reader runtime requires active ceiling two and no inherited handles",
                        ));
                    }
                    match root {
                        ReaderStorage::Path(path) => journal::Journal::create(
                            &path.ok_or(ReaderError::Invalid("root path ceiling"))?,
                            &worker_inputs,
                        ),
                        ReaderStorage::Descriptor(root) => {
                            journal::Journal::create_descriptor(root, &worker_inputs)
                        }
                    }
                })() {
                    Ok(journal) => {
                        let _ = initialized.send(Ok(()));
                        journal_worker(journal, receiver, worker_control);
                    }
                    Err(error) => {
                        cancel(&worker_control, None);
                        let _ = initialized.send(Err(error));
                    }
                }
            });
        let owner_control = control.clone();
        tokio::spawn(async move {
            let mut timer_control = owner_control.changed.subscribe();
            let deadline = tokio::time::sleep(Duration::from_secs(30));
            tokio::pin!(deadline);
            let outcome = {
                let run = async {
                    initialization.await.map_err(|_| ReaderError::Storage)??;
                    coordinate(runtime.get(), &inputs, &owner_control, &sender).await
                };
                tokio::pin!(run);
                let mut expired = false;
                loop {
                    tokio::select! {
                        outcome = &mut run => break outcome,
                        _ = &mut deadline, if !expired => { expired = true; cancel(&owner_control, None); }
                        _ = timer_control.changed() => {}
                    }
                }
            };
            let outcome = if let Some(supervision) = supervision {
                let observation = owner_control.observation.borrow().clone();
                let _ = supervision.logical.send(ReaderLogical {
                    observation,
                    outcome,
                });
                let _ = supervision.release.await;
                None
            } else {
                Some(outcome)
            };
            drop(sender);
            let joined = match worker {
                Ok(worker) => tokio::task::spawn_blocking(move || worker.join()).await,
                Err(_) => Ok(Ok(())),
            };
            let storage_joined = matches!(joined, Ok(Ok(())));
            let (shutdown, runtime_joined) = match runtime {
                ReaderRuntime::Owned(runtime) => {
                    let shutdown =
                        tokio::time::timeout(Duration::from_secs(2), runtime.shutdown_checked())
                            .await
                            .unwrap_or(Err("runtime shutdown unjoined"));
                    let joined = shutdown.is_ok();
                    (shutdown, joined)
                }
                ReaderRuntime::Retained(runtime) => {
                    let snapshot = runtime.snapshot();
                    drop(runtime);
                    (Ok(snapshot), false)
                }
            };
            owner_control.observation.send_modify(|state| {
                state.storage_joined = storage_joined;
                state.runtime_joined = runtime_joined;
                if outcome.as_ref().is_some_and(Result::is_err)
                    || !storage_joined
                    || shutdown.is_err()
                {
                    state
                        .error
                        .get_or_insert("reader owner/storage/runtime failed");
                }
            });
            let cleanup = if !storage_joined {
                Err(ReaderError::LostOwner)
            } else {
                shutdown.map_err(|_| ReaderError::LostOwner)
            };
            if let Some(outcome) = outcome {
                let outcome = match &cleanup {
                    Ok(snapshot) => outcome.map(|(tree, usage_tokens)| ReaderRun {
                        tree,
                        snapshot: *snapshot,
                        usage_tokens,
                    }),
                    Err(_) => Err(ReaderError::LostOwner),
                };
                let _ = result.send(outcome);
            }
            let _ = receipt.send(cleanup);
        });
        Ok((
            Self {
                control,
                result: Some(receive),
            },
            ReaderCleanup { receipt: cleanup },
        ))
    }
    pub fn observer(&self) -> ReaderObserver {
        ReaderObserver::new(self.control.observation.subscribe())
    }
    pub fn control(&self) -> ReaderControl {
        ReaderControl {
            control: self.control.clone(),
        }
    }
    pub fn cancel_child(&self, child: u8) -> Result<(), ReaderError> {
        if child >= 3 {
            return Err(ReaderError::Invalid("child identity"));
        }
        cancel(&self.control, Some(child));
        Ok(())
    }
    pub fn cancel_tree(&self) {
        cancel(&self.control, None);
    }
    pub async fn finish(mut self) -> Result<ReaderRun, ReaderError> {
        self.result
            .take()
            .unwrap()
            .await
            .map_err(|_| ReaderError::LostOwner)?
    }
}
impl Drop for ReaderParent {
    fn drop(&mut self) {
        cancel(&self.control, None);
    }
}
fn cancel(control: &Control, child: Option<u8>) {
    let mut state = control.state.lock().unwrap();
    for i in 0..3 {
        if child.is_none_or(|c| c as usize == i) && !state.accepted[i] {
            state.cancelled[i] = true;
        }
    }
    drop(state);
    control.changed.send_modify(|v| *v = v.wrapping_add(1));
}
fn cancelled(control: &Control, child: u8) -> bool {
    control.state.lock().unwrap().cancelled[child as usize]
}

struct Work {
    record: journal::Record,
    reply: oneshot::Sender<Result<Option<ReaderFact>, ReaderError>>,
}
async fn persist(
    sender: &mpsc::Sender<Work>,
    record: journal::Record,
) -> Result<Option<ReaderFact>, ReaderError> {
    let (reply, receive) = oneshot::channel();
    sender
        .send(Work { record, reply })
        .await
        .map_err(|_| ReaderError::Storage)?;
    receive.await.map_err(|_| ReaderError::Storage)?
}
fn journal_worker(
    mut journal: journal::Journal,
    mut receiver: mpsc::Receiver<Work>,
    control: Arc<Control>,
) {
    while let Some(work) = receiver.blocking_recv() {
        let mut record = work.record;
        let result = if let journal::Record::Terminal {
            child,
            id,
            state,
            response,
        } = &mut record
        {
            if cancelled(&control, *child) {
                *state = ReaderState::Cancelled;
                response.clear();
            }
            let mut fact = ReaderFact {
                child: *child,
                request_id: *id,
                state: copy_state(state),
            };
            let child_index = *child as usize;
            (|| {
                journal.append(&record)?;
                let mut gate = control.state.lock().unwrap();
                if gate.cancelled[child_index] {
                    fact.state = ReaderState::Cancelled;
                }
                gate.accepted[child_index] = true;
                drop(gate);
                journal.append(&journal::Record::Accept {
                    child: child_index as u8,
                    cancelled: fact.state == ReaderState::Cancelled,
                })?;
                control.publish(|state| state.accepted(&fact))?;
                Ok(Some(fact))
            })()
        } else {
            journal.append(&record).map(|()| None)
        };
        if result.is_err() {
            control.observation.send_modify(|state| {
                state.error.get_or_insert("journal acknowledgement failed");
            });
            cancel(&control, None);
        }
        let _ = work.reply.send(result);
    }
}
fn copy_state(state: &ReaderState) -> ReaderState {
    match state {
        ReaderState::Completed { summary } => ReaderState::Completed {
            summary: summary.to_owned(),
        },
        ReaderState::Failed(f) => ReaderState::Failed(*f),
        ReaderState::Cancelled => ReaderState::Cancelled,
        ReaderState::NotDispatched => ReaderState::NotDispatched,
        ReaderState::Interrupted => ReaderState::Interrupted,
    }
}
async fn coordinate(
    runtime: &Runtime,
    inputs: &ReaderInputs,
    control: &Arc<Control>,
    sender: &mpsc::Sender<Work>,
) -> Result<(ReaderTree, Option<u64>), ReaderError> {
    let (a, b, c) = tokio::join!(
        child(runtime, inputs, 0, control, sender),
        child(runtime, inputs, 1, control, sender),
        child(runtime, inputs, 2, control, sender)
    );
    let outcomes = [a?, b?, c?];
    let usage = outcomes
        .iter()
        .try_fold(0u64, |n, (_, u)| n.checked_add((*u)?));
    let children = outcomes.map(|(fact, _)| fact);
    let cancelled_mask = children.iter().fold(0, |mask, fact| {
        mask | if fact.state == ReaderState::Cancelled {
            1 << fact.child
        } else {
            0
        }
    });
    persist(sender, journal::Record::End(cancelled_mask)).await?;
    control.publish(|state| state.ended(&children))?;
    Ok((
        ReaderTree {
            session: inputs.session.to_owned(),
            generation: inputs.generation,
            children,
            parent_finished: true,
        },
        usage,
    ))
}
async fn child(
    runtime: &Runtime,
    inputs: &ReaderInputs,
    child: u8,
    control: &Arc<Control>,
    sender: &mpsc::Sender<Work>,
) -> Result<(ReaderFact, Option<u64>), ReaderError> {
    if cancelled(control, child) {
        return Ok((
            ReaderFact {
                child,
                request_id: None,
                state: ReaderState::NotDispatched,
            },
            Some(0),
        ));
    }
    let text = context(inputs, child)?;
    persist(sender, journal::Record::Started(child)).await?;
    let mut response = String::with_capacity(RESPONSE);
    let mut id = None;
    let mut usage = Some(0);
    let mut state = ReaderState::Cancelled;
    let submitted = {
        let gate = control.state.lock().unwrap();
        (!gate.cancelled[child as usize])
            .then(|| runtime.submit(&text, crate::SyntheticCost::new(1, 1).unwrap()))
    };
    drop(text);
    if let Some(submitted) = submitted {
        match submitted {
            Err(_) => state = ReaderState::Failed(ReaderFailure::Admission),
            Ok(mut handle) => {
                id = Some(handle.id());
                control.publish(|state| state.submitted(child, handle.id()))?;
                let mut changed = control.changed.subscribe();
                let mut overflow = false;
                let absolute = tokio::time::Instant::now() + Duration::from_secs(35);
                let mut limit = absolute;
                let mut cancellation_seen = false;
                let mut received = 0usize;
                loop {
                    if cancelled(control, child) || overflow {
                        handle.cancel();
                        if !cancellation_seen {
                            cancellation_seen = true;
                            limit =
                                absolute.min(tokio::time::Instant::now() + Duration::from_secs(6));
                        }
                    }
                    let delta = tokio::select! {
                        biased;
                        _ = changed.changed() => continue,
                        _ = tokio::time::sleep_until(limit) => return Err(ReaderError::LostOwner),
                        delta = handle.next_delta() => delta,
                    };
                    let Some(delta) = delta else { break };
                    if delta.len() > RESPONSE - response.len() {
                        overflow = true;
                        handle.cancel();
                    } else if !overflow && !cancelled(control, child) {
                        response.push_str(&delta);
                    }
                    received = received.saturating_add(delta.len()).min(RESPONSE);
                    control.publish(|state| state.progress(child, handle.id(), received))?;
                }
                let terminal = tokio::time::timeout_at(limit, handle.terminal())
                    .await
                    .map_err(|_| ReaderError::LostOwner)?;
                let duplicate = tokio::time::timeout_at(limit, handle.terminal())
                    .await
                    .map_err(|_| ReaderError::LostOwner)?
                    .is_some();
                usage = terminal
                    .as_ref()
                    .filter(|t| Some(t.id) == id && !duplicate)
                    .and_then(|t| t.synthetic_usage.add_to(Some(0)));
                if terminal.as_ref().is_none_or(|t| Some(t.id) != id) || duplicate {
                    return Err(ReaderError::LostOwner);
                }
                let (unknown_usage, unresolved_exposure) = handle
                    .accounting_receipt()
                    .map_err(|_| ReaderError::LostOwner)?;
                control.publish(|state| {
                    state.accounted(
                        child,
                        ReaderAccounting {
                            request_id: handle.id(),
                            unknown_usage,
                            unresolved_exposure,
                        },
                    )
                })?;
                state = if cancelled(control, child) {
                    ReaderState::Cancelled
                } else if overflow {
                    ReaderState::Failed(ReaderFailure::ResponseLimit)
                } else if duplicate
                    || !terminal.is_some_and(|t| Some(t.id) == id && t.outcome == Outcome::Success)
                {
                    ReaderState::Failed(ReaderFailure::Transport)
                } else {
                    match decode(&response, &inputs.session, inputs.generation, child) {
                        Ok(summary) => ReaderState::Completed { summary },
                        Err(_) => ReaderState::Failed(ReaderFailure::InvalidResponse),
                    }
                };
                drop(handle);
            }
        }
    }
    if !matches!(state, ReaderState::Completed { .. }) {
        response.clear();
    }
    let fact = persist(
        sender,
        journal::Record::Terminal {
            child,
            id,
            state,
            response,
        },
    )
    .await?
    .ok_or(ReaderError::Storage)?;
    Ok((fact, usage))
}

pub fn reconstruct_readers(
    root: &Path,
    session: &str,
    generation: u64,
) -> Result<ReaderTree, ReaderError> {
    journal::reconstruct(root, session, generation)
}
