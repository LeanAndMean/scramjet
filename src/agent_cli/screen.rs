//! The full-screen interface (`--fullscreen`): the conversation in the alternate screen, scrolled
//! by the application, with the status line, the prompt editor and lists docked under it; the
//! editor keeps taking keys while a turn runs. A thread owns the terminal; the session runs on
//! the caller's thread and talks to it through a `Handle`.

use super::editor::{
    Action, ByteSource, Draft, ESCAPE_WAIT, Editor, Key, RESIZE_CHECK, RawMode, read_key,
    terminal_size,
};
use super::record::{self, Event};
use super::select::Picker;
use super::terminal::{Terminal, elapsed};
use super::text::{Wrap, clip, visible};
use super::{editor, job, style, view};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};
use scramjet_rs::agent::{
    self, AgentError, AgentUi, AutoCompaction, Choice, Compaction, Purpose, StreamProgress,
    ToolDetail,
};
use std::{
    fmt::Write as _,
    io::{self, PipeReader, PipeWriter, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// How often the status line's seconds are counted while a turn runs.
const TICK: Duration = Duration::from_millis(200);
/// The shortest time between two frames, so that a fast stream does not redraw for each delta.
const FRAME: Duration = Duration::from_millis(16);
/// The share of the screen's rows the prompt's text takes at most, as Scramjet's default
/// `editorMaxHeightPercent`.
const EDITOR_SHARE: usize = 30;
const ALLOW: &str = "Allow? [y/N, a = allow all until exit]";
const BUSY: &str = "A turn is running: Esc interrupts it";

/// What the session asks the screen to show or do.
enum ToScreen {
    Event(Event),
    /// The session was replaced: the conversation shown starts again.
    Clear,
    /// A turn, `/compact`, a branch summary or a `!` command starts (`true`) or ends.
    Turn(bool),
    /// The session waits for what the user enters next.
    Ready,
    SetText(String),
    Remember(String),
    Note(String),
    Dialog(Dialog, mpsc::Sender<Answer>),
    /// The dialog's question is no longer asked.
    Dismiss,
    Quit,
}

/// A question the screen shows in place of the editor until it is answered.
pub(super) enum Dialog {
    Pick(Picker),
    /// A text typed under the title; Escape cancels.
    Text(String),
    /// An `--ask` approval question.
    Approve(String),
}

pub(super) enum Answer {
    Picked(Option<usize>),
    Text(Option<String>),
    Approved(agent::Answer),
}

/// What the user enters while the session waits for it.
pub(super) enum Input {
    Prompt(String),
    /// Escape twice on an empty prompt.
    DoubleEscape,
    /// Shift+Tab.
    CycleEffort,
    /// Ctrl-C twice or Ctrl-D on an empty prompt, or the screen is gone.
    End,
}

/// Sends to the screen; also the agent's interface during a turn.
pub(super) struct Link {
    to: mpsc::Sender<ToScreen>,
    wake: PipeWriter,
}

impl Link {
    fn send(&self, message: ToScreen) {
        if self.to.send(message).is_ok() {
            // A full pipe already wakes the screen.
            let _ = (&self.wake).write(&[0]);
        }
    }
    pub(super) fn event(&self, event: Event) {
        self.send(ToScreen::Event(event));
    }
    /// Shows a message of the session's own, wrapped on the screen.
    pub(super) fn say(&self, text: &str) {
        self.event(Event::Text(text.to_owned()));
    }
    pub(super) fn clear(&self) {
        self.send(ToScreen::Clear);
    }
    pub(super) fn turn(&self, running: bool) {
        self.send(ToScreen::Turn(running));
    }
    pub(super) fn set_text(&self, text: String) {
        self.send(ToScreen::SetText(text));
    }
    pub(super) fn remember(&self, prompt: &str) {
        self.send(ToScreen::Remember(prompt.to_owned()));
    }
    pub(super) fn note(&self, note: String) {
        self.send(ToScreen::Note(note));
    }
    /// Shows `dialog` and waits for its answer; `None` once the screen is gone or `cancel` set.
    pub(super) fn ask(&self, dialog: Dialog, cancel: Option<&AtomicBool>) -> Option<Answer> {
        let (reply, answer) = mpsc::channel();
        self.send(ToScreen::Dialog(dialog, reply));
        loop {
            match answer.recv_timeout(Duration::from_millis(50)) {
                Ok(answer) => return Some(answer),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                        self.send(ToScreen::Dismiss);
                        return None;
                    }
                }
            }
        }
    }
    /// The row chosen from `picker`, `None` when cancelled.
    pub(super) fn pick(&self, picker: Picker) -> Option<usize> {
        match self.ask(Dialog::Pick(picker), None) {
            Some(Answer::Picked(chosen)) => chosen,
            _ => None,
        }
    }
}

