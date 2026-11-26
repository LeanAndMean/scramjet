//! What `/resume` lists of each project's session files, kept in the state directory by each
//! file's size and modification time, so that only the files changed since are read again. An
//! index that cannot be read is rebuilt; an entry is used only while its file's size and time
//! are those it was read at.
use super::SessionInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};

const VERSION: u32 = 2;

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Index {
    version: u32,
    /// By file name.
    files: HashMap<String, Indexed>,
    #[serde(skip)]
    changed: bool,
}

#[derive(Serialize, Deserialize)]
struct Indexed {
    size: u64,
    info: SessionInfo,
}

/// The index of the sessions in `directory`, kept for a project's directory in the state
/// directory only: `--session-dir`'s and unsaved runs' are read whole each time.
pub(super) fn file(directory: &Path) -> Option<PathBuf> {
    let projects = super::projects_directory().ok()?;
    if directory.parent()? != projects {
        return None;
    }
    let name = directory.file_name()?.to_str()?;
    let state = projects.parent()?;
    Some(state.join("session-cache").join(format!("{name}.json")))
}

impl Index {
    pub(super) fn load(file: &Path) -> Self {
        let index = std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok());
        index
            .filter(|index| index.version == VERSION)
            .unwrap_or_default()
    }

    /// What it holds of the file `path` while its size and time are `metadata`'s.
    pub(super) fn get(&self, path: &Path, metadata: &Metadata) -> Option<SessionInfo> {
        let indexed = self.files.get(path.file_name()?.to_str()?)?;
        let fresh = indexed.size == metadata.len()
            && metadata.modified().ok() == Some(indexed.info.modified)
            && indexed.info.path == path;
        fresh.then(|| indexed.info.clone())
    }

    /// Keeps `info`, read from a file of `size` bytes.
    pub(super) fn put(&mut self, info: &SessionInfo, size: u64) {
        let Some(name) = info.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let info = info.clone();
        self.files.insert(name.to_owned(), Indexed { size, info });
        self.changed = true;
    }

    /// Forgets the files not among `names`, deleted since.
    pub(super) fn keep_only(&mut self, names: &[&str]) {
        let before = self.files.len();
        self.files.retain(|name, _| names.contains(&name.as_str()));
        self.changed |= self.files.len() != before;
    }

    /// Writes it to `file` if it changed since it was loaded, private, whole or not at all.
    pub(super) fn save(&mut self, file: &Path) -> io::Result<()> {
        if !self.changed {
            return Ok(());
        }
        self.version = VERSION;
        let directory = file.parent().unwrap_or(Path::new("."));
        crate::os::private_directory(std::fs::DirBuilder::new().recursive(true))
            .create(directory)?;
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("index");
        // A list read on a thread of its own may save while another is saved.
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let save = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = std::process::id();
        let temporary = directory.join(format!(".{name}.{id}.{save}.tmp"));
        let written = crate::os::private_file(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        )
        .open(&temporary)
        .and_then(|output| {
            let mut output = io::BufWriter::new(output);
            serde_json::to_writer(&mut output, self)?;
            io::Write::flush(&mut output)
        })
        .and_then(|()| std::fs::rename(&temporary, file));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        self.changed = false;
        written
    }
}

#[cfg(test)]
mod tests {
    use super::super::Session;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn write(path: &Path, prompt: &str, modified: SystemTime) {
        let header = r#"{"type":"session","version":1}"#;
        let line = serde_json::json!({"role":"user","content":prompt});
        std::fs::write(path, format!("{header}\n{line}\n")).unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(modified).unwrap();
    }

    fn prompts(directory: &Path, index: &Path) -> Vec<String> {
        let paths = (std::fs::read_dir(directory).unwrap())
            .map(|entry| entry.unwrap().path())
            .collect();
        let (infos, _) = Session::indexed_infos(paths, |_| Some(index.to_owned()), usize::MAX);
        let prompts = infos.into_iter().map(|info| info.first_prompt.unwrap());
        prompts.collect()
    }

    #[test]
    fn unchanged_files_are_listed_from_the_index_and_changed_ones_read_again() {
        let scratch = std::env::temp_dir().join(format!("scramjet-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let directory = scratch.join("project");
        std::fs::create_dir_all(&directory).unwrap();
        let index = scratch.join("cache").join("project.json");
        let now = SystemTime::now();
        let (first, second): (PathBuf, PathBuf) = (
            directory.join("1000-1.jsonl"),
            directory.join("2000-1.jsonl"),
        );
        write(&first, "first", now - Duration::from_secs(60));
        write(&second, "second", now);
        assert_eq!(prompts(&directory, &index), ["second", "first"]);
        assert!(index.exists());

        // The index's entry is used while its file's size and time are unchanged; this one's
        // says what no file says, which shows it was used.
        let text = std::fs::read_to_string(&index).unwrap();
        let altered = text.replace("\"first\"", "\"from the index\"");
        std::fs::write(&index, altered).unwrap();
        assert_eq!(prompts(&directory, &index), ["second", "from the index"]);

        // A file written since is read again, here one of the same size but a later time.
        write(&first, "again", now - Duration::from_secs(60));
        std::fs::File::options()
            .append(true)
            .open(&first)
            .unwrap()
            .set_modified(now - Duration::from_secs(30))
            .unwrap();
        assert_eq!(prompts(&directory, &index), ["second", "again"]);

        // A deleted file leaves the index; one that cannot be read is rebuilt.
        std::fs::remove_file(&second).unwrap();
        assert_eq!(prompts(&directory, &index), ["again"]);
        assert!(!std::fs::read_to_string(&index).unwrap().contains("2000-1"));
        std::fs::write(&index, "{not json").unwrap();
        assert_eq!(prompts(&directory, &index), ["again"]);
        assert!(std::fs::read_to_string(&index).unwrap().contains("1000-1"));
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_limited_read_takes_the_newest_unindexed_files_and_counts_the_rest() {
        let scratch = std::env::temp_dir().join(format!("scramjet-soon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let directory = scratch.join("project");
        std::fs::create_dir_all(&directory).unwrap();
        let index = scratch.join("cache").join("project.json");
        let now = SystemTime::now();
        for (at, prompt) in ["old", "middle", "new"].iter().enumerate() {
            let path = directory.join(format!("{at}000-1.jsonl"));
            write(
                &path,
                prompt,
                now - Duration::from_secs(60 - at as u64 * 10),
            );
        }
        let paths = || {
            (std::fs::read_dir(&directory).unwrap())
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>()
        };
        let read = |at_most| {
            let (infos, left) = Session::indexed_infos(paths(), |_| Some(index.clone()), at_most);
            let prompts: Vec<String> = (infos.into_iter())
                .map(|info| info.first_prompt.unwrap())
                .collect();
            (prompts, left)
        };
        assert_eq!(read(1), (vec!["new".to_owned()], 2));
        // What was read is indexed, so the next read reaches further.
        assert_eq!(read(1), (vec!["new".to_owned(), "middle".to_owned()], 1));
        assert_eq!(
            read(usize::MAX),
            (["new", "middle", "old"].map(String::from).to_vec(), 0)
        );
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
