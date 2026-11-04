//! The built-in commands: `/settings`, `/copy`, `/new`, `/name`, `/session`, `/hotkeys`,
//! `/resume`, `/fork`, `/clone`, `/tree`, `/compact`, `/autopilot`, `/reload`, and `!` commands;
//! `/model` and `/scoped-models` are in `scope.rs`.
use super::fuzzy::Haystack;
use super::interactive::{Interactive, completions, model_line};
use super::screen;
use super::settings::{ROWS, Row, scramjet_level};
use super::terminal::Terminal;
use super::text::{clip, one_line};
use super::{clipboard, editor, installed, select, style, tree};
use scramjet_rs::agent::{
    Agent, AgentError, Approval, Approvals, CODING_TOOLS, Edge, Installed, Navigated, NextStep,
    NextSteps, Publication, Removal, Scope, SessionInfo, Setting, search_program, shortened,
};
use std::{io::IsTerminal, sync::atomic::Ordering, time::Duration};
use unicode_width::UnicodeWidthStr;

const SETTINGS_KEYS: &str = "↑↓ move · Enter change · Esc close";
const NO_LEVELS: &str = "The model's card lists no thinking levels to choose from.";
const BACK_KEYS: &str = "↑↓ move · Enter select · Esc back";
const TOOLS_KEYS: &str = "↑↓ move · Enter switch on or off · Esc back";
const CHANGE_KEYS: &str = "↑↓ move · Enter change · Esc back";
const VIEW_KEYS: &str = "↑↓ move · Esc back";
const RESUME_KEYS: &str = "↑↓ move · Enter select · Ctrl+R rename · Ctrl+D delete · Esc cancel";
/// What a next step can be set to, in the order `/settings` offers them.
const STEP_CHOICES: [(&str, Option<(Edge, Scope)>); 5] = [
    (
        "Continue automatically, in this project",
        Some((Edge::Chain, Scope::Project)),
    ),
    (
        "Continue automatically, in all projects",
        Some((Edge::Chain, Scope::Global)),
    ),
    (
        "Ask me, in this project",
        Some((Edge::Pause, Scope::Project)),
    ),
    (
        "Ask me, in all projects",
        Some((Edge::Pause, Scope::Global)),
    ),
    ("Autopilot decides", None),
];
const AUTOPILOT_DECIDES: &str = "With autopilot on, the recommended step runs after a 3-second \
    countdown; off, the next steps wait for you to choose.";

/// What a coding tool does, as `/settings` says under its row, in the instructions' words.
fn tool_note(tool: &str) -> &'static str {
    match tool {
        "read" => "Read file contents",
        "bash" => "Execute bash commands",
        "edit" => "Make precise file edits with exact text replacement",
        "write" => "Create or overwrite files",
        "grep" => "Search file contents for patterns, with ripgrep",
        "find" => "Find files by glob pattern, with fd",
        _ => "List directory contents",
    }
}

