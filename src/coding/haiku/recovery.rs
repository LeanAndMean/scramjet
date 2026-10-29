use super::{
    HaikuAttempt, HaikuCapture, HaikuEffect, HaikuHistory, HaikuHistoryEntry, HaikuIntent,
    HaikuOutcome, HaikuRecord, history::validate_intent,
};
use crate::{
    CodingError,
    http::responses::{bounded_json_bytes, bounded_json_value},
};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, openat};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, Read},
    path::Path,
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    sequence: usize,
    record: HaikuRecord,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Next,
    Attempt,
    Receipt,
    Selected,
    Authorized,
    Dispatched,
    Started,
    Effect,
    Result,
    Ending,
}
struct Call {
    name: String,
    arguments: String,
    id: String,
    response: String,
}
struct Check {
    phase: Phase,
    turn: usize,
    input: Vec<Value>,
    receipt: Option<HaikuAttempt>,
    call: Option<Call>,
    intent: Option<HaikuIntent>,
    created: [Option<HaikuCapture>; 2],
    reads: usize,
    read_haiku: bool,
    process: Option<super::HaikuProcessResult>,
    continuation: bool,
    requests: BTreeSet<u64>,
    ordinals: BTreeSet<usize>,
    responses: BTreeSet<String>,
    items: BTreeSet<String>,
    calls: BTreeSet<String>,
}
fn invalid() -> CodingError {
    CodingError::History("contradictory haiku history transition/provenance")
}
fn require(condition: bool) -> Result<(), CodingError> {
    if condition { Ok(()) } else { Err(invalid()) }
}
fn wire(input: &[Value]) -> Value {
    let tools: Vec<_> = super::haiku_tools().into_iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters,"strict":true})).collect();
    json!({"model":crate::PROVIDER_MODEL,"input":input,"tools":tools,"stream":true,"store":false,"include":["reasoning.encrypted_content"],"max_output_tokens":1024,"parallel_tool_calls":false})
}
impl Check {
    fn new() -> Self {
        Self {
            phase: Phase::Next,
            turn: 0,
            input: vec![json!({"role":"user","content":super::haiku_prompt()})],
            receipt: None,
            call: None,
            intent: None,
            created: [None, None],
            reads: 0,
            read_haiku: false,
            process: None,
            continuation: true,
            requests: BTreeSet::new(),
            ordinals: BTreeSet::new(),
            responses: BTreeSet::new(),
            items: BTreeSet::new(),
            calls: BTreeSet::new(),
        }
    }
    fn success(&self) -> bool {
        self.created.iter().all(Option::is_some)
            && self.read_haiku
            && self.process.as_ref().is_some_and(|result| {
                result.exit_code == Some(0)
                    && result.stderr.is_empty()
                    && result.stopped.is_none()
                    && result.launcher_reaped
                    && result.namespace_dead
                    && self.created[0]
                        .as_ref()
                        .is_some_and(|file| result.stdout == file.content)
            })
    }
    fn approval(
        &self,
        intent: &HaikuIntent,
        session: &str,
        root: [u64; 2],
    ) -> Result<(), CodingError> {
        validate_intent(intent, session, root)?;
        let call = self.call.as_ref().ok_or_else(invalid)?;
        let receipt = self.receipt.as_ref().ok_or_else(invalid)?;
        require(
            intent.operation == self.turn
                && intent.request_id == receipt.request_id
                && Some(intent.ordinal) == receipt.ordinal
                && intent.call_id == call.id
                && intent.response_id == call.response,
        )?;
        match &intent.effect {
            HaikuEffect::Create { path, content, .. } => {
                require(call.name == "write")?;
                let args: super::WriteArgs =
                    serde_json::from_str(&call.arguments).map_err(|_| invalid())?;
                let slot = super::name(path)?;
                require(
                    args.path == *path
                        && args.content == *content
                        && self.created[slot].is_none()
                        && (slot == 0 || self.read_haiku),
                )
            }
            HaikuEffect::Process {
                command, captures, ..
            } => {
                require(call.name == "bash" && self.process.is_none() && self.read_haiku)?;
                let args: super::BashArgs =
                    serde_json::from_str(&call.arguments).map_err(|_| invalid())?;
                require(
                    args.command == *command
                        && self.created[0].as_ref() == Some(&captures[0])
                        && self.created[1].as_ref() == Some(&captures[1]),
                )
            }
        }
    }
    fn push(
        &mut self,
        record: &HaikuRecord,
        session: &str,
        root: [u64; 2],
    ) -> Result<(), CodingError> {
        require(self.phase != Phase::Ending)?;
        match record {
            HaikuRecord::Header { .. } => return Err(invalid()),
            HaikuRecord::Attempt { turn, request } => {
                require(
                    matches!(self.phase, Phase::Next | Phase::Result)
                        && self.continuation
                        && *turn == self.turn + 1
                        && *turn <= 8
                        && self.call.is_none(),
                )?;
                let actual = bounded_json_value(request.as_bytes(), crate::PROVIDER_ENVELOPE_BYTES)
                    .map_err(|_| invalid())?;
                require(actual == wire(&self.input))?;
                self.turn = *turn;
                self.receipt = None;
                self.intent = None;
                self.phase = Phase::Attempt;
            }
            HaikuRecord::Receipt { attempt } => {
                require(
                    self.phase == Phase::Attempt
                        && attempt.turn == self.turn
                        && attempt.request_id != 0
                        && attempt.response_request_id != 0
                        && self.requests.insert(attempt.request_id),
                )?;
                if let Some(ordinal) = attempt.ordinal {
                    require(
                        ordinal > 0
                            && ordinal <= crate::PROVIDER_POST_BUDGET
                            && self.ordinals.insert(ordinal),
                    )?;
                }
                if let Some(usage) = &attempt.usage {
                    usage.validate().map_err(|_| invalid())?;
                }
                bounded_json_bytes(attempt, crate::PROVIDER_FRAME_BYTES + 1024)
                    .map_err(|_| invalid())?;
                self.receipt = Some(attempt.clone());
                self.phase = Phase::Receipt;
            }
            HaikuRecord::Selected {
                turn,
                request_id,
                ordinal,
                response_id,
                items,
                usage,
            } => {
                let receipt = self.receipt.as_ref().ok_or_else(invalid)?;
                require(
                    self.phase == Phase::Receipt
                        && *turn == self.turn
                        && receipt.request_id == *request_id
                        && receipt.response_request_id == *request_id
                        && receipt.ordinal == Some(*ordinal)
                        && receipt.durable
                        && receipt.outcome == HaikuOutcome::Success
                        && receipt.usage.as_ref() == Some(usage)
                        && usage.complete()
                        && receipt.terminal.as_ref().is_some_and(|terminal| {
                            terminal.response_id == *response_id && terminal.status == "completed"
                        })
                        && self.responses.insert(response_id.clone()),
                )?;
                bounded_json_bytes(
                    &json!({"response_id":response_id,"items":items}),
                    crate::PROVIDER_ENVELOPE_BYTES,
                )
                .map_err(|_| invalid())?;
                require(items.len() <= 8)?;
                for item in items {
                    let id = item["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(invalid)?;
                    require(self.items.insert(id.into()))?;
                    match item["type"].as_str() {
                        Some("message" | "reasoning") => {}
                        Some("function_call") => {
                            require(self.call.is_none())?;
                            let id = item["call_id"]
                                .as_str()
                                .filter(|id| !id.is_empty())
                                .ok_or_else(invalid)?;
                            require(self.calls.insert(id.into()))?;
                            self.call = Some(Call {
                                name: item["name"].as_str().ok_or_else(invalid)?.into(),
                                arguments: item["arguments"].as_str().ok_or_else(invalid)?.into(),
                                id: id.into(),
                                response: response_id.clone(),
                            });
                        }
                        _ => return Err(invalid()),
                    }
                    self.input.push(item.clone());
                }
                self.phase = Phase::Selected;
            }
            HaikuRecord::Authorized { intent } | HaikuRecord::Denied { intent } => {
                require(self.phase == Phase::Selected)?;
                self.approval(intent, session, root)?;
                self.intent = Some(intent.clone());
                self.phase = if matches!(record, HaikuRecord::Denied { .. }) {
                    self.continuation = false;
                    Phase::Effect
                } else {
                    Phase::Authorized
                };
            }
            HaikuRecord::NotDispatched { operation } => {
                require(
                    self.phase == Phase::Authorized
                        && self
                            .intent
                            .as_ref()
                            .is_some_and(|intent| intent.operation == *operation),
                )?;
                self.continuation = false;
                self.phase = Phase::Effect;
            }
            HaikuRecord::Dispatched { operation } => {
                require(
                    self.phase == Phase::Authorized
                        && self
                            .intent
                            .as_ref()
                            .is_some_and(|intent| intent.operation == *operation),
                )?;
                self.phase = Phase::Dispatched;
            }
            HaikuRecord::Started {
                operation,
                launcher,
                namespace_init,
            } => {
                require(
                    self.phase == Phase::Dispatched
                        && *launcher > 0
                        && *namespace_init > 0
                        && self.intent.as_ref().is_some_and(|intent| {
                            intent.operation == *operation
                                && matches!(intent.effect, HaikuEffect::Process { .. })
                        }),
                )?;
                self.phase = Phase::Started;
            }
            HaikuRecord::Created { operation, capture } => {
                let intent = self.intent.as_ref().ok_or_else(invalid)?;
                let HaikuEffect::Create { path, content, .. } = &intent.effect else {
                    return Err(invalid());
                };
                require(
                    self.phase == Phase::Dispatched
                        && intent.operation == *operation
                        && capture.path == *path
                        && capture.content == *content
                        && capture.content.len() <= 512,
                )?;
                self.created[super::name(path)?] = Some(capture.clone());
                self.phase = Phase::Effect;
            }
            HaikuRecord::Process { operation, result } => {
                require(
                    self.phase == Phase::Started
                        && self.intent.as_ref().is_some_and(|intent| {
                            intent.operation == *operation
                                && matches!(intent.effect, HaikuEffect::Process { .. })
                        })
                        && result.stdout.len() + result.stderr.len() <= 16384
                        && result.launcher_reaped
                        && result.namespace_dead
                        && !(result.exit_code.is_some() && result.signal.is_some())
                        && result.signal.is_none_or(|signal| signal > 0),
                )?;
                self.process = Some(result.clone());
                self.continuation = self.success();
                self.phase = Phase::Effect;
            }
            HaikuRecord::Result { call_id, output } => {
                let call = self.call.as_ref().ok_or_else(invalid)?;
                require(*call_id == call.id)?;
                let actual = bounded_json_value(output.as_bytes(), super::journal::RECORD_BYTES)
                    .map_err(|_| invalid())?;
                let expected = if self.phase == Phase::Selected && call.name == "read" {
                    let args: super::ReadArgs =
                        serde_json::from_str(&call.arguments).map_err(|_| invalid())?;
                    let slot = super::name(&args.path)?;
                    let capture = self.created[slot].as_ref().ok_or_else(invalid)?;
                    require(self.reads < 3)?;
                    self.reads += 1;
                    self.read_haiku |= slot == 0;
                    json!({"status":"read","path":args.path,"content":capture.content})
                } else {
                    require(
                        self.phase == Phase::Effect && self.continuation
                            || self.phase == Phase::Effect && self.process.is_some(),
                    )?;
                    match &self.intent.as_ref().ok_or_else(invalid)?.effect {
                        HaikuEffect::Create { path, .. } => {
                            json!({"status":"created","path":path,"mode":384})
                        }
                        HaikuEffect::Process { .. } => {
                            serde_json::to_value(self.process.as_ref().ok_or_else(invalid)?)
                                .unwrap()
                        }
                    }
                };
                require(actual == expected)?;
                self.input
                    .push(json!({"type":"function_call_output","call_id":call_id,"output":output}));
                self.call = None;
                self.phase = Phase::Result;
            }
            HaikuRecord::Finished => {
                require(self.phase == Phase::Selected && self.call.is_none() && self.success())?;
                self.phase = Phase::Ending;
            }
            HaikuRecord::Stopped { turn, reason } => {
                require(*turn == self.turn && self.turn > 0 && !reason.is_empty())?;
                self.phase = Phase::Ending;
            }
        }
        Ok(())
    }
}

pub fn reconstruct_haiku_history(
    storage: &Path,
    session: &str,
) -> Result<Arc<HaikuHistory>, CodingError> {
    super::super::validate_session(session)?;
    let storage_path = storage;
    let storage = crate::project::open_directory(storage_path)?;
    crate::project::validate_directory(&storage, true)?;
    let directory = File::from(openat(
        &storage,
        session,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    crate::project::validate_directory(&directory, true)?;
    let file = File::from(openat(
        &directory,
        "haiku.jsonl",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    crate::project::validate_stage(&file, 0o600)?;
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;
    let before = rustix::fs::fstat(&file)?;
    if before.st_size < 0 || before.st_size > 2097152 {
        return Err(CodingError::ResourceLimit("haiku history resource limit"));
    }
    let mut reader = std::io::BufReader::with_capacity(4096, file);
    let mut line = Vec::with_capacity(super::journal::RECORD_BYTES);
    let mut history: Option<HaikuHistory> = None;
    let mut check = Check::new();
    let mut total = 0;
    loop {
        line.clear();
        let count = Read::by_ref(&mut reader)
            .take(super::journal::RECORD_BYTES as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            break;
        }
        total += count;
        if total > 2097152 || !line.ends_with(b"\n") && count == super::journal::RECORD_BYTES {
            return Err(CodingError::ResourceLimit(
                "haiku record/history resource limit",
            ));
        }
        if !line.ends_with(b"\n") {
            return Err(CodingError::History("torn haiku record"));
        }
        let entry: Envelope = serde_json::from_slice(&line).map_err(|error| {
            let message = error.to_string();
            if message.starts_with("haiku selected")
                || message.starts_with("selected item resource limit")
                || message.starts_with("recursion limit exceeded")
            {
                CodingError::ResourceLimit("haiku recovery JSON resource limit")
            } else {
                CodingError::History("invalid bounded haiku record")
            }
        })?;
        if let Some(history) = &mut history {
            require(entry.sequence == history.entries.len() && entry.sequence < 128)?;
            check.push(&entry.record, session, history.root)?;
            if let Some(previous) = history.entries.last_mut() {
                previous.acknowledged = true;
            }
            history.entries.push(HaikuHistoryEntry {
                sequence: entry.sequence,
                record: Arc::new(entry.record),
                acknowledged: false,
            });
        } else {
            let HaikuRecord::Header {
                version,
                session: recorded,
                root,
            } = &entry.record
            else {
                return Err(invalid());
            };
            require(entry.sequence == 0 && *version == 3 && recorded == session)?;
            history = Some(HaikuHistory {
                session: session.into(),
                root: *root,
                entries: vec![HaikuHistoryEntry {
                    sequence: 0,
                    record: Arc::new(entry.record),
                    acknowledged: false,
                }],
            });
        }
    }
    let after = rustix::fs::fstat(reader.get_ref())?;
    require(
        (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_mtime,
            before.st_mtime_nsec,
            before.st_ctime,
            before.st_ctime_nsec,
        ) == (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_mtime,
            after.st_mtime_nsec,
            after.st_ctime,
            after.st_ctime_nsec,
        ),
    )?;
    crate::project::validate_stage(reader.get_ref(), 0o600)?;
    let named = File::from(openat(
        &directory,
        "haiku.jsonl",
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    require(super::identity(&named)? == super::identity(reader.get_ref())?)?;
    let named_directory = File::from(openat(
        &storage,
        session,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    require(super::identity(&named_directory)? == super::identity(&directory)?)?;
    require(
        super::identity(&crate::project::open_directory(storage_path)?)?
            == super::identity(&storage)?,
    )?;
    Ok(Arc::new(history.ok_or_else(invalid)?))
}
