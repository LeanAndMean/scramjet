use super::journal::{BoundedString, Record};
use super::observation::{
    CodingAccounting, CodingControl, CodingDecision, CodingObserver, CodingProposal, Shared,
};
use super::{
    CodingError, CodingSession, EditApproval, EditCancellation, OperationFact, OperationState,
    validate_name,
};
use crate::{Outcome, RequestId, Runtime};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
static OWNERS: AtomicU64 = AtomicU64::new(1);
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::io::Write;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::JoinHandle;
use tokio::sync::oneshot;

const TURNS: u64 = 8;
const RESPONSE_BYTES: usize = 16_384;

mod driver;
use driver::{DriverMode, DriverRun, drive};
pub(crate) async fn run_haiku(
    agent: &mut super::haiku::agent::HaikuAgent,
    runtime: &Runtime,
) -> Result<super::haiku::agent::HaikuRun, CodingError> {
    let result = drive(DriverMode::Haiku(agent), runtime).await;
    if let Err(error) = &result {
        agent.stop(error).await?;
    }
    match result? {
        DriverRun::Haiku(run) => Ok(run),
        DriverRun::Fixture(_) | DriverRun::Invocation => unreachable!(),
    }
}
pub(crate) async fn run_invocation(
    agent: &mut super::invocation::CodingInvocation,
    runtime: &Runtime,
) -> Result<(), CodingError> {
    let result = drive(DriverMode::Invocation(agent), runtime).await;
    if let Err(error) = &result {
        agent.stop(error).await?;
    }
    match result? {
        DriverRun::Invocation => Ok(()),
        _ => unreachable!(),
    }
}

pub(super) use super::CodingMaterial as Material;

#[derive(Default)]
pub(super) struct History {
    turn: u64,
    request: u64,
    phase: u8,
    ended: bool,
    action: Option<Action>,
    read: Option<String>,
    replaced: Option<(String, String)>,
    requests: [u64; 8],
    response: String,
    authorized: Option<u64>,
    verified: bool,
    context: Option<OwnedContext>,
    context_valid: bool,
    prior: Option<PriorContext>,
    follow_prompt: Option<String>,
    previous: String,
    first_final: Option<String>,
    project: Option<crate::project::context::Goal>,
    verified_identity: Option<super::RootIdentity>,
}

impl History {
    pub(super) fn validate_bound(
        &self,
        session: &str,
        root: super::RootIdentity,
    ) -> Result<(), CodingError> {
        if let Some(project) = &self.project
            && (project.evidence.root != root
                || self
                    .context
                    .as_ref()
                    .is_some_and(|context| context.session != session))
        {
            return Err(CodingError::History(
                "project material session/root differs from journal header",
            ));
        }
        Ok(())
    }
    pub(super) fn follow_up(
        &mut self,
        invocation: u64,
        after_turn: u64,
        prompt: &str,
        session: &str,
        facts: &[OperationFact],
    ) -> Result<(), CodingError> {
        let context = self
            .context
            .as_ref()
            .ok_or(CodingError::History("missing first context"))?;
        let (target, replacement) = self
            .replaced
            .as_ref()
            .ok_or(CodingError::History("missing first effect"))?;
        if invocation != 2
            || after_turn != 4
            || self.turn != 4
            || !self.ended
            || !self.verified
            || !self.context_valid
            || context.session != session
            || self.follow_prompt.is_some()
            || facts.len() != 1
            || facts[0].state != OperationState::Completed
            || facts[0].target != *target
            || replacement != &context.prompt
            || self
                .first_final
                .as_ref()
                .is_none_or(|s| s.chars().any(char::is_control))
        {
            return Err(CodingError::History("invalid follow-up transition"));
        }
        let prior = PriorContext {
            prompt: Arc::from(context.prompt.as_str()),
            summary: Arc::from(
                self.first_final
                    .as_deref()
                    .ok_or(CodingError::History("missing first final"))?,
            ),
            target: Arc::from(target.as_str()),
            operation: 1,
            verified: true,
        };
        validate_follow_prompt(session, &prior, prompt)?;
        self.prior = Some(prior);
        self.follow_prompt = Some(prompt.to_owned());
        self.ended = false;
        self.read = None;
        self.replaced = None;
        self.verified = false;
        self.previous.clear();
        Ok(())
    }

    pub(super) fn authorize(&mut self, intent: &super::EditIntent) -> Result<(), CodingError> {
        if let Some(goal) = &self.project
            && (intent.root != goal.evidence.root
                || intent.target != goal.evidence.request.target()
                || intent.expected != goal.expected
                || intent.replacement != goal.desired()?)
        {
            return Err(CodingError::History(
                "authorization differs from project goal",
            ));
        }
        if self.turn == 0 && self.phase == 0 {
            return Ok(());
        }
        match self.action.as_ref() {
            Some(Action::Replace {
                op,
                target,
                expected,
                replacement,
            }) if self.phase == 2
                && *op == self.turn
                && target == &intent.target
                && expected == &intent.expected
                && replacement == &intent.replacement
                && self.authorized.is_none() =>
            {
                self.authorized = Some(intent.operation);
                Ok(())
            }
            _ => Err(CodingError::History(
                "edit authorization does not match completed model invocation",
            )),
        }
    }

