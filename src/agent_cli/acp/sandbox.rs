//! The `sandbox` extension: what the full screen's `/sandbox` screen shows, and the changes its
//! rows make (`docs/RPC.md`, section 8).
use super::super::drive::{SandboxChange, Session};
use super::super::sandbox_menu::{self, Grant, Listed};
use super::super::settings::{PathList, Settings};
use super::FULL_ACCESS;
use scramjet::agent::{Reach, Remembered, SandboxMode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The extension's names of a reach.
const REACH_NAMES: [(Reach, &str); 3] = [
    (Reach::Everything, "everything"),
    (Reach::Project, "project"),
    (Reach::Minimal, "minimal"),
];

pub(super) fn reach(name: &str) -> Option<Reach> {
    REACH_NAMES
        .iter()
        .find(|(_, known)| *known == name)
        .map(|(reach, _)| *reach)
}

fn reach_name(reach: Reach) -> &'static str {
    REACH_NAMES
        .iter()
        .find(|(known, _)| *known == reach)
        .map_or("everything", |(_, name)| name)
}

/// Subagents' reach when it is the main agent's.
const SAME: &str = "same";

const LISTS: [(PathList, &str); 3] = [
    (PathList::Writable, "writable"),
    (PathList::Hidden, "hidden"),
    (PathList::Readable, "readable"),
];

/// The request's keys, of which at most one is given.
const KEYS: [&str; 10] = [
    "mode",
    "network",
    "add",
    "remove",
    "reach",
    "listed",
    "subagents",
    "ownList",
    "save",
    "reset",
];

/// A change asked for, made by the session's thread.
pub(super) enum Asked {
    Change(SandboxChange),
    /// Subagents' mode, network and reach, each kept where `None`; `Some(None)` gives them the
    /// main agent's reach.
    Subagents(Option<SandboxMode>, Option<bool>, Option<Option<Reach>>),
}

impl Asked {
    pub(super) fn change(self, settings: &Settings) -> SandboxChange {
        match self {
            Self::Change(change) => change,
            Self::Subagents(mode, network, reach) => {
                let (now_mode, now_network, now_reach) = settings.subagent_policy();
                SandboxChange::Subagents(
                    mode.unwrap_or(now_mode),
                    network.unwrap_or(now_network),
                    reach.unwrap_or(now_reach),
                )
            }
        }
    }
}

/// The change `params` asks for, if any; why not, in the words `-32602` gives.
pub(super) fn asked(params: &Value) -> Result<Option<Asked>, String> {
    let given: Vec<&str> = (KEYS.into_iter())
        .filter(|key| !params[key].is_null())
        .collect();
    let key = match given[..] {
        [] => return Ok(None),
        [key] => key,
        _ => return Err(format!("at most one of {}", KEYS.join(", "))),
    };
    let value = &params[key];
    let change = match key {
        "mode" => SandboxChange::Mode(
            (value.as_str().and_then(SandboxMode::parse))
                .ok_or("mode is read-only, workspace-write or danger-full-access")?,
        ),
        "network" => SandboxChange::Network(value.as_bool().ok_or("network is true or false")?),
        "reach" => SandboxChange::Reach(
            (value.as_str().and_then(reach)).ok_or("reach is everything, project or minimal")?,
        ),
        "add" => SandboxChange::Add(directory(value, "add")?),
        "remove" => SandboxChange::Remove(directory(value, "remove")?),
        "listed" => {
            let list = (value["list"].as_str())
                .and_then(|name| LISTS.iter().find(|(_, known)| *known == name))
                .map(|(list, _)| *list)
                .ok_or("listed.list is writable, hidden or readable")?;
            let path = directory(&value["path"], "listed.path")?;
            let add = flag(&value["add"], "listed.add")?;
            let subagents = match &value["subagents"] {
                Value::Null => false,
                given => flag(given, "listed.subagents")?,
            };
            match (list, subagents, add) {
                (PathList::Writable, false, true) => SandboxChange::AddHere(path),
                (PathList::Writable, false, false) => SandboxChange::Remove(path),
                _ => SandboxChange::Listed {
                    subagents,
                    list,
                    path,
                    add,
                },
            }
        }
        "subagents" => return subagents(value).map(Some),
        "ownList" => {
            let list = match value["list"].as_str() {
                Some("hidden") => PathList::Hidden,
                Some("readable") => PathList::Readable,
                _ => return Err("ownList.list is hidden or readable".to_owned()),
            };
            SandboxChange::OwnList(list, flag(&value["own"], "ownList.own")?)
        }
        _ if *value != json!(true) => return Err(format!("{key} is true")),
        "save" => SandboxChange::Save,
        _ => SandboxChange::Reset,
    };
    Ok(Some(Asked::Change(change)))
}

