use super::*;
use crate::{
    CodingAgent, CodingControl, CodingError, CodingObservation, CodingObserver, CodingRun,
    CodingSession, EditCancellation, OperationState, Runtime,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandCodingFact {
    pub session: Arc<str>,
    pub target: Arc<str>,
    pub input: Arc<str>,
    pub owner: Option<u64>,
    pub receipt_accepted: bool,
    pub execution: Option<Arc<CommandCodingExecution>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandCodingExecution {
    pub disposition: CommandCompletion,
    pub reason: Option<Arc<str>>,
    pub observation: Arc<CodingObservation>,
}

pub struct CommandCodingReceipt {
    state: Arc<Mutex<State>>,
    id: CommandInvocation,
    owner: u64,
    execution: Option<Arc<CommandCodingExecution>>,
}

pub struct CommandCodingAdapter {
    state: Arc<Mutex<State>>,
    id: CommandInvocation,
    agent: Option<CodingAgent>,
    observer: CodingObserver,
    result: Option<Result<CodingRun, CodingError>>,
    ran: bool,
}

impl CommandSession {
    pub fn validate_coding_input(&self, name: &str, input: &str) -> Result<(), CommandError> {
        if self.definition(name)?.outcome != Outcome::ReplaceVerified {
            return Err(CommandError("not a coding command"));
        }
        if input.len() > COMMAND_INPUT_BYTES {
            return Err(CommandError("coding input byte ceiling"));
        }
        if input.trim().is_empty() {
            return Err(CommandError("coding input must be nonblank"));
        }
        CodingAgent::validate_prompt(&format!("command-{}-1", self.identity), input)
            .map_err(|_| CommandError("input exceeds complete coding boundary"))
    }

    pub fn begin_coding(
        &mut self,
        name: &str,
        input: &str,
    ) -> Result<CommandInvocation, CommandError> {
        self.validate_coding_input(name, input)?;
        let target = self.definition(name)?.target.as_ref().unwrap();
        let mut state = self.state.lock().unwrap();
        if state.snapshot.settled
            || state.snapshot.cancellation_requested
            || state.snapshot.invocations[0].is_some()
        {
            return Err(CommandError("initial invocation unavailable"));
        }
        let id = self.admit(&mut state, name, 0);
        state.snapshot.retained_bytes += input.len();
        state.snapshot.invocations[0].as_mut().unwrap().coding = Some(CommandCodingFact {
            session: Arc::from(format!("command-{}-1", self.identity)),
            target: Arc::from(target.as_str()),
            input: Arc::from(input),
            owner: None,
            receipt_accepted: false,
            execution: None,
        });
        Ok(id)
    }

    pub fn start_coding(
        &mut self,
        id: CommandInvocation,
        session: CodingSession,
    ) -> Result<CommandCodingAdapter, CommandError> {
        #[cfg(test)]
        self.hit(super::Checkpoint::BeforeCodingLaunch);
        let mut state = self.state.lock().unwrap();
        let index = self.index(&state, id)?;
        if state.snapshot.settled || state.snapshot.cancellation_requested {
            return Err(CommandError("coding launch unavailable"));
        }
        let fact = state.snapshot.invocations[index].as_mut().unwrap();
        if fact.phase != CommandPhase::Ready {
            return Err(CommandError("coding launch already consumed"));
        }
        let coding = fact
            .coding
            .as_mut()
            .ok_or(CommandError("not a coding invocation"))?;
        if session.session != coding.session.as_ref() {
            return Err(CommandError("foreign coding session"));
        }
        let cancellation = EditCancellation::default();
        let agent = CodingAgent::start_exact_replacement(
            session,
            cancellation.clone(),
            coding.input.clone(),
            coding.target.clone(),
        )
        .map_err(|_| CommandError("cannot bind fresh coding owner"))?;
        let observer = agent.observer();
        coding.owner = Some(observer.snapshot().owner);
        fact.phase = CommandPhase::Running;
        state.coding_cancel = Some(cancellation);
        state.coding_outstanding = true;
        drop(state);
        #[cfg(test)]
        self.hit(super::Checkpoint::AfterCodingLaunch);
        Ok(CommandCodingAdapter {
            state: self.state.clone(),
            id,
            agent: Some(agent),
            observer,
            result: None,
            ran: false,
        })
    }

    pub fn accept_coding(
        &mut self,
        receipt: &mut CommandCodingReceipt,
    ) -> Result<(), CommandError> {
        if !Arc::ptr_eq(&self.state, &receipt.state) {
            return Err(CommandError("foreign coding receipt"));
        }
        let mut state = self.state.lock().unwrap();
        let index = self.index(&state, receipt.id)?;
        if state.snapshot.settled {
            return Err(CommandError("session finalized"));
        }
        let fact = state.snapshot.invocations[index].as_mut().unwrap();
        let coding = fact
            .coding
            .as_mut()
            .ok_or(CommandError("not a coding invocation"))?;
        if fact.phase != CommandPhase::Running
            || coding.owner != Some(receipt.owner)
            || coding.execution.is_some()
        {
            return Err(CommandError("stale coding receipt"));
        }
        coding.execution = receipt.execution.take();
        coding.receipt_accepted = true;
        fact.phase = CommandPhase::AwaitingCompletion;
        state.coding_outstanding = false;
        state.coding_cancel = None;
        Ok(())
    }
}

impl CommandCodingAdapter {
    pub fn observer(&self) -> CodingObserver {
        self.observer.clone()
    }
    pub fn control(&self) -> CodingControl {
        self.agent.as_ref().unwrap().control()
    }
    pub fn result(&self) -> Option<&Result<CodingRun, CodingError>> {
        self.result.as_ref()
    }
    pub async fn run_unjoined(&mut self, runtime: &Runtime) -> Result<(), CommandError> {
        if self.ran {
            return Err(CommandError("coding run already consumed"));
        }
        self.ran = true;
        self.result = Some(self.agent.as_mut().unwrap().run_unjoined(runtime).await);
        Ok(())
    }
    fn settle(&mut self) -> Arc<CommandCodingExecution> {
        let joined = self.agent.take().unwrap().finish();
        let observation = self.observer.snapshot();
        let mut disposition = CommandCompletion::Incomplete;
        let mut reason: Option<Arc<str>> = None;
        if joined.is_ok() && observation.worker_joined && observation.worker_terminated {
            match &self.result {
                Some(Ok(run))
                    if run.turns == 4
                        && run.edit_operation == 1
                        && run.request_ids.len() == 4
                        && observation.run_ended
                        && observation.effect == Some(OperationState::Completed)
                        && observation.verified
                        && observation.final_summary.as_deref() == Some(run.summary.as_str())
                        && observation.error.is_none()
                        && observation.attempts[..4].iter().zip(&run.request_ids).all(
                            |(a, id)| {
                                a.recorded
                                    && a.request_id == Some(*id)
                                    && a.outcome == Some(crate::Outcome::Success)
                            },
                        ) =>
                {
                    disposition = CommandCompletion::Completed;
                }
                Some(Err(error)) => {
                    reason = Some(Arc::from(error.to_string()));
                    disposition = match error {
                        CodingError::Denied => CommandCompletion::Failed,
                        CodingError::Cancelled
                        | CodingError::Poisoned
                        | CodingError::Io(_)
                        | CodingError::History(_) => CommandCompletion::Incomplete,
                        _ if observation.effect == Some(OperationState::Uncertain) => {
                            CommandCompletion::Incomplete
                        }
                        _ => CommandCompletion::Failed,
                    };
                }
                _ => reason = Some(Arc::from("logical execution acknowledgement unavailable")),
            }
        } else {
            reason = Some(Arc::from("coding worker acknowledgement unavailable"));
        }
        if self.state.lock().unwrap().snapshot.cancellation_requested {
            disposition = CommandCompletion::Cancelled;
        }
        Arc::new(CommandCodingExecution {
            disposition,
            reason,
            observation,
        })
    }
    pub fn finish(mut self) -> CommandCodingReceipt {
        let execution = self.settle();
        CommandCodingReceipt {
            state: self.state.clone(),
            id: self.id,
            owner: execution.observation.owner,
            execution: Some(execution),
        }
    }
}

fn abandon(
    state: &Arc<Mutex<State>>,
    id: CommandInvocation,
    execution: Arc<CommandCodingExecution>,
) {
    let mut state = state.lock().unwrap();
    let fact = state.snapshot.invocations[id.ordinal as usize - 1]
        .as_mut()
        .unwrap();
    fact.coding.as_mut().unwrap().execution = Some(execution);
    fact.phase = CommandPhase::Incomplete;
    state.coding_cancel = None;
    state.coding_outstanding = false;
    if state.closed {
        state.snapshot.settled = true;
    }
}
impl Drop for CommandCodingAdapter {
    fn drop(&mut self) {
        if self.agent.is_some() {
            let execution = self.settle();
            abandon(&self.state, self.id, execution);
        }
    }
}
impl Drop for CommandCodingReceipt {
    fn drop(&mut self) {
        if let Some(execution) = self.execution.take() {
            abandon(&self.state, self.id, execution);
        }
    }
}
