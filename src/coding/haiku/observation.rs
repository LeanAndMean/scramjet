use super::{EditCancellation, HaikuHistory, HaikuProcessResult, HaikuProposal};
use crate::CodingError;
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaikuPhase {
    Ready,
    Model,
    Approval,
    Effect,
    Finished,
    Stopped,
}
#[derive(Clone, Debug)]
pub struct HaikuSnapshot {
    pub phase: HaikuPhase,
    pub proposal: Option<HaikuProposal>,
    pub history: Arc<HaikuHistory>,
    pub observed_attempt: Option<Arc<super::HaikuAttempt>>,
    pub attempt_acknowledged: bool,
    pub observed_process: Option<Arc<HaikuProcessResult>>,
    pub process_acknowledged: bool,
    pub failure: Option<String>,
    pub request_id: Option<crate::RequestId>,
    pub run_ended: bool,
    pub worker_terminated: bool,
    pub worker_joined: bool,
}
struct State {
    snapshot: Arc<HaikuSnapshot>,
    decision: Option<(HaikuProposal, bool)>,
}
pub(super) struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    cancellation: EditCancellation,
}
#[derive(Clone)]
pub struct HaikuObserver(Arc<Shared>);
#[derive(Clone)]
pub struct HaikuControl(Arc<Shared>);
impl Shared {
    pub(super) fn new(history: Arc<HaikuHistory>, cancellation: EditCancellation) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                snapshot: Arc::new(HaikuSnapshot {
                    phase: HaikuPhase::Ready,
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
                }),
                decision: None,
            }),
            changed: Condvar::new(),
            cancellation,
        })
    }
    pub(super) fn handles(self: &Arc<Self>) -> (HaikuObserver, HaikuControl) {
        (HaikuObserver(self.clone()), HaikuControl(self.clone()))
    }
    pub(super) fn update(&self, f: impl FnOnce(&mut HaikuSnapshot)) {
        let mut state = self.state.lock().unwrap();
        f(Arc::make_mut(&mut state.snapshot));
        self.changed.notify_all();
    }
    pub(super) fn history(&self, history: Arc<HaikuHistory>) {
        self.update(|snapshot| {
            snapshot.phase = match history.entries.last().map(|entry| entry.record.as_ref()) {
                Some(
                    super::HaikuRecord::Authorized { .. }
                    | super::HaikuRecord::Dispatched { .. }
                    | super::HaikuRecord::Started { .. },
                ) => HaikuPhase::Effect,
                Some(super::HaikuRecord::Finished) => HaikuPhase::Finished,
                Some(super::HaikuRecord::Stopped { .. }) => HaikuPhase::Stopped,
                _ => snapshot.phase,
            };
            snapshot.history = history;
        });
    }
    pub(super) fn approve(
        &self,
        proposal: HaikuProposal,
        deadline: std::time::Instant,
    ) -> Result<(HaikuProposal, bool), CodingError> {
        let mut state = self.state.lock().unwrap();
        let snapshot = Arc::make_mut(&mut state.snapshot);
        snapshot.phase = HaikuPhase::Approval;
        snapshot.proposal = Some(proposal.clone());
        self.changed.notify_all();
        loop {
            if self.cancellation.is_cancelled() || std::time::Instant::now() >= deadline {
                Arc::make_mut(&mut state.snapshot).proposal = None;
                return Ok((proposal, false));
            }
            if let Some(decision) = state.decision.take() {
                return Ok(decision);
            }
            let remaining = deadline
                .saturating_duration_since(std::time::Instant::now())
                .min(std::time::Duration::from_millis(10));
            state = self.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }
    pub(super) fn fail(&self, error: &CodingError) {
        self.update(|snapshot| {
            snapshot.failure = Some(error.to_string());
            snapshot.phase = HaikuPhase::Stopped;
            snapshot.proposal = None;
        });
    }
}
impl HaikuObserver {
    pub fn snapshot(&self) -> Arc<HaikuSnapshot> {
        self.0.state.lock().unwrap().snapshot.clone()
    }
}
impl HaikuControl {
    pub fn cancellation(&self) -> EditCancellation {
        self.0.cancellation.clone()
    }
    pub fn cancel(&self) {
        self.0.cancellation.cancel();
        self.0.update(|snapshot| {
            snapshot.proposal = None;
        });
    }
    pub fn decide(&self, proposal: &HaikuProposal, approve: bool) -> Result<(), CodingError> {
        let mut state = self.0.state.lock().unwrap();
        if self.0.cancellation.is_cancelled()
            || state.decision.is_some()
            || state.snapshot.proposal.as_ref().is_none_or(|current| {
                !Arc::ptr_eq(&current.owner, &proposal.owner) || current.intent != proposal.intent
            })
        {
            return Err(CodingError::Stale);
        }
        state.decision = Some((proposal.clone(), approve));
        Arc::make_mut(&mut state.snapshot).proposal = None;
        self.0.changed.notify_all();
        Ok(())
    }
}
