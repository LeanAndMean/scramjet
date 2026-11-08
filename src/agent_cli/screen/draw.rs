//! Drawing a frame: the conversation's lines in view with the scrollbar, and the dock under
//! it: the status line with what the agent does, queued messages, the prompt or a dialog, and
//! the footer.

use super::transcript::Look;
use super::{ALLOW, App, Open, Shown};
use crate::agent_cli::editor::{Draft, Note, terminal_size};
use crate::agent_cli::keymap::{self, Binding};
use crate::agent_cli::record::{self, Event};
use crate::agent_cli::selection::{self, Line};
use crate::agent_cli::settings::{Layout, scramjet_level};
use crate::agent_cli::style::{self, Band};
use crate::agent_cli::terminal::{
    COMMAND_LINES, Terminal, elapsed, retrying, spinner, waiting_for,
};
use crate::agent_cli::text::{Wrap, clip, columns, one_line, plain, visible};
use scramjet_rs::agent::{Purpose, StreamProgress};
use std::{
    fmt::Write as _,
    io::{self, Write},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

/// What the agent does now, as the status line says it.
pub(super) struct Activity {
    pub(super) what: String,
    since: Instant,
    /// Shows the seconds left of this wait instead of the elapsed seconds.
    pub(super) countdown: Option<Duration>,
    /// What Escape does meanwhile, as Scramjet's status says: `interrupt` or `cancel`; `None`
    /// while the turn waits for the user's answer, when no spinner turns either.
    escape: Option<&'static str>,
}

impl Activity {
    /// Part of a turn, which Escape interrupts.
    pub(super) fn new(what: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            since: Instant::now(),
            countdown: None,
            escape: Some("interrupt"),
        }
    }
    /// A summary or a wait to retry, which Escape cancels.
    fn cancellable(what: impl Into<String>) -> Self {
        Self {
            escape: Some("cancel"),
            ..Self::new(what)
        }
    }
    /// The turn waits for the user to answer a question.
    pub(super) fn asking() -> Self {
        Self {
            escape: None,
            ..Self::new("Waiting for you")
        }
    }
    pub(super) fn asks(&self) -> bool {
        self.escape.is_none()
    }
    /// What the status line says once `event` happened, if that changes it.
    pub(super) fn after(event: &Event) -> Option<Self> {
        Some(match event {
            Event::Waiting(Purpose::Reply) => Self::new(waiting_for(Purpose::Reply)),
            Event::Waiting(purpose) => Self::cancellable(waiting_for(*purpose)),
            Event::Progress(StreamProgress::Reasoning(_)) => Self::new("Thinking"),
            Event::Progress(StreamProgress::Text(_)) => Self::new("Responding"),
            Event::Progress(StreamProgress::Call(name)) => Self::new(format!("Preparing {name}")),
            Event::ToolRunning(since) => Self {
                since: *since,
                ..Self::new("Running")
            },
            Event::CommandStarted(..) => Self::new("Running"),
            Event::ToolResult(..) | Event::CommandResult(..) => Self::new("Working"),
            Event::Retrying(_, attempt, retries, delay) => Self {
                countdown: Some(*delay),
                ..Self::cancellable(retrying(*attempt, *retries))
            },
            _ => return None,
        })
    }
    /// The status line's text, with what Escape does when `escape` says it does that now.
    fn line(&self, escape: bool) -> String {
        let hint = (self.escape.filter(|_| escape))
            .and_then(|does| {
                keymap::key(Binding::Interrupt).map(|key| format!(" ({key} to {does})"))
            })
            .unwrap_or_default();
        match self.countdown {
            Some(wait) => {
                let left = wait.saturating_sub(self.since.elapsed()).as_secs_f64();
                format!("{} {}s{hint}", self.what, left.ceil())
            }
            None => format!(
                "{}{}{hint}",
                self.what,
                elapsed(self.since.elapsed().as_secs())
            ),
        }
    }
    /// The status line's text after the spinner, while the agent works.
    fn shown(&self, escape: bool) -> String {
        let color = match self.countdown {
            Some(_) => style::SPINNER_RETRYING,
            None => style::SPINNER,
        };
        match self.escape {
            // Out of the status line's dim style, as Scramjet's accent color shows it.
            Some(_) => format!(
                "{}{}{} {}",
                style::RESET,
                spinner(color),
                style::STATUS,
                self.line(escape)
            ),
            None => self.line(escape),
        }
    }
}

