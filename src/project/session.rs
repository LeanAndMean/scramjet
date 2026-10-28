use super::context::Goal;
use super::journal::*;
use super::*;
use crate::commands::project::{self as command, *};
use crate::{
    CodingAgent, CodingHistoryEnding, CommandCompletion, EditCancellation, OperationState,
    ReaderCleanup, ReaderInputs, ReaderParent, ReaderState, Runtime, Snapshot,
    readers::{ReaderLogical, ReaderSupervision},
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};
use std::time::Duration;
use tokio::sync::{Notify, mpsc, oneshot};

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

static INSTANCES: AtomicU64 = AtomicU64::new(1);
#[cfg(test)]
type Callback = Arc<dyn Fn(&'static str, bool) -> Result<(), CodingError> + Send + Sync>;
#[cfg(test)]
#[derive(Clone, Default)]
struct Hooks {
    callback: Option<Callback>,
    coding: Option<Arc<Mutex<Option<crate::coding::Checkpoint>>>>,
}
#[cfg(test)]
impl Hooks {
    fn hit(&self, point: &'static str, after: bool) -> Result<(), CodingError> {
        if let Some(callback) = &self.callback {
            callback(point, after)?;
        }
        Ok(())
    }
}
struct HostGate {
    returned: AtomicBool,
    changed: Notify,
}
impl HostGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            returned: AtomicBool::new(false),
            changed: Notify::new(),
        })
    }
    fn release(&self) {
        self.returned.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
    async fn wait(&self) {
        loop {
            let wait = self.changed.notified();
            if self.returned.load(Ordering::Acquire) {
                return;
            }
            wait.await;
        }
    }
}
struct WaitGuard {
    control: ProjectControl,
    acknowledged: bool,
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.control.stop();
        }
    }
}
pub struct ProjectSession {
    state: Arc<Mutex<command::State>>,
    sender: Option<mpsc::Sender<Action>>,
}
pub struct ProjectSettlement {
    state: Arc<Mutex<command::State>>,
    gate: Arc<HostGate>,
    close: Option<oneshot::Sender<()>>,
    receipt: Option<oneshot::Receiver<ProjectFinish>>,
}
pub struct ProjectFinish {
    pub runtime: Snapshot,
    pub logical_settled: bool,
    pub readers_joined: bool,
    pub coding_joined: bool,
    pub storage_joined: bool,
    pub runtime_joined: bool,
    pub history_acknowledged: bool,
    pub error: Option<Arc<str>>,
    retained_runtime: Option<Arc<Runtime>>,
}
impl ProjectFinish {
    pub fn retains_runtime_owner(&self) -> bool {
        self.retained_runtime.is_some()
    }
}
enum Action {
    Submit {
        id: ProjectInvocation,
        request: ProjectRequest,
        reply: oneshot::Sender<Result<ProjectSourceReceipt, CodingError>>,
    },
    Source {
        id: ProjectInvocation,
        report: ProjectReport,
        reply: oneshot::Sender<Result<Option<ProjectContinuation>, CodingError>>,
    },
    Resume {
        id: ProjectInvocation,
        reply: oneshot::Sender<Result<ProjectReady, CodingError>>,
    },
    Verify {
        id: ProjectInvocation,
        reply: oneshot::Sender<Result<ProjectVerificationReceipt, CodingError>>,
    },
    Verification {
        id: ProjectInvocation,
        report: ProjectReport,
        reply: oneshot::Sender<Result<(), CodingError>>,
    },
}
struct Prepared {
    captured: ProjectCaptured,
    project: RootIdentity,
    storage: File,
    inputs: ReaderInputs,
}
struct PreparedCoding {
    agent: CodingAgent,
    storage: RootIdentity,
}
enum FsWork {
    Begin {
        id: ProjectInvocation,
        request: ProjectRequest,
        reply: oneshot::Sender<Result<Arc<ProjectRecord>, CodingError>>,
    },
    Capture {
        id: ProjectInvocation,
        request: ProjectRequest,
        reply: oneshot::Sender<Result<Prepared, CodingError>>,
    },
    Readers {
        record: ProjectReaders,
        reply: oneshot::Sender<Result<Arc<ProjectRecord>, CodingError>>,
    },
    CodingStart {
        id: ProjectInvocation,
        goal: Goal,
        reply: oneshot::Sender<Result<PreparedCoding, CodingError>>,
    },
    CodingEnd {
        record: ProjectCoding,
        reply: oneshot::Sender<Result<Arc<ProjectRecord>, CodingError>>,
    },
    Record {
        record: ProjectRecord,
        release: bool,
        reply: oneshot::Sender<Result<Arc<ProjectRecord>, CodingError>>,
    },
    Verify {
        id: ProjectInvocation,
        reply: oneshot::Sender<Result<Arc<ProjectRecord>, CodingError>>,
    },
}
struct FsState {
    workspace: ProjectWorkspace,
    journal: Journal,
    captures: [Option<ProjectCapture>; 2],
    captured: [Option<ProjectCaptured>; 2],
    readers: [Option<ProjectReaders>; 2],
    coding: [Option<ProjectCoding>; 2],
    instance: u64,
    failed: bool,
    #[cfg(test)]
    hooks: Hooks,
}
fn storage_child(
    workspace: &ProjectWorkspace,
    prefix: &str,
    journey: u8,
) -> Result<File, CodingError> {
    let name = format!("{prefix}-{journey}");
    let directory = File::from(openat(
        &workspace.0.storage,
        name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    validate_directory(&directory, true)?;
    Ok(directory)
}
impl FsState {
    fn persist(&mut self, record: ProjectRecord) -> Result<Arc<ProjectRecord>, CodingError> {
        #[cfg(test)]
        let point = match &record {
            ProjectRecord::Header(_) => "Header",
            ProjectRecord::Requested(_) => "Requested",
            ProjectRecord::Captured(_) => "Captured",
            ProjectRecord::Readers(_) => "Readers",
            ProjectRecord::Coding(_) => "Coding",
            ProjectRecord::SourceReport(_) => "SourceReport",
            ProjectRecord::VerificationReady(_) => "Ready",
            ProjectRecord::VerificationRead(_) => "Read",
            ProjectRecord::VerificationReport(_) => "VerificationReport",
            ProjectRecord::Closure(_) => "Closure",
        };
        #[cfg(test)]
        self.hooks.hit(point, false)?;
        let durable = self.journal.append(record)?;
        #[cfg(test)]
        if let Err(error) = self.hooks.hit(point, true) {
            self.journal.poison();
            return Err(error);
        }
        Ok(durable)
    }
    fn capture(
        &mut self,
        id: ProjectInvocation,
        request: ProjectRequest,
    ) -> Result<Prepared, CodingError> {
        #[cfg(test)]
        self.hooks.hit("Capture", false)?;
        let capture = self.workspace.capture(request)?;
        let name = format!("readers-{}", id.journey);
        mkdirat(&self.workspace.0.storage, name.as_str(), Mode::RWXU)?;
        let storage = storage_child(&self.workspace, "readers", id.journey)?;
        self.workspace.0.storage.sync_all()?;
        let reader_session = format!("p-{}-{}-r", self.instance, id.journey);
        let coding_session = format!("p-{}-{}-c", self.instance, id.journey);
        let inputs = ReaderInputs::new(
            &reader_session,
            id.journey as u64,
            capture.files.each_ref().map(|f| f.content.as_str()),
        )
        .map_err(|_| CodingError::Invalid("complete reader input boundary"))?;
        let captured = ProjectCaptured {
            journey: id.journey,
            request: capture.request.clone(),
            files: capture.files.clone(),
            desired: capture.desired.clone(),
            reader_session,
            coding_session,
            reader_storage: identity(&storage)?,
        };
        self.persist(ProjectRecord::Captured(captured.clone()))?;
        self.captures[id.journey as usize - 1] = Some(capture);
        self.captured[id.journey as usize - 1] = Some(captured.clone());
        Ok(Prepared {
            captured,
            project: self.workspace.root_identity(),
            storage,
            inputs,
        })
    }
    fn readers(&mut self, record: ProjectReaders) -> Result<Arc<ProjectRecord>, CodingError> {
        let captured = self.captured[record.journey as usize - 1]
            .as_ref()
            .ok_or(CodingError::Stale)?;
        if record.ended {
            let inputs = ReaderInputs::new(
                &captured.reader_session,
                record.journey as u64,
                captured.files.each_ref().map(|f| f.content.as_str()),
            )
            .unwrap();
            let root = storage_child(&self.workspace, "readers", record.journey)?;
            let tree = crate::readers::journal::reconstruct_descriptor(root, &inputs)
                .map_err(|_| CodingError::History("reader artifact acknowledgement mismatch"))?;
            if !tree.parent_finished || tree.children.each_ref().map(reader_fact) != record.facts {
                return Err(CodingError::History(
                    "reader accepted facts differ from artifact",
                ));
            }
        }
        let durable = self.persist(ProjectRecord::Readers(record.clone()))?;
        let index = record.journey as usize - 1;
        self.readers[index] = Some(record);
        Ok(durable)
    }
    fn start_coding(
        &mut self,
        id: ProjectInvocation,
        goal: Goal,
    ) -> Result<PreparedCoding, CodingError> {
        let capture = self.captures[id.journey as usize - 1]
            .as_ref()
            .ok_or(CodingError::Stale)?;
        let readers = self.readers[id.journey as usize - 1]
            .as_ref()
            .ok_or(CodingError::Stale)?;
        if !readers.ended
            || !readers.joined
            || !goal.matches_capture(capture)
            || goal.evidence.reader_session
                != self.captured[id.journey as usize - 1]
                    .as_ref()
                    .unwrap()
                    .reader_session
            || goal.evidence.generation != id.journey as u64
            || goal
                .evidence
                .readers
                .iter()
                .zip(&readers.facts)
                .any(|(e, r)| {
                    e.child != r.child
                        || Some(e.request) != r.request
                        || Some(e.summary.as_str()) != r.summary.as_deref()
                })
        {
            return Err(CodingError::Stale);
        }
        let session_name = &self.captured[id.journey as usize - 1]
            .as_ref()
            .unwrap()
            .coding_session;
        #[cfg(test)]
        self.hooks.hit("CodingStart", false)?;
        let session =
            self.workspace
                .coding_session(capture.clone_for_coding(), session_name, id.journey)?;
        #[cfg(test)]
        let session = {
            let mut session = session;
            if let Some(checkpoint) = self
                .hooks
                .coding
                .as_ref()
                .and_then(|point| point.lock().unwrap().take())
            {
                session.set_checkpoint(checkpoint);
            }
            session
        };
        let storage = identity(&storage_child(&self.workspace, "coding", id.journey)?)?;
        let agent = CodingAgent::start_project(session, EditCancellation::default(), goal)?;
        Ok(PreparedCoding { agent, storage })
    }
    fn end_coding(&mut self, mut record: ProjectCoding) -> Result<Arc<ProjectRecord>, CodingError> {
        let index = record.journey as usize - 1;
        let captured = self.captured[index].as_ref().ok_or(CodingError::Stale)?;
        let history = crate::coding::history::from_storage(
            &storage_child(&self.workspace, "coding", record.journey)?,
            &captured.coding_session,
            self.workspace.root_identity(),
            true,
        )?;
        if record.completion == "Completed" {
            let valid = history.ending() == CodingHistoryEnding::RecordedFinal
                && history.operations().len() == 1
                && history.operations()[0].state == OperationState::Completed;
            let actual = self.captures[index]
                .as_ref()
                .unwrap()
                .check_coding(true)
                .and_then(|()| read_file(&self.workspace.0.root, captured.request.target()));
            match actual {
                Ok(file) if valid && record.edited.as_ref() == Some(&file) => {
                    record.edited = Some(file)
                }
                _ => {
                    record.completion = "Incomplete".into();
                    record.edited = None;
                    record.error =
                        Some("edited project recheck/history failed after coding".into());
                }
            }
        }
        super::history::validate_coding(
            captured,
            self.readers[index].as_ref().unwrap(),
            &record,
            &history,
        )?;
        let durable = self.persist(ProjectRecord::Coding(record.clone()))?;
        self.coding[index] = Some(record);
        Ok(durable)
    }
    fn verify(&mut self, id: ProjectInvocation) -> Result<Arc<ProjectRecord>, CodingError> {
        #[cfg(test)]
        self.hooks.hit("Verify", false)?;
        let index = id.journey as usize - 1;
        let capture = self.captures[index].as_ref().ok_or(CodingError::Stale)?;
        let coding = self.coding[index].as_ref().ok_or(CodingError::Stale)?;
        let result = capture
            .check_coding(true)
            .and_then(|()| read_file(&self.workspace.0.root, capture.request.target()));
        let (file, matched, error) = match result {
            Ok(file) => {
                let matched = coding.edited.as_ref() == Some(&file);
                (
                    Some(file),
                    matched,
                    (!matched).then(|| "edited target identity/content mismatch".into()),
                )
            }
            Err(_) => (
                None,
                false,
                Some("verification project/metadata/read failure".into()),
            ),
        };
        self.persist(ProjectRecord::VerificationRead(ProjectRead {
            journey: id.journey,
            file,
            matched,
            error,
        }))
    }
    fn work(&mut self, work: FsWork) {
        match work {
            FsWork::Begin { id, request, reply } => {
                let result = self.journal.reserve_journey().and_then(|()| {
                    self.persist(ProjectRecord::Requested(ProjectRequested {
                        journey: id.journey,
                        request,
                    }))
                });
                let _ = reply.send(result);
            }
            FsWork::Capture { id, request, reply } => {
                let _ = reply.send(self.capture(id, request));
            }
            FsWork::Readers { record, reply } => {
                let _ = reply.send(self.readers(record));
            }
            FsWork::CodingStart { id, goal, reply } => {
                let _ = reply.send(self.start_coding(id, goal));
            }
            FsWork::CodingEnd { record, reply } => {
                let _ = reply.send(self.end_coding(record));
            }
            FsWork::Record {
                record,
                release,
                reply,
            } => {
                let result = (|| {
                    if matches!(record, ProjectRecord::VerificationReady(_)) {
                        self.workspace.0.validate()?;
                    }
                    self.persist(record)
                })();
                if release {
                    self.journal.release_journey();
                }
                let _ = reply.send(result);
            }
            FsWork::Verify { id, reply } => {
                let _ = reply.send(self.verify(id));
            }
        }
    }
}
struct FsOwner {
    sender: Option<SyncSender<FsWork>>,
    worker: Option<std::thread::JoinHandle<Option<FsState>>>,
    gate: Arc<HostGate>,
}
impl Drop for FsOwner {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let gate = self.gate.clone();
            tokio::spawn(async move {
                gate.wait().await;
                let _ = tokio::task::spawn_blocking(move || worker.join()).await;
            });
        }
    }
}
struct AgentOwner {
    agent: Option<CodingAgent>,
    gate: Arc<HostGate>,
}
impl Drop for AgentOwner {
    fn drop(&mut self) {
        if let Some(agent) = self.agent.take() {
            agent.control().stop();
            let gate = self.gate.clone();
            tokio::spawn(async move {
                gate.wait().await;
                let _ = tokio::task::spawn_blocking(move || agent.finish()).await;
            });
        }
    }
}
struct ReaderOwner {
    parent: ReaderParent,
    cleanup: Option<ReaderCleanup>,
    release: Option<oneshot::Sender<()>>,
    gate: Arc<HostGate>,
}
impl ReaderOwner {
    async fn join(&mut self) -> Result<Snapshot, CodingError> {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        self.cleanup
            .take()
            .unwrap()
            .wait()
            .await
            .map_err(|_| CodingError::Invalid("reader storage join unavailable"))
    }
}
impl Drop for ReaderOwner {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            self.parent.cancel_tree();
            let release = self.release.take();
            let gate = self.gate.clone();
            tokio::spawn(async move {
                gate.wait().await;
                drop(release);
                let _ = cleanup.wait().await;
            });
        }
    }
}
async fn filesystem<T>(
    owner: &FsOwner,
    build: impl FnOnce(oneshot::Sender<Result<T, CodingError>>) -> FsWork,
) -> Result<T, CodingError> {
    let (reply, receive) = oneshot::channel();
    owner
        .sender
        .as_ref()
        .ok_or(CodingError::Stale)?
        .try_send(build(reply))
        .map_err(|_| CodingError::Invalid("project storage owner unavailable"))?;
    receive
        .await
        .map_err(|_| CodingError::Invalid("project storage acknowledgement lost"))?
}
pub(super) fn reader_fact(fact: &crate::ReaderFact) -> ProjectReaderFact {
    let (state, summary) = match &fact.state {
        ReaderState::Completed { summary } => ("Completed".into(), Some(summary.clone())),
        ReaderState::Failed(reason) => (format!("Failed:{reason:?}"), None),
        other => (format!("{other:?}"), None),
    };
    ProjectReaderFact {
        child: fact.child,
        request: fact.request_id.map(|id| id.0),
        state,
        summary,
    }
}
fn reader_record(id: ProjectInvocation, logical: &ReaderLogical, joined: bool) -> ProjectReaders {
    let facts = std::array::from_fn(|index| {
        logical.observation.children[index]
            .fact
            .as_ref()
            .map(reader_fact)
            .unwrap_or(ProjectReaderFact {
                child: index as u8,
                request: logical.observation.children[index]
                    .request_id
                    .map(|id| id.0),
                state: "Interrupted".into(),
                summary: None,
            })
    });
    ProjectReaders {
        journey: id.journey,
        ended: logical.observation.tree_ended,
        joined,
        facts,
        error: logical
            .outcome
            .is_err()
            .then(|| "reader logical acknowledgement failed".into()),
    }
}
struct StageJoins {
    readers: bool,
    coding: bool,
}
struct ActorChannels {
    initialization: oneshot::Receiver<Result<Arc<ProjectRecord>, CodingError>>,
    actions: mpsc::Receiver<Action>,
    closing: oneshot::Receiver<()>,
    receipt: oneshot::Sender<ProjectFinish>,
}

