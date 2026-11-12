//! `/hotkeys`, as Scramjet's and Codex's `/keymap`: every key, and on the full screen each
//! action's keys changed (a new key pressed, another added, one removed, the defaults restored)
//! and saved in the keybindings file at once; `/keybindings`, as Claude Code's, opens that file.

use super::fuzzy::Haystack;
use super::interactive::Interactive;
use super::keymap::{self, Binding, shown_keys};
use super::keys::Press;
use super::select::{self, Picker, Spot};
use super::text::clip;
use super::{editor, style};
use scramjet::agent::{shortened, write_replacing};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

const EXAMPLE: &str = "{\"app.message.followUp\": \"ctrl+q\"}";

/// What a change does to an action's keys.
#[derive(Clone, Copy)]
enum Change {
    /// The key pressed takes the place of them all.
    Replace,
    Add,
    Remove(Press),
    Defaults,
}

/// `/hotkeys`' rows: the headings and under each the shortcuts, with what each list needs.
struct Rows {
    keys: Vec<String>,
    what: Vec<&'static str>,
    texts: Vec<Haystack>,
    headings: Vec<bool>,
    notes: Vec<String>,
    bindings: Vec<Option<Binding>>,
}

impl Rows {
    fn now() -> Self {
        let mut rows = Self {
            keys: Vec::new(),
            what: Vec::new(),
            texts: Vec::new(),
            headings: Vec::new(),
            notes: Vec::new(),
            bindings: Vec::new(),
        };
        for (heading, shortcuts) in keymap::listing() {
            rows.keys.push(String::new());
            rows.what.push(heading);
            rows.texts.push(Haystack::new(""));
            rows.headings.push(true);
            rows.notes.push(String::new());
            rows.bindings.push(None);
            for shortcut in shortcuts {
                let note = match shortcut.binding {
                    Some(binding) => format!(
                        "{}\n{}, by default {}",
                        shortcut.what,
                        keymap::id(binding),
                        shown_keys(&keymap::defaults_of(binding))
                    ),
                    None => format!("{}\nNot an action of its own: it stays.", shortcut.what),
                };
                let id = shortcut.binding.map_or("", keymap::id);
                let text = format!("{} {} {id}", shortcut.keys, shortcut.what);
                rows.texts.push(Haystack::new(&text));
                rows.keys.push(shortcut.keys);
                rows.what.push(shortcut.what);
                rows.headings.push(false);
                rows.notes.push(note);
                rows.bindings.push(shortcut.binding);
            }
        }
        rows
    }

    /// Row `index` in `width` columns: its keys in a column as wide as the widest, at most half
    /// the row, then what they do; a heading's text.
    fn shown(&self, index: usize, width: usize) -> String {
        if self.headings[index] {
            return clip(self.what[index], width);
        }
        let widest = self.keys.iter().map(|keys| keys.width()).max().unwrap_or(0);
        let column = widest.min(width / 2);
        let keys = clip(&self.keys[index], column);
        let pad = " ".repeat(column.saturating_sub(keys.width()));
        clip(&format!("{keys}{pad}  {}", self.what[index]), width)
    }
}

impl Interactive {
    /// `/hotkeys`: on the full screen, every shortcut in a list to search, where Enter changes an
    /// action's keys, the list then showing again with its query and the row; else, or without a
    /// settings file to save them beside, the list printed.
    pub(super) fn hotkeys(&mut self) {
        let settings = self.session.settings.path().map(Path::to_path_buf);
        let (Some(_), Some(settings)) = (&self.screen, settings) else {
            let text = printed(self.session.settings.path());
            return self.say(text.trim_end());
        };
        let mut place: Option<(Spot, usize)> = None;
        let mut status = None;
        loop {
            let Some(screen) = &self.screen else {
                return;
            };
            let mut rows = Rows::now();
            let texts = std::mem::take(&mut rows.texts);
            let visible = (editor::terminal_size().1 / 2).max(select::VISIBLE);
            let picker = Picker::search(
                "Keyboard shortcuts",
                visible,
                texts,
                1,
                None,
                "No matching shortcut",
                |index, width| rows.shown(index, width),
            );
            let mut picker = (picker.in_order().headed(rows.headings.clone()))
                .noting(select::hint("change", &[], "close"), rows.notes.clone())
                .held();
            if let Some((spot, index)) = place.take() {
                picker = picker.at(spot, status.take()).on_row(index);
            }
            let Some((index, spot)) = screen.pick_placed(picker) else {
                break;
            };
            place = Some((spot, index));
            if let Some(binding) = rows.bindings[index] {
                status = self.change_keys(&settings, binding);
            }
        }
        if let Some(screen) = &self.screen {
            screen.dismiss();
        }
    }