/// Adds the prompt's rows to `rows`, at most the `layout`'s share of `room` around the cursor,
/// then its completion list; returns where the cursor is. With `spelled`, misspelled words are
/// marked.
fn prompt_rows(
    draft: &Draft,
    rows: &mut Vec<Line>,
    (width, room): (usize, usize),
    layout: Layout,
    spelled: bool,
) -> Option<(usize, usize)> {
    let (text, row, column) = draft.rows(width, spelled);
    let shown = (room * layout.editor_share / 100).max(1).min(text.len());
    let top = (row + 1).saturating_sub(shown).min(text.len() - shown);
    let start = rows.len();
    rows.extend(selection::rows(&text[top..top + shown]));
    rows.extend(selection::rows(
        &draft.completion_rows(width, layout.completion_rows),
    ));
    Some((start + row - top, column))
}

/// The style of the prompt's rules, as Scramjet's editor border: the bash-mode color while
/// `draft` is a command, else the color of the thinking level `effort`, off for a model that
/// does not reason or a level Scramjet does not know.
fn prompt_border(effort: Option<&str>, draft: &Draft) -> style::Style {
    if draft.text().trim_start().starts_with('!') {
        return style::PROMPT_COMMAND;
    }
    match effort.map(scramjet_level) {
        Some("minimal") => style::PROMPT_MINIMAL,
        Some("low") => style::PROMPT_LOW,
        Some("medium") => style::PROMPT_MEDIUM,
        Some("high") => style::PROMPT_HIGH,
        Some("xhigh" | "max") => style::PROMPT_XHIGH,
        _ => style::PROMPT_OFF,
    }
}

/// The status line's rows, `width` columns wide: `activity`, then `note`, and at the right, as
/// the footer places the model, the `below` lines past the view and the keys that go back
/// (`back`), shortened to `↓N`, then left out, where the left needs the room. A note that does
/// not fit after the activity goes on rows of its own, wrapped, so that it stays in place as the
/// seconds go by.
fn status_rows(
    activity: Option<String>,
    note: Option<&str>,
    below: usize,
    back: &str,
    width: usize,
) -> Vec<String> {
    let room = width.saturating_sub(1);
    let wrapped = |text: &str| {
        let mut wrap = Wrap::new(room.max(10));
        let text = wrap.push(text) + &wrap.finish();
        text.split('\n').map(str::to_owned).collect::<Vec<_>>()
    };
    let line = [activity.as_deref(), note].into_iter().flatten();
    let line = line.collect::<Vec<_>>().join(" · ");
    let mut rows = match (activity, note) {
        (_, None) => vec![clip(&line, room)],
        (_, Some(note)) if !note.contains('\n') && columns(&line) <= room => vec![line],
        (None, Some(note)) => wrapped(note),
        (Some(activity), Some(note)) => [clip(&activity, room)]
            .into_iter()
            .chain(wrapped(note))
            .collect(),
    };
    if below > 0 {
        let first = &mut rows[0];
        let used = columns(first);
        // Two columns apart, as the footer's model.
        let gap = if used == 0 { 0 } else { 2 };
        let right = [
            format!("{below} more lines below{back}"),
            format!("↓{below}"),
        ]
        .into_iter()
        .find(|right| used + gap + columns(right) <= room);
        if let Some(right) = right {
            *first += &" ".repeat(room - used - columns(&right));
            *first += &right;
        }
    }
    (rows.into_iter())
        .map(|row| format!("{}{row}{}", style::STATUS, style::RESET))
        .collect()
}

/// A dock row of `text` in the muted style.
fn muted(text: &str) -> Line {
    selection::row(&format!("{}{text}{}", style::MUTED, style::RESET))
}

impl App {
    /// The status line: what a running turn does and the note, and how far the view is from
    /// the end.
    fn status(&self, width: usize) -> Vec<String> {
        // Escape goes back to the end, lets a selection go or closes completion first.
        let escape = self.top.is_none() && self.selection.is_none() && !self.draft.completing();
        let handed = self.handed.as_ref().map(|(waiting, _)| waiting);
        let activity = (self.activity.as_ref().or(handed)).map(|activity| activity.shown(escape));
        let below = self.last.saturating_sub(self.shown_top());
        // A list takes PgDn, and a dialog or the completion list Escape, but not from a selection.
        let page = !matches!(
            self.dialog,
            Some(Open {
                shown: Shown::Pick(_),
                ..
            })
        );
        let escape =
            self.selection.is_some() || (self.dialog.is_none() && !self.draft.completing());
        let page = page.then(|| keymap::key(Binding::PageDown)).flatten();
        let escape = escape.then(|| keymap::key(Binding::Interrupt)).flatten();
        let back = match (page, escape) {
            (Some(page), Some(escape)) => format!(" · {page} or {escape} to go back"),
            (Some(key), None) | (None, Some(key)) => format!(" · {key} to go back"),
            (None, None) => String::new(),
        };
        let note = self.shown_draft().note();
        status_rows(activity, note.as_deref(), below, &back, width)
    }

