use super::agent::Material;
use super::{
    CODING_HISTORY_PAYLOAD_BYTES, CodingError, CodingHistoryEntry, CodingHistoryRecord, EditIntent,
    OperationFact, OperationState, RootIdentity, validate_intent,
};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, openat};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};

pub(super) const JOURNAL_NAME: &str = ".coding-journal";
pub(super) const RECORD_BYTES: usize = 16_384;
pub(super) const JOURNAL_BYTES: usize = 1_048_576;
pub(super) const RECORDS: usize = 64;
pub(super) const OPERATIONS: usize = 8;

#[derive(Serialize)]
struct Envelope {
    sequence: usize,
    record: Record,
}

#[derive(Serialize)]
#[serde(tag = "kind")]
pub(super) enum Record {
    Header {
        version: u8,
        session: String,
        root: RootIdentity,
    },
    Authorized {
        intent: EditIntent,
    },
    Dispatched {
        operation: u64,
    },
    Completed {
        operation: u64,
    },
    Cancelled {
        operation: u64,
    },
    Material {
        entry: super::agent::Material,
    },
    FollowUp {
        invocation: u64,
        after_turn: u64,
        prompt: String,
    },
}

pub(crate) struct BoundedString<const LIMIT: usize>(pub(crate) String);

impl<'de, const LIMIT: usize> Deserialize<'de> for BoundedString<LIMIT> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text<const LIMIT: usize>;
        impl<const LIMIT: usize> Visitor<'_> for Text<LIMIT> {
            type Value = BoundedString<LIMIT>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bounded string")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                if value.len() > LIMIT {
                    return Err(E::custom("string byte ceiling"));
                }
                Ok(BoundedString(value.to_owned()))
            }
        }
        deserializer.deserialize_str(Text::<LIMIT>)
    }
}

macro_rules! object_decoder {
    ($type:ident { $($field:ident : $wire:ty => $convert:expr),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Object;
                impl<'de> Visitor<'de> for Object {
                    type Value = $type;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("a journal schema object")
                    }
                    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<$type, M::Error> {
                        $(let mut $field = None;)+
                        while let Some(key) = map.next_key::<BoundedString<16>>()? {
                            match key.0.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(de::Error::custom("duplicate journal field"));
                                    }
                                    let value: $wire = map.next_value()?;
                                    $field = Some(($convert)(value));
                                })+
                                _ => return Err(de::Error::custom("unknown journal field")),
                            }
                        }
                        Ok($type {
                            $($field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,)+
                        })
                    }
                }
                deserializer.deserialize_map(Object)
            }
        }
    };
}

pub(crate) use object_decoder;

object_decoder!(Envelope { sequence: usize => |v| v, record: Record => |v| v });
object_decoder!(Material {
    turn: u64 => |v| v,
    request: u64 => |v| v,
    operation: u64 => |v| v,
    event: BoundedString<16> => |v: BoundedString<16>| v.0,
    status: BoundedString<32> => |v: BoundedString<32>| v.0,
    text: BoundedString<4_096> => |v: BoundedString<4_096>| v.0,
});
object_decoder!(RootIdentity { device: u64 => |v| v, inode: u64 => |v| v });
object_decoder!(EditIntent {
    session: BoundedString<64> => |v: BoundedString<64>| v.0,
    root: RootIdentity => |v| v,
    operation: u64 => |v| v,
    target: BoundedString<64> => |v: BoundedString<64>| v.0,
    expected: BoundedString<1_024> => |v: BoundedString<1_024>| v.0,
    replacement: BoundedString<1_024> => |v: BoundedString<1_024>| v.0,
});

