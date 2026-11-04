//! scramjet-rs's own settings file, `$XDG_CONFIG_HOME/scramjet-rs/settings.json` (by default
//! under `~/.config`), apart from Scramjet's, in its format and with its key names where the
//! behavior matches.
use super::tree::Filter;
use scramjet_rs::agent::{
    CODING_TOOLS, DEFAULT_TOOLS, Origin, Set, search_program, shortened, write_replacing,
};
use serde_json::{Map, Value, json};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Scramjet's thinking levels; its `off` is the `none` of Proxy's model cards.
pub(super) const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Scramjet's name for a model card's effort level.
pub(super) fn scramjet_level(level: &str) -> &str {
    if level == "none" { "off" } else { level }
}

/// The model cards' name for one of Scramjet's thinking levels, or for a card's own `none`;
/// `None` for another word.
pub(super) fn card_level(level: &str) -> Option<&str> {
    match level {
        "off" | "none" => Some("none"),
        level => LEVELS.contains(&level).then_some(level),
    }
}

/// What Escape twice on an empty prompt opens (`doubleEscapeAction`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DoubleEscape {
    Tree,
    Fork,
    Nothing,
}

/// When the agent's coding tools run with the credential stores hidden (`hideCredentials`, a
/// key of scramjet-rs's own).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum HideCredentials {
    /// In turns that no one started: a next step run at once, and `-p`.
    Unattended,
    Always,
    Never,
}

