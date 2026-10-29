use super::{HaikuCapture, HaikuIntent, HaikuProcessResult};
use crate::{CodingError, ProviderUsage};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, mkdirat, openat};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) const RECORD_BYTES: usize = 131_072;
const HISTORY_BYTES: usize = 2_097_152;
const RECORDS: usize = 128;
const TURN_RECORDS: usize = 9;

#[derive(Serialize)]
#[serde(tag = "kind")]
pub(super) enum Record {
    Header {
        version: u8,
        session: String,
        root: [u64; 2],
    },
    Attempt {
        turn: usize,
        request: String,
    },
    Selected {
        turn: usize,
        request_id: u64,
        ordinal: usize,
        response_id: String,
        items: Vec<Value>,
        usage: ProviderUsage,
    },
    Denied {
        intent: HaikuIntent,
    },
    Authorized {
        intent: HaikuIntent,
    },
    NotDispatched {
        operation: usize,
    },
    Dispatched {
        operation: usize,
    },
    Created {
        operation: usize,
        capture: HaikuCapture,
    },
    Process {
        operation: usize,
        result: HaikuProcessResult,
    },
    Result {
        call_id: String,
        output: String,
    },
    Finished,
    Stopped {
        turn: usize,
        reason: String,
    },
}

pub(super) struct Journal {
    storage: File,
    storage_path: PathBuf,
    directory: File,
    session: String,
    file: File,
    bytes: usize,
    records: usize,
    remaining: usize,
    poisoned: bool,
}

impl Journal {
    pub(super) fn create(
        storage: &File,
        storage_path: &Path,
        session: &str,
        root: [u64; 2],
    ) -> Result<Self, CodingError> {
        mkdirat(storage, session, Mode::from_bits_truncate(0o700))?;
        let directory = File::from(openat(
            storage,
            session,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        crate::project::validate_directory(&directory, true)?;
        let file = File::from(openat(
            &directory,
            "haiku.jsonl",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        crate::project::validate_stage(&file, 0o600)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        let mut journal = Self {
            storage: storage.try_clone()?,
            storage_path: storage_path.into(),
            directory,
            session: session.into(),
            file,
            bytes: 0,
            records: 0,
            remaining: 1,
            poisoned: false,
        };
        journal.append(Record::Header {
            version: 1,
            session: session.into(),
            root,
        })?;
        journal.directory.sync_all()?;
        storage.sync_all()?;
        journal.validate()?;
        Ok(journal)
    }
    pub(super) fn validate(&self) -> Result<(), CodingError> {
        if self.poisoned {
            return Err(CodingError::Poisoned);
        }
        crate::project::validate_directory(&self.storage, true)?;
        crate::project::validate_directory(&self.directory, true)?;
        crate::project::validate_stage(&self.file, 0o600)?;
        let storage = crate::project::open_directory(&self.storage_path)?;
        let directory = File::from(openat(
            &self.storage,
            self.session.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let file = File::from(openat(
            &self.directory,
            "haiku.jsonl",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        crate::project::validate_stage(&file, 0o600)?;
        if super::identity(&storage)? != super::identity(&self.storage)?
            || super::identity(&directory)? != super::identity(&self.directory)?
            || super::identity(&file)? != super::identity(&self.file)?
        {
            return Err(CodingError::Stale);
        }
        Ok(())
    }
    pub(super) fn check(&mut self) -> Result<(), CodingError> {
        let result = self.validate();
        self.poisoned |= result.is_err();
        result
    }
    pub(super) fn reserve_turn(&mut self) -> Result<(), CodingError> {
        self.check()?;
        if self.records + TURN_RECORDS > RECORDS
            || self.bytes + TURN_RECORDS * RECORD_BYTES > HISTORY_BYTES
        {
            return Err(CodingError::History(
                "haiku attempt/effect/ending history resource limit",
            ));
        }
        self.remaining = TURN_RECORDS;
        Ok(())
    }
    pub(super) fn append(&mut self, record: Record) -> Result<(), CodingError> {
        self.check()?;
        if self.remaining == 0 {
            return Err(CodingError::History("unreserved haiku record"));
        }
        let mut bytes = super::bounded_json_bytes(
            &serde_json::json!({"sequence":self.records,"record":record}),
            RECORD_BYTES - 1,
        )
        .map_err(|_| CodingError::History("haiku record resource limit"))?;
        bytes.push(b'\n');
        if self.records >= RECORDS || bytes.len() > HISTORY_BYTES - self.bytes {
            return Err(CodingError::History("haiku history resource limit"));
        }
        if let Err(error) = self
            .file
            .write_all(&bytes)
            .and_then(|()| self.file.sync_all())
        {
            self.poisoned = true;
            return Err(error.into());
        }
        self.check()?;
        self.records += 1;
        self.remaining -= 1;
        self.bytes += bytes.len();
        Ok(())
    }
}
