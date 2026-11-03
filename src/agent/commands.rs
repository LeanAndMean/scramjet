//! Command sets: directories with `commands/<set>:<command>.md` and `agents/<set>:<agent>.md`,
//! installed by reference or found under Scramjet's global root and the project's `.scramjet`
//! directory. A prompt `/<set>:<command> args` runs one of their commands as Scramjet sends it:
//! the arguments are substituted into the body and the result is wrapped in `<scramjet-command>`.

use std::{
    fs,
    path::{Path, PathBuf},
};
use yaml_rust2::{Yaml, YamlLoader};

/// Where a command set was found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Origin {
    /// Installed by reference (`scramjet-rs install`), listed in the settings.
    Installed,
    /// Under Scramjet's global root.
    Global,
    /// Under the project's `.scramjet` directory.
    Project,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

/// A command set: its commands are named `<name>:…`, and so are its agents.
#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    /// The directory's name, as Scramjet names a set.
    pub name: String,
    pub directory: PathBuf,
    pub origin: Origin,
}

impl Set {
    /// The set in `directory`, or why it is none: it must have a `commands` or an `agents`
    /// directory.
    pub fn at(directory: &Path, origin: Origin) -> Result<Self, String> {
        let shown = directory.display();
        if !directory.is_dir() {
            return Err(match directory.exists() {
                true => format!("{shown} is not a directory"),
                false => format!("{shown} does not exist"),
            });
        }
        if !directory.join("commands").is_dir() && !directory.join("agents").is_dir() {
            return Err(format!(
                "{shown} is not a command set: it has no commands or agents directory"
            ));
        }
        let name = directory.file_name().and_then(|name| name.to_str());
        let name = name.ok_or_else(|| format!("{shown} has no usable name"))?;
        Ok(Self {
            name: name.to_owned(),
            directory: directory.to_path_buf(),
            origin,
        })
    }
}

/// The command sets of a session, in the order their commands and agents are registered.
#[derive(Clone, Debug, Default)]
pub struct Sets {
    list: Vec<Set>,
    warnings: Vec<String>,
}

impl Sets {
    /// The `installed` sets in their order, then the sets under `global` and under `project`
    /// (the project's `.scramjet` directory), each by name; other directories there are not sets.
    /// A set under a root whose name an installed set has is skipped with a warning, as is an
    /// installed one that is missing.
    pub fn find(installed: &[PathBuf], global: Option<&Path>, project: &Path) -> Self {
        let mut sets = Self::default();
        for directory in installed {
            match Set::at(directory, Origin::Installed) {
                Ok(set) => match sets.list.iter().find(|known| known.name == set.name) {
                    Some(known) => sets.warnings.push(format!(
                        "skipping installed command set {}: {} is installed from {}",
                        directory.display(),
                        set.name,
                        known.directory.display()
                    )),
                    None => sets.list.push(set),
                },
                Err(error) => sets
                    .warnings
                    .push(format!("skipping installed command set: {error}")),
            }
        }
        let installed = sets.list.len();
        let roots = global.map(|global| (global, Origin::Global));
        for (root, origin) in roots.into_iter().chain([(project, Origin::Project)]) {
            for name in set_directories(root) {
                let Ok(set) = Set::at(&root.join(name), origin) else {
                    continue;
                };
                match sets.list[..installed]
                    .iter()
                    .find(|known| known.name == set.name)
                {
                    Some(known) => sets.warnings.push(format!(
                        "skipping {} command set {}: {} is installed from {}",
                        origin.name(),
                        set.directory.display(),
                        set.name,
                        known.directory.display()
                    )),
                    None => sets.list.push(set),
                }
            }
        }
        sets
    }

    /// The sets of a session in `root`: the `installed` ones, then those under Scramjet's global
    /// root and the project's `.scramjet` directory.
    pub fn for_project(installed: &[PathBuf], root: &Path) -> Self {
        Self::find(installed, global_root().as_deref(), &root.join(".scramjet"))
    }

    pub fn list(&self) -> &[Set] {
        &self.list
    }

