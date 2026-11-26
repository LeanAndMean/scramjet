//! `/diff`, as Codex's and Claude Code's: the project's uncommitted changes against `HEAD`,
//! untracked files shown as new ones.
use super::report::{ReportView, Tab};
use super::style::{self, Style};
use super::text::printable;
use std::{
    io,
    path::Path,
    process::{Command, Stdio},
};

/// Lines shown at most; more would be slow to draw and read, and `git diff` shows them whole.
const LINES: usize = 5_000;

/// Bytes of an untracked file read at most: a larger one is named but not shown.
const UNTRACKED_BYTES: u64 = 256 * 1024;

pub(super) struct Changes {
    /// The repository's top directory.
    pub(super) top: String,
    /// `git diff`'s text, the untracked files' after it.
    pub(super) diff: String,
}

fn git(directory: &Path, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(["-c", "core.quotePath=false"])
        .args(arguments)
        // Its messages untranslated, for `uncommitted` to recognize one.
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => "git is not installed".to_owned(),
            _ => format!("git: {error}"),
        })?;
    match output.status.success() {
        true => Ok(output.stdout),
        false => Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    }
}

/// The uncommitted changes of the repository `root` is in; `Ok(None)` when it is not in one.
pub(super) fn uncommitted(root: &Path) -> Result<Option<Changes>, String> {
    let top = match git(root, &["rev-parse", "--show-toplevel"]) {
        Ok(top) => String::from_utf8_lossy(&top).trim_end().to_owned(),
        Err(error) if error.contains("not a git repository") => return Ok(None),
        Err(error) => return Err(error),
    };
    let at = Path::new(&top);
    // A repository without commits yet is compared with the empty tree.
    let base = match git(at, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        Ok(_) => "HEAD".to_owned(),
        Err(_) => String::from_utf8_lossy(&git(at, &["hash-object", "-t", "tree", "--stdin"])?)
            .trim()
            .to_owned(),
    };
    let tracked = git(at, &["diff", "--no-color", "--no-ext-diff", &base, "--"])?;
    let mut diff = String::from_utf8_lossy(&tracked).into_owned();
    let untracked = git(at, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in String::from_utf8_lossy(&untracked).split('\0') {
        if !path.is_empty() && !path.ends_with('/') {
            diff += &new_file(at, path);
        }
    }
    Ok(Some(Changes { top, diff }))
}

/// An untracked file as `git diff` shows a new one.
fn new_file(top: &Path, path: &str) -> String {
    let file = top.join(path);
    let Ok(metadata) = std::fs::symlink_metadata(&file) else {
        return String::new();
    };
    let mut text = format!("diff --git a/{path} b/{path}\n");
    let content = if metadata.is_symlink() {
        text += "new file mode 120000\n";
        match std::fs::read_link(&file) {
            Ok(target) => target.to_string_lossy().into_owned().into_bytes(),
            Err(_) => return text,
        }
    } else if metadata.is_file() {
        text += &format!("new file mode {}\n", mode(&metadata));
        if metadata.len() > UNTRACKED_BYTES {
            return text + &format!("Untracked file of {} bytes, not shown\n", metadata.len());
        }
        match std::fs::read(&file) {
            Ok(content) => content,
            Err(error) => return text + &format!("Not read: {error}\n"),
        }
    } else {
        return String::new();
    };
    if content.is_empty() {
        return text;
    }
    if content[..content.len().min(8000)].contains(&0) {
        return text + &format!("Binary files /dev/null and b/{path} differ\n");
    }
    let content = String::from_utf8_lossy(&content);
    let lines: Vec<&str> = content.lines().collect();
    text += &format!("--- /dev/null\n+++ b/{path}\n@@ -0,0 +1");
    if lines.len() != 1 {
        text += &format!(",{}", lines.len());
    }
    text += " @@\n";
    for line in &lines {
        text += &format!("+{line}\n");
    }
    if !content.ends_with('\n') {
        text += "\\ No newline at end of file\n";
    }
    text
}

#[cfg(unix)]
fn mode(metadata: &std::fs::Metadata) -> &'static str {
    use std::os::unix::fs::PermissionsExt;
    match metadata.permissions().mode() & 0o111 {
        0 => "100644",
        _ => "100755",
    }
}

