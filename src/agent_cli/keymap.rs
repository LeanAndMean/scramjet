//! What each key does: every action with its id and keys, Scramjet's where the action is
//! Scramjet's, and the user's `keybindings.json` beside the settings, in Scramjet's format, which
//! gives an action other keys. Each part asks whether a key means one of its actions
//! (`Key::is`), so one key can mean one thing in the editor and another in a list.

use super::keys::{ALT, CTRL, Code, Press, SHIFT};
use scramjet_rs::agent::Problem;
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
    sync::{LazyLock, PoisonError, RwLock},
};

pub(super) const FILE: &str = "keybindings.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Binding {
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    CursorLineStart,
    CursorLineEnd,
    JumpForward,
    JumpBackward,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Submit,
    NewLine,
    DeleteCharBackward,
    DeleteCharForward,
    DeleteWordBackward,
    DeleteWordForward,
    DeleteToLineStart,
    DeleteToLineEnd,
    Yank,
    YankPop,
    Undo,
    Tab,
    Interrupt,
    Clear,
    Copy,
    Exit,
    Suspend,
    ThinkingCycle,
    ModelCycleForward,
    ModelCycleBackward,
    ModelSelect,
    ToolsExpand,
    ThinkingToggle,
    External,
    FollowUp,
    Dequeue,
    PasteImage,
    SelectUp,
    SelectDown,
    SelectPageUp,
    SelectPageDown,
    SelectFirst,
    SelectLast,
    SelectConfirm,
    SelectCancel,
    SessionRename,
    SessionDelete,
    SessionToggleSort,
    SessionToggleNamedFilter,
    SessionTogglePath,
    ModelsSave,
    ModelsEnableAll,
    ModelsClearAll,
    ModelsToggleProvider,
    ModelsReorderUp,
    ModelsReorderDown,
    TreeFoldOrUp,
    TreeUnfoldOrDown,
    TreeEditLabel,
    TreeToggleLabelTimestamp,
    TreeFilterDefault,
    TreeFilterNoTools,
    TreeFilterUserOnly,
    TreeFilterLabeledOnly,
    TreeFilterAll,
    TreeFilterCycleForward,
    TreeFilterCycleBackward,
}

/// Where `/hotkeys` lists an action.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Navigation,
    Editing,
    Other,
    Lists,
    Resume,
    ScopedModels,
    Tree,
}

const SECTIONS: [(Section, &str); 7] = [
    (Section::Navigation, "Navigation"),
    (Section::Editing, "Editing"),
    (Section::Other, "Other"),
    (Section::Lists, "Lists"),
    (Section::Resume, "In /resume"),
    (Section::ScopedModels, "In /scoped-models"),
    (Section::Tree, "In /tree"),
];

struct Definition {
    binding: Binding,
    id: &'static str,
    keys: &'static [&'static str],
    section: Section,
    description: &'static str,
}

const fn define(
    binding: Binding,
    id: &'static str,
    keys: &'static [&'static str],
    section: Section,
    description: &'static str,
) -> Definition {
    Definition {
        binding,
        id,
        keys,
        section,
        description,
    }
}

use Binding as B;
use Section as S;