impl<'de> Deserialize<'de> for Record {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Record;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a tagged journal object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Record, M::Error> {
                let mut kind = None;
                let mut version = None;
                let mut session = None;
                let mut root = None;
                let mut intent = None;
                let mut operation = None;
                let mut entry = None;
                let mut invocation = None;
                let mut after_turn = None;
                let mut prompt = None;
                while let Some(key) = map.next_key::<BoundedString<16>>()? {
                    macro_rules! field {
                        ($field:ident, $wire:ty) => {{
                            if $field.is_some() {
                                return Err(de::Error::custom("duplicate journal field"));
                            }
                            $field = Some(map.next_value::<$wire>()?);
                        }};
                    }
                    match key.0.as_str() {
                        "kind" => field!(kind, BoundedString<16>),
                        "version" => field!(version, u8),
                        "session" => field!(session, BoundedString<64>),
                        "root" => field!(root, RootIdentity),
                        "intent" => field!(intent, EditIntent),
                        "operation" => field!(operation, u64),
                        "entry" => field!(entry, super::agent::Material),
                        "invocation" => field!(invocation, u64),
                        "after_turn" => field!(after_turn, u64),
                        "prompt" => field!(prompt, BoundedString<1024>),
                        _ => return Err(de::Error::custom("unknown journal field")),
                    }
                }
                let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
                if kind.0 != "FollowUp"
                    && (invocation.is_some() || after_turn.is_some() || prompt.is_some())
                {
                    return Err(de::Error::custom("inapplicable follow-up field"));
                }
                match kind.0.as_str() {
                    "FollowUp"
                        if version.is_none()
                            && session.is_none()
                            && root.is_none()
                            && intent.is_none()
                            && operation.is_none()
                            && entry.is_none() =>
                    {
                        Ok(Record::FollowUp {
                            invocation: invocation
                                .ok_or_else(|| de::Error::missing_field("invocation"))?,
                            after_turn: after_turn
                                .ok_or_else(|| de::Error::missing_field("after_turn"))?,
                            prompt: prompt.ok_or_else(|| de::Error::missing_field("prompt"))?.0,
                        })
                    }
                    "Header" if intent.is_none() && operation.is_none() && entry.is_none() => {
                        Ok(Record::Header {
                            version: version.ok_or_else(|| de::Error::missing_field("version"))?,
                            session: session
                                .ok_or_else(|| de::Error::missing_field("session"))?
                                .0,
                            root: root.ok_or_else(|| de::Error::missing_field("root"))?,
                        })
                    }
                    "Authorized"
                        if version.is_none()
                            && session.is_none()
                            && root.is_none()
                            && operation.is_none()
                            && entry.is_none() =>
                    {
                        Ok(Record::Authorized {
                            intent: intent.ok_or_else(|| de::Error::missing_field("intent"))?,
                        })
                    }
                    "Dispatched" | "Completed" | "Cancelled"
                        if version.is_none()
                            && session.is_none()
                            && root.is_none()
                            && intent.is_none()
                            && entry.is_none() =>
                    {
                        let operation =
                            operation.ok_or_else(|| de::Error::missing_field("operation"))?;
                        Ok(match kind.0.as_str() {
                            "Dispatched" => Record::Dispatched { operation },
                            "Completed" => Record::Completed { operation },
                            _ => Record::Cancelled { operation },
                        })
                    }
                    "Material"
                        if version.is_none()
                            && session.is_none()
                            && root.is_none()
                            && intent.is_none()
                            && operation.is_none() =>
                    {
                        Ok(Record::Material {
                            entry: entry.ok_or_else(|| de::Error::missing_field("entry"))?,
                        })
                    }
                    _ => Err(de::Error::custom("invalid journal kind or fields")),
                }
            }
        }
        deserializer.deserialize_map(Object)
    }
}

struct RecordBuffer(Vec<u8>);