#[cfg(not(unix))]
fn mode(_: &std::fs::Metadata) -> &'static str {
    "100644"
}

/// A file the diff changes, with its lines added and removed.
#[derive(Debug, PartialEq)]
struct File {
    path: String,
    added: usize,
    removed: usize,
    binary: bool,
}

/// The path a `diff --git a/X b/Y` line names: Y, or the line's rest when it cannot be split.
fn named(rest: &str) -> String {
    if rest.len() >= 5 && (rest.len() - 3).is_multiple_of(2) {
        let half = (rest.len() - 3) / 2;
        if rest.is_char_boundary(half)
            && rest.is_char_boundary(half + 3)
            && &rest[half..half + 3] == " b/"
            && rest[..half] == rest[half + 3..]
        {
            return rest[..half].to_owned();
        }
    }
    rest.to_owned()
}

/// The diff styled, each file under a rule naming it and cut after `LINES`, and the files it
/// changes, counted to its end.
fn shown(diff: &str) -> (String, Vec<File>) {
    let mut text = String::new();
    let mut files: Vec<File> = Vec::new();
    let mut hunk = false;
    let mut lines = 0;
    for line in diff.lines() {
        lines += 1;
        let line = match lines > LINES {
            true => line.to_owned(),
            false => printable(line),
        };
        let (mark, line): (Option<Style>, String) =
            if let Some(rest) = line.strip_prefix("diff --git a/") {
                hunk = false;
                files.push(File {
                    path: named(rest),
                    added: 0,
                    removed: 0,
                    binary: false,
                });
                if !text.is_empty() {
                    text += "\n";
                }
                let path = &files.last().unwrap().path;
                (Some(style::INFO_HEADING), format!("── {path} ──"))
            } else if line.starts_with("@@") {
                hunk = true;
                (Some(style::MUTED), line)
            } else if hunk && line.starts_with('+') {
                if let Some(file) = files.last_mut() {
                    file.added += 1;
                }
                (Some(style::DIFF_ADDED), line)
            } else if hunk && line.starts_with('-') {
                if let Some(file) = files.last_mut() {
                    file.removed += 1;
                }
                (Some(style::DIFF_REMOVED), line)
            } else if hunk && (line.starts_with(' ') || line.starts_with('\\')) {
                (None, line)
            } else {
                hunk = false;
                if line.starts_with("Binary files ")
                    && let Some(file) = files.last_mut()
                {
                    file.binary = true;
                }
                (Some(style::MUTED), line)
            };
        if lines > LINES {
            continue;
        }
        match mark {
            Some(mark) => text += &style::mark(mark, &line),
            None => text += &line,
        }
        text += "\n";
    }
    if lines > LINES {
        let more = format!("… {} more lines: git diff shows them whole", lines - LINES);
        text += &format!("\n{}\n", style::mark(style::MUTED, &more));
    }
    (text, files)
}

/// The files changed, one a line with their counts.
fn listed(files: &[File]) -> String {
    let (added, removed) = files
        .iter()
        .fold((0, 0), |(a, r), file| (a + file.added, r + file.removed));
    let mut text = format!(
        "{} {}, {} {}\n\n",
        files.len(),
        if files.len() == 1 { "file" } else { "files" },
        style::mark(style::DIFF_ADDED, &format!("+{added}")),
        style::mark(style::DIFF_REMOVED, &format!("-{removed}")),
    );
    for file in files {
        let counts = match file.binary {
            true => style::mark(style::MUTED, &format!("{:<13}", "binary")),
            false => format!(
                "{} {}",
                style::mark(style::DIFF_ADDED, &format!("+{:<5}", file.added)),
                style::mark(style::DIFF_REMOVED, &format!("-{:<5}", file.removed)),
            ),
        };
        text += &format!("{counts}  {}\n", printable(&file.path));
    }
    text
}

/// The heading above the diff and the list.
fn heading(top: &str) -> String {
    let heading = format!("Uncommitted changes in {}", printable(top));
    format!("{}\n\n", style::mark(style::INFO_HEADING, &heading))
}

