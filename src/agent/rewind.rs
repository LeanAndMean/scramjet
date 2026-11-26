//! `/rewind`'s checkpoints, as Claude Code's: what each `write` and `edit` replaced is kept by its
//! digest in the state directory's `file-history`, and named with the call's result, so the
//! files can go back to how they were before a prompt. What commands and subagents change is not
//! kept.
use super::{Agent, AgentError, session};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// Stored with a `write`'s or `edit`'s result: the file's path and the digest of what it held
/// before (`sha256`, `null` when it did not exist), or the `error` that kept it from being kept.
pub(super) const BEFORE: &str = "_scramjet_before";

/// What a file held before a tool changed it; `content` is `None` when it did not exist.
#[derive(Debug)]
pub struct Before {
    pub path: PathBuf,
    pub content: Option<Vec<u8>>,
}

/// A file as it was before a checkpoint's prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptFile {
    pub path: PathBuf,
    kept: Kept,
}

#[derive(Clone, Debug, PartialEq)]
enum Kept {
    Content(String),
    Missing,
    Lost(String),
}

/// A prompt of the conversation as it is now, and the files changed since.
#[derive(Debug, PartialEq)]
pub struct Checkpoint {
    /// Its line in the session file, which `Agent::navigate` takes.
    pub line: usize,
    pub prompt: String,
    /// Each file `write` or `edit` changed at or after it, as it was before the first change.
    pub files: Vec<KeptFile>,
}

/// What going back did to a file.
#[derive(Debug, PartialEq)]
pub enum Restored {
    Restored,
    /// Made after the checkpoint, so removed.
    Removed,
    Unchanged,
    Failed(String),
}

/// Where the copies are kept; none in unit tests, which give their own.
pub(super) fn store() -> Option<PathBuf> {
    match cfg!(test) {
        true => None,
        false => Some(session::records_directory().ok()?.join("file-history")),
    }
}

fn unstored(store: Option<&Path>) -> Result<&Path, String> {
    store.ok_or_else(|| "no state directory to keep it in".to_owned())
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Keeps `before`'s content in `store`; what the call's result says of it (`BEFORE`).
pub(super) fn keep(store: Option<&Path>, before: &Before) -> Value {
    let path = before.path.to_string_lossy();
    let Some(content) = &before.content else {
        return json!({"path": path, "sha256": null});
    };
    let sha256 = digest(content);
    let kept = unstored(store).and_then(|store| {
        let file = store.join(&sha256);
        if file.is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(store).map_err(|e| e.to_string())?;
        let temporary = super::tools::temporary_beside(&file);
        let written =
            std::fs::write(&temporary, content).and_then(|()| std::fs::rename(&temporary, &file));
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written.map_err(|e| e.to_string())
    });
    match kept {
        Ok(()) => json!({"path": path, "sha256": sha256}),
        Err(error) => json!({"path": path, "error": error}),
    }
}

fn kept_file(value: &Value) -> Option<KeptFile> {
    let path = PathBuf::from(value["path"].as_str()?);
    let kept = match (&value["sha256"], value["error"].as_str()) {
        (_, Some(error)) => Kept::Lost(error.to_owned()),
        (Value::String(sha256), _) => Kept::Content(sha256.clone()),
        (Value::Null, _) => Kept::Missing,
        _ => return None,
    };
    Some(KeptFile { path, kept })
}

/// The checkpoints of the conversation `path` (each entry by line), oldest first.
fn checkpoints(path: &[(usize, Value)]) -> Vec<Checkpoint> {
    let mut earliest: HashMap<PathBuf, (usize, KeptFile)> = HashMap::new();
    let mut points = Vec::new();
    for (index, (line, value)) in path.iter().enumerate().rev() {
        if value["type"] == "function_call_output"
            && let Some(file) = value.get(BEFORE).and_then(kept_file)
        {
            earliest.insert(file.path.clone(), (index, file));
        }
        if session::is_prompt(value) {
            let mut files: Vec<_> = earliest.values().cloned().collect();
            files.sort_by_key(|(at, _)| *at);
            points.push(Checkpoint {
                line: *line,
                prompt: session::prompt_text(value).to_owned(),
                files: files.into_iter().map(|(_, file)| file).collect(),
            });
        }
    }
    points.reverse();
    points
}

impl Agent {
    /// The prompts of the conversation as it is now, oldest first, each with the files changed
    /// since, as `/rewind` lists them.
    pub fn checkpoints(&self) -> Result<Vec<Checkpoint>, AgentError> {
        Ok(checkpoints(&self.session.current_path()?))
    }

    /// Puts `files` back as they were: each with what it held, or removed if it did not exist.
    pub fn restore_files(&self, files: &[KeptFile]) -> Vec<(PathBuf, Restored)> {
        let store = self.file_history.as_deref();
        (files.iter())
            .map(|file| (file.path.clone(), restore(store, file)))
            .collect()
    }
}

fn restore(store: Option<&Path>, file: &KeptFile) -> Restored {
    let path = &file.path;
    let failed = |error: io::Error| Restored::Failed(error.to_string());
    match &file.kept {
        Kept::Lost(error) => Restored::Failed(format!("it was not kept: {error}")),
        Kept::Missing => match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Restored::Unchanged,
            Err(error) => failed(error),
            Ok(metadata) if metadata.is_dir() => {
                Restored::Failed("it is a directory now".to_owned())
            }
            Ok(_) => std::fs::remove_file(path).map_or_else(failed, |()| Restored::Removed),
        },
        Kept::Content(sha256) => {
            let kept = unstored(store)
                .and_then(|store| std::fs::read(store.join(sha256)).map_err(|e| e.to_string()));
            let content = match kept {
                Ok(content) => content,
                Err(error) => return Restored::Failed(format!("its copy is gone: {error}")),
            };
            if std::fs::read(path).is_ok_and(|now| now == content) {
                return Restored::Unchanged;
            }
            if let Some(directory) = path.parent()
                && let Err(error) = std::fs::create_dir_all(directory)
            {
                return failed(error);
            }
            std::fs::write(path, content).map_or_else(failed, |()| Restored::Restored)
        }
    }
}

