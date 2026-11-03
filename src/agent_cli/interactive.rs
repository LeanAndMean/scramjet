//! The interactive session: prompts read from the editor or from piped lines, the built-in
//! commands (`slash.rs`), model turns and a completed command's next step.
use super::settings::{DoubleEscape, Settings};
use super::terminal::{InputMode, Terminal};
use super::{editor, record, select, style, type_ahead, view};
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
    pub(super) ui: Terminal,
    /// The line editor, when stdin and stderr are terminals. Without it, prompts are read as lines
    /// and lists are printed numbered.
    pub(super) editor: Option<editor::Editor>,
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
        mut ui: Terminal,
        cancel: Arc<AtomicBool>,
        hide_thinking: Arc<AtomicBool>,
        settings: Settings,
    ) -> Self {
        let editor =
            (std::io::stdin().is_terminal() && std::io::stderr().is_terminal()).then(|| {
                editor::Editor::new(
                    agent.prompts().to_vec(),
                    hide_thinking.clone(),
                    agent.root().to_path_buf(),
                    agent.commands().list().iter().map(command_entry).collect(),
                    settings.double_escape != DoubleEscape::Nothing,
                )
            });
        ui.expandable = editor.is_some();
        Self {
            agent,
            ui,
            editor,
            spaced: false,
            cancel,
            hide_thinking,
            settings,
        }
    }

    /// Reads and runs prompts until the user quits or the input ends.
    pub(super) fn run(mut self) -> Result<(), String> {
        let result = self.read_and_run();
        self.save_hide_thinking();
        result
    }

    fn read_and_run(&mut self) -> Result<(), String> {
        eprintln!(
            "scramjet-rs · {} · {}\nsession {}",
            model_line(&self.agent),
            self.agent.root().display(),
            self.agent.session_path().display(),
        );
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
                eprintln!();
            }
            for warning in &warnings {
                self.ui.notice(warning);
            }
            self.spaced = true;
        }
        // A completed command's next step, run as if typed.
        let mut queued: Option<(String, Start)> = None;
        loop {
            self.save_hide_thinking();
            let (dispatched, start) = queued.take().unzip();
            let (prompt, escaped) = match self.read(dispatched)? {
                Read::Prompt(prompt) => (prompt, false),
                Read::DoubleEscape => match self.settings.double_escape {
                    DoubleEscape::Fork => ("/fork".to_owned(), true),
                    DoubleEscape::Tree | DoubleEscape::Nothing => ("/tree".to_owned(), true),
                },
                Read::Again => continue,
                Read::End => return Ok(()),
            };
            let prompt = prompt.trim();
            if prompt.is_empty() {
                continue;
            }
            if matches!(prompt, "/exit" | "/quit") {
                return Ok(());
            }
            if !self.builtin(prompt, escaped)? {
                queued = self.send(prompt, start.unwrap_or(Start::Typed))?;
            }
        }
    }

    /// The next prompt: `dispatched`, a next step to run as if typed, or what the user enters.
    fn read(&mut self, dispatched: Option<String>) -> Result<Read, String> {
        let prompt = match (dispatched, &mut self.editor) {
            (Some(prompt), _) => {
                if !std::mem::take(&mut self.spaced) {
                    eprintln!();
                }
                self.ui.user_prompt(&prompt);
                prompt
            }
            (None, Some(editor)) => {
                if !std::mem::take(&mut self.spaced) {
                    eprintln!();
                }
                let (agent, settings) = (&mut self.agent, &mut self.settings);
                let read = editor.read(&mut || {
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
                });
                match read.map_err(|e| e.to_string())? {
                    Some(editor::Input::Prompt(prompt)) => prompt,
                    Some(editor::Input::DoubleEscape) => return Ok(Read::DoubleEscape),
                    Some(editor::Input::Conversation(typed)) => {
                        let render = |width| {
                            let mut capture = Terminal::capture(width, self.hide_thinking.clone());
                            self.agent.show_conversation(&mut capture);
                            capture.captured()
                        };
                        match view::show(render, None, &self.hide_thinking, typed) {
                            Ok(typed) => editor.type_ahead(typed),
                            Err(error) => eprintln!("error: {error}"),
                        }
                        // The prompt comes back where it was erased.
                        self.spaced = true;
                        return Ok(Read::Again);
                    }
                    None => return Ok(Read::End),
                }
            }
            (None, None) => match read_lines()? {
                Some(prompt) => prompt,
                None => return Ok(Read::End),
            },
        };
        Ok(Read::Prompt(prompt))
    }

    /// Runs `prompt` as a model turn. Returns the next step to run, when the turn's command
    /// completed with one.
    fn send(&mut self, prompt: &str, start: Start) -> Result<Option<(String, Start)>, String> {
        if let Some(editor) = &mut self.editor {
            editor.remember(prompt);
            if start == Start::Typed {
                eprintln!();
            }
        }
        let hide = (self.settings.hide_credentials).hides(start == Start::Unattended);
        self.agent.set_hide_credentials(hide);
        let result = self.during_turn(Some(prompt), |agent, ui, cancel| {
            agent.turn(prompt, ui, cancel)
        });
        match result {
            Ok(()) => {}
            Err(AgentError::Cancelled) => eprintln!("{}", self.ui.dim("(cancelled)")),
            Err(error) => eprintln!("error: {error}"),
        }
        self.ui.usage(&self.agent);
        match self.agent.take_completed() {
            Some(completed) => self.next_step(&completed),
            None => Ok(None),
        }
    }

    /// Runs a model turn, `/compact`, a branch summary or a `!` command with the terminal in its
    /// turn mode, while keys typed meanwhile are read for the editor's next prompt
    /// (`type_ahead`), and Ctrl+O shows the conversation, `prompt` if any, and the turn so far.
    /// A Ctrl-C from before it starts does not cancel it.
    pub(super) fn during_turn<T>(
        &mut self,
        prompt: Option<&str>,
        run: impl FnOnce(&mut Agent, &mut dyn AgentUi, &AtomicBool) -> T,
    ) -> T {
        self.cancel.store(false, Ordering::Relaxed);
        let shown = self.editor.as_ref().map(|_| {
            Arc::new(record::Shown::new(
                self.agent.conversation(),
                prompt,
                &self.ui,
            ))
        });
        let mode = InputMode::turn();
        let expand = shown.clone().map(|shown| -> type_ahead::Expand {
            Arc::new(move |typed, waiting: &AtomicBool| shown.follow(typed, waiting))
        });
        self.ui.type_ahead = self
            .editor
            .is_some()
            .then(|| type_ahead::TypeAhead::start(self.cancel.clone(), expand));
        let result = match &shown {
            Some(shown) => run(
                &mut self.agent,
                &mut record::Recording {
                    ui: &mut self.ui,
                    shown,
                },
                &self.cancel,
            ),
            None => run(&mut self.agent, &mut self.ui, &self.cancel),
        };
        let typed = self.ui.type_ahead.take().map(type_ahead::TypeAhead::finish);
        drop(mode);
        if let (Some(editor), Some(typed)) = (&mut self.editor, typed) {
            editor.type_ahead(typed);
        }
        self.ui.clear();
        self.cancel.store(false, Ordering::Relaxed);
        result
    }

    /// The next step of a completed command, as Scramjet routes it with autopilot off: an edge
    /// set to `chain` or a forced step runs at once; otherwise the user chooses from the steps
    /// the model offered. Returns the prompt to run; a text step goes into the editor instead.
    fn next_step(&mut self, completed: &Completed) -> Result<Option<(String, Start)>, String> {
        let routing = self.agent.route(completed, self.editor.is_some());
        if routing.notices.is_empty() && routing.next == Next::Nothing {
            return Ok(None);
        }
        // Under the usage line, as a prompt would be.
        eprintln!();
        for notice in &routing.notices {
            self.ui.notice(notice);
        }
        let (step, start) = match routing.next {
            Next::Nothing => {
                self.spaced = true;
                return Ok(None);
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
                let chosen = select::question("Select next step", &choices, recommended)
                    .map_err(|e| e.to_string())?;
                self.show_record(&next_step_record(&routing.source, &steps, chosen));
                match chosen {
                    Some(index) => (steps[index].clone(), Start::Chosen),
                    None => return Ok(None),
                }
            }
        };
        let Some(command) = step.command else {
            if let Some(editor) = &mut self.editor {
                editor.set_text(step.message);
            }
            return Ok(None);
        };
        if step.fresh_session && !self.new_session() {
            return Ok(None);
        }
        Ok(Some((command, start)))
    }

    /// Saves Ctrl+T's choice, made at the prompt, in a turn or in the conversation view, when
    /// it changed.
    fn save_hide_thinking(&mut self) {
        let hide = self.hide_thinking.load(Ordering::Relaxed);
        if let Err(error) = self.settings.set_hide_thinking_block(hide) {
            eprintln!("error: settings not saved: {error}");
        }
    }

    fn show_record(&self, record: &str) {
        eprintln!(
            "{}",
            self.ui.styled(style::NEXT_STEP, &self.ui.wrapped(record))
        );
    }

    /// Shows a resumed conversation under its header, as Scramjet redraws it; it then ends with
    /// a blank line.
    pub(super) fn show_resumed(&mut self) {
        self.spaced = !self.agent.conversation().is_empty();
        if self.spaced {
            eprintln!();
            self.agent.show_conversation(&mut self.ui);
            self.ui.clear();
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

/// What a Shift+Tab says under the prompt, as Scramjet reports it.
fn effort_note(cycled: Result<Option<String>, AgentError>) -> String {
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
