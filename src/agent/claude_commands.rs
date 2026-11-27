//! Claude Code's custom commands: the `.md` files of a `.claude/commands` directory, which a
//! prompt runs as `/claude:NAME args` (a file in a subdirectory as `/claude:DIR:NAME`), named so
//! that where it comes from shows. Unlike a command
//! set's, one is sent as the user's prompt, with Claude Code's argument substitution, and starts
//! no command: no next steps, no status report. A Claude Code plugin's commands are its
//! `commands` directory's, named `/PLUGIN:NAME`.
use super::commands::{Origin, Set, split_args, split_frontmatter};
use super::tools::{self, Approval, Call};
use super::{Agent, AgentUi, Answer, Problem, yaml};
use serde_json::json;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use yaml_rust2::Yaml;

#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    pub path: PathBuf,
    /// The directory of the Claude Code plugin it comes from, its `${CLAUDE_PLUGIN_ROOT}`.
    pub plugin: Option<PathBuf>,
}

/// A directory of commands, with what their names start with and the plugin they come from.
struct Place {
    directory: PathBuf,
    prefix: String,
    plugin: Option<PathBuf>,
}

impl Place {
    fn plain(directory: PathBuf) -> Self {
        Self {
            directory,
            prefix: "claude:".to_owned(),
            plugin: None,
        }
    }
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
fn places(root: &Path, claude: Option<&Path>) -> Vec<Place> {
    let user = claude.map(|claude| crate::os::join(claude, "commands"));
    let real = claude
        .and_then(|claude| crate::os::canonicalize(claude).ok())
        .map(|claude| crate::os::join(&claude, "commands"));
    let mut places = Vec::new();
    for directory in root.ancestors() {
        let commands = crate::os::join(directory, COMMANDS);
        if user.as_ref() != Some(&commands) && real.as_ref() != Some(&commands) {
            places.push(Place::plain(commands));
        }
        if directory.join(".git").exists() {
            break;
        }
    }
    places.extend(user.filter(|_| !super::bare()).map(Place::plain));
    places
}

impl ClaudeCommands {
    /// The commands of a session in `root` when `own` (Claude Code's own directories are read),
    /// then those of the Claude Code plugins among `sets`.
    pub fn discover(root: &Path, sets: &[Set], own: bool) -> Self {
        let mut places = match own {
            true => places(root, super::login::claude_directory().as_deref()),
            false => Vec::new(),
        };
        places.extend(plugins(sets));
        Self::load(&places)
    }

