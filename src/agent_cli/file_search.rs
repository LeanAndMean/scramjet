//! `@` file references in the prompt, as Scramjet's editor completes them: `fd` lists the
//! entries under the project (or a directory the word names) whose names match what follows the
//! `@`, and the best matches are offered. The text is sent as typed; the model reads the files.

use super::completion::Suggestion;
use std::{
    io::Read,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, TryRecvError},
};

/// Characters that end a word, as Scramjet's autocomplete counts them.
pub(super) const DELIMITERS: [char; 5] = [' ', '\t', '"', '\'', '='];
/// How many entries `fd` may list, and how many of the best are offered, as in Scramjet.
const FOUND: usize = 100;
const OFFERED: usize = 20;

/// An `@` word that ends at the cursor.
#[derive(Debug, PartialEq)]
pub(super) struct Mention {
    /// Where its `@` is in the text.
    pub(super) start: usize,
    /// What follows the `@` and the quote it may open.
    query: String,
    /// Whether it opens a quote: `@"my fi`.
    quoted: bool,
}

impl Mention {
    /// The `@` word before the cursor on its line, as Scramjet's `extractAtPrefix` finds it.
    pub(super) fn find(before: &str) -> Option<Self> {
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        let line = &before[line_start..];
        if let Some(quote) = open_quote(line) {
            let opened = line[..quote]
                .strip_suffix('@')
                .filter(|word| word.is_empty() || word.ends_with(DELIMITERS));
            if let Some(word) = opened {
                return Some(Self {
                    start: line_start + word.len(),
                    query: line[quote + 1..].to_owned(),
                    quoted: true,
                });
            }
        }
        let start = line.rfind(DELIMITERS).map_or(0, |at| at + 1);
        let query = line[start..].strip_prefix('@')?;
        Some(Self {
            start: line_start + start,
            query: query.to_owned(),
            quoted: false,
        })
    }

    /// The `@` word that Scramjet offers entries for as it is typed: one whose `@` starts a line
    /// or follows a space or tab.
    pub(super) fn typed(before: &str) -> Option<Self> {
        Self::find(before).filter(|mention| {
            matches!(
                before[..mention.start].chars().next_back(),
                None | Some(' ' | '\t' | '\n')
            )
        })
    }
}

/// Where the quote that `line` leaves open is, as Scramjet's `findUnclosedQuoteStart` finds it.
pub(super) fn open_quote(line: &str) -> Option<usize> {
    (line.matches('"').count() % 2 == 1).then(|| line.rfind('"').expect("counted"))
}

/// `fd` looking for a mention's entries, until it ends or this is dropped.
pub(super) struct Search {
    pub(super) mention: Mention,
    /// What `fd` looks for: the query, or its last part in the directory the rest names.
    query: String,
    /// That directory as typed, ending with `/`.
    directory: Option<String>,
    child: Child,
    output: mpsc::Receiver<Vec<u8>>,
}