    /// Why sets were skipped.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The set whose namespace `name` (`<set>:…`) is in.
    pub(super) fn owner(&self, name: &str) -> Option<&Set> {
        let (prefix, _) = name.split_once(':')?;
        self.list.iter().find(|set| set.name == prefix)
    }
}

/// A command a prompt can run.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// `<set>:<command>`, which is also what the prompt names.
    pub name: String,
    pub path: PathBuf,
    /// The frontmatter's, else the body's first line, as Scramjet's autocomplete shows it.
    pub description: String,
    /// Whether `description` is the frontmatter's.
    described: bool,
    pub argument_hint: Option<String>,
    /// Meant to be run by another command; Scramjet neither lists it for the model nor runs it
    /// as a next step, and the `/` list leaves it out, though typed in full it runs.
    pub delegate_only: bool,
    body: String,
    /// What may follow it, from the frontmatter's `next:` block.
    pub(super) next: Option<Policy>,
}

/// A command's `next:` policy, as Scramjet's `parseNextStepPolicy` reads it: which commands the
/// model may offer as next steps when it reports the command completed.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Policy {
    /// `target` runs after the command completes.
    Forced { target: String },
    /// Only these commands may follow.
    Closed { candidates: Vec<Candidate> },
    /// Any command may follow except the blacklisted ones; the candidates are suggestions.
    Open {
        candidates: Vec<Candidate>,
        blacklist: Vec<String>,
    },
    /// The user chooses; the model offers nothing.
    Ask { hint: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Candidate {
    pub name: String,
    pub hint: Option<String>,
}

/// The commands of a session's sets, by name.
#[derive(Clone, Debug, Default)]
pub struct Commands {
    list: Vec<Command>,
    warnings: Vec<String>,
}

/// Scramjet's global root for command sets: `$SCRAMJET_CACHE`, else `$XDG_DATA_HOME/scramjet`,
/// else `~/.local/share/scramjet`.
fn global_root() -> Option<PathBuf> {
    if let Some(cache) = std::env::var_os("SCRAMJET_CACHE") {
        return Some(PathBuf::from(cache));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".local/share")))?;
    Some(data.join("scramjet"))
}

impl Commands {
    /// The commands of `sets`, with their warnings first. The first file registered under a
    /// name wins, so a global command shadows a project one of the same name, as in Scramjet.
    /// Files that are not `<set>:` commands, or whose frontmatter Scramjet would reject, are
    /// skipped with a warning.
    pub fn discover(sets: &Sets) -> Self {
        let mut found = Self {
            list: Vec::new(),
            warnings: sets.warnings.clone(),
        };
        for set in &sets.list {
            let directory = set.directory.join("commands");
            for (file, content) in command_files(&directory) {
                let path = directory.join(&file);
                let command = match parse(&set.name, &file, &content, &mut found.warnings) {
                    Ok(command) => Command { path, ..command },
                    Err(error) => {
                        let path = path.display();
                        found.warnings.push(format!("skipping {path}: {error}"));
                        continue;
                    }
                };
                match found.list.iter().find(|known| known.name == command.name) {
                    Some(known) => found.warnings.push(format!(
                        "skipping {} command {} at {}: name already registered from {}",
                        set.origin.name(),
                        command.name,
                        command.path.display(),
                        known.path.display()
                    )),
                    None => found.list.push(command),
                }
            }
        }
        found.list.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }

    pub fn list(&self) -> &[Command] {
        &self.list
    }

    /// Why command files were skipped, as Scramjet warns about them.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Scramjet's `# Available commands` section of the instructions, empty without commands.
    pub(super) fn catalog(&self) -> String {
        let lines: Vec<String> = self
            .list
            .iter()
            .filter(|command| !command.delegate_only)
            .map(|command| {
                let prefix = match &command.argument_hint {
                    Some(hint) => format!("/{} {hint}", command.name),
                    None => format!("/{}", command.name),
                };
                match command.described {
                    true => format!("- {prefix}: {}", command.description),
                    false => format!("- {prefix}"),
                }
            })
            .collect();
        if lines.is_empty() {
            return String::new();
        }
        format!(
            "# Available commands\n\nThe following slash commands can be invoked directly:\n\n{}",
            lines.join("\n")
        )
    }

