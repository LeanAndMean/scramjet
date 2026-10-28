use super::*;
use crate::readers::{ReaderLogical, ReaderSupervision};
use crate::{
    ReaderControl, ReaderInputs, ReaderObservation, ReaderObserver, ReaderParent, ReaderState,
    Runtime, Snapshot,
};
use tokio::sync::oneshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandReaderFact {
    pub session: Arc<str>,
    pub contents: [Arc<str>; 3],
    pub launched: bool,
    pub receipt_accepted: bool,
    pub execution: Option<Arc<CommandReaderExecution>>,
    pub cleanup: Option<Result<(), &'static str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandReaderExecution {
    pub disposition: CommandCompletion,
    pub observation: Arc<ReaderObservation>,
    pub usage_tokens: Option<u64>,
    pub error: Option<&'static str>,
}

pub struct CommandReaderReceipt {
    state: Arc<Mutex<State>>,
    id: CommandInvocation,
    execution: Option<Arc<CommandReaderExecution>>,
}

pub struct CommandReaderAdapter {
    state: Arc<Mutex<State>>,
    id: CommandInvocation,
    parent: ReaderParent,
    logical: Option<oneshot::Receiver<ReaderLogical>>,
    delivered: bool,
}

#[derive(Clone)]
pub struct CommandReaderControl {
    state: Arc<Mutex<State>>,
    control: ReaderControl,
}
impl CommandReaderControl {
    pub fn cancel_child(
        &self,
        session: &str,
        generation: u64,
        child: u8,
    ) -> Result<(), crate::ReaderError> {
        self.control.cancel_child(session, generation, child)
    }
    pub fn cancel_tree(&self, session: &str, generation: u64) -> Result<(), crate::ReaderError> {
        let mut state = self.state.lock().unwrap();
        self.control.cancel_tree(session, generation)?;
        if !state.snapshot.settled {
            state.snapshot.cancellation_requested = true;
        }
        Ok(())
    }
}
pub struct CommandReaderSettlement {
    release: Option<oneshot::Sender<()>>,
    receipt: Option<oneshot::Receiver<Result<Snapshot, &'static str>>>,
}

impl CommandSession {
    pub fn begin_readers(
        &mut self,
        name: &str,
        contents: [&str; 3],
    ) -> Result<CommandInvocation, CommandError> {
        if self.definition(name)?.outcome != Outcome::AllReaders {
            return Err(CommandError("not a reader command"));
        }
        let inputs = ReaderInputs::new(&format!("command-{}-1", self.identity), 1, contents)
            .map_err(|_| CommandError("invalid complete reader inputs"))?;
        let mut state = self.state.lock().unwrap();
        if state.closed
            || state.snapshot.cancellation_requested
            || state.snapshot.invocations[0].is_some()
        {
            return Err(CommandError("initial invocation unavailable"));
        }
        let id = self.admit(&mut state, name, 0);
        state.snapshot.retained_bytes += contents.iter().map(|s| s.len()).sum::<usize>();
        state.snapshot.invocations[0].as_mut().unwrap().readers = Some(CommandReaderFact {
            session: Arc::from(inputs.session),
            contents: inputs.contents,
            launched: false,
            receipt_accepted: false,
            execution: None,
            cleanup: None,
        });
        Ok(id)
    }

    pub fn start_readers(
        &mut self,
        id: CommandInvocation,
        runtime: Runtime,
        root: &Path,
    ) -> Result<(CommandReaderAdapter, CommandReaderSettlement), CommandError> {
        let inputs = {
            let mut state = self.state.lock().unwrap();
            let index = self.index(&state, id)?;
            if state.closed || state.snapshot.cancellation_requested {
                return Err(CommandError("reader launch unavailable"));
            }
            let fact = state.snapshot.invocations[index].as_mut().unwrap();
            let readers = fact
                .readers
                .as_mut()
                .ok_or(CommandError("not a reader invocation"))?;
            if fact.phase != CommandPhase::Ready || readers.launched {
                return Err(CommandError("reader launch already consumed"));
            }
            readers.launched = true;
            fact.phase = CommandPhase::Running;
            let inputs = ReaderInputs::shared(&readers.session, readers.contents.clone());
            state.reader_outstanding = true;
            inputs
        };
        let (logical, receive) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let (parent, cleanup) = ReaderParent::start_supervised(
            runtime,
            root,
            inputs,
            ReaderSupervision {
                logical,
                release: released,
            },
        )
        .map_err(|_| CommandError("reader supervisor unavailable"))?;
        {
            let mut state = self.state.lock().unwrap();
            let control = parent.control();
            if state.closed || state.snapshot.cancellation_requested {
                control.cancel_owned_tree();
            }
            state.reader_cancel = Some(control);
        }
        let shared = self.state.clone();
        let (receipt, settled) = oneshot::channel();
        tokio::spawn(async move {
            let outcome = cleanup
                .wait()
                .await
                .map_err(|_| "reader cleanup acknowledgement failed");
            {
                let mut state = shared.lock().unwrap();
                state.snapshot.invocations[id.ordinal as usize - 1]
                    .as_mut()
                    .unwrap()
                    .readers
                    .as_mut()
                    .unwrap()
                    .cleanup = Some(outcome.map(|_| ()));
                state.reader_outstanding = outcome.is_err();
                state.reader_cancel = None;
                if state.closed {
                    state.snapshot.settled = !state.reader_outstanding && !state.coding_outstanding;
                }
            }
            let _ = receipt.send(outcome);
        });
        Ok((
            CommandReaderAdapter {
                state: self.state.clone(),
                id,
                parent,
                logical: Some(receive),
                delivered: false,
            },
            CommandReaderSettlement {
                release: Some(release),
                receipt: Some(settled),
            },
        ))
    }

