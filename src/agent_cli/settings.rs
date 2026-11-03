//! scramjet-rs's own settings file, `$XDG_CONFIG_HOME/scramjet-rs/settings.json` (by default
//! under `~/.config`), apart from Scramjet's, in its format and with its key names where the
//! behavior matches.
use scramjet_rs::agent::{CODING_TOOLS, DEFAULT_TOOLS, Origin, Set, search_program, shortened};
use serde_json::{Map, Value, json};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Scramjet's thinking levels; its `off` is the `none` of Proxy's model cards.
const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

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

#[derive(Debug)]
pub(super) struct Settings {
    /// `None` without `XDG_CONFIG_HOME` and `HOME`: nothing is read or saved.
    path: Option<PathBuf>,
    pub(super) hide_thinking_block: bool,
    pub(super) double_escape: DoubleEscape,
    /// `branchSummary.skipPrompt`: leaving a branch in `/tree` summarizes nothing, unasked.
    pub(super) skip_branch_summary_prompt: bool,
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
        let mut settings = Self {
            path,
            hide_thinking_block: false,
            double_escape: DoubleEscape::Tree,
            skip_branch_summary_prompt: false,
            default_thinking_level: None,
            tools: DEFAULT_TOOLS.map(str::to_owned).to_vec(),
            hide_credentials: HideCredentials::Unattended,
            steering_mode: Delivery::All,
            follow_up_mode: Delivery::OneAtATime,
            command_sets: Vec::new(),
            skills: Vec::new(),
            warnings: Vec::new(),
        };
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
        let level = if level == "none" { "off" } else { level };
        self.save("defaultThinkingLevel", json!(level))
    }

    /// Sets `key` in the file as it is now, keeping the rest of it; a file that cannot be read
    /// is left as it is.
    fn save(&self, key: &str, value: Value) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let mut object = read_object(path).map_err(io::Error::other)?;
        object.insert(key.to_owned(), value);
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

/// Writes `text` to a file beside `path`'s target and renames it over the target, so a reader
/// never sees part of it. A symbolic link stays one.
fn write_replacing(path: &Path, text: &str) -> io::Result<()> {
    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)?;
            }
            path.to_path_buf()
        }
        Err(error) => return Err(error),
    };
    let mut temporary = target.clone().into_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let written =
        std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, &target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
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
                "defaultThinkingLevel": "off", "theme": "dark",
                "steeringMode": "one-at-a-time", "followUpMode": "all"}"#,
        );
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
                "branchSummary": {"skipPrompt": 1}, "followUpMode": "each"}"#,
        );
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
                    r#"{path}: defaultThinkingLevel must be "off", "minimal", "low", "medium", "high", "xhigh", "max"; ignored"#
                ),
                format!(r#"{path}: followUpMode must be "all" or "one-at-a-time"; ignored"#),
            ]
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
