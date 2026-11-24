//! The sandbox as `/sandbox` changed it in one project: `sandbox-projects.json` beside the
//! settings, an object from a project's real path to what differs there from every project's
//! default. Only the rows changed in the project are kept, so a later default reaches the rest.
//! Never read from a repository, so a project cannot loosen its own sandbox.

use super::{BLOCK_READS, MINIMAL, Settings, reach as reach_of};
use scramjet::agent::{Reach, SandboxMode, shortened};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The sandbox's rows, as the settings have them.
#[derive(Clone, PartialEq, Debug)]
pub(in super::super) struct Rows {
    pub(in super::super) mode: Option<SandboxMode>,
    pub(in super::super) network: bool,
    pub(in super::super) reach: Reach,
    allow_write: Vec<String>,
    deny_read: Vec<String>,
    allow_read: Vec<String>,
    pub(in super::super) subagent_mode: SandboxMode,
    pub(in super::super) subagent_network: bool,
    pub(in super::super) subagent_reach: Option<Reach>,
    subagent_allow_write: Vec<String>,
    subagent_deny_read: Option<Vec<String>>,
    subagent_allow_read: Option<Vec<String>>,
}

/// The project the settings are for, and the sandbox's rows as the settings file gives them.
#[derive(Debug)]
pub(super) struct Project {
    key: String,
    default: Rows,
}

const FILE: &str = "sandbox-projects.json";

impl Settings {
    /// Takes the sandbox's rows changed in the project at `root` over the file's.
    pub(in super::super) fn in_project(&mut self, root: &Path) {
        let project = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
        self.project = Some(Project {
            key: project.display().to_string(),
            default: self.rows(),
        });
        self.apply_project();
    }

    /// The project's rows read again over the file's, as `in_project` and `/reload` take them.
    pub(super) fn apply_project(&mut self) {
        let Some(project) = &self.project else {
            return;
        };
        let key = project.key.clone();
        let entry = match self.projects() {
            Ok(mut projects) => projects.remove(&key),
            Err(error) => {
                self.warnings.push(error);
                return;
            }
        };
        let default = self.rows();
        if let Some(project) = &mut self.project {
            project.default = default;
        }
        match entry {
            Some(Value::Object(entry)) => self.apply_entry(&entry),
            Some(_) => (self.warnings).push(format!("{FILE}: {key} is not an object; ignored")),
            None => {}
        }
    }

    /// The sandbox's rows as every project has them by default, unless the session is for none.
    pub(in super::super) fn sandbox_default(&self) -> Option<&Rows> {
        self.project.as_ref().map(|project| &project.default)
    }

    pub(in super::super) fn rows(&self) -> Rows {
        Rows {
            mode: self.sandbox,
            network: self.sandbox_network,
            reach: self.reach,
            allow_write: self.allow_write.clone(),
            deny_read: self.deny_read.clone(),
            allow_read: self.allow_read.clone(),
            subagent_mode: self.subagent_mode,
            subagent_network: self.subagent_network,
            subagent_reach: self.subagent_reach,
            subagent_allow_write: self.subagent_allow_write.clone(),
            subagent_deny_read: self.subagent_deny_read.clone(),
            subagent_allow_read: self.subagent_allow_read.clone(),
        }
    }

    fn set_rows(&mut self, rows: Rows) {
        self.sandbox = rows.mode;
        self.sandbox_network = rows.network;
        self.reach = rows.reach;
        self.allow_write = rows.allow_write;
        self.deny_read = rows.deny_read;
        self.allow_read = rows.allow_read;
        self.subagent_mode = rows.subagent_mode;
        self.subagent_network = rows.subagent_network;
        self.subagent_reach = rows.subagent_reach;
        self.subagent_allow_write = rows.subagent_allow_write;
        self.subagent_deny_read = rows.subagent_deny_read;
        self.subagent_allow_read = rows.subagent_allow_read;
    }

    /// Whether the project sets any of the sandbox's rows.
    pub(in super::super) fn set_here(&self) -> bool {
        self.sandbox_default()
            .is_some_and(|default| *default != self.rows())
    }

