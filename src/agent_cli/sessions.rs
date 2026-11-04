//! The built-in commands that move to another session or entry: `/resume`, `/fork`,
//! `/branch`, `/clone` and `/tree`.
use super::fuzzy::Haystack;
use super::interactive::{Interactive, model_line, saved_line};
use super::screen;
use super::text::{clip, one_line};
use super::{editor, select, style, tree};
use scramjet_rs::agent::{AgentError, Navigated, Removal, SessionInfo, shortened};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

const RESUME_KEYS: &str =
    "Ctrl+S sort · Ctrl+N named · Ctrl+P path · Ctrl+R rename · Ctrl+D delete";

impl Interactive {
    /// `/resume`: chooses a session to continue in, by its number with piped input. Returns
    /// whether it switched to one.
    pub(super) fn resume(&mut self, wanted: &str) -> bool {
        let sessions = match self.agent.sessions() {
            Ok(sessions) => threaded(sessions),
            Err(error) => {
                self.say(&format!("Error: {error}"));
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
                    let width = width.saturating_sub(number.width());
                    let line = session_line(session, width, None, false);
                    eprintln!("{number}{line}");
                }
                if sessions.len() > 20 {
                    eprintln!("({} older sessions not shown)", sessions.len() - 20);
                }
                None
            }
            wanted => match wanted.parse::<usize>() {
                Err(_) | Ok(0) => {
                    self.say("Error: /resume requires a positive session number");
                    None
                }
                Ok(number) if number > sessions.len() => {
                    self.say(&format!(
                        "Error: no session numbered {number} (/resume lists them)"
                    ));
                    None
                }
                Ok(number) if current == Some(number - 1) => {
                    self.say(&format!("Error: session {number} is the current one"));
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
                    "✓ Resumed session · {}\n{}",
                    model_line(&self.agent),
                    saved_line(&self.agent),
                ));
                true
            }
            Err(error) => {
                self.say(&format!("Error: {error}"));
                false
            }
        }
    }

    /// Chooses a session on the full screen, as Scramjet's session selector does: forks under
    /// the sessions they came from, a query matching them by their prompts and replies, Ctrl+S
    /// sorting them another way, Ctrl+N showing only the named ones, Ctrl+P their paths, Ctrl+R
    /// renaming the selected one and Ctrl+D deleting it.
    fn choose_session(&mut self) -> Option<std::path::PathBuf> {
        let mut spot = None;
        let (mut sort, mut named_only, mut paths) = (Sort::Threaded, false, false);
        // The session selected when a key changed how the list shows, to select again.
        let mut kept: Option<std::path::PathBuf> = None;
        loop {
            let sessions = match self.agent.sessions() {
                Ok(sessions) => {
                    let sessions =
                        (sessions.into_iter()).filter(|session| !named_only || named(session));
                    match sort {
                        Sort::Threaded => threaded(sessions.collect()),
                        Sort::Recent | Sort::Relevance => {
                            sessions.map(|session| (session, String::new())).collect()
                        }
                    }
                }
                Err(error) => {
                    self.screen.as_ref()?.dismiss();
                    self.say(&format!("Error: {error}"));
                    return None;
                }
            };
            let screen = self.screen.as_ref()?;
            if sessions.is_empty() && !named_only {
                screen.dismiss();
                self.say("No sessions to resume");
                return None;
            }
            let current =
                (sessions.iter()).position(|(session, _)| self.agent.is_session(&session.path));
            // Newest first, so the first other than the current one is the likely choice.
            let initial = usize::from(current == Some(0));
            let texts = searched(&sessions);
            let row = |index: usize, width| {
                let session = &sessions[index].0;
                let style = match current == Some(index) {
                    true => Some(style::SESSION_CURRENT),
                    false => named(session).then_some(style::SESSION_NAMED),
                };
                session_line(session, width, style, paths)
            };
            let title = match named_only {
                true => format!("Resume session (named, {})", sort.label()),
                false => format!("Resume session ({})", sort.label()),
            };
            let none = match named_only {
                true => "No named sessions (Ctrl+N shows all)",
                false => "No matching sessions",
            };
            let picker =
                select::Picker::search(&title, select::VISIBLE, texts, initial, current, none, row);
            let mut picker = picker.noting(RESUME_KEYS, Vec::new()).acting().held();
            match sort {
                Sort::Threaded => {
                    let tree = sessions.iter().map(|(_, tree)| tree.clone()).collect();
                    picker = picker.threaded(tree, row);
                }
                Sort::Recent => picker = picker.in_order(),
                Sort::Relevance => {}
            }
            if let Some((at, status)) = spot.take() {
                picker = picker.at(at, status);
            }
            let keep = kept.take();
            if let Some(at) =
                (sessions.iter()).position(|(session, _)| Some(&session.path) == keep.as_ref())
            {
                picker = picker.on_row(at);
            }
            let (act, index, at) = match screen.pick_or_act(picker) {
                select::Picked::Row(chosen) => {
                    screen.dismiss();
                    return chosen.map(|index| sessions[index].0.path.clone());
                }
                select::Picked::Act(act, index, at) => (act, index, at),
                select::Picked::Toggle(toggle, at) => {
                    kept = (at.shown.get(at.position)).map(|&row| sessions[row].0.path.clone());
                    match toggle {
                        select::Toggle::Sort => sort = sort.next(),
                        select::Toggle::Named => named_only = !named_only,
                        select::Toggle::Paths => paths = !paths,
                    }
                    spot = Some((at, None));
                    continue;
                }
                // Not keys of this list (`Picker::tabbing`, `Picker::tree`).
                select::Picked::Tab(_) | select::Picked::Label(_) => {
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

    pub(super) fn fork(&mut self, wanted: &str) {
        let prompts = match self.agent.fork_prompts() {
            Ok(prompts) => prompts,
            Err(error) => {
                self.say(&format!("Error: {error}"));
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
                    self.say("Error: /fork requires a positive message number");
                    None
                }
                Ok(number) if number > prompts.len() => {
                    self.say(&format!(
                        "Error: no message numbered {number} (/fork lists them)"
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
                    "✓ Forked to new session · {}\n{}",
                    model_line(&self.agent),
                    saved_line(&self.agent),
                ));
                self.set_text(prompt);
            }
            Ok(None) => self.say(&format!(
                "Error: no message numbered {} (/fork lists them)",
                index + 1
            )),
            Err(error) => self.say(&format!("Error: {error}")),
        }
    }

    /// `/branch`, with the session's name, or `name` when given.
    pub(super) fn clone_session(&mut self, name: &str) {
        match self.agent.clone_session() {
            Ok(true) => {
                self.agent.load_model_card();
                self.replaced(&format!(
                    "✓ Cloned to new session · {}\n{}",
                    model_line(&self.agent),
                    saved_line(&self.agent),
                ));
                if !name.is_empty() {
                    self.name(name);
                }
            }
            Ok(false) => self.say("Nothing to clone yet"),
            Err(error) => self.say(&format!("Error: {error}")),
        }
    }

    pub(super) fn tree(&mut self, mut wanted: &str) {
        // The entry the tree shows again at after its summary question or summary is cancelled.
        let mut reopen = None;
        loop {
            let session_tree = match self.agent.tree() {
                Ok(tree) => tree,
                Err(error) => {
                    self.say(&format!("Error: {error}"));
                    return;
                }
            };
            let lines: Vec<usize> = session_tree.entries.iter().map(|e| e.line).collect();
            let filter = self.settings.tree_filter;
            let view = tree::View::new(session_tree, self.interactive(), filter);
            let rows = view.rows();
            let chosen = match wanted {
                "" if rows.is_empty() => {
                    self.say("No entries in session");
                    None
                }
                "" if self.interactive() => {
                    // As Scramjet's tree, half the terminal.
                    let visible = (editor::terminal_size().1 / 2).max(5);
                    let initial = reopen.or(view.current());
                    let picker = select::Picker::tree(view, visible, initial);
                    self.pick_entry(picker, &lines)
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
                        self.say("Error: /tree requires a positive entry number");
                        None
                    }
                    Ok(number) if number > rows.len() => {
                        self.say(&format!(
                            "Error: no entry numbered {number} (/tree lists them)"
                        ));
                        None
                    }
                    Ok(number) => Some(rows[number - 1].0),
                },
            };
            let Some(index) = chosen else {
                return;
            };
            let line = lines[index];
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
                    self.say(&format!("Error: {error}"));
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
                Err(error) => self.say(&format!("Error: {error}")),
            }
            return;
        }
    }

    /// Shows `/tree`'s list until an entry is chosen or it is cancelled, giving the entries
    /// Shift+L asks to label their label (`lines`: each one's line), as Scramjet's tree does.
    fn pick_entry(&mut self, mut picker: select::Picker, lines: &[usize]) -> Option<usize> {
        loop {
            let screen = self.screen.as_ref()?;
            let (entry, back) = match screen.pick_or_label(picker) {
                Ok(chosen) => return chosen,
                Err(labeling) => labeling,
            };
            picker = *back;
            let label = picker.tree_label(entry).unwrap_or_default().to_owned();
            let dialog = screen::Dialog::Text(
                "Label (empty to remove)".into(),
                label,
                screen::Optional::Yes,
            );
            let Some(screen::Answer::Text(Some(text))) = screen.ask(dialog, None) else {
                continue;
            };
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            match self.agent.set_label(lines[entry], Some(&text)) {
                Ok(label) => picker.relabel(entry, Some(label), None),
                Err(error) => {
                    let status = (format!("Failed to save the label: {error}"), true);
                    picker.relabel(entry, None, Some(status))
                }
            }
        }
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

/// How `/resume` orders the sessions; Ctrl+S goes to the next, as in Scramjet's session selector.
#[derive(Clone, Copy, PartialEq)]
enum Sort {
    /// Forks under the sessions they came from, newest first at each level; a query's matches
    /// best first.
    Threaded,
    /// Newest first, a query's matches too.
    Recent,
    /// Newest first; a query's matches best first.
    Relevance,
}

impl Sort {
    fn next(self) -> Self {
        match self {
            Self::Threaded => Self::Recent,
            Self::Recent => Self::Relevance,
            Self::Relevance => Self::Threaded,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Threaded => "threaded",
            Self::Recent => "recent",
            Self::Relevance => "relevance",
        }
    }
}

/// Whether the session has a name, as `/name` gives one.
fn named(session: &SessionInfo) -> bool {
    (session.name.as_deref()).is_some_and(|name| !name.trim().is_empty())
}

/// A `/resume` line: the session's name or first prompt, in `style` when given, cut to `width`
/// with its item count and age kept; with `path`, the session file's path after them, cut at its
/// start where the name or prompt would get fewer than 10 columns, as in Scramjet.
fn session_line(
    session: &SessionInfo,
    width: usize,
    style: Option<style::Style>,
    path: bool,
) -> String {
    let age = std::time::SystemTime::now()
        .duration_since(session.modified)
        .unwrap_or_default();
    let items = match session.item_count {
        1 => "1 item".to_owned(),
        count => format!("{count} items"),
    };
    let mut suffix = format!(" ({items}, {})", age_label(age));
    let label = session
        .name
        .as_deref()
        .or(session.first_prompt.as_deref())
        .unwrap_or("(no messages)");
    let prompt = one_line(label);
    if path {
        let path = shortened(&session.path.display().to_string());
        let room = width.saturating_sub(suffix.width() + 1 + prompt.width().min(10));
        let path = match path.width() <= room {
            true => path,
            false => format!("…{}", select::tail(&path, room.saturating_sub(1))),
        };
        suffix = format!("{suffix} {path}");
    }
    let prompt = clip(&prompt, width.saturating_sub(suffix.width()).max(2));
    match style {
        Some(style) => format!("{style}{prompt}{}{suffix}", style::RESET),
        None => format!("{prompt}{suffix}"),
    }
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
        assert_eq!(
            session_line(&session, 30, None, false),
            "Fix the parser… (12 items, 2h)"
        );
        assert_eq!(
            session_line(&session, 80, None, false),
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
        let line = session_line(&named, 80, Some(style::SESSION_NAMED), false);
        assert_eq!(
            line,
            format!(
                "{}Parser work{} (12 items, 2h)",
                style::SESSION_NAMED,
                style::RESET
            )
        );
        let empty = SessionInfo {
            first_prompt: None,
            item_count: 1,
            ..session
        };
        assert_eq!(
            session_line(&empty, 80, None, false),
            "(no messages) (1 item, 2h)"
        );
        // The path after them, its start cut where the prompt would get fewer than 10 columns.
        let filed = SessionInfo {
            path: PathBuf::from("/nonexistent/sessions/project/1793-1.jsonl"),
            first_prompt: named.first_prompt,
            text: String::new(),
            name: None,
            item_count: 12,
            modified: named.modified,
            parent: None,
        };
        assert_eq!(
            session_line(&filed, 100, None, true),
            "Fix the parser then run the tests (12 items, 2h) /nonexistent/sessions/project/1793-1.jsonl"
        );
        assert_eq!(
            session_line(&filed, 50, None, true),
            "Fix the p… (12 items, 2h) …ns/project/1793-1.jsonl"
        );
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
}
