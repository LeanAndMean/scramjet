pub(crate) mod agent;
mod captured_files;
pub(crate) mod haiku;
pub(crate) mod history;
pub(crate) mod invocation;
pub(crate) mod journal;
mod process;
pub use history::{
    CODING_HISTORY_PAYLOAD_BYTES, CodingHistory, CodingHistoryEnding, CodingHistoryEntry,
    CodingHistoryRecord, CodingMaterial, reconstruct_coding_history,
};
pub(crate) mod observation;

use journal::{Journal, OPERATIONS, Record};
use rustix::fs::{FileType, Mode, OFlags, fstat, open, openat, renameat};
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const CONTENT_BYTES: usize = 1_024;
const STAGE_NAME: &str = ".coding-stage";

#[derive(Debug)]
pub enum CodingError {
    Invalid(&'static str),
    Unsupported(&'static str),
    ResourceLimit(&'static str),
    Deadline,
    Denied,
    Stale,
    TargetMismatch,
    Cancelled,
    Poisoned,
    History(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for CodingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message)
            | Self::History(message)
            | Self::Unsupported(message)
            | Self::ResourceLimit(message) => write!(f, "{message}"),
            Self::Deadline => f.write_str("coding invocation deadline exceeded"),
            Self::Denied => f.write_str("mutation capability denied"),
            Self::Stale => f.write_str("approval/intent is not the current exact invocation"),
            Self::TargetMismatch => f.write_str("target differs from exact expected bytes"),
            Self::Cancelled => f.write_str("cancelled before dispatch"),
            Self::Poisoned => {
                f.write_str("session acknowledgement failed; further execution refused")
            }
            Self::Io(error) => write!(f, "filesystem acknowledgement: {error}"),
        }
    }
}

impl std::error::Error for CodingError {}
impl From<std::io::Error> for CodingError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<rustix::io::Errno> for CodingError {
    fn from(error: rustix::io::Errno) -> Self {
        Self::Io(error.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EditIntent {
    pub session: String,
    pub root: RootIdentity,
    pub operation: u64,
    pub target: String,
    pub expected: String,
    pub replacement: String,
}

pub struct EditApproval {
    intent: EditIntent,
    owner: Arc<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingCapability {
    ReadOnly,
    ControlledEdit,
}

#[derive(Clone, Default)]
pub struct EditCancellation(Arc<AtomicBool>);

impl EditCancellation {
    /// Performs only an atomic store and is safe to call from a signal handler.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationState {
    Authorized,
    NotDispatched,
    Uncertain,
    Completed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct OperationFact {
    pub operation: u64,
    pub target: String,
    pub state: OperationState,
}

#[derive(Debug)]
pub struct EditResult {
    pub operation: u64,
    pub state: OperationState,
    pub cancellation_observed: bool,
    pub acknowledgement_error: Option<CodingError>,
}

pub struct CodingSession {
    root: File,
    identity: RootIdentity,
    pub(crate) session: String,
    capability: CodingCapability,
    journal: Journal,
    next_operation: u64,
    current: Option<EditIntent>,
    observation: Option<Arc<observation::Shared>>,
    selected: Option<crate::project::ProjectCapture>,
    selected_edited: bool,
    selected_identity: Option<RootIdentity>,
    stage_name: String,
    approval_owner: Arc<()>,
    #[cfg(test)]
    checkpoint: Option<Checkpoint>,
}

pub(crate) fn validate_name(name: &str) -> Result<(), CodingError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        || name.bytes().all(|b| b == b'.')
        || matches!(name, STAGE_NAME | journal::JOURNAL_NAME)
    {
        return Err(CodingError::Invalid(
            "target must be a nonreserved single ASCII basename of at most 64 bytes",
        ));
    }
    Ok(())
}

pub(crate) fn validate_session(session: &str) -> Result<(), CodingError> {
    if session.is_empty()
        || session.len() > 64
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(CodingError::Invalid("invalid bounded session identity"));
    }
    Ok(())
}

fn validate_intent(intent: &EditIntent) -> Result<(), CodingError> {
    validate_session(&intent.session)?;
    validate_name(&intent.target)?;
    if intent.operation == 0 || intent.operation > OPERATIONS as u64 {
        return Err(CodingError::Invalid("operation ceiling"));
    }
    if intent.expected.len() > CONTENT_BYTES || intent.replacement.len() > CONTENT_BYTES {
        return Err(CodingError::Invalid("content byte ceiling"));
    }
    if intent
        .expected
        .chars()
        .chain(intent.replacement.chars())
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(CodingError::Invalid(
            "content contains unsupported controls",
        ));
    }
    Ok(())
}

fn open_root(path: &Path) -> Result<(File, RootIdentity), CodingError> {
    let root = File::from(open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let stat = fstat(&root)?;
    if stat.st_mode & 0o7777 != 0o700 {
        return Err(CodingError::Invalid("owned root must have mode 0700"));
    }
    Ok((
        root,
        RootIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        },
    ))
}

fn validate_regular(file: &File) -> Result<(), CodingError> {
    let stat = fstat(file)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(CodingError::Invalid(
            "target must be a regular single-link file",
        ));
    }
    Ok(())
}

fn read_target(root: &File, target: &str) -> Result<String, CodingError> {
    validate_name(target)?;
    let fd = openat(
        root,
        target,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let file = File::from(fd);
    validate_regular(&file)?;
    let stat = fstat(&file)?;
    if stat.st_size < 0 || stat.st_size as u64 > CONTENT_BYTES as u64 {
        return Err(CodingError::Invalid("target byte ceiling"));
    }
    let mut buffer = [0; CONTENT_BYTES + 1];
    let mut count = 0;
    let mut file = file.take((CONTENT_BYTES + 1) as u64);
    loop {
        let read = file.read(&mut buffer[count..])?;
        if read == 0 {
            break;
        }
        count += read;
        if count > CONTENT_BYTES {
            return Err(CodingError::Invalid("target byte ceiling"));
        }
    }
    let text = std::str::from_utf8(&buffer[..count])
        .map_err(|_| CodingError::Invalid("target is not UTF-8"))?;
    if text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(CodingError::Invalid("target contains unsupported controls"));
    }
    Ok(text.to_owned())
}

impl CodingSession {
    pub fn create(
        path: &Path,
        session: &str,
        capability: CodingCapability,
    ) -> Result<Self, CodingError> {
        validate_session(session)?;
        let (root, identity) = open_root(path)?;
        let journal = Journal::create(&root, session, identity)?;
        Ok(Self {
            root,
            identity,
            session: session.to_owned(),
            capability,
            journal,
            next_operation: 1,
            current: None,
            observation: None,
            selected: None,
            selected_edited: false,
            selected_identity: None,
            stage_name: STAGE_NAME.into(),
            approval_owner: Arc::new(()),
            #[cfg(test)]
            checkpoint: None,
        })
    }