/// The screen's thread, and what the user enters from it.
pub(super) struct Handle {
    pub(super) link: Link,
    from: mpsc::Receiver<Input>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl Handle {
    /// Takes the terminal for the screen, `editor` editing the prompts. Escape and Ctrl-C set
    /// `cancel` while a turn runs; Ctrl+T flips `hide_thinking`.
    pub(super) fn start(
        editor: Editor,
        hide_thinking: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<Self> {
        let (to, messages) = mpsc::channel();
        let (inputs, from) = mpsc::channel();
        let (woken, wake) = io::pipe()?;
        rustix::fs::fcntl_setfl(&wake, rustix::fs::OFlags::NONBLOCK)?;
        rustix::fs::fcntl_setfl(&woken, rustix::fs::OFlags::NONBLOCK)?;
        let raw = RawMode::enter()?;
        let thread = std::thread::Builder::new()
            .name("screen".into())
            .spawn(move || {
                let app = App::new(editor, hide_thinking, cancel, inputs);
                app.run(raw, &messages, woken)
            })?;
        Ok(Self {
            link: Link { to, wake },
            from,
            thread: Some(thread),
        })
    }

    /// Waits for what the user enters next.
    pub(super) fn input(&self) -> Input {
        self.link.send(ToScreen::Ready);
        self.from.recv().unwrap_or(Input::End)
    }
}

impl std::ops::Deref for Handle {
    type Target = Link;
    fn deref(&self) -> &Link {
        &self.link
    }
}

impl Drop for Handle {
    /// Gives the terminal back as it was.
    fn drop(&mut self) {
        self.link.send(ToScreen::Quit);
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(Err(error))) => eprintln!("error: {error}"),
            Some(Err(_)) => eprintln!("error: the screen failed"),
            _ => {}
        }
    }
}

impl AgentUi for Link {
    fn waiting(&mut self, purpose: Purpose) {
        self.event(Event::Waiting(purpose));
    }
    fn progress(&mut self, update: &StreamProgress) {
        self.event(Event::Progress(update.clone()));
    }
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration) {
        self.event(Event::Retrying(error.to_owned(), attempt, retries, delay));
    }
    fn assistant(&mut self, text: &str) {
        self.event(Event::Assistant(text.to_owned()));
    }
    fn tool_call(&mut self, name: &str, summary: &str) {
        self.event(Event::ToolCall(name.to_owned(), summary.to_owned()));
    }
    fn tool_running(&mut self) {
        self.event(Event::ToolRunning);
    }
    fn tool_output(&mut self, output: &str) {
        self.event(Event::ToolOutput(output.to_owned()));
    }
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        let event = Event::ToolResult(name.to_owned(), output.to_owned(), detail.cloned(), failed);
        self.event(event);
    }
    fn command_result(&mut self, output: &str, failed: bool) {
        self.event(Event::CommandResult(output.to_owned(), failed));
    }
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>) {
        self.event(Event::Compacted(compaction.clone(), automatic));
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        self.event(Event::CompactionFailed(error.to_string()));
    }
    fn user_prompt(&mut self, prompt: &str) {
        self.event(Event::UserPrompt(prompt.to_owned()));
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.event(Event::CompactionSummary(summary.to_owned()));
    }
    fn branch_summary(&mut self, summary: &str) {
        self.event(Event::BranchSummary(summary.to_owned()));
    }
    /// As Scramjet's, the question shows until answered; a cancelled one shows no answer.
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> agent::Answer {
        match self.ask(Dialog::Approve(question.to_owned()), Some(cancel)) {
            Some(Answer::Approved(answer)) => {
                self.event(Event::Approval(question.to_owned(), answer));
                answer
            }
            _ => agent::Answer::Deny,
        }
    }
    fn question(&mut self, message: &str) {
        self.event(Event::Question(message.to_owned()));
    }
    fn notice(&mut self, text: &str) {
        self.event(Event::Notice(text.to_owned()));
    }
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        self.pick(Picker::question(title, options, recommended))
    }
}