impl Search {
    /// Starts `fd` in `root`, or in the directory the mention's query names, as Scramjet's
    /// `getFuzzyFileSuggestions` runs it. `None` without `fd`.
    pub(super) fn start(root: &Path, mention: Mention) -> Option<Self> {
        let program = scramjet_rs::agent::search_program("find")?;
        let scoped = mention.query.rfind('/').and_then(|slash| {
            let (shown, query) = mention.query.split_at(slash + 1);
            let base = match shown.strip_prefix("~/") {
                Some(rest) => std::env::home_dir()?.join(rest),
                None => root.join(shown),
            };
            base.is_dir()
                .then(|| (base, query.to_owned(), shown.to_owned()))
        });
        let (base, query, directory) = match scoped {
            Some((base, query, shown)) => (base, query, Some(shown)),
            None => (root.to_path_buf(), mention.query.clone(), None),
        };
        let mut command = Command::new(program);
        command
            .arg("--base-directory")
            .arg(&base)
            .args(["--max-results", &FOUND.to_string()])
            .args(["--type", "f", "--type", "d", "--follow", "--hidden"])
            .args([
                "--exclude",
                ".git",
                "--exclude",
                ".git/*",
                "--exclude",
                ".git/**",
            ]);
        if query.contains('/') {
            command.arg("--full-path");
        }
        if !query.is_empty() {
            command.arg("--").arg(pattern(&query));
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take().expect("piped");
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            let _ = send.send(bytes);
        });
        Some(Self {
            mention,
            query,
            directory,
            child,
            output,
        })
    }

    /// The entries offered once `fd` has ended, best first; none when it failed.
    pub(super) fn poll(&mut self) -> Option<Vec<Suggestion>> {
        let output = match self.output.try_recv() {
            Ok(output) => output,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Vec::new(),
        };
        if !self.child.wait().is_ok_and(|status| status.success()) {
            return Some(Vec::new());
        }
        Some(offered(
            &String::from_utf8_lossy(&output),
            &self.query,
            self.directory.as_deref(),
            self.mention.quoted,
        ))
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        // Does nothing to a process already waited for.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `fd`'s pattern for `query`, as Scramjet's `buildFdPathQuery` makes it: as typed, or for a
/// path, its parts escaped and joined by a separator.
fn pattern(query: &str) -> String {
    if !query.contains('/') {
        return query.to_owned();
    }
    let parts: Vec<String> = query
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.chars().fold(String::new(), |mut escaped, c| {
                if ".*+?^${}()|[]\\".contains(c) {
                    escaped.push('\\');
                }
                escaped.push(c);
                escaped
            })
        })
        .collect();
    if parts.is_empty() {
        return query.to_owned();
    }
    let mut pattern = parts.join("[\\\\/]");
    if query.ends_with('/') {
        pattern += "[\\\\/]";
    }
    pattern
}

/// The best of `fd`'s `output` for `query`, as Scramjet ranks them: by `score`, at most 20, each
/// shown under `directory` when the query named one, and quoted when it has a space or the
/// mention opened a quote.
fn offered(output: &str, query: &str, directory: Option<&str>, quoted: bool) -> Vec<Suggestion> {
    let mut found: Vec<(u32, &str, bool)> = output
        .lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let is_directory = line.ends_with('/');
            let path = line.strip_suffix('/').unwrap_or(line);
            if path == ".git" || path.starts_with(".git/") || path.contains("/.git/") {
                return None;
            }
            let score = match query.is_empty() {
                true => 1,
                false => score(path, query, is_directory),
            };
            (score > 0).then_some((score, path, is_directory))
        })
        .collect();
    found.sort_by_key(|&(score, ..)| std::cmp::Reverse(score));
    found
        .into_iter()
        .take(OFFERED)
        .map(|(_, path, is_directory)| {
            let shown = format!("{}{path}", directory.unwrap_or(""));
            let slash = if is_directory { "/" } else { "" };
            let name = path.rsplit('/').next().unwrap_or(path);
            let value = match quoted || shown.contains(' ') {
                true => format!("@\"{shown}{slash}\""),
                false => format!("@{shown}{slash}"),
            };
            Suggestion {
                value,
                label: format!("{name}{slash}"),
                description: shown,
            }
        })
        .collect()
}

