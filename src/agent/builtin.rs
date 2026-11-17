//! What the program carries with it, as Scramjet 1's npm package carries its `scramjet` set and
//! README: the `scramjet` command set (`/scramjet:troubleshoot`, the command-authoring agents and
//! skill) and the README its instructions point to, with the guides it links. A build run from
//! its checkout uses the checkout's files; a release, which has none, writes its own under
//! `$XDG_DATA_HOME/scramjet/builtin` and uses those.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The checkout this program was built from.
const CHECKOUT: &str = env!("CARGO_MANIFEST_DIR");

macro_rules! set_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../sets/scramjet/", $path)))),*]
    };
}

/// The `scramjet` set's files, by their paths in the set.
const SET: &[(&str, &str)] = set_files![
    "set.yaml",
    "commands/troubleshoot.md",
    "agents/command-architect.md",
    "agents/command-failure-analyst.md",
    "agents/command-reviewer.md",
    "agents/command-set-explorer.md",
    "agents/independent-command-assessor.md",
    "agents/instruction-semantics-analyzer.md",
    "skills/reading-past-sessions/SKILL.md",
    "skills/setting-up-command-sets/SKILL.md",
    "skills/writing-scramjet-commands/SKILL.md",
];

macro_rules! doc_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../", $path)))),*]
    };
}

/// The README and the documents it links for users, by their paths in the checkout.
const DOCS: &[(&str, &str)] = doc_files![
    "README.md",
    "docs/TERMINALS.md",
    "docs/guide/acp.md",
    "docs/guide/command-sets.md",
    "docs/guide/configuration.md",
    "docs/guide/install.md",
    "docs/guide/migration.md",
    "docs/guide/providers.md",
    "docs/guide/sessions.md",
];

/// Where a release writes what it carries.
fn written() -> Option<PathBuf> {
    let data = crate::os::base_directory("XDG_DATA_HOME", ".local/share")?;
    Some(data.join("scramjet").join("builtin"))
}

/// The documents a read is of, by their paths in the checkout (`docs/guide/install.md`), when
/// `path` is the checkout's copy of one or the one a release writes.
pub(super) fn document(path: &Path) -> Option<&'static str> {
    let roots = [Some(PathBuf::from(CHECKOUT)), written()];
    DOCS.iter().map(|(relative, _)| *relative).find(|relative| {
        (roots.iter().flatten()).any(|root| path == crate::os::join(root, relative))
    })
}

/// The README for the instructions: the checkout's when it is there, else the one written for
/// this version with its documents; `None` when neither can be had.
pub(super) fn readme() -> Option<PathBuf> {
    let checkout = crate::os::join(Path::new(CHECKOUT), "README.md");
    if checkout.is_file() {
        return Some(checkout);
    }
    let root = written()?;
    write_documents(&root).ok()?;
    Some(root.join("README.md"))
}

/// Writes the documents under `root`, removing the guides of an older version, but not the
/// `scramjet` set beside them.
fn write_documents(root: &Path) -> io::Result<()> {
    write(root, DOCS, false)?;
    let kept: Vec<PathBuf> = (DOCS.iter())
        .map(|(relative, _)| crate::os::join(root, relative))
        .collect();
    remove_others(&root.join("docs"), &kept);
    Ok(())
}

/// The directory of the `scramjet` set, or why there is none.
pub(super) fn set() -> Result<PathBuf, String> {
    let checkout = crate::os::join(Path::new(CHECKOUT), "sets/scramjet");
    if checkout.join("set.yaml").is_file() {
        return Ok(checkout);
    }
    let root = written()
        .ok_or("neither XDG_DATA_HOME nor HOME is set")?
        .join("scramjet");
    write(&root, SET, true).map_err(|error| format!("{}: {error}", root.display()))?;
    Ok(root)
}

