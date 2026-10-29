use super::{
    HaikuControl, HaikuObserver, HaikuPhase, HaikuSession, HaikuSnapshot, HaikuStep,
    observation::Shared,
};
use crate::{CodingError, EditCancellation, ProviderRequest, ProviderResponse, RequestId, Runtime};
use std::{
    sync::{
        Arc,
        mpsc::{SyncSender, sync_channel},
    },
    thread::JoinHandle,
    time::Instant,
};
use tokio::sync::oneshot;

enum Work {
    Request(oneshot::Sender<Result<ProviderRequest, CodingError>>),
    Response {
        expected: RequestId,
        response: Box<ProviderResponse>,
        reply: oneshot::Sender<Result<bool, CodingError>>,
    },
    Stop {
        reason: String,
        reply: oneshot::Sender<Result<(), CodingError>>,
    },
}
#[derive(Clone, Debug)]
pub struct HaikuRun {
    pub turns: usize,
    pub request_ids: Vec<RequestId>,
    pub snapshot: Arc<HaikuSnapshot>,
}
pub struct HaikuAgent {
    sender: Option<SyncSender<Work>>,
    worker: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    pub(crate) cancellation: EditCancellation,
    pub(crate) deadline: Instant,
    ran: bool,
}
impl HaikuAgent {
    pub fn start(mut session: HaikuSession) -> Result<Self, CodingError> {
        let cancellation = EditCancellation::default();
        let deadline = session.deadline();
        let shared = Shared::new(session.history(), cancellation.clone());
        session.observe(shared.clone());
        let worker_shared = shared.clone();
        let worker_cancel = cancellation.clone();
        let (sender, receiver) = sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("haiku-effects".into())
            .spawn(move || {
                while let Ok(work) = receiver.recv() {
                    match work {
                        Work::Request(reply) => {
                            let _ = reply.send(session.prepare_request());
                        }
                        Work::Response {
                            expected,
                            response,
                            reply,
                        } => {
                            let result = (|| match session.select_response(expected, *response)? {
                                HaikuStep::Approval(proposal) => {
                                    let (proposal, approve) =
                                        worker_shared.approve(proposal, deadline)?;
                                    session.decide(proposal, approve, &worker_cancel)?;
                                    Ok(false)
                                }
                                HaikuStep::Read { .. } => Ok(false),
                                HaikuStep::Finished => Ok(true),
                            })();
                            if let Err(error) = &result {
                                worker_shared.fail(error);
                            }
                            let _ = reply.send(result);
                        }
                        Work::Stop { reason, reply } => {
                            let result = session.stop_reason(reason);
                            let _ = reply.send(result);
                        }
                    }
                }
                worker_shared.update(|snapshot| {
                    snapshot.worker_terminated = true;
                    snapshot.proposal = None;
                });
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            shared,
            cancellation,
            deadline,
            ran: false,
        })
    }
    pub(crate) fn admitted(&self, id: RequestId) {
        self.shared
            .update(|snapshot| snapshot.request_id = Some(id));
    }
    pub fn observer(&self) -> HaikuObserver {
        self.shared.handles().0
    }
    pub fn control(&self) -> HaikuControl {
        self.shared.handles().1
    }
    pub async fn run(mut self, runtime: &Runtime) -> Result<HaikuRun, CodingError> {
        let result = self.run_unjoined(runtime).await;
        let snapshot = tokio::task::spawn_blocking(move || self.finish())
            .await
            .map_err(|_| CodingError::Invalid("haiku join owner failed"))??;
        result.map(|mut run| {
            run.snapshot = snapshot;
            run
        })
    }
    pub async fn run_unjoined(&mut self, runtime: &Runtime) -> Result<HaikuRun, CodingError> {
        if self.ran {
            return Err(CodingError::Stale);
        }
        self.ran = true;
        let result = crate::coding::agent::run_haiku(self, runtime).await;
        if let Err(error) = &result {
            self.shared.fail(error);
        }
        self.shared.update(|snapshot| snapshot.run_ended = true);
        result
    }
    async fn receive<T>(
        &self,
        mut receive: oneshot::Receiver<Result<T, CodingError>>,
    ) -> Result<T, CodingError> {
        let result = tokio::select! {
            result = &mut receive => result.map_err(|_| CodingError::Invalid("haiku worker acknowledgement lost"))?,
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(self.deadline)) => {
                self.control().cancel();
                let _ = receive.await.map_err(|_| CodingError::Invalid("haiku timed-out worker acknowledgement lost"))?;
                Err(CodingError::Deadline)
            }
        };
        if Instant::now() >= self.deadline {
            self.control().cancel();
            return Err(CodingError::Deadline);
        }
        result
    }
    pub(crate) async fn request(&mut self) -> Result<ProviderRequest, CodingError> {
        self.shared.update(|snapshot| {
            snapshot.phase = HaikuPhase::Model;
            snapshot.request_id = None;
            snapshot.observed_attempt = None;
            snapshot.attempt_acknowledged = false;
        });
        let (reply, receive) = oneshot::channel();
        self.sender
            .as_ref()
            .ok_or(CodingError::Stale)?
            .try_send(Work::Request(reply))
            .map_err(|_| CodingError::Invalid("haiku serialized owner unavailable"))?;
        self.receive(receive).await
    }
    pub(crate) async fn response(
        &mut self,
        expected: RequestId,
        response: Box<ProviderResponse>,
    ) -> Result<bool, CodingError> {
        let (reply, receive) = oneshot::channel();
        self.sender
            .as_ref()
            .ok_or(CodingError::Stale)?
            .try_send(Work::Response {
                expected,
                response,
                reply,
            })
            .map_err(|_| CodingError::Invalid("haiku serialized owner unavailable"))?;
        self.receive(receive).await
    }
    pub(crate) async fn stop(&mut self, error: &CodingError) -> Result<(), CodingError> {
        self.control().cancel();
        let (reply, receive) = oneshot::channel();
        self.sender
            .as_ref()
            .ok_or(CodingError::Stale)?
            .try_send(Work::Stop {
                reason: error.to_string(),
                reply,
            })
            .map_err(|_| CodingError::Invalid("haiku stop owner unavailable"))?;
        receive
            .await
            .map_err(|_| CodingError::Invalid("haiku stop acknowledgement lost"))?
    }
    pub fn finish(mut self) -> Result<Arc<HaikuSnapshot>, CodingError> {
        self.join()?;
        Ok(self.observer().snapshot())
    }
    fn join(&mut self) -> Result<(), CodingError> {
        self.control().cancel();
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let joined = worker.join().is_ok();
            self.shared.update(|snapshot| {
                snapshot.worker_joined = joined;
            });
            if !joined {
                return Err(CodingError::Invalid("haiku effect owner panicked"));
            }
        }
        Ok(())
    }
}
impl Drop for HaikuAgent {
    fn drop(&mut self) {
        let _ = self.join();
    }
}
