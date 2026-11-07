use super::{
    CompactRead, ToolDetail,
    image::{self, Image},
    instructions,
    sandbox::{self, Hidden, Sandbox},
};
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

mod matching;

pub(super) const MAX_LINES: usize = 2000;
pub(super) const MAX_BYTES: usize = 50 * 1024;
/// How the line that says what a cut read left out starts.
pub const READ_MORE: &str = "[Showing lines ";
/// Bash output kept in memory while a command runs; only the tail is shown to the model.
const CAPTURE_BYTES: usize = 4 << 20;
/// How often a running command's output is passed on to be shown, as in Scramjet.
const OUTPUT_UPDATE: Duration = Duration::from_millis(100);
/// How much of the end of a running command's output is passed on.
const OUTPUT_TAIL_BYTES: usize = 8 << 10;
const DEFAULT_TIMEOUT: u64 = 120;
/// Pi's default number of entries `ls` lists, of matches `grep` returns and of paths `find`
/// returns.
const LS_LIMIT: usize = 500;
const GREP_LIMIT: usize = 100;
const FIND_LIMIT: usize = 1000;
/// Pi cuts a longer `grep` line to this many characters.
const GREP_LINE_CHARS: usize = 500;
const MAX_TIMEOUT: u64 = 3600;
/// Unchanged lines shown around each change in an edit diff, as in Scramjet.
const DIFF_CONTEXT: usize = 4;

/// The coding tools the agent can be given, in the order the model sees them.
pub const CODING_TOOLS: [&str; 7] = ["read", "bash", "edit", "write", "grep", "find", "ls"];
/// Scramjet's, which the agent has unless the settings list others.
pub const DEFAULT_TOOLS: [&str; 4] = ["read", "bash", "edit", "write"];

/// The harness's own tools, which the model sees after the coding tools, in this order.
pub const HARNESS_TOOLS: [&str; 12] = [
    super::status::NAME,
    super::publish::TOOLS[0],
    super::publish::TOOLS[1],
    super::publish::TOOLS[2],
    super::publish::TOOLS[3],
    super::question::NAME,
    super::delegate::NAME,
    super::subagent::NAME,
    super::switch::NAME,
    super::suggest::NAME,
    super::mcp::SEARCH,
    super::mcp::CALL,
];

/// The tools the model has, in the order it sees them: the coding tools `coding` lists, then the
/// harness's own, those for MCP servers' tools only with `mcp` (servers are configured); with
/// `only` (`--tools`), those it names alone, as Scramjet's allowlist.
pub(super) fn offered(coding: &[String], only: Option<&[String]>, mcp: bool) -> Vec<&'static str> {
    let named = |names: &[String], tool: &str| names.iter().any(|name| name == tool);
    let coding = (CODING_TOOLS.into_iter()).filter(|tool| named(only.unwrap_or(coding), tool));
    let harness = (HARNESS_TOOLS.into_iter())
        .filter(|tool| only.is_none_or(|only| named(only, tool)))
        .filter(|tool| mcp || !super::mcp::TOOLS.contains(tool));
    coding.chain(harness).collect()
}

/// The definitions of the tools `offered` names, in the order `offered` gives them.
pub(super) fn definitions(offered: &[&str]) -> Value {
    let mut tools: Vec<Value> = (CODING_TOOLS.iter())
        .map(|tool| definition(tool, false))
        .collect();
    tools.push(super::status::definition());
    tools.extend(super::publish::definitions());
    tools.push(super::question::definition());
    tools.push(super::delegate::definition());
    tools.push(super::subagent::definition());
    tools.push(super::switch::definition());
    tools.push(super::suggest::definition());
    tools.extend(super::mcp::definitions());
    tools.retain(|tool| (tool["name"].as_str()).is_some_and(|name| offered.contains(&name)));
    Value::Array(tools)
}

/// The definitions of a subagent's tools, `names` among `read`, `grep`, `find`, `ls` and `bash`.
pub(super) fn child_definitions(names: &[&str]) -> Value {
    let tools = ["read", "grep", "find", "ls", "bash"]
        .into_iter()
        .filter(|tool| names.contains(tool))
        .map(|tool| definition(tool, true));
    Value::Array(tools.collect())
}

/// Pi's definition of the coding tool `name`; `sandboxed`: a bash that runs in the read-only
/// sandbox.
fn definition(name: &str, sandboxed: bool) -> Value {
    match name {
        "read" => json!({"type":"function","name":"read","strict":false,
         "description":"Read a text file or an image (jpg, png, gif, webp), which is sent as an attachment. Text is truncated to 2000 lines or 50KB; use offset/limit (1-based line numbers) to read large files in parts.",
         "parameters":{"type":"object","properties":{
            "path":{"type":"string","description":"File path, relative to the project directory or absolute"},
            "offset":{"type":"integer","description":"First line to read (1-based)"},
            "limit":{"type":"integer","description":"Maximum number of lines to read"}},
          "required":["path"]}}),
        "write" => json!({"type":"function","name":"write","strict":false,
         "description":"Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Automatically creates parent directories.",
         "parameters":{"type":"object","properties":{
            "path":{"type":"string","description":"Path to the file to write, relative to the project directory or absolute"},
            "content":{"type":"string","description":"Content to write to the file"}},
          "required":["path","content"]}}),
        "edit" => json!({"type":"function","name":"edit","strict":false,
         "description":"Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes.",
         "parameters":{"type":"object","properties":{
            "path":{"type":"string","description":"Path to the file to edit, relative to the project directory or absolute"},
            "edits":{"type":"array",
                "description":"One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits. If two changes touch the same block or nearby lines, merge them into one edit instead.",
                "items":{"type":"object","properties":{
                "oldText":{"type":"string","description":"Exact text for one targeted replacement. It must be unique in the original file and must not overlap with any other edits[].oldText in the same call."},
                "newText":{"type":"string","description":"Replacement text for this targeted edit."}},"required":["oldText","newText"]}}},
          "required":["path","edits"]}}),
        "bash" => json!({"type":"function","name":"bash","strict":false,
         "description":match sandboxed {
            true => "Run a bash command in the working directory, in a read-only sandbox without network: no file can be changed, except in a private /tmp that is discarded when the command ends. Returns combined stdout/stderr, truncated to the last 2000 lines or 50KB; a non-zero exit code is reported after the output.",
            false => "Run a bash command in the project directory. Returns combined stdout/stderr, truncated to the last 2000 lines or 50KB; a non-zero exit code is reported after the output.",
         },
         "parameters":{"type":"object","properties":{
            "command":{"type":"string","description":"Bash command to execute"},
            "timeout":{"type":"integer","description":"Seconds before the command is killed (default 120)"}},
          "required":["command"]}}),
        "grep" => json!({"type":"function","name":"grep","strict":false,
        "description":format!("Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to {GREP_LIMIT} matches or {}KB (whichever is hit first). Long lines are truncated to {GREP_LINE_CHARS} chars.", MAX_BYTES / 1024),
        "parameters":{"type":"object","properties":{
            "pattern":{"type":"string","description":"Search pattern (regex or literal string)"},
            "path":{"type":"string","description":"Directory or file to search (default: current directory)"},
            "glob":{"type":"string","description":"Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'"},
            "ignoreCase":{"type":"boolean","description":"Case-insensitive search (default: false)"},
            "literal":{"type":"boolean","description":"Treat pattern as literal string instead of regex (default: false)"},
            "context":{"type":"integer","description":"Number of lines to show before and after each match (default: 0)"},
            "limit":{"type":"integer","description":format!("Maximum number of matches to return (default: {GREP_LIMIT})")}},
          "required":["pattern"]}}),
        "find" => json!({"type":"function","name":"find","strict":false,
        "description":format!("Search for files by glob pattern. Returns matching file paths relative to the search directory. Respects .gitignore. Output is truncated to {FIND_LIMIT} results or {}KB (whichever is hit first).", MAX_BYTES / 1024),
        "parameters":{"type":"object","properties":{
            "pattern":{"type":"string","description":"Glob pattern to match files, e.g. '*.ts', '**/*.json', or 'src/**/*.spec.ts'"},
            "path":{"type":"string","description":"Directory to search in (default: current directory)"},
            "limit":{"type":"integer","description":format!("Maximum number of results (default: {FIND_LIMIT})")}},
          "required":["pattern"]}}),
        _ => json!({"type":"function","name":"ls","strict":false,
        "description":format!("List directory contents. Returns entries sorted alphabetically, with '/' suffix for directories. Includes dotfiles. Output is truncated to {LS_LIMIT} entries or {}KB (whichever is hit first).", MAX_BYTES / 1024),
        "parameters":{"type":"object","properties":{
            "path":{"type":"string","description":"Directory to list (default: current directory)"},
            "limit":{"type":"integer","description":format!("Maximum number of entries to return (default: {LS_LIMIT})")}}}}),
    }
}

/// What the user is asked to approve before a tool runs, if anything, when approvals are on.
pub(super) enum Approval {
    None,
    Ask(String),
}

pub(super) struct Call<'a> {
    pub name: &'a str,
    pub arguments: Value,
}