/// What the screen does after a key.
enum Flow {
    Go,
    Suspend,
    External,
}

/// What the agent does now, as the status line says it.
struct Activity {
    what: String,
    since: Instant,
    /// Shows the seconds left of this wait instead of the elapsed seconds.
    countdown: Option<Duration>,
}

impl Activity {
    fn new(what: impl Into<String>) -> Self {
        Self {
            what: what.into(),
            since: Instant::now(),
            countdown: None,
        }
    }
    fn line(&self) -> String {
        match self.countdown {
            Some(wait) => {
                let left = wait.saturating_sub(self.since.elapsed()).as_secs_f64();
                format!("… {} {}s", self.what, left.ceil())
            }
            None => format!("… {}{}", self.what, elapsed(self.since.elapsed().as_secs())),
        }
    }
}

enum Shown {
    Pick(Picker),
    Text(Box<TextDialog>),
    Approve(String),
}

struct TextDialog {
    title: String,
    editor: Editor,
    draft: Draft,
}

struct Open {
    shown: Shown,
    reply: mpsc::Sender<Answer>,
}

/// How the conversation is rendered: width, reasoning hidden, tool results whole.
type Look = (usize, bool, bool);

/// A line of the conversation by its part, so that it stays in view as parts before it or it
/// render to more or fewer lines.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    part: usize,
    line: usize,
}

/// The conversation shown, in parts that each render on their own: a turn, or what came
/// between turns. Each part is rendered again only when it or the look changes.
#[derive(Default)]
struct Transcript {
    parts: Vec<Part>,
}

#[derive(Default)]
struct Part {
    events: Vec<Event>,
    version: usize,
    lines: Vec<String>,
    rendered: Option<(Look, usize)>,
}

impl Transcript {
    fn push(&mut self, event: Event) {
        if self.parts.is_empty() {
            self.parts.push(Part::default());
        }
        let part = self.parts.last_mut().expect("pushed above");
        part.version += 1;
        // Streamed text is kept whole, and only a running tool's latest output.
        match (part.events.last_mut(), event) {
            (Some(Event::ToolOutput(last)), Event::ToolOutput(output)) => *last = output,
            (
                Some(Event::Progress(StreamProgress::Text(last))),
                Event::Progress(StreamProgress::Text(delta)),
            )
            | (
                Some(Event::Progress(StreamProgress::Reasoning(last))),
                Event::Progress(StreamProgress::Reasoning(delta)),
            ) => last.push_str(&delta),
            (_, event) => part.events.push(event),
        }
    }

    /// Starts a new part, unless the last one is empty.
    fn split(&mut self) {
        if self.parts.last().is_none_or(|part| !part.events.is_empty()) {
            self.parts.push(Part::default());
        }
    }

    fn render(&mut self, look: Look, hide_thinking: &Arc<AtomicBool>) {
        for part in &mut self.parts {
            if part.rendered != Some((look, part.version)) {
                part.lines = render(&part.events, look, hide_thinking.clone());
                part.rendered = Some((look, part.version));
            }
        }
    }

    fn len(&self) -> usize {
        self.parts.iter().map(|part| part.lines.len()).sum()
    }

    /// The part that line `line` is in, and where in it.
    fn locate(&self, mut line: usize) -> Place {
        for (part, lines) in self.parts.iter().map(|part| part.lines.len()).enumerate() {
            if line < lines {
                return Place { part, line };
            }
            line -= lines;
        }
        Place {
            part: self.parts.len(),
            line,
        }
    }

    /// The line at `place`, as near as the part's lines now reach.
    fn line(&self, place: Place) -> usize {
        let before: usize = (self.parts.iter().take(place.part))
            .map(|part| part.lines.len())
            .sum();
        let within = match self.parts.get(place.part) {
            Some(part) => place.line.min(part.lines.len().saturating_sub(1)),
            None => place.line,
        };
        before + within
    }

    /// `count` lines from `top`, as far as there are.
    fn window(&self, top: usize, count: usize) -> Vec<String> {
        let lines = self.parts.iter().flat_map(|part| &part.lines);
        lines.skip(top).take(count).cloned().collect()
    }
}

