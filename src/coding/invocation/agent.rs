use super::*;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::mpsc::{SyncSender, sync_channel},
    thread::JoinHandle,
    time::Duration,
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
}
pub struct CodingInvocationResult {
    pub record: Vec<u8>,
    pub success: bool,
    pub failure: Option<String>,
    pub(super) publication: Option<PublicationOwner>,
}
pub(super) struct PublicationOwner {
    pub store: store::Store,
    pub cancellation: EditCancellation,
}
impl Drop for PublicationOwner {
    fn drop(&mut self) {
        if self.cancellation.is_cancelled()
            && let Err(error) = self.store.stop(CodingError::Cancelled.to_string())
        {
            CodingInvocationResult::report_failure(&format!(
                "publication stop persistence incomplete; no retry: {error}"
            ));
        }
    }
}
struct PublicationTask {
    handle: tokio::task::JoinHandle<Result<(Vec<u8>, PublicationOwner), CodingError>>,
    cancellation: EditCancellation,
    armed: bool,
}
impl Drop for PublicationTask {
    fn drop(&mut self) {
        // Cancel before the handle discards a completed, unconsumed publication owner.
        if self.armed {
            self.cancellation.cancel();
        }
    }
}
impl CodingInvocationResult {
    /// Best-effort terminal-safe diagnostics never wait for a pipe, socket or terminal reader.
    pub fn report_failure(message: &str) {
        use rustix::fs::{FileType, OFlags, fcntl_getfl, fcntl_setfl, fstat};
        let stderr = std::io::stderr();
        let Ok(stat) = fstat(&stderr) else { return };
        if !matches!(
            FileType::from_raw_mode(stat.st_mode),
            FileType::Fifo | FileType::Socket
        ) && !rustix::termios::isatty(&stderr)
        {
            return;
        }
        let Ok(flags) = fcntl_getfl(&stderr) else {
            return;
        };
        if fcntl_setfl(&stderr, flags | OFlags::NONBLOCK).is_err() {
            return;
        }
        let mut text = String::from("coding: ");
        for c in serde_json::to_string(message).unwrap().chars() {
            if c.is_ascii() && !c.is_ascii_control() {
                text.push(c);
            } else {
                text.extend(c.escape_unicode());
            }
        }
        text.push('\n');
        let _ = rustix::io::write(&stderr, text.as_bytes());
        let _ = fcntl_setfl(&stderr, flags);
    }

    /// Synchronous writers must return before cancellation or deadline checks can run.
    pub fn deliver(self, writer: &mut impl std::io::Write) -> Result<(), CodingError> {
        let result: Result<(), CodingError> = (|| {
            if let Some(owner) = &self.publication {
                owner.store.publication_guard(&owner.cancellation)?;
            }
            writer.write_all(&self.record)?;
            if let Some(owner) = &self.publication {
                owner.store.publication_guard(&owner.cancellation)?;
            }
            writer.write_all(b"\n")?;
            if let Some(owner) = &self.publication {
                owner.store.publication_guard(&owner.cancellation)?;
            }
            writer.flush()?;
            if let Some(owner) = &self.publication {
                owner.store.publication_guard(&owner.cancellation)?;
            }
            Ok(())
        })();
        self.finish_delivery(result)
    }

    /// Requires exclusive ownership of the descriptor's output and shared status flags.
    pub fn deliver_fd(self, fd: impl std::os::fd::AsFd) -> Result<(), CodingError> {
        self.deliver_descriptor(fd, None)
    }

    /// Retains caller cancellation through delivery, including noncontinuable diagnostics.
    pub fn deliver_fd_with_cancellation(
        self,
        fd: impl std::os::fd::AsFd,
        cancellation: &EditCancellation,
    ) -> Result<(), CodingError> {
        self.deliver_descriptor(fd, Some(cancellation))
    }