impl<'a> Call<'a> {
    pub fn parse(item: &'a Value) -> Result<Self, String> {
        let name = item["name"].as_str().unwrap_or_default();
        let arguments = serde_json::from_str(item["arguments"].as_str().unwrap_or("{}"))
            .map_err(|e| format!("invalid tool arguments: {e}"))?;
        // An MCP server's tool offered directly runs, shows and asks as its `mcp_call`.
        if let Some((server, tool)) = super::mcp::direct_name(name) {
            return Ok(Self {
                name: super::mcp::CALL,
                arguments: json!({"server": server, "tool": tool, "arguments": arguments}),
            });
        }
        Ok(Self { name, arguments })
    }
    fn str(&self, field: &str) -> Result<&str, String> {
        self.arguments[field]
            .as_str()
            .ok_or_else(|| format!("missing string argument `{field}`"))
    }
    /// A one-line description for display.
    pub fn summary(&self) -> String {
        match self.name {
            "bash" => self.arguments["command"].as_str().unwrap_or("").to_owned(),
            "ls" => shortened(self.arguments["path"].as_str().unwrap_or(".")),
            "grep" | "find" => {
                let pattern = self.arguments["pattern"].as_str().unwrap_or("");
                let path = shortened(self.arguments["path"].as_str().unwrap_or("."));
                match self.name {
                    "grep" => format!("/{pattern}/ in {path}"),
                    _ => format!("{pattern} in {path}"),
                }
            }
            super::subagent::NAME => super::subagent::summary(&self.arguments),
            super::delegate::NAME => super::delegate::summary(&self.arguments),
            super::suggest::NAME => super::suggest::summary(&self.arguments),
            super::switch::NAME => super::switch::summary(&self.arguments),
            name if super::publish::TOOLS.contains(&name) => {
                super::publish::summary(name, &self.arguments)
            }
            super::status::NAME => self.arguments["status"].as_str().unwrap_or("").to_owned(),
            super::mcp::SEARCH | super::mcp::CALL => {
                super::mcp::summary_line(self.name, &self.arguments)
            }
            super::status::RECORD => super::status::record_summary(&self.arguments),
            "read" => {
                let path = shortened(self.arguments["path"].as_str().unwrap_or(""));
                path + &self.lines()
            }
            _ => shortened(self.arguments["path"].as_str().unwrap_or("")),
        }
    }
    /// A read's line range as its call line shows it (`:3-10`, `:3`), if it asks for one.
    fn lines(&self) -> String {
        let (offset, limit) = (
            self.arguments["offset"].as_u64(),
            self.arguments["limit"].as_u64(),
        );
        let start = offset.unwrap_or(1).max(1);
        match limit {
            _ if offset.is_none() && limit.is_none() => String::new(),
            Some(limit) => format!(":{start}-{}", (start + limit).saturating_sub(1)),
            None => format!(":{start}"),
        }
    }
    /// As Scramjet's compact reads, a read of a skill's `SKILL.md`, an `AGENTS.md` or
    /// `CLAUDE.md`, or the README the instructions point to, by its path in `root`.
    pub fn compact(&self, root: &Path) -> Option<CompactRead> {
        let given = (self.arguments["path"].as_str()).filter(|_| self.name == "read")?;
        let path = resolve(root, given);
        let name = path.file_name()?.to_str()?;
        let lines = self.lines();
        if name == "SKILL.md" {
            let skill = path.parent().and_then(Path::file_name);
            let skill = skill.map_or(name.into(), |skill| skill.to_string_lossy());
            return Some(CompactRead::Skill(format!("{skill}{lines}")));
        }
        if path == Path::new(instructions::README) {
            return Some(CompactRead::Docs(format!("README.md{lines}")));
        }
        if !matches!(name, "AGENTS.md" | "AGENTS.MD" | "CLAUDE.md" | "CLAUDE.MD") {
            return None;
        }
        let shown = match path.strip_prefix(root) {
            Ok(relative) => relative.display().to_string(),
            Err(_) => shortened(&path.to_string_lossy()),
        };
        Some(CompactRead::Resource(format!("{shown}{lines}")))
    }
    pub fn approval(&self, root: &Path) -> Approval {
        match self.name {
            super::mcp::CALL => Approval::Ask(format!(
                "Call {} on MCP server {}",
                self.arguments["tool"].as_str().unwrap_or(""),
                self.arguments["server"].as_str().unwrap_or("")
            )),
            // The call line above the question shows the whole command.
            "bash" => Approval::Ask(format!("Run in {}", root.display())),
            "write" | "edit" => {
                let path = self.arguments["path"].as_str().unwrap_or("");
                if resolve(root, path).starts_with(root) {
                    Approval::None
                } else {
                    Approval::Ask(format!("{} outside the project: {path}", self.name))
                }
            }
            _ => Approval::None,
        }
    }
    /// `progress` receives the end of a running command's output as it grows. With `hidden`, the
    /// tools do not show or change the credential stores: a path in them is refused, `grep` and
    /// `find` run in the read-only sandbox and bash with the stores hidden.
    pub fn run(
        &self,
        root: &Path,
        hidden: Option<&Hidden>,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Output, String> {
        if let Some(hidden) = hidden {
            self.check_path(root, hidden)?;
        }
        let text = match self.name {
            "read" => return read(root, self, hidden),
            "ls" => ls(root, self)?,
            "grep" => grep(root, self, hidden, cancel)?,
            "find" => find(root, self, hidden, cancel)?,
            "write" | "edit" => {
                let put = |path: &Path, content: &str| put(path, content, hidden);
                return match self.name {
                    "write" => write(root, self, &put),
                    _ => edit(root, self, hidden, &put),
                };
            }
            "bash" => return bash(root, self, hidden, cancel, progress),
            other => return Err(format!("Tool {other} not found")),
        };
        Ok(Output {
            text,
            detail: None,
            failed: false,
            image: None,
        })
    }

    /// As `run` in the main agent's `sandbox`, with the credential stores `hidden`: bash runs
    /// confined as it says, and `write` and `edit` only where it lets them.
    pub fn run_in_sandbox(
        &self,
        root: &Path,
        sandbox: Sandbox,
        hidden: &Hidden,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Output, String> {
        match self.name {
            "write" | "edit" => {
                let path = resolve(root, self.arguments["path"].as_str().unwrap_or(""));
                sandbox.check_write(root, &path)?;
                self.check_path(root, hidden)?;
                let put =
                    |path: &Path, content: &str| sandbox.write(root, path, content.as_bytes());
                match self.name {
                    "write" => write(root, self, &put),
                    _ => edit(root, self, Some(hidden), &put),
                }
            }
            "bash" => {
                let process = sandbox::sandboxed(root, self.str("command")?, hidden, sandbox)?;
                let timeout = Some(timeout(self));
                Ok(
                    run_process(process, sandbox::PROGRAM, timeout, cancel, progress)?
                        .tool_output(),
                )
            }
            _ => self.run(root, Some(hidden), cancel, progress),
        }
    }

    /// As `run` for a subagent's tools, none of which changes a file or shows the credential
    /// stores `hidden`: bash, `grep` and `find` run in the read-only sandbox.
    pub fn run_read_only(
        &self,
        root: &Path,
        hidden: &Hidden,
        cancel: &AtomicBool,
    ) -> Result<Output, String> {
        self.check_path(root, hidden)?;
        let text = match self.name {
            "read" => return read(root, self, Some(hidden)),
            "ls" => ls(root, self)?,
            "grep" => grep(root, self, Some(hidden), cancel)?,
            "find" => find(root, self, Some(hidden), cancel)?,
            "bash" => {
                let ran = run_sandboxed(
                    root,
                    self.str("command")?,
                    hidden,
                    Some(timeout(self)),
                    cancel,
                    &mut |_| {},
                )?;
                return Ok(ran.tool_output());
            }
            other => return Err(format!("Tool {other} not found")),
        };
        Ok(Output {
            text,
            detail: None,
            failed: false,
            image: None,
        })
    }

    /// Refuses a path argument in a hidden credential store; bash has none.
    fn check_path(&self, root: &Path, hidden: &Hidden) -> Result<(), String> {
        match self.name {
            "bash" => Ok(()),
            _ => hidden.check(&resolve(
                root,
                self.arguments["path"].as_str().unwrap_or("."),
            )),
        }
    }
}

#[derive(Debug)]
pub(super) struct Output {
    /// What the model receives, with `image`.
    pub text: String,
    pub detail: Option<ToolDetail>,
    /// The tool ran but reports failure, such as a command's non-zero exit.
    pub failed: bool,
    pub image: Option<Image>,
}

impl Output {
    pub fn failure(text: String) -> Self {
        Self {
            text,
            detail: None,
            failed: true,
            image: None,
        }
    }
    /// The result's `output` as the conversation keeps it: the text, or with an image the
    /// Responses content parts.
    pub fn sent(&self) -> Value {
        match &self.image {
            None => json!(self.text),
            Some(image) => json!([{"type":"input_text","text":self.text}, image.part()]),
        }
    }
}

/// The text of a tool result's `output`, without the images it may have.
pub(super) fn result_text(output: &Value) -> String {
    match output {
        Value::Array(parts) => (parts.iter())
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        output => output.as_str().unwrap_or_default().to_owned(),
    }
}

/// How many images a tool result's `output` has.
pub(super) fn result_images(output: &Value) -> usize {
    (output.as_array().into_iter().flatten())
        .filter(|part| part["type"] == "input_image")
        .count()
}

/// How many images a conversation item has: a tool result's, or a user message's.
pub(super) fn item_images(item: &Value) -> usize {
    result_images(&item["output"]) + result_images(&item["content"])
}

/// Writes `text` to a file beside `path`'s target and renames it over the target, so a reader
/// never sees part of it. A symbolic link stays one.
pub fn write_replacing(path: &Path, text: &str) -> io::Result<()> {
    let target = match std::fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)?;
            }
            path.to_path_buf()
        }
        Err(error) => return Err(error),
    };
    let mut temporary = target.clone().into_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let written =
        std::fs::write(&temporary, text).and_then(|()| std::fs::rename(&temporary, &target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// A path in the home directory as `~/...`, as Scramjet shows tool paths.
pub fn shortened(path: &str) -> String {
    let home = std::env::home_dir().filter(|home| home.parent().is_some());
    match home
        .as_ref()
        .and_then(|home| home.to_str())
        .map(|home| path.strip_prefix(home))
    {
        Some(Some(rest)) if rest.is_empty() || rest.starts_with(std::path::is_separator) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

/// Resolves `path` against the project root without following symlinks. A leading `~` is the
/// home directory, as in Scramjet.
pub(super) fn resolve(root: &Path, path: &str) -> PathBuf {
    let joined = match crate::os::after_tilde(path).zip(std::env::home_dir()) {
        Some((rest, home)) => home.join(rest),
        None => root.join(path),
    };
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

/// The contents of the file at `path`, checked against the stores `hidden` as opened.
fn contents(path: &Path, hidden: Option<&Hidden>) -> Result<Vec<u8>, String> {
    let failed = |e: io::Error| format!("{}: {e}", path.display());
    let mut file = match hidden {
        Some(hidden) => hidden.opened(path, std::fs::File::open(path))?,
        None => std::fs::File::open(path).map_err(failed)?,
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(failed)?;
    Ok(bytes)
}

fn read(root: &Path, call: &Call, hidden: Option<&Hidden>) -> Result<Output, String> {
    let given = call.str("path")?;
    let path = resolve(root, given);
    let bytes = contents(&path, hidden)?;
    let (text, image) = match image::mime(&bytes) {
        Some(mime) => read_image(&bytes, mime),
        None => (read_text(bytes, call, &path)?, None),
    };
    Ok(Output {
        text,
        detail: Some(ToolDetail::Read(given.to_owned())),
        failed: false,
        image,
    })
}

/// Scramjet's text for an image, which says how it was scaled or why it is left out.
fn read_image(bytes: &[u8], mime: &'static str) -> (String, Option<Image>) {
    match image::fit(bytes, mime) {
        Some(fitted) => {
            let mut text = format!("Read image file [{}]", fitted.image.mime);
            if let Some(note) = fitted.note() {
                text = format!("{text}\n{note}");
            }
            (text, Some(fitted.image))
        }
        None => (
            format!(
                "Read image file [{mime}]\n[Image omitted: could not be resized below the \
inline image size limit.]"
            ),
            None,
        ),
    }
}

fn read_text(bytes: Vec<u8>, call: &Call, path: &Path) -> Result<String, String> {
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("{} is not a UTF-8 text file", path.display()))?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let offset = call.arguments["offset"].as_u64().unwrap_or(1).max(1) as usize;
    if offset > lines.len().max(1) {
        return Err(format!(
            "offset {offset} is past the end of the file ({} lines)",
            lines.len()
        ));
    }
    let limit = call.arguments["limit"]
        .as_u64()
        .map_or(usize::MAX, |n| n as usize);
    let selected = &lines[offset - 1..lines.len().min((offset - 1).saturating_add(limit))];
    let mut output = String::new();
    let mut shown = 0;
    for line in selected {
        if shown == MAX_LINES || output.len() + line.len() > MAX_BYTES {
            break;
        }
        output.push_str(line);
        shown += 1;
    }
    let last = offset - 1 + shown;
    if last < lines.len() {
        output.push_str(&format!(
            "\n{READ_MORE}{offset}-{last} of {}. Use offset={} to continue.]",
            lines.len(),
            last + 1
        ));
    }
    Ok(output)
}

/// Writes a file, making the directories missing.
type Put<'a> = dyn Fn(&Path, &str) -> Result<(), String> + 'a;

/// Writes `content` to `path`, checked against the stores `hidden` as opened, before the file
/// is cut.
fn put(path: &Path, content: &str, hidden: Option<&Hidden>) -> Result<(), String> {
    let failed = |e: io::Error| format!("{}: {e}", path.display());
    let Some(hidden) = hidden else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        return std::fs::write(path, content).map_err(failed);
    };
    #[cfg(windows)]
    {
        use std::io::Write;
        hidden.check(path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(path);
        let mut file = hidden.opened(path, file)?;
        file.set_len(0).map_err(failed)?;
        file.write_all(content.as_bytes()).map_err(failed)
    }
    #[cfg(unix)]
    put_checked(path, content, hidden)
}

#[cfg(unix)]
fn put_checked(path: &Path, content: &str, hidden: &Hidden) -> Result<(), String> {
    use std::io::Write;
    let failed = |e: io::Error| format!("{}: {e}", path.display());
    use rustix::{
        fs::{Mode, OFlags},
        io::Errno,
    };
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(format!("{}: not a file", path.display()));
    };
    // Made only in a directory checked as opened, and a symlink there followed only to a file
    // that exists, so that nothing is made in a store before the check refuses it.
    let directory = checked_directory(parent, hidden)?;
    let flags = OFlags::WRONLY | OFlags::CLOEXEC;
    let made = OFlags::CREATE | OFlags::NOFOLLOW;
    let file = match rustix::fs::openat(&directory, name, flags | made, Mode::from_raw_mode(0o666))
    {
        Err(Errno::LOOP) => rustix::fs::openat(&directory, name, flags, Mode::empty()),
        opened => opened,
    };
    let file = file.map(std::fs::File::from).map_err(io::Error::from);
    let mut file = hidden.opened(path, file)?;
    file.set_len(0).map_err(failed)?;
    file.write_all(content.as_bytes()).map_err(failed)
}

/// The directory `path`, checked against the stores `hidden` as opened, made where missing in
/// the directory above it, itself checked first.
#[cfg(unix)]
fn checked_directory(path: &Path, hidden: &Hidden) -> Result<std::fs::File, String> {
    use rustix::{
        fs::{Mode, OFlags},
        io::Errno,
    };
    let flags = super::sandbox::LOOKUP | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let failed = |e: Errno| format!("{}: {}", path.display(), io::Error::from(e));
    let opened = match rustix::fs::open(path, flags, Mode::empty()) {
        Err(Errno::NOENT) => {
            let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                return Err(failed(Errno::NOENT));
            };
            let above = checked_directory(parent, hidden)?;
            match rustix::fs::mkdirat(&above, name, Mode::from_raw_mode(0o777)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(error) => return Err(failed(error)),
            }
            rustix::fs::openat(&above, name, flags | OFlags::NOFOLLOW, Mode::empty())
        }
        opened => opened,
    };
    let opened = opened.map(std::fs::File::from).map_err(io::Error::from);
    hidden.opened(path, opened)
}

fn write(root: &Path, call: &Call, put: &Put) -> Result<Output, String> {
    let given = call.str("path")?;
    let path = resolve(root, given);
    let content = call.str("content")?;
    put(&path, content)?;
    Ok(Output {
        text: format!("Wrote {} bytes to {}", content.len(), path.display()),
        detail: Some(ToolDetail::Written {
            path: given.to_owned(),
            content: content.to_owned(),
        }),
        failed: false,
        image: None,
    })
}

fn edit(root: &Path, call: &Call, hidden: Option<&Hidden>, put: &Put) -> Result<Output, String> {
    let path = resolve(root, call.str("path")?);
    let original = String::from_utf8(contents(&path, hidden)?)
        .map_err(|_| format!("{}: stream did not contain valid UTF-8", path.display()))?;
    // As Scramjet's `prepareEditArguments`: `edits` may come as a JSON string, and one edit's
    // fields beside it.
    let parsed: Option<Value>;
    let mut edits: Vec<&Value> = match &call.arguments["edits"] {
        Value::Array(edits) => edits.iter().collect(),
        Value::String(text) => {
            parsed = serde_json::from_str(text).ok();
            parsed
                .as_ref()
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |edits| edits.iter().collect())
        }
        _ => Vec::new(),
    };
    if call.arguments["oldText"].is_string() && call.arguments["newText"].is_string() {
        edits.push(&call.arguments);
    }
    if edits.is_empty() {
        return Err(
            "Edit tool input is invalid. edits must contain at least one replacement.".into(),
        );
    }
    // Scramjet's wording; `edits[N]` rather than `edit N`, which models read as 1-based.
    let single = edits.len() == 1;
    let shown = path.display();
    let finder = matching::Finder::new(&original);
    let mut spans = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let old = edit["oldText"]
            .as_str()
            .ok_or(format!("edits[{index}]: missing oldText"))?;
        let new = edit["newText"]
            .as_str()
            .ok_or(format!("edits[{index}]: missing newText"))?;
        if old.is_empty() {
            return Err(match single {
                true => format!("oldText must not be empty in {shown}."),
                false => format!("edits[{index}].oldText must not be empty in {shown}."),
            });
        }
        let (start, end, new) = match finder.find(old, new) {
            Ok(span) => span,
            Err(matching::Miss::Absent) => {
                return Err(match single {
                    true => format!(
                        "Could not find the exact text in {shown}. The old text must match exactly \
including all whitespace and newlines."
                    ),
                    false => format!(
                        "Could not find edits[{index}] in {shown}. The oldText must match exactly \
including all whitespace and newlines."
                    ),
                });
            }
            Err(matching::Miss::Many(occurrences)) => {
                return Err(match single {
                    true => format!(
                        "Found {occurrences} occurrences of the text in {shown}. The text must be \
unique. Please provide more context to make it unique."
                    ),
                    false => format!(
                        "Found {occurrences} occurrences of edits[{index}] in {shown}. Each oldText \
must be unique. Please provide more context to make it unique."
                    ),
                });
            }
        };
        spans.push((start, end, new, index));
    }
    spans.sort_by_key(|span| span.0);
    if let Some(pair) = spans.windows(2).find(|pair| pair[0].1 > pair[1].0) {
        return Err(format!(
            "edits[{}] and edits[{}] overlap in {shown}. Merge them into one edit or target \
disjoint regions.",
            pair[0].3, pair[1].3
        ));
    }
    let spans: Vec<_> = spans
        .iter()
        .map(|(start, end, new, _)| (*start, *end, new.as_str()))
        .collect();
    let mut updated = String::with_capacity(original.len());
    let mut cursor = 0;
    for (start, end, new) in &spans {
        updated.push_str(&original[cursor..*start]);
        updated.push_str(new);
        cursor = *end;
    }
    updated.push_str(&original[cursor..]);
    if updated == original {
        return Err(match single {
            true => format!(
                "No changes made to {shown}. The replacement produced identical content. This \
might indicate an issue with special characters or the text not existing as expected."
            ),
            false => {
                format!("No changes made to {shown}. The replacements produced identical content.")
            }
        });
    }
    put(&path, &updated)?;
    // Matches never cover a byte order mark; as in Scramjet, the diff leaves it out.
    let text = original.strip_prefix('\u{feff}').unwrap_or(&original);
    let skipped = original.len() - text.len();
    let text_spans: Vec<_> = spans
        .iter()
        .map(|&(start, end, new)| (start - skipped, end - skipped, new))
        .collect();
    Ok(Output {
        text: format!("Applied {} edit(s) to {}", spans.len(), path.display()),
        detail: Some(ToolDetail::Diff(diff(text, &text_spans))),
        failed: false,
        image: None,
    })
}

/// Shows replacing the sorted, non-overlapping byte `spans` of `original` as numbered lines, as
/// Scramjet does: ` ` unchanged and `-` removed (old numbering), `+` added (new numbering), and
/// `...` for skipped unchanged lines.
fn diff(original: &str, spans: &[(usize, usize, &str)]) -> String {
    let line_of = |offset: usize| original[..offset].matches('\n').count();
    let old_lines: Vec<&str> = original.lines().collect();
    // Edits touching the same lines form one hunk of whole lines: its byte range and new text.
    let mut hunks: Vec<(usize, usize, String)> = Vec::new();
    let mut index = 0;
    while index < spans.len() {
        let start = original[..spans[index].0].rfind('\n').map_or(0, |i| i + 1);
        let mut end = start;
        let mut replaced = String::new();
        let mut cursor = start;
        while index < spans.len() && (spans[index].0 < end || cursor == start) {
            let (from, to, new) = spans[index];
            replaced.push_str(&original[cursor..from]);
            replaced.push_str(new);
            cursor = to;
            end = match original[..to].ends_with('\n') {
                true => to,
                false => original[to..]
                    .find('\n')
                    .map_or(original.len(), |i| to + i + 1),
            };
            index += 1;
        }
        replaced.push_str(&original[cursor..end]);
        hunks.push((start, end, replaced));
    }
    let mut ops: Vec<(char, &str)> = Vec::new();
    let mut next = 0;
    for (start, end, replaced) in &hunks {
        let first = line_of(*start);
        let removed: Vec<&str> = original[*start..*end].lines().collect();
        ops.extend(old_lines[next..first].iter().map(|line| (' ', *line)));
        sequence_diff(&removed, &replaced.lines().collect::<Vec<_>>(), &mut ops);
        next = first + removed.len();
    }
    ops.extend(old_lines[next..].iter().map(|line| (' ', *line)));

    // An unchanged line is shown when it is at most DIFF_CONTEXT lines from a change.
    let mut near = vec![false; ops.len()];
    for order in [false, true] {
        let mut since = usize::MAX;
        for i in 0..ops.len() {
            let i = if order { ops.len() - 1 - i } else { i };
            since = if ops[i].0 == ' ' {
                since.saturating_add(1)
            } else {
                0
            };
            near[i] |= since <= DIFF_CONTEXT;
        }
    }
    let new_count = ops.iter().filter(|(mark, _)| *mark != '-').count();
    let width = old_lines.len().max(new_count).to_string().len();
    let (mut old, mut new) = (0, 0);
    let mut skipped = false;
    let mut out = String::new();
    for ((mark, text), near) in ops.into_iter().zip(near) {
        if !near {
            skipped = true;
        } else {
            if std::mem::take(&mut skipped) {
                out.push_str(&format!(" {:width$} ...\n", ""));
            }
            let number = if mark == '+' { new } else { old } + 1;
            out.push_str(&format!("{mark}{number:>width$} {text}\n"));
        }
        old += usize::from(mark != '+');
        new += usize::from(mark != '-');
    }
    if skipped && !out.is_empty() {
        out.push_str(&format!(" {:width$} ...\n", ""));
    }
    out
}

/// Above this many compared pairs, all old items are shown removed and all new items added, to
/// bound the memory of the comparison table.
const DIFF_CELLS: usize = 1 << 22;

/// Appends the items (lines, or the words of a line) that change `old` into `new` as ` `, `-`
/// and `+` operations, keeping their longest common subsequence and removals before additions.
pub fn sequence_diff<'a>(old: &[&'a str], new: &[&'a str], ops: &mut Vec<(char, &'a str)>) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    ops.extend(old[..prefix].iter().map(|line| (' ', *line)));
    let (a, b) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );
    let columns = b.len() + 1;
    if a.len().saturating_mul(b.len()) > DIFF_CELLS {
        ops.extend(a.iter().map(|line| ('-', *line)));
        ops.extend(b.iter().map(|line| ('+', *line)));
    } else {
        // common[i * columns + j] is the length of the longest common subsequence of a[i..], b[j..].
        let mut common = vec![0u32; (a.len() + 1) * columns];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                common[i * columns + j] = match a[i] == b[j] {
                    true => common[(i + 1) * columns + j + 1] + 1,
                    false => common[(i + 1) * columns + j].max(common[i * columns + j + 1]),
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                ops.push((' ', a[i]));
                (i, j) = (i + 1, j + 1);
            } else if j == b.len()
                || (i < a.len() && common[(i + 1) * columns + j] >= common[i * columns + j + 1])
            {
                ops.push(('-', a[i]));
                i += 1;
            } else {
                ops.push(('+', b[j]));
                j += 1;
            }
        }
    }
    ops.extend(old[old.len() - suffix..].iter().map(|line| (' ', *line)));
}

