use super::{CodingError, EditCancellation, EditIntent, OperationState};
use crate::{Outcome, RequestId};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodingProposal {
    pub owner: u64,
    pub invocation: u64,
    pub request_id: RequestId,
    pub turn: u64,
    pub intent: EditIntent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingDecision {
    Approve,
    Deny,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodingAccounting {
    pub request_id: RequestId,
    pub unknown_usage: bool,
    pub unresolved_exposure: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodingAttempt {
    pub recorded: bool,
    pub request_id: Option<RequestId>,
    pub outcome: Option<Outcome>,
    pub accounting: Option<CodingAccounting>,
    pub material: Option<&'static str>,
    pub output: Option<Arc<str>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodingObservation {
    pub owner: u64,
    pub invocation: u64,
    pub attempts: [CodingAttempt; 8],
    pub proposal: Option<Arc<CodingProposal>>,
    pub decision: Option<CodingDecision>,
    pub authorized: bool,
    pub effect: Option<OperationState>,
    pub dispatched: bool,
    pub verified: bool,
    pub verified_identity: Option<super::RootIdentity>,
    pub final_summary: Option<Arc<str>>,
    pub error: Option<Arc<str>>,
    pub run_ended: bool,
    pub worker_terminated: bool,
    pub worker_joined: bool,
}
struct State {
    observation: Arc<CodingObservation>,
    pending: bool,
}
pub(super) struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    cancellation: EditCancellation,
}
#[derive(Clone)]
pub struct CodingObserver(pub(super) Arc<Shared>);
#[derive(Clone)]
pub struct CodingControl(pub(super) Arc<Shared>, pub(super) u64);
impl Shared {
    pub(super) fn new(owner: u64, cancellation: EditCancellation) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                observation: Arc::new(CodingObservation {
                    owner,
                    invocation: 1,
                    attempts: std::array::from_fn(|_| CodingAttempt::default()),
                    proposal: None,
                    decision: None,
                    authorized: false,
                    effect: None,
                    dispatched: false,
                    verified: false,
                    verified_identity: None,
                    final_summary: None,
                    error: None,
                    run_ended: false,
                    worker_terminated: false,
                    worker_joined: false,
                }),
                pending: false,
            }),
            changed: Condvar::new(),
            cancellation,
        })
    }
    pub(super) fn update(&self, update: impl FnOnce(&mut CodingObservation)) {
        let mut state = self.state.lock().unwrap();
        update(Arc::make_mut(&mut state.observation));
    }
    fn eligible(&self, state: &State) -> Result<(), CodingError> {
        if self.cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        if state.pending
            || state.observation.invocation != 1
            || !state.observation.run_ended
            || state.observation.error.is_some()
            || !state.observation.verified
            || state.observation.effect != Some(OperationState::Completed)
            || state.observation.decision != Some(CodingDecision::Approve)
        {
            return Err(CodingError::Stale);
        }
        Ok(())
    }
    pub(super) fn checkpoint_eligible(&self) -> Result<(), CodingError> {
        self.eligible(&self.state.lock().unwrap())
    }
    pub(super) fn advance(&self) -> Result<(), CodingError> {
        let mut state = self.state.lock().unwrap();
        self.eligible(&state)?;
        let o = Arc::make_mut(&mut state.observation);
        o.invocation = 2;
        o.proposal = None;
        o.decision = None;
        o.authorized = false;
        o.effect = None;
        o.dispatched = false;
        o.verified = false;
        o.verified_identity = None;
        o.final_summary = None;
        o.run_ended = false;
        Ok(())
    }
    pub(super) fn failure(&self, message: &str) {
        let mut end = message.len().min(4096);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message: Arc<str> = Arc::from(&message[..end]);
        self.update(|o| {
            if o.error.is_none() {
                o.error = Some(message);
            }
        });
    }
    pub(super) fn decide(&self, proposal: CodingProposal) -> Result<CodingDecision, CodingError> {
        let proposal = Arc::new(proposal);
        let mut state = self.state.lock().unwrap();
        Arc::make_mut(&mut state.observation).proposal = Some(proposal);
        state.pending = true;
        loop {
            if self.cancellation.is_cancelled() {
                state.pending = false;
                return Err(CodingError::Cancelled);
            }
            if let Some(decision) = state.observation.decision {
                return Ok(decision);
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
}
impl CodingObserver {
    pub fn snapshot(&self) -> Arc<CodingObservation> {
        self.0.state.lock().unwrap().observation.clone()
    }
}
impl CodingControl {
    pub fn stop(&self) {
        let state = self.0.state.lock().unwrap();
        if state.observation.invocation == self.1 {
            self.0.cancellation.cancel();
            self.0.changed.notify_all();
        }
    }
    pub fn decide(
        &self,
        displayed: &CodingProposal,
        decision: CodingDecision,
    ) -> Result<(), CodingError> {
        let mut state = self.0.state.lock().unwrap();
        if self.0.cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        if state.observation.invocation != self.1
            || displayed.invocation != self.1
            || !state.pending
            || state.observation.proposal.as_deref() != Some(displayed)
        {
            return Err(CodingError::Stale);
        }
        state.pending = false;
        Arc::make_mut(&mut state.observation).decision = Some(decision);
        self.0.changed.notify_all();
        Ok(())
    }
}
