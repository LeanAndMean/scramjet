//! What each key does: every action with its id and keys, Scramjet's where the action is
//! Scramjet's, and the user's `keybindings.json` beside the settings, in Scramjet's format, which
//! gives an action other keys. Each part asks whether a key means one of its actions
//! (`Key::is`), so one key can mean one thing in the editor and another in a list, and the hints
//! name the keys in use.

use super::keys::{ALT, CTRL, Code, Press, SHIFT};
use scramjet_rs::agent::{Problem, shortened, write_replacing};
use serde_json::{Map, Value};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{LazyLock, PoisonError, RwLock, RwLockReadGuard},
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
    SelectLeft,
    SelectRight,
    SelectLineUp,
    SelectLineDown,
    SelectWordLeft,
    SelectWordRight,
    SelectLineStart,
    SelectLineEnd,
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
    SessionDeleteNoninvasive,
    SessionToggleSort,
    SessionToggleNamedFilter,
    SessionTogglePath,
    SessionToggleChains,
    ModelsSave,
    ModelsEnableAll,
    ModelsClearAll,
    ModelsToggleProvider,
    ModelsReorderUp,
    ModelsReorderDown,
    NextStepFresh,
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
    ThemeRoll,
    ThemeLock,
    ThemeHarmonize,
    ThemeContrastUp,
    ThemeContrastDown,
    ThemeUndo,
    ThemeRedo,
    ThemeRoles,
    ThemeExact,
    ThemeReset,
    ThemeMoveUp,
    ThemeMoveDown,
}

/// Where `/hotkeys` lists an action.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Navigation,
    Selecting,
    Editing,
    Other,
    Lists,
    Resume,
    ScopedModels,
    NextSteps,
    Tree,
    ThemeEditor,
}

