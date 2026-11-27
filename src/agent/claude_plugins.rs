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
    /// The directories its skills are found in, when its marketplace entry or `plugin.json`
    /// lists some; else its `skills` directory.
    pub skills: Option<Vec<PathBuf>>,
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
        let directory = PathBuf::from(directory);
        let market = key.split_once('@').map_or("", |(_, market)| market);
        let entry = marketplace_entry(claude, market, name, problems);
        let skills = skill_directories(&directory, entry.as_ref(), problems);
        found.push(Plugin {
            name: name.to_owned(),
            directory,
            skills,
        });
    }
    found
}

/// The plugin in `directory`, as `--plugin-dir` loads it: named by its `plugin.json`'s `name`,
/// else by the directory's.
pub(super) fn at(directory: &Path, problems: &mut Vec<Problem>) -> Option<Plugin> {
    let manifest = directory.join(".claude-plugin").join("plugin.json");
    let declared = read(&manifest, problems)
        .and_then(|manifest| manifest.get("name")?.as_str().map(str::to_owned));
    let name = declared.or_else(|| Some(directory.file_name()?.to_str()?.to_owned()));
    let Some(name) = name.filter(|name| !name.is_empty() && !name.contains([':', '/', '\\', ' ']))
    else {
        let text = format!(
            "--plugin-dir {}: its name is not usable here; not loaded",
            directory.display()
        );
        problems.push(Problem::in_file(text, directory));
        return None;
    };
    let skills = skill_directories(directory, None, problems);
    Some(Plugin {
        name,
        directory: directory.to_path_buf(),
        skills,
    })
}

/// The entry naming `plugin` in the `.claude-plugin/marketplace.json` of the marketplace
/// `market`, where `known_marketplaces.json` says it is.
fn marketplace_entry(
    claude: &Path,
    market: &str,
    plugin: &str,
    problems: &mut Vec<Problem>,
) -> Option<Value> {
    let known = claude.join("plugins").join("known_marketplaces.json");
    let known = read(&known, problems)?;
    let location = known.get(market)?.get("installLocation")?.as_str()?;
    let file = Path::new(location)
        .join(".claude-plugin")
        .join("marketplace.json");
    let mut marketplace = read(&file, problems)?;
    let plugins = marketplace.get_mut("plugins")?.as_array_mut()?;
    let named = |entry: &Value| entry.get("name").and_then(Value::as_str) == Some(plugin);
    let at = plugins.iter().position(named)?;
    Some(plugins.swap_remove(at))
}