fn subagents(value: &Value) -> Result<Asked, String> {
    let mode = match &value["mode"] {
        Value::Null => None,
        given => Some(match given.as_str() {
            Some("read-only") => SandboxMode::ReadOnly,
            Some("workspace-write") => SandboxMode::WorkspaceWrite,
            _ => return Err("subagents.mode is read-only or workspace-write".to_owned()),
        }),
    };
    let network = match &value["network"] {
        Value::Null => None,
        given => Some(flag(given, "subagents.network")?),
    };
    let reach = match &value["reach"] {
        Value::Null => None,
        given => Some(match given.as_str() {
            Some(SAME) => None,
            Some(name) if let Some(reach) = reach(name) => Some(reach),
            _ => return Err("subagents.reach is same, everything, project or minimal".to_owned()),
        }),
    };
    if mode.is_none() && network.is_none() && reach.is_none() {
        return Err("subagents gives mode, network or reach".to_owned());
    }
    Ok(Asked::Subagents(mode, network, reach))
}

fn directory(value: &Value, key: &str) -> Result<String, String> {
    match value.as_str().map(str::trim) {
        Some(directory) if !directory.is_empty() => Ok(directory.to_owned()),
        _ => Err(format!("{key} is a directory")),
    }
}

fn flag(value: &Value, key: &str) -> Result<bool, String> {
    value.as_bool().ok_or(format!("{key} is true or false"))
}

/// The session's sandbox as the full screen's `/sandbox` screen shows it; with `confined`, the
/// profile's reach the agent was given, which the settings do not hold.
pub(super) fn state(session: &Session, confined: bool) -> Value {
    let (agent, settings) = (&session.agent, &session.settings);
    let sandbox = agent.sandbox();
    let mode = sandbox.map_or(FULL_ACCESS, |sandbox| sandbox.mode.name());
    let network = sandbox.map_or(!agent.offline(), |sandbox| sandbox.network);
    let summary = super::super::slash::sandbox_summary(agent, &settings.allow_write());
    let reads = agent.reads();
    if confined {
        let theirs = agent.subagents();
        let writable = sandbox.map_or(&[][..], |sandbox| &sandbox.writable[..]);
        return json!({
            "mode": mode,
            "network": network,
            "reach": reach_name(reads.reach),
            "profile": true,
            "granted": (writable.iter())
                .map(|path| json!({"path": path, "granted": true, "always": false, "from": "profile"}))
                .collect::<Vec<_>>(),
            "hidden": plain(&reads.deny),
            "readable": plain(&reads.allow),
            "setHere": {"mode": false, "network": false, "reach": false},
            "projectSets": false,
            "subagents": {
                "mode": theirs.mode.name(),
                "network": theirs.network,
                "reach": reach_name(theirs.reads.reach),
                "setHere": {"mode": false, "network": false, "reach": false},
                "writable": plain(&theirs.writable),
                "hidden": plain(&theirs.reads.deny),
                "readable": plain(&theirs.reads.allow),
            },
            "summary": summary,
        });
    }
    let always =
        (Remembered::user().and_then(|kept| kept.of(agent.root()).ok())).unwrap_or_default();
    let writable = sandbox.map_or(agent.writable(), |sandbox| &sandbox.writable[..]);
    let mut granted: Vec<Value> = (sandbox_menu::grants(agent, settings).unwrap_or_default())
        .into_iter()
        .map(|(path, grant)| {
            let from = match grant {
                Grant::Session => "session",
                Grant::Project => "project",
                Grant::Default => "default",
            };
            json!({
                "path": path,
                "granted": writable.contains(&path),
                "always": always.contains(&path),
                "from": from,
            })
        })
        .collect();
    if sandbox.is_some() || reads.reach.kept() {
        let main = sandbox_menu::listed(settings, false, PathList::Writable).unwrap_or_default();
        granted.extend(main.iter().filter(|listed| listed.taken_out).map(|listed| {
            json!({"path": listed.path, "granted": false, "always": false, "from": "default",
                "takenOut": true})
        }));
    }
    let now = settings.rows();
    let default = settings.sandbox_default().unwrap_or(&now);
    let (their_mode, their_network, their_reach) = settings.subagent_policy();
    let list = |subagents, list| sandbox_menu::listed(settings, subagents, list).map(entries);
    json!({
        "mode": mode,
        "network": network,
        "reach": reach_name(reads.reach),
        "profile": false,
        "granted": granted,
        "hidden": list(false, PathList::Hidden),
        "readable": list(false, PathList::Readable),
        "setHere": {
            "mode": now.mode != default.mode,
            "network": cfg!(unix) && now.network != default.network,
            "reach": now.reach != default.reach,
        },
        "projectSets": settings.set_here() || !always.is_empty(),
        "subagents": {
            "mode": their_mode.name(),
            "network": their_network,
            "reach": their_reach.map_or(SAME, reach_name),
            "setHere": {
                "mode": now.subagent_mode != default.subagent_mode,
                "network": now.subagent_network != default.subagent_network,
                "reach": now.subagent_reach != default.subagent_reach,
            },
            "writable": list(true, PathList::Writable),
            "hidden": list(true, PathList::Hidden),
            "readable": list(true, PathList::Readable),
        },
        "summary": summary,
    })
}

fn entries(listed: Vec<Listed>) -> Vec<Value> {
    (listed.into_iter())
        .map(|listed| {
            json!({"path": listed.path, "setHere": listed.set_here, "takenOut": listed.taken_out})
        })
        .collect()
}

fn plain(paths: &[PathBuf]) -> Vec<Value> {
    let entry = |path: &Path| json!({"path": path, "setHere": false, "takenOut": false});
    paths.iter().map(|path| entry(path)).collect()
}
