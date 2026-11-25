//! `/sandbox` (`/permissions`) on the full screen: what the agent and its subagents can change
//! and read, and whether their commands reach the network, each row changed at once with Enter.
use super::drive::SandboxChange;
use super::interactive::Interactive;
use super::keymap::Binding;
use super::screen::{Answer, Dialog, Optional};
use super::select::{self, Act, Picked};
use super::settings::PathList;
use super::settings::Settings;
use super::text::clip;
use scramjet::agent::{Agent, Reach, Remembered, SandboxMode, shortened};
use std::path::{Path, PathBuf};

/// The widest label, `Also writable`.
const LABEL: usize = 13;

const REACHES: [Reach; 3] = [Reach::Everything, Reach::Project, Reach::Minimal];

/// What the agent can read, as the Can read row says it.
pub(super) fn reach_name(reach: Reach) -> &'static str {
    match reach {
        Reach::Everything => "everything you can",
        Reach::Project => "this project and your tools",
        Reach::Minimal => "this project and the system only",
    }
}

fn reach_note(reach: Reach) -> &'static str {
    match reach {
        Reach::Everything => "All you can read but what Hidden lists and the credential stores.",
        Reach::Project => {
            "The project and the writable directories, the programs on PATH and git's \
             configuration; the rest of your home and the mounted drives hidden."
        }
        Reach::Minimal => {
            "The project and the writable directories, and the system's own directories (/usr, \
             /etc, /opt…); Also readable opens more."
        }
    }
}

/// What the agent can change, as the Can change row says it.
fn change_name(mode: Option<SandboxMode>) -> &'static str {
    match mode {
        Some(SandboxMode::ReadOnly) => "nothing",
        Some(SandboxMode::WorkspaceWrite) => "this project",
        None => "anything, no sandbox",
    }
}

const CHANGES: [(Option<SandboxMode>, &str); 3] = [
    (
        Some(SandboxMode::ReadOnly),
        "Commands read but change no file, and write and edit are refused.",
    ),
    (
        Some(SandboxMode::WorkspaceWrite),
        "The project and Also writable's directories, but their .git, .scramjet and .agents.",
    ),
    (
        None,
        "The agent's tools run with your access, the credential stores hidden as /settings says.",
    ),
];

/// `/settings`' Sandbox row: `can change the project · reads everything · network on`.
pub(super) fn summary(mode: Option<SandboxMode>, network: bool, reach: Reach) -> String {
    let change = match mode {
        Some(SandboxMode::ReadOnly) => "changes nothing",
        Some(SandboxMode::WorkspaceWrite) => "can change the project",
        None => "can change anything",
    };
    let network = match network || (mode.is_none() && cfg!(windows)) {
        true => "network on",
        false => "network off",
    };
    format!("{change} · {} · {network}", reads(Some(reach)))
}

fn reads(reach: Option<Reach>) -> &'static str {
    match reach {
        None => "reads as the main agent",
        Some(Reach::Everything) => "reads everything",
        Some(Reach::Project) => "reads the project and your tools",
        Some(Reach::Minimal) => "reads the project and the system",
    }
}

/// Where a writable directory of the main agent's comes from.
#[derive(Clone, Copy)]
pub(super) enum Grant {
    Session,
    /// Granted always in this project (`sandbox-grants.json`).
    Project,
    /// `allowWrite`.
    Default,
}

enum Entry {
    Change,
    Read,
    Network,
    Path(PathList, PathBuf),
    Add(PathList),
    /// Subagents' list is the main agent's.
    Same(PathList),
    /// Makes subagents' list the main agent's again.
    Back(PathList),
    /// A path of the default the project took out of the list.
    Restore(PathList, PathBuf),
    Subagents,
}

struct Row {
    label: &'static str,
    value: String,
    note: String,
    entry: Entry,
}

fn row(
    label: &'static str,
    value: impl Into<String>,
    note: impl Into<String>,
    entry: Entry,
) -> Row {
    let (value, note) = (value.into(), note.into());
    Row {
        label,
        value,
        note,
        entry,
    }
}