/// A built-in command as typed, with its argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Builtin<'a> {
    Settings,
    Copy,
    New,
    Name(&'a str),
    Session,
    Hotkeys,
    Resume(&'a str),
    Fork(&'a str),
    Clone,
    Tree(&'a str),
    Model(&'a str),
    ScopedModels,
    Compact(&'a str),
    Autopilot(&'a str),
    Reload,
    Quit,
}

impl<'a> Builtin<'a> {
    /// The built-in command `prompt` runs, if any; `!` commands are not among them.
    pub(super) fn parse(prompt: &'a str) -> Option<Self> {
        let prompt = prompt.trim();
        let (name, argument) = prompt
            .split_once(char::is_whitespace)
            .unwrap_or((prompt, ""));
        let argument = argument.trim();
        Some(match name {
            "/settings" if argument.is_empty() => Self::Settings,
            "/copy" if argument.is_empty() => Self::Copy,
            "/new" if argument.is_empty() => Self::New,
            "/name" => Self::Name(argument),
            "/session" if argument.is_empty() => Self::Session,
            "/hotkeys" if argument.is_empty() => Self::Hotkeys,
            "/resume" => Self::Resume(argument),
            "/fork" => Self::Fork(argument),
            "/clone" if argument.is_empty() => Self::Clone,
            "/tree" => Self::Tree(argument),
            "/model" => Self::Model(argument),
            "/scoped-models" if argument.is_empty() => Self::ScopedModels,
            "/compact" => Self::Compact(argument),
            "/autopilot" => Self::Autopilot(argument),
            "/reload" if argument.is_empty() => Self::Reload,
            "/exit" | "/quit" if argument.is_empty() => Self::Quit,
            _ => return None,
        })
    }

    /// Whether, typed during a turn, it stops the turn and runs once it has: it replaces or
    /// rewrites the conversation, as Scramjet's `/compact` and `/new` abort the turn first and
    /// a session switch disposes of it. The others run between the turn's requests, but
    /// `/reload`, which the screen refuses during a turn, as Scramjet does.
    pub(super) fn interrupts(self) -> bool {
        !matches!(
            self,
            Self::Settings
                | Self::Copy
                | Self::Name(_)
                | Self::Session
                | Self::Hotkeys
                | Self::Model(_)
                | Self::ScopedModels
                | Self::Autopilot(_)
                | Self::Reload
        )
    }
}

impl Interactive {
    /// Runs `prompt` if it is a built-in command other than `/quit`, or a `!` command; returns
    /// whether it was.
    pub(super) fn builtin(&mut self, prompt: &str) -> bool {
        if let Some(command) = prompt.strip_prefix('!') {
            return self.bang(command);
        }
        match Builtin::parse(prompt) {
            Some(Builtin::Settings) => self.settings(),
            Some(Builtin::Copy) => self.copy_reply(),
            Some(Builtin::New) => {
                self.new_session();
            }
            Some(Builtin::Name(name)) => self.name(name),
            Some(Builtin::Session) => self.say(session_info(&self.agent, &self.ui).trim_end()),
            Some(Builtin::Hotkeys) => self.say(hotkeys(&self.ui).trim_end()),
            Some(Builtin::Resume(wanted)) => {
                self.resume(wanted);
            }
            Some(Builtin::Fork(wanted)) => self.fork(wanted),
            Some(Builtin::Clone) => self.clone_session(),
            Some(Builtin::Tree(wanted)) => self.tree(wanted),
            Some(Builtin::Model(wanted)) => self.model(wanted),
            Some(Builtin::ScopedModels) => self.scoped_models(),
            Some(Builtin::Compact(focus)) => self.compact(focus),
            Some(Builtin::Autopilot(wanted)) => self.autopilot(wanted),
            Some(Builtin::Reload) => self.reload(),
            Some(Builtin::Quit) | None => return false,
        }
        true
    }

    /// `/new`, also before a next step that wants a fresh session. Returns whether it started.
    pub(super) fn new_session(&mut self) -> bool {
        match self.agent.new_session() {
            Ok(()) => {
                let path = self.agent.session_path().display();
                self.replaced(&format!("✓ New session started\nsession {path}"));
                true
            }
            Err(error) => {
                self.say(&format!("error: {error}"));
                false
            }
        }
    }

    fn name(&mut self, name: &str) {
        if name.is_empty() {
            match self.agent.session_name() {
                Some(current) => self.say(&self.ui.dim(&format!("Session name: {current}"))),
                None => self.say("Usage: /name <name>"),
            }
        } else {
            match self.agent.set_session_name(name) {
                Ok(()) => self.say(&self.ui.dim(&format!("Session name set: {name}"))),
                Err(error) => self.say(&format!("error: {error}")),
            }
        }
    }

    /// `/copy`, as Scramjet's: the last reply's text, as the model wrote it, to the clipboard.
    fn copy_reply(&mut self) {
        let Some(text) = self.agent.last_reply() else {
            return self.say("No agent messages to copy yet.");
        };
        let terminal = self.screen.is_some() || std::io::stderr().is_terminal();
        match clipboard::copy(&text, terminal) {
            Ok(sequence) => {
                if let Some(sequence) = sequence {
                    match &self.screen {
                        Some(screen) => screen.to_terminal(sequence),
                        None => eprint!("{sequence}"),
                    }
                }
                self.say(&self.ui.dim("Copied last agent message to clipboard"));
            }
            Err(error) => self.say(&format!("error: {error}")),
        }
    }

    /// `/autopilot on|off|status`, as Scramjet's: on, a completed command's recommended next
    /// step runs unless the user chooses otherwise.
    fn autopilot(&mut self, wanted: &str) {
        let set = match wanted.to_lowercase().as_str() {
            "on" => Some(true),
            "off" => Some(false),
            "" | "status" => None,
            _ => return self.say("Usage: /autopilot on|off|status"),
        };
        if let Some(on) = set
            && let Err(error) = self.agent.set_autopilot(on)
        {
            return self.say(&format!("error: {error}"));
        }
        let state = if self.agent.autopilot() { "on" } else { "off" };
        self.say(&self.ui.dim(&format!("Autopilot is {state}")));
    }

    /// `/settings`, as Scramjet's: the settings in a list, each changed with Enter, saved and
    /// used at once. Without the full screen they are printed.
    fn settings(&mut self) {
        self.save_hide_thinking();
        if self.screen.is_none() {
            let path = self.settings.shown_path();
            let path = path.unwrap_or_else(|| "not saved: HOME is not set".into());
            eprintln!("Settings ({path})");
            for row in self.setting_rows() {
                eprintln!("  {row}");
            }
            return;
        }
        let mut at = 0;
        loop {
            // Ctrl+T in the list changes the thinking blocks too.
            self.save_hide_thinking();
            let Some(screen) = &self.screen else {
                return;
            };
            let rows = self.setting_rows();
            let levels = !self.agent.effort_levels().is_empty();
            let problems = self.agent.next_steps().problems;
            let publishing = self.agent.approvals().problem;
            let notes = (ROWS.iter())
                .map(|row| match row {
                    Row::ThinkingLevel if !levels => NO_LEVELS.to_owned(),
                    Row::NextSteps if !problems.is_empty() => {
                        format!("{} {}", row.description(), not_used(&problems))
                    }
                    Row::Publishing if let Some(problem) = &publishing => {
                        format!("{} {}", row.description(), every_one_asks(problem))
                    }
                    row => row.description().to_owned(),
                })
                .collect();
            let picker = select::Picker::select(
                "Settings",
                select::VISIBLE,
                ROWS.len(),
                at,
                None,
                |index, width| clip(&rows[index], width),
            );
            let picker = picker.noting(SETTINGS_KEYS, notes).held();
            let Some(index) = screen.pick(picker) else {
                break;
            };
            at = index;
            match ROWS[index] {
                Row::ThinkingLevel => self.choose_thinking_level(),
                Row::Tools => self.choose_tools(),
                Row::NextSteps => self.choose_next_steps(),
                Row::Publishing => self.choose_approvals(),
                Row::Installed => self.show_installed(),
                Row::Autopilot => {
                    if let Err(error) = self.agent.set_autopilot(!self.agent.autopilot()) {
                        self.say(&format!("error: {error}"));
                    }
                }
                row => {
                    if let Err(error) = self.settings.cycle(row) {
                        self.say(&format!("error: settings not saved: {error}"));
                    }
                }
            }
            self.hide_thinking
                .store(self.settings.hide_thinking_block, Ordering::Relaxed);
            if let Some(screen) = &mut self.screen {
                (screen.link).reloaded(completions(&self.agent), &self.settings);
            }
        }
        if let Some(screen) = &self.screen {
            screen.dismiss();
        }
    }

    /// `/settings`' rows: each label, as wide as the widest, and its value.
    fn setting_rows(&self) -> Vec<String> {
        let width = ROWS.iter().map(|row| row.label().len()).max().unwrap_or(0);
        let value = |row| match row {
            Row::ThinkingLevel => (self.agent.effort().map(scramjet_level))
                .unwrap_or("model default")
                .to_owned(),
            Row::Autopilot if self.agent.autopilot() => "on".into(),
            Row::Autopilot => "off".into(),
            Row::NextSteps => steps_summary(&self.agent.next_steps()),
            Row::Publishing => approvals_summary(&self.agent.approvals()),
            Row::Installed => {
                let found = self.agent.installed();
                let problems = self.problems(&found);
                installed::summary(&self.loaded(&found, &problems))
            }
            row => self.settings.shown(row),
        };
        (ROWS.iter())
            .map(|&row| format!("{:width$}  {}", row.label(), value(row)))
            .collect()
    }

    /// The levels of the model's card to choose from; the one chosen is the session's and the
    /// settings' default, as Shift+Tab sets them.
    fn choose_thinking_level(&mut self) {
        let levels = self.agent.effort_levels().to_vec();
        let Some(screen) = self.screen.as_ref().filter(|_| !levels.is_empty()) else {
            return;
        };
        let current =
            (self.agent.effort()).and_then(|effort| levels.iter().position(|l| l == effort));
        let picker = select::Picker::select(
            "Thinking level",
            select::VISIBLE,
            levels.len(),
            current.unwrap_or(0),
            current,
            |index, width| clip(scramjet_level(&levels[index]), width),
        );
        let Some(index) = screen.pick(picker.noting(BACK_KEYS, Vec::new()).held()) else {
            return;
        };
        let level = &levels[index];
        if let Err(error) = self.agent.set_effort(level.clone()) {
            return self.say(&format!("error: {error}"));
        }
        if let Err(error) = self.settings.set_default_thinking_level(level) {
            self.say(&format!("error: settings not saved: {error}"));
        }
    }

    /// The coding tools, each switched on or off with Enter; the agent has them from its next
    /// request on. One whose program is not installed stays off.
    fn choose_tools(&mut self) {
        let mut at = 0;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let on = |tool: &str| self.settings.tools.iter().any(|known| known == tool);
            let missing = |tool: &str| match tool {
                "grep" if search_program(tool).is_none() => Some("ripgrep (rg)"),
                "find" if search_program(tool).is_none() => Some("fd"),
                _ => None,
            };
            let rows: Vec<String> = (CODING_TOOLS.iter())
                .map(|tool| format!("[{}] {tool}", if on(tool) { "x" } else { " " }))
                .collect();
            let notes = (CODING_TOOLS.iter())
                .map(|tool| match missing(tool) {
                    Some(program) => format!("Needs {program}, which is not installed."),
                    None => tool_note(tool).to_owned(),
                })
                .collect();
            let picker = select::Picker::select(
                "Tools",
                select::VISIBLE,
                rows.len(),
                at,
                None,
                |index, width| clip(&rows[index], width),
            );
            let Some(index) = screen.pick(picker.noting(TOOLS_KEYS, notes).held()) else {
                return;
            };
            at = index;
            let tool = CODING_TOOLS[index];
            if !on(tool) && missing(tool).is_some() {
                continue;
            }
            let mut tools = self.settings.tools.clone();
            match on(tool) {
                true => tools.retain(|known| known != tool),
                false => tools.push(tool.to_owned()),
            }
            let saved = self.settings.set_tools(tools);
            self.agent.set_tools(self.settings.tools.clone());
            if let Err(error) = saved {
                self.say(&format!("error: settings not saved: {error}"));
            }
        }
    }

    /// Each command's next steps under the command, with what follows each and where that is
    /// set, after a row applying what the command sets suggest when they suggest a change;
    /// Enter changes one. The routing after the next command uses the files as changed.
    fn choose_next_steps(&mut self) {
        enum Line {
            Apply,
            Heading,
            Step(usize),
        }
        // The step chosen last, which stays selected when the rows above it change.
        let mut chosen: Option<(String, Option<String>)> = None;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let steps = self.agent.next_steps();
            if steps.steps.is_empty() {
                return;
            }
            let width = (steps.steps.iter())
                .map(|step| target_name(step).width())
                .max()
                .unwrap_or(0);
            let mut rows = Vec::new();
            let mut lines = Vec::new();
            let mut notes = Vec::new();
            let suggested = steps.suggested();
            if !suggested.is_empty() {
                rows.push(apply_label(suggested.len()));
                lines.push(Line::Apply);
                notes.push(
                    "What the command sets suggest, in their autonomy-defaults.yaml, for the \
                     steps marked \"suggested\" below, which you have not set yourself. Enter \
                     chooses this project or all projects."
                        .to_owned(),
                );
            }
            let mut at = 0;
            for (index, step) in steps.steps.iter().enumerate() {
                if index == 0 || steps.steps[index - 1].source != step.source {
                    rows.push(format!("After /{}", step.source));
                    lines.push(Line::Heading);
                    notes.push(String::new());
                }
                if chosen.as_ref() == Some(&(step.source.clone(), step.target.clone())) {
                    at = rows.len();
                }
                let target = target_name(step);
                let pad = " ".repeat(width - target.width());
                rows.push(format!("  → {target}{pad}  {}", step_value(step)));
                lines.push(Line::Step(index));
                notes.push(step_note(step, &steps, self.agent.autopilot()));
            }
            let headings = (lines.iter())
                .map(|line| matches!(line, Line::Heading))
                .collect();
            let visible = (editor::terminal_size().1 / 2).max(select::VISIBLE);
            let picker = select::Picker::select(
                "Next steps",
                visible,
                rows.len(),
                at,
                None,
                |index, width| clip(&rows[index], width),
            );
            let picker = picker.headed(headings).noting(CHANGE_KEYS, notes).held();
            let Some(index) = screen.pick(picker) else {
                return;
            };
            chosen = None;
            match lines[index] {
                Line::Apply => self.choose_suggestions(&steps),
                Line::Step(index) => {
                    let step = &steps.steps[index];
                    chosen = Some((step.source.clone(), step.target.clone()));
                    if !step.forced {
                        self.choose_next_step(&steps, step);
                    }
                }
                Line::Heading => {}
            }
        }
    }

    /// Saves what the command sets suggest for the steps without a setting of their own, for
    /// this project or all projects.
    fn choose_suggestions(&mut self, steps: &NextSteps) {
        let Some(screen) = &self.screen else {
            return;
        };
        let suggested = steps.suggested();
        let list = (suggested.iter())
            .filter_map(|step| {
                let suggestion = step.suggestion()?;
                let words = edge_words(suggestion.edge);
                Some(format!("/{} → {}: {words}", step.source, target_name(step)))
            })
            .collect::<Vec<_>>()
            .join("; ");
        let choices = [
            ("In all projects", Scope::Global),
            ("In this project", Scope::Project),
        ];
        let project = shortened(&steps.project.display().to_string());
        let global = (steps.global.as_ref()).map(|path| shortened(&path.display().to_string()));
        let mut masked: Vec<String> = Vec::new();
        for step in &suggested {
            let source = format!("/{}", step.source);
            let by_project = Some(Scope::Project) == step.setting.map(|setting| setting.scope);
            if by_project && !masked.contains(&source) {
                masked.push(source);
            }
        }
        let held = match masked.is_empty() {
            true => String::new(),
            false => format!(
                " Here, this project's setting for any other command after {} still holds.",
                masked.join(", ")
            ),
        };
        let notes = (choices.iter())
            .map(|(_, scope)| match (scope, &global) {
                (Scope::Global, None) => "Not available: HOME is not set.".to_owned(),
                (Scope::Global, Some(global)) => {
                    format!("Saved for all projects in {global}: {list}.{held}")
                }
                (Scope::Project, _) => format!("Saved for this project in {project}: {list}."),
            })
            .collect();
        let title = apply_label(suggested.len());
        let picker = select::Picker::select(
            &title,
            select::VISIBLE,
            choices.len(),
            0,
            None,
            |index, width| clip(choices[index].0, width),
        );
        let Some(index) = screen.pick(picker.noting(BACK_KEYS, notes).held()) else {
            return;
        };
        if let Err(error) = self.agent.apply_suggestions(choices[index].1) {
            self.say(&format!("error: suggested settings not saved: {error}"));
        }
    }

    /// What follows `step`: continue automatically or ask, for this project or all of them, or
    /// autopilot decides; saved at once.
    fn choose_next_step(&mut self, steps: &NextSteps, step: &NextStep) {
        let Some(screen) = &self.screen else {
            return;
        };
        let current = match step.setting {
            None => Some(STEP_CHOICES.len() - 1),
            Some(Setting {
                any_other: true, ..
            }) => None,
            Some(Setting { edge, scope, .. }) => {
                (STEP_CHOICES.iter()).position(|(_, choice)| *choice == Some((edge, scope)))
            }
        };
        let project = shortened(&steps.project.display().to_string());
        let global = (steps.global.as_ref()).map(|path| shortened(&path.display().to_string()));
        let wildcard =
            (steps.steps.iter()).any(|other| other.source == step.source && other.target.is_none());
        let source = format!("/{}", step.source);
        let suggests = |choice: &Option<(Edge, Scope)>| match (&step.suggested, choice) {
            (Some(suggested), Some((edge, _))) if suggested.edge == *edge => {
                format!(" {} suggests this.", suggested.set)
            }
            _ => String::new(),
        };
        let notes = (STEP_CHOICES.iter())
            .map(|(_, choice)| match choice {
                Some((edge, Scope::Project)) => {
                    format!(
                        "{} Saved for this project in {project}.",
                        meaning(*edge, &source)
                    )
                }
                Some((_, Scope::Global)) if global.is_none() => {
                    "Not available: HOME is not set.".to_owned()
                }
                Some((edge, Scope::Global)) => {
                    let instead = match step.in_project {
                        true => {
                            ", and taken out of this project's file, whose setting would hold here"
                        }
                        false => "",
                    };
                    let global = global.as_deref().unwrap_or_default();
                    let meaning = meaning(*edge, &source);
                    format!("{meaning} Saved for all projects in {global}{instead}.")
                }
                None => {
                    let out = match (step.in_project, step.in_global) {
                        (true, true) => {
                            " Takes its settings out of this project's file and the \
                                         file for all projects."
                        }
                        (true, false) => " Takes its setting out of this project's file.",
                        (false, true) => " Takes its setting out of the file for all projects.",
                        (false, false) => "",
                    };
                    let still = match wildcard && step.target.is_some() {
                        true => " The setting for any other command still holds; change that too.",
                        false => "",
                    };
                    format!("{AUTOPILOT_DECIDES}{out}{still}")
                }
            })
            .zip(STEP_CHOICES.iter())
            .map(|(note, (_, choice))| note + &suggests(choice))
            .collect();
        let title = format!("After {source} → {}", target_name(step));
        let picker = select::Picker::select(
            &title,
            select::VISIBLE,
            STEP_CHOICES.len(),
            current.unwrap_or(0),
            current,
            |index, width| clip(STEP_CHOICES[index].0, width),
        );
        let Some(index) = screen.pick(picker.noting(BACK_KEYS, notes).held()) else {
            return;
        };
        let target = step.target.as_deref();
        let chosen = STEP_CHOICES[index].1;
        if let Err(error) = self.agent.set_next_step(&step.source, target, chosen) {
            self.say(&format!("error: next step not saved: {error}"));
        }
    }

    /// The publications of each command whose policy can approve them, under the command, with
    /// whether each asks first and who says so; Enter changes one. The next publication uses
    /// the file as changed.
    fn choose_approvals(&mut self) {
        let mut at = 0;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let approvals = self.agent.approvals();
            if approvals.list.is_empty() {
                return;
            }
            let width = (approvals.list.iter())
                .map(|approval| publishes(approval.tool).0.width())
                .max()
                .unwrap_or(0);
            // A heading before each command's publications: `None` in `shown`.
            let mut rows = Vec::new();
            let mut shown = Vec::new();
            let mut notes = Vec::new();
            for (index, approval) in approvals.list.iter().enumerate() {
                if index == 0 || approvals.list[index - 1].command != approval.command {
                    rows.push(format!("/{}", approval.command));
                    shown.push(None);
                    notes.push(String::new());
                }
                let value = approval_value(approval, approvals.problem.is_some());
                rows.push(format!("  {:width$}  {value}", publishes(approval.tool).0));
                shown.push(Some(index));
                notes.push(approval_note(approval, &approvals));
            }
            let headings = shown.iter().map(Option::is_none).collect();
            let visible = (editor::terminal_size().1 / 2).max(select::VISIBLE);
            let picker = select::Picker::select(
                "Publishing",
                visible,
                rows.len(),
                at,
                None,
                |index, width| clip(&rows[index], width),
            );
            let picker = picker.headed(headings).noting(CHANGE_KEYS, notes).held();
            let Some(index) = screen.pick(picker) else {
                return;
            };
            at = index;
            if let Some(approval) = shown[index].map(|index| &approvals.list[index]) {
                self.choose_approval(&approvals, approval);
            }
        }
    }

    /// Whether `approval`'s publications ask first, publish without asking, or go as its set
    /// says; saved at once, for all projects.
    fn choose_approval(&mut self, approvals: &Approvals, approval: &Approval) {
        let Some(screen) = &self.screen else {
            return;
        };
        let set = set_of(&approval.command);
        let default = asks(approval.default.unwrap_or(Publication::Ask));
        let choices = [
            ("Ask me".to_owned(), Some(Publication::Ask)),
            ("Publish without asking".to_owned(), Some(Publication::Auto)),
            (format!("As {set} says ({default})"), None),
        ];
        let current = match approval.setting {
            Some(Publication::Ask) => 0,
            Some(Publication::Auto) => 1,
            None => 2,
        };
        let global = (approvals.global.as_ref()).map(|path| shortened(&path.display().to_string()));
        let saved = |what: &str| match &global {
            Some(global) => format!("{what} Saved for all projects in {global}."),
            None => "Not available: HOME is not set.".to_owned(),
        };
        let notes = (choices.iter())
            .map(|(_, choice)| match choice {
                Some(Publication::Ask) => {
                    saved("It shows you what it will publish and waits for your approval.")
                }
                Some(Publication::Auto) => saved(
                    "Published without showing it to you first, in the project's own repository \
                     only.",
                ),
                None => {
                    let says = match approval.default {
                        None => format!("{set} says nothing about it, so it asks."),
                        Some(Publication::Ask) => {
                            format!("{set} has it ask, in its autonomy-defaults.yaml.")
                        }
                        Some(Publication::Auto) => format!(
                            "{set} has it publish without asking, in its autonomy-defaults.yaml."
                        ),
                    };
                    match (&global, approval.setting) {
                        (Some(global), Some(_)) => {
                            format!("{says} Takes your setting out of {global}.")
                        }
                        _ => says,
                    }
                }
            })
            .collect();
        let (_, does) = publishes(approval.tool);
        let title = format!("When /{} {does}", approval.command);
        let picker = select::Picker::select(
            &title,
            select::VISIBLE,
            choices.len(),
            current,
            Some(current),
            |index, width| clip(&choices[index].0, width),
        );
        let Some(index) = screen.pick(picker.noting(BACK_KEYS, notes).held()) else {
            return;
        };
        let chosen = choices[index].1;
        if let Err(error) = self
            .agent
            .set_approval(&approval.command, approval.tool, chosen)
        {
            self.say(&format!("error: publishing not saved: {error}"));
        }
    }

    /// What the session loaded, each with where it comes from, under headings, the problems
    /// first; the notes say more, and nothing changes here.
    fn show_installed(&mut self) {
        let mut at = 0;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let found = self.agent.installed();
            let problems = self.problems(&found);
            let listing = installed::listing(&self.loaded(&found, &problems));
            let visible = (editor::terminal_size().1 / 2).max(select::VISIBLE);
            let rows = &listing.rows;
            let picker = select::Picker::select(
                "Installed",
                visible,
                rows.len(),
                at,
                None,
                |index, width| clip(&rows[index], width),
            );
            let picker = (picker.headed(listing.headings)).noting(VIEW_KEYS, listing.notes);
            let Some(index) = screen.pick(picker.held()) else {
                return;
            };
            at = index;
        }
    }

    fn loaded<'a>(&'a self, found: &'a Installed, problems: &'a [String]) -> installed::Loaded<'a> {
        installed::Loaded {
            installed: found,
            commands: self.agent.commands().list(),
            skills: self.agent.skills().list(),
            settings: self.settings.path(),
            problems,
        }
    }

    /// `/reload`, as Scramjet's: the settings, the command sets with their commands and agents,
    /// the skills and the context files read again, and what is wrong in them said as at start.
    fn reload(&mut self) {
        self.save_hide_thinking();
        self.settings.reload();
        let settings = &self.settings;
        (self.hide_thinking).store(settings.hide_thinking_block, Ordering::Relaxed);
        self.agent.reload(
            settings.tools.clone(),
            settings.command_sets.clone(),
            settings.skills.clone(),
        );
        if let Some(screen) = &mut self.screen {
            (screen.link).reloaded(completions(&self.agent), &self.settings);
            (screen.link).models(self.agent.models().unwrap_or_default());
        }
        for notice in self.problem_notices() {
            self.ui().notice(&notice);
        }
        let done = "Reloaded settings, command sets, agents, skills and context files";
        self.say(&self.ui.dim(done));
    }

    /// `/resume`: chooses a session to continue in, by its number with piped input. Returns
    /// whether it switched to one.
    pub(super) fn resume(&mut self, wanted: &str) -> bool {
        let sessions = match self.agent.sessions() {
            Ok(sessions) => threaded(sessions),
            Err(error) => {
                self.say(&format!("error: {error}"));
                return false;
            }
        };
        let current =
            (sessions.iter()).position(|(session, _)| self.agent.is_session(&session.path));
        let chosen = match wanted {
            "" if sessions.is_empty() => {
                self.say("No sessions to resume");
                None
            }
            "" if self.interactive() => self.choose_session(),
            "" => {
                let width = editor::terminal_size().0;
                for (index, (session, tree)) in sessions.iter().take(20).enumerate() {
                    let mark = if current == Some(index) { "*" } else { " " };
                    let number = format!("{}{mark} {tree}", index + 1);
                    let line = session_line(session, width.saturating_sub(number.width()));
                    eprintln!("{number}{line}");
                }
                if sessions.len() > 20 {
                    eprintln!("({} older sessions not shown)", sessions.len() - 20);
                }
                None
            }
            wanted => match wanted.parse::<usize>() {
                Err(_) | Ok(0) => {
                    self.say("error: /resume requires a positive session number");
                    None
                }
                Ok(number) if number > sessions.len() => {
                    self.say(&format!(
                        "error: no session numbered {number} (/resume lists them)"
                    ));
                    None
                }
                Ok(number) if current == Some(number - 1) => {
                    self.say(&format!("error: session {number} is the current one"));
                    None
                }
                Ok(number) => Some(sessions[number - 1].0.path.clone()),
            },
        };
        let Some(path) = chosen else {
            return false;
        };
        match self.agent.resume_session(&path) {
            Ok(()) => {
                self.agent.load_model_card();
                self.remember_prompts();
                self.replaced(&format!(
                    "✓ Resumed session · {}\nsession {}",
                    model_line(&self.agent),
                    self.agent.session_path().display(),
                ));
                true
            }
            Err(error) => {
                self.say(&format!("error: {error}"));
                false
            }
        }
    }

    /// Chooses a session on the full screen, as Scramjet's session selector does: forks under the sessions they came from, a query matching them by their
    /// prompts and replies, Ctrl+R renaming the selected one and Ctrl+D deleting it.
    fn choose_session(&mut self) -> Option<std::path::PathBuf> {
        let mut spot = None;
        loop {
            let sessions = match self.agent.sessions() {
                Ok(sessions) => threaded(sessions),
                Err(error) => {
                    self.screen.as_ref()?.dismiss();
                    self.say(&format!("error: {error}"));
                    return None;
                }
            };
            let screen = self.screen.as_ref()?;
            if sessions.is_empty() {
                screen.dismiss();
                self.say("No sessions to resume");
                return None;
            }
            let current =
                (sessions.iter()).position(|(session, _)| self.agent.is_session(&session.path));
            // Newest first, so the first other than the current one is the likely choice.
            let initial = usize::from(current == Some(0));
            let texts = searched(&sessions);
            let row = |index: usize, width| session_line(&sessions[index].0, width);
            let tree = sessions.iter().map(|(_, tree)| tree.clone()).collect();
            let picker = select::Picker::search(
                "Resume session",
                select::VISIBLE,
                texts,
                initial,
                current,
                "No matching sessions",
                row,
            );
            let picker = (picker.noting(RESUME_KEYS, Vec::new()))
                .threaded(tree, row)
                .acting()
                .held();
            let picker = match spot.take() {
                Some((at, status)) => picker.at(at, status),
                None => picker,
            };
            let (act, index, at) = match screen.pick_or_act(picker) {
                select::Picked::Row(chosen) => {
                    screen.dismiss();
                    return chosen.map(|index| sessions[index].0.path.clone());
                }
                select::Picked::Act(act, index, at) => (act, index, at),
                // Not a key of this list (`Picker::tabbing`).
                select::Picked::Tab(_) => {
                    screen.dismiss();
                    return None;
                }
            };
            let session = &sessions[index].0;
            let status = match act {
                select::Act::Rename => {
                    let name = session.name.clone().unwrap_or_default();
                    let dialog =
                        screen::Dialog::Text("Rename session".into(), name, screen::Optional::No);
                    match screen.ask(dialog, None) {
                        Some(screen::Answer::Text(Some(name))) => {
                            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
                            match self.agent.rename_session(&session.path, &name) {
                                Ok(()) => None,
                                Err(error) => Some((format!("Failed to rename: {error}"), true)),
                            }
                        }
                        _ => None,
                    }
                }
                select::Act::Delete => Some(match self.agent.delete_session(&session.path) {
                    Ok(Removal::Trashed) => ("Session moved to trash".to_owned(), false),
                    Ok(Removal::Deleted) => ("Session deleted".to_owned(), false),
                    Err(error) => (format!("Failed to delete: {error}"), true),
                }),
                // Not keys of this list (`Picker::scoping`).
                _ => None,
            };
            spot = Some((at, status));
        }
    }

    fn fork(&mut self, wanted: &str) {
        let prompts = match self.agent.fork_prompts() {
            Ok(prompts) => prompts,
            Err(error) => {
                self.say(&format!("error: {error}"));
                return;
            }
        };
        let chosen = match wanted {
            "" if prompts.is_empty() => {
                self.say("No messages to fork from");
                None
            }
            "" if self.interactive() => self.select(
                "Fork from message",
                select::VISIBLE,
                prompts.len(),
                prompts.len() - 1,
                None,
                |index, width| clip(&one_line(&prompts[index]), width),
            ),
            "" => {
                let digits = prompts.len().to_string().len();
                let width = editor::terminal_size().0.saturating_sub(digits + 1);
                for (index, prompt) in prompts.iter().enumerate() {
                    let line = clip(&one_line(prompt), width.max(2));
                    eprintln!("{:>digits$} {line}", index + 1);
                }
                None
            }
            wanted => match wanted.parse::<usize>() {
                Err(_) | Ok(0) => {
                    self.say("error: /fork requires a positive message number");
                    None
                }
                Ok(number) if number > prompts.len() => {
                    self.say(&format!(
                        "error: no message numbered {number} (/fork lists them)"
                    ));
                    None
                }
                Ok(number) => Some(number - 1),
            },
        };
        let Some(index) = chosen else {
            return;
        };
        match self.agent.fork(index) {
            Ok(Some(prompt)) => {
                self.agent.load_model_card();
                self.replaced(&format!(
                    "✓ Forked to new session · {}\nsession {}",
                    model_line(&self.agent),
                    self.agent.session_path().display(),
                ));
                self.set_text(prompt);
            }
            Ok(None) => self.say(&format!(
                "error: no message numbered {} (/fork lists them)",
                index + 1
            )),
            Err(error) => self.say(&format!("error: {error}")),
        }
    }

    fn clone_session(&mut self) {
        match self.agent.clone_session() {
            Ok(true) => {
                self.agent.load_model_card();
                self.replaced(&format!(
                    "✓ Cloned to new session · {}\nsession {}",
                    model_line(&self.agent),
                    self.agent.session_path().display(),
                ));
            }
            Ok(false) => self.say("Nothing to clone yet"),
            Err(error) => self.say(&format!("error: {error}")),
        }
    }

    fn tree(&mut self, mut wanted: &str) {
        // The row the tree shows again at after its summary question or summary is cancelled.
        let mut reopen = None;
        loop {
            let session_tree = match self.agent.tree() {
                Ok(tree) => tree,
                Err(error) => {
                    self.say(&format!("error: {error}"));
                    return;
                }
            };
            let (rows, current) = tree::rows(&session_tree, self.interactive());
            let chosen = match wanted {
                "" if rows.is_empty() => {
                    self.say("No entries in session");
                    None
                }
                "" if self.interactive() => {
                    // As Scramjet's tree, half the terminal.
                    let visible = (editor::terminal_size().1 / 2).max(5);
                    self.select(
                        "Session Tree",
                        visible,
                        rows.len(),
                        reopen.or(current).unwrap_or(rows.len() - 1),
                        None,
                        |index, width| clip(&rows[index].1, width),
                    )
                }
                "" => {
                    let digits = rows.len().to_string().len();
                    let width = editor::terminal_size().0.saturating_sub(digits + 1);
                    for (index, (_, row)) in rows.iter().enumerate() {
                        eprintln!("{:>digits$} {}", index + 1, clip(row, width.max(2)));
                    }
                    None
                }
                wanted => match wanted.parse::<usize>() {
                    Err(_) | Ok(0) => {
                        self.say("error: /tree requires a positive entry number");
                        None
                    }
                    Ok(number) if number > rows.len() => {
                        self.say(&format!(
                            "error: no entry numbered {number} (/tree lists them)"
                        ));
                        None
                    }
                    Ok(number) => Some(number - 1),
                },
            };
            let Some(index) = chosen else {
                return;
            };
            let line = session_tree.entries[rows[index].0].line;
            // As Scramjet asks on the full screen, unless the settings skip the question; piped
            // input moves without one.
            let ask = self.interactive() && !self.settings.skip_branch_summary_prompt;
            let summary = match ask.then(|| self.agent.leaves_branch(line)) {
                Some(Ok(true)) => match self.ask_branch_summary() {
                    Some(summary) => summary,
                    None => {
                        (wanted, reopen) = ("", Some(index));
                        continue;
                    }
                },
                None | Some(Ok(false)) => None,
                Some(Err(error)) => {
                    self.say(&format!("error: {error}"));
                    return;
                }
            };
            let result = match summary.as_deref() {
                Some(focus) => self
                    .during_turn(|agent, ui, cancel| agent.navigate(line, Some(focus), ui, cancel)),
                None => match &mut self.screen {
                    Some(screen) => self
                        .agent
                        .navigate(line, None, &mut screen.link, &self.cancel),
                    None => self.agent.navigate(line, None, &mut self.ui, &self.cancel),
                },
            };
            match result {
                Ok(Navigated::AlreadyThere) => self.say("Already at this point"),
                Ok(Navigated::Moved { prompt }) => {
                    self.replaced("✓ Navigated to selected point");
                    if let Some(prompt) = prompt {
                        self.set_text(prompt);
                    }
                }
                Err(AgentError::Cancelled) => {
                    self.say("Branch summarization cancelled");
                    (wanted, reopen) = ("", Some(index));
                    continue;
                }
                Err(error) => self.say(&format!("error: {error}")),
            }
            return;
        }
    }

    fn compact(&mut self, focus: &str) {
        let result = self.during_turn(|agent, ui, cancel| agent.compact(focus, None, ui, cancel));
        match result {
            Ok(Some(compaction)) => self.ui().compacted(&compaction, None),
            Ok(None) => self.say("Nothing to compact: the conversation is all recent."),
            Err(AgentError::Cancelled) => self.say(&self.ui.dim("(cancelled)")),
            Err(error) => self.say(&format!("error: {error}")),
        }
    }

    /// `!command` runs a command without a model turn; `!!command` keeps it out of the context.
    /// Returns whether there was a command to run.
    fn bang(&mut self, command: &str) -> bool {
        let (include, command) = match command.strip_prefix('!') {
            Some(command) => (false, command),
            None => (true, command),
        };
        let command = command.trim();
        if command.is_empty() {
            return false;
        }
        let result =
            self.during_turn(|agent, ui, cancel| agent.shell(command, include, ui, cancel));
        match result {
            Ok(()) => self.spaced = true,
            Err(error) => self.say(&format!("error: {error}")),
        }
        true
    }

    /// Asks on the full screen whether to summarize the branch `/tree` leaves, as Scramjet does.
    /// `None`: cancelled; otherwise no summary, or one with the additional focus given (empty
    /// for none).
    fn ask_branch_summary(&self) -> Option<Option<String>> {
        const CHOICES: [&str; 3] = ["No summary", "Summarize", "Summarize with custom prompt"];
        const TITLE: &str = "Custom summarization instructions";
        let screen = self.screen.as_ref()?;
        loop {
            let chosen = self.select(
                "Summarize branch?",
                select::VISIBLE,
                CHOICES.len(),
                0,
                None,
                |index, width| clip(CHOICES[index], width),
            );
            match chosen? {
                0 => return Some(None),
                1 => return Some(Some(String::new())),
                _ => {
                    // As in Scramjet, no instructions give the default summary.
                    let dialog =
                        screen::Dialog::Text(TITLE.into(), String::new(), screen::Optional::Yes);
                    if let Some(screen::Answer::Text(Some(focus))) = screen.ask(dialog, None) {
                        return Some(Some(focus));
                    }
                }
            }
        }
    }
}