    /// Commands from `(file, content)` pairs, each in the set its name starts with.
    #[cfg(test)]
    pub(super) fn parsed(files: &[(&str, &str)]) -> Self {
        let list = files.iter().map(|(file, content)| {
            let set = file.split(':').next().unwrap();
            parse(set, file, content, &mut Vec::new()).unwrap()
        });
        Self {
            list: list.collect(),
            warnings: Vec::new(),
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<&Command> {
        self.list.iter().find(|command| command.name == name)
    }

    /// The command a prompt runs: `/<name> args` names it.
    pub(super) fn named(&self, prompt: &str) -> Option<&Command> {
        let rest = prompt.strip_prefix('/')?;
        let name = rest.split(char::is_whitespace).next().unwrap_or(rest);
        self.get(name)
    }

    /// The text a prompt sends when it runs a command, `None` when it runs none: `/<name> args`
    /// names a command, whose body takes the arguments split as Scramjet splits them.
    pub fn expand(&self, prompt: &str) -> Option<String> {
        let command = self.named(prompt)?;
        let rest = &prompt[1..];
        let args = rest
            .split_once(char::is_whitespace)
            .map_or("", |(_, args)| args);
        let body = substitute(&command.body, &split_args(args));
        if body.starts_with("<scramjet-command") {
            return Some(body);
        }
        Some(format!(
            "<scramjet-command name=\"{}\">\n{body}\n</scramjet-command>",
            command.name
        ))
    }
}

/// The directories under `root`, by name. Links count as what they point to.
fn set_directories(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The `.md` files in `directory` with their contents, by file name. Unreadable files are skipped.
fn command_files(directory: &Path) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file = entry.file_name().into_string().ok()?;
            let path = entry.path();
            (file.ends_with(".md") && path.is_file()).then_some((file, path))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|(file, path)| Some((file, fs::read_to_string(path).ok()?)))
        .collect()
}

/// The command in `content`, the file `file` of set `set`, with Scramjet's notices about fields
/// it ignores added to `notices`; otherwise why Scramjet's loader rejects it, in its words.
fn parse(
    set: &str,
    file: &str,
    content: &str,
    notices: &mut Vec<String>,
) -> Result<Command, String> {
    let name = file.strip_suffix(".md").unwrap_or(file);
    if !name.starts_with(&format!("{set}:")) {
        return Err(format!("{file}: filename must start with \"{set}:\""));
    }
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (frontmatter, body) = split_frontmatter(content);
    let frontmatter = YamlLoader::load_from_str(&frontmatter)
        .map_err(|error| format!("{file}: malformed frontmatter — {error}"))?
        .into_iter()
        .next()
        .unwrap_or(Yaml::Null);
    let next = Policy::parse(&frontmatter["next"])
        .map_err(|error| format!("{file}: invalid next block — {error}"))?;
    let field = |key: &str| {
        frontmatter[key]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    let described = field("description");
    let description = described.clone().unwrap_or_else(|| {
        let first = body.lines().find(|line| !line.trim().is_empty());
        let first = first.unwrap_or_default();
        match first.char_indices().nth(60) {
            Some((at, _)) => format!("{}...", &first[..at]),
            None => first.to_owned(),
        }
    });
    let delegate_only = match &frontmatter["delegate-only"] {
        Yaml::BadValue => false,
        Yaml::Boolean(true) => true,
        other => {
            notices.push(format!(
                "{file}: \"delegate-only\" must be exactly true when present (got {}; treated as \
 absent)",
                json(other)
            ));
            false
        }
    };
    Ok(Command {
        name: name.to_owned(),
        path: PathBuf::from(file),
        description,
        described: described.is_some(),
        argument_hint: field("argument-hint").map(|hint| hint.trim().to_owned()),
        delegate_only,
        body,
        next,
    })
}

/// A YAML value as JavaScript's `JSON.stringify` writes it.
fn json(value: &Yaml) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Yaml::String(text) => Value::String(text.clone()),
        Yaml::Integer(number) => Value::from(*number),
        Yaml::Real(text) => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Yaml::Boolean(value) => Value::Bool(*value),
        Yaml::Array(items) => items.iter().map(json).collect(),
        Yaml::Hash(entries) => entries
            .iter()
            .map(|(key, value)| (super::autonomy::yaml_key(key), json(value)))
            .collect(),
        _ => Value::Null,
    }
}

