//! The editor's completion list, as Scramjet's autocomplete: the built-in and loaded
//! slash commands, their arguments, paths and `@` file references.

use super::editor::Buffer;
use super::file_search;
use super::keymap::{self, Binding};
use super::text;
use std::{fs, path::Path};
use unicode_width::UnicodeWidthStr;

/// Scramjet's commands that this agent has, in Scramjet's order, with its descriptions, named
/// as in Claude Code or Codex where they name the same action differently.
const SLASH_COMMANDS: [(&str, &str); 26] = [
    ("settings", "Open settings menu"),
    ("model", "Select model (opens selector UI)"),
    // Shown with the key that cycles in place of "model" (`scoped_models`).
    ("scoped-models", "Enable/disable models for model cycling"),
    (
        "export",
        "Export session (HTML default, or specify path: .html/.jsonl)",
    ),
    ("import", "Import and resume a session from a JSONL file"),
    ("copy", "Copy last agent message to clipboard"),
    ("rename", "Set session display name"),
    ("status", "Show session info and stats"),
    (
        "usage",
        "Show quota left and what was spent, or its details (/usage details)",
    ),
    ("release-notes", "View release notes"),
    ("hotkeys", "Show all keyboard shortcuts, and change them"),
    ("keybindings", "Open your keyboard shortcuts file"),
    ("fork", "Create a new fork from a previous user message"),
    (
        "branch",
        "Duplicate the current session at the current position",
    ),
    ("tree", "Navigate session tree (switch branches)"),
    ("login", "Log in to a provider (GitHub Copilot)"),
    ("logout", "Remove a provider's saved login"),
    ("clear", "Start a new session"),
    ("compact", "Manually compact the session context"),
    ("resume", "Resume a different session"),
    (
        "reload",
        "Reload settings, themes, command sets, agents, skills and context files",
    ),
    (
        "autopilot",
        "Toggle command auto-continuation: /autopilot on|off|status",
    ),
    (
        "mcp",
        "Show the MCP servers and their tools: /mcp [reconnect|enable|disable [SERVER|all]]",
    ),
    (
        "sandbox",
        "Show or change the sandbox: /sandbox [MODE|network on|off|add DIR|remove DIR]",
    ),
    (
        "add-dir",
        "Let the sandboxed agent change another directory: /add-dir DIR",
    ),
    ("exit", "Quit Scramjet"),
];

/// Other names of the listed commands, accepted but not listed: Scramjet's, and Claude Code's
/// and Codex's for the same action.
const ALIASES: [(&str, &str); 13] = [
    ("new", "clear"),
    ("reset", "clear"),
    ("quit", "exit"),
    ("name", "rename"),
    ("session", "status"),
    ("cost", "usage"),
    ("stats", "usage"),
    ("config", "settings"),
    ("clone", "branch"),
    ("continue", "resume"),
    ("help", "hotkeys"),
    ("keymap", "hotkeys"),
    ("changelog", "release-notes"),
];

/// The listed name of the built-in command typed as `name` (without its `/`).
pub(super) fn listed(name: &str) -> &str {
    ALIASES
        .iter()
        .find(|&&(alias, _)| alias == name)
        .map_or(name, |&(_, listed)| listed)
}

/// `/scoped-models`' description, as Scramjet's, with the key that cycles now: `Enable/disable
/// models for Ctrl+P cycling`.
fn scoped_models() -> Option<String> {
    keymap::key(Binding::ModelCycleForward)
        .map(|key| format!("Enable/disable models for {key} cycling"))
}

