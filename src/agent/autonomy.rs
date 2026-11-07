//! Scramjet's `autonomy.yaml`, as its `autonomy-settings.ts` reads it: `edges` say whether a
//! completed command's recommended next command runs at once (`chain`) or waits for the user
//! (`pause`), from the project's `.scramjet/autonomy.yaml`, then the global one; the global file's
//! `publications` say whether a command's publication with a tool asks the user first. A command
//! set's `autonomy-defaults.yaml` gives its own commands' publication defaults, and edges it
//! suggests, which hold once the user applies them. `/settings` shows the edges of each
//! command's next steps (`NextSteps`) and its publications (`Approvals`), and changes them.

use super::Problem;
use super::commands::{Commands, Policy};
use super::tools::{shortened, write_replacing};
use std::path::{Path, PathBuf};
use yaml_rust2::{Yaml, YamlLoader};

/// The project's file, under its root.
const PROJECT_FILE: &str = ".scramjet/autonomy.yaml";

/// Whether a completed command's recommended next command runs at once or waits for the user.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge {
    Chain,
    Pause,
}

/// Whether a publication asks the user to approve it first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Publication {
    Ask,
    Auto,
}

/// One file's edges: each source command with its targets' settings, in the file's order.
pub(super) type Layer = Vec<(String, Vec<(String, Edge)>)>;

/// What a command set suggests to follow one of its commands' next steps, in its
/// `autonomy-defaults.yaml`. It holds only once the user applies it, as in Scramjet.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub edge: Edge,
    /// The set whose file suggests it.
    pub set: String,
}

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
    pub diagnostics: Vec<Problem>,
}

/// The project's file, under its `root`.
pub(super) fn project_path(root: &Path) -> PathBuf {
    crate::os::join(root, PROJECT_FILE)
}

/// Scramjet's global file: `$XDG_CONFIG_HOME/scramjet/autonomy.yaml`, else under `~/.config`.
pub(super) fn global_path() -> Option<PathBuf> {
    let config = crate::os::base_directory("XDG_CONFIG_HOME", ".config")?;
    Some(crate::os::join(&config, "scramjet/autonomy.yaml"))
}

impl Edges {
    /// Reads the project's file under `root`, then the global one; a missing file has no edges.
    /// Names that are not among `commands` are warned about, as Scramjet does.
    pub fn load(global: Option<&Path>, root: &Path, commands: &Commands) -> Self {
        let project = crate::os::join(root, PROJECT_FILE);
        let mut edges = Self::default();
        let mut invalid = false;
        for (scope, path) in [("project", Some(project.as_path())), ("global", global)] {
            let Some(path) = path else { continue };
            let note = |message: &str| {
                let text = format!("scramjet: {scope} ({}): {message}", path.display());
                Problem::in_file(text, path)
            };
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

/// Which file a next step's setting is in: the project's, or the global one, for all projects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    Project,
    Global,
}

/// The setting a next step goes by, and where it is from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Setting {
    pub edge: Edge,
    pub scope: Scope,
    /// It is the file's setting for any other command (`*`).
    pub any_other: bool,
}

/// A command's next step, as `/settings` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct NextStep {
    pub source: String,
    /// `None`: any other command, the files' `*`.
    pub target: Option<String>,
    /// The command always runs it next (`mode: forced`), whatever the files say.
    pub forced: bool,
    /// What the step goes by; `None`: autopilot decides.
    pub setting: Option<Setting>,
    /// The project's file has a setting of its own for it.
    pub in_project: bool,
    /// The global file has a setting of its own for it.
    pub in_global: bool,
    /// What a command set suggests for it.
    pub suggested: Option<Suggestion>,
}

impl NextStep {
    /// What its set suggests, when applying that would change what follows: neither file has a
    /// setting of its own for it, and what applies now is not that.
    pub fn suggestion(&self) -> Option<&Suggestion> {
        let suggested = self.suggested.as_ref()?;
        let own = self.forced || self.in_project || self.in_global;
        let applies = self.setting.map(|setting| setting.edge);
        (!own && applies != Some(suggested.edge)).then_some(suggested)
    }
}

/// The next steps of the commands that have them, and the files their settings are in.
#[derive(Debug)]
pub struct NextSteps {
    pub steps: Vec<NextStep>,
    pub project: PathBuf,
    pub global: Option<PathBuf>,
    /// Why a file cannot be used, shortened; then neither is, as in Scramjet.
    pub problems: Vec<String>,
}