/// What follows a command's next step, as the list of next steps shows it, with what its set
/// suggests when that would change it.
fn step_value(step: &NextStep) -> String {
    let value = match step.setting {
        None if step.forced => "always continues".to_owned(),
        None => "autopilot decides".to_owned(),
        Some(setting) => {
            let edge = edge_words(setting.edge);
            match (setting.any_other, setting.scope) {
                (true, _) => format!("{edge} · as any other command"),
                (false, Scope::Project) => format!("{edge} · this project"),
                (false, Scope::Global) => format!("{edge} · all projects"),
            }
        }
    };
    match step.suggestion() {
        Some(suggestion) => format!("{value} · suggested: {}", edge_words(suggestion.edge)),
        None => value,
    }
}

/// What follows a step that goes by `edge`, as the next steps' rows and choices word it.
fn edge_words(edge: Edge) -> &'static str {
    match edge {
        Edge::Chain => "continue automatically",
        Edge::Pause => "ask me",
    }
}

/// The row that saves what the command sets suggest for `count` steps.
fn apply_label(count: usize) -> String {
    match count {
        1 => "Apply 1 suggested setting".into(),
        count => format!("Apply {count} suggested settings"),
    }
}

/// The step's command, or what stands for any other.
fn target_name(step: &NextStep) -> String {
    match &step.target {
        Some(target) => format!("/{target}"),
        None => "any other command".into(),
    }
}

