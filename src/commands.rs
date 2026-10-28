use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat};
use std::{
    fmt,
    fs::File,
    io::Read,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub(crate) mod coding;
pub(crate) mod project;
pub(crate) mod readers;
mod schema;
use schema::{Catalog, Definition, Outcome};

pub const COMMAND_CATALOG_BYTES: usize = 4096;
pub const COMMAND_INPUT_BYTES: usize = 1024;
pub const COMMAND_TEXT_BYTES: usize = 512;
pub const COMMAND_INVOCATIONS: usize = 2;
pub const COMMAND_READS: usize = 4;
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandPolicy {
    #[default]
    Pause,
    Automatic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandInvocation {
    session: u64,
    ordinal: u8,
}
impl CommandInvocation {
    pub fn session(self) -> u64 {
        self.session
    }
    pub fn ordinal(self) -> u8 {
        self.ordinal
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandResume {
    source: CommandInvocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandPhase {
    Ready,
    Running,
    AwaitingCompletion,
    Completed,
    Failed,
    Cancelled,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandCompletion {
    Completed,
    Failed,
    Cancelled,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandReadFailure {
    Io,
    NotRegularSingleLink,
    Oversized,
    NotUtf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandReadResult {
    Read(String),
    Failed(CommandReadFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandStepFact {
    pub ordinal: u8,
    pub target: String,
    pub result: CommandReadResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandReportFact {
    pub completion: CommandCompletion,
    pub summary: String,
    pub next: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandInvocationFact {
    pub id: CommandInvocation,
    pub command: String,
    pub phase: CommandPhase,
    pub steps: [Option<CommandStepFact>; 2],
    pub report: Option<CommandReportFact>,
    pub coding: Option<coding::CommandCodingFact>,
    pub readers: Option<readers::CommandReaderFact>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandContinuation {
    None,
    Paused {
        source: CommandInvocation,
        target: String,
    },
    Pending {
        source: CommandInvocation,
        target: String,
    },
    Followed {
        source: CommandInvocation,
        target: CommandInvocation,
    },
    Suppressed {
        source: CommandInvocation,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSnapshot {
    pub invocations: [Option<CommandInvocationFact>; COMMAND_INVOCATIONS],
    pub continuation: CommandContinuation,
    pub cancellation_requested: bool,
    pub reads: usize,
    pub retained_bytes: usize,
    pub retained_text_bytes: usize,
    pub settled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandError(pub(crate) &'static str);
impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for CommandError {}

struct State {
    snapshot: CommandSnapshot,
    coding_cancel: Option<crate::EditCancellation>,
    coding_outstanding: bool,
    reader_cancel: Option<crate::ReaderControl>,
    reader_outstanding: bool,
    closed: bool,
}
#[derive(Clone)]
pub struct CommandControl {
    state: Arc<Mutex<State>>,
}
impl CommandControl {
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.snapshot.settled {
            return false;
        }
        state.snapshot.cancellation_requested = true;
        if let Some(cancel) = &state.coding_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &state.reader_cancel {
            cancel.cancel_owned_tree();
        }
        true
    }
}

#[derive(Clone)]
pub struct CommandObserver {
    identity: u64,
    state: Arc<Mutex<State>>,
}
impl CommandObserver {
    pub fn session(&self) -> u64 {
        self.identity
    }
    pub fn snapshot(&self) -> CommandSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }
}

pub struct CommandSession {
    _definitions_root: File,
    input_root: File,
    catalog: Catalog,
    identity: u64,
    policy: CommandPolicy,
    state: Arc<Mutex<State>>,
    #[cfg(test)]
    checkpoint: Option<Box<dyn Fn(Checkpoint) + Send>>,
}

fn open_root(path: &Path) -> Result<File, CommandError> {
    let root = File::from(
        open(
            path,
            OFlags::RDONLY
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| CommandError("cannot open owned root"))?,
    );
    let stat = fstat(&root).map_err(|_| CommandError("cannot inspect owned root"))?;
    if stat.st_mode & 0o7777 != 0o700 {
        return Err(CommandError("owned root must have mode 0700"));
    }
    Ok(root)
}

fn read_file<const N: usize>(root: &File, target: &str) -> Result<String, CommandReadFailure> {
    let mut file = File::from(
        openat(
            root,
            target,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| CommandReadFailure::Io)?,
    );
    let stat = fstat(&file).map_err(|_| CommandReadFailure::Io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(CommandReadFailure::NotRegularSingleLink);
    }
    if stat.st_size < 0 || stat.st_size as u64 > (N - 1) as u64 {
        return Err(CommandReadFailure::Oversized);
    }
    // The extra byte detects growth after metadata inspection without truncating semantics.
    let mut buffer = [0; N];
    let mut count = 0;
    loop {
        let read = file
            .read(&mut buffer[count..])
            .map_err(|_| CommandReadFailure::Io)?;
        if read == 0 {
            break;
        }
        count += read;
        if count == N {
            return Err(CommandReadFailure::Oversized);
        }
    }
    Ok(std::str::from_utf8(&buffer[..count])
        .map_err(|_| CommandReadFailure::NotUtf8)?
        .to_owned())
}

impl CommandSession {
    pub fn load(
        definitions: &Path,
        inputs: &Path,
        policy: CommandPolicy,
    ) -> Result<Self, CommandError> {
        let definitions_root = open_root(definitions)?;
        let input_root = open_root(inputs)?;
        let a = fstat(&definitions_root).map_err(|_| CommandError("cannot inspect owned root"))?;
        let b = fstat(&input_root).map_err(|_| CommandError("cannot inspect owned root"))?;
        if (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino) {
            return Err(CommandError("roots must be separate"));
        }
        let text = read_file::<{ COMMAND_CATALOG_BYTES + 1 }>(&definitions_root, "commands.json")
            .map_err(|_| CommandError("invalid catalog file"))?;
        let catalog = schema::decode(&text)?;
        let identity = NEXT_SESSION
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| CommandError("session identity exhausted"))?;
        Ok(Self {
            _definitions_root: definitions_root,
            input_root,
            catalog,
            identity,
            policy,
            state: Arc::new(Mutex::new(State {
                snapshot: CommandSnapshot {
                    invocations: [None, None],
                    continuation: CommandContinuation::None,
                    cancellation_requested: false,
                    reads: 0,
                    retained_bytes: 0,
                    retained_text_bytes: 0,
                    settled: false,
                },
                coding_cancel: None,
                coding_outstanding: false,
                reader_cancel: None,
                reader_outstanding: false,
                closed: false,
            })),
            #[cfg(test)]
            checkpoint: None,
        })
    }
    pub fn control(&self) -> CommandControl {
        CommandControl {
            state: self.state.clone(),
        }
    }
    pub fn observer(&self) -> CommandObserver {
        CommandObserver {
            identity: self.identity,
            state: self.state.clone(),
        }
    }
    pub fn snapshot(&self) -> CommandSnapshot {
        self.state.lock().unwrap().snapshot.clone()
    }
    pub fn discover(&self, name: &str) -> Result<(), CommandError> {
        self.definition(name).map(|_| ())
    }
    fn definition(&self, name: &str) -> Result<&Definition, CommandError> {
        schema::validate_command_name(name)?;
        self.catalog
            .definitions
            .iter()
            .find(|d| d.name == name)
            .ok_or(CommandError("unknown command"))
    }
    pub fn begin(&mut self, name: &str) -> Result<CommandInvocation, CommandError> {
        if matches!(
            self.definition(name)?.outcome,
            Outcome::ReplaceVerified | Outcome::AllReaders
        ) {
            return Err(CommandError(
                "coding command requires complete input admission",
            ));
        }
        let mut state = self.state.lock().unwrap();
        if state.snapshot.settled
            || state.snapshot.cancellation_requested
            || state.snapshot.invocations[0].is_some()
        {
            return Err(CommandError("initial invocation unavailable"));
        }
        Ok(self.admit(&mut state, name, 0))
    }
    fn admit(&self, state: &mut State, name: &str, index: usize) -> CommandInvocation {
        let id = CommandInvocation {
            session: self.identity,
            ordinal: index as u8 + 1,
        };
        state.snapshot.invocations[index] = Some(CommandInvocationFact {
            id,
            command: name.to_owned(),
            phase: CommandPhase::Ready,
            steps: [None, None],
            report: None,
            coding: None,
            readers: None,
        });
        id
    }
    fn index(&self, state: &State, id: CommandInvocation) -> Result<usize, CommandError> {
        if id.session != self.identity
            || id.ordinal == 0
            || id.ordinal as usize > COMMAND_INVOCATIONS
        {
            return Err(CommandError("foreign invocation"));
        }
        let index = id.ordinal as usize - 1;
        if state.snapshot.invocations[index]
            .as_ref()
            .is_none_or(|f| f.id != id)
        {
            return Err(CommandError("unknown invocation"));
        }
        Ok(index)
    }
    pub fn execute(&mut self, id: CommandInvocation) -> Result<(), CommandError> {
        let definition = {
            let mut state = self.state.lock().unwrap();
            let index = self.index(&state, id)?;
            if state.snapshot.settled {
                return Err(CommandError("session finalized"));
            }
            let fact = state.snapshot.invocations[index].as_mut().unwrap();
            if fact.phase != CommandPhase::Ready {
                return Err(CommandError("invocation already executed"));
            }
            let definition = self.definition(&fact.command)?.clone();
            if matches!(
                definition.outcome,
                Outcome::ReplaceVerified | Outcome::AllReaders
            ) {
                return Err(CommandError("command requires bound execution"));
            }
            fact.phase = CommandPhase::Running;
            definition
        };
        for (ordinal, target) in definition.steps.iter().enumerate() {
            #[cfg(test)]
            self.hit(Checkpoint::BeforeRead(ordinal));
            {
                let mut state = self.state.lock().unwrap();
                if state.snapshot.cancellation_requested {
                    break;
                }
                if state.snapshot.reads >= COMMAND_READS {
                    return Err(CommandError("read ceiling"));
                }
                state.snapshot.reads += 1;
            }
            #[cfg(test)]
            self.hit(Checkpoint::Dispatched(ordinal));
            let result = match read_file::<{ COMMAND_INPUT_BYTES + 1 }>(&self.input_root, target) {
                Ok(text) => CommandReadResult::Read(text),
                Err(error) => CommandReadResult::Failed(error),
            };
            #[cfg(test)]
            self.hit(Checkpoint::Returned(ordinal));
            let mut state = self.state.lock().unwrap();
            if let CommandReadResult::Read(text) = &result {
                state.snapshot.retained_bytes += text.len();
            }
            let failed = matches!(result, CommandReadResult::Failed(_));
            state.snapshot.invocations[id.ordinal as usize - 1]
                .as_mut()
                .unwrap()
                .steps[ordinal] = Some(CommandStepFact {
                ordinal: ordinal as u8,
                target: target.clone(),
                result,
            });
            if failed || state.snapshot.cancellation_requested {
                break;
            }
        }
        self.state.lock().unwrap().snapshot.invocations[id.ordinal as usize - 1]
            .as_mut()
            .unwrap()
            .phase = CommandPhase::AwaitingCompletion;
        Ok(())
    }
    pub fn report(
        &mut self,
        id: CommandInvocation,
        completion: CommandCompletion,
        summary: &str,
        next: Option<&str>,
    ) -> Result<(), CommandError> {
        if summary.len() > COMMAND_TEXT_BYTES || !summary.bytes().all(|b| (32..=126).contains(&b)) {
            return Err(CommandError("invalid completion text"));
        }
        if let Some(next) = next {
            schema::validate_command_name(next)?;
        }
        #[cfg(test)]
        self.hit(Checkpoint::BeforeReport);
        let mut state = self.state.lock().unwrap();
        let index = self.index(&state, id)?;
        if state.snapshot.settled {
            return Err(CommandError("session finalized"));
        }
        let fact = state.snapshot.invocations[index].as_ref().unwrap();
        let cancelled_unlaunched = fact.phase == CommandPhase::Ready
            && (fact.coding.as_ref().is_some_and(|c| c.owner.is_none())
                || fact.readers.as_ref().is_some_and(|r| !r.launched))
            && state.snapshot.cancellation_requested
            && matches!(
                completion,
                CommandCompletion::Cancelled | CommandCompletion::Incomplete
            );
        if fact.phase != CommandPhase::AwaitingCompletion && !cancelled_unlaunched {
            return Err(CommandError("completion is not pending"));
        }
        let definition = self.definition(&fact.command)?;
        let coding = fact.coding.as_ref().and_then(|c| c.execution.as_ref());
        let readers = fact.readers.as_ref().and_then(|r| r.execution.as_ref());
        let satisfied = if definition.outcome == Outcome::AllReaders {
            readers.is_some_and(|e| e.disposition == CommandCompletion::Completed)
        } else if definition.outcome == Outcome::ReplaceVerified {
            coding.is_some_and(|e| e.disposition == CommandCompletion::Completed)
        } else {
            definition.satisfied(&fact.steps)
        };
        let failed = if definition.outcome == Outcome::AllReaders {
            readers.is_some_and(|e| e.disposition == CommandCompletion::Failed)
        } else if definition.outcome == Outcome::ReplaceVerified {
            coding.is_some_and(|e| e.disposition == CommandCompletion::Failed)
        } else {
            fact.steps
                .iter()
                .flatten()
                .any(|s| matches!(s.result, CommandReadResult::Failed(_)))
                || (!state.snapshot.cancellation_requested && !satisfied)
        };
        match completion {
            CommandCompletion::Completed if !satisfied || state.snapshot.cancellation_requested => {
                return Err(CommandError("success is not established"));
            }
            CommandCompletion::Failed
                if !failed
                    || (matches!(
                        definition.outcome,
                        Outcome::ReplaceVerified | Outcome::AllReaders
                    ) && state.snapshot.cancellation_requested) =>
            {
                return Err(CommandError("failure is not established"));
            }
            CommandCompletion::Cancelled if !state.snapshot.cancellation_requested => {
                return Err(CommandError("cancellation is not accepted"));
            }
            _ => (),
        }
        if let Some(next) = next {
            if completion != CommandCompletion::Completed
                || definition.next.as_ref().is_none_or(|e| e.target != next)
            {
                return Err(CommandError("ineligible continuation"));
            }
            self.definition(next)?;
            if index + 1 >= COMMAND_INVOCATIONS {
                return Err(CommandError("invocation ceiling"));
            }
        }
        if state.snapshot.retained_text_bytes + summary.len() > COMMAND_TEXT_BYTES {
            return Err(CommandError("completion text ceiling"));
        }
        let continuation = next.map(|target| {
            if definition.next.as_ref().unwrap().policy == CommandPolicy::Automatic
                && self.policy == CommandPolicy::Automatic
            {
                CommandContinuation::Pending {
                    source: id,
                    target: target.to_owned(),
                }
            } else {
                CommandContinuation::Paused {
                    source: id,
                    target: target.to_owned(),
                }
            }
        });
        state.snapshot.retained_text_bytes += summary.len();
        let fact = state.snapshot.invocations[index].as_mut().unwrap();
        fact.phase = match completion {
            CommandCompletion::Completed => CommandPhase::Completed,
            CommandCompletion::Failed => CommandPhase::Failed,
            CommandCompletion::Cancelled => CommandPhase::Cancelled,
            CommandCompletion::Incomplete => CommandPhase::Incomplete,
        };
        fact.report = Some(CommandReportFact {
            completion,
            summary: summary.to_owned(),
            next: next.map(str::to_owned),
        });
        if let Some(continuation) = continuation {
            state.snapshot.continuation = continuation;
        }
        Ok(())
    }
    pub fn paused_continuation(&self) -> Option<CommandResume> {
        let state = self.state.lock().unwrap();
        if state.closed || state.snapshot.cancellation_requested {
            return None;
        }
        match state.snapshot.continuation {
            CommandContinuation::Paused { source, .. }
                if state.snapshot.invocations[1].is_none() =>
            {
                Some(CommandResume { source })
            }
            _ => None,
        }
    }
    pub fn resume(
        &mut self,
        handle: CommandResume,
    ) -> Result<Option<CommandInvocation>, CommandError> {
        #[cfg(test)]
        self.hit(Checkpoint::BeforeResume);
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err(CommandError("session finalized"));
        }
        let CommandContinuation::Paused { source, target } = &state.snapshot.continuation else {
            return Err(CommandError("no paused continuation"));
        };
        if handle.source != *source || state.snapshot.invocations[1].is_some() {
            return Err(CommandError("stale or foreign resume"));
        }
        let source = *source;
        let index = self.index(&state, source)?;
        let fact = state.snapshot.invocations[index].as_ref().unwrap();
        let definition = self.definition(&fact.command)?;
        if fact.phase != CommandPhase::Completed
            || fact.report.as_ref().is_none_or(|r| {
                r.completion != CommandCompletion::Completed || r.next.as_ref() != Some(target)
            })
            || definition.next.as_ref().is_none_or(|e| e.target != *target)
            || matches!(
                self.definition(target)?.outcome,
                Outcome::ReplaceVerified | Outcome::AllReaders
            )
        {
            return Err(CommandError("ineligible paused continuation"));
        }
        if state.snapshot.cancellation_requested {
            state.snapshot.continuation = CommandContinuation::Suppressed { source };
            return Ok(None);
        }
        let target = target.clone();
        let id = self.admit(&mut state, &target, 1);
        state.snapshot.continuation = CommandContinuation::Followed { source, target: id };
        Ok(Some(id))
    }
    pub fn follow(&mut self) -> Result<Option<CommandInvocation>, CommandError> {
        #[cfg(test)]
        self.hit(Checkpoint::BeforeFollow);
        let id = {
            let mut state = self.state.lock().unwrap();
            if state.snapshot.settled {
                return Err(CommandError("session finalized"));
            }
            let CommandContinuation::Pending { source, target } = &state.snapshot.continuation
            else {
                return Err(CommandError("no automatic continuation pending"));
            };
            let source = *source;
            let target = target.clone();
            if state.snapshot.cancellation_requested {
                state.snapshot.continuation = CommandContinuation::Suppressed { source };
                return Ok(None);
            }
            let index = state
                .snapshot
                .invocations
                .iter()
                .filter(|f| f.is_some())
                .count();
            if index >= COMMAND_INVOCATIONS {
                return Err(CommandError("invocation ceiling"));
            }
            let id = self.admit(&mut state, &target, index);
            state.snapshot.continuation = CommandContinuation::Followed { source, target: id };
            id
        };
        #[cfg(test)]
        self.hit(Checkpoint::AdmittedFollow);
        self.execute(id)?;
        Ok(Some(id))
    }
    pub fn finalize(self) -> CommandSnapshot {
        self.close();
        self.snapshot()
    }
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        for fact in state.snapshot.invocations.iter_mut().flatten() {
            if matches!(
                fact.phase,
                CommandPhase::Ready | CommandPhase::Running | CommandPhase::AwaitingCompletion
            ) {
                fact.phase = CommandPhase::Incomplete;
            }
        }
        match state.snapshot.continuation {
            CommandContinuation::Pending { source, .. } => {
                state.snapshot.continuation = CommandContinuation::Suppressed { source };
            }
            CommandContinuation::Paused { source, .. } if state.snapshot.cancellation_requested => {
                state.snapshot.continuation = CommandContinuation::Suppressed { source };
            }
            _ => (),
        }
        if let Some(cancel) = &state.coding_cancel {
            cancel.cancel();
        }
        if let Some(cancel) = &state.reader_cancel {
            cancel.cancel_owned_tree();
        }
        state.closed = true;
        state.snapshot.settled = !state.coding_outstanding && !state.reader_outstanding;
    }
    #[cfg(test)]
    fn hit(&self, at: Checkpoint) {
        if let Some(hook) = &self.checkpoint {
            hook(at);
        }
    }
}
impl Drop for CommandSession {
    fn drop(&mut self) {
        self.close();
    }
}

impl Definition {
    fn satisfied(&self, facts: &[Option<CommandStepFact>; 2]) -> bool {
        let texts: [Option<&str>; 2] =
            std::array::from_fn(|i| match facts[i].as_ref().map(|f| &f.result) {
                Some(CommandReadResult::Read(text)) => Some(text.as_str()),
                _ => None,
            });
        match self.outcome {
            Outcome::Equal => texts[0].is_some() && texts[0] == texts[1],
            Outcome::Readable => texts[0].is_some(),
            Outcome::ReplaceVerified | Outcome::AllReaders => false,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Checkpoint {
    BeforeRead(usize),
    Dispatched(usize),
    Returned(usize),
    BeforeReport,
    BeforeCodingLaunch,
    AfterCodingLaunch,
    BeforeFollow,
    AdmittedFollow,
    BeforeResume,
}
#[cfg(test)]
mod tests;
