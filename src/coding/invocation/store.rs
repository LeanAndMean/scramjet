use super::*;
use crate::coding::captured_files::RUN_ANCHOR;
use rustix::fs::{FlockOperation, Mode, OFlags, flock, fstat, mkdirat, openat};
use std::{
    io::{BufRead, Read},
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

const JOURNAL: &str = "invocation.jsonl";
const PROVIDER: &str = "provider";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Dormant,
    Ready,
    Attempt,
    Receipt,
    Selected,
    Authorized,
    Charged,
    Dispatched,
    Started,
    Effect,
    Complete,
    Final,
    Export,
    Stopped,
}
struct Call {
    id: String,
    name: String,
    arguments: String,
    response: String,
}
pub(super) struct State {
    pub header: Header,
    pub generation: usize,
    pub material: Vec<Material>,
    pub counters: Counters,
    pub captures: [Option<CapturedFile>; 2],
    receipts: Vec<Receipt>,
    phase: Phase,
    call: Option<Call>,
    intent: Option<CodingInvocationIntent>,
    charge: Option<Charge>,
    process: Option<super::super::process::ProcessResult>,
    pub(super) responses: Vec<String>,
    exports: Vec<(usize, String)>,
    results: usize,
    acknowledged_results: usize,
    uncertain_effect: bool,
}
fn invalid() -> CodingError {
    CodingError::History("contradictory or incomplete invocation history")
}
fn require(ok: bool) -> Result<(), CodingError> {
    if ok { Ok(()) } else { Err(invalid()) }
}
pub(super) fn identity(file: &File) -> Result<[u64; 2], CodingError> {
    let s = fstat(file)?;
    Ok([s.st_dev, s.st_ino])
}
fn boot() -> Result<String, CodingError> {
    let file = File::open("/proc/sys/kernel/random/boot_id")?;
    let mut bytes = Vec::with_capacity(129);
    file.take(129).read_to_end(&mut bytes)?;
    if bytes.len() > 128 {
        return Err(CodingError::ResourceLimit("boot identity resource limit"));
    }
    let boot = String::from_utf8(bytes)
        .map_err(|_| CodingError::Unsupported("boot identity unavailable"))?;
    if boot.trim().is_empty() {
        return Err(CodingError::Unsupported("boot identity unavailable"));
    }
    Ok(boot)
}
#[cfg(target_os = "linux")]
fn now_ns() -> Result<u64, CodingError> {
    let t = rustix::time::clock_gettime_dynamic(rustix::time::DynamicClockId::Boottime)?;
    u64::try_from(t.tv_sec)
        .ok()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .and_then(|s| u64::try_from(t.tv_nsec).ok().and_then(|n| s.checked_add(n)))
        .ok_or(CodingError::Unsupported("boot clock unavailable"))
}
#[cfg(not(target_os = "linux"))]
fn now_ns() -> Result<u64, CodingError> {
    Err(CodingError::Unsupported(
        "boot-qualified Linux run clock unavailable",
    ))
}
fn open_file(root: &File, name: &str, writable: bool) -> Result<File, CodingError> {
    let file = File::from(openat(
        root,
        name,
        (if writable {
            OFlags::RDWR | OFlags::APPEND
        } else {
            OFlags::RDONLY
        }) | OFlags::NOFOLLOW
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    crate::project::validate_stage(&file, 0o600)?;
    Ok(file)
}
fn read_bytes(file: &File, limit: usize) -> Result<Vec<u8>, CodingError> {
    crate::project::validate_stage(file, 0o600)?;
    let before = fstat(file)?;
    if before.st_size < 0 || before.st_size as u64 > limit as u64 {
        return Err(CodingError::ResourceLimit(
            "invocation artifact resource limit",
        ));
    }
    let mut bytes = Vec::with_capacity(before.st_size as usize + 1);
    use std::os::unix::fs::FileExt;
    let mut chunk = [0; 4096];
    loop {
        let n = file.read_at(&mut chunk, bytes.len() as u64)?;
        if n == 0 {
            break;
        }
        if n > limit - bytes.len() {
            return Err(CodingError::ResourceLimit(
                "invocation artifact resource limit",
            ));
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let after = fstat(file)?;
    require(
        (
            before.st_dev,
            before.st_ino,
            before.st_size,
            before.st_mode,
            before.st_nlink,
            before.st_uid,
            before.st_mtime,
            before.st_mtime_nsec,
            before.st_ctime,
            before.st_ctime_nsec,
        ) == (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_mode,
            after.st_nlink,
            after.st_uid,
            after.st_mtime,
            after.st_mtime_nsec,
            after.st_ctime,
            after.st_ctime_nsec,
        ),
    )?;
    Ok(bytes)
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, CodingError> {
    let value = crate::http::responses::bounded_json_value(bytes, limit).map_err(context_error)?;
    serde_json::from_value(value).map_err(|_| invalid())
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, CodingError> {
    crate::http::responses::bounded_json_bytes(value, RECORD_BYTES - 1).map_err(context_error)
}
impl State {
    fn new(header: Header) -> Result<Self, CodingError> {
        require(
            header.version == 1
                && header.model == crate::PROVIDER_MODEL
                && header.limits == LIMITS
                && header.deadline_ns
                    == header
                        .started_ns
                        .checked_add(RUN_SECONDS * 1_000_000_000)
                        .ok_or_else(invalid)?,
        )?;
        validate_run(&header.run)?;
        FileSlots::new([&header.names[0], &header.names[1]])?;
        for (index, capture) in header.initial.iter().enumerate() {
            if let Some(c) = capture {
                require(c.path == header.names[index] && c.content.len() <= FILE_BYTES)?;
            }
        }
        Ok(Self {
            captures: header.initial.clone(),
            header,
            generation: 0,
            material: Vec::new(),
            counters: Counters::default(),
            receipts: Vec::with_capacity(ATTEMPTS),
            phase: Phase::Dormant,
            call: None,
            intent: None,
            charge: None,
            process: None,
            responses: Vec::with_capacity(ATTEMPTS),
            exports: Vec::with_capacity(ATTEMPTS),
            results: 0,
            acknowledged_results: 0,
            uncertain_effect: false,
        })
    }
    fn apply(&mut self, record: &Record) -> Result<(), CodingError> {
        require(self.phase != Phase::Stopped)?;
        match record {
            Record::Header { .. } => return Err(invalid()),
            Record::InvocationStarted { generation, prompt } => {
                require(
                    matches!(self.phase, Phase::Dormant | Phase::Export)
                        && *generation == self.generation + 1
                        && *generation <= ATTEMPTS,
                )?;
                self.material.push(Material::User {
                    text: prompt.clone(),
                });
                request(&self.material, &self.header.names)?;
                self.generation = *generation;
                self.phase = Phase::Ready;
            }
            Record::Attempt {
                operation,
                request: wire,
            } => {
                require(
                    self.phase == Phase::Ready
                        && *operation == self.counters.attempts + 1
                        && *operation <= ATTEMPTS,
                )?;
                let expected = request(&self.material, &self.header.names)?;
                require(
                    decode::<Value>(wire.as_bytes(), crate::PROVIDER_ENVELOPE_BYTES)?
                        == decode::<Value>(
                            expected.encoded.as_bytes(),
                            crate::PROVIDER_ENVELOPE_BYTES,
                        )?,
                )?;
                self.counters.attempts = *operation;
                self.acknowledged_results = self.results;
                self.intent = None;
                self.charge = None;
                self.process = None;
                self.phase = Phase::Attempt;
            }
            Record::Receipt { receipt } => {
                require(
                    self.phase == Phase::Attempt
                        && receipt.request_id > 0
                        && receipt.response_request_id > 0
                        && self
                            .receipts
                            .iter()
                            .all(|r| r.request_id != receipt.request_id),
                )?;
                crate::http::responses::bounded_json_bytes(
                    receipt,
                    crate::PROVIDER_FRAME_BYTES + 1024,
                )
                .map_err(context_error)?;
                if let Some(u) = &receipt.usage {
                    u.validate().map_err(context_error)?;
                }
                if let Some(o) = receipt.ordinal {
                    require(
                        o > 0
                            && o <= crate::PROVIDER_POST_BUDGET
                            && self.receipts.iter().all(|r| r.ordinal != Some(o)),
                    )?;
                }
                self.receipts.push(receipt.clone());
                self.phase = Phase::Receipt;
            }
            Record::Selected { response_id, items } => {
                let r = self.receipts.last().ok_or_else(invalid)?;
                require(
                    self.phase == Phase::Receipt
                        && r.durable
                        && r.request_id == r.response_request_id
                        && r.ordinal.is_some()
                        && r.outcome == super::super::haiku::HaikuOutcome::Success
                        && r.usage.as_ref().is_some_and(|u| u.complete())
                        && r.terminal.as_ref().is_some_and(|t| {
                            t.response_id == *response_id && t.status == "completed"
                        })
                        && !self.responses.contains(response_id)
                        && !items.is_empty()
                        && items.len() <= crate::PROVIDER_OUTPUT_ITEMS,
                )?;
                crate::http::responses::bounded_json_bytes(
                    &json!({"response_id":response_id,"items":items}),
                    crate::PROVIDER_ENVELOPE_BYTES,
                )
                .map_err(context_error)?;
                for value in items {
                    let item = ProviderItem::from_context(value.clone()).map_err(context_error)?;
                    require(
                        !self
                            .material
                            .iter()
                            .any(|m| matches!(m,Material::Item {value:v} if v["id"]==value["id"])),
                    )?;
                    if let Some(c) = item.function_call() {
                        require(self.call.is_none() && !self.material.iter().any(|m|matches!(m,Material::Item {value:v} if v["type"]=="function_call" && v["call_id"].as_str()==Some(c.call_id))))?;
                        self.call = Some(Call {
                            id: c.call_id.into(),
                            name: c.name.into(),
                            arguments: c.arguments.into(),
                            response: response_id.clone(),
                        });
                    }
                    self.material.push(Material::Item {
                        value: value.clone(),
                    });
                }
                self.responses.push(response_id.clone());
                self.phase = Phase::Selected;
            }
            Record::Authorized { intent } | Record::Denied { intent } => {
                let c = self.call.as_ref().ok_or_else(invalid)?;
                let r = self.receipts.last().ok_or_else(invalid)?;
                require(
                    self.phase == Phase::Selected
                        && intent.run == self.header.run
                        && intent.root == self.header.root
                        && intent.generation == self.generation
                        && intent.operation == self.counters.attempts
                        && intent.request_id == r.request_id
                        && Some(intent.ordinal) == r.ordinal
                        && intent.response_id == c.response
                        && intent.call_id == c.id,
                )?;
                match &intent.effect {
                    CodingInvocationEffect::Create {
                        path,
                        content,
                        mode,
                    } => {
                        let args: WriteArgs =
                            serde_json::from_str(&c.arguments).map_err(|_| invalid())?;
                        let index = self
                            .header
                            .names
                            .iter()
                            .position(|n| n == path)
                            .ok_or_else(invalid)?;
                        require(
                            c.name == "write"
                                && args.path == *path
                                && args.content == *content
                                && *mode == 0o600
                                && content.len() <= FILE_BYTES
                                && self.captures[index].is_none(),
                        )?;
                    }
                    CodingInvocationEffect::Python {
                        path,
                        captures,
                        argv,
                        profile,
                    } => {
                        let args: PathArgs =
                            serde_json::from_str(&c.arguments).map_err(|_| invalid())?;
                        require(
                            c.name == "python"
                                && args.path == *path
                                && self.captures.iter().flatten().any(|c| c.path == *path)
                                && captures
                                    == &self.captures.iter().flatten().cloned().collect::<Vec<_>>()
                                && *argv
                                    == [
                                        "/usr/bin/python3".to_owned(),
                                        "-I".to_owned(),
                                        format!("/workspace/{path}"),
                                    ]
                                && profile == super::super::process::PYTHON_PROFILE,
                        )?;
                    }
                }
                self.intent = Some(intent.clone());
                self.phase = if matches!(record, Record::Denied { .. }) {
                    Phase::Stopped
                } else {
                    Phase::Authorized
                };
            }
            Record::Charged { operation, charge } => {
                require(*operation == self.counters.attempts)?;
                match charge {
                    Charge::Read => {
                        require(
                            self.phase == Phase::Selected
                                && self.call.as_ref().is_some_and(|c| c.name == "read")
                                && self.counters.reads < LIMITS.reads,
                        )?;
                        self.counters.reads += 1;
                    }
                    Charge::Create => {
                        require(
                            self.phase == Phase::Authorized
                                && self.intent.as_ref().is_some_and(|i| {
                                    matches!(i.effect, CodingInvocationEffect::Create { .. })
                                })
                                && self.counters.creates < LIMITS.creates,
                        )?;
                        self.counters.creates += 1;
                    }
                    Charge::Python => {
                        require(
                            self.phase == Phase::Authorized
                                && self.intent.as_ref().is_some_and(|i| {
                                    matches!(i.effect, CodingInvocationEffect::Python { .. })
                                })
                                && self.counters.processes < LIMITS.processes,
                        )?;
                        self.counters.processes += 1;
                    }
                }
                self.charge = Some(*charge);
                self.phase = Phase::Charged;
            }
            Record::Dispatched { operation } => {
                require(
                    self.phase == Phase::Charged
                        && *operation == self.counters.attempts
                        && self.charge != Some(Charge::Read),
                )?;
                self.uncertain_effect = true;
                self.phase = Phase::Dispatched;
            }
            Record::Started {
                operation,
                launcher,
                namespace_init,
            } => {
                require(
                    self.phase == Phase::Dispatched
                        && self.charge == Some(Charge::Python)
                        && *operation == self.counters.attempts
                        && *launcher > 0
                        && *namespace_init > 0,
                )?;
                self.phase = Phase::Started;
            }
            Record::Created { operation, capture } => {
                let i = self.intent.as_ref().ok_or_else(invalid)?;
                let CodingInvocationEffect::Create { path, content, .. } = &i.effect else {
                    return Err(invalid());
                };
                require(
                    self.phase == Phase::Dispatched
                        && self.charge == Some(Charge::Create)
                        && *operation == self.counters.attempts
                        && capture.path == *path
                        && capture.content == *content,
                )?;
                let index = self
                    .header
                    .names
                    .iter()
                    .position(|n| n == path)
                    .ok_or_else(invalid)?;
                self.captures[index] = Some(capture.clone());
                self.phase = Phase::Effect;
            }
            Record::Process { operation, result } => {
                require(
                    self.phase == Phase::Started
                        && *operation == self.counters.attempts
                        && result.launcher_reaped
                        && result.namespace_dead
                        && !(result.exit_code.is_some() && result.signal.is_some())
                        && result.stdout.byte_len() + result.stderr.byte_len() <= 16384,
                )?;
                self.process = Some(result.clone());
                self.phase = Phase::Effect;
            }
            Record::Result { call_id, output } => {
                let c = self.call.as_ref().ok_or_else(invalid)?;
                require(c.id == *call_id)?;
                let actual = decode::<Value>(output.as_bytes(), RECORD_BYTES)?;
                let expected = if self.phase == Phase::Charged && self.charge == Some(Charge::Read)
                {
                    let args: PathArgs =
                        serde_json::from_str(&c.arguments).map_err(|_| invalid())?;
                    let index = self
                        .header
                        .names
                        .iter()
                        .position(|n| n == &args.path)
                        .ok_or_else(invalid)?;
                    match &self.captures[index] {
                        Some(c) => json!({"status":"read","path":args.path,"content":c.content}),
                        None => json!({"status":"error","kind":"not_found","path":args.path}),
                    }
                } else {
                    require(self.phase == Phase::Effect)?;
                    match &self.intent.as_ref().ok_or_else(invalid)?.effect {
                        CodingInvocationEffect::Create { path, .. } => {
                            json!({"status":"created","path":path,"mode":384})
                        }
                        CodingInvocationEffect::Python { .. } => {
                            serde_json::to_value(self.process.as_ref().ok_or_else(invalid)?)
                                .unwrap()
                        }
                    }
                };
                require(actual == expected)?;
                self.material.push(Material::ToolResult {
                    call_id: call_id.clone(),
                    output: output.clone(),
                });
                self.results += 1;
                self.uncertain_effect = false;
                self.call = None;
                self.phase = Phase::Ready;
            }
            Record::MessageComplete => {
                require(self.phase == Phase::Selected && self.call.is_none())?;
                self.acknowledged_results = self.results;
                self.phase = Phase::Complete;
            }
            Record::Final {
                generation,
                worker_joined,
                transports_joined,
            } => {
                require(
                    self.phase == Phase::Complete
                        && *generation == self.generation
                        && *worker_joined
                        && *transports_joined,
                )?;
                self.phase = Phase::Final;
            }
            Record::Export { generation, path } => {
                require(
                    self.phase == Phase::Final
                        && *generation == self.generation
                        && *path == format!("export-{generation}.json"),
                )?;
                self.exports.push((*generation, path.clone()));
                self.phase = Phase::Export;
            }
            Record::Stopped { reason } => {
                require(!reason.is_empty())?;
                self.acknowledged_results = self.results;
                self.phase = Phase::Stopped;
            }
        }
        Ok(())
    }
    fn export(&self) -> Export {
        Export {
            version: 1,
            header: self.header.clone(),
            generation: self.generation,
            disposition: "complete".into(),
            conversation: self.material.clone(),
            tools: tool_values(&self.header.names),
            counters: self.counters,
            captures: self.captures.clone(),
            receipts: self.receipts.clone(),
            worker_joined: true,
            transports_joined: true,
            export_producer_acknowledgement: "unknown_on_recovery".into(),
            execution: json!({"phase":if matches!(self.phase,Phase::Final|Phase::Export){"Complete".to_owned()}else{format!("{:?}",self.phase)},"uncertain_effect":self.uncertain_effect,"results":self.results,"acknowledged_results":self.acknowledged_results}),
        }
    }
}

pub(super) struct Store {
    pub workspace: Arc<crate::project::Workspace>,
    storage_path: PathBuf,
    directory: File,
    file: File,
    pub state: State,
    records: usize,
    retained: usize,
    reserved: usize,
    poisoned: bool,
}
impl Store {
    pub fn open(
        project: &Path,
        storage: &Path,
        run: &str,
        names: [String; 2],
        previous: Option<&[u8]>,
        endpoint: SocketAddr,
    ) -> Result<(Self, crate::Runtime, Arc<Mutex<ProviderLedger>>), CodingError> {
        let entry_ns = now_ns()?;
        validate_run(run)?;
        FileSlots::new([&names[0], &names[1]])?;
        let root = crate::project::open_directory(project)?;
        crate::project::validate_directory(&root, true)?;
        let anchor = match open_file(&root, RUN_ANCHOR, false) {
            Ok(a) => Some(a),
            Err(CodingError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        require(anchor.is_some() == previous.is_some())?;
        if anchor.is_some() {
            open_file(&root, crate::PROJECT_LOCK_NAME, false)?;
        }
        for entry in rustix::fs::Dir::read_from(&root)? {
            let entry = entry?;
            let bytes = entry.file_name().to_bytes();
            if ![
                b".".as_slice(),
                b"..",
                crate::PROJECT_LOCK_NAME.as_bytes(),
                RUN_ANCHOR.as_bytes(),
                names[0].as_bytes(),
                names[1].as_bytes(),
            ]
            .contains(&bytes)
            {
                return Err(CodingError::Invalid("unselected project entry"));
            }
        }
        let workspace = crate::ProjectWorkspace::open(project, storage)?.workspace();
        require(identity(&root)? == identity(&workspace.root)?)?;
        if anchor.is_none() && !workspace.coding_run_marker()?.is_empty() {
            return Err(CodingError::Stale);
        }
        let directory = match openat(
            &workspace.storage,
            run,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => {
                require(anchor.is_some() && previous.is_some())?;
                File::from(fd)
            }
            Err(rustix::io::Errno::NOENT) => {
                require(anchor.is_none() && previous.is_none())?;
                mkdirat(&workspace.storage, run, Mode::from_raw_mode(0o700))?;
                File::from(openat(
                    &workspace.storage,
                    run,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?)
            }
            Err(e) => return Err(e.into()),
        };
        crate::project::validate_directory(&directory, true)?;
        let provider_path = storage.join(run).join(PROVIDER);
        let (mut store, runtime, ledger) = if let Some(anchor) = anchor {
            let file = open_file(&directory, JOURNAL, true)?;
            flock(&file, FlockOperation::NonBlockingLockExclusive)?;
            let (state, records, retained) = Self::load(&file, &directory)?;
            let header = &state.header;
            require(
                header.run == run
                    && header.names == names
                    && header.root == identity(&workspace.root)?
                    && header.storage == identity(&workspace.storage)?
                    && header.directory == identity(&directory)?
                    && header.journal == identity(&file)?
                    && header.provider.endpoint == endpoint,
            )?;
            let anchor_value: Value = decode(&read_bytes(&anchor, RECORD_BYTES)?, RECORD_BYTES)?;
            require(
                anchor_value
                    == json!({"run":run,"storage":header.storage,"directory":header.directory,"journal":header.journal}),
            )?;
            require(decode::<Value>(&workspace.coding_run_marker()?, 1024)? == anchor_value)?;
            let supplied: Export = decode(previous.ok_or_else(invalid)?, RECORD_BYTES)?;
            require(state.phase == Phase::Export && supplied == state.export())?;
            let latest = state.exports.last().ok_or_else(invalid)?;
            let artifact = open_file(&directory, &latest.1, false)?;
            require(
                decode::<Export>(&read_bytes(&artifact, RECORD_BYTES)?, RECORD_BYTES)? == supplied,
            )?;
            let provider_directory = File::from(openat(
                &directory,
                PROVIDER,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            require(identity(&provider_directory)? == header.provider.directory)?;
            let provider_file = open_file(&provider_directory, "provider-attempts.jsonl", false)?;
            require(identity(&provider_file)? == header.provider.file)?;
            let store = Self {
                workspace,
                storage_path: storage.into(),
                directory,
                file,
                state,
                records,
                retained,
                reserved: 0,
                poisoned: false,
            };
            store.guard()?;
            let runtime = crate::Runtime::new_provider(endpoint, &provider_path)?;
            let ledger = runtime.coding_provider(endpoint)?;
            store.check_receipts(&*ledger.lock().map_err(|_| CodingError::Poisoned)?)?;
            (store, runtime, ledger)
        } else {
            let file = File::from(openat(
                &directory,
                JOURNAL,
                OFlags::RDWR
                    | OFlags::APPEND
                    | OFlags::CREATE
                    | OFlags::EXCL
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )?);
            crate::project::validate_stage(&file, 0o600)?;
            flock(&file, FlockOperation::NonBlockingLockExclusive)?;
            mkdirat(&directory, PROVIDER, Mode::from_raw_mode(0o700))?;
            let runtime = crate::Runtime::new_provider(endpoint, &provider_path)?;
            let ledger = runtime.coding_provider(endpoint)?;
            let initial = [0, 1].map(|i| {
                let slots = FileSlots::new([&names[0], &names[1]])?;
                match slots.capture(&workspace.root, &names[i]) {
                    Ok(c) => Ok(Some(c)),
                    Err(CodingError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(e),
                }
            });
            let [a, b] = initial;
            let initial = [a?, b?];
            let started_ns = entry_ns;
            let header = Header {
                version: 1,
                model: crate::PROVIDER_MODEL.into(),
                run: run.into(),
                root: identity(&workspace.root)?,
                storage: identity(&workspace.storage)?,
                directory: identity(&directory)?,
                journal: identity(&file)?,
                provider: ledger
                    .lock()
                    .map_err(|_| CodingError::Poisoned)?
                    .binding()?,
                names,
                initial,
                boot: boot()?,
                started_ns,
                deadline_ns: started_ns
                    .checked_add(RUN_SECONDS * 1_000_000_000)
                    .ok_or_else(invalid)?,
                limits: LIMITS,
            };
            let marker = encode(
                &json!({"run":run,"storage":header.storage,"directory":header.directory,"journal":header.journal}),
            )?;
            workspace.install_coding_run_marker(&marker)?;
            let mut bytes = encode(
                &json!({"sequence":0,"record":Record::Header{header:Box::new(header.clone())}}),
            )?;
            bytes.push(b'\n');
            (&file).write_all(&bytes)?;
            file.sync_all()?;
            directory.sync_all()?;
            workspace.storage.sync_all()?;
            let anchor = File::from(openat(
                &workspace.root,
                RUN_ANCHOR,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            )?);
            crate::project::validate_stage(&anchor, 0o600)?;
            (&anchor).write_all(&marker)?;
            anchor.sync_all()?;
            workspace.root.sync_all()?;
            let state = State::new(header)?;
            (
                Self {
                    workspace,
                    storage_path: storage.into(),
                    directory,
                    file,
                    state,
                    records: 1,
                    retained: bytes.len(),
                    reserved: 0,
                    poisoned: false,
                },
                runtime,
                ledger,
            )
        };
        store.guard()?;
        store.check_receipts(&*ledger.lock().map_err(|_| CodingError::Poisoned)?)?;
        store.reserved = 1;
        Ok((store, runtime, ledger))
    }
    fn load(file: &File, directory: &File) -> Result<(State, usize, usize), CodingError> {
        crate::project::validate_stage(file, 0o600)?;
        let before = fstat(file)?;
        if before.st_size < 0 || before.st_size as u64 > RETAINED_BYTES as u64 {
            return Err(CodingError::ResourceLimit("retained run resource limit"));
        }
        let mut reader = std::io::BufReader::with_capacity(4096, file.try_clone()?);
        let mut line = Vec::with_capacity(RECORD_BYTES);
        let mut state: Option<State> = None;
        let mut records = 0;
        let mut retained = 0usize;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Envelope {
            sequence: usize,
            record: Record,
        }
        loop {
            line.clear();
            let count = Read::by_ref(&mut reader)
                .take(RECORD_BYTES as u64)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                break;
            }
            if !line.ends_with(b"\n") {
                return Err(if count == RECORD_BYTES {
                    CodingError::ResourceLimit("invocation record resource limit")
                } else {
                    invalid()
                });
            }
            if records >= RECORDS {
                return Err(CodingError::ResourceLimit("invocation record count limit"));
            }
            retained = retained
                .checked_add(count)
                .ok_or(CodingError::ResourceLimit("retained run resource limit"))?;
            if retained > RETAINED_BYTES {
                return Err(CodingError::ResourceLimit("retained run resource limit"));
            }
            let envelope: Envelope = decode(&line[..count - 1], RECORD_BYTES)?;
            require(envelope.sequence == records)?;
            if let Some(state) = &mut state {
                state.apply(&envelope.record)?;
                if let Record::Export { path, .. } = &envelope.record {
                    let artifact = open_file(directory, path, false)?;
                    let data = read_bytes(&artifact, RECORD_BYTES)?;
                    require(decode::<Export>(&data, RECORD_BYTES)? == state.export())?;
                    retained = retained
                        .checked_add(data.len())
                        .ok_or(CodingError::ResourceLimit("retained run resource limit"))?;
                }
            } else {
                let Record::Header { header } = envelope.record else {
                    return Err(invalid());
                };
                state = Some(State::new(*header)?);
            }
            records += 1;
        }
        let after = fstat(file)?;
        require(
            (
                before.st_dev,
                before.st_ino,
                before.st_size,
                before.st_mode,
                before.st_nlink,
                before.st_uid,
                before.st_mtime,
                before.st_mtime_nsec,
                before.st_ctime,
                before.st_ctime_nsec,
            ) == (
                after.st_dev,
                after.st_ino,
                after.st_size,
                after.st_mode,
                after.st_nlink,
                after.st_uid,
                after.st_mtime,
                after.st_mtime_nsec,
                after.st_ctime,
                after.st_ctime_nsec,
            ),
        )?;
        let state = state.ok_or_else(invalid)?;
        for entry in rustix::fs::Dir::read_from(directory)? {
            let entry = entry?;
            let name = entry.file_name().to_str().map_err(|_| invalid())?;
            require(
                [".", "..", JOURNAL, PROVIDER].contains(&name)
                    || state.exports.iter().any(|(_, path)| path == name),
            )?;
        }
        if retained > RETAINED_BYTES {
            return Err(CodingError::ResourceLimit("retained run resource limit"));
        }
        Ok((state, records, retained))
    }
    pub fn check_receipts(&self, ledger: &ProviderLedger) -> Result<(), CodingError> {
        require(ledger.binding()? == self.state.header.provider)?;
        let history = ledger.history();
        require(
            history.unknown_usage() == 0
                && history.unresolved_remote() == 0
                && history.attempts.len() == self.state.receipts.len(),
        )?;
        for (receipt, fact) in self.state.receipts.iter().zip(&history.attempts) {
            require(
                receipt.durable
                    && receipt.request_id == fact.request_id
                    && receipt.ordinal == Some(fact.ordinal)
                    && receipt.terminal == fact.terminal
                    && receipt.usage == fact.usage
                    && fact.locally_joined,
            )?;
        }
        Ok(())
    }
    fn storage_guard(&self) -> Result<(), CodingError> {
        if self.poisoned {
            return Err(CodingError::Poisoned);
        }
        self.workspace.validate()?;
        crate::project::validate_directory(&self.workspace.root, true)?;
        crate::project::validate_directory(&self.workspace.storage, true)?;
        crate::project::validate_directory(&self.directory, true)?;
        let storage = crate::project::open_directory(&self.storage_path)?;
        let directory = File::from(openat(
            &self.workspace.storage,
            self.state.header.run.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let file = open_file(&self.directory, JOURNAL, false)?;
        require(
            identity(&storage)? == self.state.header.storage
                && identity(&directory)? == self.state.header.directory
                && identity(&file)? == self.state.header.journal
                && identity(&self.file)? == self.state.header.journal,
        )?;
        let anchor = open_file(&self.workspace.root, RUN_ANCHOR, false)?;
        let expected = json!({"run":self.state.header.run,"storage":self.state.header.storage,"directory":self.state.header.directory,"journal":self.state.header.journal});
        require(
            decode::<Value>(&read_bytes(&anchor, RECORD_BYTES)?, RECORD_BYTES)? == expected
                && decode::<Value>(&self.workspace.coding_run_marker()?, 1024)? == expected,
        )?;
        Ok(())
    }
    pub fn guard(&self) -> Result<(), CodingError> {
        self.storage_guard()?;
        require(boot()? == self.state.header.boot)?;
        let now = now_ns()?;
        if now < self.state.header.started_ns {
            return Err(CodingError::History("boot clock contradiction"));
        }
        if now >= self.state.header.deadline_ns {
            return Err(CodingError::Deadline);
        }
        let names = &self.state.header.names;
        let slots = FileSlots::new([&names[0], &names[1]])?;
        for (i, name) in names.iter().enumerate() {
            match &self.state.captures[i] {
                Some(c) => require(slots.capture(&self.workspace.root, name)? == *c)?,
                None => slots.absent(&self.workspace.root, name)?,
            }
        }
        Ok(())
    }
    pub fn deadline(&self) -> Result<Instant, CodingError> {
        self.guard()?;
        let remaining = self
            .state
            .header
            .deadline_ns
            .checked_sub(now_ns()?)
            .filter(|n| *n > 0)
            .ok_or(CodingError::Deadline)?;
        Ok(Instant::now() + Duration::from_nanos(remaining))
    }
    pub fn begin(&mut self, prompt: String) -> Result<(), CodingError> {
        self.guard()?;
        if self.state.generation >= ATTEMPTS || self.state.counters.attempts >= ATTEMPTS {
            return Err(CodingError::ResourceLimit("run invocation/attempt ceiling"));
        }
        let mut material = self.state.material.clone();
        material.push(Material::User {
            text: prompt.clone(),
        });
        request(&material, &self.state.header.names)?;
        self.append(Record::InvocationStarted {
            generation: self.state.generation + 1,
            prompt,
        })
    }
    fn used_bytes(&self) -> Result<usize, CodingError> {
        let provider_directory = File::from(openat(
            &self.directory,
            PROVIDER,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let provider_file = open_file(&provider_directory, "provider-attempts.jsonl", false)?;
        require(
            identity(&provider_directory)? == self.state.header.provider.directory
                && identity(&provider_file)? == self.state.header.provider.file,
        )?;
        let anchor = open_file(&self.workspace.root, RUN_ANCHOR, false)?;
        let marker_bytes = self.workspace.coding_run_marker()?.len() as u64;
        let extra = provider_file
            .metadata()?
            .len()
            .checked_add(anchor.metadata()?.len())
            .and_then(|n| n.checked_add(marker_bytes))
            .ok_or(CodingError::ResourceLimit("retained run resource limit"))?;
        self.retained
            .checked_add(
                usize::try_from(extra)
                    .map_err(|_| CodingError::ResourceLimit("retained run resource limit"))?,
            )
            .ok_or(CodingError::ResourceLimit("retained run resource limit"))
    }
    pub fn reserve_turn(&mut self) -> Result<(), CodingError> {
        self.guard()?;
        let provider_growth = 2 * (crate::PROVIDER_FRAME_BYTES + 1024 + 1);
        if self.records + 12 > RECORDS
            || self.used_bytes()? + 13 * RECORD_BYTES + provider_growth > RETAINED_BYTES
        {
            return Err(CodingError::ResourceLimit(
                "run transition/export reservation limit",
            ));
        }
        self.reserved = 12;
        Ok(())
    }
    pub fn append(&mut self, record: Record) -> Result<(), CodingError> {
        self.storage_guard()?;
        if self.reserved == 0 || self.records >= RECORDS {
            return Err(CodingError::ResourceLimit("unreserved invocation record"));
        }
        let mut bytes = encode(&json!({"sequence":self.records,"record":record}))?;
        bytes.push(b'\n');
        if self
            .used_bytes()?
            .checked_add(bytes.len())
            .is_none_or(|n| n > RETAINED_BYTES)
        {
            return Err(CodingError::ResourceLimit("retained run resource limit"));
        }
        self.poisoned = true;
        self.file.write_all(&bytes)?;
        self.file.sync_all()?;
        self.state.apply(&record)?;
        self.records += 1;
        self.retained += bytes.len();
        self.reserved -= 1;
        self.poisoned = false;
        self.storage_guard()
    }
    pub fn complete(&self) -> bool {
        self.state.phase == Phase::Complete
    }
    pub fn diagnostic(&self, transports_joined: bool) -> Result<Vec<u8>, CodingError> {
        let mut record = self.state.export();
        record.disposition = "stopped".into();
        record.transports_joined = transports_joined;
        record.export_producer_acknowledgement = "not_a_continuation_export".into();
        encode(&record)
    }
    pub fn stop(&mut self, reason: String) -> Result<(), CodingError> {
        if self.state.phase == Phase::Stopped {
            return Ok(());
        }
        self.reserved = self.reserved.max(1);
        self.append(Record::Stopped { reason })
    }
    pub fn publish(
        &mut self,
        worker_joined: bool,
        transports_joined: bool,
        cancellation: &EditCancellation,
    ) -> Result<Vec<u8>, CodingError> {
        self.guard()?;
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        self.reserved = self.reserved.max(2);
        self.append(Record::Final {
            generation: self.state.generation,
            worker_joined,
            transports_joined,
        })?;
        let export = self.state.export();
        let bytes = encode(&export)?;
        if self
            .used_bytes()?
            .checked_add(bytes.len() + RECORD_BYTES)
            .is_none_or(|n| n > RETAINED_BYTES)
        {
            return Err(CodingError::ResourceLimit(
                "complete export retained resource limit",
            ));
        }
        let path = format!("export-{}.json", self.state.generation);
        let file = File::from(openat(
            &self.directory,
            path.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        crate::project::validate_stage(&file, 0o600)?;
        (&file).write_all(&bytes)?;
        file.sync_all()?;
        self.directory.sync_all()?;
        self.retained += bytes.len();
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        self.append(Record::Export {
            generation: self.state.generation,
            path: path.clone(),
        })?;
        let artifact = open_file(&self.directory, &path, false)?;
        let acquired = read_bytes(&artifact, RECORD_BYTES)?;
        require(acquired == bytes)?;
        Ok(acquired)
    }
}
