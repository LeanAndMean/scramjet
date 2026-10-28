use super::*;
use crate::coding::journal::{BoundedString, object_decoder};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, Visitor},
};
use std::{
    fmt,
    io::{Read, Write},
};

pub const PROJECT_JOURNAL_RECORDS: usize = 32;
pub const PROJECT_JOURNAL_RECORD_BYTES: usize = 16384;
pub const PROJECT_JOURNAL_BYTES: usize = 262144;
const NAME: &str = ".project-journal";

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;

#[derive(Clone, Debug, Serialize)]
pub struct ProjectHeader {
    pub version: u8,
    pub session: String,
    pub instance: u64,
    pub project_path: String,
    pub project: RootIdentity,
    pub storage: RootIdentity,
}
object_decoder!(ProjectHeader {
    version: u8 => |v| v,
    session: BoundedString<64> => |v: BoundedString<64>| v.0,
    instance: u64 => |v| v,
    project_path: BoundedString<4096> => |v: BoundedString<4096>| v.0,
    project: RootIdentity => |v| v,
    storage: RootIdentity => |v| v,
});
object_decoder!(ProjectFile {
    name: BoundedString<64> => |v: BoundedString<64>| v.0,
    identity: RootIdentity => |v| v,
    mode: u32 => |v| v,
    content: BoundedString<1024> => |v: BoundedString<1024>| v.0,
});
#[derive(Clone, Debug, Serialize)]
pub struct ProjectRequested {
    pub journey: u8,
    pub request: ProjectRequest,
}
object_decoder!(ProjectRequested { journey: u8 => |v| v, request: ProjectRequest => |v| v });

