//! `display`: the user's settings for how the full screen draws a session, for a client that
//! draws it as theirs would, and told again when the settings file changes them.

use super::super::settings::{MarkdownReads, Settings};
use super::Out;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

const DISPLAY: &str = "_io.github.leanandmean.scramjet/display";

/// How often the settings file is looked at.
const EVERY: Duration = Duration::from_secs(1);

/// The display settings in `settings`, keyed as `settings.json` keys them.
pub(super) fn of(settings: &Settings) -> Value {
    let tool_display = match settings.markdown_reads {
        MarkdownReads::Rendered => "rendered",
        MarkdownReads::Source => "source",
    };
    json!({
        "look": settings.look.name(),
        "bottom": settings.bottom.name(),
        "banner": settings.banner.name(),
        "hideThinkingBlock": settings.hide_thinking_block,
        "markdown": {"toolDisplay": tool_display},
    })
}

pub(super) struct Watcher {
    stop: mpsc::Sender<()>,
    thread: JoinHandle<()>,
}

impl Watcher {
    /// Follows the settings file, notifying the client each time what `of` gives differs from
    /// `shown`.
    pub(super) fn start(out: Out, path: Option<PathBuf>, shown: Value) -> Self {
        let (stop, stopped) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let stamp = || {
                let metadata = std::fs::metadata(path.as_ref()?).ok()?;
                Some((metadata.modified().ok(), metadata.len()))
            };
            let (mut seen, mut shown) = (stamp(), shown);
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(EVERY) {
                let now = stamp();
                if now == seen {
                    continue;
                }
                seen = now;
                let display = of(&Settings::load());
                if display != shown {
                    shown = display;
                    out.send(&json!({"jsonrpc": "2.0", "method": DISPLAY,
                        "params": {"display": shown}}));
                }
            }
        });
        Self { stop, thread }
    }

    pub(super) fn stop(self) {
        drop(self.stop);
        let _ = self.thread.join();
    }
}
