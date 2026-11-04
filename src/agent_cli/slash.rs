//! The built-in commands: `/export`, `/import`, `/copy`, `/clear`, `/rename`, `/status`,
//! `/hotkeys`, `/compact`, `/autopilot`, `/reload`, and `!` commands. `/settings` is in
//! `settings_menu.rs`; `/resume`, `/fork`, `/branch`, `/clone` and `/tree` in `sessions.rs`;
//! `/model` and `/scoped-models` in `scope.rs`.
use super::interactive::{Interactive, completions, model_line, saved_line};
use super::terminal::{self, Terminal};
use super::text::clip;
use super::{clipboard, completion, export, style};
use scramjet_rs::agent::{Agent, AgentError};
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};
use unicode_width::UnicodeWidthStr;

/// A built-in command as typed, with its argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Builtin<'a> {
    Settings,
    Export(&'a str),
    Import(&'a str),
    Copy,
    /// With the new session's name, if given.
    Clear(&'a str),
    Rename(&'a str),
    Status,
    Hotkeys,
    Resume(&'a str),
    Fork(&'a str),
    /// With the new session's name, if given.
    Branch(&'a str),
    Tree(&'a str),
    Model(&'a str),
    ScopedModels,
    Compact(&'a str),
    Autopilot(&'a str),
    Reload,
    Exit,
}

impl<'a> Builtin<'a> {
    /// The built-in command `prompt` runs, if any; `!` commands are not among them.
    pub(super) fn parse(prompt: &'a str) -> Option<Self> {
        let prompt = prompt.trim();
        let (name, argument) = prompt
            .split_once(char::is_whitespace)
            .unwrap_or((prompt, ""));
        let argument = argument.trim();
        Some(match completion::listed(name.strip_prefix('/')?) {
            "settings" if argument.is_empty() => Self::Settings,
            "export" => Self::Export(argument),
            "import" => Self::Import(argument),
            "copy" if argument.is_empty() => Self::Copy,
            "clear" => Self::Clear(argument),
            "rename" => Self::Rename(argument),
            "status" if argument.is_empty() => Self::Status,
            "hotkeys" if argument.is_empty() => Self::Hotkeys,
            "resume" => Self::Resume(argument),
            "fork" => Self::Fork(argument),
            "branch" => Self::Branch(argument),
            "tree" => Self::Tree(argument),
            "model" => Self::Model(argument),
            "scoped-models" if argument.is_empty() => Self::ScopedModels,
            "compact" => Self::Compact(argument),
            "autopilot" => Self::Autopilot(argument),
            "reload" if argument.is_empty() => Self::Reload,
            "exit" if argument.is_empty() => Self::Exit,
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
                | Self::Export(_)
                | Self::Copy
                | Self::Rename(_)
                | Self::Status
                | Self::Hotkeys
                | Self::Model(_)
                | Self::ScopedModels
                | Self::Autopilot(_)
                | Self::Reload
        )
    }
}

impl Interactive {
    /// Runs `prompt` if it is a built-in command other than `/exit`, or a `!` command; returns
    /// whether it was.
    pub(super) fn builtin(&mut self, prompt: &str) -> bool {
        if let Some(command) = prompt.strip_prefix('!') {
            return self.bang(command);
        }
        match Builtin::parse(prompt) {
            Some(Builtin::Settings) => self.settings(),
            Some(Builtin::Export(path)) => self.export(path),
            Some(Builtin::Import(path)) => self.import(path),
            Some(Builtin::Copy) => self.copy_reply(),
            Some(Builtin::Clear(name)) => {
                if self.new_session() && !name.is_empty() {
                    self.name(name);
                }
            }
            Some(Builtin::Rename(name)) => self.name(name),
            Some(Builtin::Status) => self.say(session_info(&self.agent, &self.ui).trim_end()),
            Some(Builtin::Hotkeys) => self.say(hotkeys(&self.ui).trim_end()),
            Some(Builtin::Resume(wanted)) => {
                self.resume(wanted);
            }
            Some(Builtin::Fork(wanted)) => self.fork(wanted),
            Some(Builtin::Branch(name)) => self.clone_session(name),
            Some(Builtin::Tree(wanted)) => self.tree(wanted),
            Some(Builtin::Model(wanted)) => self.model(wanted),
            Some(Builtin::ScopedModels) => self.scoped_models(),
            Some(Builtin::Compact(focus)) => self.compact(focus),
            Some(Builtin::Autopilot(wanted)) => self.autopilot(wanted),
            Some(Builtin::Reload) => self.reload(),
            Some(Builtin::Exit) | None => return false,
        }
        true
    }

    /// `/clear`, also before a next step that wants a fresh session. Returns whether it started.
    pub(super) fn new_session(&mut self) -> bool {
        match self.agent.new_session() {
            Ok(()) => {
                let session = saved_line(&self.agent);
                self.replaced(&format!("✓ New session started\n{session}"));
                true
            }
            Err(error) => {
                self.say(&format!("Error: {error}"));
                false
            }
        }
    }

    pub(super) fn name(&mut self, name: &str) {
        if name.is_empty() {
            match self.agent.session_name() {
                Some(current) => self.say_styled(style::MUTED, &format!("Session name: {current}")),
                None => self.say("Usage: /rename <name>"),
            }
        } else {
            match self.agent.set_session_name(name) {
                Ok(()) => self.say_styled(style::MUTED, &format!("Session name set: {name}")),
                Err(error) => self.say(&format!("Error: {error}")),
            }
        }
    }

    /// `/export [path]`, as Scramjet's: the current branch as a web page, or, to a path ending in
    /// `.jsonl`, as a session file of its own.
    fn export(&mut self, argument: &str) {
        let path = path_argument(argument);
        let written = match path {
            _ if self.agent.conversation().is_empty() => Ok(None),
            Some(path) if path.ends_with(".jsonl") => std::path::absolute(path)
                .map_err(AgentError::from)
                .and_then(|path| Ok(self.agent.export_session(&path)?.then_some(path))),
            path => self.export_page(path).map(Some),
        };
        match written {
            Ok(Some(path)) => {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let said = format!("Session exported to: {}", path.display());
                self.say_styled(style::MUTED, &said)
            }
            Ok(None) => self.say("Nothing to export yet"),
            Err(error) => self.say(&format!("Error: failed to export the session: {error}")),
        }
    }

    /// Writes the current branch's page to `path`, by default named for the session in the
    /// current directory, as Scramjet names it.
    fn export_page(&self, path: Option<&str>) -> Result<PathBuf, AgentError> {
        let id = self.agent.session_id();
        let path = match path {
            Some(path) => PathBuf::from(path),
            None => PathBuf::from(format!("scramjet-rs-session-{id}.html")),
        };
        let path = std::path::absolute(path)?;
        let tools = self.agent.tool_definitions();
        let setup = export::Setup {
            instructions: &self.agent.instructions(),
            tools: &tools,
        };
        let page = export::page(&self.agent.branch()?, &id, Some(setup));
        std::fs::write(&path, page)?;
        Ok(path)
    }

    /// `/import <path.jsonl>`, as Scramjet's: continues in a copy of a session file, once the user
    /// confirms on the full screen; the current session stays as it is.
    fn import(&mut self, argument: &str) {
        let Some(path) = path_argument(argument) else {
            return self.say("Usage: /import <path.jsonl>");
        };
        if self.interactive() {
            let title = format!("Replace current session with {path}?");
            let choices = ["Yes", "No"];
            let row = |index: usize, width| clip(choices[index], width);
            if self.select(&title, 2, 2, 0, None, row) != Some(0) {
                return self.say("Import cancelled");
            }
        }
        match self.agent.import_session(Path::new(path)) {
            Ok(()) => {
                self.agent.load_model_card();
                self.remember_prompts();
                self.replaced(&format!(
                    "✓ Imported session from {path} · {}\n{}",
                    model_line(&self.agent),
                    saved_line(&self.agent),
                ));
            }
            Err(error) => self.say(&format!("Error: failed to import the session: {error}")),
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
                self.say_styled(style::MUTED, "Copied last agent message to clipboard");
            }
            Err(error) => self.say(&format!("Error: {error}")),
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
            return self.say(&format!("Error: {error}"));
        }
        let state = if self.agent.autopilot() { "on" } else { "off" };
        self.say_styled(style::MUTED, &format!("Autopilot is {state}"));
    }

    /// `/reload`, as Scramjet's: the settings, the command sets with their commands and agents,
    /// the skills and the context files read again, and what is wrong in them said as at start.
    pub(super) fn reload(&mut self) {
        self.save_hide_thinking();
        let theme = self.settings.theme.clone();
        self.settings.reload();
        if self.settings.theme != theme {
            style::set(&self.settings.theme());
        }
        let settings = &self.settings;
        (self.hide_thinking).store(settings.hide_thinking_block, Ordering::Relaxed);
        self.ui.markdown_reads = settings.markdown_reads;
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
        self.say_styled(style::MUTED, done);
    }

    fn compact(&mut self, focus: &str) {
        let result = self.during_turn(|agent, ui, cancel| agent.compact(focus, None, ui, cancel));
        match result {
            Ok(Some(compaction)) => self.ui().compacted(&compaction, None),
            Ok(None) => self.say("Nothing to compact: the conversation is all recent."),
            Err(AgentError::Cancelled) => self.say_styled(style::MUTED, "(cancelled)"),
            Err(error) => self.say(&format!("Error: {error}")),
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
            Err(error) => self.say(&format!("Error: {error}")),
        }
        true
    }
}

/// `/session`: the session's name, file and id, its messages, token usage and cost over the
/// current conversation (subagents' too, in the cost), as Scramjet's `/session` shows them. Ends
/// with a newline.
fn session_info(agent: &Agent, ui: &Terminal) -> String {
    let stats = agent.stats();
    let row = |label: &str, value: String| format!("{} {value}\n", ui.dim(label));
    let heading = |text: &str| format!("{}\n", ui.styled(style::INFO_HEADING, text));
    let mut info = heading("Session Info") + "\n";
    if let Some(name) = agent.session_name() {
        info += &row("Name:", name.to_owned());
    }
    // Scramjet's says `In-memory`.
    let file = match agent.session_saved() {
        true => agent.session_path().display().to_string(),
        false => "not saved (--no-session)".to_owned(),
    };
    info += &row("File:", file);
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
    let tokens = stats.tokens;
    info += &row("Input:", grouped(tokens.input));
    info += &row("Output:", grouped(tokens.output));
    if tokens.cache_read > 0 {
        info += &row("Cache Read:", grouped(tokens.cache_read));
    }
    if tokens.cache_write > 0 {
        info += &row("Cache Write:", grouped(tokens.cache_write));
    }
    let total = tokens.input + tokens.output + tokens.cache_read + tokens.cache_write;
    info += &row("Total:", grouped(total));
    if let Some(spent) = terminal::spent(tokens.cost, tokens.unpriced) {
        info += "\n";
        info += &heading("Cost");
        info += &row("Total:", spent);
    }
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
/// The path a command's argument starts with, as Scramjet reads one: up to the first space, or
/// within quotes; `None` when there is none.
fn path_argument(argument: &str) -> Option<&str> {
    let quote = argument.chars().next().filter(|c| matches!(c, '"' | '\''));
    let path = match quote {
        Some(quote) => argument[1..].split_once(quote)?.0,
        None => argument.split_whitespace().next()?,
    };
    Some(path).filter(|path| !path.is_empty())
}

pub(super) fn grouped(count: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_argument_is_its_first_word_or_quoted() {
        assert_eq!(path_argument(""), None);
        assert_eq!(path_argument("out.html"), Some("out.html"));
        assert_eq!(path_argument("out.jsonl now"), Some("out.jsonl"));
        assert_eq!(path_argument("\"my file.html\" x"), Some("my file.html"));
        assert_eq!(path_argument("'a b.jsonl'"), Some("a b.jsonl"));
        assert_eq!(path_argument("\"open"), None);
        assert_eq!(path_argument("\"\""), None);
    }

    #[test]
    fn builtins_typed_during_a_turn_stop_it_when_they_replace_the_conversation() {
        let stops = |prompt| Builtin::parse(prompt).map(Builtin::interrupts);
        for prompt in [
            "/status",
            "/export",
            "/export out.jsonl",
            "/hotkeys",
            "/copy",
            "/rename",
            "/rename  my work ",
            "/model",
            "/model gpt-6-terra",
            "/scoped-models",
            "/autopilot on",
            "/reload",
        ] {
            assert_eq!(stops(prompt), Some(false), "{prompt}");
        }
        for prompt in [
            "/clear",
            "/import old.jsonl",
            "/compact keep tests",
            "/resume 2",
            "/fork",
            "/branch",
            "/tree",
            "/exit",
        ] {
            assert_eq!(stops(prompt), Some(true), "{prompt}");
        }
        // These go to the model as they are, as before a turn.
        for prompt in [
            "/status now",
            "/hotkeys all",
            "/copy all",
            "/scoped-models luna",
            "/exit now",
            "/quit now",
            "/reload all",
            "/sessions",
            "/Clear",
            "/rs:loop",
            "clear",
            "!ls",
        ] {
            assert_eq!(stops(prompt), None, "{prompt}");
        }
        assert_eq!(
            Builtin::parse("/rename  my work "),
            Some(Builtin::Rename("my work"))
        );
        // A new session's name, as Claude Code's `/clear [name]` and `/branch [name]` take one.
        assert_eq!(
            Builtin::parse("/clear  next "),
            Some(Builtin::Clear("next"))
        );
        assert_eq!(Builtin::parse("/branch try"), Some(Builtin::Branch("try")));
    }

    #[test]
    fn other_names_typed_run_the_listed_commands() {
        for (typed, listed) in [
            ("/new", Builtin::Clear("")),
            ("/reset", Builtin::Clear("")),
            ("/new one", Builtin::Clear("one")),
            ("/quit", Builtin::Exit),
            ("/name my work", Builtin::Rename("my work")),
            ("/session", Builtin::Status),
            ("/usage", Builtin::Status),
            ("/cost", Builtin::Status),
            ("/stats", Builtin::Status),
            ("/config", Builtin::Settings),
            ("/clone", Builtin::Branch("")),
            ("/continue 2", Builtin::Resume("2")),
            ("/help", Builtin::Hotkeys),
        ] {
            assert_eq!(Builtin::parse(typed), Some(listed), "{typed}");
        }
        assert_eq!(Builtin::parse("/session now"), None);
    }

    #[test]
    fn hotkeys_line_up_their_actions_within_80_columns() {
        let ui = Terminal::new(std::sync::Arc::default(), false);
        let text = hotkeys(&ui);
        let text = [style::CODE.get(), style::INFO_HEADING.get(), style::RESET]
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
    fn token_counts_are_grouped_by_thousands() {
        let counts = [0, 999, 1_000, 15_224, 1_234_567].map(grouped);
        assert_eq!(counts, ["0", "999", "1,000", "15,224", "1,234,567"]);
    }
}
