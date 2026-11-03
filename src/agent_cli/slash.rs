//! The built-in commands: `/new`, `/name`, `/session`, `/resume`, `/fork`, `/tree`, `/model`,
//! `/compact`, and `!` commands.
use super::interactive::{Interactive, model_line};
use super::terminal::Terminal;
use super::text::{clip, one_line, visible};
use super::{editor, select, style, tree};
use scramjet_rs::agent::{Agent, AgentError, AgentUi, Navigated, SessionInfo};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use unicode_width::UnicodeWidthStr;

impl Interactive {
    /// Runs `prompt` if it is a built-in command; returns whether it was. `escaped`: a double
    /// Escape erased the prompt for `/tree`.
    pub(super) fn builtin(&mut self, prompt: &str, escaped: bool) -> Result<bool, String> {
        if let Some(command) = prompt.strip_prefix('!') {
            return Ok(self.bang(prompt, command));
        }
        let (name, argument) = prompt
            .split_once(char::is_whitespace)
            .unwrap_or((prompt, ""));
        let argument = argument.trim();
        match name {
            "/new" if argument.is_empty() => {
                self.new_session();
            }
            "/name" => self.name(argument),
            "/session" if argument.is_empty() => {
                eprint!("{}", session_info(&self.agent, &self.ui))
            }
            "/resume" => self.resume(argument)?,
            "/fork" => self.fork(argument)?,
            "/tree" => self.tree(argument, escaped)?,
            "/model" => self.model(argument)?,
            "/compact" => self.compact(argument),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// `/new`, also before a next step that wants a fresh session. Returns whether it started.
    pub(super) fn new_session(&mut self) -> bool {
        match self.agent.new_session() {
            Ok(()) => {
                eprintln!(
                    "✓ New session started\nsession {}",
                    self.agent.session_path().display()
                );
                true
            }
            Err(error) => {
                eprintln!("error: {error}");
                false
            }
        }
    }

    fn name(&mut self, name: &str) {
        if name.is_empty() {
            match self.agent.session_name() {
                Some(current) => eprintln!("{}", self.ui.dim(&format!("Session name: {current}"))),
                None => eprintln!("Usage: /name <name>"),
            }
        } else {
            match self.agent.set_session_name(name) {
                Ok(()) => eprintln!("{}", self.ui.dim(&format!("Session name set: {name}"))),
                Err(error) => eprintln!("error: {error}"),
            }
        }
    }

    fn resume(&mut self, wanted: &str) -> Result<(), String> {
        let sessions = match self.agent.sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                eprintln!("error: {error}");
                return Ok(());
            }
        };
        let current = sessions
            .iter()
            .position(|session| self.agent.is_session(&session.path));
        let chosen = match wanted {
            "" if sessions.is_empty() => {
                eprintln!("No sessions to resume");
                None
            }
            "" if self.editor.is_some() => {
                // Newest first, so the first other than the current one is the likely choice.
                let initial = usize::from(current == Some(0));
                let texts: Vec<_> = sessions.iter().map(search_text).collect();
                select::search(
                    "Resume session",
                    select::VISIBLE,
                    &texts,
                    initial,
                    current,
                    "No matching sessions",
                    |index, width| session_line(&sessions[index], width),
                )
                .map_err(|e| e.to_string())?
            }
            "" => {
                let width = editor::terminal_size().0;
                for (index, session) in sessions.iter().take(20).enumerate() {
                    let mark = if current == Some(index) { "*" } else { " " };
                    let number = format!("{}{mark}", index + 1);
                    let line = session_line(session, width.saturating_sub(number.len() + 1));
                    eprintln!("{number} {line}");
                }
                if sessions.len() > 20 {
                    eprintln!("({} older sessions not shown)", sessions.len() - 20);
                }
                None
            }
            wanted => match wanted.parse::<usize>() {
                Err(_) | Ok(0) => {
                    eprintln!("error: /resume requires a positive session number");
                    None
                }
                Ok(number) if number > sessions.len() => {
                    eprintln!("error: no session numbered {number} (/resume lists them)");
                    None
                }
                Ok(number) => Some(number - 1),
            },
        };
        match chosen {
            None => {}
            Some(index) if current == Some(index) => {
                eprintln!("error: session {} is the current one", index + 1)
            }
            Some(index) => match self.agent.resume_session(&sessions[index].path) {
                Ok(()) => {
                    self.agent.load_model_card();
                    if let Some(editor) = &mut self.editor {
                        for prompt in self.agent.prompts() {
                            editor.remember(prompt);
                        }
                    }
                    eprintln!(
                        "✓ Resumed session · {}\nsession {}",
                        model_line(&self.agent),
                        self.agent.session_path().display(),
                    );
                    self.show_resumed();
                }
                Err(error) => eprintln!("error: {error}"),
            },
        }
        Ok(())
    }

