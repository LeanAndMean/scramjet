use super::CommandCompletion;
use crate::readers::ReaderSupervision;
use crate::{
    CodingControl, CodingDecision, CodingError, CodingObservation, CodingObserver, CodingProposal,
    ReaderControl, ReaderInputs, ReaderObservation, ReaderObserver, ReaderParent, Runtime,
    Snapshot,
    project::journal::{ProjectCoding, ProjectRead, ProjectRecord, ProjectReport},
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectPhase {
    Idle,
    Preparing,
    Readers,
    Coding,
    AwaitingSourceReport,
    SourceReporting,
    Paused,
    VerificationReady,
    Verifying,
    AwaitingVerificationReport,
    VerificationReporting,
    Verified,
    Failed,
    Incomplete,
    VerificationAdmitting,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectInvocation {
    pub(crate) instance: u64,
    pub(crate) journey: u8,
}
impl ProjectInvocation {
    pub fn journey(&self) -> u8 {
        self.journey
    }
}
#[derive(Clone, Debug)]
pub struct ProjectLiveJourney {
    pub readers: Option<ReaderObservation>,
    pub coding: Option<Arc<CodingObservation>>,
}
#[derive(Clone, Debug)]
pub struct ProjectSnapshot {
    pub instance: u64,
    pub session: Arc<str>,
    pub current: Option<ProjectInvocation>,
    pub phase: ProjectPhase,
    pub cancelled: bool,
    pub closed: bool,
    pub settled: bool,
    pub records: Vec<Arc<ProjectRecord>>,
    pub journeys: [ProjectLiveJourney; 2],
    pub runtime: Option<Snapshot>,
    pub error: Option<Arc<str>>,
}
pub(crate) struct State {
    pub instance: u64,
    pub session: Arc<str>,
    pub current: Option<ProjectInvocation>,
    pub phase: ProjectPhase,
    pub cancelled: bool,
    pub closed: bool,
    pub settled: bool,
    pub records: Vec<Arc<ProjectRecord>>,
    pub readers: [Option<ReaderObserver>; 2],
    pub coding: [Option<CodingObserver>; 2],
    pub reader_control: Option<ReaderControl>,
    pub coding_control: Option<CodingControl>,
    pub runtime: Option<Snapshot>,
    pub error: Option<Arc<str>>,
}
#[derive(Clone)]
pub struct ProjectObserver(pub(crate) Arc<Mutex<State>>);
#[derive(Clone)]
pub struct ProjectControl(pub(crate) Arc<Mutex<State>>);
pub struct ProjectSourceReceipt {
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) id: ProjectInvocation,
    pub(crate) execution: Option<Arc<ProjectCoding>>,
}
pub struct ProjectContinuation {
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) id: ProjectInvocation,
}
pub struct ProjectReady {
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) id: ProjectInvocation,
}
pub struct ProjectVerificationReceipt {
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) id: ProjectInvocation,
    pub(crate) execution: Option<Arc<ProjectRead>>,
}
pub(crate) fn new(instance: u64, session: Arc<str>) -> Arc<Mutex<State>> {
    Arc::new(Mutex::new(State {
        instance,
        session,
        current: None,
        phase: ProjectPhase::Idle,
        cancelled: false,
        closed: false,
        settled: false,
        records: Vec::with_capacity(32),
        readers: std::array::from_fn(|_| None),
        coding: std::array::from_fn(|_| None),
        reader_control: None,
        coding_control: None,
        runtime: None,
        error: None,
    }))
}
impl ProjectObserver {
    pub fn snapshot(&self) -> ProjectSnapshot {
        let state = self.0.lock().unwrap();
        let readers = state.readers.clone();
        let coding = state.coding.clone();
        let mut snapshot = ProjectSnapshot {
            instance: state.instance,
            session: state.session.clone(),
            current: state.current,
            phase: state.phase,
            cancelled: state.cancelled,
            closed: state.closed,
            settled: state.settled,
            records: state.records.clone(),
            journeys: std::array::from_fn(|_| ProjectLiveJourney {
                readers: None,
                coding: None,
            }),
            runtime: state.runtime,
            error: state.error.clone(),
        };
        drop(state);
        for index in 0..2 {
            snapshot.journeys[index].readers =
                readers[index].as_ref().map(ReaderObserver::snapshot);
            snapshot.journeys[index].coding = coding[index].as_ref().map(CodingObserver::snapshot);
        }
        snapshot
    }
}
impl ProjectControl {
    pub fn stop(&self) {
        let mut state = self.0.lock().unwrap();
        if state.settled {
            return;
        }
        state.cancelled = true;
        if let Some(control) = &state.reader_control {
            control.cancel_owned_tree();
        }
        if let Some(control) = &state.coding_control {
            control.stop();
        }
    }
    pub fn decide(
        &self,
        proposal: &CodingProposal,
        decision: CodingDecision,
    ) -> Result<(), CodingError> {
        let state = self.0.lock().unwrap();
        if state.closed || state.cancelled || state.phase != ProjectPhase::Coding {
            return Err(CodingError::Cancelled);
        }
        state
            .coding_control
            .as_ref()
            .ok_or(CodingError::Stale)?
            .decide(proposal, decision)
    }
    pub fn cancel_child(
        &self,
        session: &str,
        generation: u64,
        child: u8,
    ) -> Result<(), crate::ReaderError> {
        let state = self.0.lock().unwrap();
        state
            .reader_control
            .as_ref()
            .ok_or(crate::ReaderError::Invalid("no current readers"))?
            .cancel_child(session, generation, child)
    }
    pub(crate) fn close(&self) {
        let mut state = self.0.lock().unwrap();
        state.closed = true;
        if let Some(control) = &state.reader_control {
            control.cancel_owned_tree();
        }
        if let Some(control) = &state.coding_control {
            control.stop();
        }
    }
}
pub(crate) fn logical_settled(state: &State) -> bool {
    if state.error.is_some() {
        return false;
    }
    if state.current.is_none() {
        return state.phase == ProjectPhase::Idle;
    }
    match state.phase {
        ProjectPhase::Paused | ProjectPhase::Verified => true,
        ProjectPhase::Failed => state.records.last().is_some_and(|record| {
            matches!(record.as_ref(),
            ProjectRecord::SourceReport(report) | ProjectRecord::VerificationReport(report)
                if matches!(report.completion.as_str(), "Failed" | "Cancelled"))
        }),
        _ => false,
    }
}