async fn source(
    state: &Arc<Mutex<command::State>>,
    id: ProjectInvocation,
    request: ProjectRequest,
    runtime: &Arc<Runtime>,
    fs: &FsOwner,
    gate: &Arc<HostGate>,
    stages: &mut StageJoins,
) -> Result<ProjectSourceReceipt, CodingError> {
    let requested = filesystem(fs, |reply| FsWork::Begin {
        id,
        request: request.clone(),
        reply,
    })
    .await?;
    command::push(state, requested);
    if state.lock().unwrap().cancelled {
        return Err(CodingError::Cancelled);
    }
    let prepared = filesystem(fs, |reply| FsWork::Capture { id, request, reply }).await?;
    command::push(
        state,
        Arc::new(ProjectRecord::Captured(prepared.captured.clone())),
    );
    let (logical, receive) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (parent, cleanup) = command::launch_readers(
        state,
        id,
        runtime.clone(),
        prepared.storage,
        prepared.inputs,
        ReaderSupervision {
            logical,
            release: released,
        },
    )?;
    let mut owner = ReaderOwner {
        parent,
        cleanup: Some(cleanup),
        release: Some(release),
        gate: gate.clone(),
    };
    let logical = receive
        .await
        .map_err(|_| CodingError::Invalid("reader logical receipt lost"))?;
    let capture_goal = logical.outcome.as_ref().ok().filter(|(tree, _)| {
        tree.children
            .iter()
            .all(|f| matches!(f.state, ReaderState::Completed { .. }))
    });
    let viable = capture_goal.is_some() && !state.lock().unwrap().cancelled;
    if !viable {
        command::fail(state, "readers did not produce a coding-admissible result");
        gate.wait().await;
    }
    let joined = owner.join().await;
    stages.readers &= joined.is_ok();
    let record = reader_record(id, &logical, joined.is_ok());
    let durable = filesystem(fs, |reply| FsWork::Readers { record, reply }).await?;
    command::push(state, durable);
    drop(owner);
    joined?;
    if !viable || state.lock().unwrap().cancelled {
        return Err(CodingError::Cancelled);
    }
    let goal = Goal::from_readers(&prepared.captured, prepared.project, &logical)?;
    let captured = prepared.captured;
    let prepared = filesystem(fs, |reply| FsWork::CodingStart { id, goal, reply }).await?;
    let observer = prepared.agent.observer();
    let mut agent = AgentOwner {
        agent: Some(prepared.agent),
        gate: gate.clone(),
    };
    if let Err(error) = command::install_coding(
        state,
        id,
        observer.clone(),
        agent.agent.as_ref().unwrap().control(),
    ) {
        gate.wait().await;
        let owned = agent.agent.take().unwrap();
        let joined = tokio::task::spawn_blocking(move || owned.finish()).await;
        stages.coding &= matches!(joined, Ok(Ok(())));
        return Err(error);
    }
    let result = agent.agent.as_mut().unwrap().run_unjoined(runtime).await;
    if result.is_err() || state.lock().unwrap().cancelled {
        gate.wait().await;
    }
    let owned = agent.agent.take().unwrap();
    let joined = tokio::task::spawn_blocking(move || owned.finish()).await;
    let joined = matches!(joined, Ok(Ok(())));
    stages.coding &= joined;
    let observation = observer.snapshot();
    let completed = joined
        && observation.worker_joined
        && observation.worker_terminated
        && observation.effect == Some(OperationState::Completed)
        && observation.verified
        && observation.verified_identity.is_some()
        && observation.run_ended
        && observation.error.is_none()
        && result.as_ref().is_ok_and(|run| {
            run.turns == 4
                && run.request_ids.len() == 4
                && run.edit_operation == 1
                && observation.final_summary.as_deref() == Some(run.summary.as_str())
                && observation.attempts[..4]
                    .iter()
                    .zip(&run.request_ids)
                    .all(|(slot, id)| {
                        slot.recorded
                            && slot.request_id == Some(*id)
                            && slot.outcome == Some(crate::Outcome::Success)
                    })
        });
    let disposition = if completed {
        "Completed"
    } else if !joined
        || observation.effect == Some(OperationState::Uncertain)
        || matches!(
            &result,
            Err(CodingError::Io(_)
                | CodingError::History(_)
                | CodingError::Poisoned
                | CodingError::Cancelled)
        )
    {
        "Incomplete"
    } else {
        "Failed"
    };
    let record = ProjectCoding {
        journey: id.journey,
        owner: observation.owner,
        joined,
        completion: disposition.into(),
        effect: observation
            .effect
            .map_or_else(|| "None".into(), |effect| format!("{effect:?}")),
        requests: std::array::from_fn(|i| observation.attempts[i].request_id.map(|id| id.0)),
        summary: observation.final_summary.as_deref().map(str::to_owned),
        error: (!completed).then(|| "coding execution did not establish source success".into()),
        storage: prepared.storage,
        edited: if completed {
            Some(ProjectFile {
                name: captured.request.target().into(),
                identity: observation.verified_identity.unwrap(),
                mode: captured.files[0].mode,
                content: captured.desired,
            })
        } else {
            None
        },
    };
    let durable = filesystem(fs, |reply| FsWork::CodingEnd { record, reply }).await?;
    let ProjectRecord::Coding(record) = durable.as_ref() else {
        unreachable!()
    };
    let execution = Arc::new(record.clone());
    command::push(state, durable);
    let mut current = state.lock().unwrap();
    current.coding_control = None;
    current.reader_control = None;
    current.phase = ProjectPhase::AwaitingSourceReport;
    current.runtime = Some(runtime.snapshot());
    drop(current);
    Ok(ProjectSourceReceipt {
        state: state.clone(),
        id,
        execution: Some(execution),
    })
}

