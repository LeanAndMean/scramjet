use super::{CodingError, EditIntent, OperationFact, RootIdentity, open_root, validate_name};
use std::path::Path;

pub const CODING_HISTORY_PAYLOAD_BYTES: usize = 524_288;

#[derive(Clone, Debug, serde::Serialize)]
pub struct CodingMaterial {
    pub turn: u64,
    pub request: u64,
    pub operation: u64,
    pub event: String,
    pub status: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub enum CodingHistoryEntry {
    Material(CodingMaterial),
    Authorized(EditIntent),
    Dispatched {
        operation: u64,
    },
    Completed {
        operation: u64,
    },
    Cancelled {
        operation: u64,
    },
    FollowUp {
        invocation: u64,
        after_turn: u64,
        prompt: String,
    },
}
impl CodingHistoryEntry {
    pub(super) fn payload_bytes(&self) -> usize {
        match self {
            Self::FollowUp { prompt, .. } => prompt.len(),
            Self::Material(m) => m.event.len() + m.status.len() + m.text.len(),
            Self::Authorized(i) => {
                i.session.len() + i.target.len() + i.expected.len() + i.replacement.len()
            }
            _ => 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CodingHistoryRecord {
    pub sequence: usize,
    pub entry: CodingHistoryEntry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodingHistoryEnding {
    RecordedFinal,
    RecordedResponseFailed,
    RecordedToolError,
    Incomplete,
}

#[derive(Debug)]
pub struct CodingHistory {
    pub(super) session: String,
    pub(super) root: RootIdentity,
    pub(super) records: Vec<CodingHistoryRecord>,
    pub(super) operations: Vec<OperationFact>,
    pub(super) ending: CodingHistoryEnding,
    pub(super) payload_bytes: usize,
}
impl CodingHistory {
    pub fn session(&self) -> &str {
        &self.session
    }
    pub fn root(&self) -> RootIdentity {
        self.root
    }
    pub fn version(&self) -> u8 {
        1
    }
    pub fn records(&self) -> &[CodingHistoryRecord] {
        &self.records
    }
    pub fn operations(&self) -> &[OperationFact] {
        &self.operations
    }
    pub fn ending(&self) -> CodingHistoryEnding {
        self.ending
    }
    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }
}

pub fn reconstruct_coding_history(
    path: &Path,
    session: &str,
) -> Result<CodingHistory, CodingError> {
    validate_name(session)?;
    let (root, identity) = open_root(path)?;
    from_storage(&root, session, identity, false)
}

pub(crate) fn from_storage(
    storage: &std::fs::File,
    session: &str,
    identity: RootIdentity,
    selected_storage: bool,
) -> Result<CodingHistory, CodingError> {
    super::validate_session(session)?;
    let (operations, records) =
        super::journal::scan(storage, session, identity, true, selected_storage)?;
    let records = records.unwrap();
    let ending = records
        .iter()
        .rev()
        .find_map(|r| match &r.entry {
            CodingHistoryEntry::FollowUp { .. } => Some(CodingHistoryEnding::Incomplete),
            CodingHistoryEntry::Material(m) if m.event == "Request" => {
                Some(CodingHistoryEnding::Incomplete)
            }
            CodingHistoryEntry::Material(m) if m.event == "Final" && m.status == "Final" => {
                Some(CodingHistoryEnding::RecordedFinal)
            }
            CodingHistoryEntry::Material(m) if m.event == "Response" && m.status == "Failed" => {
                Some(CodingHistoryEnding::RecordedResponseFailed)
            }
            CodingHistoryEntry::Material(m) if m.event == "Tool" && m.status == "Error" => {
                Some(CodingHistoryEnding::RecordedToolError)
            }
            _ => None,
        })
        .unwrap_or(CodingHistoryEnding::Incomplete);
    let payload_bytes = session.len()
        + records
            .iter()
            .map(|r| r.entry.payload_bytes())
            .sum::<usize>()
        + operations.iter().map(|f| f.target.len()).sum::<usize>();
    Ok(CodingHistory {
        session: session.into(),
        root: identity,
        records,
        operations,
        ending,
        payload_bytes,
    })
}
