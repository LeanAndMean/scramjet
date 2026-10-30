use super::ToolDetail;
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_LINES: usize = 2000;
const MAX_BYTES: usize = 50 * 1024;
/// Bash output kept in memory while a command runs; only the tail is shown to the model.
const CAPTURE_BYTES: usize = 4 << 20;
const DEFAULT_TIMEOUT: u64 = 120;
const MAX_TIMEOUT: u64 = 3600;
/// Unchanged lines shown around each change in an edit diff, as in Scramjet.
const DIFF_CONTEXT: usize = 4;

pub(super) fn definitions() -> Value {
    json!([
        {"type":"function","name":"read","strict":false,
         "description":"Read a text file. Output is truncated to 2000 lines or 50KB; use offset/limit (1-based line numbers) to read large files in parts.",
         "parameters":{"type":"object","properties":{
            "path":{"type":"string","description":"File path, relative to the project directory or absolute"},
            "offset":{"type":"integer","description":"First line to read (1-based)"},
            "limit":{"type":"integer","description":"Maximum number of lines to read"}},
          "required":["path"]}},
        {"type":"function","name":"write","strict":false,
         "description":"Create or overwrite a file with the given content. Creates parent directories.",
         "parameters":{"type":"object","properties":{
            "path":{"type":"string"},"content":{"type":"string"}},
          "required":["path","content"]}},
        {"type":"function","name":"edit","strict":false,
         "description":"Edit a file by exact text replacement. Each oldText must match exactly once in the original file; edits must not overlap.",
         "parameters":{"type":"object","properties":{
            "path":{"type":"string"},
            "edits":{"type":"array","items":{"type":"object","properties":{
                "oldText":{"type":"string"},"newText":{"type":"string"}},"required":["oldText","newText"]}}},
          "required":["path","edits"]}},
        {"type":"function","name":"bash","strict":false,
         "description":"Run a bash command in the project directory. Returns combined stdout/stderr, truncated to the last 2000 lines or 50KB; a non-zero exit code is reported after the output. The user approves each command; just call this tool.",
         "parameters":{"type":"object","properties":{
            "command":{"type":"string"},
            "timeout":{"type":"integer","description":"Seconds before the command is killed (default 120)"}},
          "required":["command"]}}
    ])
}

/// What the user is asked to approve before a tool runs, if anything.
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
            "read" => {
                let path = self.arguments["path"].as_str().unwrap_or("");
                let (offset, limit) = (
                    self.arguments["offset"].as_u64(),
                    self.arguments["limit"].as_u64(),
                );
                let start = offset.unwrap_or(1).max(1);
                match limit {
                    _ if offset.is_none() && limit.is_none() => path.to_owned(),
                    Some(limit) => format!("{path}:{start}-{}", (start + limit).saturating_sub(1)),
                    None => format!("{path}:{start}"),
                }
            }
            _ => self.arguments["path"].as_str().unwrap_or("").to_owned(),
        }
    }
    pub fn approval(&self, root: &Path) -> Approval {
        match self.name {
            "bash" => Approval::Ask(format!(
                "Run in {}:\n{}",
                root.display(),
                self.arguments["command"].as_str().unwrap_or("")
            )),
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
    pub fn run(&self, root: &Path, cancel: &AtomicBool) -> Result<Output, String> {
        match self.name {
            "read" => Ok(Output {
                text: read(root, self)?,
                detail: None,
                failed: false,
            }),
            "write" => write(root, self),
            "edit" => edit(root, self),
            "bash" => bash(root, self, cancel),
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

#[derive(Debug)]
pub(super) struct Output {
    /// What the model receives.
    pub text: String,
    pub detail: Option<ToolDetail>,
    /// The tool ran but reports failure, such as a command's non-zero exit.
    pub failed: bool,
}

/// Resolves `path` against the project root without following symlinks.
fn resolve(root: &Path, path: &str) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in root.join(path).components() {
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

fn read(root: &Path, call: &Call) -> Result<String, String> {
    let path = resolve(root, call.str("path")?);
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
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
            "\n[Showing lines {offset}-{last} of {}. Use offset={} to continue.]",
            lines.len(),
            last + 1
        ));
    }
    Ok(output)
}

fn write(root: &Path, call: &Call) -> Result<Output, String> {
    let path = resolve(root, call.str("path")?);
    let content = call.str("content")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Output {
        text: format!("Wrote {} bytes to {}", content.len(), path.display()),
        detail: Some(ToolDetail::Written(content.to_owned())),
        failed: false,
    })
}

fn edit(root: &Path, call: &Call) -> Result<Output, String> {
    let path = resolve(root, call.str("path")?);
    let original =
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let edits = call.arguments["edits"]
        .as_array()
        .ok_or("missing array argument `edits`")?;
    let mut spans = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let old = edit["oldText"]
            .as_str()
            .ok_or(format!("edit {index}: missing oldText"))?;
        let new = edit["newText"]
            .as_str()
            .ok_or(format!("edit {index}: missing newText"))?;
        if old.is_empty() {
            return Err(format!("edit {index}: oldText is empty"));
        }
        let mut matches = original.match_indices(old);
        let Some((start, _)) = matches.next() else {
            return Err(format!(
                "edit {index}: oldText not found in {}",
                path.display()
            ));
        };
        if matches.next().is_some() {
            return Err(format!(
                "edit {index}: oldText matches more than once; include more context"
            ));
        }
        spans.push((start, start + old.len(), new));
    }
    spans.sort_by_key(|span| span.0);
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err("edits overlap; merge them into one edit".into());
    }
    let mut updated = String::with_capacity(original.len());
    let mut cursor = 0;
    for (start, end, new) in &spans {
        updated.push_str(&original[cursor..*start]);
        updated.push_str(new);
        cursor = *end;
    }
    updated.push_str(&original[cursor..]);
    std::fs::write(&path, &updated).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Output {
        text: format!("Applied {} edit(s) to {}", spans.len(), path.display()),
        detail: Some(ToolDetail::Diff(diff(&original, &spans))),
        failed: false,
    })
}