const SECTIONS: [(Section, &str); 10] = [
    (Section::Navigation, "Navigation"),
    (Section::Selecting, "Selecting"),
    (Section::Editing, "Editing"),
    (Section::Other, "Other"),
    (Section::Lists, "Lists"),
    (Section::Resume, "In /resume"),
    (Section::ScopedModels, "In /scoped-models"),
    (Section::NextSteps, "In next steps"),
    (Section::Tree, "In /tree"),
    (Section::ThemeEditor, "In the theme editor"),
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
const DEFINITIONS: [Definition; 94] = [
    define(B::CursorUp, "tui.editor.cursorUp", &["up"], S::Navigation, "Move cursor up / browse history"),
    define(B::CursorDown, "tui.editor.cursorDown", &["down", "ctrl+n"], S::Navigation, "Move cursor down / browse history"),
    define(B::CursorLeft, "tui.editor.cursorLeft", &["left", "ctrl+b"], S::Navigation, "Move cursor left"),
    define(B::CursorRight, "tui.editor.cursorRight", &["right", "ctrl+f"], S::Navigation, "Move cursor right"),
    define(B::CursorWordLeft, "tui.editor.cursorWordLeft", &["alt+left", "ctrl+left", "alt+b"], S::Navigation, "Move a word left"),
    define(B::CursorWordRight, "tui.editor.cursorWordRight", &["alt+right", "ctrl+right", "alt+f"], S::Navigation, "Move a word right"),
    define(B::CursorLineStart, "tui.editor.cursorLineStart", &["home", "ctrl+a"], S::Navigation, "Start of line"),
    define(B::CursorLineEnd, "tui.editor.cursorLineEnd", &["end", "ctrl+e"], S::Navigation, "End of line"),
    define(B::SelectLeft, "tui.editor.selectLeft", &["shift+left"], S::Selecting, "Select a character left"),
    define(B::SelectRight, "tui.editor.selectRight", &["shift+right"], S::Selecting, "Select a character right"),
    define(B::SelectLineUp, "tui.editor.selectUp", &["shift+up"], S::Selecting, "Select a line up"),
    define(B::SelectLineDown, "tui.editor.selectDown", &["shift+down"], S::Selecting, "Select a line down"),
    define(B::SelectWordLeft, "tui.editor.selectWordLeft", &["ctrl+shift+left", "alt+shift+left"], S::Selecting, "Select a word left"),
    define(B::SelectWordRight, "tui.editor.selectWordRight", &["ctrl+shift+right", "alt+shift+right"], S::Selecting, "Select a word right"),
    define(B::SelectLineStart, "tui.editor.selectLineStart", &["shift+home"], S::Selecting, "Select to the start of the line"),
    define(B::SelectLineEnd, "tui.editor.selectLineEnd", &["shift+end"], S::Selecting, "Select to the end of the line"),
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
    define(B::DeleteWordBackward, "tui.editor.deleteWordBackward", &["ctrl+w", "alt+backspace", "ctrl+backspace"], S::Editing, "Delete word backwards"),
    define(B::DeleteWordForward, "tui.editor.deleteWordForward", &["alt+d", "alt+delete", "ctrl+delete"], S::Editing, "Delete word forwards"),
    define(B::DeleteToLineStart, "tui.editor.deleteToLineStart", &["ctrl+u"], S::Editing, "Delete to start of line"),
    define(B::DeleteToLineEnd, "tui.editor.deleteToLineEnd", &["ctrl+k"], S::Editing, "Delete to end of line"),
    define(B::Yank, "tui.editor.yank", &["ctrl+y"], S::Editing, "Paste the most-recently-deleted text"),
    define(B::YankPop, "tui.editor.yankPop", &["alt+y"], S::Editing, "Cycle through the deleted text after pasting"),
    define(B::Undo, "tui.editor.undo", &["ctrl+-"], S::Editing, "Undo"),
    define(B::Tab, "tui.input.tab", &["tab"], S::Other, "Path completion / accept autocomplete"),
    define(B::Interrupt, "app.interrupt", &["escape"], S::Other, "Cancel autocomplete / interrupt the turn"),
    define(B::Clear, "app.clear", &["ctrl+c"], S::Other, "Clear editor (first) / exit (second)"),
    define(B::Copy, "tui.input.copy", &["ctrl+c"], S::Other, "Copy the text selected"),
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
    define(B::SessionDeleteNoninvasive, "app.session.deleteNoninvasive", &["ctrl+backspace"], S::Resume, "Delete the session when the query is empty"),
    define(B::SessionToggleSort, "app.session.toggleSort", &["ctrl+s"], S::Resume, "Cycle the order"),
    define(B::SessionToggleNamedFilter, "app.session.toggleNamedFilter", &["ctrl+n"], S::Resume, "Show only named sessions, or all"),
    define(B::SessionTogglePath, "app.session.togglePath", &["ctrl+p"], S::Resume, "Show or hide the session files' paths"),
    define(B::SessionToggleChains, "app.session.toggleChains", &["ctrl+x"], S::Resume, "Show chains of sessions flat, threaded or folded"),
    define(B::ModelsSave, "app.models.save", &["ctrl+s"], S::ScopedModels, "Save the models switched on"),
    define(B::ModelsEnableAll, "app.models.enableAll", &["ctrl+a"], S::ScopedModels, "Switch on all models shown"),
    define(B::ModelsClearAll, "app.models.clearAll", &["ctrl+x"], S::ScopedModels, "Switch off all models shown"),
    define(B::ModelsToggleProvider, "app.models.toggleProvider", &["ctrl+p"], S::ScopedModels, "Switch the provider's models on or off"),
    define(B::ModelsReorderUp, "app.models.reorderUp", &["alt+up"], S::ScopedModels, "Move the model up"),
    define(B::ModelsReorderDown, "app.models.reorderDown", &["alt+down"], S::ScopedModels, "Move the model down"),
    define(B::NextStepFresh, "app.nextStep.toggleFreshSession", &["tab"], S::NextSteps, "Run the step in a fresh session, or in this one"),
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
    define(B::ThemeRoll, "app.theme.roll", &["r"], S::ThemeEditor, "Roll the colors not locked"),
    define(B::ThemeLock, "app.theme.lock", &["l", "space"], S::ThemeEditor, "Lock or unlock the main color"),
    define(B::ThemeHarmonize, "app.theme.harmonize", &["h"], S::ThemeEditor, "Turn the colors a step toward the harmony"),
    define(B::ThemeContrastUp, "app.theme.contrastUp", &["+", "="], S::ThemeEditor, "More contrast"),
    define(B::ThemeContrastDown, "app.theme.contrastDown", &["-"], S::ThemeEditor, "Less contrast"),
    define(B::ThemeUndo, "app.theme.undo", &["u"], S::ThemeEditor, "Undo a roll or step"),
    define(B::ThemeRedo, "app.theme.redo", &["shift+u"], S::ThemeEditor, "Redo a roll or step"),
    define(B::ThemeRoles, "app.theme.roles", &["tab"], S::ThemeEditor, "Show every role, or the main colors again"),
    define(B::ThemeExact, "app.theme.exact", &["#"], S::ThemeEditor, "Type the role's exact color"),
    define(B::ThemeReset, "app.theme.reset", &["backspace", "delete"], S::ThemeEditor, "Make the role follow its own main color again"),
    define(B::ThemeMoveUp, "app.theme.moveUp", &["alt+up"], S::ThemeEditor, "Move the main color up"),
    define(B::ThemeMoveDown, "app.theme.moveDown", &["alt+down"], S::ThemeEditor, "Move the main color down"),
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

    fn of(&self, binding: Binding) -> &[Press] {
        &self.0[binding as usize]
    }

    /// The first key of each of `bindings` that has any, named together as hints do: `Ctrl+O`;
    /// arrows with the same modifiers as `↑↓` or `Ctrl+←→`, other keys with the same ones as
    /// `Ctrl+D/T/U`, else `Ctrl+O/Ctrl+Shift+O`; `None` when none has a key.
    fn hinted(&self, bindings: &[Binding]) -> Option<String> {
        let presses: Vec<Press> = (bindings.iter())
            .filter_map(|&binding| self.0[binding as usize].first().copied())
            .collect();
        let first = *presses.first()?;
        let shared =
            presses.len() > 1 && (presses.iter()).all(|press| press.modifiers == first.modifiers);
        if shared {
            let prefix = modifiers(first.modifiers);
            let arrows: Option<String> = presses.iter().map(|press| arrow(press.code)).collect();
            if let Some(arrows) = arrows {
                return Some(prefix + &arrows);
            }
            if first.modifiers != 0 {
                let names: Vec<_> = presses
                    .iter()
                    .map(|press| named(press.code, true))
                    .collect();
                return Some(prefix + &names.join("/"));
            }
        }
        let names: Vec<_> = (presses.iter())
            .map(|press| modifiers(press.modifiers) + &named(press.code, true))
            .collect();
        Some(names.join("/"))
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

fn current() -> RwLockReadGuard<'static, Keymap> {
    CURRENT.read().unwrap_or_else(PoisonError::into_inner)
}

/// Whether `press` is one of `binding`'s keys.
pub(super) fn bound(binding: Binding, press: Press) -> bool {
    current().has(binding, press)
}

/// `binding`'s first key as hints name it, `Ctrl+O`; `None` when it has none.
pub(super) fn key(binding: Binding) -> Option<String> {
    current().hinted(&[binding])
}

/// The first keys of `bindings` named together as hints do: `↑↓`, `Ctrl+D/T/U`.
pub(super) fn keys(bindings: &[Binding]) -> Option<String> {
    current().hinted(bindings)
}

/// A hint's part, `↑↓ move`: `bindings`' keys and what they do; `None` without keys.
pub(super) fn part(bindings: &[Binding], what: &str) -> Option<String> {
    keys(bindings).map(|keys| format!("{keys} {what}"))
}

/// A hint of the parts that have keys: `↑↓ move · Enter select · Esc cancel`.
pub(super) fn hint(parts: impl IntoIterator<Item = Option<String>>) -> String {
    let parts: Vec<_> = parts.into_iter().flatten().collect();
    parts.join(" · ")
}

/// The keybindings file, beside the settings file.
pub(super) fn path(settings: &Path) -> PathBuf {
    settings.with_file_name(FILE)
}

/// `binding`'s keys now.
pub(super) fn keys_of(binding: Binding) -> Vec<Press> {
    current().of(binding).to_vec()
}

/// `binding`'s default keys.
pub(super) fn defaults_of(binding: Binding) -> Vec<Press> {
    Keymap::defaults().of(binding).to_vec()
}

/// What `binding` does, as `/hotkeys` says.
pub(super) fn description(binding: Binding) -> &'static str {
    DEFINITIONS[binding as usize].description
}

/// `binding`'s id in the keybindings file: `app.message.followUp`.
pub(super) fn id(binding: Binding) -> &'static str {
    DEFINITIONS[binding as usize].id
}