fn timeout(call: &Call) -> Duration {
    Duration::from_secs(
        call.arguments["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT)
            .clamp(1, MAX_TIMEOUT),
    )
}

fn bash(
    root: &Path,
    call: &Call,
    hidden: Option<&Hidden>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Output, String> {
    let command = call.str("command")?;
    let timeout = Some(timeout(call));
    let ran = match hidden {
        None => run_command(root, command, timeout, cancel, progress)?,
        Some(hidden) => {
            let process = sandbox::hiding(root, command, hidden)?;
            run_process(process, sandbox::PROGRAM, timeout, cancel, progress).map_err(|error| {
                format!("{error}; it hides the credential stores from this command")
            })?
        }
    };
    Ok(ran.tool_output())
}

/// Pi's `ls`: the entries of a directory, sorted case-insensitively, directories with `/`.
fn ls(root: &Path, call: &Call) -> Result<String, String> {
    let path = resolve(root, call.arguments["path"].as_str().unwrap_or("."));
    let limit = call.arguments["limit"]
        .as_u64()
        .map_or(LS_LIMIT, |n| n as usize);
    let metadata =
        std::fs::metadata(&path).map_err(|_| format!("Path not found: {}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("Not a directory: {}", path.display()));
    }
    let entries = std::fs::read_dir(&path).map_err(|e| format!("Cannot read directory: {e}"))?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    let mut listed = Vec::new();
    let mut limited = false;
    for name in names {
        if listed.len() == limit {
            limited = true;
            break;
        }
        // Entries that cannot be examined are left out, as Pi leaves them out.
        let Ok(metadata) = std::fs::metadata(path.join(&name)) else {
            continue;
        };
        listed.push(if metadata.is_dir() {
            format!("{name}/")
        } else {
            name
        });
    }
    if listed.is_empty() {
        return Ok("(empty directory)".into());
    }
    let limited = limited.then(|| {
        format!(
            "{limit} entries limit reached. Use limit={} for more",
            limit * 2
        )
    });
    Ok(with_notices(&listed, limited.into_iter().collect()))
}

/// The first `lines` within `MAX_BYTES`, then Pi's notices in brackets, the given ones first.
fn with_notices(lines: &[String], mut notices: Vec<String>) -> String {
    let mut output = String::new();
    for line in lines {
        if output.len() + line.len() + 1 > MAX_BYTES {
            notices.push(format!("{}KB limit reached", MAX_BYTES / 1024));
            break;
        }
        output.push_str(line);
        output.push('\n');
    }
    output.pop();
    if !notices.is_empty() {
        output += &format!("\n\n[{}]", notices.join(". "));
    }
    output
}

/// Where `rg` (for `grep`) or `fd` (for `find`) is, as Pi finds them: in Scramjet's `bin`
/// directory, else on the `PATH` (`fd` also as Debian's `fdfind`).
pub fn search_program(tool: &str) -> Option<PathBuf> {
    let (binary, names): (&str, &[&str]) = match tool {
        "grep" => ("rg", &["rg"]),
        _ => ("fd", &["fd", "fdfind"]),
    };
    let executable = |path: &Path| std::fs::metadata(path).is_ok_and(|m| crate::os::executable(&m));
    let binary = crate::os::program_file(binary);
    let local = super::subagent::agent_directory().map(|dir| dir.join("bin").join(binary));
    if let Some(local) = local.filter(|path| executable(path)) {
        return Some(local);
    }
    let path = std::env::var_os("PATH")?;
    names.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(crate::os::program_file(name)))
            .find(|candidate| executable(candidate))
    })
}

