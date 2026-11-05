//! The session's side of the screen: what it sends the screen's thread, the agent's
//! interface during a turn, and the `Handle` that starts the thread and reads the user's input.

use super::{Answer, App, Dialog, Input, Queue, Sides, ToScreen, stops_turn};
use crate::agent_cli::clipboard;
use crate::agent_cli::editor::{Editor, Note};
use crate::agent_cli::footer;
use crate::agent_cli::keys::RawMode;
use crate::agent_cli::record::Event;
use crate::agent_cli::select::{Picked, Picker, Routed};
use crate::agent_cli::settings::{Delivery, DoubleEscape, MarkdownReads, Settings};
use crate::agent_cli::style::Style;
use scramjet_rs::agent::{
    self, AgentError, AgentUi, AutoCompaction, Children, Choice, CompactRead, Compaction,
    EffortCycle, Purpose, Shell, StreamProgress, Tokens, ToolDetail, UserCommand,
};
use std::{
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

/// The messages `delivery` gives the agent at once from `queued`.
fn deliver(queued: &mut Vec<String>, delivery: Delivery) -> Vec<String> {
    match delivery {
        Delivery::All => std::mem::take(queued),
        Delivery::OneAtATime if queued.is_empty() => Vec::new(),
        Delivery::OneAtATime => vec![queued.remove(0)],
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
    fn queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
    fn send(&self, message: ToScreen) {
        if self.to.send(message).is_ok() {
            // A full pipe already wakes the screen.
            let _ = (&self.wake).write(&[0]);
        }
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
    pub(in crate::agent_cli) fn turn(&self, efforts: Option<EffortCycle>) {
        self.send(ToScreen::Turn(efforts));
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
    /// What `/reload` read: the commands and skills the `/` list shows, and the `settings`.
    pub(in crate::agent_cli) fn reloaded(
        &mut self,
        commands: Vec<(String, String)>,
        settings: &Settings,
    ) {
        self.modes = (settings.steering_mode, settings.follow_up_mode);
        self.send(ToScreen::Reloaded {
            commands,
            double_escape: settings.double_escape != DoubleEscape::Nothing,
            markdown_reads: settings.markdown_reads,
            copy_on_select: settings.copy_on_select,
        });
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
    /// The row chosen from `picker`, `None` when cancelled.
    pub(in crate::agent_cli) fn pick(&self, picker: Picker) -> Option<usize> {
        self.pick_timed(picker).map(|(row, ..)| row)
    }

    /// As `pick`, or what one of the list's own keys asked for (`Picker::acting`).
    pub(in crate::agent_cli) fn pick_or_act(&self, picker: Picker) -> Picked {
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
    /// model and effort chosen in a next step's list (`Picker::routing`).
    pub(in crate::agent_cli) fn pick_timed(&self, picker: Picker) -> Option<(usize, bool, Routed)> {
        match self.ask(Dialog::Pick(Box::new(picker)), None) {
            Some(Answer::Picked(chosen)) => chosen.map(|row| (row, false, Routed::default())),
            Some(Answer::Routed(row, routed)) => Some((row, false, routed)),
            Some(Answer::Expired(row)) => Some((row, true, Routed::default())),
            _ => None,
        }
    }
}

/// The screen's thread, and what the user enters from it.
pub(in crate::agent_cli) struct Handle {
    pub(in crate::agent_cli) link: Link,
    from: mpsc::Receiver<Input>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl Handle {
    /// Takes the terminal for the screen, `editor` editing the prompts. Escape and Ctrl-C set
    /// `cancel` while a turn runs; Ctrl+T flips `hide_thinking`. The agent takes the messages
    /// queued during a turn as `modes` (`steeringMode`, `followUpMode`) say; `!` commands run
    /// meanwhile run in `root`. Markdown files read show as `markdown_reads` says, and a
    /// completed mouse selection is copied if `copy_on_select`.
    pub(in crate::agent_cli) fn start(
        editor: Editor,
        hide_thinking: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        modes: (Delivery, Delivery),
        markdown_reads: MarkdownReads,
        copy_on_select: bool,
        root: PathBuf,
    ) -> io::Result<Self> {
        let (to, messages) = mpsc::channel();
        let (inputs, from) = mpsc::channel();
        let (woken, wake) = io::pipe()?;
        rustix::fs::fcntl_setfl(&wake, rustix::fs::OFlags::NONBLOCK)?;
        rustix::fs::fcntl_setfl(&woken, rustix::fs::OFlags::NONBLOCK)?;
        let raw = RawMode::enter()?;
        clipboard::warm();
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
                    copy_on_select,
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
    pub(in crate::agent_cli) fn input(&self) -> Input {
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
            Some(Ok(Err(error))) => eprintln!("Error: {error}"),
            Some(Err(_)) => eprintln!("Error: the screen failed"),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