/// What `edge` does after `source`, in a sentence.
fn meaning(edge: Edge, source: &str) -> String {
    match edge {
        Edge::Chain => format!("The recommended step runs as soon as {source} finishes."),
        Edge::Pause => "The next steps wait for you to choose, also with autopilot on.".into(),
    }
}

/// The note under a next step: what happens, where that is set, and why the files are not used.
fn step_note(step: &NextStep, steps: &NextSteps, autopilot: bool) -> String {
    let source = format!("/{}", step.source);
    let target = target_name(step);
    if step.forced {
        return format!(
            "{source} always runs {target} next: the command says so, and no setting changes it."
        );
    }
    let when = match &step.target {
        Some(_) => format!("When {source} finishes and recommends {target}"),
        None => {
            format!("When {source} finishes and recommends a step without a setting of its own")
        }
    };
    let what = match step.setting.map(|setting| setting.edge) {
        Some(Edge::Chain) => format!("{when}, it runs at once."),
        Some(Edge::Pause) => {
            format!("{when}, the next steps wait for you to choose, also with autopilot on.")
        }
        None => {
            let state = if autopilot { "on" } else { "off" };
            format!(
                "{when}: with autopilot on, it runs after a 3-second countdown; off, the next \
                 steps wait for you to choose. Autopilot is {state}."
            )
        }
    };
    let file = |scope| {
        let path = match scope {
            Scope::Project => Some(&steps.project),
            Scope::Global => steps.global.as_ref(),
        };
        path.map_or_else(String::new, |path| shortened(&path.display().to_string()))
    };
    let from = match step.setting {
        None => String::new(),
        Some(Setting {
            any_other: true,
            scope,
            ..
        }) => format!(
            " As any other command after {source}, set in {}.",
            file(scope)
        ),
        Some(Setting {
            scope: Scope::Project,
            ..
        }) => format!(" Set for this project in {}.", file(Scope::Project)),
        Some(Setting {
            scope: Scope::Global,
            ..
        }) => format!(" Set for all projects in {}.", file(Scope::Global)),
    };
    let suggests = match &step.suggested {
        Some(suggested) => format!(
            " {} suggests: {}.",
            suggested.set,
            edge_words(suggested.edge)
        ),
        None => String::new(),
    };
    match steps.problems.is_empty() {
        true => what + &from + &suggests,
        false => format!("{what}{suggests} {}", not_used(&steps.problems)),
    }
}

