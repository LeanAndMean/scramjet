mod agent;
mod store;
#[cfg(test)]
mod tests;
pub use agent::{CodingInvocation, CodingInvocationOptions, CodingInvocationResult};

use super::{
    CodingError, EditCancellation,
    captured_files::{CapturedFile, FILE_BYTES, FileSlots},
};
use crate::runtime::provider_history::{ProviderBinding, ProviderLedger};
use crate::{
    Outcome, ProviderInput, ProviderItem, ProviderRequest, ProviderResponse, ProviderTool,
    RequestId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::Write,
    sync::{Arc, Mutex},
    time::Instant,
};

const RECORD_BYTES: usize = 131_072;
const RETAINED_BYTES: usize = 2_097_152;
const RECORDS: usize = 128;
const ATTEMPTS: usize = 8;
const RUN_SECONDS: u64 = 300;
fn validate_run(run: &str) -> Result<(), CodingError> {
    super::captured_files::validate_basename(run)?;
    if run.len() > 64 {
        return Err(CodingError::ResourceLimit("run namespace resource limit"));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Material {
    User { text: String },
    Item { value: Value },
    ToolResult { call_id: String, output: String },
}
fn request(material: &[Material], names: &[String; 2]) -> Result<ProviderRequest, CodingError> {
    let input = material
        .iter()
        .map(|item| match item {
            Material::User { text } => Ok(ProviderInput::User(text.clone())),
            Material::Item { value } => {
                ProviderItem::from_context(value.clone()).map(ProviderInput::Item)
            }
            Material::ToolResult { call_id, output } => Ok(ProviderInput::ToolResult {
                call_id: call_id.clone(),
                output: output.clone(),
            }),
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(context_error)?;
    ProviderRequest::new(&input, &tools(names)).map_err(context_error)
}
fn context_error(outcome: Outcome) -> CodingError {
    match outcome {
        Outcome::ResourceLimit => {
            CodingError::ResourceLimit("complete conversation resource limit; no truncation")
        }
        Outcome::Unsupported => CodingError::Unsupported("unsupported conversation continuation"),
        _ => CodingError::History("contradictory conversation material"),
    }
}
fn tools(names: &[String; 2]) -> Vec<ProviderTool> {
    let paths = json!({"type":"string","enum":names});
    vec![
        ProviderTool {name:"write".into(),description:"Exclusively create an absent selected UTF-8 file, at most 512 bytes; no overwrites".into(),parameters:json!({"type":"object","properties":{"path":paths,"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})},
        ProviderTool {name:"read".into(),description:"Read a selected file".into(),parameters:json!({"type":"object","properties":{"path":paths},"required":["path"],"additionalProperties":false})},
        ProviderTool {name:"python".into(),description:"Execute selected Python in a read-only, no-spawn isolated child after exact approval of complete captures".into(),parameters:json!({"type":"object","properties":{"path":paths},"required":["path"],"additionalProperties":false})},
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodingInvocationIntent {
    pub run: String,
    pub root: [u64; 2],
    pub generation: usize,
    pub operation: usize,
    pub request_id: u64,
    pub ordinal: usize,
    pub response_id: String,
    pub call_id: String,
    pub effect: CodingInvocationEffect,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum CodingInvocationEffect {
    Create {
        path: String,
        content: String,
        mode: u32,
    },
    Python {
        path: String,
        captures: Vec<CapturedFile>,
        argv: Vec<String>,
        profile: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Receipt {
    request_id: u64,
    response_request_id: u64,
    ordinal: Option<usize>,
    outcome: super::haiku::HaikuOutcome,
    terminal: Option<crate::ProviderTerminalEvidence>,
    usage: Option<crate::ProviderUsage>,
    durable: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u8,
    model: String,
    run: String,
    root: [u64; 2],
    storage: [u64; 2],
    directory: [u64; 2],
    journal: [u64; 2],
    provider: ProviderBinding,
    names: [String; 2],
    initial: [Option<CapturedFile>; 2],
    boot: String,
    started_ns: u64,
    deadline_ns: u64,
    limits: Limits,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Limits {
    creates: usize,
    reads: usize,
    processes: usize,
    attempts: usize,
    seconds: u64,
}
const LIMITS: Limits = Limits {
    creates: 2,
    reads: 3,
    processes: 1,
    attempts: ATTEMPTS,
    seconds: RUN_SECONDS,
};
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Counters {
    creates: usize,
    reads: usize,
    processes: usize,
    attempts: usize,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum Charge {
    Create,
    Read,
    Python,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Record {
    Header {
        header: Box<Header>,
    },
    InvocationStarted {
        generation: usize,
        prompt: String,
    },
    Attempt {
        operation: usize,
        request: String,
    },
    Receipt {
        receipt: Receipt,
    },
    Selected {
        response_id: String,
        items: Vec<Value>,
    },
    Authorized {
        intent: CodingInvocationIntent,
    },
    Denied {
        intent: CodingInvocationIntent,
    },
    Charged {
        operation: usize,
        charge: Charge,
    },
    Dispatched {
        operation: usize,
    },
    Started {
        operation: usize,
        launcher: u32,
        namespace_init: u32,
    },
    Created {
        operation: usize,
        capture: CapturedFile,
    },
    Process {
        operation: usize,
        result: super::process::ProcessResult,
    },
    Result {
        call_id: String,
        output: String,
    },
    MessageComplete,
    Final {
        generation: usize,
        worker_joined: bool,
        transports_joined: bool,
    },
    Export {
        generation: usize,
        path: String,
    },
    Stopped {
        reason: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Export {
    version: u8,
    header: Header,
    generation: usize,
    disposition: String,
    conversation: Vec<Material>,
    tools: Vec<Value>,
    counters: Counters,
    captures: [Option<CapturedFile>; 2],
    receipts: Vec<Receipt>,
    worker_joined: bool,
    transports_joined: bool,
    export_producer_acknowledgement: String,
    execution: Value,
}
fn tool_values(names: &[String; 2]) -> Vec<Value> {
    tools(names)
        .into_iter()
        .map(|t| json!({"name":t.name,"description":t.description,"parameters":t.parameters}))
        .collect()
}

enum Selection {
    Approval(Box<CodingInvocationIntent>),
    Continue,
    Finished,
}

struct Session {
    store: store::Store,
    provider: Arc<Mutex<ProviderLedger>>,
    pending: Option<(Arc<()>, CodingInvocationIntent)>,
    owner: Arc<()>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}
impl Session {
    fn guard(&self) -> Result<(), CodingError> {
        self.store.guard()?;
        let ledger = self.provider.lock().map_err(|_| CodingError::Poisoned)?;
        self.store.check_receipts(&ledger)?;
        if ledger.binding()? != self.store.state.header.provider
            || !ledger.eligible()
            || ledger.history().unknown_usage() != 0
            || ledger.history().unresolved_remote() != 0
        {
            return Err(CodingError::History(
                "provider owner or uncertainty changed",
            ));
        }
        Ok(())
    }
    fn prepare(&mut self) -> Result<ProviderRequest, CodingError> {
        self.guard()?;
        if self.store.state.counters.attempts >= ATTEMPTS {
            return Err(CodingError::ResourceLimit("run model attempt ceiling"));
        }
        let request = request(&self.store.state.material, &self.store.state.header.names)?;
        self.store.reserve_turn()?;
        self.store.append(Record::Attempt {
            operation: self.store.state.counters.attempts + 1,
            request: request.encoded.clone(),
        })?;
        Ok(request)
    }
    fn select(
        &mut self,
        expected: RequestId,
        response: ProviderResponse,
        cancellation: &EditCancellation,
    ) -> Result<Selection, CodingError> {
        let receipt = Receipt {
            request_id: expected.0,
            response_request_id: response.id.0,
            ordinal: response.reservation,
            outcome: (&response.outcome).into(),
            terminal: response.remote_terminal.clone(),
            usage: response.usage.clone(),
            durable: response.durable,
        };
        crate::http::responses::bounded_json_bytes(&receipt, crate::PROVIDER_FRAME_BYTES + 1024)
            .map_err(context_error)?;
        self.store.append(Record::Receipt {
            receipt: receipt.clone(),
        })?;
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        self.store.guard()?;
        if response.id != expected
            || !response.durable
            || response.outcome != Outcome::Success
            || response.usage.as_ref().is_none_or(|u| !u.complete())
            || response.reservation.is_none()
        {
            return Err(match response.outcome {
                Outcome::ResourceLimit => CodingError::ResourceLimit("provider resource refusal"),
                Outcome::Unsupported => CodingError::Unsupported("provider replay refusal"),
                Outcome::Cancelled => CodingError::Cancelled,
                Outcome::Deadline => CodingError::Deadline,
                _ => CodingError::Invalid("provider completion is not executable"),
            });
        }
        self.store
            .check_receipts(&*self.provider.lock().map_err(|_| CodingError::Poisoned)?)?;
        let envelope = response.envelope.ok_or(CodingError::Unsupported(
            "envelope-free completion is not executable",
        ))?;
        let terminal = response
            .remote_terminal
            .as_ref()
            .ok_or(CodingError::Stale)?;
        if terminal.response_id != envelope.response_id || terminal.status != "completed" {
            return Err(CodingError::Stale);
        }
        let calls = envelope
            .items
            .iter()
            .filter_map(|i| i.function_call())
            .collect::<Vec<_>>();
        if calls.len() > 1 {
            return Err(CodingError::Unsupported(
                "one completed tool call per response",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        crate::http::responses::bounded_json_bytes(&json!({"response_id":envelope.response_id,"items":envelope.items.iter().map(|i|i.raw()).collect::<Vec<_>>()}),crate::PROVIDER_ENVELOPE_BYTES).map_err(context_error)?;
        if self.store.state.material.iter().any(|m|matches!(m,Material::Item {value} if envelope.items.iter().any(|item|item.id()==value["id"].as_str().unwrap() || item.function_call().is_some_and(|call|value["type"]=="function_call" && value["call_id"].as_str()==Some(call.call_id))))) {return Err(CodingError::Stale);}
        if self.store.state.responses.contains(&envelope.response_id) {
            return Err(CodingError::Stale);
        }
        self.store.append(Record::Selected {
            response_id: envelope.response_id.clone(),
            items: envelope.items.iter().map(|i| i.raw().clone()).collect(),
        })?;
        let Some(call) = calls.first() else {
            if cancellation.is_cancelled() {
                return Err(CodingError::Cancelled);
            }
            self.store.append(Record::MessageComplete)?;
            return Ok(Selection::Finished);
        };
        let names = self.store.state.header.names.clone();
        let slots = FileSlots::new([&names[0], &names[1]])?;
        let effect = match call.name {
            "write" => {
                if self.store.state.counters.creates >= LIMITS.creates {
                    return Err(CodingError::ResourceLimit("run creation ceiling"));
                }
                let args: WriteArgs = serde_json::from_str(call.arguments)
                    .map_err(|_| CodingError::Invalid("malformed write arguments"))?;
                if args.content.len() > FILE_BYTES {
                    return Err(CodingError::ResourceLimit("selected file resource limit"));
                }
                self.guard()?;
                slots.absent(&self.store.workspace.root, &args.path)?;
                CodingInvocationEffect::Create {
                    path: args.path,
                    content: args.content,
                    mode: 0o600,
                }
            }
            "read" => {
                if self.store.state.counters.reads >= LIMITS.reads {
                    return Err(CodingError::ResourceLimit("run read ceiling"));
                }
                let args: PathArgs = serde_json::from_str(call.arguments)
                    .map_err(|_| CodingError::Invalid("malformed read arguments"))?;
                self.guard()?;
                if !names.contains(&args.path) {
                    return Err(CodingError::Unsupported("read outside selected slots"));
                }
                if cancellation.is_cancelled() {
                    return Err(CodingError::Cancelled);
                }
                self.store.append(Record::Charged {
                    operation: self.store.state.counters.attempts,
                    charge: Charge::Read,
                })?;
                if cancellation.is_cancelled() {
                    return Err(CodingError::Cancelled);
                }
                let output = match slots.capture(&self.store.workspace.root, &args.path) {
                    Ok(capture) => serde_json::to_string(
                        &json!({"status":"read","path":args.path,"content":capture.content}),
                    )
                    .unwrap(),
                    Err(CodingError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                        serde_json::to_string(
                            &json!({"status":"error","kind":"not_found","path":args.path}),
                        )
                        .unwrap()
                    }
                    Err(e) => return Err(e),
                };
                self.store.append(Record::Result {
                    call_id: call.call_id.into(),
                    output,
                })?;
                return Ok(Selection::Continue);
            }
            "python" => {
                if self.store.state.counters.processes >= LIMITS.processes {
                    return Err(CodingError::ResourceLimit("run Python ceiling"));
                }
                let args: PathArgs = serde_json::from_str(call.arguments)
                    .map_err(|_| CodingError::Invalid("malformed python arguments"))?;
                self.guard()?;
                slots.capture(&self.store.workspace.root, &args.path)?;
                let captures = self
                    .store
                    .state
                    .captures
                    .iter()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>();
                super::process::check_python_host()?;
                CodingInvocationEffect::Python {
                    path: args.path.clone(),
                    captures,
                    argv: vec![
                        "/usr/bin/python3".into(),
                        "-I".into(),
                        format!("/workspace/{}", args.path),
                    ],
                    profile: super::process::PYTHON_PROFILE.into(),
                }
            }
            _ => return Err(CodingError::Unsupported("unsupported coding tool")),
        };
        let intent = CodingInvocationIntent {
            run: self.store.state.header.run.clone(),
            root: self.store.state.header.root,
            generation: self.store.state.generation,
            operation: self.store.state.counters.attempts,
            request_id: expected.0,
            ordinal: receipt.ordinal.unwrap(),
            response_id: envelope.response_id,
            call_id: call.call_id.into(),
            effect,
        };
        self.pending = Some((self.owner.clone(), intent.clone()));
        Ok(Selection::Approval(Box::new(intent)))
    }
    fn decide(
        &mut self,
        intent: CodingInvocationIntent,
        approve: bool,
        cancellation: &EditCancellation,
    ) -> Result<(), CodingError> {
        if self
            .pending
            .as_ref()
            .is_none_or(|(owner, p)| !Arc::ptr_eq(owner, &self.owner) || *p != intent)
        {
            return Err(CodingError::Stale);
        }
        self.pending.take();
        self.guard()?;
        if !approve || cancellation.is_cancelled() {
            self.store.append(Record::Denied { intent })?;
            return Err(if cancellation.is_cancelled() {
                CodingError::Cancelled
            } else {
                CodingError::Denied
            });
        }
        self.store.append(Record::Authorized {
            intent: intent.clone(),
        })?;
        let charge = match intent.effect {
            CodingInvocationEffect::Create { .. } => Charge::Create,
            CodingInvocationEffect::Python { .. } => Charge::Python,
        };
        self.store.append(Record::Charged {
            operation: intent.operation,
            charge,
        })?;
        self.guard()?;
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        self.store.append(Record::Dispatched {
            operation: intent.operation,
        })?;
        self.guard()?;
        if cancellation.is_cancelled() {
            return Err(CodingError::Cancelled);
        }
        let names = self.store.state.header.names.clone();
        let slots = FileSlots::new([&names[0], &names[1]])?;
        let output = match &intent.effect {
            CodingInvocationEffect::Create { path, content, .. } => {
                let mut file = slots.create(&self.store.workspace.root, path)?;
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                let capture = slots.capture(&self.store.workspace.root, path)?;
                if capture.content != *content || capture.identity != store::identity(&file)? {
                    return Err(CodingError::TargetMismatch);
                }
                self.store.workspace.validate()?;
                self.store.workspace.root.sync_all()?;
                self.store.append(Record::Created {
                    operation: intent.operation,
                    capture,
                })?;
                serde_json::to_string(&json!({"status":"created","path":path,"mode":384})).unwrap()
            }
            CodingInvocationEffect::Python { path, captures, .. } => {
                let result = super::process::python(
                    captures,
                    path,
                    cancellation,
                    |launcher, namespace_init| {
                        self.store.append(Record::Started {
                            operation: intent.operation,
                            launcher,
                            namespace_init,
                        })
                    },
                )?;
                let unsupported_stop =
                    result.stopped.as_deref() == Some("isolated job-control stop unsupported");
                let stopped = result.stopped.is_some();
                let failed = result.exit_code != Some(0) || result.signal.is_some();
                let output = serde_json::to_string(&result).unwrap();
                self.store.append(Record::Process {
                    operation: intent.operation,
                    result,
                })?;
                self.store.append(Record::Result {
                    call_id: intent.call_id.clone(),
                    output,
                })?;
                if unsupported_stop {
                    return Err(CodingError::Unsupported(
                        "isolated job-control stop unsupported",
                    ));
                }
                if stopped {
                    return Err(if cancellation.is_cancelled() {
                        CodingError::Cancelled
                    } else {
                        CodingError::ResourceLimit("isolated process resource refusal")
                    });
                }
                if failed {
                    return Err(CodingError::Invalid(
                        "isolated Python process failed; complete result retained",
                    ));
                }
                return Ok(());
            }
        };
        self.store.append(Record::Result {
            call_id: intent.call_id,
            output,
        })?;
        Ok(())
    }
}
