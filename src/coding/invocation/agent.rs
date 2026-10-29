use super::*;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::mpsc::{SyncSender, sync_channel},
    thread::JoinHandle,
};
use tokio::sync::oneshot;

pub struct CodingInvocationOptions {
    pub project: PathBuf,
    pub storage: PathBuf,
    pub run: String,
    pub files: [String; 2],
    pub prompt: String,
    pub previous: Option<Vec<u8>>,
    pub endpoint: SocketAddr,
    pub owned_fixture: bool,
}
pub struct CodingInvocationResult {
    pub record: Vec<u8>,
    pub success: bool,
    pub failure: Option<String>,
}
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
pub struct CodingInvocation {
    sender: Option<SyncSender<Work>>,
    worker: Option<JoinHandle<Session>>,
    pub(crate) cancellation: EditCancellation,
    pub(crate) deadline: Instant,
    ran: bool,
    runtime_owner: std::sync::Weak<crate::runtime::Shared>,
    endpoint: SocketAddr,
}
impl CodingInvocation {
    pub fn cancellation(&self) -> EditCancellation {
        self.cancellation.clone()
    }
    pub fn start(
        options: CodingInvocationOptions,
        mut approval: impl FnMut(
            &CodingInvocationIntent,
            &EditCancellation,
            Instant,
        ) -> Result<bool, CodingError>
        + Send
        + 'static,
    ) -> Result<(Self, crate::Runtime), CodingError> {
        if !options.owned_fixture
            || !options.endpoint.ip().is_loopback()
            || options.endpoint.port() == 18080
        {
            return Err(CodingError::Unsupported(
                "only an explicitly owned loopback fixture is eligible; live execution is stopped",
            ));
        }
        if options.prompt.len() > crate::PROVIDER_ENVELOPE_BYTES
            || options
                .previous
                .as_ref()
                .is_some_and(|b| b.len() > RECORD_BYTES)
        {
            return Err(CodingError::ResourceLimit(
                "invocation input resource limit",
            ));
        }
        if options.previous.is_none() {
            request(
                &[Material::User {
                    text: options.prompt.clone(),
                }],
                &options.files,
            )?;
        }
        let (mut store, runtime, provider) = store::Store::open(
            &options.project,
            &options.storage,
            &options.run,
            options.files,
            options.previous.as_deref(),
            options.endpoint,
        )?;
        store.begin(options.prompt)?;
        let deadline = store.deadline()?;
        let runtime_owner = Arc::downgrade(&runtime.ownership());
        let cancellation = EditCancellation::default();
        let cancel = cancellation.clone();
        let (sender, receiver) = sync_channel(1);
        let worker = std::thread::Builder::new()
            .name("coding-invocation-effects".into())
            .spawn(move || {
                let mut session = Session {
                    store,
                    provider,
                    pending: None,
                    owner: Arc::new(()),
                };
                while let Ok(work) = receiver.recv() {
                    match work {
                        Work::Request(reply) => {
                            let mut result = session.prepare();
                            if let Err(error) = &result
                                && let Err(stop) = session.store.stop(error.to_string())
                            {
                                result = Err(stop);
                            }
                            let failed = result.is_err();
                            let _ = reply.send(result);
                            if failed {
                                break;
                            }
                        }
                        Work::Response {
                            expected,
                            response,
                            reply,
                        } => {
                            let mut result: Result<bool, CodingError> =
                                (|| match session.select(expected, *response, &cancel)? {
                                    Selection::Approval(intent) => {
                                        let approve = approval(&intent, &cancel, deadline)?;
                                        session.decide(*intent, approve, &cancel)?;
                                        Ok(false)
                                    }
                                    Selection::Continue => Ok(false),
                                    Selection::Finished => Ok(true),
                                })();
                            if let Err(error) = &result
                                && let Err(stop) = session.store.stop(error.to_string())
                            {
                                result = Err(stop);
                            }
                            let finished = !matches!(result, Ok(false));
                            let _ = reply.send(result);
                            if finished {
                                break;
                            }
                        }
                        Work::Stop { reason, reply } => {
                            let _ = reply.send(session.store.stop(reason));
                            break;
                        }
                    }
                }
                session
            })?;
        Ok((
            Self {
                sender: Some(sender),
                worker: Some(worker),
                cancellation,
                deadline,
                ran: false,
                runtime_owner,
                endpoint: options.endpoint,
            },
            runtime,
        ))
    }
    async fn receive<T>(
        &self,
        mut receiver: oneshot::Receiver<Result<T, CodingError>>,
    ) -> Result<T, CodingError> {
        tokio::select! {
            result=&mut receiver=>result.map_err(|_|CodingError::History("effect worker acknowledgement lost"))?,
            _=tokio::time::sleep_until(tokio::time::Instant::from_std(self.deadline))=>{
                self.cancellation.cancel();
                let _=receiver.await.map_err(|_|CodingError::History("timed-out effect acknowledgement lost"))?;
                Err(CodingError::Deadline)
            }
        }
    }
    pub(crate) async fn request(&mut self) -> Result<ProviderRequest, CodingError> {
        let (reply, receive) = oneshot::channel();
        self.sender
            .as_ref()
            .ok_or(CodingError::Stale)?
            .try_send(Work::Request(reply))
            .map_err(|_| CodingError::History("effect worker unavailable"))?;
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
            .map_err(|_| CodingError::History("effect worker unavailable"))?;
        self.receive(receive).await
    }
    pub(crate) async fn stop(&mut self, error: &CodingError) -> Result<(), CodingError> {
        self.cancellation.cancel();
        if self.worker.as_ref().is_some_and(|w| w.is_finished()) {
            return Ok(());
        }
        let (reply, receive) = oneshot::channel();
        if self
            .sender
            .as_ref()
            .ok_or(CodingError::Stale)?
            .try_send(Work::Stop {
                reason: error.to_string(),
                reply,
            })
            .is_err()
        {
            return Ok(());
        }
        receive
            .await
            .map_err(|_| CodingError::History("stop acknowledgement lost"))?
    }
    pub async fn run(
        mut self,
        runtime: crate::Runtime,
    ) -> Result<CodingInvocationResult, CodingError> {
        if self.ran {
            return Err(CodingError::Stale);
        }
        self.ran = true;
        let result = if !self
            .runtime_owner
            .ptr_eq(&Arc::downgrade(&runtime.ownership()))
            || runtime.coding_provider(self.endpoint).is_err()
        {
            self.stop(&CodingError::Stale).await?;
            Err(CodingError::Stale)
        } else {
            crate::coding::agent::run_invocation(&mut self, &runtime).await
        };
        let local = runtime.shutdown().await;
        self.sender.take();
        let worker = self.worker.take().ok_or(CodingError::Stale)?;
        let mut session = tokio::task::spawn_blocking(move || worker.join())
            .await
            .map_err(|_| CodingError::History("effect join owner failed"))?
            .map_err(|_| CodingError::History("effect owner panicked"))?;
        let transports_joined = local.active == 0 && local.pending == 0;
        let mut failure = result.err().map(|e| e.to_string());
        if self.cancellation.is_cancelled() && failure.is_none() {
            failure = Some(CodingError::Cancelled.to_string());
            session.store.stop(CodingError::Cancelled.to_string())?;
        }
        if failure.is_none() && session.store.complete() && transports_joined {
            let record = tokio::task::spawn_blocking({
                let cancellation = self.cancellation.clone();
                move || session.store.publish(true, true, &cancellation)
            })
            .await
            .map_err(|_| CodingError::History("export owner failed"))??;
            Ok(CodingInvocationResult {
                record,
                success: true,
                failure: None,
            })
        } else {
            let failure = failure.unwrap_or_else(|| "local execution settlement incomplete".into());
            let record =
                tokio::task::spawn_blocking(move || session.store.diagnostic(transports_joined))
                    .await
                    .map_err(|_| CodingError::History("diagnostic owner failed"))??;
            Ok(CodingInvocationResult {
                record,
                success: false,
                failure: Some(failure),
            })
        }
    }
}
impl Drop for CodingInvocation {
    fn drop(&mut self) {
        self.cancellation.cancel();
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