/// What the loaded settings add to the completions: the commands and skills the `/` list shows
/// after the built-in ones (names and descriptions), and the MCP servers `/mcp reconnect`, `enable` and `disable` name.
#[derive(Default)]
pub(super) struct Completions {
    pub commands: Vec<(String, String)>,
    pub mcp_servers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Suggestion {
    /// What replaces the completed text.
    pub(super) value: String,
    pub(super) label: String,
    pub(super) description: String,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Slash,
    /// A built-in command's argument.
    Argument,
    Path,
    /// An `@` file reference.
    File,
}

/// The list under the prompt that completes a slash command or its argument, a path or an `@`
/// file reference, as Scramjet's editor does. It replaces the text from `start` to the cursor.
pub(super) struct Completion {
    pub(super) start: usize,
    pub(super) kind: Kind,
    pub(super) items: Vec<Suggestion>,
    pub(super) selected: usize,
}

/// An `@` search started for the prompt's text and cursor as they were then.
pub(super) struct Lookup {
    pub(super) search: file_search::Search,
    pub(super) text: String,
    pub(super) cursor: usize,
    /// Started by Tab, which puts in a single entry found at once.
    pub(super) tab: bool,
}

impl Lookup {
    pub(super) fn start(
        root: &Path,
        mention: file_search::Mention,
        buffer: &Buffer,
        tab: bool,
    ) -> Self {
        Self {
            search: file_search::Search::start(root, mention),
            text: buffer.text.clone(),
            cursor: buffer.cursor,
            tab,
        }
    }
}

impl Completion {
    /// The commands whose names fuzzily match what follows a `/` that begins the prompt: the
    /// built-in ones, then `loaded` (names and descriptions).
    pub(super) fn slash(before: &str, loaded: &[(String, String)]) -> Option<Self> {
        let typed = before.strip_prefix('/')?;
        if typed.contains(char::is_whitespace) {
            return None;
        }
        let scoped = scoped_models();
        let builtin = SLASH_COMMANDS
            .iter()
            .map(|&(name, description)| match name {
                "scoped-models" => (name, scoped.as_deref().unwrap_or(description)),
                _ => (name, description),
            });
        let loaded = loaded
            .iter()
            .map(|(name, description)| (&**name, &**description));
        let commands: Vec<_> = builtin.chain(loaded).collect();
        let names: Vec<_> = commands
            .iter()
            .map(|(name, _)| super::fuzzy::Haystack::new(name))
            .collect();
        let mut indices = super::fuzzy::filter(&super::fuzzy::parse(typed), &names);
        // Enter runs the first, so another name typed in full puts its command there, before
        // a set's command its letters match too.
        let aliased = listed(typed);
        if aliased != typed {
            let at = commands.iter().position(|&(name, _)| name == aliased);
            indices.retain(|&index| Some(index) != at);
            indices.splice(0..0, at);
        }
        let items = (indices.into_iter())
            .map(|index| {
                let (name, description) = commands[index];
                Suggestion {
                    value: format!("/{name} "),
                    label: format!("/{name}"),
                    description: description.to_owned(),
                }
            })
            .collect();
        Self::new(0, Kind::Slash, items)
    }

    /// The argument of a built-in command that Scramjet completes: after `/model `, the
    /// providers' `models` (references, `provider/id`) whose id and provider match it fuzzily,
    /// each shown by its id with its provider; after `/autopilot `, the words starting with it;
    /// after `/mcp reconnect `, `enable ` or `disable `, `all` and the `mcp_servers` starting
    /// with it.
    pub(super) fn argument(
        before: &str,
        models: &[String],
        mcp_servers: &[String],
    ) -> Option<Self> {
        let (name, mut typed) = before.strip_prefix('/')?.split_once(' ')?;
        if typed.contains('\n') {
            return None;
        }
        let word = |word: &str| Suggestion {
            value: word.to_owned(),
            label: word.to_owned(),
            description: String::new(),
        };
        let items = match name {
            "mcp" if typed.contains(' ') => {
                let (action, rest) = typed.split_once(' ').unwrap_or_default();
                if !["reconnect", "enable", "disable"].contains(&action) {
                    return None;
                }
                typed = rest.trim_start();
                if typed.contains(char::is_whitespace) {
                    return None;
                }
                (std::iter::once("all").chain(mcp_servers.iter().map(String::as_str)))
                    .filter(|name| name.starts_with(typed))
                    .map(word)
                    .collect()
            }
            "autopilot" | "mcp" => (match name {
                "autopilot" => &["on", "off", "status"][..],
                _ => &["reconnect", "enable", "disable"][..],
            })
            .iter()
            .filter(|word| word.starts_with(typed))
            .map(|&text| word(text))
            .collect(),
            "model" => {
                let parts = |reference: &'_ str| {
                    let (provider, id) = reference.split_once('/').unwrap_or(("", reference));
                    (provider.to_owned(), id.to_owned())
                };
                let texts: Vec<_> = (models.iter())
                    .map(|reference| {
                        let (provider, id) = parts(reference);
                        super::fuzzy::Haystack::new(&format!("{id} {provider}"))
                    })
                    .collect();
                (super::fuzzy::filter(&super::fuzzy::parse(typed), &texts).into_iter())
                    .map(|index| {
                        let (description, label) = parts(&models[index]);
                        let value = models[index].clone();
                        Suggestion {
                            value,
                            label,
                            description,
                        }
                    })
                    .collect()
            }
            _ => return None,
        };
        let mut list = Self::new(before.len() - typed.len(), Kind::Argument, items)?;
        list.select_best(typed);
        Some(list)
    }