/// `program` searching `path`: confined when there are credential stores to hide.
fn search(
    root: &Path,
    path: &Path,
    program: &Path,
    hidden: Option<&Hidden>,
) -> Result<Command, String> {
    match hidden {
        Some(hidden) => sandbox::program(root, path, program, hidden),
        None => {
            let mut command = Command::new(program);
            command.current_dir(root);
            Ok(command)
        }
    }
}

/// The process groups of the commands running, which `end_commands` kills.
static GROUPS: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// A command started in a process group of its own, which `end_commands` kills until this is
/// dropped. Drop it once the command was waited for.
pub(super) struct Group(u32);

impl Group {
    pub(super) fn spawn(command: &mut Command) -> io::Result<(std::process::Child, Self)> {
        // Held while spawning, so that none starts once `end_commands` has run.
        let mut groups = GROUPS.lock().unwrap_or_else(PoisonError::into_inner);
        let child = crate::os::own_group(command).spawn()?;
        let group = child.id();
        groups.push(group);
        Ok((child, Self(group)))
    }

    /// As `spawn`, for a process `start` starts in a process group of its own and returns
    /// with its id.
    pub(super) fn started<T>(
        start: impl FnOnce() -> io::Result<(T, u32)>,
    ) -> io::Result<(T, Self)> {
        let mut groups = GROUPS.lock().unwrap_or_else(PoisonError::into_inner);
        let (started, id) = start()?;
        groups.push(id);
        Ok((started, Self(id)))
    }

    pub(super) fn kill(&self) {
        crate::os::kill_group(self.0);
    }

    pub(super) fn terminate(&self) {
        crate::os::end_group(self.0);
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        let mut groups = GROUPS.lock().unwrap_or_else(PoisonError::into_inner);
        groups.retain(|&group| group != self.0);
    }
}

/// Kills the commands running, as cancelling them does, and keeps any other from starting, for a
/// process about to exit.
pub(super) fn end_commands() {
    let mut groups = GROUPS.lock().unwrap_or_else(PoisonError::into_inner);
    groups.drain(..).for_each(crate::os::kill_group);
    std::mem::forget(groups);
}

/// Runs `command` until it ends, `cancel` is set or `line` returns false for a line of its
/// standard output: its exit status, `None` when `line` stopped it, and the start of its
/// standard error.
fn run_lines(
    mut command: Command,
    cancel: &AtomicBool,
    mut line: impl FnMut(&[u8]) -> bool,
) -> Result<(Option<ExitStatus>, String), String> {
    use std::{io::BufRead, sync::mpsc};
    let program = command.get_program().to_string_lossy().into_owned();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, group) =
        Group::spawn(&mut command).map_err(|e| format!("cannot start {program}: {e}"))?;
    let kill = || group.kill();
    let mut errors = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let (mut kept, mut buffer) = (Vec::new(), [0u8; 8192]);
        while let Ok(n) = errors.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let room = OUTPUT_TAIL_BYTES.saturating_sub(kept.len()).min(n);
            kept.extend_from_slice(&buffer[..room]);
        }
        kept
    });
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let (stopped, cancelled) = std::thread::scope(|scope| {
        let (finished, wait) = mpsc::channel::<()>();
        let watcher = scope.spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) =
                wait.recv_timeout(Duration::from_millis(20))
            {
                if cancel.load(Ordering::Relaxed) {
                    kill();
                    return true;
                }
            }
            false
        });
        let (mut buffer, mut stopped) = (Vec::new(), false);
        while matches!(stdout.read_until(b'\n', &mut buffer), Ok(n) if n > 0) {
            if !line(&buffer) {
                kill();
                stopped = true;
                break;
            }
            buffer.clear();
        }
        drop(finished);
        (stopped, watcher.join().unwrap_or(true))
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
    match () {
        _ if cancelled || cancel.load(Ordering::Relaxed) => Err("Operation aborted".into()),
        _ if stopped => Ok((None, stderr)),
        _ => Ok((Some(status), stderr)),
    }
}

