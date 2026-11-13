//! `@` file references in the prompt, as Scramjet's editor completes them: a walk as Scramjet's
//! `fd` call makes it lists the entries under the project (or a directory the word names) whose names match what follows the
//! `@`, and the best matches are offered. The text is sent as typed; the model reads the files.

use super::completion::Suggestion;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, TryRecvError},
    },
};

/// Characters that end a word, as Scramjet's autocomplete counts them.
pub(super) const DELIMITERS: [char; 5] = [' ', '\t', '"', '\'', '='];
/// How many entries the walk may list, and how many of the best are offered, as in Scramjet.
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

/// A walk looking for a mention's entries, until it ends or this is dropped.
pub(super) struct Search {
    pub(super) mention: Mention,
    /// What the walk looks for: the query, or its last part in the directory the rest names.
    query: String,
    /// That directory as typed, ending with `/`.
    directory: Option<String>,
    stop: Arc<AtomicBool>,
    output: mpsc::Receiver<String>,
}

impl Search {
    /// Starts looking in `root`, or in the directory the mention's query names, as Scramjet's
    /// `getFuzzyFileSuggestions` runs `fd`.
    pub(super) fn start(root: &Path, mention: Mention) -> Self {
        let (base, query, directory) = scope(root, &mention.query);
        let stop = Arc::new(AtomicBool::new(false));
        let (send, output) = mpsc::channel();
        let (walking, wanted) = (stop.clone(), query.clone());
        std::thread::spawn(move || {
            let _ = send.send(entries(&base, &wanted, &walking));
        });
        Self {
            mention,
            query,
            directory,
            stop,
            output,
        }
    }

    /// The entries offered once the walk has ended, best first; none when it failed.
    pub(super) fn poll(&mut self) -> Option<Vec<Suggestion>> {
        let output = match self.output.try_recv() {
            Ok(output) => output,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => String::new(),
        };
        Some(offered(
            &output,
            &self.query,
            self.directory.as_deref(),
            self.mention.quoted,
        ))
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The entries offered for what follows an `@` (and the quote it may open), found as the
/// editor finds them, waiting for the walk.
pub(super) fn lookup(root: &Path, query: &str, quoted: bool) -> Vec<Suggestion> {
    let (base, query, directory) = scope(root, query);
    let output = entries(&base, &query, &AtomicBool::new(false));
    offered(&output, &query, directory.as_deref(), quoted)
}

/// Where a query looks: `root`, or the directory its part up to a last `/` names (`~/` the home
/// directory), with what is left to match and that directory as typed.
fn scope(root: &Path, query: &str) -> (PathBuf, String, Option<String>) {
    let typed = query.replace('\\', "/");
    let scoped = typed.rfind('/').and_then(|slash| {
        let (shown, query) = typed.split_at(slash + 1);
        let base = match shown.strip_prefix("~/") {
            Some(rest) => std::env::home_dir()?.join(rest),
            None => root.join(shown),
        };
        base.is_dir()
            .then(|| (base, query.to_owned(), shown.to_owned()))
    });
    match scoped {
        Some((base, query, shown)) => (base, query, Some(shown)),
        None => (root.to_path_buf(), typed, None),
    }
}

/// The files and directories under `base` matching `query`, one a line relative to `base`,
/// directories ending with `/`, as `fd --follow --hidden --type f --type d --exclude .git` lists
/// them: the ignore files honored, the name matched (the whole path for a query with a `/`) by
/// `pattern`, ignoring case unless the query has a capital. Empty for a pattern that is not a
/// regular expression.
fn entries(base: &Path, query: &str, stop: &AtomicBool) -> String {
    let regex = regex::RegexBuilder::new(&pattern(query))
        .case_insensitive(!query.chars().any(char::is_uppercase))
        .build();
    let Ok(regex) = regex else {
        return String::new();
    };
    let mut walk = ignore::WalkBuilder::new(base);
    walk.hidden(false)
        .follow_links(true)
        .add_custom_ignore_filename(".fdignore")
        .sort_by_file_name(Ord::cmp)
        .filter_entry(|entry| entry.file_name() != ".git");
    let (mut found, mut listed) = (String::new(), 0);
    for entry in walk.build().flatten() {
        if stop.load(Ordering::Relaxed) || listed == FOUND {
            break;
        }
        let Some(kind) = entry.file_type().filter(|_| entry.depth() > 0) else {
            continue;
        };
        if !kind.is_file() && !kind.is_dir() {
            continue;
        }
        let matched: PathBuf = match query.contains('/') {
            true => entry.path().to_owned(),
            false => entry.file_name().into(),
        };
        if !query.is_empty() && !regex.is_match(&matched.to_string_lossy()) {
            continue;
        }
        let relative = entry.path().strip_prefix(base).unwrap_or(entry.path());
        found += &relative.to_string_lossy();
        found += if kind.is_dir() { "/\n" } else { "\n" };
        listed += 1;
    }
    found
}

/// The pattern for `query`, as Scramjet's `buildFdPathQuery` makes it: as typed, or for a
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

/// The best of the walk's `output` for `query`, as Scramjet ranks them: by `score`, at most 20, each
/// shown under `directory` when the query named one, and quoted when it has a space or the
/// mention opened a quote. Windows' `\` are shown as `/`, as Scramjet shows them.
fn offered(output: &str, query: &str, directory: Option<&str>, quoted: bool) -> Vec<Suggestion> {
    let output = output.replace('\\', "/");
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
        // Windows' separators, shown as `/`, and `.git` left out there too.
        let found = offered("src\\a.rs\nsrc\\sub\\\n.git\\HEAD\n", "", None, false);
        let values: Vec<_> = found.iter().map(|item| item.value.as_str()).collect();
        assert_eq!(values, ["@src/a.rs", "@src/sub/"]);
        assert_eq!(found[1].label, "sub/");
        let many: String = (0..30).map(|n| format!("f{n}\n")).collect();
        assert_eq!(offered(&many, "", None, false).len(), 20);
    }

    #[test]
    fn a_walk_finds_the_entries_of_the_project_or_a_named_directory() {
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
            let mut search = Search::start(&root, Mention::find(&format!("@{query}")).unwrap());
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