    pub(super) fn accept(
        &mut self,
        entry: &Material,
        facts: &[OperationFact],
    ) -> Result<(), CodingError> {
        if entry.event == "VerifiedIdentity" {
            if self.project.is_none()
                || self.turn != 3
                || self.phase != 0
                || !self.verified
                || self.verified_identity.is_some()
                || entry.turn != 3
                || entry.request != self.request
                || entry.operation != 0
                || entry.status != "Captured"
                || entry.text.len() > 128
            {
                return Err(CodingError::History(
                    "invalid verified project identity material",
                ));
            }
            self.verified_identity = Some(
                serde_json::from_str(&entry.text)
                    .map_err(|_| CodingError::History("invalid verified identity schema"))?,
            );
            return Ok(());
        }
        if entry.event == "ProjectGoal" {
            if self.turn != 0
                || self.phase != 0
                || self.project.is_some()
                || entry.turn != 0
                || entry.request != 0
                || entry.operation != 0
                || entry.status != "Captured"
                || entry.text.len() > 4096
            {
                return Err(CodingError::History("invalid project goal history"));
            }
            let goal: crate::project::context::Goal = serde_json::from_str(&entry.text)
                .map_err(|_| CodingError::History("invalid strict project goal"))?;
            goal.validate()?;
            self.project = Some(goal);
            return Ok(());
        }
        if self.ended || entry.turn == 0 || entry.turn > TURNS || entry.text.len() > RESPONSE_BYTES
        {
            return Err(CodingError::History("invalid material history bounds"));
        }
        match entry.event.as_str() {
            "Request"
                if self.phase == 0
                    && entry.turn == self.turn + 1
                    && entry.request == 0
                    && entry.operation == 0
                    && entry.status == "Pending"
                    && entry.text.len() <= 4096 =>
            {
                if let Some(goal) = &self.project {
                    let context: crate::project::context::Context =
                        serde_json::from_str(&entry.text).map_err(|_| {
                            CodingError::History("invalid complete project context")
                        })?;
                    if context.turn != entry.turn
                        || context.project != goal.evidence
                        || context.previous != self.previous
                        || self
                            .context
                            .as_ref()
                            .is_some_and(|first| first.session != context.session)
                    {
                        return Err(CodingError::History("project context/evidence mismatch"));
                    }
                    super::validate_session(&context.session)?;
                    self.context = Some(OwnedContext {
                        session: context.session,
                        turn: context.turn,
                        prompt: "replace_once".into(),
                        previous: context.previous,
                    });
                    self.context_valid = true;
                } else if let Some(prompt) = &self.follow_prompt {
                    let context: OwnedFollowContext = serde_json::from_str(&entry.text)
                        .map_err(|_| CodingError::History("invalid follow-up request context"))?;
                    if context.invocation != 2
                        || context.turn != entry.turn
                        || context.session != self.context.as_ref().unwrap().session
                        || context.prompt != *prompt
                        || context.previous != self.previous
                        || Some(&context.prior) != self.prior.as_ref()
                    {
                        return Err(CodingError::History("follow-up context mismatch"));
                    }
                } else {
                    let context = serde_json::from_str::<OwnedContext>(&entry.text).ok();
                    if entry.turn == 1 {
                        self.context_valid = context.as_ref().is_some_and(|c| {
                            c.turn == 1
                                && c.previous.is_empty()
                                && CodingAgent::validate_prompt(&c.session, &c.prompt).is_ok()
                        });
                        self.context = context;
                    } else {
                        self.context_valid &= context
                            .as_ref()
                            .zip(self.context.as_ref())
                            .is_some_and(|(c, first)| {
                                c.session == first.session
                                    && c.prompt == first.prompt
                                    && c.turn == entry.turn
                                    && c.previous == self.previous
                            });
                    }
                }
                self.turn = entry.turn;
                self.request = 0;
                if self.response.capacity() == 0 {
                    self.response = String::with_capacity(RESPONSE_BYTES);
                }
                self.response.clear();
                self.phase = 1;
            }
            "ResponsePart"
                if self.phase == 1
                    && entry.turn == self.turn
                    && entry.request != 0
                    && entry.operation == 0
                    && entry.status == "Pending"
                    && entry.text.len() <= 2048
                    && self.response.len() + entry.text.len() <= RESPONSE_BYTES =>
            {
                if self.request == 0 {
                    if self.requests.contains(&entry.request) {
                        return Err(CodingError::History("reused material request identity"));
                    }
                    self.request = entry.request;
                }
                if entry.request != self.request {
                    return Err(CodingError::History(
                        "response fragment correlation mismatch",
                    ));
                }
                self.response.push_str(&entry.text);
            }
            "Response"
                if self.phase == 1
                    && entry.turn == self.turn
                    && entry.request != 0
                    && entry.operation == 0
                    && matches!(entry.status.as_str(), "Success" | "Failed") =>
            {
                if !entry.text.is_empty()
                    || (self.request != 0 && self.request != entry.request)
                    || self.requests.contains(&entry.request)
                {
                    return Err(CodingError::History(
                        "invalid material response correlation",
                    ));
                }
                self.request = entry.request;
                self.requests[(self.turn - 1) as usize] = entry.request;
                self.phase = 2;
                if entry.status == "Success" {
                    self.action = serde_json::from_str(&self.response).ok();
                }
                if entry.status == "Failed" {
                    self.ended = true;
                }
            }
            "Tool" | "Final"
                if self.phase == 2
                    && entry.turn == self.turn
                    && entry.request == self.request
                    && entry.operation == self.turn
                    && entry.text.len() <= 1024
                    && matches!(
                        entry.status.as_str(),
                        "Read" | "Replaced" | "Verified" | "Final" | "Error"
                    ) =>
            {
                if entry.status != "Error"
                    && let Some(goal) = &self.project
                {
                    if entry.status == "Final" && self.verified_identity.is_none() {
                        return Err(CodingError::History(
                            "project Final lacks acknowledged verified identity",
                        ));
                    }
                    validate_project_action(
                        self.action
                            .as_ref()
                            .ok_or(CodingError::History("missing project action"))?,
                        self.turn,
                        goal,
                    )?;
                    let expected = match entry.status.as_str() {
                        "Read" => Some(goal.expected.clone()),
                        "Replaced" => Some(goal.desired()?),
                        _ => None,
                    };
                    if expected.is_some_and(|text| text != entry.text) {
                        return Err(CodingError::History(
                            "project tool material differs from capture/goal",
                        ));
                    }
                }
                let matches = match (entry.status.as_str(), self.action.as_ref()) {
                    ("Read", Some(Action::Read { op, target }))
                        if *op == self.turn && validate_name(target).is_ok() =>
                    {
                        self.read = Some(target.clone());
                        true
                    }
                    (
                        "Replaced",
                        Some(Action::Replace {
                            op,
                            target,
                            replacement,
                            ..
                        }),
                    ) if *op == self.turn
                        && self.read.as_ref() == Some(target)
                        && self.replaced.is_none() =>
                    {
                        self.replaced = Some((target.clone(), replacement.clone()));
                        true
                    }
                    (
                        "Verified",
                        Some(Action::Verify {
                            op,
                            target,
                            expected,
                        }),
                    ) if *op == self.turn
                        && self.replaced.as_ref() == Some(&(target.clone(), expected.clone())) =>
                    {
                        self.verified = true;
                        true
                    }
                    ("Final", Some(Action::Final { summary }))
                        if self.verified && summary == &entry.text =>
                    {
                        true
                    }
                    ("Error", _) => true,
                    _ => false,
                };
                if !matches {
                    return Err(CodingError::History(
                        "tool outcome does not match completed response",
                    ));
                }
                if entry.status == "Replaced"
                    && facts.last().is_none_or(|f| {
                        f.state != OperationState::Completed || Some(f.operation) != self.authorized
                    })
                {
                    return Err(CodingError::History(
                        "material replacement without acknowledged effect",
                    ));
                }
                if (entry.event == "Final") != (entry.status == "Final") {
                    return Err(CodingError::History("invalid final material correlation"));
                }
                self.previous = entry.text.clone();
                if entry.status == "Final" && self.follow_prompt.is_none() {
                    self.first_final = Some(entry.text.clone());
                }
                self.phase = 0;
                self.action = None;
                self.authorized = None;
                self.ended = matches!(entry.status.as_str(), "Final" | "Error");
            }
            _ => return Err(CodingError::History("invalid material history transition")),
        }
        Ok(())
    }
}