/// Pi's `grep`: ripgrep's matches as `path:line: text`, context lines read from the file as
/// `path-line- text`, paths relative to a directory searched. With `hidden`, ripgrep runs in the
/// read-only sandbox, which hides them.
fn grep(
    root: &Path,
    call: &Call,
    hidden: Option<&Hidden>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let pattern = call.str("pattern")?;
    let path = resolve(root, call.arguments["path"].as_str().unwrap_or("."));
    let metadata =
        std::fs::metadata(&path).map_err(|_| format!("Path not found: {}", path.display()))?;
    let context = call.arguments["context"].as_u64().unwrap_or(0) as usize;
    let limit = call.arguments["limit"]
        .as_u64()
        .map_or(GREP_LIMIT, |n| n.max(1) as usize);
    let program = search_program("grep").ok_or("ripgrep (rg) is not available")?;
    let mut command = search(root, &path, &program, hidden)?;
    command.args(["--json", "--line-number", "--color=never", "--hidden"]);
    if call.arguments["ignoreCase"] == true {
        command.arg("--ignore-case");
    }
    if call.arguments["literal"] == true {
        command.arg("--fixed-strings");
    }
    if let Some(glob) = call.arguments["glob"].as_str() {
        command.arg("--glob").arg(glob);
    }
    command.arg("--").arg(pattern).arg(&path);
    let (mut found, mut limited) = (Vec::new(), false);
    // rg stops at the match after the last one shown, which tells that there are more.
    let (status, stderr) = run_lines(command, cancel, |line| {
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return true;
        };
        let data = &event["data"];
        let (true, Some(file), Some(number)) = (
            event["type"] == "match",
            data["path"]["text"].as_str(),
            data["line_number"].as_u64(),
        ) else {
            return true;
        };
        if found.len() == limit {
            limited = true;
            return false;
        }
        let text = data["lines"]["text"].as_str().map(str::to_owned);
        found.push((file.to_owned(), number as usize, text));
        true
    })?;
    if found.is_empty() {
        return match status.and_then(|status| status.code()) {
            // The sandbox's own failures exit with 1 too.
            Some(0 | 1) if !stderr.starts_with(sandbox::FAILED) => Ok("No matches found".into()),
            code => Err(match stderr.trim() {
                "" => format!("ripgrep exited with code {}", code.unwrap_or(-1)),
                error => error.to_owned(),
            }),
        };
    }
    let shown = |file: &str| {
        let file = Path::new(file);
        match file.strip_prefix(&path) {
            Ok(relative) if metadata.is_dir() && !relative.as_os_str().is_empty() => {
                relative.display().to_string()
            }
            _ => file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        }
    };
    let mut long = false;
    let mut cut = |text: &str| match text.char_indices().nth(GREP_LINE_CHARS) {
        Some((at, _)) => {
            long = true;
            format!("{}... [truncated]", &text[..at])
        }
        None => text.to_owned(),
    };
    // As in Pi, a line rg gives as bytes rather than text, and context, come from the file.
    let mut files: std::collections::HashMap<String, Vec<String>> = Default::default();
    let mut lines = Vec::new();
    for (file, number, text) in &found {
        if let (0, Some(text)) = (context, text) {
            let text = text.trim_end_matches(['\n', '\r']).replace('\r', "");
            lines.push(format!("{}:{number}: {}", shown(file), cut(&text)));
            continue;
        }
        let content = files.entry(file.clone()).or_insert_with(|| {
            std::fs::read(file).map_or_else(
                |_| Vec::new(),
                |bytes| {
                    String::from_utf8_lossy(&bytes)
                        .replace("\r\n", "\n")
                        .split(['\n', '\r'])
                        .map(str::to_owned)
                        .collect()
                },
            )
        });
        if content.is_empty() {
            lines.push(format!("{}:{number}: (unable to read file)", shown(file)));
            continue;
        }
        let start = number.saturating_sub(context).max(1);
        let end = (number + context).min(content.len()).max(*number);
        for current in start..=end {
            let text = cut(content.get(current - 1).map_or("", String::as_str));
            lines.push(match current == *number {
                true => format!("{}:{current}: {text}", shown(file)),
                false => format!("{}-{current}- {text}", shown(file)),
            });
        }
    }
    let mut notices = Vec::new();
    if limited {
        notices.push(format!(
            "{limit} matches limit reached. Use limit={} for more, or refine pattern",
            limit * 2
        ));
    }
    if long {
        notices.push(format!(
            "Some lines truncated to {GREP_LINE_CHARS} chars. Use read tool to see full lines"
        ));
    }
    Ok(with_notices(&lines, notices))
}

/// Pi's `find`: fd's paths matching a glob, relative to the directory searched; in the sandbox
/// with `hidden`, as `grep`.
fn find(
    root: &Path,
    call: &Call,
    hidden: Option<&Hidden>,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let pattern = call.str("pattern")?;
    let path = resolve(root, call.arguments["path"].as_str().unwrap_or("."));
    if std::fs::metadata(&path).is_err() {
        return Err(format!("Path not found: {}", path.display()));
    }
    let limit = call.arguments["limit"]
        .as_u64()
        .map_or(FIND_LIMIT, |n| n.max(1) as usize);
    let program = search_program("find").ok_or("fd is not available")?;
    let mut command = search(root, &path, &program, hidden)?;
    command.args([
        "--glob",
        "--color=never",
        "--hidden",
        "--no-require-git",
        "--max-results",
    ]);
    command.arg(limit.to_string());
    // As in Pi: fd matches a glob against the base name unless `--full-path` is set, and then
    // against the absolute path, so a pattern with a `/` gets a leading `**/`.
    let mut pattern = pattern.to_owned();
    if pattern.contains('/') {
        command.arg("--full-path");
        if !pattern.starts_with('/') && !pattern.starts_with("**/") && pattern != "**" {
            pattern = format!("**/{pattern}");
        }
    }
    command.arg("--").arg(&pattern).arg(&path);
    let mut found = Vec::new();
    let (status, stderr) = run_lines(command, cancel, |line| {
        let line = String::from_utf8_lossy(line);
        let line = line.trim();
        if !line.is_empty() {
            found.push(match Path::new(line).strip_prefix(&path) {
                Ok(relative) => {
                    let slash = if line.ends_with('/') { "/" } else { "" };
                    format!("{}{slash}", relative.display())
                }
                Err(_) => line.to_owned(),
            });
        }
        true
    })?;
    if found.is_empty() {
        return match status.filter(ExitStatus::success) {
            Some(_) => Ok("No files found matching pattern".into()),
            None => Err(match stderr.trim() {
                "" => format!(
                    "fd exited with code {}",
                    status.and_then(|status| status.code()).unwrap_or(-1)
                ),
                error => error.to_owned(),
            }),
        };
    }
    let limited = (found.len() >= limit).then(|| {
        format!(
            "{limit} results limit reached. Use limit={} for more, or refine pattern",
            limit * 2
        )
    });
    Ok(with_notices(&found, limited.into_iter().collect()))
}

/// Runs `command` with bash in `root` until it ends, `cancel` is set or, if given, `timeout` passes.
/// `progress` receives the end of its output as it grows.
pub(super) fn run_command(
    root: &Path,
    command: &str,
    timeout: Option<Duration>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Ran, String> {
    let mut process = Command::new(crate::os::bash()?);
    process.arg("-c").arg(command).current_dir(root);
    run_process(process, "bash", timeout, cancel, progress)
}

/// As `run_command`, inside the read-only sandbox.
pub(super) fn run_sandboxed(
    root: &Path,
    command: &str,
    hidden: &Hidden,
    timeout: Option<Duration>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Ran, String> {
    let process = sandbox::command(root, command, hidden)?;
    run_process(process, sandbox::PROGRAM, timeout, cancel, progress)
}

fn run_process(
    mut process: Command,
    program: &str,
    timeout: Option<Duration>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<Ran, String> {
    let (mut reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let (mut child, group) = {
        process
            .stdin(Stdio::null())
            .stdout(writer.try_clone().map_err(|e| e.to_string())?)
            .stderr(writer);
        let spawned = Group::spawn(&mut process);
        // It holds the pipe's write ends, which must close for the output to end.
        drop(process);
        spawned.map_err(|e| format!("cannot start {program}: {e}"))?
    };
    let output = Arc::new(Mutex::new(Vec::new()));
    let total = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let collector = {
        let (output, total) = (output.clone(), total.clone());
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                total.fetch_add(n, Ordering::Relaxed);
                let mut output = output.lock().unwrap();
                output.extend_from_slice(&buffer[..n]);
                if output.len() > CAPTURE_BYTES {
                    let excess = output.len() - CAPTURE_BYTES / 2;
                    output.drain(..excess);
                }
            }
        })
    };
    let started = Instant::now();
    let mut stopped = None;
    let mut reported = (0, started);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        let seen = total.load(Ordering::Relaxed);
        if seen != reported.0 && reported.1.elapsed() >= OUTPUT_UPDATE {
            reported = (seen, Instant::now());
            let tail = output_tail(&output.lock().unwrap());
            progress(&tail);
        }
        if cancel.load(Ordering::Relaxed) {
            stopped = Some("cancelled by the user");
        } else if timeout.is_some_and(|timeout| started.elapsed() > timeout) {
            stopped = Some("timed out");
        }
        if stopped.is_some() {
            group.kill();
            break child.wait().map_err(|e| e.to_string())?;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Background processes may keep the pipe open; don't wait for them indefinitely.
    let deadline = Instant::now() + Duration::from_secs(2);
    while !collector.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let bytes = output.lock().unwrap().clone();
    let (mut text, truncated) = tail(&String::from_utf8_lossy(&bytes));
    if truncated || total.load(Ordering::Relaxed) > bytes.len() {
        text = format!(
            "[Output truncated to the last {} lines/{}KB of {} bytes]\n{text}",
            MAX_LINES,
            MAX_BYTES / 1024,
            total.load(Ordering::Relaxed)
        );
    }
    Ok(Ran {
        output: text,
        stopped,
        status,
        elapsed: started.elapsed(),
    })
}

/// A command that has ended: the end of its output, with a note when earlier output was left out,
/// and how it ended.
pub(super) struct Ran {
    output: String,
    /// Why it was stopped before it ended: `cancelled by the user` or `timed out`.
    stopped: Option<&'static str>,
    status: ExitStatus,
    elapsed: Duration,
}

impl Ran {
    /// As in Scramjet, a command that succeeds returns just its output; any other ending is a
    /// failure with its reason after the output.
    pub fn tool_output(&self) -> Output {
        let ending = match (self.stopped, self.status.code()) {
            (Some(reason), _) => format!("Command {reason} after {}s", self.elapsed.as_secs()),
            (None, Some(0)) => {
                return Output {
                    text: if self.output.is_empty() {
                        "(no output)".into()
                    } else {
                        self.output.clone()
                    },
                    detail: None,
                    failed: false,
                    image: None,
                };
            }
            (None, Some(code)) => format!("Command exited with code {code}"),
            (None, None) => format!("Command terminated ({})", self.status),
        };
        let text = match self.output.trim_end_matches('\n') {
            "" => ending,
            output => format!("{output}\n\n{ending}"),
        };
        Output {
            text,
            detail: None,
            failed: true,
            image: None,
        }
    }

    /// As Scramjet's text for a `!` command: what it ran, its output without escape sequences,
    /// and how it ended.
    pub fn message(&self, command: &str) -> String {
        let mut text = format!("Ran `{command}`\n");
        let output: Vec<String> = self.output.split('\n').map(terminal_line).collect();
        match output.join("\n").trim_end_matches('\n') {
            "" => text += "(no output)",
            output => text += &format!("```\n{output}\n```"),
        }
        if self.stopped.is_some() {
            text += "\n\n(command cancelled)";
        } else if let Some(code) = self.status.code().filter(|code| *code != 0) {
            text += &format!("\n\nCommand exited with code {code}");
        }
        text
    }
}

/// A line of output as a terminal shows it: only the text after its last carriage return, without
/// escape sequences or control characters other than tabs.
pub fn terminal_line(line: &str) -> String {
    let line = line
        .split('\r')
        .rfind(|part| !part.is_empty())
        .unwrap_or("");
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\t' => text.push(c),
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next().is_some()) {
                            break;
                        }
                    }
                }
                // Intermediate bytes, then one final byte, such as `ESC ( B` from `tput sgr0`.
                Some(' '..='/') => {
                    for c in chars.by_ref() {
                        if !(' '..='/').contains(&c) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            c if c.is_control() => {}
            c => text.push(c),
        }
    }
    text
}