impl Write for RecordBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > RECORD_BYTES - self.0.len() {
            return Err(std::io::Error::other("journal record byte ceiling"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode(sequence: usize, record: Record) -> Result<Vec<u8>, CodingError> {
    let mut buffer = RecordBuffer(Vec::with_capacity(RECORD_BYTES));
    serde_json::to_writer(&mut buffer, &Envelope { sequence, record })
        .map_err(|_| CodingError::History("record encoding exceeds ceiling"))?;
    buffer.write_all(b"\n")?;
    Ok(buffer.0)
}

pub(super) struct Journal {
    file: File,
    records: usize,
    bytes: usize,
    reserved: usize,
    pub(super) poisoned: bool,
    #[cfg(test)]
    pub(super) fault: Option<JournalFault>,
    #[cfg(test)]
    pub(super) limits: (usize, usize),
}

#[cfg(test)]
#[derive(Clone, Copy)]
pub(crate) enum JournalFault {
    ShortWrite,
    Append,
    Sync,
}

impl Journal {
    pub(super) fn create(
        root: &File,
        session: &str,
        identity: RootIdentity,
    ) -> Result<Self, CodingError> {
        let fd = openat(
            root,
            JOURNAL_NAME,
            OFlags::CREATE
                | OFlags::EXCL
                | OFlags::WRONLY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let file = File::from(fd);
        flock(&file, FlockOperation::NonBlockingLockExclusive)?;
        let mut journal = Self {
            file,
            records: 0,
            bytes: 0,
            reserved: 0,
            poisoned: false,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            limits: (RECORDS, JOURNAL_BYTES),
        };
        journal.append(Record::Header {
            version: 1,
            session: session.to_owned(),
            root: identity,
        })?;
        if let Err(error) = root.sync_all() {
            journal.poisoned = true;
            return Err(error.into());
        }
        Ok(journal)
    }

    pub(super) fn ensure_turn_capacity(&self) -> Result<(), CodingError> {
        if self.poisoned {
            return Err(CodingError::Poisoned);
        }
        if self.reserved != 0
            || self.records + 16 > RECORDS
            || self.bytes + 16 * RECORD_BYTES > JOURNAL_BYTES
        {
            return Err(CodingError::History(
                "insufficient model/tool/effect history capacity",
            ));
        }
        Ok(())
    }

    pub(super) fn ensure_follow_up_capacity(&self) -> Result<(), CodingError> {
        self.ensure_turn_capacity()?;
        if self.records + 17 > RECORDS || self.bytes + 17 * RECORD_BYTES > JOURNAL_BYTES {
            return Err(CodingError::History(
                "insufficient follow-up transition capacity",
            ));
        }
        Ok(())
    }

    pub(super) fn reserve_operation(&mut self) -> Result<(), CodingError> {
        if self.poisoned {
            return Err(CodingError::Poisoned);
        }
        let limits = (RECORDS, JOURNAL_BYTES);
        #[cfg(test)]
        let limits = {
            let _ = limits;
            self.limits
        };
        if self.reserved != 0
            || self.records + 3 > limits.0
            || self.bytes + 3 * RECORD_BYTES > limits.1
        {
            return Err(CodingError::History(
                "insufficient authorization/dispatch/terminal capacity",
            ));
        }
        self.reserved = 3;
        Ok(())
    }

    pub(super) fn append(&mut self, record: Record) -> Result<(), CodingError> {
        if self.poisoned {
            return Err(CodingError::Poisoned);
        }
        let bytes = encode(self.records, record)?;
        if self.records >= RECORDS || bytes.len() > JOURNAL_BYTES - self.bytes {
            return Err(CodingError::History("journal ceiling"));
        }
        let result = (|| -> Result<(), CodingError> {
            #[cfg(test)]
            match self.fault.take() {
                Some(JournalFault::ShortWrite) => {
                    self.file.write_all(&bytes[..bytes.len() / 2])?;
                    return Err(std::io::Error::other("injected partial append").into());
                }
                Some(JournalFault::Append) => {
                    return Err(std::io::Error::other("injected append failure").into());
                }
                Some(JournalFault::Sync) => {
                    self.file.write_all(&bytes)?;
                    return Err(std::io::Error::other("injected sync failure").into());
                }
                None => {}
            }
            self.file.write_all(&bytes)?;
            self.file.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
            return result;
        }
        self.records += 1;
        self.bytes += bytes.len();
        self.reserved = self.reserved.saturating_sub(1);
        Ok(())
    }

    pub(super) fn release_reservation(&mut self) {
        self.reserved = 0;
    }
}

pub(super) fn reconstruct(
    root: &File,
    session: &str,
    identity: RootIdentity,
) -> Result<Vec<OperationFact>, CodingError> {
    scan(root, session, identity, false, false).map(|(facts, _)| facts)
}

type ScanResult = (Vec<OperationFact>, Option<Vec<CodingHistoryRecord>>);
pub(super) fn scan(
    root: &File,
    session: &str,
    identity: RootIdentity,
    collect: bool,
    selected_storage: bool,
) -> Result<ScanResult, CodingError> {
    let fd = openat(
        root,
        JOURNAL_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut file = File::from(fd);
    super::validate_regular(&file)?;
    if selected_storage {
        crate::project::validate_stage(&file, 0o600)?;
    }
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    let stat = rustix::fs::fstat(&file)?;
    if stat.st_size < 0 || stat.st_size as u64 > JOURNAL_BYTES as u64 {
        return Err(CodingError::History("journal byte ceiling"));
    }
    let mut buffer = Vec::with_capacity(RECORD_BYTES);
    let mut chunk = [0; 1_024];
    let mut total = 0;
    let mut sequence = 0;
    let mut facts = Vec::with_capacity(OPERATIONS);
    let mut current: Option<EditIntent> = None;
    let mut material = super::agent::History::default();
    let mut records = collect.then(|| Vec::with_capacity(RECORDS - 1));
    let mut payload_bytes = session.len();
    loop {
        let count = file.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        total += count;
        if total > JOURNAL_BYTES {
            return Err(CodingError::History("journal byte ceiling"));
        }
        for &byte in &chunk[..count] {
            if buffer.len() == RECORD_BYTES {
                return Err(CodingError::History("record byte ceiling"));
            }
            buffer.push(byte);
            if byte != b'\n' {
                continue;
            }
            if sequence >= RECORDS {
                return Err(CodingError::History("record count ceiling"));
            }
            let envelope: Envelope = serde_json::from_slice(&buffer[..buffer.len() - 1])
                .map_err(|_| CodingError::History("invalid strict journal record"))?;
            if envelope.sequence != sequence {
                return Err(CodingError::History("record sequence mismatch"));
            }
            if let Some(records) = &mut records {
                let entry = match &envelope.record {
                    Record::Header { .. } => None,
                    Record::Authorized { intent } => {
                        Some(CodingHistoryEntry::Authorized(intent.clone()))
                    }
                    Record::Dispatched { operation } => Some(CodingHistoryEntry::Dispatched {
                        operation: *operation,
                    }),
                    Record::Completed { operation } => Some(CodingHistoryEntry::Completed {
                        operation: *operation,
                    }),
                    Record::Cancelled { operation } => Some(CodingHistoryEntry::Cancelled {
                        operation: *operation,
                    }),
                    Record::Material { entry } => Some(CodingHistoryEntry::Material(entry.clone())),
                    Record::FollowUp {
                        invocation,
                        after_turn,
                        prompt,
                    } => Some(CodingHistoryEntry::FollowUp {
                        invocation: *invocation,
                        after_turn: *after_turn,
                        prompt: prompt.clone(),
                    }),
                };
                if let Some(entry) = entry {
                    payload_bytes += entry.payload_bytes();
                    if payload_bytes > CODING_HISTORY_PAYLOAD_BYTES {
                        return Err(CodingError::History("retained history payload ceiling"));
                    }
                    records.push(CodingHistoryRecord { sequence, entry });
                }
            }
            match envelope.record {
                Record::Header {
                    version,
                    session: recorded_session,
                    root: recorded_root,
                } if sequence == 0 => {
                    if version != 1 || recorded_session != session || recorded_root != identity {
                        return Err(CodingError::History("header session/root/version mismatch"));
                    }
                }
                Record::Authorized { intent } if sequence > 0 => {
                    validate_intent(&intent)?;
                    material.authorize(&intent)?;
                    if intent.session != session
                        || intent.root != identity
                        || facts.len() >= OPERATIONS
                        || intent.operation != facts.len() as u64 + 1
                        || current.is_some()
                    {
                        return Err(CodingError::History("invalid authorization transition"));
                    }
                    facts.push(OperationFact {
                        operation: intent.operation,
                        target: intent.target.clone(),
                        state: OperationState::Authorized,
                    });
                    current = Some(intent);
                }
                Record::Dispatched { operation } if sequence > 0 => {
                    let fact = facts
                        .last_mut()
                        .ok_or(CodingError::History("dispatch without authorization"))?;
                    if fact.operation != operation
                        || fact.state != OperationState::Authorized
                        || current.is_none()
                    {
                        return Err(CodingError::History("invalid dispatch transition"));
                    }
                    fact.state = OperationState::Uncertain;
                }
                Record::Completed { operation } if sequence > 0 => {
                    let fact = facts
                        .last_mut()
                        .ok_or(CodingError::History("completion without dispatch"))?;
                    if fact.operation != operation || fact.state != OperationState::Uncertain {
                        return Err(CodingError::History("invalid completion transition"));
                    }
                    fact.state = OperationState::Completed;
                    current = None;
                }
                Record::Cancelled { operation } if sequence > 0 => {
                    let fact = facts
                        .last_mut()
                        .ok_or(CodingError::History("cancellation without authorization"))?;
                    if fact.operation != operation || fact.state != OperationState::Authorized {
                        return Err(CodingError::History("invalid cancellation transition"));
                    }
                    fact.state = OperationState::NotDispatched;
                    current = None;
                }
                Record::FollowUp {
                    invocation,
                    after_turn,
                    prompt,
                } if sequence > 0 && current.is_none() => {
                    material.follow_up(invocation, after_turn, &prompt, session, &facts)?;
                }
                Record::Material { entry } if sequence > 0 && current.is_none() => {
                    material.accept(&entry, &facts)?;
                    material.validate_bound(session, identity)?;
                }
                _ => return Err(CodingError::History("unexpected journal record")),
            }
            sequence += 1;
            buffer.clear();
        }
    }
    if sequence == 0 || !buffer.is_empty() {
        return Err(CodingError::History("empty or truncated journal framing"));
    }
    if collect
        && payload_bytes + facts.iter().map(|f| f.target.len()).sum::<usize>()
            > CODING_HISTORY_PAYLOAD_BYTES
    {
        return Err(CodingError::History("retained history payload ceiling"));
    }
    Ok((facts, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializer_reserves_newline_and_never_grows_backing() {
        let mut buffer = RecordBuffer(Vec::with_capacity(RECORD_BYTES));
        buffer.write_all(&[b'x'; RECORD_BYTES]).unwrap();
        assert!(buffer.write_all(b"\n").is_err());
        assert_eq!(buffer.0.capacity(), RECORD_BYTES);
        assert_eq!(buffer.0.len(), RECORD_BYTES);
    }
}