/// Every action, in `Binding`'s order. The ids and the default keys are Scramjet's
/// (`docs/keybindings.md`) where scramjet-rs's action is Scramjet's; those Scramjet lacks are
/// named in its manner.
#[rustfmt::skip]
const DEFINITIONS: [Definition; 71] = [
    define(B::CursorUp, "tui.editor.cursorUp", &["up"], S::Navigation, "Move cursor up / browse history"),
    define(B::CursorDown, "tui.editor.cursorDown", &["down", "ctrl+n"], S::Navigation, "Move cursor down / browse history"),
    define(B::CursorLeft, "tui.editor.cursorLeft", &["left", "ctrl+b"], S::Navigation, "Move cursor left"),
    define(B::CursorRight, "tui.editor.cursorRight", &["right", "ctrl+f"], S::Navigation, "Move cursor right"),
    define(B::CursorWordLeft, "tui.editor.cursorWordLeft", &["alt+left", "ctrl+left", "alt+b"], S::Navigation, "Move a word left"),
    define(B::CursorWordRight, "tui.editor.cursorWordRight", &["alt+right", "ctrl+right", "alt+f"], S::Navigation, "Move a word right"),
    define(B::CursorLineStart, "tui.editor.cursorLineStart", &["home", "ctrl+a"], S::Navigation, "Start of line"),
    define(B::CursorLineEnd, "tui.editor.cursorLineEnd", &["end", "ctrl+e"], S::Navigation, "End of line"),
    define(B::JumpForward, "tui.editor.jumpForward", &["ctrl+]"], S::Navigation, "Jump forward to character"),
    define(B::JumpBackward, "tui.editor.jumpBackward", &["ctrl+alt+]"], S::Navigation, "Jump backward to character"),
    define(B::PageUp, "tui.viewport.pageUp", &["pageUp"], S::Navigation, "Scroll the conversation up a page"),
    define(B::PageDown, "tui.viewport.pageDown", &["pageDown"], S::Navigation, "Scroll the conversation down a page"),
    define(B::Top, "tui.viewport.top", &["ctrl+home"], S::Navigation, "Scroll to the conversation's start"),
    define(B::Bottom, "tui.viewport.bottom", &["ctrl+end"], S::Navigation, "Scroll to the conversation's end"),
    define(B::Submit, "tui.input.submit", &["enter"], S::Editing, "Send message (during a turn: steer)"),
    define(B::NewLine, "tui.input.newLine", &["shift+enter", "ctrl+j"], S::Editing, "New line (also a trailing \\ and Enter)"),
    define(B::DeleteCharBackward, "tui.editor.deleteCharBackward", &["backspace"], S::Editing, "Delete character backwards"),
    define(B::DeleteCharForward, "tui.editor.deleteCharForward", &["delete", "ctrl+d"], S::Editing, "Delete character forwards"),
    define(B::DeleteWordBackward, "tui.editor.deleteWordBackward", &["ctrl+w", "alt+backspace"], S::Editing, "Delete word backwards"),
    define(B::DeleteWordForward, "tui.editor.deleteWordForward", &["alt+d", "alt+delete", "ctrl+delete"], S::Editing, "Delete word forwards"),
    define(B::DeleteToLineStart, "tui.editor.deleteToLineStart", &["ctrl+u"], S::Editing, "Delete to start of line"),
    define(B::DeleteToLineEnd, "tui.editor.deleteToLineEnd", &["ctrl+k"], S::Editing, "Delete to end of line"),
    define(B::Yank, "tui.editor.yank", &["ctrl+y"], S::Editing, "Paste the most-recently-deleted text"),
    define(B::YankPop, "tui.editor.yankPop", &["alt+y"], S::Editing, "Cycle through the deleted text after pasting"),
    define(B::Undo, "tui.editor.undo", &["ctrl+-"], S::Editing, "Undo"),
    define(B::Tab, "tui.input.tab", &["tab"], S::Other, "Path completion / accept autocomplete"),
    define(B::Interrupt, "app.interrupt", &["escape"], S::Other, "Cancel autocomplete / interrupt the turn"),
    define(B::Clear, "app.clear", &["ctrl+c"], S::Other, "Clear editor (first) / exit (second)"),
    define(B::Copy, "tui.input.copy", &["ctrl+c"], S::Other, "Copy the text selected with the mouse"),
    define(B::Exit, "app.exit", &["ctrl+d"], S::Other, "Exit (when editor is empty)"),
    define(B::Suspend, "app.suspend", &["ctrl+z"], S::Other, "Suspend to background"),
    define(B::ThinkingCycle, "app.thinking.cycle", &["shift+tab"], S::Other, "Cycle thinking level"),
    define(B::ModelCycleForward, "app.model.cycleForward", &["ctrl+p"], S::Other, "Cycle to the next model"),
    define(B::ModelCycleBackward, "app.model.cycleBackward", &["shift+ctrl+p"], S::Other, "Cycle to the previous model"),
    define(B::ModelSelect, "app.model.select", &["ctrl+l"], S::Other, "Open model selector"),
    define(B::ToolsExpand, "app.tools.expand", &["ctrl+o"], S::Other, "Toggle tool output expansion"),
    define(B::ThinkingToggle, "app.thinking.toggle", &["ctrl+t"], S::Other, "Toggle thinking block visibility"),
    define(B::External, "app.editor.external", &["ctrl+g"], S::Other, "Edit message in external editor"),
    define(B::FollowUp, "app.message.followUp", &["alt+enter"], S::Other, "Queue follow-up message (or send)"),
    define(B::Dequeue, "app.message.dequeue", &["alt+up"], S::Other, "Restore queued messages"),
    define(B::PasteImage, "app.clipboard.pasteImage", &["ctrl+v", "alt+v"], S::Other, "Paste image from clipboard"),
    define(B::SelectUp, "tui.select.up", &["up"], S::Lists, "Move selection up"),
    define(B::SelectDown, "tui.select.down", &["down", "ctrl+n"], S::Lists, "Move selection down"),
    define(B::SelectPageUp, "tui.select.pageUp", &["pageUp"], S::Lists, "Move selection up a page"),
    define(B::SelectPageDown, "tui.select.pageDown", &["pageDown"], S::Lists, "Move selection down a page"),
    define(B::SelectFirst, "tui.select.first", &["home", "ctrl+a"], S::Lists, "Select the first row"),
    define(B::SelectLast, "tui.select.last", &["end", "ctrl+e"], S::Lists, "Select the last row"),
    define(B::SelectConfirm, "tui.select.confirm", &["enter"], S::Lists, "Choose the selected row"),
    define(B::SelectCancel, "tui.select.cancel", &["escape", "ctrl+c"], S::Lists, "Cancel"),
    define(B::SessionRename, "app.session.rename", &["ctrl+r"], S::Resume, "Rename the session"),
    define(B::SessionDelete, "app.session.delete", &["ctrl+d"], S::Resume, "Delete the session, once confirmed"),
    define(B::SessionToggleSort, "app.session.toggleSort", &["ctrl+s"], S::Resume, "Cycle the order"),
    define(B::SessionToggleNamedFilter, "app.session.toggleNamedFilter", &["ctrl+n"], S::Resume, "Show only named sessions, or all"),
    define(B::SessionTogglePath, "app.session.togglePath", &["ctrl+p"], S::Resume, "Show or hide the session files' paths"),
    define(B::ModelsSave, "app.models.save", &["ctrl+s"], S::ScopedModels, "Save the models switched on"),
    define(B::ModelsEnableAll, "app.models.enableAll", &["ctrl+a"], S::ScopedModels, "Switch on all models shown"),
    define(B::ModelsClearAll, "app.models.clearAll", &["ctrl+x"], S::ScopedModels, "Switch off all models shown"),
    define(B::ModelsToggleProvider, "app.models.toggleProvider", &["ctrl+p"], S::ScopedModels, "Switch the provider's models on or off"),
    define(B::ModelsReorderUp, "app.models.reorderUp", &["alt+up"], S::ScopedModels, "Move the model up"),
    define(B::ModelsReorderDown, "app.models.reorderDown", &["alt+down"], S::ScopedModels, "Move the model down"),
    define(B::TreeFoldOrUp, "app.tree.foldOrUp", &["ctrl+left", "alt+left", "alt+b"], S::Tree, "Fold the branch, or up to the one before"),
    define(B::TreeUnfoldOrDown, "app.tree.unfoldOrDown", &["ctrl+right", "alt+right", "alt+f"], S::Tree, "Unfold the branch, or down to the next"),
    define(B::TreeEditLabel, "app.tree.editLabel", &["shift+l"], S::Tree, "Label the entry"),
    define(B::TreeToggleLabelTimestamp, "app.tree.toggleLabelTimestamp", &["shift+t"], S::Tree, "Show when labels were given"),
    define(B::TreeFilterDefault, "app.tree.filter.default", &["ctrl+d"], S::Tree, "Filter: the default view"),
    define(B::TreeFilterNoTools, "app.tree.filter.noTools", &["ctrl+t"], S::Tree, "Filter: without tool results"),
    define(B::TreeFilterUserOnly, "app.tree.filter.userOnly", &["ctrl+u"], S::Tree, "Filter: only your messages"),
    define(B::TreeFilterLabeledOnly, "app.tree.filter.labeledOnly", &["ctrl+l"], S::Tree, "Filter: only labeled entries"),
    define(B::TreeFilterAll, "app.tree.filter.all", &["ctrl+a"], S::Tree, "Filter: all entries"),
    define(B::TreeFilterCycleForward, "app.tree.filter.cycleForward", &["ctrl+o"], S::Tree, "Next filter"),
    define(B::TreeFilterCycleBackward, "app.tree.filter.cycleBackward", &["shift+ctrl+o"], S::Tree, "Previous filter"),
];

