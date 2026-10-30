use crate::{
    Outcome, ProviderCapture, ProviderInput, ProviderRequest, ProviderResponse, ProviderTool,
    Runtime,
    http::{
        provider_metadata::{self, MetadataEvidence},
        responses::bounded_json_bytes,
    },
    runtime::provider_history::{self, ProviderLedger, ValidationCheckpoint},
};
use rustix::fs::{Mode, OFlags, openat};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    net::SocketAddr,
    os::{fd::IntoRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const LIVE_STORAGE: &str = "/home/user/.local/state/scramjet-rs/s4-14-live";
const LIVE_HASH: &str = "b730fd0c2dc82bc4e9d26c9417d267a535609abe7c8caad73606013e2a8b77c9";
const TEXT_PROMPT: &str = "Reply with exactly synthetic-smoke. This is a transport test.";
const CALL_PROMPT: &str = "Call inert exactly once with message synthetic-smoke. This is a transport test; no tool will execute.";
const REPORT_BYTES: usize = 4096;
const METADATA_BYTES: usize = 131072;
const BINARY_BYTES: u64 = 256 * 1024 * 1024;

fn refused(reason: &'static str) -> String {
    reason.into()
}
fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn live_hash() -> [u8; 32] {
    std::array::from_fn(|i| {
        u8::from_str_radix(&LIVE_HASH[i * 2..i * 2 + 2], 16).expect("fixed digest")
    })
}
fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, String> {
    bounded_json_bytes(value, limit)
        .map_err(|_| refused("validation evidence resource refusal; no retry"))
}
fn create(directory: &File, name: &str) -> Result<File, String> {
    let file = File::from(
        openat(
            directory,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| refused("exclusive validation artifact refused"))?,
    );
    crate::project::validate_stage(&file, 0o600)
        .map_err(|_| refused("private validation artifact refused"))?;
    Ok(file)
}
fn subdirectory(directory: &File, name: &str) -> Result<File, String> {
    rustix::fs::mkdirat(directory, name, Mode::RWXU)
        .map_err(|_| refused("exclusive validation directory refused"))?;
    Ok(File::from(
        openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| refused("validation directory open failed"))?,
    ))
}
unsafe extern "C" {
    fn close(fd: std::os::fd::RawFd) -> std::os::raw::c_int;
}
fn close_checked(file: File) -> Result<(), String> {
    // close may release the descriptor even when reporting failure.
    if unsafe { close(file.into_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(refused("validation checked-close failed; no retry"))
    }
}
fn persist(mut file: File, bytes: &[u8]) -> Result<(), String> {
    crate::project::validate_stage(&file, 0o600)
        .map_err(|_| refused("validation artifact binding refused"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| refused("validation artifact write/sync failed; no retry"))?;
    close_checked(file)
}
fn binary_hash() -> Result<[u8; 32], String> {
    let mut file = File::open("/proc/self/exe")
        .map_err(|_| refused("executing binary identity unavailable"))?;
    let before = file
        .metadata()
        .map_err(|_| refused("executing binary stat failed"))?;
    if !before.is_file() || before.len() > BINARY_BYTES {
        return Err(refused("executing binary identity budget refused"));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    let mut read = 0;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| refused("executing binary hash failed"))?;
        if count == 0 {
            break;
        }
        read += count as u64;
        if read > BINARY_BYTES {
            return Err(refused("executing binary identity budget refused"));
        }
        digest.update(&buffer[..count]);
    }
    let after = file
        .metadata()
        .map_err(|_| refused("executing binary stat failed"))?;
    if read != before.len()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err(refused("executing binary changed"));
    }
    Ok(digest.finalize().into())
}
fn source_hash() -> [u8; 32] {
    let sources: [(&str, &[u8]); 10] = [
        ("Cargo.toml", include_bytes!("../Cargo.toml")),
        ("Cargo.lock", include_bytes!("../Cargo.lock")),
        ("src/lib.rs", include_bytes!("lib.rs")),
        ("src/main.rs", include_bytes!("main.rs")),
        (
            "src/provider_validation.rs",
            include_bytes!("provider_validation.rs"),
        ),
        ("src/runtime.rs", include_bytes!("runtime.rs")),
        (
            "src/runtime/provider_history.rs",
            include_bytes!("runtime/provider_history.rs"),
        ),
        ("src/http/provider.rs", include_bytes!("http/provider.rs")),
        ("src/http/responses.rs", include_bytes!("http/responses.rs")),
        (
            "src/http/provider_metadata.rs",
            include_bytes!("http/provider_metadata.rs"),
        ),
    ];
    let mut digest = Sha256::new();
    for (name, bytes) in sources {
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    digest.finalize().into()
}
fn file_id(file: &File) -> Result<[u64; 2], String> {
    let metadata = file
        .metadata()
        .map_err(|_| refused("evidence identity unavailable"))?;
    Ok([metadata.dev(), metadata.ino()])
}
struct EvidenceBinding {
    name: &'static str,
    id: [u64; 2],
    limit: usize,
    content: Option<([u8; 32], usize)>,
}
impl EvidenceBinding {
    fn new(name: &'static str, file: &File, limit: usize) -> Result<Self, String> {
        Ok(Self {
            name,
            id: file_id(file)?,
            limit,
            content: None,
        })
    }
    fn written(&mut self, bytes: &[u8]) {
        self.content = Some((hash(bytes), bytes.len()));
    }
    fn verify(&self, directory: &File) -> Result<(), String> {
        let mut file = File::from(
            openat(
                directory,
                self.name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| refused("named evidence artifact unavailable"))?,
        );
        crate::project::validate_stage(&file, 0o600)
            .map_err(|_| refused("named evidence permissions refused"))?;
        if file_id(&file)? != self.id {
            return Err(refused("named evidence identity changed"));
        }
        let (expected, length) = self.content.unwrap_or((hash(&[]), 0));
        if length > self.limit
            || file
                .metadata()
                .map_err(|_| refused("named evidence stat failed"))?
                .len()
                != length as u64
        {
            return Err(refused("named evidence length changed"));
        }
        let mut digest = Sha256::new();
        let mut buffer = [0; 8192];
        let mut total = 0;
        loop {
            let n = file
                .read(&mut buffer)
                .map_err(|_| refused("named evidence read failed"))?;
            if n == 0 {
                break;
            }
            total += n;
            if total > length {
                return Err(refused("named evidence content changed"));
            }
            digest.update(&buffer[..n]);
        }
        let actual: [u8; 32] = digest.finalize().into();
        if total != length
            || actual != expected
            || file_id(&file)? != self.id
            || file
                .metadata()
                .map_err(|_| refused("named evidence stat failed"))?
                .len()
                != length as u64
        {
            return Err(refused("named evidence content changed"));
        }
        Ok(())
    }
}
struct AttemptBinding {
    name: &'static str,
    id: [u64; 2],
    files: [EvidenceBinding; 4],
}
struct AttemptFiles {
    directory: File,
    request: Option<File>,
    raw: File,
    diagnostic: File,
    provenance: File,
}
struct Artifacts {
    parent: File,
    directory: File,
    attempts: [Option<AttemptFiles>; 2],
    checkpoint: Option<File>,
    metadata: [Option<File>; 3],
    summary: Option<File>,
    binary: [u8; 32],
    path: PathBuf,
    bindings: [AttemptBinding; 2],
    root_bindings: [EvidenceBinding; 5],
}
impl Artifacts {
    fn verify(&self) -> Result<(), String> {
        let named = provider_history::directory(&self.path)
            .map_err(|_| refused("named evidence directory unavailable"))?;
        if file_id(&named)? != file_id(&self.directory)? {
            return Err(refused("named evidence directory changed"));
        }
        for binding in &self.root_bindings {
            binding.verify(&self.directory)?;
        }
        for binding in &self.bindings {
            let directory = File::from(
                openat(
                    &self.directory,
                    binding.name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| refused("named attempt directory unavailable"))?,
            );
            crate::project::validate_directory(&directory, true)
                .map_err(|_| refused("named attempt directory permissions refused"))?;
            if file_id(&directory)? != binding.id {
                return Err(refused("named attempt directory changed"));
            }
            for file in &binding.files {
                file.verify(&directory)?;
            }
        }
        Ok(())
    }
    fn prepare(path: &Path) -> Result<Self, String> {
        let binary = binary_hash()?;
        let parent = provider_history::directory(
            path.parent()
                .ok_or_else(|| refused("absolute private evidence parent required"))?,
        )
        .map_err(|_| refused("private evidence parent refused"))?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| refused("validation directory name refused"))?;
        let directory = subdirectory(&parent, name)?;
        let attempts = ["first", "second"].map(|name| -> Result<_, String> {
            let directory = subdirectory(&directory, name)?;
            let files = AttemptFiles {
                request: Some(create(&directory, "request.json")?),
                raw: create(&directory, "response.data")?,
                diagnostic: create(&directory, "diagnostic.json")?,
                provenance: create(&directory, "provenance.json")?,
                directory,
            };
            files
                .directory
                .sync_all()
                .map_err(|_| refused("validation setup sync failed"))?;
            Ok(Some(files))
        });
        let [first, second] = attempts;
        let first = first?;
        let second = second?;
        let binding = |name, files: &AttemptFiles| -> Result<AttemptBinding, String> {
            Ok(AttemptBinding {
                name,
                id: file_id(&files.directory)?,
                files: [
                    EvidenceBinding::new("request.json", files.request.as_ref().unwrap(), 16384)?,
                    EvidenceBinding::new(
                        "response.data",
                        &files.raw,
                        crate::PROVIDER_CAPTURE_BYTES,
                    )?,
                    EvidenceBinding::new("diagnostic.json", &files.diagnostic, REPORT_BYTES)?,
                    EvidenceBinding::new("provenance.json", &files.provenance, REPORT_BYTES)?,
                ],
            })
        };
        let bindings = [
            binding("first", first.as_ref().unwrap())?,
            binding("second", second.as_ref().unwrap())?,
        ];
        let checkpoint = create(&directory, "checkpoint.json")?;
        let status = create(&directory, "status.json")?;
        let models = create(&directory, "models.json")?;
        let cards = create(&directory, "cards.json")?;
        let summary = create(&directory, "summary.json")?;
        let root_bindings = [
            EvidenceBinding::new("checkpoint.json", &checkpoint, REPORT_BYTES)?,
            EvidenceBinding::new("status.json", &status, METADATA_BYTES)?,
            EvidenceBinding::new("models.json", &models, METADATA_BYTES)?,
            EvidenceBinding::new("cards.json", &cards, METADATA_BYTES)?,
            EvidenceBinding::new("summary.json", &summary, REPORT_BYTES)?,
        ];
        let result = Self {
            checkpoint: Some(checkpoint),
            metadata: [Some(status), Some(models), Some(cards)],
            summary: Some(summary),
            parent,
            directory,
            attempts: [first, second],
            binary,
            bindings,
            root_bindings,
            path: path.into(),
        };
        result
            .directory
            .sync_all()
            .and_then(|()| result.parent.sync_all())
            .map_err(|_| refused("validation setup sync failed"))?;
        result.verify()?;
        Ok(result)
    }
    fn request(&mut self, index: usize, request: &ProviderRequest) -> Result<[u8; 32], String> {
        self.verify()?;
        let bytes = request.encoded.as_bytes();
        let digest = hash(bytes);
        persist(
            self.attempts[index]
                .as_mut()
                .unwrap()
                .request
                .take()
                .unwrap(),
            bytes,
        )?;
        self.attempts[index]
            .as_ref()
            .unwrap()
            .directory
            .sync_all()
            .map_err(|_| refused("request directory sync failed"))?;
        self.bindings[index].files[0].written(bytes);
        self.verify()?;
        Ok(digest)
    }
    fn baseline(&mut self, checkpoint: &ValidationCheckpoint) -> Result<(), String> {
        let bytes = encode(checkpoint, REPORT_BYTES)?;
        persist(self.checkpoint.take().unwrap(), &bytes)?;
        self.root_bindings[0].written(&bytes);
        self.verify()
    }
    fn metadata(&mut self, evidence: &MetadataEvidence) -> Result<(), String> {
        for (index, body) in [&evidence.status, &evidence.models, &evidence.cards]
            .into_iter()
            .enumerate()
        {
            let bytes = encode(body, METADATA_BYTES)?;
            persist(self.metadata[index].take().unwrap(), &bytes)?;
            self.root_bindings[index + 1].written(&bytes);
        }
        self.directory
            .sync_all()
            .map_err(|_| refused("metadata evidence sync failed"))?;
        self.verify()
    }
    fn response(
        &mut self,
        index: usize,
        capture: ProviderCapture,
        response: &ProviderResponse,
        report: &AttemptReport,
    ) -> Result<(), String> {
        let files = self.attempts[index].take().unwrap();
        if files.request.is_some() {
            return Err(refused("request was not durably captured"));
        }
        let diagnostic = json!({"version":1,"outcome":format!("{:?}",response.outcome),"durable":response.durable,"failure":response.failure});
        let provenance = json!({"version":1,"representation":"client-visible-http-response-DATA","binary_sha256":hex(&self.binary),"source_components_sha256":hex(&source_hash()),"attempt":report});
        persist(files.raw, &capture.bytes)?;
        self.bindings[index].files[1].written(&capture.bytes);
        let diagnostic = encode(&diagnostic, REPORT_BYTES)?;
        persist(files.diagnostic, &diagnostic)?;
        self.bindings[index].files[2].written(&diagnostic);
        let provenance = encode(&provenance, REPORT_BYTES)?;
        persist(files.provenance, &provenance)?;
        self.bindings[index].files[3].written(&provenance);
        files
            .directory
            .sync_all()
            .map_err(|_| refused("response evidence directory sync failed"))?;
        close_checked(files.directory)?;
        self.verify()
    }
    fn finish(mut self, summary: &PairSummary) -> Result<(), String> {
        self.verify()?;
        for files in &mut self.attempts {
            if let Some(files) = files.take() {
                if let Some(request) = files.request {
                    close_checked(request)?;
                }
                close_checked(files.raw)?;
                close_checked(files.diagnostic)?;
                close_checked(files.provenance)?;
                close_checked(files.directory)?;
            }
        }
        let bytes = encode(summary, REPORT_BYTES)?;
        persist(self.summary.take().unwrap(), &bytes)?;
        self.root_bindings[4].written(&bytes);
        self.root_bindings[4].verify(&self.directory)?;
        self.directory
            .sync_all()
            .map_err(|_| refused("pair summary sync failed"))?;
        self.verify()?;
        close_checked(self.directory)?;
        close_checked(self.parent)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptReport {
    request_id: u64,
    ordinal: Option<usize>,
    request_sha256: String,
    data_sha256: String,
    terminal: Option<crate::ProviderTerminalEvidence>,
    usage: Option<crate::ProviderUsage>,
    durable: bool,
    body_eof: bool,
    parser_accepted: bool,
    local_joined: bool,
    overflow: bool,
    received_bytes: usize,
    captured_bytes: usize,
    parser_fed_bytes: usize,
    passed: bool,
}
#[derive(Serialize)]
struct AttemptReference {
    request_id: u64,
    ordinal: Option<usize>,
    request_sha256: String,
    data_sha256: String,
    passed: bool,
    durable: bool,
    evidence_persisted: bool,
}
#[derive(Serialize)]
struct PairSummary {
    version: u8,
    route: &'static str,
    binary_sha256: String,
    source_components_sha256: String,
    baseline: ValidationCheckpoint,
    after_sha256: Option<String>,
    after_bytes: Option<usize>,
    consumed: Option<usize>,
    remaining: Option<usize>,
    new_uncertainty: Option<bool>,
    unresolved_remote: Option<usize>,
    unknown_usage: Option<usize>,
    acknowledged_consumed: usize,
    accounting_observed: bool,
    local_active: usize,
    active_peak: usize,
    attempts: [Option<AttemptReference>; 2],
    metadata: [Option<serde_json::Value>; 2],
    stop: Option<String>,
    checks_passed: bool,
    producer_acknowledgement: &'static str,
}
fn tools() -> [ProviderTool; 1] {
    [ProviderTool {
        name: "inert".into(),
        description: "Return an inert synthetic envelope; no effects execute.".into(),
        parameters: json!({"type":"object","properties":{"message":{"type":"string"}},"required":["message"],"additionalProperties":false}),
    }]
}
fn accepted(
    index: usize,
    response: &ProviderResponse,
    capture: &ProviderCapture,
) -> Result<(), String> {
    if response.outcome != Outcome::Success
        || !response.durable
        || response.reservation != Some(5 + index)
        || response
            .usage
            .as_ref()
            .is_none_or(|u| !u.complete() || u.output_tokens.is_none_or(|n| n > 1024))
        || response
            .remote_terminal
            .as_ref()
            .is_none_or(|t| t.status != "completed")
        || capture.id != response.id
        || capture.reservation != response.reservation
        || !capture.local_joined
        || !capture.body_eof
        || !capture.parser_accepted
        || capture.overflow
        || capture.received_bytes != capture.bytes.len()
        || capture.parser_fed_bytes != capture.received_bytes
        || capture.bytes.len() > crate::PROVIDER_CAPTURE_BYTES
    {
        return Err(refused(
            "pair completion/usage/capture gate failed; no retry",
        ));
    }
    let envelope = response
        .envelope
        .as_ref()
        .ok_or_else(|| refused("pair envelope unavailable"))?;
    let calls: Vec<_> = envelope
        .items
        .iter()
        .filter_map(|i| i.function_call())
        .collect();
    if index == 0 {
        let text: String = envelope.items.iter().filter_map(|i| i.text()).collect();
        if !calls.is_empty() || text != "synthetic-smoke" {
            return Err(refused("pair aggregate text differs; no retry"));
        }
    } else if calls.len() != 1
        || calls[0].name != "inert"
        || serde_json::from_str::<serde_json::Value>(calls[0].arguments).ok()
            != Some(json!({"message":"synthetic-smoke"}))
    {
        return Err(refused("pair inert call differs; no retry"));
    }
    Ok(())
}
struct Signals {
    stopped: Arc<AtomicBool>,
    ids: Vec<signal_hook::SigId>,
}
impl Signals {
    fn new() -> Result<Self, String> {
        let mut result = Self {
            stopped: Arc::new(AtomicBool::new(false)),
            ids: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            result.ids.push(
                signal_hook::flag::register(signal, result.stopped.clone())
                    .map_err(|_| refused("validation cancellation registration failed"))?,
            );
        }
        Ok(result)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

async fn run_pair(
    endpoint: SocketAddr,
    storage: PathBuf,
    expected: [u8; 32],
    evidence: PathBuf,
    stopped: Arc<AtomicBool>,
    live: bool,
) -> Result<Vec<u8>, String> {
    let mut artifacts = tokio::task::spawn_blocking(move || Artifacts::prepare(&evidence))
        .await
        .map_err(|_| refused("validation artifact worker failed"))??;
    let ledger = tokio::task::spawn_blocking(move || {
        ProviderLedger::validation_open(&storage, endpoint, expected)
    })
    .await
    .map_err(|_| refused("validation ledger worker failed"))?
    .map_err(|_| refused("validation existing baseline/ownership refused; no dispatch"))?;
    ledger
        .lock()
        .unwrap()
        .validation_cancellation(stopped.clone())
        .map_err(|_| refused("validation cancellation binding refused"))?;
    let checkpoint = ledger
        .lock()
        .unwrap()
        .validation_checkpoint()
        .map_err(|_| refused("validation checkpoint refused"))?;
    let runtime = Runtime::provider_validation(endpoint, ledger.clone())
        .map_err(|_| refused("validation runtime ownership refused"))?;
    let mut summary = PairSummary {
        version: 1,
        route: if live {
            "proxy-responses"
        } else {
            "fixture-responses"
        },
        binary_sha256: hex(&artifacts.binary),
        source_components_sha256: hex(&source_hash()),
        baseline: checkpoint.clone(),
        after_sha256: Some(hex(&expected)),
        after_bytes: Some(checkpoint.bytes),
        consumed: Some(4),
        remaining: Some(28),
        new_uncertainty: Some(false),
        unresolved_remote: Some(4),
        unknown_usage: Some(4),
        acknowledged_consumed: 4,
        accounting_observed: true,
        local_active: 0,
        active_peak: 0,
        attempts: [None, None],
        metadata: [None, None],
        stop: None,
        checks_passed: false,
        producer_acknowledgement: "unknown",
    };
    let outcome = drive(
        &runtime,
        &ledger,
        &mut artifacts,
        &mut summary,
        endpoint,
        &stopped,
    )
    .await;
    if let Err(reason) = outcome {
        summary.stop = Some(reason);
    }
    let stats = runtime.shutdown().await;
    summary.local_active = stats.active;
    summary.active_peak = stats.active_peak;
    let observed = ledger.lock().unwrap().validation_observe();
    match observed {
        Ok((history, digest, length)) => {
            summary.consumed = Some(history.consumed());
            summary.remaining = Some(history.remaining());
            summary.unresolved_remote = Some(history.unresolved_remote());
            summary.unknown_usage = Some(history.unknown_usage());
            summary.new_uncertainty = Some(history.has_unknown_after(4));
            summary.after_sha256 = Some(hex(&digest));
            summary.after_bytes = Some(length);
            summary.acknowledged_consumed = history.consumed();
        }
        Err(_) => {
            let history = ledger.lock().unwrap().history();
            summary.acknowledged_consumed = history.consumed();
            summary.consumed = None;
            summary.remaining = None;
            summary.unresolved_remote = None;
            summary.unknown_usage = None;
            summary.new_uncertainty = None;
            summary.after_sha256 = None;
            summary.after_bytes = None;
            summary.accounting_observed = false;
            if summary.stop.is_none() {
                summary.stop = Some(refused(
                    "post-run content/accounting observation refused; no retry",
                ));
            }
        }
    }
    summary.checks_passed = summary.stop.is_none()
        && summary.consumed == Some(6)
        && summary.unresolved_remote == Some(4)
        && summary.unknown_usage == Some(4)
        && summary.new_uncertainty == Some(false)
        && summary.local_active == 0
        && summary.active_peak <= 1;
    let mut sanitized = json!({"report":"provider-validation-pair","version":1,"success":summary.checks_passed,"consumed":summary.consumed,"remaining":summary.remaining,"unresolved_remote":summary.unresolved_remote,"unknown_usage":summary.unknown_usage,"new_uncertainty":summary.new_uncertainty,"accounting_observed":summary.accounting_observed,"acknowledged_consumed":summary.acknowledged_consumed,"local_active":summary.local_active,"active_peak":summary.active_peak,"binary_sha256":summary.binary_sha256,"source_components_sha256":summary.source_components_sha256,"after_sha256":summary.after_sha256,"stop":summary.stop,"summary_durable":true});
    let persisted = tokio::task::spawn_blocking(move || artifacts.finish(&summary)).await;
    if !matches!(persisted, Ok(Ok(()))) {
        sanitized["success"] = json!(false);
        sanitized["summary_durable"] = json!(false);
        sanitized["evidence_stop"] = json!("validation summary persistence failed; no retry");
    }
    drop(ledger);
    encode(&sanitized, REPORT_BYTES)
}
async fn drive(
    runtime: &Runtime,
    ledger: &Arc<Mutex<ProviderLedger>>,
    artifacts: &mut Artifacts,
    summary: &mut PairSummary,
    endpoint: SocketAddr,
    stopped: &AtomicBool,
) -> Result<(), String> {
    artifacts.baseline(&summary.baseline)?;
    let mut context = vec![ProviderInput::User(TEXT_PROMPT.into())];
    let mut predicates = None;
    for index in 0..2 {
        if stopped.load(Ordering::SeqCst) {
            return Err(refused("pair cancelled; no further dispatch"));
        }
        let metadata_expiry = Instant::now() + Duration::from_secs(5);
        ledger
            .lock()
            .unwrap()
            .validation_metadata_expiry(metadata_expiry)
            .map_err(|_| refused("pair metadata expiry binding refused; no dispatch"))?;
        let metadata = provider_metadata::observe_controlled(endpoint, Some(stopped))
            .await
            .map_err(|_| refused("pair metadata readiness refused; no further dispatch"))?;
        let current = metadata.predicates();
        if predicates.as_ref().is_some_and(|prior| prior != &current) {
            return Err(refused(
                "pair metadata predicates changed; no further dispatch",
            ));
        }
        let meta = json!({"representation":"canonical-metadata-json","observed_unix_ms":metadata.observed_unix_ms,"predicates_sha256":hex(&hash(&encode(&current,REPORT_BYTES)?)),"status_sha256":hex(&hash(&encode(&metadata.status,METADATA_BYTES)?)),"models_sha256":hex(&hash(&encode(&metadata.models,METADATA_BYTES)?)),"cards_sha256":hex(&hash(&encode(&metadata.cards,METADATA_BYTES)?))});
        summary.metadata[index] = Some(meta);
        if index == 0 {
            artifacts.metadata(&metadata)?;
        }
        predicates = Some(current);
        drop(metadata);
        let definitions = tools();
        let request = ProviderRequest::new(&context, if index == 0 { &[] } else { &definitions })
            .map_err(|_| {
            refused("complete pair context resource refusal; no further dispatch")
        })?;
        let request_hash = artifacts.request(index, &request)?;
        if Instant::now() >= metadata_expiry || stopped.load(Ordering::SeqCst) {
            return Err(refused("pair metadata stale or cancelled; no dispatch"));
        }
        {
            let owner = ledger.lock().unwrap();
            owner
                .validation_checkpoint()
                .map_err(|_| refused("pair ledger content/binding refused; no dispatch"))?;
            if !owner.eligible() || owner.history_ref().consumed() != 4 + index {
                return Err(refused("pair frozen admission refused; no dispatch"));
            }
        }
        artifacts.verify()?;
        if Instant::now() >= metadata_expiry || stopped.load(Ordering::SeqCst) {
            return Err(refused("pair metadata stale or cancelled before admission"));
        }
        let (mut handle, receiver) = runtime
            .submit_provider_captured(request)
            .map_err(|_| refused("pair submission refused; no retry"))?;
        if stopped.load(Ordering::SeqCst) {
            handle.cancel();
        }
        let response = loop {
            tokio::select! {
                result=handle.result()=>break result.ok_or_else(||refused("pair local result lost; no retry"))?,
                ()=tokio::time::sleep(Duration::from_millis(10))=>{if stopped.load(Ordering::SeqCst){handle.cancel();}}
            }
        };
        drop(handle);
        let capture = receiver
            .receive()
            .await
            .ok_or_else(|| refused("pair capture lost; no retry"))?;
        let gate = accepted(index, &response, &capture);
        let report = AttemptReport {
            request_id: response.id.0,
            ordinal: response.reservation,
            request_sha256: hex(&request_hash),
            data_sha256: hex(&hash(&capture.bytes)),
            terminal: response.remote_terminal.clone(),
            usage: response.usage.clone(),
            durable: response.durable,
            body_eof: capture.body_eof,
            parser_accepted: capture.parser_accepted,
            local_joined: capture.local_joined,
            overflow: capture.overflow,
            received_bytes: capture.received_bytes,
            captured_bytes: capture.bytes.len(),
            parser_fed_bytes: capture.parser_fed_bytes,
            passed: gate.is_ok(),
        };
        summary.attempts[index] = Some(AttemptReference {
            request_id: report.request_id,
            ordinal: report.ordinal,
            request_sha256: report.request_sha256.clone(),
            data_sha256: report.data_sha256.clone(),
            passed: report.passed,
            durable: report.durable,
            evidence_persisted: false,
        });
        artifacts.response(index, capture, &response, &report)?;
        summary.attempts[index].as_mut().unwrap().evidence_persisted = true;
        gate?;
        {
            let mut owner = ledger.lock().unwrap();
            let (history, _, _) = owner
                .validation_observe()
                .map_err(|_| refused("pair joined ledger observation refused; no retry"))?;
            let attempt = history
                .attempts
                .last()
                .ok_or_else(|| refused("pair correlated fact missing"))?;
            if history.consumed() != 5 + index
                || history.has_unknown_after(4)
                || !attempt.locally_joined
                || attempt.request_id != response.id.0
                || attempt.terminal != response.remote_terminal
                || attempt.usage != response.usage
            {
                return Err(refused(
                    "pair joined accounting correlation refused; no retry",
                ));
            }
        }
        if index == 0 {
            context.extend(
                response
                    .envelope
                    .unwrap()
                    .items
                    .into_iter()
                    .map(ProviderInput::Item),
            );
            context.push(ProviderInput::User(CALL_PROMPT.into()));
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    if stopped.load(Ordering::SeqCst) {
        return Err(refused("pair cancelled before final evidence"));
    }
    Ok(())
}

pub fn provider_validation_live_pair(evidence: &Path) -> Result<Vec<u8>, String> {
    let signals = Signals::new()?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| refused("validation executor unavailable"))?;
    executor.block_on(run_pair(
        "127.0.0.1:18080".parse().unwrap(),
        PathBuf::from(LIVE_STORAGE),
        live_hash(),
        evidence.into(),
        signals.stopped.clone(),
        true,
    ))
}

fn read_checkpoint(evidence: &Path) -> Result<ValidationCheckpoint, String> {
    let directory = provider_history::directory(evidence)
        .map_err(|_| refused("private observation directory refused"))?;
    let mut file = File::from(
        openat(
            &directory,
            "checkpoint.json",
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| refused("validation checkpoint artifact unavailable"))?,
    );
    crate::project::validate_stage(&file, 0o600)
        .map_err(|_| refused("private checkpoint artifact refused"))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(REPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| refused("checkpoint read failed"))?;
    if bytes.len() > REPORT_BYTES {
        return Err(refused("checkpoint observation budget refused"));
    }
    let checkpoint: ValidationCheckpoint = serde_json::from_value(
        crate::http::responses::bounded_json_value(&bytes, REPORT_BYTES)
            .map_err(|_| refused("checkpoint malformed"))?,
    )
    .map_err(|_| refused("checkpoint malformed"))?;
    Ok(checkpoint)
}
fn observe_at(storage: &Path, checkpoint: &ValidationCheckpoint) -> Result<Vec<u8>, String> {
    let history = provider_history::observe_validation(storage, checkpoint)
        .map_err(|_| refused("validation read-only observation refused"))?;
    encode(
        &json!({"report":"provider-validation-observation","consumed":history.consumed(),"remaining":history.remaining(),"unresolved_remote":history.unresolved_remote(),"unknown_usage":history.unknown_usage(),"new_uncertainty":history.has_unknown_after(4),"dispatch_authority":false,"producer_acknowledgement":"unknown"}),
        REPORT_BYTES,
    )
}
pub fn provider_validation_observe(evidence: &Path) -> Result<Vec<u8>, String> {
    let checkpoint = read_checkpoint(evidence)?;
    if checkpoint.sha256 != live_hash()
        || checkpoint.binding.endpoint != "127.0.0.1:18080".parse::<SocketAddr>().unwrap()
    {
        return Err(refused("foreign validation checkpoint refused"));
    }
    observe_at(Path::new(LIVE_STORAGE), &checkpoint)
}
pub fn provider_validation_fixture_observe(
    storage: &Path,
    evidence: &Path,
) -> Result<Vec<u8>, String> {
    let checkpoint = read_checkpoint(evidence)?;
    if !checkpoint.binding.endpoint.ip().is_loopback()
        || checkpoint.binding.endpoint.port() == 18080
    {
        return Err(refused("fixture observation endpoint refused"));
    }
    observe_at(storage, &checkpoint)
}

use crate::provider_fixture as fixture;
#[path = "provider_validation_fixture.rs"]
mod fixture_driver;
pub fn provider_validation_fixture_pair(
    storage: &Path,
    evidence: &Path,
) -> Result<Vec<u8>, String> {
    let signals = Signals::new()?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| refused("validation executor unavailable"))?;
    executor.block_on(fixture_driver::run(
        storage,
        evidence,
        signals.stopped.clone(),
    ))
}
#[cfg(test)]
#[path = "provider_validation_meter.rs"]
mod meter;
#[cfg(test)]
#[path = "provider_validation_tests.rs"]
mod tests;