impl Policy {
    /// The policy of a `next:` value; `None` when there is none.
    fn parse(raw: &Yaml) -> Result<Option<Self>, String> {
        match raw {
            Yaml::BadValue | Yaml::Null => return Ok(None),
            Yaml::Hash(_) => {}
            _ => return Err("next must be a mapping".into()),
        }
        let mode = raw["mode"].as_str().ok_or("next.mode must be a string")?;
        let policy = match mode {
            "forced" => {
                let target = non_blank(&raw["target"])
                    .ok_or("next.target must be a non-empty string for mode: forced")?;
                Self::Forced { target }
            }
            "closed" => {
                let candidates = candidates(&raw["candidates"]).map_err(|e| format!("next.{e}"))?;
                if candidates.is_empty() {
                    return Err("next.candidates must not be empty for mode: closed".into());
                }
                Self::Closed { candidates }
            }
            "open" => {
                let candidates = candidates(&raw["candidates"]).map_err(|e| format!("next.{e}"))?;
                let blacklist = match &raw["blacklist"] {
                    Yaml::BadValue => Vec::new(),
                    Yaml::Array(entries) => entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| {
                            non_blank(entry).ok_or_else(|| {
                                format!("next.blacklist[{index}] must be a non-empty string")
                            })
                        })
                        .collect::<Result<_, _>>()?,
                    _ => return Err("next.blacklist must be a list".into()),
                };
                Self::Open {
                    candidates,
                    blacklist,
                }
            }
            "ask" => Self::Ask {
                hint: optional_text(&raw["hint"]).map_err(|e| format!("next.{e}"))?,
            },
            other => return Err(format!("unknown next.mode: {other}")),
        };
        Ok(Some(policy))
    }
}

fn candidates(raw: &Yaml) -> Result<Vec<Candidate>, String> {
    let Yaml::Array(entries) = raw else {
        return Err("candidates must be a list".into());
    };
    let mut candidates = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if !matches!(entry, Yaml::Hash(_)) {
            return Err(format!("candidates[{index}] must be a mapping"));
        }
        let name = non_blank(&entry["name"])
            .ok_or_else(|| format!("candidates[{index}].name must be a non-empty string"))?;
        let hint = optional_text(&entry["hint"]).map_err(|e| format!("candidates[{index}].{e}"))?;
        candidates.push(Candidate { name, hint });
    }
    Ok(candidates)
}

/// A string with more than blanks in it.
fn non_blank(value: &Yaml) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

/// A `hint`: absent, or a string with more than blanks in it.
fn optional_text(value: &Yaml) -> Result<Option<String>, String> {
    match value {
        Yaml::BadValue => Ok(None),
        Yaml::String(text) if text.trim().is_empty() => {
            Err("hint must be a non-empty string when set".into())
        }
        Yaml::String(text) => Ok(Some(text.clone())),
        _ => Err("hint must be a string when set".into()),
    }
}

/// The frontmatter and the body after it, trimmed, as Scramjet's `parseFrontmatter` splits
/// them: the text must start with `---` and the frontmatter ends at the next line starting with
/// `---`. Otherwise the whole text is the body, as it is.
pub(super) fn split_frontmatter(content: &str) -> (String, String) {
    let content = content.replace("\r\n", "\n").replace('\r', "\n");
    match content.get(3..).and_then(|rest| rest.find("\n---")) {
        Some(end) if content.starts_with("---") => {
            let end = end + 3;
            let frontmatter = content.get(4..end).unwrap_or_default().to_owned();
            (frontmatter, content[end + 4..].trim().to_owned())
        }
        _ => (String::new(), content),
    }
}

/// The arguments of a command as Scramjet splits them: at spaces and tabs, where quotes group
/// words and are removed.
fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in text.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => current.push(character),
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == ' ' || character == '\t' => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            None => current.push(character),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Scramjet's substitution: `$1`… first, then `${@:N}` and `${@:N:L}`, then `$ARGUMENTS` and `$@`.
/// Each pass runs over the whole text, as Scramjet's does.
fn substitute(body: &str, args: &[String]) -> String {
    let all = args.join(" ");
    let positional = replace_positional(body, args);
    let sliced = replace_slices(&positional, args);
    sliced.replace("$ARGUMENTS", &all).replace("$@", &all)
}