/// What a line of output says: the files, then the whole diff.
pub(super) fn text(changes: &Changes) -> String {
    let (diff, files) = shown(&changes.diff);
    heading(&changes.top) + &listed(&files) + "\n" + &diff
}

/// The full screen's view: the whole diff, `d` the list of files.
pub(super) fn view(changes: &Changes) -> ReportView {
    let (diff, files) = shown(&changes.diff);
    let tab = Tab {
        name: "Diff".to_owned(),
        summary: heading(&changes.top) + &listed(&files),
        details: heading(&changes.top) + &diff,
    };
    ReportView::new("Diff", "files", vec![tab], 0, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(directory: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(arguments)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    }

    #[test]
    fn the_changes_against_head_and_the_untracked_files_are_shown() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-changes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir(&directory).unwrap();
        let root = directory.as_path();
        assert!(uncommitted(root).unwrap().is_none());
        run(root, &["init", "-q"]);
        // Before the first commit, against the empty tree.
        std::fs::write(root.join("kept.txt"), "one\ntwo\n").unwrap();
        run(root, &["add", "kept.txt"]);
        let changes = uncommitted(root).unwrap().unwrap();
        assert!(changes.diff.contains("+one\n+two\n"), "{}", changes.diff);
        run(root, &["commit", "-q", "-m", "start"]);
        assert_eq!(uncommitted(root).unwrap().unwrap().diff, "");

        std::fs::write(root.join("kept.txt"), "one\n2\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/new file.txt"), "fresh\nlast").unwrap();
        std::fs::write(root.join("empty"), "").unwrap();
        std::fs::write(root.join("data.bin"), b"a\0b").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored\n").unwrap();
        std::fs::write(root.join("ignored"), "secret").unwrap();
        // From a directory under the top, as the agent's may be.
        let changes = uncommitted(&root.join("sub")).unwrap().unwrap();
        let diff = &changes.diff;
        assert!(diff.contains("-two\n+2\n"), "{diff}");
        assert!(
            diff.contains(
                "diff --git a/sub/new file.txt b/sub/new file.txt\nnew file mode 100644\n\
--- /dev/null\n+++ b/sub/new file.txt\n@@ -0,0 +1,2 @@\n+fresh\n+last\n\
\\ No newline at end of file\n"
            ),
            "{diff}"
        );
        assert!(diff.contains("diff --git a/empty b/empty\nnew file mode 100644\n"));
        assert!(diff.contains("Binary files /dev/null and b/data.bin differ\n"));
        assert!(!diff.contains("secret"), "{diff}");

        let (_, files) = shown(diff);
        assert_eq!(
            style::themed(&listed(&files), false),
            "5 files, +4 -1\n\n+1     -1      kept.txt\n+1     -0      .gitignore\n\
binary         data.bin\n+0     -0      empty\n+2     -0      sub/new file.txt\n"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_diff_is_styled_by_part_and_cut_at_its_limit() {
        let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n\
@@ -1,2 +1,2 @@\n ctx\t1\n-old\n+new\n--- a/x b/y\n";
        let (text, files) = shown(diff);
        assert_eq!(
            style::themed(&text, false),
            "── a.rs ──\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n ctx    1\n\
-old\n+new\n--- a/x b/y\n"
        );
        // A removed line reading `-- a/x b/y` is counted as one, not taken as a file's header.
        assert_eq!(
            files,
            [File {
                path: "a.rs".into(),
                added: 1,
                removed: 2,
                binary: false,
            }]
        );
        assert_eq!(named("x y b/x y"), "x y");
        assert_eq!(named("old b/new"), "old b/new");

        let long = "diff --git a/f b/f\n@@ -1 +1 @@\n".to_owned() + &"+x\n".repeat(LINES + 3);
        let (text, files) = shown(&long);
        assert_eq!(files[0].added, LINES + 3);
        let text = style::themed(&text, false);
        assert!(
            text.ends_with("+x\n\n… 5 more lines: git diff shows them whole\n"),
            "{}",
            &text[text.len() - 80..]
        );
    }
}
