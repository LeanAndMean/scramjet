use super::Tokens;
use super::compaction::FileLists;
pub use kept::{Written, write_kept};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::SystemTime,
};

pub(super) mod kept;

/// Held while lines are written to a session file, so that a process ending on a signal leaves
/// none cut short, which would make the file unreadable.
static WRITING: Mutex<()> = Mutex::new(());

/// Writes `bytes` to the session file `file` whole, unless the process ends first.
fn write_whole(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    file.write_all(bytes)
}

/// The most a session keeps in memory while its file cannot take its lines (a full disk); past
/// it, writes fail as the disk's.
const HELD_LIMIT: usize = 64 << 20;

/// Whether `error` says the disk or the user's quota is full, which a session outlasts by keeping
/// its lines in memory.
pub(super) fn full(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    )
}

/// A change in whether a session's lines reach its file, for the agent to say once.
#[derive(Debug, PartialEq)]
pub enum Saving {
    /// Its file could not take a line, for this reason; the lines are kept in memory.
    Held(String),
    /// What was kept in memory is in the file now.
    Saved,
}

/// Waits for lines being written to a session file, and keeps others from starting, for a
/// process about to exit.
pub(super) fn end_writes() {
    std::mem::forget(WRITING.lock().unwrap_or_else(PoisonError::into_inner));
}

/// A saved session as `/resume` lists it.
#[derive(Clone)]
pub struct SessionInfo {
    pub path: PathBuf,
    /// Its first prompt to the model, as typed; not a `!` command.
    pub first_prompt: Option<String>,
    /// Its prompts and replies on every branch, which `/resume` searches as Scramjet searches
    /// a session's messages.
    pub text: String,
    /// The session's name from its latest `session_info` line, which `/resume` shows instead.
    pub name: Option<String>,
    /// Conversation items after the latest compaction, as a resume restores them.
    pub item_count: usize,
    /// The summary of the latest `completed` report among them.
    pub summary: Option<String>,
    pub modified: SystemTime,
    /// The session it was forked from, as its header records it.
    pub parent: Option<PathBuf>,
    /// The session whose next step started it, as its header records it: the one before it in
    /// a chain.
    pub continues: Option<PathBuf>,
    /// The project it belongs to, as its header names it.
    pub root: Option<PathBuf>,
}

/// The conversation on a session's current branch, as `/export` shows it.
pub struct Branch {
    /// The session file's first line.
    pub header: Value,
    /// The entries on the path from the start to the leaf, compactions, branch summaries and
    /// model changes among them, with the items a compaction summarized before it.
    pub entries: Vec<Value>,
    pub name: Option<String>,
}

impl Branch {
    /// The models that wrote its replies or that it switched to, in the order they first appear.
    pub fn models(&self) -> Vec<String> {
        let mut models: Vec<String> = Vec::new();
        for entry in &self.entries {
            let model = match entry["type"].as_str() {
                Some("model_change") => entry["model"].as_str(),
                _ => entry[super::PRODUCER].as_str(),
            };
            if let Some(model) = model.filter(|model| !models.iter().any(|m| m == model)) {
                models.push(model.to_owned());
            }
        }
        models
    }
}

/// A label the user gave an entry in `/tree`, as Scramjet's tree labels mark entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    /// When it was given, in milliseconds since the Unix epoch.
    pub time: u64,
}

/// A session opened: it, its conversation items and every prompt the user gave in it.
pub type Opened = (Session, Vec<Value>, Vec<String>);

/// The saved session `Session::find` found.
#[derive(Debug, PartialEq)]
pub enum Found {
    /// One of the project's.
    Here(PathBuf),
    /// One of another project's, `root`, as its header names it.
    Elsewhere { path: PathBuf, root: PathBuf },
}

impl Found {
    pub fn path(&self) -> &Path {
        match self {
            Self::Here(path) | Self::Elsewhere { path, .. } => path,
        }
    }
}

/// The sessions in `directory` whose id starts with `prefix` and that hold a conversation, the
/// newest first, with when they were written to and their headers.
fn matching(directory: &Path, prefix: &str) -> io::Result<Vec<(SystemTime, PathBuf, Value)>> {
    let listed = match std::fs::read_dir(directory) {
        Ok(listed) => listed,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut candidates = Vec::new();
    for entry in listed.flatten() {
        let path = entry.path();
        let named = (path.file_name().and_then(|name| name.to_str()))
            .and_then(|name| name.strip_suffix(".jsonl"))
            .is_some_and(|id| id.starts_with(prefix));
        if let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified())
            && named
        {
            candidates.push((modified, path));
        }
    }
    candidates.sort_by(|a, b| b.cmp(a));
    let mut found = Vec::new();
    for (modified, path) in candidates {
        let Ok(entries) = read(&path) else {
            continue;
        };
        if (entries.context(&path, entries.tail)).is_ok_and(|context| !context.items.is_empty()) {
            found.push((modified, path, entries.header));
        }
    }
    Ok(found)
}

/// How `delete` removed a session file.
#[derive(Debug, PartialEq)]
pub enum Removal {
    /// The `trash` command moved it to the trash.
    Trashed,
    Deleted,
}

/// An append-only JSON-lines log: a header, then each input item, each model change, each
/// compaction, which replaces the items before `first_kept` with its summary and lists the files
/// read and modified before then, and each branch summary, a message summarizing a branch the
/// conversation came back from, with the files read and modified there, and each change of the
/// active command (`status::Lifecycle::saved`), and each `/autopilot` toggle. Entries form a tree (see `Entries`); the
/// conversation is the path from the start to the current entry, the leaf, which each appended
/// entry continues. Names and labels are lines of their own, not entries.
pub struct Session {
    path: PathBuf,
    /// `None` until the file can be created, on a disk too full for it at the start.
    file: Option<File>,
    /// The latest model change or response item producer on the path, as Scramjet restores it.
    model: Option<String>,
    /// The models that produced response items on the path, compacted ones included, in the
    /// order they first did.
    contributors: Vec<String>,
    /// The latest `session_info` name in the file, which applies to the whole session.
    name: Option<String>,
    /// The latest thinking level change on the path, which a resume restores.
    effort: Option<String>,
    /// The file lists of the latest compaction on the path, which the next one carries on.
    files: FileLists,
    /// The context size the latest response on the path reported, unless a compaction followed.
    context_tokens: Option<u64>,
    /// The latest `command_state` line on the path, which a resume restores.
    command: Option<Value>,
    /// The latest `/autopilot` toggle on the path, which a resume restores.
    autopilot: Option<bool>,
    /// The tokens of every response in the file.
    tokens: Tokens,
    leaf: Option<usize>,
    /// The entry a line appended without a branch line would continue.
    tail: Option<usize>,
    /// Lines in the file.
    lines: usize,
    /// The file's length after the last line read or written here; another length means another
    /// process appended lines (a name given in its `/resume`).
    size: u64,
    /// Created by this process with no conversation entry yet: only model and effort changes,
    /// names and `/autopilot` toggles, which Scramjet would not have written yet. Its lines wait
    /// in `held`, with no file, until its first conversation entry.
    unused: bool,
    /// Whether the last write failed for want of room, its lines held since.
    failing: bool,
    /// The lines after the header that could not be read and were passed over.
    skipped: Vec<usize>,
    /// Whether the file's last line lacks its newline, which the next write starts with.
    unterminated: bool,
    /// Lines written here that the file could not take (a full disk), counted in `lines` but not
    /// in `size`; each write tries them first.
    held: Vec<u8>,
    /// The change in `held` not yet said.
    change: Option<Saving>,
    /// Where `held` is copied so that it outlives the process (`kept`), for a saved session.
    keep_in: Option<PathBuf>,
    /// The copy of `held` while there is one.
    kept: Option<kept::Kept>,
    /// The session whose next step started this one, as the header's `parentSession` with
    /// `nextStep` records it.
    continues: Option<PathBuf>,
}