impl NextSteps {
    /// Each candidate of each command's `next:` policy (its target when forced) and, where a
    /// file sets or a set suggests one, its `*`, with the setting that applies to it, read
    /// afresh.
    pub(super) fn load(global: Option<&Path>, root: &Path, commands: &Commands) -> Self {
        let project = crate::os::join(root, PROJECT_FILE);
        let mut problems = Vec::new();
        let mut load = |path: Option<&Path>, in_project| {
            let path = path?;
            read(path, in_project).unwrap_or_else(|message| {
                problems.push(format!("{}: {message}", shown_path(path)));
                None
            })
        };
        let in_project = &load(Some(&project), true).unwrap_or_default();
        let in_global = &load(global, false).unwrap_or_default();
        let mut steps = Vec::new();
        for command in commands.list() {
            let source = &command.name;
            let candidates = match &command.next {
                Some(Policy::Forced { target }) => {
                    steps.push(NextStep {
                        source: source.clone(),
                        target: Some(target.clone()),
                        forced: true,
                        setting: None,
                        in_project: false,
                        in_global: false,
                        suggested: None,
                    });
                    continue;
                }
                Some(Policy::Closed { candidates } | Policy::Open { candidates, .. }) => candidates,
                Some(Policy::Ask { .. }) | None => continue,
            };
            let mut targets: Vec<Option<&str>> = Vec::new();
            for candidate in candidates {
                if !targets.contains(&Some(candidate.name.as_str())) {
                    targets.push(Some(&candidate.name));
                }
            }
            if [in_project, in_global]
                .iter()
                .any(|file| exact(file, source, "*").is_some())
                || commands.suggested(source, "*").is_some()
            {
                targets.push(None);
            }
            for target in targets {
                let key = target.unwrap_or("*");
                let setting = problems.is_empty().then(|| {
                    [(in_project, Scope::Project), (in_global, Scope::Global)]
                        .into_iter()
                        .find_map(|(file, scope)| {
                            let own = exact(file, source, key).map(|edge| (edge, false));
                            let other = || exact(file, source, "*").map(|edge| (edge, true));
                            let (edge, any_other) = own.or_else(other)?;
                            Some(Setting {
                                edge,
                                scope,
                                any_other,
                            })
                        })
                });
                steps.push(NextStep {
                    source: source.clone(),
                    target: target.map(str::to_owned),
                    forced: false,
                    setting: setting.flatten(),
                    in_project: exact(in_project, source, key).is_some(),
                    in_global: exact(in_global, source, key).is_some(),
                    suggested: commands.suggested(source, key).cloned(),
                });
            }
        }
        Self {
            steps,
            project,
            global: global.map(Path::to_path_buf),
            problems,
        }
    }

    /// The steps whose suggestion would change what follows them, none while a file cannot be
    /// used.
    pub fn suggested(&self) -> Vec<&NextStep> {
        match self.problems.is_empty() {
            true => (self.steps.iter())
                .filter(|step| step.suggestion().is_some())
                .collect(),
            false => Vec::new(),
        }
    }
}

/// A command's publications with one tool, as `/settings` shows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Approval {
    pub command: String,
    pub tool: &'static str,
    /// The user's own setting, in the global file.
    pub setting: Option<Publication>,
    /// What the command's set says; with neither, the publication asks.
    pub default: Option<Publication>,
}

impl Approval {
    /// What applies to a publication in the project's own repository, as the publication tools
    /// decide it: the user's setting, else the set's, else ask.
    pub fn applies(&self) -> Publication {
        (self.setting.or(self.default)).unwrap_or(Publication::Ask)
    }
}

/// The publications of the commands whose policy can approve them: those run by the user (not
/// `delegate-only`), with each publication tool their `allowed-tools` name.
#[derive(Debug)]
pub struct Approvals {
    pub list: Vec<Approval>,
    /// The file the user's settings are in, for all projects.
    pub global: Option<PathBuf>,
    /// Why it cannot be used, shortened; then every publication asks.
    pub problem: Option<String>,
}

impl Approvals {
    /// Read afresh, as the publication tools read the settings at each call.
    pub(super) fn load(global: Option<&Path>, commands: &Commands) -> Self {
        let (file, problem) = match global.map(|path| (path, read(path, false))) {
            None | Some((_, Ok(None))) => (File::default(), None),
            Some((_, Ok(Some(file)))) => (file, None),
            Some((path, Err(message))) => (
                File::default(),
                Some(format!("{}: {message}", shown_path(path))),
            ),
        };
        let mut list = Vec::new();
        for command in commands
            .list()
            .iter()
            .filter(|command| !command.delegate_only)
        {
            let declared = |tool: &str| {
                (command.allowed_tools.iter().flatten()).any(|allowed| allowed == tool)
            };
            for tool in PUBLICATION_TOOLS.into_iter().filter(|tool| declared(tool)) {
                list.push(Approval {
                    command: command.name.clone(),
                    tool,
                    setting: lookup(&file.publications, &command.name, tool),
                    default: commands.publication_default(&command.name, tool),
                });
            }
        }
        Self {
            list,
            global: global.map(Path::to_path_buf),
            problem,
        }
    }
}

