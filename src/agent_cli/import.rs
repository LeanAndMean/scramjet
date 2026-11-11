//! `scramjet import [--dry-run] [DIR]`: Scramjet 1's setup (`~/.scramjet/agent`, or where
//! `SCRAMJET_CODING_AGENT_DIR` puts it, or DIR) copied into the settings' directory, converted
//! where the formats differ, never over what is there already; and the offer of it, once, at
//! the first interactive start that finds one. Its sessions are not imported.
use super::packages::Source;
use super::settings::{read_object, write_object};
use scramjet::agent::{login, shortened};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub(super) const USAGE: &str = "Usage: scramjet import [--dry-run] [DIR]
Copies Scramjet 1's setup from DIR (by default ~/.scramjet/agent) into ~/.config/scramjet:
settings (merged, the keys set here kept), models.json, keybindings.json, ChatGPT's and GitHub
Copilot's logins, agents, themes, skills, SYSTEM.md, APPEND_SYSTEM.md, AGENTS.md and CLAUDE.md,
never over a file here, and lists what it leaves and why. --dry-run only lists.";

pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    (command == "import").then(|| import_command(arguments))
}

fn import_command(arguments: &[String]) -> Result<(), String> {
    let (mut dry, mut from) = (false, None);
    for argument in arguments {
        match argument.as_str() {
            "--dry-run" | "-n" => dry = true,
            "--help" | "-h" => {
                outln!("{USAGE}");
                return Ok(());
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option {option}\n{USAGE}"));
            }
            _ if from.is_some() => return Err(USAGE.to_owned()),
            directory => from = Some(PathBuf::from(directory)),
        }
    }
    let from = from
        .or_else(v1_directory)
        .ok_or("HOME is not set: give Scramjet 1's directory")?;
    let to = login::config_directory().ok_or("HOME is not set")?;
    if !from.is_dir() {
        return Err(format!("{} is not a directory", shown(&from)));
    }
    let report = import(&from, &to, dry)?;
    let (from, to) = (shown(&from), shown(&to));
    match (dry, report.taken.is_empty()) {
        (_, true) => outln!("Nothing to import from {from} into {to}."),
        (true, false) => outln!("Would import from {from} into {to}:"),
        (false, false) => outln!("Imported from {from} into {to}:"),
    }
    for line in &report.taken {
        outln!("  {line}");
    }
    if !report.left.is_empty() {
        outln!("Left out:");
        for line in &report.left {
            outln!("  {line}");
        }
    }
    if !dry && !report.taken.is_empty() {
        outln!("Scramjet 1 keeps its own copies; both can stay installed.");
    }
    Ok(())
}

/// Scramjet 1's directory: `SCRAMJET_CODING_AGENT_DIR` (a leading `~` expanded, as Scramjet 1
/// does), else `~/.scramjet/agent`.
fn v1_directory() -> Option<PathBuf> {
    let home = std::env::home_dir();
    match std::env::var("SCRAMJET_CODING_AGENT_DIR") {
        Ok(set) if !set.is_empty() => match scramjet::os::after_tilde(&set).zip(home) {
            Some((rest, home)) => Some(scramjet::os::join(&home, rest)),
            None => Some(PathBuf::from(set)),
        },
        _ => home.map(|home| home.join(".scramjet").join("agent")),
    }
}

/// What to tell the user at an interactive start, once: that Scramjet 1's setup can be
/// imported, when it has settings and these have none. Recorded as said in the state directory.
pub(super) fn offer() -> Option<String> {
    let said = scramjet::agent::state_directory()
        .ok()?
        .join("scramjet-1-import-offered");
    let ours = login::config_directory()?.join("settings.json");
    offer_in(&v1_directory()?, &ours, &said)
}

fn offer_in(theirs: &Path, ours: &Path, said: &Path) -> Option<String> {
    if !theirs.join("settings.json").is_file() || ours.exists() || said.exists() {
        return None;
    }
    let _ = std::fs::create_dir_all(said.parent()?);
    std::fs::write(said, "").ok()?;
    Some(format!(
        "Scramjet 1's setup is in {}: `scramjet import` copies its settings, models, logins, \
         agents, themes and skills here (`scramjet import --dry-run` lists what it would).",
        shown(theirs)
    ))
}

#[derive(Debug, Default, PartialEq)]
struct Report {
    /// What was carried over, or would be.
    taken: Vec<String>,
    /// What was not, each with why.
    left: Vec<String>,
}

