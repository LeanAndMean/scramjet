//! Scramjet's command sets: `<set>:<command>.md` files under the global root and the project's
//! `.scramjet` directory. A prompt `/<set>:<command> args` runs one of them as Scramjet sends it:
//! the arguments are substituted into the body and the result is wrapped in `<scramjet-command>`.

use std::{
    fs,
    path::{Path, PathBuf},
};

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
}

/// The commands found for a session, by name.
#[derive(Clone, Debug, Default)]
pub struct Commands {
    list: Vec<Command>,
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
    /// project one of the same name. Files that are not `<set>:` commands are skipped.
    pub fn discover(global: Option<&Path>, project: &Path) -> Self {
        let mut list: Vec<Command> = Vec::new();
        for root in global.into_iter().chain([project]) {
            for set in sets(root) {
                let directory = root.join(&set).join("commands");
                for (file, content) in command_files(&directory) {
                    let Some(command) = parse(&set, &file, &content) else {
                        continue;
                    };
                    if !list.iter().any(|known| known.name == command.name) {
                        list.push(command);
                    }
                }
            }
        }
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Self { list }
    }

    pub fn list(&self) -> &[Command] {
        &self.list
    }

    /// The text a prompt sends when it runs a command, `None` when it runs none: `/<name> args`
    /// names a command, whose body takes the arguments split as Scramjet splits them.
    pub fn expand(&self, prompt: &str) -> Option<String> {
        let rest = prompt.strip_prefix('/')?;
        let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let command = self.list.iter().find(|command| command.name == name)?;
        let body = substitute(&command.body, &split_args(args));
        if body.starts_with("<scramjet-command") {
            return Some(body);
        }
        Some(format!(
            "<scramjet-command name=\"{name}\">\n{body}\n</scramjet-command>"
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

/// The command in `content`, the file `file` of set `set`; `None` when the file name lacks the
/// set's `<set>:` prefix.
fn parse(set: &str, file: &str, content: &str) -> Option<Command> {
    let name = file.strip_suffix(".md")?;
    if !name.starts_with(&format!("{set}:")) {
        return None;
    }
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (frontmatter, body) = split_frontmatter(content);
    // Only top-level scalars are read; nested blocks such as `next:` are indented.
    let field = |key: &str| {
        frontmatter
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(found, _)| *found == key)
            .map(|(_, value)| unquote(value.trim()).to_owned())
            .filter(|value| !value.is_empty())
    };
    let description = field("description").unwrap_or_else(|| {
        let first = body.lines().find(|line| !line.trim().is_empty());
        let first = first.unwrap_or_default();
        match first.char_indices().nth(60) {
            Some((at, _)) => format!("{}...", &first[..at]),
            None => first.to_owned(),
        }
    });
    Some(Command {
        name: name.to_owned(),
        description,
        argument_hint: field("argument-hint"),
        body,
    })
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

/// A scalar without its matching quotes.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
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
            "---\ndescription: Global review\nargument-hint: \"[commits or area]\"\nnext:\n  description: nested\n---\nGlobal $1\n",
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
        assert!(parse("rs", "other.md", "Body").is_none());
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
