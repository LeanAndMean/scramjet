use super::compaction::FileLists;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A saved session as `/resume` lists it.
pub struct SessionInfo {
    pub path: PathBuf,
    pub first_prompt: Option<String>,
    /// Conversation items after the latest compaction, as a resume restores them.
    pub item_count: usize,
    pub modified: SystemTime,
}

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
    /// The context size the latest response reported, unless a compaction followed it.
    context_tokens: Option<u64>,
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

/// A prompt sent to the model, which the session can be forked before.
struct ForkPoint {
    /// Where its line starts in the file.
    offset: u64,
    prompt: String,
}

/// What a session file holds once read.
struct Loaded {
    items: Vec<Value>,
    prompts: Vec<String>,
    model: Option<String>,
    files: FileLists,
    context_tokens: Option<u64>,
    /// Where the line after the header starts.
    header_end: u64,
    /// Every prompt in the file, including those a compaction summarized; not `!` commands.
    forks: Vec<ForkPoint>,
}

fn load(path: &Path) -> io::Result<Loaded> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut items = Vec::new();
    let mut prompts = Vec::new();
    let mut model = None;
    let mut files = FileLists::default();
    let mut context_tokens = None;
    let mut header_end = 0;
    let mut forks = Vec::new();
    let mut line = String::new();
    let mut end = 0;
    for index in 0.. {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        let start = end;
        end += read as u64;
        if index == 0 {
            header_end = end;
        }
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
            context_tokens = None;
        } else if value["type"] == "model_change" {
            let changed = value["model"]
                .as_str()
                .ok_or_else(|| invalid(path, "missing model in a model change"))?;
            model = Some(changed.to_owned());
        } else {
            if let Some(producer) = value[super::PRODUCER].as_str() {
                model = Some(producer.to_owned());
                context_tokens = value[super::CONTEXT].as_u64();
            }
            if value["role"] == "user" {
                if let Some(prompt) = value[super::TYPED].as_str().or(value["content"].as_str()) {
                    prompts.push(prompt.to_owned());
                }
                if value.get(super::TYPED).is_none()
                    && let Some(prompt) = value["content"].as_str()
                {
                    forks.push(ForkPoint {
                        offset: start,
                        prompt: prompt.to_owned(),
                    });
                }
            }
            items.push(value);
        }
    }
    if items.is_empty() && std::fs::metadata(path)?.len() == 0 {
        return Err(invalid(path, "empty session file"));
    }
    Ok(Loaded {
        items,
        prompts,
        model,
        files,
        context_tokens,
        header_end,
        forks,
    })
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
    /// The sessions saved in `directory`, newest first. Unreadable and invalid files are skipped.
    pub fn list(directory: &Path) -> io::Result<Vec<SessionInfo>> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut sessions = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(loaded) = load(&path) else {
                continue;
            };
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            sessions.push(SessionInfo {
                path,
                first_prompt: loaded.prompts.into_iter().next(),
                item_count: loaded.items.len(),
                modified,
            });
        }
        sessions.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| b.path.cmp(&a.path))
        });
        Ok(sessions)
    }

    /// Starts a session for the project `root` in `directory`.
    pub fn create(directory: &Path, root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let mut created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        // A session created in the same millisecond (a quick `/new` or `/fork`) has that name.
        let (path, file) = loop {
            let path = directory.join(format!("{created}-{}.jsonl", std::process::id()));
            match OpenOptions::new().append(true).create_new(true).open(&path) {
                Ok(file) => break (path, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => created += 1,
                Err(error) => return Err(error),
            }
        };
        let mut session = Self {
            path,
            file,
            model: None,
            files: FileLists::default(),
            context_tokens: None,
        };
        session
            .append(&json!({"type":"session","version":1,"root":root,"created":created as u64}))?;
        Ok(session)
    }

    /// The session in `directory` written to last, as Scramjet's `--continue` picks it, if any.
    pub fn latest(directory: &Path) -> io::Result<Option<PathBuf>> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut latest: Option<((SystemTime, u128), PathBuf)> = None;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let Some(created) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".jsonl"))
                .and_then(|n| n.split('-').next())
                .and_then(|n| n.parse::<u128>().ok())
            else {
                continue;
            };
            let key = (entry.metadata()?.modified()?, created);
            if latest.as_ref().is_none_or(|(best, _)| key > *best) {
                latest = Some((key, path));
            }
        }
        Ok(latest.map(|(_, path)| path))
    }

    /// Opens an existing session for appending and returns its conversation items and every
    /// prompt the user gave in it, including those a compaction summarized.
    pub fn open(path: &Path) -> io::Result<(Self, Vec<Value>, Vec<String>)> {
        let loaded = load(path)?;
        let file = OpenOptions::new().append(true).open(path)?;
        Ok((
            Self {
                path: path.to_owned(),
                file,
                model: loaded.model,
                files: loaded.files,
                context_tokens: loaded.context_tokens,
            },
            loaded.items,
            loaded.prompts,
        ))
    }

    /// The prompts this session can be forked before, oldest first, including those a compaction
    /// summarized, as Scramjet's `/fork` lists its user messages.
    pub fn fork_prompts(&self) -> io::Result<Vec<String>> {
        Ok(load(&self.path)?
            .forks
            .into_iter()
            .map(|fork| fork.prompt)
            .collect())
    }

    /// Saves a session for the project `root` next to this one with everything this one recorded
    /// before its prompt `index` (0-based, as `fork_prompts` lists them), as Scramjet's `/fork`
    /// copies a session's path. Returns the new file and that prompt; `None`: no such prompt.
    pub fn fork(&self, root: &Path, index: usize) -> io::Result<Option<(PathBuf, String)>> {
        let loaded = load(&self.path)?;
        let Some(fork) = loaded.forks.into_iter().nth(index) else {
            return Ok(None);
        };
        let mut source = File::open(&self.path)?;
        source.seek(SeekFrom::Start(loaded.header_end))?;
        let mut history = Vec::new();
        source
            .take(fork.offset - loaded.header_end)
            .read_to_end(&mut history)?;
        let directory = self.path.parent().unwrap_or(Path::new("."));
        let mut forked = Self::create(directory, root)?;
        if let Err(error) = forked.file.write_all(&history) {
            let _ = std::fs::remove_file(&forked.path);
            return Err(error);
        }
        Ok(Some((forked.path, fork.prompt)))
    }

    /// Whether `path` is this session's file, however it is named.
    pub fn is_file(&self, path: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        match (self.file.metadata(), std::fs::metadata(path)) {
            (Ok(own), Ok(other)) => own.dev() == other.dev() && own.ino() == other.ino(),
            _ => false,
        }
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

    pub fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
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
            self.context_tokens = None;
        } else if let Some(producer) = item[super::PRODUCER].as_str() {
            self.model = Some(producer.to_owned());
            self.context_tokens = item[super::CONTEXT].as_u64();
        }
        Ok(())
    }
}
