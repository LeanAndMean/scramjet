//! The full-screen interface (`--fullscreen`): the conversation in the alternate screen, scrolled
//! by the application, with the status line, the prompt editor and lists docked under it; the
//! editor keeps taking keys while a turn runs, and what is sent then is queued for the model, as
//! Scramjet's steering and follow-up messages. The mouse wheel scrolls, dragging or a double or
//! triple click selects text to copy, and a right click copies it or pastes, as in Scramjet's
//! retained view. A thread owns the terminal; the session runs on the caller's thread and talks
//! to it through a `Handle`.

use super::editor::{
    Action, ByteSource, Draft, ESCAPE_WAIT, Editor, Key, Mouse, RESIZE_CHECK, RawMode, read_key,
    terminal_size,
};
use super::record::{self, Event};
use super::select::Picker;
use super::settings::Delivery;
use super::terminal::{Terminal, elapsed};
use super::text::{Wrap, clip, one_line, visible};
use super::{clipboard, editor, job, selection, style, view};
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
        Arc, Mutex, PoisonError,
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
/// The lines a notch of the mouse wheel scrolls, Scramjet's default `wheel step`.
const WHEEL: isize = 3;
/// How often a selection dragged to the top or bottom edge scrolls on, as in Scramjet.
const EDGE: Duration = Duration::from_millis(80);
/// The longest time from a click's press to the next one's that counts them as a double or
/// triple click, as in Scramjet.
const CLICKS: Duration = Duration::from_millis(500);
const LEFT: u16 = 0;
const RIGHT: u16 = 2;
const DRAG: u16 = 32;
const WHEEL_UP: u16 = 64;
const WHEEL_DOWN: u16 = 65;
const ALLOW: &str = "Allow? [y/N, a = allow all until exit]";
const BUSY: &str = "Busy: the session is not ready for a prompt yet";
const WAITS: &str = "Commands wait for the turn to end: Esc interrupts it";

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

/// Messages sent while a turn runs, until the agent takes them.
#[derive(Default)]
struct Queue {
    steering: Vec<String>,
    follow_ups: Vec<String>,
}

impl Queue {
    fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_ups.is_empty()
    }
    /// All the messages, in the order they are sent, leaving none.
    fn take(&mut self) -> Vec<String> {
        let mut all = std::mem::take(&mut self.steering);
        all.append(&mut self.follow_ups);
        all
    }
}

/// The messages `delivery` gives the agent at once from `queued`.
fn deliver(queued: &mut Vec<String>, delivery: Delivery) -> Vec<String> {
    match delivery {
        Delivery::All => std::mem::take(queued),
        Delivery::OneAtATime if queued.is_empty() => Vec::new(),
        Delivery::OneAtATime => vec![queued.remove(0)],
    }
}

/// Sends to the screen; also the agent's interface during a turn.
pub(super) struct Link {
    to: mpsc::Sender<ToScreen>,
    wake: PipeWriter,
    queue: Arc<Mutex<Queue>>,
    /// `steeringMode` and `followUpMode`.
    modes: (Delivery, Delivery),
}