fn invalid(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{}: {reason}", path.display()),
    )
}

/// A line of a session file after its header, other than a branch line: a conversation item, a
/// model change, a compaction or a branch summary.
pub struct Entry {
    /// Its line number in the file, where the header is line 0.
    pub line: usize,
    /// The entry it continues: the one before it, unless a branch line after that one names
    /// another; `None` at the start of a conversation.
    pub parent: Option<usize>,
    pub value: Value,
    /// Its line's bytes in the file.
    bytes: std::ops::Range<usize>,
}

/// The entries of a session file, in file order. They form a tree, as Scramjet's session entries
/// do with their `parentId`: a branch line (`{"type":"branch","parent":N}`, `null` for the start
/// of a conversation) makes the entry after it continue entry `N` instead of the one before.
struct Entries {
    entries: Vec<Entry>,
    /// The latest `session_info` name in the file; an empty one clears it.
    name: Option<String>,
    /// Each labeled entry's latest label (`{"type":"label","target":N,"label":…,"time":…}`),
    /// by its line; one without `label` clears it.
    labels: HashMap<usize, Label>,
    /// The entry a line appended without a branch line continues, where a resume continues.
    tail: Option<usize>,
    /// Lines in the file, the header and blank ones included.
    lines: usize,
    /// The file's length.
    size: u64,
    /// The header's `parentSession`: the session this one was forked from, or, with `nextStep`,
    /// the one whose next step started it.
    parent: Option<PathBuf>,
    header: Value,
    tokens: Tokens,
    /// The lines after the header that could not be read and were passed over.
    skipped: Vec<usize>,
    /// Whether the last line lacks its newline.
    unterminated: bool,
}

/// The conversation on the path from the start to an entry.
struct Context {
    items: Vec<Value>,
    model: Option<String>,
    contributors: Vec<String>,
    effort: Option<String>,
    files: FileLists,
    context_tokens: Option<u64>,
    command: Option<Value>,
    autopilot: Option<bool>,
}

fn read(path: &Path) -> io::Result<Entries> {
    parse(path, BufReader::new(File::open(path)?))
}

fn parse(path: &Path, mut reader: impl BufRead) -> io::Result<Entries> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut tail = None;
    let mut name = None;
    let mut labels = HashMap::new();
    let mut parent = None;
    let mut header = Value::Null;
    let mut tokens = Tokens::default();
    let mut line = String::new();
    let mut skipped = Vec::new();
    let mut unterminated = false;
    let mut end = 0;
    let mut lines = 0;
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        // A last line without its newline that is not whole is one a write left cut short (a
        // full disk, a kill), or one being written: it is not counted, and the next append cuts
        // it off. A whole one (a file written by hand) gets its newline then.
        unterminated = !line.ends_with('\n');
        if unterminated && serde_json::from_str::<Value>(&line).is_err() {
            unterminated = false;
            break;
        }
        let index = lines;
        lines += 1;
        let start = end;
        end += read;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) if index == 0 => return Err(invalid(path, "malformed line")),
            Err(_) => {
                skipped.push(index);
                continue;
            }
        };
        if index == 0 {
            if value["type"] != "session" {
                return Err(invalid(path, "not a Scramjet session"));
            }
            parent = value["parentSession"].as_str().map(PathBuf::from);
            header = value;
        } else if value["type"] == "session_info" {
            name = value["name"]
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned);
        } else if value["type"] == "label" {
            // As Scramjet's, a label of no entry is not an error; it shows nowhere.
            let target = (value["target"].as_u64()).and_then(|line| usize::try_from(line).ok());
            if let Some(target) = target.filter(|&line| find(&entries, line).is_some()) {
                match value["label"].as_str().filter(|text| !text.is_empty()) {
                    Some(text) => labels.insert(
                        target,
                        Label {
                            text: text.to_owned(),
                            time: value["time"].as_u64().unwrap_or_default(),
                        },
                    ),
                    None => labels.remove(&target),
                };
            }
        } else if value["type"] == "branch" {
            let parent = match &value["parent"] {
                Value::Null => Some(None),
                parent => (parent.as_u64())
                    .and_then(|parent| usize::try_from(parent).ok())
                    .filter(|&parent| find(&entries, parent).is_some())
                    .map(Some),
            };
            match parent {
                Some(parent) => tail = parent,
                None => skipped.push(index),
            }
        } else {
            tokens.add(&value);
            entries.push(Entry {
                line: index,
                parent: tail,
                value,
                bytes: start..end,
            });
            tail = Some(index);
        }
    }
    if lines == 0 {
        return Err(invalid(path, "empty session file"));
    }
    Ok(Entries {
        entries,
        name,
        labels,
        tail,
        lines,
        size: end as u64,
        parent,
        header,
        tokens,
        skipped,
        unterminated,
    })
}

fn contribute(contributors: &mut Vec<String>, model: &str) {
    if !contributors.iter().any(|known| known == model) {
        contributors.push(model.to_owned());
    }
}

fn find(entries: &[Entry], line: usize) -> Option<&Entry> {
    entries
        .binary_search_by_key(&line, |entry| entry.line)
        .ok()
        .map(|index| &entries[index])
}

/// Whether `entry` is a prompt the user sent to the model, which a branch can start before.
pub(super) fn is_prompt(entry: &Value) -> bool {
    entry["role"] == "user"
        && !is_bang(entry)
        && (entry["content"].is_string() || entry["content"].is_array())
        && entry.get(super::PROBE).is_none()
}

/// Whether `entry` is a `!` command, which is typed as `!...` and is not a prompt.
pub(super) fn is_bang(entry: &Value) -> bool {
    entry[super::TYPED]
        .as_str()
        .is_some_and(|typed| typed.starts_with('!'))
}