/// Files copied as they are.
const AS_THEY_ARE: [&str; 5] = [
    "keybindings.json",
    "SYSTEM.md",
    "APPEND_SYSTEM.md",
    "AGENTS.md",
    "CLAUDE.md",
];

/// Scramjet 1's files and directories left behind, with why.
const LEFT: [(&str, &str); 4] = [
    (
        "extensions",
        "TypeScript extensions: Scramjet 2 has no extension API",
    ),
    (
        "prompts",
        "prompt templates: make them a command set's commands/",
    ),
    (
        "sessions",
        "Scramjet 1's session files cannot be read by Scramjet 2",
    ),
    ("git", "Scramjet 2 clones the settings' git packages again"),
];

fn import(from: &Path, to: &Path, dry: bool) -> Result<Report, String> {
    let mut report = Report::default();
    let made = (dry || to.is_dir())
        || scramjet::os::private_directory(std::fs::DirBuilder::new().recursive(true))
            .create(to)
            .is_ok();
    if !made {
        return Err(format!("{} could not be made", shown(to)));
    }
    settings(from, to, dry, &mut report)?;
    models(from, to, dry, &mut report)?;
    for name in AS_THEY_ARE {
        let source = from.join(name);
        if source.is_file() {
            copy_file(&source, &to.join(name), name, dry, &mut report);
        }
    }
    let (taken, left) = login::import(&from.join("auth.json"), &to.join("auth.json"), dry)?;
    if !taken.is_empty() {
        report.taken.push(format!(
            "auth.json: the logins to {}. A ChatGPT login renewed by one program stops working \
             in the other, which then needs /login again",
            taken.join(" and ")
        ));
    }
    report
        .left
        .extend(left.into_iter().map(|why| format!("auth.json: {why}")));
    agents(from, to, dry, &mut report);
    for name in ["themes", "skills"] {
        if from.join(name).is_dir() {
            copy_entries(&from.join(name), &to.join(name), name, dry, &mut report);
        }
    }
    for (name, why) in LEFT {
        if from.join(name).exists() {
            report.left.push(format!("{name}/: {why}"));
        }
    }
    Ok(report)
}

/// Scramjet 1's `settings.json` merged into this one's: its keys not set here, but those
/// Scramjet 2 reads differently, its paths made absolute (a relative one was relative to its
/// directory) and its npm packages left out.
fn settings(from: &Path, to: &Path, dry: bool, report: &mut Report) -> Result<(), String> {
    let source = from.join("settings.json");
    if !source.is_file() {
        return Ok(());
    }
    let theirs = read_object(&source)?;
    let target = to.join("settings.json");
    let mut ours = read_object(&target)?;
    let (converted, mut left) = converted_settings(theirs, from);
    let mut taken = Vec::new();
    for (key, value) in converted {
        match ours.contains_key(&key) {
            true if ours[&key] != value => left.push(format!("{key}: set here already")),
            true => {}
            false => {
                taken.push(key.clone());
                ours.insert(key, value);
            }
        }
    }
    if !taken.is_empty() {
        if !dry {
            write_object(&target, ours)?;
        }
        report
            .taken
            .push(format!("settings.json: {}", taken.join(", ")));
    }
    let left = left.into_iter().map(|why| format!("settings.json: {why}"));
    report.left.extend(left);
    Ok(())
}