/// Actions the screen does in lists too, as at the prompt.
const EVERYWHERE: [Binding; 6] = [
    B::Suspend,
    B::PasteImage,
    B::ToolsExpand,
    B::ThinkingToggle,
    B::Top,
    B::Bottom,
];

/// Whether a key of `a` can also mean `b`: both act at the prompt, both in the same list (those
/// of all lists in each), or one acts everywhere.
fn together(a: Binding, b: Binding) -> bool {
    let at_prompt = |binding: Binding| {
        let section = DEFINITIONS[binding as usize].section;
        matches!(
            section,
            S::Navigation | S::Selecting | S::Editing | S::Other
        )
    };
    let (s, t) = (
        DEFINITIONS[a as usize].section,
        DEFINITIONS[b as usize].section,
    );
    match (at_prompt(a), at_prompt(b)) {
        (true, true) => true,
        (false, false) => s == t || s == S::Lists || t == S::Lists,
        (true, false) => EVERYWHERE.contains(&a),
        (false, true) => EVERYWHERE.contains(&b),
    }
}

/// What else `press` does where `binding` acts: each other action with it there, as `what (its
/// heading)`, and, for a character without Ctrl or Alt at the prompt, typing it.
pub(super) fn conflicts(binding: Binding, press: Press) -> Vec<String> {
    let keymap = current();
    let mut also: Vec<String> = (DEFINITIONS.iter())
        .filter(|other| other.binding != binding && keymap.has(other.binding, press))
        .filter(|other| together(binding, other.binding))
        .map(|other| {
            let heading = SECTIONS
                .iter()
                .find(|(section, _)| *section == other.section);
            let heading = heading.map_or("", |(_, heading)| heading);
            format!("{} ({heading})", other.description)
        })
        .collect();
    let section = DEFINITIONS[binding as usize].section;
    let typed = matches!(press.code, Code::Char(_)) && press.modifiers & (CTRL | ALT) == 0;
    if typed
        && matches!(
            section,
            S::Navigation | S::Selecting | S::Editing | S::Other
        )
    {
        also.push("typing it in the prompt".to_owned());
    }
    also
}