/// Sets whether `command`'s publications with `tool` ask first, in the global file; with no
/// choice, the user's setting goes and the command's set decides.
pub(super) fn choose_publication(
    global: Option<&Path>,
    command: &str,
    tool: &str,
    choice: Option<Publication>,
) -> Result<(), String> {
    let global = global.ok_or("no file for all projects: HOME is not set")?;
    update(global, false, |file| {
        if lookup(&file.publications, command, tool) == choice {
            return false;
        }
        set(&mut file.publications, command, tool, choice);
        true
    })
}

/// The setting `file` has for `source` → `target` itself.
fn exact(file: &File, source: &str, target: &str) -> Option<Edge> {
    let (_, targets) = file.edges.iter().find(|(name, _)| name == source)?;
    let (_, edge) = targets.iter().find(|(name, _)| name == target)?;
    Some(*edge)
}

/// Sets what follows `source` when it recommends `target` (`None`: any other command): `edge`
/// in the file of `scope`, and for all projects without the project's own setting, which would
/// hide it here; with no edge, neither file keeps a setting for it, and autopilot decides.
pub(super) fn choose(
    global: Option<&Path>,
    root: &Path,
    (source, target): (&str, Option<&str>),
    choice: Option<(Edge, Scope)>,
) -> Result<(), String> {
    let project = crate::os::join(root, PROJECT_FILE);
    let target = target.unwrap_or("*");
    let global = || global.ok_or("no file for all projects: HOME is not set");
    match choice {
        Some((edge, Scope::Project)) => change(&project, true, source, target, Some(edge)),
        Some((edge, Scope::Global)) => {
            change(global()?, false, source, target, Some(edge))?;
            change(&project, true, source, target, None)
        }
        None => {
            change(&project, true, source, target, None)?;
            match global() {
                Ok(path) => change(path, false, source, target, None),
                Err(_) => Ok(()),
            }
        }
    }
}

/// Saves the suggestions of `steps` in the file of `scope` where it has no setting of its own
/// for them, as Scramjet's `applyRecommendations` adds only what is missing; how many it saved.
pub(super) fn apply(
    global: Option<&Path>,
    root: &Path,
    steps: &[&NextStep],
    scope: Scope,
) -> Result<usize, String> {
    let (path, in_project) = match scope {
        Scope::Project => (crate::os::join(root, PROJECT_FILE), true),
        Scope::Global => {
            let global = global.ok_or("no file for all projects: HOME is not set")?;
            (global.to_path_buf(), false)
        }
    };
    let mut applied = 0;
    update(&path, in_project, |file| {
        for step in steps {
            let target = step.target.as_deref().unwrap_or("*");
            if let Some(suggestion) = step.suggestion()
                && exact(file, &step.source, target).is_none()
            {
                set(&mut file.edges, &step.source, target, Some(suggestion.edge));
                applied += 1;
            }
        }
        applied > 0
    })?;
    Ok(applied)
}

/// Sets `source` → `target` to `edge` in the file at `path`, or takes it out.
fn change(
    path: &Path,
    in_project: bool,
    source: &str,
    target: &str,
    edge: Option<Edge>,
) -> Result<(), String> {
    update(path, in_project, |file| {
        if exact(file, source, target) == edge {
            return false;
        }
        set(&mut file.edges, source, target, edge);
        true
    })
}

/// Sets `name`'s setting for `key` among `entries` to `value`, or takes it out, with a command
/// left with none.
fn set<T>(entries: &mut Vec<(String, Vec<(String, T)>)>, name: &str, key: &str, value: Option<T>) {
    let at = match entries.iter().position(|(known, _)| known == name) {
        Some(at) => at,
        None => {
            entries.push((name.to_owned(), Vec::new()));
            entries.len() - 1
        }
    };
    let settings = &mut entries[at].1;
    match (settings.iter_mut().find(|(known, _)| known == key), value) {
        (Some((_, known)), Some(value)) => *known = value,
        (None, Some(value)) => settings.push((key.to_owned(), value)),
        (_, None) => settings.retain(|(known, _)| known != key),
    }
    entries.retain(|(_, settings)| !settings.is_empty());
}