    /// The paths of `list` in the default that the project took out, resolved.
    pub(in super::super) fn removed_here(
        &self,
        subagents: bool,
        list: super::PathList,
    ) -> Vec<PathBuf> {
        let Some(default) = self.sandbox_default() else {
            return Vec::new();
        };
        let (default, now) = match (default.list(subagents, list), self.entries(subagents, list)) {
            (Some(default), Some(now)) => (default, now),
            _ => return Vec::new(),
        };
        let now = self.resolved(now);
        (self.resolved(default).into_iter())
            .filter(|path| !now.contains(path))
            .collect()
    }

    /// The paths of `list` in the default, resolved; `None`: subagents take the main agent's,
    /// or the session is for no project.
    pub(in super::super) fn default_listed(
        &self,
        subagents: bool,
        list: super::PathList,
    ) -> Option<Vec<PathBuf>> {
        let default = self.sandbox_default()?.list(subagents, list)?;
        Some(self.resolved(default))
    }

    /// Whether `path` is in `list` for this project only, not in the default.
    pub(in super::super) fn added_here(
        &self,
        subagents: bool,
        list: super::PathList,
        path: &Path,
    ) -> bool {
        let Some(default) = self.sandbox_default() else {
            return false;
        };
        let base = default.list(subagents, list).or(default.list(false, list));
        !self
            .resolved(base.unwrap_or_default())
            .iter()
            .any(|known| known == path)
    }

    /// Keeps what the project's rows differ in from the default, and only that.
    pub(in super::super) fn remember_project(&self) -> Result<(), String> {
        let Some(project) = &self.project else {
            return Ok(());
        };
        let entry = self.changed(&project.default, &self.rows());
        self.change_projects(&project.key, (!entry.is_empty()).then_some(entry))
    }

    /// The project's rows back to the default, forgotten.
    pub(in super::super) fn reset_project(&mut self) -> Result<(), String> {
        let Some(project) = &self.project else {
            return Ok(());
        };
        self.change_projects(&project.key, None)?;
        let default = project.default.clone();
        self.set_rows(default);
        Ok(())
    }

    /// After the rows were saved as every project's default: the project follows it.
    pub(in super::super) fn follow_default(&mut self) -> Result<(), String> {
        let rows = self.rows();
        let Some(project) = &mut self.project else {
            return Ok(());
        };
        project.default = rows;
        let key = project.key.clone();
        self.change_projects(&key, None)
    }

    fn store(&self) -> Option<PathBuf> {
        Some(self.path.as_deref()?.parent()?.join(FILE))
    }