/// `$N` for the N-th argument (1-based); `$0` and arguments beyond the last are empty.
fn replace_positional(text: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            out.push('$');
            rest = after;
            continue;
        }
        let index: usize = after[..digits].parse().unwrap_or(0);
        if let Some(arg) = index.checked_sub(1).and_then(|index| args.get(index)) {
            out.push_str(arg);
        }
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// `${@:N}` and `${@:N:L}`, bash-style: from the N-th argument (1-based, 0 meaning the first)
/// on, or L of them from there.
fn replace_slices(text: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("${@:") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "${@:".len()..];
        match slice_spec(after) {
            Some((start, length, tail)) => {
                let end = length.map_or(args.len(), |length| start.saturating_add(length));
                let from = start.min(args.len());
                let to = end.min(args.len());
                out.push_str(&args[from..to].join(" "));
                rest = tail;
            }
            None => {
                out.push_str("${@:");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `N` or `N:L` of a `${@:` slice and the text after its `}`; `start` is 0-based.
fn slice_spec(text: &str) -> Option<(usize, Option<usize>, &str)> {
    let (first, tail) = split_number(text)?;
    let (length, tail) = match tail.strip_prefix(':') {
        Some(after) => {
            let (length, tail) = split_number(after)?;
            (Some(length), tail)
        }
        None => (None, tail),
    };
    let tail = tail.strip_prefix('}')?;
    Some((first.saturating_sub(1), length, tail))
}

/// The decimal number at the start of `text` and the rest after it.
fn split_number(text: &str) -> Option<(usize, &str)> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let number = text[..digits].parse().unwrap_or(usize::MAX);
    Some((number, &text[digits..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory removed when dropped, also when a test panics.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("scramjet-commands-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self, path: &str, content: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(commands: &Commands) -> Vec<&str> {
        commands.list().iter().map(|c| c.name.as_str()).collect()
    }

    /// The sets of `project` alone, as a session without installed sets or a global root.
    fn project_sets(project: &Path) -> Sets {
        Sets::find(&[], None, project)
    }

    #[test]
    fn sets_are_the_installed_ones_then_the_global_and_project_ones() {
        let scratch = Scratch::new("sets");
        let (global, project) = (scratch.0.join("global"), scratch.0.join("project"));
        scratch.file("installed/mach12/commands/mach12:x.md", "Installed\n");
        scratch.file("installed/agents-only/agents/agents-only:a.md", "");
        scratch.file("elsewhere/mach12/commands/mach12:x.md", "Elsewhere\n");
        scratch.file("installed/empty/notes.txt", "");
        scratch.file("global/mach12/commands/mach12:x.md", "Global\n");
        scratch.file("global/rs/commands/rs:a.md", "A\n");
        scratch.file("global/plain/notes.md", "Not a set\n");
        scratch.file("project/rs/commands/rs:b.md", "B\n");
        scratch.file("project/agents/reviewer.md", "A loose agent\n");
        let installed = [
            "mach12",
            "agents-only",
            "../elsewhere/mach12",
            "missing",
            "empty",
        ]
        .map(|name| scratch.0.join("installed").join(name));
        let sets = Sets::find(&installed, Some(&global), &project);
        let found: Vec<String> = (sets.list().iter())
            .map(|set| format!("{} {}", set.name, set.origin.name()))
            .collect();
        assert_eq!(
            found,
            [
                "mach12 installed",
                "agents-only installed",
                "rs global",
                "rs project"
            ]
        );
        let shown = |path: &str| scratch.0.join(path).display().to_string();
        assert_eq!(
            sets.warnings(),
            [
                format!(
                    "skipping installed command set {}: mach12 is installed from {}",
                    shown("installed/../elsewhere/mach12"),
                    shown("installed/mach12")
                ),
                format!(
                    "skipping installed command set: {} does not exist",
                    shown("installed/missing")
                ),
                format!(
                    "skipping installed command set: {} is not a command set: it has no commands \
or agents directory",
                    shown("installed/empty")
                ),
                format!(
                    "skipping global command set {}: mach12 is installed from {}",
                    shown("global/mach12"),
                    shown("installed/mach12")
                ),
            ]
        );
        assert_eq!(
            sets.owner("mach12:code-reviewer").unwrap().origin,
            Origin::Installed
        );
        assert_eq!(sets.owner("rs:x").unwrap().origin, Origin::Global);
        assert_eq!(sets.owner("other:x"), None);
        assert_eq!(sets.owner("reviewer"), None);
        let commands = Commands::discover(&sets);
        assert_eq!(names(&commands), ["mach12:x", "rs:a", "rs:b"]);
        assert_eq!(&commands.warnings()[..4], sets.warnings());
        assert_eq!(
            commands.expand("/mach12:x"),
            Some("<scramjet-command name=\"mach12:x\">\nInstalled\n\n</scramjet-command>".into())
        );
    }

    #[test]
    fn discovery_takes_global_then_project_and_skips_what_scramjet_skips() {
        let scratch = Scratch::new("discover");
        let global = scratch.0.join("global");
        let project = scratch.0.join("project");
        scratch.file(
            "global/rs/commands/rs:review.md",
            "---\ndescription: Global review\nargument-hint: \"[commits or area]\"\nnext:\n  mode: ask\n  hint: nested\n---\nGlobal $1\n",
        );
        scratch.file(
            "global/rs/commands/rs:helper.md",
            "---\ndelegate-only: true\n---\n\nHelp with <caller-context>$ARGUMENTS</caller-context>\n",
        );
        scratch.file(
            "global/rs/commands/rs:odd.md",
            "---\ndescription: Odd\nargument-hint: \"  <x>  \"\ndelegate-only: \"yes\"\n---\nOdd\n",
        );
        scratch.file("global/rs/commands/notes.md", "---\n---\nNo prefix\n");
        scratch.file("global/rs/commands/rs:notes.txt", "not markdown\n");
        scratch.file("global/rs/commands/rs:folder.md/inner.txt", "not a file\n");
        scratch.file("global/other/commands/rs:wrong-set.md", "Wrong set\n");
        scratch.file(
            "project/rs/commands/rs:review.md",
            "---\ndescription: Project review\n---\nProject\n",
        );
        scratch.file("project/rs/commands/rs:loop.md", "Loop $ARGUMENTS\n");

        let commands = Commands::discover(&Sets::find(&[], Some(&global), &project));
        assert_eq!(
            names(&commands),
            ["rs:helper", "rs:loop", "rs:odd", "rs:review"]
        );
        let review = &commands.list()[3];
        assert_eq!(review.description, "Global review");
        assert_eq!(review.path, global.join("rs/commands/rs:review.md"));
        assert_eq!(review.argument_hint.as_deref(), Some("[commits or area]"));
        let hint = Some("nested".to_owned());
        assert_eq!(review.next, Some(Policy::Ask { hint }));
        let skipped = |file: &str, set: &str| {
            let path = global.join(format!("{set}/commands/{file}"));
            let error = format!("{file}: filename must start with \"{set}:\"");
            format!("skipping {}: {error}", path.display())
        };
        assert_eq!(
            commands.warnings(),
            [
                skipped("rs:wrong-set.md", "other"),
                skipped("notes.md", "rs"),
                "rs:odd.md: \"delegate-only\" must be exactly true when present (got \"yes\"; \
 treated as absent)"
                    .into(),
                format!(
                    "skipping project command rs:review at {}: name already registered from {}",
                    project.join("rs/commands/rs:review.md").display(),
                    review.path.display()
                ),
            ]
        );
        // Scramjet's catalog: frontmatter descriptions only, without delegate-only commands.
        assert_eq!(
            commands.catalog(),
            "# Available commands\n\nThe following slash commands can be invoked directly:\n\n\
- /rs:loop\n- /rs:odd <x>: Odd\n- /rs:review [commits or area]: Global review"
        );
        assert_eq!(Commands::default().catalog(), "");
        assert_eq!(
            commands.expand("/rs:review HEAD~2.."),
            Some(
                "<scramjet-command name=\"rs:review\">\nGlobal HEAD~2..\n</scramjet-command>"
                    .into()
            )
        );
        // A delegate-only command runs when typed in full, as in Scramjet.
        assert_eq!(
            commands.expand("/rs:helper a b"),
            Some(
                "<scramjet-command name=\"rs:helper\">\nHelp with <caller-context>a b</caller-context>\n</scramjet-command>"
                    .into()
            )
        );
        assert_eq!(commands.expand("/rs:notes"), None);
    }

    #[test]
    fn a_project_command_is_found_without_a_global_root() {
        let scratch = Scratch::new("no-global");
        scratch.file("project/rs/commands/rs:only.md", "Only\n");
        let commands = Commands::discover(&project_sets(&scratch.0.join("project")));
        assert_eq!(names(&commands), ["rs:only"]);
        assert_eq!(
            commands.expand("/rs:only"),
            Some("<scramjet-command name=\"rs:only\">\nOnly\n\n</scramjet-command>".into())
        );
    }

    #[test]
    fn frontmatter_is_read_as_scramjet_reads_it() {
        let crlf = "\u{feff}---\r\ndescription: \"Quoted\"\r\nargument-hint: '<a> [b]'\r\n---\r\n\r\n  Body\r\nmore  \r\n";
        let command = parse("rs", "rs:x.md", crlf, &mut Vec::new()).unwrap();
        assert_eq!(command.description, "Quoted");
        assert_eq!(command.next, None);
        assert_eq!(command.argument_hint.as_deref(), Some("<a> [b]"));
        assert_eq!(command.body, "Body\nmore");
        // Without a closing line, the whole text is the body, untrimmed.
        let open = parse(
            "rs",
            "rs:x.md",
            "---\ndescription: no end\n",
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(open.argument_hint, None);
        assert_eq!(open.body, "---\ndescription: no end\n");
        assert_eq!(open.description, "---");
        // Without a description, the body's first line, cut at 60 characters.
        let long = format!("\n\n{}é tail\nsecond\n", "x".repeat(59));
        let command = parse("rs", "rs:x.md", &long, &mut Vec::new()).unwrap();
        assert_eq!(command.description, format!("{}é...", "x".repeat(59)));
        assert!(parse("rs", "other.md", "Body", &mut Vec::new()).is_err());
        // As YAML, a folded description is one line, and a non-string one is not a description.
        let folded = parse(
            "rs",
            "rs:x.md",
            "---\ndescription: >\n  Two\n  lines\n---\nB",
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(folded.description, "Two lines\n");
        let number = parse(
            "rs",
            "rs:x.md",
            "---\ndescription: 12\n---\nBody line",
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(number.description, "Body line");
        assert_eq!(
            parse(
                "rs",
                "rs:x.md",
                "---\ndescription: a: b\n---\nB",
                &mut Vec::new()
            )
            .unwrap_err(),
            "rs:x.md: malformed frontmatter — mapping values are not allowed in this context \
at byte 14 line 1 column 15"
        );
    }

    fn policy(next: &str) -> Result<Option<Policy>, String> {
        let content = format!("---\nnext:\n{next}\n---\nBody");
        parse("rs", "rs:x.md", &content, &mut Vec::new()).map(|command| command.next)
    }

    #[test]
    fn next_policies_are_read_and_checked_as_scramjet_does() {
        let candidate = |name: &str, hint: Option<&str>| Candidate {
            name: name.into(),
            hint: hint.map(str::to_owned),
        };
        assert_eq!(
            policy(
                "  mode: closed\n  candidates:\n    - name: rs:loop\n      hint: Again.\n    - name: rs:steer"
            ),
            Ok(Some(Policy::Closed {
                candidates: vec![
                    candidate("rs:loop", Some("Again.")),
                    candidate("rs:steer", None)
                ]
            }))
        );
        assert_eq!(
            policy("  mode: open\n  candidates: []\n  blacklist: [rs:x]"),
            Ok(Some(Policy::Open {
                candidates: Vec::new(),
                blacklist: vec!["rs:x".into()]
            }))
        );
        assert_eq!(
            policy("  mode: forced\n  target: rs:review"),
            Ok(Some(Policy::Forced {
                target: "rs:review".into()
            }))
        );
        assert_eq!(policy("  ~"), Ok(None));
        let error = |next: &str| format!("rs:x.md: invalid next block — {next}");
        for (next, message) in [
            ("  - closed", "next must be a mapping"),
            ("  mode: 3", "next.mode must be a string"),
            ("  mode: sometimes", "unknown next.mode: sometimes"),
            (
                "  mode: forced\n  target: ' '",
                "next.target must be a non-empty string for mode: forced",
            ),
            (
                "  mode: closed\n  candidates: []",
                "next.candidates must not be empty for mode: closed",
            ),
            ("  mode: closed", "next.candidates must be a list"),
            (
                "  mode: open\n  candidates: [rs:x]",
                "next.candidates[0] must be a mapping",
            ),
            (
                "  mode: open\n  candidates:\n    - hint: h",
                "next.candidates[0].name must be a non-empty string",
            ),
            (
                "  mode: open\n  candidates:\n    - name: a\n      hint: ~",
                "next.candidates[0].hint must be a string when set",
            ),
            (
                "  mode: open\n  candidates: []\n  blacklist: x",
                "next.blacklist must be a list",
            ),
            (
                "  mode: open\n  candidates: []\n  blacklist: ['']",
                "next.blacklist[0] must be a non-empty string",
            ),
            (
                "  mode: ask\n  hint: ''",
                "next.hint must be a non-empty string when set",
            ),
        ] {
            assert_eq!(policy(next), Err(error(message)), "{next}");
        }
    }

    fn args(text: &str) -> Vec<String> {
        split_args(text)
    }

    #[test]
    fn arguments_split_at_blanks_with_quotes_grouping_them() {
        assert_eq!(args("a  'b c' \"d e\"f\tg"), ["a", "b c", "d ef", "g"]);
        assert_eq!(args("\"\" x"), ["x"]);
        assert_eq!(args("it's"), ["its"]);
        assert_eq!(args("one\ntwo"), ["one\ntwo"]);
        assert!(args("   ").is_empty());
    }

    #[test]
    fn placeholders_substitute_as_scramjet_does() {
        let body = "$1|$2|$3|${@:2}|${@:2:1}|${@:0:1}|$ARGUMENTS|$@|$0|${@:x}";
        assert_eq!(
            substitute(body, &args("one 'two three' four")),
            "one|two three|four|two three four|two three|one|one two three four|one two three four||${@:x}"
        );
        // Values are not substituted again by the positional pass, but the `$@` pass sees them.
        assert_eq!(substitute("$1|$@", &args("'$2' b")), "$2|$2 b");
        assert_eq!(substitute("$ARGUMENTS", &[]), "");
        assert_eq!(substitute("${@:9}$12", &args("a")), "");
    }

    #[test]
    fn a_prompt_runs_a_command_only_when_it_names_one() {
        let scratch = Scratch::new("expand");
        scratch.file("project/rs/commands/rs:echo.md", "Say $1 and ${@:2}");
        scratch.file(
            "project/rs/commands/rs:wrapped.md",
            "<scramjet-command name=\"x\">\nDone",
        );
        let commands = Commands::discover(&project_sets(&scratch.0.join("project")));
        assert_eq!(
            commands.expand("/rs:echo 'one two' three"),
            Some(
                "<scramjet-command name=\"rs:echo\">\nSay one two and three\n</scramjet-command>"
                    .into()
            )
        );
        assert_eq!(
            commands.expand("/rs:wrapped"),
            Some("<scramjet-command name=\"x\">\nDone".into())
        );
        assert_eq!(
            commands.expand("/rs:echo"),
            Some("<scramjet-command name=\"rs:echo\">\nSay  and \n</scramjet-command>".into())
        );
        assert_eq!(commands.expand("/rs:missing x"), None);
        assert_eq!(commands.expand("/rs:echo-more x"), None);
        assert_eq!(commands.expand("rs:echo x"), None);
        assert_eq!(commands.expand("/"), None);
    }
}
