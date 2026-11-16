//! The sessions the user's running terminals hold, for a front end such as Flightdeck to show:
//! each full screen or line mode keeps a record of its session and whether a turn runs or waits
//! for the user, in a private directory (`$XDG_RUNTIME_DIR`, else the temporary directory).
//! A record's `.lock` file stays locked while its process lives, so one that can be locked was
//! left by a process gone, and whoever finds it removes it.

use scramjet::agent::Agent;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum State {
    Idle,
    Running,
    /// A turn waits for the user's answer: an approval, a question, a next step's list.
    Waiting,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Running => "running",
            State::Waiting => "waiting",
        }
    }
}

/// This process's record.
struct Record {
    /// Held locked while the record stands.
    _lock: File,
    lock_path: PathBuf,
    json_path: PathBuf,
    shown: Shown,
}

#[derive(Clone, PartialEq)]
struct Shown {
    id: String,
    path: PathBuf,
    cwd: PathBuf,
    state: State,
    since: u64,
}

static RECORD: Mutex<Option<Record>> = Mutex::new(None);
/// The state a waiting turn goes back to.
static BEFORE_WAITING: Mutex<Option<State>> = Mutex::new(None);

fn now() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |since| since.as_millis().try_into().unwrap_or(u64::MAX))
}

/// The records' directory.
pub(super) fn directory() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    if let Some(runtime) = runtime.filter(|runtime| runtime.is_absolute() && runtime.is_dir()) {
        return runtime.join("scramjet-live");
    }
    #[cfg(unix)]
    let name = format!("scramjet-live-{}", rustix::process::geteuid().as_raw());
    #[cfg(not(unix))]
    let name = "scramjet-live".to_owned();
    std::env::temp_dir().join(name)
}

/// Makes `directory`, private to the user; one already there must be the user's own directory.
fn make_directory(directory: &Path) -> io::Result<()> {
    if let Some(parent) = directory.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut builder = std::fs::DirBuilder::new();
    match scramjet::os::private_directory(&mut builder).create(directory) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let found = std::fs::symlink_metadata(directory)?;
            if !found.is_dir() || !scramjet::os::owned(&found) {
                return Err(io::Error::other("not a directory of this user's"));
            }
            Ok(())
        }
        made => made,
    }
}

/// Records `agent`'s session in `state`, as the session now is: a new, resumed or forked one
/// replaces the one before. An unsaved session (`--no-session`) has no record. A record that
/// cannot be written is passed over: it only shows the session elsewhere.
pub(super) fn record(agent: &Agent, state: State) {
    let mut held = RECORD.lock().unwrap_or_else(PoisonError::into_inner);
    if !agent.session_saved() {
        *held = None;
        return;
    }
    *BEFORE_WAITING
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    let mut shown = Shown {
        id: agent.session_id(),
        path: agent.session_path().to_owned(),
        cwd: agent.root().to_owned(),
        state,
        since: now(),
    };
    if let Some(record) = held.as_ref() {
        let same = Shown {
            since: record.shown.since,
            ..shown.clone()
        };
        if same == record.shown {
            return;
        }
        if record.shown.state == state && record.shown.id == shown.id {
            shown.since = record.shown.since;
        }
    }
    if held.is_none() {
        *held = open().ok();
    }
    if let Some(record) = held.as_mut()
        && write(&record.json_path, &shown).is_ok()
    {
        record.shown = shown;
    }
}

/// Marks a turn as waiting for the user until `answered`.
pub(super) fn waiting() {
    let mut held = RECORD.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(record) = held.as_mut() else { return };
    let mut before = BEFORE_WAITING
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if before.is_some() {
        return;
    }
    *before = Some(record.shown.state);
    set(record, State::Waiting);
}

pub(super) fn answered() {
    let mut held = RECORD.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(record) = held.as_mut() else { return };
    if let Some(before) = BEFORE_WAITING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    {
        set(record, before);
    }
}

fn set(record: &mut Record, state: State) {
    let shown = Shown {
        state,
        since: now(),
        ..record.shown.clone()
    };
    if write(&record.json_path, &shown).is_ok() {
        record.shown = shown;
    }
}

/// Removes this process's record, as it ends.
pub(super) fn end() {
    *RECORD.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.json_path);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn open() -> io::Result<Record> {
    let directory = directory();
    make_directory(&directory)?;
    let name = format!("{}-{}", std::process::id(), now());
    let lock_path = directory.join(format!("{name}.lock"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let lock = scramjet::os::private_file(&mut options).open(&lock_path)?;
    if lock.try_lock().is_err() {
        let _ = std::fs::remove_file(&lock_path);
        return Err(io::Error::other("cannot lock the record"));
    }
    Ok(Record {
        _lock: lock,
        lock_path,
        json_path: directory.join(format!("{name}.json")),
        shown: Shown {
            id: String::new(),
            path: PathBuf::new(),
            cwd: PathBuf::new(),
            state: State::Idle,
            since: 0,
        },
    })
}

/// Writes the record whole, in place of the one before, so a reader never sees half of it.
fn write(path: &Path, shown: &Shown) -> io::Result<()> {
    let record = json!({
        "pid": std::process::id(),
        "sessionId": shown.id,
        "path": shown.path,
        "cwd": shown.cwd,
        "state": shown.state.word(),
        "since": shown.since,
    });
    let fresh = path.with_extension("json.new");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = scramjet::os::private_file(&mut options).open(&fresh)?;
    file.write_all(record.to_string().as_bytes())?;
    drop(file);
    std::fs::rename(&fresh, path)
}

/// The records of the processes running now, but this one's; those of processes gone are
/// removed.
pub(super) fn list() -> Vec<Value> {
    list_in(&directory())
}

fn list_in(directory: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let own = (RECORD.lock().unwrap_or_else(PoisonError::into_inner))
        .as_ref()
        .map(|record| record.json_path.clone());
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json")
            || own.as_ref() == Some(&path)
        {
            continue;
        }
        let lock_path = path.with_extension("lock");
        let lock = match File::open(&lock_path) {
            Ok(lock) => lock,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            Err(_) => continue,
        };
        if lock.try_lock_shared().is_ok() {
            drop(lock);
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(&lock_path);
            continue;
        }
        let read = std::fs::read(&path).ok();
        if let Some(record) = read.and_then(|read| serde_json::from_slice::<Value>(&read).ok()) {
            records.push(record);
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_whose_lock_is_free_is_removed_and_a_held_one_listed() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-live-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        let held = directory.join("1-1.lock");
        let lock = File::create(&held).unwrap();
        lock.lock().unwrap();
        std::fs::write(directory.join("1-1.json"), r#"{"pid":1,"state":"running"}"#).unwrap();
        std::fs::write(directory.join("2-2.json"), r#"{"pid":2}"#).unwrap();
        File::create(directory.join("2-2.lock")).unwrap();
        std::fs::write(directory.join("3-3.json"), r#"{"pid":3}"#).unwrap();
        let listed = list_in(&directory);
        assert_eq!(listed, vec![json!({"pid": 1, "state": "running"})]);
        let mut left: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["1-1.json", "1-1.lock"]);
        drop(lock);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
