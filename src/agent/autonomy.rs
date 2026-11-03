//! Scramjet's `autonomy.yaml`, as its `autonomy-settings.ts` reads it: `edges` say whether a
//! completed command's recommended next command runs at once (`chain`) or waits for the user
//! (`pause`), from the project's `.scramjet/autonomy.yaml`, then the global one; the global file's
//! `publications` say whether a command's publication with a tool asks the user first. A command
//! set's `autonomy-defaults.yaml` gives its own commands' publication defaults.

use super::commands::Commands;
use std::path::{Path, PathBuf};
use yaml_rust2::{Yaml, YamlLoader};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Edge {
    Chain,
    Pause,
}

/// Whether a publication asks the user to approve it first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Publication {
    Ask,
    Auto,
}

/// One file's edges: each source command with its targets' settings, in the file's order.
type Layer = Vec<(String, Vec<(String, Edge)>)>;

/// Publication settings: each command with its tools' settings, in the file's order.
pub(super) type Publications = Vec<(String, Vec<(String, Publication)>)>;

/// The tools whose publications these settings are about.
const PUBLICATION_TOOLS: [&str; 4] = [
    "create_issue",
    "create_pr",
    "add_issue_comment",
    "add_pr_comment",
];

/// What an `autonomy.yaml` file says.
#[derive(Debug, Default, PartialEq)]
struct File {
    edges: Layer,
    publications: Publications,
}

/// The edge settings of both files, project first.
#[derive(Debug, Default)]
pub(super) struct Edges {
    layers: Vec<Layer>,
    /// Why a file was not used (then, as in Scramjet, neither is), and the commands a file names
    /// that are not registered.
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
    /// Names that are not among `commands` are warned about, as Scramjet does.
    pub fn load(global: Option<&Path>, root: &Path, commands: &Commands) -> Self {
        let project = root.join(".scramjet/autonomy.yaml");
        let mut edges = Self::default();
        let mut invalid = false;
        for (scope, path) in [("project", Some(project.as_path())), ("global", global)] {
            let Some(path) = path else { continue };
            let note = |message: &str| format!("scramjet: {scope} ({}): {message}", path.display());
            let ignored =
                |message: &str| note(&format!("{message}; edge settings ignored this dispatch"));
            match read(path, scope == "project") {
                Ok(None) => {}
                Ok(Some(file)) => {
                    let unknown = unknown_commands(&file, commands);
                    edges
                        .diagnostics
                        .extend(unknown.iter().map(|warning| note(warning)));
                    edges.layers.push(file.edges);
                }
                Err(message) => {
                    invalid = true;
                    edges.diagnostics.push(ignored(&message));
                }
            }
        }
        if invalid {
            edges.layers.clear();
        }
        edges
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
            layers: vec![parse(text, true).unwrap().edges],
            diagnostics: Vec::new(),
        }
    }
}

/// The user's setting in the `global` file for `command`'s publications with `tool`, read
/// afresh; an error when the file cannot be used.
pub(super) fn publication_override(
    global: Option<&Path>,
    command: &str,
    tool: &str,
) -> Result<Option<Publication>, String> {
    let Some(file) = global.map(|path| read(path, false)).transpose()?.flatten() else {
        return Ok(None);
    };
    Ok(lookup(&file.publications, command, tool))
}

/// The setting for `command` and `tool` among `publications`.
pub(super) fn lookup(
    publications: &Publications,
    command: &str,
    tool: &str,
) -> Option<Publication> {
    let (_, tools) = publications.iter().find(|(name, _)| name == command)?;
    tools
        .iter()
        .find(|(name, _)| name == tool)
        .map(|(_, setting)| *setting)
}

/// The publication defaults of an `autonomy-defaults.yaml` file, as Scramjet's
/// `parseAutonomyRecommendations` reads them: what it cannot use is skipped with a warning.
pub(super) fn publication_defaults(
    text: &str,
    warnings: &mut Vec<String>,
) -> Result<Publications, String> {
    let document = load(text)?;
    if !matches!(document, Yaml::Hash(_)) {
        return Ok(Vec::new());
    }
    let values = [
        ("require-approval", Publication::Ask),
        ("auto-approve", Publication::Auto),
    ];
    publications(&document["publications"], values, &mut |message| {
        warnings.push(message);
        Ok(())
    })
}

/// The file at `path`, `None` when there is none, or why it cannot be used.
fn read(path: &Path, project: bool) -> Result<Option<File>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text, project).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("autonomy.yaml: failed to load config: {error}")),
    }
}

