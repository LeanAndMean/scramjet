use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

/// An append-only JSON-lines log of one conversation: a header, then each input item.
pub struct Session {
    path: PathBuf,
    file: File,
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
        let mut session = Self { path, file };
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

    /// Opens an existing session for appending and returns its conversation items.
    pub fn open(path: &Path) -> io::Result<(Self, Vec<Value>)> {
        let reader = BufReader::new(File::open(path)?);
        let mut items = Vec::new();
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
            } else {
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
            },
            items,
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

    pub fn append(&mut self, item: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(item)?;
        line.push(b'\n');
        self.file.write_all(&line)
    }
}