/// Where the plugin in `directory` has its skills, as Claude Code 2.1.259 finds them: a
/// marketplace `entry` whose `source` is the plugin's own directory (`./`) and which lists
/// `skills` names its only skill directories, its `skills` among them only when listed; else
/// the `skills` of its `plugin.json` are looked in after its `skills` directory. Each listed path
/// is a skill's directory or a directory of skills, inside the plugin's.
fn skill_directories(
    directory: &Path,
    entry: Option<&Value>,
    problems: &mut Vec<Problem>,
) -> Option<Vec<PathBuf>> {
    use std::path::Component;
    let listed = |value: Option<&Value>| -> Option<Vec<String>> {
        let paths = match value? {
            Value::String(path) => vec![path.clone()],
            Value::Array(paths) => (paths.iter())
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect(),
            _ => return None,
        };
        (!paths.is_empty()).then_some(paths)
    };
    let own_source = |entry: &&Value| {
        let source = entry.get("source").and_then(Value::as_str);
        source.is_some_and(|source| {
            (source.split(['/', '\\'])).all(|part| part.is_empty() || part == ".")
        })
    };
    let from_entry = entry
        .filter(own_source)
        .and_then(|entry| listed(entry.get("skills")));
    let (paths, replace) = match from_entry {
        Some(paths) => (paths, true),
        None => {
            let manifest = directory.join(".claude-plugin").join("plugin.json");
            let manifest = std::fs::read_to_string(manifest).ok();
            let manifest = manifest.and_then(|text| serde_json::from_str::<Value>(&text).ok());
            (
                listed(manifest.as_ref().and_then(|m| m.get("skills")))?,
                false,
            )
        }
    };
    let default = directory.join("skills");
    let mut keep_default = !replace;
    let (mut others, mut outside) = (Vec::new(), false);
    for path in paths {
        let relative = Path::new(&path);
        let inside = (relative.components())
            .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
        if !inside {
            let text = format!(
                "{}: skill path {path} is outside the plugin; not looked in",
                directory.display()
            );
            problems.push(Problem::in_file(text, directory));
            outside = true;
            continue;
        }
        let joined = (relative.components())
            .filter(|part| matches!(part, Component::Normal(_)))
            .fold(directory.to_path_buf(), |path, part| path.join(part));
        if !joined.is_dir() {
            continue;
        }
        if joined == default || joined == directory {
            keep_default = true;
        } else if !others.contains(&joined) {
            others.push(joined);
        }
    }
    if replace && !keep_default && others.is_empty() {
        return outside.then(Vec::new);
    }
    Some(
        keep_default
            .then_some(default)
            .into_iter()
            .chain(others)
            .collect(),
    )
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

    #[test]
    fn a_plugins_skills_are_where_its_marketplace_entry_or_manifest_lists_them() {
        let scratch = Scratch(
            std::env::temp_dir().join(format!("scramjet-plugin-skills-{}", std::process::id())),
        );
        let (claude, root) = (scratch.0.join("claude"), scratch.0.join("project"));
        let plugin = |name: &str, skills: &[&str]| {
            let directory = scratch.0.join("cache").join(name);
            for skill in skills {
                std::fs::create_dir_all(directory.join(skill)).unwrap();
            }
            std::fs::create_dir_all(&directory).unwrap();
            directory
        };
        let docs = plugin("docs", &["skills/pdf", "skills/xlsx", "skills/art"]);
        let all = plugin("all", &["skills/a", "extra/b"]);
        let own = plugin("own", &["skills/a", "more"]);
        for name in ["away", "plain", "gone"] {
            plugin(name, &["skills/a"]);
        }
        write(
            &own.join(".claude-plugin/plugin.json"),
            json!({"name": "own", "skills": ["./more", "./missing"]}),
        );
        let market = scratch.0.join("market");
        write(
            &market.join(".claude-plugin/marketplace.json"),
            json!({"plugins": [
                {"name": "docs", "source": "./", "strict": false,
                 "skills": ["./skills/pdf", "./skills/xlsx"]},
                {"name": "all", "source": ".", "skills": ["./skills", "extra/b"]},
                {"name": "own", "source": "./plugins/own", "skills": ["./skills/a"]},
                {"name": "away", "source": "./", "skills": ["../docs/skills"]},
                {"name": "gone", "source": "./", "skills": ["./nothing"]},
            ]}),
        );
        write(
            &claude.join("plugins/known_marketplaces.json"),
            json!({"m": {"installLocation": market.display().to_string()}}),
        );
        let names = ["docs", "all", "own", "away", "plain", "gone"];
        let installed: Map<String, Value> = (names.iter())
            .map(|name| {
                let directory = scratch.0.join("cache").join(name).display().to_string();
                let entry = json!([{"scope": "user", "installPath": directory}]);
                (format!("{name}@m"), entry)
            })
            .collect();
        write(
            &claude.join("plugins/installed_plugins.json"),
            json!({"plugins": installed}),
        );
        let on: Map<String, Value> = (names.iter())
            .map(|name| (format!("{name}@m"), Value::Bool(true)))
            .collect();
        write(&claude.join("settings.json"), json!({"enabledPlugins": on}));
        std::fs::create_dir_all(&root).unwrap();
        let mut problems = Vec::new();
        let found = enabled(&claude, &root, &mut problems);
        let mut skills: Vec<(String, Option<Vec<PathBuf>>)> =
            (found.into_iter()).map(|p| (p.name, p.skills)).collect();
        skills.sort();
        let expected = [
            ("all", Some(vec![all.join("skills"), all.join("extra/b")])),
            ("away", Some(vec![])),
            (
                "docs",
                Some(vec![docs.join("skills/pdf"), docs.join("skills/xlsx")]),
            ),
            ("gone", None),
            ("own", Some(vec![own.join("skills"), own.join("more")])),
            ("plain", None),
        ];
        assert_eq!(skills, expected.map(|(name, s)| (name.to_owned(), s)));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].text.contains("../docs/skills"), "{problems:?}");
    }
}