/// The prompt as the user typed it: a command's expansion is sent as its content, and what was
/// typed is saved with it. Empty for a message without text.
pub(super) fn prompt_text(entry: &Value) -> &str {
    entry[super::TYPED]
        .as_str()
        .filter(|_| !is_bang(entry))
        .or(entry["content"].as_str())
        .unwrap_or_default()
}

impl Entries {
    /// Whether a next step of the session before it started this one (`nextStep`).
    fn continues(&self) -> bool {
        self.header["nextStep"] == true
    }

    /// The entries from the start of the conversation to `leaf`, in order.
    fn path(&self, leaf: Option<usize>) -> Vec<&Entry> {
        let mut path = Vec::new();
        // A parent is always an earlier line, so this ends.
        let mut at = leaf.and_then(|line| find(&self.entries, line));
        while let Some(entry) = at {
            path.push(entry);
            at = entry.parent.and_then(|line| find(&self.entries, line));
        }
        path.reverse();
        path
    }

    /// The conversation a request at `leaf` continues, after the latest compaction on its path.
    fn context(&self, path: &Path, leaf: Option<usize>) -> io::Result<Context> {
        let mut context = Context {
            items: Vec::new(),
            model: None,
            contributors: Vec::new(),
            effort: None,
            files: FileLists::default(),
            context_tokens: None,
            command: None,
            autopilot: None,
        };
        let items = &mut context.items;
        for entry in self.path(leaf) {
            let value = &entry.value;
            if value["type"] == "compaction" {
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
                context.files = FileLists::of(value);
                context.context_tokens = None;
            } else if value["type"] == "branch_summary" {
                let summary = value["summary"]
                    .as_str()
                    .ok_or_else(|| invalid(path, "missing branch summary"))?;
                items.push(super::compaction::branch_summary_message(summary));
            } else if value["type"] == "model_change" {
                let changed = value["model"]
                    .as_str()
                    .ok_or_else(|| invalid(path, "missing model in a model change"))?;
                context.model = Some(changed.to_owned());
            } else if value["type"] == "thinking_level_change" {
                let level = value["level"]
                    .as_str()
                    .ok_or_else(|| invalid(path, "missing level in a thinking level change"))?;
                context.effort = Some(level.to_owned());
            } else if value["type"] == "command_state" {
                context.command = Some(value.clone());
            } else if value["type"] == "autopilot" {
                context.autopilot = value["on"].as_bool();
            } else {
                if let Some(producer) = value[super::PRODUCER].as_str() {
                    context.model = Some(producer.to_owned());
                    context.context_tokens = value[super::CONTEXT].as_u64();
                    contribute(&mut context.contributors, producer);
                }
                items.push(value.clone());
            }
        }
        Ok(context)
    }

    /// Every prompt in the file, on every branch and including those a compaction summarized, as
    /// the user typed it.
    fn prompts(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.value["role"] == "user" && entry.value.get(super::PROBE).is_none()
            })
            .filter_map(|entry| {
                let value = &entry.value;
                value[super::TYPED].as_str().or(value["content"].as_str())
            })
            .map(str::to_owned)
            .collect()
    }

    /// Every prompt and reply text in the file, separated by spaces.
    fn text(&self) -> String {
        let mut text = Vec::new();
        for entry in &self.entries {
            let value = &entry.value;
            if is_prompt(value) {
                text.push(prompt_text(value));
            } else if value["type"] == "message" && value["role"] == "assistant" {
                let parts = value["content"].as_array().into_iter().flatten();
                text.extend(parts.filter_map(|part| part["text"].as_str()));
            }
        }
        text.join(" ")
    }

    /// The prompts sent to the model, which a fork can start before; not `!` commands.
    fn fork_points(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| is_prompt(&entry.value))
    }

    fn branch(&self, leaf: Option<usize>) -> Branch {
        Branch {
            header: self.header.clone(),
            entries: (self.path(leaf).into_iter())
                .map(|entry| entry.value.clone())
                .collect(),
            name: self.name.clone(),
        }
    }

    /// The lines of `file` (this session's bytes) on the path to `leaf`, for a file of their own
    /// after its header, then their labels, as Scramjet's branched sessions keep them, and a
    /// `session_info` line naming it `name` when given.
    fn history(
        &self,
        file: &Path,
        source: &[u8],
        leaf: Option<usize>,
        name: Option<&str>,
    ) -> io::Result<Vec<u8>> {
        let mut history = Vec::new();
        let mut labels = Vec::new();
        for (index, entry) in self.path(leaf).into_iter().enumerate() {
            let line = source
                .get(entry.bytes.clone())
                .ok_or_else(|| invalid(file, "changed while it was read"))?;
            history.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                history.push(b'\n');
            }
            if let Some(label) = self.labels.get(&entry.line) {
                labels.push(label_line(index + 1, Some(&label.text), label.time));
            }
        }
        for label in labels {
            history.extend(serde_json::to_vec(&label)?);
            history.push(b'\n');
        }
        if let Some(name) = name {
            history.extend(serde_json::to_vec(
                &json!({"type":"session_info","name":name}),
            )?);
            history.push(b'\n');
        }
        Ok(history)
    }
}

fn label_line(target: usize, text: Option<&str>, time: u64) -> Value {
    let mut line = json!({"type":"label","target":target,"time":time});
    if let Some(text) = text {
        line["label"] = json!(text);
    }
    line
}

fn now_ms() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.unwrap_or_default().as_millis() as u64
}

/// A name in `directory` for a new session file, for the millisecond it is started in and this
/// process, with that time; the file is created at its first conversation entry.
fn new_name(directory: &Path) -> (PathBuf, u128) {
    // Sessions started in the same millisecond (a quick `/new` or `/fork`) take the next ones.
    static LAST: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    let mut last = LAST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = |created| directory.join(format!("{created}-{}.jsonl", std::process::id()));
    let mut created = now_ms().max(*last + 1);
    while name(created).exists() {
        created += 1;
    }
    *last = created;
    (name(created), created as u128)
}

/// Creates a session directory, private to the user, and its parents. One already there must be
/// the user's own directory, not a link: an `Unsaved` directory a full disk kept from being made at
/// start is made in the shared temporary directory, where another user could have made its name.
pub(in crate::agent) fn make_directory(directory: &Path) -> io::Result<()> {
    if let Some(parent) = directory.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    match crate::os::private_directory(&mut std::fs::DirBuilder::new()).create(directory) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let found = std::fs::symlink_metadata(directory)?;
            if !found.is_dir() || !crate::os::owned(&found) {
                return Err(io::Error::other(format!(
                    "{}: not a directory of this user's",
                    directory.display()
                )));
            }
            Ok(())
        }
        made => made,
    }
}

