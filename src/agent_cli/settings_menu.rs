//! `/settings`, as Scramjet's: the settings in a list, each changed with Enter and saved at
//! once, with the next steps of commands, publication approval and what is installed.
use super::interactive::{Interactive, completions};
use super::settings::{ROWS, Row, scramjet_level};
use super::text::clip;
use super::{editor, installed, select};
use scramjet_rs::agent::{
    Approval, Approvals, CODING_TOOLS, Choice, Edge, Installed, NextStep, NextSteps, Problem,
    Publication, Scope, Setting, search_program, shortened,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};
use unicode_width::UnicodeWidthStr;

const SETTINGS_KEYS: &str = "↑↓ move · Enter change · Esc close";
const NO_LEVELS: &str = "The model's card lists no thinking levels to choose from.";
const BACK_KEYS: &str = "↑↓ move · Enter select · Esc back";
const TOOLS_KEYS: &str = "↑↓ move · Enter switch on or off · Esc back";
const CHANGE_KEYS: &str = "↑↓ move · Enter change · Esc back";
const INSTALLED_KEYS: &str = "↑↓ move · Enter edit · Esc back";
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

impl Interactive {
    /// `/settings`, as Scramjet's: the settings in a list, each changed with Enter, saved and
    /// used at once. Without the full screen they are printed.
    pub(super) fn settings(&mut self) {
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
            let command_line_tools = self.agent.command_line_tools();
            let notes = (ROWS.iter())
                .map(|row| match row {
                    Row::ThinkingLevel if !levels => NO_LEVELS.to_owned(),
                    Row::Tools if let Some(tools) = &command_line_tools => format!(
                        "{} This run has those the command line gave: {}.",
                        row.description(),
                        if tools.is_empty() {
                            "none".to_owned()
                        } else {
                            tools.join(", ")
                        }
                    ),
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
                        self.say(&format!("Error: {error}"));
                    }
                }
                row => {
                    if let Err(error) = self.settings.cycle(row) {
                        self.say(&format!("Error: settings not saved: {error}"));
                    }
                }
            }
            self.hide_thinking
                .store(self.settings.hide_thinking_block, Ordering::Relaxed);
            self.ui.markdown_reads = self.settings.markdown_reads;
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
            return self.say(&format!("Error: {error}"));
        }
        if let Err(error) = self.settings.set_default_thinking_level(level) {
            self.say(&format!("Error: settings not saved: {error}"));
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
                self.say(&format!("Error: settings not saved: {error}"));
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
            self.say(&format!("Error: suggested settings not saved: {error}"));
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
            self.say(&format!("Error: next step not saved: {error}"));
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
            self.say(&format!("Error: publishing not saved: {error}"));
        }
    }

    /// What the session loaded, each with where it comes from, under headings, the problems
    /// first; the notes say more, and Enter opens a row's file in the user's editor
    /// (`edit_installed`), the list then showing that file's row again, or, for a link that
    /// leads nowhere, offers to remove it (`remove_link`).
    fn show_installed(&mut self) {
        let mut at = 0;
        let mut edited: Option<PathBuf> = None;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let found = self.agent.installed();
            let problems = self.problems(&found);
            let listing = installed::listing(&self.loaded(&found, &problems));
            if let Some(file) = edited.take() {
                at = listing.row_of(&file, at).unwrap_or(at);
            }
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
            let picker = (picker.headed(listing.headings)).noting(INSTALLED_KEYS, listing.notes);
            let Some(index) = screen.pick(picker.held()) else {
                return;
            };
            at = index;
            if let Some(file) = &listing.files[index] {
                match installed::broken_link(file) {
                    Some(target) => self.remove_link(file, &target),
                    None => {
                        self.edit_installed(file);
                        edited = Some(file.clone());
                    }
                }
            }
        }
    }

    /// Opens `file` in `$VISUAL` or `$EDITOR`; when it changed, reads what was loaded again, as
    /// `/reload` does, or, while a turn waits (`/reload` waits for turns), says to run it after.
    fn edit_installed(&mut self, file: &Path) {
        let Some(screen) = &self.screen else {
            return;
        };
        let before = std::fs::read(file).ok();
        if let Err(error) = screen.edit(file) {
            return self.say(&format!("Error: {error}"));
        }
        if std::fs::read(file).ok() == before {
            return;
        }
        match self.in_turn {
            true => {
                let shown = shortened(&file.display().to_string());
                let note = format!("{shown} changed; run /reload once the turn ends");
                self.say(&self.ui.dim(&note));
            }
            false => self.reload(),
        }
    }

    /// Removes the link `file`, which leads to `target`, once the user confirms and while it
    /// still leads nowhere, then reads what was loaded again as `edit_installed` does.
    fn remove_link(&mut self, file: &Path, target: &Path) {
        let Some(screen) = &self.screen else {
            return;
        };
        let shown = |path: &Path| shortened(&path.display().to_string());
        let (link, target) = (shown(file), shown(target));
        let choices = [
            Choice {
                label: "Keep it".to_owned(),
                description: None,
                value: String::new(),
            },
            Choice {
                label: format!("Remove {link}"),
                description: Some(format!(
                    "It leads to {target}, which is not there. Only the link goes."
                )),
                value: String::new(),
            },
        ];
        let question = select::Picker::question("Remove this broken link?", &choices, None);
        if screen.pick(question.held()) != Some(1) {
            return;
        }
        if installed::broken_link(file).is_none() {
            return self.say(&format!("{link} no longer leads nowhere; left as it is"));
        }
        if let Err(error) = std::fs::remove_file(file) {
            return self.say(&format!("Error: {link} not removed: {error}"));
        }
        self.say(&self.ui.dim(&format!("Removed {link}, a link to {target}")));
        match self.in_turn {
            true => self.say(&self.ui.dim("Run /reload once the turn ends")),
            false => self.reload(),
        }
    }

    fn loaded<'a>(
        &'a self,
        found: &'a Installed,
        problems: &'a [Problem],
    ) -> installed::Loaded<'a> {
        installed::Loaded {
            installed: found,
            commands: self.agent.commands().list(),
            skills: self.agent.skills().list(),
            settings: self.settings.path(),
            problems,
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
