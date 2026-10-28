use super::{Control, ReaderError, ReaderFact, ReaderState};
use crate::RequestId;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReaderPhase {
    #[default]
    Pending,
    Submitted,
    Streaming,
    Terminal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReaderAccounting {
    pub request_id: RequestId,
    pub unknown_usage: bool,
    pub unresolved_exposure: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReaderSlot {
    pub request_id: Option<RequestId>,
    pub phase: ReaderPhase,
    pub received: usize,
    pub accounting: Option<ReaderAccounting>,
    pub fact: Option<ReaderFact>,
    pub acceptance_ordinal: Option<u8>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReaderObservation {
    pub session: String,
    pub generation: u64,
    pub children: [ReaderSlot; 3],
    pub tree_ended: bool,
    pub storage_joined: bool,
    pub runtime_joined: bool,
    pub error: Option<&'static str>,
}
impl ReaderObservation {
    pub(super) fn new(session: &str, generation: u64) -> Self {
        Self {
            session: session.to_owned(),
            generation,
            children: std::array::from_fn(|_| ReaderSlot::default()),
            tree_ended: false,
            storage_joined: false,
            runtime_joined: false,
            error: None,
        }
    }
    pub(super) fn submitted(&mut self, child: u8, id: RequestId) -> Result<(), ReaderError> {
        if self.children.iter().any(|slot| slot.request_id == Some(id)) {
            return Err(ReaderError::Invalid("duplicate submission"));
        }
        let slot = self
            .children
            .get_mut(child as usize)
            .ok_or(ReaderError::Invalid("child identity"))?;
        if slot.request_id.is_some() || slot.fact.is_some() {
            return Err(ReaderError::Invalid("late submission"));
        }
        slot.request_id = Some(id);
        slot.phase = ReaderPhase::Submitted;
        Ok(())
    }
    fn live_slot(&mut self, child: u8, id: RequestId) -> Result<&mut ReaderSlot, ReaderError> {
        let slot = self
            .children
            .get_mut(child as usize)
            .ok_or(ReaderError::Invalid("child identity"))?;
        if slot.request_id != Some(id) || slot.fact.is_some() {
            return Err(ReaderError::Invalid("foreign or late update"));
        }
        Ok(slot)
    }
    pub(super) fn progress(
        &mut self,
        child: u8,
        id: RequestId,
        received: usize,
    ) -> Result<(), ReaderError> {
        let slot = self.live_slot(child, id)?;
        if slot.accounting.is_some() || received < slot.received || received > super::RESPONSE {
            return Err(ReaderError::Invalid("progress ceiling or ordering"));
        }
        slot.received = received;
        slot.phase = ReaderPhase::Streaming;
        Ok(())
    }
    pub(super) fn accounted(
        &mut self,
        child: u8,
        receipt: ReaderAccounting,
    ) -> Result<(), ReaderError> {
        let slot = self.live_slot(child, receipt.request_id)?;
        if slot.accounting.is_some() {
            return Err(ReaderError::Invalid("duplicate accounting"));
        }
        slot.accounting = Some(receipt);
        slot.phase = ReaderPhase::Terminal;
        Ok(())
    }
    pub(super) fn accepted(&mut self, fact: &ReaderFact) -> Result<(), ReaderError> {
        let ordinal = 1 + self
            .children
            .iter()
            .filter(|s| s.acceptance_ordinal.is_some())
            .count() as u8;
        let slot = self
            .children
            .get_mut(fact.child as usize)
            .ok_or(ReaderError::Invalid("child identity"))?;
        if slot.fact.is_some()
            || slot.request_id != fact.request_id
            || fact.state == ReaderState::NotDispatched
        {
            return Err(ReaderError::Invalid("duplicate or foreign acceptance"));
        }
        if let ReaderState::Completed { summary } = &fact.state {
            super::validate_text(summary, super::SUMMARY, false)?;
        }
        slot.fact = Some(fact.clone());
        slot.acceptance_ordinal = Some(ordinal);
        slot.phase = ReaderPhase::Terminal;
        Ok(())
    }
    pub(super) fn ended(&mut self, facts: &[ReaderFact; 3]) -> Result<(), ReaderError> {
        for (index, fact) in facts.iter().enumerate() {
            let slot = &self.children[index];
            if fact.child as usize != index
                || match &slot.fact {
                    Some(accepted) => accepted != fact,
                    None => {
                        fact.state != ReaderState::NotDispatched
                            || fact.request_id.is_some()
                            || slot.request_id.is_some()
                    }
                }
            {
                return Err(ReaderError::Invalid("foreign final fact"));
            }
        }
        for (slot, fact) in self.children.iter_mut().zip(facts) {
            if slot.fact.is_none() {
                slot.fact = Some(fact.clone());
                slot.phase = ReaderPhase::Terminal;
            }
        }
        self.tree_ended = true;
        Ok(())
    }
}
#[derive(Clone)]
pub struct ReaderObserver {
    receiver: watch::Receiver<ReaderObservation>,
}
impl ReaderObserver {
    pub fn snapshot(&self) -> ReaderObservation {
        self.receiver.borrow().clone()
    }
    pub async fn changed(&mut self) -> Result<(), ReaderError> {
        self.receiver
            .changed()
            .await
            .map_err(|_| ReaderError::LostOwner)
    }
    pub(super) fn new(receiver: watch::Receiver<ReaderObservation>) -> Self {
        Self { receiver }
    }
}
#[derive(Clone)]
pub struct ReaderControl {
    pub(super) control: Arc<Control>,
}
impl ReaderControl {
    pub(crate) fn cancel_owned_tree(&self) {
        super::cancel(&self.control, None);
    }
    pub fn cancel_child(
        &self,
        session: &str,
        generation: u64,
        child: u8,
    ) -> Result<(), ReaderError> {
        self.validate(session, generation)?;
        if child >= 3 {
            return Err(ReaderError::Invalid("child identity"));
        }
        super::cancel(&self.control, Some(child));
        Ok(())
    }
    pub fn cancel_tree(&self, session: &str, generation: u64) -> Result<(), ReaderError> {
        self.validate(session, generation)?;
        super::cancel(&self.control, None);
        Ok(())
    }
    fn validate(&self, session: &str, generation: u64) -> Result<(), ReaderError> {
        let observation = self.control.observation.borrow();
        if observation.session != session || observation.generation != generation {
            return Err(ReaderError::Invalid("foreign control invocation"));
        }
        Ok(())
    }
}
impl Control {
    pub(super) fn new(session: &str, generation: u64) -> Self {
        let (changed, _) = watch::channel(0);
        let (observation, _) = watch::channel(ReaderObservation::new(session, generation));
        Self {
            state: std::sync::Mutex::new(super::ControlState {
                cancelled: [false; 3],
                accepted: [false; 3],
            }),
            changed,
            observation,
        }
    }
    pub(super) fn publish(
        &self,
        update: impl FnOnce(&mut ReaderObservation) -> Result<(), ReaderError>,
    ) -> Result<(), ReaderError> {
        let mut result = Ok(());
        self.observation.send_modify(|state| {
            result = update(state);
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_state_rejects_foreign_duplicate_and_late_updates() {
        let control = Arc::new(Control::new("retained", 7));
        let observer = ReaderObserver::new(control.observation.subscribe());
        let handle = ReaderControl {
            control: control.clone(),
        };
        assert!(handle.cancel_child("foreign", 7, 0).is_err());
        assert!(handle.cancel_tree("retained", 8).is_err());
        assert!(handle.cancel_child("retained", 7, 3).is_err());
        for child in 0..3 {
            let id = RequestId(child as u64 + 1);
            control.publish(|state| state.submitted(child, id)).unwrap();
            for received in 1..=2048 {
                control
                    .publish(|state| state.progress(child, id, received))
                    .unwrap();
            }
            control
                .publish(|state| {
                    state.accounted(
                        child,
                        ReaderAccounting {
                            request_id: id,
                            unknown_usage: child == 1,
                            unresolved_exposure: child == 2,
                        },
                    )
                })
                .unwrap();
        }
        for child in [1, 0, 2] {
            let fact = ReaderFact {
                child,
                request_id: Some(RequestId(child as u64 + 1)),
                state: ReaderState::Completed {
                    summary: "x".repeat(512),
                },
            };
            control.publish(|state| state.accepted(&fact)).unwrap();
            assert!(control.publish(|state| state.accepted(&fact)).is_err());
            assert!(
                control
                    .publish(|state| state.progress(child, fact.request_id.unwrap(), 2048))
                    .is_err()
            );
        }
        let state = observer.snapshot();
        assert_eq!(
            state
                .children
                .each_ref()
                .map(|slot| slot.acceptance_ordinal),
            [Some(2), Some(1), Some(3)]
        );
        assert!(
            control
                .publish(|state| state.submitted(0, RequestId(99)))
                .is_err()
        );
        assert!(
            control
                .publish(|state| state.accounted(
                    0,
                    ReaderAccounting {
                        request_id: RequestId(99),
                        unknown_usage: false,
                        unresolved_exposure: false
                    }
                ))
                .is_err()
        );
        assert_eq!(observer.snapshot(), state);
        drop(observer);
        drop(handle);
        assert_eq!(control.state.lock().unwrap().cancelled, [false; 3]);
    }
    #[test]
    fn early_children_are_final_only_after_end_and_do_not_get_accept_ordinals() {
        let mut state = ReaderObservation::new("early", 1);
        let facts = std::array::from_fn(|child| ReaderFact {
            child: child as u8,
            request_id: None,
            state: ReaderState::NotDispatched,
        });
        assert!(state.accepted(&facts[0]).is_err());
        assert!(state.children.iter().all(|slot| slot.fact.is_none()));
        state.ended(&facts).unwrap();
        assert!(state.tree_ended);
        assert!(
            state
                .children
                .iter()
                .all(|slot| slot.fact.is_some() && slot.acceptance_ordinal.is_none())
        );
    }
}