fn label(list: PathList) -> &'static str {
    match list {
        PathList::Writable => "Also writable",
        PathList::Hidden => "Hidden",
        PathList::Readable => "Also readable",
    }
}

/// The main agent's writable directories as the screen lists them, each with where it comes
/// from; `None` where everything is writable (no sandbox, and a reach that hides nothing).
pub(super) fn grants(agent: &Agent, settings: &Settings) -> Option<Vec<(PathBuf, Grant)>> {
    let writable = match agent.sandbox() {
        Some(sandbox) => &sandbox.writable[..],
        None if settings.reach().kept() => agent.writable(),
        None => return None,
    };
    let always =
        (Remembered::user().and_then(|kept| kept.of(agent.root()).ok())).unwrap_or_default();
    let real = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
        (paths.iter())
            .filter_map(|path| std::fs::canonicalize(path).ok())
            .collect()
    };
    let from_settings = real(settings.allow_write());
    let from_default = (settings.default_listed(false, PathList::Writable))
        .map_or_else(|| from_settings.clone(), real);
    let grant = |path: &PathBuf| match from_settings.contains(path) {
        _ if always.contains(path) => Grant::Project,
        true if from_default.contains(path) => Grant::Default,
        true => Grant::Project,
        false => Grant::Session,
    };
    let mut granted: Vec<(PathBuf, Grant)> = (writable.iter())
        .map(|path| (path.clone(), grant(path)))
        .collect();
    let others = always.iter().filter(|path| !writable.contains(path));
    granted.extend(others.map(|path| (path.clone(), Grant::Project)));
    Some(granted)
}

/// A path of a list, as the screen shows it.
pub(super) struct Listed {
    pub(super) path: PathBuf,
    /// Added in this project, or for `taken_out` taken out in it.
    pub(super) set_here: bool,
    /// In every project's default, taken out in this one.
    pub(super) taken_out: bool,
}

/// `list`'s paths, then those of the default the project took out; `None` where subagents
/// take the main agent's list.
pub(super) fn listed(settings: &Settings, subagents: bool, list: PathList) -> Option<Vec<Listed>> {
    let paths = settings.listed(subagents, list)?;
    let default_own = settings.default_listed(subagents, list).is_some();
    let own_here = subagents && settings.sandbox_default().is_some() && !default_own;
    let mut listed: Vec<Listed> = (paths.into_iter())
        .map(|path| Listed {
            set_here: !own_here && settings.added_here(subagents, list, &path),
            path,
            taken_out: false,
        })
        .collect();
    if !own_here {
        listed.extend(taken_out(settings, subagents, list));
    }
    Some(listed)
}

fn taken_out(settings: &Settings, subagents: bool, list: PathList) -> impl Iterator<Item = Listed> {
    (settings.removed_here(subagents, list).into_iter()).map(|path| Listed {
        path,
        set_here: true,
        taken_out: true,
    })
}

fn shown(path: &Path) -> String {
    shortened(&path.to_string_lossy())
}

/// `value`, and `note`, saying when this project sets the row.
fn here(value: &str, note: String, set: bool) -> (String, String) {
    match set {
        true => (
            format!("{value} (set here)"),
            format!("{note} Set for this project."),
        ),
        false => (value.to_owned(), note),
    }
}

impl Interactive {
    /// `/sandbox` without an argument: the screen on the full screen, else what `/sandbox` says.
    pub(super) fn sandbox_menu(&mut self) {
        if self.screen.is_none() {
            return self.sandbox("");
        }
        self.sandbox_screen(false);
        if let Some(screen) = &self.screen {
            screen.dismiss();
        }
    }

