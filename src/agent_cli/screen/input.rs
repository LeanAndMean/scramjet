//! The keys the screen takes, from the terminal's input and the session's wake-ups.

use super::draw::Activity;
use super::{Answer, App, BUSY, Flow, Input, MODEL_NEXT, NOTHING_QUEUED, Open, Optional, Shown};
use crate::agent_cli::clipboard;
use crate::agent_cli::editor::{Action, Note};
use crate::agent_cli::interactive::effort_note;
use crate::agent_cli::keymap::Binding;
use crate::agent_cli::keys::{ByteSource, ESCAPE_WAIT, Key};
use crate::agent_cli::select::Picked;
use crate::agent_cli::slash::Builtin;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};
use scramjet_rs::agent;
use std::{
    io::{self, PipeReader, Read},
    sync::atomic::Ordering,
    time::Duration,
};

/// The terminal's input, and the pipe the session writes to so that the screen looks at what it
/// sent.
pub(super) struct Source {
    buffer: Vec<u8>,
    start: usize,
    woken: PipeReader,
}

impl Source {
    pub(super) fn new(woken: PipeReader) -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            woken,
        }
    }
    pub(super) fn pending(&self) -> bool {
        self.start < self.buffer.len()
    }
}

impl ByteSource for Source {
    /// `None` also when the session sent something, except within a key's sequence, which a
    /// message must not split.
    fn byte(&mut self, wait: Option<Duration>) -> io::Result<Option<u8>> {
        if self.pending() {
            self.start += 1;
            return Ok(Some(self.buffer[self.start - 1]));
        }
        let stdin = io::stdin();
        let wakes = wait != Some(ESCAPE_WAIT);
        let timeout = wait.map(|wait| Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        });
        let mut fds = vec![PollFd::new(&stdin, PollFlags::IN)];
        if wakes {
            fds.push(PollFd::new(&self.woken, PollFlags::IN));
        }
        match poll(&mut fds, timeout.as_ref()) {
            Ok(0) | Err(Errno::INTR) => return Ok(None),
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
        let typed = !fds[0].revents().is_empty();
        if wakes && !fds[1].revents().is_empty() {
            let mut drained = [0; 256];
            while matches!(self.woken.read(&mut drained), Ok(n) if n > 0) {}
        }
        if !typed {
            return Ok(None);
        }
        self.buffer.resize(4096, 0);
        let read = loop {
            match rustix::io::read(&stdin, &mut self.buffer) {
                Err(Errno::INTR) => continue,
                result => break result?,
            }
        };
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.buffer.truncate(read);
        self.start = 1;
        Ok(Some(self.buffer[0]))
    }
}