/// Changes the file at `path` as `change` does, when it says it changed it. A file that would
/// not change is not written, and one left with no settings is removed, as Scramjet removes it.
fn update(
    path: &Path,
    in_project: bool,
    change: impl FnOnce(&mut File) -> bool,
) -> Result<(), String> {
    let shown = shown_path(path);
    let mut file = read(path, in_project)
        .map_err(|message| format!("{shown}: {message}"))?
        .unwrap_or_default();
    if !change(&mut file) {
        return Ok(());
    }
    let written = match file.edges.is_empty() && file.publications.is_empty() {
        true => std::fs::remove_file(path),
        false => write_replacing(path, &serialized(&file)),
    };
    written.map_err(|error| format!("{shown}: {error}"))
}

/// A file's settings in Scramjet's layout, in their order.
fn serialized(file: &File) -> String {
    fn section<T>(
        name: &str,
        entries: &[(String, Vec<(String, T)>)],
        value: fn(&T) -> &str,
    ) -> String {
        let mut text = String::new();
        for (command, settings) in entries.iter().filter(|(_, settings)| !settings.is_empty()) {
            text += &format!("  {}:\n", yaml_scalar(command));
            for (name, setting) in settings {
                text += &format!("    {}: {}\n", yaml_scalar(name), value(setting));
            }
        }
        match text.is_empty() {
            true => text,
            false => format!("{name}:\n{text}"),
        }
    }
    let edge = |edge: &Edge| match edge {
        Edge::Chain => "chain",
        Edge::Pause => "pause",
    };
    let publication = |publication: &Publication| match publication {
        Publication::Ask => "always-ask",
        Publication::Auto => "auto-approve",
    };
    section("edges", &file.edges, edge) + &section("publications", &file.publications, publication)
}

/// `name` as a YAML key that reads back as it is: plain when it is a name such as `rs:loop` or
/// `create_pr`, else double-quoted (JSON's quoting is YAML's).
fn yaml_scalar(name: &str) -> String {
    let plain = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && (name.chars()).all(|c| c.is_ascii_alphanumeric() || "_-.:/".contains(c))
        && !name.ends_with(':')
        && !["true", "false", "null", "yes", "no", "on", "off", "y", "n"]
            .contains(&name.to_ascii_lowercase().as_str());
    match plain {
        true => name.to_owned(),
        false => serde_json::to_string(name).unwrap_or_default(),
    }
}