#[derive(Debug)]
enum Action {
    Read {
        op: u64,
        target: String,
    },
    Replace {
        op: u64,
        target: String,
        expected: String,
        replacement: String,
    },
    Verify {
        op: u64,
        target: String,
        expected: String,
    },
    Final {
        summary: String,
    },
}

impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Action;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a bounded tool response object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Action, M::Error> {
                let (mut kind, mut op, mut target, mut expected, mut replacement, mut summary) =
                    (None, None, None, None, None, None);
                while let Some(key) = map.next_key::<BoundedString<16>>()? {
                    macro_rules! field {
                        ($slot:ident, $wire:ty) => {{
                            if $slot.is_some() {
                                return Err(de::Error::custom("duplicate response field"));
                            }
                            $slot = Some(map.next_value::<$wire>()?);
                        }};
                    }
                    match key.0.as_str() {
                        "kind" => field!(kind, BoundedString<16>),
                        "op" => field!(op, u64),
                        "target" => field!(target, BoundedString<64>),
                        "expected" => field!(expected, BoundedString<1024>),
                        "replacement" => field!(replacement, BoundedString<1024>),
                        "summary" => field!(summary, BoundedString<1024>),
                        _ => return Err(de::Error::custom("unknown response field")),
                    }
                }
                let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?.0;
                if kind == "Final"
                    && op.is_none()
                    && target.is_none()
                    && expected.is_none()
                    && replacement.is_none()
                {
                    return Ok(Action::Final {
                        summary: summary
                            .ok_or_else(|| de::Error::missing_field("summary"))?
                            .0,
                    });
                }
                if summary.is_some() {
                    return Err(de::Error::custom("unexpected summary"));
                }
                let op = op.ok_or_else(|| de::Error::missing_field("op"))?;
                let target = target.ok_or_else(|| de::Error::missing_field("target"))?.0;
                match kind.as_str() {
                    "Read" if expected.is_none() && replacement.is_none() => {
                        Ok(Action::Read { op, target })
                    }
                    "Verify" if replacement.is_none() => Ok(Action::Verify {
                        op,
                        target,
                        expected: expected
                            .ok_or_else(|| de::Error::missing_field("expected"))?
                            .0,
                    }),
                    "Replace" => Ok(Action::Replace {
                        op,
                        target,
                        expected: expected
                            .ok_or_else(|| de::Error::missing_field("expected"))?
                            .0,
                        replacement: replacement
                            .ok_or_else(|| de::Error::missing_field("replacement"))?
                            .0,
                    }),
                    _ => Err(de::Error::custom("invalid response kind/fields")),
                }
            }
        }
        deserializer.deserialize_map(Object)
    }
}

#[derive(Debug)]
pub struct CodingRun {
    pub summary: String,
    pub turns: u64,
    pub request_ids: Vec<RequestId>,
    pub edit_operation: u64,
    /// Live synthetic fixture usage only; not reconstructed or provider billing.
    pub usage_tokens: Option<u64>,
}

fn validate_project_action(
    action: &Action,
    turn: u64,
    goal: &crate::project::context::Goal,
) -> Result<(), CodingError> {
    let target = goal.evidence.request.target();
    let valid = match action {
        Action::Read { target: actual, .. } => turn == 1 && actual == target,
        Action::Replace {
            target: actual,
            expected,
            replacement,
            ..
        } => {
            turn == 2
                && actual == target
                && expected == &goal.expected
                && replacement == &goal.desired()?
        }
        Action::Verify {
            target: actual,
            expected,
            ..
        } => turn == 3 && actual == target && expected == &goal.desired()?,
        Action::Final { summary } => turn == 4 && summary.len() <= 512,
    };
    if !valid {
        return Err(CodingError::Invalid(
            "response does not satisfy captured project goal",
        ));
    }
    Ok(())
}
#[derive(Serialize)]
struct ProjectContext<'a> {
    session: &'a str,
    turn: u64,
    project: &'a crate::project::context::Evidence,
    previous: &'a str,
}
fn serialize_project_context(
    session: &str,
    turn: u64,
    project: &crate::project::context::Evidence,
    previous: &str,
) -> Result<String, CodingError> {
    let mut input = InputBuffer(Vec::with_capacity(4096));
    serde_json::to_writer(
        &mut input,
        &ProjectContext {
            session,
            turn,
            project,
            previous,
        },
    )
    .map_err(|_| {
        CodingError::Invalid("complete project context exceeds 4096 bytes; nothing truncated")
    })?;
    Ok(String::from_utf8(input.0).expect("JSON UTF-8"))
}