impl App {
    pub(super) fn key(&mut self, key: Key, pending: bool) -> Flow {
        if self.exiting {
            return Flow::Go;
        }
        if key != Key::Idle
            && let Some(Open {
                shown: Shown::Pick(picker),
                ..
            }) = &mut self.dialog
        {
            picker.stop();
        }
        if let Key::Mouse(mouse) = key {
            self.mouse(mouse);
            return Flow::Go;
        }
        if key != Key::Idle {
            self.click = None;
        }
        // As in Scramjet, Ctrl+C copies what is selected, Escape lets it go and goes back to the
        // end, and other keys let it go and do what they do.
        let paging = match (key.is(Binding::PageUp), key.is(Binding::PageDown)) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        };
        if self.selection.is_some() {
            if key.is(Binding::Copy) {
                self.copy();
                return Flow::Go;
            }
            if key.is(Binding::Interrupt) {
                self.clear_selection();
                self.top = None;
                return Flow::Go;
            }
            if key != Key::Idle && paging.is_none() {
                self.clear_selection();
            }
        }
        let listing = matches!(
            self.dialog,
            Some(Open {
                shown: Shown::Pick(_),
                ..
            })
        );
        match key {
            Key::Idle => return Flow::Go,
            _ if key.is(Binding::Suspend) => return Flow::Suspend,
            _ if key.is(Binding::PasteImage) => {
                self.paste(clipboard::paste_image);
                return Flow::Go;
            }
            _ if key.is(Binding::ToolsExpand) && !self.list_takes(&key) => {
                self.whole = !self.whole;
                return Flow::Go;
            }
            _ if let Some(up) = paging
                && !listing =>
            {
                let page = self.rows.saturating_sub(1).max(1) as isize;
                self.navigate(if up { -page } else { page });
                return Flow::Go;
            }
            _ if key.is(Binding::Top) => {
                self.navigate(isize::MIN);
                return Flow::Go;
            }
            _ if key.is(Binding::Bottom) => {
                self.navigate(isize::MAX);
                return Flow::Go;
            }
            _ => {}
        }
        if self.dialog.is_some() {
            return self.dialog_key(key, pending);
        }
        let running = self.activity.is_some();
        if key.is(Binding::Interrupt) && !self.draft.completing() {
            if self.top.is_some() {
                self.top = None;
                return Flow::Go;
            }
            if running {
                self.restore();
                self.cancel.store(true, Ordering::Relaxed);
                return Flow::Go;
            }
            let queue = self.queue();
            if let Some(side) = queue.shells.iter().find(|side| side.ended.is_none()) {
                side.cancel.store(true, Ordering::Relaxed);
                return Flow::Go;
            }
        }
        if key.is(Binding::Clear) && running {
            self.cancel.store(true, Ordering::Relaxed);
        }
        if key.is(Binding::Dequeue) {
            let empty = self.queue().is_empty();
            match empty {
                true => (self.draft).set_note(Note::Info(NOTHING_QUEUED.to_owned())),
                false => self.restore(),
            }
            return Flow::Go;
        }
        // Between turns, the editor sends it as Enter, as Scramjet's.
        if key.is(Binding::FollowUp) && running {
            self.enqueue(true);
            return Flow::Go;
        }
        // `/model`, as typed, but the draft stays.
        if key.is(Binding::ModelSelect) {
            let model = "/model".to_owned();
            if running {
                self.queue().commands.push(model);
            } else if self.ready {
                self.ready = false;
                self.top = None;
                let _ = self.inputs.send(Input::Prompt(model));
            } else {
                self.draft.set_note(Note::Info(BUSY.to_owned()));
            }
            return Flow::Go;
        }
        match self.editor.key(&mut self.draft, key, pending) {
            Action::Edited(_) => {}
            // The draft holds the text sent.
            Action::Submit(_) if running => self.enqueue(false),
            Action::Submit(text) if self.ready => {
                // As in Scramjet, Up does not recall built-in commands.
                if Builtin::parse(&text).is_none() {
                    self.editor.remember(&text);
                }
                self.ready = false;
                self.top = None;
                let _ = self.inputs.send(Input::Prompt(text));
                self.draft = self.editor.draft(String::new());
            }
            Action::Submit(_) => self.draft.set_note(Note::Info(BUSY.to_owned())),
            Action::Exit => self.exit(),
            Action::DoubleEscape if self.ready => {
                let _ = self.inputs.send(Input::DoubleEscape);
                self.ready = false;
            }
            Action::DoubleEscape => {}
            Action::CycleEffort if running => {
                let level = self.efforts.advance();
                if let Some(level) = &level {
                    self.queue().effort = Some(level.clone());
                    self.footer.shown.effort = Some(level.clone());
                }
                if let Some(note) = effort_note(Ok(level)) {
                    self.draft.set_note(note);
                }
            }
            Action::CycleEffort if self.ready => {
                let _ = self.inputs.send(Input::CycleEffort);
            }
            Action::CycleEffort => self.draft.set_note(Note::Info(BUSY.to_owned())),
            Action::CycleModel(steps) if running => {
                self.queue().model_steps += steps;
                self.draft.set_note(Note::Info(MODEL_NEXT.to_owned()));
            }
            Action::CycleModel(steps) if self.ready => {
                let _ = self.inputs.send(Input::CycleModel(steps));
            }
            Action::CycleModel(_) => self.draft.set_note(Note::Info(BUSY.to_owned())),
            Action::Suspend => return Flow::Suspend,
            Action::External => return Flow::External,
        }
        Flow::Go
    }

    /// Whether the list shown takes `key`, which the screen otherwise takes for itself.
    fn list_takes(&self, key: &Key) -> bool {
        matches!(&self.dialog, Some(Open { shown: Shown::Pick(picker), .. }) if picker.takes(key))
    }

    fn dialog_key(&mut self, key: Key, pending: bool) -> Flow {
        if self.dialog.as_ref().is_some_and(|open| open.answered) {
            self.held_keys.push((key, pending));
            return Flow::Go;
        }
        if key.is(Binding::Clear)
            && matches!(
                self.dialog,
                Some(Open {
                    shown: Shown::Approve(_),
                    ..
                })
            )
        {
            // As during the rest of the turn, Ctrl-C stops it; the question goes unanswered, so
            // the session shows no answer to it.
            self.cancel.store(true, Ordering::Relaxed);
            self.dialog = None;
            if let Some(activity) = &mut self.activity {
                *activity = Activity::new("Working");
            }
            return Flow::Go;
        }
        let Some(open) = &mut self.dialog else {
            return Flow::Go;
        };
        let answer = match &mut open.shown {
            Shown::Pick(picker) if key.is(Binding::ThinkingToggle) && !picker.takes(&key) => {
                self.hide_thinking.fetch_xor(true, Ordering::Relaxed);
                None
            }
            Shown::Pick(picker) => picker.key(&key).map(|picked| match picked {
                Picked::Row(Some(row)) if let Some(routed) = picker.routed() => {
                    Answer::Routed(row, routed)
                }
                Picked::Row(row) => Answer::Picked(row),
                Picked::Act(act, row, place) => Answer::Acted(act, row, place),
                Picked::Toggle(toggle, place) => Answer::Toggled(toggle, place),
                Picked::Tab(place) => Answer::Tabbed(place),
                Picked::Label(entry) => Answer::Label(entry, None),
            }),
            Shown::Approve(_) => match key {
                Key::Text(text) if text.eq_ignore_ascii_case("y") => {
                    Some(Answer::Approved(agent::Answer::Allow))
                }
                Key::Text(text) if text.eq_ignore_ascii_case("a") => {
                    Some(Answer::Approved(agent::Answer::AllowAll))
                }
                Key::Text(text) if text.eq_ignore_ascii_case("n") => {
                    Some(Answer::Approved(agent::Answer::Deny))
                }
                _ if key.is(Binding::SelectConfirm) || key.is(Binding::SelectCancel) => {
                    Some(Answer::Approved(agent::Answer::Deny))
                }
                _ => None,
            },
            Shown::Text(text)
                if key.is(Binding::Submit)
                    && text.optional == Optional::Yes
                    && text.draft.text().trim().is_empty()
                    && !text.draft.completing() =>
            {
                Some(Answer::Text(Some(String::new())))
            }
            Shown::Text(text) => match text.editor.key(&mut text.draft, key, pending) {
                Action::Submit(text) => Some(Answer::Text(Some(text))),
                Action::Exit => Some(Answer::Text(None)),
                Action::Suspend => return Flow::Suspend,
                _ => None,
            },
        };
        if answer.is_some() {
            self.answer(answer);
        }
        Flow::Go
    }
}
