//! The interactive session: prompts read from the editor, the full screen or piped lines, the
//! built-in commands (`slash.rs`), model turns and a completed command's next step.
use super::record::Event;
use super::screen::{self, Handle};
use super::settings::{DoubleEscape, Settings};
use super::slash::Builtin;
use super::terminal::{Terminal, usage_line};
use super::{editor, select, style};
use scramjet_rs::agent::{
    Agent, AgentError, AgentUi, Choice, Command, Completed, Next, next_step_record,
};
use std::{
    io::{BufRead, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Interactive {
    pub(super) agent: Agent,
    /// Prints what the session shows without the full screen.
    pub(super) ui: Terminal,
    /// The full screen, when stdin and stderr are terminals: it then shows everything and edits
    /// the prompts. Without it, prompts are read as lines and lists are printed numbered.
    pub(super) screen: Option<Handle>,
    /// Whether what was shown last ends with a blank line, so the next prompt needs none.
    pub(super) spaced: bool,
    /// Set by Ctrl-C: cancels what runs.
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) hide_thinking: Arc<AtomicBool>,
    pub(super) settings: Settings,
}

/// How a prompt came to run.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    Typed,
    /// A next step the user chose from those offered.
    Chosen,
    /// A next step run at once: forced, or an edge chains it.
    Unattended,
}

/// What the user gave at the prompt.
enum Read {
    Prompt(String),
    /// A double Escape erased the prompt for `/tree` or `/fork`, which then takes its place.
    DoubleEscape,
    /// Nothing to run; the prompt is read again.
    Again,
    /// The input ended.
    End,
}

impl Interactive {
    pub(super) fn new(
        agent: Agent,
        ui: Terminal,
        cancel: Arc<AtomicBool>,
        hide_thinking: Arc<AtomicBool>,
        settings: Settings,
    ) -> Result<Self, String> {
        let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        let screen = match terminal {
            true => {
                let editor = editor::Editor::new(
                    agent.prompts().to_vec(),
                    hide_thinking.clone(),
                    agent.root().to_path_buf(),
                    agent.commands().list().iter().map(command_entry).collect(),
                    settings.double_escape != DoubleEscape::Nothing,
                );
                let started = Handle::start(
                    editor,
                    hide_thinking.clone(),
                    cancel.clone(),
                    (settings.steering_mode, settings.follow_up_mode),
                    agent.root().to_path_buf(),
                );
                Some(started.map_err(|error| format!("cannot show the full screen: {error}"))?)
            }
            false => None,
        };
        Ok(Self {
            agent,
            ui,
            screen,
            spaced: false,
            cancel,
            hide_thinking,
            settings,
        })
    }

    /// Reads and runs prompts until the user quits or the input ends.
    pub(super) fn run(mut self) -> Result<(), String> {
        let result = self.read_and_run();
        // The terminal is given back first, so that an error saving the settings shows.
        self.screen = None;
        self.save_hide_thinking();
        result
    }

    fn read_and_run(&mut self) -> Result<(), String> {
        self.say(&format!(
            "scramjet-rs · {} · {}\nsession {}",
            model_line(&self.agent),
            self.agent.root().display(),
            self.agent.session_path().display(),
        ));
        // A tool block ends with a blank line; other output gets one before the prompt.
        self.show_resumed();
        let warnings: Vec<_> = (self.settings.warnings.iter())
            .map(|warning| format!("settings: {warning}"))
            .chain(
                (self.agent.commands().warnings().iter())
                    .map(|warning| format!("scramjet: {warning}")),
            )
            .collect();
        if !warnings.is_empty() {
            if !std::mem::take(&mut self.spaced) {
                self.blank();
            }
            for warning in &warnings {
                self.ui().notice(warning);
            }
            self.spaced = true;
        }
        // A completed command's next step, run as if typed.
        let mut queued: Option<(String, Start)> = None;
        loop {
            self.save_hide_thinking();
            self.typed_during_turn(false);
            let (dispatched, start) = queued.take().unzip();
            let prompt = match self.read(dispatched)? {
                Read::Prompt(prompt) => prompt,
                Read::DoubleEscape => match self.settings.double_escape {
                    DoubleEscape::Fork => "/fork".to_owned(),
                    DoubleEscape::Tree | DoubleEscape::Nothing => "/tree".to_owned(),
                },
                Read::Again => continue,
                Read::End => return Ok(()),
            };
            let prompt = prompt.trim();
            if prompt.is_empty() {
                continue;
            }
            if Builtin::parse(prompt) == Some(Builtin::Quit) {
                return Ok(());
            }
            if !self.builtin(prompt) {
                queued = self.send(prompt, start.unwrap_or(Start::Typed));
            }
        }
    }