    pub(crate) fn create_selected(
        capture: crate::project::ProjectCapture,
        storage: &File,
        session: &str,
        capability: CodingCapability,
        journey: u8,
    ) -> Result<Self, CodingError> {
        validate_session(session)?;
        capture.recheck()?;
        let root = capture.workspace.root.try_clone()?;
        let identity = capture.root_identity();
        let journal = Journal::create(storage, session, identity)?;
        Ok(Self {
            root,
            identity,
            session: session.into(),
            capability,
            journal,
            next_operation: 1,
            current: None,
            observation: None,
            selected: Some(capture),
            selected_edited: false,
            selected_identity: None,
            stage_name: format!(".scramjet-stage-{session}-{journey}"),
            approval_owner: Arc::new(()),
            #[cfg(test)]
            checkpoint: None,
        })
    }

    fn recheck_selected(&self) -> Result<(), CodingError> {
        if let Some(capture) = &self.selected {
            capture.check_coding(self.selected_edited)?;
            if self.selected_edited
                && self.selected_identity
                    != Some(
                        crate::project::read_file(&self.root, capture.request().target())?.identity,
                    )
            {
                return Err(CodingError::TargetMismatch);
            }
        }
        Ok(())
    }

    pub fn read(&self, target: &str) -> Result<String, CodingError> {
        self.recheck_selected()?;
        if self.selected.is_some() {
            return crate::project::read_file(&self.root, target).map(|file| file.content);
        }
        read_target(&self.root, target)
    }
    pub fn root_identity(&self) -> RootIdentity {
        self.identity
    }