/// The end of a running command's output, from the start of a line where it is cut.
fn output_tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL_BYTES);
    let text = String::from_utf8_lossy(&bytes[start..]);
    match text.split_once('\n') {
        Some((_, rest)) if start > 0 => rest.to_owned(),
        _ => text.into_owned(),
    }
}

/// Keeps the last `MAX_LINES` lines, then at most the last `MAX_BYTES` bytes.
fn tail(text: &str) -> (String, bool) {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut start = body
        .rmatch_indices('\n')
        .nth(MAX_LINES - 1)
        .map_or(0, |(index, _)| index + 1)
        .max(text.len().saturating_sub(MAX_BYTES));
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_owned(), start > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, arguments: Value) -> Call<'_> {
        Call { name, arguments }
    }

    #[test]
    fn edit_requires_unique_non_overlapping_matches() {
        let root = std::env::temp_dir().join(format!("scramjet-agent-edit-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        std::fs::write(root.join("f.txt"), "alpha beta gamma beta\n").unwrap();
        let cancel = AtomicBool::new(false);
        let run = |edits: Value| {
            call("edit", json!({"path":"f.txt","edits":edits})).run(
                &root,
                None,
                &cancel,
                &mut |_| {},
            )
        };
        let path = root.join("f.txt").display().to_string();
        assert_eq!(
            run(json!([{"oldText":"beta","newText":"x"}])).unwrap_err(),
            format!(
                "Found 2 occurrences of the text in {path}. The text must be unique. Please \
provide more context to make it unique."
            )
        );
        assert_eq!(
            run(json!([{"oldText":"delta","newText":"x"}])).unwrap_err(),
            format!(
                "Could not find the exact text in {path}. The old text must match exactly \
including all whitespace and newlines."
            )
        );
        assert_eq!(
            run(json!([{"oldText":"alpha","newText":"x"},{"oldText":"delta","newText":"x"}]))
                .unwrap_err(),
            format!(
                "Could not find edits[1] in {path}. The oldText must match exactly including \
all whitespace and newlines."
            )
        );
        assert_eq!(
            run(json!([{"oldText":"gamma","newText":"x"},{"oldText":"","newText":"x"}]))
                .unwrap_err(),
            format!("edits[1].oldText must not be empty in {path}.")
        );
        assert_eq!(
            run(json!([{"oldText":"gamma beta","newText":"y"},{"oldText":"alpha beta","newText":"x"},{"oldText":"beta gamma","newText":"y"}])).unwrap_err(),
            format!(
                "edits[1] and edits[2] overlap in {path}. Merge them into one edit or target \
disjoint regions."
            )
        );
        run(json!([{"oldText":"gamma","newText":"G"},{"oldText":"alpha","newText":"A"}])).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "A beta G beta\n"
        );
        let written = call("write", json!({"path":"new/g.txt","content":"é\nx\n"}))
            .run(&root, None, &cancel, &mut |_| {})
            .unwrap();
        assert_eq!(
            written.text,
            format!("Wrote 5 bytes to {}", root.join("new/g.txt").display())
        );
        assert_eq!(
            written.detail,
            Some(ToolDetail::Written {
                path: "new/g.txt".into(),
                content: "é\nx\n".into()
            })
        );
        assert_eq!(
            std::fs::read_to_string(root.join("new/g.txt")).unwrap(),
            "é\nx\n"
        );
    }

    #[test]
    fn read_sends_an_image_with_its_text_and_says_why_one_is_left_out() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-image-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        let png = crate::agent::image::tests::png(3, 2);
        std::fs::write(root.join("shot.png"), &png).unwrap();
        std::fs::write(root.join("bad.jpg"), b"\xff\xd8\xff\xe0 cut short").unwrap();
        let cancel = AtomicBool::new(false);
        let read = |path: &str| {
            call("read", json!({"path":path}))
                .run_read_only(&root, &Hidden::current(&[]), &cancel)
                .unwrap()
        };
        let shot = read("shot.png");
        assert_eq!(shot.text, "Read image file [image/png]");
        let image = shot.image.as_ref().unwrap();
        assert_eq!(image.mime, "image/png");
        let output = shot.sent();
        assert_eq!(
            output[0],
            json!({"type":"input_text","text":"Read image file [image/png]"})
        );
        assert_eq!(output[1]["type"], "input_image");
        assert_eq!(
            output[1]["image_url"],
            format!("data:image/png;base64,{}", image.data)
        );
        assert_eq!(result_text(&output), shot.text);
        assert_eq!(
            (result_images(&output), result_images(&json!("text"))),
            (1, 0)
        );
        let bad = read("bad.jpg");
        assert_eq!(
            (bad.text.as_str(), bad.image.is_none(), bad.sent()),
            (
                "Read image file [image/jpeg]\n[Image omitted: could not be resized below the \
inline image size limit.]",
                true,
                json!(bad.text),
            )
        );
    }

    #[test]
    fn reads_of_skills_context_files_and_the_readme_are_compact() {
        let root = Path::new("/work/project");
        let compact = |arguments| call("read", arguments).compact(root);
        let home = std::env::home_dir().unwrap();
        let skill = home.join(".agents/skills/release-notes/SKILL.md");
        assert_eq!(
            compact(json!({"path":skill,"offset":5,"limit":10})),
            Some(CompactRead::Skill("release-notes:5-14".into()))
        );
        assert_eq!(
            compact(json!({"path":"SKILL.md"})),
            Some(CompactRead::Skill("project".into()))
        );
        assert_eq!(
            compact(json!({"path":"./CLAUDE.md"})),
            Some(CompactRead::Resource("CLAUDE.md".into()))
        );
        assert_eq!(
            compact(json!({"path":"/work/project/sub/AGENTS.MD"})),
            Some(CompactRead::Resource("sub/AGENTS.MD".into()))
        );
        assert_eq!(
            compact(json!({"path":home.join("AGENTS.md"),"offset":2})),
            Some(CompactRead::Resource("~/AGENTS.md:2".into()))
        );
        assert_eq!(
            compact(json!({"path":"../CLAUDE.md"})),
            Some(CompactRead::Resource("/work/CLAUDE.md".into()))
        );
        assert_eq!(
            compact(json!({"path":instructions::README})),
            Some(CompactRead::Docs("README.md".into()))
        );
        for other in [
            "README.md",
            "skill.md",
            "CLAUDE.md.bak",
            "docs/AGENT.md",
            "",
        ] {
            assert_eq!(compact(json!({"path":other})), None, "{other}");
        }
        let write = call("write", json!({"path":"CLAUDE.md","content":""}));
        assert_eq!(write.compact(root), None);
    }

    #[test]
    fn read_pages_long_files_and_bash_reports_exit_and_tail() {
        let root = std::env::temp_dir().join(format!("scramjet-agent-read-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        let text: String = (1..=3000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(root.join("long.txt"), &text).unwrap();
        let cancel = AtomicBool::new(false);
        let first = call("read", json!({"path":"long.txt"}))
            .run(&root, None, &cancel, &mut |_| {})
            .unwrap()
            .text;
        assert!(first.starts_with("line 1\n") && first.contains("Use offset=2001 to continue"));
        let page = call("read", json!({"path":"long.txt","offset":2999}))
            .run(&root, None, &cancel, &mut |_| {})
            .unwrap()
            .text;
        assert_eq!(page, "line 2999\nline 3000\n");
        let summary = |arguments| call("read", arguments).summary();
        assert_eq!(summary(json!({"path":"f"})), "f");
        assert_eq!(summary(json!({"path":"f","offset":40})), "f:40");
        assert_eq!(
            summary(json!({"path":"f","offset":40,"limit":20})),
            "f:40-59"
        );
        assert_eq!(summary(json!({"path":"f","limit":20})), "f:1-20");
        let home = std::env::home_dir().unwrap();
        let home = home.to_str().unwrap();
        assert_eq!(
            summary(json!({"path":format!("{home}/repos/a.rs"),"offset":3,"limit":2})),
            "~/repos/a.rs:3-4"
        );
        assert_eq!(
            summary(json!({"path":format!("{home}x/a.rs")})),
            format!("{home}x/a.rs")
        );
        assert_eq!(call("write", json!({"path":home})).summary(), "~");
        let bash = |command: &str| {
            let output = call("bash", json!({"command":command,"timeout":1}))
                .run(&root, None, &cancel, &mut |_| {})
                .unwrap();
            (output.text, output.failed)
        };
        let (output, failed) = bash("seq 1 5000; echo err >&2; exit 3");
        assert!(output.contains("[Output truncated") && failed);
        assert!(output.ends_with("5000\nerr\n\nCommand exited with code 3"));
        assert!(!output.contains("\n1\n"));
        assert_eq!(bash("echo ok"), ("ok\n".into(), false));
        assert_eq!(bash("true"), ("(no output)".into(), false));
        assert_eq!(bash("false"), ("Command exited with code 1".into(), true));
        let (timed, failed) = bash("echo started; sleep 30");
        assert!(timed.starts_with("started\n\nCommand timed out after ") && failed);
        let mut shown = Vec::new();
        call(
            "bash",
            json!({"command":"echo first; sleep 1; echo second"}),
        )
        .run(&root, None, &cancel, &mut |output| {
            shown.push(output.to_owned())
        })
        .unwrap();
        assert_eq!(shown.first().map(String::as_str), Some("first\n"));
        assert!(shown.len() <= 2, "{shown:?}");
        let long: String = (1..=5000).map(|n| format!("{n}\n")).collect();
        let end = output_tail(long.as_bytes());
        assert!(end.len() <= OUTPUT_TAIL_BYTES && end.ends_with("\n5000\n"));
        assert!(long.contains(&format!("\n{end}")));
        assert_eq!(output_tail(b"partial"), "partial");
        let question =
            |command: &str| match call("bash", json!({"command":command})).approval(&root) {
                Approval::Ask(question) => question.replace(&root.display().to_string(), "ROOT"),
                Approval::None => panic!("bash runs without approval"),
            };
        assert_eq!(question("make -j4\n"), "Run in ROOT");
        assert_eq!(question("git status"), "Run in ROOT");
        assert_eq!(question("cd x\nmake"), "Run in ROOT");
        assert!(
            call("write", json!({"path":"a/b.txt","content":"hi"}))
                .approval(&root)
                .is_none()
        );
        assert!(
            !call("write", json!({"path":"../escape.txt","content":"x"}))
                .approval(&root)
                .is_none()
        );
        assert!(
            !call("edit", json!({"path":"~/escape.txt","edits":[]}))
                .approval(&root)
                .is_none()
        );
    }

    #[test]
    fn read_only_search_tools_answer_as_pis_do() {
        assert!(search_program("grep").is_some() && search_program("find").is_some());
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-search-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(
            root.join("src/a.rs"),
            "fn alpha() {}\n// one\n// two\nfn beta() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/deep/b.rs"),
            format!("fn Alpha() {{}}\n{}\n", "x".repeat(600)),
        )
        .unwrap();
        std::fs::write(root.join("notes.txt"), "a.b\naxb\n").unwrap();
        std::fs::write(root.join("latin1.txt"), b"caf\xe9 needle\n").unwrap();
        std::fs::write(root.join("near.txt"), "a\nb\nm1\nm2\nc\nd\n").unwrap();
        std::fs::write(
            root.join("huge.min.js"),
            format!("needle{}\n", "x".repeat(5 << 20)),
        )
        .unwrap();
        std::fs::write(root.join(".hidden.rs"), "fn alpha() {}\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(root.join("ignored.rs"), "").unwrap();
        let cancel = AtomicBool::new(false);
        let run = |name: &str, arguments: Value| {
            call(name, arguments).run_read_only(&root, &Hidden::current(&[]), &cancel)
        };
        let text = |name: &str, arguments: Value| run(name, arguments).unwrap().text;
        assert_eq!(
            text("grep", json!({"pattern":"fn alpha","path":"src"})),
            "a.rs:1: fn alpha() {}"
        );
        assert_eq!(
            text(
                "grep",
                json!({"pattern":"fn alpha","path":"src","ignoreCase":true,"glob":"**/deep/*"})
            ),
            "deep/b.rs:1: fn Alpha() {}"
        );
        assert_eq!(
            text(
                "grep",
                json!({"pattern":"two","path":"src/a.rs","context":1})
            ),
            "a.rs-2- // one\na.rs:3: // two\na.rs-4- fn beta() {}"
        );
        assert_eq!(
            text(
                "grep",
                json!({"pattern":"a.b","literal":true,"path":"notes.txt"})
            ),
            "notes.txt:1: a.b"
        );
        assert_eq!(
            text(
                "grep",
                json!({"pattern":"^fn ","path":"src/a.rs","limit":1})
            ),
            "a.rs:1: fn alpha() {}\n\n[1 matches limit reached. Use limit=2 for more, or \
refine pattern]"
        );
        assert_eq!(
            text("grep", json!({"pattern":"alpha","glob":".hidden*"})),
            ".hidden.rs:1: fn alpha() {}"
        );
        assert_eq!(
            text("grep", json!({"pattern":"needle","path":"latin1.txt"})),
            "latin1.txt:1: caf\u{fffd} needle"
        );
        assert_eq!(
            text(
                "grep",
                json!({"pattern":"^m","path":"near.txt","context":2,"limit":1})
            ),
            "near.txt-1- a\nnear.txt-2- b\nnear.txt:3: m1\nnear.txt-4- m2\nnear.txt-5- c\n\n[1 \
matches limit reached. Use limit=2 for more, or refine pattern]"
        );
        assert_eq!(
            text(
                "grep",
                json!({"pattern":"Alpha|xxx","path":"src/deep","limit":1})
            ),
            "b.rs:1: fn Alpha() {}\n\n[1 matches limit reached. Use limit=2 for more, or refine \
pattern]"
        );
        assert_eq!(
            text("grep", json!({"pattern":"needle","path":"huge.min.js"})),
            format!(
                "huge.min.js:1: needle{}... [truncated]\n\n[Some lines truncated to 500 chars. \
Use read tool to see full lines]",
                "x".repeat(494)
            )
        );
        let long = text("grep", json!({"pattern":"xxx","path":"src/deep"}));
        assert_eq!(
            long,
            format!(
                "b.rs:2: {}... [truncated]\n\n[Some lines truncated to 500 chars. Use read tool \
to see full lines]",
                "x".repeat(500)
            )
        );
        assert_eq!(
            text("grep", json!({"pattern":"nowhere"})),
            "No matches found"
        );
        assert!(
            run("grep", json!({"pattern":"("}))
                .unwrap_err()
                .contains("regex parse error")
        );
        assert_eq!(
            run("grep", json!({"pattern":"x","path":"missing"})).unwrap_err(),
            format!("Path not found: {}", root.join("missing").display())
        );
        let mut found: Vec<String> = text("find", json!({"pattern":"*.rs"}))
            .lines()
            .map(str::to_owned)
            .collect();
        found.sort();
        assert_eq!(found, [".hidden.rs", "src/a.rs", "src/deep/b.rs"]);
        assert_eq!(
            text("find", json!({"pattern":"deep/*.rs","path":"src"})),
            "deep/b.rs"
        );
        assert_eq!(
            text("find", json!({"pattern":"*.md"})),
            "No files found matching pattern"
        );
        let limited = text("find", json!({"pattern":"*.rs","limit":1}));
        assert!(
            limited.ends_with(
                "\n\n[1 results limit reached. Use limit=2 for more, or refine pattern]"
            ),
            "{limited}"
        );
        assert_eq!(text("ls", json!({"path":"src"})), "a.rs\ndeep/");
        assert_eq!(
            text("ls", json!({"limit":2})),
            ".gitignore\n.hidden.rs\n\n[2 entries limit reached. Use limit=4 for more]"
        );
        assert!(run("write", json!({"path":"x","content":""})).is_err());
        assert!(!root.join("x").exists());
    }

    #[cfg(unix)]
    #[test]
    fn tools_with_hidden_credentials_keep_out_of_their_stores() {
        let home =
            std::env::temp_dir().join(format!("scramjet-agent-hiding-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        std::os::unix::fs::symlink(home.join(".ssh"), root.join("keys")).unwrap();
        let hidden = Hidden::under(&home, Vec::new());
        let cancel = AtomicBool::new(false);
        let run = |name: &str, arguments: Value| {
            call(name, arguments)
                .run(&root, Some(&hidden), &cancel, &mut |_| {})
                .map(|output| output.text)
        };
        let refused = |path: PathBuf| {
            Err(format!(
                "{}: hidden, as it holds credentials",
                path.display()
            ))
        };
        let netrc = home.join(".netrc");
        let edits = json!([{"oldText":"SECRET","newText":"x"}]);
        assert_eq!(
            run("read", json!({"path":"../.netrc"})),
            refused(netrc.clone())
        );
        assert_eq!(
            run("edit", json!({"path":"../.netrc","edits":edits})),
            refused(netrc)
        );
        let key = root.join("keys/authorized_keys");
        assert_eq!(
            run(
                "write",
                json!({"path":"keys/authorized_keys","content":"k"})
            ),
            refused(key)
        );
        assert_eq!(std::fs::read_dir(home.join(".ssh")).unwrap().count(), 0);
        assert!(run("write", json!({"path":"a.txt","content":"plain\n"})).is_ok());
        assert_eq!(
            run(
                "bash",
                json!({"command":"cat ../.netrc a.txt; echo more >> a.txt"})
            ),
            Ok("plain\n".into())
        );
        assert_eq!(
            std::fs::read_to_string(root.join("a.txt")).unwrap(),
            "plain\nmore\n"
        );
        assert_eq!(
            std::fs::read_to_string(home.join(".netrc")).unwrap(),
            "SECRET-netrc\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_swapped_in_alongside_never_leads_a_tool_into_a_store() {
        use super::super::sandbox::{Sandbox, SandboxMode};
        let home = std::env::temp_dir().join(format!("scramjet-tool-swap-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        let hidden = Hidden::under(&home, Vec::new());
        let cancel = AtomicBool::new(false);
        let sandbox = Sandbox {
            mode: SandboxMode::WorkspaceWrite,
            network: true,
        };
        let done = AtomicBool::new(false);
        let (mut ran, mut shown) = (0, Vec::new());
        std::thread::scope(|scope| {
            // As a bash call running alongside would: the checked file, then a symlink to a
            // store, each renamed into place.
            scope.spawn(|| {
                let (file, link) = (root.join("file"), root.join("link"));
                while !done.load(Ordering::Relaxed) {
                    std::os::unix::fs::symlink(home.join(".netrc"), &link).unwrap();
                    std::fs::rename(&link, root.join("a.txt")).unwrap();
                    std::fs::write(&file, "plain\n").unwrap();
                    std::fs::rename(&file, root.join("a.txt")).unwrap();
                }
            });
            let edits = json!([{"oldText":"plain","newText":"PLAIN"}]);
            for round in 0..3000 {
                let (name, arguments) = match round % 3 {
                    0 => ("read", json!({"path":"a.txt"})),
                    1 => ("edit", json!({"path":"a.txt","edits":edits})),
                    _ => ("write", json!({"path":"a.txt","content":"x\n"})),
                };
                let call = call(name, arguments);
                let output = match round % 2 {
                    0 => call.run(&root, Some(&hidden), &cancel, &mut |_| {}),
                    _ => call.run_in_sandbox(&root, sandbox, &hidden, &cancel, &mut |_| {}),
                };
                ran += usize::from(output.is_ok());
                shown.push(output.map_or_else(|error| error, |output| output.text));
            }
            done.store(true, Ordering::Relaxed);
        });
        assert!(ran > 0);
        let leaked = shown.iter().find(|text| text.contains("SECRET"));
        assert!(leaked.is_none(), "{leaked:?}");
        assert_eq!(
            std::fs::read_to_string(home.join(".netrc")).unwrap(),
            "SECRET-netrc\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_write_through_a_swapped_in_symlink_makes_nothing_in_a_store() {
        let home = std::env::temp_dir().join(format!("scramjet-put-dirs-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let hidden = Hidden::under(&home, Vec::new());
        super::put(&root.join("new/deeper/a.txt"), "made\n", Some(&hidden)).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("new/deeper/a.txt")).unwrap(),
            "made\n"
        );
        // As if swapped in after the path was checked.
        std::os::unix::fs::symlink(home.join(".ssh"), root.join("link")).unwrap();
        let error =
            super::put(&root.join("link/authorized_keys/x"), "key\n", Some(&hidden)).unwrap_err();
        assert!(error.contains("hidden, as it holds credentials"), "{error}");
        assert_eq!(std::fs::read_dir(home.join(".ssh")).unwrap().count(), 0);
    }

    #[test]
    fn a_cancelled_command_with_hidden_credentials_stops_with_everything_it_started() {
        let marker = format!("{}.{}", 400 + std::process::id() % 100, std::process::id());
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                cancel.store(true, Ordering::Relaxed);
            });
            let command = format!("sleep {marker} & sleep {marker}");
            let output = call("bash", json!({"command": command}))
                .run(
                    Path::new("/"),
                    Some(&Hidden::default()),
                    &cancel,
                    &mut |_| {},
                )
                .unwrap();
            assert!(
                output.text.contains("cancelled by the user"),
                "{}",
                output.text
            );
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(200));
        let left = Command::new("pgrep")
            .args(["-f", &format!("sleep {marker}")])
            .output()
            .unwrap();
        assert!(
            left.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&left.stdout)
        );
    }

    #[cfg(unix)]
    #[test]
    fn subagent_tools_do_not_show_credential_stores() {
        let home =
            std::env::temp_dir().join(format!("scramjet-agent-hidden-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(home.clone());
        let root = home.join("project");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(home.join(".netrc"), "SECRET-netrc\n").unwrap();
        std::fs::write(home.join(".ssh/id_x"), "SECRET-key\n").unwrap();
        std::fs::write(root.join("a.txt"), "plain SECRET-not\n").unwrap();
        std::os::unix::fs::symlink(home.join(".netrc"), root.join("link")).unwrap();
        std::fs::create_dir_all(home.join("other")).unwrap();
        std::fs::write(home.join("other/f.txt"), "needle\n").unwrap();
        std::os::unix::fs::symlink(home.join("other"), root.join("abs")).unwrap();
        std::fs::create_dir_all(home.join("agent")).unwrap();
        std::fs::write(home.join("agent/auth.json"), "SECRET-auth\n").unwrap();
        let hidden = Hidden::under(&home, vec![home.join("agent/auth.json")]);
        let cancel = AtomicBool::new(false);
        let run = |name: &str, arguments: Value| {
            call(name, arguments)
                .run_read_only(&root, &hidden, &cancel)
                .map(|output| output.text)
        };
        let refused = |path: &str| format!("{path}: hidden, as it holds credentials");
        for (name, path, shown) in [
            ("read", "../.netrc", home.join(".netrc")),
            ("read", "link", root.join("link")),
            ("read", "../agent/auth.json", home.join("agent/auth.json")),
            ("ls", "~/../../..", PathBuf::from("/")),
            ("ls", "../.ssh", home.join(".ssh")),
            ("grep", "../.ssh/id_x", home.join(".ssh/id_x")),
            ("find", "../.ssh", home.join(".ssh")),
        ] {
            let result = run(name, json!({"path": path, "pattern": "*"}));
            match path {
                "~/../../.." => assert!(result.is_ok(), "{result:?}"),
                _ => assert_eq!(result, Err(refused(&shown.display().to_string()))),
            }
        }
        assert_eq!(
            run("grep", json!({"pattern":"SECRET","path":".."})),
            Ok("project/a.txt:1: plain SECRET-not".into())
        );
        assert_eq!(
            run("grep", json!({"pattern":"nowhere","path":".."})),
            Ok("No matches found".into())
        );
        assert_eq!(
            run("find", json!({"pattern":"id_*","path":".."})),
            Ok("No files found matching pattern".into())
        );
        assert_eq!(
            run("grep", json!({"pattern":"needle","path":"abs"})),
            Ok("f.txt:1: needle".into())
        );
        assert_eq!(
            run("find", json!({"pattern":"*.txt","path":"abs"})),
            Ok("f.txt".into())
        );
        let shown = run(
            "bash",
            json!({"command":"cat ../.netrc link ../.ssh/id_x ../agent/auth.json; ls -A ../.ssh"}),
        )
        .unwrap();
        assert!(!shown.contains("SECRET"), "{shown}");
        let alias = std::path::Path::new("/mnt/wslg/distro").join(home.strip_prefix("/").unwrap());
        if alias.is_dir() {
            for (name, path) in [("read", ".netrc"), ("read", ".ssh/id_x"), ("ls", ".ssh")] {
                let path = alias.join(path).display().to_string();
                assert_eq!(run(name, json!({ "path": path })), Err(refused(&path)));
            }
            let alias = alias.display().to_string();
            let command = format!("cat {alias}/.netrc {alias}/.ssh/id_x; ls {alias}/.ssh");
            let shown = run("bash", json!({ "command": command })).unwrap();
            assert!(
                !shown.contains("SECRET") && !shown.contains("id_x\n"),
                "{shown}"
            );
            let searched = run("grep", json!({"pattern":"SECRET","path":alias}));
            assert!(!format!("{searched:?}").contains("SECRET-"), "{searched:?}");
        }
    }

    #[test]
    fn a_search_stops_its_program_when_done_or_cancelled() {
        let started = Instant::now();
        let mut command = Command::new("sh");
        command.args(["-c", "echo one; echo two; sleep 30"]);
        let mut lines = Vec::new();
        let (status, _) = run_lines(command, &AtomicBool::new(false), |line| {
            lines.push(line.to_owned());
            false
        })
        .unwrap();
        assert_eq!((status, lines), (None, vec![b"one\n".to_vec()]));
        let cancel = AtomicBool::new(false);
        let stopped = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::Relaxed);
            });
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 30 & sleep 30"]);
            run_lines(command, &cancel, |_| true)
        });
        assert_eq!(stopped.unwrap_err(), "Operation aborted");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn paths_starting_with_a_tilde_are_in_the_home_directory() {
        let home = std::env::home_dir().unwrap();
        let root = Path::new("/project");
        assert_eq!(resolve(root, "~/a/../b.txt"), home.join("b.txt"));
        assert_eq!(resolve(root, "~"), home);
        assert_eq!(resolve(root, "~other/x"), root.join("~other/x"));
        assert_eq!(resolve(root, "x/~/y"), root.join("x/~/y"));
    }

    #[test]
    fn edit_keeps_line_endings_and_replaces_only_what_a_loose_match_covers() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-edit-loose-{}", std::process::id()));
        let _scratch = crate::agent::tests::ScratchDir::new(root.clone());
        let cancel = AtomicBool::new(false);
        let file = root.join("f.txt");
        let path = file.display().to_string();
        let run = |arguments: Value| call("edit", arguments).run(&root, None, &cancel, &mut |_| {});

        std::fs::write(&file, "\u{feff}one\r\ntwo\nthree\r\n").unwrap();
        let output = run(json!({"path":"f.txt","edits":[
            {"oldText":"one\ntwo","newText":"uno\ndos"},
            {"oldText":"three\n","newText":"tres\r\nfour\n"}]}))
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "\u{feff}uno\r\ndos\ntres\r\nfour\r\n"
        );
        let Some(ToolDetail::Diff(diff)) = output.detail else {
            panic!("no diff")
        };
        assert_eq!(
            diff,
            "-1 one\n-2 two\n+1 uno\n+2 dos\n-3 three\n+3 tres\n+4 four\n"
        );

        let code = "say(“hi”) – 1  \nend\t\nsay(“hi”)\n";
        std::fs::write(&file, code).unwrap();
        assert_eq!(
            run(json!({"path":"f.txt","edits":[{"oldText":"say(\"hi\")","newText":"x"}]}))
                .unwrap_err(),
            format!(
                "Found 2 occurrences of the text in {path}. The text must be unique. Please \
provide more context to make it unique."
            )
        );
        run(json!({"path":"f.txt","edits":[{"oldText":"say(\"hi\") - 1\nend","newText":"say('hi')\nend"}]}))
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "say('hi')\nend\t\nsay(“hi”)\n"
        );
        assert_eq!(
            run(json!({"path":"f.txt","edits":[{"oldText":"end","newText":"end"}]})).unwrap_err(),
            format!(
                "No changes made to {path}. The replacement produced identical content. This \
might indicate an issue with special characters or the text not existing as expected."
            )
        );
        assert_eq!(
            run(json!({"path":"f.txt","edits":[]})).unwrap_err(),
            "Edit tool input is invalid. edits must contain at least one replacement."
        );
        // As in Scramjet, `edits` as a JSON string, and one edit's fields beside it, are taken.
        run(
            json!({"path":"f.txt","edits":"[{\"oldText\":\"end\",\"newText\":\"fin\"}]",
            "oldText":"say('hi')","newText":"go()"}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "go()\nfin\t\nsay(“hi”)\n"
        );
    }

    #[test]
    fn edit_diffs_show_changed_lines_with_context_and_new_numbering() {
        let original: String = (1..=20).map(|n| format!("line {n}.\n")).collect();
        let span = |old: &str, new: &'static str| {
            let start = original.find(old).unwrap();
            (start, start + old.len(), new)
        };
        let spans = [
            span("line 3.", "line 3!\nnew."),
            span("line 15.", "line 15!"),
        ];
        let expected = [
            "  1 line 1.",
            "  2 line 2.",
            "- 3 line 3.",
            "+ 3 line 3!",
            "+ 4 new.",
            "  4 line 4.",
            "  5 line 5.",
            "  6 line 6.",
            "  7 line 7.",
            "    ...",
            " 11 line 11.",
            " 12 line 12.",
            " 13 line 13.",
            " 14 line 14.",
            "-15 line 15.",
            "+16 line 15!",
            " 16 line 16.",
            " 17 line 17.",
            " 18 line 18.",
            " 19 line 19.",
            "    ...",
        ];
        assert_eq!(
            diff(&original, &spans).lines().collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            diff("one\ntwo\nthree\n", &[(0, 13, "one\n2\nthree")]),
            " 1 one\n-2 two\n+2 2\n 3 three\n"
        );
        assert_eq!(
            diff("f(x, y)\nend\n", &[(2, 3, "left"), (5, 6, "right")]),
            "-1 f(x, y)\n+1 f(left, right)\n 2 end\n"
        );
        // Unchanged lines inside one edit stay unchanged, as in a dogfood edit of a function.
        let original = "fn f() -> P {\n    let r =\n        g();\n    r\n}\n";
        let new = "fn f() -> G {\n    let r =\n        g();\n    G(r)\n";
        assert_eq!(
            diff(original, &[(0, original.len() - 2, new)]),
            "-1 fn f() -> P {\n+1 fn f() -> G {\n 2     let r =\n 3         g();\n-4     r\n+4     G(r)\n 5 }\n"
        );
        let original: String = (1..=12).map(|n| format!("{n}\n")).collect();
        let new = original.replace("1\n2", "one\n2").replace("12", "twelve");
        assert_eq!(
            diff(&original, &[(0, original.len(), &new)])
                .lines()
                .collect::<Vec<_>>(),
            [
                "- 1 1",
                "+ 1 one",
                "  2 2",
                "  3 3",
                "  4 4",
                "  5 5",
                "    ...",
                "  8 8",
                "  9 9",
                " 10 10",
                " 11 11",
                "-12 12",
                "+12 twelve",
            ]
        );
        assert_eq!(diff("same\n", &[(0, 4, "same")]), "");
    }

    impl Approval {
        fn is_none(&self) -> bool {
            matches!(self, Approval::None)
        }
    }
}