    fn load(places: &[Place]) -> Self {
        let mut found = Self::default();
        let mut seen = HashSet::new();
        for Place {
            directory: place,
            prefix,
            plugin,
        } in places
        {
            if !place.is_dir() {
                continue;
            }
            if let Some(owner) = crate::os::stranger(place) {
                let text = crate::os::left_out(place, owner);
                found.warnings.push(Problem::in_file(text, place));
                continue;
            }
            let mut files = Vec::new();
            command_files(place, prefix, &mut seen, &mut files);
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
                let command = ClaudeCommand {
                    plugin: plugin.clone(),
                    ..parse(name, &path, &content, &mut notices)
                };
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
    /// substituted, then `session`'s variables. `None` when the prompt names no command; an
    /// error when its file cannot be read.
    pub(super) fn expand(
        &self,
        prompt: &str,
        session: &Variables,
    ) -> Option<Result<String, String>> {
        let command = self.named(prompt)?;
        let args = (prompt[1..].split_once(char::is_whitespace)).map_or("", |(_, args)| args);
        Some(match fs::read_to_string(&command.path) {
            Ok(content) => {
                let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
                let (frontmatter, body) = split_frontmatter(content);
                let names = yaml::frontmatter(&frontmatter)
                    .map(|frontmatter| argument_names(&frontmatter["arguments"]))
                    .unwrap_or_default();
                let text = session.substitute(&substitute(&body, args.trim(), &names));
                Ok(match &command.plugin {
                    Some(root) => {
                        text.replace("${CLAUDE_PLUGIN_ROOT}", &root.display().to_string())
                    }
                    None => text,
                })
            }
            Err(error) => Err(format!(
                "/{}: cannot read {} ({error}); the prompt was sent as typed",
                command.name,
                command.path.display()
            )),
        })
    }
}

/// The `commands` directories of the Claude Code plugins among `sets`.
fn plugins(sets: &[Set]) -> impl Iterator<Item = Place> {
    let plugins = sets.iter().filter(|set| set.origin == Origin::ClaudeCode);
    plugins.map(|set| Place {
        directory: set.directory.join("commands"),
        prefix: format!("{}:", set.name),
        plugin: Some(set.directory.clone()),
    })
}

/// What Claude Code puts for `${CLAUDE_PROJECT_DIR}`, `${CLAUDE_SESSION_ID}` and
/// `${CLAUDE_EFFORT}` in a command, after its arguments.
pub(super) struct Variables {
    pub project: PathBuf,
    pub session: String,
    pub effort: String,
}

impl Variables {
    pub(super) fn substitute(&self, text: &str) -> String {
        let project = self.project.display().to_string();
        (text.replace("${CLAUDE_PROJECT_DIR}", &project))
            .replace("${CLAUDE_SESSION_ID}", &self.session)
            .replace("${CLAUDE_EFFORT}", &self.effort)
    }
}

impl Agent {
    /// The variables of a Claude Code command run now: the project's root, the session's id
    /// and the effort requests ask for, `high` when none is set, as Claude Code's default.
    pub(super) fn claude_variables(&self) -> Variables {
        Variables {
            project: self.config.root.clone(),
            session: self.session_id(),
            effort: self.effort().unwrap_or("high").to_owned(),
        }
    }

    /// `text`, the body `/<name>` sends, with each of its `shell_commands` replaced by what the
    /// command printed, as Claude Code runs them before sending: one after another, through
    /// the agent's bash as its calls run (its sandbox, its hidden stores, asked first under
    /// `--ask`). The first that fails or is denied stops it, saying why: the prompt is not sent.
    pub(super) fn run_shell_commands(
        &mut self,
        name: &str,
        mut text: String,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        for (raw, command) in shell_commands(&text) {
            let call = Call {
                name: "bash",
                arguments: json!({"command": command}),
            };
            if let Approval::Ask(question) = call.approval(&self.config.root)
                && self.config.ask
            {
                ui.question(&format!(
                    "/{name} runs this command before its prompt is sent:\n\n```bash\n{command}\n```"
                ));
                match ui.approve(&question, cancel) {
                    Answer::Deny => {
                        return Err(format!(
                            "/{name} was not sent: the user denied its command `{command}`"
                        ));
                    }
                    Answer::AllowAll => self.config.ask = false,
                    Answer::Allow => {}
                }
            }
            let output = self.run_aside(&call, ui, cancel);
            if output.failed {
                return Err(format!(
                    "/{name} was not sent: its command `{command}` failed: {}",
                    output.text.trim()
                ));
            }
            let printed = match output.text.as_str() {
                tools::NO_OUTPUT => "",
                printed => printed.trim(),
            };
            if let Some(at) = text.find(&raw) {
                text.replace_range(at..at + raw.len(), printed);
            }
        }
        Ok(text)
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
    let frontmatter = yaml::frontmatter(&frontmatter).unwrap_or_else(|error| {
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
        plugin: None,
    }
}

/// The names `arguments` gives the positional arguments, as Claude Code reads a list or a
/// whitespace-separated string; numbers and empty names are not names.
pub(super) fn argument_names(value: &Yaml) -> Vec<String> {
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
pub(super) fn substitute(body: &str, args: &str, names: &[String]) -> String {
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
                out.push_str(&inert(value));
                used = true;
            }
            None => out.push_str(&body[dollar..dollar + 1 + length]),
        }
        at = dollar + 1 + length;
    }
    out.push_str(&body[at..]);
    if !used && !args.is_empty() {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(&inert(args));
    }
    out
}

/// An argument as Claude Code puts it in a command's body, where it cannot start one of
/// `shell_commands`: `` `! `` and `` !` `` split by a space, and a `!` at the start or after
/// white space escaped as `\!`.
fn inert(value: &str) -> String {
    let value = value.replace("`!", "` !").replace("!`", "! `");
    let mut out = String::with_capacity(value.len());
    let mut after_space = true;
    for character in value.chars() {
        if character == '!' && after_space {
            out.push('\\');
        }
        out.push(character);
        after_space = character.is_whitespace();
    }
    out
}

/// The shell commands a Claude Code command's body runs before it is sent, as Claude Code finds
/// them: each ```` ```! ```` block and each `` !`command` `` at the start or after white space,
/// outside other code spans, in the order they come. Each is the text it replaces and its
/// command, trimmed; empty ones are left out.
pub(super) fn shell_commands(body: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = body[at..].find("```!").map(|start| at + start) {
        let content = start + "```!".len();
        let Some(end) = body[content..].find("```").map(|end| content + end) else {
            break;
        };
        found.push((start, &body[start..end + 3], &body[content..end]));
        at = end + 3;
    }
    if body.contains("!`") {
        let blanked = blank_code_spans(body);
        let bytes = blanked.as_bytes();
        let mut at = 0;
        while let Some(bang) = blanked[at..].find("!`").map(|bang| at + bang) {
            let after_space = bang == 0 || blanked[..bang].ends_with(char::is_whitespace);
            let content = bang + 2;
            let end = (bytes[content..].iter()).position(|&byte| byte == b'`');
            match end.filter(|&end| after_space && end > 0) {
                Some(end) => {
                    let end = content + end;
                    found.push((bang, &body[bang..=end], &body[content..end]));
                    at = end + 1;
                }
                None => at = bang + 1,
            }
        }
    }
    found.sort_by_key(|(at, ..)| *at);
    (found.into_iter())
        .filter(|(_, _, command)| !command.trim().is_empty())
        .map(|(_, raw, command)| (raw.to_owned(), command.trim().to_owned()))
        .collect()
}

/// `body` with each one-line code span not right after `!` or `` ` `` blanked to spaces between
/// its backticks, as Claude Code keeps a `` !`command` `` written in one from running.
fn blank_code_spans(body: &str) -> String {
    let mut bytes = body.as_bytes().to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let end = (bytes[at] == b'`')
            .then(|| (bytes[at + 1..].iter()).position(|&byte| byte == b'`' || byte == b'\n'))
            .flatten()
            .filter(|&length| length > 0 && bytes[at + 1 + length] == b'`');
        match end {
            Some(length) => {
                let kept = at > 0 && matches!(body.as_bytes()[at - 1], b'!' | b'`');
                if !kept {
                    bytes[at + 1..at + 1 + length].fill(b' ');
                }
                at += length + 2;
            }
            None => at += 1,
        }
    }
    // Only ASCII bytes between backticks became spaces; a character cut there was all replaced.
    String::from_utf8(bytes).unwrap_or_else(|_| body.to_owned())
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

    fn none() -> Variables {
        Variables {
            project: PathBuf::from("/p"),
            session: "s".to_owned(),
            effort: "high".to_owned(),
        }
    }

    #[test]
    fn the_sessions_variables_follow_the_arguments() {
        let variables = Variables {
            project: PathBuf::from("/work/repo"),
            session: "1795-42".to_owned(),
            effort: "low".to_owned(),
        };
        let body = substitute(
            "In ${CLAUDE_PROJECT_DIR} (${CLAUDE_SESSION_ID}, ${CLAUDE_EFFORT}): $0; \\${CLAUDE_EFFORT} ${CLAUDE_OTHER} $CLAUDE_EFFORT",
            "${CLAUDE_SESSION_ID}",
            &[],
        );
        assert_eq!(
            variables.substitute(&body),
            "In /work/repo (1795-42, low): 1795-42; \\low ${CLAUDE_OTHER} $CLAUDE_EFFORT"
        );
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
            "---\nbad: x\n[\n---\nFrom the top",
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
        assert_eq!(
            names,
            [
                "claude:ops:deploy",
                "claude:review",
                "claude:top",
                "claude:mine"
            ]
        );
        // Not read, none of them is found.
        assert!(
            ClaudeCommands::discover(&root, &[], false)
                .list()
                .is_empty()
        );
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
            found.expand("/claude:review  42 now\n", &none()),
            Some(Ok("Review 42 now".to_owned()))
        );
        assert_eq!(
            found.expand("/claude:top 7", &none()),
            Some(Ok("From the top\n\nARGUMENTS: 7".to_owned()))
        );
        assert_eq!(found.expand("/claude:reviews", &none()), None);
        assert_eq!(found.expand("claude:review", &none()), None);
        // Never by the bare name, which hides where it comes from.
        assert_eq!(found.expand("/review 42", &none()), None);
        fs::remove_file(root.join(COMMANDS).join("review.md")).unwrap();
        assert!(
            found
                .expand("/claude:review", &none())
                .unwrap()
                .unwrap_err()
                .contains("cannot read")
        );
        // The home as the project lists its commands once, as the user's.
        let mine: Vec<PathBuf> = (places(&home, Some(&home.join(".claude"))).into_iter())
            .map(|place| place.directory)
            .collect();
        assert_eq!(mine.last(), Some(&home.join(COMMANDS)));
        assert_eq!(
            mine.iter().filter(|p| **p == home.join(COMMANDS)).count(),
            1
        );
    }