    fn projects(&self) -> Result<Map<String, Value>, String> {
        let Some(file) = self.store() else {
            return Ok(Map::new());
        };
        let shown = shortened(&file.display().to_string());
        match std::fs::read_to_string(&file) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(Value::Object(projects)) => Ok(projects),
                Ok(_) => Err(format!("{shown}: not a JSON object; ignored")),
                Err(error) => Err(format!("{shown}: {error}; ignored")),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Map::new()),
            Err(error) => Err(format!("{shown}: {error}")),
        }
    }

    /// Puts `entry` in the store as the project's, or with `None` forgets the project.
    fn change_projects(&self, key: &str, entry: Option<Map<String, Value>>) -> Result<(), String> {
        let Some(file) = self.store().filter(|_| !scramjet::agent::ephemeral()) else {
            return Ok(());
        };
        let mut projects = self.projects()?;
        let before = projects.get(key).cloned();
        let after = entry.map(Value::Object);
        if before == after {
            return Ok(());
        }
        match after {
            Some(entry) => projects.insert(key.to_owned(), entry),
            None => projects.remove(key),
        };
        let text = serde_json::to_string_pretty(&Value::Object(projects))
            .map_err(|error| error.to_string())?;
        scramjet::agent::write_replacing(&file, &(text + "\n"))
            .map_err(|error| format!("{}: {error}", shortened(&file.display().to_string())))
    }

    fn resolved(&self, entries: &[String]) -> Vec<PathBuf> {
        entries.iter().map(|entry| self.resolve(entry)).collect()
    }

    /// `now`'s paths missing from `base`, and `base`'s missing from `now`; `None` if neither.
    fn list_change(&self, base: &[String], now: &[String]) -> Option<Value> {
        let (base, now) = (self.resolved(base), self.resolved(now));
        let shown = |paths: Vec<&PathBuf>| -> Vec<String> {
            (paths.into_iter())
                .map(|path| shortened(&path.to_string_lossy()))
                .collect()
        };
        let add = shown(now.iter().filter(|path| !base.contains(path)).collect());
        let remove = shown(base.iter().filter(|path| !now.contains(path)).collect());
        (!add.is_empty() || !remove.is_empty()).then(|| json!({"add": add, "remove": remove}))
    }

    /// `base` without what `change` removes and with what it adds.
    fn list_applied(&mut self, key: &str, base: &[String], change: &Value) -> Vec<String> {
        let paths = |name: &str| -> Option<Vec<String>> {
            match change.get(name) {
                None => Some(Vec::new()),
                Some(Value::Array(paths)) => (paths.iter())
                    .map(|path| path.as_str().map(str::to_owned))
                    .collect(),
                Some(_) => None,
            }
        };
        let (true, Some(add), Some(remove)) = (change.is_object(), paths("add"), paths("remove"))
        else {
            (self.warnings).push(format!("{FILE}: {key} must hold lists of paths; ignored"));
            return base.to_vec();
        };
        let removed = self.resolved(&remove);
        let mut list: Vec<String> = (base.iter())
            .filter(|entry| !removed.contains(&self.resolve(entry)))
            .cloned()
            .collect();
        for entry in add {
            if !self.resolved(&list).contains(&self.resolve(&entry)) {
                list.push(entry);
            }
        }
        list
    }

    fn changed(&self, default: &Rows, now: &Rows) -> Map<String, Value> {
        let mut entry = Map::new();
        if now.mode != default.mode {
            entry.insert(
                "mode".into(),
                json!(now.mode.map_or("off", SandboxMode::name)),
            );
        }
        if now.network != default.network {
            entry.insert("network".into(), json!(now.network));
        }
        if now.reach != default.reach {
            entry.insert(BLOCK_READS.into(), reach_value(now.reach));
        }
        let lists = [
            ("allowWrite", &default.allow_write, &now.allow_write),
            ("denyRead", &default.deny_read, &now.deny_read),
            ("allowRead", &default.allow_read, &now.allow_read),
        ];
        for (key, default, now) in lists {
            if let Some(change) = self.list_change(default, now) {
                entry.insert(key.into(), change);
            }
        }
        let mut subagents = Map::new();
        if now.subagent_mode != default.subagent_mode {
            subagents.insert("mode".into(), json!(now.subagent_mode.name()));
        }
        if now.subagent_network != default.subagent_network {
            subagents.insert("network".into(), json!(now.subagent_network));
        }
        if now.subagent_reach != default.subagent_reach {
            let reach = now.subagent_reach.map_or(Value::Null, reach_value);
            subagents.insert(BLOCK_READS.into(), reach);
        }
        let base = &default.subagent_allow_write;
        if let Some(change) = self.list_change(base, &now.subagent_allow_write) {
            subagents.insert("allowWrite".into(), change);
        }
        let own = [
            (
                "denyRead",
                &default.subagent_deny_read,
                &now.subagent_deny_read,
                &now.deny_read,
            ),
            (
                "allowRead",
                &default.subagent_allow_read,
                &now.subagent_allow_read,
                &now.allow_read,
            ),
        ];
        for (key, default, own, main) in own {
            let change = match own {
                None if default.is_none() => continue,
                None => Value::Null,
                Some(own) => {
                    let base = default.as_deref().unwrap_or(main);
                    match self.list_change(base, own) {
                        Some(change) => change,
                        None if default.is_some() => continue,
                        None => json!({"add": [], "remove": []}),
                    }
                }
            };
            subagents.insert(key.into(), change);
        }
        if !subagents.is_empty() {
            entry.insert("subagents".into(), Value::Object(subagents));
        }
        entry
    }

    fn apply_entry(&mut self, entry: &Map<String, Value>) {
        let mut wrong = Vec::new();
        if let Some(mode) = entry.get("mode") {
            match mode.as_str().and_then(SandboxMode::parse) {
                Some(mode) => self.sandbox = mode,
                None => wrong.push("mode"),
            }
        }
        if let Some(network) = entry.get("network") {
            match network.as_bool() {
                Some(network) => self.sandbox_network = network,
                None => wrong.push("network"),
            }
        }
        if let Some(reach) = entry.get(BLOCK_READS) {
            match reach_of(reach) {
                Some(reach) => self.reach = reach,
                None => wrong.push(BLOCK_READS),
            }
        }
        if let Some(change) = entry.get("allowWrite") {
            let base = self.allow_write.clone();
            self.allow_write = self.list_applied("allowWrite", &base, change);
        }
        if let Some(change) = entry.get("denyRead") {
            let base = self.deny_read.clone();
            self.deny_read = self.list_applied("denyRead", &base, change);
        }
        if let Some(change) = entry.get("allowRead") {
            let base = self.allow_read.clone();
            self.allow_read = self.list_applied("allowRead", &base, change);
        }
        match entry.get("subagents") {
            None => {}
            Some(Value::Object(subagents)) => self.apply_project_subagents(subagents, &mut wrong),
            Some(_) => wrong.push("subagents"),
        }
        for key in wrong {
            (self.warnings).push(format!("{FILE}: {key} is not a value it takes; ignored"));
        }
    }

    fn apply_project_subagents(&mut self, entry: &Map<String, Value>, wrong: &mut Vec<&str>) {
        if let Some(mode) = entry.get("mode") {
            match mode.as_str().and_then(SandboxMode::parse) {
                Some(Some(mode)) => self.subagent_mode = mode,
                _ => wrong.push("subagents.mode"),
            }
        }
        if let Some(network) = entry.get("network") {
            match network.as_bool() {
                Some(network) => self.subagent_network = network,
                None => wrong.push("subagents.network"),
            }
        }
        match entry.get(BLOCK_READS) {
            None => {}
            Some(Value::Null) => self.subagent_reach = None,
            Some(reach) => match reach_of(reach) {
                Some(reach) => self.subagent_reach = Some(reach),
                None => wrong.push("subagents.blockReadsOutsideWorkingDirectories"),
            },
        }
        if let Some(change) = entry.get("allowWrite") {
            let base = self.subagent_allow_write.clone();
            self.subagent_allow_write = self.list_applied("subagents.allowWrite", &base, change);
        }
        for (key, hidden) in [("denyRead", true), ("allowRead", false)] {
            let Some(change) = entry.get(key) else {
                continue;
            };
            let (own, main) = match hidden {
                true => (self.subagent_deny_read.clone(), self.deny_read.clone()),
                false => (self.subagent_allow_read.clone(), self.allow_read.clone()),
            };
            let list = match change {
                Value::Null => None,
                change => Some(self.list_applied(key, &own.unwrap_or(main), change)),
            };
            match hidden {
                true => self.subagent_deny_read = list,
                false => self.subagent_allow_read = list,
            }
        }
    }
}