/// A key as the keybindings file names it, as Scramjet's does: `ctrl+shift+p`, `pageUp`,
/// `space`; `None` for a character outside ASCII, which the file cannot name.
pub(super) fn name(press: Press) -> Option<String> {
    let key = match press.code {
        Code::Char(' ') => "space".to_owned(),
        Code::Char(c) if c.is_ascii_graphic() => c.to_ascii_lowercase().to_string(),
        Code::Char(_) => return None,
        Code::F(n) => format!("f{n}"),
        code => match code {
            Code::Enter => "enter",
            Code::Tab => "tab",
            Code::Escape => "escape",
            Code::Backspace => "backspace",
            Code::Delete => "delete",
            Code::Insert => "insert",
            Code::Home => "home",
            Code::End => "end",
            Code::PageUp => "pageUp",
            Code::PageDown => "pageDown",
            Code::Up => "up",
            Code::Down => "down",
            Code::Left => "left",
            _ => "right",
        }
        .to_owned(),
    };
    let modifiers: String = [(CTRL, "ctrl+"), (SHIFT, "shift+"), (ALT, "alt+")]
        .into_iter()
        .filter(|&(bit, _)| press.modifiers & bit != 0)
        .map(|(_, name)| name)
        .collect();
    Some(modifiers + &key)
}

