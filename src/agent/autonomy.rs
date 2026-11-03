//! Scramjet's `autonomy.yaml` edges: whether a completed command's recommended next command runs
//! at once (`chain`) or waits for the user (`pause`), as Scramjet's `autonomy-settings.ts` reads
//! them from the project's `.scramjet/autonomy.yaml`, then the global one.

use std::path::{Path, PathBuf};
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Edge {
    Chain,
    Pause,
}

/// One file's edges: each source command with its targets' settings, in the file's order.
type Layer = Vec<(String, Vec<(String, Edge)>)>;

/// The edge settings of both files, project first.
#[derive(Debug, Default)]
pub(super) struct Edges {
    layers: Vec<Layer>,
    /// Why a file was not used; then, as in Scramjet, neither is.
    pub diagnostics: Vec<String>,
}

/// Scramjet's global file: `$XDG_CONFIG_HOME/scramjet/autonomy.yaml`, else under `~/.config`.
pub(super) fn global_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".config")))?;
    Some(config.join("scramjet/autonomy.yaml"))
}

impl Edges {
    /// Reads the project's file under `root`, then the global one; a missing file has no edges.
    pub fn load(global: Option<&Path>, root: &Path) -> Self {
        let project = root.join(".scramjet/autonomy.yaml");
        let mut edges = Self::default();
        let mut invalid = false;
        for (scope, path) in [("project", Some(project.as_path())), ("global", global)] {
            let Some(path) = path else { continue };
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    invalid = true;
                    let message = format!("autonomy.yaml: failed to load config: {error}");
                    edges.diagnostic(scope, path, &message);
                    continue;
                }
            };
            match parse(&text, scope == "project") {
                Ok(layer) => edges.layers.push(layer),
                Err(message) => {
                    invalid = true;
                    edges.diagnostic(scope, path, &message);
                }
            }
        }
        if invalid {
            edges.layers.clear();
        }
        edges
    }

    fn diagnostic(&mut self, scope: &str, path: &Path, message: &str) {
        self.diagnostics.push(format!(
            "scramjet: {scope} ({}): {message}; edge settings ignored this dispatch",
            path.display()
        ));
    }

    /// The setting for `source` → `target`: the project's before the global one, an exact
    /// target before `*`.
    pub fn resolve(&self, source: &str, target: &str) -> Option<Edge> {
        self.layers.iter().find_map(|layer| {
            let (_, targets) = layer.iter().find(|(name, _)| name == source)?;
            let setting = |wanted: &str| targets.iter().find(|(name, _)| name == wanted);
            setting(target)
                .or_else(|| setting("*"))
                .map(|(_, edge)| *edge)
        })
    }

    #[cfg(test)]
    pub fn parse(text: &str) -> Self {
        Self {
            layers: vec![parse(text, true).unwrap()],
            diagnostics: Vec::new(),
        }
    }
}

/// The edges in a file's text, with Scramjet's errors for what it rejects.
fn parse(text: &str, project: bool) -> Result<Layer, String> {
    let document = YamlLoader::load_from_str(text)
        .map_err(|error| format!("autonomy.yaml: failed to load config: {error}"))?
        .into_iter()
        .next()
        .unwrap_or(Yaml::Null);
    let root = match &document {
        Yaml::Null => return Ok(Vec::new()),
        Yaml::Hash(root) => root,
        _ => return Err("autonomy.yaml: root must be a map".into()),
    };
    let key = |yaml: &Yaml| yaml_key(yaml);
    if project && root.keys().any(|k| key(k) == "publications") {
        return Err(
            "autonomy.yaml: publications are global-only; use the global autonomy configuration"
                .into(),
        );
    }
    if let Some(unknown) = root
        .keys()
        .map(key)
        .find(|k| k != "edges" && k != "publications")
    {
        return Err(format!("autonomy.yaml: unknown top-level key {unknown}"));
    }
    let sources = match &document["edges"] {
        Yaml::BadValue | Yaml::Null => return Ok(Vec::new()),
        Yaml::Hash(sources) => sources,
        _ => return Err("autonomy.yaml: edges must be a command map".into()),
    };
    let mut edges = Vec::new();
    for (source, targets) in sources {
        let source = key(source);
        let Yaml::Hash(targets) = targets else {
            return Err(format!(
                "autonomy.yaml: edge settings for {source} must be a target map"
            ));
        };
        let mut settings = Vec::new();
        for (target, setting) in targets {
            let target = key(target);
            let edge = match setting.as_str() {
                Some("chain") => Edge::Chain,
                Some("pause") => Edge::Pause,
                _ => {
                    let shown = setting
                        .as_str()
                        .map_or_else(|| format!("{setting:?}"), str::to_owned);
                    return Err(format!(
                        "autonomy.yaml: invalid edge setting {shown} for {source} → {target}"
                    ));
                }
            };
            settings.push((target, edge));
        }
        edges.push((source, settings));
    }
    Ok(edges)
}