/// The lines of `events` as the inline output shows them, rendered at the look's width.
fn render(
    events: &[Event],
    (width, _, whole): Look,
    hide_thinking: Arc<AtomicBool>,
) -> Vec<String> {
    let mut ui = Terminal::capture_shown(width, hide_thinking, whole);
    for event in events {
        record::replay(event, &mut ui);
    }
    if let Some(Event::ToolOutput(output)) = events.last() {
        ui.running_output(output);
    }
    let mut lines = view::screen_lines(&ui.captured());
    // Each block ends with a blank line, which separates it from the next part.
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

struct App {
    editor: Editor,
    draft: Draft,
    transcript: Transcript,
    hide_thinking: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    inputs: mpsc::Sender<Input>,
    /// The session waits for input.
    ready: bool,
    /// A turn runs, with what it does now.
    activity: Option<Activity>,
    dialog: Option<Open>,
    /// The first line shown, while the user has scrolled away from the end.
    top: Option<Place>,
    /// Tool results are shown whole (Ctrl+O).
    whole: bool,
    /// The user asked to exit; keys are no longer taken.
    exiting: bool,
    /// The terminal's size, the rows and the cursor drawn last.
    size: (usize, usize),
    drawn: Vec<String>,
    cursor: Option<(usize, usize)>,
    /// The last frame's transcript rows and its length, for scrolling.
    rows: usize,
    last: usize,
    drawn_at: Instant,
}

impl App {
    fn new(
        editor: Editor,
        hide_thinking: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        inputs: mpsc::Sender<Input>,
    ) -> Self {
        Self {
            draft: editor.draft(String::new()),
            editor,
            transcript: Transcript::default(),
            hide_thinking,
            cancel,
            inputs,
            ready: false,
            activity: None,
            dialog: None,
            top: None,
            whole: false,
            exiting: false,
            size: (0, 0),
            drawn: Vec::new(),
            cursor: None,
            rows: 1,
            last: 0,
            drawn_at: Instant::now(),
        }
    }

    fn run(
        mut self,
        raw: RawMode,
        messages: &mpsc::Receiver<ToScreen>,
        woken: PipeReader,
    ) -> io::Result<()> {
        // Dropped before `raw`, which gives the terminal its mode back.
        let mut screen = Screen::enter();
        let mut source = Source::new(woken);
        let mut closed = false;
        loop {
            loop {
                match messages.try_recv() {
                    Ok(ToScreen::Quit) | Err(TryRecvError::Disconnected) => return Ok(()),
                    Ok(message) => self.message(message),
                    Err(TryRecvError::Empty) => break,
                }
            }
            self.editor.poll(&mut self.draft);
            if let Some(Open {
                shown: Shown::Text(text),
                ..
            }) = &mut self.dialog
            {
                text.editor.poll(&mut text.draft);
            }
            let since = self.drawn_at.elapsed();
            let mut wait = match self.activity.is_some() || self.draft.searching() {
                true => TICK,
                false => RESIZE_CHECK,
            };
            if !source.pending() {
                if since >= FRAME {
                    self.draw()?;
                } else {
                    wait = FRAME - since;
                }
            }
            if closed {
                std::thread::sleep(wait);
                continue;
            }
            let key = match read_key(&mut source, Some(wait)) {
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    closed = true;
                    self.exit();
                    continue;
                }
                result => result?,
            };
            match self.key(key, source.pending()) {
                Flow::Go => {}
                Flow::Suspend => {
                    drop(screen);
                    raw.suspend();
                    job::suspend();
                    job::continued();
                    raw.resume()?;
                    screen = Screen::enter();
                    self.drawn.clear();
                }
                Flow::External => {
                    drop(screen);
                    raw.suspend();
                    let edited = editor::external(self.draft.text());
                    raw.resume()?;
                    screen = Screen::enter();
                    self.drawn.clear();
                    self.draft.edited(edited);
                }
            }
        }
    }

    fn message(&mut self, message: ToScreen) {
        match message {
            ToScreen::Event(event) => {
                self.track(&event);
                self.transcript.push(event);
            }
            ToScreen::Clear => {
                self.transcript = Transcript::default();
                self.top = None;
            }
            ToScreen::Turn(running) => {
                self.transcript.split();
                self.activity = running.then(|| Activity::new("working"));
                self.draft.clear_note(BUSY);
            }
            ToScreen::Ready => self.ready = true,
            ToScreen::SetText(text) => self.draft = self.editor.draft(text),
            ToScreen::Remember(prompt) => self.editor.remember(&prompt),
            ToScreen::Note(note) => self.draft.set_note(note),
            ToScreen::Dialog(dialog, reply) => {
                let shown = match dialog {
                    Dialog::Pick(picker) if picker.is_empty() => {
                        let _ = reply.send(Answer::Picked(None));
                        return;
                    }
                    Dialog::Pick(picker) => Shown::Pick(picker),
                    Dialog::Text(title) => {
                        let editor = Editor::dialog(self.hide_thinking.clone());
                        let draft = editor.draft(String::new());
                        Shown::Text(Box::new(TextDialog {
                            title,
                            editor,
                            draft,
                        }))
                    }
                    Dialog::Approve(question) => Shown::Approve(question),
                };
                self.dialog = Some(Open { shown, reply });
                self.top = None;
                if let Some(activity) = &mut self.activity {
                    *activity = Activity::new("waiting for you");
                }
            }
            ToScreen::Dismiss => self.dialog = None,
            ToScreen::Quit => {}
        }
    }

    /// Follows what the agent does for the status line.
    fn track(&mut self, event: &Event) {
        let Some(activity) = &mut self.activity else {
            return;
        };
        let what = match event {
            Event::Waiting(Purpose::Reply) => "thinking".to_owned(),
            Event::Waiting(Purpose::Compaction) => "compacting".to_owned(),
            Event::Waiting(Purpose::BranchSummary) => "summarizing branch".to_owned(),
            Event::Progress(StreamProgress::Call(name)) => format!("preparing {name}"),
            Event::ToolRunning => "running".to_owned(),
            Event::ToolResult(..) | Event::CommandResult(..) => "working".to_owned(),
            Event::Retrying(_, _, _, delay) => {
                *activity = Activity {
                    countdown: Some(*delay),
                    ..Activity::new("retrying in")
                };
                return;
            }
            _ => return,
        };
        if activity.what != what || activity.countdown.is_some() {
            *activity = Activity::new(what);
        }
    }

    fn exit(&mut self) {
        if !self.exiting {
            self.exiting = true;
            self.cancel.store(true, Ordering::Relaxed);
            self.answer(None);
            let _ = self.inputs.send(Input::End);
        }
    }

    /// Answers the open dialog, if any: with `answer`, or as cancelled.
    fn answer(&mut self, answer: Option<Answer>) {
        let Some(open) = self.dialog.take() else {
            return;
        };
        let answer = answer.unwrap_or(match open.shown {
            Shown::Pick(_) => Answer::Picked(None),
            Shown::Text(_) => Answer::Text(None),
            Shown::Approve(_) => Answer::Approved(agent::Answer::Deny),
        });
        let _ = open.reply.send(answer);
        if let Some(activity) = &mut self.activity {
            *activity = Activity::new("working");
        }
    }

    fn scroll(&mut self, up: bool) {
        let page = self.rows.saturating_sub(1).max(1);
        let top = self.top.map_or(self.last, |top| self.transcript.line(top));
        let top = match up {
            true => top.saturating_sub(page),
            false => top + page,
        };
        self.top = Some(self.transcript.locate(top));
    }

    fn key(&mut self, key: Key, pending: bool) -> Flow {
        if self.exiting {
            return Flow::Go;
        }
        match key {
            Key::Idle => return Flow::Go,
            Key::Suspend => return Flow::Suspend,
            Key::Expand => {
                self.whole = !self.whole;
                return Flow::Go;
            }
            Key::PageUp | Key::PageDown
                if !matches!(
                    self.dialog,
                    Some(Open {
                        shown: Shown::Pick(_),
                        ..
                    })
                ) =>
            {
                self.scroll(key == Key::PageUp);
                return Flow::Go;
            }
            _ => {}
        }
        if self.dialog.is_some() {
            return self.dialog_key(key, pending);
        }
        let running = self.activity.is_some();
        if key == Key::Escape && !self.draft.completing() {
            if self.top.is_some() {
                self.top = None;
                return Flow::Go;
            }
            if running {
                self.cancel.store(true, Ordering::Relaxed);
                return Flow::Go;
            }
        }
        if key == Key::Interrupt && running {
            self.cancel.store(true, Ordering::Relaxed);
        }
        match self.editor.key(&mut self.draft, key, pending) {
            Action::Edited(_) | Action::Conversation => {}
            Action::Submit(text) if self.ready => {
                self.editor.remember(&text);
                self.ready = false;
                self.top = None;
                let _ = self.inputs.send(Input::Prompt(text));
                self.draft = self.editor.draft(String::new());
            }
            Action::Submit(_) => self.draft.set_note(BUSY.to_owned()),
            Action::Exit => self.exit(),
            Action::DoubleEscape if self.ready => {
                let _ = self.inputs.send(Input::DoubleEscape);
                self.ready = false;
            }
            Action::DoubleEscape => {}
            Action::CycleEffort if self.ready => {
                let _ = self.inputs.send(Input::CycleEffort);
            }
            Action::CycleEffort => self.draft.set_note(BUSY.to_owned()),
            Action::Suspend => return Flow::Suspend,
            Action::External => return Flow::External,
        }
        Flow::Go
    }

    fn dialog_key(&mut self, key: Key, pending: bool) -> Flow {
        let Some(open) = &mut self.dialog else {
            return Flow::Go;
        };
        let answer = match &mut open.shown {
            Shown::Pick(_) if key == Key::ToggleThinking => {
                self.hide_thinking.fetch_xor(true, Ordering::Relaxed);
                None
            }
            Shown::Pick(picker) => picker.key(&key).map(Answer::Picked),
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
                Key::Enter | Key::Escape | Key::Interrupt => {
                    Some(Answer::Approved(agent::Answer::Deny))
                }
                _ => None,
            },
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

    /// The status line: what a running turn does, and how far the view is from the end.
    fn status(&self, width: usize) -> String {
        let mut parts = Vec::new();
        if let Some(activity) = &self.activity {
            parts.push(activity.line());
        }
        if let Some(top) = self.top {
            let below = self.last.saturating_sub(self.transcript.line(top));
            parts.push(format!("{below} more lines below · PgDn or Esc to go back"));
        }
        let line = clip(&parts.join(" · "), width.saturating_sub(1));
        format!("{}{line}{}", style::STATUS, style::RESET)
    }

    /// The rows under the conversation, and where the cursor goes in them.
    fn dock(&self, (width, height): (usize, usize)) -> (Vec<String>, Option<(usize, usize)>) {
        let mut rows = vec![self.status(width)];
        let room = height.saturating_sub(rows.len() + 1).max(1);
        let cursor = match &self.dialog {
            Some(Open {
                shown: Shown::Pick(picker),
                ..
            }) => {
                rows.extend(picker.lines((width, room)));
                None
            }
            Some(Open {
                shown: Shown::Approve(question),
                ..
            }) => {
                let mut wrap = Wrap::new(width.saturating_sub(1).max(10));
                let text = wrap.push(&visible(question)) + &wrap.finish();
                rows.extend(text.split('\n').map(str::to_owned));
                rows.push(format!("{ALLOW} "));
                Some((rows.len() - 1, ALLOW.len() + 1))
            }
            Some(Open {
                shown: Shown::Text(text),
                ..
            }) => {
                rows.push(format!(
                    "{}{}{}  {}Enter submit · Esc cancel{}",
                    style::SELECTOR_TITLE,
                    text.title,
                    style::RESET,
                    style::MUTED,
                    style::RESET
                ));
                prompt_rows(&text.draft, &mut rows, width, room)
            }
            None => {
                rows.push(format!(
                    "{}{}{}",
                    style::MUTED,
                    "─".repeat(width.saturating_sub(1)),
                    style::RESET
                ));
                prompt_rows(&self.draft, &mut rows, width, room)
            }
        };
        (rows, cursor)
    }

    fn draw(&mut self) -> io::Result<()> {
        let (width, height) = terminal_size();
        let mut out = String::from("\x1b[?2026h\x1b[?25l");
        // Some terminals move what they show on a resize.
        if (width, height) != self.size {
            self.size = (width, height);
            self.drawn.clear();
            out.push_str("\x1b[2J");
        }
        let (dock, cursor) = self.dock((width, height));
        let rows = height.saturating_sub(dock.len()).max(1);
        let look = (
            width,
            self.hide_thinking.load(Ordering::Relaxed),
            self.whole,
        );
        self.transcript.render(look, &self.hide_thinking);
        let last = self.transcript.len().saturating_sub(rows);
        let top = match self.top.map(|top| self.transcript.line(top)) {
            Some(top) if top < last => top,
            _ => {
                self.top = None;
                last
            }
        };
        (self.rows, self.last) = (rows, last);
        let mut frame = self.transcript.window(top, rows);
        frame.resize(rows, String::new());
        frame.extend(dock);
        frame.truncate(height);
        let cursor = cursor.map(|(row, column)| (rows + row, column));
        if frame == self.drawn && cursor == self.cursor {
            return Ok(());
        }
        for (index, row) in frame.iter().enumerate() {
            if self.drawn.get(index) != Some(row) {
                write!(out, "\x1b[{};1H{row}{}\x1b[K", index + 1, style::RESET).unwrap();
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

/// Adds the prompt's rows to `rows`, at most a share of `room` around the cursor, then its hint
/// and completion list; returns where the cursor is.
fn prompt_rows(
    draft: &Draft,
    rows: &mut Vec<String>,
    width: usize,
    room: usize,
) -> Option<(usize, usize)> {
    let (text, row, column) = draft.rows(width);
    let shown = (room * EDITOR_SHARE / 100).max(1).min(text.len());
    let top = (row + 1).saturating_sub(shown).min(text.len() - shown);
    let start = rows.len();
    rows.extend(text[top..top + shown].iter().cloned());
    if let Some(hint) = draft.hint() {
        let hint = clip(hint, width.saturating_sub(1));
        rows.push(format!("{}{hint}{}", style::MUTED, style::RESET));
    }
    rows.extend(draft.completion_rows(width));
    Some((start + row - top, column))
}

/// The alternate screen, lines cut at the edge rather than wrapped, until dropped.
struct Screen;

impl Screen {
    fn enter() -> Self {
        eprint!("\x1b[?1049h\x1b[?7l\x1b[2J");
        Self
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        eprint!("\x1b[?7h\x1b[?25h\x1b[?1049l");
        let _ = io::stderr().flush();
    }
}

/// The terminal's input, and the pipe the session writes to so that the screen looks at what it
/// sent.
struct Source {
    buffer: Vec<u8>,
    start: usize,
    woken: PipeReader,
}

impl Source {
    fn new(woken: PipeReader) -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            woken,
        }
    }
    fn pending(&self) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(part: &Part) -> Vec<&str> {
        part.events
            .iter()
            .filter_map(|event| match event {
                Event::Progress(StreamProgress::Text(text)) | Event::ToolOutput(text) => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streamed_text_and_running_output_are_kept_whole() {
        let mut transcript = Transcript::default();
        transcript.push(Event::Progress(StreamProgress::Text("Hel".into())));
        transcript.push(Event::Progress(StreamProgress::Text("lo".into())));
        transcript.push(Event::ToolRunning);
        transcript.push(Event::ToolOutput("1".into()));
        transcript.push(Event::ToolOutput("1\n2".into()));
        assert_eq!(text(&transcript.parts[0]), ["Hello", "1\n2"]);
        transcript.split();
        transcript.split();
        assert_eq!(transcript.parts.len(), 2);
    }

    #[test]
    fn parts_render_again_only_when_they_or_the_look_change() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::UserPrompt("count".into()));
        transcript.push(Event::ToolCall("bash".into(), "seq 1 9".into()));
        let output: Vec<_> = (1..=9).map(|n| n.to_string()).collect();
        transcript.push(Event::ToolResult(
            "bash".into(),
            output.join("\n"),
            None,
            false,
        ));
        transcript.split();
        transcript.push(Event::Text("✓ done".into()));
        transcript.render((40, false, false), &hide);
        let cut = transcript.window(0, 100);
        assert!(
            cut.iter()
                .any(|line| line.contains("earlier lines, Ctrl+O to expand")),
            "{cut:?}"
        );
        assert_eq!(cut[cut.len() - 2..], ["✓ done", ""]);
        transcript.parts[0].lines.clear();
        transcript.render((40, false, false), &hide);
        assert!(transcript.parts[0].lines.is_empty());
        transcript.render((40, false, true), &hide);
        let whole = transcript.window(0, 100);
        assert!(whole.len() > cut.len() && !whole.iter().any(|line| line.contains("earlier")));
        assert_eq!(transcript.len(), whole.len());
        assert_eq!(transcript.window(2, 1), whole[2..3]);
        // A line of the part after the result stays in view as the result grows.
        let done = Place { part: 1, line: 0 };
        assert_eq!(transcript.locate(whole.len() - 2), done);
        assert_eq!(transcript.line(done), whole.len() - 2);
        transcript.render((40, false, false), &hide);
        assert_eq!(transcript.line(done), cut.len() - 2);
        assert_eq!(transcript.window(transcript.line(done), 1), ["✓ done"]);
    }
}
