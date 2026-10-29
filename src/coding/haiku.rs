mod journal;
mod process;

use super::{CodingError, EditCancellation};
use crate::http::responses::bounded_json_bytes;
use crate::{
    Outcome, ProjectWorkspace, ProviderInput, ProviderRequest, ProviderResponse, ProviderTool,
    RequestId,
};
use rustix::fs::{Mode, OFlags, fstat, openat};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

pub const HAIKU_SCRIPT: &str = "from pathlib import Path\nimport sys\nsys.stdout.buffer.write(Path('haiku.md').read_bytes())\n";
pub const HAIKU_COMMAND: &str = "exec /usr/bin/python3 -I print_haiku.py";
const FILE_BYTES: usize = 512;
const NAMES: [&str; 2] = ["haiku.md", "print_haiku.py"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaikuCapture {
    pub path: String,
    pub identity: [u64; 2],
    pub content: String,
}

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
        .ok_or(CodingError::Invalid("unsupported haiku target"))
}
fn identity(file: &File) -> Result<[u64; 2], CodingError> {
    let stat = fstat(file)?;
    Ok([stat.st_dev, stat.st_ino])
}
fn read_capture(root: &File, path: &str) -> Result<HaikuCapture, CodingError> {
    name(path)?;
    let mut file = File::from(openat(
        root,
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    crate::project::validate_stage(&file, 0o600)?;
    let before = fstat(&file)?;
    if before.st_size < 0 || before.st_size > FILE_BYTES as i64 {
        return Err(CodingError::Invalid("haiku file resource limit"));
    }
    let mut bytes = [0; FILE_BYTES + 1];
    let mut count = 0;
    loop {
        let n = file.read(&mut bytes[count..])?;
        if n == 0 {
            break;
        }
        count += n;
        if count > FILE_BYTES {
            return Err(CodingError::Invalid("haiku file resource limit"));
        }
    }
    let after = fstat(&file)?;
    if (
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
    ) != (
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
    ) {
        return Err(CodingError::TargetMismatch);
    }
    crate::project::validate_stage(&file, 0o600)?;
    Ok(HaikuCapture {
        path: path.into(),
        identity: identity(&file)?,
        content: String::from_utf8(bytes[..count].to_vec())
            .map_err(|_| CodingError::Invalid("haiku file is not UTF-8"))?,
    })
}
fn absent(root: &File, path: &str) -> Result<(), CodingError> {
    name(path)?;
    match rustix::fs::statat(root, path, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(CodingError::TargetMismatch),
    }
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
            started: std::time::Instant::now(),
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
        if self.stopped || self.request_pending || self.pending.is_some() || self.attempts >= 8 {
            return Err(CodingError::Stale);
        }
        self.validate_workspace()?;
        let request = ProviderRequest::new(&self.history, &haiku_tools())
            .map_err(|_| CodingError::Invalid("complete haiku context refused; no truncation"))?;
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
        self.validate_workspace()?;
        self.request_pending = false;
        if response.id != expected
            || !response.durable
            || response.outcome != Outcome::Success
            || response.usage.as_ref().is_none_or(|u| !u.complete())
            || response.reservation.is_none()
            || response.remote_terminal.is_none()
        {
            return Err(CodingError::Invalid(
                "provider completion has no haiku effect authority",
            ));
        }
        let envelope = response
            .envelope
            .ok_or(CodingError::Invalid("provider envelope unavailable"))?;
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
            return Err(CodingError::Invalid(
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
                    return Err(CodingError::Invalid("haiku file resource limit"));
                }
                if slot == 1 && args.content != HAIKU_SCRIPT {
                    return Err(CodingError::Invalid("unreviewed Python script"));
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
            _ => return Err(CodingError::Invalid("unsupported haiku tool")),
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
        self.validate_workspace()?;
        self.pending.take();
        if !approve || cancellation.is_cancelled() {
            self.journal.append(journal::Record::Denied {
                intent: (*proposal.intent).clone(),
            })?;
            return Err(if approve {
                CodingError::Cancelled
            } else {
                CodingError::Denied
            });
        }
        match &proposal.intent.effect {
            HaikuEffect::Create { path, .. } => absent(&self.workspace.root, path)?,
            HaikuEffect::Process { captures, .. } => self.recheck_captures(captures)?,
        }
        self.journal.append(journal::Record::Authorized {
            intent: (*proposal.intent).clone(),
        })?;
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
        self.journal.append(journal::Record::Dispatched {
            operation: proposal.intent.operation,
        })?;
        let execution = (|| match &proposal.intent.effect {
            HaikuEffect::Create { path, content, .. } => {
                let mut file = File::from(openat(
                    &self.workspace.root,
                    path.as_str(),
                    OFlags::WRONLY
                        | OFlags::CREATE
                        | OFlags::EXCL
                        | OFlags::NOFOLLOW
                        | OFlags::CLOEXEC,
                    Mode::RUSR | Mode::WUSR,
                )?);
                crate::project::validate_stage(&file, 0o600)?;
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                let capture = read_capture(&self.workspace.root, path)?;
                if capture.identity != identity(&file)? || capture.content != *content {
                    return Err(CodingError::TargetMismatch);
                }
                self.validate_workspace()?;
                self.workspace.root.sync_all()?;
                self.journal.append(journal::Record::Created {
                    operation: proposal.intent.operation,
                    capture: capture.clone(),
                })?;
                self.created[name(path)?] = Some(capture);
                self.record_result(
                    &proposal.intent.call_id,
                    serde_json::to_string(&json!({"status":"created","path":path,"mode":384}))
                        .unwrap(),
                )
            }
            HaikuEffect::Process { captures, .. } => {
                let result = process::execute(captures, cancellation)?;
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
                self.process = Some(result);
                self.record_result(&proposal.intent.call_id, output)?;
                if !success {
                    return Err(CodingError::Invalid(
                        "isolated haiku process did not succeed",
                    ));
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
        if !self.stopped {
            self.stopped = true;
            if self.attempts > 0 {
                self.journal.append(journal::Record::Stopped {
                    turn: self.attempts,
                    reason: error.to_string(),
                })?;
            }
        }
        Ok(())
    }
    fn record_result(&mut self, call_id: &str, output: String) -> Result<(), CodingError> {
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
            return Err(CodingError::Invalid("haiku invocation deadline exceeded"));
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
            return Err(CodingError::Invalid("unreviewed captured script"));
        }
        Ok(())
    }
    pub fn successful(&self) -> bool {
        self.created.iter().all(Option::is_some)
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
