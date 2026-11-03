//! Scramjet's command sets: `<set>:<command>.md` files under the global root and the project's
//! `.scramjet` directory. A prompt `/<set>:<command> args` runs one of them as Scramjet sends it:
//! the arguments are substituted into the body and the result is wrapped in `<scramjet-command>`.

use std::{
    fs,
    path::{Path, PathBuf},
};
use yaml_rust2::{Yaml, YamlLoader};

/// Sets Scramjet loads from its own package, so a directory of these names is not a set here.
const BUNDLED_SETS: [&str; 2] = ["mach12", "scramjet"];

/// A command a prompt can run.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// `<set>:<command>`, which is also what the prompt names.
    pub name: String,
    /// The frontmatter's, else the body's first line, as Scramjet's autocomplete shows it.
    pub description: String,
    pub argument_hint: Option<String>,
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

/// The commands found for a session, by name.
#[derive(Clone, Debug, Default)]
pub struct Commands {
    list: Vec<Command>,
    warnings: Vec<String>,
}

/// Scramjet's global root for command sets: `$SCRAMJET_CACHE`, else `$XDG_DATA_HOME/scramjet`,
/// else `~/.local/share/scramjet`.
pub fn global_root() -> Option<PathBuf> {
    if let Some(cache) = std::env::var_os("SCRAMJET_CACHE") {
        return Some(PathBuf::from(cache));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".local/share")))?;
    Some(data.join("scramjet"))
}

