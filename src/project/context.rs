use super::*;
use crate::coding::journal::{BoundedString, object_decoder};
use crate::{ReaderState, readers::ReaderLogical};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, Visitor},
};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReaderEvidence {
    pub child: u8,
    pub request: u64,
    pub summary: String,
}
object_decoder!(ReaderEvidence {
    child: u8 => |v| v,
    request: u64 => |v| v,
    summary: BoundedString<512> => |v: BoundedString<512>| v.0,
});
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Evidence {
    pub request: ProjectRequest,
    pub root: RootIdentity,
    pub capture: [RootIdentity; 3],
    pub reader_session: String,
    pub generation: u64,
    pub readers: [ReaderEvidence; 3],
}
object_decoder!(Evidence {
    request: ProjectRequest => |v| v,
    root: RootIdentity => |v| v,
    capture: [RootIdentity; 3] => |v| v,
    reader_session: BoundedString<64> => |v: BoundedString<64>| v.0,
    generation: u64 => |v| v,
    readers: [ReaderEvidence; 3] => |v| v,
});
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Goal {
    pub evidence: Evidence,
    pub expected: String,
}
object_decoder!(Goal {
    evidence: Evidence => |v| v,
    expected: BoundedString<1024> => |v: BoundedString<1024>| v.0,
});
impl Goal {
    pub fn from_readers(
        capture: &super::journal::ProjectCaptured,
        root: RootIdentity,
        logical: &ReaderLogical,
    ) -> Result<Self, CodingError> {
        let (tree, _) = logical
            .outcome
            .as_ref()
            .map_err(|_| CodingError::Invalid("reader logical result failed"))?;
        let observation = &logical.observation;
        if tree.session != capture.reader_session
            || tree.generation != capture.journey as u64
            || !tree.parent_finished
            || !observation.tree_ended
            || tree.session != observation.session
            || tree.generation != observation.generation
        {
            return Err(CodingError::Invalid("reader End/correlation unavailable"));
        }
        let mut readers = Vec::with_capacity(3);
        for (index, fact) in tree.children.iter().enumerate() {
            let slot = &observation.children[index];
            if fact.child as usize != index
                || slot.fact.as_ref() != Some(fact)
                || slot.request_id != fact.request_id
                || slot.acceptance_ordinal.is_none()
            {
                return Err(CodingError::Invalid("uncorrelated reader acceptance"));
            }
            let ReaderState::Completed { summary } = &fact.state else {
                return Err(CodingError::Invalid(
                    "all three actual readers must complete",
                ));
            };
            readers.push(ReaderEvidence {
                child: fact.child,
                request: fact
                    .request_id
                    .ok_or(CodingError::Invalid("missing reader request"))?
                    .0,
                summary: summary.clone(),
            });
        }
        let goal = Self {
            evidence: Evidence {
                request: capture.request.clone(),
                root,
                capture: capture.files.each_ref().map(|f| f.identity),
                reader_session: tree.session.clone(),
                generation: tree.generation,
                readers: readers.try_into().unwrap(),
            },
            expected: capture.files[0].content.clone(),
        };
        goal.validate()?;
        Ok(goal)
    }
    pub fn validate(&self) -> Result<(), CodingError> {
        self.evidence.request.validate()?;
        crate::coding::validate_session(&self.evidence.reader_session)?;
        validate_content(&self.expected)?;
        self.desired()?;
        if !matches!(self.evidence.generation, 1 | 2) {
            return Err(CodingError::Invalid("project generation ceiling"));
        }
        for (index, reader) in self.evidence.readers.iter().enumerate() {
            if reader.child as usize != index
                || reader.request == 0
                || reader.summary.len() > 512
                || reader.summary.chars().any(char::is_control)
                || self.evidence.readers[..index]
                    .iter()
                    .any(|r| r.request == reader.request)
            {
                return Err(CodingError::Invalid("invalid complete reader evidence"));
            }
        }
        Ok(())
    }
    pub fn desired(&self) -> Result<String, CodingError> {
        self.evidence.request.derive(&self.expected)
    }
    pub fn matches_capture(&self, capture: &ProjectCapture) -> bool {
        self.evidence.root == capture.root_identity()
            && self.evidence.request == capture.request
            && self.evidence.capture == capture.files.each_ref().map(|f| f.identity)
            && self.expected == capture.expected()
    }
}
#[derive(Serialize)]
pub(crate) struct Context {
    pub session: String,
    pub turn: u64,
    pub project: Evidence,
    pub previous: String,
}
object_decoder!(Context {
    session: BoundedString<64> => |v: BoundedString<64>| v.0,
    turn: u64 => |v| v,
    project: Evidence => |v| v,
    previous: BoundedString<1024> => |v: BoundedString<1024>| v.0,
});
