//! Keeping the unsent prompt with its session: written a few seconds after it changes and as
//! the screen ends, and read back when the session opens.

use super::App;
use crate::agent_cli::editor::{Kept, Note};
use scramjet::agent::draft;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

/// How long after a change the draft is written.
const AFTER: Duration = Duration::from_secs(3);

/// The session the draft is kept with, and how far it was written.
#[derive(Default)]
pub(super) struct Drafts {
    /// The saved session's file; `None` before the session says, or for one saved nowhere.
    session: Option<PathBuf>,
    /// The draft's revision written last.
    written: u64,
    /// When the changes since are written.
    due: Option<Instant>,
    /// The last write failed, which was said.
    failed: bool,
}

impl App {
    /// The draft goes with the session saved at `session` from now on: the one before is written
    /// for the session before, and the new session's comes back if the prompt is empty.
    pub(super) fn drafting(&mut self, session: Option<PathBuf>) {
        if self.drafts.session == session {
            return;
        }
        self.keep_draft();
        self.drafts = Drafts {
            session,
            ..Drafts::default()
        };
        if let Some(session) = &self.drafts.session
            && self.draft.text().is_empty()
        {
            let restored = draft::read(session).map(|bytes| {
                let kept = bytes.map(|bytes| serde_json::from_slice::<Kept>(&bytes));
                kept.map(|kept| kept.ok().and_then(|kept| self.editor.restored(kept)))
            });
            match restored {
                Ok(None) => {}
                Ok(Some(Some(draft))) => self.draft = draft,
                Ok(Some(None)) => self.draft.set_note(Note::Error(
                    "Error: the unsent prompt kept is not one this version reads".to_owned(),
                )),
                Err(error) => self.draft.set_note(Note::Error(format!(
                    "Error: the unsent prompt kept not read: {error}"
                ))),
            }
        }
        self.drafts.written = self.draft.revision();
    }

    /// Notes a change of the draft, which is written `AFTER` it, or at once when nothing is left
    /// to keep (a prompt sent); returns how long until it is due.
    pub(super) fn draft_due(&mut self) -> Option<Duration> {
        self.drafts.session.as_ref()?;
        if self.draft.revision() == self.drafts.written {
            self.drafts.due = None;
            return None;
        }
        let now = Instant::now();
        let due = match self.draft.keeps() {
            true => *self.drafts.due.get_or_insert(now + AFTER),
            false => now,
        };
        if due > now {
            return Some(due - now);
        }
        self.keep_draft();
        None
    }

    /// Writes the draft if it changed since it was last written. A write that fails is said
    /// once, and tried again at the next change.
    pub(super) fn keep_draft(&mut self) {
        let Some(session) = &self.drafts.session else {
            return;
        };
        let revision = self.draft.revision();
        if revision == self.drafts.written {
            return;
        }
        let kept = self.draft.kept().map(|kept| serde_json::to_vec(&kept));
        let written = match kept.transpose() {
            Ok(bytes) => draft::write(session, bytes.as_deref()),
            Err(error) => Err(error.into()),
        };
        (self.drafts.written, self.drafts.due) = (revision, None);
        match written {
            Ok(()) => self.drafts.failed = false,
            Err(_) if self.drafts.failed => {}
            Err(error) => {
                self.drafts.failed = true;
                self.draft.set_note(Note::Error(format!(
                    "Error: the unsent prompt not kept: {error}"
                )));
            }
        }
    }
}
