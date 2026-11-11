//! The full-screen interface, the default on a terminal: the conversation in the alternate screen, scrolled
//! by the application, with the status line, the prompt editor and lists docked under it; the
//! editor keeps taking keys while a turn runs, and what is sent then is queued for the model, as
//! Scramjet's steering and follow-up messages. The mouse wheel scrolls, dragging or a double or
//! triple click selects text to copy, and a right click copies it or pastes, as in Scramjet's
//! retained view. A thread owns the terminal; the session runs on the caller's thread and talks
//! to it through a `Handle` (`link.rs`). The thread's `App` keeps the conversation shown
//! (`transcript.rs`), takes keys (`input.rs`) and the mouse (`mouse.rs`), and draws (`draw.rs`).

use super::background;
use super::banner::Banner;
use super::completion::Completions;
use super::design::Design;
use super::editor::{Draft, Editor, Note, RESIZE_CHECK};
use super::footer::{self, Footer};
use super::keymap::{self, Binding};
use super::keys::{self, Key, Press, RawMode, read_key};
use super::record::{self, Event};
use super::report::ReportView;
use super::select::{Act, Picker, Routed, Spot, Toggle};
use super::selection::Line;
use super::settings::{Layout, MarkdownReads};
use super::slash::{self, Builtin};
use super::terminal::next_spin;
use super::theme::Watched;
use super::theme_editor::{self, ThemeEditor};
use super::{external, job, style};
use scramjet::agent::{self, EffortCycle, Shell, Tokens};
use std::{
    io::{self, PipeReader, PipeWriter, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, TryRecvError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

mod draw;
mod input;
mod link;
mod mouse;
mod transcript;

use draw::Activity;
use input::Source;
pub(super) use link::{Handle, Relisting};
use mouse::{Click, Copying, Edge, Pasting, Selection, Thumb};
use transcript::{Place, Transcript};

/// How often an `@` search is looked at for its entries.
const TICK: Duration = Duration::from_millis(200);
/// The shortest time between two frames, so that a fast stream does not redraw for each delta.
const FRAME: Duration = Duration::from_millis(16);
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
/// A pause in typing this long ends the keys meant for the prompt: until then, the agent's
/// question that appeared meanwhile takes none of them.
const TYPING: Duration = Duration::from_millis(500);
const BUSY: &str = "Busy: the session is not ready for a prompt yet";
const RELOAD_BUSY: &str = "Wait for the current response to finish before reloading.";
const MCP_BUSY: &str = "Wait for the current response to finish before changing MCP servers.";
const MODEL_NEXT: &str = "The model changes before the next request";
const NOTHING_QUEUED: &str = "No queued messages to restore";

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
    Note(Note),
    /// `/reload` or `/settings` read the commands and skills (the `/` list's rows after the
    /// built-in ones) and the settings the screen follows.
    Reloaded {
        completions: Completions,
        /// Escape twice on an empty prompt does something.
        double_escape: bool,
        markdown_reads: MarkdownReads,
        copy_on_select: bool,
        layout: Layout,
        retain_transcript: bool,
        terminal_progress: bool,
        design: Design,
        banner: Banner,
    },
    /// `/resume` moved to another project's session: the project's directory, where `@`
    /// completes paths and `!` commands run, and whose branch the footer shows.
    Moved(PathBuf),
    /// The file of the theme in use, read again as it changes, and whether the theme follows
    /// the terminal's background, as no setting names one.
    Theme(Option<Watched>, bool),
    /// The providers' models, which complete `/model`'s argument.
    Models(Vec<String>),
    /// What the footer shows of the session now.
    Footer(footer::Shown),
    /// The agent switched the model during the turn: the footer, and the levels Shift+Tab
    /// cycles through from then on.
    Switched(footer::Shown, EffortCycle),
    /// The session's tokens, how full the context is and the model's output rate, after a
    /// response or a compaction.
    Usage(Tokens, Option<u64>, Option<f64>),
    /// A dialog and where its answer goes; `true` when the agent asks it, so that the turn
    /// waits for the user, `false` for one the user opened.
    Dialog(Dialog, mpsc::Sender<Answer>, bool),
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
    /// Waits for a key, under the title and the note: `/hotkeys`' new key. Escape cancels.
    Press(String, String),
    /// Shows the title and the note until Escape (answered `Pressed(None)`) or the session
    /// dismisses it: a login waiting for the user.
    Wait(String, String),
    /// The theme editor, a sample session showing its theme in the conversation's place.
    Theme(Box<ThemeEditor>),
    /// A report view, `/usage`'s or `/changelog`'s, shown in the conversation's place until
    /// Escape (answered `Pressed(None)`).
    Report(Box<ReportView>),
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
    /// The row chosen with Enter, and where the list was (`Picker::placing`).
    Placed(usize, Spot),
    /// Shift+L in `/tree`'s list on an entry, with the list as it was, to show again once the
    /// label is given; the screen puts the list in.
    Label(usize, Option<Box<Picker>>),
    /// The row a picker's countdown chose.
    Expired(usize),
    Text(Option<String>),
    Approved(agent::Answer),
    /// The key pressed, `None` for Escape.
    Pressed(Option<Press>),
    /// The theme editor as it was saved; `None`: cancelled.
    Theme(Option<Box<ThemeEditor>>),
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
    /// The logins' model lists changed: `Link::take_relisted` has what to say of it.
    Relisted,
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
    /// What to say of the logins' model lists, which changed, until the session reads them
    /// again (`Relisting`).
    relisted: Option<Vec<String>>,
}

/// A `!` command run apart from the session.
struct Side {
    /// Which of those typed in this screen it is.
    serial: usize,
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

/// What the screen does after a key.
enum Flow {
    Go,
    Suspend,
    External,
}

enum Shown {
    Pick(Box<Picker>),
    Text(Box<TextDialog>),
    Approve(String),
    /// The title and the note.
    Press(String, String),
    Wait(String, String),
    Theme(Box<ThemeEditor>),
    Report(Box<ReportView>),
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
    /// Before then, keys are typing that began before an agent's question showed, not answers.
    armed: Instant,
}

/// What runs the `!` commands typed during a turn.
struct Sides {
    root: PathBuf,
    /// Wakes the screen when one's output grows or it ends.
    wake: PipeWriter,
    threads: Vec<JoinHandle<()>>,
    started: usize,
}

struct App {
    editor: Editor,
    draft: Draft,
    transcript: Transcript,
    /// The conversation and where it was scrolled to, while the theme editor's sample session
    /// or `/usage`'s report is shown in its place.
    conversation: Option<(Transcript, Option<Place>)>,
    /// What the status line said before the open dialog asked the user.
    before_dialog: Option<Activity>,
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
    /// The session runs a `!` command of its own, typed while no turn ran.
    session_shell: bool,
    /// Since the last turn ended, until the user sends or types something: `Waiting for you`,
    /// with the draft as it was then.
    handed: Option<(Activity, String)>,
    dialog: Option<Open>,
    /// Keys typed while an answered held list waits for the session's next, which then takes
    /// them, or the prompt does.
    held_keys: Vec<(Key, bool)>,
    /// When the last key was typed (`TYPING`).
    typed: Option<Instant>,
    footer: Footer,
    theme: Option<Watched>,
    /// The theme follows the terminal's background (`theme::following`).
    following: bool,
    /// The first line shown, while the user has scrolled away from the end.
    top: Option<Place>,
    /// Tool results are shown whole (Ctrl+O).
    whole: bool,
    markdown_reads: MarkdownReads,
    /// How prompts, reasoning, calls and replies look (`look`).
    design: Design,
    /// The banner the session started with (`banner`).
    banner: Banner,
    /// A completed mouse selection is copied (`copyOnSelect`).
    copy_on_select: bool,
    /// The wheel's step, the prompt's height and the completion list's rows.
    layout: Layout,
    /// Leaving for good writes the conversation into the terminal's scrollback
    /// (`retainTranscriptOnExit`).
    retain_transcript: bool,
    /// The terminal is told it is busy while a turn runs (`terminal.showTerminalProgress`).
    terminal_progress: bool,
    /// When the terminal was last told so.
    busy_told: Option<Instant>,
    selection: Option<Selection>,
    /// The selections made so far, which tells a copy's from those made after it.
    selections: u64,
    edge: Option<Edge>,
    thumb: Option<Thumb>,
    copying: Option<Copying>,
    pasting: Option<Pasting>,
    click: Option<Click>,
    /// The user asked to exit; keys are no longer taken.
    exiting: bool,
    /// The terminal's size, the theme's base, the rows and the cursor drawn last.
    size: (usize, usize),
    base: &'static str,
    drawn: Vec<String>,
    /// The terminal's title, as last given with a frame.
    title: String,
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
            before_dialog: None,
            conversation: None,
            hide_thinking,
            cancel,
            inputs,
            queue,
            sides,
            efforts: EffortCycle::default(),
            ready: false,
            activity: None,
            session_shell: false,
            handed: None,
            dialog: None,
            held_keys: Vec::new(),
            typed: None,
            theme: None,
            following: false,
            top: None,
            whole: false,
            markdown_reads: MarkdownReads::Rendered,
            design: Design::Classic,
            banner: Banner::Plain,
            copy_on_select: false,
            layout: Layout::default(),
            retain_transcript: false,
            terminal_progress: false,
            busy_told: None,
            selection: None,
            selections: 0,
            edge: None,
            thumb: None,
            copying: None,
            pasting: None,
            click: None,
            exiting: false,
            size: (0, 0),
            base: "",
            drawn: Vec::new(),
            title: String::new(),
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
        let windows_terminal = keys::windows_terminal();
        let mut closed = false;
        loop {
            loop {
                match messages.try_recv() {
                    Ok(ToScreen::Quit) | Err(TryRecvError::Disconnected) => {
                        self.stop_shells();
                        let retained = self.retain_transcript.then(|| self.retained());
                        drop(screen);
                        drop(raw);
                        if let Some(retained) = retained {
                            let mut stderr = io::stderr().lock();
                            stderr.write_all(retained.as_bytes())?;
                            stderr.flush()?;
                        }
                        return Ok(());
                    }
                    Ok(ToScreen::Terminal(sequence)) => to_terminal(&sequence)?,
                    Ok(ToScreen::Edit(file, reply)) => {
                        drop(screen);
                        raw.suspend();
                        let edited = external::edit_file(&file);
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
            if self.ready && self.queue().relisted.is_some() {
                self.ready = false;
                let _ = self.inputs.send(Input::Relisted);
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
            if (self.handed.as_ref()).is_some_and(|(_, draft)| draft != self.draft.text()) {
                self.handed = None;
            }
            self.footer.check_branch();
            self.check_theme();
            self.tell_progress()?;
            if self
                .edge
                .as_ref()
                .is_some_and(|edge| edge.scrolled.elapsed() >= EDGE)
            {
                self.edge_scroll();
            }
            let since = self.drawn_at.elapsed();
            let mut wait = match self.activity.is_some() {
                true => next_spin(),
                false if self.draft.searching() => TICK,
                false => RESIZE_CHECK,
            };
            if let Some(edge) = &self.edge {
                wait = wait.min(EDGE.saturating_sub(edge.scrolled.elapsed()));
            }
            if let Some(theme) = &self.theme {
                wait = wait.min(theme.due(Instant::now()));
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
            let key = match read_key(&mut source, Some(wait), windows_terminal) {
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
                // No job control there.
                Flow::Suspend if cfg!(windows) => {}
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
                    let edited = external::external(self.draft.text());
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
                match event {
                    Event::CommandStarted(..) => self.session_shell = true,
                    Event::CommandResult(..) => self.session_shell = false,
                    _ => {}
                }
                if let Event::Subagents(children) = &event {
                    self.footer.subagents(0, children);
                }
                self.conversation_mut().push(event);
            }
            ToScreen::Call(index, event) => {
                // The status line says `running` until every call has ended.
                let calls = self.conversation_mut().calls();
                let others = (calls.into_iter().flatten().enumerate())
                    .any(|(other, events)| other != index && !record::ended(events));
                if !(others && matches!(event, Event::ToolResult(..))) {
                    self.track(&event);
                }
                if let Event::Subagents(children) = &event {
                    self.footer.subagents(index, children);
                }
                self.conversation_mut().push_call(index, event);
            }
            ToScreen::Clear => match &mut self.conversation {
                Some(conversation) => *conversation = (Transcript::default(), None),
                None => {
                    self.transcript = Transcript::default();
                    self.top = None;
                    (self.selection, self.edge) = (None, None);
                }
            },
            ToScreen::Turn(efforts) => {
                let running = efforts.is_some();
                self.conversation_mut().split();
                // A turn going on after a pause still does what it did.
                let doing = self.activity.take();
                self.activity = running.then(|| doing.unwrap_or_else(|| Activity::new("Working")));
                if let Some(efforts) = efforts {
                    self.efforts = efforts;
                }
                // What the turn did not take is not sent unasked.
                if !running {
                    self.conversation_mut().turn_ended();
                    self.restore();
                }
                self.handed =
                    (!running).then(|| (Activity::asking(), self.draft.text().to_owned()));
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
            ToScreen::Reloaded {
                completions,
                double_escape,
                markdown_reads,
                copy_on_select,
                layout,
                retain_transcript,
                terminal_progress,
                design,
                banner,
            } => {
                self.editor.reloaded(completions, double_escape);
                self.markdown_reads = markdown_reads;
                self.copy_on_select = copy_on_select;
                self.layout = layout;
                self.retain_transcript = retain_transcript;
                self.terminal_progress = terminal_progress;
                self.design = design;
                self.banner = banner;
            }
            ToScreen::Moved(root) => {
                self.editor.set_root(root.clone());
                let shown = std::mem::take(&mut self.footer.shown);
                self.footer = Footer::new(&root);
                self.footer.shown = shown;
                self.sides.root = root;
            }
            ToScreen::Theme(watched, following) => {
                self.theme = watched;
                self.following = following;
            }
            ToScreen::Models(models) => self.editor.set_models(models),
            ToScreen::Footer(shown) => self.footer.show(shown),
            ToScreen::Switched(shown, efforts) => {
                self.footer.show(shown);
                self.efforts = efforts;
            }
            ToScreen::Usage(tokens, context, rate) => self.footer.usage(tokens, context, rate),
            ToScreen::Dialog(dialog, reply, asks) => {
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
                    Dialog::Press(title, note) => Shown::Press(title, note),
                    Dialog::Wait(title, note) => Shown::Wait(title, note),
                    Dialog::Theme(editor) => {
                        style::set(&editor.theme());
                        Shown::Theme(editor)
                    }
                    Dialog::Report(view) => Shown::Report(view),
                };
                let now = Instant::now();
                let armed = match self.typed {
                    Some(typed) if asks => (typed + TYPING).max(now),
                    _ => now,
                };
                self.dialog = Some(Open {
                    shown,
                    reply,
                    answered: false,
                    armed,
                });
                self.top = None;
                if let Some(activity) = &mut self.activity
                    && asks
                {
                    let doing = std::mem::replace(activity, Activity::asking());
                    if !doing.asks() {
                        self.before_dialog = Some(doing);
                    }
                }
                self.replay();
            }
            ToScreen::Dismiss => {
                self.dialog = None;
                self.after_dialog();
                self.replay();
            }
            ToScreen::Terminal(_) | ToScreen::Edit(..) | ToScreen::Quit => {}
        }
    }

    /// Shows everything in the theme in use as its file is now, once it changed. A file that is
    /// no theme leaves the colors as they were, as in Scramjet, and the note says why.
    fn check_theme(&mut self) {
        let Some(watched) = &mut self.theme else {
            return;
        };
        if self.conversation.is_some() {
            return;
        }
        let failed = watched.failed;
        let note = match watched.check(Instant::now()) {
            None => return,
            Some(Ok(theme)) => {
                style::set(&theme);
                match failed {
                    true => Note::Info(format!("Theme {} read again", watched.name)),
                    false => return,
                }
            }
            Some(Err(error)) => Note::Error(format!(
                "Error: theme {}: {error}; its colors stay as they were",
                watched.name
            )),
        };
        self.draft.set_note(note);
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

    /// The conversation, which events go to, also while the theme editor's sample is shown.
    fn conversation_mut(&mut self) -> &mut Transcript {
        match &mut self.conversation {
            Some((conversation, _)) => conversation,
            None => &mut self.transcript,
        }
    }

    /// Shows the theme editor's sample session, or `/usage`'s report, from its start in the
    /// conversation's place while it is open, and the conversation again, where it was, once it
    /// closes.
    fn preview(&mut self) {
        let shown = match &self.dialog {
            Some(Open {
                shown: Shown::Theme(_),
                ..
            }) => Some(None),
            Some(Open {
                shown: Shown::Report(view),
                ..
            }) => Some(Some(view)),
            _ => None,
        };
        match (shown, self.conversation.take()) {
            (Some(usage), None) => {
                let sample = match usage {
                    Some(view) => report_shown(view),
                    None => {
                        let mut sample = Transcript::default();
                        for part in theme_editor::sample() {
                            sample.split();
                            part.into_iter().for_each(|event| sample.push(event));
                        }
                        sample
                    }
                };
                let conversation = std::mem::replace(&mut self.transcript, sample);
                self.conversation = Some((conversation, self.top.take()));
                self.top = Some(Place::START);
            }
            (None, Some((conversation, top))) => {
                self.transcript = conversation;
                self.top = top;
            }
            (_, conversation) => {
                self.conversation = conversation;
                return;
            }
        }
        (self.selection, self.edge) = (None, None);
    }

    /// Shows the report `/usage`'s view moved to, from its start.
    fn report_moved(&mut self) {
        if let Some(Open {
            shown: Shown::Report(view),
            ..
        }) = &self.dialog
            && self.conversation.is_some()
        {
            self.transcript = report_shown(view);
            self.top = Some(Place::START);
            (self.selection, self.edge) = (None, None);
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
                self.after_dialog();
                return;
            }
            pair => pair,
        };
        let answer = answer.unwrap_or(match open.shown {
            Shown::Pick(_) => Answer::Picked(None),
            Shown::Text(_) => Answer::Text(None),
            Shown::Approve(_) => Answer::Approved(agent::Answer::Deny),
            Shown::Press(..) | Shown::Wait(..) | Shown::Report(_) => Answer::Pressed(None),
            Shown::Theme(_) => Answer::Theme(None),
        });
        let _ = open.reply.send(answer);
        if !self.exiting && matches!(&open.shown, Shown::Pick(picker) if picker.is_held()) {
            self.dialog = Some(Open {
                answered: true,
                ..open
            });
        }
        self.after_dialog();
    }

    /// The status line says again what the turn did before the agent asked; a dialog the user
    /// opened left it as it was.
    fn after_dialog(&mut self) {
        let doing = self.before_dialog.take();
        if let Some(activity) = &mut self.activity {
            match doing {
                Some(doing) => *activity = doing,
                None if activity.asks() => *activity = Activity::new("Working"),
                None => {}
            }
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
        self.draft.set_note(Note::Info(restored(count)));
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
    /// `/exit` exits, `/reload` is refused, a command that replaces or rewrites the conversation
    /// stops the turn and runs once it has, and the others run before the turn's next request
    /// (`AgentUi::pause`).
    fn command(&mut self, text: String) {
        match Builtin::parse(&text) {
            Some(Builtin::Exit) => return self.exit(),
            Some(Builtin::Reload) => {
                self.draft = self.editor.draft(String::new());
                return self.draft.set_note(Note::Info(RELOAD_BUSY.to_owned()));
            }
            Some(Builtin::Mcp(argument))
                if !matches!(
                    slash::mcp_command(argument),
                    Some(slash::McpCommand::List) | None
                ) =>
            {
                self.draft = self.editor.draft(String::new());
                return self.draft.set_note(Note::Info(MCP_BUSY.to_owned()));
            }
            Some(builtin) if builtin.interrupts() => self.cancel.store(true, Ordering::Relaxed),
            _ => {}
        }
        self.queue().commands.push(text);
        self.draft = self.editor.draft(String::new());
        self.top = None;
    }

    /// Runs `typed` if it is a `!` or `!!` command, as Scramjet does during a turn: apart from
    /// the session, shown under the conversation while it runs and at its end once it ended,
    /// added to the session (`!`) or only shown (`!!`) when the turn ends. Returns whether it
    /// was one.
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
        self.sides.started += 1;
        let mut queue = self.queue();
        if self.session_shell || queue.shells.iter().any(|side| side.ended.is_none()) {
            drop(queue);
            let busy = "A bash command is already running.";
            let busy = match keymap::key(Binding::Interrupt) {
                Some(key) => format!("{busy} Press {key} to cancel it first."),
                None => busy.to_owned(),
            };
            self.draft.set_note(Note::Info(busy));
            return true;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        queue.shells.push(Side {
            serial: self.sides.started,
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
                let error = format!("Error: cannot run the command: {error}");
                self.draft.set_note(Note::Error(error));
            }
        }
        true
    }

    /// Tells the terminal that a turn runs, again each `PROGRESS_AGAIN` as Scramjet does, and
    /// that it no longer does.
    fn tell_progress(&mut self) -> io::Result<()> {
        let busy = self.terminal_progress && self.activity.is_some() && !self.exiting;
        if !busy {
            if PROGRESS_SHOWN.swap(false, Ordering::SeqCst) {
                to_terminal(PROGRESS_CLEAR)?;
            }
            self.busy_told = None;
            return Ok(());
        }
        let due = (self.busy_told).is_none_or(|told| told.elapsed() >= PROGRESS_AGAIN);
        if due || !PROGRESS_SHOWN.load(Ordering::SeqCst) {
            to_terminal(PROGRESS_BUSY)?;
            PROGRESS_SHOWN.store(true, Ordering::SeqCst);
            self.busy_told = Some(Instant::now());
        }
        Ok(())
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

/// The alternate screen, lines cut at the edge rather than wrapped, and the mouse's buttons,
/// drags and wheel reported (SGR), until dropped, the title it gave the terminal then cleared, as
/// Codex and Claude Code clear theirs.
struct Screen;

/// Set while a `Screen` is shown, from before it is entered until after it is left.
static SHOWN: AtomicBool = AtomicBool::new(false);
const LEAVE: &str = "\x1b[?1002l\x1b[?1006l\x1b[?7h\x1b[?25h\x1b[?1049l\x1b]0;\x07";
/// OSC 9;4's indeterminate progress and its end, Scramjet's sequences; Windows Terminal, among
/// others, shows them in the tab and the taskbar.
const PROGRESS_BUSY: &str = "\x1b]9;4;3\x07";
const PROGRESS_CLEAR: &str = "\x1b]9;4;0;\x07";
/// Scramjet repeats the busy sequence each second.
const PROGRESS_AGAIN: Duration = Duration::from_secs(1);
/// The terminal was told the agent is busy and not yet that it ended.
static PROGRESS_SHOWN: AtomicBool = AtomicBool::new(false);

/// The end of the progress the terminal shows, when it shows some, which leaving takes with it.
fn progress_cleared() -> &'static str {
    match PROGRESS_SHOWN.swap(false, Ordering::SeqCst) {
        true => PROGRESS_CLEAR,
        false => "",
    }
}

impl Screen {
    /// Shown, and the terminal asked for its background, which may have changed meanwhile.
    fn enter() -> Self {
        SHOWN.store(true, Ordering::SeqCst);
        eprint!(
            "\x1b[?1049h\x1b[?7l\x1b[?1002h\x1b[?1006h\x1b[2J{}",
            background::BACKGROUND_QUESTION
        );
        Self
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        eprint!("{}{LEAVE}", progress_cleared());
        let _ = io::stderr().flush();
        SHOWN.store(false, Ordering::SeqCst);
    }
}

/// `view`'s report as the conversation shows a note, or a reply when it is markdown.
fn report_shown(view: &ReportView) -> Transcript {
    let mut shown = Transcript::default();
    let report = view.report().to_owned();
    shown.push(match view.markdown {
        true => Event::Assistant(report),
        false => Event::Text(report),
    });
    shown
}

/// What gives the terminal back from the screen, when it is shown, for a process exiting without
/// dropping it: the end of a frame being drawn, bracketed paste off, as `RawMode` turns it off,
/// and the screen left. Read it holding stderr's lock, so that no frame follows it.
pub(super) fn leave_on_exit() -> Option<String> {
    (SHOWN.load(Ordering::SeqCst))
        .then(|| format!("\x1b[?2026l\x1b[?2004l{}{LEAVE}", progress_cleared()))
}