/// Scramjet 1's settings as Scramjet 2 reads them, and what was left out, with why.
fn converted_settings(
    theirs: Map<String, Value>,
    from: &Path,
) -> (Map<String, Value>, Vec<String>) {
    let (mut converted, mut left) = (Map::new(), Vec::new());
    let absolute = |entry: &str| -> String {
        let trimmed = entry.trim();
        let kept = scramjet::os::after_tilde(trimmed).is_some() || Path::new(trimmed).is_absolute();
        // Pi's pattern prefixes, which Scramjet 2 does not read, are kept as written.
        match kept || trimmed.starts_with(['!', '+', '-']) {
            true => entry.to_owned(),
            false => shown(&scramjet::os::join(from, trimmed)),
        }
    };
    for (key, value) in theirs {
        match key.as_str() {
            "transport" => left.push("transport: not read by Scramjet 2".into()),
            "lastChangelogVersion" => {}
            "sessionDir" => left.push(
                "sessionDir: Scramjet 1's sessions are not Scramjet 2's; set it again if you want \
                 another directory"
                    .into(),
            ),
            "extensions" | "prompts" => left.push(format!(
                "{key}: Scramjet 2 has no TypeScript extensions or prompt templates"
            )),
            "packages" => {
                let Value::Array(entries) = value else {
                    converted.insert(key, value);
                    continue;
                };
                let mut kept = Vec::new();
                for entry in entries {
                    // Pi's filtered form, `{"source": …, "skills": […]}`.
                    let source = match &entry {
                        Value::String(text) => text.clone(),
                        Value::Object(object) => match object.get("source") {
                            Some(Value::String(text)) => {
                                left.push(format!("packages: {text}'s filters"));
                                text.clone()
                            }
                            _ => {
                                kept.push(entry);
                                continue;
                            }
                        },
                        _ => {
                            kept.push(entry);
                            continue;
                        }
                    };
                    match Source::parse(&source) {
                        Some(Err(why)) => left.push(format!("packages: {why}")),
                        Some(Ok(_)) => kept.push(Value::String(source)),
                        None => kept.push(Value::String(absolute(&source))),
                    }
                }
                converted.insert(key, Value::Array(kept));
            }
            "skills" | "themes" | "commandSets" => {
                let value = match value {
                    Value::Array(entries) => Value::Array(
                        (entries.into_iter())
                            .map(|entry| match entry {
                                Value::String(text) => Value::String(absolute(&text)),
                                other => other,
                            })
                            .collect(),
                    ),
                    other => other,
                };
                converted.insert(key, value);
            }
            _ => {
                converted.insert(key, value);
            }
        }
    }
    (converted, left)
}

/// Scramjet 1's `models.json` without the providers that only change one of its built-in
/// providers (no `baseUrl` or no `api`, on the entry or each of its models), which Scramjet 2
/// does not have.
fn models(from: &Path, to: &Path, dry: bool, report: &mut Report) -> Result<(), String> {
    let source = from.join("models.json");
    if !source.is_file() {
        return Ok(());
    }
    let target = to.join("models.json");
    if target.exists() {
        report
            .left
            .push("models.json: Scramjet 2 has its own already".into());
        return Ok(());
    }
    let mut theirs = read_object(&source)?;
    let Some(Value::Object(providers)) = theirs.get_mut("providers") else {
        report.left.push("models.json: no providers".into());
        return Ok(());
    };
    let complete = |entry: &Map<String, Value>, key: &str| {
        let models = entry.get("models").and_then(Value::as_array);
        entry.contains_key(key)
            || models.is_some_and(|models| {
                !models.is_empty() && models.iter().all(|model| model.get(key).is_some())
            })
    };
    let built_in: Vec<String> = (providers.iter())
        .filter(|(_, entry)| {
            entry
                .as_object()
                .is_none_or(|entry| !complete(entry, "baseUrl") || !complete(entry, "api"))
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in &built_in {
        providers.remove(name);
        report.left.push(format!(
            "models.json: {name}, which changes Scramjet 1's built-in provider; Scramjet 2 has \
             none: copy it with a baseUrl and an api (anthropic-messages, openai-responses or \
             openai-completions)"
        ));
    }
    if providers.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = providers.keys().cloned().collect();
    if !dry {
        write_object(&target, theirs)?;
    }
    report
        .taken
        .push(format!("models.json: {}", names.join(", ")));
    Ok(())
}

/// The user's own agents: not those named `<set>:<name>`, which come with their command set.
fn agents(from: &Path, to: &Path, dry: bool, report: &mut Report) {
    let Ok(entries) = std::fs::read_dir(from.join("agents")) else {
        return;
    };
    let mut names: Vec<String> = (entries.flatten())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();
    let (own, sets): (Vec<String>, Vec<String>) =
        names.into_iter().partition(|name| !name.contains(':'));
    for name in own {
        let path = format!("agents/{name}");
        copy_file(
            &from.join("agents").join(&name),
            &to.join("agents").join(&name),
            &path,
            dry,
            report,
        );
    }
    if !sets.is_empty() {
        report.left.push(format!(
            "agents/: {} agents of command sets ({}), which come with their set: install it \
             (scramjet install DIR|REPO)",
            sets.len(),
            set_names(&sets)
        ));
    }
}

fn set_names(agents: &[String]) -> String {
    let mut sets: Vec<&str> = (agents.iter())
        .filter_map(|name| name.split_once(':').map(|(set, _)| set))
        .collect();
    sets.dedup();
    sets.join(", ")
}

/// Each file and directory in `from` copied into `to`, links followed, but those there already.
fn copy_entries(from: &Path, to: &Path, name: &str, dry: bool, report: &mut Report) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.file_name()).collect();
    entries.sort();
    for entry in entries {
        let shown = format!("{name}/{}", entry.to_string_lossy());
        let (source, target) = (from.join(&entry), to.join(&entry));
        match source.is_dir() {
            true if target.exists() => report.left.push(format!("{shown}/: there already")),
            true => match dry || copy_tree(&source, &target, 0).is_ok() {
                true => report.taken.push(format!("{shown}/")),
                false => report.left.push(format!("{shown}/: could not be copied")),
            },
            false => copy_file(&source, &target, &shown, dry, report),
        }
    }
}