fn shown_path(path: &Path) -> String {
    shortened(&path.display().to_string())
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

/// The edges an `autonomy-defaults.yaml` file suggests and its publication defaults, as
/// Scramjet's `parseAutonomyRecommendations` reads them: what it cannot use is skipped, with a
/// warning where Scramjet gives one.
pub(super) fn defaults(
    text: &str,
    warnings: &mut Vec<String>,
) -> Result<(Layer, Publications), String> {
    let document = load(text)?;
    if !matches!(document, Yaml::Hash(_)) {
        return Ok(Default::default());
    }
    let mut edges = Vec::new();
    if let Yaml::Hash(sources) = &document["edges"] {
        for (source, targets) in sources {
            let Yaml::Hash(targets) = targets else {
                continue;
            };
            let source = yaml_key(source);
            let mut settings = Vec::new();
            for (target, setting) in targets {
                let target = yaml_key(target);
                match setting.as_str() {
                    Some("chain") => settings.push((target, Edge::Chain)),
                    Some("pause") => settings.push((target, Edge::Pause)),
                    Some("default") | None => {}
                    Some(value) => warnings.push(format!(
                        "unknown autonomy recommendation value \"{value}\" for edge {source} → \
                         {target} (expected: chain, pause, default)"
                    )),
                }
            }
            if !settings.is_empty() {
                edges.push((source, settings));
            }
        }
    }
    let values = [
        ("require-approval", Publication::Ask),
        ("auto-approve", Publication::Auto),
    ];
    let publications = publications(&document["publications"], values, &mut |message| {
        warnings.push(message);
        Ok(())
    })?;
    Ok((edges, publications))
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
    let mut warnings = unknown_in_edges(&file.edges, commands);
    for (command, _) in &file.publications {
        if commands.get(command).is_none() {
            warnings.push(format!("unknown publication command \"{command}\""));
        }
    }
    warnings
}

/// The commands `edges` name that are not registered.
pub(super) fn unknown_in_edges(edges: &Layer, commands: &Commands) -> Vec<String> {
    let unknown = |name: &str| commands.get(name).is_none();
    let mut warnings = Vec::new();
    for (source, targets) in edges {
        if unknown(source) {
            warnings.push(format!("unknown source command \"{source}\""));
        }
        for (target, _) in targets {
            if target != "*" && unknown(target) {
                warnings.push(format!("unknown target command \"{target}\" (in {source})"));
            }
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
        let text = format!(
            "scramjet: global ({}): autonomy.yaml: invalid edge setting always for rs:loop → \
rs:loop; edge settings ignored this dispatch",
            global.display()
        );
        assert_eq!(edges.diagnostics, [Problem::in_file(text, &global)]);
        // Without files there are no edges.
        let edges = Edges::load(None, &root.join("none"), &commands());
        assert_eq!(
            (edges.resolve("rs:loop", "rs:loop"), edges.diagnostics.len()),
            (None, 0)
        );
    }

    fn flow() -> Commands {
        let closed = "---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:loop\n    - name: rs:steer\n---\nLoop";
        Commands::parsed(&[
            ("rs:loop.md", closed),
            (
                "rs:review.md",
                "---\nnext:\n  mode: forced\n  target: rs:loop\n---\nReview",
            ),
            ("rs:steer.md", "---\nnext:\n  mode: ask\n---\nSteer"),
            ("rs:plain.md", "Plain"),
        ])
    }

    #[test]
    fn next_steps_show_what_applies_and_where_it_is_from() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-autonomy-steps-{}", std::process::id())),
        );
        let root = &scratch.0;
        let global = root.join("global.yaml");
        let steps = || NextSteps::load(Some(&global), root, &flow());
        let step =
            |source: &str, target: Option<&str>, setting, (in_project, in_global)| NextStep {
                source: source.into(),
                target: target.map(str::to_owned),
                forced: false,
                setting,
                in_project,
                in_global,
                suggested: None,
            };
        let set = |edge, scope, any_other| {
            Some(Setting {
                edge,
                scope,
                any_other,
            })
        };
        let forced = NextStep {
            forced: true,
            ..step("rs:review", Some("rs:loop"), None, (false, false))
        };
        // Without files autopilot decides every step but the forced one; ask and no policy
        // have none.
        assert_eq!(
            steps().steps,
            [
                step("rs:loop", Some("rs:loop"), None, (false, false)),
                step("rs:loop", Some("rs:steer"), None, (false, false)),
                forced.clone(),
            ]
        );
        std::fs::create_dir_all(root.join(".scramjet")).unwrap();
        std::fs::write(
            root.join(".scramjet/autonomy.yaml"),
            "edges:\n  rs:loop:\n    rs:steer: pause\n  rs:review:\n    rs:loop: pause\n",
        )
        .unwrap();
        std::fs::write(
            &global,
            "edges:\n  rs:loop:\n    rs:steer: chain\n    '*': chain\n",
        )
        .unwrap();
        let loaded = steps();
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
        assert_eq!(
            loaded.steps,
            [
                step(
                    "rs:loop",
                    Some("rs:loop"),
                    set(Edge::Chain, Scope::Global, true),
                    (false, false)
                ),
                step(
                    "rs:loop",
                    Some("rs:steer"),
                    set(Edge::Pause, Scope::Project, false),
                    (true, true)
                ),
                step(
                    "rs:loop",
                    None,
                    set(Edge::Chain, Scope::Global, false),
                    (false, true)
                ),
                forced,
            ]
        );
        // A file that cannot be used leaves both unused, as routing does.
        std::fs::write(&global, "edges: [\n").unwrap();
        let broken = steps();
        assert!(broken.steps.iter().all(|step| step.setting.is_none()));
        assert_eq!(broken.problems.len(), 1);
        assert!(
            broken.problems[0]
                .starts_with(&format!("{}: autonomy.yaml:", crate::os::tilde(&global))),
            "{:?}",
            broken.problems
        );
    }

    #[test]
    fn choosing_a_next_step_writes_the_files_routing_reads() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-autonomy-choose-{}", std::process::id())),
        );
        let root = &scratch.0;
        let global = root.join("config/scramjet/autonomy.yaml");
        let project = crate::os::join(root, ".scramjet/autonomy.yaml");
        let choose = |target, choice| choose(Some(&global), root, ("rs:loop", target), choice);
        let edges = || Edges::load(Some(&global), root, &flow());
        let text = |path: &Path| std::fs::read_to_string(path).ok();

        choose(Some("rs:steer"), Some((Edge::Pause, Scope::Project))).unwrap();
        assert_eq!(
            text(&project).as_deref(),
            Some("edges:\n  rs:loop:\n    rs:steer: pause\n")
        );
        assert_eq!(text(&global), None);
        // For all projects: this project's own setting goes, so that it holds here too.
        choose(Some("rs:steer"), Some((Edge::Chain, Scope::Global))).unwrap();
        assert_eq!(text(&project), None, "an emptied file is removed");
        assert_eq!(edges().resolve("rs:loop", "rs:steer"), Some(Edge::Chain));
        std::fs::write(
            &global,
            "# mine\nedges:\n  rs:loop:\n    rs:steer: chain\n    '*': pause\n\
publications:\n  rs:loop:\n    create_pr: auto-approve\n",
        )
        .unwrap();
        // A change that changes nothing leaves the file as it is, comments and all.
        choose(Some("rs:steer"), Some((Edge::Chain, Scope::Global))).unwrap();
        choose(Some("rs:loop"), None).unwrap();
        assert!(text(&global).unwrap().starts_with("# mine\n"));
        choose(Some("rs:loop"), Some((Edge::Chain, Scope::Global))).unwrap();
        choose(None, Some((Edge::Chain, Scope::Project))).unwrap();
        assert_eq!(
            text(&global).as_deref(),
            Some(
                "edges:\n  rs:loop:\n    rs:steer: chain\n    \"*\": pause\n    rs:loop: chain\n\
publications:\n  rs:loop:\n    create_pr: auto-approve\n"
            )
        );
        assert_eq!(
            text(&project).as_deref(),
            Some("edges:\n  rs:loop:\n    \"*\": chain\n")
        );
        // Autopilot decides: neither file keeps a setting for it.
        choose(Some("rs:steer"), Some((Edge::Pause, Scope::Project))).unwrap();
        choose(Some("rs:steer"), None).unwrap();
        assert_eq!(
            text(&project).as_deref(),
            Some("edges:\n  rs:loop:\n    \"*\": chain\n")
        );
        let read_back = parse(&text(&global).unwrap(), false).unwrap();
        assert_eq!(
            read_back.edges,
            [(
                "rs:loop".to_owned(),
                vec![
                    ("*".to_owned(), Edge::Pause),
                    ("rs:loop".to_owned(), Edge::Chain)
                ]
            )]
        );
        assert_eq!(read_back.publications.len(), 1);
        // A file that cannot be read is not overwritten.
        std::fs::write(&project, "edges: [\n").unwrap();
        let error = choose(Some("rs:loop"), Some((Edge::Pause, Scope::Project))).unwrap_err();
        assert!(error.starts_with(&crate::os::tilde(&project)), "{error}");
        assert_eq!(text(&project).as_deref(), Some("edges: [\n"));
        assert!(choose(Some("rs:loop"), None).is_err());
        let without_home = super::choose(
            None,
            root,
            ("rs:loop", Some("rs:loop")),
            Some((Edge::Chain, Scope::Global)),
        );
        assert_eq!(
            without_home.unwrap_err(),
            "no file for all projects: HOME is not set"
        );
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "command files are named with `:`, which Windows cannot hold"
    )]
    fn sets_suggest_next_steps_which_hold_once_applied_where_nothing_is_set() {
        let scratch = ScratchDir::new(std::env::temp_dir().join(format!(
            "scramjet-autonomy-suggested-{}",
            std::process::id()
        )));
        let root = &scratch.0;
        let file = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        file(
            "sets/rs/commands/rs:loop.md",
            "---\nnext:\n  mode: closed\n  candidates:\n    - name: rs:loop\n    - name: rs:steer\n    \
             - name: rs:review\n---\nLoop",
        );
        file("sets/rs/commands/rs:steer.md", "Steer");
        file("sets/rs/commands/rs:review.md", "Review");
        file(
            "sets/rs/autonomy-defaults.yaml",
            "edges:\n  rs:loop:\n    rs:steer: chain\n    rs:loop: chain\n    rs:review: pause\n    \
             '*': pause\n  rs:gone:\n    rs:loop: chain\n",
        );
        // A set before it suggests first.
        file("sets/a/commands/a:x.md", "X");
        file(
            "sets/a/autonomy-defaults.yaml",
            "edges:\n  rs:loop:\n    rs:steer: pause\n",
        );
        let commands = Commands::discover(&super::super::Sets::find(&[], None, &root.join("sets")));
        let defaults = root.join("sets/rs/autonomy-defaults.yaml");
        let text = format!("{}: unknown source command \"rs:gone\"", defaults.display());
        assert_eq!(commands.warnings(), [Problem::in_file(text, &defaults)]);
        let global = root.join("config/autonomy.yaml");
        let project = crate::os::join(root, ".scramjet/autonomy.yaml");
        file(
            "config/autonomy.yaml",
            "edges:\n  rs:loop:\n    rs:review: chain\n",
        );
        file(
            ".scramjet/autonomy.yaml",
            "edges:\n  rs:loop:\n    '*': chain\n",
        );
        let steps = || NextSteps::load(Some(&global), root, &commands);
        let suggested = |edge, set: &str| {
            Some(Suggestion {
                edge,
                set: set.into(),
            })
        };
        let loaded = steps();
        assert_eq!(
            (loaded.steps.iter())
                .map(|step| (step.target.as_deref(), step.suggested.clone()))
                .collect::<Vec<_>>(),
            [
                (Some("rs:loop"), suggested(Edge::Chain, "rs")),
                (Some("rs:steer"), suggested(Edge::Pause, "a")),
                (Some("rs:review"), suggested(Edge::Pause, "rs")),
                (None, suggested(Edge::Pause, "rs")),
            ]
        );
        // Only rs:steer would change: rs:loop goes by the project's `*` as suggested, and the
        // user has set rs:review and `*` themselves.
        let pending = |steps: &NextSteps| {
            (steps.suggested().iter())
                .map(|step| step.target.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(pending(&loaded), [Some("rs:steer".to_owned())]);
        let text = |path: &Path| std::fs::read_to_string(path).unwrap();
        assert_eq!(
            apply(Some(&global), root, &loaded.suggested(), Scope::Project),
            Ok(1)
        );
        assert_eq!(
            text(&project),
            "edges:\n  rs:loop:\n    \"*\": chain\n    rs:steer: pause\n"
        );
        let edges = Edges::load(Some(&global), root, &commands);
        assert_eq!(edges.resolve("rs:loop", "rs:steer"), Some(Edge::Pause));
        assert!(pending(&steps()).is_empty());
        // For all projects, without a project file: what the global file sets stays.
        std::fs::remove_file(&project).unwrap();
        let loaded = steps();
        assert_eq!(
            pending(&loaded),
            [Some("rs:loop".into()), Some("rs:steer".into()), None]
        );
        assert_eq!(
            apply(Some(&global), root, &loaded.suggested(), Scope::Global),
            Ok(3)
        );
        assert_eq!(
            text(&global),
            "edges:\n  rs:loop:\n    rs:review: chain\n    rs:loop: chain\n    rs:steer: pause\n    \
             \"*\": pause\n"
        );
        assert!(pending(&steps()).is_empty());
        assert_eq!(apply(Some(&global), root, &[], Scope::Global), Ok(0));
        // A file that cannot be used offers nothing to apply.
        file(".scramjet/autonomy.yaml", "edges: [\n");
        assert!(steps().suggested().is_empty());
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "command files are named with `:`, which Windows cannot hold"
    )]
    fn approvals_list_each_publishing_commands_tools_with_who_says_what() {
        let scratch = ScratchDir::new(std::env::temp_dir().join(format!(
            "scramjet-autonomy-approvals-{}",
            std::process::id()
        )));
        let root = &scratch.0;
        let file = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        file(
            "sets/rs/commands/rs:post.md",
            "---\nallowed-tools: [add_pr_comment, read, create_issue]\n---\nPost",
        );
        file(
            "sets/rs/commands/rs:helper.md",
            "---\ndelegate-only: true\nallowed-tools: [create_pr]\n---\nHelp",
        );
        file("sets/rs/commands/rs:plain.md", "Plain");
        file(
            "sets/rs/autonomy-defaults.yaml",
            "publications:\n  rs:post:\n    create_issue: require-approval\n    add_pr_comment: auto-approve\n",
        );
        let commands = Commands::discover(&super::super::Sets::find(&[], None, &root.join("sets")));
        let global = root.join("autonomy.yaml");
        let approval = |tool, setting, default| Approval {
            command: "rs:post".into(),
            tool,
            setting,
            default,
        };
        // Without the user's file the set decides, in the tools' order; a delegate-only command
        // and one that names no publication tool have none.
        let approvals = Approvals::load(Some(&global), &commands);
        assert_eq!(
            (approvals.list.as_slice(), approvals.problem),
            (
                [
                    approval("create_issue", None, Some(Publication::Ask)),
                    approval("add_pr_comment", None, Some(Publication::Auto)),
                ]
                .as_slice(),
                None
            )
        );
        assert_eq!(
            approvals
                .list
                .iter()
                .map(Approval::applies)
                .collect::<Vec<_>>(),
            [Publication::Ask, Publication::Auto]
        );
        // The user's setting comes first; with neither, it asks.
        assert_eq!(
            approval("create_pr", None, None).applies(),
            Publication::Ask
        );
        std::fs::write(
            &global,
            "publications:\n  rs:post:\n    create_issue: auto-approve\n    add_pr_comment: always-ask\n",
        )
        .unwrap();
        let approvals = Approvals::load(Some(&global), &commands);
        assert_eq!(
            approvals
                .list
                .iter()
                .map(Approval::applies)
                .collect::<Vec<_>>(),
            [Publication::Auto, Publication::Ask]
        );
        // A file that cannot be used says why; the publication tools then ask.
        std::fs::write(&global, "publications: [\n").unwrap();
        let approvals = Approvals::load(Some(&global), &commands);
        assert!(
            approvals
                .list
                .iter()
                .all(|approval| approval.setting.is_none())
        );
        let problem = approvals.problem.unwrap();
        assert!(
            problem.starts_with(&format!("{}: autonomy.yaml:", global.display())),
            "{problem}"
        );
    }

    #[test]
    fn choosing_a_publication_setting_writes_the_global_file_the_tools_read() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-autonomy-approve-{}", std::process::id())),
        );
        let global = scratch.0.join("config/scramjet/autonomy.yaml");
        let choose =
            |tool, choice| choose_publication(Some(&global), "rs:post", tool, choice).unwrap();
        let text = || std::fs::read_to_string(&global).ok();
        choose("create_issue", Some(Publication::Auto));
        assert_eq!(
            text().as_deref(),
            Some("publications:\n  rs:post:\n    create_issue: auto-approve\n")
        );
        assert_eq!(
            publication_override(Some(&global), "rs:post", "create_issue"),
            Ok(Some(Publication::Auto))
        );
        // The set decides again: the setting goes, and so does the emptied file.
        choose("create_issue", None);
        assert_eq!(text(), None);
        std::fs::write(
            &global,
            "# mine\nedges:\n  rs:post:\n    rs:post: chain\npublications:\n  rs:post:\n    create_pr: always-ask\n",
        )
        .unwrap();
        // A change that changes nothing leaves the file as it is, comments and all.
        choose("create_pr", Some(Publication::Ask));
        choose("add_pr_comment", None);
        assert!(text().unwrap().starts_with("# mine\n"));
        choose("create_pr", Some(Publication::Auto));
        choose("add_pr_comment", Some(Publication::Ask));
        assert_eq!(
            text().as_deref(),
            Some(
                "edges:\n  rs:post:\n    rs:post: chain\npublications:\n  rs:post:\n    \
create_pr: auto-approve\n    add_pr_comment: always-ask\n"
            )
        );
        // A file that cannot be read is not overwritten.
        std::fs::write(&global, "publications: [\n").unwrap();
        let error = choose_publication(Some(&global), "rs:post", "create_pr", None).unwrap_err();
        assert!(error.starts_with(&crate::os::tilde(&global)), "{error}");
        assert_eq!(text().as_deref(), Some("publications: [\n"));
        assert_eq!(
            choose_publication(None, "rs:post", "create_pr", Some(Publication::Ask)).unwrap_err(),
            "no file for all projects: HOME is not set"
        );
    }

    #[test]
    fn names_are_written_so_that_they_read_back_as_they_are() {
        let names = [
            "rs:loop",
            "create_pr",
            "*",
            "true",
            "No",
            "a: b",
            "end:",
            "it's",
            "1e3",
            "a\"b\nc",
            "~",
        ];
        let file = File {
            edges: names
                .iter()
                .map(|name| (name.to_string(), vec![(name.to_string(), Edge::Pause)]))
                .collect(),
            publications: vec![("a b".into(), vec![("create_pr".into(), Publication::Ask)])],
        };
        let text = serialized(&file);
        assert!(
            text.contains("\n  rs:loop:\n    rs:loop: pause\n"),
            "{text}"
        );
        assert!(text.contains("\n    create_pr: always-ask\n"), "{text}");
        assert_eq!(parse(&text, false), Ok(file));
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
        let texts = [
            "unknown source command \"rs:gone\"",
            "unknown target command \"rs:old\" (in rs:loop)",
            "unknown publication command \"rs:old\"",
        ];
        let problems = texts.map(|text| Problem::in_file(format!("{prefix}{text}"), &global));
        assert_eq!(edges.diagnostics, problems);
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
    fn defaults_skip_what_they_cannot_use_with_a_warning() {
        let mut warnings = Vec::new();
        let (edges, publications) = defaults(
            &[
                "edges:\n  a:\n    b: chain\n    c: default\n    d: always\n    e: 1\n    '*': pause",
                "  b: chain\n  c:\n    d: default\npublications:\n  a:",
                "    create_issue: require-approval\n    add_issue_comment: auto-approve",
                "    merge_pr: auto-approve\n    create_pr: always-ask\n  b: auto-approve",
            ]
            .join("\n"),
            &mut warnings,
        )
        .unwrap();
        assert_eq!(
            edges,
            [(
                "a".to_owned(),
                vec![("b".to_owned(), Edge::Chain), ("*".to_owned(), Edge::Pause)]
            )]
        );
        assert_eq!(
            publications,
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
                "unknown autonomy recommendation value \"always\" for edge a → d (expected: \
                 chain, pause, default)",
                "unknown publication tool merge_pr for a",
                "invalid publication setting always-ask for a → create_pr",
                "publication settings for b must be a tool map",
            ]
        );
        assert_eq!(defaults("- a", &mut warnings), Ok(Default::default()));
        assert!(defaults("a: [", &mut warnings).is_err());
    }
}
