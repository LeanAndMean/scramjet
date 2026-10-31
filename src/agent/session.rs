use super::compaction::FileLists;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

/// An append-only JSON-lines log: a header, then each input item, each model change and each
/// compaction, which replaces the items before `first_kept` with its summary and lists the files
/// read and modified before then.
pub struct Session {
    path: PathBuf,
    file: File,
    /// The latest model change or response item producer in the file, as Scramjet restores it.
    model: Option<String>,
    /// The file lists of the latest compaction, which the next one carries on.
    files: FileLists,
}

fn compacted_files(compaction: &Value) -> FileLists {
    let paths = |key: &str| {
        compaction[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|path| path.as_str().map(str::to_owned))
            .collect()
    };
    FileLists {
        read: paths("read_files"),
        modified: paths("modified_files"),
    }
}

fn invalid(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: {reason}", path.display()),
    )
}

/// `$XDG_STATE_HOME/scramjet-rs/sessions/<project path with / as ->`.
pub fn directory(root: &Path) -> io::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| io::Error::other("neither XDG_STATE_HOME nor HOME is set"))?;
    let slug = root.to_string_lossy().trim_matches('/').replace('/', "-");
    Ok(state.join("scramjet-rs/sessions").join(slug))
}

impl Session {
    /// Starts a session for the project `root` in `directory`.
    pub fn create(directory: &Path, root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = directory.join(format!("{created}-{}.jsonl", std::process::id()));
        let file = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&path)?;
        let mut session = Self {
            path,
            file,
            model: None,
            files: FileLists::default(),
        };
        session
            .append(&json!({"type":"session","version":1,"root":root,"created":created as u64}))?;
        Ok(session)
    }

    /// The most recently created session for `root`, if any.
    pub fn latest(root: &Path) -> io::Result<Option<PathBuf>> {
        let directory = directory(root)?;
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut latest: Option<(u128, PathBuf)> = None;
        for entry in entries {
            let path = entry?.path();
            let Some(created) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".jsonl"))
                .and_then(|n| n.split('-').next())
                .and_then(|n| n.parse::<u128>().ok())
            else {
                continue;
            };
            if latest.as_ref().is_none_or(|(best, _)| created > *best) {
                latest = Some((created, path));
            }
        }
        Ok(latest.map(|(_, path)| path))
    }

    /// Opens an existing session for appending and returns its conversation items and every
    /// prompt the user gave in it, including those a compaction summarized.
    pub fn open(path: &Path) -> io::Result<(Self, Vec<Value>, Vec<String>)> {
        let reader = BufReader::new(File::open(path)?);
        let mut items = Vec::new();
        let mut prompts = Vec::new();
        let mut model = None;
        let mut files = FileLists::default();
        for (index, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&line).map_err(|_| invalid(path, "malformed line"))?;
            if index == 0 {
                if value["type"] != "session" {
                    return Err(invalid(path, "not a scramjet-rs session"));
                }
            } else if value["type"] == "compaction" {
                let cut = value["first_kept"]
                    .as_u64()
                    .and_then(|cut| usize::try_from(cut).ok())
                    .filter(|&cut| cut <= items.len())
                    .ok_or_else(|| invalid(path, "invalid compaction cut point"))?;
                let summary = value["summary"]
                    .as_str()
                    .ok_or_else(|| invalid(path, "missing compaction summary"))?;
                items.drain(..cut);
                items.insert(0, super::compaction::summary_message(summary));
                files = compacted_files(&value);
            } else if value["type"] == "model_change" {
                let changed = value["model"]
                    .as_str()
                    .ok_or_else(|| invalid(path, "missing model in a model change"))?;
                model = Some(changed.to_owned());
            } else {
                if let Some(producer) = value[super::PRODUCER].as_str() {
                    model = Some(producer.to_owned());
                }
                if value["role"] == "user"
                    && let Some(prompt) = value["content"].as_str()
                {
                    prompts.push(prompt.to_owned());
                }
                items.push(value);
            }
        }
        if items.is_empty() && std::fs::metadata(path)?.len() == 0 {
            return Err(invalid(path, "empty session file"));
        }
        let file = OpenOptions::new().append(true).open(path)?;
        Ok((
            Self {
                path: path.to_owned(),
                file,
                model,
                files,
            },
            items,
            prompts,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stable across resumes: the file name without its extension.
    pub fn id(&self) -> String {
        self.path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub(super) fn compacted_files(&self) -> &FileLists {
        &self.files
    }

    /// Records that later requests use `model`, unless the file already says so.
    pub fn record_model(&mut self, model: &str) -> io::Result<()> {
        if self.model.as_deref() != Some(model) {
            self.append(&json!({"type":"model_change","model":model}))?;
        }
        Ok(())
    }

    pub fn append(&mut self, item: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(item)?;
        line.push(b'\n');
        self.file.write_all(&line)?;
        if item["type"] == "model_change" {
            self.model = item["model"].as_str().map(str::to_owned);
        } else if item["type"] == "compaction" {
            self.files = compacted_files(item);
        } else if let Some(producer) = item[super::PRODUCER].as_str() {
            self.model = Some(producer.to_owned());
        }
        Ok(())
    }
}