/// Why the `autonomy.yaml` files are not used.
fn not_used(problems: &[String]) -> String {
    format!(
        "Not used now, as {}; until that is fixed, autopilot decides every step.",
        problems.join("; ")
    )
}

/// What a publication tool does: as a row names it, and in a sentence.
fn publishes(tool: &str) -> (&'static str, &'static str) {
    match tool {
        "create_issue" => ("create an issue", "creates an issue"),
        "create_pr" => ("create a pull request", "creates a pull request"),
        "add_issue_comment" => ("comment on an issue", "comments on an issue"),
        _ => ("comment on a pull request", "comments on a pull request"),
    }
}

/// The command set a command is from, which its name begins with.
fn set_of(command: &str) -> &str {
    command.split_once(':').map_or(command, |(set, _)| set)
}

fn asks(publication: Publication) -> &'static str {
    match publication {
        Publication::Ask => "ask me",
        Publication::Auto => "publish without asking",
    }
}

/// Whether a command's publications with a tool ask first, and who says so.
fn approval_value(approval: &Approval, problem: bool) -> String {
    if problem {
        return "ask me · the file has an error".into();
    }
    let from = match (approval.setting, approval.default) {
        (Some(_), _) => "your setting".to_owned(),
        (None, Some(_)) => format!("as {} says", set_of(&approval.command)),
        (None, None) => "by default".to_owned(),
    };
    format!("{} · {from}", asks(approval.applies()))
}