    pub fn intent(
        &self,
        target: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<EditIntent, CodingError> {
        validate_name(target)?;
        self.recheck_selected()?;
        if let Some(capture) = &self.selected
            && (self.selected_edited
                || target != capture.request().target()
                || expected != capture.expected()
                || replacement != capture.desired_target())
        {
            return Err(CodingError::Invalid(
                "edit does not satisfy captured replace_once goal",
            ));
        }
        if expected.len() > CONTENT_BYTES || replacement.len() > CONTENT_BYTES {
            return Err(CodingError::Invalid("content byte ceiling"));
        }
        let intent = EditIntent {
            session: self.session.clone(),
            root: self.identity,
            operation: self.next_operation,
            target: target.to_owned(),
            expected: expected.to_owned(),
            replacement: replacement.to_owned(),
        };
        validate_intent(&intent)?;
        Ok(intent)
    }

    pub fn approve_exact(&self, intent: &EditIntent) -> Result<EditApproval, CodingError> {
        self.validate_current(intent)?;
        if self.capability != CodingCapability::ControlledEdit {
            return Err(CodingError::Denied);
        }
        Ok(EditApproval {
            intent: intent.clone(),
            owner: self.approval_owner.clone(),
        })
    }

    fn validate_current(&self, intent: &EditIntent) -> Result<(), CodingError> {
        if self.journal.poisoned {
            return Err(CodingError::Poisoned);
        }
        validate_intent(intent)?;
        self.recheck_selected()?;
        if let Some(capture) = &self.selected
            && (self.selected_edited
                || intent.target != capture.request().target()
                || intent.expected != capture.expected()
                || intent.replacement != capture.desired_target())
        {
            return Err(CodingError::Invalid(
                "edit does not satisfy captured replace_once goal",
            ));
        }
        if intent.session != self.session
            || intent.root != self.identity
            || intent.operation != self.next_operation
            || self.current.is_some()
        {
            return Err(CodingError::Stale);
        }
        Ok(())
    }

    pub fn authorize(
        &mut self,
        intent: EditIntent,
        approval: EditApproval,
        cancellation: &EditCancellation,
    ) -> Result<(), CodingError> {
        self.validate_current(&intent)?;
        if self.capability != CodingCapability::ControlledEdit {
            return Err(CodingError::Denied);
        }
        if approval.intent != intent || !Arc::ptr_eq(&approval.owner, &self.approval_owner) {
            return Err(CodingError::Stale);
        }
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        if self.read(&intent.target)? != intent.expected {
            return Err(CodingError::TargetMismatch);
        }
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        self.journal.reserve_operation()?;
        self.journal.append(Record::Authorized {
            intent: intent.clone(),
        })?;
        self.next_operation += 1;
        self.current = Some(approval.intent);
        if let Some(observation) = &self.observation {
            observation.update(|o| {
                o.authorized = true;
                o.effect = Some(OperationState::Authorized);
            });
        }
        Ok(())
    }

    pub fn dispatch(&mut self, cancellation: &EditCancellation) -> Result<EditResult, CodingError> {
        if self.journal.poisoned {
            return Err(CodingError::Poisoned);
        }
        let intent = self.current.as_ref().ok_or(CodingError::Stale)?;
        let operation = intent.operation;
        if !cancellation.is_cancelled() {
            if self.read(&intent.target)? != intent.expected {
                return Err(CodingError::TargetMismatch);
            }
            #[cfg(test)]
            self.checkpoint_at(CheckpointPhase::BeforeDispatch)?;
        }
        if cancellation.is_cancelled() {
            self.journal.append(Record::Cancelled { operation })?;
            self.current = None;
            self.journal.release_reservation();
            if let Some(observation) = &self.observation {
                observation.update(|o| o.effect = Some(OperationState::NotDispatched));
            }
            return Ok(EditResult {
                operation,
                state: OperationState::NotDispatched,
                cancellation_observed: true,
                acknowledgement_error: None,
            });
        }
        self.journal.append(Record::Dispatched { operation })?;
        if let Some(observation) = &self.observation {
            observation.update(|o| {
                o.dispatched = true;
                o.effect = Some(OperationState::Uncertain);
            });
        }
        let intent = self.current.take().expect("serialized authorized intent");
        let result = self
            .replace(&intent)
            .and_then(|()| self.journal.append(Record::Completed { operation }));
        self.journal.release_reservation();
        if result.is_err() {
            self.journal.poisoned = true;
        } else if self.selected.is_some() {
            self.selected_edited = true;
        }
        if let Some(observation) = &self.observation {
            observation.update(|o| {
                o.effect = Some(if result.is_ok() {
                    OperationState::Completed
                } else {
                    OperationState::Uncertain
                })
            });
        }
        Ok(EditResult {
            operation,
            state: if result.is_ok() {
                OperationState::Completed
            } else {
                OperationState::Uncertain
            },
            cancellation_observed: cancellation.is_cancelled(),
            acknowledgement_error: result.err(),
        })
    }

    fn replace(&mut self, intent: &EditIntent) -> Result<(), CodingError> {
        let fd = openat(
            &self.root,
            self.stage_name.as_str(),
            OFlags::WRONLY
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let mut stage = File::from(fd);
        validate_regular(&stage)?;
        #[cfg(test)]
        if matches!(self.checkpoint, Some(Checkpoint::ShortStageWrite)) {
            stage.write_all(&intent.replacement.as_bytes()[..intent.replacement.len() / 2])?;
            return Err(std::io::Error::other("injected partial stage write").into());
        }
        #[cfg(test)]
        self.checkpoint_at(CheckpointPhase::StageWrite)?;
        stage.write_all(intent.replacement.as_bytes())?;
        if let Some(capture) = &self.selected {
            rustix::fs::fchmod(&stage, Mode::from_bits_truncate(capture.mode()))?;
            crate::project::validate_stage(&stage, capture.mode())?;
        }
        #[cfg(test)]
        self.checkpoint_at(CheckpointPhase::StageSync)?;
        stage.sync_all()?;
        #[cfg(test)]
        self.checkpoint_at(CheckpointPhase::Rename)?;
        if let Some(capture) = &self.selected {
            crate::project::check_stage(
                &self.root,
                &self.stage_name,
                &stage,
                capture.mode(),
                &intent.replacement,
            )?;
        }
        self.recheck_selected()?;
        renameat(
            &self.root,
            self.stage_name.as_str(),
            &self.root,
            intent.target.as_str(),
        )?;
        if self.selected.is_some() {
            let stat = fstat(&stage)?;
            self.selected_identity = Some(RootIdentity {
                device: stat.st_dev,
                inode: stat.st_ino,
            });
        }
        #[cfg(test)]
        self.checkpoint_at(CheckpointPhase::AfterRename)?;
        self.root.sync_all()?;
        #[cfg(test)]
        self.checkpoint_at(CheckpointPhase::BeforeCompletion)?;
        Ok(())
    }
}

pub fn reconstruct_coding(path: &Path, session: &str) -> Result<Vec<OperationFact>, CodingError> {
    validate_session(session)?;
    let (root, identity) = open_root(path)?;
    journal::reconstruct(&root, session, identity)
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointPhase {
    BeforeAuthorization,
    AfterAuthorization,
    AfterCompletion,
    BeforeVerificationMaterial,
    BeforeFinalMaterial,
    BeforeFollowUpMaterial,
    AfterFollowUpMaterial,
    BeforeDispatch,
    StageWrite,
    StageSync,
    Rename,
    AfterRename,
    BeforeCompletion,
}

#[cfg(test)]
pub(crate) enum Checkpoint {
    ShortStageWrite,
    Fail(CheckpointPhase),
    Panic(CheckpointPhase),
    Wait(
        CheckpointPhase,
        Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        Arc<AtomicBool>,
    ),
    Cancel(CheckpointPhase, EditCancellation),
    ExitAfterRename,
    Journal(CheckpointPhase, journal::JournalFault),
}

#[cfg(test)]
impl CodingSession {
    pub(crate) fn set_checkpoint(&mut self, checkpoint: Checkpoint) {
        self.checkpoint = Some(checkpoint);
    }
    fn checkpoint_at(&mut self, phase: CheckpointPhase) -> Result<(), CodingError> {
        match self.checkpoint.as_ref() {
            Some(Checkpoint::Panic(at)) if *at == phase => panic!("injected owner panic"),
            Some(Checkpoint::Wait(at, gate, entered)) if *at == phase => {
                entered.store(true, Ordering::Release);
                let (lock, changed) = &**gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = changed.wait(released).unwrap();
                }
            }
            Some(Checkpoint::Fail(at)) if *at == phase => {
                return Err(std::io::Error::other("injected effect failure").into());
            }
            Some(Checkpoint::Cancel(at, cancel)) if *at == phase => cancel.cancel(),
            Some(Checkpoint::ExitAfterRename) if phase == CheckpointPhase::AfterRename => {
                self.root.sync_all()?;
                std::process::exit(17);
            }
            Some(Checkpoint::Journal(at, fault)) if *at == phase => {
                self.journal.fault = Some(*fault)
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod selected_tests;
#[cfg(test)]
pub(crate) mod tests;
