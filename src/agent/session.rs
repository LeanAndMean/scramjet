use super::Tokens;
use super::compaction::FileLists;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

/// A saved session as `/resume` lists it.
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
    pub modified: SystemTime,
    /// The session it was forked from, as its header records it.
    pub parent: Option<PathBuf>,
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
    file: File,
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
    /// names and `/autopilot` toggles, which Scramjet would not have written yet.
    unused: bool,
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
    /// The header's `parentSession`: the session this one was forked from.
    parent: Option<PathBuf>,
    header: Value,
    tokens: Tokens,
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
    let mut reader = BufReader::new(File::open(path)?);
    let mut entries: Vec<Entry> = Vec::new();
    let mut tail = None;
    let mut name = None;
    let mut labels = HashMap::new();
    let mut parent = None;
    let mut header = Value::Null;
    let mut tokens = Tokens::default();
    let mut line = String::new();
    let mut end = 0;
    let mut lines = 0;
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        let index = lines;
        lines += 1;
        let start = end;
        end += read;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(&line).map_err(|_| invalid(path, "malformed line"))?;
        if index == 0 {
            if value["type"] != "session" {
                return Err(invalid(path, "not a scramjet-rs session"));
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
            tail = match &value["parent"] {
                Value::Null => None,
                parent => Some(
                    parent
                        .as_u64()
                        .and_then(|parent| usize::try_from(parent).ok())
                        .filter(|&parent| find(&entries, parent).is_some())
                        .ok_or_else(|| invalid(path, "invalid branch parent"))?,
                ),
            };
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

/// Creates a session file in `directory`, named for the millisecond it is created in and this
/// process; returns it with that time.
fn new_file(directory: &Path) -> io::Result<(PathBuf, File, u128)> {
    std::fs::create_dir_all(directory)?;
    let mut created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // A session created in the same millisecond (a quick `/new` or `/fork`) has that name.
    loop {
        let path = directory.join(format!("{created}-{}.jsonl", std::process::id()));
        match OpenOptions::new().append(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file, created)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => created += 1,
            Err(error) => return Err(error),
        }
    }
}

/// The current branch of the session saved at `path`, where a resume would continue it.
pub fn read_branch(path: &Path) -> io::Result<Branch> {
    let entries = read(path)?;
    Ok(entries.branch(entries.tail))
}

/// `$XDG_STATE_HOME/scramjet-rs/sessions/<project path with / as ->`.
pub fn directory(root: &Path) -> io::Result<PathBuf> {
    let slug = root.to_string_lossy().trim_matches('/').replace('/', "-");
    Ok(state_directory()?.join("sessions").join(slug))
}

/// `$XDG_STATE_HOME/scramjet-rs`, by default `~/.local/state/scramjet-rs`.
pub fn state_directory() -> io::Result<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .ok_or_else(|| io::Error::other("neither XDG_STATE_HOME nor HOME is set"))?;
    Ok(state.join("scramjet-rs"))
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
                modified,
                parent: entries.parent.clone(),
            });
        }
        sessions.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| b.path.cmp(&a.path))
        });
        Ok(sessions)
    }

    /// Starts a session for the project `root` in `directory`; a fork names the session it came
    /// from, `parent`, in its header, as Scramjet's `parentSession`.
    pub fn create(directory: &Path, root: &Path, parent: Option<&Path>) -> io::Result<Self> {
        let (path, mut file, created) = new_file(directory)?;
        let mut header = json!({"type":"session","version":1,"root":root,"created":created as u64});
        if let Some(parent) = parent {
            header["parentSession"] = json!(std::path::absolute(parent)?);
        }
        let mut header = serde_json::to_vec(&header)?;
        header.push(b'\n');
        file.write_all(&header)?;
        let _ = file.lock_shared();
        Ok(Self {
            path,
            file,
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
            size: header.len() as u64,
            unused: true,
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
    pub fn open(path: &Path) -> io::Result<(Self, Vec<Value>, Vec<String>)> {
        let entries = read(path)?;
        let context = entries.context(path, entries.tail)?;
        let file = OpenOptions::new().append(true).open(path)?;
        // Held while open, so that the process that created it does not discard it.
        let _ = file.lock_shared();
        Ok((
            Self {
                path: path.to_owned(),
                file,
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
                size: entries.size,
                unused: false,
            },
            context.items,
            entries.prompts(),
        ))
    }

    /// The prompts this session can be forked before, in file order, on every branch and
    /// including those a compaction summarized, as Scramjet's `/fork` lists its user messages.
    pub fn fork_prompts(&self) -> io::Result<Vec<String>> {
        Ok(read(&self.path)?
            .fork_points()
            .map(|entry| prompt_text(&entry.value).to_owned())
            .collect())
    }

    /// Saves a session for the project `root` next to this one with the entries on the path to
    /// its prompt `index` (0-based, as `fork_prompts` lists them), that prompt left out, as
    /// Scramjet's `/fork` copies a session's path. Returns the new file and that prompt; `None`:
    /// no such prompt.
    pub fn fork(&self, root: &Path, index: usize) -> io::Result<Option<(PathBuf, String)>> {
        let entries = read(&self.path)?;
        let Some(fork) = entries.fork_points().nth(index) else {
            return Ok(None);
        };
        let prompt = prompt_text(&fork.value).to_owned();
        let path = self.branch_file(&entries, fork.parent, root, None)?;
        Ok(Some((path, prompt)))
    }

    /// Saves a session for the project `root` next to this one with the entries on the path to
    /// the leaf and the session's name, as Scramjet's `/clone` duplicates a session at its
    /// current position. Returns the new file.
    pub fn clone_at_leaf(&self, root: &Path) -> io::Result<PathBuf> {
        let entries = read(&self.path)?;
        self.branch_file(&entries, self.leaf, root, entries.name.as_deref())
    }

    /// Saves a session next to this one, forked from it, with the entries of `entries` (this
    /// file's) on the path to `leaf`, then `name` when given.
    fn branch_file(
        &self,
        entries: &Entries,
        leaf: Option<usize>,
        root: &Path,
        name: Option<&str>,
    ) -> io::Result<PathBuf> {
        let source = std::fs::read(&self.path)?;
        let history = entries.history(&self.path, &source, leaf, name)?;
        let directory = self.path.parent().unwrap_or(Path::new("."));
        let mut branched = Self::create(directory, root, Some(&self.path))?;
        if let Err(error) = branched.file.write_all(&history) {
            let _ = std::fs::remove_file(&branched.path);
            return Err(error);
        }
        Ok(branched.path)
    }

    /// The conversation from the start to the leaf.
    pub fn branch(&self) -> io::Result<Branch> {
        Ok(read(&self.path)?.branch(self.leaf))
    }

    /// Writes the session's current branch to `to` as a session file of its own, as Scramjet's
    /// `/export` writes a `.jsonl`: its header, without the session it was forked from, the
    /// entries on the path to the leaf in order, and its name. An existing file is replaced.
    pub fn export(&self, to: &Path) -> io::Result<()> {
        let entries = read(&self.path)?;
        let source = std::fs::read(&self.path)?;
        let mut header = entries.header.clone();
        if let Some(header) = header.as_object_mut() {
            header.remove("parentSession");
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
    /// is listed and continued as one made there; returns the copy. A file that is not a session
    /// is not copied.
    pub fn import(directory: &Path, source: &Path) -> io::Result<PathBuf> {
        let bytes = std::fs::read(source).map_err(|error| {
            io::Error::new(error.kind(), format!("{}: {error}", source.display()))
        })?;
        read(source)?;
        let (path, mut file, _) = new_file(directory)?;
        if let Err(error) = file.write_all(&bytes) {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        Ok(path)
    }

    /// Every entry of the session, in file order.
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        Ok(read(&self.path)?.entries)
    }

    /// Every entry of the session, in file order, and the labels of those labeled, by line.
    pub fn labeled_entries(&self) -> io::Result<(Vec<Entry>, HashMap<usize, Label>)> {
        let entries = read(&self.path)?;
        Ok((entries.entries, entries.labels))
    }

    /// Gives the entry on `line` the label `text`, or takes its label away (`None`), as
    /// Scramjet's tree does; a line of its own, as a name is. Returns the label given.
    pub fn set_label(&mut self, line: usize, text: Option<&str>) -> io::Result<Option<Label>> {
        let text = text.filter(|text| !text.is_empty());
        let _writing = self.writing()?;
        self.catch_up()?;
        if find(&read(&self.path)?.entries, line).is_none() {
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
        let entries = read(&self.path)?;
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

    /// The entries on the path to the leaf after the last one it shares with the path to
    /// `target`: the branch a move to `target` leaves, as Scramjet collects it for a summary.
    pub fn left_behind(&self, target: Option<usize>) -> io::Result<Vec<Value>> {
        let entries = read(&self.path)?;
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
        let modified = session.file.metadata()?.modified()?;
        session.write_name(name)?;
        session.file.set_modified(modified)
    }

    /// Removes the session saved at `path`, as Scramjet's `/resume` deletes one: to the trash with
    /// the `trash` command when there is one, else for good. A file that is not a session stays.
    pub fn delete(path: &Path) -> io::Result<Removal> {
        Self::delete_with(path, "trash")
    }

    /// As `delete`, with `trash` for the `trash` command.
    pub(super) fn delete_with(path: &Path, trash: &str) -> io::Result<Removal> {
        read(path)?;
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

    /// Removes the file of a session this process created and left without a conversation, as
    /// Scramjet writes none before the first reply; one another process wrote to or has open stays.
    pub fn discard_if_unused(self) -> io::Result<()> {
        if self.unused && self.file.try_lock().is_ok() && self.file.metadata()?.len() == self.size {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    /// Locks the session directory until dropped, so that no other process's line lands between
    /// `catch_up` and the write after it. Without `flock`, writes go unserialized.
    fn writing(&self) -> io::Result<File> {
        let directory = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        let directory = File::open(directory)?;
        let _ = directory.lock();
        Ok(directory)
    }

    /// Writes `lines` at the file's end; callers count them once it returns.
    fn write(&mut self, lines: &[u8]) -> io::Result<()> {
        self.file.write_all(lines)?;
        self.size += lines.len() as u64;
        Ok(())
    }

    /// Takes in the lines another process appended to the file since this one last read or wrote
    /// it, so that line numbers and branches stay right: its line count, where a line without a
    /// branch line continues, and the name it may have given.
    fn catch_up(&mut self) -> io::Result<()> {
        if self.file.metadata()?.len() != self.size {
            let entries = read(&self.path)?;
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
        self.write(&lines)?;
        let line = self.lines + usize::from(branched);
        self.lines = line + 1;
        self.leaf = Some(line);
        self.tail = self.leaf;
        let kind = item["type"].as_str();
        if !matches!(
            kind,
            Some("model_change" | "thinking_level_change" | "autopilot")
        ) {
            self.unused = false;
        }
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