    /// The next prompt: `dispatched`, a next step to run as if typed, or what the user enters.
    fn read(&mut self, dispatched: Option<String>) -> Result<Read, String> {
        if let Some(screen) = &mut self.screen {
            // Those that ended during the turn go before the next prompt, as Scramjet adds them
            // when the turn ends.
            add_shells(&mut self.agent, screen);
            if let Some(prompt) = dispatched {
                return Ok(Read::Prompt(prompt));
            }
            loop {
                match screen.input() {
                    screen::Input::Prompt(prompt) => return Ok(Read::Prompt(prompt)),
                    screen::Input::DoubleEscape => return Ok(Read::DoubleEscape),
                    screen::Input::CycleEffort => {
                        screen.note(cycle_effort(&mut self.agent, &mut self.settings));
                    }
                    screen::Input::Shells => add_shells(&mut self.agent, screen),
                    screen::Input::Commands => return Ok(Read::Again),
                    screen::Input::End => return Ok(Read::End),
                }
            }
        }
        let prompt = match dispatched {
            Some(prompt) => {
                if !std::mem::take(&mut self.spaced) {
                    eprintln!();
                }
                self.ui.user_prompt(&prompt);
                prompt
            }
            None => match read_lines()? {
                Some(prompt) => prompt,
                None => return Ok(Read::End),
            },
        };
        Ok(Read::Prompt(prompt))
    }

    /// Runs `prompt` as a model turn. Returns the next step to run, when the turn's command
    /// completed with one.
    fn send(&mut self, prompt: &str, start: Start) -> Option<(String, Start)> {
        if let Some(screen) = &self.screen {
            screen.event(match self.agent.commands().expand(prompt) {
                Some(sent) => Event::CommandPrompt(prompt.to_owned(), sent),
                None => Event::UserPrompt(prompt.to_owned()),
            });
        }
        let hide = (self.settings.hide_credentials).hides(start == Start::Unattended);
        self.agent.set_hide_credentials(hide);
        let result = match self.screen.is_some() {
            true => self.turn_on_screen(prompt),
            false => self.during_turn(|agent, ui, cancel| agent.turn(prompt, ui, cancel)),
        };
        match result {
            Ok(()) => {}
            Err(AgentError::Cancelled) => self.say(&self.ui.dim("(cancelled)")),
            Err(error) => self.say(&format!("error: {error}")),
        }
        self.say_styled(style::MUTED, &usage_line(&self.agent));
        let completed = self.agent.take_completed()?;
        self.next_step(&completed)
    }

    /// Runs `prompt` as a model turn on the full screen, as `on_screen` does. Where the turn
    /// pauses, the built-in commands typed meanwhile that do not stop it run, and it goes on;
    /// Escape or Ctrl-C meanwhile stop it.
    fn turn_on_screen(&mut self, prompt: &str) -> Result<(), AgentError> {
        self.cancel.store(false, Ordering::Relaxed);
        let effort = self.agent.effort().map(str::to_owned);
        let mut result = self.on_screen(|agent, ui, cancel| agent.turn(prompt, ui, cancel));
        while let Err(AgentError::Paused) = result {
            self.typed_during_turn(true);
            result = self.on_screen(Agent::resume_turn);
        }
        self.ended_on_screen(effort);
        result
    }

    /// Runs `run` with the full screen showing that a turn runs, Shift+Tab cycling the current
    /// model's levels; without it, printed.
    fn on_screen<T>(
        &mut self,
        run: impl FnOnce(&mut Agent, &mut dyn AgentUi, &AtomicBool) -> T,
    ) -> T {
        match &mut self.screen {
            Some(screen) => {
                screen.turn(Some(self.agent.effort_cycle()));
                run(&mut self.agent, &mut screen.link, &self.cancel)
            }
            None => run(&mut self.agent, &mut self.ui, &self.cancel),
        }
    }

    /// Shows on the full screen that what `on_screen` ran has ended. Sets an effort Shift+Tab
    /// chose meanwhile that no request took, and saves the effort as the settings' default if
    /// it changed from `effort`.
    fn ended_on_screen(&mut self, effort: Option<String>) {
        let Some(screen) = &self.screen else {
            return;
        };
        screen.turn(None);
        self.cancel.store(false, Ordering::Relaxed);
        let unset = (screen.take_effort()).and_then(|level| self.agent.set_effort(level).err());
        let changed = (self.agent.effort()).filter(|level| effort.as_deref() != Some(level));
        let unsaved =
            changed.and_then(|level| self.settings.set_default_thinking_level(level).err());
        if let Some(error) = unset {
            self.say(&format!("error: {error}"));
        }
        if let Some(error) = unsaved {
            self.say(&format!("error: settings not saved: {error}"));
        }
    }