/// Writes `files` under `root` where they differ, each through a file renamed over it, so that
/// another process reads it whole; with `prune`, removes the other files there, those of an
/// older version.
fn write(root: &Path, files: &[(&str, &str)], prune: bool) -> io::Result<()> {
    for (relative, content) in files {
        let path = crate::os::join(root, relative);
        if fs::read(&path).is_ok_and(|known| known == content.as_bytes()) {
            continue;
        }
        let directory = path.parent().unwrap_or(root);
        fs::create_dir_all(directory)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
        fs::write(&temporary, content)?;
        if let Err(error) = fs::rename(&temporary, &path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    }
    if prune {
        let kept: Vec<PathBuf> = (files.iter())
            .map(|(relative, _)| crate::os::join(root, relative))
            .collect();
        remove_others(root, &kept);
    }
    Ok(())
}

/// Removes the files under `directory` not in `kept`, leaving hidden ones (another process's
/// file being written) and links alone.
fn remove_others(directory: &Path, kept: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => remove_others(&path, kept),
            Ok(kind) if kind.is_file() && !kept.contains(&path) => {
                let _ = fs::remove_file(&path);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(root: &Path) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).unwrap().map(Result::unwrap) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                let relative = relative.replace('\\', "/");
                found.push((relative, fs::read_to_string(&path).unwrap()));
            }
        }
        found.sort();
        found
    }

    #[test]
    fn the_set_carried_is_the_checkouts_whole() {
        let mut carried: Vec<(String, String)> = (SET.iter())
            .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
            .collect();
        carried.sort();
        // At run time, as Windows' test runs give the copy of the checkout they run in.
        let checkout = std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or(CHECKOUT.into());
        assert_eq!(carried, files(&Path::new(&checkout).join("sets/scramjet")));
    }

    #[test]
    fn the_documents_carried_are_the_checkouts_guides_whole() {
        let checkout = std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or(CHECKOUT.into());
        let checkout = Path::new(&checkout);
        for (path, content) in DOCS {
            let known = fs::read_to_string(checkout.join(path)).unwrap();
            assert_eq!(known, *content, "{path}");
        }
        let guides: Vec<String> = files(&checkout.join("docs/guide"))
            .into_iter()
            .map(|(path, _)| format!("docs/guide/{path}"))
            .collect();
        let carried: Vec<&str> = (DOCS.iter().map(|(path, _)| *path))
            .filter(|path| path.starts_with("docs/guide/"))
            .collect();
        assert_eq!(guides, carried);
    }

    #[test]
    fn a_release_writes_what_differs_and_removes_an_older_versions_files() {
        let root = std::env::temp_dir().join(format!("scramjet-builtin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write(&root, SET, true).unwrap();
        let written = |root: &Path| {
            let mut expected: Vec<(String, String)> = (SET.iter())
                .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
                .collect();
            expected.sort();
            assert_eq!(files(root), expected);
        };
        written(&root);

        fs::write(root.join("commands/troubleshoot.md"), "changed").unwrap();
        fs::write(root.join("agents/dropped.md"), "older").unwrap();
        fs::write(root.join("agents/.other.md.1.tmp"), "being written").unwrap();
        write(&root, SET, true).unwrap();
        assert!(root.join("agents/.other.md.1.tmp").is_file());
        fs::remove_file(root.join("agents/.other.md.1.tmp")).unwrap();
        written(&root);
        let set = crate::agent::Set::at(&root, crate::agent::Origin::BuiltIn).unwrap();
        assert_eq!(set.name, "scramjet");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_release_writes_the_documents_beside_the_set_and_removes_older_guides() {
        let root = std::env::temp_dir().join(format!("scramjet-docs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write(&root.join("scramjet"), SET, true).unwrap();
        fs::create_dir_all(root.join("docs/guide")).unwrap();
        fs::write(root.join("docs/guide/dropped.md"), "older").unwrap();
        write_documents(&root).unwrap();
        assert!(!root.join("docs/guide/dropped.md").exists());
        let documents: Vec<String> = files(&root)
            .into_iter()
            .map(|(path, _)| path)
            .filter(|path| !path.starts_with("scramjet/"))
            .collect();
        let mut expected: Vec<&str> = DOCS.iter().map(|(path, _)| *path).collect();
        expected.sort();
        assert_eq!(documents, expected);
        assert_eq!(files(&root.join("scramjet")).len(), SET.len());
        let guide = root.join("docs/guide/providers.md");
        assert_eq!(document(&guide), None, "not this program's written copy");
        fs::remove_dir_all(&root).unwrap();
    }
}