#[derive(Clone, Debug, Serialize)]
pub struct ProjectCaptured {
    pub journey: u8,
    pub request: ProjectRequest,
    pub files: [ProjectFile; 3],
    pub desired: String,
    pub reader_session: String,
    pub coding_session: String,
    pub reader_storage: RootIdentity,
}
object_decoder!(ProjectCaptured {
    journey: u8 => |v| v,
    request: ProjectRequest => |v| v,
    files: [ProjectFile; 3] => |v| v,
    desired: BoundedString<1024> => |v: BoundedString<1024>| v.0,
    reader_session: BoundedString<64> => |v: BoundedString<64>| v.0,
    coding_session: BoundedString<64> => |v: BoundedString<64>| v.0,
    reader_storage: RootIdentity => |v| v,
});
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProjectReaderFact {
    pub child: u8,
    pub request: Option<u64>,
    pub state: String,
    pub summary: Option<String>,
}
object_decoder!(ProjectReaderFact {
    child: u8 => |v| v,
    request: Option<u64> => |v| v,
    state: BoundedString<32> => |v: BoundedString<32>| v.0,
    summary: Option<BoundedString<512>> => |v: Option<BoundedString<512>>| v.map(|s| s.0),
});
#[derive(Clone, Debug, Serialize)]
pub struct ProjectReaders {
    pub journey: u8,
    pub ended: bool,
    pub joined: bool,
    pub facts: [ProjectReaderFact; 3],
    pub error: Option<String>,
}
object_decoder!(ProjectReaders {
    journey: u8 => |v| v,
    ended: bool => |v| v,
    joined: bool => |v| v,
    facts: [ProjectReaderFact; 3] => |v| v,
    error: Option<BoundedString<512>> => |v: Option<BoundedString<512>>| v.map(|s| s.0),
});
#[derive(Clone, Debug, Serialize)]
pub struct ProjectCoding {
    pub journey: u8,
    pub owner: u64,
    pub joined: bool,
    pub completion: String,
    pub effect: String,
    pub requests: [Option<u64>; 4],
    pub summary: Option<String>,
    pub error: Option<String>,
    pub storage: RootIdentity,
    pub edited: Option<ProjectFile>,
}
object_decoder!(ProjectCoding {
    journey: u8 => |v| v,
    owner: u64 => |v| v,
    joined: bool => |v| v,
    completion: BoundedString<16> => |v: BoundedString<16>| v.0,
    effect: BoundedString<16> => |v: BoundedString<16>| v.0,
    requests: [Option<u64>; 4] => |v| v,
    summary: Option<BoundedString<512>> => |v: Option<BoundedString<512>>| v.map(|s| s.0),
    error: Option<BoundedString<512>> => |v: Option<BoundedString<512>>| v.map(|s| s.0),
    storage: RootIdentity => |v| v,
    edited: Option<ProjectFile> => |v| v,
});
#[derive(Clone, Debug, Serialize)]
pub struct ProjectReport {
    pub journey: u8,
    pub completion: String,
    pub summary: String,
    pub paused: bool,
}
object_decoder!(ProjectReport {
    journey: u8 => |v| v,
    completion: BoundedString<16> => |v: BoundedString<16>| v.0,
    summary: BoundedString<512> => |v: BoundedString<512>| v.0,
    paused: bool => |v| v,
});
#[derive(Clone, Debug, Serialize)]
pub struct ProjectReadyRecord {
    pub journey: u8,
}
object_decoder!(ProjectReadyRecord { journey: u8 => |v| v });
#[derive(Clone, Debug, Serialize)]
pub struct ProjectRead {
    pub journey: u8,
    pub file: Option<ProjectFile>,
    pub matched: bool,
    pub error: Option<String>,
}
object_decoder!(ProjectRead {
    journey: u8 => |v| v,
    file: Option<ProjectFile> => |v| v,
    matched: bool => |v| v,
    error: Option<BoundedString<512>> => |v: Option<BoundedString<512>>| v.map(|s| s.0),
});
#[derive(Clone, Debug, Serialize)]
pub struct ProjectClosure {
    pub cancelled: bool,
    pub logical_settled: bool,
    pub readers_joined: bool,
    pub coding_joined: bool,
    pub runtime_joined: bool,
    pub attempts: usize,
    pub unknown_usage: usize,
    pub unresolved: usize,
}
object_decoder!(ProjectClosure {
    cancelled: bool => |v| v,
    logical_settled: bool => |v| v,
    readers_joined: bool => |v| v,
    coding_joined: bool => |v| v,
    runtime_joined: bool => |v| v,
    attempts: usize => |v| v,
    unknown_usage: usize => |v| v,
    unresolved: usize => |v| v,
});
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ProjectRecord {
    Header(ProjectHeader),
    Requested(ProjectRequested),
    Captured(ProjectCaptured),
    Readers(ProjectReaders),
    Coding(ProjectCoding),
    SourceReport(ProjectReport),
    VerificationReady(ProjectReadyRecord),
    VerificationRead(ProjectRead),
    VerificationReport(ProjectReport),
    Closure(ProjectClosure),
}
#[derive(Serialize)]
struct Envelope {
    sequence: usize,
    record: ProjectRecord,
}
object_decoder!(Envelope { sequence: usize => |v| v, record: ProjectRecord => |v| v });
struct Buffer(Vec<u8>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > PROJECT_JOURNAL_RECORD_BYTES - self.0.len() {
            return Err(std::io::Error::other("project record ceiling"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) struct Journal {
    file: File,
    records: usize,
    bytes: usize,
    reservation: Option<(usize, usize)>,
    poisoned: bool,
}
impl Journal {
    pub fn create(
        workspace: &ProjectWorkspace,
        session: &str,
        instance: u64,
    ) -> Result<Self, CodingError> {
        crate::coding::validate_session(session)?;
        let project_path = workspace
            .0
            .path
            .to_str()
            .ok_or(CodingError::Invalid("project session requires UTF-8 path"))?
            .to_owned();
        let file = File::from(openat(
            &workspace.0.storage,
            NAME,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?);
        validate_stage(&file, 0o600)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        let mut journal = Self {
            file,
            records: 0,
            bytes: 0,
            reservation: None,
            poisoned: false,
        };
        journal.append(ProjectRecord::Header(ProjectHeader {
            version: 1,
            session: session.into(),
            instance,
            project_path,
            project: workspace.root_identity(),
            storage: identity(&workspace.0.storage)?,
        }))?;
        workspace.0.storage.sync_all()?;
        Ok(journal)
    }
    pub fn reserve_journey(&mut self) -> Result<(), CodingError> {
        if self.poisoned
            || self.reservation.is_some()
            || self.records + 15 > PROJECT_JOURNAL_RECORDS
            || self.bytes + 98304 + PROJECT_JOURNAL_RECORD_BYTES > PROJECT_JOURNAL_BYTES
        {
            return Err(CodingError::History(
                "project journey history headroom exhausted",
            ));
        }
        self.reservation = Some((14, 98304));
        Ok(())
    }
    pub fn release_journey(&mut self) {
        self.reservation = None;
    }
    pub fn poison(&mut self) {
        self.poisoned = true;
    }
    pub fn append(&mut self, record: ProjectRecord) -> Result<Arc<ProjectRecord>, CodingError> {
        if self.poisoned {
            return Err(CodingError::Poisoned);
        }
        let mut buffer = Buffer(Vec::with_capacity(PROJECT_JOURNAL_RECORD_BYTES));
        serde_json::to_writer(
            &mut buffer,
            &Envelope {
                sequence: self.records,
                record: record.clone(),
            },
        )
        .map_err(|_| CodingError::History("project record serialization ceiling"))?;
        buffer.write_all(b"\n")?;
        if self.records >= PROJECT_JOURNAL_RECORDS
            || buffer.0.len() > PROJECT_JOURNAL_BYTES - self.bytes
            || self
                .reservation
                .is_some_and(|(records, bytes)| records == 0 || buffer.0.len() > bytes)
        {
            return Err(CodingError::History("project journal ceiling"));
        }
        if let Err(error) = self
            .file
            .write_all(&buffer.0)
            .and_then(|()| self.file.sync_all())
        {
            self.poisoned = true;
            return Err(error.into());
        }
        self.records += 1;
        self.bytes += buffer.0.len();
        if let Some((records, bytes)) = &mut self.reservation {
            *records -= 1;
            *bytes -= buffer.0.len();
        }
        Ok(Arc::new(record))
    }
}

pub(crate) fn read(storage: &File) -> Result<Vec<Arc<ProjectRecord>>, CodingError> {
    validate_directory(storage, true)?;
    let mut file = File::from(openat(
        storage,
        NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    validate_stage(&file, 0o600)?;
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    let size = fstat(&file)?.st_size;
    if size < 0 || size as usize > PROJECT_JOURNAL_BYTES {
        return Err(CodingError::History("project journal byte ceiling"));
    }
    let mut records = Vec::with_capacity(PROJECT_JOURNAL_RECORDS);
    let mut buffer = Vec::with_capacity(PROJECT_JOURNAL_RECORD_BYTES);
    let mut chunk = [0u8; 1024];
    let mut total = 0;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        total += n;
        if total > PROJECT_JOURNAL_BYTES {
            return Err(CodingError::History("project journal byte ceiling"));
        }
        for byte in &chunk[..n] {
            if buffer.len() == PROJECT_JOURNAL_RECORD_BYTES {
                return Err(CodingError::History("project record byte ceiling"));
            }
            buffer.push(*byte);
            if *byte == b'\n' {
                if records.len() >= PROJECT_JOURNAL_RECORDS {
                    return Err(CodingError::History("project record count ceiling"));
                }
                let envelope: Envelope = serde_json::from_slice(&buffer[..buffer.len() - 1])
                    .map_err(|_| CodingError::History("invalid strict project record"))?;
                if envelope.sequence != records.len() {
                    return Err(CodingError::History("project sequence mismatch"));
                }
                records.push(Arc::new(envelope.record));
                buffer.clear();
            }
        }
    }
    if records.is_empty() || !buffer.is_empty() {
        return Err(CodingError::History("empty/truncated project journal"));
    }
    Ok(records)
}
