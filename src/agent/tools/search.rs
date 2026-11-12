//! `grep` and `find` in-process, with ripgrep's libraries, answering as the `rg` and `fd` calls
//! of Pi's tools do: nothing to download or run, and no sandbox needed to keep out of the
//! credential stores, which a walk skips and a file opened is checked against.

use super::super::sandbox::Hidden;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const ABORTED: &str = "Operation aborted";

/// A line `grep` found: its file, its number and its text, invalid UTF-8 replaced.
pub(super) struct Matched {
    pub file: PathBuf,
    pub number: usize,
    pub text: String,
}

/// What `grep` looks for, as Pi's call of `rg` takes it.
pub(super) struct Grep<'a> {
    pub pattern: &'a str,
    pub ignore_case: bool,
    pub literal: bool,
    pub glob: Option<&'a str>,
    pub limit: usize,
}

/// A walk of `path` as `rg --hidden` and `fd --hidden` make it: hidden entries included, the
/// ignore files honored, symlinks not followed, in name order. With `hidden`, the stores are
/// left out, whatever leads to them.
fn walk(path: &Path, hidden: Option<&Hidden>, require_git: bool, ignore_file: &str) -> WalkBuilder {
    let mut builder = WalkBuilder::new(path);
    builder
        .hidden(false)
        .follow_links(false)
        .require_git(require_git)
        .add_custom_ignore_filename(ignore_file)
        .sort_by_file_name(Ord::cmp);
    if let Some(hidden) = hidden {
        let hidden = hidden.clone();
        let given = path.to_owned();
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| given.clone());
        builder.filter_entry(move |entry| {
            let real = match entry.path().strip_prefix(&given) {
                Ok(inside) => real.join(inside),
                Err(_) => entry.path().to_owned(),
            };
            !(entry.metadata()).is_ok_and(|metadata| hidden.is_store(&real, &metadata))
        });
    }
    builder
}

/// The lines matching in `path` (relative globs from `root`, as `rg` run there takes them), at
/// most `limit`, and whether there were more.
pub(super) fn grep(
    root: &Path,
    path: &Path,
    options: &Grep,
    hidden: Option<&Hidden>,
    cancel: &AtomicBool,
) -> Result<(Vec<Matched>, bool), String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(options.ignore_case)
        .fixed_strings(options.literal)
        .multi_line(true)
        .line_terminator(Some(b'\n'))
        .build(options.pattern)
        .map_err(|e| e.to_string())?;
    let mut builder = walk(path, hidden, true, ".rgignore");
    if let Some(glob) = options.glob {
        let mut overrides = ignore::overrides::OverrideBuilder::new(root);
        overrides.add(glob).map_err(|e| e.to_string())?;
        builder.overrides(overrides.build().map_err(|e| e.to_string())?);
    }
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let (mut found, mut limited, mut failed) = (Vec::new(), false, None);
    for entry in builder.build() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ABORTED.into());
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failed.get_or_insert(error.to_string());
                continue;
            }
        };
        let kind = entry.file_type();
        // As rg: a file given is searched whole, also when ignored or binary; one walked to only
        // if a regular file, and up to a NUL byte.
        let given = entry.depth() == 0;
        let directory = match given {
            true => entry.path().is_dir(),
            false => kind.is_some_and(|kind| kind.is_dir()),
        };
        if directory || !given && !kind.is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let file = match super::regular_file(entry.path(), hidden) {
            Ok(file) => file,
            Err(error) if given => return Err(error),
            Err(error) => {
                failed.get_or_insert(error);
                continue;
            }
        };
        searcher.set_binary_detection(match given {
            true => BinaryDetection::none(),
            false => BinaryDetection::quit(b'\0'),
        });
        let mut sink = Collect {
            file: entry.path(),
            found: &mut found,
            limit: options.limit,
            limited: &mut limited,
            cancel,
        };
        if let Err(error) = searcher.search_file(&matcher, &file, &mut sink) {
            let error = format!("{}: {error}", entry.path().display());
            match given {
                true => return Err(error),
                false => failed.get_or_insert(error),
            };
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(ABORTED.into());
        }
        if limited {
            break;
        }
    }
    match failed {
        Some(error) if found.is_empty() => Err(error),
        _ => Ok((found, limited)),
    }
}

struct Collect<'a> {
    file: &'a Path,
    found: &'a mut Vec<Matched>,
    limit: usize,
    limited: &'a mut bool,
    cancel: &'a AtomicBool,
}

impl Sink for Collect<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, found: &SinkMatch) -> Result<bool, std::io::Error> {
        // A match after the last one shown tells that there are more.
        if self.found.len() == self.limit {
            *self.limited = true;
            return Ok(false);
        }
        self.found.push(Matched {
            file: self.file.to_owned(),
            number: found.line_number().unwrap_or(0) as usize,
            text: String::from_utf8_lossy(found.bytes()).into_owned(),
        });
        Ok(!self.cancel.load(Ordering::Relaxed))
    }
}

/// The entries in `path` whose name matches the glob `pattern` (or, with a `/` in it, whose whole
/// path does), as `fd --glob` finds them: shown as `super::shown` shows them, directories ending
/// with `/`, at most `limit`.
pub(super) fn find(
    root: &Path,
    path: &Path,
    pattern: &str,
    limit: usize,
    hidden: Option<&Hidden>,
    cancel: &AtomicBool,
) -> Result<Vec<String>, String> {
    let full = pattern.contains('/');
    // As Pi: a path pattern is matched against the absolute path, so it gets a leading `**/`.
    let pattern = match full && !pattern.starts_with('/') && !pattern.starts_with("**/") {
        true if pattern != "**" => format!("**/{pattern}"),
        _ => pattern.to_owned(),
    };
    let glob = globset::GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| e.to_string())?
        .compile_matcher();
    let (mut found, mut failed) = (Vec::new(), None);
    for entry in walk(path, hidden, false, ".fdignore").build() {
        if cancel.load(Ordering::Relaxed) {
            return Err(ABORTED.into());
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                failed.get_or_insert(error.to_string());
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let matched = match full {
            true => glob.is_match(entry.path()),
            false => glob.is_match(entry.file_name()),
        };
        if !matched {
            continue;
        }
        let slash = match entry.file_type().is_some_and(|kind| kind.is_dir()) {
            true => "/",
            false => "",
        };
        found.push(format!("{}{slash}", super::shown(root, entry.path())));
        if found.len() == limit {
            break;
        }
    }
    match failed {
        Some(error) if found.is_empty() => Err(error),
        _ => Ok(found),
    }
}
