//! A session profile: launch options for one ACP session, from `session/new`'s, `resume`'s or
//! `load`'s `_meta` (RPC.md section 3), saved in the session file and applied again.

use super::super::{args::Options, settings::Settings};
use scramjet::agent::{
    Agent, CODING_TOOLS, HARNESS_TOOLS, Reach, Reads, Sandbox, SandboxMode, Subagents,
    missing_shells,
};
use serde_json::{Map, Value};
use std::path::PathBuf;

/// The keys of the second step (RPC.md section 3), refused until it is built.
const LATER: &[&str] = &["model", "thinkingLevel", "maxTokens"];

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Profile {
    /// As given, which the session file keeps.
    pub(super) given: Value,
    system: Option<String>,
    append: Option<Vec<String>>,
    context_files: Option<bool>,
    directives: Option<bool>,
    tools: Option<Vec<String>>,
    reach: Option<Confined>,
}

/// The reach a profile gives: the minimal one with `read`'s paths, and the sandbox it runs in.
#[derive(Clone, Debug, Default, PartialEq)]
struct Confined {
    read: Vec<PathBuf>,
    writable: bool,
    allow_write: Vec<PathBuf>,
    network: bool,
}

impl Profile {
    /// The profile `given` sets; refused, naming the key, for a key not listed or a value of the
    /// wrong type, so that a misspelt key never runs a task with more than meant.
    pub(super) fn parse(given: &Value) -> Result<Self, String> {
        let object = given.as_object().ok_or("profile must be an object")?;
        for key in object.keys() {
            if LATER.contains(&key.as_str()) {
                return Err(format!("profile.{key} is not supported yet"));
            }
        }
        let mut profile = Profile {
            given: given.clone(),
            system: text(object, "systemPrompt")?,
            append: match object.get("appendSystemPrompt") {
                None => None,
                Some(Value::String(text)) => Some(vec![text.clone()]),
                Some(_) => Some(strings(object, "appendSystemPrompt")?.unwrap_or_default()),
            },
            context_files: boolean(object, "contextFiles")?,
            directives: boolean(object, "directives")?,
            tools: strings(object, "tools")?,
            reach: None,
        };
        let read = paths(object, "read")?;
        let writable = boolean(object, "writable")?;
        let allow_write = paths(object, "allowWrite")?;
        let network = boolean(object, "network")?;
        if read.is_some() || writable.is_some() || allow_write.is_some() || network.is_some() {
            profile.reach = Some(Confined {
                read: read.unwrap_or_default(),
                writable: writable.unwrap_or(false),
                allow_write: allow_write.unwrap_or_default(),
                network: network.unwrap_or(false),
            });
        }
        const KNOWN: &[&str] = &[
            "systemPrompt",
            "appendSystemPrompt",
            "contextFiles",
            "directives",
            "tools",
            "read",
            "writable",
            "allowWrite",
            "network",
        ];
        if let Some(key) = object.keys().find(|key| !KNOWN.contains(&key.as_str())) {
            return Err(format!("profile.{key} is not a profile key"));
        }
        if let Some(tools) = &profile.tools {
            let unknown = (tools.iter()).find(|name| {
                !CODING_TOOLS.contains(&name.as_str()) && !HARNESS_TOOLS.contains(&name.as_str())
            });
            if let Some(name) = unknown {
                return Err(format!(
                    "profile.tools: {name} is not one of Scramjet's tools"
                ));
            }
            if let Some(missing) = missing_shells(tools).into_iter().next() {
                return Err(format!("profile.tools: {missing}"));
            }
        }
        Ok(profile)
    }

    /// The launch options `options` with this profile's instructions and tools over them.
    pub(super) fn apply(&self, options: &mut Options) {
        let prompt = &mut options.prompt;
        if let Some(system) = &self.system {
            (prompt.system, prompt.system_text) = (Some(system.clone()), true);
        }
        if let Some(append) = &self.append {
            (prompt.append, prompt.append_text) = (append.clone(), true);
        }
        if let Some(context_files) = self.context_files {
            prompt.no_context_files = !context_files;
        }
        if let Some(directives) = self.directives {
            prompt.no_directives = !directives;
        }
        if let Some(tools) = &self.tools {
            options.only_tools = Some(tools.clone());
        }
    }

    /// Whether this profile sets the session's reach, which `/sandbox` then cannot change.
    pub(super) fn confines(&self) -> bool {
        self.reach.is_some()
    }