    pub fn accept_readers(
        &mut self,
        receipt: &mut CommandReaderReceipt,
    ) -> Result<(), CommandError> {
        if !Arc::ptr_eq(&self.state, &receipt.state) {
            return Err(CommandError("foreign reader receipt"));
        }
        let mut state = self.state.lock().unwrap();
        let index = self.index(&state, receipt.id)?;
        if state.closed {
            return Err(CommandError("session finalized"));
        }
        let fact = state.snapshot.invocations[index].as_mut().unwrap();
        let readers = fact
            .readers
            .as_mut()
            .ok_or(CommandError("not a reader invocation"))?;
        if fact.phase != CommandPhase::Running
            || !readers.launched
            || readers.receipt_accepted
            || receipt.execution.is_none()
        {
            return Err(CommandError("stale reader receipt"));
        }
        readers.execution = receipt.execution.take();
        readers.receipt_accepted = true;
        fact.phase = CommandPhase::AwaitingCompletion;
        Ok(())
    }
}

impl CommandReaderAdapter {
    pub fn observer(&self) -> ReaderObserver {
        self.parent.observer()
    }
    pub fn control(&self) -> CommandReaderControl {
        CommandReaderControl {
            state: self.state.clone(),
            control: self.parent.control(),
        }
    }
    pub async fn logical(&mut self) -> Result<CommandReaderReceipt, CommandError> {
        let receive = self
            .logical
            .as_mut()
            .ok_or(CommandError("reader logical wait consumed"))?;
        let logical = receive.await;
        self.logical = None;
        let logical = logical.map_err(|_| CommandError("reader logical owner lost"))?;
        let mut disposition = CommandCompletion::Incomplete;
        let mut usage_tokens = None;
        let mut error = None;
        match logical.outcome {
            Ok((tree, usage))
                if tree.parent_finished
                    && logical.observation.tree_ended
                    && tree.session == logical.observation.session
                    && tree.generation == 1
                    && tree.children.iter().enumerate().all(|(i, fact)| {
                        fact.child as usize == i
                            && logical.observation.children[i].fact.as_ref() == Some(fact)
                            && logical.observation.children[i].request_id == fact.request_id
                    }) =>
            {
                usage_tokens = usage;
                disposition = if tree.children.iter().enumerate().all(|(i, f)| {
                    matches!(f.state, ReaderState::Completed { .. })
                        && f.request_id.is_some()
                        && logical.observation.children[i].acceptance_ordinal.is_some()
                }) {
                    CommandCompletion::Completed
                } else {
                    CommandCompletion::Failed
                };
            }
            Err(_) => error = Some("reader logical acknowledgement failed"),
            _ => error = Some("reader logical outcome mismatch"),
        }
        self.delivered = true;
        Ok(CommandReaderReceipt {
            state: self.state.clone(),
            id: self.id,
            execution: Some(Arc::new(CommandReaderExecution {
                disposition,
                observation: Arc::new(logical.observation),
                usage_tokens,
                error,
            })),
        })
    }
}
impl CommandReaderSettlement {
    pub fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
    pub async fn wait(mut self) -> Result<Snapshot, CommandError> {
        self.release();
        self.receipt
            .take()
            .unwrap()
            .await
            .map_err(|_| CommandError("reader settlement owner lost"))?
            .map_err(CommandError)
    }
}
impl Drop for CommandReaderSettlement {
    fn drop(&mut self) {
        self.release();
    }
}
fn abandon(
    state: &Arc<Mutex<State>>,
    id: CommandInvocation,
    execution: Option<Arc<CommandReaderExecution>>,
) {
    let mut state = state.lock().unwrap();
    let fact = state.snapshot.invocations[id.ordinal as usize - 1]
        .as_mut()
        .unwrap();
    if let Some(execution) = execution {
        fact.readers.as_mut().unwrap().execution = Some(execution);
    }
    if matches!(
        fact.phase,
        CommandPhase::Ready | CommandPhase::Running | CommandPhase::AwaitingCompletion
    ) {
        fact.phase = CommandPhase::Incomplete;
    }
}
impl Drop for CommandReaderAdapter {
    fn drop(&mut self) {
        if !self.delivered {
            abandon(&self.state, self.id, None);
        }
    }
}
impl Drop for CommandReaderReceipt {
    fn drop(&mut self) {
        if let Some(execution) = self.execution.take() {
            abandon(&self.state, self.id, Some(execution));
        }
    }
}