    fn fork(&mut self, wanted: &str) -> Result<(), String> {
        let prompts = match self.agent.fork_prompts() {
            Ok(prompts) => prompts,
            Err(error) => {
                eprintln!("error: {error}");
                return Ok(());
            }
        };
        let chosen = match wanted {
            "" if prompts.is_empty() => {
                eprintln!("No messages to fork from");
                None
            }
            "" if self.editor.is_some() => select::select(
                "Fork from message",
                select::VISIBLE,
                prompts.len(),
                prompts.len() - 1,
                None,
                |index, width| clip(&one_line(&prompts[index]), width),
            )
            .map_err(|e| e.to_string())?,
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
                    eprintln!("error: /fork requires a positive message number");
                    None
                }
                Ok(number) if number > prompts.len() => {
                    eprintln!("error: no message numbered {number} (/fork lists them)");
                    None
                }
                Ok(number) => Some(number - 1),
            },
        };
        let Some(index) = chosen else {
            return Ok(());
        };
        match self.agent.fork(index) {
            Ok(Some(prompt)) => {
                self.agent.load_model_card();
                eprintln!(
                    "✓ Forked to new session · {}\nsession {}",
                    model_line(&self.agent),
                    self.agent.session_path().display(),
                );
                self.show_resumed();
                if let Some(editor) = &mut self.editor {
                    editor.set_text(prompt);
                }
            }
            Ok(None) => eprintln!(
                "error: no message numbered {} (/fork lists them)",
                index + 1
            ),
            Err(error) => eprintln!("error: {error}"),
        }
        Ok(())
    }

    fn tree(&mut self, mut wanted: &str, mut escaped: bool) -> Result<(), String> {
        // The row the tree shows again at after its summary question or summary is cancelled.
        let mut reopen = None;
        loop {
            let session_tree = match self.agent.tree() {
                Ok(tree) => tree,
                Err(error) => {
                    eprintln!("error: {error}");
                    return Ok(());
                }
            };
            let (rows, current) = tree::rows(&session_tree);
            let chosen = match wanted {
                "" if rows.is_empty() => {
                    eprintln!("No entries in session");
                    None
                }
                "" if self.editor.is_some() => {
                    // As Scramjet's tree, half the terminal.
                    let visible = (editor::terminal_size().1 / 2).max(5);
                    select::select(
                        "Session Tree",
                        visible,
                        rows.len(),
                        reopen.or(current).unwrap_or(rows.len() - 1),
                        None,
                        |index, width| clip(&rows[index].1, width),
                    )
                    .map_err(|e| e.to_string())?
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
                        eprintln!("error: /tree requires a positive entry number");
                        None
                    }
                    Ok(number) if number > rows.len() => {
                        eprintln!("error: no entry numbered {number} (/tree lists them)");
                        None
                    }
                    Ok(number) => Some(number - 1),
                },
            };
            let Some(index) = chosen else {
                // Nothing is left where the erased prompt was, so it is drawn there again.
                self.spaced = escaped && !rows.is_empty();
                return Ok(());
            };
            let line = session_tree.entries[rows[index].0].line;
            // As Scramjet asks when the editor is in use; piped input moves without one.
            let summary = match self
                .editor
                .is_some()
                .then(|| self.agent.leaves_branch(line))
            {
                Some(Ok(true)) => {
                    match ask_branch_summary(&self.hide_thinking).map_err(|e| e.to_string())? {
                        Some(summary) => summary,
                        None => {
                            (wanted, reopen) = ("", Some(index));
                            continue;
                        }
                    }
                }
                None | Some(Ok(false)) => None,
                Some(Err(error)) => {
                    eprintln!("error: {error}");
                    return Ok(());
                }
            };
            let result = match summary.as_deref() {
                Some(focus) => self.during_turn(None, |agent, ui, cancel| {
                    agent.navigate(line, Some(focus), ui, cancel)
                }),
                None => self.agent.navigate(line, None, &mut self.ui, &self.cancel),
            };
            match result {
                Ok(Navigated::AlreadyThere) => eprintln!("Already at this point"),
                Ok(Navigated::Moved { prompt }) => {
                    eprintln!("✓ Navigated to selected point");
                    self.show_resumed();
                    if let (Some(editor), Some(prompt)) = (&mut self.editor, prompt) {
                        editor.set_text(prompt);
                    }
                }
                Err(AgentError::Cancelled) => {
                    eprintln!("Branch summarization cancelled");
                    escaped = false;
                    (wanted, reopen) = ("", Some(index));
                    continue;
                }
                Err(error) => eprintln!("error: {error}"),
            }
            return Ok(());
        }
    }

    fn model(&mut self, wanted: &str) -> Result<(), String> {
        let Some(models) = self.agent.models() else {
            eprintln!("error: could not list the endpoint's models");
            return Ok(());
        };
        let current = models.iter().position(|model| model == self.agent.model());
        let chosen = match wanted {
            "" if models.is_empty() => {
                eprintln!("The endpoint lists no models");
                None
            }
            "" if self.editor.is_some() => select::search(
                "Select model",
                select::VISIBLE,
                &models,
                current.unwrap_or(0),
                current,
                "No matching models",
                |index, width| clip(&visible(&models[index]), width),
            )
            .map_err(|e| e.to_string())?
            .map(|index| models[index].clone()),
            "" => {
                for (index, model) in models.iter().enumerate() {
                    let mark = if current == Some(index) { "*" } else { " " };
                    eprintln!("{mark} {model}");
                }
                None
            }
            wanted if models.iter().any(|model| model == wanted) => Some(wanted.to_owned()),
            wanted => {
                eprintln!("error: no model {wanted} (/model lists them)");
                None
            }
        };
        if let Some(model) = chosen {
            match self.agent.set_model(&model) {
                Ok(()) => {
                    self.agent.load_model_card();
                    eprintln!("✓ Model set to {}", model_line(&self.agent));
                }
                Err(error) => eprintln!("error: {error}"),
            }
        }
        Ok(())
    }

    fn compact(&mut self, focus: &str) {
        let result = self.during_turn(None, |agent, ui, cancel| agent.compact(focus, ui, cancel));
        match result {
            Ok(Some(compaction)) => self.ui.compacted(&compaction, None),
            Ok(None) => eprintln!("Nothing to compact: the conversation is all recent."),
            Err(AgentError::Cancelled) => eprintln!("{}", self.ui.dim("(cancelled)")),
            Err(error) => eprintln!("error: {error}"),
        }
    }

    /// `!command` runs a command without a model turn; `!!command` keeps it out of the context.
    /// Returns whether there was a command to run.
    fn bang(&mut self, prompt: &str, command: &str) -> bool {
        let (include, command) = match command.strip_prefix('!') {
            Some(command) => (false, command),
            None => (true, command),
        };
        let command = command.trim();
        if command.is_empty() {
            return false;
        }
        if let Some(editor) = &mut self.editor {
            editor.remember(prompt);
            eprintln!();
        }
        let result = self.during_turn(None, |agent, ui, cancel| {
            agent.shell(command, include, ui, cancel)
        });
        match result {
            Ok(()) => self.spaced = true,
            Err(error) => eprintln!("error: {error}"),
        }
        true
    }
}

