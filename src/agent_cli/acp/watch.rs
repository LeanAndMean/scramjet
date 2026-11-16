//! `watch`: a session a terminal holds, shown read-only from its file as it grows, never opened.

use super::{INVALID_PARAMS, Log, Out, Updates, lock};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

pub(super) const WATCH: &str = "_io.github.leanandmean.scramjet/watch";
pub(super) const UNWATCH: &str = "_io.github.leanandmean.scramjet/unwatch";
/// The watched session's branch is no longer what was shown: it is shown again from its start.
const WATCH_RESET: &str = "_io.github.leanandmean.scramjet/watch_reset";
/// The watched session's file can no longer be read, or no terminal holds it any more: the
/// watch is over.
const WATCH_ENDED: &str = "_io.github.leanandmean.scramjet/watch_ended";

/// How often a watched file is looked at.
const EVERY: Duration = Duration::from_millis(300);
/// Every how many looks the terminal's record is looked for.
const HOLDER_EVERY: u32 = 5;

pub(super) struct Watch {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Watch {
    pub(super) fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

/// The client's flags its updates follow.
#[derive(Clone, Copy)]
pub(super) struct Shown {
    pub scramjet: bool,
    pub terminals: super::Terminals,
}

/// The file and project of the session `id` that a terminal holds, as `live_sessions` lists it.
pub(super) fn held(id: &str) -> Option<(PathBuf, PathBuf, Value)> {
    (super::super::live::list().into_iter())
        .find(|record| record["sessionId"] == id)
        .and_then(|record| {
            let path = PathBuf::from(record["path"].as_str()?);
            let cwd = PathBuf::from(record["cwd"].as_str()?);
            Some((path, cwd, record["pid"].clone()))
        })
}

/// Starts following session `id`: its branch replayed as `session/load` replays it, then request
/// `request` answered, then each entry as its file takes it.
pub(super) fn start(out: &Out, request: &Value, id: &str, shown: Shown) -> Option<Watch> {
    let Some((path, root, _)) = held(id) else {
        let message = format!("no terminal holds session {id:?}");
        out.error(request, INVALID_PARAMS, &message);
        return None;
    };
    let stop = Arc::<AtomicBool>::default();
    let (out, request, id, halt) = (out.clone(), request.clone(), id.to_owned(), stop.clone());
    let thread = std::thread::spawn(move || {
        let mut follower = Follower::new(out.clone(), id, path, root, shown);
        if let Err(error) = follower.look(true) {
            return out.error(&request, INVALID_PARAMS, &error);
        }
        out.respond(&request, json!({}));
        let mut looks = 0u32;
        while !halt.load(Ordering::Relaxed) {
            std::thread::sleep(EVERY);
            looks += 1;
            let gone = looks.is_multiple_of(HOLDER_EVERY)
                && held(&follower.id).is_none_or(|(path, ..)| path != follower.path);
            let ended = match follower.look(false) {
                Err(error) => error,
                Ok(()) if gone => "no terminal holds it any more".to_owned(),
                Ok(()) => continue,
            };
            let params = json!({"sessionId": follower.id, "message": ended});
            return lock(&follower.ui.log).notify(&out, WATCH_ENDED, params);
        }
    });
    Some(Watch { stop, thread })
}

struct Follower {
    out: Out,
    id: String,
    path: PathBuf,
    root: PathBuf,
    shown: Shown,
    ui: Updates,
    /// The lines of the items shown.
    lines: Vec<Option<usize>>,
    /// The file's length and time when last read.
    seen: Option<(u64, SystemTime)>,
}

impl Follower {
    fn new(out: Out, id: String, path: PathBuf, root: PathBuf, shown: Shown) -> Self {
        let ui = updates(&out, &id, &root, shown);
        Follower {
            out,
            id,
            path,
            root,
            shown,
            ui,
            lines: Vec::new(),
            seen: None,
        }
    }

    /// Shows what the file holds that was not shown; `first`: the first look.
    fn look(&mut self, first: bool) -> Result<(), String> {
        let failed = |error: std::io::Error| format!("{}: {error}", self.path.display());
        let metadata = std::fs::metadata(&self.path).map_err(failed)?;
        let seen = (metadata.len(), metadata.modified().map_err(failed)?);
        if self.seen == Some(seen) {
            return Ok(());
        }
        self.seen = Some(seen);
        let (items, lines) = scramjet::agent::read_conversation(&self.path).map_err(failed)?;
        let settled = settled(&items);
        let from = self.lines.len();
        let continues = lines.len() >= from && lines[..from] == self.lines[..];
        let from = match continues {
            true => from,
            false => {
                if !first {
                    let params = json!({"sessionId": self.id});
                    lock(&self.ui.log).notify(&self.out, WATCH_RESET, params);
                }
                self.ui = updates(&self.out, &self.id, &self.root, self.shown);
                0
            }
        };
        if settled > from {
            let root = Some(self.root.as_path());
            scramjet::agent::show_entries(
                &items[from..settled],
                &lines[from..settled],
                root,
                &mut self.ui,
            );
            self.ui.flush();
            self.ui.entry = None;
        }
        self.lines = lines[..settled.max(from)].to_vec();
        Ok(())
    }
}

fn updates(out: &Out, id: &str, root: &Path, shown: Shown) -> Updates {
    let log = Arc::new(Mutex::new(Log::new(shown.scramjet)));
    Updates {
        scramjet: shown.scramjet,
        terminals: shown.terminals,
        replaying: true,
        ..Updates::new(out.clone(), log, id.to_owned(), root)
    }
}

/// How many of `items` show now: up to the last where each call before has its result, as a
/// replay shows a call with its result.
fn settled(items: &[Value]) -> usize {
    let mut open = HashSet::new();
    let mut settled = 0;
    for (at, item) in items.iter().enumerate() {
        let id = item["call_id"].as_str().unwrap_or_default();
        match item["type"].as_str() {
            Some("function_call") => {
                open.insert(id);
            }
            Some("function_call_output") => {
                open.remove(id);
            }
            _ => {}
        }
        if open.is_empty() {
            settled = at + 1;
        }
    }
    settled
}

#[cfg(test)]
mod tests {
    use super::settled;
    use serde_json::json;

    #[test]
    fn calls_show_once_every_call_of_their_reply_has_its_result() {
        let call = |id: &str| json!({"type": "function_call", "call_id": id});
        let output = |id: &str| json!({"type": "function_call_output", "call_id": id});
        let message = json!({"type": "message", "role": "user"});
        let items = [message.clone(), call("a"), call("b"), output("a")];
        assert_eq!(settled(&items), 1);
        let items = [
            message.clone(),
            call("a"),
            call("b"),
            output("a"),
            output("b"),
        ];
        assert_eq!(settled(&items), 5);
        let items = [message, call("a"), output("a"), call("b")];
        assert_eq!(settled(&items), 3);
    }
}