impl HideCredentials {
    /// Whether a turn hides them; `unattended`: it started without the user.
    pub(super) fn hides(self, unattended: bool) -> bool {
        match self {
            Self::Unattended => unattended,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

/// How many queued messages go to the model at once (`steeringMode`, `followUpMode`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Delivery {
    All,
    OneAtATime,
}

/// A row of `/settings`, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Row {
    ThinkingLevel,
    ThinkingBlocks,
    Steering,
    FollowUp,
    DoubleEscape,
    BranchSummary,
    TreeFilter,
    Tools,
    Credentials,
    Autopilot,
    NextSteps,
    Publishing,
    Installed,
}

pub(super) const ROWS: [Row; 13] = [
    Row::ThinkingLevel,
    Row::ThinkingBlocks,
    Row::Steering,
    Row::FollowUp,
    Row::DoubleEscape,
    Row::BranchSummary,
    Row::TreeFilter,
    Row::Tools,
    Row::Credentials,
    Row::Autopilot,
    Row::NextSteps,
    Row::Publishing,
    Row::Installed,
];

impl Row {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ThinkingLevel => "Thinking level",
            Self::ThinkingBlocks => "Thinking blocks",
            Self::Steering => "Steering messages",
            Self::FollowUp => "Follow-up messages",
            Self::DoubleEscape => "Escape twice",
            Self::BranchSummary => "Leaving a branch",
            Self::TreeFilter => "Tree entries",
            Self::Tools => "Tools",
            Self::Credentials => "Credential stores",
            Self::Autopilot => "Autopilot",
            Self::NextSteps => "Next steps",
            Self::Publishing => "Publishing",
            Self::Installed => "Installed",
        }
    }

    /// What the row is about, shown under the list while it is selected.
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::ThinkingLevel => {
                "How much the model reasons; also the level new sessions start at. Shift+Tab cycles it."
            }
            Self::ThinkingBlocks => "The model's reasoning in the conversation. Ctrl+T toggles it.",
            Self::Steering => {
                "Sent with Enter during a turn: how many the model gets before its next request."
            }
            Self::FollowUp => {
                "Sent with Alt+Enter during a turn: how many the model gets when it would stop."
            }
            Self::DoubleEscape => "Escape pressed twice on an empty prompt.",
            Self::BranchSummary => {
                "When /tree moves to another branch: ask whether to summarize the one left behind."
            }
            Self::TreeFilter => {
                "The entries /tree shows when it opens; there Ctrl+O cycles through the choices."
            }
            Self::Tools => "The coding tools the agent has. Subagents have their own.",
            Self::Credentials => {
                "~/.ssh, ~/.netrc and other known stores, kept from the agent's tools: in turns no one \
                 started (a next step run at once, -p), always or never. Subagents never see them."
            }
            Self::Autopilot => {
                "On: when a command finishes, the next step it recommends runs after a 3-second \
                 countdown, unless you choose. For this session; /autopilot switches it too."
            }
            Self::NextSteps => {
                "What follows a command that finishes and recommends a next one: continue \
                 automatically, ask you, or let autopilot decide; command sets can suggest which. \
                 Kept in Scramjet's autonomy.yaml files, which Scramjet reads too."
            }
            Self::Publishing => {
                "Whether the issues, pull requests and comments a command publishes on GitHub wait \
                 for your approval, in the project's own repository; elsewhere they always do. \
                 Kept in Scramjet's autonomy.yaml for all projects, which Scramjet reads too."
            }
            Self::Installed => {
                "What this session loaded: command sets, commands, agents, skills, context and \
                 settings files, where each comes from, and what is wrong there."
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Settings {
    /// `None` without `XDG_CONFIG_HOME` and `HOME`: nothing is read or saved.
    path: Option<PathBuf>,
    pub(super) hide_thinking_block: bool,
    pub(super) double_escape: DoubleEscape,
    /// `branchSummary.skipPrompt`: leaving a branch in `/tree` summarizes nothing, unasked.
    pub(super) skip_branch_summary_prompt: bool,
    /// `treeFilterMode`: the entries `/tree` shows when it opens.
    pub(super) tree_filter: Filter,
    /// `defaultThinkingLevel`, named as the model cards name levels.
    pub(super) default_thinking_level: Option<String>,
    /// `tools`: the agent's coding tools, Scramjet's by default.
    pub(super) tools: Vec<String>,
    pub(super) hide_credentials: HideCredentials,
    pub(super) steering_mode: Delivery,
    pub(super) follow_up_mode: Delivery,
    /// `commandSets`: the directories of the installed command sets; a relative one is relative
    /// to the settings file's directory.
    pub(super) command_sets: Vec<PathBuf>,
    /// `skills`: skill files and directories besides those found by default, as `commandSets`'.
    pub(super) skills: Vec<PathBuf>,
    /// `enabledModels`: the patterns of the models Ctrl+P cycles through (`scope::resolve`);
    /// empty: all.
    pub(super) enabled_models: Vec<String>,
    /// What is wrong in the file; those keys keep their defaults.
    pub(super) warnings: Vec<String>,
}

impl Settings {
    pub(super) fn load() -> Self {
        let path = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|config| !config.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|config| config.join("scramjet-rs/settings.json"));
        Self::read(path)
    }

    fn read(path: Option<PathBuf>) -> Self {
        let mut settings = Self::defaults(path);
        let Some(path) = &settings.path else {
            return settings;
        };
        match read_object(path) {
            Ok(object) => settings.apply(&object),
            Err(error) => settings
                .warnings
                .push(format!("{error}; using the defaults")),
        }
        settings
    }

    /// `/reload`: the file read again; one that cannot be read leaves the settings as they were,
    /// as Scramjet's reload does.
    pub(super) fn reload(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        match read_object(path) {
            Ok(object) => {
                *self = Self::defaults(self.path.take());
                self.apply(&object);
            }
            Err(error) => self.warnings = vec![format!("{error}; kept as they were")],
        }
    }

    fn defaults(path: Option<PathBuf>) -> Self {
        Self {
            path,
            hide_thinking_block: false,
            double_escape: DoubleEscape::Tree,
            skip_branch_summary_prompt: false,
            tree_filter: Filter::Default,
            default_thinking_level: None,
            tools: DEFAULT_TOOLS.map(str::to_owned).to_vec(),
            hide_credentials: HideCredentials::Unattended,
            steering_mode: Delivery::All,
            follow_up_mode: Delivery::OneAtATime,
            command_sets: Vec::new(),
            skills: Vec::new(),
            enabled_models: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn apply(&mut self, object: &Map<String, Value>) {
        let path = shown(self.path.as_deref().unwrap_or(Path::new("")));
        let mut wrong = |key: &str, expected: &str| {
            self.warnings
                .push(format!("{path}: {key} must be {expected}; ignored"))
        };
        match object.get("hideThinkingBlock") {
            None => {}
            Some(Value::Bool(hide)) => self.hide_thinking_block = *hide,
            Some(_) => wrong("hideThinkingBlock", "true or false"),
        }
        match object.get("doubleEscapeAction").map(|value| value.as_str()) {
            None => {}
            Some(Some("tree")) => self.double_escape = DoubleEscape::Tree,
            Some(Some("fork")) => self.double_escape = DoubleEscape::Fork,
            Some(Some("none")) => self.double_escape = DoubleEscape::Nothing,
            Some(_) => wrong("doubleEscapeAction", r#""tree", "fork" or "none""#),
        }
        match object
            .get("branchSummary")
            .map(|value| value.get("skipPrompt"))
        {
            None | Some(None) => {}
            Some(Some(Value::Bool(skip))) => self.skip_branch_summary_prompt = *skip,
            Some(Some(_)) => wrong("branchSummary.skipPrompt", "true or false"),
        }
        match object.get("treeFilterMode").map(|value| value.as_str()) {
            None => {}
            Some(Some(name))
                if let Some(&filter) = Filter::CYCLE.iter().find(|f| f.setting() == name) =>
            {
                self.tree_filter = filter
            }
            Some(_) => wrong(
                "treeFilterMode",
                &Filter::CYCLE
                    .map(|f| format!("\"{}\"", f.setting()))
                    .join(", "),
            ),
        }
        match object
            .get("defaultThinkingLevel")
            .map(|value| value.as_str())
        {
            None => {}
            Some(Some("off")) => self.default_thinking_level = Some("none".to_owned()),
            Some(Some(level)) if LEVELS.contains(&level) => {
                self.default_thinking_level = Some(level.to_owned())
            }
            Some(_) => wrong(
                "defaultThinkingLevel",
                &LEVELS.map(|l| format!("\"{l}\"")).join(", "),
            ),
        }
        match object.get("hideCredentials").map(|value| value.as_str()) {
            None => {}
            Some(Some("unattended")) => self.hide_credentials = HideCredentials::Unattended,
            Some(Some("always")) => self.hide_credentials = HideCredentials::Always,
            Some(Some("never")) => self.hide_credentials = HideCredentials::Never,
            Some(_) => wrong("hideCredentials", r#""unattended", "always" or "never""#),
        }
        for (key, mode) in [
            ("steeringMode", &mut self.steering_mode),
            ("followUpMode", &mut self.follow_up_mode),
        ] {
            match object.get(key).map(|value| value.as_str()) {
                None => {}
                Some(Some("all")) => *mode = Delivery::All,
                Some(Some("one-at-a-time")) => *mode = Delivery::OneAtATime,
                Some(_) => wrong(key, r#""all" or "one-at-a-time""#),
            }
        }
        match object.get("tools") {
            None => {}
            Some(Value::Array(names)) => self.tools = self.read_tools(&path, names),
            Some(_) => wrong(
                "tools",
                &format!("a list of tool names ({})", CODING_TOOLS.join(", ")),
            ),
        }
        match command_sets(object) {
            Ok(entries) => {
                self.command_sets = (entries.iter()).map(|entry| self.resolve(entry)).collect()
            }
            Err(()) => self.warnings.push(format!(
                "{path}: commandSets must be a list of directories; ignored"
            )),
        }
        match paths(object, "skills") {
            Ok(entries) => {
                self.skills = (entries.iter()).map(|entry| self.resolve(entry)).collect()
            }
            Err(()) => self.warnings.push(format!(
                "{path}: skills must be a list of files and directories; ignored"
            )),
        }
        match paths(object, "enabledModels") {
            Ok(patterns) => self.enabled_models = patterns,
            Err(()) => self.warnings.push(format!(
                "{path}: enabledModels must be a list of model patterns; ignored"
            )),
        }
    }

    /// A path in the settings, as Pi resolves one: `~` is the home directory, and a relative one
    /// is relative to the settings file's directory.
    fn resolve(&self, entry: &str) -> PathBuf {
        let entry = entry.trim();
        let home = std::env::home_dir().filter(|_| entry == "~" || entry.starts_with("~/"));
        if let Some(home) = home {
            return home.join(entry[1..].trim_start_matches('/'));
        }
        let directory = self.path.as_deref().and_then(Path::parent);
        directory.unwrap_or(Path::new("/")).join(entry)
    }

    /// Adds the command set in `directory` to `commandSets`, by its absolute path, unless a set
    /// of its name is there. Returns it.
    pub(super) fn install_set(&mut self, directory: &Path) -> Result<Set, String> {
        let directory = std::path::absolute(directory).map_err(|e| e.to_string())?;
        let set = Set::at(&directory, Origin::Installed)?;
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let mut entries = command_sets(&object).map_err(|()| {
            format!(
                "{}: commandSets must be a list of directories",
                shown(&path)
            )
        })?;
        for entry in &entries {
            let known = self.resolve(entry);
            if known.file_name() == directory.file_name() {
                return Err(match same_directory(&known, &directory) {
                    true => format!("{} is already installed", set.name),
                    false => format!(
                        "a command set named {} is installed from {}; remove it first",
                        set.name,
                        known.display()
                    ),
                });
            }
        }
        entries.push(directory.display().to_string());
        object.insert("commandSets".into(), json!(entries));
        write_object(&path, object)?;
        self.command_sets.push(directory);
        Ok(set)
    }

    /// Removes from `commandSets` the set named `reference`, or installed from that directory.
    /// Returns the directory it was installed from.
    pub(super) fn remove_set(&mut self, reference: &str) -> Result<PathBuf, String> {
        let path = self
            .path
            .clone()
            .ok_or("no settings file: HOME is not set")?;
        let mut object = read_object(&path)?;
        let mut entries = command_sets(&object).map_err(|()| {
            format!(
                "{}: commandSets must be a list of directories",
                shown(&path)
            )
        })?;
        let given = std::path::absolute(reference).ok();
        let position = entries.iter().position(|entry| {
            let known = self.resolve(entry);
            known.file_name().is_some_and(|name| name == reference)
                || given
                    .as_deref()
                    .is_some_and(|given| same_directory(&known, given))
        });
        let Some(position) = position else {
            return Err(format!(
                "no installed command set is named {reference} or installed from there"
            ));
        };
        let removed = self.resolve(&entries.remove(position));
        object.insert("commandSets".into(), json!(entries));
        write_object(&path, object)?;
        self.command_sets.retain(|directory| directory != &removed);
        Ok(removed)
    }

    pub(super) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The settings file, shortened for display.
    pub(super) fn shown_path(&self) -> Option<String> {
        self.path.as_deref().map(shown)
    }

    /// The coding tools `names` lists that there are and that can run here.
    fn read_tools(&mut self, path: &str, names: &[Value]) -> Vec<String> {
        let mut tools: Vec<String> = Vec::new();
        for name in names {
            match name.as_str() {
                Some(tool) if CODING_TOOLS.contains(&tool) => {
                    let missing = match tool {
                        "grep" => search_program(tool).is_none().then_some("ripgrep (rg)"),
                        "find" => search_program(tool).is_none().then_some("fd"),
                        _ => None,
                    };
                    match missing {
                        Some(program) => self.warnings.push(format!(
                            "{path}: tools: {tool} left out, as {program} is not installed"
                        )),
                        None if !tools.iter().any(|known| known == tool) => {
                            tools.push(tool.to_owned())
                        }
                        None => {}
                    }
                }
                _ => self.warnings.push(format!(
                    "{path}: tools: {name} is not one of {}; ignored",
                    CODING_TOOLS.join(", ")
                )),
            }
        }
        tools
    }

    /// Ctrl+T's choice, saved when it changed.
    pub(super) fn set_hide_thinking_block(&mut self, hide: bool) -> io::Result<()> {
        if self.hide_thinking_block == hide {
            return Ok(());
        }
        self.hide_thinking_block = hide;
        self.save("hideThinkingBlock", json!(hide))
    }

    /// A reasoning effort chosen with Shift+Tab, saved when it changed.
    pub(super) fn set_default_thinking_level(&mut self, level: &str) -> io::Result<()> {
        if self.default_thinking_level.as_deref() == Some(level) {
            return Ok(());
        }
        self.default_thinking_level = Some(level.to_owned());
        self.save("defaultThinkingLevel", json!(scramjet_level(level)))
    }

    /// `/scoped-models`' choice: the models Ctrl+P cycles through, in order; none: all of them,
    /// which leaves `enabledModels` out, as Scramjet saves it.
    pub(super) fn set_enabled_models(&mut self, models: Vec<String>) -> io::Result<()> {
        self.enabled_models = models.clone();
        self.update(|object| match models.is_empty() {
            true => drop(object.remove("enabledModels")),
            false => drop(object.insert("enabledModels".into(), json!(models))),
        })
    }

    /// The value `row` shows in `/settings`; the thinking level, autopilot, the next steps,
    /// publishing and what is installed are the session's, not the settings'.
    pub(super) fn shown(&self, row: Row) -> String {
        let delivery = |mode| match mode {
            Delivery::All => "all at once",
            Delivery::OneAtATime => "one at a time",
        };
        match row {
            Row::ThinkingLevel
            | Row::Autopilot
            | Row::NextSteps
            | Row::Publishing
            | Row::Installed => String::new(),
            Row::ThinkingBlocks if self.hide_thinking_block => "hidden".into(),
            Row::ThinkingBlocks => "shown".into(),
            Row::Steering => delivery(self.steering_mode).into(),
            Row::FollowUp => delivery(self.follow_up_mode).into(),
            Row::DoubleEscape => match self.double_escape {
                DoubleEscape::Tree => "opens /tree",
                DoubleEscape::Fork => "opens /fork",
                DoubleEscape::Nothing => "does nothing",
            }
            .into(),
            Row::BranchSummary if self.skip_branch_summary_prompt => "no summary".into(),
            Row::BranchSummary => "asks for a summary".into(),
            Row::TreeFilter => match self.tree_filter {
                Filter::Default => "all but model changes",
                Filter::NoTools => "no tool results",
                Filter::UserOnly => "prompts only",
                Filter::Labeled => "labeled only",
                Filter::All => "everything",
            }
            .into(),
            Row::Tools if self.tools.is_empty() => "none".into(),
            Row::Tools => (CODING_TOOLS.iter())
                .filter(|tool| self.tools.iter().any(|known| known == *tool))
                .copied()
                .collect::<Vec<_>>()
                .join(", "),
            Row::Credentials => match self.hide_credentials {
                HideCredentials::Unattended => "hidden when unattended",
                HideCredentials::Always => "always hidden",
                HideCredentials::Never => "never hidden",
            }
            .into(),
        }
    }

    /// Moves `row` to its next value, as Enter in `/settings` does, and saves it. Those that are
    /// not in the file, and the tools, which have a list of their own, stay.
    pub(super) fn cycle(&mut self, row: Row) -> io::Result<()> {
        let next = |mode| match mode {
            Delivery::All => Delivery::OneAtATime,
            Delivery::OneAtATime => Delivery::All,
        };
        let delivery = |mode| match mode {
            Delivery::All => "all",
            Delivery::OneAtATime => "one-at-a-time",
        };
        match row {
            Row::ThinkingLevel
            | Row::Tools
            | Row::Autopilot
            | Row::NextSteps
            | Row::Publishing
            | Row::Installed => Ok(()),
            Row::ThinkingBlocks => self.set_hide_thinking_block(!self.hide_thinking_block),
            Row::Steering => {
                self.steering_mode = next(self.steering_mode);
                self.save("steeringMode", json!(delivery(self.steering_mode)))
            }
            Row::FollowUp => {
                self.follow_up_mode = next(self.follow_up_mode);
                self.save("followUpMode", json!(delivery(self.follow_up_mode)))
            }
            Row::DoubleEscape => {
                let (action, name) = match self.double_escape {
                    DoubleEscape::Tree => (DoubleEscape::Fork, "fork"),
                    DoubleEscape::Fork => (DoubleEscape::Nothing, "none"),
                    DoubleEscape::Nothing => (DoubleEscape::Tree, "tree"),
                };
                self.double_escape = action;
                self.save("doubleEscapeAction", json!(name))
            }
            Row::BranchSummary => {
                self.skip_branch_summary_prompt = !self.skip_branch_summary_prompt;
                let skip = self.skip_branch_summary_prompt;
                self.save_in("branchSummary", "skipPrompt", json!(skip))
            }
            Row::TreeFilter => {
                let at = Filter::CYCLE.iter().position(|&f| f == self.tree_filter);
                let next = (at.unwrap_or(0) + 1) % Filter::CYCLE.len();
                self.tree_filter = Filter::CYCLE[next];
                self.save("treeFilterMode", json!(self.tree_filter.setting()))
            }
            Row::Credentials => {
                let (hide, name) = match self.hide_credentials {
                    HideCredentials::Unattended => (HideCredentials::Always, "always"),
                    HideCredentials::Always => (HideCredentials::Never, "never"),
                    HideCredentials::Never => (HideCredentials::Unattended, "unattended"),
                };
                self.hide_credentials = hide;
                self.save("hideCredentials", json!(name))
            }
        }
    }

    /// The agent's coding tools, chosen in `/settings`, saved in `CODING_TOOLS`' order.
    pub(super) fn set_tools(&mut self, tools: Vec<String>) -> io::Result<()> {
        let tools: Vec<String> = (CODING_TOOLS.iter())
            .filter(|tool| tools.iter().any(|chosen| chosen == *tool))
            .map(|tool| tool.to_string())
            .collect();
        let saved = json!(tools);
        self.tools = tools;
        self.save("tools", saved)
    }

    /// Sets `key` in the file as it is now, keeping the rest of it; a file that cannot be read
    /// is left as it is.
    fn save(&self, key: &str, value: Value) -> io::Result<()> {
        self.update(|object| {
            object.insert(key.to_owned(), value);
        })
    }

    /// Sets `key` in the object `parent`, keeping its other keys.
    fn save_in(&self, parent: &str, key: &str, value: Value) -> io::Result<()> {
        self.update(|object| {
            let entry = object.entry(parent).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            if let Value::Object(inner) = entry {
                inner.insert(key.to_owned(), value);
            }
        })
    }

    fn update(&self, change: impl FnOnce(&mut Map<String, Value>)) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut object = read_object(path).map_err(io::Error::other)?;
        change(&mut object);
        write_object(path, object).map_err(io::Error::other)
    }
}

/// `commandSets`' entries; an error when it is not a list of strings.
fn command_sets(object: &Map<String, Value>) -> Result<Vec<String>, ()> {
    paths(object, "commandSets")
}

/// The entries of the list of paths `key`; an error when it is not a list of strings.
fn paths(object: &Map<String, Value>, key: &str) -> Result<Vec<String>, ()> {
    match object.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(entries)) => (entries.iter())
            .map(|entry| entry.as_str().map(str::to_owned).ok_or(()))
            .collect(),
        Some(_) => Err(()),
    }
}

/// Whether `a` and `b` are the same directory, also through links.
fn same_directory(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

fn write_object(path: &Path, object: Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    write_replacing(path, &(text + "\n")).map_err(|error| format!("{}: {error}", shown(path)))
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}

/// The settings in `path`; none when there is no file.
fn read_object(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("{}: {error}", shown(path))),
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(format!("{}: not a JSON object", shown(path))),
        Err(error) => Err(format!("{}: {error}", shown(path))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "scramjet-rs-settings-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn settings_with(scratch: &Scratch, text: &str) -> Settings {
        let path = scratch.0.join("settings.json");
        std::fs::write(&path, text).unwrap();
        Settings::read(Some(path))
    }

    #[test]
    fn without_a_file_the_defaults_are_scramjets() {
        let scratch = Scratch::new("missing");
        let settings = Settings::read(Some(scratch.0.join("config/settings.json")));
        assert!(!settings.hide_thinking_block);
        assert_eq!(settings.double_escape, DoubleEscape::Tree);
        assert!(!settings.skip_branch_summary_prompt);
        assert_eq!(settings.tree_filter, Filter::Default);
        assert_eq!(settings.default_thinking_level, None);
        assert_eq!(settings.tools, ["read", "bash", "edit", "write"]);
        assert_eq!(settings.hide_credentials, HideCredentials::Unattended);
        assert_eq!(settings.steering_mode, Delivery::All);
        assert_eq!(settings.follow_up_mode, Delivery::OneAtATime);
        assert!(settings.skills.is_empty());
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    }

    #[test]
    fn hide_credentials_says_which_turns_hide_them() {
        let scratch = Scratch::new("credentials");
        for (value, typed, unattended) in [
            ("unattended", false, true),
            ("always", true, true),
            ("never", false, false),
        ] {
            let settings = settings_with(&scratch, &format!(r#"{{"hideCredentials": "{value}"}}"#));
            assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
            let hides = settings.hide_credentials;
            assert_eq!((hides.hides(false), hides.hides(true)), (typed, unattended));
        }
        let wrong = settings_with(&scratch, r#"{"hideCredentials": true}"#);
        assert_eq!(wrong.hide_credentials, HideCredentials::Unattended);
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            wrong.warnings,
            [format!(
                r#"{path}: hideCredentials must be "unattended", "always" or "never"; ignored"#
            )]
        );
    }

    #[test]
    fn tools_lists_the_agents_coding_tools() {
        let scratch = Scratch::new("tools");
        let settings = settings_with(
            &scratch,
            r#"{"tools": ["read", "grep", "gerp", "bash", 3, "ls", "read", "find"]}"#,
        );
        assert_eq!(settings.tools, ["read", "grep", "bash", "ls", "find"]);
        let path = shown(&scratch.0.join("settings.json"));
        let known = "read, bash, edit, write, grep, find, ls";
        assert_eq!(
            settings.warnings,
            [
                format!(r#"{path}: tools: "gerp" is not one of {known}; ignored"#),
                format!("{path}: tools: 3 is not one of {known}; ignored"),
            ]
        );
        let none = settings_with(&scratch, r#"{"tools": []}"#);
        assert!(none.tools.is_empty() && none.warnings.is_empty());
        let wrong = settings_with(&scratch, r#"{"tools": "read,grep"}"#);
        assert_eq!(wrong.tools, ["read", "bash", "edit", "write"]);
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: tools must be a list of tool names ({known}); ignored"
            )]
        );
    }

    #[test]
    fn reads_scramjets_keys() {
        let scratch = Scratch::new("keys");
        let settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": true, "doubleEscapeAction": "fork",
                "branchSummary": {"reserveTokens": 1, "skipPrompt": true},
                "defaultThinkingLevel": "off", "theme": "dark", "treeFilterMode": "labeled-only",
                "steeringMode": "one-at-a-time", "followUpMode": "all"}"#,
        );
        assert_eq!(settings.tree_filter, Filter::Labeled);
        assert!(settings.hide_thinking_block);
        assert_eq!(settings.steering_mode, Delivery::OneAtATime);
        assert_eq!(settings.follow_up_mode, Delivery::All);
        assert_eq!(settings.double_escape, DoubleEscape::Fork);
        assert!(settings.skip_branch_summary_prompt);
        assert_eq!(settings.default_thinking_level.as_deref(), Some("none"));
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let none = settings_with(&scratch, r#"{"doubleEscapeAction": "none"}"#);
        assert_eq!(none.double_escape, DoubleEscape::Nothing);
    }

    #[test]
    fn skills_are_paths_from_home_or_the_settings_directory() {
        let scratch = Scratch::new("skills");
        let settings = settings_with(
            &scratch,
            r#"{"skills": ["~/.claude/skills", "~", "own", "/abs/one.md"]}"#,
        );
        let home = std::env::home_dir().unwrap();
        assert_eq!(
            settings.skills,
            [
                home.join(".claude/skills"),
                home.clone(),
                scratch.0.join("own"),
                PathBuf::from("/abs/one.md"),
            ]
        );
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let wrong = settings_with(&scratch, r#"{"skills": "~/.claude/skills"}"#);
        assert!(wrong.skills.is_empty());
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            wrong.warnings,
            [format!(
                "{path}: skills must be a list of files and directories; ignored"
            )]
        );
    }

    #[test]
    fn a_wrong_value_is_named_and_keeps_its_default() {
        let scratch = Scratch::new("wrong");
        let settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": "yes", "doubleEscapeAction": "tree", "defaultThinkingLevel": "hgih",
                "branchSummary": {"skipPrompt": 1}, "followUpMode": "each", "treeFilterMode": "user"}"#,
        );
        assert_eq!(settings.tree_filter, Filter::Default);
        assert!(!settings.hide_thinking_block);
        assert_eq!(settings.follow_up_mode, Delivery::OneAtATime);
        assert_eq!(settings.default_thinking_level, None);
        let path = shown(&scratch.0.join("settings.json"));
        assert_eq!(
            settings.warnings,
            [
                format!("{path}: hideThinkingBlock must be true or false; ignored"),
                format!("{path}: branchSummary.skipPrompt must be true or false; ignored"),
                format!(
                    r#"{path}: treeFilterMode must be "default", "no-tools", "user-only", "labeled-only", "all"; ignored"#
                ),
                format!(
                    r#"{path}: defaultThinkingLevel must be "off", "minimal", "low", "medium", "high", "xhigh", "max"; ignored"#
                ),
                format!(r#"{path}: followUpMode must be "all" or "one-at-a-time"; ignored"#),
            ]
        );
    }

    #[test]
    fn a_reload_reads_the_file_afresh_and_keeps_the_settings_when_it_cannot() {
        let scratch = Scratch::new("reload");
        let mut settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": true, "tools": ["read"], "skills": ["x"], "followUpMode": "each"}"#,
        );
        assert_eq!(settings.warnings.len(), 1);
        let path = scratch.0.join("settings.json");
        // Keys taken out go back to their defaults, and the old warnings go.
        std::fs::write(
            &path,
            r#"{"commandSets": ["sets/mach12"], "steeringMode": 1}"#,
        )
        .unwrap();
        settings.reload();
        assert!(!settings.hide_thinking_block);
        assert_eq!(settings.tools, DEFAULT_TOOLS);
        assert!(settings.skills.is_empty());
        assert_eq!(settings.command_sets, [scratch.0.join("sets/mach12")]);
        let shown_path = shown(&path);
        assert_eq!(
            settings.warnings,
            [format!(
                r#"{shown_path}: steeringMode must be "all" or "one-at-a-time"; ignored"#
            )]
        );
        std::fs::write(&path, r#"{"commandSets": ["#).unwrap();
        settings.reload();
        assert_eq!(settings.command_sets, [scratch.0.join("sets/mach12")]);
        assert_eq!(settings.warnings.len(), 1);
        assert!(
            (settings.warnings[0].starts_with(&shown_path))
                && settings.warnings[0].ends_with("; kept as they were"),
            "{:?}",
            settings.warnings
        );
        // Changes are still saved to that file.
        assert_eq!(settings.path.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn enabled_models_are_patterns_saved_in_order_and_left_out_for_all() {
        let scratch = Scratch::new("enabled-models");
        let mut settings = settings_with(&scratch, r#"{"enabledModels": "sonnet"}"#);
        assert!(settings.enabled_models.is_empty());
        let path = scratch.0.join("settings.json");
        assert_eq!(
            settings.warnings,
            [format!(
                "{}: enabledModels must be a list of model patterns; ignored",
                shown(&path)
            )]
        );
        std::fs::write(
            &path,
            r#"{"theme": "x", "enabledModels": ["sonnet:high", "gpt-*"]}"#,
        )
        .unwrap();
        settings.reload();
        assert_eq!(settings.enabled_models, ["sonnet:high", "gpt-*"]);
        let saved = || -> Value { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };
        settings
            .set_enabled_models(vec!["b".into(), "a".into()])
            .unwrap();
        assert_eq!(saved(), json!({"theme": "x", "enabledModels": ["b", "a"]}));
        settings.set_enabled_models(Vec::new()).unwrap();
        assert_eq!(
            (saved(), settings.enabled_models.len()),
            (json!({"theme": "x"}), 0)
        );
    }

    #[test]
    fn saving_keeps_the_other_keys_and_a_broken_file_is_not_overwritten() {
        let scratch = Scratch::new("save");
        let path = scratch.0.join("config/scramjet-rs/settings.json");
        let mut settings = Settings::read(Some(path.clone()));
        settings.set_hide_thinking_block(true).unwrap();
        settings.set_default_thinking_level("none").unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved,
            json!({"hideThinkingBlock": true, "defaultThinkingLevel": "off"})
        );

        // Another process changed the file meanwhile: its keys stay.
        std::fs::write(&path, r#"{"theme": "x", "hideThinkingBlock": true}"#).unwrap();
        settings.set_default_thinking_level("high").unwrap();
        settings.set_hide_thinking_block(false).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved,
            json!({"theme": "x", "hideThinkingBlock": false, "defaultThinkingLevel": "high"})
        );
        let reread = Settings::read(Some(path.clone()));
        assert!(!reread.hide_thinking_block);
        assert_eq!(reread.default_thinking_level.as_deref(), Some("high"));

        std::fs::write(&path, "{\"hideThinkingBlock\": tru").unwrap();
        let mut broken = Settings::read(Some(path.clone()));
        assert_eq!(broken.warnings.len(), 1);
        assert!(
            broken.warnings[0].ends_with("; using the defaults"),
            "{:?}",
            broken.warnings
        );
        let error = broken.set_hide_thinking_block(true).unwrap_err();
        assert!(error.to_string().starts_with(&shown(&path)), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"hideThinkingBlock\": tru"
        );
        // Nothing is left beside it.
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn settings_rows_cycle_through_their_values_and_save_each() {
        let scratch = Scratch::new("rows");
        let mut settings = settings_with(&scratch, r#"{"branchSummary": {"reserveTokens": 1}}"#);
        let path = scratch.0.join("settings.json");
        let saved =
            || -> Value { serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap() };
        let seen = |row: Row, settings: &mut Settings| {
            let mut values = vec![settings.shown(row)];
            for _ in 0..3 {
                settings.cycle(row).unwrap();
                values.push(settings.shown(row));
            }
            values
        };
        assert_eq!(
            seen(Row::DoubleEscape, &mut settings),
            ["opens /tree", "opens /fork", "does nothing", "opens /tree"]
        );
        assert_eq!(
            seen(Row::Credentials, &mut settings),
            [
                "hidden when unattended",
                "always hidden",
                "never hidden",
                "hidden when unattended"
            ]
        );
        assert_eq!(
            seen(Row::Steering, &mut settings),
            [
                "all at once",
                "one at a time",
                "all at once",
                "one at a time"
            ]
        );
        assert_eq!(
            seen(Row::BranchSummary, &mut settings),
            [
                "asks for a summary",
                "no summary",
                "asks for a summary",
                "no summary"
            ]
        );
        assert_eq!(
            seen(Row::ThinkingBlocks, &mut settings),
            ["shown", "hidden", "shown", "hidden"]
        );
        assert_eq!(
            seen(Row::TreeFilter, &mut settings),
            [
                "all but model changes",
                "no tool results",
                "prompts only",
                "labeled only"
            ]
        );
        settings.cycle(Row::TreeFilter).unwrap();
        assert_eq!(settings.shown(Row::TreeFilter), "everything");
        settings.cycle(Row::FollowUp).unwrap();
        settings
            .set_tools(vec!["ls".into(), "read".into()])
            .unwrap();
        assert_eq!(settings.shown(Row::Tools), "read, ls");
        assert_eq!(
            saved(),
            json!({
                "branchSummary": {"reserveTokens": 1, "skipPrompt": true},
                "doubleEscapeAction": "tree",
                "hideCredentials": "unattended",
                "steeringMode": "one-at-a-time",
                "hideThinkingBlock": true,
                "treeFilterMode": "all",
                "followUpMode": "all",
                "tools": ["read", "ls"],
            })
        );
        let reread = Settings::read(Some(path.clone()));
        assert!(reread.warnings.is_empty(), "{:?}", reread.warnings);
        for row in ROWS {
            assert_eq!(reread.shown(row), settings.shown(row), "{row:?}");
        }
        settings.set_tools(Vec::new()).unwrap();
        assert_eq!(settings.shown(Row::Tools), "none");
    }

    #[test]
    fn command_sets_are_installed_and_removed_by_reference() {
        let scratch = Scratch::new("sets");
        let path = scratch.0.join("config/settings.json");
        let set = |name: &str| {
            let directory = scratch.0.join(name);
            std::fs::create_dir_all(directory.join("commands")).unwrap();
            directory
        };
        let (mach12, other) = (set("repo/mach12"), set("other/mach12"));
        let rs = set("rs");
        std::fs::create_dir_all(scratch.0.join("plain")).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"theme": "x", "commandSets": ["../rs"]}"#).unwrap();
        let mut settings = Settings::read(Some(path.clone()));
        assert_eq!(settings.command_sets, [scratch.0.join("config/../rs")]);

        let installed = settings.install_set(&mach12).unwrap();
        assert_eq!(
            (installed.name.as_str(), &installed.directory),
            ("mach12", &mach12)
        );
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let absolute = mach12.display().to_string();
        assert_eq!(
            saved,
            json!({"theme": "x", "commandSets": ["../rs", absolute]})
        );
        assert_eq!(
            Settings::read(Some(path.clone())).command_sets,
            settings.command_sets
        );
        assert_eq!(
            settings
                .install_set(&scratch.0.join("repo/./mach12"))
                .unwrap_err(),
            "mach12 is already installed"
        );
        assert_eq!(
            settings.install_set(&other).unwrap_err(),
            format!("a command set named mach12 is installed from {absolute}; remove it first")
        );
        let plain = scratch.0.join("plain");
        assert_eq!(
            settings.install_set(&plain).unwrap_err(),
            format!(
                "{} is not a command set: it has no commands or agents directory",
                plain.display()
            )
        );

        assert_eq!(
            settings.remove_set("rs").unwrap(),
            scratch.0.join("config/../rs")
        );
        assert_eq!(settings.command_sets, std::slice::from_ref(&mach12));
        let gone = rs.display().to_string();
        assert_eq!(
            settings.remove_set(&gone).unwrap_err(),
            format!("no installed command set is named {gone} or installed from there")
        );
        assert!(settings.remove_set(&other.display().to_string()).is_err());
        assert_eq!(settings.remove_set(&absolute).unwrap(), mach12);
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, json!({"theme": "x", "commandSets": []}));

        std::fs::write(&path, r#"{"commandSets": "../rs"}"#).unwrap();
        let mut wrong = Settings::read(Some(path.clone()));
        assert_eq!(
            wrong.warnings,
            [format!(
                "{}: commandSets must be a list of directories; ignored",
                shown(&path)
            )]
        );
        assert!(wrong.install_set(&rs).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"commandSets": "../rs"}"#
        );
    }

    #[test]
    fn saving_through_a_symbolic_link_writes_its_target() {
        let scratch = Scratch::new("link");
        let target = scratch.0.join("dotfiles.json");
        std::fs::write(&target, "{}").unwrap();
        let path = scratch.0.join("settings.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let mut settings = Settings::read(Some(path.clone()));
        settings.set_hide_thinking_block(true).unwrap();
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("\"hideThinkingBlock\": true")
        );
    }
}