/// Asks whether to summarize the branch `/tree` leaves, as Scramjet does. `None`: cancelled;
/// otherwise no summary, or one with the additional focus given (empty for none).
fn ask_branch_summary(hide_thinking: &Arc<AtomicBool>) -> std::io::Result<Option<Option<String>>> {
    const CHOICES: [&str; 3] = ["No summary", "Summarize", "Summarize with custom prompt"];
    const TITLE: &str = "Custom summarization instructions";
    loop {
        let chosen = select::select(
            "Summarize branch?",
            select::VISIBLE,
            CHOICES.len(),
            0,
            None,
            |index, width| clip(CHOICES[index], width),
        )?;
        match chosen {
            None => return Ok(None),
            Some(0) => return Ok(Some(None)),
            Some(1) => return Ok(Some(Some(String::new()))),
            Some(_) => {
                let room = editor::terminal_size().0.saturating_sub(TITLE.width() + 3);
                eprintln!(
                    "{}{TITLE}{}  {}{}{}",
                    style::SELECTOR_TITLE,
                    style::RESET,
                    style::MUTED,
                    clip("Enter submit · Esc cancel", room),
                    style::RESET
                );
                let focus = editor::Editor::dialog(hide_thinking.clone()).read(&mut String::new)?;
                // The title's row, above the erased editor.
                eprint!("\x1b[1A\r\x1b[J");
                if let Some(editor::Input::Prompt(focus)) = focus {
                    return Ok(Some(Some(focus)));
                }
            }
        }
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
    fn token_counts_are_grouped_by_thousands() {
        let counts = [0, 999, 1_000, 15_224, 1_234_567].map(grouped);
        assert_eq!(counts, ["0", "999", "1,000", "15,224", "1,234,567"]);
    }
}