/// Saves `keys` as `binding`'s in the keybindings file beside `settings`, keeping the rest of
/// it, or takes its entry out when they are its defaults. A file that cannot be read is left as
/// it is. The keys take effect once the file is read again (`load`).
pub(super) fn save(settings: &Path, binding: Binding, keys: &[Press]) -> Result<(), String> {
    let path = path(settings);
    let shown = shortened(&path.display().to_string());
    let mut object = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(Value::Object(object)) => object,
            Ok(_) => return Err(format!("{shown} is not a JSON object")),
            Err(error) => return Err(format!("{shown}: {error}")),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(format!("{shown}: {error}")),
    };
    let id = DEFINITIONS[binding as usize].id.to_owned();
    if keys == defaults_of(binding) {
        object.remove(&id);
    } else {
        let names: Option<Vec<String>> = keys.iter().map(|&key| name(key)).collect();
        let names = names.ok_or("a key outside ASCII cannot be saved")?;
        let value = match names.as_slice() {
            [one] => Value::String(one.clone()),
            _ => Value::Array(names.into_iter().map(Value::String).collect()),
        };
        object.insert(id, value);
    }
    let text = serde_json::to_string_pretty(&Value::Object(object)).map_err(|e| e.to_string())?;
    write_replacing(&path, &(text + "\n")).map_err(|error| format!("{shown}: {error}"))
}

/// Reads the keybindings file beside `settings`, at start and at `/reload`, its keys taking the
/// place of the defaults; what is wrong in it. Without it every action has its default keys.
pub(super) fn load(settings: Option<&Path>) -> Vec<Problem> {
    let Some(path) = settings.map(path) else {
        return Vec::new();
    };
    let shown = shortened(&path.display().to_string());
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
    modifiers(press.modifiers) + &named(press.code, false)
}

/// The modifiers' part of a key's name: `Ctrl+Shift+`.
fn modifiers(bits: u8) -> String {
    [(CTRL, "Ctrl+"), (SHIFT, "Shift+"), (ALT, "Alt+")]
        .into_iter()
        .filter(|&(bit, _)| bits & bit != 0)
        .map(|(_, name)| name)
        .collect()
}

/// A key's name without its modifiers, `short` as hints have it: `Esc`, `PgUp`, `PgDn`.
fn named(code: Code, short: bool) -> String {
    match code {
        Code::Char(' ') => "Space".to_owned(),
        Code::Char(c) => c.to_ascii_uppercase().to_string(),
        Code::F(n) => format!("F{n}"),
        Code::Escape if short => "Esc".to_owned(),
        Code::PageUp if short => "PgUp".to_owned(),
        Code::PageDown if short => "PgDn".to_owned(),
        code => match code {
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
        }
        .to_owned(),
    }
}

/// An arrow key's arrow, as hints show arrows together.
fn arrow(code: Code) -> Option<char> {
    match code {
        Code::Up => Some('↑'),
        Code::Down => Some('↓'),
        Code::Left => Some('←'),
        Code::Right => Some('→'),
        _ => None,
    }
}

/// Keys as `/hotkeys` shows them: `Home / Ctrl+A`; `(none)` without any.
pub(super) fn shown_keys(keys: &[Press]) -> String {
    match keys {
        [] => "(none)".to_owned(),
        keys => keys
            .iter()
            .map(|&key| shown(key))
            .collect::<Vec<_>>()
            .join(" / "),
    }
}

/// A row of `/hotkeys`: keys, what they do, and the action they are, when they are one.
pub(super) struct Shortcut {
    pub(super) keys: String,
    pub(super) what: &'static str,
    pub(super) binding: Option<Binding>,
}