    /// The changes to `binding`'s keys, the one chosen saved beside `settings`; what to say on
    /// the list after it: that it was saved, or why not. `None`: none was made.
    fn change_keys(&mut self, settings: &Path, binding: Binding) -> Option<(String, bool)> {
        loop {
            let screen = self.screen.as_ref()?;
            let keys = keymap::keys_of(binding);
            let defaults = keymap::defaults_of(binding);
            let now = shown_keys(&keys);
            let mut changes = match keys.is_empty() {
                true => vec![(Change::Replace, "Press a key".to_owned(), String::new())],
                false => vec![
                    (
                        Change::Replace,
                        "Press a new key".to_owned(),
                        format!("In place of {now}."),
                    ),
                    (
                        Change::Add,
                        "Press another key".to_owned(),
                        format!("Besides {now}."),
                    ),
                ],
            };
            for &key in &keys {
                let note = match keys.len() {
                    1 => "It is left without keys.",
                    _ => "The others stay.",
                };
                let label = format!("Remove {}", keymap::shown(key));
                changes.push((Change::Remove(key), label, note.to_owned()));
            }
            if keys != defaults {
                let label = format!("Restore the default: {}", shown_keys(&defaults));
                let note = "Takes its entry out of the keybindings file.".to_owned();
                changes.push((Change::Defaults, label, note));
            }
            let title = format!("{}: {now}", keymap::description(binding));
            let notes = changes.iter().map(|(.., note)| note.clone()).collect();
            let picker = Picker::select(
                &title,
                select::VISIBLE,
                changes.len(),
                0,
                None,
                |index, width| clip(&changes[index].1, width),
            );
            let picker = picker.noting(select::hint("choose", &[], "back"), notes);
            let index = screen.pick(picker.held())?;
            let mut chosen = match changes[index].0 {
                Change::Replace => vec![],
                Change::Add => keys.clone(),
                Change::Remove(key) => (keys.iter().copied()).filter(|&k| k != key).collect(),
                Change::Defaults => defaults,
            };
            if let Change::Replace | Change::Add = changes[index].0 {
                let adding = matches!(changes[index].0, Change::Add);
                let Some(press) = self.new_key(binding, &keys, adding) else {
                    continue;
                };
                chosen.push(press);
            }
            if let Err(error) = keymap::save(settings, binding, &chosen) {
                return Some((format!("Not saved: {error}"), true));
            }
            self.keybindings = keymap::load(Some(settings));
            let file = shortened(&keymap::path(settings).display().to_string());
            return Some((format!("Saved in {file}"), false));
        }
    }