    /// The draft whose note the status line shows: a text dialog's, else the prompt's.
    fn shown_draft(&self) -> &Draft {
        match &self.dialog {
            Some(Open {
                shown: Shown::Text(text),
                ..
            }) => &text.draft,
            _ => &self.draft,
        }
    }

    /// Sets the note of the draft the status line shows.
    pub(super) fn note(&mut self, note: Note) {
        match self.editing() {
            Some((_, draft)) => draft.set_note(note),
            None => self.draft.set_note(note),
        }
    }

    /// The rows under the conversation, and where the cursor goes in them.
    fn dock(&self, (width, height): (usize, usize)) -> (Vec<Line>, Option<(usize, usize)>) {
        let mut rows = Vec::new();
        let queue = self.queue();
        // Above the status line, as Scramjet shows them, each in at most about half the screen
        // (Scramjet's view scrolls instead, and its editor stays).
        let lines = (height / 2).saturating_sub(6).clamp(1, COMMAND_LINES);
        for side in &queue.shells {
            let events = [
                Event::CommandStarted(side.command.clone(), side.include),
                match &side.ended {
                    Some(shell) => Event::CommandResult(shell.shown()),
                    None => Event::ToolOutput(side.output.clone()),
                },
            ];
            let content = width.saturating_sub(1).max(1);
            let mut ui = Terminal::capture_dock(content, self.hide_thinking.clone(), lines);
            for event in &events {
                record::replay(event, &mut ui);
            }
            record::running(events.last(), &mut ui);
            rows.extend(selection::lines(&ui.captured()));
        }
        rows.extend(selection::rows(&self.status(width)));
        let queued = [
            ("Steering", &queue.steering),
            ("Follow-up", &queue.follow_ups),
        ];
        for (kind, messages) in queued {
            for message in messages {
                let line = clip(
                    &format!("{kind}: {}", one_line(message)),
                    width.saturating_sub(1),
                );
                rows.push(muted(&line));
            }
        }
        for command in &queue.commands {
            let line = clip(
                &format!("Command: {}", one_line(command)),
                width.saturating_sub(1),
            );
            rows.push(muted(&line));
        }
        if !queue.is_empty()
            && let Some(key) = keymap::key(Binding::Dequeue)
        {
            let hint = format!("↳ {key} to edit all queued messages");
            rows.push(muted(&clip(&hint, width.saturating_sub(1))));
        }
        drop(queue);
        let footer = self.footer.lines(width);
        // The rule above the footer, and the footer.
        let under = 1 + footer.len();
        let room = height.saturating_sub(rows.len() + 1 + under).max(1);
        let cursor = match &self.dialog {
            Some(Open {
                shown: Shown::Pick(picker),
                ..
            }) => {
                rows.extend(selection::rows(&picker.lines((width, room))));
                None
            }
            Some(Open {
                shown: Shown::Theme(editor),
                ..
            }) => {
                rows.extend(selection::rows(&editor.lines((width, room))));
                None
            }
            Some(Open {
                shown: Shown::Usage(view),
                ..
            }) => {
                rows.extend(selection::rows(&view.lines(width)));
                None
            }
            Some(Open {
                shown: Shown::Approve(question),
                ..
            }) => {
                let mut wrap = Wrap::new(width.saturating_sub(1).max(10)).marking(true);
                let text = wrap.push(&visible(question)) + &wrap.finish();
                rows.extend(selection::rows(&text.split('\n').collect::<Vec<_>>()));
                rows.push(selection::row(&format!("{ALLOW} ")));
                Some((rows.len() - 1, ALLOW.len() + 1))
            }
            Some(Open {
                shown: Shown::Text(text),
                ..
            }) => {
                let keys = keymap::hint([
                    keymap::part(&[Binding::Submit], "submit"),
                    keymap::part(&[Binding::SelectCancel], "cancel"),
                ]);
                let title = format!(
                    "{}{}{}  {}{keys}{}",
                    style::SELECTOR_TITLE,
                    text.title,
                    style::RESET,
                    style::MUTED,
                    style::RESET
                );
                rows.push(selection::row(&clip(&title, width.saturating_sub(1))));
                prompt_rows(&text.draft, &mut rows, (width, room), self.layout, false)
            }
            Some(Open {
                shown: Shown::Press(title, note) | Shown::Wait(title, note),
                ..
            }) => {
                // A key pressed for `/hotkeys` is taken as it is: Escape is not remapped there.
                let cancel = match &self.dialog {
                    Some(Open {
                        shown: Shown::Wait(..),
                        ..
                    }) => keymap::hint([keymap::part(&[Binding::SelectCancel], "cancel")]),
                    _ => "Esc cancels".to_owned(),
                };
                let title = format!(
                    "{}{title}{}  {}{cancel}{}",
                    style::SELECTOR_TITLE,
                    style::RESET,
                    style::MUTED,
                    style::RESET
                );
                rows.push(selection::row(&clip(&title, width.saturating_sub(1))));
                let mut wrap = Wrap::new(width.saturating_sub(3).max(10));
                let note = wrap.push(&visible(note)) + &wrap.finish();
                for line in note.split('\n') {
                    let line = format!("  {}{line}{}", style::MUTED, style::RESET);
                    rows.push(selection::row(&line));
                }
                None
            }
            None => {
                let border = prompt_border(self.footer.shown.effort.as_deref(), &self.draft);
                let rule = Line::rule(&format!(
                    "{border}{}{}",
                    "─".repeat(width.saturating_sub(1)),
                    style::RESET
                ));
                rows.push(rule.clone());
                let cursor = prompt_rows(&self.draft, &mut rows, (width, room), self.layout, true);
                rows.push(rule);
                cursor
            }
        };
        // As Scramjet's editor and its lists end with a rule over the footer.
        let ruled = (rows.last()).is_some_and(|row| {
            let text = plain(&row.shown);
            !text.is_empty() && text.chars().all(|c| c == '─')
        });
        if !ruled {
            let color = match &self.dialog {
                Some(Open {
                    shown: Shown::Theme(editor),
                    ..
                }) => editor.rule(),
                _ => style::DIALOG_RULE,
            };
            rows.push(Line::rule(&format!(
                "{}{}{}",
                color,
                "─".repeat(width.saturating_sub(1)),
                style::RESET
            )));
        }
        rows.extend(selection::rows(&footer));
        (rows, cursor)
    }

