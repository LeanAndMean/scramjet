//! `/sandbox` on the full screen: the session's sandbox in a list, its mode, network and the
//! directories granted, each changed with Enter.
use super::interactive::Interactive;
use super::screen::{Answer, Dialog, Optional};
use super::text::clip;
use super::{footer, select};
use scramjet::agent::{Remembered, SandboxMode};
use std::path::PathBuf;

/// The modes `/sandbox` offers, by the names it takes.
const MODES: [(&str, &str); 3] = [
    (
        "read-only",
        "Commands read every file but change none, and write and edit are refused.",
    ),
    (
        "workspace-write",
        "Commands, write and edit change the project and the directories granted, except its \
         .git, .scramjet and .agents.",
    ),
    (
        "off",
        "No sandbox: the agent's tools run with your access, the credential stores hidden as \
         the hideCredentials setting says.",
    ),
];

enum Entry {
    Mode,
    Network,
    Add,
    /// A directory granted in this session or always in this project, and whether always.
    Granted(PathBuf, bool),
}

impl Interactive {
    /// `/sandbox` without an argument: the list on the full screen, else what `/sandbox` says.
    pub(super) fn sandbox_menu(&mut self) {
        if self.screen.is_none() {
            return self.sandbox("");
        }
        let mut at = 0;
        loop {
            let root = self.agent.root().to_owned();
            let sandbox = self.agent.sandbox().cloned();
            let always =
                (Remembered::user().and_then(|kept| kept.of(&root).ok())).unwrap_or_default();
            let mut entries = vec![Entry::Mode];
            if sandbox.is_some() {
                entries.extend([Entry::Network, Entry::Add]);
            }
            let granted = sandbox
                .as_ref()
                .map_or(&[][..], |sandbox| &sandbox.writable[..]);
            for path in granted {
                entries.push(Entry::Granted(path.clone(), always.contains(path)));
            }
            for path in always.iter().filter(|path| !granted.contains(path)) {
                entries.push(Entry::Granted(path.clone(), true));
            }
            let mode = sandbox
                .as_ref()
                .map_or("off", |sandbox| sandbox.mode.name());
            let summary = super::slash::sandbox_summary(&self.agent);
            let rows: Vec<String> = (entries.iter())
                .map(|entry| match entry {
                    Entry::Mode => format!("Mode      {mode}"),
                    Entry::Network => match sandbox.as_ref().is_some_and(|s| s.network) {
                        true => "Network   on".to_owned(),
                        false => "Network   off".to_owned(),
                    },
                    Entry::Add => "Add a directory…".to_owned(),
                    Entry::Granted(path, false) => format!("Writable  {}", path.display()),
                    Entry::Granted(path, true) => {
                        format!("Writable  {} (always here)", path.display())
                    }
                })
                .collect();
            let notes = (entries.iter())
                .map(|entry| match entry {
                    Entry::Mode => format!("How the agent's tools are confined. {summary}"),
                    Entry::Network => "Whether commands in the sandbox reach the network.".into(),
                    Entry::Add => {
                        "Let the agent change another directory too, for this session (as \
                         /add-dir DIR)."
                            .into()
                    }
                    Entry::Granted(_, false) => {
                        "Granted for this session. Enter makes it read-only again.".into()
                    }
                    Entry::Granted(_, true) => {
                        "Granted always in this project. Enter makes it read-only again and \
                         forgets it."
                            .into()
                    }
                })
                .collect();
            let Some(screen) = &self.screen else {
                return;
            };
            screen.footer(footer::Shown::of(&self.agent));
            let picker = select::Picker::select(
                "Sandbox",
                select::VISIBLE,
                rows.len(),
                at.min(rows.len() - 1),
                None,
                |index, width| clip(&rows[index], width),
            );
            let hint = select::hint("change", &[], "close");
            let Some(index) = screen.pick(picker.noting(hint, notes).held()) else {
                break;
            };
            at = index;
            match &entries[index] {
                Entry::Mode => self.choose_sandbox_mode(sandbox.map(|sandbox| sandbox.mode)),
                Entry::Network => match sandbox.is_some_and(|sandbox| sandbox.network) {
                    true => self.sandbox("network off"),
                    false => self.sandbox("network on"),
                },
                Entry::Add => {
                    let title = "Directory the agent may change".to_owned();
                    let dialog = Dialog::Text(title, String::new(), Optional::No);
                    if let Some(Answer::Text(Some(typed))) = screen.ask(dialog, None) {
                        self.add_dir(typed.trim());
                    }
                }
                Entry::Granted(path, _) => self.remove_dir(&path.display().to_string()),
            }
        }
        if let Some(screen) = &self.screen {
            screen.dismiss();
        }
    }

    /// The modes, the session's marked; the one chosen is the session's.
    fn choose_sandbox_mode(&mut self, current: Option<SandboxMode>) {
        let Some(screen) = &self.screen else {
            return;
        };
        let name = current.map_or("off", SandboxMode::name);
        let current = MODES.iter().position(|(mode, _)| *mode == name);
        let picker = select::Picker::select(
            "Sandbox mode",
            select::VISIBLE,
            MODES.len(),
            current.unwrap_or(0),
            current,
            |index, width| clip(MODES[index].0, width),
        );
        let notes = MODES.iter().map(|(_, note)| (*note).to_owned()).collect();
        let hint = select::hint("select", &[], "back");
        if let Some(index) = screen.pick(picker.noting(hint, notes).held())
            && Some(index) != current
        {
            self.sandbox(MODES[index].0);
        }
    }
}