impl Commands {
    /// Finds the commands under `global`, then under `project` (the project's `.scramjet`
    /// directory). The first file registered under a name wins, so a global command shadows a
    /// project one of the same name. Files that are not `<set>:` commands, or whose frontmatter
    /// Scramjet would reject, are skipped with a warning.
    pub fn discover(global: Option<&Path>, project: &Path) -> Self {
        let mut found = Self::default();
        for root in global.into_iter().chain([project]) {
            for set in sets(root) {
                let directory = root.join(&set).join("commands");
                for (file, content) in command_files(&directory) {
                    let command = match parse(&set, &file, &content) {
                        Ok(command) => command,
                        Err(error) => {
                            let path = directory.join(&file);
                            found
                                .warnings
                                .push(format!("skipping {}: {error}", path.display()));
                            continue;
                        }
                    };
                    if !found.list.iter().any(|known| known.name == command.name) {
                        found.list.push(command);
                    }
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

    /// Commands from `(file, content)` pairs, each in the set its name starts with.
    #[cfg(test)]
    pub(super) fn parsed(files: &[(&str, &str)]) -> Self {
        let list = files.iter().map(|(file, content)| {
            let set = file.split(':').next().unwrap();
            parse(set, file, content).unwrap()
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

/// The set directories under `root`, by name, except the bundled ones. Links count as what they
/// point to.
fn sets(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !BUNDLED_SETS.contains(&name.as_str()))
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

/// The command in `content`, the file `file` of set `set`; otherwise why Scramjet's loader
/// rejects it, in its words.
fn parse(set: &str, file: &str, content: &str) -> Result<Command, String> {
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
    let description = field("description").unwrap_or_else(|| {
        let first = body.lines().find(|line| !line.trim().is_empty());
        let first = first.unwrap_or_default();
        match first.char_indices().nth(60) {
            Some((at, _)) => format!("{}...", &first[..at]),
            None => first.to_owned(),
        }
    });
    Ok(Command {
        name: name.to_owned(),
        description,
        argument_hint: field("argument-hint"),
        body,
        next,
    })
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
fn split_frontmatter(content: &str) -> (String, String) {
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
        scratch.file("global/rs/commands/notes.md", "---\n---\nNo prefix\n");
        scratch.file("global/rs/commands/rs:notes.txt", "not markdown\n");
        scratch.file("global/rs/commands/rs:folder.md/inner.txt", "not a file\n");
        scratch.file("global/mach12/commands/mach12:x.md", "Bundled\n");
        scratch.file("global/scramjet/commands/scramjet:x.md", "Bundled\n");
        scratch.file("global/other/commands/rs:wrong-set.md", "Wrong set\n");
        scratch.file(
            "project/rs/commands/rs:review.md",
            "---\ndescription: Project review\n---\nProject\n",
        );
        scratch.file("project/rs/commands/rs:loop.md", "Loop $ARGUMENTS\n");

        let commands = Commands::discover(Some(&global), &project);
        assert_eq!(names(&commands), ["rs:helper", "rs:loop", "rs:review"]);
        let review = &commands.list()[2];
        assert_eq!(review.description, "Global review");
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
                skipped("notes.md", "rs")
            ]
        );
        assert_eq!(
            commands.expand("/rs:review HEAD~2.."),
            Some(
                "<scramjet-command name=\"rs:review\">\nGlobal HEAD~2..\n</scramjet-command>"
                    .into()
            )
        );
        // A delegate-only command runs from the prompt too, as in Scramjet.
        assert_eq!(
            commands.expand("/rs:helper a b"),
            Some(
                "<scramjet-command name=\"rs:helper\">\nHelp with <caller-context>a b</caller-context>\n</scramjet-command>"
                    .into()
            )
        );
        assert_eq!(commands.expand("/mach12:x"), None);
        assert_eq!(commands.expand("/rs:notes"), None);
    }

    #[test]
    fn a_project_command_is_found_without_a_global_root() {
        let scratch = Scratch::new("no-global");
        scratch.file("project/rs/commands/rs:only.md", "Only\n");
        let commands = Commands::discover(None, &scratch.0.join("project"));
        assert_eq!(names(&commands), ["rs:only"]);
        assert_eq!(
            commands.expand("/rs:only"),
            Some("<scramjet-command name=\"rs:only\">\nOnly\n\n</scramjet-command>".into())
        );
    }

    #[test]
    fn frontmatter_is_read_as_scramjet_reads_it() {
        let crlf = "\u{feff}---\r\ndescription: \"Quoted\"\r\nargument-hint: '<a> [b]'\r\n---\r\n\r\n  Body\r\nmore  \r\n";
        let command = parse("rs", "rs:x.md", crlf).unwrap();
        assert_eq!(command.description, "Quoted");
        assert_eq!(command.next, None);
        assert_eq!(command.argument_hint.as_deref(), Some("<a> [b]"));
        assert_eq!(command.body, "Body\nmore");
        // Without a closing line, the whole text is the body, untrimmed.
        let open = parse("rs", "rs:x.md", "---\ndescription: no end\n").unwrap();
        assert_eq!(open.argument_hint, None);
        assert_eq!(open.body, "---\ndescription: no end\n");
        assert_eq!(open.description, "---");
        // Without a description, the body's first line, cut at 60 characters.
        let long = format!("\n\n{}é tail\nsecond\n", "x".repeat(59));
        let command = parse("rs", "rs:x.md", &long).unwrap();
        assert_eq!(command.description, format!("{}é...", "x".repeat(59)));
        assert!(parse("rs", "other.md", "Body").is_err());
        // As YAML, a folded description is one line, and a non-string one is not a description.
        let folded = parse(
            "rs",
            "rs:x.md",
            "---\ndescription: >\n  Two\n  lines\n---\nB",
        )
        .unwrap();
        assert_eq!(folded.description, "Two lines\n");
        let number = parse("rs", "rs:x.md", "---\ndescription: 12\n---\nBody line").unwrap();
        assert_eq!(number.description, "Body line");
        assert_eq!(
            parse("rs", "rs:x.md", "---\ndescription: a: b\n---\nB").unwrap_err(),
            "rs:x.md: malformed frontmatter — mapping values are not allowed in this context \
at byte 14 line 1 column 15"
        );
    }

    fn policy(next: &str) -> Result<Option<Policy>, String> {
        let content = format!("---\nnext:\n{next}\n---\nBody");
        parse("rs", "rs:x.md", &content).map(|command| command.next)
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
        let commands = Commands::discover(None, &scratch.0.join("project"));
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