    /// The main agent's rows, or with `subagents` theirs, until Escape.
    pub(super) fn sandbox_screen(&mut self, subagents: bool) {
        let mut at = 0;
        loop {
            let rows = match subagents {
                false => self.main_rows(),
                true => self.subagent_rows(),
            };
            let Some(screen) = &self.screen else {
                return;
            };
            screen.footer(super::footer::Shown::of(&self.session.agent));
            let texts: Vec<String> = (rows.iter().enumerate())
                .map(|(index, row)| {
                    let first = index == 0 || rows[index - 1].label != row.label;
                    let label = if first { row.label } else { "" };
                    format!("{label:LABEL$}  {}", row.value)
                })
                .collect();
            let title = if subagents {
                "Subagents' sandbox"
            } else {
                "Sandbox"
            };
            let picker = select::Picker::select(
                title,
                select::VISIBLE.max(texts.len()),
                texts.len(),
                at.min(texts.len() - 1),
                None,
                |index, width| clip(&texts[index], width),
            );
            let mut keys = vec![(Binding::SandboxSave, "save for every project")];
            if self.project_sets() {
                keys.push((Binding::SandboxReset, "reset this project"));
            }
            let hint = select::hint("change", &keys, if subagents { "back" } else { "close" });
            let notes = rows.iter().map(|row| row.note.clone()).collect();
            let picker = (picker.noting(hint, notes))
                .keyed(vec![
                    (Binding::SandboxSave, Act::Save),
                    (Binding::SandboxReset, Act::Reset),
                ])
                .held();
            let index = match screen.pick_or_act(picker) {
                Picked::Row(Some(index)) => index,
                Picked::Act(Act::Save, index, _) => {
                    at = index;
                    self.change_sandbox(SandboxChange::Save);
                    continue;
                }
                Picked::Act(Act::Reset, index, _) => {
                    at = index;
                    self.reset_sandbox();
                    continue;
                }
                _ => return,
            };
            at = index;
            let row = &rows[index];
            match &row.entry {
                Entry::Change if subagents => self.choose_subagent_change(),
                Entry::Change => self.choose_change(),
                Entry::Read => self.choose_reach(subagents),
                Entry::Network if subagents => {
                    let (mode, network, reach) = self.session.settings.subagent_policy();
                    self.change_sandbox(SandboxChange::Subagents(mode, !network, reach));
                }
                Entry::Network => match self.session.agent.sandbox().map(|s| s.network) {
                    Some(network) => self.change_sandbox(SandboxChange::Network(!network)),
                    None if cfg!(windows) => {
                        self.say("Nothing confines commands on Windows: they reach the network")
                    }
                    None => {
                        let offline = self.session.agent.offline();
                        self.change_sandbox(SandboxChange::Network(offline))
                    }
                },
                Entry::Path(PathList::Writable, path) if !subagents => {
                    self.remove_dir(&path.display().to_string())
                }
                Entry::Path(list, path) => self.change_sandbox(SandboxChange::Listed {
                    subagents,
                    list: *list,
                    path: path.display().to_string(),
                    add: false,
                }),
                Entry::Add(list) => {
                    let title = match (list, subagents) {
                        (PathList::Writable, false) => "Directory the agent may change",
                        (PathList::Writable, true) => "Directory subagents may change",
                        (PathList::Hidden, _) => "Path to hide",
                        (PathList::Readable, _) => "Path to make readable",
                    };
                    let dialog = Dialog::Text(title.to_owned(), String::new(), Optional::No);
                    let Some(Answer::Text(Some(typed))) = screen.ask(dialog, None) else {
                        continue;
                    };
                    let typed = typed.trim().to_owned();
                    match (list, subagents) {
                        (PathList::Writable, false) => {
                            self.change_sandbox(SandboxChange::AddHere(typed))
                        }
                        (list, _) => self.change_sandbox(SandboxChange::Listed {
                            subagents,
                            list: *list,
                            path: typed,
                            add: true,
                        }),
                    }
                }
                Entry::Same(list) => self.change_sandbox(SandboxChange::OwnList(*list, true)),
                Entry::Back(list) => self.change_sandbox(SandboxChange::OwnList(*list, false)),
                Entry::Restore(PathList::Writable, path) if !subagents => {
                    self.change_sandbox(SandboxChange::AddHere(path.display().to_string()))
                }
                Entry::Restore(list, path) => self.change_sandbox(SandboxChange::Listed {
                    subagents,
                    list: *list,
                    path: path.display().to_string(),
                    add: true,
                }),
                Entry::Subagents => {
                    self.sandbox_screen(true);
                    at = index;
                }
            }
        }
    }