fn fixed(keys: impl Into<String>, what: &'static str) -> Shortcut {
    Shortcut {
        keys: keys.into(),
        what,
        binding: None,
    }
}

/// What `/hotkeys` lists: under each heading, each action's keys and what it does, with the keys
/// that are not actions of their own (Escape twice, `/` and `!` at the prompt) and the mouse's.
pub(super) fn listing() -> Vec<(&'static str, Vec<Shortcut>)> {
    let keymap = current();
    let mut sections: Vec<_> = (SECTIONS.iter())
        .map(|&(section, heading)| {
            let rows: Vec<_> = (DEFINITIONS.iter())
                .filter(|definition| definition.section == section)
                .flat_map(|definition| {
                    let row = Shortcut {
                        keys: shown_keys(keymap.of(definition.binding)),
                        what: definition.description,
                        binding: Some(definition.binding),
                    };
                    let twice = (definition.binding == Binding::Interrupt)
                        .then(|| keymap.of(Binding::Interrupt).first())
                        .flatten()
                        .map(|&key| {
                            let key = shown(key);
                            let what = "Open /tree on an empty prompt (doubleEscapeAction)";
                            fixed(format!("{key} {key}"), what)
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
        other.push(fixed(key, what));
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
    sections.push(("Mouse", mouse.map(|(key, what)| fixed(key, what)).into()));
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
    fn keys_are_named_as_the_file_names_them_and_read_back_the_same() {
        let keymap = Keymap::defaults();
        for press in keymap.0.iter().flatten() {
            let name = name(*press).unwrap_or_else(|| panic!("{press:?}"));
            assert_eq!(parse(&name), Some(*press), "{name}");
        }
        let cases = [
            (Press::new(Code::Char('p'), CTRL | SHIFT), "ctrl+shift+p"),
            (Press::new(Code::PageUp, 0), "pageUp"),
            (Press::new(Code::Char(' '), ALT), "alt+space"),
            (Press::new(Code::Char('+'), CTRL), "ctrl++"),
            (Press::new(Code::F(12), SHIFT), "shift+f12"),
            (Press::new(Code::Char(']'), CTRL | ALT), "ctrl+alt+]"),
        ];
        for (press, expected) in cases {
            assert_eq!(name(press).as_deref(), Some(expected));
            assert_eq!(parse(expected), Some(press), "{expected}");
        }
        assert_eq!(name(Press::new(Code::Char('é'), 0)), None);
    }

    #[test]
    fn a_key_conflicts_with_the_actions_that_act_where_it_would() {
        let ctrl = |c| Press::new(Code::Char(c), CTRL);
        // The defaults, which tests meanwhile keep.
        let cases: [(Binding, Press, &[&str]); 7] = [
            (B::FollowUp, ctrl('k'), &["Delete to end of line (Editing)"]),
            // The editor's and the lists' keys do not meet; those of each list and all lists do.
            (B::CursorDown, ctrl('r'), &[]),
            (
                B::SelectDown,
                ctrl('r'),
                &["Rename the session (In /resume)"],
            ),
            (B::SessionRename, ctrl('l'), &[]),
            (
                B::SessionRename,
                ctrl('o'),
                &["Toggle tool output expansion (Other)"],
            ),
            (
                B::FollowUp,
                Press::new(Code::Char('q'), SHIFT),
                &["typing it in the prompt"],
            ),
            (B::SelectUp, Press::new(Code::Char('k'), 0), &[]),
        ];
        for (binding, press, expected) in cases {
            assert_eq!(conflicts(binding, press), expected, "{binding:?} {press:?}");
        }
    }

    #[test]
    fn saving_sets_one_actions_keys_and_keeps_the_rest_of_the_file() {
        let directory =
            std::env::temp_dir().join(format!("scramjet-rs-keys-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let _scratch = Scratch(directory.clone());
        let settings = directory.join("settings.json");
        let read = || -> Value {
            serde_json::from_str(&std::fs::read_to_string(path(&settings)).unwrap()).unwrap()
        };
        let ctrl = |c| Press::new(Code::Char(c), CTRL);
        let saved = [
            save(&settings, B::FollowUp, &[ctrl('q')]),
            save(
                &settings,
                B::ModelSelect,
                &[ctrl('l'), Press::new(Code::F(5), 0)],
            ),
            save(&settings, B::ThinkingToggle, &[]),
        ];
        let text = std::fs::read_to_string(path(&settings)).unwrap();
        // Another program's entry stays, as does a key the user wrote wrong.
        std::fs::write(
            path(&settings),
            text.replacen('{', "{\"app.nothing\": 1, \"app.exit\": \"hyper+x\",", 1),
        )
        .unwrap();
        // The defaults take the entry out.
        let restored = save(&settings, B::FollowUp, &defaults_of(B::FollowUp));
        let file = read();
        std::fs::write(path(&settings), "{\"app.exit\": ").unwrap();
        let broken = save(&settings, B::Exit, &[ctrl('x')]);
        let left = std::fs::read_to_string(path(&settings)).unwrap();
        assert!(saved.iter().all(Result::is_ok), "{saved:?}");
        assert_eq!(restored, Ok(()));
        assert_eq!(
            file,
            serde_json::json!({
                "app.nothing": 1,
                "app.exit": "hyper+x",
                "app.model.select": ["ctrl+l", "f5"],
                "app.thinking.toggle": [],
            })
        );
        assert!(text.ends_with("}\n") && text.contains("\n  \"app.message.followUp\": \"ctrl+q\""));
        // A file that cannot be read stays as it is.
        assert!(
            broken
                .unwrap_err()
                .contains("keybindings.json: EOF while parsing")
        );
        assert_eq!(left, "{\"app.exit\": ");
    }

    #[test]
    fn hints_name_each_actions_first_key_and_keys_alike_together() {
        let keymap = Keymap::defaults();
        let hinted = |keymap: &Keymap, bindings: &[Binding]| keymap.hinted(bindings);
        let filters = [
            B::TreeFilterDefault,
            B::TreeFilterNoTools,
            B::TreeFilterUserOnly,
        ];
        let cases: [(&[Binding], &str); 7] = [
            (&[B::SelectUp, B::SelectDown], "↑↓"),
            (&[B::TreeFoldOrUp, B::TreeUnfoldOrDown], "Ctrl+←→"),
            (&filters, "Ctrl+D/T/U"),
            (
                &[B::TreeFilterCycleForward, B::TreeFilterCycleBackward],
                "Ctrl+O/Ctrl+Shift+O",
            ),
            (&[B::SelectConfirm, B::SelectCancel], "Enter/Esc"),
            (&[B::PageDown], "PgDn"),
            (&[B::Dequeue], "Alt+Up"),
        ];
        for (bindings, hint) in cases {
            assert_eq!(hinted(&keymap, bindings).as_deref(), Some(hint));
        }
        // The user's keys, and actions left without any.
        let (keymap, _) = Keymap::read(
            r#"{"tui.select.up": ["ctrl+k", "up"], "tui.select.down": [],
                "app.message.dequeue": "ctrl+q", "app.tree.filter.noTools": []}"#,
        );
        assert_eq!(
            hinted(&keymap, &[B::SelectUp, B::SelectDown]).as_deref(),
            Some("Ctrl+K")
        );
        assert_eq!(hinted(&keymap, &[B::SelectDown]), None);
        assert_eq!(hinted(&keymap, &[B::Dequeue]).as_deref(), Some("Ctrl+Q"));
        assert_eq!(hinted(&keymap, &filters).as_deref(), Some("Ctrl+D/U"));
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
        let _scratch = Scratch(directory.clone());
        let settings = directory.join("settings.json");
        assert_eq!(path(&settings), directory.join(FILE));
        assert!(load(Some(&settings)).is_empty());
        std::fs::write(path(&settings), "{\"tui.select.up\": \"bogus\"}").unwrap();
        let problems = load(Some(&settings));
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
