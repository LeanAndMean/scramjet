//! The built-in commands that move to another session or entry: `/resume`, `/fork`,
//! `/branch`, `/clone` and `/tree`.
use super::drive;
use super::fuzzy::Haystack;
use super::interactive::Interactive;
use super::keymap::{self, Binding};
use super::screen;
use super::text::{clip, one_line};
use super::{editor, select, style, tree};
use scramjet::agent::{AgentError, Removal, SessionInfo, shortened};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

/// The keys' hint; with `chains`, the form chains are shown in, the keys that change it.
fn resume_keys(all: bool, chains: Option<Chains>) -> String {
    let scope = if all { "this folder" } else { "all folders" };
    let next = chains.map(|chains| format!("chains {}", chains.next().setting()));
    let fold = (chains == Some(Chains::Folded))
        .then(|| keymap::part(&[Binding::CursorLeft, Binding::CursorRight], "close/open"))
        .flatten();
    keymap::hint([
        keymap::part(&[Binding::Tab], scope),
        fold,
        next.and_then(|next| keymap::part(&[Binding::SessionToggleChains], &next)),
        keymap::part(&[Binding::SessionToggleSort], "sort"),
        keymap::part(&[Binding::SessionToggleNamedFilter], "named"),
        keymap::part(&[Binding::SessionTogglePath], "path"),
        keymap::part(&[Binding::SessionRename], "rename"),
        keymap::part(&[Binding::SessionDelete], "delete"),
    ])
}