/// A map key as JavaScript's object keys see it: its text.
pub(super) fn yaml_key(key: &Yaml) -> String {
    match key {
        Yaml::String(text) | Yaml::Real(text) => text.clone(),
        Yaml::Integer(number) => number.to_string(),
        Yaml::Boolean(value) => value.to_string(),
        _ => "null".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_resolve_project_first_then_global_with_wildcards() {
        let root = std::env::temp_dir().join(format!("scramjet-autonomy-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".scramjet")).unwrap();
        let global = root.join("global.yaml");
        std::fs::write(
            root.join(".scramjet/autonomy.yaml"),
            "edges:\n  rs:loop:\n    rs:loop: chain\n    rs:steer: pause\n",
        )
        .unwrap();
        std::fs::write(
            &global,
            "edges:\n  rs:loop:\n    '*': chain\n  rs:review:\n    rs:loop: pause\n\
publications:\n  rs:loop:\n    create_pr: auto-approve\n",
        )
        .unwrap();
        let edges = Edges::load(Some(&global), &root);
        assert!(edges.diagnostics.is_empty());
        assert_eq!(edges.resolve("rs:loop", "rs:loop"), Some(Edge::Chain));
        assert_eq!(edges.resolve("rs:loop", "rs:steer"), Some(Edge::Pause));
        // Not set in the project's map for rs:loop, so the global wildcard applies.
        assert_eq!(edges.resolve("rs:loop", "rs:review"), Some(Edge::Chain));
        assert_eq!(edges.resolve("rs:review", "rs:loop"), Some(Edge::Pause));
        assert_eq!(edges.resolve("rs:steer", "rs:loop"), None);

        // An invalid file disables both, and says why.
        std::fs::write(&global, "edges:\n  rs:loop:\n    rs:loop: always\n").unwrap();
        let edges = Edges::load(Some(&global), &root);
        assert_eq!(edges.resolve("rs:loop", "rs:loop"), None);
        assert_eq!(
            edges.diagnostics,
            [format!(
                "scramjet: global ({}): autonomy.yaml: invalid edge setting always for rs:loop → \
rs:loop; edge settings ignored this dispatch",
                global.display()
            )]
        );
        let _ = std::fs::remove_dir_all(&root);
        // Without files there are no edges.
        let edges = Edges::load(None, &root);
        assert_eq!(
            (edges.resolve("rs:loop", "rs:loop"), edges.diagnostics.len()),
            (None, 0)
        );
    }

    #[test]
    fn files_scramjet_rejects_are_rejected_with_its_errors() {
        for (text, project, error) in [
            ("- edges", false, "autonomy.yaml: root must be a map"),
            (
                "other: 1",
                false,
                "autonomy.yaml: unknown top-level key other",
            ),
            (
                "publications: {}",
                true,
                "autonomy.yaml: publications are global-only; use the global autonomy configuration",
            ),
            (
                "edges: [a]",
                false,
                "autonomy.yaml: edges must be a command map",
            ),
            (
                "edges:\n  a: chain",
                false,
                "autonomy.yaml: edge settings for a must be a target map",
            ),
        ] {
            assert_eq!(parse(text, project).unwrap_err(), error, "{text}");
        }
        assert_eq!(parse("", true), Ok(Vec::new()));
        assert_eq!(parse("edges:", true), Ok(Vec::new()));
    }
}