    /// The key pressed for `binding`, which has `keys`, once what else it means where `binding`
    /// acts, if anything, is accepted; `None` when cancelled.
    fn new_key(&self, binding: Binding, keys: &[Press], adding: bool) -> Option<Press> {
        let screen = self.screen.as_ref()?;
        let title = match adding {
            true => "Press another key",
            false => "Press the new key",
        };
        let asked = format!(
            "For {}, now {}.",
            keymap::description(binding),
            shown_keys(keys)
        );
        let mut note = asked.clone();
        loop {
            let press = screen.press(title, &note)?;
            let key = keymap::shown(press);
            if keymap::name(press).is_none() {
                note = format!("{key} cannot be saved: the file names only ASCII characters.");
                continue;
            }
            if adding && keys.contains(&press) {
                note = format!("{key} is one of its keys already.");
                continue;
            }
            let also = keymap::conflicts(binding, press);
            if also.is_empty() {
                return Some(press);
            }
            let warning = format!(
                "{key} already means: {}. Where they act together, only one of them does.",
                also.join("; ")
            );
            let options = ["Press another key".to_owned(), format!("Use {key} anyway")];
            let picker = Picker::select(
                &format!("{key} is taken"),
                select::VISIBLE,
                options.len(),
                0,
                None,
                |index, width| clip(&options[index], width),
            );
            let picker = picker.noting(select::hint("choose", &[], "back"), vec![warning; 2]);
            match screen.pick(picker.held())? {
                0 => note = asked.clone(),
                _ => return Some(press),
            }
        }
    }

    /// `/keybindings`, as Claude Code's: opens the keybindings file in the user's editor, written
    /// as `{}` first when there is none (and removed again when left so), and reads it again
    /// once the editor exits. Without the full screen, it says where the file is.
    pub(super) fn keybindings(&mut self) {
        let Some(path) = self.session.settings.path().map(keymap::path) else {
            return self.say("Error: no keybindings file: HOME is not set");
        };
        let shown = shortened(&path.display().to_string());
        if self.screen.is_none() {
            return self.say(&format!(
                "Keys go in {shown}, as in Scramjet 1's keybindings.json: {EXAMPLE}. \
                 /reload reads it again."
            ));
        }
        let created = !path.exists();
        if created && let Err(error) = write_replacing(&path, "{}\n") {
            return self.say(&format!("Error: {shown}: {error}"));
        }
        let edited = self.screen.as_ref().map(|screen| screen.edit(&path));
        if created && std::fs::read_to_string(&path).is_ok_and(|text| text == "{}\n") {
            let _ = std::fs::remove_file(&path);
        }
        if let Some(Err(error)) = edited {
            return self.say(&format!("Error: {error}"));
        }
        self.read_keybindings(&path);
    }

    /// Reads the keybindings file at `path` again, saying what is wrong in it.
    fn read_keybindings(&mut self, path: &Path) {
        self.keybindings = keymap::load(self.session.settings.path());
        if self.keybindings.is_empty() {
            let shown = shortened(&path.display().to_string());
            return self.say_styled(style::MUTED, &format!("Keys read from {shown}"));
        }
        for problem in self.keybindings.clone() {
            self.ui().notice(&problem.text);
        }
    }
}

/// `/hotkeys` printed, as Scramjet's: the keys by section, in two columns, and the file that
/// changes them beside the `settings` file.
fn printed(settings: Option<&Path>) -> String {
    let mut text = format!(
        "{}\n",
        style::mark(style::INFO_HEADING, "Keyboard Shortcuts")
    );
    for (section, keys) in keymap::listing() {
        let widest = (keys.iter()).map(|row| row.keys.width()).max().unwrap_or(0);
        let what = (keys.iter()).map(|row| row.what.width()).max().unwrap_or(0);
        // Keys that would push a row past 80 columns wrap at their ` / `.
        let width = widest.min(76usize.saturating_sub(what)).max(1);
        text += &format!("\n{}\n", style::mark(style::INFO_HEADING, section));
        for row in keys {
            let mut what = row.what;
            for line in wrapped(&row.keys, width) {
                let pad = " ".repeat(width.saturating_sub(line.width()));
                let line = style::mark(style::CODE, &line);
                text += format!("  {line}{pad}  {what}").trim_end();
                text.push('\n');
                what = "";
            }
        }
    }
    if let Some(settings) = settings {
        let file = shortened(&keymap::path(settings).display().to_string());
        text += &format!(
            "\nOther keys for an action go in {file}, as in Scramjet 1's keybindings.json: \
             {EXAMPLE}. /reload reads it again.\n"
        );
    }
    text
}

