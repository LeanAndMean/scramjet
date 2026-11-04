//! The full-screen interface, the default on a terminal: the conversation in the alternate screen, scrolled
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
use super::footer::{self, Footer};
use super::interactive::effort_note;
use super::record::{self, Event};
use super::select::{Act, Picked, Picker, Routed, Spot, Toggle};
use super::selection::{self, Line};
use super::settings::{Delivery, DoubleEscape, MarkdownReads, Settings};
use super::slash::Builtin;
use super::terminal::{COMMAND_LINES, Terminal, elapsed, waiting_for};
use super::text::{Wrap, clip, one_line, plain, visible};
use super::{clipboard, editor, job, style};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};
use scramjet_rs::agent::{
    self, AgentError, AgentUi, AutoCompaction, Children, Choice, CompactRead, Compaction,
    EffortCycle, Purpose, Shell, StreamProgress, Tokens, ToolDetail, UserCommand,
};
use std::{
    fmt::Write as _,
    io::{self, PipeReader, PipeWriter, Read, Write},
    path::{Path, PathBuf},
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
const SHELL_BUSY: &str = "A bash command is already running. Press Esc to cancel it first.";
const RELOAD_BUSY: &str = "Wait for the current response to finish before reloading.";
const MODEL_NEXT: &str = "The model changes before the next request";

/// What the session asks the screen to show or do.
enum ToScreen {
    Event(Event),
    /// An event of call `.0` of the `Event::Calls` shown last.
    Call(usize, Event),
    /// The session was replaced: the conversation shown starts again.
    Clear,
    /// A turn, `/compact`, a branch summary or a `!` command starts, Shift+Tab cycling the
    /// effort from where it is, or ends (`None`).
    Turn(Option<EffortCycle>),
    /// The session waits for what the user enters next.
    Ready,
    SetText(String),
    Remember(String),
    Note(String),
    /// `/reload` read the commands and skills (the `/` list's rows after the built-in ones),
    /// whether Escape twice on an empty prompt does something, and how Markdown files read show.
    Reloaded(Vec<(String, String)>, bool, MarkdownReads),
    /// The endpoint's models, which complete `/model`'s argument.
    Models(Vec<String>),
    /// What the footer shows of the session now.
    Footer(footer::Shown),
    /// The agent switched the model during the turn: the footer, and the levels Shift+Tab
    /// cycles through from then on.
    Switched(footer::Shown, EffortCycle),
    /// The session's tokens, how full the context is and the model's output rate, after a
    /// response or a compaction.
    Usage(Tokens, Option<u64>, Option<f64>),
    Dialog(Dialog, mpsc::Sender<Answer>),
    /// The dialog's question is no longer asked.
    Dismiss,
    /// A sequence for the terminal itself: OSC 52's copy.
    Terminal(String),
    /// Opens the file in `$VISUAL` or `$EDITOR` with the terminal as the shell left it, and
    /// replies once the editor exited.
    Edit(PathBuf, mpsc::Sender<Result<(), String>>),
    Quit,
}

/// A question the screen shows in place of the editor until it is answered.
pub(super) enum Dialog {
    Pick(Box<Picker>),
    /// A text typed under the title, starting as the second; Escape cancels. Enter on an empty
    /// text submits it when it is optional, and else does nothing.
    Text(String, String, Optional),
    /// An `--ask` approval question.
    Approve(String),
}

/// Whether a dialog's text may be left empty.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Optional {
    Yes,
    No,
}

pub(super) enum Answer {
    Picked(Option<usize>),
    /// The row chosen from a next step's list, with the model and effort chosen for it.
    Routed(usize, Routed),
    /// What one of a picker's own keys asked for (`Picker::acting`).
    Acted(Act, usize, Spot),
    /// A key of the session list that shows it another way (`Picker::acting`).
    Toggled(Toggle, Spot),
    /// Tab in a picker that takes it (`Picker::tabbing`).
    Tabbed(Spot),
    /// Shift+L in `/tree`'s list on an entry, with the list as it was, to show again once the
    /// label is given; the screen puts the list in.
    Label(usize, Option<Box<Picker>>),
    /// The row a picker's countdown chose.
    Expired(usize),
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
    /// Ctrl+P presses, or Shift+Ctrl+P (negative), each a model further (`scope::cycle`).
    CycleModel(isize),
    /// A `!` command run during a turn ended: `Link::take_shells` has it.
    Shells,
    /// A built-in command typed during a turn waits: `Link::take_commands` has it.
    Commands,
    /// Ctrl-C twice or Ctrl-D on an empty prompt, or the screen is gone.
    End,
}

/// What the user gives while a turn runs, until the agent or the session takes it.
#[derive(Default)]
struct Queue {
    steering: Vec<String>,
    follow_ups: Vec<String>,
    /// The effort level Shift+Tab chose, for the next request.
    effort: Option<String>,
    /// The models Ctrl+P (positive) or Shift+Ctrl+P moved on by, for the next request.
    model_steps: isize,
    /// `!` commands run during a turn, as typed, until the session adds them to the
    /// conversation; at most the last one still runs.
    shells: Vec<Side>,
    /// Built-in commands typed during a turn, until the session runs them.
    commands: Vec<String>,
}

/// A `!` command run apart from the session.
struct Side {
    command: String,
    /// Run with `!`, rather than `!!`.
    include: bool,
    /// The end of its output so far.
    output: String,
    ended: Option<Shell>,
    cancel: Arc<AtomicBool>,
}