impl Link {
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
    /// `cancel` while a turn runs; Ctrl+T flips `hide_thinking`. The agent takes the messages
    /// queued during a turn as `modes` (`steeringMode`, `followUpMode`) say.
    pub(super) fn start(
        editor: Editor,
        hide_thinking: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        modes: (Delivery, Delivery),
    ) -> io::Result<Self> {
        let (to, messages) = mpsc::channel();
        let (inputs, from) = mpsc::channel();
        let (woken, wake) = io::pipe()?;
        rustix::fs::fcntl_setfl(&wake, rustix::fs::OFlags::NONBLOCK)?;
        rustix::fs::fcntl_setfl(&woken, rustix::fs::OFlags::NONBLOCK)?;
        let raw = RawMode::enter()?;
        let queue = Arc::new(Mutex::new(Queue::default()));
        let thread = std::thread::Builder::new().name("screen".into()).spawn({
            let queue = queue.clone();
            move || {
                let app = App::new(editor, hide_thinking, cancel, inputs, queue);
                app.run(raw, &messages, woken)
            }
        })?;
        Ok(Self {
            link: Link {
                to,
                wake,
                queue,
                modes,
            },
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
    fn steering(&mut self) -> Vec<String> {
        deliver(&mut self.queue().steering, self.modes.0)
    }
    fn follow_up(&mut self) -> Vec<String> {
        deliver(&mut self.queue().follow_ups, self.modes.1)
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

/// A point of the conversation between two cells: before column `column` of the line at `place`.
#[derive(Clone, Copy, PartialEq)]
struct Mark {
    place: Place,
    column: usize,
}

/// Text selected with the mouse, from where the left button was pressed to where it went.
struct Selection {
    anchor: Mark,
    head: Mark,
    /// The button is held, and moved since it was pressed.
    held: bool,
    dragged: bool,
    /// The view followed the end when the selection started, and goes back to it when the
    /// selection ends, unless the user scrolled meanwhile.
    follow: bool,
}

/// Where the pointer is while a selection is dragged at the top (-1) or bottom (1) edge of the
/// conversation, which then scrolls on.
struct Edge {
    direction: isize,
    column: usize,
    row: usize,
    scrolled: Instant,
}

/// A copy under way: the terminal's sequence that copies, if any, or the failure.
type Copying = mpsc::Receiver<Result<Option<String>, String>>;

/// The clipboard read for a right click, pasted only into the editor it was for, unchanged.
struct Pasting {
    /// A text dialog's editor, rather than the prompt's.
    dialog: bool,
    text: String,
    read: mpsc::Receiver<Result<String, String>>,
}

/// The last press of the left button, which the next one may repeat as a double or triple click.
struct Click {
    count: u8,
    at: Instant,
    /// The screen's column and row.
    cell: (usize, usize),
    mark: Mark,
    /// The line pressed, as shown.
    line: String,
    /// Released where it was pressed.
    released: bool,
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
    queue: Arc<Mutex<Queue>>,
    /// The session waits for input.
    ready: bool,
    /// A turn runs, with what it does now.
    activity: Option<Activity>,
    dialog: Option<Open>,
    /// The first line shown, while the user has scrolled away from the end.
    top: Option<Place>,
    /// Tool results are shown whole (Ctrl+O).
    whole: bool,
    selection: Option<Selection>,
    edge: Option<Edge>,
    copying: Option<Copying>,
    pasting: Option<Pasting>,
    click: Option<Click>,
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
        queue: Arc<Mutex<Queue>>,
    ) -> Self {
        Self {
            draft: editor.draft(String::new()),
            editor,
            transcript: Transcript::default(),
            hide_thinking,
            cancel,
            inputs,
            queue,
            ready: false,
            activity: None,
            dialog: None,
            top: None,
            whole: false,
            selection: None,
            edge: None,
            copying: None,
            pasting: None,
            click: None,
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
            self.copied()?;
            self.pasted();
            if self
                .edge
                .as_ref()
                .is_some_and(|edge| edge.scrolled.elapsed() >= EDGE)
            {
                self.edge_scroll();
            }
            let since = self.drawn_at.elapsed();
            let mut wait = match self.activity.is_some() || self.draft.searching() {
                true => TICK,
                false => RESIZE_CHECK,
            };
            if let Some(edge) = &self.edge {
                wait = wait.min(EDGE.saturating_sub(edge.scrolled.elapsed()));
            }
            if self.copying.is_some() || self.pasting.is_some() {
                wait = wait.min(FRAME);
            }
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
            let flow = self.key(key, source.pending());
            if !matches!(flow, Flow::Go) {
                // The button's release goes to whatever has the terminal meanwhile.
                self.edge = None;
                if let Some(selection) = &mut self.selection {
                    selection.held = false;
                }
            }
            match flow {
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
                (self.selection, self.edge) = (None, None);
            }
            ToScreen::Turn(running) => {
                self.transcript.split();
                self.activity = running.then(|| Activity::new("working"));
                self.draft.clear_note(WAITS);
                // What the turn did not take is not sent unasked.
                if !running {
                    self.restore();
                }
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

    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Puts the queued messages back in the editor, before its text, as Scramjet does.
    fn restore(&mut self) {
        let queued = self.queue().take();
        if queued.is_empty() {
            return;
        }
        let count = queued.len();
        let text = (queued.into_iter())
            .chain(Some(self.draft.text().to_owned()))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        self.draft = self.editor.draft(text);
        self.draft.set_note(restored(count));
    }

    /// Queues the draft's text while a turn runs: for the model's next request, or, as a
    /// follow-up, for when the turn would end. A command waits in the editor.
    fn enqueue(&mut self, follow_up: bool) {
        let text = self.draft.text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        if self.editor.is_command(&text) {
            self.draft.set_note(WAITS.to_owned());
            return;
        }
        self.editor.remember(&text);
        let mut queue = self.queue();
        match follow_up {
            true => queue.follow_ups.push(text),
            false => queue.steering.push(text),
        }
        drop(queue);
        self.draft = self.editor.draft(String::new());
        self.top = None;
    }

    /// The first line of the conversation shown: where the user scrolled to, else the end's. A
    /// selection keeps the view where it is, also at the end.
    fn shown_top(&self) -> usize {
        match self.top.map(|top| self.transcript.line(top)) {
            Some(top) if top < self.last || self.selection.is_some() => top.min(self.last),
            _ => self.last,
        }
    }

    /// Scrolls by `lines`, up when negative; returns whether the view moved.
    fn scroll(&mut self, lines: isize) -> bool {
        let top = self.shown_top();
        let next = top.saturating_add_signed(lines).min(self.last);
        if next != top {
            self.top = Some(self.transcript.locate(next));
        }
        next != top
    }

    /// Scrolls as the user asks, which leaves the view there when a selection ends.
    fn navigate(&mut self, lines: isize) {
        if let Some(selection) = &mut self.selection {
            selection.follow = false;
        }
        self.scroll(lines);
    }

    /// The point of the conversation at the screen's `column` and `row`, kept to the lines shown.
    fn mark(&self, column: usize, row: usize) -> Mark {
        let line = self.shown_top() + row.min(self.rows.saturating_sub(1));
        let line = line.min(self.transcript.len().saturating_sub(1));
        let text = self.transcript.window(line, 1).pop().unwrap_or_default();
        Mark {
            place: self.transcript.locate(line),
            column: selection::snap(&text, column),
        }
    }

    /// What is selected, by line and column, from its start to before its end.
    fn selected(&self) -> Option<((usize, usize), (usize, usize))> {
        let selection = self.selection.as_ref()?;
        let point = |mark: Mark| (self.transcript.line(mark.place), mark.column);
        let (anchor, head) = (point(selection.anchor), point(selection.head));
        (anchor != head).then(|| (anchor.min(head), anchor.max(head)))
    }

    fn clear_selection(&mut self) {
        self.edge = None;
        if self
            .selection
            .take()
            .is_some_and(|selection| selection.follow)
        {
            self.top = None;
        }
    }

    /// As Scramjet's retained view: the wheel scrolls; the left button selects from where it is
    /// pressed in the conversation to where it is dragged, a drag to an edge scrolling on, and
    /// pressed twice or three times selects a word or the line; a right click copies what is
    /// selected, else pastes.
    fn mouse(&mut self, mouse: Mouse) {
        let Mouse {
            button,
            column,
            row,
            release,
        } = mouse;
        if release {
            if let Some(click) = &mut self.click
                && button == LEFT
                && click.cell == (column, row)
            {
                click.released = true;
            }
            self.edge = None;
            if let Some(selection) = &mut self.selection {
                selection.held = false;
                if selection.anchor == selection.head {
                    self.clear_selection();
                }
            }
            return;
        }
        if button != LEFT && button != DRAG {
            self.click = None;
        }
        match button {
            WHEEL_UP | WHEEL_DOWN => {
                self.navigate(if button == WHEEL_UP { -WHEEL } else { WHEEL });
                let head = self.mark(column, row);
                if let Some(selection) = &mut self.selection
                    && selection.held
                    && selection.dragged
                {
                    selection.head = head;
                }
            }
            RIGHT if self.selection.is_some() => self.copy(),
            RIGHT if self.copying.is_none() => self.paste(),
            LEFT => {
                let previous = self.click.take();
                self.clear_selection();
                let top = self.shown_top();
                if row < self.rows && top + row < self.transcript.len() {
                    let follow = self.top.is_none();
                    self.top = Some(self.transcript.locate(top));
                    let mark = self.mark(column, row);
                    let line = self
                        .transcript
                        .window(top + row, 1)
                        .pop()
                        .unwrap_or_default();
                    let count = match previous {
                        Some(click)
                            if click.released
                                && click.at.elapsed() <= CLICKS
                                && click.cell == (column, row)
                                && click.mark == mark
                                && click.line == line =>
                        {
                            click.count % 3 + 1
                        }
                        _ => 1,
                    };
                    let at = |column| Mark { column, ..mark };
                    let (anchor, head) = match count {
                        2 => selection::word(&line, column)
                            .map_or((mark, mark), |(start, end)| (at(start), at(end))),
                        3 => (at(0), at(selection::width(&line))),
                        _ => (mark, mark),
                    };
                    self.selection = Some(Selection {
                        anchor,
                        head,
                        held: true,
                        dragged: false,
                        follow,
                    });
                    self.click = Some(Click {
                        count,
                        at: Instant::now(),
                        cell: (column, row),
                        mark,
                        line,
                        released: false,
                    });
                }
            }
            DRAG => {
                // A press reported again where it was is not yet a drag.
                if self
                    .click
                    .as_ref()
                    .is_some_and(|click| click.cell == (column, row))
                {
                    return;
                }
                self.click = None;
                let head = self.mark(column, row);
                let Some(selection) = self.selection.as_mut().filter(|selection| selection.held)
                else {
                    return;
                };
                selection.dragged = true;
                selection.head = head;
                let direction = match row {
                    0 => -1,
                    row if row + 1 >= self.rows => 1,
                    _ => 0,
                };
                let scrolled = (self.edge.as_ref())
                    .filter(|edge| edge.direction == direction)
                    .map_or_else(Instant::now, |edge| edge.scrolled);
                self.edge = (direction != 0).then_some(Edge {
                    direction,
                    column,
                    row,
                    scrolled,
                });
            }
            _ => {}
        }
    }

    /// Scrolls on while a selection is dragged at an edge, until the view can go no further.
    fn edge_scroll(&mut self) {
        let Some(Edge {
            direction,
            column,
            row,
            ..
        }) = self.edge
        else {
            return;
        };
        if let Some(selection) = &mut self.selection {
            selection.follow = false;
        }
        if !self.scroll(direction) {
            self.edge = None;
            return;
        }
        let head = self.mark(column, row);
        if let Some(selection) = &mut self.selection {
            selection.head = head;
        }
        if let Some(edge) = &mut self.edge {
            edge.scrolled = Instant::now();
        }
    }

    /// Copies the text selected, as shown, on a thread of its own: a clipboard tool may take a
    /// while.
    fn copy(&mut self) {
        let Some((start, end)) = self.selected().filter(|_| self.copying.is_none()) else {
            return;
        };
        let lines = self.transcript.window(start.0, end.0 + 1 - start.0);
        let text = selection::text(&lines, start.1, end.1);
        if text.is_empty() {
            return;
        }
        self.edge = None;
        let (done, outcome) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _ = done.send(clipboard::copy(&text));
            });
        match started {
            Ok(_) => self.copying = Some(outcome),
            Err(error) => self.draft.set_note(format!("Copy failed: {error}")),
        }
    }

    /// Ends a copy that has finished: the selection goes once copied, and stays if not.
    fn copied(&mut self) -> io::Result<()> {
        let outcome = match self.copying.as_ref().map(mpsc::Receiver::try_recv) {
            None | Some(Err(TryRecvError::Empty)) => return Ok(()),
            Some(Ok(outcome)) => outcome,
            Some(Err(TryRecvError::Disconnected)) => Err("the clipboard failed".to_owned()),
        };
        self.copying = None;
        match outcome {
            Ok(sequence) => {
                if let Some(sequence) = sequence {
                    let mut stderr = io::stderr().lock();
                    stderr.write_all(sequence.as_bytes())?;
                    stderr.flush()?;
                }
                self.clear_selection();
            }
            Err(error) => self.draft.set_note(format!("Copy failed: {error}")),
        }
        Ok(())
    }

    /// The editor shown, if any: a text dialog's, or the prompt's when no dialog is open.
    fn editing(&mut self) -> Option<(&Editor, &mut Draft)> {
        match &mut self.dialog {
            None => Some((&self.editor, &mut self.draft)),
            Some(Open {
                shown: Shown::Text(text),
                ..
            }) => Some((&text.editor, &mut text.draft)),
            Some(_) => None,
        }
    }

    /// Reads the clipboard, on a thread of its own, for the editor shown.
    fn paste(&mut self) {
        if self.pasting.is_some() {
            return;
        }
        let dialog = self.dialog.is_some();
        let Some((_, draft)) = self.editing() else {
            return;
        };
        let text = draft.text().to_owned();
        let (done, read) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _ = done.send(clipboard::paste());
            });
        match started {
            Ok(_) => self.pasting = Some(Pasting { dialog, text, read }),
            Err(error) => {
                if let Some((_, draft)) = self.editing() {
                    draft.set_note(format!("Paste failed: {error}"));
                }
            }
        }
    }

    /// Ends a paste whose clipboard was read: as Scramjet's, it goes into the editor only if
    /// that is still shown and unchanged, as if pasted from the terminal.
    fn pasted(&mut self) {
        let read = match self.pasting.as_ref().map(|pasting| pasting.read.try_recv()) {
            None | Some(Err(TryRecvError::Empty)) => return,
            Some(Ok(read)) => read,
            Some(Err(TryRecvError::Disconnected)) => Err("the clipboard failed".to_owned()),
        };
        let pasting = self.pasting.take().expect("checked above");
        if self.exiting || self.dialog.is_some() != pasting.dialog {
            return;
        }
        let Some((editor, draft)) = self
            .editing()
            .filter(|(_, draft)| draft.text() == pasting.text)
        else {
            return;
        };
        match read {
            Ok(text) => {
                editor.key(draft, Key::Paste(editor::clean(&text)), false);
            }
            Err(error) => draft.set_note(format!("Paste failed: {error}")),
        }
    }

    fn key(&mut self, key: Key, pending: bool) -> Flow {
        if self.exiting {
            return Flow::Go;
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
        if self.selection.is_some() {
            match key {
                Key::Interrupt => {
                    self.copy();
                    return Flow::Go;
                }
                Key::Escape => {
                    self.clear_selection();
                    self.top = None;
                    return Flow::Go;
                }
                Key::Idle | Key::PageUp | Key::PageDown => {}
                _ => self.clear_selection(),
            }
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
                let page = self.rows.saturating_sub(1).max(1) as isize;
                self.navigate(if key == Key::PageUp { -page } else { page });
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
                self.restore();
                self.cancel.store(true, Ordering::Relaxed);
                return Flow::Go;
            }
        }
        if key == Key::Interrupt && running {
            self.cancel.store(true, Ordering::Relaxed);
        }
        let key = match key {
            Key::AltUp => {
                let empty = self.queue().is_empty();
                match empty {
                    true => self
                        .draft
                        .set_note("No queued messages to restore".to_owned()),
                    false => self.restore(),
                }
                return Flow::Go;
            }
            Key::AltEnter if running => {
                self.enqueue(true);
                return Flow::Go;
            }
            // As Scramjet's, Alt+Enter sends between turns as Enter does.
            Key::AltEnter => Key::Enter,
            key => key,
        };
        match self.editor.key(&mut self.draft, key, pending) {
            Action::Edited(_) | Action::Conversation => {}
            // The draft holds the text sent.
            Action::Submit(_) if running => self.enqueue(false),
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
            Action::CycleEffort => self.draft.set_note(WAITS.to_owned()),
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
        let below = self.last.saturating_sub(self.shown_top());
        if below > 0 {
            parts.push(format!("{below} more lines below · PgDn or Esc to go back"));
        }
        let line = clip(&parts.join(" · "), width.saturating_sub(1));
        format!("{}{line}{}", style::STATUS, style::RESET)
    }

    /// The rows under the conversation, and where the cursor goes in them.
    fn dock(&self, (width, height): (usize, usize)) -> (Vec<String>, Option<(usize, usize)>) {
        let mut rows = vec![self.status(width)];
        let queue = self.queue();
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
                rows.push(format!("{}{line}{}", style::MUTED, style::RESET));
            }
        }
        if !queue.is_empty() {
            let hint = clip(
                "↳ Alt+Up to edit all queued messages",
                width.saturating_sub(1),
            );
            rows.push(format!("{}{hint}{}", style::MUTED, style::RESET));
        }
        drop(queue);
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
        (self.rows, self.last) = (rows, last);
        let top = self.shown_top();
        if top == last && self.selection.is_none() {
            self.top = None;
        }
        let mut frame = self.transcript.window(top, rows);
        if let Some((start, end)) = self.selected() {
            for (line, row) in (top..).zip(&mut frame) {
                if (start.0..=end.0).contains(&line) {
                    let left = if line == start.0 { start.1 } else { 0 };
                    let right = if line == end.0 { end.1 } else { usize::MAX };
                    *row = selection::highlight(row, left, right);
                }
            }
        }
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

/// What Alt+Up or Escape says once it put `count` queued messages back in the editor.
fn restored(count: usize) -> String {
    match count {
        1 => "Restored 1 queued message to editor".to_owned(),
        count => format!("Restored {count} queued messages to editor"),
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

/// The alternate screen, lines cut at the edge rather than wrapped, and the mouse's buttons,
/// drags and wheel reported (SGR), until dropped.
struct Screen;

impl Screen {
    fn enter() -> Self {
        eprint!("\x1b[?1049h\x1b[?7l\x1b[?1002h\x1b[?1006h\x1b[2J");
        Self
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        eprint!("\x1b[?1002l\x1b[?1006l\x1b[?7h\x1b[?25h\x1b[?1049l");
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
    fn queued_messages_are_delivered_as_the_modes_say() {
        let mut queued = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(deliver(&mut queued, Delivery::OneAtATime), ["one"]);
        assert_eq!(deliver(&mut queued, Delivery::OneAtATime), ["two"]);
        assert!(deliver(&mut queued, Delivery::OneAtATime).is_empty());
        let mut queued = vec!["one".to_owned(), "two".to_owned()];
        assert_eq!(deliver(&mut queued, Delivery::All), ["one", "two"]);
        assert!(queued.is_empty());
        let mut queue = Queue {
            steering: vec!["steer".into()],
            follow_ups: vec!["later".into()],
        };
        assert_eq!(queue.take(), ["steer", "later"]);
        assert!(queue.is_empty());
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
