//! Claude Code's installed plugins, as Claude Code 2.1.259 finds them: `installed_plugins.json` in
//! its `plugins` directory names where each is, by `NAME@MARKETPLACE`, and the `enabledPlugins` of
//! the user's `settings.json`, then of the project's `.claude/settings.json` and
//! `.claude/settings.local.json` (the later winning), says which are on: `true` or a list.
use super::Problem;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Plugin {
    /// The part of its key before `@`, which names its commands, agents and skills.
    pub name: String,
    pub directory: PathBuf,
}

/// The plugins on in a session in `root`, in the order `installed_plugins.json` lists them, with
/// Claude Code's directory `claude`.
pub(super) fn enabled(claude: &Path, root: &Path, problems: &mut Vec<Problem>) -> Vec<Plugin> {
    let path = claude.join("plugins").join("installed_plugins.json");
    let Some(installed) = read(&path, problems) else {
        return Vec::new();
    };
    let Some(plugins) = installed.get("plugins").and_then(Value::as_object) else {
        let text = format!("{}: no `plugins` object; no plugins loaded", path.display());
        problems.push(Problem::in_file(text, &path));
        return Vec::new();
    };
    let mut on = Map::new();
    let settings = [
        claude.join("settings.json"),
        root.join(".claude").join("settings.json"),
        root.join(".claude").join("settings.local.json"),
    ];
    for settings in settings {
        let read = read(&settings, problems);
        if let Some(Value::Object(enabled)) = read.as_ref().and_then(|s| s.get("enabledPlugins")) {
            on.extend(enabled.clone());
        }
    }
    let root = crate::os::canonicalize(root).unwrap_or(root.to_path_buf());
    let mut found = Vec::new();
    for (key, entries) in plugins {
        if !on
            .get(key)
            .is_some_and(|value| value == true || value.is_array())
        {
            continue;
        }
        let entries = match entries {
            Value::Array(entries) => entries.iter().collect(),
            entry => vec![entry],
        };
        let Some(entry) = chosen(&entries, &root) else {
            continue;
        };
        let name = key.split('@').next().unwrap_or(key);
        if name.is_empty() || name.contains([':', '/', '\\', ' ']) {
            let text = format!("skipping Claude Code plugin {key}: its name is not usable here");
            problems.push(Problem::in_file(text, &path));
            continue;
        }
        let Some(directory) = entry.get("installPath").and_then(Value::as_str) else {
            let text = format!("skipping Claude Code plugin {key}: no installPath");
            problems.push(Problem::in_file(text, &path));
            continue;
        };
        if !Path::new(directory).is_dir() {
            let text = format!("skipping Claude Code plugin {key}: {directory} is not a directory");
            problems.push(Problem::in_file(text, &path));
            continue;
        }
        found.push(Plugin {
            name: name.to_owned(),
            directory: PathBuf::from(directory),
        });
    }
    found
}

/// The install used here: one made for this project (`local`, then `project`), else the user's.
fn chosen<'a>(entries: &[&'a Value], root: &Path) -> Option<&'a Value> {
    let scope = |entry: &Value| {
        entry
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let here = |entry: &Value| {
        let project = entry.get("projectPath").and_then(Value::as_str)?;
        let project = crate::os::canonicalize(Path::new(project)).ok()?;
        Some(project == root)
    };
    let of = |wanted: &str| {
        (entries.iter().copied()).find(|entry| {
            scope(entry).as_deref() == Some(wanted)
                && (wanted == "user" || here(entry) == Some(true))
        })
    };
    of("local").or_else(|| of("project")).or_else(|| of("user"))
}

/// The JSON object in `path`; `None` when there is none, said in `problems` when it does not
/// read.
fn read(path: &Path, problems: &mut Vec<Problem>) -> Option<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            let text = format!(
                "{}: {error}; Claude Code's plugins not read",
                path.display()
            );
            problems.push(Problem::in_file(text, path));
            return None;
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(value @ Value::Object(_)) => Some(value),
        Ok(_) | Err(_) => {
            let text = format!("{}: not a JSON object; Claude Code's plugins not read", {
                path.display()
            });
            problems.push(Problem::in_file(text, path));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, value: Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    #[test]
    fn enabled_plugins_are_found_where_installed_for_the_user_or_this_project() {
        let scratch = Scratch(
            std::env::temp_dir().join(format!("scramjet-claude-plugins-{}", std::process::id())),
        );
        let (claude, root, other) = (
            scratch.0.join("claude"),
            scratch.0.join("project"),
            scratch.0.join("other"),
        );
        let plugin = |name: &str| {
            let directory = scratch.0.join("cache").join(name);
            std::fs::create_dir_all(&directory).unwrap();
            directory.display().to_string()
        };
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        write(
            &claude.join("plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {
                "dev@market": [{"scope": "user", "installPath": plugin("dev")}],
                "off@market": [{"scope": "user", "installPath": plugin("off")}],
                "unlisted@market": [{"scope": "user", "installPath": plugin("unlisted")}],
                "docs@skills": [
                    {"scope": "local", "projectPath": other.display().to_string(),
                     "installPath": plugin("docs-other")},
                    {"scope": "project", "projectPath": root.display().to_string(),
                     "installPath": plugin("docs-here")},
                    {"scope": "user", "installPath": plugin("docs-user")}],
                "elsewhere@market": [{"scope": "project",
                    "projectPath": other.display().to_string(), "installPath": plugin("e")}],
                "gone@market": {"scope": "user", "installPath": "/nonexistent/gone"},
                "listed@market": [{"scope": "user", "installPath": plugin("listed")}],
            }}),
        );
        write(
            &claude.join("settings.json"),
            json!({"enabledPlugins": {"dev@market": true, "off@market": true,
                "docs@skills": true, "elsewhere@market": true, "gone@market": true}}),
        );
        write(
            &root.join(".claude/settings.local.json"),
            json!({"enabledPlugins": {"off@market": false, "listed@market": ["x"]}}),
        );
        let mut problems = Vec::new();
        let found = enabled(&claude, &root, &mut problems);
        let found: Vec<(&str, String)> = (found.iter())
            .map(|plugin| {
                let directory = plugin.directory.file_name().unwrap().to_string_lossy();
                (plugin.name.as_str(), directory.into_owned())
            })
            .collect();
        let expected = [("dev", "dev"), ("docs", "docs-here"), ("listed", "listed")];
        assert_eq!(found, expected.map(|(n, d)| (n, d.to_owned())));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].text.contains("gone@market"), "{problems:?}");

        std::fs::write(claude.join("settings.json"), "{").unwrap();
        let mut problems = Vec::new();
        let found = enabled(&claude, &root, &mut problems);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            problems[0].text.contains("not a JSON object"),
            "{problems:?}"
        );
        assert!(enabled(&scratch.0.join("none"), &root, &mut problems).is_empty());
    }
}
