//! What saved sessions spent: each response's model, tokens, cost and timing, read back from the
//! session files, for `/usage`.

use super::{PRODUCER, SUBAGENTS, TIMING, TYPED, USAGE};
use serde_json::Value;
use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

/// One response, the main agent's or a subagent's.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    /// When it started, in milliseconds since 1970, or its session's start when it was saved
    /// without its timing (before 0.47.0).
    pub at: u64,
    /// `provider/id`, or empty when it was saved without its model.
    pub model: String,
    /// Input tokens neither read from the cache nor written to it.
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Dollars, when known.
    pub cost: Option<f64>,
    pub first_output_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// Its session file's index in `History::files`.
    pub session: usize,
}

/// A session file's project and title.
#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    /// The project directory it ran in, as its header says.
    pub root: Option<PathBuf>,
    /// Its name, else the first line of its first prompt as typed.
    pub title: Option<String>,
}

#[derive(Debug, Default)]
pub struct History {
    pub responses: Vec<Response>,
    /// The session files read, oldest first.
    pub files: Vec<PathBuf>,
    /// What each of `files` is, by its index there.
    pub summaries: Vec<Summary>,
    /// The files that could not be read, with why.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Each provider's latest quota, as its responses reported it (`quota::read`).
    pub quotas: Vec<(String, super::quota::Quota)>,
}

impl History {
    /// The responses of every session under `sessions` (one directory per project) and of
    /// `current`, wherever it is. A line a fork, `/clone` or `/import` copied into another
    /// session counts once, for the session it was first written in.
    pub fn read(sessions: &Path, current: &Path) -> Self {
        let mut files = Vec::new();
        if let Ok(projects) = std::fs::read_dir(sessions) {
            for project in projects.flatten() {
                let Ok(entries) = std::fs::read_dir(project.path()) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "jsonl")
                    {
                        files.push(path);
                    }
                }
            }
        }
        if !files.iter().any(|file| file == current) {
            files.push(current.to_owned());
        }
        // Named by the millisecond they were made in, so an original comes before its copies.
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        let mut history = Self {
            quotas: super::quota::read(),
            ..Self::default()
        };
        let mut seen = HashSet::new();
        for path in files {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let session = history.files.len();
                    let summary = history.add(&bytes, session, &mut seen);
                    history.files.push(path);
                    history.summaries.push(summary);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => history.unreadable.push((path, error.to_string())),
            }
        }
        history
    }

    /// The index of `path` in `files`.
    pub fn session(&self, path: &Path) -> Option<usize> {
        self.files.iter().position(|file| file == path)
    }

    /// Adds the session's responses.
    fn add(&mut self, bytes: &[u8], session: usize, seen: &mut HashSet<u64>) -> Summary {
        let mut lines = bytes.split(|&byte| byte == b'\n');
        let header: Value = (lines.next())
            .and_then(|line| serde_json::from_slice(line).ok())
            .unwrap_or_default();
        if header["type"] != "session" {
            return Summary::default();
        }
        let created = header["created"].as_u64().unwrap_or(0);
        let (usage, subagents) = (format!("\"{USAGE}\""), format!("\"{SUBAGENTS}\""));
        let (mut name, mut prompt) = (None, None);
        for line in lines {
            let Ok(text) = std::str::from_utf8(line) else {
                continue;
            };
            let titled = |value: &Value, key: &str| {
                (value[key].as_str())
                    .map(|text| text.lines().next().unwrap_or("").trim().to_owned())
            };
            if text.contains("\"session_info\"")
                && let Ok(item) = serde_json::from_str::<Value>(text)
                && item["type"] == "session_info"
            {
                name = titled(&item, "name").filter(|name| !name.is_empty());
                continue;
            }
            if prompt.is_none()
                && text.contains("\"role\":\"user\"")
                && let Ok(item) = serde_json::from_str::<Value>(text)
                && super::session::is_prompt(&item)
            {
                prompt = titled(&item, TYPED).or_else(|| titled(&item, "content"));
            }
            if !text.contains(&usage) && !text.contains(&subagents) {
                continue;
            }
            let mut hasher = DefaultHasher::new();
            line.hash(&mut hasher);
            if !seen.insert(hasher.finish()) {
                continue;
            }
            let Ok(item) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            let model = item[PRODUCER].as_str().unwrap_or("");
            if let Some(response) = response(&item[USAGE], &item[TIMING], model, created) {
                self.responses.push(Response {
                    session,
                    ..response
                });
            }
            for run in item[SUBAGENTS]["runs"].as_array().into_iter().flatten() {
                self.add_run(run, created, session);
            }
        }
        Summary {
            root: header["root"].as_str().map(PathBuf::from),
            title: name.or(prompt).filter(|title| !title.is_empty()),
        }
    }

    /// A subagent's responses, or, saved before each response was (0.47.0), its total as one.
    fn add_run(&mut self, run: &Value, created: u64, session: usize) {
        let model = run["model"].as_str().unwrap_or("");
        let responses = run["responses"].as_array().filter(|list| !list.is_empty());
        if let Some(responses) = responses {
            for saved in responses {
                if let Some(response) = response(&saved["usage"], &saved["timing"], model, created)
                {
                    self.responses.push(Response {
                        session,
                        ..response
                    });
                }
            }
            return;
        }
        let usage = &run["usage"];
        if usage["turns"].as_u64().unwrap_or(0) == 0 {
            return;
        }
        let count = |key: &str| usage[key].as_u64().unwrap_or(0);
        let (read, written) = (count("cached_input_tokens"), count("cache_write_tokens"));
        self.responses.push(Response {
            at: created,
            model: model.to_owned(),
            input: count("input_tokens").saturating_sub(read + written),
            output: count("output_tokens"),
            cache_read: read,
            cache_write: written,
            cost: usage["cost"].as_f64(),
            first_output_ms: None,
            duration_ms: None,
            session,
        });
    }
}