/// The note under a command's publications with a tool: what happens, and who says so.
fn approval_note(approval: &Approval, approvals: &Approvals) -> String {
    let (_, does) = publishes(approval.tool);
    let when = format!(
        "When /{} {does} in the project's own repository",
        approval.command
    );
    if let Some(problem) = &approvals.problem {
        return format!(
            "{when}, it waits for your approval. {}",
            every_one_asks(problem)
        );
    }
    let what = match approval.applies() {
        Publication::Ask => {
            format!("{when}, it shows you what it will publish and waits for your approval.")
        }
        Publication::Auto => format!(
            "{when}, it publishes without showing it to you first; what was written is still \
             checked."
        ),
    };
    let set = set_of(&approval.command);
    let global = (approvals.global.as_ref()).map(|path| shortened(&path.display().to_string()));
    let from = match (approval.setting, approval.default) {
        (Some(_), _) => format!(" Your setting, in {}.", global.unwrap_or_default()),
        (None, Some(_)) => format!(" As {set} says, in its autonomy-defaults.yaml."),
        (None, None) => format!(" {set} says nothing about it, so it asks."),
    };
    what + &from
}

/// Why the file with the publication settings is not used.
fn every_one_asks(problem: &str) -> String {
    format!("Not used now, as {problem}; until that is fixed, every publication asks.")
}