/// `path` as `/rewind` names it: from `root` when it is inside it.
pub fn shown(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) => relative.display().to_string(),
        Err(_) => super::tools::shortened(&path.to_string_lossy()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{Recorder, ScratchDir, agent, serve, stream, tool_call};
    use crate::provider_fixture as fixture;
    use std::sync::atomic::AtomicBool;

    fn scratch(name: &str) -> ScratchDir {
        let name = format!("scramjet-rewind-{name}-{}", std::process::id());
        ScratchDir::new(std::env::temp_dir().join(name))
    }

    fn prompt(text: &str) -> Value {
        json!({"role": "user", "content": text})
    }

    fn result(before: Value) -> Value {
        json!({"type": "function_call_output", "call_id": "c", "output": "ok", BEFORE: before})
    }

    #[test]
    fn each_prompt_lists_the_files_changed_since_as_they_were_first() {
        let scratch = scratch("points");
        let store = scratch.0.join("store");
        let store = Some(store.as_path());
        let file = scratch.0.join("a.txt");
        let new = scratch.0.join("new/b.txt");
        let first = keep(
            store,
            &Before {
                path: file.clone(),
                content: Some(b"one".to_vec()),
            },
        );
        let second = keep(
            store,
            &Before {
                path: file.clone(),
                content: Some(b"two".to_vec()),
            },
        );
        let made = keep(
            store,
            &Before {
                path: new.clone(),
                content: None,
            },
        );
        let path = [
            (1, prompt("first")),
            (2, result(first)),
            (3, prompt("second")),
            (4, result(second)),
            (5, result(made)),
            (6, prompt("third")),
        ];
        let points = checkpoints(&path);
        let lines: Vec<_> = points.iter().map(|p| (p.line, p.prompt.as_str())).collect();
        assert_eq!(lines, [(1, "first"), (3, "second"), (6, "third")]);
        let paths = |point: &Checkpoint| -> Vec<PathBuf> {
            point.files.iter().map(|f| f.path.clone()).collect()
        };
        assert_eq!(paths(&points[0]), [file.clone(), new.clone()]);
        assert_eq!(paths(&points[1]), [file.clone(), new.clone()]);
        assert!(points[2].files.is_empty());

        std::fs::write(&file, "three").unwrap();
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, "made").unwrap();
        let restored: Vec<_> = (points[1].files.iter())
            .map(|f| restore(store, f))
            .collect();
        assert_eq!(restored, [Restored::Restored, Restored::Removed]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert!(!new.exists());
        let again: Vec<_> = (points[0].files.iter())
            .map(|f| restore(store, f))
            .collect();
        assert_eq!(again, [Restored::Restored, Restored::Unchanged]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "one");
    }

    #[test]
    fn a_copy_that_could_not_be_kept_is_reported_when_going_back() {
        let scratch = scratch("lost");
        let blocked = scratch.0.join("file");
        std::fs::write(&blocked, "").unwrap();
        let store = blocked.join("store");
        let store = Some(store.as_path());
        let path = scratch.0.join("a.txt");
        let value = keep(
            store,
            &Before {
                path: path.clone(),
                content: Some(b"one".to_vec()),
            },
        );
        assert!(value["error"].is_string());
        let file = kept_file(&value).unwrap();
        assert!(matches!(restore(store, &file), Restored::Failed(_)));
    }

    #[test]
    fn a_turns_writes_and_edits_go_back_to_before_its_prompt() {
        let text = |text: &str| stream(&fixture::text_events(text, fixture::known_usage()));
        let edit = json!({"path":"kept.txt","edits":[{"oldText":"one","newText":"two"}]});
        let (endpoint, server) = serve(vec![
            tool_call(
                "call_1",
                "write",
                json!({"path":"made/new.txt","content":"new"}),
            ),
            tool_call("call_2", "edit", edit),
            text("Done."),
        ]);
        let (mut agent, scratch) = agent(endpoint, "rewind");
        let (kept, made) = (scratch.0.join("kept.txt"), scratch.0.join("made/new.txt"));
        std::fs::write(&kept, "one").unwrap();
        let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
        agent.turn("change them", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "two");
        // Nothing of the checkpoint is sent.
        assert!(!agent.request().to_string().contains("_scramjet_before"));

        let points = agent.checkpoints().unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].prompt, "change them");
        let restored = agent.restore_files(&points[0].files);
        assert_eq!(
            restored,
            [
                (made.clone(), Restored::Removed),
                (kept.clone(), Restored::Restored)
            ]
        );
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "one");
        assert!(!made.exists());
        let navigated = agent.navigate(points[0].line, None, &mut ui, &cancel);
        assert!(matches!(
            navigated.unwrap(),
            crate::agent::Navigated::Moved { prompt: Some(prompt), .. } if prompt == "change them"
        ));
        assert!(agent.checkpoints().unwrap().is_empty());
    }
}
