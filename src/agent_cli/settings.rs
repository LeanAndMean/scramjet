//! scramjet-rs's own settings file, `$XDG_CONFIG_HOME/scramjet-rs/settings.json` (by default
//! under `~/.config`), apart from Scramjet's, in its format and with its key names where the
//! behavior matches.
use scramjet_rs::agent::shortened;
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
        let text = serde_json::to_string_pretty(&Value::Object(object))? + "\n";
        write_replacing(path, &text)
            .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", shown(path))))
    }
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
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
    }

    #[test]
    fn reads_scramjets_keys() {
        let scratch = Scratch::new("keys");
        let settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": true, "doubleEscapeAction": "fork",
                "branchSummary": {"reserveTokens": 1, "skipPrompt": true},
                "defaultThinkingLevel": "off", "theme": "dark"}"#,
        );
        assert!(settings.hide_thinking_block);
        assert_eq!(settings.double_escape, DoubleEscape::Fork);
        assert!(settings.skip_branch_summary_prompt);
        assert_eq!(settings.default_thinking_level.as_deref(), Some("none"));
        assert!(settings.warnings.is_empty(), "{:?}", settings.warnings);
        let none = settings_with(&scratch, r#"{"doubleEscapeAction": "none"}"#);
        assert_eq!(none.double_escape, DoubleEscape::Nothing);
    }

    #[test]
    fn a_wrong_value_is_named_and_keeps_its_default() {
        let scratch = Scratch::new("wrong");
        let settings = settings_with(
            &scratch,
            r#"{"hideThinkingBlock": "yes", "doubleEscapeAction": "tree", "defaultThinkingLevel": "hgih",
                "branchSummary": {"skipPrompt": 1}}"#,
        );
        assert!(!settings.hide_thinking_block);
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