    /// Gives `agent` this profile's reach, if it sets one: its commands in the sandbox, its own
    /// tools and its subagents kept to the minimal reach and `read`'s paths, whatever the
    /// settings say but their `denyRead`; refused where the sandbox cannot start.
    pub(super) fn confine(&self, agent: &mut Agent, settings: &Settings) -> Result<(), String> {
        let Some(reach) = &self.reach else {
            return Ok(());
        };
        Sandbox::check()?;
        let mode = match reach.writable {
            true => SandboxMode::WorkspaceWrite,
            false => SandboxMode::ReadOnly,
        };
        let mut sandbox = Sandbox {
            mode,
            network: reach.network,
            writable: Vec::new(),
        };
        let root = agent.root().to_owned();
        for directory in &reach.allow_write {
            match sandbox.grant(&root, &directory.to_string_lossy()) {
                Err(error) if error.ends_with("already writable") => {}
                Err(error) => return Err(format!("profile.allowWrite: {error}")),
                Ok(_) => {}
            }
        }
        let reads = |deny| Reads {
            deny,
            allow: reach.read.clone(),
            reach: Reach::Minimal,
        };
        agent.set_writable(Vec::new());
        agent.set_sandbox(Some(sandbox));
        agent.set_offline(!reach.network);
        agent.set_reads(reads(settings.reads().deny));
        agent.set_subagents(Subagents {
            network: false,
            reads: reads(settings.subagents().reads.deny),
            mode: SandboxMode::ReadOnly,
            writable: Vec::new(),
        });
        Ok(())
    }
}

fn wrong(key: &str, kind: &str) -> String {
    format!("profile.{key} must be {kind}")
}

fn text(object: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(wrong(key, "a string")),
    }
}

fn boolean(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(wrong(key, "true or false")),
    }
}

fn strings(object: &Map<String, Value>, key: &str) -> Result<Option<Vec<String>>, String> {
    let Some(given) = object.get(key) else {
        return Ok(None);
    };
    let list = (given.as_array().into_iter().flatten())
        .map(|entry| entry.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .filter(|_| given.is_array());
    list.map(Some)
        .ok_or_else(|| wrong(key, "a list of strings"))
}

fn paths(object: &Map<String, Value>, key: &str) -> Result<Option<Vec<PathBuf>>, String> {
    let Some(list) = strings(object, key)? else {
        return Ok(None);
    };
    let paths: Vec<PathBuf> = list.iter().map(PathBuf::from).collect();
    match paths.iter().find(|path| !path.is_absolute()) {
        Some(path) => Err(format!(
            "profile.{key}: {} is not an absolute path",
            path.display()
        )),
        None => Ok(Some(paths)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_profile_refuses_what_it_does_not_know_naming_it() {
        let refused = |given: Value| Profile::parse(&given).unwrap_err();
        assert_eq!(refused(json!([])), "profile must be an object");
        assert_eq!(
            refused(json!({"writeable": true})),
            "profile.writeable is not a profile key"
        );
        assert_eq!(
            refused(json!({"writable": "yes"})),
            "profile.writable must be true or false"
        );
        assert_eq!(
            refused(json!({"tools": "read"})),
            "profile.tools must be a list of strings"
        );
        assert_eq!(
            refused(json!({"tools": ["read", 1]})),
            "profile.tools must be a list of strings"
        );
        assert_eq!(
            refused(json!({"tools": ["read", "fly"]})),
            "profile.tools: fly is not one of Scramjet's tools"
        );
        assert_eq!(
            refused(json!({"read": ["refs"]})),
            "profile.read: refs is not an absolute path"
        );
        assert_eq!(
            refused(json!({"model": "local/qwen"})),
            "profile.model is not supported yet"
        );
    }

    #[test]
    fn a_profile_sets_the_sessions_instructions_and_tools_over_the_launch_options() {
        let given = json!({
            "systemPrompt": "/etc/hostname",
            "appendSystemPrompt": ["one", "two"],
            "contextFiles": false,
            "directives": false,
            "tools": ["read", "bash"],
        });
        let profile = Profile::parse(&given).unwrap();
        assert!(!profile.confines());
        let arguments = ["--system-prompt", "launch", "--append-system-prompt", "x"];
        let mut options = Options::parse(arguments.iter().map(Into::into))
            .unwrap()
            .unwrap();
        profile.apply(&mut options);
        let prompt = &options.prompt;
        assert_eq!(prompt.system.as_deref(), Some("/etc/hostname"));
        assert!(prompt.system_text && prompt.append_text);
        assert_eq!(prompt.append, ["one", "two"]);
        assert!(prompt.no_context_files && prompt.no_directives);
        assert_eq!(options.only_tools, Some(vec!["read".into(), "bash".into()]));
        assert_eq!(profile.given, given);

        // What it leaves unset stays the launch's.
        let mut options = Options::parse(arguments.iter().map(Into::into))
            .unwrap()
            .unwrap();
        Profile::parse(&json!({"appendSystemPrompt": "only"}))
            .unwrap()
            .apply(&mut options);
        assert_eq!(options.prompt.system.as_deref(), Some("launch"));
        assert!(!options.prompt.system_text);
        assert_eq!(options.prompt.append, ["only"]);
        assert_eq!(options.only_tools, None);
    }

    #[cfg(unix)]
    #[test]
    fn any_reach_key_confines_the_session_read_only_and_off_the_network_by_default() {
        let profile = Profile::parse(&json!({"read": ["/data/refs"]})).unwrap();
        let reach = profile.reach.unwrap();
        assert_eq!(reach.read, [PathBuf::from("/data/refs")]);
        assert!(!reach.writable && !reach.network && reach.allow_write.is_empty());
        let profile = Profile::parse(&json!({"network": true})).unwrap();
        assert!(profile.confines());
    }
}
