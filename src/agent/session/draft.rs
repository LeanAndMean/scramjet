//! Where the full screen keeps each saved session's unsent prompt: `drafts` in the state
//! directory, one file per session, in a directory named as the session's. What a file holds is
//! the editor's.

use std::{
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

const SUFFIX: &str = ".json";

/// The draft file of the session saved at `session`; `None` for a session of a run that saves
/// none (`--no-session`) or without a state directory.
pub fn file(session: &Path) -> Option<PathBuf> {
    if super::Unsaved::holds(session) {
        return None;
    }
    let state = super::state_directory().ok()?;
    let projects = super::projects_directory().ok()?;
    file_in(&state.join("drafts"), &projects, session)
}

fn file_in(drafts: &Path, projects: &Path, session: &Path) -> Option<PathBuf> {
    let session = std::path::absolute(session).ok()?;
    let directory = session.parent()?;
    let stem = session.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    let named = match directory.strip_prefix(projects) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_owned(),
        _ => PathBuf::from(super::slug(&directory.to_string_lossy())),
    };
    Some(drafts.join(named).join(format!("{stem}{SUFFIX}")))
}

/// The draft kept for `session`, if any.
pub fn read(session: &Path) -> io::Result<Option<Vec<u8>>> {
    let Some(file) = file(session) else {
        return Ok(None);
    };
    match std::fs::read(file) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Keeps `bytes` as the draft of `session`, replacing the one before whole; `None` removes it.
pub fn write(session: &Path, bytes: Option<&[u8]>) -> io::Result<()> {
    match file(session) {
        Some(file) => write_file(&file, bytes),
        None => Ok(()),
    }
}

fn write_file(file: &Path, bytes: Option<&[u8]>) -> io::Result<()> {
    let Some(bytes) = bytes else {
        return match std::fs::remove_file(file) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    };
    let directory = file.parent().unwrap_or(Path::new("."));
    crate::os::private_directory(std::fs::DirBuilder::new().recursive(true)).create(directory)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let temporary = directory.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = crate::os::private_file(
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true),
    )
    .open(&temporary)
    .and_then(|mut output| io::Write::write_all(&mut output, bytes))
    .and_then(|()| std::fs::rename(&temporary, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The newest draft kept for a session in `directory` that was never written, as a session
/// is not before its first prompt: the session's path, and when the draft was kept.
pub fn newest_unwritten(directory: &Path) -> Option<(PathBuf, SystemTime)> {
    let probe = directory.join("0.jsonl");
    let drafts = file(&probe)?.parent()?.to_owned();
    newest_unwritten_in(&drafts, directory)
}

fn newest_unwritten_in(drafts: &Path, directory: &Path) -> Option<(PathBuf, SystemTime)> {
    let entries = std::fs::read_dir(drafts).ok()?;
    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(stem) = name.to_str().and_then(|name| name.strip_suffix(SUFFIX)) else {
            continue;
        };
        if stem.starts_with('.') {
            continue;
        }
        let session = directory.join(format!("{stem}.jsonl"));
        if std::fs::symlink_metadata(&session).is_ok() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(_, at)| modified > *at) {
            newest = Some((session, modified));
        }
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;

    fn scratch(name: &str) -> ScratchDir {
        let name = format!("scramjet-draft-{name}-{}", std::process::id());
        ScratchDir::new(std::env::temp_dir().join(name))
    }

    #[test]
    fn a_draft_is_kept_beside_its_sessions_project_and_replaced_whole() {
        let scratch = scratch("kept");
        let (drafts, projects) = (scratch.0.join("drafts"), scratch.0.join("sessions"));
        let session = projects.join("home-me-repo").join("17-42.jsonl");
        let file = file_in(&drafts, &projects, &session).unwrap();
        assert_eq!(file, drafts.join("home-me-repo").join("17-42.json"));
        // A session elsewhere (`--session-dir`) is named by its directory's whole path.
        let elsewhere = scratch.0.join("mine").join("17-43.jsonl");
        let other = file_in(&drafts, &projects, &elsewhere).unwrap();
        assert_eq!(other.parent().unwrap().parent().unwrap(), drafts);
        assert!(other.ends_with("17-43.json"));
        assert_eq!(file_in(&drafts, &projects, Path::new("/notes.txt")), None);

        write_file(&file, Some(b"first")).unwrap();
        write_file(&file, Some(b"second")).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"second");
        let left: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().collect();
        assert_eq!(left.len(), 1, "no temporary file is left");
        write_file(&file, None).unwrap();
        assert!(!file.exists());
        write_file(&file, None).unwrap();
    }

    #[test]
    fn the_newest_draft_of_a_session_never_written_is_found() {
        let scratch = scratch("unwritten");
        let (drafts, directory) = (scratch.0.join("drafts"), scratch.0.join("project"));
        std::fs::create_dir_all(&drafts).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        assert_eq!(newest_unwritten_in(&drafts, &directory), None);
        let keep = |name: &str, at: u64| {
            let file = drafts.join(name);
            std::fs::write(&file, "{}").unwrap();
            let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(at);
            std::fs::File::options()
                .write(true)
                .open(&file)
                .unwrap()
                .set_modified(at)
                .unwrap();
        };
        keep("1-1.json", 1_000);
        keep("2-1.json", 3_000);
        keep("3-1.json", 2_000);
        keep(".4-1.json.7.tmp", 4_000);
        // A session written has its draft read as it opens.
        std::fs::write(directory.join("2-1.jsonl"), "").unwrap();
        let (session, _) = newest_unwritten_in(&drafts, &directory).unwrap();
        assert_eq!(session, directory.join("3-1.jsonl"));
    }
}