impl Interactive {
    /// `/resume`: chooses a session to continue in, by its number with piped input. Returns
    /// whether it switched to one.
    pub(super) fn resume(&mut self, wanted: &str) -> bool {
        let mut sessions = match self.session.agent.sessions() {
            Ok(sessions) => listed(sessions, Sort::Threaded, Chains::Flat, &[]),
            Err(error) => {
                self.say(&format!("Error: {error}"));
                return false;
            }
        };
        let current =
            (sessions.iter()).position(|row| self.session.agent.is_session(&row.session.path));
        let chosen = match wanted {
            "" if sessions.is_empty() => {
                self.say("No sessions to resume");
                None
            }
            "" if self.interactive() => self.choose_session(),
            "" => {
                let width = editor::terminal_size().0;
                for (index, row) in sessions.iter().take(20).enumerate() {
                    let mark = if current == Some(index) { "*" } else { " " };
                    let number = format!("{}{mark} {}", index + 1, row.tree);
                    let width = width.saturating_sub(number.width());
                    let chain = row.chain.as_deref();
                    let line = session_line(&row.session, width, None, false, false, chain);
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
                Ok(number) => Some(sessions.swap_remove(number - 1).session),
            },
        };
        let Some(session) = chosen else {
            return false;
        };
        drive::resume(self, &session)
    }

    /// Chooses a session on the full screen, as Scramjet's session selector does: forks under
    /// the sessions they came from, a query matching them by their prompts and replies, Ctrl+S
    /// sorting them another way, Ctrl+N showing only the named ones, Ctrl+P their paths, Ctrl+R
    /// renaming the selected one and Ctrl+D deleting it, and Tab showing every project's
    /// sessions, each with its folder, or this one's again. Ctrl+X shows chains flat, threaded
    /// or folded, a folded chain opening with Right or Enter and closing with Left.
    fn choose_session(&mut self) -> Option<SessionInfo> {
        let mut spot = None;
        let (mut sort, mut named_only, mut paths) = (Sort::Threaded, false, false);
        let mut all = false;
        let mut chains = self.session.settings.resume_chains;
        // The first steps of the chains opened.
        let mut open: Vec<std::path::PathBuf> = Vec::new();
        // The row selected when a key changed how the list shows, to select again.
        let mut kept: Option<(std::path::PathBuf, bool)> = None;
        loop {
            let listed_sessions = match all {
                true => self.session.agent.all_sessions(),
                false => self.session.agent.sessions(),
            };
            let mut sessions = match listed_sessions {
                Ok(sessions) => {
                    let sessions = (sessions.into_iter())
                        .filter(|session| !named_only || named(session))
                        .collect();
                    listed(sessions, sort, chains, &open)
                }
                Err(error) => {
                    self.screen.as_ref()?.dismiss();
                    self.say(&format!("Error: {error}"));
                    return None;
                }
            };
            let screen = self.screen.as_ref()?;
            let none_anywhere = || match all {
                true => true,
                false => (self.session.agent.all_sessions()).map_or(true, |all| all.is_empty()),
            };
            if sessions.is_empty() && !named_only && none_anywhere() {
                screen.dismiss();
                self.say("No sessions to resume");
                return None;
            }
            let current = (sessions.iter())
                .position(|row| row.holds(|path| self.session.agent.is_session(path)));
            // Newest first, so the first other than the current one is the likely choice.
            let initial = usize::from(current == Some(0));
            let texts = searched(&sessions);
            let row = |index: usize, width: usize| {
                let row = &sessions[index];
                let style = match current == Some(index) {
                    true => Some(style::SESSION_CURRENT),
                    false => named(&row.session).then_some(style::SESSION_NAMED),
                };
                let chain = row.chain.as_deref();
                match &row.head {
                    Some(head) => {
                        let fold = if head.open { '⊟' } else { '⊞' };
                        let width = width.saturating_sub(2);
                        let line = session_line(&row.session, width, style, paths, all, chain);
                        format!("{fold} {line}")
                    }
                    None => session_line(&row.session, width, style, paths, all, chain),
                }
            };
            let chained = sessions.iter().any(|row| row.chain.is_some());
            let scope = if all { "all folders, " } else { "" };
            let shown = match chained {
                true => format!("{}, chains {}", sort.label(), chains.setting()),
                false => sort.label().to_owned(),
            };
            let title = match named_only {
                true => format!("Resume session ({scope}named, {shown})"),
                false => format!("Resume session ({scope}{shown})"),
            };
            let none = match keymap::key(Binding::SessionToggleNamedFilter) {
                _ if sessions.is_empty() && !named_only => match keymap::key(Binding::Tab) {
                    Some(key) => format!("No sessions in this folder ({key} shows all)"),
                    None => "No sessions in this folder".to_owned(),
                },
                _ if !named_only => "No matching sessions".to_owned(),
                Some(key) => format!("No named sessions ({key} shows all)"),
                None => "No named sessions".to_owned(),
            };
            let picker = select::Picker::search(
                &title,
                select::VISIBLE,
                texts,
                initial,
                current,
                &none,
                row,
            );
            let keys = resume_keys(all, chained.then_some(chains));
            let mut picker = (picker.noting(keys, Vec::new())).acting().held().tabbing();
            if sort == Sort::Threaded || chains != Chains::Flat {
                let tree = sessions.iter().map(|row| row.tree.clone()).collect();
                picker = picker.threaded(tree, row);
            }
            match sort {
                Sort::Recent => picker = picker.in_order(),
                Sort::Threaded | Sort::Relevance => {}
            }
            if chains == Chains::Folded {
                picker = picker.folding(sessions.iter().map(|row| row.fold).collect());
            }
            if let Some((at, status)) = spot.take() {
                picker = picker.at(at, status);
            }
            let keep = kept.take();
            if let Some(at) = (sessions.iter()).position(|row| Some(row.key()) == keep) {
                picker = picker.on_row(at);
            }
            let selected = |at: &select::Spot| -> Option<(std::path::PathBuf, bool)> {
                at.shown.get(at.position).map(|&row| sessions[row].key())
            };
            let (act, index, at) = match screen.pick_or_act(picker) {
                select::Picked::Row(chosen) => {
                    screen.dismiss();
                    return chosen.map(|index| sessions.swap_remove(index).session);
                }
                select::Picked::Act(act, index, at) => (act, index, at),
                select::Picked::Toggle(toggle, at) => {
                    kept = selected(&at);
                    let mut status = None;
                    match toggle {
                        select::Toggle::Sort => sort = sort.next(),
                        select::Toggle::Named => named_only = !named_only,
                        select::Toggle::Paths => paths = !paths,
                        select::Toggle::Chains => {
                            chains = chains.next();
                            if let Err(error) = self.session.settings.set_resume_chains(chains) {
                                status = Some((format!("Settings not saved: {error}"), true));
                            }
                        }
                    }
                    spot = Some((at, status));
                    continue;
                }
                select::Picked::Tab(at) => {
                    kept = selected(&at);
                    all = !all;
                    spot = Some((at, None));
                    continue;
                }
                // Not keys of this list (`Picker::tree`, `Picker::placing`).
                select::Picked::Label(_) | select::Picked::Placed(..) => {
                    screen.dismiss();
                    return None;
                }
            };
            let row = &sessions[index];
            let status = match act {
                select::Act::Fold => {
                    if let Some(head) = &row.head {
                        match head.open {
                            true => open.retain(|first| *first != head.first),
                            false => open.push(head.first.clone()),
                        }
                        kept = Some(row.key());
                    }
                    None
                }
                select::Act::Rename | select::Act::Delete if row.head.is_some() => {
                    let why = "Open the chain to rename or delete one of its sessions";
                    Some((why.to_owned(), true))
                }
                select::Act::Rename => {
                    let session = &row.session;
                    let name = session.name.clone().unwrap_or_default();
                    let dialog =
                        screen::Dialog::Text("Rename session".into(), name, screen::Optional::No);
                    match screen.ask(dialog, None) {
                        Some(screen::Answer::Text(Some(name))) => {
                            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
                            match self.session.agent.rename_session(&session.path, &name) {
                                Ok(()) => None,
                                Err(error) => Some((format!("Failed to rename: {error}"), true)),
                            }
                        }
                        _ => None,
                    }
                }
                select::Act::Delete => {
                    Some(match self.session.agent.delete_session(&row.session.path) {
                        Ok(Removal::Trashed) => ("Session moved to trash".to_owned(), false),
                        Ok(Removal::Deleted) => ("Session deleted".to_owned(), false),
                        Err(error) => (format!("Failed to delete: {error}"), true),
                    })
                }
                // Not keys of this list (`Picker::scoping`).
                _ => None,
            };
            spot = Some((at, status));
        }
    }

    pub(super) fn fork(&mut self, wanted: &str) {
        let prompts = match self.session.agent.fork_prompts() {
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
        drive::fork(self, index);
    }

    /// `/branch`, with the session's name, or `name` when given.
    pub(super) fn clone_session(&mut self, name: &str) {
        if drive::clone_session(self) && !name.is_empty() {
            self.name(name);
        }
    }

    pub(super) fn tree(&mut self, mut wanted: &str) {
        // The entry the tree shows again at after its summary question or summary is cancelled.
        let mut reopen = None;
        loop {
            let session_tree = match self.session.agent.tree() {
                Ok(tree) => tree,
                Err(error) => {
                    self.say(&format!("Error: {error}"));
                    return;
                }
            };
            let lines: Vec<usize> = session_tree.entries.iter().map(|e| e.line).collect();
            let filter = self.session.settings.tree_filter;
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
            let ask = self.interactive() && !self.session.settings.skip_branch_summary_prompt;
            let summary = match ask.then(|| self.session.agent.leaves_branch(line)) {
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
                    Some(screen) => {
                        self.session
                            .agent
                            .navigate(line, None, &mut screen.link, &self.cancel)
                    }
                    None => self
                        .session
                        .agent
                        .navigate(line, None, &mut self.ui, &self.cancel),
                },
            };
            match result {
                Ok(navigated) => drive::navigated(self, navigated),
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
            match self.session.agent.set_label(lines[entry], Some(&text)) {
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

/// A `/resume` line: the session's name or first prompt (for a step of a `chain`, its latest
/// `completed` summary, then its prompt), in `style` when given, cut to `width` with its
/// `chain`, item count and age kept; with `folder`, its project's folder after them, and with
/// `path`, the session file's path after those, cut at its start where the name or prompt would
/// get fewer than 10 columns, as in Scramjet.
fn session_line(
    session: &SessionInfo,
    width: usize,
    style: Option<style::Style>,
    path: bool,
    folder: bool,
    chain: Option<&str>,
) -> String {
    let age = std::time::SystemTime::now()
        .duration_since(session.modified)
        .unwrap_or_default();
    let items = match session.item_count {
        1 => "1 item".to_owned(),
        count => format!("{count} items"),
    };
    let chain = chain.map_or_else(String::new, |chain| format!("{chain}, "));
    let mut suffix = format!(" ({chain}{items}, {})", age_label(age));
    if folder && let Some(root) = &session.root {
        suffix = format!("{suffix} {}", shortened(&root.display().to_string()));
    }
    let label = session
        .name
        .as_deref()
        .or(session.summary.as_deref().filter(|_| !chain.is_empty()))
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

/// How `/resume` shows the sessions next steps chained (`SessionInfo::continues`); Ctrl+X goes
/// to the next, and the last one chosen is saved (`resumeChains`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Chains {
    /// Each step a row of its own, where its order puts it.
    Flat,
    /// Each step under the one before, as forks are threaded.
    Threaded,
    /// One row per chain, which opens into its steps one level deep.
    Folded,
}

impl Chains {
    pub(super) const CYCLE: [Self; 3] = [Self::Flat, Self::Threaded, Self::Folded];

    fn next(self) -> Self {
        let at = Self::CYCLE.iter().position(|&chains| chains == self);
        Self::CYCLE[at.map_or(0, |at| (at + 1) % Self::CYCLE.len())]
    }

    /// Its name in the settings, and in the list's title.
    pub(super) fn setting(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Threaded => "threaded",
            Self::Folded => "folded",
        }
    }
}

/// A row of `/resume`'s list.
struct Row {
    /// The session it resumes; for a chain's row (`steps`), the chain's newest step.
    session: SessionInfo,
    /// The tree lines before it when the list shows them.
    tree: String,
    /// `chain N/M` for a step of a chain, `chain of M` for a chain's row.
    chain: Option<String>,
    head: Option<Head>,
    fold: select::Fold,
}

/// A chain's row.
struct Head {
    /// Its first step's file, which names the chain.
    first: std::path::PathBuf,
    /// Its steps' files, newest first.
    steps: Vec<std::path::PathBuf>,
    open: bool,
    /// What a query finds it by: every step's.
    text: String,
}

impl Row {
    /// What selects the row again once the list is shown another way: a chain's row by its
    /// first step, `true` telling it from that step's own row.
    fn key(&self) -> (std::path::PathBuf, bool) {
        match &self.head {
            Some(head) => (head.first.clone(), true),
            None => (self.session.path.clone(), false),
        }
    }

    fn holds(&self, is_current: impl Fn(&std::path::Path) -> bool) -> bool {
        match &self.head {
            Some(head) => !head.open && head.steps.iter().any(|step| is_current(step)),
            None => is_current(&self.session.path),
        }
    }
}

/// `sessions` (newest first) as `/resume` lists them: with `sort` `Threaded`, every fork under
/// the session it came from, newest first at each level, as Scramjet's threaded list; chains
/// in their `chains` form under every order, a threaded chain where its newest step is, the
/// chains whose first steps are in `open` opened.
fn listed(
    sessions: Vec<SessionInfo>,
    sort: Sort,
    chains: Chains,
    open: &[std::path::PathBuf],
) -> Vec<Row> {
    let key = |path: &std::path::Path| std::fs::canonicalize(path).unwrap_or(path.to_owned());
    let keys: Vec<_> = sessions.iter().map(|session| key(&session.path)).collect();
    let find = |path: Option<&std::path::Path>, at: usize| {
        let path = key(path?);
        (keys.iter().position(|key| *key == path)).filter(|&found| found != at)
    };
    let forks: Vec<Option<usize>> = (sessions.iter().enumerate())
        .map(|(at, session)| find(session.parent.as_deref(), at))
        .collect();
    let before: Vec<Option<usize>> = (sessions.iter().enumerate())
        .map(|(at, session)| find(session.continues.as_deref(), at))
        .collect();
    // Each session's chain, by its first listed step, and its place in it; a cycle of links
    // ends where it comes round.
    let mut first = vec![0; sessions.len()];
    let mut place = vec![1; sessions.len()];
    for at in 0..sessions.len() {
        let mut step = at;
        while let Some(earlier) = before[step].filter(|_| place[at] <= sessions.len()) {
            (step, place[at]) = (earlier, place[at] + 1);
        }
        first[at] = step;
    }
    let mut length = vec![0; sessions.len()];
    let mut size = vec![0; sessions.len()];
    for at in 0..sessions.len() {
        length[first[at]] = length[first[at]].max(place[at]);
        size[first[at]] += 1;
    }
    let chained = |at: usize| size[first[at]] > 1;
    let steps = |chain: usize| -> Vec<usize> {
        (0..sessions.len())
            .filter(|&at| chained(at) && first[at] == chain)
            .collect()
    };
    // The rows: a session, or a chain (by its first step).
    #[derive(Clone, Copy, PartialEq)]
    enum Node {
        Session(usize),
        Chain(usize),
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut row_of = vec![None; sessions.len()];
    let mut chain_row = vec![None; sessions.len()];
    for at in 0..sessions.len() {
        if chains != Chains::Folded || !chained(at) {
            row_of[at] = Some(nodes.len());
            nodes.push(Node::Session(at));
        } else if chain_row[first[at]].is_none() {
            chain_row[first[at]] = Some(nodes.len());
            nodes.push(Node::Chain(first[at]));
            if open.contains(&sessions[first[at]].path) {
                for step in steps(first[at]) {
                    row_of[step] = Some(nodes.len());
                    nodes.push(Node::Session(step));
                }
            }
        }
    }
    let row_for = |at: usize| row_of[at].or(chain_row[first[at]]);
    let threads = sort == Sort::Threaded;
    let parents: Vec<Option<usize>> = (nodes.iter())
        .map(|&node| match node {
            Node::Session(at) if chains == Chains::Folded && chained(at) => chain_row[first[at]],
            Node::Session(at) if chains == Chains::Threaded && before[at].is_some() => {
                before[at].and_then(row_for)
            }
            Node::Session(at) | Node::Chain(at) => (forks[at].filter(|_| threads))
                .filter(|&fork| !chained(at) || !chained(fork) || first[fork] != first[at])
                .and_then(row_for),
        })
        .collect();
    // A threaded chain sits where its newest step is.
    let ranks: Vec<usize> = (nodes.iter().enumerate())
        .map(|(row, &node)| match node {
            Node::Session(at) if chains == Chains::Threaded && chained(at) => {
                row_for(steps(first[at])[0]).unwrap_or(row)
            }
            _ => row,
        })
        .collect();
    let order = thread(&parents, &ranks);
    let mut shown = vec![0; nodes.len()];
    for (position, &(row, _)) in order.iter().enumerate() {
        shown[row] = position;
    }
    // Each chain's newest step and its steps' files, by its first step.
    let mut heads: Vec<Option<(SessionInfo, Head)>> = (0..sessions.len())
        .map(|at| {
            let steps = steps(at);
            let paths = steps.iter().map(|&step| sessions[step].path.clone());
            let texts = steps.iter().map(|&step| search_text(&sessions[step]));
            let path = sessions[at].path.clone();
            (chained(at) && first[at] == at).then(|| {
                let head = Head {
                    open: open.contains(&path),
                    first: path,
                    steps: paths.collect(),
                    text: texts.collect::<Vec<_>>().join(" "),
                };
                (sessions[steps[0]].clone(), head)
            })
        })
        .collect();
    let mut sessions: Vec<Option<SessionInfo>> = sessions.into_iter().map(Some).collect();
    (order.into_iter())
        .filter_map(|(row, tree)| match nodes[row] {
            Node::Session(at) => Some(Row {
                session: sessions[at].take()?,
                tree,
                chain: chained(at).then(|| format!("chain {}/{}", place[at], length[first[at]])),
                head: None,
                fold: match parents[row] {
                    Some(head) if chains == Chains::Folded && chained(at) => {
                        select::Fold::In(shown[head])
                    }
                    _ => select::Fold::None,
                },
            }),
            Node::Chain(chain) => {
                let (newest, head) = heads[chain].take()?;
                Some(Row {
                    session: newest,
                    tree,
                    chain: Some(format!("chain of {}", length[chain])),
                    fold: match head.open {
                        true => select::Fold::Open,
                        false => select::Fold::Closed,
                    },
                    head: Some(head),
                })
            }
        })
        .collect()
}

/// The order of rows whose `parents` are given, each with the tree lines before it: every row
/// under its parent, in their order at each level, the top level by `ranks`.
fn thread(parents: &[Option<usize>], ranks: &[usize]) -> Vec<(usize, String)> {
    let children = |parent: Option<usize>| -> Vec<usize> {
        (0..parents.len())
            .filter(|&at| parents[at] == parent)
            .collect()
    };
    // Each row's index and its tree lines; a walk from the roots, then from what a cycle of
    // parents kept from them.
    let mut order: Vec<(usize, String)> = Vec::new();
    let mut seen = vec![false; parents.len()];
    let mut stack: Vec<(usize, Vec<bool>, bool)> = Vec::new();
    let mut roots = children(None);
    roots.sort_by_key(|&at| (ranks[at], at));
    let unreached = (0..parents.len()).filter(|&at| parents[at].is_some());
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
    order
}

/// What `/resume` searches in each session, those matching equally the newest first, as in
/// Scramjet.
fn searched(rows: &[Row]) -> Vec<Haystack> {
    let mut newest: Vec<usize> = (0..rows.len()).collect();
    newest.sort_by_key(|&at| std::cmp::Reverse(rows[at].session.modified));
    let mut rank = vec![0; rows.len()];
    for (order, at) in newest.into_iter().enumerate() {
        rank[at] = order;
    }
    (rows.iter().zip(rank))
        .map(|(row, rank)| match &row.head {
            Some(head) => Haystack::ranked(&head.text, rank),
            None => Haystack::ranked(&search_text(&row.session), rank),
        })
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
            summary: None,
            modified: std::time::SystemTime::now() - Duration::from_secs(7200),
            parent: None,
            continues: None,
            root: None,
        };
        assert_eq!(
            session_line(&session, 30, None, false, false, None),
            "Fix the parser… (12 items, 2h)"
        );
        assert_eq!(
            session_line(&session, 80, None, false, false, None),
            "Fix the parser then run the tests (12 items, 2h)"
        );
        let named = SessionInfo {
            path: PathBuf::new(),
            first_prompt: session.first_prompt.clone(),
            text: String::new(),
            name: Some("Parser\nwork".into()),
            item_count: 12,
            summary: None,
            modified: session.modified,
            parent: None,
            continues: None,
            root: None,
        };
        let line = session_line(&named, 80, Some(style::SESSION_NAMED), false, false, None);
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
            session_line(&empty, 80, None, false, false, None),
            "(no messages) (1 item, 2h)"
        );
        // The path after them, its start cut where the prompt would get fewer than 10 columns.
        let filed = SessionInfo {
            path: PathBuf::from("/nonexistent/sessions/project/1793-1.jsonl"),
            first_prompt: named.first_prompt,
            text: String::new(),
            name: None,
            item_count: 12,
            summary: None,
            modified: named.modified,
            parent: None,
            continues: None,
            root: None,
        };
        assert_eq!(
            session_line(&filed, 100, None, true, false, None),
            "Fix the parser then run the tests (12 items, 2h) /nonexistent/sessions/project/1793-1.jsonl"
        );
        assert_eq!(
            session_line(&filed, 50, None, true, false, None),
            "Fix the p… (12 items, 2h) …ns/project/1793-1.jsonl"
        );
        // Every folder's list names each session's project before the path.
        let elsewhere = SessionInfo {
            root: Some(PathBuf::from("/work/other")),
            ..filed
        };
        assert_eq!(
            session_line(&elsewhere, 100, None, false, true, None),
            "Fix the parser then run the tests (12 items, 2h) /work/other"
        );
        assert_eq!(
            session_line(&elsewhere, 60, None, true, true, None),
            "Fix the p… (12 items, 2h) /work/other …/project/1793-1.jsonl"
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
            summary: None,
            modified: std::time::SystemTime::now(),
            parent: parent.map(path),
            continues: None,
            root: None,
        });
        let rows: Vec<String> = (listed(sessions.into(), Sort::Threaded, Chains::Flat, &[])
            .into_iter())
        .map(|row| format!("{}{}", row.tree, row.session.name.unwrap()))
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
            summary: None,
            modified: std::time::SystemTime::now(),
            parent: parent.map(path),
            continues: None,
            root: None,
        });
        let rows: Vec<String> = (listed(deep.into(), Sort::Threaded, Chains::Flat, &[])
            .into_iter())
        .map(|row| format!("{}{}", row.tree, row.session.name.unwrap()))
        .collect();
        assert_eq!(rows, ["R", "   ├─ P", "   │  └─ K", "   └─ Q"]);
    }

