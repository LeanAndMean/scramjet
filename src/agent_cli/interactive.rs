//! The interactive session: prompts read from the editor, the full screen or piped lines, the
//! built-in commands (`slash.rs`), model turns and a completed command's next step.
use super::banner::{self, Opening};
use super::editor::Note;
use super::record::Event;
use super::screen::{self, Handle};
use super::select::Routed;
use super::settings::{DoubleEscape, Settings};
use super::slash::Builtin;
use super::terminal::{Terminal, usage_line};
use super::theme::{ThemeOptions, Themes, Watched};
use super::{editor, footer, installed, keymap, scope, select, settings, style};
use scramjet_rs::agent::{
    Agent, AgentError, AgentUi, Attached, Choice, Command, Completed, Installed, Next, Problem,
    Source, Step, Suggested,
};
use std::{
    io::{BufRead, IsTerminal},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// How long `/autopilot on` waits before it chooses the recommended next step, as Scramjet.
const AUTOSELECT: Duration = Duration::from_secs(3);
/// How often the full screen, waiting for a prompt, tries again to write the lines a full disk
/// kept the session from writing.
const RETRY_HELD: Duration = Duration::from_secs(5);

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
    /// The themes `/settings` offers, found at start, by `/reload` and as its list opens.
    pub(super) themes: Themes,
    pub(super) theme_options: ThemeOptions,
    /// What is wrong in the keybindings file, read at start and by `/reload` (`keymap::load`).
    pub(super) keybindings: Vec<Problem>,
    /// The patterns of the models Ctrl+P cycles through (`scope::resolve`); empty: all.
    pub(super) scope: Vec<String>,
    /// A turn waits for the built-in commands typed during it (`typed_during_turn`).
    pub(super) in_turn: bool,
    /// The packages' logins and the providers that list their models themselves, as the
    /// session last read them, for those lists to be kept current (`login::keep_lists_current`).
    pub(super) kept: Arc<Mutex<super::login::Kept>>,
}

/// How a prompt came to run.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    Typed,
    /// A next step the user chose from those offered.
    Chosen,
    /// A next step run at once (forced, an edge chains it, or `/autopilot on` without a list),
    /// or one `/autopilot on` chose when the user did not.
    Unattended,
}