    /// The entries an `@` search found for the text from `start` to the cursor.
    pub(super) fn found(start: usize, items: Vec<Suggestion>, buffer: &Buffer) -> Option<Self> {
        let mut list = Self::new(start, Kind::File, items)?;
        list.select_best(&buffer.text[start..buffer.cursor]);
        Some(list)
    }

    /// Selects the entry that is `typed` or else the first that starts with it, as Scramjet
    /// selects its best match; an argument's shown label counts too, as a model's id does.
    fn select_best(&mut self, typed: &str) {
        let labelled = matches!(self.kind, Kind::Argument);
        let any = |test: &dyn Fn(&str) -> bool| {
            (self.items.iter())
                .position(|item| test(&item.value) || (labelled && test(&item.label)))
        };
        let best = any(&|text| text == typed).or_else(|| any(&|text| text.starts_with(typed)));
        self.selected = best.unwrap_or(0);
    }

    /// The entries of the directory named by the word before the cursor whose names start with
    /// the rest of it, as Scramjet's `getFileSuggestions` lists them: relative to `root`, `~/` in
    /// the home directory, ignoring case, directories first. The word is all that follows a
    /// quote left open where a word starts, and an entry with a space or after it is quoted.
    pub(super) fn path(before: &str, root: &Path) -> Option<Self> {
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        let line = &before[line_start..];
        let quote = file_search::open_quote(line)
            .filter(|&at| at == 0 || line[..at].ends_with(file_search::DELIMITERS));
        let start = line_start
            + quote.unwrap_or_else(|| line.rfind(file_search::DELIMITERS).map_or(0, |at| at + 1));
        let word = &before[start + usize::from(quote.is_some())..];
        let (shown, name) = match word.rfind('/') {
            Some(slash) => word.split_at(slash + 1),
            None if word == "~" => ("~/", ""),
            None => ("", word),
        };
        let directory = match shown.strip_prefix("~/") {
            Some(rest) => std::env::home_dir()?.join(rest),
            None => root.join(shown),
        };
        let name = name.to_lowercase();
        let mut items: Vec<(bool, Suggestion)> = fs::read_dir(directory)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file = entry.file_name().into_string().ok()?;
                if !file.to_lowercase().starts_with(&name) {
                    return None;
                }
                // A link to a directory counts as one.
                let kind = entry.file_type().ok()?;
                let directory = kind.is_dir() || kind.is_symlink() && entry.path().is_dir();
                let label = if directory { format!("{file}/") } else { file };
                let value = format!("{shown}{label}");
                let value = match quote.is_some() || value.contains(' ') {
                    true => format!("\"{value}\""),
                    false => value,
                };
                Some((
                    directory,
                    Suggestion {
                        value,
                        label,
                        description: String::new(),
                    },
                ))
            })
            .collect();
        items.sort_by_cached_key(|(directory, item)| (!directory, item.label.to_lowercase()));
        Self::new(
            start,
            Kind::Path,
            items.into_iter().map(|(_, item)| item).collect(),
        )
    }

    fn new(start: usize, kind: Kind, items: Vec<Suggestion>) -> Option<Self> {
        (!items.is_empty()).then_some(Self {
            start,
            kind,
            items,
            selected: 0,
        })
    }

    pub(super) fn select(&mut self, up: bool) {
        let count = self.items.len();
        self.selected = (self.selected + if up { count - 1 } else { 1 }) % count;
    }

    pub(super) fn apply(self, buffer: &mut Buffer) {
        let item = &self.items[self.selected];
        let value = &item.value;
        if matches!(self.kind, Kind::Slash | Kind::Argument) {
            buffer.text.replace_range(self.start..buffer.cursor, value);
            buffer.cursor = self.start + value.len();
            return;
        }
        // As Scramjet: an `@` file is followed by a space, a directory not, so that its
        // entries can be completed next, inside its quotes when quoted, and the quote after the
        // cursor is the one a quoted entry closes.
        let directory = item.label.ends_with('/');
        let closed = value.ends_with('"');
        let typed = &buffer.text[self.start..buffer.cursor];
        let mut end = buffer.cursor;
        if closed
            && (typed.starts_with('"') || typed.starts_with("@\""))
            && buffer.text[end..].starts_with('"')
        {
            end += 1;
        }
        let suffix = if self.kind == Kind::File && !directory {
            " "
        } else {
            ""
        };
        buffer
            .text
            .replace_range(self.start..end, &format!("{value}{suffix}"));
        buffer.cursor = self.start + value.len() + suffix.len() - usize::from(directory && closed);
    }

    /// The rows under the prompt, as Scramjet's select list draws them: the selected one
    /// highlighted with `→ `, descriptions in a column, at most `most` entries, and the position
    /// when some are hidden.
    pub(super) fn rows(&self, width: usize, most: usize) -> Vec<String> {
        use super::style::{COMPLETION_SELECTED, MUTED, RESET};
        let width = width.saturating_sub(1);
        let count = self.items.len();
        let visible = count.min(most);
        let top = self
            .selected
            .saturating_sub(visible / 2)
            .min(count - visible);
        // Scramjet's commands' column fits their names; other lists' is 32 columns wide.
        let column = match self.kind {
            Kind::Slash => self
                .items
                .iter()
                .map(|item| text::visible(&item.label).width() + 2)
                .max(),
            _ => None,
        };
        let column = column.map_or(32, |column| column.clamp(12, 32));
        let mut rows: Vec<String> = (top..top + visible)
            .map(|index| {
                let item = &self.items[index];
                let room = width.saturating_sub(2 + column);
                let described = !item.description.is_empty() && room > 10;
                // As Scramjet's, a name before a description keeps 2 cells of its column free.
                let label_room = match described {
                    true => column - 2,
                    false => width.saturating_sub(2),
                };
                // File names and command descriptions come from the project.
                let label = text::clip(&text::visible(&item.label), label_room);
                let description = if !described {
                    String::new()
                } else {
                    let pad = " ".repeat(column.saturating_sub(label.width()));
                    format!(
                        "{pad}{}",
                        text::clip(&text::visible(&item.description), room)
                    )
                };
                if index == self.selected {
                    format!("{COMPLETION_SELECTED}→ {label}{description}{RESET}")
                } else if description.is_empty() {
                    format!("  {label}")
                } else {
                    format!("  {label}{MUTED}{description}{RESET}")
                }
            })
            .collect();
        if visible < count {
            let position = format!("  ({}/{count})", self.selected + 1);
            rows.push(format!("{MUTED}{}{RESET}", text::clip(&position, width)));
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::editor::{Action, Editor};
    use crate::agent_cli::keys::Key;
    use crate::agent_cli::keys::tests::key;
    use std::{path::PathBuf, sync::Arc};

    #[test]
    fn completion_rows_escape_control_characters() {
        let loaded = [("z\x1b[2J".to_owned(), "clears\x1b]0;title\x07".to_owned())];
        let list = Completion::slash("/z", &loaded).unwrap();
        assert_eq!(
            plain(list.rows(60, 5)),
            ["→ /z\\u{1b}[2J  clears\\u{1b}]0;title\\u{7}"]
        );
    }

    fn plain(rows: Vec<String>) -> Vec<String> {
        rows.iter()
            .map(|row| {
                row.replace(crate::agent_cli::style::COMPLETION_SELECTED.get(), "")
                    .replace(crate::agent_cli::style::MUTED.get(), "")
                    .replace(crate::agent_cli::style::RESET, "")
            })
            .collect()
    }

    fn labels(list: &Completion) -> Vec<&str> {
        list.items.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn slash_commands_filter_and_complete_with_a_trailing_space() {
        let list = Completion::slash("/", &[]).unwrap();
        assert_eq!(
            labels(&list),
            [
                "/settings",
                "/model",
                "/scoped-models",
                "/export",
                "/import",
                "/copy",
                "/rename",
                "/status",
                "/usage",
                "/release-notes",
                "/hotkeys",
                "/keybindings",
                "/fork",
                "/branch",
                "/tree",
                "/login",
                "/logout",
                "/clear",
                "/compact",
                "/resume",
                "/reload",
                "/autopilot",
                "/mcp",
                "/sandbox",
                "/add-dir",
                "/exit",
            ]
        );
        assert_eq!(
            plain(list.rows(60, 5)),
            [
                "→ /settings       Open settings menu",
                "  /model          Select model (opens selector UI)",
                "  /scoped-models  Enable/disable models for Ctrl+P cycling",
                "  /export         Export session (HTML default, or specify…",
                "  /import         Import and resume a session from a JSONL…",
                "  (1/26)",
            ]
        );
        // The selected row stays in the middle where it can; a narrow terminal drops descriptions.
        let mut list = list;
        for _ in 0..8 {
            list.select(false);
        }
        assert_eq!(
            plain(list.rows(24, 5)),
            [
                "  /rename",
                "  /status",
                "→ /usage",
                "  /release-notes",
                "  /hotkeys",
                "  (9/26)"
            ]
        );
        for _ in 0..18 {
            list.select(false);
        }
        assert_eq!(list.selected, 0);
        list.select(true);
        assert_eq!(list.items[list.selected].label, "/exit");

        let mut buffer = Buffer {
            text: "/resu rest".into(),
            cursor: 5,
            anchor: None,
        };
        let list = Completion::slash(&buffer.text[..buffer.cursor], &[]).unwrap();
        assert_eq!(labels(&list), ["/resume"]);
        list.apply(&mut buffer);
        assert_eq!((buffer.text.as_str(), buffer.cursor), ("/resume  rest", 8));

        assert!(Completion::slash("/zz", &[]).is_none());
        assert!(Completion::slash("/model ", &[]).is_none());
        assert!(Completion::slash("a /model", &[]).is_none());
    }

    #[test]
    fn slash_commands_match_fuzzily_and_rank_best_matches_first() {
        let list = Completion::slash("/mdl", &[]).unwrap();
        assert_eq!(labels(&list), ["/model", "/scoped-models"]);

        let list = Completion::slash("/m", &[]).unwrap();
        assert_eq!(labels(&list)[..2], ["/mcp", "/model"]);
        assert!(labels(&list).contains(&"/compact"));
        assert!(Completion::slash("/zzz", &[]).is_none());
    }

    #[test]
    fn another_name_typed_in_full_lists_its_command_first() {
        let loaded = [("mach12:pr-review-assessment".to_owned(), String::new())];
        let first = |typed| labels(&Completion::slash(typed, &loaded).unwrap())[0].to_owned();
        // `reset`'s letters are in the set's command too, which Enter would otherwise run.
        assert_eq!(first("/reset"), "/clear");
        assert_eq!(first("/new"), "/clear");
        assert_eq!(first("/name"), "/rename");
        assert_eq!(first("/stats"), "/usage");
        assert_eq!(first("/quit"), "/exit");
        // Unlisted names are not in the list until typed in full.
        assert!(Completion::slash("/qui", &loaded).is_none());
        let all = Completion::slash("/", &loaded).unwrap();
        assert!(
            !labels(&all)
                .iter()
                .any(|label| ["/new", "/quit", "/name"].contains(label))
        );
    }

    #[test]
    fn built_in_arguments_complete_as_scramjets() {
        let models = [
            "openai/gpt-6-luna",
            "anthropic/claude-haiku-5-5",
            "openai/gpt-6.1-sol",
        ]
        .map(String::from);
        // The models in the providers' order, then those matching what is typed, best first,
        // each by its id with its provider.
        let list = Completion::argument("/model ", &models, &[]).unwrap();
        assert_eq!(
            labels(&list),
            ["gpt-6-luna", "claude-haiku-5-5", "gpt-6.1-sol"]
        );
        assert_eq!(
            plain(list.rows(60, 5))[..2],
            [
                format!("→ gpt-6-luna{}openai", " ".repeat(22)),
                format!("  claude-haiku-5-5{}anthropic", " ".repeat(16))
            ]
        );
        let list = Completion::argument("/model 6 sol", &models, &[]).unwrap();
        assert_eq!(labels(&list), ["gpt-6.1-sol"]);
        let list = Completion::argument("/model luna openai", &models, &[]).unwrap();
        assert_eq!(labels(&list), ["gpt-6-luna"]);
        // An id two providers offer is two entries, the one typed in full selected.
        let twice = [
            "openai/gpt-6.1-sol",
            "openai/gpt-6-luna",
            "github-copilot/gpt-6-luna",
        ];
        let twice = twice.map(String::from);
        let list = Completion::argument("/model gpt-6-luna", &twice, &[]).unwrap();
        let shown: Vec<_> = (list.items.iter())
            .map(|item| (item.label.as_str(), item.description.as_str()))
            .collect();
        assert_eq!(
            shown,
            [("gpt-6-luna", "openai"), ("gpt-6-luna", "github-copilot")]
        );
        assert_eq!(list.items[list.selected].value, "openai/gpt-6-luna");
        // Equal matches put the shorter id first, as Scramjet's `fuzzyFilter` does.
        let gpt = ["openai/gpt-6.1-sol", "openai/gpt-6-luna"].map(String::from);
        let list = Completion::argument("/model gpt", &gpt, &[]).unwrap();
        assert_eq!(labels(&list), ["gpt-6-luna", "gpt-6.1-sol"]);
        let list = Completion::slash("/se", &[]).unwrap();
        assert_eq!(labels(&list)[0], "/settings");
        let mut buffer = Buffer {
            text: "/model hai".into(),
            cursor: 10,
            anchor: None,
        };
        Completion::argument(&buffer.text, &models, &[])
            .unwrap()
            .apply(&mut buffer);
        assert_eq!(
            (buffer.text.as_str(), buffer.cursor),
            ("/model anthropic/claude-haiku-5-5", 33)
        );
        // `/autopilot`'s words are those that start with what is typed, as typed, and so are
        // `/mcp reconnect`'s servers.
        let servers = ["tavily", "test"].map(String::from);
        let words = |before| {
            Completion::argument(before, &[], &servers).map(|list| labels(&list).join(" "))
        };
        assert_eq!(words("/autopilot ").unwrap(), "on off status");
        assert_eq!(words("/autopilot o").unwrap(), "on off");
        assert_eq!(words("/autopilot st").unwrap(), "status");
        assert!(words("/autopilot O").is_none());
        assert!(words("/autopilot on x").is_none());
        assert_eq!(words("/mcp r").unwrap(), "reconnect");
        assert_eq!(words("/mcp ").unwrap(), "reconnect enable disable");
        assert_eq!(words("/mcp reconnect ").unwrap(), "all tavily test");
        assert_eq!(words("/mcp disable t").unwrap(), "tavily test");
        assert_eq!(words("/mcp enable ").unwrap(), "all tavily test");
        assert!(words("/mcp x ").is_none());
        assert_eq!(words("/mcp reconnect  t").unwrap(), "tavily test");
        assert!(words("/mcp reconnect x").is_none());
        assert!(words("/mcp reconnect test x").is_none());
        let mut buffer = Buffer {
            text: "/mcp reconnect ta".into(),
            cursor: 17,
            anchor: None,
        };
        let list = Completion::argument(&buffer.text, &[], &servers).unwrap();
        list.apply(&mut buffer);
        assert_eq!(buffer.text, "/mcp reconnect tavily");
        // Nothing else completes: other commands, a later line, no models.
        assert!(Completion::argument("/name ", &models, &[]).is_none());
        assert!(Completion::argument("/model x\nhai", &models, &[]).is_none());
        assert!(Completion::argument("say /model ", &models, &[]).is_none());
        assert!(Completion::argument("/model ", &[], &[]).is_none());
        assert!(Completion::argument("/model zzz", &models, &[]).is_none());
    }

    #[test]
    fn a_list_shows_as_many_entries_as_it_is_given() {
        let models: Vec<String> = (1..=9).map(|n| format!("model-{n}")).collect();
        let mut editor = Editor::new(
            Vec::new(),
            Arc::default(),
            PathBuf::new(),
            Completions::default(),
            models,
            false,
        );
        let mut draft = editor.draft("/model".into());
        editor.key(&mut draft, Key::Text(" ".into()), false);
        let shown = |most| plain(draft.completion_rows(40, most));
        assert_eq!(shown(3), ["→ model-1", "  model-2", "  model-3", "  (1/9)"]);
        assert_eq!(shown(7).len(), 8);
        assert_eq!(shown(20).len(), 9, "all nine, without a position");
    }

    #[test]
    fn enter_puts_in_an_argument_without_sending_it() {
        let models = vec!["claude-haiku-5-5".to_owned(), "gpt-6-luna".to_owned()];
        let mut editor = Editor::new(
            Vec::new(),
            Arc::default(),
            PathBuf::new(),
            Completions::default(),
            models,
            false,
        );
        let mut draft = editor.draft("/model".into());
        for text in [" ", "luna"] {
            let key = Key::Text(text.into());
            assert_eq!(editor.key(&mut draft, key, false), Action::Edited(true));
        }
        assert_eq!(plain(draft.completion_rows(40, 5)), ["→ gpt-6-luna"]);
        assert_eq!(
            editor.key(&mut draft, key("enter"), false),
            Action::Edited(true)
        );
        assert_eq!(draft.text(), "/model gpt-6-luna");
        assert!(!draft.completing());
        assert_eq!(
            editor.key(&mut draft, key("enter"), false),
            Action::Submit("/model gpt-6-luna".into())
        );
        // One typed in full is sent at once.
        let mut draft = editor.draft("/autopilot o".into());
        assert_eq!(
            editor.key(&mut draft, Key::Text("n".into()), false),
            Action::Edited(true)
        );
        assert_eq!(plain(draft.completion_rows(40, 5)), ["→ on"]);
        assert_eq!(
            editor.key(&mut draft, key("enter"), false),
            Action::Submit("/autopilot on".into())
        );
    }

    #[test]
    fn loaded_commands_are_listed_after_the_built_in_ones() {
        let loaded = [
            (
                "rs:loop".to_owned(),
                "[focus] — Build one increment".to_owned(),
            ),
            ("rs:review".to_owned(), "Review".to_owned()),
        ];
        let list = Completion::slash("/", &loaded).unwrap();
        assert_eq!(
            labels(&list)[SLASH_COMMANDS.len()..],
            ["/rs:loop", "/rs:review"]
        );
        let mut list = Completion::slash("/rs:", &loaded).unwrap();
        assert_eq!(labels(&list), ["/rs:loop", "/rs:review"]);
        list.select(false);
        assert_eq!(
            plain(list.rows(60, 5)),
            [
                "  /rs:loop    [focus] — Build one increment",
                "→ /rs:review  Review",
            ]
        );
        let mut buffer = Buffer {
            text: "/rs:l".into(),
            cursor: 5,
            anchor: None,
        };
        let list = Completion::slash("/rs:l", &loaded).unwrap();
        list.apply(&mut buffer);
        assert_eq!(buffer.text, "/rs:loop ");
        // A name too long for the column is cut before its description.
        let long = [(
            "skill:writing-scramjet-commands".to_owned(),
            "Use when creating commands".to_owned(),
        )];
        let list = Completion::slash("/skill:", &long).unwrap();
        assert_eq!(
            plain(list.rows(80, 5)),
            ["→ /skill:writing-scramjet-comma…  Use when creating commands"]
        );
        assert_eq!(
            plain(list.rows(40, 5)),
            ["→ /skill:writing-scramjet-commands"]
        );
    }

    #[test]
    fn paths_complete_from_the_word_before_the_cursor() {
        let root = std::env::temp_dir().join(format!("sjrs-editor-paths-{}", std::process::id()));
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _scratch = Scratch(root.clone());
        fs::create_dir_all(root.join("Alpha-dir")).unwrap();
        for file in ["alpine.txt", "also.txt", "beta", "Alpha-dir/inner.rs"] {
            fs::write(root.join(file), "").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Alpha-dir"), root.join("link")).unwrap();

        // Directories first, then by name ignoring case.
        let list = Completion::path("see AL", &root).unwrap();
        assert_eq!(labels(&list), ["Alpha-dir/", "alpine.txt", "also.txt"]);
        assert_eq!(plain(list.rows(40, 5))[0], "→ Alpha-dir/");
        let mut buffer = Buffer {
            text: "see AL".into(),
            cursor: 6,
            anchor: None,
        };
        let mut list = list;
        list.select(false);
        list.apply(&mut buffer);
        assert_eq!(buffer.text, "see alpine.txt");

        let list = |before: &str| {
            Completion::path(before, &root).map(|list| {
                list.items
                    .into_iter()
                    .map(|item| item.value)
                    .collect::<Vec<_>>()
            })
        };
        // As Scramjet completes the cursor's line, a word can start one.
        #[cfg(unix)]
        {
            assert_eq!(list("li").unwrap(), ["link/"]);
            assert_eq!(list("see\nli").unwrap(), ["link/"]);
        }
        assert_eq!(list("x=./Alpha-dir/").unwrap(), ["./Alpha-dir/inner.rs"]);
        assert_eq!(list("'Alpha-dir/I").unwrap(), ["Alpha-dir/inner.rs"]);
        let absolute = format!("{}/b", root.display());
        assert_eq!(list(&absolute).unwrap(), [format!("{absolute}eta")]);
        assert_eq!(list("").unwrap().len(), if cfg!(unix) { 5 } else { 4 });
        assert!(list("nothing").is_none());
        assert!(list("missing/a").is_none());

        // As Scramjet's, a path with a space is quoted, and one is completed inside an open
        // quote where a word starts, the quote after the cursor closing it.
        fs::create_dir(root.join("my dir")).unwrap();
        fs::write(root.join("my dir/a file.txt"), "").unwrap();
        fs::write(root.join("my dir/b.txt"), "").unwrap();
        assert_eq!(list("see m").unwrap(), ["\"my dir/\""]);
        assert_eq!(list("\"my d").unwrap(), ["\"my dir/\""]);
        assert_eq!(list("x \"Alpha").unwrap(), ["\"Alpha-dir/\""]);
        assert_eq!(
            list("x=\"my dir/").unwrap(),
            ["\"my dir/a file.txt\"", "\"my dir/b.txt\""]
        );
        // A quote inside a word opens none; a closed one ends the word.
        assert_eq!(list("a\"Alpha").unwrap(), ["Alpha-dir/"]);
        assert_eq!(list("\"x\" Alpha").unwrap(), ["Alpha-dir/"]);
        // A quote on an earlier line is closed by its end.
        assert_eq!(list("\"a\nAlpha").unwrap(), ["Alpha-dir/"]);
        let tab = |text: &str, cursor: usize| {
            let mut buffer = Buffer {
                text: text.into(),
                cursor,
                anchor: None,
            };
            Completion::path(&text[..cursor], &root)
                .unwrap()
                .apply(&mut buffer);
            (buffer.text, buffer.cursor)
        };
        assert_eq!(tab("see my", 6), ("see \"my dir/\"".into(), 12));
        assert_eq!(
            tab("see \"my dir/b\" x", 13),
            ("see \"my dir/b.txt\" x".into(), 18)
        );
        assert_eq!(
            tab("\"Alpha-dir/", 11),
            ("\"Alpha-dir/inner.rs\"".into(), 20)
        );

        let home = std::env::home_dir().unwrap();
        let name = fs::read_dir(&home)
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
            .min()
            .unwrap();
        assert!(
            list("~")
                .unwrap()
                .iter()
                .all(|value| value.trim_start_matches('"').starts_with("~/"))
        );
        assert!(list(&format!("~/{name}")).unwrap().iter().any(|value| {
            value
                .trim_start_matches('"')
                .starts_with(&format!("~/{name}"))
        }));
    }

    #[test]
    fn file_references_select_the_best_match_and_are_put_in_as_scramjet_does() {
        let item = |value: &str, label: &str| Suggestion {
            value: value.into(),
            label: label.into(),
            description: value.trim_start_matches(['@', '"']).into(),
        };
        let apply = |text: &str, cursor: usize, start: usize, value: &str, label: &str| {
            let mut buffer = Buffer {
                text: text.into(),
                cursor,
                anchor: None,
            };
            Completion::found(start, vec![item(value, label)], &buffer)
                .unwrap()
                .apply(&mut buffer);
            (buffer.text, buffer.cursor)
        };
        // A file gets a space after it, a directory none.
        assert_eq!(
            apply("see @edi and", 8, 4, "@src/editor.rs", "editor.rs"),
            ("see @src/editor.rs  and".into(), 19)
        );
        assert_eq!(apply("@sr", 3, 0, "@src/", "src/"), ("@src/".into(), 5));
        // A quoted directory leaves the cursor inside its quotes, and a quoted entry closes the
        // quote that follows the cursor.
        assert_eq!(
            apply("@\"my", 4, 0, "@\"my dir/\"", "my dir/"),
            ("@\"my dir/\"".into(), 9)
        );
        assert_eq!(
            apply("@\"my\" x", 4, 0, "@\"my file\"", "my file"),
            ("@\"my file\"  x".into(), 11)
        );
        assert_eq!(
            apply("@my\"", 3, 0, "@\"my file\"", "my file"),
            ("@\"my file\" \"".into(), 11)
        );

        let buffer = Buffer {
            text: "@src/ed".into(),
            cursor: 7,
            anchor: None,
        };
        let items = || {
            vec![
                item("@lib/edit.rs", "edit.rs"),
                item("@src/editor.rs", "editor.rs"),
            ]
        };
        let list = Completion::found(0, items(), &buffer).unwrap();
        assert_eq!(list.selected, 1);
        assert_eq!(
            plain(list.rows(60, 5)),
            [
                "  edit.rs                         lib/edit.rs",
                "→ editor.rs                       src/editor.rs",
            ]
        );
        let buffer = Buffer {
            text: "@xyz".into(),
            cursor: 4,
            anchor: None,
        };
        assert_eq!(Completion::found(0, items(), &buffer).unwrap().selected, 0);
        assert!(Completion::found(0, Vec::new(), &buffer).is_none());
    }
}