/// `keys` in lines of at most `width` columns, broken after a ` /`; a single key wider stays whole.
fn wrapped(keys: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for (index, key) in keys.split(" / ").enumerate() {
        let line = lines.last_mut().expect("one line at least");
        if index > 0 && line.width() + 3 + key.width() > width {
            line.push_str(" /");
            lines.push(key.to_owned());
        } else {
            if index > 0 {
                line.push_str(" / ");
            }
            line.push_str(key);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_hotkeys_list_every_action_with_its_keys_in_columns_within_80() {
        let settings = Path::new("/config/scramjet-rs/settings.json");
        let text = style::themed(&printed(Some(settings)), false);
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines[..3], ["Keyboard Shortcuts", "", "Navigation"]);
        // Each section's actions start in one column, after its widest keys.
        let row = |width: usize, key: &str, action: &str| format!("  {key:<width$}  {action}");
        assert_eq!(lines[3], row(30, "Up", "Move cursor up / browse history"));
        for line in [
            row(30, "Home / Ctrl+A", "Start of line"),
            row(14, "Ctrl+Shift+P", "Cycle to the previous model"),
            row(
                14,
                "Escape Escape",
                "Open /tree on an empty prompt (doubleEscapeAction)",
            ),
            row(30, "Shift+L", "Label the entry"),
            row(
                21,
                "Right click",
                "Paste text into the prompt (with no selection)",
            ),
        ] {
            assert!(lines.contains(&line.as_str()), "{line:?} in {text}");
        }
        let rows: Vec<_> = lines.iter().filter(|line| line.starts_with("  ")).collect();
        let listed: usize = keymap::listing().iter().map(|(_, rows)| rows.len()).sum();
        let wrapped = (rows.iter())
            .filter(|row| !row.trim_start().contains("  "))
            .count();
        assert_eq!(rows.len() - wrapped, listed);
        let at = |start: &str| {
            lines
                .iter()
                .position(|line| line.starts_with(start))
                .unwrap()
        };
        let word = at("  Ctrl+W / Alt+Backspace /");
        assert_eq!(
            lines[word],
            row(32, "Ctrl+W / Alt+Backspace /", "Delete word backwards")
        );
        assert_eq!(lines[word + 1], "  Ctrl+Backspace");
        assert_eq!(
            lines[word + 2],
            row(
                32,
                "Alt+D / Alt+Delete / Ctrl+Delete",
                "Delete word forwards"
            )
        );
        for row in rows {
            assert!(row.width() <= 80, "{row}");
        }
        assert!(
            lines.last().unwrap().starts_with(&format!(
                "Other keys for an action go in {}, as in",
                settings.with_file_name("keybindings.json").display()
            )),
            "{text}"
        );
    }

    #[test]
    fn the_lists_rows_put_keys_in_a_column_of_at_most_half_the_row() {
        let rows = Rows::now();
        assert_eq!((rows.what[0], rows.headings[0]), ("Navigation", true));
        assert_eq!(rows.bindings[1], Some(Binding::CursorUp));
        assert_eq!(rows.shown(0, 40), "Navigation");
        let widest = rows.keys.iter().map(|keys| keys.width()).max().unwrap();
        let wide = rows.shown(1, 100);
        assert_eq!(
            wide,
            format!("{:widest$}  Move cursor up / browse history", "Up")
        );
        assert_eq!(
            rows.shown(1, 40),
            format!("{:20}  Move cursor up / …", "Up")
        );
        let word_left = (rows.bindings.iter())
            .position(|&binding| binding == Some(Binding::CursorWordLeft))
            .unwrap();
        assert!(
            rows.shown(word_left, 40)
                .starts_with("Alt+Left / Ctrl+Lef…  ")
        );
        assert_eq!(
            rows.notes[1],
            "Move cursor up / browse history\ntui.editor.cursorUp, by default Up"
        );
        // The fixed rows are listed, with no action to change.
        let mouse = (rows.what.iter())
            .position(|&what| what == "Mouse")
            .unwrap();
        assert!(rows.headings[mouse] && rows.bindings[mouse + 1].is_none());
    }
}