    fn deliver_descriptor(
        self,
        fd: impl std::os::fd::AsFd,
        cancellation: Option<&EditCancellation>,
    ) -> Result<(), CodingError> {
        use rustix::{
            event::{PollFd, PollFlags, Timespec, poll},
            fs::{FileType, OFlags, fcntl_getfl, fcntl_setfl, fstat},
            io::Errno,
        };
        let result = (|| {
            let kind = FileType::from_raw_mode(fstat(&fd)?.st_mode);
            if !matches!(kind, FileType::Fifo | FileType::Socket) && !rustix::termios::isatty(&fd) {
                return Err(CodingError::History(
                    "stdout requires a pipe, socket or terminal",
                ));
            }
            let mut output = std::fs::File::from(fd.as_fd().try_clone_to_owned()?);
            let flags = fcntl_getfl(&fd)?;
            fcntl_setfl(&fd, flags | OFlags::NONBLOCK)?;
            let delivered = (|| {
                let diagnostic_deadline = Instant::now() + Duration::from_secs(5);
                for bytes in [self.record.as_slice(), b"\n"] {
                    let mut remaining = bytes;
                    while !remaining.is_empty() {
                        if cancellation.is_some_and(EditCancellation::is_cancelled) {
                            return Err(CodingError::Cancelled);
                        }
                        if let Some(owner) = &self.publication {
                            owner.store.publication_guard(&owner.cancellation)?;
                        } else if Instant::now() >= diagnostic_deadline {
                            return Err(CodingError::Deadline);
                        }
                        match std::io::Write::write(
                            &mut output,
                            &remaining[..remaining.len().min(4096)],
                        ) {
                            Ok(0) => {
                                return Err(
                                    std::io::Error::from(std::io::ErrorKind::WriteZero).into()
                                );
                            }
                            Ok(count) => remaining = &remaining[count..],
                            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                continue;
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                let mut descriptors = [PollFd::new(&fd, PollFlags::OUT)];
                                match poll(
                                    &mut descriptors,
                                    Some(&Timespec {
                                        tv_sec: 0,
                                        tv_nsec: 10_000_000,
                                    }),
                                ) {
                                    Ok(_) | Err(Errno::INTR) => {}
                                    Err(error) => return Err(error.into()),
                                }
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                if cancellation.is_some_and(EditCancellation::is_cancelled) {
                    return Err(CodingError::Cancelled);
                }
                if let Some(owner) = &self.publication {
                    owner.store.publication_guard(&owner.cancellation)?;
                }
                Ok(())
            })();
            let restored = fcntl_setfl(&fd, flags).map_err(CodingError::from);
            delivered.and(restored)
        })();
        self.finish_delivery(result)
    }

    fn finish_delivery(mut self, result: Result<(), CodingError>) -> Result<(), CodingError> {
        let result = result.and_then(|()| {
            if let Some(owner) = &self.publication {
                owner.store.publication_guard(&owner.cancellation)?;
            }
            Ok(())
        });
        if let Err(error) = &result
            && let Some(owner) = &mut self.publication
        {
            owner.store.stop(error.to_string()).map_err(|stop| {
                CodingError::Io(std::io::Error::other(format!(
                    "publication stop persistence incomplete; no retry: {stop}"
                )))
            })?;
        }
        result?;
        if self.success {
            Ok(())
        } else {
            Err(CodingError::History("invocation stopped"))
        }
    }
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
    publication_transferred: bool,
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
        if !options.endpoint.ip().is_loopback() {
            return Err(CodingError::Unsupported(
                "only a loopback Responses endpoint is supported",
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
                publication_transferred: false,
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
            let mut task = PublicationTask {
                handle: tokio::task::spawn_blocking({
                    let cancellation = self.cancellation.clone();
                    move || {
                        let mut owner = PublicationOwner {
                            store: session.store,
                            cancellation,
                        };
                        let record = owner.store.publish(true, true, &owner.cancellation)?;
                        Ok((record, owner))
                    }
                }),
                cancellation: self.cancellation.clone(),
                armed: true,
            };
            let record = (&mut task.handle)
                .await
                .map_err(|_| CodingError::History("export owner failed"))??;
            task.armed = false;
            self.publication_transferred = true;
            Ok(CodingInvocationResult {
                record: record.0,
                success: true,
                failure: None,
                publication: Some(record.1),
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
                publication: None,
            })
        }
    }
}
impl Drop for CodingInvocation {
    fn drop(&mut self) {
        if !self.publication_transferred {
            self.cancellation.cancel();
        }
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
