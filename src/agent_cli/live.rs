//! The sessions the user's running terminals and ACP clients hold, for a front end such as
//! Flightdeck to show, and so that no two processes write one session: each full screen, line
//! mode and `scramjet acp` session keeps a record of its session and whether a turn runs or
//! waits for the user, in a private directory (`$XDG_RUNTIME_DIR`, else the temporary directory).
//! A record's `.lock` file stays locked while its process lives, so one that can be locked was
//! left by a process gone, and whoever finds it removes it.

use scramjet::agent::Agent;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
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

/// A record standing.
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

/// A session's record, kept while it lasts: the full screen's or line mode's one, or each of
/// `scramjet acp`'s, which `client` names.
pub(super) struct Live {
    record: Option<Record>,
    /// The state a waiting turn goes back to.
    before_waiting: Option<State>,
    /// The ACP client's name; a terminal's record has none.
    client: Option<String>,
}

static TERMINAL: Mutex<Live> = Mutex::new(Live::new(None));
/// Tells apart the records a process opens within one millisecond.
static OPENED: AtomicU64 = AtomicU64::new(0);

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

fn terminal() -> std::sync::MutexGuard<'static, Live> {
    TERMINAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records the terminal's session in `state` (`Live::record`).
pub(super) fn record(agent: &Agent, state: State) {
    terminal().record(agent, state);
}

/// Marks the terminal's turn as waiting for the user until `answered`.
pub(super) fn waiting() {
    terminal().waiting();
}

pub(super) fn answered() {
    terminal().answered();
}

/// Removes the terminal's record, as it ends.
pub(super) fn end() {
    terminal().record = None;
}

impl Live {
    pub(super) const fn new(client: Option<String>) -> Self {
        Live {
            record: None,
            before_waiting: None,
            client,
        }
    }

    /// Records `agent`'s session in `state`, as the session now is: a new, resumed or forked
    /// one replaces the one before. An unsaved session (`--no-session`) has no record. A record
    /// that cannot be written is passed over: it only shows the session elsewhere.
    pub(super) fn record(&mut self, agent: &Agent, state: State) {
        if !agent.session_saved() {
            self.record = None;
            return;
        }
        self.before_waiting = None;
        let mut shown = Shown {
            id: agent.session_id(),
            path: agent.session_path().to_owned(),
            cwd: agent.root().to_owned(),
            state,
            since: now(),
        };
        if let Some(record) = self.record.as_ref() {
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
        if self.record.is_none() {
            self.record = open().ok();
        }
        let client = self.client.as_deref();
        if let Some(record) = self.record.as_mut()
            && write(&record.json_path, &shown, client).is_ok()
        {
            record.shown = shown;
        }
    }

    /// Records `agent`'s session again, in the state it was in, after it changed.
    pub(super) fn changed(&mut self, agent: &Agent) {
        let state = (self.record.as_ref()).map_or(State::Idle, |record| record.shown.state);
        let before = self.before_waiting;
        self.record(agent, state);
        self.before_waiting = before;
    }

    /// Marks a turn as waiting for the user until `answered`.
    pub(super) fn waiting(&mut self) {
        if self.before_waiting.is_some() {
            return;
        }
        let Some(record) = self.record.as_ref() else {
            return;
        };
        self.before_waiting = Some(record.shown.state);
        self.set(State::Waiting);
    }

    pub(super) fn answered(&mut self) {
        if let Some(before) = self.before_waiting.take() {
            self.set(before);
        }
    }

    fn set(&mut self, state: State) {
        let client = self.client.as_deref();
        let Some(record) = self.record.as_mut() else {
            return;
        };
        let shown = Shown {
            state,
            since: now(),
            ..record.shown.clone()
        };
        if write(&record.json_path, &shown, client).is_ok() {
            record.shown = shown;
        }
    }
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
    let opened = OPENED.fetch_add(1, Ordering::Relaxed);
    let name = format!("{}-{}-{opened}", std::process::id(), now());
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
fn write(path: &Path, shown: &Shown, client: Option<&str>) -> io::Result<()> {
    let mut record = json!({
        "pid": std::process::id(),
        "sessionId": shown.id,
        "path": shown.path,
        "cwd": shown.cwd,
        "state": shown.state.word(),
        "since": shown.since,
    });
    if let Some(client) = client {
        record["client"] = json!(client);
    }
    let fresh = path.with_extension("json.new");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = scramjet::os::private_file(&mut options).open(&fresh)?;
    file.write_all(record.to_string().as_bytes())?;
    drop(file);
    std::fs::rename(&fresh, path)
}

/// The records of the other processes running now; those of processes gone are removed.
pub(super) fn list() -> Vec<Value> {
    list_in(&directory())
}

/// The other process holding the session saved at `file`, if one does: a terminal, or
/// `scramjet acp` for a client.
pub(super) fn holder(file: &Path) -> Option<Holder> {
    holder_in(&directory(), file)
}

/// The process holding a session.
pub(super) struct Holder {
    pid: u64,
    client: Option<String>,
}

impl Holder {
    /// "another terminal has it open (process N)", or the ACP client's name in its place.
    pub(super) fn has_it_open(&self) -> String {
        format!("{} has it open (process {})", self.name(), self.pid)
    }

    fn name(&self) -> &str {
        self.client.as_deref().unwrap_or("another terminal")
    }
}

impl std::fmt::Display for Holder {
    /// "another terminal (process N)", or the ACP client's name in its place.
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} (process {})", self.name(), self.pid)
    }
}

fn holder_in(directory: &Path, file: &Path) -> Option<Holder> {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let file = canonical(file);
    (list_in(directory).into_iter())
        .find(|record| {
            (record["path"].as_str()).is_some_and(|path| canonical(Path::new(path)) == file)
        })
        .and_then(|record| {
            Some(Holder {
                pid: record["pid"].as_u64()?,
                client: record["client"].as_str().map(str::to_owned),
            })
        })
}

fn list_in(directory: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let own = format!("{}-", std::process::id());
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.extension().is_none_or(|extension| extension != "json")
            || name.to_string_lossy().starts_with(&own)
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