impl Rows {
    /// `list` as the rows have it; `None`: subagents take the main agent's.
    fn list(&self, subagents: bool, list: super::PathList) -> Option<&[String]> {
        use super::PathList;
        match (subagents, list) {
            (false, PathList::Writable) => Some(&self.allow_write),
            (false, PathList::Hidden) => Some(&self.deny_read),
            (false, PathList::Readable) => Some(&self.allow_read),
            (true, PathList::Writable) => Some(&self.subagent_allow_write),
            (true, PathList::Hidden) => self.subagent_deny_read.as_deref(),
            (true, PathList::Readable) => self.subagent_allow_read.as_deref(),
        }
    }
}

/// A reach as `blockReadsOutsideWorkingDirectories` gives it.
pub(super) fn reach_value(reach: Reach) -> Value {
    match reach {
        Reach::Minimal => json!(MINIMAL),
        reach => json!(reach.kept()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::PathList;
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store(scratch: &Scratch) -> Value {
        let text = std::fs::read_to_string(scratch.0.join(FILE)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn a_project_keeps_only_the_rows_changed_there_over_a_later_default() {
        let scratch =
            std::env::temp_dir().join(format!("scramjet-projects-{}", std::process::id()));
        _ = std::fs::remove_dir_all(&scratch);
        let scratch = Scratch(scratch);
        let (one, two) = (scratch.0.join("one"), scratch.0.join("two"));
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        let path = scratch.0.join("settings.json");
        let write = |text: &str| std::fs::write(&path, text).unwrap();
        write(
            r#"{"sandbox": {"mode": "workspace-write",
                "filesystem": {"denyRead": ["/hidden/a"], "allowWrite": ["/w"]}}}"#,
        );
        let mut settings = Settings::read(Some(path.clone()));
        settings.in_project(&one);
        let default = settings.rows();
        assert!(!settings.set_here());
        settings.sandbox_network = false;
        settings.reach = Reach::Minimal;
        assert!(settings.add_listed(false, PathList::Hidden, Path::new("/hidden/b")));
        assert!(settings.remove_listed(false, PathList::Hidden, Path::new("/hidden/a")));
        settings.set_subagent_policy(SandboxMode::WorkspaceWrite, true, Some(Reach::Project));
        settings.own_list(PathList::Readable, true);
        settings.remember_project().unwrap();
        assert!(settings.set_here());
        assert!(settings.added_here(false, PathList::Hidden, Path::new("/hidden/b")));
        let removed = settings.removed_here(false, PathList::Hidden);
        assert_eq!(removed, [PathBuf::from("/hidden/a")]);
        let real = std::fs::canonicalize(&one).unwrap().display().to_string();
        assert_eq!(
            store(&scratch),
            json!({real.clone(): {"network": false, BLOCK_READS: "minimal",
                "denyRead": {"add": ["/hidden/b"], "remove": ["/hidden/a"]},
                "subagents": {"mode": "workspace-write", "network": true, BLOCK_READS: true,
                    "allowRead": {"add": [], "remove": []}}}})
        );
        let changed = settings.rows();
        let mut again = Settings::read(Some(path.clone()));
        again.in_project(&one);
        assert_eq!(again.rows(), changed);
        assert_eq!(again.sandbox_default(), Some(&default));
        let mut other = Settings::read(Some(path.clone()));
        other.in_project(&two);
        assert_eq!(other.rows(), default);
        // A later default reaches the rows the project did not change.
        write(
            r#"{"sandbox": {"mode": "read-only", "network": true,
                "filesystem": {"denyRead": ["/hidden/a", "/hidden/c"]}}}"#,
        );
        again.reload();
        assert_eq!(again.sandbox, Some(SandboxMode::ReadOnly));
        assert!(!again.sandbox_network);
        assert_eq!(again.deny_read, ["/hidden/c", "/hidden/b"]);
        assert!(again.allow_write.is_empty());
        assert_eq!(again.subagent_allow_read, Some(Vec::new()));
        // Saved as the default, the project follows it and keeps nothing of its own.
        let mode = again.sandbox;
        again.save_sandbox_defaults(mode, false).unwrap();
        again.follow_default().unwrap();
        assert_eq!(store(&scratch), json!({}));
        assert!(!again.set_here());
        let mut saved = Settings::read(Some(path.clone()));
        saved.in_project(&two);
        assert_eq!(saved.rows(), again.rows());
        // Reset: the default again, the project forgotten.
        settings.reload();
        settings.allow_write.push("/more".into());
        settings.remember_project().unwrap();
        assert_ne!(store(&scratch), json!({}));
        settings.reset_project().unwrap();
        assert_eq!(store(&scratch), json!({}));
        assert_eq!(Some(&settings.rows()), settings.sandbox_default());
        // What cannot be read is said, and the default kept.
        std::fs::write(
            scratch.0.join(FILE),
            json!({real: {"mode": 1, "denyRead": 2}}).to_string(),
        )
        .unwrap();
        let mut wrong = Settings::read(Some(path));
        wrong.in_project(&one);
        assert_eq!(wrong.rows(), saved.rows());
        assert_eq!(wrong.warnings.len(), 2, "{:?}", wrong.warnings);
    }
}
