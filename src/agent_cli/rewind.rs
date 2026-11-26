//! `/rewind`, as Claude Code's: back to before a prompt of the conversation as it is now, the
//! files `write` and `edit` changed since put back as they were, the conversation moved there as
//! `/tree` moves it, or both.
use super::drive;
use super::interactive::Interactive;
use super::text::{clip, one_line};
use super::{editor, select};
use scramjet::agent::rewind::{self, Checkpoint, Restored};

/// What going back restores.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Restore {
    Both,
    Conversation,
    Code,
}

impl Restore {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "" | "both" | "all" => Some(Self::Both),
            "conversation" | "chat" => Some(Self::Conversation),
            "code" | "files" => Some(Self::Code),
            _ => None,
        }
    }
}

/// What the full screen offers for a checkpoint, as Claude Code's: code only where files changed.
fn choices(files: bool) -> Vec<(Option<Restore>, &'static str)> {
    let mut choices = Vec::new();
    if files {
        choices.push((Some(Restore::Both), "Restore code and conversation"));
    }
    choices.push((Some(Restore::Conversation), "Restore conversation"));
    if files {
        choices.push((Some(Restore::Code), "Restore code"));
    }
    choices.push((None, "Never mind"));
    choices
}

/// ` · 2 files changed since`, after a checkpoint's prompt.
fn changed(point: &Checkpoint) -> String {
    match point.files.len() {
        0 => String::new(),
        1 => " · 1 file changed since".to_owned(),
        count => format!(" · {count} files changed since"),
    }
}

/// What restoring did, a line for each kind; `None` when every file was as it had been.
fn report(root: &std::path::Path, restored: &[(std::path::PathBuf, Restored)]) -> Vec<String> {
    let names = |wanted: fn(&Restored) -> bool| -> Vec<String> {
        (restored.iter())
            .filter(|(_, how)| wanted(how))
            .map(|(path, _)| rewind::shown(root, path))
            .collect()
    };
    let mut lines = Vec::new();
    let put_back = names(|how| *how == Restored::Restored);
    if !put_back.is_empty() {
        lines.push(format!("✓ Restored {}", put_back.join(", ")));
    }
    let removed = names(|how| *how == Restored::Removed);
    if !removed.is_empty() {
        lines.push(format!("✓ Removed {}, made since", removed.join(", ")));
    }
    for (path, how) in restored {
        if let Restored::Failed(error) = how {
            let path = rewind::shown(root, path);
            lines.push(format!("Error: {path} not restored: {error}"));
        }
    }
    if lines.is_empty() {
        lines.push("The files are already as they were".to_owned());
    }
    lines
}

impl Interactive {
    /// `/rewind` picks a prompt and what to restore; `/rewind N [code|conversation]` goes back
    /// to before prompt N at once, both by default; in line mode `/rewind` lists them.
    pub(super) fn rewind(&mut self, wanted: &str) {
        let points = match self.session.agent.checkpoints() {
            Ok(points) => points,
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        if points.is_empty() {
            return self.say("No messages to rewind to");
        }
        let mut words = wanted.split_whitespace();
        let (number, what) = (words.next(), words.next().unwrap_or(""));
        let (index, restore) = match number {
            None if self.interactive() => {
                let Some(index) = self.select(
                    "Rewind to before",
                    select::VISIBLE,
                    points.len(),
                    points.len() - 1,
                    None,
                    |index, width| {
                        let point = &points[index];
                        let after = changed(point);
                        let room = width.saturating_sub(after.chars().count()).max(2);
                        format!("{}{after}", clip(&one_line(&point.prompt), room))
                    },
                ) else {
                    return;
                };
                let choices = choices(!points[index].files.is_empty());
                let title = self.rewind_title(&points[index]);
                let Some(chosen) = self.select(
                    &title,
                    select::VISIBLE,
                    choices.len(),
                    0,
                    None,
                    |index, width| clip(choices[index].1, width),
                ) else {
                    return;
                };
                let Some(restore) = choices[chosen].0 else {
                    return;
                };
                (index, restore)
            }
            None => {
                let digits = points.len().to_string().len();
                let width = editor::terminal_size().0.saturating_sub(digits + 1);
                for (index, point) in points.iter().enumerate() {
                    let after = changed(point);
                    let room = width.saturating_sub(after.chars().count()).max(2);
                    let line = clip(&one_line(&point.prompt), room);
                    eprintln!("{:>digits$} {line}{after}", index + 1);
                }
                return;
            }
            Some(number) => {
                let Some(restore) = Restore::parse(what) else {
                    return self.say("Error: /rewind N takes code, conversation or nothing");
                };
                match number.parse::<usize>() {
                    Ok(number) if (1..=points.len()).contains(&number) => (number - 1, restore),
                    Ok(number) if number > 0 => {
                        let said =
                            format!("Error: no message numbered {number} (/rewind lists them)");
                        return self.say(&said);
                    }
                    _ => return self.say("Error: /rewind requires a positive message number"),
                }
            }
        };
        let point = &points[index];
        if point.files.is_empty() && restore == Restore::Code {
            return self.say("No files were changed since that message");
        }
        // Said after the move, which shows the conversation afresh.
        let said = match restore {
            Restore::Conversation => Vec::new(),
            _ => {
                let root = self.session.agent.root().to_owned();
                report(&root, &self.session.agent.restore_files(&point.files))
            }
        };
        if restore != Restore::Code {
            let navigated = match &mut self.screen {
                Some(screen) => {
                    (self.session.agent).navigate(point.line, None, &mut screen.link, &self.cancel)
                }
                None => (self.session.agent).navigate(point.line, None, &mut self.ui, &self.cancel),
            };
            match navigated {
                Ok(navigated) => drive::navigated(self, navigated),
                Err(error) => self.say(&format!("Error: {error}")),
            }
        }
        for line in said {
            self.say(&line);
        }
    }

    /// The question after a prompt is picked: what code going back restores.
    fn rewind_title(&self, point: &Checkpoint) -> String {
        if point.files.is_empty() {
            return "No files were changed since: restore the conversation?".to_owned();
        }
        let root = self.session.agent.root();
        let files: Vec<String> = (point.files.iter())
            .map(|file| rewind::shown(root, &file.path))
            .collect();
        format!(
            "Restore {} (changes made by commands and subagents stay)",
            files.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choices_offer_code_only_where_files_changed() {
        let labels = |files| -> Vec<&str> { choices(files).iter().map(|c| c.1).collect() };
        assert_eq!(
            labels(true),
            [
                "Restore code and conversation",
                "Restore conversation",
                "Restore code",
                "Never mind"
            ]
        );
        assert_eq!(labels(false), ["Restore conversation", "Never mind"]);
        assert_eq!(Restore::parse(""), Some(Restore::Both));
        assert_eq!(Restore::parse("code"), Some(Restore::Code));
        assert_eq!(Restore::parse("x"), None);
    }

    #[test]
    fn the_report_names_what_was_restored_removed_and_left() {
        let root = std::path::Path::new("/p");
        let restored = [
            ("/p/a.rs".into(), Restored::Restored),
            ("/p/new.rs".into(), Restored::Removed),
            ("/p/same.rs".into(), Restored::Unchanged),
            ("/p/b.rs".into(), Restored::Failed("denied".into())),
        ];
        assert_eq!(
            report(root, &restored),
            [
                "✓ Restored a.rs",
                "✓ Removed new.rs, made since",
                "Error: b.rs not restored: denied"
            ]
        );
        let same = [("/p/same.rs".into(), Restored::Unchanged)];
        assert_eq!(report(root, &same), ["The files are already as they were"]);
    }
}