    /// Runs the built-in commands typed in the full screen while a turn ran, in order;
    /// `paused`: the turn paused for them, and those from the first that stops it on wait for
    /// it to end.
    fn typed_during_turn(&mut self, paused: bool) {
        let Some(screen) = &self.screen else {
            return;
        };
        for command in screen.take_commands(paused) {
            self.builtin(&command);
        }
    }

    /// Runs a model turn, `/compact`, a branch summary or a `!` command; a Ctrl-C from before it
    /// starts does not cancel it. Full screen, an effort that Shift+Tab chose meanwhile is set if
    /// no request took it, and saved as the settings' default.
    pub(super) fn during_turn<T>(
        &mut self,
        run: impl FnOnce(&mut Agent, &mut dyn AgentUi, &AtomicBool) -> T,
    ) -> T {
        self.cancel.store(false, Ordering::Relaxed);
        if self.screen.is_some() {
            let effort = self.agent.effort().map(str::to_owned);
            let result = self.on_screen(run);
            self.ended_on_screen(effort);
            return result;
        }
        let result = run(&mut self.agent, &mut self.ui, &self.cancel);
        self.ui.clear();
        self.cancel.store(false, Ordering::Relaxed);
        result
    }

    /// The next step of a completed command, as Scramjet routes it with autopilot off: an edge
    /// set to `chain` or a forced step runs at once; otherwise the user chooses from the steps
    /// the model offered. Returns the prompt to run; a text step goes into the editor instead.
    fn next_step(&mut self, completed: &Completed) -> Option<(String, Start)> {
        let routing = self.agent.route(completed, self.interactive());
        if routing.notices.is_empty() && routing.next == Next::Nothing {
            return None;
        }
        // Under the usage line, as a prompt would be.
        self.blank();
        for notice in &routing.notices {
            self.ui().notice(notice);
        }
        let (step, start) = match routing.next {
            Next::Nothing => {
                self.spaced = true;
                return None;
            }
            Next::Run(step) => {
                let record =
                    next_step_record(&routing.source, std::slice::from_ref(&step), Some(0));
                self.show_record(&record);
                (step, Start::Unattended)
            }
            Next::Offer { steps, recommended } => {
                let choices: Vec<_> = steps
                    .iter()
                    .map(|step| Choice {
                        label: step.label(),
                        description: Some(step.description()).filter(|text| !text.is_empty()),
                        value: String::new(),
                    })
                    .collect();
                let chosen = (self.screen.as_ref()).and_then(|screen| {
                    screen.pick(select::Picker::question(
                        "Select next step",
                        &choices,
                        recommended,
                    ))
                });
                self.show_record(&next_step_record(&routing.source, &steps, chosen));
                (steps[chosen?].clone(), Start::Chosen)
            }
        };
        let Some(command) = step.command else {
            self.set_text(step.message);
            return None;
        };
        if step.fresh_session && !self.new_session() {
            return None;
        }
        Some((command, start))
    }

    /// Saves Ctrl+T's choice, made at the prompt or in a turn, when it changed.
    fn save_hide_thinking(&mut self) {
        let hide = self.hide_thinking.load(Ordering::Relaxed);
        if let Err(error) = self.settings.set_hide_thinking_block(hide) {
            self.say(&format!("error: settings not saved: {error}"));
        }
    }

    fn show_record(&self, record: &str) {
        self.say_styled(style::NEXT_STEP, record);
    }

    /// Whether the user can choose from lists and answer questions as they show.
    pub(super) fn interactive(&self) -> bool {
        self.screen.is_some()
    }

    /// Shows a message, on its own line or, full screen, as a block.
    pub(super) fn say(&self, text: &str) {
        match &self.screen {
            Some(screen) => screen.say(text),
            None => eprintln!("{text}"),
        }
    }

    /// Shows a message in `style`, wrapped to the terminal's width.
    pub(super) fn say_styled(&self, style: &str, text: &str) {
        match &self.screen {
            Some(screen) => screen.say(&self.ui.styled(style, text)),
            None => eprintln!("{}", self.ui.styled(style, &self.ui.wrapped(text))),
        }
    }

    /// A blank line, where printed output needs one; the full screen spaces blocks itself.
    pub(super) fn blank(&self) {
        if self.screen.is_none() {
            eprintln!();
        }
    }

    /// Where the agent shows what it does outside a turn.
    pub(super) fn ui(&mut self) -> &mut dyn AgentUi {
        match &mut self.screen {
            Some(screen) => &mut screen.link,
            None => &mut self.ui,
        }
    }

    /// Starts the next prompt with `text`.
    pub(super) fn set_text(&mut self, text: String) {
        if let Some(screen) = &self.screen {
            screen.set_text(text);
        }
    }

