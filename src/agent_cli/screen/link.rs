//! The session's side of the screen: what it sends the screen's thread, the agent's
//! interface during a turn, and the `Handle` that starts the thread and reads the user's input.

use super::{Answer, App, Dialog, Input, Queue, Sides, ToScreen, stops_turn};
use crate::agent_cli::completion::Completions;
use crate::agent_cli::editor::{Editor, Note};
use crate::agent_cli::footer;
use crate::agent_cli::keys::{Press, RawMode};
use crate::agent_cli::record::Event;
use crate::agent_cli::report::ReportView;
use crate::agent_cli::select::{Picked, Picker, Routed, Spot};
use crate::agent_cli::settings::{Delivery, DoubleEscape, Settings};
use crate::agent_cli::style::Style;
use crate::agent_cli::theme::Watched;
use crate::agent_cli::theme_editor::ThemeEditor;
use crate::agent_cli::{clipboard, console, spelling};
use scramjet::agent::{
    self, AgentError, AgentUi, AutoCompaction, Children, Choice, CompactRead, Compaction,
    EffortCycle, Purpose, Reply, Shell, StreamProgress, Tokens, ToolDetail, UserCommand,
};
use std::{
    cell::Cell,
    io::{self, PipeWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// Tells the session, once it waits for input, that the logins' model lists changed.
pub(in crate::agent_cli) struct Relisting {
    queue: Arc<Mutex<Queue>>,
    wake: PipeWriter,
}

impl Relisting {
    /// The lists changed, with `notes` to say of it, after those not yet said.
    pub(in crate::agent_cli) fn changed(&self, notes: Vec<String>) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        queue.relisted.get_or_insert_default().extend(notes);
        drop(queue);
        let _ = (&self.wake).write(&[0]);
    }
}

/// Sends to the screen; also the agent's interface during a turn.
pub(in crate::agent_cli) struct Link {
    to: mpsc::Sender<ToScreen>,
    wake: PipeWriter,
    queue: Arc<Mutex<Queue>>,
    /// `steeringMode` and `followUpMode`.
    modes: (Delivery, Delivery),
    /// The call of those running at the same time whose events are sent now.
    call: Option<usize>,
}

impl Link {
    /// Another link to the same screen, sending events of none of the calls.
    pub(in crate::agent_cli) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            to: self.to.clone(),
            wake: self.wake.try_clone()?,
            queue: self.queue.clone(),
            modes: self.modes,
            call: None,
        })
    }
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn send(&self, message: ToScreen) {
        if self.to.send(message).is_ok() {
            // A full pipe already wakes the screen.
            let _ = (&self.wake).write(&[0]);
        }
    }
    /// The events `show` gives a link like this one, kept instead of shown.
    pub(in crate::agent_cli) fn record(
        &self,
        show: impl FnOnce(&mut Self),
    ) -> io::Result<Vec<Event>> {
        let (to, from) = mpsc::channel();
        let mut link = Self {
            to,
            ..self.try_clone()?
        };
        show(&mut link);
        drop(link);
        let events = from.into_iter().filter_map(|message| match message {
            ToScreen::Event(event) | ToScreen::Call(_, event) => Some(event),
            _ => None,
        });
        Ok(events.collect())
    }
    pub(in crate::agent_cli) fn event(&self, event: Event) {
        self.send(match self.call {
            Some(index) => ToScreen::Call(index, event),
            None => ToScreen::Event(event),
        });
    }
    /// Shows a message of the session's own, wrapped on the screen.
    pub(in crate::agent_cli) fn say(&self, text: &str) {
        self.event(Event::Text(text.to_owned()));
    }
    pub(in crate::agent_cli) fn say_styled(&self, style: Style, text: &str) {
        self.event(Event::Styled(style, text.to_owned()));
    }
    pub(in crate::agent_cli) fn clear(&self) {
        self.send(ToScreen::Clear);
    }
    pub(in crate::agent_cli) fn turn(&self, efforts: EffortCycle) {
        self.send(ToScreen::Turn(efforts));
    }
    pub(in crate::agent_cli) fn aside_ended(&self) {
        self.send(ToScreen::AsideEnded);
    }
    pub(in crate::agent_cli) fn ended(&self, finished: bool) {
        self.send(ToScreen::Ended { finished });
    }
    /// The `!` commands run during a turn that have ended, in the order typed.
    pub(in crate::agent_cli) fn take_shells(&self) -> Vec<Shell> {
        let mut queue = self.queue();
        let ended = (queue.shells.iter())
            .take_while(|side| side.ended.is_some())
            .count();
        let sides = queue.shells.drain(..ended);
        sides.filter_map(|side| side.ended).collect()
    }
    /// The built-in commands typed during a turn, in order; `paused`: those before the first
    /// that stops the turn, which goes on.
    pub(in crate::agent_cli) fn take_commands(&self, paused: bool) -> Vec<String> {
        let mut queue = self.queue();
        let count = match paused {
            true => (queue.commands.iter())
                .take_while(|command| !stops_turn(command))
                .count(),
            false => queue.commands.len(),
        };
        queue.commands.drain(..count).collect()
    }
    /// What to say of the logins' model lists since they changed, if they did.
    pub(in crate::agent_cli) fn take_relisted(&self) -> Option<Vec<String>> {
        self.queue().relisted.take()
    }
    /// What another thread tells the session of the logins' model lists with.
    pub(in crate::agent_cli) fn relisting(&self) -> io::Result<Relisting> {
        Ok(Relisting {
            queue: self.queue.clone(),
            wake: self.wake.try_clone()?,
        })
    }
    /// The effort level Shift+Tab chose during a turn that no request took.
    pub(in crate::agent_cli) fn take_effort(&self) -> Option<String> {
        self.queue().effort.take()
    }
    /// The models Ctrl+P and Shift+Ctrl+P moved on by during a turn, which no request took.
    pub(in crate::agent_cli) fn take_model_steps(&self) -> isize {
        std::mem::take(&mut self.queue().model_steps)
    }
    pub(in crate::agent_cli) fn set_text(&self, text: String) {
        self.send(ToScreen::SetText(text));
    }
    pub(in crate::agent_cli) fn remember(&self, prompt: &str) {
        self.send(ToScreen::Remember(prompt.to_owned()));
    }
    pub(in crate::agent_cli) fn note(&self, note: Note) {
        self.send(ToScreen::Note(note));
    }
    /// What `/reload` read: what the editor completes, and the `settings`.
    pub(in crate::agent_cli) fn reloaded(&mut self, completions: Completions, settings: &Settings) {
        self.modes = (settings.steering_mode, settings.follow_up_mode);
        spelling::configure(settings.spell_check, settings.words().as_deref());
        self.send(ToScreen::Reloaded {
            completions,
            double_escape: settings.double_escape != DoubleEscape::Nothing,
            markdown_reads: settings.markdown_reads,
            copy_on_select: settings.copy_on_select,
            layout: settings.layout,
            retain_transcript: settings.retain_transcript,
            terminal_progress: settings.terminal_progress,
            notify: settings
                .notifications
                .resolved(|name| std::env::var(name).ok()),
            design: settings.look,
            banner: settings.banner,
            bottom: settings.bottom,
            status_line: settings.status_line.clone(),
        });
    }
    /// The prompt being typed is kept with the session saved at `session`, if any.
    pub(in crate::agent_cli) fn drafting(&self, session: Option<PathBuf>) {
        self.send(ToScreen::Drafting(session));
    }
    pub(in crate::agent_cli) fn moved(&self, root: PathBuf) {
        self.send(ToScreen::Moved(root));
    }
    /// The file of the theme in use, to read again when it changes; none for a built-in one.
    pub(in crate::agent_cli) fn watch_theme(&self, watched: Option<Watched>, following: bool) {
        self.send(ToScreen::Theme(watched, following));
    }
    pub(in crate::agent_cli) fn models(&self, models: Vec<String>) {
        self.send(ToScreen::Models(models));
    }
    pub(in crate::agent_cli) fn footer(&self, shown: footer::Shown) {
        self.send(ToScreen::Footer(shown));
    }
    /// Writes `sequence` to the terminal between frames.
    pub(in crate::agent_cli) fn to_terminal(&self, sequence: String) {
        self.send(ToScreen::Terminal(sequence));
    }
    /// Shows `dialog` and waits for its answer; `None` once the screen is gone or `cancel` set.
    pub(in crate::agent_cli) fn ask(
        &self,
        dialog: Dialog,
        cancel: Option<&AtomicBool>,
    ) -> Option<Answer> {
        self.open(dialog, cancel, false)
    }
    /// As `ask`, for a question of the agent's, which the turn waits on (`ToScreen::Dialog`).
    fn open(&self, dialog: Dialog, cancel: Option<&AtomicBool>, asks: bool) -> Option<Answer> {
        if asks {
            crate::agent_cli::live::waiting();
        }
        let answer = self.answer(dialog, cancel, asks);
        if asks {
            crate::agent_cli::live::answered();
        }
        answer
    }
    fn answer(&self, dialog: Dialog, cancel: Option<&AtomicBool>, asks: bool) -> Option<Answer> {
        let (reply, answer) = mpsc::channel();
        self.send(ToScreen::Dialog(dialog, reply, asks));
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
    pub(in crate::agent_cli) fn edit(&self, file: &Path) -> Result<(), String> {
        let (reply, edited) = mpsc::channel();
        self.send(ToScreen::Edit(file.to_path_buf(), reply));
        edited
            .recv()
            .unwrap_or_else(|_| Err("the screen is gone".to_owned()))
    }
    /// Takes away a held list (`Picker::held`) the session no longer shows.
    pub(in crate::agent_cli) fn dismiss(&self) {
        self.send(ToScreen::Dismiss);
    }
    /// The key pressed under `title` and `note`; `None` for Escape.
    pub(in crate::agent_cli) fn press(&self, title: &str, note: &str) -> Option<Press> {
        match self.ask(Dialog::Press(title.to_owned(), note.to_owned()), None) {
            Some(Answer::Pressed(press)) => press,
            _ => None,
        }
    }

    /// The theme editor, opening as `editor` is; the editor as it was saved, `None` when
    /// cancelled.
    pub(in crate::agent_cli) fn edit_theme(
        &self,
        editor: Box<ThemeEditor>,
    ) -> Option<Box<ThemeEditor>> {
        match self.ask(Dialog::Theme(editor), None) {
            Some(Answer::Theme(saved)) => saved,
            _ => None,
        }
    }

    /// Shows a report view, `/usage`'s or `/changelog`'s, until the user closes it.
    pub(in crate::agent_cli) fn show_report(&self, view: ReportView) {
        self.ask(Dialog::Report(Box::new(view)), None);
    }

    /// The row chosen from `picker`, `None` when cancelled.
    pub(in crate::agent_cli) fn pick(&self, picker: Picker) -> Option<usize> {
        self.timed(picker, false).map(|(row, ..)| row)
    }

    /// As `pick`, with where the list was when the row was chosen (`Picker::placing`).
    pub(in crate::agent_cli) fn pick_placed(&self, picker: Picker) -> Option<(usize, Spot)> {
        match self.ask(Dialog::Pick(Box::new(picker.placing())), None) {
            Some(Answer::Placed(row, place)) => Some((row, place)),
            _ => None,
        }
    }

    /// As `pick`, or what one of the list's own keys asked for (`Picker::acting`).
    pub(in crate::agent_cli) fn pick_or_act(&self, picker: Picker) -> Picked {
        match self.ask(Dialog::Pick(Box::new(picker)), None) {
            Some(Answer::Acted(act, row, place)) => Picked::Act(act, row, place),
            Some(Answer::Toggled(toggle, place)) => Picked::Toggle(toggle, place),
            Some(Answer::Tabbed(place)) => Picked::Tab(place),
            Some(Answer::Read(place)) => Picked::Read(place),
            Some(Answer::Picked(row)) => Picked::Row(row),
            _ => Picked::Row(None),
        }
    }

    /// As `pick`, or, when Shift+L asks to label an entry of `/tree`'s list, that entry with the
    /// list as it was, to show again.
    pub(in crate::agent_cli) fn pick_or_label(
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
    /// model and effort chosen in a next step's list (`Picker::routing`): for the agent's next
    /// steps and questions, which the turn waits on.
    pub(in crate::agent_cli) fn pick_timed(&self, picker: Picker) -> Option<(usize, bool, Routed)> {
        self.timed(picker, true)
    }
    fn timed(&self, picker: Picker, asks: bool) -> Option<(usize, bool, Routed)> {
        match self.open(Dialog::Pick(Box::new(picker)), None, asks) {
            Some(Answer::Picked(chosen)) => chosen.map(|row| (row, false, Routed::default())),
            Some(Answer::Routed(row, routed)) => Some((row, false, routed)),
            Some(Answer::Expired(row)) => Some((row, true, Routed::default())),
            _ => None,
        }
    }
}

/// What asks the screen to write its draft as the process exits on a signal (`keep_draft`).
static KEEPING: Mutex<Option<(mpsc::Sender<ToScreen>, PipeWriter)>> = Mutex::new(None);

/// Has the screen, if one is shown, write its draft, waiting `within` at most: the process exits
/// on a signal. A screen handing the terminal to an external editor does not answer.
pub(in crate::agent_cli) fn keep_draft(within: Duration) {
    let keeping = KEEPING.lock().unwrap_or_else(PoisonError::into_inner);
    let Some((to, wake)) = keeping.as_ref() else {
        return;
    };
    let (reply, kept) = mpsc::channel();
    if to.send(ToScreen::KeepDraft(reply)).is_ok() {
        let _ = (&*wake).write(&[0]);
        drop(keeping);
        let _ = kept.recv_timeout(within);
    }
}

/// The screen's thread, and what the user enters from it.
pub(in crate::agent_cli) struct Handle {
    pub(in crate::agent_cli) link: Link,
    from: mpsc::Receiver<Input>,
    /// The screen was told the session waits (`ToScreen::Ready`) and has not answered yet.
    waiting: Cell<bool>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl Handle {
    /// Takes the terminal for the screen, `editor` editing the prompts. Escape and Ctrl-C set
    /// `cancel` while a turn runs; Ctrl+T flips `hide_thinking`. The agent takes the messages
    /// queued during a turn as the `settings` say (`steeringMode`, `followUpMode`), which also
    /// say how Markdown files read show, whether a completed mouse selection is copied and how
    /// the conversation looks; `!` commands run meanwhile run in `root`.
    pub(in crate::agent_cli) fn start(
        editor: Editor,
        hide_thinking: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        settings: &Settings,
        root: PathBuf,
    ) -> io::Result<Self> {
        let modes = (settings.steering_mode, settings.follow_up_mode);
        spelling::configure(settings.spell_check, settings.words().as_deref());
        let (markdown_reads, copy_on_select) = (settings.markdown_reads, settings.copy_on_select);
        let (layout, terminal_progress) = (settings.layout, settings.terminal_progress);
        let notify = settings
            .notifications
            .resolved(|name| std::env::var(name).ok());
        let retain_transcript = settings.retain_transcript;
        let (design, banner, bottom) = (settings.look, settings.banner, settings.bottom);
        let status_line = settings.status_line.clone();
        let (to, messages) = mpsc::channel();
        let (inputs, from) = mpsc::channel();
        let (woken, wake) = io::pipe()?;
        console::nonblocking(&wake)?;
        console::nonblocking(&woken)?;
        let raw = RawMode::enter()?;
        clipboard::warm();
        let queue = Arc::new(Mutex::new(Queue::default()));
        let woke = wake.try_clone()?;
        super::hold_panics();
        let thread = std::thread::Builder::new().name("screen".into()).spawn({
            let queue = queue.clone();
            move || {
                let _stops = StopsTurnOnPanic(cancel.clone());
                let sides = Sides {
                    root,
                    wake: woke,
                    threads: Vec::new(),
                    started: 0,
                };
                let app = App::new(editor, hide_thinking, cancel, inputs, queue, sides);
                let mut app = App {
                    markdown_reads,
                    copy_on_select,
                    layout,
                    retain_transcript,
                    terminal_progress,
                    notify,
                    design,
                    banner,
                    bottom,
                    ..app
                };
                app.status_line.configure(status_line);
                app.run(raw, &messages, woken)
            }
        })?;
        *KEEPING.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((to.clone(), wake.try_clone()?));
        Ok(Self {
            link: Link {
                to,
                wake,
                queue,
                modes,
                call: None,
            },
            from,
            waiting: Cell::new(false),
            thread: Some(thread),
        })
    }

    /// Waits for what the user enters next, or `None` after `timeout`, the wait going on at the
    /// next call. The screen is told once per wait, as a second `Ready` after it answered would
    /// count the session waiting through the turn that answer starts.
    pub(in crate::agent_cli) fn input_within(&self, timeout: Option<Duration>) -> Option<Input> {
        if !self.waiting.replace(true) {
            self.link.send(ToScreen::Ready);
        }
        let input = match timeout {
            None => self.from.recv().unwrap_or(Input::End),
            Some(timeout) => match self.from.recv_timeout(timeout) {
                Ok(input) => input,
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => Input::End,
            },
        };
        self.waiting.set(false);
        Some(input)
    }

    /// Ends a wait `input_within` left going on, the session going on without input.
    pub(in crate::agent_cli) fn stop_waiting(&self) {
        self.waiting.set(false);
    }
}

impl std::ops::Deref for Handle {
    type Target = Link;
    fn deref(&self) -> &Link {
        &self.link
    }
}

impl Handle {
    /// Gives the terminal back as it was, then prints the panics held meanwhile; an error when
    /// the screen failed.
    pub(in crate::agent_cli) fn close(mut self) -> Result<(), String> {
        self.give_back()
    }

    fn give_back(&mut self) -> Result<(), String> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        KEEPING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.link.send(ToScreen::Quit);
        let joined = thread.join();
        let _ = io::stderr().write_all(super::held_panics().as_bytes());
        match joined {
            Ok(Err(error)) => Err(format!("the screen failed: {error}")),
            Err(_) => Err(format!(
                "the screen stopped on a bug: please report it at {ISSUES}"
            )),
            Ok(Ok(())) => Ok(()),
        }
    }
}

const ISSUES: &str = "https://github.com/LeanAndMean/scramjet/issues";

/// Stops the turn running when the screen's thread panics, as Escape would, rather than let it
/// go on with nothing shown.
struct StopsTurnOnPanic(Arc<AtomicBool>);

impl Drop for StopsTurnOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Err(error) = self.give_back() {
            let _ = writeln!(io::stderr(), "Error: {error}");
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
    fn cooling(&mut self, delay: Duration) {
        self.event(Event::Cooling(delay));
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
        self.tool_result_at(name, output, detail, failed, Instant::now());
    }
    fn tool_result_at(
        &mut self,
        name: &str,
        output: &str,
        detail: Option<&ToolDetail>,
        failed: bool,
        at: Instant,
    ) {
        let (name, output, detail) = (name.to_owned(), output.to_owned(), detail.cloned());
        self.event(Event::ToolResult(name, output, detail, failed, at));
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
    fn command_sent(&mut self, typed: &str, sent: &str) {
        self.event(Event::Sent(typed.to_owned(), sent.to_owned()));
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.event(Event::CompactionSummary(summary.to_owned()));
    }
    fn branch_summary(&mut self, summary: &str) {
        self.event(Event::BranchSummary(summary.to_owned()));
    }
    /// As Scramjet's, the question shows until answered; a cancelled one shows no answer.
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> agent::Answer {
        match self.open(Dialog::Approve(question.to_owned()), Some(cancel), true) {
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
        self.queue().messages.steer(self.modes.0)
    }
    fn follow_up(&mut self) -> Vec<String> {
        self.queue().messages.follow_up(self.modes.1)
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
        self.pick_timed(Picker::question(title, options, recommended))
            .map(|(row, ..)| row)
    }
    fn reply(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
        _: bool,
    ) -> Option<Reply> {
        let picker = Picker::question(title, options, recommended).typing();
        match self.open(Dialog::Pick(Box::new(picker)), None, true)? {
            Answer::Picked(row) => row.map(Reply::Chosen),
            Answer::Text(typed) => typed.map(Reply::Typed),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_screen_thread_panicking_stops_the_turn_and_ending_does_not() {
        let cancel = Arc::new(AtomicBool::new(false));
        let ended = std::thread::spawn({
            let cancel = cancel.clone();
            move || drop(StopsTurnOnPanic(cancel))
        });
        assert!(ended.join().is_ok());
        assert!(!cancel.load(Ordering::Relaxed));
        let panicked = std::thread::spawn({
            let cancel = cancel.clone();
            move || {
                let _stops = StopsTurnOnPanic(cancel);
                panic!("a bug in the screen");
            }
        });
        assert!(panicked.join().is_err());
        assert!(cancel.load(Ordering::Relaxed));
    }
}