    fn main_rows(&self) -> Vec<Row> {
        let agent = &self.session.agent;
        let settings = &self.session.settings;
        let sandbox = agent.sandbox();
        let mode = sandbox.map(|sandbox| sandbox.mode);
        let reach = settings.reach();
        let now = settings.rows();
        let default = settings.sandbox_default().unwrap_or(&now);
        let mut rows = Vec::new();
        let change = match cfg!(windows) {
            true => "anything (Windows has no sandbox)",
            false => change_name(mode),
        };
        let note = match mode {
            None => "No sandbox: the agent's tools run with your access. Enter chooses.",
            Some(_) => {
                "What commands, write and edit may change; .git, .scramjet and .agents stay \
                 read-only. Enter chooses."
            }
        };
        let (change, note) = here(change, note.to_owned(), now.mode != default.mode);
        rows.push(row("Can change", change, note, Entry::Change));
        let mut read = reach_name(reach).to_owned();
        let mut note = format!("{} Enter chooses.", reach_note(reach));
        if reach.kept() && cfg!(windows) {
            read += " (read, grep and find only)";
            note = "Windows cannot hide files from commands: only read, grep and find keep to it."
                .to_owned();
        }
        let (read, note) = here(&read, note, now.reach != default.reach);
        rows.push(row("Can read", read, note, Entry::Read));
        let network = match sandbox.map(|sandbox| sandbox.network) {
            _ if cfg!(windows) && sandbox.is_none() => None,
            Some(network) => Some(network),
            None => Some(!agent.offline()),
        };
        let (network, note) = match network {
            Some(true) => (
                "on",
                "Commands reach the internet, the local network and this machine's servers.",
            ),
            Some(false) => (
                "off",
                "Commands reach neither the internet, the local network nor this machine's servers.",
            ),
            None => (
                "on (nothing confines it on Windows)",
                "Commands reach the network as you can.",
            ),
        };
        let set = cfg!(unix) && now.network != default.network;
        let (network, note) = here(network, note.to_owned(), set);
        rows.push(row("Network", network, note, Entry::Network));
        if let Some(granted) = grants(agent, settings) {
            // What the reach hides is empty and unchangeable to commands, so hidden it is gone.
            let back = match sandbox {
                Some(_) => "read-only",
                None => "hidden",
            };
            for (path, grant) in granted {
                let (mark, note) = match grant {
                    Grant::Session => (
                        "",
                        format!("For this session. Enter makes it {back} again."),
                    ),
                    Grant::Project => (
                        " (set here)",
                        format!("Writable in this project. Enter makes it {back} and forgets it."),
                    ),
                    Grant::Default => (
                        " (every project)",
                        format!("In every project's default. Enter makes it {back} in this one."),
                    ),
                };
                let value = format!("{}{mark}", shown(&path));
                let entry = Entry::Path(PathList::Writable, path);
                rows.push(row(label(PathList::Writable), value, note, entry));
            }
            self.removed_rows(&mut rows, false, PathList::Writable);
            let note = "Another directory the agent may change in this project.";
            rows.push(row(
                label(PathList::Writable),
                "Add…",
                note,
                Entry::Add(PathList::Writable),
            ));
        }
        for list in [PathList::Hidden, PathList::Readable] {
            self.list_rows(&mut rows, false, list);
        }
        let (mode, network, reach) = settings.subagent_policy();
        let change = match mode {
            SandboxMode::ReadOnly => "change nothing",
            SandboxMode::WorkspaceWrite => "can change where they work",
        };
        let network = if network { "network on" } else { "network off" };
        let value = format!("{change} · {} · {network}", reads(reach));
        let note = "What subagents can change and read, and their network. Enter opens them.";
        let theirs = |rows: &super::settings::Rows| {
            (
                rows.subagent_mode,
                rows.subagent_network,
                rows.subagent_reach,
            )
        };
        let set = theirs(&now) != theirs(default)
            || settings.sandbox_default().is_some()
                && [PathList::Writable, PathList::Hidden, PathList::Readable]
                    .into_iter()
                    .any(|list| settings.listed(true, list) != settings.default_listed(true, list));
        let (value, note) = here(&value, note.to_owned(), set);
        rows.push(row("Subagents", value, note, Entry::Subagents));
        rows
    }

