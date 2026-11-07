//! The built-in commands: `/export`, `/import`, `/copy`, `/clear`, `/rename`, `/status`,
//! `/compact`, `/autopilot`, `/reload`, and `!` commands. `/settings` is in `settings_menu.rs`;
//! `/hotkeys` and `/keybindings` in `hotkeys.rs`; `/resume`, `/fork`, `/branch`, `/clone` and
//! `/tree` in `sessions.rs`; `/model` and `/scoped-models` in `scope.rs`; `/login` and `/logout`
//! in `login.rs`.
use super::interactive::{Interactive, completions, model_line, saved_line};
use super::terminal::{self, Terminal};
use super::text::{clip, visible};
use super::{clipboard, completion, export, keymap, style};
use scramjet_rs::agent::{Agent, AgentError, McpState, Providers};
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
};

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
    Usage,
    Hotkeys,
    Keybindings,
    Resume(&'a str),
    Fork(&'a str),
    /// With the new session's name, if given.
    Branch(&'a str),
    Tree(&'a str),
    Model(&'a str),
    ScopedModels,
    Login,
    Logout,
    Compact(&'a str),
    Autopilot(&'a str),
    /// With its argument: `reconnect`, `enable` or `disable`, with a server's name or `all`.
    Mcp(&'a str),
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
            "usage" if argument.is_empty() => Self::Usage,
            "hotkeys" if argument.is_empty() => Self::Hotkeys,
            "keybindings" if argument.is_empty() => Self::Keybindings,
            "resume" => Self::Resume(argument),
            "fork" => Self::Fork(argument),
            "branch" => Self::Branch(argument),
            "tree" => Self::Tree(argument),
            "model" => Self::Model(argument),
            "scoped-models" if argument.is_empty() => Self::ScopedModels,
            "login" if argument.is_empty() => Self::Login,
            "logout" if argument.is_empty() => Self::Logout,
            "compact" => Self::Compact(argument),
            "autopilot" => Self::Autopilot(argument),
            "mcp" => Self::Mcp(argument),
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
                | Self::Usage
                | Self::Hotkeys
                | Self::Keybindings
                | Self::Model(_)
                | Self::ScopedModels
                | Self::Login
                | Self::Logout
                | Self::Autopilot(_)
                | Self::Mcp(_)
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
            Some(Builtin::Usage) => self.usage(),
            Some(Builtin::Hotkeys) => self.hotkeys(),
            Some(Builtin::Keybindings) => self.keybindings(),
            Some(Builtin::Resume(wanted)) => {
                self.resume(wanted);
            }
            Some(Builtin::Fork(wanted)) => self.fork(wanted),
            Some(Builtin::Branch(name)) => self.clone_session(name),
            Some(Builtin::Tree(wanted)) => self.tree(wanted),
            Some(Builtin::Model(wanted)) => self.model(wanted),
            Some(Builtin::ScopedModels) => self.scoped_models(),
            Some(Builtin::Login) => self.login(),
            Some(Builtin::Logout) => self.logout(),
            Some(Builtin::Compact(focus)) => self.compact(focus),
            Some(Builtin::Autopilot(wanted)) => self.autopilot(wanted),
            Some(Builtin::Mcp(argument)) => self.mcp(argument),
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
                self.agent.load_model_info();
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

    /// `/usage`: what the saved sessions spent, read from their files; on the full screen, a
    /// view of it, provider by provider.
    fn usage(&mut self) {
        let sessions = match scramjet_rs::agent::state_directory() {
            Ok(state) => state.join("sessions"),
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        let current = self.agent.session_path();
        let history = scramjet_rs::agent::usage::History::read(&sessions, current);
        let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
            .map_or(0, |since| since.as_millis() as u64);
        let model = self.agent.model();
        if let Some(screen) = &self.screen {
            let view =
                super::usage::UsageView::new(&history, &sessions, current, model, now, &self.ui);
            return screen.show_usage(view);
        }
        let report = super::usage::report(&history, &sessions, current, model, now, None, &self.ui);
        self.say(report.trim_end());
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
        let servers = self.agent.mcp_servers();
        let disabled = self.agent.disabled_mcp();
        let enabled = self.agent.mcp_names().len() - disabled.len();
        if servers.is_empty() && (disabled.is_empty() || enabled > 0) {
            let settings = (self.settings.path()).map_or("the settings".into(), |path| {
                scramjet_rs::agent::shortened(&path.to_string_lossy())
            });
            return match self.settings.mcp_servers.is_empty() {
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
        let ui = &self.ui;
        // What a server wrote or says of itself is shown as text, its control characters escaped.
        let row = |label: &str, value: &str| format!("  {} {}\n", ui.dim(label), visible(value));
        let mut info = format!("{}\n", ui.styled(style::INFO_HEADING, "MCP Servers"));
        for (server, state) in servers {
            info += "\n";
            let said = match &state {
                McpState::Starting => "starting".to_owned(),
                McpState::Ready { tools, .. } if tools.len() == 1 => "1 tool".to_owned(),
                McpState::Ready { tools, .. } => format!("{} tools", tools.len()),
                McpState::Failed(_) => "not available".to_owned(),
            };
            let name = visible(&server.name);
            info += &format!("{name} {}\n", ui.dim(&format!("· {said}")));
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
            info += &format!("\n{} {}\n", visible(&server.name), ui.dim("· disabled"));
            let (label, address) = server.address();
            info += &row(label, &address);
        }
        self.say(info.trim_end());
    }

    /// `/mcp enable` and `disable`: starts or stops the server named `name`, or all with `None`,
    /// and saves which are disabled in the settings, as Claude Code keeps them.
    fn enable_mcp(&mut self, name: Option<&str>, enable: bool) {
        let names = self.agent.mcp_names();
        if names.is_empty() {
            return self.say_styled(style::MUTED, "No MCP servers: see /mcp");
        }
        if let Some(name) = name
            && !names.iter().any(|listed| listed == name)
        {
            return self.say(&format!("Error: no MCP server named {}", visible(name)));
        }
        let mut disabled = self.agent.mcp_disabled().to_vec();
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
        if let Err(error) = self.settings.set_disabled_mcp_servers(disabled.clone()) {
            self.say(&format!("Error: settings not saved: {error}"));
        }
        self.agent.set_mcp_disabled(disabled);
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
        self.settings.reload();
        self.discover_themes();
        self.use_theme();
        self.keybindings = keymap::load(self.settings.path());
        let settings = &self.settings;
        (self.hide_thinking).store(settings.hide_thinking_block, Ordering::Relaxed);
        self.ui.markdown_reads = settings.markdown_reads;
        self.agent.reload(
            settings.tools.clone(),
            settings.command_sets.clone(),
            settings.skills.clone(),
            settings.mcp_servers.clone(),
            settings.disabled_mcp_servers.clone(),
        );
        self.agent.set_providers(Providers::read(
            settings.models_files(),
            scramjet_rs::agent::login::Logins::user(),
        ));
        if let Some(screen) = &mut self.screen {
            (screen.link).reloaded(completions(&self.agent), &self.settings);
            (screen.link).models(self.agent.models().unwrap_or_default());
        }
        for notice in self.problem_notices() {
            self.ui().notice(&notice);
        }
        for problem in self.agent.providers().problems().to_vec() {
            self.ui().notice(&problem);
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
    if let Some(sandbox) = agent.sandbox() {
        let network = match sandbox.network {
            true => "network on",
            false => "no network",
        };
        info += &row("Sandbox:", format!("{}, {network}", sandbox.mode.name()));
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
    let quotas = scramjet_rs::agent::quota::read();
    if let Some(quota) = super::usage::status_quota(&quotas, agent.model(), now, ui) {
        info += "\n";
        info += &quota;
    }
    info
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
            "/keybindings",
            "/keymap",
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
            "/hotkeys all",
            "/copy all",
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
            ("/cost", Builtin::Usage),
            ("/stats", Builtin::Usage),
            ("/config", Builtin::Settings),
            ("/clone", Builtin::Branch("")),
            ("/continue 2", Builtin::Resume("2")),
            ("/help", Builtin::Hotkeys),
            ("/keymap", Builtin::Hotkeys),
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
