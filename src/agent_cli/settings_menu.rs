//! `/settings`, as Scramjet's: the settings in a list, each changed with Enter and saved at
//! once, with the next steps of commands, publication approval and what is installed.
use super::interactive::{Interactive, completions};
use super::palette::Background;
use super::screen::{Answer, Dialog, Optional};
use super::settings::{ROWS, Row, scramjet_level};
use super::text::clip;
use super::theme::{Color, Listed, Mode, TERMINAL_COLORS};
use super::theme_editor::ThemeEditor;
use super::{background, editor, installed, palette, select, style};
use scramjet_rs::agent::{
    Approval, Approvals, CODING_TOOLS, Choice, Edge, Installed, NextStep, NextSteps, Problem,
    Publication, Scope, Setting, agent_directory, shortened, unavailable, write_replacing,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthStr;

const NO_LEVELS: &str = "The model has no thinking levels to choose from.";
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
const NEW_THEME: &str = "New theme…";
/// How long the theme list waits for the terminal's colors, which the theme editor starts from.
const COLORS_WAIT: Duration = Duration::from_millis(100);
const NEW_THEME_NOTE: &str = "Make a theme of your own: choose how many main colors and how they \
    go together, then roll until you like it, lock the colors you like and roll again. A sample \
    session shows it as you go.";

/// What the theme editor's rolls start from: a new one each time.
fn seed() -> u64 {
    (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
        .map_or(0, |since| since.as_nanos() as u64)
        ^ u64::from(std::process::id())
}

/// The hint of a list under `/settings`' own.
fn back_keys(confirm: &str) -> String {
    select::hint(confirm, &[], "back")
}

/// What a theme is, as `/settings` says under its row.
fn theme_note(listed: &Listed) -> String {
    let note = match (listed.name.as_str(), &listed.file) {
        (_, Some(file)) => format!("From {}.", shortened(&file.display().to_string())),
        (TERMINAL_COLORS, None) => {
            return "The terminal's own 16 colors, which follow its color scheme.".into();
        }
        ("scramjet-dark", None) => {
            "Scramjet's theme, in exact colors, for a dark background.".into()
        }
        _ => "Pi's light theme, in exact colors, for a light background.".into(),
    };
    match Mode::detect() {
        Mode::TrueColor => note,
        Mode::Indexed => format!("{note} This terminal gets the nearest of its 256 colors."),
    }
}

/// The theme in use opened in the theme editor, as the row after New theme… offers it.
struct Editable {
    row: String,
    note: String,
    /// Its name and file, which saving under its name replaces.
    own: Option<(String, PathBuf)>,
    editor: Box<ThemeEditor>,
}

/// `listed` in the theme editor: as it was saved when the editor made it, else as a new theme
/// starting from its colors, those of the terminal's in the colors it answered it shows them in;
/// `None` when it has no exact colors to start from.
fn editable(listed: &Listed) -> Option<Editable> {
    let name = &listed.name;
    let terminal = background::terminal();
    let text = (listed.file.as_ref()).and_then(|file| std::fs::read_to_string(file).ok());
    if let Some(editor) = text.and_then(|text| ThemeEditor::open(&text, seed(), terminal)) {
        let file = listed.file.clone()?;
        let shown = shortened(&file.display().to_string());
        return Some(Editable {
            row: format!("Edit {name}…"),
            note: format!(
                "Open {name} in the theme editor as it was saved: its main colors and locks, \
                 harmony, background, contrast and picked roles. Saved as {name} again, it \
                 replaces {shown}."
            ),
            own: Some((name.clone(), file)),
            editor: Box::new(editor),
        });
    }
    let answers = background::answers();
    let shown = listed.theme.exactly(|color| answers.shown(color));
    let editor = ThemeEditor::from_theme(name, &shown, seed(), terminal)?;
    let how = match shown != listed.theme {
        true => " in the colors this terminal shows it in",
        false => "",
    };
    Some(Editable {
        row: format!("New theme from {name}…"),
        note: format!(
            "Start the theme editor from {name}{how}: its main colors read from its accent, \
             borders, headings and code, each role keeping {name}'s color until a roll or a step \
             changes what it follows. It is saved as a new theme; {name} stays as it is."
        ),
        own: None,
        editor: Box::new(editor),
    })
}

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
                    row => row.description(),
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
            let picker = picker
                .noting(select::hint("change", &[], "close"), notes)
                .held();
            let Some(index) = screen.pick(picker) else {
                break;
            };
            at = index;
            match ROWS[index] {
                Row::ThinkingLevel => self.choose_thinking_level(),
                Row::Tools => self.choose_tools(),
                Row::Theme => self.choose_theme(),
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
            self.agent.set_compacting(self.settings.compacting);
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

    /// The model's levels to choose from; the one chosen is the session's and the
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
        let Some(index) = screen.pick(picker.noting(back_keys("select"), Vec::new()).held()) else {
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

    /// The themes; Enter shows everything in one at once and keeps the list open to try the
    /// next, and the one chosen last stays.
    fn choose_theme(&mut self) {
        if let Some(screen) = &self.screen {
            screen.link.to_terminal(background::color_questions());
            // A terminal answers within a frame, or not at all (tmux); Scramjet waits 100 ms
            // for its OSC 11 answer. The answers of an earlier opening stand meanwhile.
            let asked = Instant::now();
            while !background::answers().has_colors() && asked.elapsed() < COLORS_WAIT {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        // As the files are now, the one in use read again as it changed included.
        self.discover_themes();
        let mut at = None;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let listed = &self.themes.listed;
            let current =
                (listed.iter()).position(|known| known.name == self.settings.theme_name());
            let editable = current.and_then(|current| editable(&listed[current]));
            let notes = (listed.iter().map(theme_note))
                .chain([NEW_THEME_NOTE.to_owned()])
                .chain(editable.iter().map(|editable| editable.note.clone()))
                .collect();
            let picker = select::Picker::select(
                "Theme",
                select::VISIBLE,
                listed.len() + 1 + usize::from(editable.is_some()),
                at.or(current).unwrap_or(0),
                current,
                |index, width| match listed.get(index) {
                    Some(listed) => clip(&listed.name, width),
                    None if index == listed.len() => clip(NEW_THEME, width),
                    None => clip(editable.as_ref().map_or("", |e| e.row.as_str()), width),
                },
            );
            let Some(index) = screen.pick(picker.noting(back_keys("use"), notes).held()) else {
                return;
            };
            at = Some(index);
            if index >= listed.len() {
                let made = match editable.filter(|_| index > listed.len()) {
                    Some(editable) => self.make_theme(editable.own, editable.editor),
                    None => self.make_theme(None, Box::new(self.new_theme_editor())),
                };
                if let Some(name) = made {
                    at = (self.themes.listed.iter()).position(|known| known.name == name);
                }
                continue;
            }
            let name = listed[index].name.clone();
            let saved = self.settings.set_theme(&name);
            self.use_theme();
            if let Err(error) = saved {
                self.say(&format!("Error: settings not saved: {error}"));
            }
        }
    }

    /// The theme editor `editor`, on a new theme or on `own`, the name and file of a theme it
    /// made; a theme saved from it is named, written to the `themes` directory of Scramjet's
    /// agent directory, where Scramjet finds it too, or over `own`'s file when saved under its
    /// name, and used. Escape on the name goes back to the editor as it was; Escape in the
    /// editor goes back to the theme in use.
    fn make_theme(
        &mut self,
        own: Option<(String, PathBuf)>,
        mut editor: Box<ThemeEditor>,
    ) -> Option<String> {
        let Some(directory) = agent_directory().map(|directory| directory.join("themes")) else {
            self.say("Error: a theme cannot be saved: HOME is not set");
            return None;
        };
        loop {
            let saved = self.screen.as_ref()?.edit_theme(editor);
            let Some(saved) = saved else {
                self.use_theme();
                return None;
            };
            editor = saved;
            let own_name = own.as_ref().map(|(name, _)| name.as_str());
            let Some(name) = self.theme_name(&directory, own_name) else {
                continue;
            };
            let (file, saved) = match &own {
                Some((own, file)) if *own == name => (file.clone(), "Replaced"),
                _ => (directory.join(format!("{name}.json")), "Saved"),
            };
            let shown = shortened(&file.display().to_string());
            if let Err(error) = write_replacing(&file, &editor.file_text(&name)) {
                self.say(&format!("Error: theme not saved in {shown}: {error}"));
                continue;
            }
            self.discover_themes();
            let set = self.settings.set_theme(&name);
            self.use_theme();
            self.say_styled(style::MUTED, &format!("{saved} theme {name} in {shown}"));
            if let Err(error) = set {
                self.say(&format!("Error: settings not saved: {error}"));
            }
            return Some(name);
        }
    }

    /// A theme editor on a new theme, on a background as the theme in use: its own, dark or
    /// light, else the terminal's.
    fn new_theme_editor(&self) -> ThemeEditor {
        let terminal = background::terminal();
        let start = match (
            self.themes.theme(self.settings.theme_name()).background(),
            terminal,
        ) {
            (Color::Rgb(r, g, b), _) if palette::is_dark((r, g, b)) => Background::Dark,
            (Color::Rgb(..), _) => Background::Light,
            (_, Some(rgb)) => Background::Terminal(rgb),
            _ if self.settings.theme_name() == "pi-light" => Background::Light,
            _ => Background::Dark,
        };
        ThemeEditor::new(seed(), start, terminal)
    }

    /// A name for a theme saved in `directory`, typed: one no other theme has, which is also its
    /// file's name, or `own`, the name of the theme edited, suggested first; `None` when
    /// cancelled.
    fn theme_name(&self, directory: &Path, own: Option<&str>) -> Option<String> {
        let taken = |name: &str| {
            own != Some(name)
                && (self.themes.get(name).is_some()
                    || directory.join(format!("{name}.json")).exists())
        };
        let suggested = (1..)
            .map(|number| match number {
                1 => "my-theme".to_owned(),
                number => format!("my-theme-{number}"),
            })
            .find(|name| !taken(name))
            .unwrap_or_default();
        let mut title = match own {
            Some(own) => format!("Name the theme: {own} replaces it, another name makes a copy"),
            None => "Name the theme".to_owned(),
        };
        let mut typed = own.map_or(suggested, str::to_owned);
        loop {
            let dialog = Dialog::Text(title.clone(), typed.clone(), Optional::No);
            let Some(Answer::Text(Some(text))) = self.screen.as_ref()?.ask(dialog, None) else {
                return None;
            };
            typed = text.trim().to_owned();
            let problem = match typed.as_str() {
                "" => "Name the theme",
                name if name.starts_with('.') || name.contains(['/', '\\']) => {
                    "A name cannot start with a dot or hold a slash"
                }
                name if name.chars().any(char::is_control) || name.len() > 64 => {
                    "A name is up to 64 characters, without control characters"
                }
                name if taken(name) => "That name is taken: choose another",
                _ => return Some(typed),
            };
            title = problem.to_owned();
        }
    }

    /// The coding tools, each switched on or off with Enter; the agent has them from its next
    /// request on. One whose program is missing and cannot be downloaded stays off.
    fn choose_tools(&mut self) {
        let mut at = 0;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let on = |tool: &str| self.settings.tools.iter().any(|known| known == tool);
            let missing = unavailable;
            let rows: Vec<String> = (CODING_TOOLS.iter())
                .map(|tool| format!("[{}] {tool}", if on(tool) { "x" } else { " " }))
                .collect();
            let notes = (CODING_TOOLS.iter())
                .map(|tool| match missing(tool) {
                    Some(program) => format!(
                        "Needs {program}, which is not installed and SCRAMJET_OFFLINE keeps from \
                         downloading."
                    ),
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
            let Some(index) =
                screen.pick(picker.noting(back_keys("switch on or off"), notes).held())
            else {
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
            let picker = picker
                .headed(headings)
                .noting(back_keys("change"), notes)
                .held();
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
        let Some(index) = screen.pick(picker.noting(back_keys("select"), notes).held()) else {
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
        let Some(index) = screen.pick(picker.noting(back_keys("select"), notes).held()) else {
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
            let picker = picker
                .headed(headings)
                .noting(back_keys("change"), notes)
                .held();
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
        let Some(index) = screen.pick(picker.noting(back_keys("select"), notes).held()) else {
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
            let picker = (picker.headed(listing.headings)).noting(back_keys("edit"), listing.notes);
            let Some(index) = screen.pick(picker.held()) else {
                return;
            };
            at = index;
            if let Some(file) = &listing.files[index] {
                match installed::broken_link(file) {
                    Some(target) => self.remove_link(file, &target),
                    None if listing.approved[index] && self.forget_approval(file) => {}
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
                self.say_styled(style::MUTED, &note);
            }
            false => self.reload(),
        }
    }

    /// Asks whether to open the project agent `file` or forget the user's approval of running
    /// the project's agents, and forgets it when chosen; `false`: open the file.
    fn forget_approval(&mut self, file: &Path) -> bool {
        let Some(screen) = &self.screen else {
            return true;
        };
        let choices = [
            Choice {
                label: format!("Open {}", shortened(&file.display().to_string())),
                description: None,
                value: String::new(),
            },
            Choice {
                label: "Forget the approval of this project's agents".to_owned(),
                description: Some(
                    "The next subagent call naming one of them asks again before it runs."
                        .to_owned(),
                ),
                value: String::new(),
            },
        ];
        let question = select::Picker::question("Open it, or forget the approval?", &choices, None);
        match screen.pick(question.held()) {
            Some(0) => false,
            Some(_) => {
                match self.agent.forget_project_agents() {
                    Ok(0) => self.say("No approval of this project's agents to forget"),
                    Ok(_) => self
                        .say_styled(style::MUTED, "Forgot the approval of this project's agents"),
                    Err(error) => self.say(&format!("Error: the approval stays: {error}")),
                }
                true
            }
            None => true,
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
        self.say_styled(style::MUTED, &format!("Removed {link}, a link to {target}"));
        match self.in_turn {
            true => self.say_styled(style::MUTED, "Run /reload once the turn ends"),
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