/// Creates the session file `path`, private to the user, and its directory; one already there is
/// an error.
fn create_file(path: &Path) -> io::Result<File> {
    if let Some(directory) = path.parent() {
        make_directory(directory)?;
    }
    crate::os::private_file(OpenOptions::new().append(true).create_new(true)).open(path)
}

/// The current branch of the session saved at `path`, where a resume would continue it.
pub fn read_branch(path: &Path) -> io::Result<Branch> {
    let entries = read(path)?;
    Ok(entries.branch(entries.tail))
}

/// A session read to be shown before a later one of its chain, without opening it.
pub struct Earlier {
    /// Its first prompt as typed, such as the command that ran in it.
    pub first_prompt: Option<String>,
    /// The conversation where a resume would continue it, after its latest compaction.
    pub items: Vec<Value>,
    /// When it started, in milliseconds since 1970, as its header says.
    pub created: Option<u64>,
    pub modified: SystemTime,
    /// The session before it in the chain.
    pub continues: Option<PathBuf>,
}

/// The session saved at `path`, read as an earlier one of a chain is shown.
pub fn read_earlier(path: &Path) -> io::Result<Earlier> {
    let entries = read(path)?;
    let context = entries.context(path, entries.tail)?;
    Ok(Earlier {
        first_prompt: (entries.fork_points().next())
            .map(|entry| prompt_text(&entry.value).to_owned()),
        items: context.items,
        created: entries.header["created"].as_u64(),
        modified: std::fs::metadata(path)?.modified()?,
        continues: entries.parent.clone().filter(|_| entries.continues()),
    })
}

/// `$XDG_STATE_HOME/scramjet/sessions/<project path with / as ->` (and on Windows `\` and
/// `:`, so that `C:\…` names a directory there rather than itself).
pub fn directory(root: &Path) -> io::Result<PathBuf> {
    Ok(projects_directory()?.join(slug(&root.to_string_lossy())))
}

fn slug(root: &str) -> String {
    let replaced = |c: char| std::path::is_separator(c) || (cfg!(windows) && c == ':');
    root.trim_matches(replaced).replace(replaced, "-")
}

/// `$XDG_STATE_HOME/scramjet/sessions`, holding each project's sessions' directory.
pub fn projects_directory() -> io::Result<PathBuf> {
    Ok(state_directory()?.join("sessions"))
}

/// The directory of the sessions of a run that saves none (`--no-session`): private, in the
/// temporary directory, and removed with them once dropped, or at exit.
pub struct Unsaved(PathBuf);

/// The `Unsaved` directories not dropped yet, which `remove_unsaved` removes at exit: one on a
/// signal (`std::process::exit`) runs no destructors.
static UNSAVED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