    #[test]
    fn shell_commands_are_found_as_claude_code_finds_them() {
        let found = |body: &str| -> Vec<(String, String)> { shell_commands(body) };
        let pair = |raw: &str, command: &str| (raw.to_owned(), command.to_owned());
        assert_eq!(
            found("Status: !`git status`\n!`git diff`, and !`  `."),
            [
                pair("!`git status`", "git status"),
                pair("!`git diff`", "git diff")
            ]
        );
        assert_eq!(
            found("Log: !`git log`\n```!\ngit log -3\ngit tag\n```\nthen !`date`"),
            [
                pair("!`git log`", "git log"),
                pair("```!\ngit log -3\ngit tag\n```", "git log -3\ngit tag"),
                pair("!`date`", "date")
            ]
        );
        // Not after a word, nor in a code span, nor unclosed; a span right after `!` is one.
        assert_eq!(found("Hi!`no` and `!`no`` and !`open"), []);
        assert_eq!(found("`code` !`yes` `x`"), [pair("!`yes`", "yes")]);
        assert_eq!(found("é !`echo ü`"), [pair("!`echo ü`", "echo ü")]);
        assert_eq!(found("```!```"), []);
    }

    #[test]
    fn arguments_cannot_start_a_shell_command() {
        assert_eq!(inert("!`rm -rf x`"), "\\! `rm -rf x`");
        assert_eq!(inert("a `!b` c! !d"), "a ` \\!b` c! \\!d");
        let body = substitute("Look at $ARGUMENTS", "!`touch x`", &[]);
        assert_eq!(body, "Look at \\! `touch x`");
        assert_eq!(shell_commands(&body), []);
        let body = substitute("No placeholders.", "x !`id`", &[]);
        assert_eq!(shell_commands(&body), []);
    }