    pub(super) fn draw(&mut self) -> io::Result<()> {
        self.preview();
        let (width, height) = terminal_size();
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        // Some terminals move what they show on a resize.
        if (width, height) != self.size {
            self.size = (width, height);
            self.drawn.clear();
            self.thumb = None;
            out.push_str("\x1b[2J");
        }
        let base = style::base();
        if base != self.base {
            self.base = base;
            self.drawn.clear();
        }
        // Given again with a whole frame, as after the screen was left, which clears it.
        let title = self.footer.title();
        let titled = self.drawn.is_empty() || title != self.title;
        if titled {
            write!(out, "\x1b]0;{title}\x07").unwrap();
            self.title = title;
        }
        let (dock, cursor) = self.dock((width, height));
        let rows = height.saturating_sub(dock.len()).max(1);
        // As Scramjet's, the last column is the scrollbar's, whether it shows or not, so that
        // the conversation does not wrap again when it grows taller than the screen.
        let look = Look {
            width: width.saturating_sub(1).max(1),
            hide_thinking: self.hide_thinking.load(Ordering::Relaxed),
            whole: self.whole,
            markdown_reads: self.markdown_reads,
            theme: style::generation(),
            design: self.design,
            banner: self.banner,
        };
        self.transcript.render(look, &self.hide_thinking);
        let last = self.transcript.len().saturating_sub(rows);
        (self.rows, self.last) = (rows, last);
        let top = self.shown_top();
        if top == last && self.selection.is_none() {
            self.top = None;
        }
        self.docked = dock;
        let selected = self.selected();
        // A row on a band is erased and painted on it rather than on the base.
        let shown = |line: &Line, at: usize| {
            let shown = match selected.as_ref().and_then(|s| s.columns(at)) {
                Some((left, right)) => selection::highlight(&line.shown, left, right),
                None => line.shown.clone(),
            };
            (shown, line.band.map_or(base, Band::get))
        };
        let window = self.transcript.window(top, rows);
        let mut frame: Vec<(String, &str)> = (window.iter().zip(top..))
            .map(|(line, at)| shown(line, at))
            .collect();
        frame.resize(rows, (String::new(), base));
        if let Some((size, first)) = self.thumb() {
            for (row, (line, _)) in frame.iter_mut().enumerate() {
                let cell = match (first..first + size).contains(&row) {
                    true => '█',
                    false => '│',
                };
                write!(line, "{}\x1b[{};{width}H{cell}", style::RESET, row + 1).unwrap();
            }
        }
        let length = self.transcript.len();
        frame.extend((self.docked.iter().zip(length..)).map(|(line, at)| shown(line, at)));
        frame.truncate(height);
        // Erased first: without autowrap, a row as wide as the screen leaves the cursor on its
        // last cell, which erasing after it would take. Erasing fills the row with the
        // background it is painted on.
        let frame: Vec<String> = (frame.into_iter())
            .map(|(row, base)| {
                let (row, reset) = (style::painted(&row, base), style::RESET);
                format!("{base}\x1b[2K{row}{reset}")
            })
            .collect();
        let cursor = cursor.map(|(row, column)| (rows + row, column));
        if frame == self.drawn && cursor == self.cursor && !titled {
            return Ok(());
        }
        for (index, row) in frame.iter().enumerate() {
            if self.drawn.get(index) != Some(row) {
                write!(out, "\x1b[{};1H{row}", index + 1).unwrap();
            }
        }
        if let Some((row, column)) = cursor.filter(|(row, _)| *row < height) {
            write!(out, "\x1b[{};{}H\x1b[?25h", row + 1, column + 1).unwrap();
        }
        out.push_str("\x1b[?2026l");
        let mut stderr = io::stderr().lock();
        stderr.write_all(out.as_bytes())?;
        stderr.flush()?;
        self.drawn = frame;
        self.cursor = cursor;
        self.drawn_at = Instant::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet_rs::agent::AutoCompaction;

    #[test]
    fn the_status_line_says_what_runs_and_what_escape_does() {
        let said = |event: Event, escape| Activity::after(&event).unwrap().line(escape);
        assert_eq!(
            said(Event::Waiting(Purpose::Reply), true),
            "Waiting for reply (Esc to interrupt)"
        );
        assert_eq!(
            said(Event::Waiting(Purpose::Reply), false),
            "Waiting for reply"
        );
        // What arrives says what the model does.
        let progress = |update| said(Event::Progress(update), true);
        assert_eq!(
            [
                progress(StreamProgress::Reasoning("Let me see".into())),
                progress(StreamProgress::Text("Sure".into())),
                progress(StreamProgress::Call("bash".into())),
            ],
            [
                "Thinking (Esc to interrupt)",
                "Responding (Esc to interrupt)",
                "Preparing bash (Esc to interrupt)",
            ]
        );
        let compaction = |reason| Event::Waiting(Purpose::Compaction(reason));
        assert_eq!(
            [
                said(compaction(None), true),
                said(compaction(Some(AutoCompaction::Threshold)), true),
                said(compaction(Some(AutoCompaction::Overflow)), true),
                said(Event::Waiting(Purpose::BranchSummary), true),
            ],
            [
                "Compacting context (Esc to cancel)",
                "Auto-compacting (Esc to cancel)",
                "Context overflow, compacting (Esc to cancel)",
                "Summarizing branch (Esc to cancel)",
            ]
        );
        assert_eq!(
            said(Event::ToolRunning(Instant::now()), true),
            "Running (Esc to interrupt)"
        );
        let retry = Event::Retrying("busy".into(), 2, 3, Duration::from_secs(2));
        assert_eq!(said(retry, true), "Retrying (2/3) in 2s (Esc to cancel)");
        assert_eq!(Activity::asking().line(true), "Waiting for you");
    }

    #[test]
    fn the_prompts_rules_take_the_thinking_levels_color_and_a_commands() {
        use crate::agent_cli::editor::Editor;
        use std::sync::{Arc, atomic::AtomicBool};
        let editor = Editor::dialog(Arc::new(AtomicBool::new(false)));
        let prompt = editor.draft("Fix the test".into());
        let levels = [
            (None, style::PROMPT_OFF),
            (Some("none"), style::PROMPT_OFF),
            (Some("off"), style::PROMPT_OFF),
            (Some("minimal"), style::PROMPT_MINIMAL),
            (Some("low"), style::PROMPT_LOW),
            (Some("medium"), style::PROMPT_MEDIUM),
            (Some("high"), style::PROMPT_HIGH),
            (Some("xhigh"), style::PROMPT_XHIGH),
            (Some("max"), style::PROMPT_XHIGH),
            (Some("ultra"), style::PROMPT_OFF),
        ];
        for (effort, border) in levels {
            assert_eq!(prompt_border(effort, &prompt), border, "{effort:?}");
        }
        for text in ["!cargo test", "  !!git status"] {
            let command = editor.draft(text.into());
            assert_eq!(prompt_border(Some("high"), &command), style::PROMPT_COMMAND);
        }
        let mentioned = editor.draft("Why did !cargo test fail?".into());
        assert_eq!(prompt_border(Some("high"), &mentioned), style::PROMPT_HIGH);
    }

    #[test]
    fn a_spinner_turns_before_what_the_agent_does_but_not_while_it_waits_for_you() {
        let working = plain(&Activity::new("Working").shown(true));
        let (frame, rest) = working.split_at(working.chars().next().unwrap().len_utf8());
        assert!("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(frame), "{working}");
        assert_eq!(rest, " Working (Esc to interrupt)");
        let retry = Event::Retrying("busy".into(), 1, 3, Duration::from_secs(2));
        let retrying = Activity::after(&retry).unwrap().shown(true);
        assert!(
            retrying.contains(style::SPINNER_RETRYING.get()),
            "{retrying:?}"
        );
        assert_eq!(plain(&Activity::asking().shown(true)), "Waiting for you");
    }

    #[test]
    fn the_status_line_puts_the_note_after_the_activity_and_the_view_at_the_right() {
        let rows = |activity: Option<&str>, note, below, width| {
            let back = " · PgDn or Esc to go back";
            let rows = status_rows(activity.map(str::to_owned), note, below, back, width);
            rows.iter().map(|row| plain(row)).collect::<Vec<_>>()
        };
        // `left`, then `right` ending in the last column but the scrollbar's.
        let apart = |left: &str, right: &str, width: usize| {
            let padding = width - 1 - columns(left) - columns(right);
            format!("{left}{}{right}", " ".repeat(padding))
        };
        let working = "⠹ Working 2s (Esc to interrupt)";
        assert_eq!(rows(Some(working), None, 0, 60), [working]);
        assert_eq!(
            rows(Some(working), Some("Copied"), 0, 60),
            ["⠹ Working 2s (Esc to interrupt) · Copied"]
        );
        assert_eq!(rows(None, Some("Copied"), 0, 60), ["Copied"]);
        assert_eq!(rows(None, None, 0, 60), [""]);
        let all = "18 more lines below · PgDn or Esc to go back";
        assert_eq!(rows(None, None, 18, 60), [apart("", all, 60)]);
        assert_eq!(
            rows(Some("⠹ Working 2s"), None, 18, 80),
            [apart("⠹ Working 2s", all, 80)]
        );
        // Narrower, the activity and the note keep their room, and the view's place is cut short,
        // then left out.
        assert_eq!(
            rows(Some(working), Some("Copied"), 18, 60),
            [apart("⠹ Working 2s (Esc to interrupt) · Copied", "↓18", 60)]
        );
        assert_eq!(
            rows(Some(working), Some("Copied"), 18, 40),
            [apart(working, "↓18", 40).as_str(), "Copied"]
        );
        assert_eq!(rows(Some(working), None, 18, 34), [working]);
        assert_eq!(rows(Some(working), None, 0, 20), ["⠹ Working 2s (Esc …"]);
        // A note that does not fit after the activity goes under it, wrapped.
        let long = "A bash command is already running. Press Esc to cancel it first.";
        assert_eq!(
            rows(Some(working), Some(long), 18, 60),
            [
                apart(working, "↓18", 60).as_str(),
                "A bash command is already running. Press Esc to cancel it",
                "first."
            ]
        );
        assert_eq!(
            rows(None, Some(long), 0, 60),
            [
                "A bash command is already running. Press Esc to cancel it",
                "first."
            ]
        );
    }
}