extern "C" fn remove_unsaved() {
    // A thread holding it as the process exits leaves its directory rather than hang the exit.
    if let Ok(directories) = UNSAVED.try_lock() {
        for directory in directories.iter() {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

impl Unsaved {
    pub fn create() -> io::Result<Self> {
        static AT_EXIT: std::sync::Once = std::sync::Once::new();
        // SAFETY: `remove_unsaved` neither unwinds nor exits.
        AT_EXIT.call_once(|| unsafe {
            libc::atexit(remove_unsaved);
        });
        let mut builder = std::fs::DirBuilder::new();
        crate::os::private_directory(&mut builder);
        let mut n = 0u64;
        loop {
            let name = format!("scramjet-unsaved-{}-{n}", std::process::id());
            let path = std::env::temp_dir().join(name);
            match builder.create(&path) {
                Ok(()) => return Ok(Self::named(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => n += 1,
                // The session made in it keeps its lines, and makes it once there is room.
                Err(error) if full(&error) => return Ok(Self::named(path)),
                Err(error) => return Err(error),
            }
        }
    }

    fn named(path: PathBuf) -> Self {
        let unsaved = UNSAVED.lock();
        let mut unsaved = unsaved.unwrap_or_else(std::sync::PoisonError::into_inner);
        unsaved.push(path.clone());
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Whether `path` is in the directory of a run that saves no session.
    pub(super) fn holds(path: &Path) -> bool {
        let unsaved = UNSAVED.lock();
        let unsaved = unsaved.unwrap_or_else(std::sync::PoisonError::into_inner);
        unsaved.iter().any(|directory| path.starts_with(directory))
    }
}

impl Drop for Unsaved {
    fn drop(&mut self) {
        let unsaved = UNSAVED.lock();
        let mut unsaved = unsaved.unwrap_or_else(std::sync::PoisonError::into_inner);
        unsaved.retain(|directory| *directory != self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `$XDG_STATE_HOME/scramjet`, by default `~/.local/state/scramjet`.
pub fn state_directory() -> io::Result<PathBuf> {
    let state = crate::os::base_directory("XDG_STATE_HOME", ".local/state")
        .ok_or_else(|| io::Error::other("neither XDG_STATE_HOME nor HOME is set"))?;
    Ok(state.join("scramjet"))
}

impl Drop for Session {
    /// Tries once more to write what the file could not take; a file left empty goes.
    fn drop(&mut self) {
        if self.held.is_empty() || self.unused {
            return;
        }
        let writing = self.writing();
        let _ = self.write(&[]);
        if !self.held.is_empty() && self.size == 0 {
            let _ = std::fs::remove_file(&self.path);
        }
        drop(writing);
    }
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
            let Ok(entries) = read(&path) else {
                continue;
            };
            let Ok(context) = entries.context(&path, entries.tail) else {
                continue;
            };
            let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
                continue;
            };
            sessions.push(SessionInfo {
                path,
                first_prompt: entries
                    .fork_points()
                    .next()
                    .map(|entry| prompt_text(&entry.value).to_owned()),
                text: entries.text(),
                name: entries.name.clone(),
                item_count: context.items.len(),
                summary: super::status::completed_summary(&context.items),
                modified,
                parent: entries.parent.clone().filter(|_| !entries.continues()),
                continues: entries.parent.clone().filter(|_| entries.continues()),
                root: entries.header["root"].as_str().map(PathBuf::from),
            });
        }
        sessions.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| b.path.cmp(&a.path))
        });
        Ok(sessions)
    }

    /// The saved session whose id starts with `prefix`, as Scramjet's `--session` and `--fork`
    /// find one by part of its id: the newest of the project's, in `directory`, else the newest
    /// of the other projects', in the directories of `projects`. Sessions without a conversation
    /// (one another process just started) are left out, as Scramjet writes none.
    pub fn find(directory: &Path, projects: &Path, prefix: &str) -> io::Result<Option<Found>> {
        if let Some((_, path, _)) = matching(directory, prefix)?.into_iter().next() {
            return Ok(Some(Found::Here(path)));
        }
        let Ok(listed) = std::fs::read_dir(projects) else {
            return Ok(None);
        };
        let mut found = Vec::new();
        for entry in listed.flatten() {
            let other = entry.path();
            if other != directory {
                // As Scramjet's, a project whose sessions cannot be read is passed over.
                found.extend(matching(&other, prefix).unwrap_or_default());
            }
        }
        let newest = found
            .into_iter()
            .max_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        Ok(newest.map(|(_, path, header)| {
            let root = match header["root"].as_str() {
                Some(root) => PathBuf::from(root),
                None => path.parent().unwrap_or(Path::new(".")).to_owned(),
            };
            Found::Elsewhere { path, root }
        }))
    }

    /// Starts a session for the project `root` in `directory` with every entry, branch, name and
    /// label of the session file `source`, forked from it, as Scramjet's `--fork` copies a
    /// session, and opens it; a file that is not a session is not copied.
    pub fn fork_from(directory: &Path, root: &Path, source: &Path) -> io::Result<Opened> {
        let bytes = std::fs::read(source).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", source.display()))
        })?;
        read(source)?;
        // The header is line 0; the lines after it keep their numbers, which branches and labels
        // refer to.
        let after_header = bytes.iter().position(|&byte| byte == b'\n');
        let lines = after_header.map_or(&[][..], |end| &bytes[end + 1..]);
        Self::create(directory, root, Some(source))?.filled(lines)
    }

    /// Starts a session for the project `root` in `directory`; a fork names the session it came
    /// from, `parent`, in its header, as Scramjet's `parentSession`.
    pub fn create(directory: &Path, root: &Path, parent: Option<&Path>) -> io::Result<Self> {
        Self::start(directory, root, parent, false)
    }

    /// Starts a session for the project `root` in `directory` that a next step of the session
    /// `from` starts: its header names `from` as `parentSession`, as a fork's does, and says
    /// `nextStep`, so that a chain can be told from a fork.
    pub fn continuing(directory: &Path, root: &Path, from: &Path) -> io::Result<Self> {
        Self::start(directory, root, Some(from), true)
    }

    fn start(
        directory: &Path,
        root: &Path,
        parent: Option<&Path>,
        next_step: bool,
    ) -> io::Result<Self> {
        let continues = match (parent, next_step) {
            (Some(parent), true) => Some(std::path::absolute(parent)?),
            _ => None,
        };
        let mut session = Self::begin(directory, |created| {
            let mut header =
                json!({"type":"session","version":1,"root":root,"created":created as u64});
            if let Some(parent) = parent {
                header["parentSession"] = json!(std::path::absolute(parent)?);
            }
            if next_step {
                header["nextStep"] = json!(true);
            }
            let mut header = serde_json::to_vec(&header)?;
            header.push(b'\n');
            Ok(header)
        })?;
        session.continues = continues;
        Ok(session)
    }

    /// Starts a session in `directory` whose first line is `header`, given the millisecond its
    /// file is named for. Its lines wait in memory until its first conversation entry, as
    /// Scramjet writes no session before the first reply, so that a run ended or killed before
    /// any prompt leaves no file.
    fn begin(
        directory: &Path,
        header: impl FnOnce(u128) -> io::Result<Vec<u8>>,
    ) -> io::Result<Self> {
        // A directory that cannot be made fails now, not at the first prompt.
        match make_directory(directory) {
            Err(error) if full(&error) => {}
            made => made?,
        }
        let (path, created) = new_name(directory);
        let held = header(created)?;
        Ok(Self {
            keep_in: kept::directory_for(&path),
            kept: None,
            path,
            file: None,
            model: None,
            contributors: Vec::new(),
            name: None,
            effort: None,
            files: FileLists::default(),
            context_tokens: None,
            command: None,
            autopilot: None,
            tokens: Tokens::default(),
            leaf: None,
            tail: None,
            lines: 1,
            size: 0,
            unused: true,
            failing: false,
            skipped: Vec::new(),
            unterminated: false,
            held,
            change: None,
            continues: None,
        })
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
    ///
    /// Lines an earlier run kept for want of room are written first, or taken over when there is
    /// still none.
    pub fn open(path: &Path) -> io::Result<Opened> {
        let file = OpenOptions::new().append(true).open(path)?;
        let keep_in = kept::directory_for(path);
        let taken = keep_in.as_ref().and_then(|_| kept::adopt(path));
        let entries = match &taken {
            Some((taken, _)) => {
                let mut bytes = std::fs::read(path)?;
                bytes.truncate(taken.size as usize);
                bytes.extend_from_slice(&taken.lines);
                parse(path, &bytes[..])?
            }
            None => read(path)?,
        };
        let context = entries.context(path, entries.tail)?;
        let (size, held, kept, change) = match taken {
            Some((taken, error)) => {
                let change = Saving::Held(error.to_string());
                (taken.size, taken.lines, Some(taken.kept), Some(change))
            }
            None => (entries.size, Vec::new(), None, None),
        };
        Ok((
            Self {
                keep_in,
                kept,
                path: path.to_owned(),
                file: Some(file),
                model: context.model,
                contributors: context.contributors,
                name: entries.name.clone(),
                effort: context.effort,
                files: context.files,
                context_tokens: context.context_tokens,
                command: context.command,
                autopilot: context.autopilot,
                tokens: entries.tokens,
                leaf: entries.tail,
                tail: entries.tail,
                lines: entries.lines,
                size,
                unused: false,
                failing: !held.is_empty(),
                skipped: entries.skipped.clone(),
                unterminated: entries.unterminated,
                held,
                change,
                continues: entries.parent.clone().filter(|_| entries.continues()),
            },
            context.items,
            entries.prompts(),
        ))
    }

    /// The prompts this session can be forked before, in file order, on every branch and
    /// including those a compaction summarized, as Scramjet's `/fork` lists its user messages.
    pub fn fork_prompts(&self) -> io::Result<Vec<String>> {
        Ok(self
            .read()?
            .fork_points()
            .map(|entry| prompt_text(&entry.value).to_owned())
            .collect())
    }

    /// Starts a session for the project `root` next to this one with the entries on the path to
    /// its prompt `index` (0-based, as `fork_prompts` lists them), that prompt left out, as
    /// Scramjet's `/fork` copies a session's path, or on the path to the entry `leaf` gives for
    /// the one before it. Returns it opened and that prompt; `None`: no such prompt.
    pub fn fork(
        &self,
        root: &Path,
        index: usize,
        leaf: impl FnOnce(&[Entry], Option<usize>) -> Option<usize>,
    ) -> io::Result<Option<(Opened, String)>> {
        let entries = self.read()?;
        let Some(fork) = entries.fork_points().nth(index) else {
            return Ok(None);
        };
        let prompt = prompt_text(&fork.value).to_owned();
        let leaf = leaf(&entries.entries, fork.parent);
        let opened = self.branch_file(&entries, leaf, root, None)?;
        Ok(Some((opened, prompt)))
    }

    /// Starts a session for the project `root` next to this one with the entries on the path to
    /// the leaf and the session's name, as Scramjet's `/clone` duplicates a session at its
    /// current position, and opens it.
    pub fn clone_at_leaf(&self, root: &Path) -> io::Result<Opened> {
        let entries = self.read()?;
        self.branch_file(&entries, self.leaf, root, entries.name.as_deref())
    }

    /// Starts a session next to this one, forked from it, with the entries of `entries` (this
    /// file's) on the path to `leaf`, then `name` when given, and opens it.
    fn branch_file(
        &self,
        entries: &Entries,
        leaf: Option<usize>,
        root: &Path,
        name: Option<&str>,
    ) -> io::Result<Opened> {
        let source = self.bytes()?;
        let history = entries.history(&self.path, &source, leaf, name)?;
        let directory = self.path.parent().unwrap_or(Path::new("."));
        Self::create(directory, root, Some(&self.path))?.filled(&history)
    }

    /// The conversation from the start to the leaf.
    pub fn branch(&self) -> io::Result<Branch> {
        Ok(self.read()?.branch(self.leaf))
    }

    /// Writes the session's current branch to `to` as a session file of its own, as Scramjet's
    /// `/export` writes a `.jsonl`: its header, without the session it was forked from, the
    /// entries on the path to the leaf in order, and its name. An existing file is replaced.
    pub fn export(&self, to: &Path) -> io::Result<()> {
        let source = self.bytes()?;
        let entries = parse(&self.path, &source[..])?;
        let mut header = entries.header.clone();
        if let Some(header) = header.as_object_mut() {
            header.remove("parentSession");
            header.remove("nextStep");
        }
        let mut lines = serde_json::to_vec(&header)?;
        lines.push(b'\n');
        let name = entries.name.as_deref();
        lines.extend(entries.history(&self.path, &source, self.leaf, name)?);
        if let Some(directory) = to.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(to, lines)
    }

    /// Copies the session file `source` into `directory` under a new session's name, so that it
    /// is listed and continued as one made there, and opens the copy. A file that is not a
    /// session is not copied.
    pub fn import(directory: &Path, source: &Path) -> io::Result<Opened> {
        let bytes = std::fs::read(source).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", source.display()))
        })?;
        read(source)?;
        let (header, lines) = match bytes.iter().position(|&byte| byte == b'\n') {
            Some(end) => bytes.split_at(end + 1),
            None => (&bytes[..], &[][..]),
        };
        let mut header = header.to_vec();
        if !header.ends_with(b"\n") {
            header.push(b'\n');
        }
        Self::begin(directory, |_| Ok(header))?.filled(lines)
    }

    /// This session, just started, with `lines` after its header, written or held in memory as
    /// any lines are, opened as `open` opens a file: its conversation items and prompts. A last
    /// line left cut short is left out. On failure its file goes.
    pub(super) fn filled(mut self, lines: &[u8]) -> io::Result<Opened> {
        let opened = (|| {
            let mut bytes = self.bytes()?;
            let header = bytes.len();
            bytes.extend_from_slice(lines);
            let entries = parse(&self.path, &bytes[..])?;
            let context = entries.context(&self.path, entries.tail)?;
            let mut lines = bytes[header..entries.size as usize].to_vec();
            if entries.unterminated {
                lines.push(b'\n');
            }
            // A fork from before the first prompt is not written before its own.
            self.unused = context.items.is_empty();
            self.write(&lines)?;
            Ok((entries, context))
        })();
        let (entries, context) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                self.held.clear();
                self.discard_kept();
                if self.file.is_some() {
                    let _ = std::fs::remove_file(&self.path);
                }
                return Err(error);
            }
        };
        let prompts = entries.prompts();
        self.model = context.model;
        self.contributors = context.contributors;
        self.name = entries.name;
        self.effort = context.effort;
        self.files = context.files;
        self.context_tokens = context.context_tokens;
        self.command = context.command;
        self.autopilot = context.autopilot;
        self.tokens = entries.tokens;
        self.leaf = entries.tail;
        self.tail = entries.tail;
        self.lines = entries.lines;
        self.skipped = entries.skipped;
        Ok((self, context.items, prompts))
    }

    /// Every entry of the session, in file order.
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        Ok(self.read()?.entries)
    }

    /// Every entry of the session, in file order, and the labels of those labeled, by line.
    pub fn labeled_entries(&self) -> io::Result<(Vec<Entry>, HashMap<usize, Label>)> {
        let entries = self.read()?;
        Ok((entries.entries, entries.labels))
    }

    /// Gives the entry on `line` the label `text`, or takes its label away (`None`), as
    /// Scramjet's tree does; a line of its own, as a name is. Returns the label given.
    pub fn set_label(&mut self, line: usize, text: Option<&str>) -> io::Result<Option<Label>> {
        let text = text.filter(|text| !text.is_empty());
        let _writing = self.writing()?;
        self.catch_up()?;
        if find(&self.read()?.entries, line).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no such entry in the session",
            ));
        }
        let time = now_ms();
        let mut written = serde_json::to_vec(&label_line(line, text, time))?;
        written.push(b'\n');
        self.write(&written)?;
        self.lines += 1;
        Ok(text.map(|text| Label {
            text: text.to_owned(),
            time,
        }))
    }

    /// The entry the conversation ends at; `None` before its first one.
    pub fn leaf(&self) -> Option<usize> {
        self.leaf
    }

    /// Makes the conversation end at the entry on line `leaf` (`None`: before the first one), so
    /// the next entry continues it, as Scramjet's tree navigation moves its leaf. Returns the
    /// conversation there. Nothing is written until the next entry; on failure nothing changes.
    pub fn move_to(&mut self, leaf: Option<usize>) -> io::Result<Vec<Value>> {
        let entries = self.read()?;
        if leaf.is_some_and(|line| find(&entries.entries, line).is_none()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no such entry in the session",
            ));
        }
        let context = entries.context(&self.path, leaf)?;
        self.leaf = leaf;
        self.model = context.model;
        self.contributors = context.contributors;
        self.effort = context.effort;
        self.files = context.files;
        self.context_tokens = context.context_tokens;
        self.command = context.command;
        self.autopilot = context.autopilot;
        Ok(context.items)
    }

    /// Moves to the end of the session, the lines held in memory included, as `open` resumes a
    /// session at its file's end. Returns its conversation and every prompt, as `open` does.
    pub fn resumed(&mut self) -> io::Result<(Vec<Value>, Vec<String>)> {
        let prompts = self.read()?.prompts();
        Ok((self.move_to(self.tail)?, prompts))
    }

    /// The entries on the path to the leaf after the last one it shares with the path to
    /// `target`: the branch a move to `target` leaves, as Scramjet collects it for a summary.
    pub fn left_behind(&self, target: Option<usize>) -> io::Result<Vec<Value>> {
        let entries = self.read()?;
        let from = entries.path(self.leaf);
        let to = entries.path(target);
        let shared = from
            .iter()
            .zip(&to)
            .take_while(|(from, to)| from.line == to.line)
            .count();
        Ok(from[shared..]
            .iter()
            .map(|entry| entry.value.clone())
            .collect())
    }

    /// Whether `path` is this session's file, however it is named.
    pub fn is_file(&self, path: &Path) -> bool {
        let Some(file) = &self.file else {
            return false;
        };
        crate::os::same_file(file, path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The session whose next step started this one: the one before it in a chain.
    pub fn continues(&self) -> Option<&Path> {
        self.continues.as_deref()
    }

    /// Stable across resumes: the file name without its extension.
    pub fn id(&self) -> String {
        self.path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The session's name, as `/name` sets it; `None` when unnamed.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Saves `name` (trimmed; empty clears it) as the session's name, as Scramjet's `/name` does.
    /// It is a line of its own, not an entry: the conversation and its branches stay as they are.
    pub fn set_name(&mut self, name: &str) -> io::Result<()> {
        let _writing = self.writing()?;
        self.write_name(name)
    }

    fn write_name(&mut self, name: &str) -> io::Result<()> {
        let name = name.trim();
        let mut line = serde_json::to_vec(&json!({"type":"session_info","name":name}))?;
        line.push(b'\n');
        self.catch_up()?;
        self.write(&line)?;
        self.lines += 1;
        self.name = Some(name.to_owned()).filter(|name| !name.is_empty());
        Ok(())
    }

    /// Names the session saved at `path` as `set_name` names an open one. Its modification time
    /// stays, so that `/resume` keeps it in its place and `--continue` does not take it, as
    /// Scramjet orders sessions by their last message.
    pub fn name_file(path: &Path, name: &str) -> io::Result<()> {
        let mut session = Self::open(path)?.0;
        let _writing = session.writing()?;
        let modified = session.created().metadata()?.modified()?;
        session.write_name(name)?;
        session.created().set_modified(modified)
    }

    /// Removes the session saved at `path`, as Scramjet's `/resume` deletes one: to the trash with
    /// the `trash` command when there is one, else for good. A file that is not a session stays.
    pub fn delete(path: &Path) -> io::Result<Removal> {
        Self::delete_with(path, "trash")
    }

    /// As `delete`, with `trash` for the `trash` command.
    pub(super) fn delete_with(path: &Path, trash: &str) -> io::Result<Removal> {
        read(path)?;
        kept::forget(path);
        let trash = std::process::Command::new(trash)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output();
        if trash.as_ref().is_ok_and(|trash| trash.status.success()) || !path.exists() {
            return Ok(Removal::Trashed);
        }
        std::fs::remove_file(path).map_err(|error| {
            let said = match &trash {
                Ok(trash) => String::from_utf8_lossy(&trash.stderr).trim().to_owned(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                Err(error) => error.to_string(),
            };
            match said.lines().next() {
                Some(first) => io::Error::new(error.kind(), format!("{error} (trash: {first})")),
                None => error,
            }
        })?;
        Ok(Removal::Deleted)
    }

    /// The session's file as written here: what is in it, then the lines it could not take yet.
    fn bytes(&self) -> io::Result<Vec<u8>> {
        let mut bytes = match self.file {
            Some(_) => std::fs::read(&self.path)?,
            None => Vec::new(),
        };
        bytes.extend_from_slice(&self.held);
        Ok(bytes)
    }

    /// The session's entries as written here, the lines its file could not take yet included.
    fn read(&self) -> io::Result<Entries> {
        match self.held.is_empty() {
            true => read(&self.path),
            false => parse(&self.path, &self.bytes()?[..]),
        }
    }

    /// Writes go to `file` in place of the session's file from now on, as `/dev/full` fills a disk.
    #[cfg(test)]
    pub(super) fn write_to(&mut self, file: File) {
        if std::mem::take(&mut self.unused) {
            self.write(&[]).unwrap();
        }
        self.file = Some(file);
    }

    /// The session's lines as written here, those not in its file yet included.
    #[cfg(test)]
    pub(super) fn text(&self) -> String {
        String::from_utf8(self.bytes().unwrap()).unwrap()
    }

    /// The session as one started on a disk too full to create its file, to be created at `path`.
    #[cfg(test)]
    pub(super) fn not_created_at(&mut self, path: &Path) {
        self.held = self.bytes().unwrap();
        if self.file.take().is_some() {
            std::fs::remove_file(&self.path).unwrap();
        }
        (self.path, self.size, self.unused) = (path.to_owned(), 0, false);
    }

    /// The file of a session whose file exists: one opened, or one whose header was written.
    fn created(&mut self) -> &mut File {
        (self.file.as_mut()).expect("a session with its header written has its file")
    }

    /// Tries again to write the lines its file could not take, as the next write would.
    pub fn write_held(&mut self) -> io::Result<()> {
        if self.held.is_empty() || self.unused {
            return Ok(());
        }
        let _writing = self.writing()?;
        self.write(&[])
    }

    /// Whether lines its file could not take are kept in memory.
    pub fn holding(&self) -> bool {
        !self.held.is_empty() && !self.unused
    }

    /// How many bytes of lines its file could not take are kept in memory.
    pub fn held_len(&self) -> usize {
        match self.unused {
            true => 0,
            false => self.held.len(),
        }
    }

    /// The change in whether its lines reach its file since this was last asked.
    pub fn take_change(&mut self) -> Option<Saving> {
        self.change.take()
    }

    /// Locks the session directory until dropped, so that no other process's line lands between
    /// `catch_up` and the write after it. Where no lock can be taken, writes go unserialized, as they do
    /// while a session without its file has no directory either.
    fn writing(&self) -> io::Result<Option<File>> {
        let directory = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        match crate::os::lock_directory(directory) {
            Ok(lock) => Ok(lock),
            Err(error) if error.kind() == io::ErrorKind::NotFound && self.file.is_none() => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Writes `lines` at the file's end; callers count them once it returns. A write that fails
    /// (a full disk) cuts off what it wrote of them, so that no line is left cut short.
    ///
    /// On a full disk the lines are kept in memory, up to `HELD_LIMIT`, and written ahead of the
    /// next ones; the write counts as done.
    fn write(&mut self, lines: &[u8]) -> io::Result<()> {
        if self.unused {
            self.held.extend_from_slice(lines);
            return Ok(());
        }
        let before = std::mem::take(&mut self.held);
        let mut bytes = Vec::with_capacity(1 + before.len() + lines.len());
        if self.unterminated {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(&before);
        bytes.extend_from_slice(lines);
        let written = match self.file.as_mut() {
            Some(file) => write_whole(file, &bytes),
            None => create_file(&self.path).and_then(|mut file| {
                let written = write_whole(&mut file, &bytes);
                self.file = Some(file);
                written
            }),
        };
        if let Err(error) = written {
            if let Some(file) = &self.file {
                let _ = crate::os::truncate(file, &self.path, self.size);
            }
            if !full(&error) || bytes.len() > HELD_LIMIT {
                self.held = before;
                return Err(error);
            }
            if !self.failing {
                self.change = Some(Saving::Held(error.to_string()));
            }
            self.failing = true;
            self.held = bytes;
            self.unterminated = false;
            self.keep_held();
            return Ok(());
        }
        self.discard_kept();
        self.size += bytes.len() as u64;
        self.unterminated = false;
        if std::mem::take(&mut self.failing) {
            self.change = match self.change {
                Some(Saving::Held(_)) => None,
                _ => Some(Saving::Saved),
            };
        }
        Ok(())
    }

    /// Copies the lines held in memory where they outlive the process; without a copy they are
    /// only in memory, as before there was one.
    fn keep_held(&mut self) {
        let Some(directory) = &self.keep_in else {
            return;
        };
        if self.kept.is_none() {
            self.kept = kept::Kept::create(directory, &self.path).ok();
        }
        if let Some(kept) = &mut self.kept
            && kept.keep(&self.path, self.size, &self.held).is_err()
        {
            self.discard_kept();
        }
    }

    fn discard_kept(&mut self) {
        if let Some(kept) = self.kept.take() {
            kept.discard();
        }
    }

    /// Where the lines held in memory are copied to outlive the process, while they are.
    pub fn kept_path(&self) -> Option<&Path> {
        self.kept.as_ref().map(kept::Kept::path)
    }

    /// Takes in the lines another process appended to the file since this one last read or wrote
    /// it, so that line numbers and branches stay right: its line count, where a line without a
    /// branch line continues, and the name it may have given. A last line left cut short is cut
    /// off, so that the next line starts on a line of its own; callers hold `writing`.
    ///
    /// Lines held in memory were numbered without the other process's; they keep their numbers.
    fn catch_up(&mut self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if self.held.is_empty() && file.metadata()?.len() != self.size {
            let entries = read(&self.path)?;
            if file.metadata()?.len() > entries.size {
                crate::os::truncate(file, &self.path, entries.size)?;
            }
            self.skipped = entries.skipped;
            self.unterminated = entries.unterminated;
            self.lines = entries.lines;
            self.tail = entries.tail;
            self.name = entries.name;
            self.size = entries.size;
            self.tokens = entries.tokens;
            self.unused = false;
        }
        Ok(())
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The lines after the header (numbered from 0, the header's) that could not be read when
    /// the session was opened, and that it goes on without.
    pub fn skipped(&self) -> &[usize] {
        &self.skipped
    }

    /// The models that replied on the current branch, compactions aside, first ones first.
    pub fn contributors(&self) -> &[String] {
        &self.contributors
    }

    /// The thinking level the latest change on the path set, if any.
    pub fn effort(&self) -> Option<&str> {
        self.effort.as_deref()
    }

    pub fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// The tokens of every response in the file, on every branch.
    pub fn tokens(&self) -> Tokens {
        self.tokens
    }

    /// The latest `command_state` line on the path, if any.
    pub fn command(&self) -> Option<&Value> {
        self.command.as_ref()
    }

    /// Saves the active command's state, `line` (`status::Lifecycle::saved`), unless the path
    /// already ends with it; a path without one has no command.
    pub fn record_command(&mut self, line: &Value) -> io::Result<()> {
        let unchanged = match &self.command {
            Some(saved) => saved == line,
            None => line["command"].is_null(),
        };
        if !unchanged {
            self.append(line)?;
        }
        Ok(())
    }

    /// Whether the latest `/autopilot` toggle on the path turned it on; `None` without one.
    pub fn autopilot(&self) -> Option<bool> {
        self.autopilot
    }

    /// Saves an `/autopilot` toggle.
    pub fn record_autopilot(&mut self, on: bool) -> io::Result<()> {
        self.append(&json!({"type":"autopilot","on":on}))
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

    /// Records that later requests use reasoning effort `level`, unless the file already says so.
    pub fn record_effort(&mut self, level: &str) -> io::Result<()> {
        if self.effort.as_deref() != Some(level) {
            self.append(&json!({"type":"thinking_level_change","level":level}))?;
        }
        Ok(())
    }

    /// Appends `item` as the entry after the leaf, with a branch line before it when the leaf is
    /// not the last entry, and makes it the leaf.
    pub fn append(&mut self, item: &Value) -> io::Result<()> {
        let _writing = self.writing()?;
        self.catch_up()?;
        let branched = self.leaf != self.tail;
        let mut lines = Vec::new();
        if branched {
            lines = serde_json::to_vec(&json!({"type":"branch","parent":self.leaf}))?;
            lines.push(b'\n');
        }
        lines.extend(serde_json::to_vec(item)?);
        lines.push(b'\n');
        let kind = item["type"].as_str();
        let used = !matches!(
            kind,
            Some("model_change" | "thinking_level_change" | "autopilot")
        );
        let unused = self.unused;
        self.unused &= !used;
        if let Err(error) = self.write(&lines) {
            self.unused = unused;
            return Err(error);
        }
        let line = self.lines + usize::from(branched);
        self.lines = line + 1;
        self.leaf = Some(line);
        self.tail = self.leaf;
        if item["type"] == "model_change" {
            self.model = item["model"].as_str().map(str::to_owned);
        } else if item["type"] == "thinking_level_change" {
            self.effort = item["level"].as_str().map(str::to_owned);
        } else if item["type"] == "command_state" {
            self.command = Some(item.clone());
        } else if item["type"] == "autopilot" {
            self.autopilot = item["on"].as_bool();
        } else if item["type"] == "compaction" {
            self.files = FileLists::of(item);
            self.context_tokens = None;
        } else if let Some(producer) = item[super::PRODUCER].as_str() {
            self.model = Some(producer.to_owned());
            self.context_tokens = item[super::CONTEXT].as_u64();
            contribute(&mut self.contributors, producer);
        }
        self.tokens.add(item);
        Ok(())
    }
}

#[cfg(test)]
mod slug_tests {
    #[test]
    fn a_projects_sessions_directory_is_named_by_its_path_in_one_component() {
        assert_eq!(super::slug("/home/a/repos/x"), "home-a-repos-x");
        assert_eq!(super::slug("/"), "");
        let windows = super::slug(r"C:\Users\a\repo");
        assert_eq!(windows == "C--Users-a-repo", cfg!(windows));
    }
}