    /// Makes the session's prompts those that Up recalls.
    pub(super) fn remember_prompts(&mut self) {
        if let Some(screen) = &self.screen {
            for prompt in self.agent.prompts() {
                screen.remember(prompt);
            }
        }
    }

    /// Chooses one of `count` rows on the full screen, as `select::Picker::select`; `None`
    /// without it.
    pub(super) fn select(
        &self,
        title: &str,
        visible: usize,
        count: usize,
        initial: usize,
        current: Option<usize>,
        row: impl Fn(usize, usize) -> String,
    ) -> Option<usize> {
        let picker = select::Picker::select(title, visible, count, initial, current, row);
        self.screen.as_ref()?.pick(picker)
    }

    /// As `select`, searching as `select::Picker::search`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn search(
        &self,
        title: &str,
        visible: usize,
        texts: &[String],
        initial: usize,
        current: Option<usize>,
        none: &str,
        row: impl Fn(usize, usize) -> String,
    ) -> Option<usize> {
        let picker = select::Picker::search(title, visible, texts, initial, current, none, row);
        self.screen.as_ref()?.pick(picker)
    }

    /// Shows that the session was replaced, with `message`, then its conversation. Full screen,
    /// the conversation before goes and the message follows the new one.
    pub(super) fn replaced(&mut self, message: &str) {
        match &self.screen {
            Some(screen) => screen.clear(),
            None => self.say(message),
        }
        self.show_resumed();
        if self.screen.is_some() {
            self.say(message);
        }
    }

    /// Shows a resumed conversation under its header, as Scramjet redraws it; it then ends with
    /// a blank line.
    pub(super) fn show_resumed(&mut self) {
        if let Some(screen) = &mut self.screen {
            self.agent.show_conversation(&mut screen.link);
            return;
        }
        self.spaced = !self.agent.conversation().is_empty();
        if self.spaced {
            eprintln!();
            self.agent.show_conversation(&mut self.ui);
            self.ui.clear();
        }
    }
}

/// Adds the `!` commands that ended while a turn ran to the conversation, shown as they ran.
fn add_shells(agent: &mut Agent, screen: &mut Handle) {
    for shell in screen.take_shells() {
        screen.link.tool_call("bash", shell.command());
        if let Err(error) = agent.add_shell(shell, &mut screen.link) {
            screen.say(&format!("error: {error}"));
        }
    }
}

/// A loaded command's row in the `/` list: the argument hint before the description, as
/// Scramjet's autocomplete shows them.
fn command_entry(command: &Command) -> (String, String) {
    let description = match (&command.argument_hint, command.description.as_str()) {
        (Some(hint), "") => hint.clone(),
        (Some(hint), description) => format!("{hint} — {description}"),
        (None, description) => description.to_owned(),
    };
    (command.name.clone(), description)
}

/// The model, and the reasoning effort when one is set, as Scramjet's footer shows them.
pub(super) fn model_line(agent: &Agent) -> String {
    match agent.effort() {
        Some(effort) => format!("{} · thinking {effort}", agent.model()),
        None => agent.model().to_owned(),
    }
}

/// Shift+Tab: the next effort level, saved as the settings' default; returns what to say.
fn cycle_effort(agent: &mut Agent, settings: &mut Settings) -> String {
    let cycled = agent.cycle_effort();
    let unsaved = match &cycled {
        Ok(Some(level)) => settings.set_default_thinking_level(level).err(),
        _ => None,
    };
    let note = effort_note(cycled);
    match unsaved {
        Some(error) => format!("{note} (settings not saved: {error})"),
        None => note,
    }
}

/// What a Shift+Tab says under the prompt, as Scramjet reports it.
pub(super) fn effort_note(cycled: Result<Option<String>, AgentError>) -> String {
    match cycled {
        Ok(Some(level)) => format!("Thinking level: {level}"),
        Ok(None) => "Current model does not support thinking".to_owned(),
        Err(error) => format!("error: {error}"),
    }
}

/// Reads a prompt from non-terminal input, where a line ending in \ continues on the next line.
fn read_lines() -> Result<Option<String>, String> {
    let mut prompt = String::new();
    loop {
        eprint!("{}", if prompt.is_empty() { "\n> " } else { ". " });
        let mut line = String::new();
        if std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\n', '\r']);
        if let Some(start) = line.strip_suffix('\\') {
            prompt.push_str(start);
            prompt.push('\n');
        } else {
            prompt.push_str(line);
            return Ok(Some(prompt));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_note_says_as_scramjet_does_when_no_levels() {
        assert_eq!(
            effort_note(Ok(None)),
            "Current model does not support thinking"
        );
        assert_eq!(
            effort_note(Ok(Some("high".to_owned()))),
            "Thinking level: high"
        );
    }
}