fn copy_file(source: &Path, target: &Path, shown: &str, dry: bool, report: &mut Report) {
    if target.exists() {
        let same = std::fs::read(source).ok().zip(std::fs::read(target).ok());
        if !same.is_some_and(|(theirs, ours)| theirs == ours) {
            report.left.push(format!("{shown}: there already"));
        }
        return;
    }
    let copied = dry
        || (std::fs::create_dir_all(target.parent().unwrap_or(Path::new(".")))
            .and_then(|()| std::fs::copy(source, target)))
        .is_ok();
    match copied {
        true => report.taken.push(shown.to_owned()),
        false => report.left.push(format!("{shown}: could not be copied")),
    }
}

/// A directory copied whole, links followed to a depth that a loop of them cannot pass.
fn copy_tree(from: &Path, to: &Path, depth: usize) -> std::io::Result<()> {
    if depth > 16 {
        return Err(std::io::Error::other("too deep"));
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        match source.is_dir() {
            true => copy_tree(&source, &target, depth + 1)?,
            false => drop(std::fs::copy(&source, &target)?),
        }
    }
    Ok(())
}

fn shown(path: &Path) -> String {
    shortened(&path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("scramjet-import-{name}-{}", std::process::id()));
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

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// A Scramjet 1 directory shaped as the user's: built-in providers changed, set agents
    /// beside their own, npm packages and the directories Scramjet 2 has no use for.
    fn v1(root: &Path) -> PathBuf {
        let v1 = root.join("v1");
        write(
            &v1.join("settings.json"),
            &json!({
                "defaultModel": "claude", "theme": "mine", "transport": "sse",
                "lastChangelogVersion": "0.117.0", "sessionDir": "sessions",
                "packages": ["npm:pi-thing", "git:github.com/u/r", "local/pkg",
                    {"source": "~/sets", "skills": ["a"]}],
                "skills": ["~/.claude/skills", "own"], "steeringMode": "all"
            })
            .to_string(),
        );
        write(
            &v1.join("models.json"),
            &json!({"providers": {
                "anthropic": {"baseUrl": "http://x", "modelOverrides": {}},
                "openai": {"apiKey": "K", "compat": {}},
                "local": {"baseUrl": "http://l/v1", "api": "openai-completions",
                    "models": [{"id": "m"}]},
                "mixed": {"models": [{"id": "a", "api": "anthropic-messages",
                    "baseUrl": "http://a"}]}
            }})
            .to_string(),
        );
        write(
            &v1.join("auth.json"),
            &json!({
                "github-copilot": {"type": "oauth", "refresh": "R", "access": "A", "expires": 1},
                "openai-codex": {"type": "oauth", "refresh": "R2", "access": "A2", "expires": 1},
                "openrouter": {"type": "api_key", "key": "K"},
                "anthropic": {"type": "oauth", "refresh": "R3"}
            })
            .to_string(),
        );
        write(&v1.join("keybindings.json"), "{}");
        write(&v1.join("AGENTS.md"), "own");
        write(&v1.join("agents/mine.md"), "agent");
        write(&v1.join("agents/mine.md.bak"), "old");
        // Windows' file names take no `:`, so Scramjet 1 had no set agents there.
        if cfg!(unix) {
            write(&v1.join("agents/mach12:code-reviewer.md"), "set");
            write(&v1.join("agents/mach12:code-explorer.md"), "set");
        }
        write(&v1.join("themes/t.json"), "{}");
        write(&v1.join("skills/s/SKILL.md"), "skill");
        for directory in ["extensions", "prompts", "sessions", "git"] {
            std::fs::create_dir_all(v1.join(directory)).unwrap();
        }
        v1
    }

    #[test]
    fn scramjet_1s_setup_is_converted_and_what_is_left_said() {
        let scratch = Scratch::new("whole");
        let (from, to) = (v1(&scratch.0), scratch.0.join("v2"));
        // As the first start leaves it: What's new's version and an empty credentials file.
        write(
            &to.join("settings.json"),
            r#"{"lastChangelogVersion": "2.144.0", "theme": "x"}"#,
        );
        write(&to.join("auth.json"), "{}\n");
        write(&to.join("AGENTS.md"), "ours");

        let dry = import(&from, &to, true).unwrap();
        assert_eq!(read(&to.join("settings.json"))["theme"], "x");
        assert!(!to.join("models.json").exists() && !to.join("agents").exists());
        let report = import(&from, &to, false).unwrap();
        assert_eq!(dry, report);

        let settings = read(&to.join("settings.json"));
        let home = |rest: &str| shown(&scramjet::os::join(&from, rest));
        assert_eq!(
            settings,
            json!({
                "lastChangelogVersion": "2.144.0", "theme": "x", "defaultModel": "claude",
                "packages": ["git:github.com/u/r", home("local/pkg"), "~/sets"],
                "skills": ["~/.claude/skills", home("own")], "steeringMode": "all"
            })
        );
        let models = read(&to.join("models.json"));
        let names: Vec<&String> = models["providers"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["local", "mixed"]);
        let auth = read(&to.join("auth.json"));
        let names: Vec<&String> = auth.as_object().unwrap().keys().collect();
        assert_eq!(names, ["github-copilot", "openai-codex"]);
        assert_eq!(auth["github-copilot"]["refresh"], "R");
        assert_eq!(
            std::fs::read_to_string(to.join("AGENTS.md")).unwrap(),
            "ours"
        );
        assert_eq!(
            std::fs::read_to_string(to.join("agents/mine.md")).unwrap(),
            "agent"
        );
        assert!(!to.join("agents/mine.md.bak").exists());
        assert!(!to.join("agents/mach12:code-reviewer.md").exists());
        assert_eq!(
            std::fs::read_to_string(to.join("skills/s/SKILL.md")).unwrap(),
            "skill"
        );
        assert!(to.join("themes/t.json").is_file());

        assert_eq!(
            report.taken,
            [
                "settings.json: defaultModel, packages, skills, steeringMode",
                "models.json: local, mixed",
                "keybindings.json",
                "auth.json: the logins to GitHub Copilot and ChatGPT Plus/Pro (Codex \
                 Subscription). A ChatGPT login renewed by one program stops working in the \
                 other, which then needs /login again",
                "agents/mine.md",
                "themes/t.json",
                "skills/s/",
            ]
        );
        let left = report.left.join("\n");
        for said in [
            "settings.json: theme: set here already",
            "settings.json: transport: not read by Scramjet 2",
            "settings.json: sessionDir:",
            "settings.json: packages: npm:pi-thing: an npm package",
            "settings.json: packages: ~/sets's filters",
            "models.json: anthropic, which changes",
            "models.json: openai, which changes",
            "AGENTS.md: there already",
            "auth.json: openrouter's API key:",
            "auth.json: anthropic: not a login Scramjet 2 makes",
            match cfg!(unix) {
                true => "agents/: 2 agents of command sets (mach12)",
                false => "extensions/:",
            },
            "extensions/:",
            "prompts/:",
            "sessions/:",
            "git/:",
        ] {
            assert!(left.contains(said), "{said:?} not in\n{left}");
        }

        // Again: nothing more to take, and nothing changed.
        let again = import(&from, &to, false).unwrap();
        assert!(again.taken.is_empty(), "{:?}", again.taken);
        assert!(
            again
                .left
                .join("\n")
                .contains("auth.json: GitHub Copilot: logged in here already")
        );
        assert_eq!(read(&to.join("settings.json")), settings);
    }

    #[test]
    fn the_import_is_offered_once_when_scramjet_1_has_settings_and_these_none() {
        let scratch = Scratch::new("offer");
        let (theirs, ours) = (scratch.0.join("v1"), scratch.0.join("v2/settings.json"));
        let said = scratch.0.join("state/offered");
        assert_eq!(offer_in(&theirs, &ours, &said), None);
        write(&theirs.join("settings.json"), "{}");
        write(&ours, "{}");
        assert_eq!(offer_in(&theirs, &ours, &said), None);
        std::fs::remove_file(&ours).unwrap();
        let offer = offer_in(&theirs, &ours, &said).unwrap();
        assert!(offer.contains("`scramjet import`"), "{offer}");
        assert_eq!(offer_in(&theirs, &ours, &said), None);
    }
}