enum GoalMode {
    Exact(Arc<str>, Arc<str>),
    Project(Arc<crate::project::context::Goal>),
}

#[derive(Serialize)]
struct Context<'a> {
    session: &'a str,
    turn: u64,
    prompt: &'a str,
    previous: &'a str,
}

fn serialize_context(
    session: &str,
    turn: u64,
    prompt: &str,
    previous: &str,
) -> Result<String, CodingError> {
    let mut input = InputBuffer(Vec::with_capacity(4096));
    serde_json::to_writer(
        &mut input,
        &Context {
            session,
            turn,
            prompt,
            previous,
        },
    )
    .map_err(|_| CodingError::Invalid("request serialization ceiling"))?;
    Ok(String::from_utf8(input.0).expect("JSON UTF-8"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PriorContext {
    #[serde(serialize_with = "serialize_arc")]
    prompt: Arc<str>,
    #[serde(serialize_with = "serialize_arc")]
    summary: Arc<str>,
    #[serde(serialize_with = "serialize_arc")]
    target: Arc<str>,
    operation: u64,
    verified: bool,
}
fn serialize_arc<S: serde::Serializer>(text: &Arc<str>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(text)
}
struct OwnedContext {
    session: String,
    turn: u64,
    prompt: String,
    previous: String,
}
struct OwnedFollowContext {
    session: String,
    turn: u64,
    invocation: u64,
    prior: PriorContext,
    prompt: String,
    previous: String,
}
super::journal::object_decoder!(PriorContext {
    prompt: BoundedString<1024> => |v: BoundedString<1024>| Arc::from(v.0),
    summary: BoundedString<1024> => |v: BoundedString<1024>| Arc::from(v.0),
    target: BoundedString<64> => |v: BoundedString<64>| Arc::from(v.0),
    operation: u64 => |v| v,
    verified: bool => |v| v,
});
super::journal::object_decoder!(OwnedContext {
    session: BoundedString<64> => |v: BoundedString<64>| v.0,
    turn: u64 => |v| v,
    prompt: BoundedString<1024> => |v: BoundedString<1024>| v.0,
    previous: BoundedString<1024> => |v: BoundedString<1024>| v.0,
});
super::journal::object_decoder!(OwnedFollowContext {
    session: BoundedString<64> => |v: BoundedString<64>| v.0,
    turn: u64 => |v| v,
    invocation: u64 => |v| v,
    prior: PriorContext => |v| v,
    prompt: BoundedString<1024> => |v: BoundedString<1024>| v.0,
    previous: BoundedString<1024> => |v: BoundedString<1024>| v.0,
});
#[derive(Serialize)]
struct FollowContext<'a> {
    session: &'a str,
    turn: u64,
    invocation: u64,
    prior: &'a PriorContext,
    prompt: &'a str,
    previous: &'a str,
}
fn serialize_follow_context(
    session: &str,
    turn: u64,
    prior: &PriorContext,
    prompt: &str,
    previous: &str,
) -> Result<String, CodingError> {
    let mut input = InputBuffer(Vec::with_capacity(4096));
    serde_json::to_writer(
        &mut input,
        &FollowContext {
            session,
            turn,
            invocation: 2,
            prior,
            prompt,
            previous,
        },
    )
    .map_err(|_| CodingError::Invalid("follow-up context exceeds 4096 bytes; nothing truncated"))?;
    Ok(String::from_utf8(input.0).expect("JSON UTF-8"))
}
fn validate_follow_prompt(
    session: &str,
    prior: &PriorContext,
    prompt: &str,
) -> Result<(), CodingError> {
    CodingAgent::validate_prompt(session, prompt)?;
    serialize_follow_context(&"s".repeat(64), TURNS, prior, prompt, &"\n".repeat(1024))?;
    Ok(())
}

struct InputBuffer(Vec<u8>);
impl Write for InputBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > 4096 - self.0.len() {
            return Err(std::io::Error::other("request serialization ceiling"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct WorkerState {
    session: CodingSession,
    approval: Option<EditApproval>,
    cancellation: EditCancellation,
    turn: u64,
    read_target: Option<String>,
    edited: Option<(u64, String, String)>,
    verified: bool,
    shared: Arc<Shared>,
    interactive: bool,
    follow_up: bool,
    prompt: Arc<str>,
    prior: Option<Arc<PriorContext>>,
    exact_target: Option<Arc<str>>,
    project: Option<Arc<crate::project::context::Goal>>,
}

enum Work {
    AdmitFollowUp {
        reply: oneshot::Sender<Result<(), CodingError>>,
    },
    FollowUp {
        prompt: Arc<str>,
        reply: oneshot::Sender<Result<(), CodingError>>,
    },
    Request {
        turn: u64,
        text: String,
        reply: oneshot::Sender<Result<(), CodingError>>,
    },
    Response {
        turn: u64,
        id: RequestId,
        text: String,
        success: bool,
        reply: oneshot::Sender<Result<Step, CodingError>>,
    },
}
struct Step {
    output: String,
    final_edit: Option<u64>,
    prior: Option<Arc<PriorContext>>,
}

pub struct CodingAgent {
    sender: Option<SyncSender<Work>>,
    worker: Option<JoinHandle<()>>,
    session: String,
    prompt: Arc<str>,
    cancellation: EditCancellation,
    shared: Arc<Shared>,
    follow_up: bool,
    prior: Option<Arc<PriorContext>>,
    invocation: u64,
    ran: bool,
    runtime_owner: Option<Arc<crate::runtime::Shared>>,
    exact_replacement: bool,
    project: Option<Arc<crate::project::context::Goal>>,
}
#[derive(Clone)]
pub struct CodingFollowUpAdmission {
    session: Arc<str>,
    prior: Arc<PriorContext>,
    shared: Arc<Shared>,
}
impl CodingFollowUpAdmission {
    pub fn validate(&self, prompt: &str) -> Result<(), CodingError> {
        self.shared.checkpoint_eligible()?;
        validate_follow_prompt(&self.session, &self.prior, prompt)
    }
}
struct TransitionWait {
    cancellation: EditCancellation,
    shared: Arc<Shared>,
    acknowledged: bool,
}
impl Drop for TransitionWait {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.cancellation.cancel();
            self.shared
                .failure("follow-up acknowledgement lost; execution cancelled without retry");
        }
    }
}

impl WorkerState {
    fn shared_owner(&self) -> u64 {
        CodingObserver(self.shared.clone()).snapshot().owner
    }
    fn material(
        &mut self,
        turn: u64,
        id: u64,
        event: &str,
        status: &str,
        text: String,
    ) -> Result<(), CodingError> {
        let retained: Arc<str> = Arc::from(text.as_str());
        self.session.journal.append(Record::Material {
            entry: Material {
                turn,
                request: id,
                operation: if matches!(event, "Tool" | "Final") {
                    turn
                } else {
                    0
                },
                event: event.to_owned(),
                status: status.to_owned(),
                text,
            },
        })?;
        if event == "Request" {
            self.shared
                .update(|o| o.attempts[(turn - 1) as usize].recorded = true);
        }
        if matches!(status, "Read" | "Replaced" | "Verified" | "Final" | "Error") {
            let material = match status {
                "Read" => "Read",
                "Replaced" => "Replaced",
                "Verified" => "Verified",
                "Final" => "Final",
                _ => "Error",
            };
            self.shared.update(|o| {
                o.attempts[(turn - 1) as usize].material = Some(material);
                o.attempts[(turn - 1) as usize].output = Some(retained.clone());
                if material == "Verified" {
                    o.verified = true;
                }
                if material == "Final" {
                    o.final_summary = Some(retained);
                }
            });
        }
        Ok(())
    }
    fn execute(
        &mut self,
        turn: u64,
        id: RequestId,
        text: String,
        success: bool,
    ) -> Result<Step, CodingError> {
        let mut start = 0;
        while start < text.len() {
            let mut end = (start + 2048).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.material(
                turn,
                id.0,
                "ResponsePart",
                "Pending",
                text[start..end].to_owned(),
            )?;
            start = end;
        }
        self.material(
            turn,
            id.0,
            "Response",
            if success { "Success" } else { "Failed" },
            String::new(),
        )?;
        if !success {
            return Err(CodingError::Invalid(
                "model response did not complete successfully",
            ));
        }
        let result = (|| {
            if self.cancellation.is_cancelled() {
                return Err(CodingError::Cancelled);
            }
            let action: Action = serde_json::from_str(&text)
                .map_err(|_| CodingError::Invalid("invalid strict completed response"))?;
            let (op, target) = match &action {
                Action::Read { op, target }
                | Action::Replace { op, target, .. }
                | Action::Verify { op, target, .. } => (*op, Some(target)),
                Action::Final { .. } => (turn, None),
            };
            if op != turn {
                return Err(CodingError::Stale);
            }
            if let Some(target) = target {
                validate_name(target)?;
            }
            if let Some(goal) = &self.project {
                validate_project_action(&action, turn, goal)?;
            }
            if let Some(goal) = &self.exact_target {
                let valid = match &action {
                    Action::Read { target, .. } => turn == 1 && target == goal.as_ref(),
                    Action::Replace {
                        target,
                        replacement,
                        ..
                    } => {
                        turn == 2 && target == goal.as_ref() && replacement == self.prompt.as_ref()
                    }
                    Action::Verify {
                        target, expected, ..
                    } => turn == 3 && target == goal.as_ref() && expected == self.prompt.as_ref(),
                    Action::Final { .. } => turn == 4,
                };
                if !valid {
                    return Err(CodingError::Invalid(
                        "response does not satisfy exact replacement goal",
                    ));
                }
            }
            match action {
                Action::Read { target, .. } => {
                    let output = self.session.read(&target)?;
                    self.read_target = Some(target);
                    self.material(turn, id.0, "Tool", "Read", output.clone())?;
                    Ok(Step {
                        output,
                        final_edit: None,
                        prior: None,
                    })
                }
                Action::Replace {
                    target,
                    expected,
                    replacement,
                    ..
                } => {
                    if self.read_target.as_ref() != Some(&target) || self.edited.is_some() {
                        return Err(CodingError::Stale);
                    }
                    let intent = self.session.intent(&target, &expected, &replacement)?;
                    let approval = if self.interactive {
                        let proposal = CodingProposal {
                            owner: self.shared_owner(),
                            invocation: CodingObserver(self.shared.clone()).snapshot().invocation,
                            request_id: id,
                            turn,
                            intent: intent.clone(),
                        };
                        match self.shared.decide(proposal)? {
                            CodingDecision::Approve => self.session.approve_exact(&intent)?,
                            CodingDecision::Deny => return Err(CodingError::Denied),
                        }
                    } else {
                        self.approval.take().ok_or(CodingError::Denied)?
                    };
                    #[cfg(test)]
                    self.session
                        .checkpoint_at(super::CheckpointPhase::BeforeAuthorization)?;
                    self.session
                        .authorize(intent, approval, &self.cancellation)?;
                    #[cfg(test)]
                    self.session
                        .checkpoint_at(super::CheckpointPhase::AfterAuthorization)?;
                    let result = self.session.dispatch(&self.cancellation)?;
                    #[cfg(test)]
                    self.session
                        .checkpoint_at(super::CheckpointPhase::AfterCompletion)?;
                    if result.state != OperationState::Completed {
                        return Err(result
                            .acknowledgement_error
                            .unwrap_or(CodingError::Cancelled));
                    }
                    let output = if self.project.is_some() {
                        replacement.clone()
                    } else {
                        "replacement durably completed".to_owned()
                    };
                    self.edited = Some((result.operation, target, replacement));
                    self.material(turn, id.0, "Tool", "Replaced", output.clone())?;
                    Ok(Step {
                        output,
                        final_edit: None,
                        prior: None,
                    })
                }
                Action::Verify {
                    target, expected, ..
                } => {
                    if self
                        .edited
                        .as_ref()
                        .is_none_or(|(_, t, text)| t != &target || text != &expected)
                    {
                        return Err(CodingError::TargetMismatch);
                    }
                    if self.session.read(&target)? != expected {
                        return Err(CodingError::TargetMismatch);
                    }
                    self.verified = true;
                    #[cfg(test)]
                    self.session
                        .checkpoint_at(super::CheckpointPhase::BeforeVerificationMaterial)?;
                    let verified_identity = if self.project.is_some() {
                        let file = crate::project::read_file(&self.session.root, &target)?;
                        if Some(file.identity) != self.session.selected_identity
                            || file.content != expected
                        {
                            return Err(CodingError::TargetMismatch);
                        }
                        Some(file.identity)
                    } else {
                        None
                    };
                    self.material(
                        turn,
                        id.0,
                        "Tool",
                        "Verified",
                        "exact content verified".to_owned(),
                    )?;
                    if let Some(identity) = verified_identity {
                        self.session.journal.append(Record::Material {
                            entry: Material {
                                turn,
                                request: id.0,
                                operation: 0,
                                event: "VerifiedIdentity".into(),
                                status: "Captured".into(),
                                text: serde_json::to_string(&identity)
                                    .expect("fixed verified identity"),
                            },
                        })?;
                        self.shared.update(|o| o.verified_identity = Some(identity));
                    }
                    Ok(Step {
                        output: "exact content verified".to_owned(),
                        final_edit: None,
                        prior: None,
                    })
                }
                Action::Final { summary } => {
                    if !self.verified {
                        return Err(CodingError::Invalid(
                            "final without acknowledged edit and verification",
                        ));
                    }
                    if summary.chars().any(char::is_control) {
                        return Err(CodingError::Invalid("final summary contains controls"));
                    }
                    let operation = self.edited.as_ref().unwrap().0;
                    #[cfg(test)]
                    self.session
                        .checkpoint_at(super::CheckpointPhase::BeforeFinalMaterial)?;
                    self.material(turn, id.0, "Final", "Final", summary.clone())?;
                    if self.follow_up
                        && turn == 4
                        && operation == 1
                        && self.edited.as_ref().unwrap().2 == self.prompt.as_ref()
                    {
                        self.prior = Some(Arc::new(PriorContext {
                            prompt: self.prompt.clone(),
                            summary: Arc::from(summary.as_str()),
                            target: Arc::from(self.edited.as_ref().unwrap().1.as_str()),
                            operation,
                            verified: true,
                        }));
                    }
                    Ok(Step {
                        output: summary,
                        prior: self.prior.clone(),
                        final_edit: Some(operation),
                    })
                }
            }
        })();
        if result.is_err() && !self.session.journal.poisoned && self.session.current.is_none() {
            self.material(
                turn,
                id.0,
                "Tool",
                "Error",
                "tool/schema/authority/cancellation failure".to_owned(),
            )?;
        }
        result
    }
}

impl CodingAgent {
    pub fn validate_prompt(session: &str, prompt: &str) -> Result<(), CodingError> {
        if prompt.len() > 1024 {
            return Err(CodingError::Invalid("prompt exceeds 1024 UTF8 bytes"));
        }
        validate_name(session)?;
        if prompt.chars().any(|c| c.is_control() && c != '\n') {
            return Err(CodingError::Invalid("prompt contains unsupported controls"));
        }
        serialize_context(session, TURNS, prompt, &"\n".repeat(1024))?;
        Ok(())
    }

    pub fn start(
        session: CodingSession,
        approval: Option<EditApproval>,
        cancellation: EditCancellation,
        prompt: &str,
    ) -> Result<Self, CodingError> {
        Self::start_mode(session, approval, cancellation, false, false, prompt, None)
    }

    pub fn start_interactive(
        session: CodingSession,
        cancellation: EditCancellation,
        prompt: &str,
    ) -> Result<Self, CodingError> {
        Self::start_mode(session, None, cancellation, true, false, prompt, None)
    }

    pub fn start_follow_up(
        session: CodingSession,
        cancellation: EditCancellation,
        prompt: &str,
    ) -> Result<Self, CodingError> {
        Self::start_mode(session, None, cancellation, true, true, prompt, None)
    }

    pub(crate) fn start_exact_replacement(
        session: CodingSession,
        cancellation: EditCancellation,
        prompt: Arc<str>,
        target: Arc<str>,
    ) -> Result<Self, CodingError> {
        validate_name(&target)?;
        Self::start_mode(
            session,
            None,
            cancellation,
            true,
            false,
            &prompt,
            Some(GoalMode::Exact(prompt.clone(), target)),
        )
    }

    pub(crate) fn start_project(
        session: CodingSession,
        cancellation: EditCancellation,
        goal: crate::project::context::Goal,
    ) -> Result<Self, CodingError> {
        Self::start_mode(
            session,
            None,
            cancellation,
            true,
            false,
            "replace_once",
            Some(GoalMode::Project(Arc::new(goal))),
        )
    }

    pub fn validate_follow_up(&self, prompt: &str) -> Result<(), CodingError> {
        if !self.follow_up || self.invocation != 1 || !self.ran || self.cancellation.is_cancelled()
        {
            return Err(CodingError::Stale);
        }
        self.shared.checkpoint_eligible()?;
        let prior = self.prior.as_ref().ok_or(CodingError::Stale)?;
        validate_follow_prompt(&self.session, prior, prompt)
    }

    pub async fn follow_up_admission(&self) -> Result<CodingFollowUpAdmission, CodingError> {
        self.validate_follow_up("")?;
        let (reply, receive) = oneshot::channel();
        self.sender
            .as_ref()
            .unwrap()
            .try_send(Work::AdmitFollowUp { reply })
            .map_err(|_| CodingError::Invalid("serialized worker unavailable"))?;
        receive
            .await
            .map_err(|_| CodingError::Invalid("checkpoint acknowledgement lost"))??;
        self.validate_follow_up("")?;
        Ok(CodingFollowUpAdmission {
            session: Arc::from(self.session.as_str()),
            prior: self.prior.as_ref().unwrap().clone(),
            shared: self.shared.clone(),
        })
    }

    /// Caller must first acknowledge its owned fixture handlers; this checks coding and runtime eligibility.
    pub async fn follow_up(&mut self, runtime: &Runtime, prompt: &str) -> Result<(), CodingError> {
        self.validate_follow_up(prompt)?;
        if self
            .runtime_owner
            .as_ref()
            .is_none_or(|owner| !Arc::ptr_eq(owner, &runtime.ownership()))
        {
            return Err(CodingError::Stale);
        }
        let state = runtime.snapshot();
        if state.active
            + state.pending
            + state.handles
            + state.input_bytes
            + state.delta_items
            + state.delta_bytes
            + state.potential
            + state.unresolved
            + state.usage_obligations
            + state.token_debt as usize
            != 0
        {
            return Err(CodingError::Invalid(
                "follow-up retains runtime obligations",
            ));
        }
        self.transition(prompt).await
    }

    async fn transition(&mut self, prompt: &str) -> Result<(), CodingError> {
        let mut wait = TransitionWait {
            cancellation: self.cancellation.clone(),
            shared: self.shared.clone(),
            acknowledged: false,
        };
        let prompt: Arc<str> = Arc::from(prompt);
        let (reply, receive) = oneshot::channel();
        self.sender
            .as_ref()
            .unwrap()
            .try_send(Work::FollowUp {
                prompt: prompt.clone(),
                reply,
            })
            .map_err(|_| CodingError::Invalid("serialized worker unavailable"))?;
        receive
            .await
            .map_err(|_| CodingError::Invalid("worker lost follow-up acknowledgement"))??;
        self.prompt = prompt;
        self.invocation = 2;
        self.ran = false;
        wait.acknowledged = true;
        Ok(())
    }

    pub fn observer(&self) -> CodingObserver {
        CodingObserver(self.shared.clone())
    }
    pub fn control(&self) -> CodingControl {
        CodingControl(self.shared.clone(), self.invocation)
    }

    fn start_mode(
        mut session: CodingSession,
        approval: Option<EditApproval>,
        cancellation: EditCancellation,
        interactive: bool,
        follow_up: bool,
        prompt: &str,
        goal: Option<GoalMode>,
    ) -> Result<Self, CodingError> {
        let (exact_goal, project) = match goal {
            Some(GoalMode::Exact(prompt, target)) => (Some((prompt, target)), None),
            Some(GoalMode::Project(project)) => (None, Some(project)),
            None => (None, None),
        };
        Self::validate_prompt(&session.session, prompt)?;
        let exact_replacement = exact_goal.is_some() || project.is_some();
        if let Some(goal) = &project {
            goal.validate()?;
            if session
                .selected
                .as_ref()
                .is_none_or(|capture| !goal.matches_capture(capture))
            {
                return Err(CodingError::Stale);
            }
            session.recheck_selected()?;
            serialize_project_context(&"s".repeat(64), 4, &goal.evidence, &"\n".repeat(1024))?;
            let text = serde_json::to_string(goal.as_ref())
                .map_err(|_| CodingError::Invalid("project goal serialization"))?;
            if text.len() > 4096 {
                return Err(CodingError::Invalid("project goal record ceiling"));
            }
            session.journal.append(Record::Material {
                entry: Material {
                    turn: 0,
                    request: 0,
                    operation: 0,
                    event: "ProjectGoal".into(),
                    status: "Captured".into(),
                    text,
                },
            })?;
        }
        let worker_project = project.clone();
        let (prompt, exact_target) = match exact_goal {
            Some((prompt, target)) => (prompt, Some(target)),
            None => (Arc::from(prompt), None),
        };
        let worker_prompt = prompt.clone();
        if session.next_operation != 1 || session.current.is_some() || session.journal.poisoned {
            return Err(CodingError::Stale);
        }
        let owner = OWNERS
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| CodingError::Invalid("coding owner identity exhausted"))?;
        let shared = Shared::new(owner, cancellation.clone());
        session.observation = Some(shared.clone());
        let worker_shared = shared.clone();
        let session_name = session.session.clone();
        let (sender, receiver) = sync_channel(1);
        let cancel = cancellation.clone();
        let worker = std::thread::Builder::new()
            .name("coding-owner".to_owned())
            .spawn(move || {
                let mut state = WorkerState {
                    session,
                    approval,
                    cancellation: cancel,
                    turn: 0,
                    read_target: None,
                    edited: None,
                    verified: false,
                    shared: worker_shared,
                    interactive,
                    follow_up,
                    prompt: worker_prompt,
                    prior: None,
                    exact_target,
                    project: worker_project,
                };
                let mut failed = false;
                while let Ok(work) = receiver.recv() {
                    match work {
                        Work::AdmitFollowUp { reply } => {
                            let result = (|| {
                                if failed
                                    || !state.follow_up
                                    || state.turn != 4
                                    || state.session.current.is_some()
                                    || state.prior.is_none()
                                    || !state.verified
                                    || state.session.next_operation != 2
                                {
                                    return Err(CodingError::Stale);
                                }
                                state.shared.checkpoint_eligible()?;
                                state.session.journal.ensure_follow_up_capacity()
                            })();
                            let _ = reply.send(result);
                        }
                        Work::FollowUp { prompt, reply } => {
                            let result = (|| {
                                if failed
                                    || !state.follow_up
                                    || state.turn != 4
                                    || state.prior.is_none()
                                    || state.session.current.is_some()
                                    || !state.verified
                                    || state.session.next_operation != 2
                                    || CodingObserver(state.shared.clone()).snapshot().invocation
                                        != 1
                                {
                                    return Err(CodingError::Stale);
                                }
                                if state.cancellation.is_cancelled() {
                                    return Err(CodingError::Cancelled);
                                }
                                validate_follow_prompt(
                                    &state.session.session,
                                    state.prior.as_ref().unwrap(),
                                    &prompt,
                                )?;
                                state.shared.checkpoint_eligible()?;
                                state.session.journal.ensure_follow_up_capacity()?;
                                #[cfg(test)]
                                state.session.checkpoint_at(
                                    super::CheckpointPhase::BeforeFollowUpMaterial,
                                )?;
                                state.session.journal.append(Record::FollowUp {
                                    invocation: 2,
                                    after_turn: 4,
                                    prompt: prompt.to_string(),
                                })?;
                                #[cfg(test)]
                                state
                                    .session
                                    .checkpoint_at(super::CheckpointPhase::AfterFollowUpMaterial)?;
                                state.shared.advance()?;
                                state.prompt = prompt;
                                state.read_target = None;
                                state.edited = None;
                                state.verified = false;
                                state.approval = None;
                                Ok(())
                            })();
                            if result.is_err() {
                                failed = true;
                                state.shared.failure(
                                    "follow-up transition failed; further execution refused",
                                );
                            }
                            if reply.send(result).is_err() {
                                failed = true;
                                state.cancellation.cancel();
                                state.shared.failure(
                                    "follow-up acknowledgement receiver lost; execution cancelled",
                                );
                            }
                        }
                        Work::Request { turn, text, reply } => {
                            let result = (|| {
                                if failed {
                                    return Err(CodingError::Poisoned);
                                }
                                if state.cancellation.is_cancelled() {
                                    return Err(CodingError::Cancelled);
                                }
                                if turn != state.turn + 1 || turn > TURNS || text.len() > 4096 {
                                    return Err(CodingError::Stale);
                                }
                                state.session.journal.ensure_turn_capacity()?;
                                state.material(turn, 0, "Request", "Pending", text)?;
                                state.turn = turn;
                                Ok(())
                            })();
                            failed |= result.is_err();
                            let _ = reply.send(result);
                        }
                        Work::Response {
                            turn,
                            id,
                            text,
                            success,
                            reply,
                        } => {
                            let result =
                                if failed || turn != state.turn || text.len() > RESPONSE_BYTES {
                                    Err(CodingError::Stale)
                                } else {
                                    state.execute(turn, id, text, success)
                                };
                            failed |= result.is_err();
                            let _ = reply.send(result);
                        }
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            session: session_name,
            prompt,
            cancellation,
            shared,
            follow_up,
            prior: None,
            invocation: 1,
            ran: false,
            runtime_owner: None,
            exact_replacement,
            project,
        })
    }

    pub async fn run(mut self, runtime: &Runtime) -> Result<CodingRun, CodingError> {
        let result = self.run_unjoined(runtime).await;
        self.finish()?;
        result
    }

    /// Returns the logical outcome; finish or Drop still must settle the filesystem owner.
    pub async fn run_unjoined(&mut self, runtime: &Runtime) -> Result<CodingRun, CodingError> {
        let result = self.run_inner(runtime).await;
        if let Err(error) = &result
            && !matches!(error, CodingError::Denied | CodingError::Cancelled)
        {
            self.shared.failure(&error.to_string());
        }
        self.shared.update(|o| o.run_ended = true);
        result
    }

    pub fn finish(mut self) -> Result<(), CodingError> {
        self.cancellation.cancel();
        self.join()
    }

    async fn run_inner(&mut self, runtime: &Runtime) -> Result<CodingRun, CodingError> {
        if self.ran {
            return Err(CodingError::Stale);
        }
        self.ran = true;
        if self.follow_up {
            let owner = runtime.ownership();
            if self
                .runtime_owner
                .as_ref()
                .is_some_and(|current| !Arc::ptr_eq(current, &owner))
            {
                return Err(CodingError::Stale);
            }
            self.runtime_owner = Some(owner);
        }
        match drive(DriverMode::Fixture(self), runtime).await? {
            DriverRun::Fixture(run) => Ok(run),
            DriverRun::Haiku(_) | DriverRun::Invocation => unreachable!(),
        }
    }

    fn join(&mut self) -> Result<(), CodingError> {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let joined = worker.join().is_ok();
            self.shared.update(|o| {
                o.worker_terminated = true;
                o.worker_joined = joined;
            });
            if !joined {
                self.shared
                    .failure("coding owner panicked; material acknowledgement unavailable");
                return Err(CodingError::Invalid("coding owner panicked"));
            }
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

impl Drop for CodingAgent {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let _ = self.join();
        self.shared.update(|o| o.run_ended = true);
    }
}
