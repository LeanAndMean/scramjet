//! The project agents the user approved running, remembered across sessions as the SHA-256 of
//! each agent file's text when they said yes, in `$XDG_STATE_HOME/scramjet-rs/approved-agents.json`
//! (`{"<agent file>": "<sha256>"}`). An agent file added or changed since is asked about again.

use super::subagent::Definition;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Where approvals are kept.
pub(super) fn path() -> Option<PathBuf> {
    let state = super::session::state_directory().ok()?;
    Some(state.join("approved-agents.json"))
}

/// The SHA-256 of an agent file's text, as approvals record it.
pub(super) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// The approvals of a file; none when there is none or it cannot be read, so that every
/// project agent is asked about again.
pub(super) struct Approvals(Map<String, Value>);

impl Approvals {
    pub fn read(path: Option<&Path>) -> Self {
        let text = path.and_then(|path| std::fs::read_to_string(path).ok());
        match text.and_then(|text| serde_json::from_str(&text).ok()) {
            Some(Value::Object(files)) => Self(files),
            _ => Self(Map::new()),
        }
    }

    /// Whether `agent`'s file is approved as it is now.
    pub fn approved(&self, agent: &Definition) -> bool {
        self.0.get(&key(agent)).and_then(Value::as_str) == Some(agent.digest.as_str())
    }

    /// Whether `agent`'s file was approved, as it is now or before a change.
    pub fn known(&self, agent: &Definition) -> bool {
        self.0.contains_key(&key(agent))
    }
}

/// Records `agents` as approved as they are now, keeping the other approvals in `path`.
pub(super) fn remember(path: &Path, agents: &[&Definition]) -> io::Result<()> {
    let Approvals(mut files) = Approvals::read(Some(path));
    for agent in agents {
        files.insert(key(agent), Value::String(agent.digest.clone()));
    }
    let text = serde_json::to_string_pretty(&Value::Object(files)).map_err(io::Error::other)?;
    super::tools::write_replacing(path, &(text + "\n"))
}

fn key(agent: &Definition) -> String {
    agent.path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;

    #[test]
    fn a_damaged_file_approves_nothing_and_the_next_approval_replaces_it() {
        assert_eq!(
            digest("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-approvals-{}", std::process::id())),
        );
        let path = scratch.0.join("state/approved-agents.json");
        let agent = Definition {
            name: "p".into(),
            description: "P".into(),
            tools: None,
            model: None,
            prompt: String::new(),
            source: "project",
            path: PathBuf::from("/repo/.scramjet/agents/p.md"),
            digest: digest("p"),
            diagnostics: Vec::new(),
        };
        assert!(!Approvals::read(Some(&path)).known(&agent));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"/repo/.scramjet/agents/p.md\": ").unwrap();
        assert!(!Approvals::read(Some(&path)).known(&agent));
        remember(&path, &[&agent]).unwrap();
        let approved = Approvals::read(Some(&path));
        assert!(approved.approved(&agent));
        let changed = Definition {
            digest: digest("p, changed"),
            ..agent.clone()
        };
        assert!(approved.known(&changed) && !approved.approved(&changed));
    }
}
