//! Claude Code's custom commands: the `.md` files of a `.claude/commands` directory, which a
//! prompt runs as `/NAME args` (a file in a subdirectory as `/DIR:NAME`). Unlike a command
//! set's, one is sent as the user's prompt, with Claude Code's argument substitution, and starts
//! no command: no next steps, no status report.
use super::Problem;
use super::commands::{split_args, split_frontmatter};
use super::yaml;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use yaml_rust2::Yaml;

#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub path: PathBuf,
}

/// The commands found, first of a name wins, and why files were skipped.
#[derive(Clone, Debug, Default)]
pub struct ClaudeCommands {
    list: Vec<ClaudeCommand>,
    warnings: Vec<Problem>,
}

const COMMANDS: &str = ".claude/commands";

/// Where Claude Code's commands are looked for, first wins: the `.claude/commands` of `root` and
/// of the directories above it up to its Git root, then the `commands` of the user's `claude`
/// directory, `~/.claude` (not with `--bare`, and never also as a project's).
fn places(root: &Path, claude: Option<&Path>) -> Vec<PathBuf> {
    let user = claude.map(|claude| crate::os::join(claude, "commands"));
    let real = claude
        .and_then(|claude| crate::os::canonicalize(claude).ok())
        .map(|claude| crate::os::join(&claude, "commands"));
    let mut places = Vec::new();
    for directory in root.ancestors() {
        let commands = crate::os::join(directory, COMMANDS);
        if user.as_ref() != Some(&commands) && real.as_ref() != Some(&commands) {
            places.push(commands);
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    places.extend(user.filter(|_| !super::bare()));
    places
}

impl ClaudeCommands {
    /// The commands of a session in `root`.
    pub fn discover(root: &Path) -> Self {
        Self::load(&places(root, super::login::claude_directory().as_deref()))
    }

    fn load(places: &[PathBuf]) -> Self {
        let mut found = Self::default();
        let mut seen = HashSet::new();
        for place in places {
            if !place.is_dir() {
                continue;
            }
            if let Some(owner) = crate::os::stranger(place) {
                let text = crate::os::left_out(place, owner);
                found.warnings.push(Problem::in_file(text, place));
                continue;
            }
            let mut files = Vec::new();
            command_files(place, "", &mut seen, &mut files);
            for (name, path) in files {
                let content = match fs::read_to_string(&path) {
                    Ok(content) => content,
                    Err(error) => {
                        let text = format!("skipping {}: {error}", path.display());
                        found.warnings.push(Problem::in_file(text, &path));
                        continue;
                    }
                };
                let mut notices = Vec::new();
                let command = parse(name, &path, &content, &mut notices);
                let in_file = |text| Problem::in_file(text, &path);
                found.warnings.extend(notices.into_iter().map(in_file));
                match found.list.iter().find(|known| known.name == command.name) {
                    Some(known) if same_content(&known.path, &path) => {}
                    Some(known) => found.warnings.push(Problem::in_file(
                        format!(
                            "skipping command /{} at {}: name already loaded from {}",
                            command.name,
                            path.display(),
                            known.path.display()
                        ),
                        &path,
                    )),
                    None => found.list.push(command),
                }
            }
        }
        found
    }

    pub fn list(&self) -> &[ClaudeCommand] {
        &self.list
    }

    pub fn warnings(&self) -> &[Problem] {
        &self.warnings
    }

    /// The command `/<name> args` names.
    pub(super) fn named(&self, prompt: &str) -> Option<&ClaudeCommand> {
        let rest = prompt.strip_prefix('/')?;
        let name = rest.split(char::is_whitespace).next().unwrap_or(rest);
        self.list.iter().find(|command| command.name == name)
    }

    /// The text `/<name> args` sends: the command's body as its file says now, the arguments
    /// substituted. `None` when the prompt names no command; an error when its file cannot be
    /// read.
    pub(super) fn expand(&self, prompt: &str) -> Option<Result<String, String>> {
        let command = self.named(prompt)?;
        let args = (prompt[1..].split_once(char::is_whitespace)).map_or("", |(_, args)| args);
        Some(match fs::read_to_string(&command.path) {
            Ok(content) => {
                let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
                let (frontmatter, body) = split_frontmatter(content);
                let names = yaml::first(&frontmatter)
                    .map(|frontmatter| argument_names(&frontmatter["arguments"]))
                    .unwrap_or_default();
                Ok(substitute(&body, args.trim(), &names))
            }
            Err(error) => Err(format!(
                "/{}: cannot read {} ({error}); the prompt was sent as typed",
                command.name,
                command.path.display()
            )),
        })
    }
}

/// The `.md` files under `directory`, by name: a file's stem, after its subdirectories' names
/// joined by `:` as Claude Code names them. Hidden entries are left out; links are followed,
/// each directory and file once.
fn command_files(
    directory: &Path,
    prefix: &str,
    seen: &mut HashSet<PathBuf>,
    files: &mut Vec<(String, PathBuf)>,
) {
    if !seen.insert(directory.canonicalize().unwrap_or(directory.to_path_buf())) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();
    entries.sort();
    for (file, path) in entries {
        if path.is_dir() {
            command_files(&path, &format!("{prefix}{file}:"), seen, files);
        } else if let Some(stem) = file.strip_suffix(".md")
            && path.is_file()
            && seen.insert(path.canonicalize().unwrap_or(path.clone()))
        {
            files.push((format!("{prefix}{stem}"), path));
        }
    }
}

fn parse(name: String, path: &Path, content: &str, notices: &mut Vec<String>) -> ClaudeCommand {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let (frontmatter, body) = split_frontmatter(content);
    // Claude Code loads a command whose frontmatter does not parse, without it.
    let frontmatter = yaml::first(&frontmatter).unwrap_or_else(|error| {
        notices.push(format!(
            "{}: frontmatter ignored, as it does not parse: {error}",
            path.display()
        ));
        Yaml::BadValue
    });
    let field = |key: &str| {
        (frontmatter[key].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let description = field("description").unwrap_or_else(|| {
        let first = body.lines().map(str::trim).find(|line| !line.is_empty());
        let first = first.unwrap_or_default().trim_start_matches('#').trim();
        match first.char_indices().nth(60) {
            Some((at, _)) => format!("{}...", &first[..at]),
            None => first.to_owned(),
        }
    });
    ClaudeCommand {
        name,
        description,
        argument_hint: field("argument-hint"),
        path: path.to_path_buf(),
    }
}

/// The names `arguments` gives the positional arguments, as Claude Code reads a list or a
/// whitespace-separated string; numbers and empty names are not names.
fn argument_names(value: &Yaml) -> Vec<String> {
    let names: Vec<String> = match value {
        Yaml::Array(items) => (items.iter())
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Yaml::String(text) => text.split_whitespace().map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    let named = |name: &String| {
        !name.trim().is_empty() && !name.chars().all(|character| character.is_ascii_digit())
    };
    names.into_iter().filter(named).collect()
}

fn same_content(one: &Path, other: &Path) -> bool {
    matches!((fs::read(one), fs::read(other)), (Ok(one), Ok(other)) if one == other)
}

/// Claude Code's substitution: a named argument `$NAME` (empty when not given), `$ARGUMENTS[N]`
/// and `$N` (counted from 0, left as written when not given) and `$ARGUMENTS`, the whole; `\$`
/// before one of them keeps it as written. Values put in are not substituted again. With
/// arguments but nothing substituted, they follow the body as `ARGUMENTS: …`.
fn substitute(body: &str, args: &str, names: &[String]) -> String {
    let split = split_args(args);
    let mut names: Vec<(usize, &str)> = (names.iter().enumerate())
        .map(|(at, name)| (at, name.as_str()))
        .collect();
    names.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));
    let word = |character: char| character.is_alphanumeric() || character == '_';
    let ends = |rest: &str, length: usize| {
        !rest[length..].starts_with(|character: char| word(character) || character == '[')
    };
    let digits =
        |text: &str| text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let mut out = String::new();
    let mut used = false;
    let mut at = 0;
    while let Some(found) = body[at..].find('$') {
        let dollar = at + found;
        let rest = &body[dollar + 1..];
        let named =
            (names.iter()).find(|(_, name)| rest.starts_with(name) && ends(rest, name.len()));
        let known = named.is_some() || digits(rest) > 0 || rest.starts_with("ARGUMENTS");
        if dollar > at && body[..dollar].ends_with('\\') && known {
            out.push_str(&body[at..dollar - 1]);
            out.push('$');
            at = dollar + 1;
            continue;
        }
        out.push_str(&body[at..dollar]);
        let (value, length) = if let Some((index, name)) = named {
            (
                Some(split.get(*index).map_or("", String::as_str)),
                name.len(),
            )
        } else if let Some(index) = rest.strip_prefix("ARGUMENTS[")
            && digits(index) > 0
            && index[digits(index)..].starts_with(']')
        {
            let length = digits(index);
            let value = index[..length]
                .parse::<usize>()
                .ok()
                .and_then(|n| split.get(n));
            (value.map(String::as_str), "ARGUMENTS[".len() + length + 1)
        } else if digits(rest) > 0 && !rest[digits(rest)..].starts_with(word) {
            let length = digits(rest);
            let value = rest[..length]
                .parse::<usize>()
                .ok()
                .and_then(|n| split.get(n));
            (value.map(String::as_str), length)
        } else if rest.starts_with("ARGUMENTS") {
            (Some(args), "ARGUMENTS".len())
        } else {
            (None, 0)
        };
        match value {
            Some(value) => {
                out.push_str(value);
                used = true;
            }
            None => out.push_str(&body[dollar..dollar + 1 + length]),
        }
        at = dollar + 1 + length;
    }
    out.push_str(&body[at..]);
    if !used && !args.is_empty() {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(args);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::ScratchDir;
    use super::*;

    fn scratch(name: &str) -> ScratchDir {
        let path = std::env::temp_dir().join(format!(
            "scramjet-claude-commands-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        ScratchDir::new(path)
    }

    #[test]
    fn arguments_are_substituted_as_claude_code_does() {
        let names = ["file".to_owned(), "mode".to_owned()];
        let cases = [
            ("Review $ARGUMENTS.", "a b", "Review a b."),
            ("$0 then $1, $2", "a 'b c'", "a then b c, $2"),
            (
                "$ARGUMENTS[1] of $ARGUMENTS[0]; $ARGUMENTS[5]",
                "x y",
                "y of x; $ARGUMENTS[5]",
            ),
            (
                "Open $file in $mode, not $filex or $file[0]",
                "f.rs",
                "Open f.rs in , not $filex or $file[0]",
            ),
            (
                "Costs \\$1 and \\$ARGUMENTS, $1x",
                "a b",
                "Costs $1 and $ARGUMENTS, $1x\n\nARGUMENTS: a b",
            ),
            (
                "No placeholders.",
                "a b",
                "No placeholders.\n\nARGUMENTS: a b",
            ),
            ("No placeholders.", "", "No placeholders."),
            ("Price: $ alone, \\$x", "", "Price: $ alone, \\$x"),
            ("Given: $ARGUMENTS", "", "Given: "),
            (
                "$0 is \"$ARGUMENTS\"",
                "$1 $ARGUMENTS",
                "$1 is \"$1 $ARGUMENTS\"",
            ),
        ];
        for (body, args, expected) in cases {
            assert_eq!(
                substitute(body, args, &names),
                expected,
                "{body} with {args}"
            );
        }
    }

    #[test]
    fn commands_are_found_from_the_project_up_to_its_git_root_then_the_users() {
        let scratch = scratch("places");
        let top = scratch.0.join("repo");
        let root = top.join("sub");
        let home = scratch.0.join("home");
        let write = |path: PathBuf, text: &str| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        fs::create_dir_all(top.join(".git")).unwrap();
        write(
            root.join(COMMANDS).join("review.md"),
            "---\ndescription: \"Project review\"\nargument-hint: \"[issue]\"\n---\n\nReview $ARGUMENTS",
        );
        write(
            root.join(COMMANDS).join("ops/deploy.md"),
            "# Deploy it\n\nnow",
        );
        write(root.join(COMMANDS).join(".hidden.md"), "no");
        write(root.join(COMMANDS).join("notes.txt"), "no");
        write(top.join(COMMANDS).join("review.md"), "another");
        write(
            top.join(COMMANDS).join("top.md"),
            "---\nbad: [\n---\nFrom the top",
        );
        write(
            scratch.0.join(COMMANDS).join("above.md"),
            "above the Git root",
        );
        write(home.join(COMMANDS).join("mine.md"), "Mine");
        write(
            home.join(COMMANDS).join("ops/deploy.md"),
            "# Deploy it\n\nnow",
        );
        let found = ClaudeCommands::load(&places(&root, Some(&home.join(".claude"))));
        let names: Vec<_> = found.list().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["ops:deploy", "review", "top", "mine"]);
        let review = &found.list()[1];
        assert_eq!(review.description, "Project review");
        assert_eq!(review.argument_hint.as_deref(), Some("[issue]"));
        assert_eq!(found.list()[0].description, "Deploy it");
        assert_eq!(found.list()[2].description, "From the top");
        // The other review.md and the top's frontmatter warn; an identical copy does not.
        let warnings: Vec<_> = found.warnings().iter().map(|w| w.to_string()).collect();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("name already loaded"), "{warnings:?}");
        assert!(warnings[1].contains("frontmatter ignored"), "{warnings:?}");
        assert_eq!(
            found.expand("/review  42 now\n"),
            Some(Ok("Review 42 now".to_owned()))
        );
        assert_eq!(
            found.expand("/top 7"),
            Some(Ok("From the top\n\nARGUMENTS: 7".to_owned()))
        );
        assert_eq!(found.expand("/reviews"), None);
        assert_eq!(found.expand("review"), None);
        fs::remove_file(root.join(COMMANDS).join("review.md")).unwrap();
        assert!(
            found
                .expand("/review")
                .unwrap()
                .unwrap_err()
                .contains("cannot read")
        );
        // The home as the project lists its commands once, as the user's.
        let mine = places(&home, Some(&home.join(".claude")));
        assert_eq!(mine.last(), Some(&home.join(COMMANDS)));
        assert_eq!(
            mine.iter().filter(|p| **p == home.join(COMMANDS)).count(),
            1
        );
    }

    #[test]
    fn named_arguments_come_from_the_frontmatter() {
        let scratch = scratch("named");
        let directory = scratch.0.join(COMMANDS);
        fs::create_dir_all(&directory).unwrap();
        let text = "---\narguments: [issue, \"2\", branch]\n---\nFix $issue on $branch";
        fs::write(directory.join("fix.md"), text).unwrap();
        let found = ClaudeCommands::load(&[directory]);
        assert_eq!(
            found.expand("/fix 12 main"),
            Some(Ok("Fix 12 on main".to_owned()))
        );
    }
}
