pub(crate) mod agent;
mod history;
mod recovery;
pub use recovery::reconstruct_haiku_history;
#[cfg(test)]
mod fault;
mod journal;
mod json;
mod observation;
mod process;
pub use history::{
    HaikuAttempt, HaikuCertainty, HaikuFact, HaikuHistory, HaikuHistoryEntry, HaikuOutcome,
};
pub use journal::Record as HaikuRecord;
pub use observation::{HaikuControl, HaikuObserver, HaikuPhase, HaikuSnapshot};

pub use super::captured_files::CapturedFile as HaikuCapture;
use super::{
    CodingError, EditCancellation,
    captured_files::{FILE_BYTES, FileSlots},
};
use crate::http::responses::bounded_json_bytes;
use crate::{
    Outcome, ProjectWorkspace, ProviderInput, ProviderRequest, ProviderResponse, ProviderTool,
    RequestId,
};
use rustix::fs::fstat;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs::File, io::Write, path::Path, sync::Arc};

pub const HAIKU_SCRIPT: &str = "from pathlib import Path\nimport sys\nsys.stdout.buffer.write(Path('haiku.md').read_bytes())\n";
pub const HAIKU_COMMAND: &str = "exec /usr/bin/python3 -I print_haiku.py";
const NAMES: [&str; 2] = ["haiku.md", "print_haiku.py"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum HaikuEffect {
    Create {
        path: String,
        content: String,
        mode: u32,
    },
    Process {
        command: String,
        captures: [HaikuCapture; 2],
        profile: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaikuIntent {
    pub session: String,
    pub root: [u64; 2],
    pub operation: usize,
    pub request_id: u64,
    pub ordinal: usize,
    pub response_id: String,
    pub call_id: String,
    pub effect: HaikuEffect,
}

#[derive(Clone, Debug)]
pub struct HaikuProposal {
    intent: Arc<HaikuIntent>,
    owner: Arc<()>,
}
impl HaikuProposal {
    pub fn intent(&self) -> &HaikuIntent {
        &self.intent
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaikuProcessResult {
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub stopped: Option<String>,
    pub launcher_reaped: bool,
    pub namespace_dead: bool,
}

#[derive(Debug)]
pub enum HaikuStep {
    Approval(HaikuProposal),
    Read { path: String, content: String },
    Finished,
}

pub struct HaikuSession {
    workspace: Arc<crate::project::Workspace>,
    session: String,
    owner: Arc<()>,
    journal: journal::Journal,
    history: Vec<ProviderInput>,
    responses: Vec<String>,
    attempts: usize,
    reads: usize,
    created: [Option<HaikuCapture>; 2],
    haiku_read: bool,
    process: Option<HaikuProcessResult>,
    pending: Option<HaikuProposal>,
    stopped: bool,
    request_pending: bool,
    started: std::time::Instant,
    observed: Option<Arc<observation::Shared>>,
    process_acknowledged: bool,
    #[cfg(test)]
    fault: Option<fault::Fault>,
    #[cfg(test)]
    pause: Option<fault::Pause>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    command: String,
}

fn name(path: &str) -> Result<usize, CodingError> {
    NAMES
        .iter()
        .position(|name| *name == path)
        .ok_or(CodingError::Unsupported("unsupported haiku target"))
}
fn identity(file: &File) -> Result<[u64; 2], CodingError> {
    let stat = fstat(file)?;
    Ok([stat.st_dev, stat.st_ino])
}
fn read_capture(root: &File, path: &str) -> Result<HaikuCapture, CodingError> {
    name(path)?;
    FileSlots::new(NAMES)?.capture(root, path)
}
fn absent(root: &File, path: &str) -> Result<(), CodingError> {
    name(path)?;
    FileSlots::new(NAMES)?.absent(root, path)
}

pub fn haiku_tools() -> Vec<ProviderTool> {
    let paths = json!({"type":"string","enum":NAMES});
    vec![
        ProviderTool {
            name: "write".into(),
            description: "Exclusively create an absent owned file; no overwrites".into(),
            parameters: json!({"type":"object","properties":{"path":paths,"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
        },
        ProviderTool {
            name: "read".into(),
            description: "Read an owned journey file".into(),
            parameters: json!({"type":"object","properties":{"path":paths},"required":["path"],"additionalProperties":false}),
        },
        ProviderTool {
            name: "bash".into(),
            description: "Run the reviewed captured reader in isolation".into(),
            parameters: json!({"type":"object","properties":{"command":{"type":"string","enum":[HAIKU_COMMAND]}},"required":["command"],"additionalProperties":false}),
        },
    ]
}

pub fn haiku_prompt() -> String {
    format!(
        "Create haiku.md containing a trivial haiku, read it back, create print_haiku.py with exactly these bytes: {HAIKU_SCRIPT:?}. Execute the bash command {HAIKU_COMMAND:?}, then finish. Use one tool call per response. Do not overwrite files."
    )
}

impl HaikuSession {
    pub fn create(project: &Path, storage: &Path, session: &str) -> Result<Self, CodingError> {
        let started = std::time::Instant::now();
        super::validate_session(session)?;
        let initial_root = crate::project::open_directory(project)?;
        crate::project::validate_directory(&initial_root, true)?;
        for entry in rustix::fs::Dir::read_from(&initial_root)? {
            let entry = entry?;
            if ![
                b".".as_slice(),
                b"..".as_slice(),
                crate::PROJECT_LOCK_NAME.as_bytes(),
            ]
            .contains(&entry.file_name().to_bytes())
            {
                return Err(CodingError::Invalid(
                    "haiku journey requires an empty disposable project",
                ));
            }
        }
        let workspace = ProjectWorkspace::open(project, storage)?.workspace();
        if identity(&initial_root)? != identity(&workspace.root)? {
            return Err(CodingError::Stale);
        }
        crate::project::validate_directory(&workspace.root, true)?;
        for path in NAMES {
            absent(&workspace.root, path)?;
        }
        let root = [workspace.identity.device, workspace.identity.inode];
        let journal = journal::Journal::create(&workspace.storage, storage, session, root)?;
        Ok(Self {
            workspace,
            session: session.into(),
            owner: Arc::new(()),
            journal,
            history: vec![ProviderInput::User(haiku_prompt())],
            responses: Vec::with_capacity(8),
            attempts: 0,
            reads: 0,
            created: [None, None],
            haiku_read: false,
            process: None,
            pending: None,
            stopped: false,
            request_pending: false,
            started,
            observed: None,
            process_acknowledged: false,
            #[cfg(test)]
            fault: None,
            #[cfg(test)]
            pause: None,
        })
    }

    pub fn prepare_request(&mut self) -> Result<ProviderRequest, CodingError> {
        let result = self.prepare_inner();
        if let Err(error) = &result {
            self.fail(error)?;
        }
        result
    }
    fn prepare_inner(&mut self) -> Result<ProviderRequest, CodingError> {
        if self.stopped || self.request_pending || self.pending.is_some() {
            return Err(CodingError::Stale);
        }
        if self.attempts >= 8 {
            return Err(CodingError::ResourceLimit("haiku model attempt ceiling"));
        }
        self.validate_workspace()?;
        let request =
            ProviderRequest::new(&self.history, &haiku_tools()).map_err(
                |outcome| match outcome {
                    Outcome::ResourceLimit => {
                        CodingError::ResourceLimit("complete haiku context refused; no truncation")
                    }
                    _ => CodingError::Invalid("contradictory complete haiku context"),
                },
            )?;
        self.journal.reserve_turn()?;
        self.journal.append(journal::Record::Attempt {
            turn: self.attempts + 1,
            request: request.encoded.clone(),
        })?;
        self.attempts += 1;
        self.request_pending = true;
        Ok(request)
    }

    pub fn select_response(
        &mut self,
        expected: RequestId,
        response: ProviderResponse,
    ) -> Result<HaikuStep, CodingError> {
        let result = self.select_inner(expected, response);
        if let Err(error) = &result {
            self.fail(error)?;
        }
        result
    }

    fn select_inner(
        &mut self,
        expected: RequestId,
        response: ProviderResponse,
    ) -> Result<HaikuStep, CodingError> {
        if self.stopped || !self.request_pending || self.pending.is_some() {
            return Err(CodingError::Stale);
        }
        self.request_pending = false;
        #[derive(Serialize)]
        struct ReceiptView<'a> {
            request_id: u64,
            response_request_id: u64,
            ordinal: Option<usize>,
            outcome: HaikuOutcome,
            usage: &'a Option<crate::ProviderUsage>,
            terminal: &'a Option<crate::ProviderTerminalEvidence>,
            durable: bool,
        }
        bounded_json_bytes(
            &ReceiptView {
                request_id: expected.0,
                response_request_id: response.id.0,
                ordinal: response.reservation,
                outcome: (&response.outcome).into(),
                usage: &response.usage,
                terminal: &response.remote_terminal,
                durable: response.durable,
            },
            crate::PROVIDER_FRAME_BYTES + 1024,
        )
        .map_err(|_| CodingError::ResourceLimit("provider receipt resource limit"))?;
        let attempt = HaikuAttempt {
            turn: self.attempts,
            request_id: expected.0,
            response_request_id: response.id.0,
            ordinal: response.reservation,
            outcome: (&response.outcome).into(),
            usage: response.usage.clone(),
            terminal: response.remote_terminal.clone(),
            durable: response.durable,
        };
        if let Some(observed) = &self.observed {
            observed.update(|snapshot| {
                snapshot.observed_attempt = Some(Arc::new(attempt.clone()));
                snapshot.attempt_acknowledged = false;
            });
        }
        self.journal.append(journal::Record::Receipt { attempt })?;
        if let Some(observed) = &self.observed {
            observed.update(|snapshot| snapshot.attempt_acknowledged = true);
        }
        self.validate_workspace()?;
        if response.id != expected
            || !response.durable
            || response.outcome != Outcome::Success
            || response.usage.as_ref().is_none_or(|u| !u.complete())
            || response.reservation.is_none()
            || response.remote_terminal.is_none()
        {
            return Err(match response.outcome {
                Outcome::ResourceLimit => {
                    CodingError::ResourceLimit("provider resource refusal has no effect authority")
                }
                Outcome::Unsupported => {
                    CodingError::Unsupported("provider replay refusal has no effect authority")
                }
                Outcome::Cancelled => CodingError::Cancelled,
                Outcome::Deadline => CodingError::Deadline,
                _ => CodingError::Invalid("provider completion has no haiku effect authority"),
            });
        }
        let envelope = response
            .envelope
            .ok_or(CodingError::Invalid("provider envelope unavailable"))?;
        if envelope.items.len() > crate::PROVIDER_OUTPUT_ITEMS {
            return Err(CodingError::ResourceLimit(
                "haiku selected item resource limit",
            ));
        }
        #[derive(Serialize)]
        struct EnvelopeView<'a> {
            response_id: &'a str,
            items: Vec<&'a serde_json::Value>,
        }
        let view = EnvelopeView {
            response_id: &envelope.response_id,
            items: envelope.items.iter().map(|item| item.raw()).collect(),
        };
        bounded_json_bytes(&view, crate::PROVIDER_ENVELOPE_BYTES)
            .map_err(|_| CodingError::ResourceLimit("complete selected envelope resource limit"))?;
        if self.responses.contains(&envelope.response_id) {
            return Err(CodingError::Stale);
        }
        let terminal = response.remote_terminal.as_ref().unwrap();
        if terminal.response_id != envelope.response_id || terminal.status != "completed" {
            return Err(CodingError::Invalid("foreign provider terminal"));
        }
        response
            .usage
            .as_ref()
            .unwrap()
            .validate()
            .map_err(|_| CodingError::Invalid("contradictory provider usage"))?;
        let mut next = self.history.clone();
        for item in &envelope.items {
            if next.iter().any(|previous| matches!(previous, ProviderInput::Item(old) if old.id() == item.id() || old.function_call().zip(item.function_call()).is_some_and(|(a,b)| a.call_id == b.call_id))) {
                return Err(CodingError::Stale);
            }
            next.push(ProviderInput::Item(item.clone()));
        }
        let calls: Vec<_> = envelope
            .items
            .iter()
            .filter_map(|item| item.function_call())
            .collect();
        if calls.len() > 1 {
            return Err(CodingError::Unsupported(
                "one haiku call per completed response",
            ));
        }
        self.journal.append(journal::Record::Selected {
            turn: self.attempts,
            request_id: expected.0,
            ordinal: response.reservation.unwrap(),
            response_id: envelope.response_id.clone(),
            items: envelope
                .items
                .iter()
                .map(|item| item.raw().clone())
                .collect(),
            usage: response.usage.unwrap(),
        })?;
        self.responses.push(envelope.response_id.clone());
        self.history = next;
        let Some(call) = calls.first() else {
            if !self.successful() {
                return Err(CodingError::Invalid(
                    "early final without material haiku success",
                ));
            }
            self.journal.append(journal::Record::Finished)?;
            self.stopped = true;
            return Ok(HaikuStep::Finished);
        };
        let effect = match call.name {
            "write" => {
                let args: WriteArgs = serde_json::from_str(call.arguments)
                    .map_err(|_| CodingError::Invalid("malformed haiku write arguments"))?;
                let slot = name(&args.path)?;
                if self.created[slot].is_some() || (slot == 1 && !self.haiku_read) {
                    return Err(CodingError::Invalid(
                        "repeated or out-of-order haiku creation",
                    ));
                }
                if args.content.len() > FILE_BYTES {
                    return Err(CodingError::ResourceLimit("haiku file resource limit"));
                }
                if slot == 1 && args.content != HAIKU_SCRIPT {
                    return Err(CodingError::Unsupported("unreviewed Python script"));
                }
                absent(&self.workspace.root, &args.path)?;
                HaikuEffect::Create {
                    path: args.path,
                    content: args.content,
                    mode: 0o600,
                }
            }
            "read" => {
                let args: ReadArgs = serde_json::from_str(call.arguments)
                    .map_err(|_| CodingError::Invalid("malformed haiku read arguments"))?;
                let slot = name(&args.path)?;
                if self.reads >= 3 || self.created[slot].is_none() {
                    return Err(CodingError::Invalid("haiku read allowance or order"));
                }
                self.validate_workspace()?;
                let capture = read_capture(&self.workspace.root, &args.path)?;
                if Some(&capture) != self.created[slot].as_ref() {
                    return Err(CodingError::TargetMismatch);
                }
                let output = serde_json::to_string(
                    &json!({"status":"read","path":args.path,"content":capture.content}),
                )
                .unwrap();
                self.record_result(call.call_id, output)?;
                self.reads += 1;
                self.haiku_read |= slot == 0;
                return Ok(HaikuStep::Read {
                    path: args.path,
                    content: capture.content,
                });
            }
            "bash" => {
                let args: BashArgs = serde_json::from_str(call.arguments)
                    .map_err(|_| CodingError::Invalid("malformed haiku bash arguments"))?;
                if args.command != HAIKU_COMMAND || self.process.is_some() || !self.haiku_read {
                    return Err(CodingError::Invalid(
                        "unsupported or repeated haiku process",
                    ));
                }
                let captures = [
                    self.created[0].clone().ok_or(CodingError::Stale)?,
                    self.created[1].clone().ok_or(CodingError::Stale)?,
                ];
                self.recheck_captures(&captures)?;
                process::check_host()?;
                HaikuEffect::Process {
                    command: args.command,
                    captures,
                    profile: process::PROFILE.into(),
                }
            }
            _ => return Err(CodingError::Unsupported("unsupported haiku tool")),
        };
        let proposal = HaikuProposal {
            intent: Arc::new(HaikuIntent {
                session: self.session.clone(),
                root: [
                    self.workspace.identity.device,
                    self.workspace.identity.inode,
                ],
                operation: self.attempts,
                request_id: expected.0,
                ordinal: response.reservation.unwrap(),
                response_id: envelope.response_id,
                call_id: call.call_id.into(),
                effect,
            }),
            owner: self.owner.clone(),
        };
        self.pending = Some(proposal.clone());
        Ok(HaikuStep::Approval(proposal))
    }

    pub fn decide(
        &mut self,
        proposal: HaikuProposal,
        approve: bool,
        cancellation: &EditCancellation,
    ) -> Result<(), CodingError> {
        let result = self.decide_inner(proposal, approve, cancellation);
        if let Err(error) = &result {
            self.fail(error)?;
        }
        result
    }
    fn decide_inner(
        &mut self,
        proposal: HaikuProposal,
        approve: bool,
        cancellation: &EditCancellation,
    ) -> Result<(), CodingError> {
        if self.stopped
            || !Arc::ptr_eq(&proposal.owner, &self.owner)
            || self
                .pending
                .as_ref()
                .is_none_or(|current| current.intent != proposal.intent)
        {
            return Err(CodingError::Stale);
        }
        history::validate_intent(
            &proposal.intent,
            &self.session,
            [
                self.workspace.identity.device,
                self.workspace.identity.inode,
            ],
        )?;
        self.validate_workspace()?;
        self.pending.take();
        if !approve || cancellation.is_cancelled() {
            self.journal.append(journal::Record::Denied {
                intent: (*proposal.intent).clone(),
            })?;
            return Err(if cancellation.is_cancelled() {
                CodingError::Cancelled
            } else {
                CodingError::Denied
            });
        }
        match &proposal.intent.effect {
            HaikuEffect::Create { path, .. } => absent(&self.workspace.root, path)?,
            HaikuEffect::Process { captures, .. } => self.recheck_captures(captures)?,
        }
        #[cfg(test)]
        self.checkpoint(fault::Point::Authorize)?;
        self.journal.append(journal::Record::Authorized {
            intent: (*proposal.intent).clone(),
        })?;
        #[cfg(test)]
        self.checkpoint(fault::Point::Authorized)?;
        if cancellation.is_cancelled() {
            self.journal.append(journal::Record::NotDispatched {
                operation: proposal.intent.operation,
            })?;
            return Err(CodingError::Cancelled);
        }
        self.validate_workspace()?;
        match &proposal.intent.effect {
            HaikuEffect::Create { path, .. } => absent(&self.workspace.root, path)?,
            HaikuEffect::Process { captures, .. } => self.recheck_captures(captures)?,
        }
        #[cfg(test)]
        self.checkpoint(fault::Point::Dispatch)?;
        self.journal.append(journal::Record::Dispatched {
            operation: proposal.intent.operation,
        })?;
        #[cfg(test)]
        self.checkpoint(fault::Point::Dispatched)?;
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        self.validate_workspace()?;
        let execution = (|| match &proposal.intent.effect {
            HaikuEffect::Create { path, content, .. } => {
                let mut file = FileSlots::new(NAMES)?.create(&self.workspace.root, path)?;
                #[cfg(test)]
                {
                    self.checkpoint(fault::Point::CreateOpen)?;
                    if self
                        .fault
                        .is_some_and(|fault| fault.point == fault::Point::PartialWrite)
                    {
                        file.write_all(&content.as_bytes()[..content.len() / 2])?;
                        self.checkpoint(fault::Point::PartialWrite)?;
                    }
                }
                file.write_all(content.as_bytes())?;
                #[cfg(test)]
                self.checkpoint(fault::Point::FileSync)?;
                file.sync_all()?;
                let capture = read_capture(&self.workspace.root, path)?;
                if capture.identity != identity(&file)? || capture.content != *content {
                    return Err(CodingError::TargetMismatch);
                }
                self.validate_workspace()?;
                #[cfg(test)]
                self.checkpoint(fault::Point::DirectorySync)?;
                self.workspace.root.sync_all()?;
                self.journal.append(journal::Record::Created {
                    operation: proposal.intent.operation,
                    capture: capture.clone(),
                })?;
                #[cfg(test)]
                self.checkpoint(fault::Point::Created)?;
                self.created[name(path)?] = Some(capture);
                self.record_result(
                    &proposal.intent.call_id,
                    serde_json::to_string(&json!({"status":"created","path":path,"mode":384}))
                        .unwrap(),
                )
            }
            HaikuEffect::Process { captures, .. } => {
                let operation = proposal.intent.operation;
                let observed = self.observed.clone();
                let result = process::execute(
                    captures,
                    cancellation,
                    |launcher, namespace_init| {
                        self.journal.append(journal::Record::Started {
                            operation,
                            launcher,
                            namespace_init,
                        })?;
                        #[cfg(test)]
                        self.checkpoint(fault::Point::Start)?;
                        Ok(())
                    },
                    |result| {
                        if let Some(observed) = &observed {
                            observed.update(|snapshot| {
                                snapshot.observed_process = Some(Arc::new(result.clone()))
                            });
                        }
                    },
                )?;
                #[cfg(test)]
                self.checkpoint(fault::Point::Reaped)?;
                self.journal.append(journal::Record::Process {
                    operation: proposal.intent.operation,
                    result: result.clone(),
                })?;
                let success = result.exit_code == Some(0)
                    && result.stderr.is_empty()
                    && result.stdout == captures[0].content
                    && result.stopped.is_none()
                    && result.launcher_reaped
                    && result.namespace_dead;
                let output = serde_json::to_string(&result).unwrap();
                #[cfg(test)]
                self.checkpoint(fault::Point::ProcessAck)?;
                self.process = Some(result);
                self.process_acknowledged = true;
                if let Some(observed) = &self.observed {
                    observed.update(|snapshot| snapshot.process_acknowledged = true);
                }
                self.record_result(&proposal.intent.call_id, output)?;
                if !success {
                    return Err(
                        match self
                            .process
                            .as_ref()
                            .and_then(|result| result.stopped.as_deref())
                        {
                            Some(
                                "isolated output resource limit"
                                | "isolated wall-time resource limit",
                            ) => CodingError::ResourceLimit("isolated process resource limit"),
                            Some("cancelled" | "cancelled before command release") => {
                                CodingError::Cancelled
                            }
                            _ => CodingError::Invalid("isolated haiku process did not succeed"),
                        },
                    );
                }
                Ok(())
            }
        })();
        execution?;
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        Ok(())
    }

    fn fail(&mut self, error: &CodingError) -> Result<(), CodingError> {
        self.stop_reason(error.to_string())
    }
    fn stop_reason(&mut self, reason: String) -> Result<(), CodingError> {
        if !self.stopped {
            self.stopped = true;
            if self.attempts > 0 {
                self.journal.append(journal::Record::Stopped {
                    turn: self.attempts,
                    reason,
                })?;
            }
        }
        Ok(())
    }
    #[cfg(test)]
    fn checkpoint(&self, point: fault::Point) -> Result<(), CodingError> {
        if let Some(pause) = &self.pause {
            pause.hit(point);
        }
        if let Some(fault) = self.fault {
            fault.hit(point)?;
        }
        Ok(())
    }
    fn record_result(&mut self, call_id: &str, output: String) -> Result<(), CodingError> {
        #[cfg(test)]
        self.checkpoint(fault::Point::Result)?;
        self.journal.append(journal::Record::Result {
            call_id: call_id.into(),
            output: output.clone(),
        })?;
        self.history.push(ProviderInput::ToolResult {
            call_id: call_id.into(),
            output,
        });
        Ok(())
    }
    fn validate_workspace(&self) -> Result<(), CodingError> {
        if self.started.elapsed() >= std::time::Duration::from_secs(300) {
            return Err(CodingError::Deadline);
        }
        self.workspace.validate()?;
        crate::project::validate_directory(&self.workspace.root, true)?;
        self.journal.validate()
    }
    fn recheck_captures(&self, captures: &[HaikuCapture; 2]) -> Result<(), CodingError> {
        self.validate_workspace()?;
        for (capture, path) in captures.iter().zip(NAMES) {
            if capture.path != path || read_capture(&self.workspace.root, path)? != *capture {
                return Err(CodingError::TargetMismatch);
            }
        }
        if captures[1].content != HAIKU_SCRIPT {
            return Err(CodingError::Unsupported("unreviewed captured script"));
        }
        Ok(())
    }
    pub fn history(&self) -> Arc<HaikuHistory> {
        self.journal.history()
    }
    fn observe(&mut self, shared: Arc<observation::Shared>) {
        self.journal.observe(shared.clone());
        self.observed = Some(shared);
    }
    pub(super) fn deadline(&self) -> std::time::Instant {
        self.started + std::time::Duration::from_secs(300)
    }
    pub fn successful(&self) -> bool {
        self.process_acknowledged
            && self.created.iter().all(Option::is_some)
            && self.haiku_read
            && self.process.as_ref().is_some_and(|result| {
                result.exit_code == Some(0)
                    && result.stderr.is_empty()
                    && result.stopped.is_none()
                    && result.launcher_reaped
                    && result.namespace_dead
                    && self.created[0]
                        .as_ref()
                        .is_some_and(|capture| capture.content == result.stdout)
            })
    }
    pub fn process_result(&self) -> Option<&HaikuProcessResult> {
        self.process.as_ref()
    }
}

#[cfg(test)]
mod tests;