    /// Whether the project sets any of the sandbox's rows, or grants a directory always.
    fn project_sets(&self) -> bool {
        let root = self.session.agent.root();
        let always = Remembered::user().and_then(|kept| kept.of(root).ok());
        self.session.settings.set_here() || always.is_some_and(|always| !always.is_empty())
    }

    /// Ctrl+R: once confirmed, the project's sandbox every project's default again.
    fn reset_sandbox(&mut self) {
        if !self.project_sets() {
            return self.say("This project sets nothing of its own: it follows the defaults");
        }
        let names = ["Reset this project to the defaults", "Keep it"];
        let notes = vec![
            "Forgets what was set here and the directories granted always here.".to_owned(),
            "Changes nothing.".to_owned(),
        ];
        if self.choose("Reset this project", &names, notes, None) == Some(0) {
            self.change_sandbox(SandboxChange::Reset);
        }
    }

    /// The rows of the default's paths the project took out of `list`, each to put back.
    fn removed_rows(&self, rows: &mut Vec<Row>, subagents: bool, list: PathList) {
        let note = match list {
            PathList::Writable => "In every project's default, read-only here. Enter puts it back.",
            PathList::Hidden => "Hidden by default, shown in this project. Enter hides it again.",
            PathList::Readable => "In every project's default, taken out here. Enter puts it back.",
        };
        for listed in taken_out(&self.session.settings, subagents, list) {
            let value = format!("{} (taken out here)", shown(&listed.path));
            rows.push(row(
                label(list),
                value,
                note,
                Entry::Restore(list, listed.path),
            ));
        }
    }

    fn subagent_rows(&self) -> Vec<Row> {
        let settings = &self.session.settings;
        let (mode, network, reach) = settings.subagent_policy();
        let now = settings.rows();
        let default = settings.sandbox_default().unwrap_or(&now);
        let mut rows = Vec::new();
        let change = match (mode, cfg!(windows)) {
            (_, true) => "nothing (on Windows they only read)",
            (SandboxMode::ReadOnly, false) => "nothing",
            (SandboxMode::WorkspaceWrite, false) => "where they work",
        };
        let note = "Where they work: those whose agent files list write or edit change their \
                    working directory or worktree. Enter switches.";
        let set = now.subagent_mode != default.subagent_mode;
        let (change, note) = here(change, note.to_owned(), set);
        rows.push(row("Can change", change, note, Entry::Change));
        let (read, note) = match reach {
            None => (
                "same as the main agent",
                "They read what the main agent can. Enter chooses.",
            ),
            Some(reach) => (reach_name(reach), reach_note(reach)),
        };
        let set = now.subagent_reach != default.subagent_reach;
        let (read, note) = here(read, note.to_owned(), set);
        rows.push(row("Can read", read, note, Entry::Read));
        let (value, note) = match network {
            true => (
                "on",
                "Their commands reach the internet, the local network and this machine's servers.",
            ),
            false => (
                "off",
                "Their commands reach neither the internet, the local network nor this machine's servers.",
            ),
        };
        let set = now.subagent_network != default.subagent_network;
        let (value, note) = here(value, note.to_owned(), set);
        rows.push(row("Network", value, note, Entry::Network));
        for list in [PathList::Writable, PathList::Hidden, PathList::Readable] {
            self.list_rows(&mut rows, true, list);
        }
        rows
    }