pub(crate) fn admit(state: &Arc<Mutex<State>>) -> Result<ProjectInvocation, CodingError> {
    let mut state = state.lock().unwrap();
    if state.closed
        || state.cancelled
        || !matches!(state.phase, ProjectPhase::Idle | ProjectPhase::Verified)
    {
        return Err(CodingError::Stale);
    }
    let journey = state.current.map_or(1, |id| id.journey + 1);
    if journey > 2 {
        return Err(CodingError::Invalid("two project journeys exhausted"));
    }
    let id = ProjectInvocation {
        instance: state.instance,
        journey,
    };
    state.current = Some(id);
    state.phase = ProjectPhase::Preparing;
    Ok(id)
}
pub(crate) fn check(
    state: &State,
    id: ProjectInvocation,
    phase: ProjectPhase,
) -> Result<(), CodingError> {
    if state.current != Some(id) || state.phase != phase {
        return Err(CodingError::Stale);
    }
    Ok(())
}
pub(crate) fn launch_readers(
    state: &Arc<Mutex<State>>,
    id: ProjectInvocation,
    runtime: Arc<Runtime>,
    storage: std::fs::File,
    inputs: ReaderInputs,
    supervision: ReaderSupervision,
) -> Result<(ReaderParent, crate::ReaderCleanup), CodingError> {
    let mut state = state.lock().unwrap();
    check(&state, id, ProjectPhase::Preparing)?;
    if state.cancelled || state.closed {
        return Err(CodingError::Cancelled);
    }
    let (parent, cleanup) = ReaderParent::start_retained(runtime, storage, inputs, supervision)
        .map_err(|_| CodingError::Invalid("retained reader launch failed"))?;
    state.readers[id.journey as usize - 1] = Some(parent.observer());
    state.reader_control = Some(parent.control());
    state.phase = ProjectPhase::Readers;
    Ok((parent, cleanup))
}
pub(crate) fn install_coding(
    state: &Arc<Mutex<State>>,
    id: ProjectInvocation,
    observer: CodingObserver,
    control: CodingControl,
) -> Result<(), CodingError> {
    let mut state = state.lock().unwrap();
    check(&state, id, ProjectPhase::Readers)?;
    if state.cancelled || state.closed {
        control.stop();
        return Err(CodingError::Cancelled);
    }
    state.reader_control = None;
    state.coding[id.journey as usize - 1] = Some(observer);
    state.coding_control = Some(control);
    state.phase = ProjectPhase::Coding;
    Ok(())
}
pub(crate) fn push(state: &Arc<Mutex<State>>, record: Arc<ProjectRecord>) {
    let mut state = state.lock().unwrap();
    assert!(state.records.len() < 32);
    state.records.push(record);
}
pub(crate) fn fail(state: &Arc<Mutex<State>>, message: &'static str) {
    let mut state = state.lock().unwrap();
    state.error.get_or_insert_with(|| Arc::from(message));
    state.phase = ProjectPhase::Failed;
}
pub(crate) fn completion(completion: CommandCompletion) -> &'static str {
    match completion {
        CommandCompletion::Completed => "Completed",
        CommandCompletion::Failed => "Failed",
        CommandCompletion::Cancelled => "Cancelled",
        CommandCompletion::Incomplete => "Incomplete",
    }
}
fn validate_summary(summary: &str) -> Result<(), CodingError> {
    if summary.len() > 512 || summary.chars().any(char::is_control) {
        return Err(CodingError::Invalid(
            "project report summary ceiling/controls",
        ));
    }
    Ok(())
}
pub(crate) fn prepare_source(
    state: &Arc<Mutex<State>>,
    receipt: &mut ProjectSourceReceipt,
    disposition: CommandCompletion,
    summary: &str,
) -> Result<ProjectReport, CodingError> {
    if !Arc::ptr_eq(state, &receipt.state) {
        return Err(CodingError::Stale);
    }
    validate_summary(summary)?;
    let mut state = state.lock().unwrap();
    check(&state, receipt.id, ProjectPhase::AwaitingSourceReport)?;
    let execution = receipt.execution.as_ref().ok_or(CodingError::Stale)?;
    if state.closed
        || ((state.cancelled)
            && matches!(
                disposition,
                CommandCompletion::Completed | CommandCompletion::Failed
            ))
    {
        return Err(CodingError::Cancelled);
    }
    if disposition == CommandCompletion::Cancelled && !state.cancelled {
        return Err(CodingError::Invalid(
            "Cancelled source report requires accepted Stop",
        ));
    }
    if matches!(
        disposition,
        CommandCompletion::Completed | CommandCompletion::Failed
    ) && completion(disposition) != execution.completion
    {
        return Err(CodingError::Invalid(
            "source report disagrees with actual execution",
        ));
    }
    if disposition == CommandCompletion::Completed && execution.summary.as_deref() != Some(summary)
    {
        return Err(CodingError::Invalid(
            "source report differs from actual final summary",
        ));
    }
    receipt.execution.take();
    state.phase = ProjectPhase::SourceReporting;
    Ok(ProjectReport {
        journey: receipt.id.journey,
        completion: completion(disposition).into(),
        summary: summary.into(),
        paused: disposition == CommandCompletion::Completed,
    })
}
pub(crate) fn commit_source(
    state: &Arc<Mutex<State>>,
    id: ProjectInvocation,
    report: &ProjectReport,
) -> Result<Option<ProjectContinuation>, CodingError> {
    let mut current = state.lock().unwrap();
    check(&current, id, ProjectPhase::SourceReporting)?;
    current.coding_control = None;
    if report.paused {
        current.phase = ProjectPhase::Paused;
        if !current.cancelled && !current.closed {
            return Ok(Some(ProjectContinuation {
                state: state.clone(),
                id,
            }));
        }
    } else {
        current.phase = ProjectPhase::Failed;
    }
    Ok(None)
}
pub(crate) fn prepare_resume(
    state: &Arc<Mutex<State>>,
    continuation: ProjectContinuation,
) -> Result<ProjectInvocation, CodingError> {
    if !Arc::ptr_eq(state, &continuation.state) {
        return Err(CodingError::Stale);
    }
    let mut current = state.lock().unwrap();
    check(&current, continuation.id, ProjectPhase::Paused)?;
    if current.closed || current.cancelled {
        return Err(CodingError::Cancelled);
    }
    current.phase = ProjectPhase::VerificationAdmitting;
    Ok(continuation.id)
}
pub(crate) fn prepare_verify(
    state: &Arc<Mutex<State>>,
    ready: ProjectReady,
) -> Result<ProjectInvocation, CodingError> {
    if !Arc::ptr_eq(state, &ready.state) {
        return Err(CodingError::Stale);
    }
    let mut current = state.lock().unwrap();
    check(&current, ready.id, ProjectPhase::VerificationReady)?;
    if current.closed || current.cancelled {
        return Err(CodingError::Cancelled);
    }
    current.phase = ProjectPhase::Verifying;
    Ok(ready.id)
}
pub(crate) fn prepare_verification_report(
    state: &Arc<Mutex<State>>,
    receipt: &mut ProjectVerificationReceipt,
    disposition: CommandCompletion,
    summary: &str,
) -> Result<ProjectReport, CodingError> {
    if !Arc::ptr_eq(state, &receipt.state) {
        return Err(CodingError::Stale);
    }
    validate_summary(summary)?;
    let mut current = state.lock().unwrap();
    check(
        &current,
        receipt.id,
        ProjectPhase::AwaitingVerificationReport,
    )?;
    let execution = receipt.execution.as_ref().ok_or(CodingError::Stale)?;
    if current.closed
        || (current.cancelled
            && matches!(
                disposition,
                CommandCompletion::Completed | CommandCompletion::Failed
            ))
    {
        return Err(CodingError::Cancelled);
    }
    if disposition == CommandCompletion::Cancelled && !current.cancelled {
        return Err(CodingError::Invalid(
            "Cancelled verification report requires accepted Stop",
        ));
    }
    if matches!(
        disposition,
        CommandCompletion::Completed | CommandCompletion::Failed
    ) && execution.matched != (disposition == CommandCompletion::Completed)
    {
        return Err(CodingError::Invalid(
            "verification report disagrees with actual read",
        ));
    }
    receipt.execution.take();
    current.phase = ProjectPhase::VerificationReporting;
    Ok(ProjectReport {
        journey: receipt.id.journey,
        completion: completion(disposition).into(),
        summary: summary.into(),
        paused: false,
    })
}
pub(crate) fn commit_verification(
    state: &Arc<Mutex<State>>,
    id: ProjectInvocation,
    report: &ProjectReport,
) -> Result<(), CodingError> {
    let mut current = state.lock().unwrap();
    check(&current, id, ProjectPhase::VerificationReporting)?;
    current.phase = if report.completion == "Completed" {
        ProjectPhase::Verified
    } else {
        ProjectPhase::Failed
    };
    Ok(())
}