/// The keys of every action, in `DEFINITIONS`' order.
#[derive(Debug, PartialEq)]
pub(super) struct Keymap(Vec<Vec<Press>>);

impl Keymap {
    pub(super) fn defaults() -> Self {
        Self(
            (DEFINITIONS.iter())
                .map(|definition| {
                    (definition.keys.iter())
                        .map(|name| parse(name).expect("a default key is a key"))
                        .collect()
                })
                .collect(),
        )
    }

    fn has(&self, binding: Binding, press: Press) -> bool {
        self.0[binding as usize].contains(&press)
    }

    /// The keymap `text` gives, Scramjet's: an object of actions' ids, each with a key or a list
    /// of keys (an empty list leaves the action without any); what is wrong in it, each of those
    /// actions keeping its default keys.
    fn read(text: &str) -> (Self, Vec<String>) {
        let mut keymap = Self::defaults();
        let object = match serde_json::from_str(text) {
            Ok(Value::Object(object)) => object,
            Ok(_) => return (keymap, vec!["not a JSON object; using the defaults".into()]),
            Err(error) => return (keymap, vec![format!("{error}; using the defaults")]),
        };
        let mut warnings = Vec::new();
        for (id, value) in object {
            let Some(at) = DEFINITIONS
                .iter()
                .position(|definition| definition.id == id)
            else {
                warnings.push(format!("{id} is no action; ignored"));
                continue;
            };
            let names = match &value {
                Value::String(name) => vec![name.as_str()],
                Value::Array(names) => match names.iter().map(Value::as_str).collect() {
                    Some(names) => names,
                    None => {
                        warnings.push(format!("{id} must list keys as strings; ignored"));
                        continue;
                    }
                },
                _ => {
                    warnings.push(format!("{id} must be a key or a list of keys; ignored"));
                    continue;
                }
            };
            let presses: Option<Vec<Press>> = names.iter().map(|name| parse(name)).collect();
            match presses {
                Some(mut presses) => {
                    presses.dedup();
                    keymap.0[at] = presses;
                }
                None => {
                    let unknown = names.iter().find(|name| parse(name).is_none());
                    let unknown = unknown.expect("a name did not parse");
                    warnings.push(format!("{id}: {unknown:?} is no key; ignored"));
                }
            }
        }
        (keymap, warnings)
    }
}