    #[test]
    fn chains_show_flat_threaded_or_folded_each_step_by_its_summary() {
        let path = |name: &str| PathBuf::from(format!("/nonexistent/{name}.jsonl"));
        // Newest first: C1, C2 and C3 are a chain, F a fork of C1; C1 never completed.
        let sessions = || {
            [
                ("C3", None, Some("C2")),
                ("X", None, None),
                ("C2", None, Some("C1")),
                ("F", Some("C1"), None),
                ("C1", None, None),
                ("Y", None, None),
            ]
            .map(
                |(name, parent, continues): (&str, Option<&str>, Option<&str>)| {
                    let chained = name.starts_with('C');
                    SessionInfo {
                        path: path(name),
                        first_prompt: Some(if chained { "/rs:loop" } else { name }.to_owned()),
                        text: name.to_owned(),
                        name: None,
                        item_count: 1,
                        summary: (chained && name != "C1").then(|| format!("{name} done")),
                        modified: std::time::SystemTime::now(),
                        parent: parent.map(path),
                        continues: continues.map(path),
                        root: None,
                    }
                },
            )
            .to_vec()
        };
        let rows = |sort, chains, open: &[PathBuf]| -> Vec<(String, select::Fold)> {
            (listed(sessions(), sort, chains, open).into_iter())
                .map(|row| {
                    let line =
                        session_line(&row.session, 80, None, false, false, row.chain.as_deref());
                    let line = line
                        .replace(", 1 item, now)", ")")
                        .replace(" (1 item, now)", "");
                    (format!("{}{line}", row.tree), row.fold)
                })
                .collect()
        };
        let lines = |rows: Vec<(String, select::Fold)>| -> Vec<String> {
            rows.into_iter().map(|(line, _)| line).collect()
        };
        assert_eq!(
            lines(rows(Sort::Threaded, Chains::Flat, &[])),
            [
                "C3 done (chain 3/3)",
                "X",
                "C2 done (chain 2/3)",
                "/rs:loop (chain 1/3)",
                "   └─ F",
                "Y"
            ]
        );
        // A threaded chain is where its newest step is.
        assert_eq!(
            lines(rows(Sort::Threaded, Chains::Threaded, &[])),
            [
                "/rs:loop (chain 1/3)",
                "   ├─ C2 done (chain 2/3)",
                "   │  └─ C3 done (chain 3/3)",
                "   └─ F",
                "X",
                "Y"
            ]
        );
        // Folded: the chain's row is its newest step, the forks of its steps under it.
        use select::Fold;
        assert_eq!(
            rows(Sort::Threaded, Chains::Folded, &[]),
            [
                ("C3 done (chain of 3)".to_owned(), Fold::Closed),
                ("   └─ F".to_owned(), Fold::None),
                ("X".to_owned(), Fold::None),
                ("Y".to_owned(), Fold::None),
            ]
        );
        assert_eq!(
            rows(Sort::Threaded, Chains::Folded, &[path("C1")]),
            [
                ("C3 done (chain of 3)".to_owned(), Fold::Open),
                ("   ├─ C3 done (chain 3/3)".to_owned(), Fold::In(0)),
                ("   ├─ C2 done (chain 2/3)".to_owned(), Fold::In(0)),
                ("   └─ /rs:loop (chain 1/3)".to_owned(), Fold::In(0)),
                ("      └─ F".to_owned(), Fold::None),
                ("X".to_owned(), Fold::None),
                ("Y".to_owned(), Fold::None),
            ]
        );
        // Other orders keep only the chains' lines.
        assert_eq!(
            lines(rows(Sort::Recent, Chains::Folded, &[path("C1")])),
            [
                "C3 done (chain of 3)",
                "   ├─ C3 done (chain 3/3)",
                "   ├─ C2 done (chain 2/3)",
                "   └─ /rs:loop (chain 1/3)",
                "X",
                "F",
                "Y"
            ]
        );
        assert_eq!(
            lines(rows(Sort::Recent, Chains::Flat, &[])),
            [
                "C3 done (chain 3/3)",
                "X",
                "C2 done (chain 2/3)",
                "F",
                "/rs:loop (chain 1/3)",
                "Y"
            ]
        );
        // A chain's row is found by any step's text, and holds the current session when closed.
        let folded = listed(sessions(), Sort::Threaded, Chains::Folded, &[]);
        assert!(folded[0].holds(|path| path.ends_with("C1.jsonl")));
        assert_eq!(folded[0].key(), (path("C1"), true));
        let texts = searched(&folded);
        use crate::agent_cli::fuzzy::{filter, parse};
        assert_eq!(filter(&parse("C2"), &texts), [0]);
        let opened = listed(sessions(), Sort::Threaded, Chains::Folded, &[path("C1")]);
        assert!(!opened[0].holds(|path| path.ends_with("C1.jsonl")));
        assert!(opened[3].holds(|path| path.ends_with("C1.jsonl")));
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
            summary: None,
            modified: now - Duration::from_secs(age),
            parent: parent.map(|parent| PathBuf::from(format!("/nonexistent/{parent}.jsonl"))),
            continues: None,
            root: None,
        };
        // The fork, the newest, is threaded under its parent but found before it.
        let sessions = listed(
            vec![
                session("aaaa", None, 10),
                session("bbbb", None, 20),
                session("cccc", Some("bbbb"), 5),
            ],
            Sort::Threaded,
            Chains::Flat,
            &[],
        );
        let ids = |rows: Vec<usize>| -> Vec<String> {
            (rows.into_iter())
                .map(|at| {
                    sessions[at]
                        .session
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
