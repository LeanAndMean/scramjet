use crate::http::responses::{
    PROVIDER_FRAME_BYTES, PROVIDER_MODEL, bounded_json_bytes, bounded_json_value, identity,
};
use crate::{Outcome, ProviderUsage};
use rustix::fs::{CWD, FlockOperation, Mode, OFlags, flock, openat};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read, Write},
    net::SocketAddr,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
    sync::{Arc, Mutex},
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

pub(crate) struct ProviderLedger {
    file: File,
    directory: File,
    history: ProviderHistory,
    poisoned: bool,
}
fn invalid() -> io::Error {
    io::Error::other("invalid or unacknowledged provider history")
}
fn directory(path: &Path) -> io::Result<File> {
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
fn open(path: &Path, writable: bool) -> io::Result<(File, File)> {
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
        Err(rustix::io::Errno::NOENT) if writable => File::from(openat(
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
fn read(file: &mut File, endpoint: SocketAddr) -> io::Result<ProviderHistory> {
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
                && ordinal <= PROVIDER_POST_BUDGET
                && history.unresolved_remote() < 4
                && history.unknown_usage() < 4
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
    Ok(history)
}
impl ProviderLedger {
    pub(crate) fn open(path: &Path, endpoint: SocketAddr) -> io::Result<Arc<Mutex<Self>>> {
        if !endpoint.ip().is_loopback() {
            return Err(invalid());
        }
        let (mut file, directory) = open(path, true)?;
        let empty = file.metadata()?.len() == 0;
        let history = if empty {
            ProviderHistory::default()
        } else {
            read(&mut file, endpoint)?
        };
        let mut ledger = Self {
            file,
            directory,
            history,
            poisoned: false,
        };
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

    pub(crate) fn history(&self) -> ProviderHistory {
        self.history.clone()
    }
    pub(crate) fn eligible(&self) -> bool {
        !self.poisoned
            && self.history.remaining() > 0
            && self.history.unresolved_remote() < 4
            && self.history.unknown_usage() < 4
    }
    fn append(&mut self, record: &Record) -> io::Result<()> {
        if self.poisoned {
            return Err(invalid());
        }
        let mut bytes = bounded_json_bytes(record, RECORD_BYTES)
            .map_err(|_| io::Error::other("provider history record budget exhausted"))?;
        if self.file.metadata()?.len() + bytes.len() as u64 + 1 > HISTORY_BYTES {
            return Err(invalid());
        }
        bytes.push(b'\n');
        self.poisoned = true;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.poisoned = false;
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
    let (mut file, _directory) = open(path, false)?;
    read(&mut file, endpoint)
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