static CURRENT: LazyLock<RwLock<Keymap>> = LazyLock::new(|| RwLock::new(Keymap::defaults()));

/// Whether `press` is one of `binding`'s keys.
pub(super) fn bound(binding: Binding, press: Press) -> bool {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .has(binding, press)
}

/// The keybindings file, beside the settings file.
pub(super) fn path(settings: &Path) -> PathBuf {
    settings.with_file_name(FILE)
}

/// Reads the keybindings file beside `settings`, at start and at `/reload`, its keys taking the
/// place of the defaults; what is wrong in it. Without it every action has its default keys.
pub(super) fn load(settings: Option<&Path>) -> Vec<Problem> {
    let Some(path) = settings.map(path) else {
        return Vec::new();
    };
    let shown = scramjet_rs::agent::shortened(&path.display().to_string());
    let (keymap, warnings) = match std::fs::read_to_string(&path) {
        Ok(text) => Keymap::read(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (Keymap::defaults(), Vec::new()),
        Err(error) => (
            Keymap::defaults(),
            vec![format!("{error}; using the defaults")],
        ),
    };
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = keymap;
    (warnings.into_iter())
        .map(|warning| Problem::in_file(format!("{shown}: {warning}"), &path))
        .collect()
}

/// A key as Scramjet's `keybindings.json` names it: `ctrl+shift+p`, `alt+enter`, `pageUp`,
/// `shift+l`, `ctrl+-`; `None` when it names no key.
pub(super) fn parse(name: &str) -> Option<Press> {
    let name = name.trim().to_ascii_lowercase();
    // The key is what follows the last `+`, which may be the key itself: `ctrl++`.
    let (modifiers, key) = match name.strip_suffix("++") {
        Some(modifiers) => (modifiers, "+"),
        None if name == "+" => ("", "+"),
        None => name.rsplit_once('+').unwrap_or(("", &name)),
    };
    let mut bits = 0;
    for modifier in modifiers.split('+').filter(|modifier| !modifier.is_empty()) {
        bits |= match modifier {
            "ctrl" => CTRL,
            "shift" => SHIFT,
            "alt" => ALT,
            _ => return None,
        };
    }
    let code = match key {
        "escape" | "esc" => Code::Escape,
        "enter" | "return" => Code::Enter,
        "tab" => Code::Tab,
        "space" => Code::Char(' '),
        "backspace" => Code::Backspace,
        "delete" => Code::Delete,
        "insert" => Code::Insert,
        "home" => Code::Home,
        "end" => Code::End,
        "pageup" => Code::PageUp,
        "pagedown" => Code::PageDown,
        "up" => Code::Up,
        "down" => Code::Down,
        "left" => Code::Left,
        "right" => Code::Right,
        _ => match key.strip_prefix('f').and_then(|n| n.parse().ok()) {
            Some(n @ 1..=12) => Code::F(n),
            _ => {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii_graphic() => Code::Char(c),
                    _ => return None,
                }
            }
        },
    };
    Some(Press::new(code, bits))
}