/// The commands a file names that are not registered, as Scramjet's `validateConfig` words them.
fn unknown_commands(file: &File, commands: &Commands) -> Vec<String> {
    let unknown = |name: &str| commands.get(name).is_none();
    let mut warnings = Vec::new();
    for (source, targets) in &file.edges {
        if unknown(source) {
            warnings.push(format!("unknown source command \"{source}\""));
        }
        for (target, _) in targets {
            if target != "*" && unknown(target) {
                warnings.push(format!("unknown target command \"{target}\" (in {source})"));
            }
        }
    }
    for (command, _) in &file.publications {
        if unknown(command) {
            warnings.push(format!("unknown publication command \"{command}\""));
        }
    }
    warnings
}

fn load(text: &str) -> Result<Yaml, String> {
    let documents = YamlLoader::load_from_str(text)
        .map_err(|error| format!("autonomy.yaml: failed to load config: {error}"))?;
    Ok(documents.into_iter().next().unwrap_or(Yaml::Null))
}

/// A file's settings, with Scramjet's errors for what it rejects.
fn parse(text: &str, project: bool) -> Result<File, String> {
    let document = load(text)?;
    let root = match &document {
        Yaml::Null => return Ok(File::default()),
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
    let values = [
        ("always-ask", Publication::Ask),
        ("auto-approve", Publication::Auto),
    ];
    let publications = publications(&document["publications"], values, &mut |message| {
        Err(format!("autonomy.yaml: {message}"))
    })?;
    let sources = match &document["edges"] {
        Yaml::BadValue | Yaml::Null => {
            return Ok(File {
                edges: Vec::new(),
                publications,
            });
        }
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
                    return Err(format!(
                        "autonomy.yaml: invalid edge setting {} for {source} → {target}",
                        shown(setting)
                    ));
                }
            };
            settings.push((target, edge));
        }
        edges.push((source, settings));
    }
    Ok(File {
        edges,
        publications,
    })
}

/// A `publications` map whose settings are named as `values` names them, as Scramjet's
/// `parsePublicationMap` reads it; `invalid` decides whether what it cannot use is an error.
fn publications(
    value: &Yaml,
    values: [(&str, Publication); 2],
    invalid: &mut dyn FnMut(String) -> Result<(), String>,
) -> Result<Publications, String> {
    let commands = match value {
        Yaml::BadValue | Yaml::Null => return Ok(Vec::new()),
        Yaml::Hash(commands) => commands,
        _ => {
            invalid("publications must be a command map".into())?;
            return Ok(Vec::new());
        }
    };
    let mut publications = Vec::new();
    for (command, tools) in commands {
        let command = yaml_key(command);
        let Yaml::Hash(tools) = tools else {
            invalid(format!(
                "publication settings for {command} must be a tool map"
            ))?;
            continue;
        };
        let mut settings = Vec::new();
        for (tool, setting) in tools {
            let tool = yaml_key(tool);
            if !PUBLICATION_TOOLS.contains(&tool.as_str()) {
                invalid(format!("unknown publication tool {tool} for {command}"))?;
                continue;
            }
            match values
                .iter()
                .find(|(name, _)| setting.as_str() == Some(name))
            {
                Some((_, publication)) => settings.push((tool, *publication)),
                None => invalid(format!(
                    "invalid publication setting {} for {command} → {tool}",
                    shown(setting)
                ))?,
            }
        }
        if !settings.is_empty() {
            publications.push((command, settings));
        }
    }
    Ok(publications)
}