/// A response saved as `usage` and `timing`, as a response's last item and a subagent's
/// `responses` save them.
fn response(usage: &Value, timing: &Value, model: &str, created: u64) -> Option<Response> {
    if !usage.is_object() {
        return None;
    }
    let count = |key: &str| usage[key].as_u64().unwrap_or(0);
    Some(Response {
        at: timing["started"].as_u64().unwrap_or(created),
        model: model.to_owned(),
        input: count("input"),
        output: count("output"),
        cache_read: count("cache_read"),
        cache_write: count("cache_write"),
        cost: usage["cost"].as_f64(),
        first_output_ms: timing["first_output_ms"].as_u64(),
        duration_ms: timing["duration_ms"].as_u64(),
        session: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;
    use serde_json::json;

    fn write(path: &Path, lines: &[Value]) {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn responses_are_read_from_every_project_once_each() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("sjrs-usage-{}", std::process::id())),
        );
        let sessions = scratch.0.join("sessions");
        let reply = json!({"type": "message", "role": "assistant", "_scramjet_model": "a/one",
            "_scramjet_usage": {"input": 10, "output": 5, "cache_read": 100, "cache_write": 20,
                "cost": 0.5},
            "_scramjet_timing": {"started": 5000, "duration_ms": 2000, "first_output_ms": 500}});
        let old = json!({"type": "function_call", "_scramjet_model": "a/one",
            "_scramjet_usage": {"input": 1, "output": 2, "cache_read": 0, "cache_write": 0}});
        let subagents = json!({"type": "function_call_output", "_scramjet_subagents": {"runs": [
            {"model": "b/two", "usage": {"turns": 1}, "responses": [
                {"usage": {"input": 3, "output": 4, "cache_read": 0, "cache_write": 0,
                    "cost": 0.25}, "timing": {"started": 6000, "duration_ms": 900}}]},
            {"model": "b/old", "usage": {"turns": 2, "input_tokens": 50,
                "cached_input_tokens": 30, "cache_write_tokens": 5, "output_tokens": 7}},
            {"model": "b/none", "usage": {"turns": 0}}]}});
        let header = |created: u64| json!({"type": "session", "created": created, "root": "/p"});
        let first = sessions.join("home-p").join("1000-1.jsonl");
        write(
            &first,
            &[
                header(1000),
                json!({"type": "message", "role": "user", "content": "typed\nmore",
                    "_scramjet_typed": "/rs:loop\nmore"}),
                reply.clone(),
                old,
            ],
        );
        // A fork of it, which copies the reply, then says one of its own.
        let own = json!({"type": "message", "_scramjet_usage": {"input": 7, "output": 0}});
        let fork = sessions.join("home-p").join("2000-1.jsonl");
        let named = json!({"type": "session_info", "name": "Forked"});
        write(&fork, &[header(2000), reply, named, own]);
        let other = sessions.join("home-q").join("3000-1.jsonl");
        write(&other, &[header(3000), subagents]);
        write(&sessions.join("home-q").join("notes.txt"), &[json!({})]);
        let elsewhere = scratch.0.join("unsaved").join("4000-1.jsonl");
        write(&elsewhere, &[json!({"type": "not a session"})]);

        let history = History::read(&sessions, &elsewhere);
        assert_eq!(
            history.files,
            [first, fork.clone(), other, elsewhere.clone()]
        );
        assert_eq!(history.session(&fork), Some(1));
        let titles: Vec<_> = (history.summaries.iter())
            .map(|summary| summary.title.as_deref())
            .collect();
        assert_eq!(titles, [Some("/rs:loop"), Some("Forked"), None, None]);
        assert_eq!(history.summaries[0].root.as_deref(), Some(Path::new("/p")));
        let got: Vec<_> = (history.responses.iter())
            .map(|r| {
                let timing = (r.first_output_ms, r.duration_ms);
                let tokens = (r.input, r.output, r.cache_read, r.cache_write);
                (r.session, r.at, r.model.as_str(), tokens, r.cost, timing)
            })
            .collect();
        assert_eq!(
            got,
            [
                (
                    0,
                    5000,
                    "a/one",
                    (10, 5, 100, 20),
                    Some(0.5),
                    (Some(500), Some(2000))
                ),
                (0, 1000, "a/one", (1, 2, 0, 0), None, (None, None)),
                (1, 2000, "", (7, 0, 0, 0), None, (None, None)),
                (
                    2,
                    6000,
                    "b/two",
                    (3, 4, 0, 0),
                    Some(0.25),
                    (None, Some(900))
                ),
                (2, 3000, "b/old", (15, 7, 30, 5), None, (None, None)),
            ]
        );
        assert!(history.unreadable.is_empty());
    }
}