/// A key as `/hotkeys` shows it: `Ctrl+Shift+P`, `Page Up`, `Shift+L`.
pub(super) fn shown(press: Press) -> String {
    let mut text = String::new();
    for (bit, name) in [(CTRL, "Ctrl+"), (SHIFT, "Shift+"), (ALT, "Alt+")] {
        if press.modifiers & bit != 0 {
            text.push_str(name);
        }
    }
    match press.code {
        Code::Char(' ') => text.push_str("Space"),
        Code::Char(c) => text.push(c.to_ascii_uppercase()),
        Code::F(n) => text.push_str(&format!("F{n}")),
        code => text.push_str(match code {
            Code::Enter => "Enter",
            Code::Tab => "Tab",
            Code::Escape => "Escape",
            Code::Backspace => "Backspace",
            Code::Delete => "Delete",
            Code::Insert => "Insert",
            Code::Home => "Home",
            Code::End => "End",
            Code::PageUp => "Page Up",
            Code::PageDown => "Page Down",
            Code::Up => "Up",
            Code::Down => "Down",
            Code::Left => "Left",
            _ => "Right",
        }),
    }
    text
}

/// `binding`'s keys as `/hotkeys` shows them: `Home / Ctrl+A`; `(none)` without any.
fn shown_keys(keymap: &Keymap, binding: Binding) -> String {
    match keymap.0[binding as usize].as_slice() {
        [] => "(none)".to_owned(),
        keys => keys
            .iter()
            .map(|&key| shown(key))
            .collect::<Vec<_>>()
            .join(" / "),
    }
}