/// A setting's value as Scramjet's messages show it.
fn shown(setting: &Yaml) -> String {
    setting
        .as_str()
        .map_or_else(|| format!("{setting:?}"), str::to_owned)
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
    use crate::agent::tests::ScratchDir;

    fn commands() -> Commands {
        Commands::parsed(&[
            ("rs:loop.md", "Loop"),
            ("rs:steer.md", "Steer"),
            ("rs:review.md", "Review"),
        ])
    }

    #[test]
    fn edges_resolve_project_first_then_global_with_wildcards() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-autonomy-{}", std::process::id())),
        );
        let root = &scratch.0;
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
        let edges = Edges::load(Some(&global), root, &commands());
        assert!(edges.diagnostics.is_empty());
        assert_eq!(edges.resolve("rs:loop", "rs:loop"), Some(Edge::Chain));
        assert_eq!(edges.resolve("rs:loop", "rs:steer"), Some(Edge::Pause));
        // Not set in the project's map for rs:loop, so the global wildcard applies.
        assert_eq!(edges.resolve("rs:loop", "rs:review"), Some(Edge::Chain));
        assert_eq!(edges.resolve("rs:review", "rs:loop"), Some(Edge::Pause));
        assert_eq!(edges.resolve("rs:steer", "rs:loop"), None);

        // An invalid file disables both, and says why.
        std::fs::write(&global, "edges:\n  rs:loop:\n    rs:loop: always\n").unwrap();
        let edges = Edges::load(Some(&global), root, &commands());
        assert_eq!(edges.resolve("rs:loop", "rs:loop"), None);
        assert_eq!(
            edges.diagnostics,
            [format!(
                "scramjet: global ({}): autonomy.yaml: invalid edge setting always for rs:loop → \
rs:loop; edge settings ignored this dispatch",
                global.display()
            )]
        );
        // Without files there are no edges.
        let edges = Edges::load(None, &root.join("none"), &commands());
        assert_eq!(
            (edges.resolve("rs:loop", "rs:loop"), edges.diagnostics.len()),
            (None, 0)
        );
    }

    #[test]
    fn commands_a_file_names_that_are_not_registered_are_warned_about() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-autonomy-unknown-{}", std::process::id())),
        );
        let root = &scratch.0;
        let global = root.join("global.yaml");
        std::fs::write(
            &global,
            "edges:\n  rs:gone:\n    rs:loop: chain\n  rs:loop:\n    '*': pause\n    rs:old: chain\n\
publications:\n  rs:old:\n    create_pr: auto-approve\n",
        )
        .unwrap();
        let edges = Edges::load(Some(&global), root, &commands());
        let prefix = format!("scramjet: global ({}): ", global.display());
        assert_eq!(
            edges.diagnostics,
            [
                format!("{prefix}unknown source command \"rs:gone\""),
                format!("{prefix}unknown target command \"rs:old\" (in rs:loop)"),
                format!("{prefix}unknown publication command \"rs:old\""),
            ]
        );
        // The file is still used.
        assert_eq!(edges.resolve("rs:gone", "rs:loop"), Some(Edge::Chain));
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
            (
                "publications: [a]",
                false,
                "autonomy.yaml: publications must be a command map",
            ),
            (
                "publications:\n  a: auto-approve",
                false,
                "autonomy.yaml: publication settings for a must be a tool map",
            ),
            (
                "publications:\n  a:\n    merge_pr: auto-approve",
                false,
                "autonomy.yaml: unknown publication tool merge_pr for a",
            ),
            (
                "publications:\n  a:\n    create_pr: require-approval",
                false,
                "autonomy.yaml: invalid publication setting require-approval for a → create_pr",
            ),
        ] {
            assert_eq!(parse(text, project).unwrap_err(), error, "{text}");
        }
        assert_eq!(parse("", true), Ok(File::default()));
        assert_eq!(parse("edges:", true), Ok(File::default()));
    }

    #[test]
    fn publication_settings_are_read_from_the_global_file_afresh() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-autonomy-publish-{}", std::process::id())),
        );
        let global = scratch.0.join("autonomy.yaml");
        let setting = |tool: &str| publication_override(Some(&global), "rs:loop", tool);
        assert_eq!(setting("create_pr"), Ok(None));
        std::fs::write(
            &global,
            "publications:\n  rs:loop:\n    create_pr: auto-approve\n    add_pr_comment: always-ask\n",
        )
        .unwrap();
        assert_eq!(setting("create_pr"), Ok(Some(Publication::Auto)));
        assert_eq!(setting("add_pr_comment"), Ok(Some(Publication::Ask)));
        assert_eq!(setting("create_issue"), Ok(None));
        assert_eq!(
            publication_override(Some(&global), "rs:steer", "create_pr"),
            Ok(None)
        );
        std::fs::write(&global, "publications:\n  rs:loop:\n    create_pr: yes\n").unwrap();
        assert!(
            setting("create_pr")
                .unwrap_err()
                .contains("invalid publication setting yes")
        );
        assert_eq!(publication_override(None, "rs:loop", "create_pr"), Ok(None));
    }

    #[test]
    fn publication_defaults_skip_what_they_cannot_use_with_a_warning() {
        let mut warnings = Vec::new();
        let defaults = publication_defaults(
            &[
                "edges:\n  a:\n    b: chain\npublications:\n  a:",
                "    create_issue: require-approval\n    add_issue_comment: auto-approve",
                "    merge_pr: auto-approve\n    create_pr: always-ask\n  b: auto-approve",
            ]
            .join("\n"),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(
            defaults,
            [(
                "a".to_owned(),
                vec![
                    ("create_issue".to_owned(), Publication::Ask),
                    ("add_issue_comment".to_owned(), Publication::Auto),
                ]
            )]
        );
        assert_eq!(
            warnings,
            [
                "unknown publication tool merge_pr for a",
                "invalid publication setting always-ask for a → create_pr",
                "publication settings for b must be a tool map",
            ]
        );
        assert_eq!(publication_defaults("- a", &mut warnings), Ok(Vec::new()));
        assert!(publication_defaults("a: [", &mut warnings).is_err());
    }
}