/// Shows replacing the sorted, non-overlapping byte `spans` of `original` as numbered lines:
/// ` ` unchanged (old numbering), `-` removed (old numbering), `+` added (new numbering), and
/// `...` for skipped unchanged lines.
fn diff(original: &str, spans: &[(usize, usize, &str)]) -> String {
    struct Change {
        old: usize,
        new: usize,
        removed: Vec<String>,
        added: Vec<String>,
    }
    let line_of = |offset: usize| original[..offset].matches('\n').count();
    let mut changes: Vec<Change> = Vec::new();
    let mut shift = 0isize;
    let mut index = 0;
    while index < spans.len() {
        // Edits touching the same lines are shown as one change of whole lines.
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
        let mut removed: Vec<String> = original[start..end].lines().map(str::to_owned).collect();
        let mut added: Vec<String> = replaced.lines().map(str::to_owned).collect();
        let same_start = removed
            .iter()
            .zip(&added)
            .take_while(|(a, b)| a == b)
            .count();
        removed.drain(..same_start);
        added.drain(..same_start);
        let same_end = removed
            .iter()
            .rev()
            .zip(added.iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        removed.truncate(removed.len() - same_end);
        added.truncate(added.len() - same_end);
        let old = line_of(start) + same_start;
        let lines_shift = added.len() as isize - removed.len() as isize;
        if !removed.is_empty() || !added.is_empty() {
            changes.push(Change {
                old,
                new: old.saturating_add_signed(shift),
                removed,
                added,
            });
        }
        shift += lines_shift;
    }
    let old_lines: Vec<&str> = original.lines().collect();
    let width = (old_lines.len() as isize + shift.max(0)).to_string().len();
    let mut out = String::new();
    let mut shown = 0;
    let mut line = |mark: char, number: Option<usize>, text: &str| {
        let number = number.map_or(String::new(), |n| (n + 1).to_string());
        out.push_str(&format!("{mark}{number:>width$} {text}\n"));
    };
    for (i, change) in changes.iter().enumerate() {
        let from = change.old.saturating_sub(DIFF_CONTEXT).max(shown);
        if from > shown {
            line(' ', None, "...");
        }
        for (n, text) in (from..).zip(&old_lines[from..change.old]) {
            line(' ', Some(n), text);
        }
        for (n, text) in change.removed.iter().enumerate() {
            line('-', Some(change.old + n), text);
        }
        for (n, text) in change.added.iter().enumerate() {
            line('+', Some(change.new + n), text);
        }
        shown = change.old + change.removed.len();
        let next = changes.get(i + 1).map_or(old_lines.len(), |c| c.old);
        let until = (shown + DIFF_CONTEXT).min(next).min(old_lines.len());
        for (n, text) in (shown..).zip(&old_lines[shown..until]) {
            line(' ', Some(n), text);
        }
        if i + 1 == changes.len() && until < old_lines.len() {
            line(' ', None, "...");
        }
        shown = until;
    }
    out
}

/// As in Scramjet, a command that succeeds returns just its output; any other ending is a failure
/// with its reason after the output.
fn bash(root: &Path, call: &Call, cancel: &AtomicBool) -> Result<Output, String> {
    use std::os::unix::process::CommandExt;
    let command = call.str("command")?;
    let timeout = Duration::from_secs(
        call.arguments["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT)
            .clamp(1, MAX_TIMEOUT),
    );
    let (mut reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let mut child = {
        let mut process = Command::new("bash");
        process
            .arg("-c")
            .arg(command)
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(writer.try_clone().map_err(|e| e.to_string())?)
            .stderr(writer)
            .process_group(0);
        process
            .spawn()
            .map_err(|e| format!("cannot start bash: {e}"))?
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
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if cancel.load(Ordering::Relaxed) {
            stopped = Some("cancelled by the user");
        } else if started.elapsed() > timeout {
            stopped = Some("timed out");
        }
        if stopped.is_some() {
            if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
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
    let ending = match (stopped, status.code()) {
        (Some(reason), _) => format!("Command {reason} after {}s", started.elapsed().as_secs()),
        (None, Some(0)) => {
            return Ok(Output {
                text: if text.is_empty() {
                    "(no output)".into()
                } else {
                    text
                },
                detail: None,
                failed: false,
            });
        }
        (None, Some(code)) => format!("Command exited with code {code}"),
        (None, None) => format!("Command terminated ({status})"),
    };
    let text = match text.trim_end_matches('\n') {
        "" => ending,
        output => format!("{output}\n\n{ending}"),
    };
    Ok(Output {
        text,
        detail: None,
        failed: true,
    })
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
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("f.txt"), "alpha beta gamma beta\n").unwrap();
        let cancel = AtomicBool::new(false);
        let run =
            |edits: Value| call("edit", json!({"path":"f.txt","edits":edits})).run(&root, &cancel);
        assert!(
            run(json!([{"oldText":"beta","newText":"x"}]))
                .unwrap_err()
                .contains("more than once")
        );
        assert!(
            run(json!([{"oldText":"delta","newText":"x"}]))
                .unwrap_err()
                .contains("not found")
        );
        assert!(run(json!([{"oldText":"alpha beta","newText":"x"},{"oldText":"beta gamma","newText":"y"}])).unwrap_err().contains("overlap"));
        run(json!([{"oldText":"gamma","newText":"G"},{"oldText":"alpha","newText":"A"}])).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "A beta G beta\n"
        );
        let written = call("write", json!({"path":"new/g.txt","content":"é\nx\n"}))
            .run(&root, &cancel)
            .unwrap();
        assert_eq!(
            written.text,
            format!("Wrote 5 bytes to {}", root.join("new/g.txt").display())
        );
        assert_eq!(written.detail, Some(ToolDetail::Written("é\nx\n".into())));
        assert_eq!(
            std::fs::read_to_string(root.join("new/g.txt")).unwrap(),
            "é\nx\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_pages_long_files_and_bash_reports_exit_and_tail() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let root = std::env::temp_dir().join(format!("scramjet-agent-read-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let text: String = (1..=3000).map(|n| format!("line {n}\n")).collect();
        std::fs::write(root.join("long.txt"), &text).unwrap();
        let cancel = AtomicBool::new(false);
        let first = call("read", json!({"path":"long.txt"}))
            .run(&root, &cancel)
            .unwrap()
            .text;
        assert!(first.starts_with("line 1\n") && first.contains("Use offset=2001 to continue"));
        let page = call("read", json!({"path":"long.txt","offset":2999}))
            .run(&root, &cancel)
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
        let bash = |command: &str| {
            let output = call("bash", json!({"command":command,"timeout":1}))
                .run(&root, &cancel)
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
        std::fs::remove_dir_all(&root).unwrap();
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
    }

    impl Approval {
        fn is_none(&self) -> bool {
            matches!(self, Approval::None)
        }
    }
}