/// What `/hotkeys` lists: under each heading, each action's keys and what it does, with the keys
/// that are not actions of their own (Escape twice, `/` and `!` at the prompt) and the mouse's.
pub(super) fn listing() -> Vec<(&'static str, Vec<(String, &'static str)>)> {
    let keymap = CURRENT.read().unwrap_or_else(PoisonError::into_inner);
    let mut sections: Vec<_> = (SECTIONS.iter())
        .map(|&(section, heading)| {
            let rows: Vec<_> = (DEFINITIONS.iter())
                .filter(|definition| definition.section == section)
                .flat_map(|definition| {
                    let row = (
                        shown_keys(&keymap, definition.binding),
                        definition.description,
                    );
                    let twice = (definition.binding == Binding::Interrupt)
                        .then(|| keymap.0[Binding::Interrupt as usize].first())
                        .flatten()
                        .map(|&key| {
                            let key = shown(key);
                            let what = "Open /tree on an empty prompt (doubleEscapeAction)";
                            (format!("{key} {key}"), what)
                        });
                    std::iter::once(row).chain(twice)
                })
                .collect();
            (heading, rows)
        })
        .collect();
    let other = &mut sections[2].1;
    for (key, what) in [
        ("/", "Slash commands"),
        ("!", "Run bash command"),
        ("!!", "Run bash command (excluded from context)"),
    ] {
        other.push((key.to_owned(), what));
    }
    let mouse = [
        ("Wheel", "Scroll the conversation"),
        ("Drag", "Select text; a right click also copies it"),
        ("Double / triple click", "Select a word / the line"),
        (
            "Right click",
            "Paste text into the prompt (with no selection)",
        ),
    ];
    let mouse = mouse.map(|(key, what)| (key.to_owned(), what));
    sections.push(("Mouse", mouse.into()));
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_defined_once_in_order_with_keys_that_are_keys() {
        let keymap = Keymap::defaults();
        for (at, definition) in DEFINITIONS.iter().enumerate() {
            assert_eq!(definition.binding as usize, at, "{}", definition.id);
            assert!(!keymap.0[at].is_empty(), "{}", definition.id);
        }
        let mut ids: Vec<_> = DEFINITIONS.iter().map(|definition| definition.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DEFINITIONS.len());
    }

    #[test]
    fn names_keys_as_scramjets_file_and_shows_them_as_hotkeys_does() {
        let cases = [
            ("ctrl+shift+p", "Ctrl+Shift+P"),
            ("shift+ctrl+P", "Ctrl+Shift+P"),
            ("alt+enter", "Alt+Enter"),
            ("pageUp", "Page Up"),
            ("shift+l", "Shift+L"),
            ("ctrl+-", "Ctrl+-"),
            ("ctrl+alt+]", "Ctrl+Alt+]"),
            ("ctrl++", "Ctrl++"),
            ("esc", "Escape"),
            ("space", "Space"),
            ("f12", "F12"),
            ("f", "F"),
            ("!", "!"),
        ];
        for (name, display) in cases {
            let press = parse(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(shown(press), display, "{name}");
        }
        for name in ["", "ctrl+", "hyper+a", "f13", "ab", "ctrl+é"] {
            assert_eq!(parse(name), None, "{name}");
        }
    }

    #[test]
    fn the_users_file_replaces_an_actions_keys_and_says_what_it_ignores() {
        let (keymap, warnings) = Keymap::read(
            r#"{
                "app.message.followUp": ["ctrl+q", "ctrl+q"],
                "app.tools.expand": "f2",
                "app.thinking.toggle": [],
                "app.nothing": "ctrl+a",
                "app.model.select": ["ctrl+l", "hyper+l"],
                "app.exit": 4
            }"#,
        );
        let ctrl = |c| Press::new(Code::Char(c), CTRL);
        assert_eq!(keymap.0[B::FollowUp as usize], [ctrl('q')]);
        assert_eq!(
            keymap.0[B::ToolsExpand as usize],
            [Press::new(Code::F(2), 0)]
        );
        assert!(keymap.0[B::ThinkingToggle as usize].is_empty());
        assert_eq!(keymap.0[B::ModelSelect as usize], [ctrl('l')]);
        assert_eq!(keymap.0[B::Exit as usize], [ctrl('d')]);
        assert_eq!(
            warnings,
            [
                "app.exit must be a key or a list of keys; ignored",
                r#"app.model.select: "hyper+l" is no key; ignored"#,
                "app.nothing is no action; ignored",
            ]
        );
        let (keymap, warnings) = Keymap::read("[1]");
        assert_eq!((keymap, warnings.len()), (Keymap::defaults(), 1));
    }

    #[test]
    fn the_file_beside_the_settings_is_read_and_missing_means_the_defaults() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-rs-keymap-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let settings = directory.join("settings.json");
        assert_eq!(path(&settings), directory.join(FILE));
        assert!(load(Some(&settings)).is_empty());
        std::fs::write(path(&settings), "{\"tui.select.up\": \"bogus\"}").unwrap();
        let problems = load(Some(&settings));
        let _ = std::fs::remove_dir_all(&directory);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0]
                .text
                .ends_with(r#"keybindings.json: tui.select.up: "bogus" is no key; ignored"#)
        );
        // What it read is the keymap in use, the defaults here, as tests running meanwhile expect.
        assert_eq!(*CURRENT.read().unwrap(), Keymap::defaults());
    }
}