impl ProjectSession {
    pub fn start(
        runtime: Runtime,
        project_path: &Path,
        storage_path: &Path,
        session: &str,
    ) -> (Self, ProjectSettlement) {
        Self::start_inner(
            runtime,
            project_path,
            storage_path,
            session,
            #[cfg(test)]
            Hooks::default(),
        )
    }
    fn start_inner(
        runtime: Runtime,
        project_path: &Path,
        storage_path: &Path,
        session: &str,
        #[cfg(test)] hooks: Hooks,
    ) -> (Self, ProjectSettlement) {
        let instance = INSTANCES
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .unwrap_or(0);
        let valid_session = crate::coding::validate_session(session).is_ok();
        let session = if valid_session {
            Arc::<str>::from(session)
        } else {
            Arc::from("invalid-session")
        };
        let state = command::new(instance, session.clone());
        let gate = HostGate::new();
        let (sender, actions) = mpsc::channel(1);
        let (close, closing) = oneshot::channel();
        let (receipt, finished) = oneshot::channel();
        let (fs_sender, fs_receiver) = sync_channel(1);
        let (initialized, initialization) = oneshot::channel();
        let project_path =
            (project_path.as_os_str().len() <= 4096).then(|| project_path.to_owned());
        let storage_path =
            (storage_path.as_os_str().len() <= 4096).then(|| storage_path.to_owned());
        let worker = std::thread::Builder::new()
            .name("project-storage".into())
            .spawn(move || {
                let setup = (|| {
                    if !valid_session || instance == 0 {
                        return Err(CodingError::Invalid("invalid/exhausted project identity"));
                    }
                    #[cfg(test)]
                    hooks.hit("Setup", false)?;
                    let workspace = ProjectWorkspace::open(
                        &project_path.ok_or(CodingError::Invalid("project path ceiling"))?,
                        &storage_path.ok_or(CodingError::Invalid("storage path ceiling"))?,
                    )?;
                    let journal = Journal::create(&workspace, &session, instance)?;
                    let header = Arc::new(ProjectRecord::Header(ProjectHeader {
                        version: 1,
                        session: session.to_string(),
                        instance,
                        project_path: workspace.0.path.to_str().unwrap().into(),
                        project: workspace.root_identity(),
                        storage: identity(&workspace.0.storage)?,
                    }));
                    Ok::<_, CodingError>((
                        FsState {
                            workspace,
                            journal,
                            captures: std::array::from_fn(|_| None),
                            captured: std::array::from_fn(|_| None),
                            readers: std::array::from_fn(|_| None),
                            coding: std::array::from_fn(|_| None),
                            instance,
                            failed: false,
                            #[cfg(test)]
                            hooks,
                        },
                        header,
                    ))
                })();
                let (mut fs, header) = match setup {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = initialized.send(Err(error));
                        return None;
                    }
                };
                let _ = initialized.send(Ok(header));
                while let Ok(work) = fs_receiver.recv() {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fs.work(work)))
                        .is_err()
                    {
                        fs.failed = true;
                        fs.journal.poison();
                        break;
                    }
                }
                #[cfg(test)]
                if !matches!(
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fs
                        .hooks
                        .hit("ThreadExit", false))),
                    Ok(Ok(()))
                ) {
                    fs.failed = true;
                    fs.journal.poison();
                }
                Some(fs)
            });
        let fs = FsOwner {
            sender: Some(fs_sender),
            worker: worker.ok(),
            gate: gate.clone(),
        };
        let actor_state = state.clone();
        let actor_gate = gate.clone();
        tokio::spawn(async move {
            supervise(
                Arc::new(runtime),
                fs,
                ActorChannels {
                    initialization,
                    actions,
                    closing,
                    receipt,
                },
                actor_state,
                actor_gate,
            )
            .await;
        });
        (
            Self {
                state: state.clone(),
                sender: Some(sender),
            },
            ProjectSettlement {
                state,
                gate,
                close: Some(close),
                receipt: Some(finished),
            },
        )
    }
    pub fn observer(&self) -> ProjectObserver {
        ProjectObserver(self.state.clone())
    }
    pub fn control(&self) -> ProjectControl {
        ProjectControl(self.state.clone())
    }
    async fn action<T>(
        &mut self,
        build: impl FnOnce(oneshot::Sender<Result<T, CodingError>>) -> Action,
    ) -> Result<T, CodingError> {
        let (reply, receive) = oneshot::channel();
        let mut guard = WaitGuard {
            control: self.control(),
            acknowledged: false,
        };
        self.sender
            .as_ref()
            .ok_or(CodingError::Stale)?
            .try_send(build(reply))
            .map_err(|_| CodingError::Invalid("project command owner unavailable"))?;
        let result = receive
            .await
            .map_err(|_| CodingError::Invalid("project command acknowledgement lost"))?;
        guard.acknowledged = true;
        result
    }
    pub async fn submit(&mut self, json: &str) -> Result<ProjectSourceReceipt, CodingError> {
        let request = ProjectRequest::parse(json)?;
        let id = command::admit(&self.state)?;
        self.action(|reply| Action::Submit { id, request, reply })
            .await
    }
    pub async fn report_source(
        &mut self,
        receipt: &mut ProjectSourceReceipt,
        completion: CommandCompletion,
        summary: &str,
    ) -> Result<Option<ProjectContinuation>, CodingError> {
        let report = command::prepare_source(&self.state, receipt, completion, summary)?;
        let id = receipt.id;
        self.action(|reply| Action::Source { id, report, reply })
            .await
    }
    pub async fn resume(
        &mut self,
        continuation: ProjectContinuation,
    ) -> Result<ProjectReady, CodingError> {
        let id = command::prepare_resume(&self.state, continuation)?;
        self.action(|reply| Action::Resume { id, reply }).await
    }
    pub async fn verify(
        &mut self,
        ready: ProjectReady,
    ) -> Result<ProjectVerificationReceipt, CodingError> {
        let id = command::prepare_verify(&self.state, ready)?;
        self.action(|reply| Action::Verify { id, reply }).await
    }
    pub async fn report_verification(
        &mut self,
        receipt: &mut ProjectVerificationReceipt,
        completion: CommandCompletion,
        summary: &str,
    ) -> Result<(), CodingError> {
        let report =
            command::prepare_verification_report(&self.state, receipt, completion, summary)?;
        let id = receipt.id;
        self.action(|reply| Action::Verification { id, report, reply })
            .await
    }
}
impl Drop for ProjectSession {
    fn drop(&mut self) {
        self.control().stop();
        self.sender.take();
    }
}
impl ProjectSettlement {
    pub fn release_host(&self) {
        self.gate.release();
    }
    pub async fn finish(mut self) -> Result<ProjectFinish, CodingError> {
        self.release_host();
        ProjectControl(self.state.clone()).close();
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
        self.receipt
            .take()
            .unwrap()
            .await
            .map_err(|_| CodingError::Invalid("project final owner acknowledgement lost"))
    }
}
impl Drop for ProjectSettlement {
    fn drop(&mut self) {
        self.release_host();
        ProjectControl(self.state.clone()).close();
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
    }
}
async fn supervise(
    runtime: Arc<Runtime>,
    mut fs: FsOwner,
    channels: ActorChannels,
    state: Arc<Mutex<command::State>>,
    gate: Arc<HostGate>,
) {
    let ActorChannels {
        initialization,
        mut actions,
        mut closing,
        receipt,
    } = channels;
    let control = ProjectControl(state.clone());
    let deadline_control = control.clone();
    let deadline = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(120)).await;
        deadline_control.stop();
    });
    let mut stages = StageJoins {
        readers: true,
        coding: true,
    };
    match initialization.await {
        Ok(Ok(header)) => command::push(&state, header),
        _ => command::fail(&state, "project setup acknowledgement failed"),
    }
    loop {
        let action = tokio::select! { action = actions.recv() => action, _ = &mut closing => None };
        let Some(action) = action else {
            break;
        };
        let result = match action {
            Action::Submit { id, request, reply } => {
                let outcome = source(&state, id, request, &runtime, &fs, &gate, &mut stages).await;
                let failed = outcome.is_err();
                let _ = reply.send(outcome);
                if failed {
                    Err(CodingError::Invalid("project source execution failed"))
                } else {
                    Ok(())
                }
            }
            Action::Source { id, report, reply } => {
                let outcome = async {
                    let durable = filesystem(&fs, |reply| FsWork::Record {
                        record: ProjectRecord::SourceReport(report.clone()),
                        release: !report.paused,
                        reply,
                    })
                    .await?;
                    command::push(&state, durable);
                    command::commit_source(&state, id, &report)
                }
                .await;
                let failed = outcome.is_err();
                let _ = reply.send(outcome);
                if failed {
                    Err(CodingError::Invalid("source report acknowledgement failed"))
                } else {
                    Ok(())
                }
            }
            Action::Resume { id, reply } => {
                let outcome = async {
                    let durable = filesystem(&fs, |reply| FsWork::Record {
                        record: ProjectRecord::VerificationReady(ProjectReadyRecord {
                            journey: id.journey,
                        }),
                        release: false,
                        reply,
                    })
                    .await?;
                    command::push(&state, durable);
                    {
                        let mut current = state.lock().unwrap();
                        command::check(&current, id, ProjectPhase::VerificationAdmitting)?;
                        if current.cancelled || current.closed {
                            return Err(CodingError::Cancelled);
                        }
                        current.phase = ProjectPhase::VerificationReady;
                    }
                    Ok(ProjectReady {
                        state: state.clone(),
                        id,
                    })
                }
                .await;
                let failed = outcome.is_err();
                let _ = reply.send(outcome);
                if failed {
                    Err(CodingError::Invalid(
                        "verification admission acknowledgement failed",
                    ))
                } else {
                    Ok(())
                }
            }
            Action::Verify { id, reply } => {
                let outcome = async {
                    if state.lock().unwrap().cancelled {
                        return Err(CodingError::Cancelled);
                    }
                    let durable = filesystem(&fs, |reply| FsWork::Verify { id, reply }).await?;
                    let ProjectRecord::VerificationRead(read) = durable.as_ref() else {
                        unreachable!()
                    };
                    let execution = Arc::new(read.clone());
                    command::push(&state, durable);
                    state.lock().unwrap().phase = ProjectPhase::AwaitingVerificationReport;
                    Ok(ProjectVerificationReceipt {
                        state: state.clone(),
                        id,
                        execution: Some(execution),
                    })
                }
                .await;
                let failed = outcome.is_err();
                let _ = reply.send(outcome);
                if failed {
                    Err(CodingError::Invalid(
                        "verification read acknowledgement failed",
                    ))
                } else {
                    Ok(())
                }
            }
            Action::Verification { id, report, reply } => {
                let outcome = async {
                    let durable = filesystem(&fs, |reply| FsWork::Record {
                        record: ProjectRecord::VerificationReport(report.clone()),
                        release: true,
                        reply,
                    })
                    .await?;
                    command::push(&state, durable);
                    command::commit_verification(&state, id, &report)
                }
                .await;
                let failed = outcome.is_err();
                let _ = reply.send(outcome);
                if failed {
                    Err(CodingError::Invalid(
                        "verification report acknowledgement failed",
                    ))
                } else {
                    Ok(())
                }
            }
        };
        if result.is_err() {
            command::fail(
                &state,
                "project execution/history acknowledgement incomplete",
            );
        }
    }
    control.close();
    gate.wait().await;
    deadline.abort();
    let _ = deadline.await;
    fs.sender.take();
    let worker = fs.worker.take();
    let joined = match worker {
        Some(worker) => Some(tokio::task::spawn_blocking(move || worker.join()).await),
        None => None,
    };
    let (storage_joined, retained_fs) = match joined {
        Some(Ok(Ok(state))) => (true, state),
        _ => (false, None),
    };
    let (snapshot, runtime_joined, retained_runtime) = match Arc::try_unwrap(runtime) {
        Ok(runtime) => {
            let before = runtime.snapshot();
            match runtime.shutdown_checked().await {
                Ok(snapshot) => (snapshot, true, None),
                Err(_) => (before, false, None),
            }
        }
        Err(runtime) => (runtime.snapshot(), false, Some(runtime)),
    };
    if retained_fs.as_ref().is_some_and(|fs| fs.failed) {
        command::fail(&state, "project storage owner panicked/failed at exit");
    }
    let logical_settled = command::logical_settled(&state.lock().unwrap());
    let closure = ProjectClosure {
        cancelled: state.lock().unwrap().cancelled,
        logical_settled,
        readers_joined: stages.readers,
        coding_joined: stages.coding,
        runtime_joined,
        attempts: snapshot.attempts,
        unknown_usage: snapshot.unknown_usage,
        unresolved: snapshot.unresolved,
    };
    let final_record = if let Some(mut fs) = retained_fs {
        tokio::task::spawn_blocking(move || {
            fs.journal.release_journey();
            fs.persist(ProjectRecord::Closure(closure))
        })
        .await
        .ok()
        .and_then(Result::ok)
    } else {
        None
    };
    let history_acknowledged = final_record.is_some();
    if let Some(record) = final_record {
        command::push(&state, record);
    }
    let mut current = state.lock().unwrap();
    current.runtime = Some(snapshot);
    current.reader_control = None;
    current.coding_control = None;
    current.settled = logical_settled
        && storage_joined
        && stages.readers
        && stages.coding
        && runtime_joined
        && history_acknowledged;
    if !logical_settled {
        current.phase = ProjectPhase::Incomplete;
    }
    let error: Option<Arc<str>> = (!current.settled)
        .then(|| Arc::from("project stage/storage/runtime/history settlement unacknowledged"));
    if let Some(error) = &error {
        current.error.get_or_insert_with(|| error.clone());
    }
    drop(current);
    let _ = receipt.send(ProjectFinish {
        runtime: snapshot,
        logical_settled,
        readers_joined: stages.readers,
        coding_joined: stages.coding,
        storage_joined,
        runtime_joined,
        history_acknowledged,
        error,
        retained_runtime,
    });
}