/// How well `path` matches `query`, as Scramjet's `scoreEntry` rates it: its name equal to the
/// query, starting with it, containing it, or the path containing it; directories first.
fn score(path: &str, query: &str, is_directory: bool) -> u32 {
    let query = query.to_lowercase();
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let score = if name == query {
        100
    } else if name.starts_with(&query) {
        80
    } else if name.contains(&query) {
        50
    } else if path.to_lowercase().contains(&query) {
        30
    } else {
        0
    };
    score + if is_directory && score > 0 { 10 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, time::Duration};

    fn mention(start: usize, query: &str, quoted: bool) -> Option<Mention> {
        Some(Mention {
            start,
            query: query.into(),
            quoted,
        })
    }

    #[test]
    fn a_mention_is_the_at_word_before_the_cursor() {
        assert_eq!(Mention::find("see @src/ag"), mention(4, "src/ag", false));
        assert_eq!(Mention::find("@"), mention(0, "", false));
        assert_eq!(Mention::find("one\n@two"), mention(4, "two", false));
        assert_eq!(Mention::find("see @\"my fi"), mention(4, "my fi", true));
        assert_eq!(Mention::find("x=@a"), mention(2, "a", false));
        // A quote that no `@` opens ends the word as other delimiters do.
        assert_eq!(Mention::find("say \"@foo"), mention(5, "foo", false));
        assert_eq!(Mention::find("a@b"), None);
        assert_eq!(Mention::find("@a b"), None);
        assert_eq!(Mention::find("\"x\" @y"), mention(4, "y", false));
        assert_eq!(Mention::find("é\"@x"), mention(3, "x", false));

        // Typed, only after a space, a tab or at a line's start.
        assert_eq!(Mention::typed("see @a"), mention(4, "a", false));
        assert_eq!(Mention::typed("x\n@a"), mention(2, "a", false));
        assert_eq!(Mention::typed("x=@a"), None);
        assert_eq!(Mention::typed("say \"@foo"), None);
    }

    #[test]
    fn paths_become_patterns_of_escaped_parts() {
        assert_eq!(pattern("a.b"), "a.b");
        assert_eq!(pattern("sr/a.b"), "sr[\\\\/]a\\.b");
        assert_eq!(pattern("/x//y/"), "x[\\\\/]y[\\\\/]");
        assert_eq!(pattern("/"), "/");
    }

    #[test]
    fn entries_rank_by_name_then_path_with_directories_first() {
        let output = "tests/agent_editor_pty.py\nsrc/agent_cli/editor.rs\nsrc/edit/\nedit\n\
                      .git/edit\nsub/.git/edit\nother.txt\nnotes/my edit.md\n";
        let found = offered(output, "edit", None, false);
        let shown: Vec<_> = found
            .iter()
            .map(|item| {
                (
                    item.value.as_str(),
                    item.label.as_str(),
                    item.description.as_str(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("@src/edit/", "edit/", "src/edit"),
                ("@edit", "edit", "edit"),
                (
                    "@src/agent_cli/editor.rs",
                    "editor.rs",
                    "src/agent_cli/editor.rs"
                ),
                (
                    "@tests/agent_editor_pty.py",
                    "agent_editor_pty.py",
                    "tests/agent_editor_pty.py"
                ),
                ("@\"notes/my edit.md\"", "my edit.md", "notes/my edit.md"),
            ]
        );
        // Under a directory the query named, and quoted when the mention opened a quote.
        let found = offered("b.rs\nsub/\n", "", Some("~/a/"), true);
        let values: Vec<_> = found.iter().map(|item| item.value.as_str()).collect();
        assert_eq!(values, ["@\"~/a/b.rs\"", "@\"~/a/sub/\""]);
        let many: String = (0..30).map(|n| format!("f{n}\n")).collect();
        assert_eq!(offered(&many, "", None, false).len(), 20);
    }

    #[test]
    fn fd_finds_the_entries_of_the_project_or_a_named_directory() {
        let root = std::env::temp_dir().join(format!("sjrs-file-search-{}", std::process::id()));
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _scratch = Scratch(root.clone());
        fs::create_dir_all(root.join("src/deep")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        for file in [
            "src/deep/notes.md",
            "notes.txt",
            ".hidden-notes",
            ".git/notes",
        ] {
            fs::write(root.join(file), "").unwrap();
        }
        let found = |query: &str| {
            let mut search = Search::start(&root, Mention::find(&format!("@{query}")).unwrap())
                .expect("fd is installed");
            loop {
                if let Some(found) = search.poll() {
                    let mut values: Vec<_> = found.into_iter().map(|item| item.value).collect();
                    values.sort();
                    return values;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        assert_eq!(
            found("notes"),
            ["@.hidden-notes", "@notes.txt", "@src/deep/notes.md"]
        );
        assert_eq!(found("src/de"), ["@src/deep/"]);
        // A directory that does not exist is part of the pattern, matched on the whole path.
        assert_eq!(found("rc/deep"), ["@src/deep/", "@src/deep/notes.md"]);
        assert_eq!(found("missing/deep"), Vec::<String>::new());
        assert_eq!(found("nothing-matches"), Vec::<String>::new());
        assert_eq!(found("a("), Vec::<String>::new());
    }
}