/// How the session ended.
enum Ended {
    Quit,
    /// `--resume` resumed no session.
    NothingResumed,
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
        (themes, theme_options): (Themes, ThemeOptions),
    ) -> Result<Self, String> {
        let keybindings = keymap::load(settings.path());
        let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
        let kept = Arc::new(Mutex::new(super::login::Kept::of(agent.providers())));
        let screen = match terminal {
            true => {
                let editor = editor::Editor::new(
                    agent.prompts().to_vec(),
                    hide_thinking.clone(),
                    agent.root().to_path_buf(),
                    completions(&agent),
                    agent.models().unwrap_or_default(),
                    settings.double_escape != DoubleEscape::Nothing,
                );
                let started = Handle::start(
                    editor,
                    hide_thinking.clone(),
                    cancel.clone(),
                    &settings,
                    agent.root().to_path_buf(),
                );
                let screen =
                    started.map_err(|error| format!("cannot show the full screen: {error}"))?;
                if let Ok(relisting) = screen.relisting() {
                    super::login::keep_lists_current(relisting, kept.clone());
                }
                Some(screen)
            }
            false => None,
        };
        let interactive = Self {
            agent,
            ui,
            screen,
            spaced: false,
            cancel,
            hide_thinking,
            settings,
            themes,
            theme_options,
            keybindings,
            scope: Vec::new(),
            in_turn: false,
            kept,
        };
        interactive.use_theme();
        Ok(interactive)
    }

    /// The themes found now, with the settings' and the command line's.
    pub(super) fn discover_themes(&mut self) {
        let (root, listed) = (self.agent.root(), &self.settings.themes);
        self.themes = Themes::discover(root, listed, &self.theme_options);
    }

    /// Shows everything in the theme the settings name, and has the screen read its file again
    /// when it changes; without one, in the theme that follows the terminal's background, which
    /// the screen changes as the terminal answers.
    pub(super) fn use_theme(&self) {
        let name = self.settings.theme_name();
        style::set(&self.themes.theme(name));
        if let Some(screen) = &self.screen {
            let watched = self.themes.get(name).and_then(Watched::new);
            (screen.link).watch_theme(watched, self.settings.theme.is_none());
        }
    }

    /// Reads and runs prompts until the user quits or the input ends; with `resume`, after
    /// choosing a session to resume (`--resume`), unless none is chosen on the full screen, which
    /// ends the session. The command line's `messages` run first, as if typed, the first with its
    /// `files`. `notices` are shown at the start, with the problems found. A session file left
    /// without a conversation is removed at the end (`Agent::discard_unused_session`).
    pub(super) fn run(
        mut self,
        resume: bool,
        messages: Vec<String>,
        files: Option<Attached>,
        notices: Vec<String>,
    ) -> Result<(), String> {
        let result = self.read_and_run(resume, messages, files, notices);
        // The terminal is given back first, so that an error saving the settings shows.
        self.screen = None;
        self.save_hide_thinking();
        if let Ok(Ended::NothingResumed) = result {
            eprintln!("{}", self.ui.dim("No session selected"));
        }
        // As Scramjet writes no session before the first reply: `--continue` then passes over it.
        let discarded = self.agent.discard_unused_session();
        result?;
        discarded.map_err(|e| e.to_string())
    }

    fn read_and_run(
        &mut self,
        resume: bool,
        messages: Vec<String>,
        files: Option<Attached>,
        mut notices: Vec<String>,
    ) -> Result<Ended, String> {
        // As Scramjet's `--resume` shows the sessions before the session itself.
        let resumed = match resume && self.interactive() {
            true if self.resume("") => true,
            true => return Ok(Ended::NothingResumed),
            false => false,
        };
        if !resumed {
            let opening = self.opening();
            match &self.screen {
                Some(screen) => screen.event(Event::Banner(Box::new(opening))),
                None => self.say(&banner::plain(&opening)),
            }
            // A tool block ends with a blank line; other output gets one before the prompt.
            self.show_resumed();
        }
        notices.splice(0..0, self.problem_notices());
        if !notices.is_empty() {
            if !std::mem::take(&mut self.spaced) {
                self.blank();
            }
            for notice in &notices {
                self.ui().notice(notice);
            }
            self.spaced = true;
        }
        if resume && !resumed {
            self.resume("");
        }
        let mut messages = messages.into_iter();
        if let Some(files) = files {
            self.agent.attach(files);
        }
        // A completed command's next step, run as if typed.
        let mut queued: Option<(String, Start)> = None;
        loop {
            self.save_hide_thinking();
            self.typed_during_turn(false);
            let message = || messages.next().map(|message| (message, Start::Typed));
            let (dispatched, start) = queued.take().or_else(message).unzip();
            let prompt = match self.read(dispatched)? {
                Read::Prompt(prompt) => prompt,
                Read::DoubleEscape => match self.settings.double_escape {
                    DoubleEscape::Fork => "/fork".to_owned(),
                    DoubleEscape::Tree | DoubleEscape::Nothing => "/tree".to_owned(),
                },
                Read::Again => continue,
                Read::End => return Ok(Ended::Quit),
            };
            // Sent as it is, and never a command, as the files' text comes first.
            if self.agent.attached(&prompt).is_some() {
                queued = self.send(&prompt, Start::Typed);
                continue;
            }
            let prompt = prompt.trim();
            if prompt.is_empty() {
                continue;
            }
            if Builtin::parse(prompt) == Some(Builtin::Exit) {
                return Ok(Ended::Quit);
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
                screen.footer(footer::Shown::of(&self.agent));
                let kept = super::login::Kept::of(self.agent.providers());
                *self.kept.lock().unwrap_or_else(PoisonError::into_inner) = kept;
                let holding = self.agent.session_holding().then_some(RETRY_HELD);
                let Some(input) = screen.input_within(holding) else {
                    if let Some(notice) = self.agent.write_held() {
                        screen.say(&notice);
                    }
                    continue;
                };
                match input {
                    screen::Input::Prompt(prompt) => return Ok(Read::Prompt(prompt)),
                    screen::Input::DoubleEscape => return Ok(Read::DoubleEscape),
                    screen::Input::CycleEffort => {
                        if let Some(note) = cycle_effort(&mut self.agent, &mut self.settings) {
                            screen.note(note);
                        }
                    }
                    screen::Input::CycleModel(steps) => {
                        let (agent, settings) = (&mut self.agent, &mut self.settings);
                        if let Some(note) = scope::cycle(agent, settings, &self.scope, steps) {
                            screen.note(note);
                        }
                    }
                    screen::Input::Shells => add_shells(&mut self.agent, screen),
                    screen::Input::Commands => return Ok(Read::Again),
                    screen::Input::Relisted => {
                        let notes = screen.take_relisted().unwrap_or_default();
                        self.agent.load_model_info();
                        screen.models(self.agent.models().unwrap_or_default());
                        if !notes.is_empty() {
                            screen.say(&notes.join("\n"));
                        }
                    }
                    screen::Input::End => return Ok(Read::End),
                }
            }
        }
        let prompt = match dispatched {
            Some(prompt) => {
                if !std::mem::take(&mut self.spaced) {
                    eprintln!();
                }
                match self.agent.attached(&prompt) {
                    Some((typed, sent)) => self.ui.command_prompt(&typed, &sent),
                    None => self.ui.user_prompt(&prompt),
                }
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
    /// completed with one, or the user chose one the agent suggested.
    fn send(&mut self, prompt: &str, start: Start) -> Option<(String, Start)> {
        if let Some(screen) = &self.screen {
            screen.event(
                match (self.agent.attached(prompt), self.agent.expand(prompt)) {
                    (Some((typed, sent)), _) => Event::CommandPrompt(typed, sent),
                    (None, Some(sent)) => Event::CommandPrompt(prompt.to_owned(), sent),
                    (None, None) => Event::UserPrompt(prompt.to_owned()),
                },
            );
        }
        let hide = (self.settings.hide_credentials).hides(start == Start::Unattended);
        self.agent.set_hide_credentials(hide);
        let model = self.agent.model().to_owned();
        let result = match self.screen.is_some() {
            true => self.turn_on_screen(prompt),
            false => self.during_turn(|agent, ui, cancel| agent.turn(prompt, ui, cancel)),
        };
        // The agent's `switch_scramjet_model`, remembered as the user's choices are.
        if self.agent.model() != model {
            self.remember_model();
        }
        if let Err(error) = result {
            // As the session saved it, for a resume to show it so.
            let error = match error {
                AgentError::Cancelled => None,
                error => Some(error.to_string()),
            };
            match &self.screen {
                Some(screen) => screen.event(Event::Stopped(error)),
                None => self.ui.stopped(error.as_deref()),
            }
        }
        // The full screen's footer shows the same.
        if self.screen.is_none() {
            self.say_styled(style::MUTED, &usage_line(&self.agent));
        }
        match (self.agent.take_completed(), self.agent.take_suggestion()) {
            (Some(completed), _) => self.next_step(&completed),
            (None, Some(suggested)) => self.suggested(suggested),
            (None, None) => None,
        }
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
                screen.footer(footer::Shown::of(&self.agent));
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
            self.say(&format!("Error: {error}"));
        }
        if let Some(error) = unsaved {
            self.say(&format!("Error: settings not saved: {error}"));
        }
    }

    /// Runs the built-in commands typed in the full screen while a turn ran, in order;
    /// `paused`: the turn paused for them, and those from the first that stops it on wait for
    /// it to end. Then the model changes as the Ctrl+P presses meanwhile ask.
    fn typed_during_turn(&mut self, paused: bool) {
        let Some(screen) = &self.screen else {
            return;
        };
        let (commands, steps) = (screen.take_commands(paused), screen.take_model_steps());
        // What the turn left running goes on showing while a command's list is open.
        let shown = match (paused, screen.link.try_clone()) {
            (true, Ok(link)) => self.agent.show_paused(Box::new(link)),
            _ => None,
        };
        self.in_turn = paused;
        for command in commands {
            self.builtin(&command);
        }
        self.in_turn = false;
        if steps != 0 {
            let note = scope::cycle(&mut self.agent, &mut self.settings, &self.scope, steps);
            if let (Some(screen), Some(note)) = (&self.screen, note) {
                screen.note(note);
            }
        }
        if let Some(shown) = shown {
            self.agent.take_paused(shown);
        }
        if let Some(screen) = &self.screen {
            screen.footer(footer::Shown::of(&self.agent));
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

    /// The next step of a completed command, as Scramjet routes it: an edge set to `chain` or a
    /// forced step runs at once; otherwise the user chooses from the steps the model offered,
    /// and with `/autopilot on` the recommended command is chosen unless the user acts within
    /// `AUTOSELECT`. Without a list, `/autopilot on` runs the recommended command at once.
    /// Returns the prompt to run; a text step goes into the editor instead.
    fn next_step(&mut self, completed: &Completed) -> Option<(String, Start)> {
        let autopilot = self.agent.autopilot();
        let routing = self.agent.route(completed, self.interactive(), autopilot);
        if routing.notices.is_empty() && routing.next == Next::Nothing {
            return None;
        }
        // Under the usage line, as a prompt would be.
        self.blank();
        for notice in &routing.notices {
            self.ui().notice(notice);
        }
        let source = Source::Completion(&routing.source);
        let (step, start, routed) = match routing.next {
            Next::Nothing => {
                self.spaced = true;
                return None;
            }
            Next::Run { steps, chosen } => {
                self.record(source, &steps, Some(chosen));
                (steps[chosen].clone(), Start::Unattended, Routed::default())
            }
            Next::Offer {
                steps,
                recommended,
                autoselect,
            } => self.offer("Select next step", source, &steps, recommended, autoselect)?,
        };
        self.take_step(step, start, routed)
    }

    /// The step the agent suggested that the user chooses, under Scramjet's title for them, from
    /// a list that `/autopilot on` never chooses from by itself, as in Scramjet.
    fn suggested(&mut self, suggested: Suggested) -> Option<(String, Start)> {
        let (steps, recommended) = (&suggested.steps, suggested.recommended);
        let title = "Agent suggests a next step";
        let (step, start, routed) =
            self.offer(title, Source::Suggestion, steps, recommended, false)?;
        self.take_step(step, start, routed)
    }

    /// Lets the user choose one of `steps` from `source` under `title`, `recommended` selected
    /// at first, and the model and effort it runs with; with `autoselect`, the recommended one
    /// is chosen unless the user acts within `AUTOSELECT`. Records the choice, or that the list
    /// was dismissed.
    fn offer(
        &mut self,
        title: &str,
        source: Source,
        steps: &[Step],
        recommended: Option<usize>,
        autoselect: bool,
    ) -> Option<(Step, Start, Routed)> {
        let choices: Vec<_> = steps
            .iter()
            .map(|step| Choice {
                label: step.label(),
                description: Some(step.description()).filter(|text| !text.is_empty()),
                value: String::new(),
            })
            .collect();
        let mut picker = select::Picker::question(title, &choices, recommended);
        if self.screen.is_some() {
            let models = self.agent.model_options();
            let current = (models.iter()).position(|m| m.reference() == self.agent.model());
            let effort = self.agent.effort().map(str::to_owned);
            let fresh = (steps.iter())
                .map(|step| step.command.is_some().then_some(step.fresh_session))
                .collect();
            let route = select::Route::new(models, current.unwrap_or(0), effort);
            picker = picker.routing(route.fresh_sessions(fresh));
        }
        // After the models were read, which the countdown does not wait for.
        if autoselect {
            picker = picker.counting_down(AUTOSELECT);
        }
        let chosen = (self.screen.as_ref()).and_then(|screen| screen.pick_timed(picker));
        let mut steps = steps.to_vec();
        if let Some((at, _, routed)) = &chosen
            && let Some(fresh) = routed.fresh
        {
            steps[*at].fresh_session = fresh;
        }
        self.record(source, &steps, chosen.as_ref().map(|(at, ..)| *at));
        let (at, timed, mut routed) = chosen?;
        routed.fresh = None;
        let start = if timed {
            Start::Unattended
        } else {
            Start::Chosen
        };
        Some((steps[at].clone(), start, routed))
    }

    /// Takes a next step with the model and effort `routed` chose: returns its command to run,
    /// after starting a new session when it asks for one; a text step goes into the editor.
    fn take_step(&mut self, step: Step, start: Start, routed: Routed) -> Option<(String, Start)> {
        let Some(command) = step.command else {
            self.take_route(routed);
            self.set_text(step.message);
            return None;
        };
        if step.fresh_session && !self.new_session() {
            return None;
        }
        self.take_route(routed);
        Some((command, start))
    }

    /// Sets the model and effort chosen for a next step, in the session that runs it; the model
    /// is remembered as `/model` remembers one, and the effort saved as the settings' default, as
    /// Shift+Tab saves it. A model that cannot be set leaves the step to run with the current
    /// one, as in Scramjet.
    fn take_route(&mut self, routed: Routed) {
        if routed == Routed::default() {
            return;
        }
        if let Some(model) = &routed.model {
            match self.agent.set_model(model) {
                Ok(()) => {
                    self.agent.load_model_info();
                    self.remember_model();
                }
                Err(error) => self.say(&format!("Error: model not set to {model}: {error}")),
            }
        }
        if let Some(level) = &routed.effort {
            if let Err(error) = self.agent.set_effort(level) {
                self.say(&format!("Error: {error}"));
            } else if let Err(error) = self.settings.set_default_thinking_level(level) {
                self.say(&format!("Error: settings not saved: {error}"));
            }
        }
        let line = format!("Next step runs with {}", model_line(&self.agent));
        self.say_styled(style::MUTED, &line);
    }

    /// Remembers the session's model as the one chosen last, here and anywhere, which sessions
    /// started later begin with (`Settings::remember_model`).
    pub(super) fn remember_model(&mut self) {
        let (provider, id) = (self.agent.model_provider(), self.agent.model_id());
        if let Err(error) = self
            .settings
            .remember_model(self.agent.root(), provider, id)
        {
            self.say(&format!("Error: settings not saved: {error}"));
        }
    }

    /// What is wrong in the settings, then in what the session `found` (`Agent::installed`).
    pub(super) fn problems(&self, found: &Installed) -> Vec<Problem> {
        (self.settings.warnings.iter())
            .map(|warning| Problem::in_settings(format!("settings: {warning}")))
            .chain(self.themes.problems(self.settings.theme_name()))
            .chain(self.keybindings.iter().cloned())
            .chain(found.problems.iter().cloned())
            .collect()
    }

    /// What to say about the problems at start and after `/reload`: on the full screen one line
    /// at most (`installed::attention`), else each problem.
    pub(super) fn problem_notices(&self) -> Vec<String> {
        let problems = self.problems(&self.agent.installed());
        match self.screen {
            Some(_) => installed::attention(&problems).into_iter().collect(),
            None => problems.into_iter().map(|problem| problem.text).collect(),
        }
    }

    /// Saves Ctrl+T's choice, made at the prompt or in a turn, when it changed.
    pub(super) fn save_hide_thinking(&mut self) {
        let hide = self.hide_thinking.load(Ordering::Relaxed);
        if let Err(error) = self.settings.set_hide_thinking_block(hide) {
            self.say(&format!("Error: settings not saved: {error}"));
        }
    }

    /// Records and shows the next step chosen, or that the list was dismissed.
    fn record(&mut self, source: Source, steps: &[Step], chosen: Option<usize>) {
        match self.agent.record_next_step(source, steps, chosen) {
            Ok(record) => {
                self.ui().next_step(&record);
                self.spaced = true;
            }
            Err(error) => self.say(&format!("Error: next step not recorded: {error}")),
        }
    }

    /// What the banner tells of the session starting.
    fn opening(&self) -> Opening {
        let installed = self.agent.installed();
        let named = |name: &str, path: &Path| (name.to_owned(), path.to_path_buf());
        Opening {
            model: model_line(&self.agent),
            root: self.agent.root().to_path_buf(),
            session: (self.agent.session_saved()).then(|| self.agent.session_path().to_path_buf()),
            context: installed.context_files,
            sets: (installed.sets.iter())
                .map(|set| named(&set.name, &set.directory))
                .collect(),
            skills: (self.agent.skills().list().iter())
                .map(|skill| named(&skill.name, &skill.path))
                .collect(),
            commands: self.agent.commands().list().len(),
            agents: installed.agents.len(),
        }
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
    pub(super) fn say_styled(&self, style: style::Style, text: &str) {
        match &self.screen {
            Some(screen) => screen.say_styled(style, text),
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
        if let Some(notice) = self.agent.saving_notice() {
            self.say(&notice);
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
        if let Err(error) = agent.add_shell(shell, &mut screen.link) {
            screen.say(&format!("Error: {error}"));
        }
    }
}

/// The rows the `/` list shows after the built-in commands: the loaded commands that are not
/// only delegated, then the skills as `skill:<name>`.
pub(super) fn completions(agent: &Agent) -> Vec<(String, String)> {
    (agent.commands().list().iter())
        .filter(|command| !command.delegate_only)
        .map(command_entry)
        .chain(
            (agent.skills().list().iter())
                .map(|skill| (format!("skill:{}", skill.name), skill.description.clone())),
        )
        .collect()
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

/// The model, after its provider when the models are those of several, and its reasoning
/// effort, as Scramjet's footer shows them.
pub(super) fn model_line(agent: &Agent) -> String {
    let model = match agent.provider() {
        Some(provider) => format!("({provider}) {}", agent.model_id()),
        None => agent.model_id().to_owned(),
    };
    match thinking(agent) {
        Some(thinking) => format!("{model} · {thinking}"),
        None => model,
    }
}

/// Where the session is saved, as said when it starts or another replaces it.
pub(super) fn saved_line(agent: &Agent) -> String {
    let saved = match agent.session_saved() {
        true => format!("session {}", agent.session_path().display()),
        false => "session not saved (--no-session)".to_owned(),
    };
    match skipped_lines(agent) {
        Some(skipped) => format!("{saved} ({skipped})"),
        None => saved,
    }
}

/// What to say of the session file's lines that could not be read, if any.
pub(super) fn skipped_lines(agent: &Agent) -> Option<String> {
    let skipped = agent.session_skipped();
    let numbers = |lines: &[usize]| {
        let numbers: Vec<String> = lines.iter().map(|line| (line + 1).to_string()).collect();
        numbers.join(", ")
    };
    match skipped {
        [] => None,
        [line] => Some(format!(
            "its line {} could not be read and is left out",
            line + 1
        )),
        [..] if skipped.len() <= 5 => Some(format!(
            "its lines {} could not be read and are left out",
            numbers(skipped)
        )),
        _ => Some(format!(
            "{} of its lines could not be read and are left out, from line {}",
            skipped.len(),
            skipped[0] + 1
        )),
    }
}

/// The reasoning effort, as Scramjet's footer says it; `None` for a model that does not reason.
pub(super) fn thinking(agent: &Agent) -> Option<String> {
    let effort = agent.effort().filter(|_| agent.reasons())?;
    Some(format!("thinking {}", settings::scramjet_level(effort)))
}

/// Shift+Tab: the next effort level, saved as the settings' default; returns what to say.
fn cycle_effort(agent: &mut Agent, settings: &mut Settings) -> Option<Note> {
    let cycled = agent.cycle_effort();
    if let Ok(Some(level)) = &cycled
        && let Err(error) = settings.set_default_thinking_level(level)
    {
        return Some(Note::Error(format!("Error: settings not saved: {error}")));
    }
    effort_note(cycled)
}

/// What a Shift+Tab says, as Scramjet reports it but for a level set, which the footer shows.
pub(super) fn effort_note(cycled: Result<Option<String>, AgentError>) -> Option<Note> {
    match cycled {
        Ok(Some(_)) => None,
        Ok(None) => Some(Note::Info(
            "Current model does not support thinking".to_owned(),
        )),
        Err(error) => Some(Note::Error(format!("Error: {error}"))),
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
    fn effort_note_says_as_scramjet_does_when_no_levels_and_nothing_for_a_level() {
        assert_eq!(
            effort_note(Ok(None)),
            Some(Note::Info(
                "Current model does not support thinking".to_owned()
            ))
        );
        assert_eq!(effort_note(Ok(Some("high".to_owned()))), None);
    }
}
