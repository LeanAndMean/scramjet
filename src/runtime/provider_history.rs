use crate::http::responses::{
    PROVIDER_FRAME_BYTES, PROVIDER_MODEL, bounded_json_bytes, bounded_json_value, identity,
};
use crate::{Outcome, ProviderUsage};
use rustix::fs::{CWD, FlockOperation, Mode, OFlags, flock, openat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read},
    net::SocketAddr,
    os::unix::fs::{FileExt, MetadataExt},
    path::{Component, Path},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub const PROVIDER_POST_BUDGET: usize = 32;
// Terminal facts fit the accepted wire frame plus record wrapping and renamed scalar fields.
const RECORD_BYTES: usize = PROVIDER_FRAME_BYTES + 1024;
const HISTORY_RECORDS: usize = 1 + 2 * PROVIDER_POST_BUDGET;
const HISTORY_BYTES: u64 = (HISTORY_RECORDS * (RECORD_BYTES + 1)) as u64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderTerminalEvidence {
    pub response_id: String,
    pub reported_model: Option<String>,
    pub status: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderAttemptFact {
    pub ordinal: usize,
    pub request_id: u64,
    pub terminal: Option<ProviderTerminalEvidence>,
    pub usage: Option<ProviderUsage>,
    pub locally_joined: bool,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderHistory {
    pub attempts: Vec<ProviderAttemptFact>,
}
impl ProviderHistory {
    pub fn consumed(&self) -> usize {
        self.attempts.len()
    }
    pub fn remaining(&self) -> usize {
        PROVIDER_POST_BUDGET - self.consumed()
    }
    pub fn unresolved_remote(&self) -> usize {
        self.attempts
            .iter()
            .filter(|a| a.terminal.is_none())
            .count()
    }
    pub fn has_unknown_after(&self, ordinal: usize) -> bool {
        self.attempts.iter().any(|attempt| {
            attempt.ordinal > ordinal
                && (attempt.terminal.is_none()
                    || attempt.usage.as_ref().is_none_or(|usage| !usage.complete()))
        })
    }
    pub fn unknown_usage(&self) -> usize {
        self.attempts
            .iter()
            .filter(|a| a.usage.as_ref().is_none_or(|u| !u.complete()))
            .count()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Record {
    Header {
        endpoint: String,
        model: String,
        post_budget: usize,
        max_output_tokens: u16,
    },
    Reserved {
        ordinal: usize,
        request_id: u64,
    },
    Joined {
        ordinal: usize,
        terminal: Option<ProviderTerminalEvidence>,
        usage: Option<ProviderUsage>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProviderBinding {
    pub directory: [u64; 2],
    pub file: [u64; 2],
    pub endpoint: SocketAddr,
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum AppendFault {
    PartialWrite,
    Sync,
    Content,
    Binding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationCheckpoint {
    pub binding: ProviderBinding,
    pub bytes: usize,
    pub sha256: [u8; 32],
}

#[derive(Clone, Copy)]
enum Policy {
    Ordinary,
    Validation,
}
impl Policy {
    fn caps(self) -> (usize, usize) {
        match self {
            Self::Ordinary => (4, PROVIDER_POST_BUDGET),
            Self::Validation => (6, 6),
        }
    }
    fn eligible(self, history: &ProviderHistory) -> bool {
        let (exposure, ceiling) = self.caps();
        history.consumed() < ceiling
            && history.unresolved_remote() < exposure
            && history.unknown_usage() < exposure
            && (!matches!(self, Self::Validation) || !history.has_unknown_after(4))
    }
}

pub(crate) struct ProviderLedger {
    file: File,
    directory: File,
    path: std::path::PathBuf,
    endpoint: SocketAddr,
    history: ProviderHistory,
    acknowledged: Vec<Vec<u8>>,
    policy: Policy,
    checkpoint: Option<ValidationCheckpoint>,
    cancellation: Option<Arc<AtomicBool>>,
    poisoned: bool,
    #[cfg(test)]
    append_fault: Option<AppendFault>,
}
fn invalid() -> io::Error {
    io::Error::other("invalid or unacknowledged provider history")
}
pub(crate) fn directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut dir = File::from(openat(
        CWD,
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                dir = File::from(openat(
                    &dir,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?)
            }
            _ => return Err(invalid()),
        }
    }
    let metadata = dir.metadata()?;
    if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    Ok(dir)
}
fn open(path: &Path, writable: bool, create: bool) -> io::Result<(File, File)> {
    let dir = directory(path)?;
    let flags = if writable {
        OFlags::RDWR
    } else {
        OFlags::RDONLY
    };
    let file = match openat(
        &dir,
        "provider-attempts.jsonl",
        flags | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => File::from(fd),
        Err(rustix::io::Errno::NOENT) if writable && create => File::from(openat(
            &dir,
            "provider-attempts.jsonl",
            flags | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?),
        Err(error) => return Err(error.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
        || metadata.len() > HISTORY_BYTES
    {
        return Err(invalid());
    }
    flock(
        &file,
        if writable {
            FlockOperation::NonBlockingLockExclusive
        } else {
            FlockOperation::NonBlockingLockShared
        },
    )?;
    Ok((file, dir))
}
fn read(
    file: &mut File,
    endpoint: SocketAddr,
    policy: Policy,
) -> io::Result<(ProviderHistory, Vec<u8>)> {
    let capacity = file.metadata()?.len().min(HISTORY_BYTES) as usize + 1;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(HISTORY_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > HISTORY_BYTES || !bytes.ends_with(b"\n") {
        return Err(invalid());
    }
    let mut history = ProviderHistory::default();
    for (line_index, line) in bytes[..bytes.len() - 1].split(|b| *b == b'\n').enumerate() {
        if line_index >= HISTORY_RECORDS || line.len() > RECORD_BYTES {
            return Err(invalid());
        }
        let value = bounded_json_value(line, RECORD_BYTES).map_err(|_| invalid())?;
        let record: Record = serde_json::from_value(value).map_err(|_| invalid())?;
        match record {
            Record::Header {
                endpoint: stored,
                model,
                post_budget,
                max_output_tokens,
            } if line_index == 0
                && stored == endpoint.to_string()
                && model == PROVIDER_MODEL
                && post_budget == PROVIDER_POST_BUDGET
                && max_output_tokens == 1024 => {}
            Record::Reserved {
                ordinal,
                request_id,
            } if line_index > 0
                && ordinal == history.attempts.len() + 1
                && policy.eligible(&history)
                && request_id != 0
                && history.attempts.iter().all(|a| a.request_id != request_id) =>
            {
                history.attempts.push(ProviderAttemptFact {
                    ordinal,
                    request_id,
                    terminal: None,
                    usage: None,
                    locally_joined: false,
                });
            }
            Record::Joined {
                ordinal,
                terminal,
                usage,
            } if line_index > 0 => {
                let attempt = history
                    .attempts
                    .get_mut(ordinal.checked_sub(1).ok_or_else(invalid)?)
                    .ok_or_else(invalid)?;
                if attempt.locally_joined {
                    return Err(invalid());
                }
                if let Some(terminal) = &terminal
                    && (!identity(&terminal.response_id)
                        || !matches!(
                            terminal.status.as_str(),
                            "completed" | "failed" | "incomplete"
                        ))
                {
                    return Err(invalid());
                }
                if terminal.as_ref().is_some_and(|t| {
                    t.reported_model
                        .as_ref()
                        .is_some_and(|model| model.is_empty())
                }) {
                    return Err(invalid());
                }
                if usage.is_some() && terminal.is_none() {
                    return Err(invalid());
                }
                if let Some(usage) = &usage {
                    usage.validate().map_err(|_| invalid())?;
                }
                attempt.locally_joined = true;
                attempt.terminal = terminal;
                attempt.usage = usage;
            }
            _ => return Err(invalid()),
        }
    }
    Ok((history, bytes))
}
impl ProviderLedger {
    pub(crate) fn open(path: &Path, endpoint: SocketAddr) -> io::Result<Arc<Mutex<Self>>> {
        if !endpoint.ip().is_loopback() {
            return Err(invalid());
        }
        let (mut file, directory) = open(path, true, true)?;
        let empty = file.metadata()?.len() == 0;
        let (history, acknowledged) = if empty {
            (ProviderHistory::default(), Vec::new())
        } else {
            read(&mut file, endpoint, Policy::Ordinary)?
        };
        let mut ledger = Self {
            file,
            directory,
            path: path.into(),
            endpoint,
            history,
            acknowledged: vec![acknowledged],
            policy: Policy::Ordinary,
            checkpoint: None,
            cancellation: None,
            poisoned: false,
            #[cfg(test)]
            append_fault: None,
        };
        ledger.verify_content(&[])?;
        if empty {
            ledger.append(&Record::Header {
                endpoint: endpoint.to_string(),
                model: PROVIDER_MODEL.to_owned(),
                post_budget: PROVIDER_POST_BUDGET,
                max_output_tokens: 1024,
            })?;
            ledger.directory.sync_all()?;
        }
        Ok(Arc::new(Mutex::new(ledger)))
    }
    pub(crate) fn validation_open(
        path: &Path,
        endpoint: SocketAddr,
        expected: [u8; 32],
    ) -> io::Result<Arc<Mutex<Self>>> {
        if !endpoint.ip().is_loopback() {
            return Err(invalid());
        }
        let (mut file, directory) = open(path, true, false)?;
        let (history, bytes) = read(&mut file, endpoint, Policy::Validation)?;
        if Sha256::digest(&bytes).as_slice() != expected
            || history.consumed() != 4
            || history.unresolved_remote() != 4
            || history.unknown_usage() != 4
            || history.attempts.iter().any(|a| !a.locally_joined)
        {
            return Err(invalid());
        }
        let mut ledger = Self {
            file,
            directory,
            path: path.into(),
            endpoint,
            history,
            acknowledged: vec![bytes],
            policy: Policy::Validation,
            checkpoint: None,
            cancellation: None,
            poisoned: false,
            #[cfg(test)]
            append_fault: None,
        };
        ledger.verify_content(&[])?;
        ledger.checkpoint = Some(ValidationCheckpoint {
            binding: ledger.binding()?,
            bytes: ledger.acknowledged_len(),
            sha256: expected,
        });
        Ok(Arc::new(Mutex::new(ledger)))
    }

    pub(crate) fn validation_cancellation(&mut self, flag: Arc<AtomicBool>) -> io::Result<()> {
        if !matches!(self.policy, Policy::Validation) {
            return Err(invalid());
        }
        self.cancellation = Some(flag);
        Ok(())
    }
    pub(crate) fn validation_checkpoint(&self) -> io::Result<ValidationCheckpoint> {
        self.verify_content(&[])?;
        let checkpoint = self.checkpoint.as_ref().ok_or_else(invalid)?;
        if self.binding()? != checkpoint.binding {
            return Err(invalid());
        }
        Ok(checkpoint.clone())
    }

    pub(crate) fn validation_observe(&mut self) -> io::Result<(ProviderHistory, [u8; 32], usize)> {
        let checkpoint = self.validation_checkpoint()?;
        let mut file = self.file.try_clone()?;
        let mut bytes = vec![0; self.acknowledged_len()];
        file.read_exact_at(&mut bytes, 0)?;
        if bytes.len() < checkpoint.bytes
            || Sha256::digest(&bytes[..checkpoint.bytes]).as_slice() != checkpoint.sha256
        {
            return Err(invalid());
        }
        file = File::from(openat(
            &self.directory,
            "provider-attempts.jsonl",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let (history, read_bytes) = read(&mut file, self.endpoint, Policy::Validation)?;
        if read_bytes != bytes || history != self.history {
            return Err(invalid());
        }
        self.verify_content(&[])?;
        Ok((history, Sha256::digest(&bytes).into(), bytes.len()))
    }

    #[cfg(test)]
    pub(crate) fn force_write_error(&mut self) {
        self.file = File::from(
            openat(
                &self.directory,
                "provider-attempts.jsonl",
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap(),
        );
        flock(&self.file, FlockOperation::NonBlockingLockExclusive).unwrap();
    }

    pub(crate) fn binding(&self) -> io::Result<ProviderBinding> {
        if self.poisoned {
            return Err(invalid());
        }
        let named_directory = directory(&self.path)?;
        crate::project::validate_directory(&self.directory, true).map_err(|_| invalid())?;
        let named_file = File::from(openat(
            &self.directory,
            "provider-attempts.jsonl",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        crate::project::validate_stage(&self.file, 0o600).map_err(|_| invalid())?;
        crate::project::validate_stage(&named_file, 0o600).map_err(|_| invalid())?;
        let id = |file: &File| -> io::Result<[u64; 2]> {
            let metadata = file.metadata()?;
            Ok([metadata.dev(), metadata.ino()])
        };
        let binding = ProviderBinding {
            directory: id(&self.directory)?,
            file: id(&self.file)?,
            endpoint: self.endpoint,
        };
        if id(&named_directory)? != binding.directory || id(&named_file)? != binding.file {
            return Err(invalid());
        }
        Ok(binding)
    }

    pub(crate) fn history(&self) -> ProviderHistory {
        self.history.clone()
    }
    pub(crate) fn history_ref(&self) -> &ProviderHistory {
        &self.history
    }
    pub(crate) fn validation_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
    pub(crate) fn eligible(&self) -> bool {
        !self.poisoned
            && self.policy.eligible(&self.history)
            && self
                .cancellation
                .as_ref()
                .is_none_or(|flag| !flag.load(Ordering::SeqCst))
    }
    fn acknowledged_len(&self) -> usize {
        self.acknowledged.iter().map(Vec::len).sum()
    }
    fn verify_content(&self, suffix: &[u8]) -> io::Result<()> {
        self.binding()?;
        let length = self.acknowledged_len() as u64 + suffix.len() as u64;
        if length > HISTORY_BYTES || self.file.metadata()?.len() != length {
            return Err(invalid());
        }
        let mut offset = 0;
        let mut buffer = [0; 4096];
        for bytes in self
            .acknowledged
            .iter()
            .map(Vec::as_slice)
            .chain(std::iter::once(suffix))
        {
            for expected in bytes.chunks(buffer.len()) {
                let actual = &mut buffer[..expected.len()];
                self.file.read_exact_at(actual, offset)?;
                if actual != expected {
                    return Err(invalid());
                }
                offset += expected.len() as u64;
            }
        }
        self.binding()?;
        if self.file.metadata()?.len() != length {
            return Err(invalid());
        }
        Ok(())
    }
    fn append(&mut self, record: &Record) -> io::Result<()> {
        if self.poisoned {
            return Err(invalid());
        }
        if let Err(error) = self.verify_content(&[]) {
            self.poisoned = true;
            return Err(error);
        }
        let mut bytes = bounded_json_bytes(record, RECORD_BYTES)
            .map_err(|_| io::Error::other("provider history record budget exhausted"))?;
        if self.acknowledged_len() as u64 + bytes.len() as u64 + 1 > HISTORY_BYTES {
            return Err(invalid());
        }
        bytes.push(b'\n');
        self.acknowledged
            .try_reserve(1)
            .map_err(|_| io::Error::other("provider history allocation budget exhausted"))?;
        self.poisoned = true;
        #[cfg(test)]
        let fault = self.append_fault.take();
        #[cfg(test)]
        if fault == Some(AppendFault::PartialWrite) {
            self.file
                .write_all_at(&bytes[..bytes.len() / 2], self.acknowledged_len() as u64)?;
            return Err(invalid());
        }
        self.file
            .write_all_at(&bytes, self.acknowledged_len() as u64)?;
        #[cfg(test)]
        if fault == Some(AppendFault::Sync) {
            return Err(invalid());
        }
        self.file.sync_all()?;
        #[cfg(test)]
        match fault {
            Some(AppendFault::Content) => self.file.write_all_at(b"!", 0)?,
            Some(AppendFault::Binding) => std::fs::rename(
                self.path.join("provider-attempts.jsonl"),
                self.path.join("lost-binding.jsonl"),
            )?,
            _ => {}
        }
        self.poisoned = false;
        if let Err(error) = self.verify_content(&bytes) {
            self.poisoned = true;
            return Err(error);
        }
        self.acknowledged.push(bytes);
        Ok(())
    }
    pub(crate) fn reserve(&mut self, request_id: u64) -> io::Result<usize> {
        if !self.eligible() {
            return Err(invalid());
        }
        let ordinal = self.history.attempts.len() + 1;
        self.append(&Record::Reserved {
            ordinal,
            request_id,
        })?;
        self.history.attempts.push(ProviderAttemptFact {
            ordinal,
            request_id,
            terminal: None,
            usage: None,
            locally_joined: false,
        });
        Ok(ordinal)
    }
    pub(crate) fn joined(
        &mut self,
        ordinal: usize,
        terminal: Option<ProviderTerminalEvidence>,
        usage: Option<ProviderUsage>,
    ) -> io::Result<()> {
        self.append(&Record::Joined {
            ordinal,
            terminal: terminal.clone(),
            usage: usage.clone(),
        })?;
        let attempt = &mut self.history.attempts[ordinal - 1];
        attempt.terminal = terminal;
        attempt.usage = usage;
        attempt.locally_joined = true;
        Ok(())
    }
}
pub fn reconstruct_provider_history(
    path: &Path,
    endpoint: SocketAddr,
) -> io::Result<ProviderHistory> {
    let (mut file, _directory) = open(path, false, false)?;
    read(&mut file, endpoint, Policy::Ordinary).map(|(history, _)| history)
}

pub(crate) fn observe_validation(
    path: &Path,
    checkpoint: &ValidationCheckpoint,
) -> io::Result<ProviderHistory> {
    let endpoint = checkpoint.binding.endpoint;
    let (mut file, directory) = open(path, false, false)?;
    let (history, bytes) = read(&mut file, endpoint, Policy::Validation)?;
    if bytes.len() < checkpoint.bytes
        || Sha256::digest(&bytes[..checkpoint.bytes]).as_slice() != checkpoint.sha256
    {
        return Err(invalid());
    }
    let ledger = ProviderLedger {
        file,
        directory,
        path: path.into(),
        endpoint,
        history,
        acknowledged: vec![bytes],
        policy: Policy::Validation,
        checkpoint: Some(checkpoint.clone()),
        cancellation: None,
        poisoned: false,
        #[cfg(test)]
        append_fault: None,
    };
    ledger.validation_checkpoint()?;
    Ok(ledger.history)
}

#[cfg(test)]
#[path = "provider_history_tests.rs"]
mod provider_history_tests;

pub(crate) async fn reserve(ledger: Arc<Mutex<ProviderLedger>>, id: u64) -> Result<usize, Outcome> {
    tokio::task::spawn_blocking(move || ledger.lock().unwrap().reserve(id))
        .await
        .map_err(|_| Outcome::Transport)?
        .map_err(|_| Outcome::ResourceLimit)
}
pub(crate) async fn joined(
    ledger: Arc<Mutex<ProviderLedger>>,
    ordinal: usize,
    terminal: Option<ProviderTerminalEvidence>,
    usage: Option<ProviderUsage>,
) -> bool {
    tokio::task::spawn_blocking(move || ledger.lock().unwrap().joined(ordinal, terminal, usage))
        .await
        .is_ok_and(|r| r.is_ok())
}