    /// `list`'s rows, each path and Add…; for subagents' hidden and readable lists, "same as
    /// the main agent" or a row to make it so again.
    fn list_rows(&self, rows: &mut Vec<Row>, subagents: bool, list: PathList) {
        let label = label(list);
        let settings = &self.session.settings;
        let default_own = settings.default_listed(subagents, list).is_some();
        let Some(paths) = settings.listed(subagents, list) else {
            let note = "Subagents take the main agent's list. Enter gives them one of their own.";
            let (value, note) = here("same as the main agent", note.to_owned(), default_own);
            rows.push(row(label, value, note, Entry::Same(list)));
            return;
        };
        let (each, add) = match (list, subagents) {
            (PathList::Writable, _) => (
                "Writable to subagents that change where they work. Enter takes it out.",
                "Another directory subagents that write may change.",
            ),
            (PathList::Hidden, _) => (
                "Hidden from commands, read, grep and find, as the credential stores are. Enter \
                 shows it again.",
                "A file or directory to hide from the tools.",
            ),
            (PathList::Readable, _) => (
                "Readable again inside what is hidden or out of reach. Enter takes it out.",
                "A path to make readable inside what is hidden or out of reach; never a \
                 credential store.",
            ),
        };
        let own_here = subagents && settings.sandbox_default().is_some() && !default_own;
        for path in paths {
            let set = !own_here && settings.added_here(subagents, list, &path);
            let (value, note) = here(&shown(&path), each.to_owned(), set);
            rows.push(row(label, value, note, Entry::Path(list, path)));
        }
        if !own_here {
            self.removed_rows(rows, subagents, list);
        }
        rows.push(row(label, "Add…", add, Entry::Add(list)));
        if subagents && list != PathList::Writable {
            let note = match own_here {
                true => {
                    "Subagents' own list is set for this project. Enter makes them take the \
                         main agent's list again."
                }
                false => "Enter makes subagents take the main agent's list again.",
            };
            rows.push(row(
                label,
                "Same as the main agent…",
                note,
                Entry::Back(list),
            ));
        }
    }

    /// The choices of what the main agent can change; the one chosen applies at once.
    fn choose_change(&mut self) {
        let current = self.session.agent.sandbox().map(|sandbox| sandbox.mode);
        let at = CHANGES.iter().position(|(mode, _)| *mode == current);
        let names: Vec<&str> = CHANGES.iter().map(|(mode, _)| change_name(*mode)).collect();
        let notes = CHANGES.iter().map(|(_, note)| (*note).to_owned()).collect();
        if let Some(index) = self.choose("Can change", &names, notes, at)
            && Some(index) != at
        {
            self.change_sandbox(SandboxChange::Mode(CHANGES[index].0));
        }
    }

    fn choose_subagent_change(&mut self) {
        let (mode, network, reach) = self.session.settings.subagent_policy();
        let mode = match mode {
            SandboxMode::ReadOnly => SandboxMode::WorkspaceWrite,
            SandboxMode::WorkspaceWrite => SandboxMode::ReadOnly,
        };
        self.change_sandbox(SandboxChange::Subagents(mode, network, reach));
    }

    /// The choices of what the agent, or `subagents`, can read; subagents' first is the main
    /// agent's.
    fn choose_reach(&mut self, subagents: bool) {
        let (mode, network, own) = self.session.settings.subagent_policy();
        let mut choices: Vec<Option<Reach>> = REACHES.into_iter().map(Some).collect();
        let current = match subagents {
            true => {
                choices.insert(0, None);
                own
            }
            false => Some(self.session.settings.reach()),
        };
        let at = choices.iter().position(|reach| *reach == current);
        let names: Vec<&str> = (choices.iter())
            .map(|reach| reach.map_or("same as the main agent", reach_name))
            .collect();
        let notes = (choices.iter())
            .map(|reach| {
                reach
                    .map_or("What the main agent can read.", reach_note)
                    .to_owned()
            })
            .collect();
        let Some(index) = self
            .choose("Can read", &names, notes, at)
            .filter(|i| Some(*i) != at)
        else {
            return;
        };
        match (subagents, choices[index]) {
            (true, reach) => self.change_sandbox(SandboxChange::Subagents(mode, network, reach)),
            (false, Some(reach)) => self.change_sandbox(SandboxChange::Reach(reach)),
            (false, None) => {}
        }
    }

    /// `names` to choose from, `current` marked; the index chosen.
    fn choose(
        &self,
        title: &str,
        names: &[&str],
        notes: Vec<String>,
        current: Option<usize>,
    ) -> Option<usize> {
        let screen = self.screen.as_ref()?;
        let picker = select::Picker::select(
            title,
            select::VISIBLE,
            names.len(),
            current.unwrap_or(0),
            current,
            |index, width| clip(names[index], width),
        );
        let hint = select::hint("select", &[], "back");
        screen.pick(picker.noting(hint, notes).held())
    }
}