    #[test]
    fn named_arguments_come_from_the_frontmatter() {
        let scratch = scratch("named");
        let directory = scratch.0.join(COMMANDS);
        fs::create_dir_all(&directory).unwrap();
        let text = "---\narguments: [issue, \"2\", branch]\n---\nFix $issue on $branch";
        fs::write(directory.join("fix.md"), text).unwrap();
        let found = ClaudeCommands::load(&[Place::plain(directory)]);
        assert_eq!(
            found.expand("/claude:fix 12 main", &none()),
            Some(Ok("Fix 12 on main".to_owned()))
        );
    }

    #[test]
    fn a_plugins_commands_are_named_by_it_and_given_its_directory() {
        let scratch = scratch("plugin");
        let directory = scratch.0.join("dev").join("1.0.0");
        fs::create_dir_all(directory.join("commands/sub")).unwrap();
        let text = "---\ndescription: Develop\n---\nRun ${CLAUDE_PLUGIN_ROOT}/go.sh $ARGUMENTS";
        fs::write(directory.join("commands/feature.md"), text).unwrap();
        fs::write(directory.join("commands/sub/deep.md"), "Deep").unwrap();
        let sets = [
            Set {
                name: "dev".into(),
                directory: directory.clone(),
                origin: Origin::ClaudeCode,
                skills: None,
            },
            Set {
                name: "other".into(),
                directory: directory.clone(),
                origin: Origin::Installed,
                skills: None,
            },
        ];
        let found = ClaudeCommands::load(&plugins(&sets).collect::<Vec<_>>());
        let names: Vec<_> = found.list().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["dev:feature", "dev:sub:deep"]);
        assert_eq!(found.list()[0].description, "Develop");
        assert_eq!(found.list()[0].plugin.as_ref(), Some(&directory));
        let run = format!("Run {}/go.sh now", directory.display());
        assert_eq!(found.expand("/dev:feature now", &none()), Some(Ok(run)));
        assert_eq!(found.expand("/feature", &none()), None);
    }
}