impl Queue {
    /// Whether no message is queued.
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
    /// The call of those running at the same time whose events are sent now.
    call: Option<usize>,
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
        self.send(match self.call {
            Some(index) => ToScreen::Call(index, event),
            None => ToScreen::Event(event),
        });
    }
    /// Shows a message of the session's own, wrapped on the screen.
    pub(super) fn say(&self, text: &str) {
        self.event(Event::Text(text.to_owned()));
    }
    pub(super) fn clear(&self) {
        self.send(ToScreen::Clear);
    }
    pub(super) fn turn(&self, efforts: Option<EffortCycle>) {
        self.send(ToScreen::Turn(efforts));
    }
    /// The `!` commands run during a turn that have ended, in the order typed.
    pub(super) fn take_shells(&self) -> Vec<Shell> {
        let mut queue = self.queue();
        let ended = (queue.shells.iter())
            .take_while(|side| side.ended.is_some())
            .count();
        let sides = queue.shells.drain(..ended);
        sides.filter_map(|side| side.ended).collect()
    }
    /// The built-in commands typed during a turn, in order; `paused`: those before the first
    /// that stops the turn, which goes on.
    pub(super) fn take_commands(&self, paused: bool) -> Vec<String> {
        let mut queue = self.queue();
        let count = match paused {
            true => (queue.commands.iter())
                .take_while(|command| !stops_turn(command))
                .count(),
            false => queue.commands.len(),
        };
        queue.commands.drain(..count).collect()
    }
    /// The effort level Shift+Tab chose during a turn that no request took.
    pub(super) fn take_effort(&self) -> Option<String> {
        self.queue().effort.take()
    }
    /// The models Ctrl+P and Shift+Ctrl+P moved on by during a turn, which no request took.
    pub(super) fn take_model_steps(&self) -> isize {
        std::mem::take(&mut self.queue().model_steps)
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
    /// What `/reload` read: the commands and skills the `/` list shows, and the `settings`.
    pub(super) fn reloaded(&mut self, commands: Vec<(String, String)>, settings: &Settings) {
        self.modes = (settings.steering_mode, settings.follow_up_mode);
        let double_escape = settings.double_escape != DoubleEscape::Nothing;
        self.send(ToScreen::Reloaded(
            commands,
            double_escape,
            settings.markdown_reads,
        ));
    }
    pub(super) fn models(&self, models: Vec<String>) {
        self.send(ToScreen::Models(models));
    }
    pub(super) fn footer(&self, shown: footer::Shown) {
        self.send(ToScreen::Footer(shown));
    }
    /// Writes `sequence` to the terminal between frames.
    pub(super) fn to_terminal(&self, sequence: String) {
        self.send(ToScreen::Terminal(sequence));
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
    /// Opens `file` in the user's editor, the screen handing it the terminal until it exits.
    pub(super) fn edit(&self, file: &Path) -> Result<(), String> {
        let (reply, edited) = mpsc::channel();
        self.send(ToScreen::Edit(file.to_path_buf(), reply));
        edited
            .recv()
            .unwrap_or_else(|_| Err("the screen is gone".to_owned()))
    }
    /// Takes away a held list (`Picker::held`) the session no longer shows.
    pub(super) fn dismiss(&self) {
        self.send(ToScreen::Dismiss);
    }
    /// The row chosen from `picker`, `None` when cancelled.
    pub(super) fn pick(&self, picker: Picker) -> Option<usize> {
        self.pick_timed(picker).map(|(row, ..)| row)
    }

    /// As `pick`, or what one of the list's own keys asked for (`Picker::acting`).
    pub(super) fn pick_or_act(&self, picker: Picker) -> Picked {
        match self.ask(Dialog::Pick(Box::new(picker)), None) {
            Some(Answer::Acted(act, row, place)) => Picked::Act(act, row, place),
            Some(Answer::Toggled(toggle, place)) => Picked::Toggle(toggle, place),
            Some(Answer::Tabbed(place)) => Picked::Tab(place),
            Some(Answer::Picked(row)) => Picked::Row(row),
            _ => Picked::Row(None),
        }
    }

    /// As `pick`, or, when Shift+L asks to label an entry of `/tree`'s list, that entry with the
    /// list as it was, to show again.
    pub(super) fn pick_or_label(
        &self,
        picker: Picker,
    ) -> Result<Option<usize>, (usize, Box<Picker>)> {
        match self.ask(Dialog::Pick(Box::new(picker)), None) {
            Some(Answer::Picked(row)) => Ok(row),
            Some(Answer::Label(entry, Some(picker))) => Err((entry, picker)),
            _ => Ok(None),
        }
    }

    /// As `pick`, whether the picker's countdown chose the row rather than the user, and the
    /// model and effort chosen in a next step's list (`Picker::routing`).
    pub(super) fn pick_timed(&self, picker: Picker) -> Option<(usize, bool, Routed)> {
        match self.ask(Dialog::Pick(Box::new(picker)), None) {
            Some(Answer::Picked(chosen)) => chosen.map(|row| (row, false, Routed::default())),
            Some(Answer::Routed(row, routed)) => Some((row, false, routed)),
            Some(Answer::Expired(row)) => Some((row, true, Routed::default())),
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
    /// queued during a turn as `modes` (`steeringMode`, `followUpMode`) say; `!` commands run
    /// meanwhile run in `root`. Markdown files read show as `markdown_reads` says.
    pub(super) fn start(
        editor: Editor,
        hide_thinking: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        modes: (Delivery, Delivery),
        markdown_reads: MarkdownReads,
        root: PathBuf,
    ) -> io::Result<Self> {
        let (to, messages) = mpsc::channel();
        let (inputs, from) = mpsc::channel();
        let (woken, wake) = io::pipe()?;
        rustix::fs::fcntl_setfl(&wake, rustix::fs::OFlags::NONBLOCK)?;
        rustix::fs::fcntl_setfl(&woken, rustix::fs::OFlags::NONBLOCK)?;
        let raw = RawMode::enter()?;
        let queue = Arc::new(Mutex::new(Queue::default()));
        let woke = wake.try_clone()?;
        let thread = std::thread::Builder::new().name("screen".into()).spawn({
            let queue = queue.clone();
            move || {
                let sides = Sides {
                    root,
                    wake: woke,
                    threads: Vec::new(),
                };
                let app = App::new(editor, hide_thinking, cancel, inputs, queue, sides);
                let app = App {
                    markdown_reads,
                    ..app
                };
                app.run(raw, &messages, woken)
            }
        })?;
        Ok(Self {
            link: Link {
                to,
                wake,
                queue,
                modes,
                call: None,
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
    fn tool_call(&mut self, name: &str, summary: &str, compact: Option<&CompactRead>) {
        let (name, summary) = (name.to_owned(), summary.to_owned());
        self.event(Event::ToolCall(name, summary, compact.cloned()));
    }
    fn tool_running(&mut self, since: Instant) {
        self.event(Event::ToolRunning(since));
    }
    fn tool_output(&mut self, output: &str) {
        self.event(Event::ToolOutput(output.to_owned()));
    }
    fn subagents(&mut self, shown: &Children) {
        self.event(Event::Subagents(shown.clone()));
    }
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        let (name, output, detail) = (name.to_owned(), output.to_owned(), detail.cloned());
        self.event(Event::ToolResult(
            name,
            output,
            detail,
            failed,
            Instant::now(),
        ));
    }
    /// As Scramjet's, each call's rows show in its place while it runs.
    fn running_calls(&mut self, count: usize) -> bool {
        self.event(Event::Calls((0..count).map(|_| Vec::new()).collect()));
        true
    }
    fn of_call(&mut self, index: Option<usize>) {
        self.call = index;
    }
    fn command_started(&mut self, command: &str, include: bool) {
        self.event(Event::CommandStarted(command.to_owned(), include));
    }
    fn command_result(&mut self, ran: &UserCommand) {
        self.event(Event::CommandResult(ran.clone()));
    }
    fn stopped(&mut self, error: Option<&str>) {
        self.event(Event::Stopped(error.map(str::to_owned)));
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
    fn command_prompt(&mut self, typed: &str, sent: &str) {
        self.event(Event::CommandPrompt(typed.to_owned(), sent.to_owned()));
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
    fn next_step(&mut self, record: &str) {
        self.event(Event::NextStep(record.to_owned()));
    }
    fn steering(&mut self) -> Vec<String> {
        deliver(&mut self.queue().steering, self.modes.0)
    }
    fn follow_up(&mut self) -> Vec<String> {
        deliver(&mut self.queue().follow_ups, self.modes.1)
    }
    fn usage(&mut self, tokens: Tokens, context: Option<u64>, rate: Option<f64>) {
        self.send(ToScreen::Usage(tokens, context, rate));
    }
    fn model_switched(&mut self, agent: &agent::Agent) {
        self.send(ToScreen::Switched(
            footer::Shown::of(agent),
            agent.effort_cycle(),
        ));
    }
    fn effort(&mut self) -> Option<String> {
        self.take_effort()
    }
    fn offers_suggestions(&mut self) -> bool {
        true
    }
    fn pause(&mut self) -> bool {
        let queue = self.queue();
        queue.model_steps != 0
            || (queue.commands.first()).is_some_and(|command| !stops_turn(command))
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
    /// What Escape does meanwhile, as Scramjet's status says: `interrupt` or `cancel`.
    escape: Option<&'static str>,
}

impl Activity {
    /// Part of a turn, which Escape interrupts.
    fn new(what: impl Into<String>) -> Self {
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
    /// What the status line says once `event` happened, if that changes it.
    fn after(event: &Event) -> Option<Self> {
        Some(match event {
            Event::Waiting(Purpose::Reply) => Self::new("thinking"),
            Event::Waiting(purpose) => Self::cancellable(waiting_for(*purpose)),
            Event::Progress(StreamProgress::Call(name)) => Self::new(format!("preparing {name}")),
            Event::ToolRunning(since) => Self {
                since: *since,
                ..Self::new("running")
            },
            Event::CommandStarted(..) => Self::new("running"),
            Event::ToolResult(..) | Event::CommandResult(..) => Self::new("working"),
            Event::Retrying(_, _, _, delay) => Self {
                countdown: Some(*delay),
                ..Self::cancellable("retrying in")
            },
            _ => return None,
        })
    }
    /// The status line's text, with what Escape does when `escape` says it does that now.
    fn line(&self, escape: bool) -> String {
        let hint = match self.escape.filter(|_| escape) {
            Some(does) => format!(" (Esc to {does})"),
            None => String::new(),
        };
        match self.countdown {
            Some(wait) => {
                let left = wait.saturating_sub(self.since.elapsed()).as_secs_f64();
                format!("… {} {}s{hint}", self.what, left.ceil())
            }
            None => format!(
                "… {}{}{hint}",
                self.what,
                elapsed(self.since.elapsed().as_secs())
            ),
        }
    }
}

enum Shown {
    Pick(Box<Picker>),
    Text(Box<TextDialog>),
    Approve(String),
}

struct TextDialog {
    title: String,
    editor: Editor,
    draft: Draft,
    optional: Optional,
}

struct Open {
    shown: Shown,
    reply: mpsc::Sender<Answer>,
    /// A held list was answered: it shows, taking no keys, until the session replaces it.
    answered: bool,
}

/// How the conversation is rendered.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    width: usize,
    hide_thinking: bool,
    /// Tool results are shown whole.
    whole: bool,
    markdown_reads: MarkdownReads,
}

/// A line of the conversation by its part, so that it stays in view as parts before it or it
/// render to more or fewer lines.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    part: usize,
    line: usize,
}

/// Where a point of a selection is: a line of the conversation, or a row of the dock under it.
#[derive(Clone, Copy, PartialEq)]
enum At {
    Line(Place),
    Dock(usize),
}

/// A point between two cells: before column `column` of the line or row at `at`.
#[derive(Clone, Copy, PartialEq)]
struct Mark {
    at: At,
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
    /// It started in the dock, which it then reaches wherever the view is.
    in_dock: bool,
}

/// What is selected, by line and column, from its start to before its end; the dock's rows are
/// counted on from `length`, the conversation's.
struct Selected {
    start: (usize, usize),
    end: (usize, usize),
    length: usize,
    /// The conversation's lines from here on are not selected: as in Scramjet, a selection
    /// that reaches into the dock takes the conversation only as far as it is shown.
    seam: usize,
}

impl Selected {
    /// The columns selected of line `line`, from and before, if any.
    fn columns(&self, line: usize) -> Option<(usize, usize)> {
        let taken = (line < self.seam || line >= self.length)
            && (self.start.0..=self.end.0).contains(&line);
        taken.then(|| {
            let left = (line == self.start.0).then_some(self.start.1);
            let right = (line == self.end.0).then_some(self.end.1);
            (left.unwrap_or(0), right.unwrap_or(usize::MAX))
        })
    }
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

/// The clipboard read for a right click, or an image's path for Ctrl+V, pasted only into the
/// editor it was for, unchanged.
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
    lines: Vec<Line>,
    rendered: Option<(Look, usize)>,
}

impl Transcript {
    fn push(&mut self, event: Event) {
        if self.parts.is_empty() {
            self.parts.push(Part::default());
        }
        let part = self.parts.last_mut().expect("pushed above");
        part.version += 1;
        add(&mut part.events, event);
    }

    /// Adds `event` to call `index` of the calls running at the same time, shown last.
    fn push_call(&mut self, index: usize, event: Event) {
        match self.calls_mut().and_then(|calls| calls.get_mut(index)) {
            Some(events) => add(events, event),
            None => return self.push(event),
        }
        self.parts.last_mut().expect("has the calls").version += 1;
    }

    /// The calls running at the same time, when they are what was shown last.
    fn calls(&self) -> Option<&[Vec<Event>]> {
        match self.parts.last()?.events.last()? {
            Event::Calls(calls) => Some(calls),
            _ => None,
        }
    }

    fn calls_mut(&mut self) -> Option<&mut Vec<Vec<Event>>> {
        match self.parts.last_mut()?.events.last_mut()? {
            Event::Calls(calls) => Some(calls),
            _ => None,
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
    fn window(&self, top: usize, count: usize) -> Vec<Line> {
        let lines = self.parts.iter().flat_map(|part| &part.lines);
        lines.skip(top).take(count).cloned().collect()
    }

    /// Line `line` as drawn, empty past the end.
    fn shown(&self, line: usize) -> String {
        let line = self.window(line, 1).pop();
        line.map(|line| line.shown).unwrap_or_default()
    }
}

/// Adds `event` to `events`, keeping streamed text whole, and only a running tool's latest
/// output and children.
fn add(events: &mut Vec<Event>, event: Event) {
    match (events.last_mut(), event) {
        (Some(Event::ToolOutput(last)), Event::ToolOutput(output)) => *last = output,
        (Some(Event::Subagents(last)), Event::Subagents(shown)) => *last = shown,
        (
            Some(Event::Progress(StreamProgress::Text(last))),
            Event::Progress(StreamProgress::Text(delta)),
        )
        | (
            Some(Event::Progress(StreamProgress::Reasoning(last))),
            Event::Progress(StreamProgress::Reasoning(delta)),
        ) => last.push_str(&delta),
        (_, event) => events.push(event),
    }
}

/// The lines of `events` as `Terminal` renders them at the look's width.
fn render(events: &[Event], look: Look, hide_thinking: Arc<AtomicBool>) -> Vec<Line> {
    let mut ui = Terminal::capture_shown(look.width, hide_thinking, look.whole);
    ui.markdown_reads = look.markdown_reads;
    for event in events {
        record::replay(event, &mut ui);
    }
    record::running(events.last(), &mut ui);
    let mut lines = selection::lines(&ui.captured());
    // Each block ends with a blank line, which separates it from the next part.
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    lines
}

/// What runs the `!` commands typed during a turn.
struct Sides {
    root: PathBuf,
    /// Wakes the screen when one's output grows or it ends.
    wake: PipeWriter,
    threads: Vec<JoinHandle<()>>,
}

struct App {
    editor: Editor,
    draft: Draft,
    transcript: Transcript,
    hide_thinking: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    inputs: mpsc::Sender<Input>,
    queue: Arc<Mutex<Queue>>,
    sides: Sides,
    /// Where Shift+Tab's cycle is while a turn runs.
    efforts: EffortCycle,
    /// The session waits for input.
    ready: bool,
    /// A turn runs, with what it does now.
    activity: Option<Activity>,
    dialog: Option<Open>,
    /// Keys typed while an answered held list waits for the session's next, which then takes
    /// them, or the prompt does.
    held_keys: Vec<(Key, bool)>,
    footer: Footer,
    /// The first line shown, while the user has scrolled away from the end.
    top: Option<Place>,
    /// Tool results are shown whole (Ctrl+O).
    whole: bool,
    markdown_reads: MarkdownReads,
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
    /// The dock's rows drawn last.
    docked: Vec<Line>,
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
        sides: Sides,
    ) -> Self {
        Self {
            footer: Footer::new(&sides.root),
            draft: editor.draft(String::new()),
            editor,
            transcript: Transcript::default(),
            hide_thinking,
            cancel,
            inputs,
            queue,
            sides,
            efforts: EffortCycle::default(),
            ready: false,
            activity: None,
            dialog: None,
            held_keys: Vec::new(),
            top: None,
            whole: false,
            markdown_reads: MarkdownReads::Rendered,
            selection: None,
            edge: None,
            copying: None,
            pasting: None,
            click: None,
            exiting: false,
            size: (0, 0),
            drawn: Vec::new(),
            docked: Vec::new(),
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
                    Ok(ToScreen::Quit) | Err(TryRecvError::Disconnected) => {
                        self.stop_shells();
                        return Ok(());
                    }
                    Ok(ToScreen::Terminal(sequence)) => to_terminal(&sequence)?,
                    Ok(ToScreen::Edit(file, reply)) => {
                        drop(screen);
                        raw.suspend();
                        let edited = editor::edit_file(&file);
                        raw.resume()?;
                        screen = Screen::enter();
                        self.drawn.clear();
                        let _ = reply.send(edited);
                    }
                    Ok(message) => self.message(message),
                    Err(TryRecvError::Empty) => break,
                }
            }
            if self.ready && (self.queue().shells.first()).is_some_and(|side| side.ended.is_some())
            {
                self.ready = false;
                let _ = self.inputs.send(Input::Shells);
            }
            // Typed as the turn ended, after the session took those typed during it.
            if self.ready && !self.queue().commands.is_empty() {
                self.ready = false;
                let _ = self.inputs.send(Input::Commands);
            }
            self.editor.poll(&mut self.draft);
            if let Some(Open {
                shown: Shown::Text(text),
                ..
            }) = &mut self.dialog
            {
                text.editor.poll(&mut text.draft);
            }
            if let Some(Open {
                shown: Shown::Pick(picker),
                ..
            }) = &self.dialog
                && let Some(row) = picker.expired()
            {
                self.answer(Some(Answer::Expired(row)));
            }
            self.copied()?;
            self.pasted();
            self.footer.check_branch();
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
            if let Some(Open {
                shown: Shown::Pick(picker),
                ..
            }) = &self.dialog
                && let Some(change) = picker.next_change()
            {
                wait = wait.min(change);
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
                if let Event::Subagents(children) = &event {
                    self.footer.subagents(0, children);
                }
                self.transcript.push(event);
            }
            ToScreen::Call(index, event) => {
                // The status line says `running` until every call has ended.
                let others = (self.transcript.calls().into_iter().flatten().enumerate())
                    .any(|(other, events)| other != index && !record::ended(events));
                if !(others && matches!(event, Event::ToolResult(..))) {
                    self.track(&event);
                }
                if let Event::Subagents(children) = &event {
                    self.footer.subagents(index, children);
                }
                self.transcript.push_call(index, event);
            }
            ToScreen::Clear => {
                self.transcript = Transcript::default();
                self.top = None;
                (self.selection, self.edge) = (None, None);
            }
            ToScreen::Turn(efforts) => {
                let running = efforts.is_some();
                self.transcript.split();
                self.activity = running.then(|| Activity::new("working"));
                if let Some(efforts) = efforts {
                    self.efforts = efforts;
                }
                // What the turn did not take is not sent unasked.
                if !running {
                    self.restore();
                }
            }
            ToScreen::Ready => {
                self.ready = true;
                if self.dialog.as_ref().is_some_and(|open| open.answered) {
                    self.dialog = None;
                    self.replay();
                }
            }
            ToScreen::SetText(text) => self.draft.replace(text),
            ToScreen::Remember(prompt) => self.editor.remember(&prompt),
            ToScreen::Note(note) => self.draft.set_note(note),
            ToScreen::Reloaded(commands, double_escape, markdown_reads) => {
                self.editor.reloaded(commands, double_escape);
                self.markdown_reads = markdown_reads;
            }
            ToScreen::Models(models) => self.editor.set_models(models),
            ToScreen::Footer(shown) => self.footer.show(shown),
            ToScreen::Switched(shown, efforts) => {
                self.footer.show(shown);
                self.efforts = efforts;
            }
            ToScreen::Usage(tokens, context, rate) => self.footer.usage(tokens, context, rate),
            ToScreen::Dialog(dialog, reply) => {
                let shown = match dialog {
                    Dialog::Pick(picker) if picker.is_empty() => {
                        let _ = reply.send(Answer::Picked(None));
                        return;
                    }
                    Dialog::Pick(picker) => Shown::Pick(picker),
                    Dialog::Text(title, text, optional) => {
                        let editor = Editor::dialog(self.hide_thinking.clone());
                        let draft = editor.draft(text);
                        Shown::Text(Box::new(TextDialog {
                            title,
                            editor,
                            draft,
                            optional,
                        }))
                    }
                    Dialog::Approve(question) => Shown::Approve(question),
                };
                self.dialog = Some(Open {
                    shown,
                    reply,
                    answered: false,
                });
                self.top = None;
                if let Some(activity) = &mut self.activity {
                    *activity = Activity {
                        escape: None,
                        ..Activity::new("waiting for you")
                    };
                }
                self.replay();
            }
            ToScreen::Dismiss => {
                self.dialog = None;
                self.replay();
            }
            ToScreen::Terminal(_) | ToScreen::Edit(..) | ToScreen::Quit => {}
        }
    }

    /// Follows what the agent does for the status line.
    fn track(&mut self, event: &Event) {
        let Some(activity) = &mut self.activity else {
            return;
        };
        let Some(next) = Activity::after(event) else {
            return;
        };
        if activity.what != next.what || activity.countdown.is_some() || next.countdown.is_some() {
            *activity = next;
        }
    }

    fn exit(&mut self) {
        if !self.exiting {
            self.exiting = true;
            self.cancel.store(true, Ordering::Relaxed);
            for side in &self.queue().shells {
                side.cancel.store(true, Ordering::Relaxed);
            }
            self.answer(None);
            let _ = self.inputs.send(Input::End);
        }
    }

    /// Gives the keys typed while a held list waited to what is shown now.
    fn replay(&mut self) {
        for (key, pending) in std::mem::take(&mut self.held_keys) {
            // Ctrl-Z and Ctrl-G among them are dropped: only the loop reading keys hands over
            // the terminal.
            let _ = self.key(key, pending);
        }
    }

    /// Answers the open dialog, if any: with `answer`, or as cancelled.
    fn answer(&mut self, answer: Option<Answer>) {
        let Some(open) = self.dialog.take() else {
            return;
        };
        if open.answered {
            return;
        }
        let (answer, open) = match (answer, open) {
            (
                Some(Answer::Label(entry, None)),
                Open {
                    shown: Shown::Pick(picker),
                    reply,
                    ..
                },
            ) => {
                let _ = reply.send(Answer::Label(entry, Some(picker)));
                if let Some(activity) = &mut self.activity {
                    *activity = Activity::new("working");
                }
                return;
            }
            pair => pair,
        };
        let answer = answer.unwrap_or(match open.shown {
            Shown::Pick(_) => Answer::Picked(None),
            Shown::Text(_) => Answer::Text(None),
            Shown::Approve(_) => Answer::Approved(agent::Answer::Deny),
        });
        let _ = open.reply.send(answer);
        if !self.exiting && matches!(&open.shown, Shown::Pick(picker) if picker.is_held()) {
            self.dialog = Some(Open {
                answered: true,
                ..open
            });
        }
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
        self.draft.replace(text);
        self.draft.set_note(restored(count));
    }

    /// Queues the draft's text while a turn runs: for the model's next request, or, as a
    /// follow-up, for when the turn would end; a command set's command starts there. A `!`
    /// command runs at once, and a built-in command goes to the session (`command`).
    fn enqueue(&mut self, follow_up: bool) {
        let text = self.draft.text().trim().to_owned();
        if text.is_empty() || self.shell(&text) {
            return;
        }
        if Builtin::parse(&text).is_some() {
            self.command(text);
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

    /// Takes the built-in command `text` typed while a turn runs, as Scramjet runs it at once:
    /// `/quit` exits, `/reload` is refused, a command that replaces or rewrites the conversation
    /// stops the turn and runs once it has, and the others run before the turn's next request
    /// (`AgentUi::pause`).
    fn command(&mut self, text: String) {
        match Builtin::parse(&text) {
            Some(Builtin::Quit) => return self.exit(),
            Some(Builtin::Reload) => {
                self.draft = self.editor.draft(String::new());
                return self.draft.set_note(RELOAD_BUSY.to_owned());
            }
            Some(builtin) if builtin.interrupts() => self.cancel.store(true, Ordering::Relaxed),
            _ => {}
        }
        self.queue().commands.push(text);
        self.draft = self.editor.draft(String::new());
        self.top = None;
    }

    /// Runs `typed` if it is a `!` or `!!` command, as Scramjet does during a turn: apart from
    /// the session, shown under the conversation until the turn ends, then added to it (`!`) or
    /// only shown (`!!`). Returns whether it was one.
    fn shell(&mut self, typed: &str) -> bool {
        let Some(command) = typed.strip_prefix('!') else {
            return false;
        };
        let (include, command) = match command.strip_prefix('!') {
            Some(command) => (false, command.trim()),
            None => (true, command.trim()),
        };
        if command.is_empty() {
            return false;
        }
        let mut queue = self.queue();
        if queue.shells.iter().any(|side| side.ended.is_none()) {
            drop(queue);
            self.draft.set_note(SHELL_BUSY.to_owned());
            return true;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        queue.shells.push(Side {
            command: command.to_owned(),
            include,
            output: String::new(),
            ended: None,
            cancel: cancel.clone(),
        });
        drop(queue);
        let started = self.sides.wake.try_clone().and_then(|wake| {
            let (queue, root) = (self.queue.clone(), self.sides.root.clone());
            let command = command.to_owned();
            std::thread::Builder::new()
                .name("shell".into())
                .spawn(move || {
                    let running = |change: &mut dyn FnMut(&mut Side)| {
                        let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                        if let Some(side) =
                            queue.shells.iter_mut().find(|side| side.ended.is_none())
                        {
                            change(side);
                        }
                        drop(queue);
                        let _ = (&wake).write(&[0]);
                    };
                    let shell = Shell::run(&root, &command, include, &cancel, &mut |output| {
                        running(&mut |side| side.output = output.to_owned())
                    });
                    let mut shell = Some(shell);
                    running(&mut |side| side.ended = shell.take());
                })
        });
        match started {
            Ok(thread) => {
                self.sides.threads.retain(|thread| !thread.is_finished());
                self.sides.threads.push(thread);
                self.editor.remember(typed);
                self.draft = self.editor.draft(String::new());
                self.top = None;
            }
            Err(error) => {
                self.queue().shells.pop();
                self.draft
                    .set_note(format!("error: cannot run the command: {error}"));
            }
        }
        true
    }

    /// Stops the `!` commands that still run, and waits for them to end.
    fn stop_shells(&mut self) {
        for side in &self.queue().shells {
            side.cancel.store(true, Ordering::Relaxed);
        }
        for thread in self.sides.threads.drain(..) {
            let _ = thread.join();
        }
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

    /// Whether the selection reaches into the dock, as in Scramjet: it started there, or the
    /// view is at the end.
    fn reaches_dock(&self) -> bool {
        (self.selection.as_ref()).is_some_and(|selection| selection.in_dock)
            || self.shown_top() == self.last
    }

    /// The point at the screen's `column` and `row`: in the dock if `row` is there and `dock`,
    /// else in the conversation, kept to the lines shown.
    fn mark(&self, column: usize, row: usize, dock: bool) -> Mark {
        if dock && row >= self.rows && !self.docked.is_empty() {
            let row = (row - self.rows).min(self.docked.len() - 1);
            let column = selection::snap(&self.docked[row].shown, column);
            return Mark {
                at: At::Dock(row),
                column,
            };
        }
        let line = self.shown_top() + row.min(self.rows.saturating_sub(1));
        let line = line.min(self.transcript.len().saturating_sub(1));
        let text = self.transcript.shown(line);
        Mark {
            at: At::Line(self.transcript.locate(line)),
            column: selection::snap(&text, column),
        }
    }

    /// The line or row at `at`, as drawn.
    fn shown(&self, at: At) -> String {
        match at {
            At::Line(place) => self.transcript.shown(self.transcript.line(place)),
            At::Dock(row) => {
                (self.docked.get(row)).map_or_else(String::new, |line| line.shown.clone())
            }
        }
    }

    fn selected(&self) -> Option<Selected> {
        let selection = self.selection.as_ref()?;
        let length = self.transcript.len();
        let point = |mark: Mark| match mark.at {
            At::Line(place) => (self.transcript.line(place), mark.column),
            At::Dock(row) => {
                let row = row.min(self.docked.len().saturating_sub(1));
                (length + row, mark.column)
            }
        };
        let (anchor, head) = (point(selection.anchor), point(selection.head));
        let (start, end) = (anchor.min(head), anchor.max(head));
        let seam = match end.0 >= length {
            true => (self.shown_top() + self.rows).min(length),
            false => length,
        };
        (anchor != head).then_some(Selected {
            start,
            end,
            length,
            seam,
        })
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
                let head = self.mark(column, row, self.reaches_dock());
                if let Some(selection) = &mut self.selection
                    && selection.held
                    && selection.dragged
                {
                    selection.head = head;
                }
            }
            RIGHT if self.selection.is_some() => self.copy(),
            RIGHT if self.copying.is_none() => self.paste(clipboard::paste),
            LEFT => {
                let previous = self.click.take();
                self.clear_selection();
                let top = self.shown_top();
                let in_dock = row >= self.rows && row - self.rows < self.docked.len();
                if in_dock || (row < self.rows && top + row < self.transcript.len()) {
                    let follow = self.top.is_none();
                    self.top = Some(self.transcript.locate(top));
                    let mark = self.mark(column, row, in_dock);
                    let line = self.shown(mark.at);
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
                        in_dock,
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
                let head = self.mark(column, row, self.reaches_dock());
                let Some(selection) = self.selection.as_mut().filter(|selection| selection.held)
                else {
                    return;
                };
                selection.dragged = true;
                selection.head = head;
                let direction = match row {
                    0 => -1,
                    row if row + 1 >= self.rows && !selection.in_dock => 1,
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
        let scrolled = self.scroll(direction);
        // At the end, the pointer may be on the dock's rows.
        let head = self.mark(column, row, self.reaches_dock());
        if let Some(selection) = &mut self.selection {
            selection.head = head;
        }
        match (scrolled, &mut self.edge) {
            (true, Some(edge)) => edge.scrolled = Instant::now(),
            _ => self.edge = None,
        }
    }

    /// Copies the text selected, as shown, on a thread of its own: a clipboard tool may take a
    /// while.
    fn copy(&mut self) {
        let Some(selected) = self.selected().filter(|_| self.copying.is_none()) else {
            return;
        };
        let Selected {
            start,
            end,
            length,
            seam,
        } = selected;
        let shown = start.0.min(seam)..(end.0 + 1).min(seam);
        let mut lines = self.transcript.window(shown.start, shown.len());
        if end.0 >= length {
            let rows = start.0.max(length) - length..=end.0 - length;
            lines.extend(self.docked.get(rows).into_iter().flatten().cloned());
        }
        let left = match start.0 < seam || start.0 >= length {
            true => start.1,
            false => 0,
        };
        let text = selection::text(&lines, left, end.1);
        if text.is_empty() {
            return;
        }
        self.edge = None;
        let (done, outcome) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                let _ = done.send(clipboard::copy(&text, true));
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
                    to_terminal(&sequence)?;
                }
                self.clear_selection();
            }
            Err(error) => self.draft.set_note(format!("Copy failed: {error}")),
        }
        Ok(())
    }

    /// The editor shown, if any: a text dialog's, or the prompt's when no dialog is open.
    fn editing(&mut self) -> Option<(&mut Editor, &mut Draft)> {
        match &mut self.dialog {
            None => Some((&mut self.editor, &mut self.draft)),
            Some(Open {
                shown: Shown::Text(text),
                ..
            }) => Some((&mut text.editor, &mut text.draft)),
            Some(_) => None,
        }
    }

    /// Reads the `clipboard`, on a thread of its own, for the editor shown.
    fn paste(&mut self, clipboard: fn() -> Result<String, String>) {
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
                let _ = done.send(clipboard());
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
            Key::PasteImage => {
                self.paste(clipboard::paste_image);
                return Flow::Go;
            }
            Key::Expand if !self.list_takes(&key) => {
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
            Key::Top | Key::Bottom => {
                self.navigate(if key == Key::Top {
                    isize::MIN
                } else {
                    isize::MAX
                });
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
            let queue = self.queue();
            if let Some(side) = queue.shells.iter().find(|side| side.ended.is_none()) {
                side.cancel.store(true, Ordering::Relaxed);
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
            // `/model`, as typed, but the draft stays.
            Key::SelectModel => {
                let model = "/model".to_owned();
                if running {
                    self.queue().commands.push(model);
                } else if self.ready {
                    self.ready = false;
                    self.top = None;
                    let _ = self.inputs.send(Input::Prompt(model));
                } else {
                    self.draft.set_note(BUSY.to_owned());
                }
                return Flow::Go;
            }
            key => key,
        };
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
            Action::Submit(_) => self.draft.set_note(BUSY.to_owned()),
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
                self.draft.set_note(effort_note(Ok(level)));
            }
            Action::CycleEffort if self.ready => {
                let _ = self.inputs.send(Input::CycleEffort);
            }
            Action::CycleEffort => self.draft.set_note(BUSY.to_owned()),
            Action::CycleModel(steps) if running => {
                self.queue().model_steps += steps;
                self.draft.set_note(MODEL_NEXT.to_owned());
            }
            Action::CycleModel(steps) if self.ready => {
                let _ = self.inputs.send(Input::CycleModel(steps));
            }
            Action::CycleModel(_) => self.draft.set_note(BUSY.to_owned()),
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
        if key == Key::Interrupt
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
                *activity = Activity::new("working");
            }
            return Flow::Go;
        }
        let Some(open) = &mut self.dialog else {
            return Flow::Go;
        };
        let answer = match &mut open.shown {
            Shown::Pick(picker) if key == Key::ToggleThinking && !picker.takes(&key) => {
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
                Key::Enter | Key::Escape => Some(Answer::Approved(agent::Answer::Deny)),
                _ => None,
            },
            Shown::Text(text)
                if key == Key::Enter
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

    /// The status line: what a running turn does, and how far the view is from the end.
    fn status(&self, width: usize) -> String {
        let mut parts = Vec::new();
        if let Some(activity) = &self.activity {
            // Escape goes back to the end, lets a selection go or closes completion first.
            let escape = self.top.is_none() && self.selection.is_none() && !self.draft.completing();
            parts.push(activity.line(escape));
        }
        let below = self.last.saturating_sub(self.shown_top());
        if below > 0 {
            parts.push(format!("{below} more lines below · PgDn or Esc to go back"));
        }
        let line = clip(&parts.join(" · "), width.saturating_sub(1));
        format!("{}{line}{}", style::STATUS, style::RESET)
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
            let mut ui = Terminal::capture_dock(width, self.hide_thinking.clone(), lines);
            for event in &events {
                record::replay(event, &mut ui);
            }
            record::running(events.last(), &mut ui);
            rows.extend(selection::lines(&ui.captured()));
        }
        rows.push(selection::row(&self.status(width)));
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
        if !queue.is_empty() {
            let hint = clip(
                "↳ Alt+Up to edit all queued messages",
                width.saturating_sub(1),
            );
            rows.push(muted(&hint));
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
                rows.push(selection::row(&format!(
                    "{}{}{}  {}Enter submit · Esc cancel{}",
                    style::SELECTOR_TITLE,
                    text.title,
                    style::RESET,
                    style::MUTED,
                    style::RESET
                )));
                prompt_rows(&text.draft, &mut rows, width, room)
            }
            None => {
                rows.push(Line::rule(&format!(
                    "{}{}{}",
                    style::MUTED,
                    "─".repeat(width.saturating_sub(1)),
                    style::RESET
                )));
                prompt_rows(&self.draft, &mut rows, width, room)
            }
        };
        // As Scramjet's editor and its lists end with a rule over the footer.
        let ruled = (rows.last()).is_some_and(|row| {
            let text = plain(&row.shown);
            !text.is_empty() && text.chars().all(|c| c == '─')
        });
        if !ruled {
            rows.push(Line::rule(&format!(
                "{}{}{}",
                style::MUTED,
                "─".repeat(width.saturating_sub(1)),
                style::RESET
            )));
        }
        rows.extend(selection::rows(&footer));
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
        let look = Look {
            width,
            hide_thinking: self.hide_thinking.load(Ordering::Relaxed),
            whole: self.whole,
            markdown_reads: self.markdown_reads,
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
        let shown = |line: &Line, at: usize| match selected.as_ref().and_then(|s| s.columns(at)) {
            Some((left, right)) => selection::highlight(&line.shown, left, right),
            None => line.shown.clone(),
        };
        let window = self.transcript.window(top, rows);
        let mut frame: Vec<String> = (window.iter().zip(top..))
            .map(|(line, at)| shown(line, at))
            .collect();
        frame.resize(rows, String::new());
        let length = self.transcript.len();
        frame.extend((self.docked.iter().zip(length..)).map(|(line, at)| shown(line, at)));
        frame.truncate(height);
        let cursor = cursor.map(|(row, column)| (rows + row, column));
        if frame == self.drawn && cursor == self.cursor {
            return Ok(());
        }
        for (index, row) in frame.iter().enumerate() {
            if self.drawn.get(index) != Some(row) {
                // Erased first: without autowrap, a row as wide as the screen leaves the
                // cursor on its last cell, which erasing after it would take.
                let reset = style::RESET;
                write!(out, "\x1b[{};1H\x1b[2K{row}{reset}", index + 1).unwrap();
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

/// Whether the built-in command `command`, typed during a turn, waits for the turn to stop.
fn stops_turn(command: &str) -> bool {
    Builtin::parse(command).is_none_or(Builtin::interrupts)
}

fn to_terminal(sequence: &str) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    stderr.write_all(sequence.as_bytes())?;
    stderr.flush()
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
    rows: &mut Vec<Line>,
    width: usize,
    room: usize,
) -> Option<(usize, usize)> {
    let (text, row, column) = draft.rows(width);
    let shown = (room * EDITOR_SHARE / 100).max(1).min(text.len());
    let top = (row + 1).saturating_sub(shown).min(text.len() - shown);
    let start = rows.len();
    rows.extend(selection::rows(&text[top..top + shown]));
    if let Some(hint) = draft.hint() {
        let mut wrap = Wrap::new(width.saturating_sub(1).max(10));
        let text = wrap.push(hint) + &wrap.finish();
        rows.extend(text.split('\n').map(muted));
    }
    rows.extend(selection::rows(&draft.completion_rows(width)));
    Some((start + row - top, column))
}

/// A dock row of `text` in the muted style.
fn muted(text: &str) -> Line {
    selection::row(&format!("{}{text}{}", style::MUTED, style::RESET))
}

/// The alternate screen, lines cut at the edge rather than wrapped, and the mouse's buttons,
/// drags and wheel reported (SGR), until dropped.
struct Screen;

/// Set while a `Screen` is shown, from before it is entered until after it is left.
static SHOWN: AtomicBool = AtomicBool::new(false);
const LEAVE: &str = "\x1b[?1002l\x1b[?1006l\x1b[?7h\x1b[?25h\x1b[?1049l";

impl Screen {
    fn enter() -> Self {
        SHOWN.store(true, Ordering::SeqCst);
        eprint!("\x1b[?1049h\x1b[?7l\x1b[?1002h\x1b[?1006h\x1b[2J");
        Self
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        eprint!("{LEAVE}");
        let _ = io::stderr().flush();
        SHOWN.store(false, Ordering::SeqCst);
    }
}

/// What gives the terminal back from the screen, when it is shown, for a process exiting without
/// dropping it: the end of a frame being drawn, bracketed paste off, as `RawMode` turns it off,
/// and the screen left. Read it holding stderr's lock, so that no frame follows it.
pub(super) fn leave_on_exit() -> Option<String> {
    (SHOWN.load(Ordering::SeqCst)).then(|| format!("\x1b[?2026l\x1b[?2004l{LEAVE}"))
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

    /// Rendered `width` columns wide, the tool results `whole` or cut.
    fn look(width: usize, whole: bool) -> Look {
        Look {
            width,
            hide_thinking: false,
            whole,
            markdown_reads: MarkdownReads::Rendered,
        }
    }

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
    fn the_status_line_says_what_runs_and_what_escape_does() {
        let said = |event: Event, escape| Activity::after(&event).unwrap().line(escape);
        assert_eq!(
            said(Event::Waiting(Purpose::Reply), true),
            "… thinking (Esc to interrupt)"
        );
        assert_eq!(said(Event::Waiting(Purpose::Reply), false), "… thinking");
        let compaction = |reason| Event::Waiting(Purpose::Compaction(reason));
        assert_eq!(
            [
                said(compaction(None), true),
                said(compaction(Some(AutoCompaction::Threshold)), true),
                said(compaction(Some(AutoCompaction::Overflow)), true),
                said(Event::Waiting(Purpose::BranchSummary), true),
            ],
            [
                "… compacting context (Esc to cancel)",
                "… auto-compacting (Esc to cancel)",
                "… context overflow detected, auto-compacting (Esc to cancel)",
                "… summarizing branch (Esc to cancel)",
            ]
        );
        assert_eq!(
            said(Event::ToolRunning(Instant::now()), true),
            "… running (Esc to interrupt)"
        );
        let retry = Event::Retrying("busy".into(), 1, 3, Duration::from_secs(2));
        assert_eq!(said(retry, true), "… retrying in 2s (Esc to cancel)");
        let asking = Activity {
            escape: None,
            ..Activity::new("waiting for you")
        };
        assert_eq!(asking.line(true), "… waiting for you");
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
            ..Queue::default()
        };
        assert_eq!(queue.take(), ["steer", "later"]);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_selection_into_the_dock_takes_the_conversation_as_far_as_shown() {
        // Lines 0 to 9 of the conversation, shown as far as 7, then the dock's rows from 10.
        let selected = Selected {
            start: (3, 4),
            end: (11, 2),
            length: 10,
            seam: 8,
        };
        let columns = [2, 3, 7, 8, 9, 10, 11, 12].map(|line| selected.columns(line));
        let all = Some((0, usize::MAX));
        assert_eq!(
            columns,
            [
                None,
                Some((4, usize::MAX)),
                all,
                None,
                None,
                all,
                Some((0, 2)),
                None
            ]
        );
    }

    #[test]
    fn streamed_text_and_running_output_are_kept_whole() {
        let mut transcript = Transcript::default();
        transcript.push(Event::Progress(StreamProgress::Text("Hel".into())));
        transcript.push(Event::Progress(StreamProgress::Text("lo".into())));
        transcript.push(Event::ToolRunning(Instant::now()));
        transcript.push(Event::ToolOutput("1".into()));
        transcript.push(Event::ToolOutput("1\n2".into()));
        assert_eq!(text(&transcript.parts[0]), ["Hello", "1\n2"]);
        transcript.split();
        transcript.split();
        assert_eq!(transcript.parts.len(), 2);
    }

    #[test]
    fn calls_running_together_each_show_in_their_place_as_they_run() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::UserPrompt("both".into()));
        transcript.push(Event::Calls(vec![Vec::new(), Vec::new(), Vec::new()]));
        let ago = |seconds| {
            Instant::now()
                .checked_sub(Duration::from_secs(seconds))
                .unwrap()
        };
        for (index, command) in ["seq 1 2", "seq 3 4"].into_iter().enumerate() {
            transcript.push_call(index, Event::ToolCall("bash".into(), command.into(), None));
            transcript.push_call(index, Event::ToolRunning(ago(5)));
        }
        transcript.push_call(0, Event::ToolOutput("1".into()));
        transcript.push_call(1, Event::ToolOutput("3".into()));
        transcript.push_call(0, Event::ToolOutput("1\n2".into()));
        let shown = |transcript: &mut Transcript| -> Vec<String> {
            transcript.render(look(40, false), &hide);
            let lines = transcript.window(0, 100).into_iter();
            lines
                .map(|line| plain(&line.shown).trim().to_owned())
                .collect()
        };
        // The third call has not started: it shows nothing yet.
        let running = ["● bash seq 1 2", "1", "2", "", "● bash seq 3 4", "3", ""];
        assert_eq!(shown(&mut transcript)[2..], running);
        let result = Event::ToolResult("bash".into(), "1\n2\n".into(), None, false, ago(3));
        transcript.push_call(0, result);
        let ended = shown(&mut transcript);
        // It took what it took when it ended, however late it renders.
        assert_eq!(ended[2..6], ["● bash seq 1 2", "1", "2", "took 2.0s"]);
        assert_eq!(ended[ended.len() - 4..], ["", "● bash seq 3 4", "3", ""]);
        // What follows the calls comes after them all.
        transcript.push(Event::Assistant("Done.".into()));
        assert_eq!(shown(&mut transcript)[ended.len() - 1..], ["Done.", ""]);
    }

    #[test]
    fn parts_render_again_only_when_they_or_the_look_change() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        transcript.push(Event::UserPrompt("count".into()));
        transcript.push(Event::ToolCall("bash".into(), "seq 1 9".into(), None));
        let output: Vec<_> = (1..=9).map(|n| n.to_string()).collect();
        transcript.push(Event::ToolResult(
            "bash".into(),
            output.join("\n"),
            None,
            false,
            Instant::now(),
        ));
        transcript.split();
        transcript.push(Event::Text("✓ done".into()));
        transcript.render(look(40, false), &hide);
        let window = |transcript: &Transcript, top, count| -> Vec<String> {
            let lines = transcript.window(top, count).into_iter();
            lines.map(|line| line.shown).collect()
        };
        let cut = window(&transcript, 0, 100);
        assert!(
            cut.iter()
                .any(|line| line.contains("earlier lines, Ctrl+O to expand")),
            "{cut:?}"
        );
        assert_eq!(cut[cut.len() - 2..], ["✓ done", ""]);
        transcript.parts[0].lines.clear();
        transcript.render(look(40, false), &hide);
        assert!(transcript.parts[0].lines.is_empty());
        transcript.render(look(40, true), &hide);
        let whole = window(&transcript, 0, 100);
        assert!(whole.len() > cut.len() && !whole.iter().any(|line| line.contains("earlier")));
        assert_eq!(transcript.len(), whole.len());
        assert_eq!(window(&transcript, 2, 1), whole[2..3]);
        // A line of the part after the result stays in view as the result grows.
        let done = Place { part: 1, line: 0 };
        assert_eq!(transcript.locate(whole.len() - 2), done);
        assert_eq!(transcript.line(done), whole.len() - 2);
        transcript.render(look(40, false), &hide);
        assert_eq!(transcript.line(done), cut.len() - 2);
        assert_eq!(window(&transcript, transcript.line(done), 1), ["✓ done"]);
    }

    #[test]
    fn a_copy_holds_what_was_written_rather_than_how_it_wrapped() {
        let hide = Arc::new(AtomicBool::new(false));
        let mut transcript = Transcript::default();
        let prompt = "please run the tests,  then say what fails";
        transcript.push(Event::UserPrompt(prompt.into()));
        let reply = "Run `cargo test --offline --no-fail-fast` to see:\n\n- every failing test with its output";
        transcript.push(Event::Progress(StreamProgress::Text(reply.into())));
        transcript.push(Event::Assistant(reply.into()));
        let command = "cargo test --offline --no-fail-fast";
        transcript.push(Event::ToolCall("bash".into(), command.into(), None));
        let output = "test a ... ok\ntest agent::tests::compaction ... FAILED";
        transcript.push(Event::ToolResult(
            "bash".into(),
            output.into(),
            None,
            true,
            Instant::now(),
        ));
        transcript.render(look(24, true), &hide);
        let lines = transcript.window(0, 100);
        assert!(lines.len() > 16, "{lines:?}");
        assert_eq!(
            selection::text(&lines[..lines.len() - 1], 0, usize::MAX),
            format!(
                "{prompt}\n\nRun cargo test --offline --no-fail-fast to see:\n\n\
                - every failing test with its output\n\n● bash {command}\n{output}"
            )
        );
        // Cut previews are copied as shown.
        transcript.render(look(24, false), &hide);
        let lines = transcript.window(0, 100);
        let failed = lines.iter().position(|line| line.shown.contains("agent::"));
        let failed = failed.expect("the failed test's line");
        assert_eq!(
            selection::text(&lines[failed..=failed], 0, usize::MAX),
            "test agent::tests::co…"
        );
    }
}
