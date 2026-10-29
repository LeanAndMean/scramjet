use super::{HaikuEffect, HaikuIntent, HaikuProcessResult, journal::Record};
use crate::{CodingError, Outcome, ProviderTerminalEvidence, ProviderUsage};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HaikuOutcome {
    Success,
    HttpRejected(u16),
    Framing,
    Unsupported,
    Incomplete,
    Transport,
    ResourceLimit,
    Cancelled,
    ConsumerLost,
    Deadline,
}
impl From<&Outcome> for HaikuOutcome {
    fn from(value: &Outcome) -> Self {
        match value {
            Outcome::Success => Self::Success,
            Outcome::HttpRejected(code) => Self::HttpRejected(*code),
            Outcome::Framing => Self::Framing,
            Outcome::Unsupported => Self::Unsupported,
            Outcome::Incomplete => Self::Incomplete,
            Outcome::Transport => Self::Transport,
            Outcome::ResourceLimit => Self::ResourceLimit,
            Outcome::Cancelled => Self::Cancelled,
            Outcome::ConsumerLost => Self::ConsumerLost,
            Outcome::Deadline => Self::Deadline,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaikuAttempt {
    pub turn: usize,
    pub request_id: u64,
    pub response_request_id: u64,
    pub ordinal: Option<usize>,
    pub outcome: HaikuOutcome,
    pub usage: Option<ProviderUsage>,
    pub terminal: Option<ProviderTerminalEvidence>,
    pub durable: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HaikuCertainty {
    Authorized,
    NotDispatched,
    Uncertain,
    Completed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HaikuFact {
    pub operation: usize,
    pub target: String,
    pub state: HaikuCertainty,
}
#[derive(Clone, Debug)]
pub struct HaikuHistoryEntry {
    pub sequence: usize,
    pub record: Arc<Record>,
    pub acknowledged: bool,
}
#[derive(Clone, Debug)]
pub struct HaikuHistory {
    pub session: String,
    pub root: [u64; 2],
    pub entries: Vec<HaikuHistoryEntry>,
}
impl HaikuHistory {
    pub fn effects(&self) -> Vec<HaikuFact> {
        let mut facts: Vec<HaikuFact> = Vec::with_capacity(3);
        for entry in &self.entries {
            match entry.record.as_ref() {
                Record::Authorized { intent } | Record::Denied { intent } => {
                    facts.push(HaikuFact {
                        operation: intent.operation,
                        target: match &intent.effect {
                            HaikuEffect::Create { path, .. } => path.clone(),
                            HaikuEffect::Process { .. } => "bash".into(),
                        },
                        state: if matches!(entry.record.as_ref(), Record::Denied { .. }) {
                            HaikuCertainty::NotDispatched
                        } else if entry.acknowledged {
                            HaikuCertainty::Authorized
                        } else {
                            HaikuCertainty::NotDispatched
                        },
                    });
                }
                Record::Dispatched { operation } => {
                    if let Some(fact) = facts.iter_mut().find(|f| f.operation == *operation) {
                        fact.state = HaikuCertainty::Uncertain;
                    }
                }
                Record::NotDispatched { operation } => {
                    if let Some(fact) = facts.iter_mut().find(|f| f.operation == *operation) {
                        fact.state = HaikuCertainty::NotDispatched;
                    }
                }
                Record::Created { operation, .. } | Record::Process { operation, .. } => {
                    if let Some(fact) = facts.iter_mut().find(|f| f.operation == *operation)
                        && entry.acknowledged
                    {
                        fact.state = HaikuCertainty::Completed;
                    }
                }
                _ => {}
            }
        }
        facts
    }
    pub fn attempts(&self) -> Vec<HaikuAttempt> {
        self.entries
            .iter()
            .filter_map(|entry| {
                if let Record::Receipt { attempt } = entry.record.as_ref() {
                    Some(attempt.clone())
                } else {
                    None
                }
            })
            .collect()
    }
    pub fn process(&self) -> Option<&HaikuProcessResult> {
        self.entries.iter().rev().find_map(|entry| {
            if let Record::Process { result, .. } = entry.record.as_ref() {
                Some(result)
            } else {
                None
            }
        })
    }
    pub fn ending_seen(&self) -> bool {
        self.entries
            .last()
            .is_some_and(|entry| matches!(entry.record.as_ref(), Record::Finished))
    }
    pub fn finished(&self) -> bool {
        self.ending_seen() && self.entries.last().is_some_and(|entry| entry.acknowledged)
    }
}

pub(super) fn validate_intent(
    intent: &HaikuIntent,
    session: &str,
    root: [u64; 2],
) -> Result<(), CodingError> {
    if intent.session != session
        || intent.root != root
        || intent.operation == 0
        || intent.operation > 8
        || intent.ordinal == 0
        || intent.ordinal > crate::PROVIDER_POST_BUDGET
        || intent.request_id == 0
    {
        return Err(CodingError::History("foreign haiku intent provenance"));
    }
    match &intent.effect {
        HaikuEffect::Create {
            path,
            content,
            mode,
        } => {
            let slot = super::name(path)?;
            if *mode != 0o600
                || content.len() > 512
                || (slot == 1 && content != super::HAIKU_SCRIPT)
            {
                return Err(CodingError::History("unsupported haiku creation intent"));
            }
        }
        HaikuEffect::Process {
            command,
            captures,
            profile,
        } => {
            if command != super::HAIKU_COMMAND || profile != super::process::PROFILE {
                return Err(CodingError::History("unsupported haiku process intent"));
            }
            for (file, path) in captures.iter().zip(super::NAMES) {
                if file.path != path || file.content.len() > 512 {
                    return Err(CodingError::History("unsupported haiku capture"));
                }
            }
            if captures[1].content != super::HAIKU_SCRIPT {
                return Err(CodingError::History("unreviewed recovered script"));
            }
        }
    }
    Ok(())
}
