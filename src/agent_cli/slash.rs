//! The built-in commands: `/export`, `/import`, `/copy`, `/clear`, `/rename`, `/status`,
//! `/release-notes`, `/compact`, `/autopilot`, `/reload`, and `!` commands. `/settings` is in
//! `settings_menu.rs`; `/hotkeys` and `/keybindings` in `hotkeys.rs`; `/resume`, `/fork`,
//! `/branch`, `/clone` and `/tree` in `sessions.rs`; `/model` and `/scoped-models` in
//! `scope.rs`; `/login` and `/logout` in `login.rs`.
use super::drive::{self, SandboxChange};
use super::interactive::{Interactive, completions};
use super::report::{ReportView, Tab};
use super::settings::{card_level, scramjet_level};
use super::terminal;
use super::text::{clip, visible};
use super::{changelog, clipboard, completion, doctor, export, installed, keymap, style};
use scramjet::agent::{Agent, AgentError, AgentUi, McpState, Reach, SandboxMode, todo};
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// A built-in command as typed, with its argument.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Builtin<'a> {
    Settings,
    Export(&'a str),
    Import(&'a str),
    /// Claude Code's `/copy [N]`: the Nth-latest reply, or one of its code blocks.
    Copy(&'a str),
    /// With the new session's name, if given.
    Clear(&'a str),
    Rename(&'a str),
    /// Claude Code's `/color [NAME|default]`: the prompt's border in the session's own color.
    Color(&'a str),
    Status,
    Diff,
    /// Claude Code's `/tasks [stop N]`: the commands bash runs in the background.
    Tasks(&'a str),
    /// Claude Code's `/todos`: the agent's checklist.
    Todos,
    Agents,
    Skills,
    Memory,
    /// Claude Code's `/hooks`: the hooks the settings run.
    Hooks,
    Doctor,
    /// With whether the details come first.
    Usage(bool),
    Context,
    SystemPrompt,
    ReleaseNotes,
    Hotkeys,
    Keybindings,
    Resume(&'a str),
    Fork(&'a str),
    /// With the new session's name, if given.
    Branch(&'a str),
    Tree(&'a str),
    /// Claude Code's `/rewind [N [code|conversation]]`: back to before a prompt.
    Rewind(&'a str),
    Model(&'a str),
    /// Claude Code's `/effort [LEVEL]`: the thinking level.
    Effort(&'a str),
    /// Claude Code's `/theme [NAME]`: the colors.
    Theme(&'a str),
    ScopedModels,
    Login,
    Logout,
    Compact(&'a str),
    /// Claude Code's `/autocompact [on|off|auto|TOKENS]`: when compaction runs by itself.
    AutoCompact(&'a str),
    /// Claude Code's `/btw QUESTION`: a side question, answered apart from the conversation.
    Btw(&'a str),
    /// Claude Code's `/recap`: where the session stands, asked as a side question.
    Recap,
    /// Claude Code's `/goal [CONDITION|clear]`: a condition checked after each turn.
    Goal(&'a str),
    /// Claude Code's `/loop [INTERVAL] PROMPT|stop`: a prompt sent again on an interval.
    Loop(&'a str),
    /// Claude Code's and Codex's `/plan [PROMPT|off]`: plan mode.
    Plan(&'a str),
    Autopilot(&'a str),
    /// With its argument: `reconnect`, `enable` or `disable`, with a server's name or `all`.
    Mcp(&'a str),
    /// With its argument: a mode, `network on|off`, `add DIR` or `remove DIR`.
    Sandbox(&'a str),
    /// Claude Code's `/add-dir DIR`, as `/sandbox add DIR`.
    AddDir(&'a str),
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
            "copy" => Self::Copy(argument),
            "clear" => Self::Clear(argument),
            "rename" => Self::Rename(argument),
            "color" => Self::Color(argument),
            "status" if argument.is_empty() => Self::Status,
            "diff" if argument.is_empty() => Self::Diff,
            "tasks" => Self::Tasks(argument),
            "todos" if argument.is_empty() => Self::Todos,
            "agents" if argument.is_empty() => Self::Agents,
            "skills" if argument.is_empty() => Self::Skills,
            "memory" if argument.is_empty() => Self::Memory,
            "hooks" if argument.is_empty() => Self::Hooks,
            "doctor" if argument.is_empty() => Self::Doctor,
            "usage" if argument.is_empty() || argument == "details" => {
                Self::Usage(argument == "details")
            }
            "context" if argument.is_empty() => Self::Context,
            "system-prompt" if argument.is_empty() => Self::SystemPrompt,
            "release-notes" if argument.is_empty() => Self::ReleaseNotes,
            "hotkeys" if argument.is_empty() => Self::Hotkeys,
            "keybindings" if argument.is_empty() => Self::Keybindings,
            "resume" => Self::Resume(argument),
            "fork" => Self::Fork(argument),
            "branch" => Self::Branch(argument),
            "tree" => Self::Tree(argument),
            "rewind" => Self::Rewind(argument),
            "model" => Self::Model(argument),
            "effort" => Self::Effort(argument),
            "theme" => Self::Theme(argument),
            "scoped-models" if argument.is_empty() => Self::ScopedModels,
            "login" if argument.is_empty() => Self::Login,
            "logout" if argument.is_empty() => Self::Logout,
            "compact" => Self::Compact(argument),
            "autocompact" => Self::AutoCompact(argument),
            "btw" => Self::Btw(argument),
            "recap" if argument.is_empty() => Self::Recap,
            "goal" => Self::Goal(argument),
            "loop" => Self::Loop(argument),
            "plan" => Self::Plan(argument),
            "autopilot" => Self::Autopilot(argument),
            "mcp" => Self::Mcp(argument),
            "sandbox" => Self::Sandbox(argument),
            "add-dir" => Self::AddDir(argument),
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
                | Self::Copy(_)
                | Self::Rename(_)
                | Self::Color(_)
                | Self::Status
                | Self::Diff
                | Self::Tasks(_)
                | Self::Todos
                | Self::Agents
                | Self::Skills
                | Self::Memory
                | Self::Hooks
                | Self::Doctor
                | Self::Btw(_)
                | Self::Recap
                | Self::Goal(_)
                | Self::Loop(_)
                | Self::Plan(_)
                | Self::Usage(_)
                | Self::Context
                | Self::SystemPrompt
                | Self::ReleaseNotes
                | Self::Hotkeys
                | Self::Keybindings
                | Self::Model(_)
                | Self::Effort(_)
                | Self::Theme(_)
                | Self::AutoCompact(_)
                | Self::ScopedModels
                | Self::Login
                | Self::Logout
                | Self::Autopilot(_)
                | Self::Mcp(_)
                | Self::Sandbox(_)
                | Self::AddDir(_)
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
            Some(Builtin::Copy(age)) => self.copy_reply(age),
            Some(Builtin::Clear(name)) => {
                if drive::new_session(self) && !name.is_empty() {
                    self.name(name);
                }
            }
            Some(Builtin::Rename(name)) => self.name(name),
            Some(Builtin::Color(wanted)) => self.color(wanted),
            Some(Builtin::Status) => self.say(session_info(&self.session.agent).trim_end()),
            Some(Builtin::Diff) => self.diff(),
            Some(Builtin::Tasks(argument)) => self.tasks(argument),
            Some(Builtin::Todos) => self.todos(),
            Some(Builtin::Agents) => self.agents(),
            Some(Builtin::Skills) => self.skills(),
            Some(Builtin::Memory) => self.memory(),
            Some(Builtin::Hooks) => self.hooks(),
            Some(Builtin::Doctor) => self.doctor(),
            Some(Builtin::Usage(details)) => self.usage(details),
            Some(Builtin::Context) => self.context(),
            Some(Builtin::SystemPrompt) => self.system_prompt(),
            Some(Builtin::ReleaseNotes) => self.release_notes(),
            Some(Builtin::Hotkeys) => self.hotkeys(),
            Some(Builtin::Keybindings) => self.keybindings(),
            Some(Builtin::Resume(wanted)) => {
                self.resume(wanted);
            }
            Some(Builtin::Fork(wanted)) => self.fork(wanted),
            Some(Builtin::Branch(name)) => self.clone_session(name),
            Some(Builtin::Tree(wanted)) => self.tree(wanted),
            Some(Builtin::Rewind(wanted)) => self.rewind(wanted),
            Some(Builtin::Model(wanted)) => self.model(wanted),
            Some(Builtin::Effort(wanted)) => self.effort(wanted),
            Some(Builtin::Theme(wanted)) => self.theme(wanted),
            Some(Builtin::ScopedModels) => self.scoped_models(),
            Some(Builtin::Login) => self.login(),
            Some(Builtin::Logout) => self.logout(),
            Some(Builtin::Compact(focus)) => self.compact(focus),
            Some(Builtin::AutoCompact(wanted)) => self.auto_compact(wanted),
            Some(Builtin::Btw(question)) => self.btw(question),
            Some(Builtin::Recap) => self.recap(),
            Some(Builtin::Goal(condition)) => self.goal(condition),
            Some(Builtin::Loop(argument)) => self.looping(argument),
            Some(Builtin::Plan(argument)) => self.plan(argument),
            Some(Builtin::Autopilot(wanted)) => self.autopilot(wanted),
            Some(Builtin::Mcp(argument)) => self.mcp(argument),
            Some(Builtin::Sandbox("")) => self.sandbox_menu(),
            Some(Builtin::Sandbox(argument)) => self.sandbox(argument),
            Some(Builtin::AddDir(directory)) => self.add_dir(directory),
            Some(Builtin::Reload) => self.reload(),
            Some(Builtin::Exit) | None => return false,
        }
        true
    }

    /// `/todos`, as Claude Code's: the checklist the agent's latest `todo_write` left.
    fn todos(&mut self) {
        let Some(todos) = self.session.agent.todos() else {
            return self.say(
                "No todo list. The agent keeps one (todo_write) while it works on a task of \
                 several steps.",
            );
        };
        let done = (todos.iter())
            .filter(|todo| todo.status == todo::Status::Completed)
            .count();
        let mut text = format!("Todo list, {done} of {} completed:", todos.len());
        for todo in &todos {
            let line = match todo.status {
                todo::Status::Completed => format!("\n☒ {}", todo.content),
                todo::Status::InProgress => format!("\n☐ {} (in progress)", todo.content),
                todo::Status::Pending => format!("\n☐ {}", todo.content),
            };
            text.push_str(&line);
        }
        self.say(&text);
    }

    pub(super) fn name(&mut self, name: &str) {
        if name.is_empty() {
            match self.session.agent.session_name() {
                Some(current) => self.say_styled(style::MUTED, &format!("Session name: {current}")),
                None => self.say("Usage: /rename <name>"),
            }
        } else {
            match self.session.agent.set_session_name(name) {
                Ok(()) => self.say_styled(style::MUTED, &format!("Session name set: {name}")),
                Err(error) => self.say(&format!("Error: {error}")),
            }
        }
    }

    /// `/color [NAME|default]`, as Claude Code's: the prompt's border in a color of the session's
    /// own, saved with it, to tell sessions apart; a color picked at random when none is named.
    fn color(&mut self, wanted: &str) {
        let wanted = wanted.to_lowercase();
        let colors: Vec<&str> = style::session_colors().collect();
        let color = match wanted.as_str() {
            "" => {
                let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
                let at = since.unwrap_or_default().subsec_nanos() as usize % colors.len();
                Some(colors[at])
            }
            "default" | "reset" | "none" | "gray" | "grey" => None,
            name if colors.contains(&name) => Some(name),
            name => {
                let available = colors.join(", ");
                let said =
                    format!("Invalid color \"{name}\". Available colors: {available}, default");
                return self.say(&said);
            }
        };
        if let Err(error) = self.session.agent.set_session_color(color) {
            return self.say(&format!("Error: {error}"));
        }
        if let Some(screen) = &self.screen {
            screen.footer(super::footer::Shown::of(&self.session.agent));
        }
        match color {
            Some(color) => self.say_styled(style::MUTED, &format!("Session color set to: {color}")),
            None => self.say_styled(style::MUTED, "Session color reset to default"),
        }
    }

    /// `/release-notes`: what changed in each released version, the newest first; on the full
    /// screen in a view of its own, else cut to its first lines until tool results are shown
    /// whole.
    fn release_notes(&mut self) {
        match &self.screen {
            Some(screen) => {
                screen.show_report(changelog::whole(self.session.settings.collapse_changelog))
            }
            None => self.ui.release_notes(changelog::released()),
        }
    }

    /// `/export [path]`, as Scramjet's: the current branch as a web page, or, to a path ending in
    /// `.jsonl`, as a session file of its own, in `.txt` as text. Without a path the full screen
    /// asks, as Claude Code's does, whether to save the page, copy the text or save it.
    fn export(&mut self, argument: &str) {
        let mut path = path_argument(argument).map(str::to_owned);
        if path.is_none()
            && let Some(screen) = &self.screen
            && !self.session.agent.conversation().is_empty()
        {
            let id = self.session.agent.session_id();
            let choices = [
                choice(
                    "Save as a web page",
                    Some(format!("scramjet-session-{id}.html")),
                ),
                choice(
                    "Copy to clipboard",
                    Some("The conversation as text".to_owned()),
                ),
                choice("Save as text", Some(format!("scramjet-session-{id}.txt"))),
            ];
            let question = super::select::Picker::question("Export conversation", &choices, None);
            match screen.pick(question) {
                None => return self.say_styled(style::MUTED, "Export cancelled"),
                Some(0) => {}
                Some(1) => return self.copy_conversation(),
                Some(_) => path = Some(format!("scramjet-session-{id}.txt")),
            }
        }
        match export_branch(&self.session.agent, path.as_deref(), Path::new("")) {
            Ok(Some(path)) => {
                let said = format!("Session exported to: {}", path.display());
                self.say_styled(style::MUTED, &said)
            }
            Ok(None) => self.say("Nothing to export yet"),
            Err(error) => self.say(&format!("Error: failed to export the session: {error}")),
        }
    }

    /// The current branch as `/export` writes it in text, copied to the clipboard.
    fn copy_conversation(&mut self) {
        let text = match self.session.agent.branch() {
            Ok(branch) => export::text(&branch, super::editor::terminal_size().0),
            Err(error) => {
                return self.say(&format!("Error: failed to export the session: {error}"));
            }
        };
        self.copy_text(&text, "Conversation copied to clipboard");
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
        drive::import(self, path);
    }

    /// `/copy [N]`, as Claude Code's: the Nth-latest of the last 20 replies, as the model wrote
    /// it; on the full screen a reply with code blocks offers each of them, unless
    /// `copyFullResponse` is on.
    fn copy_reply(&mut self, age: &str) {
        let replies: Vec<String> = self.session.agent.replies().take(20).collect();
        if replies.is_empty() {
            return self.say("No agent messages to copy yet.");
        }
        let index = match age {
            "" => 0,
            typed => match typed.parse::<usize>() {
                Ok(age) if age >= 1 => age - 1,
                _ => {
                    return self.say(&format!(
                        "Usage: /copy [N] where N is 1 (latest), 2, 3, \u{2026} Got: {typed}"
                    ));
                }
            },
        };
        let Some(reply) = replies.get(index) else {
            let count = replies.len();
            let noun = if count == 1 { "message" } else { "messages" };
            return self.say(&format!("Only {count} agent {noun} available to copy"));
        };
        let blocks = code_blocks(reply);
        let chosen = match &self.screen {
            Some(screen) if !blocks.is_empty() && !self.session.settings.copy_full_response => {
                let mut choices = vec![choice("Full response", Some(sized(reply, "char")))];
                choices.extend(blocks.iter().map(|(code, language)| {
                    let lines = code.lines().count().max(1);
                    let lines = (lines > 1).then(|| format!("{lines} lines"));
                    let about = [language.clone(), lines].into_iter().flatten();
                    let about = about.collect::<Vec<_>>().join(", ");
                    let label = code.split_whitespace().collect::<Vec<_>>().join(" ");
                    choice(
                        &clip(&visible(&label), 60),
                        Some(about).filter(|a| !a.is_empty()),
                    )
                }));
                choices.push(choice(
                    "Always copy full response",
                    Some("Skip this list from now on (Copy full response in /settings)".to_owned()),
                ));
                let question =
                    super::select::Picker::question("Select content to copy:", &choices, None);
                match screen.pick(question) {
                    None => return self.say_styled(style::MUTED, "Copy cancelled"),
                    Some(0) => Copied::Full,
                    Some(at) if at == choices.len() - 1 => Copied::Always,
                    Some(at) => Copied::Block(at - 1),
                }
            }
            _ => Copied::Full,
        };
        let text = match chosen {
            Copied::Block(at) => blocks[at].0.as_str(),
            Copied::Full | Copied::Always => reply.as_str(),
        };
        self.copy_text(text, "Copied to clipboard");
        if let Copied::Always = chosen {
            match self.session.settings.copy_full_responses() {
                Ok(()) => self.say_styled(
                    style::MUTED,
                    "Preference saved: /copy copies whole replies (Copy full response in /settings)",
                ),
                Err(error) => self.say(&format!("Error: the preference was not saved: {error}")),
            }
        }
    }

    /// Puts `text` on the clipboard, saying `copied` and how much it was.
    fn copy_text(&mut self, text: &str, copied: &str) {
        let terminal = self.screen.is_some() || std::io::stderr().is_terminal();
        match clipboard::copy(text, terminal) {
            Ok(sequence) => {
                if let Some(sequence) = sequence {
                    match &self.screen {
                        Some(screen) => screen.to_terminal(sequence),
                        None => eprint!("{sequence}"),
                    }
                }
                let copied = format!("{copied} ({})", sized(text, "character"));
                self.say_styled(style::MUTED, &copied);
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
            && let Err(error) = self.session.agent.set_autopilot(on)
        {
            return self.say(&format!("Error: {error}"));
        }
        let state = if self.session.agent.autopilot() {
            "on"
        } else {
            "off"
        };
        self.say_styled(style::MUTED, &format!("Autopilot is {state}"));
    }

    /// `/sandbox`: how the agent's tools are confined, or with an argument a change of it for
    /// the rest of the session: a mode (`off` for none), `network on|off`, `add DIR`, `remove
    /// DIR`.
    pub(super) fn sandbox(&mut self, argument: &str) {
        let (word, rest) = argument.split_once(' ').unwrap_or((argument, ""));
        let rest = rest.trim();
        let change = match (word, rest) {
            ("", "") => {
                let allow_write = self.session.settings.allow_write();
                return self.say(&sandbox_info(&self.session.agent, &allow_write));
            }
            ("add", directory) => return self.add_dir(directory),
            ("network", "on" | "off") => SandboxChange::Network(rest == "on"),
            ("remove", directory) if !directory.is_empty() => {
                SandboxChange::Remove(directory.to_owned())
            }
            (mode, "") if let Some(mode) = SandboxMode::parse(mode) => SandboxChange::Mode(mode),
            _ => return self.say(SANDBOX_USAGE),
        };
        self.change_sandbox(change);
    }

    /// `/add-dir DIR`: the sandbox lets the agent change `DIR` too, for the rest of the session.
    pub(super) fn add_dir(&mut self, directory: &str) {
        if directory.is_empty() {
            return self.say("Usage: /add-dir DIR");
        }
        self.change_sandbox(SandboxChange::Add(directory.to_owned()));
    }

    /// `/sandbox remove DIR`: takes back the grant of `DIR`, and forgets it if it was granted
    /// always in this project.
    pub(super) fn remove_dir(&mut self, directory: &str) {
        self.change_sandbox(SandboxChange::Remove(directory.to_owned()));
    }

    pub(super) fn change_sandbox(&mut self, change: SandboxChange) {
        match self.session.change_sandbox(change) {
            Ok((said, warnings)) => {
                for warning in warnings {
                    self.say(&format!("Warning: {warning}"));
                }
                self.say_styled(style::MUTED, &said);
            }
            Err(error) => self.say(&error),
        }
    }

    /// `/usage`: what is left of the quotas and what the saved sessions spent, read from their
    /// files, or with `details` the details behind it; on the full screen, a view of both,
    /// provider by provider.
    fn usage(&mut self, details: bool) {
        let sessions = match scramjet::agent::state_directory() {
            Ok(state) => state.join("sessions"),
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        let current = self.session.agent.session_path();
        let history = scramjet::agent::usage::History::read(&sessions, current);
        let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
            .map_or(0, |since| since.as_millis() as u64);
        let model = self.session.agent.model();
        if let Some(screen) = &self.screen {
            let view = super::usage::view(&history, &sessions, current, model, now, details);
            return screen.show_report(view);
        }
        let report = match details {
            true => super::usage::details(&history, &sessions, current, model, now, None),
            false => super::usage::summary(&history, current, model, now, None),
        };
        self.say(report.trim_end());
    }

    /// `/diff`, as Codex's and Claude Code's: the uncommitted changes, untracked files included;
    /// on the full screen in a view of its own, the files changed a key away.
    fn diff(&mut self) {
        let root = self.session.agent.root().to_owned();
        match super::changes::uncommitted(&root) {
            Ok(None) => self.say("Not in a Git repository"),
            Ok(Some(changes)) if changes.diff.is_empty() => {
                self.say_styled(style::MUTED, "No uncommitted changes")
            }
            Ok(Some(changes)) => match &self.screen {
                Some(screen) => screen.show_report(super::changes::view(&changes)),
                None => self.say(super::changes::text(&changes).trim_end()),
            },
            Err(error) => self.say(&format!("Error: {error}")),
        }
    }

    /// `/tasks`, as Claude Code's: the commands bash runs in the background, each with how it
    /// ended or how long it has run, and its output's file; `/tasks stop N` stops one.
    fn tasks(&mut self, argument: &str) {
        let tasks = self.session.agent.tasks();
        if let Some(id) = argument.strip_prefix("stop") {
            let stopped = match id.trim().parse() {
                Ok(id) => tasks.stop(id, false),
                Err(_) => Err("Usage: /tasks [stop N]".to_owned()),
            };
            return match stopped {
                Ok(text) => self.say_styled(style::MUTED, &text),
                Err(error) => self.say(&format!("Error: {error}")),
            };
        }
        if !argument.is_empty() {
            return self.say("Usage: /tasks [stop N]");
        }
        let listed = tasks.list();
        if listed.is_empty() {
            let note = "No background tasks: the agent starts one with bash's run_in_background";
            return self.say_styled(style::MUTED, note);
        }
        if self.screen.is_some() {
            return self.pick_task();
        }
        let mut text = String::from("Background tasks:");
        for task in &listed {
            text += &format!(
                "\n{}  {}  {}\n   output: {}",
                task.id,
                task_state(task),
                clip(task.command.lines().next().unwrap_or_default(), 120),
                scramjet::os::tilde(&task.output)
            );
        }
        if listed.iter().any(|task| task.ended.is_none()) {
            text += "\n/tasks stop N stops one.";
        }
        self.say(&text);
    }

    /// `/tasks`' list on the full screen, each task's output file its note: Enter on a running
    /// task asks whether to show its output or stop it, on an ended one shows its output; the
    /// list then shows again, until Escape.
    fn pick_task(&mut self) {
        let mut at = 0;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let listed = self.session.agent.tasks().list();
            if listed.is_empty() {
                return;
            }
            at = at.min(listed.len() - 1);
            let rows: Vec<String> = (listed.iter())
                .map(|task| {
                    let command = task.command.lines().next().unwrap_or_default();
                    format!("{}  {}  {}", task.id, task_state(task), visible(command))
                })
                .collect();
            let notes = (listed.iter())
                .map(|task| format!("Output: {}", scramjet::os::tilde(&task.output)))
                .collect();
            let visible_rows = (super::editor::terminal_size().1 / 2).max(super::select::VISIBLE);
            let picker = super::select::Picker::select(
                "Background tasks",
                visible_rows,
                rows.len(),
                at,
                None,
                |index, width| clip(&rows[index], width),
            );
            let hint = super::select::hint("open", &[], "close");
            let Some(index) = screen.pick(picker.noting(hint, notes).held()) else {
                return;
            };
            at = index;
            let task = &listed[index];
            if task.ended.is_none() {
                let choices = [
                    scramjet::agent::Choice {
                        label: "Show its output".to_owned(),
                        description: None,
                        value: String::new(),
                    },
                    scramjet::agent::Choice {
                        label: "Stop it".to_owned(),
                        description: Some("With all it started.".to_owned()),
                        value: String::new(),
                    },
                ];
                let title = format!("Task {}", task.id);
                let question = super::select::Picker::question(&title, &choices, None);
                match screen.pick(question.held()) {
                    Some(0) => {}
                    Some(_) => {
                        let tasks = self.session.agent.tasks();
                        match tasks.stop(task.id, false) {
                            Ok(text) => {
                                tasks.wait_ended(task.id, std::time::Duration::from_secs(2));
                                self.say_styled(style::MUTED, &text)
                            }
                            Err(error) => self.say(&format!("Error: {error}")),
                        }
                        continue;
                    }
                    None => continue,
                }
            }
            screen.show_report(task_output(task));
        }
    }

    /// `/agents`, as Claude Code's: the agents a subagent call can name, each with where it
    /// comes from and what its file asks for; on the full screen in a list whose Enter opens the
    /// file, as `/settings`' Installed does.
    fn agents(&mut self) {
        let found = self.session.agent.installed();
        let problems = self.problems(&found);
        let listing = installed::agents(&self.loaded(&found, &problems));
        if listing.rows.is_empty() {
            let user = scramjet::agent::config_directory()
                .map(|config| scramjet::os::tilde(&config.join("agents")))
                .unwrap_or_else(|| "your agent directory".to_owned());
            let note = format!(
                "No agents: a markdown file in {user} or in this project's .scramjet/agents \
                 adds one"
            );
            return self.say_styled(style::MUTED, &note);
        }
        if self.screen.is_some() {
            return self.show_listing("Agents", installed::agents);
        }
        let text = installed::agents_text(&self.loaded(&found, &problems));
        self.say(text.trim_end());
    }

    /// `/skills`, as Claude Code's: the skills the session found, each with where it comes from
    /// and whether the model is offered it; on the full screen in a list whose Enter opens its
    /// file.
    fn skills(&mut self) {
        let agent = &self.session.agent;
        let user = agent.agent_directory().map(Path::to_path_buf);
        let root = agent.root().to_path_buf();
        let places = installed::Places {
            user: user.as_deref(),
            root: &root,
        };
        let found = self.session.agent.installed();
        let problems = self.problems(&found);
        let loaded = self.loaded(&found, &problems);
        if installed::skills(&loaded, &places).rows.is_empty() {
            let user = (user.as_deref())
                .map(|config| scramjet::os::tilde(&config.join("skills")))
                .unwrap_or_else(|| "your skills directory".to_owned());
            let note = format!(
                "No skills: a directory with a SKILL.md in {user} or in this project's \
                 .scramjet/skills adds one"
            );
            return self.say_styled(style::MUTED, &note);
        }
        if self.screen.is_some() {
            return self.show_listing("Skills", |loaded| installed::skills(loaded, &places));
        }
        let text = installed::skills_text(&loaded, &places);
        self.say(text.trim_end());
    }

    /// `/memory`, as Claude Code's: the context files read, and where the user's and the
    /// project's would go; on the full screen in a list whose Enter opens one in the editor,
    /// creating it when it is not there, and reads them again once it changed.
    fn memory(&mut self) {
        let agent = &self.session.agent;
        let user = agent.agent_directory().map(Path::to_path_buf);
        let root = agent.root().to_path_buf();
        let places = installed::Places {
            user: user.as_deref(),
            root: &root,
        };
        if self.screen.is_some() {
            return self.show_listing("Memory", |loaded| installed::memory(loaded, &places));
        }
        let found = self.session.agent.installed();
        let problems = self.problems(&found);
        let text = installed::memory_text(&self.loaded(&found, &problems), &places);
        self.say(text.trim_end());
    }

    /// `/hooks`, as Claude Code's: the hooks the settings run, by event, after what is wrong in
    /// them; on the full screen in a list whose Enter opens the settings file.
    fn hooks(&mut self) {
        let Some(file) = self.session.settings.path().map(Path::to_path_buf) else {
            return self.say_styled(style::MUTED, "No hooks: there is no settings file");
        };
        let listing = |settings: &super::settings::Settings| {
            let warnings: Vec<String> = (settings.warnings.iter())
                .filter(|warning| {
                    (warning.split_once(": ")).is_some_and(|(_, rest)| rest.starts_with("hooks"))
                })
                .cloned()
                .collect();
            installed::hooks(&settings.hooks.listed(), &warnings, &file)
        };
        if listing(&self.session.settings).rows.is_empty() {
            let note = format!(
                "No hooks: Claude Code's hooks setting in {} adds them",
                scramjet::os::tilde(&file)
            );
            return self.say_styled(style::MUTED, &note);
        }
        if self.screen.is_none() {
            let text = installed::hooks_text(&listing(&self.session.settings), &file);
            return self.say(text.trim_end());
        }
        let mut at = None;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let listing = listing(&self.session.settings);
            if listing.rows.is_empty() {
                return;
            }
            let first = listing.headings.iter().position(|heading| !heading);
            let at_row = at.unwrap_or(first.unwrap_or(0)).min(listing.rows.len() - 1);
            let visible = (super::editor::terminal_size().1 / 2).max(super::select::VISIBLE);
            let rows = &listing.rows;
            let picker = super::select::Picker::select(
                "Hooks",
                visible,
                rows.len(),
                at_row,
                None,
                |index, width| clip(&rows[index], width),
            );
            let keys = super::select::hint("edit", &[], "back");
            let picker = (picker.headed(listing.headings)).noting(keys, listing.notes);
            let Some(index) = screen.pick(picker.held()) else {
                return;
            };
            at = Some(index);
            self.edit_installed(&file);
        }
    }

    /// `/doctor`, as Claude Code's: `scramjet doctor`'s checks, with what this session loaded,
    /// its model and how its MCP servers are.
    fn doctor(&mut self) {
        let found = self.session.agent.installed();
        let problems = self
            .problems(&found)
            .into_iter()
            .map(|problem| problem.text);
        let agent = &self.session.agent;
        let mcp = (agent.mcp_servers().into_iter())
            .map(|(server, state)| (server.name, state))
            .collect();
        let setup = doctor::Setup {
            settings: &self.session.settings,
            providers: agent.providers(),
            root: agent.root(),
            problems: problems.collect(),
            model: Some(agent.model().to_owned()),
            mcp: Some(mcp),
        };
        let text = doctor::text(&doctor::checks(&setup));
        self.say(text.trim_end());
    }

    /// `/context`, as Claude Code's: how much of the window each part of the context takes; on
    /// the full screen in a view of its own, the items under each a key away.
    fn context(&mut self) {
        let breakdown = self.session.agent.context_breakdown();
        match &self.screen {
            Some(screen) => {
                let prompt = super::system_prompt::view(&self.session.agent.system_prompt());
                screen.show_report(super::context::view(&breakdown, prompt));
            }
            None => self.say(super::context::report(&breakdown, true).trim_end()),
        }
    }

    /// `/system-prompt`: on the full screen, the instructions and tools the last request sent.
    fn system_prompt(&mut self) {
        match &self.screen {
            Some(screen) => {
                let prompt = self.session.agent.system_prompt();
                screen.show_report(super::system_prompt::view(&prompt));
            }
            None => self.say(
                "/system-prompt shows on the full screen; /export writes the instructions and \
tools into its page",
            ),
        }
    }

    /// `/mcp`, as Codex's: the MCP servers, how each is, and its tools; `/mcp reconnect`, as
    /// Claude Code's, starts one again, or all, and `/mcp enable` and `disable` too.
    fn mcp(&mut self, argument: &str) {
        match mcp_command(argument) {
            None => return self.say("Usage: /mcp [reconnect|enable|disable [SERVER|all]]"),
            Some(McpCommand::Reconnect(name)) => return self.reconnect(name),
            Some(McpCommand::Enable(name)) => return self.enable_mcp(name, true),
            Some(McpCommand::Disable(name)) => return self.enable_mcp(name, false),
            Some(McpCommand::List) => {}
        }
        let servers = self.session.agent.mcp_servers();
        let disabled = self.session.agent.disabled_mcp();
        let enabled = self.session.agent.mcp_names().len() - disabled.len();
        if servers.is_empty() && (disabled.is_empty() || enabled > 0) {
            let settings = (self.session.settings.path()).map_or("the settings".into(), |path| {
                scramjet::agent::shortened(&path.to_string_lossy())
            });
            return match self.session.settings.mcp_servers.is_empty() {
                true => self.say_styled(
                    style::MUTED,
                    &format!(
                        "No MCP servers. Add them to mcpServers in {settings}, as Claude Code's \
.mcp.json lists them: {{\"name\": {{\"command\": \"program\", \"args\": [\"…\"]}}}}, or \
{{\"name\": {{\"type\": \"http\", \"url\": \"https://…\"}}}} for a server over HTTP"
                    ),
                ),
                false => self.say_styled(
                    style::MUTED,
                    "The MCP servers are not started: the model has no MCP tools in this run",
                ),
            };
        }
        // What a server wrote or says of itself is shown as text, its control characters escaped.
        let row = |label: &str, value: &str| {
            format!(
                "  {} {}\n",
                style::mark(style::MUTED, label),
                visible(value)
            )
        };
        let mut info = format!("{}\n", style::mark(style::INFO_HEADING, "MCP Servers"));
        for (server, state) in servers {
            info += "\n";
            let said = match &state {
                McpState::Starting => "starting".to_owned(),
                McpState::Ready { tools, .. } if tools.len() == 1 => "1 tool".to_owned(),
                McpState::Ready { tools, .. } => format!("{} tools", tools.len()),
                McpState::Failed(_) => "not available".to_owned(),
            };
            let name = visible(&server.name);
            info += &format!(
                "{name} {}\n",
                style::mark(style::MUTED, &format!("· {said}"))
            );
            let (label, address) = server.address();
            info += &row(label, &address);
            match &state {
                McpState::Ready {
                    tools,
                    about,
                    direct,
                } => {
                    if let Some(about) = about {
                        info += &row("About:", about);
                    }
                    if !tools.is_empty() {
                        info += &row("Tools:", &tools.join(", "));
                        let offered = match direct {
                            Some(true) => {
                                format!("in the tool list, as mcp__{}__TOOL", server.name)
                            }
                            Some(false) => "through mcp_search and mcp_call".to_owned(),
                            None => "decided once every server has started".to_owned(),
                        };
                        info += &row("Offered:", &offered);
                    }
                }
                McpState::Failed(why) => info += &row("Error:", &why.replace('\n', "\n    ")),
                McpState::Starting => {}
            }
        }
        for server in disabled {
            info += &format!(
                "\n{} {}\n",
                visible(&server.name),
                style::mark(style::MUTED, "· disabled")
            );
            let (label, address) = server.address();
            info += &row(label, &address);
        }
        self.say(info.trim_end());
    }

    /// `/mcp enable` and `disable`: starts or stops the server named `name`, or all with `None`,
    /// and saves which are disabled in the settings, as Claude Code keeps them.
    fn enable_mcp(&mut self, name: Option<&str>, enable: bool) {
        let names = self.session.agent.mcp_names();
        if names.is_empty() {
            return self.say_styled(style::MUTED, "No MCP servers: see /mcp");
        }
        if let Some(name) = name
            && !names.iter().any(|listed| listed == name)
        {
            return self.say(&format!("Error: no MCP server named {}", visible(name)));
        }
        let mut disabled = self.session.agent.mcp_disabled().to_vec();
        let changed: Vec<String> = (names.into_iter())
            .filter(|listed| name.is_none_or(|name| listed == name))
            .filter(|listed| disabled.contains(listed) == enable)
            .collect();
        let (done, state) = match enable {
            true => ("Enabled", "enabled"),
            false => ("Disabled", "disabled"),
        };
        if changed.is_empty() {
            let which = match name {
                Some(name) => format!("MCP server {} is", visible(name)),
                None => "All MCP servers are".to_owned(),
            };
            return self.say_styled(style::MUTED, &format!("{which} already {state}"));
        }
        match enable {
            true => disabled.retain(|listed| !changed.contains(listed)),
            false => disabled.extend(changed.iter().cloned()),
        }
        if let Err(error) = self
            .session
            .settings
            .set_disabled_mcp_servers(disabled.clone())
        {
            self.say(&format!("Error: settings not saved: {error}"));
        }
        self.session.agent.set_mcp_disabled(disabled);
        let servers = match changed.len() {
            1 => "MCP server",
            _ => "MCP servers",
        };
        let changed: Vec<String> = changed.iter().map(|listed| visible(listed)).collect();
        let said = format!("{done} {servers} {}", changed.join(", "));
        self.say_styled(style::MUTED, &said);
    }

    fn reconnect(&mut self, name: Option<&str>) {
        if let Some(name) = name
            && self
                .session
                .agent
                .mcp_disabled()
                .iter()
                .any(|listed| listed == name)
        {
            let name = visible(name);
            return self.say(&format!(
                "Error: MCP server {name} is disabled; /mcp enable {name} starts it"
            ));
        }
        let result = self.during_turn(|agent, ui, cancel| agent.reconnect_mcp(name, ui, cancel));
        let states = match result {
            Ok(states) => states,
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        for (server, state) in states {
            let name = visible(&server.name);
            match state {
                McpState::Ready { tools, .. } => {
                    let tools = match tools.len() {
                        1 => "1 tool".to_owned(),
                        count => format!("{count} tools"),
                    };
                    self.say_styled(style::MUTED, &format!("Reconnected to {name} ({tools})"));
                }
                McpState::Starting => {
                    self.say_styled(style::MUTED, &format!("{name} is still starting"));
                }
                McpState::Failed(why) => {
                    let why = visible(&why).replace('\n', "\n  ");
                    self.say(&format!("Error: could not reconnect to {name}: {why}"));
                }
            }
        }
    }

    /// `/reload`, as Scramjet's: the settings, the themes, the command sets with their commands and
    /// agents, the skills and the context files read again, and what is wrong in them said as at
    /// start.
    pub(super) fn reload(&mut self) {
        self.save_hide_thinking();
        let unwritable = self.session.reload(&[]);
        self.discover_themes();
        self.use_theme();
        self.keybindings = keymap::load(self.session.settings.path());
        let settings = &self.session.settings;
        (self.hide_thinking).store(settings.hide_thinking_block, Ordering::Relaxed);
        self.ui.markdown_reads = settings.markdown_reads;
        if let Some(screen) = &mut self.screen {
            (screen.link).reloaded(completions(&self.session.agent), &self.session.settings);
            (screen.link).models(self.session.agent.models().unwrap_or_default());
        }
        for notice in self.problem_notices() {
            self.ui().notice(&notice);
        }
        for problem in self.session.agent.providers().problems().to_vec() {
            self.ui().notice(&problem);
        }
        for warning in unwritable {
            self.ui().notice(&warning);
        }
        let done = "Reloaded settings, keybindings, themes, providers, command sets, agents, skills and context files";
        self.say_styled(style::MUTED, done);
    }

    fn compact(&mut self, focus: &str) {
        let result = self.during_turn(|agent, ui, cancel| agent.compact(focus, None, ui, cancel));
        match result {
            Ok(Some(compaction)) => {
                self.ui().compacted(&compaction, None);
                self.spaced = true;
            }
            Ok(None) => self.say("Nothing to compact: the conversation is all recent."),
            Err(AgentError::Cancelled) => self.say_styled(style::MUTED, "(cancelled)"),
            Err(error) => self.say(&format!("Error: {error}")),
        }
    }

    /// `/autocompact [on|off|auto|TOKENS]`: turns compaction by itself on or off, or sets the
    /// tokens kept free of the window (`auto` its default), saved in the settings; without an
    /// argument, where it runs.
    fn auto_compact(&mut self, wanted: &str) {
        let settings = &mut self.session.settings;
        let saved = match wanted.to_ascii_lowercase().as_str() {
            "" => Ok(()),
            "on" => settings.set_auto_compact(true),
            "off" => settings.set_auto_compact(false),
            "auto" => (settings.set_auto_compact(true))
                .and_then(|()| settings.set_compaction_reserve(None)),
            tokens => match token_count(tokens) {
                Some(reserve) => settings.set_compaction_reserve(Some(reserve)),
                None => {
                    return self.say(&format!(
                        "Error: /autocompact takes on, off, auto or a number of tokens to keep \
                         free of the window (such as 30000 or 30k), not {wanted}"
                    ));
                }
            },
        };
        self.session.agent.set_compacting(settings.compacting);
        if let Err(error) = saved {
            self.say(&format!("Error: settings not saved: {error}"));
        }
        let agent = &self.session.agent;
        if !agent.compacting().automatic {
            return self
                .say("Auto-compact: off; /compact still compacts; /autocompact on turns it on");
        }
        let when = match (agent.compaction_threshold(), agent.context_window()) {
            (Some(at), Some(window)) => format!(
                "past {} of {}'s {} window ({} kept free)",
                terminal::tokens(at),
                agent.model(),
                terminal::tokens(window),
                terminal::tokens(window - at)
            ),
            _ => format!(
                "keeping {} of the window free (its size is not known)",
                terminal::tokens(agent.compacting().reserve)
            ),
        };
        self.say(&format!(
            "Auto-compact: on, {when}; /autocompact on|off|auto|TOKENS"
        ));
    }

    /// `/effort [LEVEL]`: sets the thinking level, saved as the settings' default, as Shift+Tab
    /// does; without one, the levels to pick from (on the full screen `/settings`' list).
    fn effort(&mut self, wanted: &str) {
        let agent = &self.session.agent;
        let levels = agent.effort_levels().to_vec();
        if !agent.reasons() || levels.is_empty() {
            return self.say("Current model does not support thinking");
        }
        let named = levels.iter().map(|level| scramjet_level(level));
        let named = named.collect::<Vec<_>>().join(", ");
        if wanted.is_empty() {
            if self.screen.is_some() {
                return self.choose_thinking_level();
            }
            let current = agent.effort().map_or("the model's default", scramjet_level);
            return self.say(&format!(
                "Thinking level: {current}; {} takes {named}: /effort LEVEL",
                agent.model()
            ));
        }
        let Some(level) = card_level(wanted).filter(|&level| levels.iter().any(|l| l == level))
        else {
            return self.say(&format!(
                "Error: {} takes no thinking level {wanted}; it takes {named}",
                agent.model()
            ));
        };
        let level = level.to_owned();
        if let Err(error) = self.session.agent.set_effort(level.clone()) {
            return self.say(&format!("Error: {error}"));
        }
        if let Err(error) = self.session.settings.set_default_thinking_level(&level) {
            return self.say(&format!("Error: settings not saved: {error}"));
        }
        self.say(&format!("Thinking level: {}", scramjet_level(&level)));
    }

    /// `/theme [NAME]`: uses the theme named and saves it as the settings' `theme`; without
    /// one, the themes to pick from (on the full screen `/settings`' list).
    fn theme(&mut self, wanted: &str) {
        self.discover_themes();
        if wanted.is_empty() && self.screen.is_some() {
            return self.choose_theme();
        }
        let names = (self.themes.listed.iter()).map(|listed| listed.name.as_str());
        let names = names.collect::<Vec<_>>().join(", ");
        if wanted.is_empty() {
            let current = self.session.settings.theme_name();
            let following = match self.session.settings.theme {
                None => ", following the terminal's background",
                Some(_) => "",
            };
            return self.say(&format!(
                "Theme: {current}{following}; the themes are {names}: /theme NAME"
            ));
        }
        if self.themes.get(wanted).is_none() {
            return self.say(&format!("Error: no theme {wanted}; the themes are {names}"));
        }
        let saved = self.session.settings.set_theme(wanted);
        self.use_theme();
        if let Err(error) = saved {
            return self.say(&format!("Error: settings not saved: {error}"));
        }
        self.say(&format!("Theme: {wanted}"));
    }

    /// `/btw QUESTION`: the model's answer from the conversation so far, on the full screen in a
    /// view of its own. Typed during a turn, it is asked at the turn's next pause, the turn's
    /// reply and calls going on meanwhile, and kept once the turn has ended.
    fn btw(&mut self, question: &str) {
        if question.is_empty() {
            return self.say("Usage: /btw <question>");
        }
        let ask = |agent: &mut Agent, ui: &mut dyn AgentUi, cancel: &AtomicBool| {
            agent.side_question(question, ui, cancel)
        };
        // A paused turn's screen already shows a turn running, and goes on showing it.
        let result = match self.in_turn {
            true => {
                let result = self.on_screen(ask);
                if let Some(screen) = &self.screen {
                    screen.aside_ended();
                }
                result
            }
            false => self.during_turn(ask),
        };
        let aside = match result {
            Ok(aside) => aside,
            Err(AgentError::Cancelled) => return self.say_styled(style::MUTED, "(cancelled)"),
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        match &self.screen {
            Some(screen) => screen.show_report(aside_view(question, &aside.answer)),
            None => self.say(&format!("btw: {question}\n\n{}", aside.answer)),
        }
        if self.in_turn {
            self.asides.push(aside);
        } else if let Err(error) = self.session.agent.keep_aside(aside) {
            self.say(&format!("Error: {error}"));
        }
    }

    /// `/recap`: where the session stands, from the model as `/btw` asks, shown as a note. Typed
    /// during a turn, it is asked at the turn's next pause and kept once the turn has ended.
    fn recap(&mut self) {
        if self.session.agent.prompts().is_empty() {
            return self.say_styled(style::MUTED, "Nothing to recap yet");
        }
        let ask =
            |agent: &mut Agent, ui: &mut dyn AgentUi, cancel: &AtomicBool| agent.recap(ui, cancel);
        let result = match self.in_turn {
            true => {
                let result = self.on_screen(ask);
                if let Some(screen) = &self.screen {
                    screen.aside_ended();
                }
                result
            }
            false => self.during_turn(ask),
        };
        let aside = match result {
            Ok(aside) => aside,
            Err(AgentError::Cancelled) => return self.say_styled(style::MUTED, "(cancelled)"),
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        self.say_styled(style::MUTED, &format!("Recap: {}", aside.answer));
        if self.in_turn {
            self.asides.push(aside);
        } else if let Err(error) = self.session.agent.keep_aside(aside) {
            self.say(&format!("Error: {error}"));
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

/// `/btw`'s answer on the full screen, under its question.
fn aside_view(question: &str, answer: &str) -> ReportView {
    let summary = format!(
        "**{}**\n\n{answer}\n\n*Not in the conversation: the model does not see this again.*",
        question.trim()
    );
    let tab = Tab {
        name: "Side question".into(),
        summary,
        details: String::new(),
    };
    ReportView::new("Side question", "answer", vec![tab], 0, false).markdown()
}

/// What `/mcp ARGUMENT` asks for, as Claude Code's `/mcp`; `None` for what it does not take.
#[derive(Debug, PartialEq)]
pub(super) enum McpCommand<'a> {
    List,
    /// With the server's name, or `None` for all.
    Reconnect(Option<&'a str>),
    Enable(Option<&'a str>),
    Disable(Option<&'a str>),
}

pub(super) fn mcp_command(argument: &str) -> Option<McpCommand<'_>> {
    let mut words = argument.split_whitespace();
    let Some(action) = words.next() else {
        return Some(McpCommand::List);
    };
    let target = match (words.next(), words.next()) {
        (None, _) | (Some("all"), None) => None,
        (Some(name), None) => Some(name),
        _ => return None,
    };
    Some(match action {
        "reconnect" => McpCommand::Reconnect(target),
        "enable" => McpCommand::Enable(target),
        "disable" => McpCommand::Disable(target),
        _ => return None,
    })
}

const SANDBOX_USAGE: &str = "Usage: /sandbox [read-only|workspace-write|off | network on|off \
| add DIR | remove DIR], or /add-dir DIR";

/// What `/sandbox` shows: the mode, the network, what is writable and what stays read-only in it.
fn sandbox_info(agent: &Agent, allow_write: &[PathBuf]) -> String {
    let mut info = format!("{}\n\n", style::mark(style::INFO_HEADING, "Sandbox"));
    let mut rows = Vec::new();
    match agent.sandbox() {
        None => {
            info += "None: the agent's tools run with your access, the credential stores hidden \
as the hideCredentials setting says.\n";
            if agent.offline() {
                rows.push(("Network:", "off".to_owned()));
            }
        }
        Some(sandbox) => {
            let network = if sandbox.network { "on" } else { "off" };
            rows.push(("Mode:", sandbox.mode.name().to_owned()));
            rows.push(("Network:", network.to_owned()));
        }
    }
    rows.extend(sandbox_paths(agent, allow_write));
    if agent.sandbox().is_none() && !rows.is_empty() {
        info += "\n";
    }
    for (label, value) in rows {
        info += &format!("{} {value}\n", style::mark(style::MUTED, label));
    }
    if let Some(note) = unconfined_note(agent) {
        info += &format!("\n{note}\n");
    }
    info + &format!("\n{SANDBOX_USAGE}")
}

/// What `/sandbox`'s list says under its mode: what is writable, what stays read-only in it,
/// and what is hidden from the agent's tools.
pub(super) fn sandbox_summary(agent: &Agent, allow_write: &[PathBuf]) -> String {
    (sandbox_paths(agent, allow_write).into_iter())
        .map(|(label, value)| format!("{label} {value}."))
        .chain(unconfined_note(agent))
        .collect::<Vec<_>>()
        .join(" ")
}

/// On Windows without a sandbox, that commands are not kept to what the settings hide as the
/// file tools are.
fn unconfined_note(agent: &Agent) -> Option<String> {
    let reads = agent.reads();
    (cfg!(windows) && agent.sandbox().is_none() && (reads.reach.kept() || !reads.deny.is_empty()))
        .then(|| {
            "Commands are not kept to what is hidden: Windows cannot hide files from them; only \
read, grep and find are."
                .to_owned()
        })
}

/// The sandbox's paths, labeled: writable, granted but gone, and read-only in the writable ones; then what the settings hide from the agent's tools and re-open, with
/// or without a sandbox.
fn sandbox_paths(agent: &Agent, allow_write: &[PathBuf]) -> Vec<(&'static str, String)> {
    let root = agent.root();
    let project = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    let from_settings: Vec<PathBuf> = (allow_write.iter())
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect();
    let listed = |paths: &[PathBuf]| match paths.is_empty() {
        true => "none".to_owned(),
        false => (paths.iter())
            .map(|path| match path {
                path if *path == project => format!("{} (the project)", path.display()),
                path if from_settings.contains(path) => {
                    format!("{} (allowWrite)", path.display())
                }
                path => path.display().to_string(),
            })
            .collect::<Vec<_>>()
            .join(", "),
    };
    let mut rows = Vec::new();
    if let Some(sandbox) = agent.sandbox() {
        let writable = sandbox.writable_roots(root);
        rows.push(("Writable:", listed(&writable)));
        let missing: Vec<_> = (sandbox.writable.iter())
            .filter(|path| !path.is_dir())
            .cloned()
            .collect();
        if !missing.is_empty() {
            rows.push(("Granted, gone:", listed(&missing)));
        }
        let mut kept = sandbox.protected_paths(root);
        if sandbox.read_only_project(root).is_some()
            && (writable.iter()).any(|path| project.starts_with(path) && *path != project)
        {
            kept.insert(0, project.clone());
        }
        if !kept.is_empty() {
            rows.push(("Read-only in them:", listed(&kept)));
        }
    } else if agent.reads().reach.kept() && !agent.writable().is_empty() {
        rows.push(("Also writable:", listed(agent.writable())));
    }
    let reads = agent.reads();
    if !reads.deny.is_empty() {
        rows.push(("Hidden:", listed(&reads.deny)));
    }
    let reach = match reads.reach {
        _ if cfg!(windows) && reads.reach.kept() => Some(
            "the project, the writable directories and what Scramjet gives the agent to read; \
the rest of home and the other profiles hidden (blockReadsOutsideWorkingDirectories)",
        ),
        Reach::Everything => None,
        Reach::Project => Some(
            "the project, the writable directories and what Scramjet gives the agent to read; \
the rest of home and the mounted drives hidden (blockReadsOutsideWorkingDirectories)",
        ),
        Reach::Minimal => Some(
            "the system's directories, the project, the writable directories and what Scramjet \
gives the agent to read; the rest hidden (blockReadsOutsideWorkingDirectories: \"minimal\")",
        ),
    };
    if let Some(reach) = reach {
        rows.push(("Reach:", reach.to_owned()));
    }
    if !reads.allow.is_empty() && (reads.reach.kept() || !reads.deny.is_empty()) {
        rows.push(("Re-opened:", listed(&reads.allow)));
    }
    rows
}

/// `/session`: the session's name, file and id, its messages, token usage and cost over the
/// current conversation (subagents' too, in the cost), as Scramjet's `/session` shows them. Ends
/// with a newline.
fn session_info(agent: &Agent) -> String {
    let stats = agent.stats();
    let row =
        |label: &str, value: String| format!("{} {value}\n", style::mark(style::MUTED, label));
    let heading = |text: &str| format!("{}\n", style::mark(style::INFO_HEADING, text));
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
    if let Some(sandbox) = agent.sandbox() {
        let network = match sandbox.network {
            true => "network on",
            false => "no network",
        };
        info += &row("Sandbox:", format!("{}, {network}", sandbox.mode.name()));
    } else if agent.offline() {
        info += &row("Sandbox:", "none, no network".to_owned());
    }
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
    let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
        .map_or(0, |since| since.as_millis() as u64);
    let quotas = scramjet::agent::quota::read();
    if let Some(quota) = super::usage::status_quota(&quotas, agent.model(), now) {
        info += "\n";
        info += &quota;
    }
    info
}

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

/// Writes `agent`'s current branch to `path`, relative to `directory`: to a path ending in
/// `.jsonl` as a session file of its own, to one in `.txt` as text, else as a web page, by default named for the session,
/// as Scramjet names it. Returns where, or none before any conversation.
pub(super) fn export_branch(
    agent: &Agent,
    path: Option<&str>,
    directory: &Path,
) -> Result<Option<PathBuf>, AgentError> {
    if agent.conversation().is_empty() {
        return Ok(None);
    }
    let id = agent.session_id();
    let path = match path {
        Some(path) => directory.join(path),
        None => directory.join(format!("scramjet-session-{id}.html")),
    };
    let path = std::path::absolute(path)?;
    if path.to_string_lossy().ends_with(".jsonl") {
        agent.export_session(&path)?;
    } else if export::is_text(&path) {
        let width = super::editor::terminal_size().0;
        std::fs::write(&path, export::text(&agent.branch()?, width))?;
    } else {
        let tools = agent.tool_definitions();
        let setup = export::Setup {
            instructions: &agent.instructions(),
            tools: &tools,
        };
        let page = export::page(&agent.branch()?, &id, Some(setup));
        std::fs::write(&path, page)?;
    }
    Ok(Some(scramjet::os::canonicalize(&path).unwrap_or(path)))
}

/// A count with thousands separators, as Scramjet's `toLocaleString` writes it.
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

/// A number of tokens as typed: `30000`, `30,000`, `30k` or `1.5k`.
fn token_count(typed: &str) -> Option<u64> {
    let typed = typed.replace(',', "");
    let (number, scale) = match typed.strip_suffix(['k', 'K']) {
        Some(number) => (number, 1e3),
        None => (typed.as_str(), 1.0),
    };
    let parsed = number.parse::<f64>().ok()? * scale;
    let whole = number.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    (whole && parsed.is_finite() && parsed < 1e12).then(|| parsed.round() as u64)
}

/// How a task ended, or how long it has run: `exited with code 0 after 3s`.
fn task_state(task: &scramjet::agent::tasks::Task) -> String {
    match &task.ended {
        Some(ending) => format!("{} after {}s", ending.said(), task.ran.as_secs()),
        None => format!("running for {}s", task.ran.as_secs()),
    }
}

/// A task's command, how it ended or runs, its file and the end of its output so far.
fn task_output(task: &scramjet::agent::tasks::Task) -> ReportView {
    let output = match task.output_tail() {
        Some(text) if text.is_empty() => "(no output yet)".to_owned(),
        Some(text) => text
            .lines()
            .map(super::text::printable)
            .collect::<Vec<_>>()
            .join("\n"),
        None => "(its output file cannot be read)".to_owned(),
    };
    let summary = format!(
        "$ {}\n{}; output in {}\n\n{output}",
        visible(&task.command),
        task_state(task),
        scramjet::os::tilde(&task.output)
    );
    let tab = Tab {
        name: "Output".to_owned(),
        summary,
        details: String::new(),
    };
    ReportView::new(&format!("Task {}", task.id), "", vec![tab], 0, false).at_end()
}

/// What `/copy`'s list chose.
#[derive(Clone, Copy)]
enum Copied {
    Full,
    Always,
    Block(usize),
}

fn choice(label: &str, description: Option<String>) -> scramjet::agent::Choice {
    scramjet::agent::Choice {
        label: label.to_owned(),
        description,
        value: String::new(),
    }
}

/// `12 characters, 2 lines`, with `unit` the first count's, singular.
fn sized(text: &str, unit: &str) -> String {
    let plural = |count: usize, unit: &str| match count {
        1 => format!("1 {unit}"),
        count => format!("{count} {unit}s"),
    };
    let lines = text.matches('\n').count() + 1;
    format!(
        "{}, {}",
        plural(text.chars().count(), unit),
        plural(lines, "line")
    )
}

/// The code blocks of the Markdown `text`, each with its fence's language if it names one.
fn code_blocks(text: &str) -> Vec<(String, Option<String>)> {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut blocks = Vec::new();
    let mut open: Option<(String, Option<String>)> = None;
    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_owned)
                    }
                    CodeBlockKind::Indented => None,
                };
                open = Some((String::new(), language));
            }
            Event::Text(code) => {
                if let Some((block, _)) = &mut open {
                    block.push_str(&code);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((block, language)) = open.take() {
                    let block = block.strip_suffix('\n').unwrap_or(&block).to_owned();
                    blocks.push((block, language));
                }
            }
            _ => {}
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_offers_the_code_blocks_with_their_languages() {
        let reply = "Run:\n\n```bash title\ncargo test\ncargo fmt\n```\n\nThen\n\n    indented\n\n`inline` and\n\n~~~\nplain\n~~~";
        assert_eq!(
            code_blocks(reply),
            [
                ("cargo test\ncargo fmt".to_owned(), Some("bash".to_owned())),
                ("indented".to_owned(), None),
                ("plain".to_owned(), None),
            ]
        );
        assert!(code_blocks("No **code** here, `inline` only.").is_empty());
        assert_eq!(sized("one\ntwo", "char"), "7 chars, 2 lines");
        assert_eq!(sized("é", "character"), "1 character, 1 line");
    }

    #[test]
    fn token_counts_take_plain_grouped_and_k_forms() {
        assert_eq!(token_count("30000"), Some(30_000));
        assert_eq!(token_count("30,000"), Some(30_000));
        assert_eq!(token_count("30k"), Some(30_000));
        assert_eq!(token_count("1.5K"), Some(1_500));
        assert_eq!(token_count("0"), Some(0));
        for wrong in ["", "k", "-5", "1e5", "inf", "lots", "5m"] {
            assert_eq!(token_count(wrong), None, "{wrong}");
        }
    }

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
            "/diff",
            "/agents",
            "/skills",
            "/memory",
            "/doctor",
            "/context",
            "/system-prompt",
            "/export",
            "/export out.jsonl",
            "/hotkeys",
            "/keybindings",
            "/keymap",
            "/release-notes",
            "/changelog",
            "/copy",
            "/rename",
            "/rename  my work ",
            "/model",
            "/model gpt-6-terra",
            "/scoped-models",
            "/login",
            "/logout",
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
            "/context all",
            "/system-prompt all",
            "/hotkeys all",
            "/scoped-models luna",
            "/login github",
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
        assert_eq!(Builtin::parse("/color Blue"), Some(Builtin::Color("Blue")));
        assert_eq!(Builtin::parse("/color"), Some(Builtin::Color("")));
        assert_eq!(Builtin::parse("/copy 2 "), Some(Builtin::Copy("2")));
        assert_eq!(stops("/copy all"), Some(false));
        assert_eq!(Builtin::parse("/copy"), Some(Builtin::Copy("")));
        // A new session's name, as Claude Code's `/clear [name]` and `/branch [name]` take one.
        assert_eq!(
            Builtin::parse("/clear  next "),
            Some(Builtin::Clear("next"))
        );
        assert_eq!(Builtin::parse("/branch try"), Some(Builtin::Branch("try")));
        assert_eq!(
            Builtin::parse("/effort  high "),
            Some(Builtin::Effort("high"))
        );
        assert_eq!(Builtin::parse("/effort"), Some(Builtin::Effort("")));
        assert_eq!(
            Builtin::parse("/theme pi-light"),
            Some(Builtin::Theme("pi-light"))
        );
        assert_eq!(Builtin::parse("/theme"), Some(Builtin::Theme("")));
        assert_eq!(
            Builtin::parse("/autocompact 30k"),
            Some(Builtin::AutoCompact("30k"))
        );
        assert_eq!(
            Builtin::parse("/autocompact"),
            Some(Builtin::AutoCompact(""))
        );
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
            ("/cost", Builtin::Usage(false)),
            ("/stats", Builtin::Usage(false)),
            ("/config", Builtin::Settings),
            ("/clone", Builtin::Branch("")),
            ("/continue 2", Builtin::Resume("2")),
            ("/help", Builtin::Hotkeys),
            ("/keymap", Builtin::Hotkeys),
            ("/changelog", Builtin::ReleaseNotes),
        ] {
            assert_eq!(Builtin::parse(typed), Some(listed), "{typed}");
        }
        assert_eq!(Builtin::parse("/session now"), None);
    }

    #[test]
    fn token_counts_are_grouped_by_thousands() {
        let counts = [0, 999, 1_000, 15_224, 1_234_567].map(grouped);
        assert_eq!(counts, ["0", "999", "1,000", "15,224", "1,234,567"]);
    }

    #[test]
    fn mcp_reconnect_enable_and_disable_take_a_server_or_all() {
        use McpCommand::*;
        assert_eq!(Builtin::parse("/mcp"), Some(Builtin::Mcp("")));
        assert_eq!(mcp_command(""), Some(List));
        assert_eq!(mcp_command("reconnect"), Some(Reconnect(None)));
        assert_eq!(mcp_command("reconnect all"), Some(Reconnect(None)));
        assert_eq!(
            mcp_command("reconnect  time "),
            Some(Reconnect(Some("time")))
        );
        assert_eq!(mcp_command("enable"), Some(Enable(None)));
        assert_eq!(mcp_command("disable time"), Some(Disable(Some("time"))));
        assert_eq!(mcp_command("disable all"), Some(Disable(None)));
        assert_eq!(mcp_command("reconnect a b"), None);
        assert_eq!(mcp_command("reconnected"), None);
        assert_eq!(mcp_command("time"), None);
    }
}