/// The value of `/settings`' row for publishing: how many ask first and how many do not.
fn approvals_summary(approvals: &Approvals) -> String {
    if approvals.problem.is_some() {
        return "ask me: the file has an error".into();
    }
    if approvals.list.is_empty() {
        return "no command publishes".into();
    }
    let count = |publication| {
        (approvals.list.iter())
            .filter(|approval| approval.applies() == publication)
            .count()
    };
    [Publication::Ask, Publication::Auto]
        .into_iter()
        .map(|publication| (asks(publication), count(publication)))
        .filter(|(_, count)| *count > 0)
        .map(|(words, count)| format!("{words}: {count}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The value of `/settings`' row for the next steps: how many go which way.
fn steps_summary(steps: &NextSteps) -> String {
    if !steps.problems.is_empty() {
        return "not used: a file has an error".into();
    }
    if steps.steps.is_empty() {
        return "no command has any".into();
    }
    let count = |edge| {
        (steps.steps.iter())
            .filter(|step| step.setting.is_some_and(|setting| setting.edge == edge))
            .count()
    };
    let parts: Vec<String> = [
        ("continue automatically", count(Edge::Chain)),
        ("ask me", count(Edge::Pause)),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(words, count)| format!("{words}: {count}"))
    .collect();
    let summary = match parts.is_empty() {
        true => "autopilot decides all".to_owned(),
        false => parts.join(" · "),
    };
    match steps.suggested().len() {
        0 => summary,
        suggested => format!("{summary} · suggested: {suggested}"),
    }
}

/// `/session`: the session's name, file and id, its messages and token usage over the current
/// conversation, as Scramjet's `/session` shows them. Ends with a newline.
fn session_info(agent: &Agent, ui: &Terminal) -> String {
    let stats = agent.stats();
    let row = |label: &str, value: String| format!("{} {value}\n", ui.dim(label));
    let heading = |text: &str| format!("{}\n", ui.styled(style::INFO_HEADING, text));
    let mut info = heading("Session Info") + "\n";
    if let Some(name) = agent.session_name() {
        info += &row("Name:", name.to_owned());
    }
    info += &row("File:", agent.session_path().display().to_string());
    info += &row("ID:", agent.session_id());
    info += "\n";
    info += &heading("Messages");
    info += &row("User:", stats.user.to_string());
    info += &row("Assistant:", stats.assistant.to_string());
    info += &row("Tool Calls:", stats.tool_calls.to_string());
    info += &row("Tool Results:", stats.tool_results.to_string());
    info += &row("Total:", stats.total.to_string());
    info += "\n";
    info += &heading("Tokens");
    info += &row("Input:", grouped(stats.input));
    info += &row("Output:", grouped(stats.output));
    if stats.cache_read > 0 {
        info += &row("Cache Read:", grouped(stats.cache_read));
    }
    let total = stats.input + stats.output + stats.cache_read;
    info += &row("Total:", grouped(total));
    info
}

/// The keys `/hotkeys` lists, in Scramjet's sections and words where the key does what
/// Scramjet's does.
const HOTKEYS: [(&str, &[(&str, &str)]); 4] = [
    (
        "Navigation",
        &[
            (
                "Up / Down / Left / Right",
                "Move cursor / browse history (Up on the first line)",
            ),
            ("Ctrl+B / Ctrl+F", "Move cursor left / right"),
            ("Alt+Left / Alt+Right", "Move by word"),
            ("Ctrl+Left / Ctrl+Right", "Move by word"),
            ("Alt+B / Alt+F", "Move by word"),
            ("Home / Ctrl+A", "Start of line"),
            ("End / Ctrl+E", "End of line"),
            ("Ctrl+]", "Jump forward to character"),
            ("Ctrl+Alt+]", "Jump backward to character"),
            (
                "Page Up / Page Down",
                "Scroll the conversation by page (Escape: to its end)",
            ),
            (
                "Ctrl+Home / Ctrl+End",
                "Scroll to the conversation's start / end",
            ),
        ],
    ),
    (
        "Editing",
        &[
            ("Enter", "Send message (during a turn: steer)"),
            ("Backspace", "Delete character backwards"),
            (
                "Shift+Enter / Ctrl+J",
                "New line (also a trailing \\ and Enter)",
            ),
            ("Ctrl+W / Alt+Backspace", "Delete word backwards"),
            ("Alt+D / Alt+Delete", "Delete word forwards"),
            ("Delete / Ctrl+D", "Delete character forwards"),
            ("Ctrl+U", "Delete to start of line"),
            ("Ctrl+K", "Delete to end of line"),
            ("Ctrl+Y", "Paste the most-recently-deleted text"),
            ("Alt+Y", "Cycle through the deleted text after pasting"),
            ("Ctrl+-", "Undo"),
        ],
    ),
    (
        "Other",
        &[
            ("Tab", "Path completion / accept autocomplete"),
            ("Escape", "Cancel autocomplete / interrupt the turn"),
            (
                "Escape Escape",
                "Open /tree on an empty prompt (doubleEscapeAction)",
            ),
            ("Ctrl+C", "Clear editor (first) / exit (second)"),
            ("Ctrl+D", "Exit (when editor is empty)"),
            ("Ctrl+Z", "Suspend to background"),
            ("Shift+Tab", "Cycle thinking level"),
            ("Ctrl+P / Shift+Ctrl+P", "Cycle models"),
            ("Ctrl+L", "Open model selector"),
            ("Ctrl+O", "Toggle tool output expansion"),
            ("Ctrl+T", "Toggle thinking block visibility"),
            ("Ctrl+G", "Edit message in external editor"),
            ("Alt+Enter", "Queue follow-up message (or send)"),
            ("Alt+Up", "Restore queued messages"),
            ("Ctrl+V / Alt+V", "Paste image from clipboard"),
            ("/", "Slash commands"),
            ("!", "Run bash command"),
            ("!!", "Run bash command (excluded from context)"),
        ],
    ),
    (
        "Mouse",
        &[
            ("Wheel", "Scroll the conversation"),
            ("Drag", "Select text; Ctrl+C or a right click copies it"),
            ("Double / triple click", "Select a word / the line"),
            (
                "Right click",
                "Paste text into the prompt (with no selection)",
            ),
        ],
    ),
];

/// `/hotkeys`, as Scramjet's: the keys by section, in two columns.
fn hotkeys(ui: &Terminal) -> String {
    let sections = HOTKEYS.iter();
    let width = (sections.flat_map(|(_, keys)| keys.iter()))
        .map(|(key, _)| key.width())
        .max()
        .unwrap_or(0);
    let mut text = format!("{}\n", ui.styled(style::INFO_HEADING, "Keyboard Shortcuts"));
    for (section, keys) in HOTKEYS {
        text += &format!("\n{}\n", ui.styled(style::INFO_HEADING, section));
        for (key, action) in keys {
            let pad = " ".repeat(width - key.width());
            text += &format!("  {}{pad}  {action}\n", ui.styled(style::CODE, key));
        }
    }
    text
}

/// A count with thousands separators, as Scramjet's `toLocaleString` writes it.
fn grouped(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A `/resume` line: the session's name or first prompt, cut to `width` with its item count and
/// age kept.
fn session_line(session: &SessionInfo, width: usize) -> String {
    let age = std::time::SystemTime::now()
        .duration_since(session.modified)
        .unwrap_or_default();
    let items = match session.item_count {
        1 => "1 item".to_owned(),
        count => format!("{count} items"),
    };
    let suffix = format!(" ({items}, {})", age_label(age));
    let label = session
        .name
        .as_deref()
        .or(session.first_prompt.as_deref())
        .unwrap_or("(no messages)");
    let prompt = one_line(label);
    let prompt = clip(&prompt, width.saturating_sub(suffix.width()).max(2));
    format!("{prompt}{suffix}")
}

/// `sessions` (newest first) in the order of Scramjet's threaded session list, with the tree
/// lines before each: every fork under the session it came from, newest first at each level.
fn threaded(sessions: Vec<SessionInfo>) -> Vec<(SessionInfo, String)> {
    let key = |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or(path.to_owned());
    let keys: Vec<_> = sessions.iter().map(|session| key(&session.path)).collect();
    let parents: Vec<Option<usize>> = (sessions.iter().enumerate())
        .map(|(index, session)| {
            let parent = key(session.parent.as_deref()?);
            keys.iter()
                .position(|key| *key == parent)
                .filter(|&at| at != index)
        })
        .collect();
    let children = |parent: Option<usize>| -> Vec<usize> {
        (0..sessions.len())
            .filter(|&at| parents[at] == parent)
            .collect()
    };
    // Each row's index and its tree lines; a walk from the roots, then from what a cycle of
    // parents kept from them.
    let mut order: Vec<(usize, String)> = Vec::new();
    let mut seen = vec![false; sessions.len()];
    let mut stack: Vec<(usize, Vec<bool>, bool)> = Vec::new();
    let roots = children(None);
    let unreached = (0..sessions.len()).filter(|&at| parents[at].is_some());
    for root in roots.into_iter().chain(unreached) {
        if seen[root] {
            continue;
        }
        stack.push((root, Vec::new(), true));
        while let Some((at, continues, last)) = stack.pop() {
            if std::mem::replace(&mut seen[at], true) {
                continue;
            }
            let mut tree: String = (continues.iter())
                .map(|&continues| if continues { "│  " } else { "   " })
                .collect();
            if !continues.is_empty() {
                tree.push_str(if last { "└─ " } else { "├─ " });
            }
            order.push((at, tree));
            let below: Vec<usize> = (children(Some(at)).into_iter())
                .filter(|&child| !seen[child])
                .collect();
            for (rank, &child) in below.iter().enumerate().rev() {
                // As in Scramjet, a root's own line does not continue past its children.
                let mut continues = continues.clone();
                continues.push(!continues.is_empty() && !last);
                stack.push((child, continues, rank + 1 == below.len()));
            }
        }
    }
    let mut sessions: Vec<Option<SessionInfo>> = sessions.into_iter().map(Some).collect();
    (order.into_iter())
        .filter_map(|(at, tree)| Some((sessions[at].take()?, tree)))
        .collect()
}

/// What `/resume` searches in each session, those matching equally the newest first, as in
/// Scramjet.
fn searched(sessions: &[(SessionInfo, String)]) -> Vec<Haystack> {
    let mut newest: Vec<usize> = (0..sessions.len()).collect();
    newest.sort_by_key(|&at| std::cmp::Reverse(sessions[at].0.modified));
    let mut rank = vec![0; sessions.len()];
    for (order, at) in newest.into_iter().enumerate() {
        rank[at] = order;
    }
    (sessions.iter().zip(rank))
        .map(|((session, _), rank)| Haystack::ranked(&search_text(session), rank))
        .collect()
}

/// What `/resume` searches in a session, as Scramjet does: its id, name, prompts and replies.
fn search_text(session: &SessionInfo) -> String {
    let id = session
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let name = session.name.as_deref().unwrap_or_default();
    format!("{id} {name} {}", session.text)
}

/// Scramjet's session age: `now`, `5m`, `3h`, `2d`, `1w`, `4mo`, `1y`.
fn age_label(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    let days = minutes / 1440;
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ if days < 7 => format!("{days}d"),
        _ if days < 30 => format!("{}w", days / 7),
        _ if days < 365 => format!("{}mo", days / 30),
        _ => format!("{}y", days / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builtins_typed_during_a_turn_stop_it_when_they_replace_the_conversation() {
        let stops = |prompt| Builtin::parse(prompt).map(Builtin::interrupts);
        for prompt in [
            "/session",
            "/hotkeys",
            "/copy",
            "/name",
            "/name  my work ",
            "/model",
            "/model gpt-6-terra",
            "/scoped-models",
            "/autopilot on",
            "/reload",
        ] {
            assert_eq!(stops(prompt), Some(false), "{prompt}");
        }
        for prompt in [
            "/new",
            "/compact keep tests",
            "/resume 2",
            "/fork",
            "/clone",
            "/tree",
            "/quit",
        ] {
            assert_eq!(stops(prompt), Some(true), "{prompt}");
        }
        // These go to the model as they are, as before a turn.
        for prompt in [
            "/session now",
            "/hotkeys all",
            "/copy all",
            "/scoped-models luna",
            "/clone it",
            "/new one",
            "/exit now",
            "/reload all",
            "/sessions",
            "/rs:loop",
            "!ls",
        ] {
            assert_eq!(stops(prompt), None, "{prompt}");
        }
        assert_eq!(
            Builtin::parse("/name  my work "),
            Some(Builtin::Name("my work"))
        );
        assert_eq!(Builtin::parse("/exit"), Some(Builtin::Quit));
    }

    #[test]
    fn hotkeys_line_up_their_actions_within_80_columns() {
        let ui = Terminal::new(std::sync::Arc::default(), false);
        let text = hotkeys(&ui);
        let text = [style::CODE, style::INFO_HEADING, style::RESET]
            .iter()
            .fold(text, |text, code| text.replace(code, ""));
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[..3], ["Keyboard Shortcuts", "", "Navigation"]);
        assert_eq!(
            lines[3],
            "  Up / Down / Left / Right  Move cursor / browse history (Up on the first line)"
        );
        let rows: Vec<_> = lines.iter().filter(|line| line.starts_with("  ")).collect();
        assert_eq!(
            rows.len(),
            HOTKEYS.map(|(_, keys)| keys.len()).iter().sum::<usize>()
        );
        for row in rows {
            assert!(row.width() <= 80, "{row}");
            assert!(
                row[..28].ends_with("  ") && !row[28..].starts_with(' '),
                "{row}"
            );
        }
    }

    #[test]
    fn resume_lines_cut_the_prompt_and_keep_its_count_and_age() {
        let ages = [
            0, 59, 60, 3599, 3600, 86399, 86400, 604800, 2592000, 31536000,
        ];
        let labels: Vec<_> = ages
            .map(|secs| age_label(Duration::from_secs(secs)))
            .to_vec();
        assert_eq!(
            labels,
            [
                "now", "now", "1m", "59m", "1h", "23h", "1d", "1w", "1mo", "1y"
            ]
        );
        let session = SessionInfo {
            path: PathBuf::new(),
            first_prompt: Some("Fix\tthe parser\nthen run the tests".into()),
            text: String::new(),
            name: None,
            item_count: 12,
            modified: std::time::SystemTime::now() - Duration::from_secs(7200),
            parent: None,
        };
        assert_eq!(session_line(&session, 30), "Fix the parser… (12 items, 2h)");
        assert_eq!(
            session_line(&session, 80),
            "Fix the parser then run the tests (12 items, 2h)"
        );
        let named = SessionInfo {
            path: PathBuf::new(),
            first_prompt: session.first_prompt.clone(),
            text: String::new(),
            name: Some("Parser\nwork".into()),
            item_count: 12,
            modified: session.modified,
            parent: None,
        };
        assert_eq!(session_line(&named, 80), "Parser work (12 items, 2h)");
        let empty = SessionInfo {
            first_prompt: None,
            item_count: 1,
            ..session
        };
        assert_eq!(session_line(&empty, 80), "(no messages) (1 item, 2h)");
    }

    #[test]
    fn forks_are_threaded_under_the_sessions_they_came_from() {
        let path = |name: &str| PathBuf::from(format!("/nonexistent/{name}.jsonl"));
        // Newest first: X and Y name each other, and G's parent is not listed.
        let sessions = [
            ("A", None),
            ("B", Some("D")),
            ("C", Some("A")),
            ("D", None),
            ("E", Some("B")),
            ("F", Some("A")),
            ("G", Some("gone")),
            ("X", Some("Y")),
            ("Y", Some("X")),
        ]
        .map(|(name, parent)| SessionInfo {
            path: path(name),
            first_prompt: None,
            text: String::new(),
            name: Some(name.into()),
            item_count: 1,
            modified: std::time::SystemTime::now(),
            parent: parent.map(path),
        });
        let rows: Vec<String> = (threaded(sessions.into()).into_iter())
            .map(|(session, tree)| format!("{tree}{}", session.name.unwrap()))
            .collect();
        assert_eq!(
            rows,
            [
                "A",
                "   ├─ C",
                "   └─ F",
                "D",
                "   └─ B",
                "      └─ E",
                "G",
                "X",
                "   └─ Y"
            ]
        );
        // Deeper levels continue the line of a parent with siblings after it.
        let deep = [
            ("R", None),
            ("P", Some("R")),
            ("Q", Some("R")),
            ("K", Some("P")),
        ]
        .map(|(name, parent)| SessionInfo {
            path: path(name),
            first_prompt: None,
            text: String::new(),
            name: Some(name.into()),
            item_count: 1,
            modified: std::time::SystemTime::now(),
            parent: parent.map(path),
        });
        let rows: Vec<String> = (threaded(deep.into()).into_iter())
            .map(|(session, tree)| format!("{tree}{}", session.name.unwrap()))
            .collect();
        assert_eq!(rows, ["R", "   ├─ P", "   │  └─ K", "   └─ Q"]);
    }

    #[test]
    fn sessions_matching_equally_are_found_newest_first() {
        let now = std::time::SystemTime::now();
        let session = |name: &str, parent: Option<&str>, age: u64| SessionInfo {
            path: PathBuf::from(format!("/nonexistent/{name}.jsonl")),
            first_prompt: None,
            text: "fix the parser".into(),
            name: None,
            item_count: 1,
            modified: now - Duration::from_secs(age),
            parent: parent.map(|parent| PathBuf::from(format!("/nonexistent/{parent}.jsonl"))),
        };
        // The fork, the newest, is threaded under its parent but found before it.
        let sessions = threaded(vec![
            session("aaaa", None, 10),
            session("bbbb", None, 20),
            session("cccc", Some("bbbb"), 5),
        ]);
        let ids = |rows: Vec<usize>| -> Vec<String> {
            (rows.into_iter())
                .map(|at| {
                    sessions[at]
                        .0
                        .path
                        .file_stem()
                        .unwrap()
                        .display()
                        .to_string()
                })
                .collect()
        };
        let texts = searched(&sessions);
        assert_eq!(ids((0..3).collect()), ["aaaa", "bbbb", "cccc"]);
        use crate::agent_cli::fuzzy::{filter, parse};
        let found = filter(&parse("parser"), &texts);
        assert_eq!(ids(found), ["cccc", "aaaa", "bbbb"]);
    }

    #[test]
    fn token_counts_are_grouped_by_thousands() {
        let counts = [0, 999, 1_000, 15_224, 1_234_567].map(grouped);
        assert_eq!(counts, ["0", "999", "1,000", "15,224", "1,234,567"]);
    }
}
