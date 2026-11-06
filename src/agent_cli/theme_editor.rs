//! The theme editor in `/settings`: a theme made from a few main colors in a harmony, rolled
//! until it is close, its liked colors locked, stepped toward the harmony or toward more or less
//! contrast, with undo and redo, while a sample session shows it in the conversation's place.
//! It is saved as a theme file in Scramjet's format.

use super::keymap::{self, Binding};
use super::keys::Key;
use super::palette::{
    self, Built, CONTRAST_STEPS, Draft, HARMONIES, Kind, Oklch, Rgb, Rng, harmonies,
};
use super::record::Event;
use super::style::{self, RESET};
use super::text::{self, Wrap};
use super::theme::{self, Color, Mode, Theme, Token};
use scramjet_rs::agent::{Ending, StreamProgress, ToolDetail, UserCommand};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

/// The most main colors a theme has.
const MOST: usize = 5;
/// How far Left and Right turn a main color's hue.
const HUE_STEP: f64 = 5.0;
/// The note under the rows takes at most as many.
const NOTE_ROWS: usize = 3;

/// A row of the editor.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    Count,
    Harmony,
    Background,
    Contrast,
    Main(usize),
}

/// What a key ended the editor with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Ended {
    Save,
    Cancel,
}

pub(super) struct ThemeEditor {
    /// Each roll or step, after the one before; `at` is the one shown, and those after it what
    /// redo brings back.
    history: Vec<Draft>,
    at: usize,
    /// The main colors a roll keeps, which anchor the harmony.
    locked: Vec<bool>,
    row: usize,
    rng: Rng,
    /// Said in place of the keys' hint until the next key (`true`: a warning).
    status: Option<(String, bool)>,
    /// The draft's colors, and how well its main colors fit the harmony.
    built: Built,
    score: f64,
}

impl ThemeEditor {
    /// Three main colors in a triadic harmony, rolled from `seed`, on a dark or `light`
    /// background.
    pub(super) fn new(seed: u64, light: bool) -> Self {
        let mut rng = Rng::new(seed);
        let draft = Draft::rolled(harmonies(3)[0], light, &mut rng);
        let locked = vec![false; draft.mains.len()];
        Self {
            built: draft.build(),
            score: draft.fit(&locked).score,
            locked,
            history: vec![draft],
            at: 0,
            row: 0,
            rng,
            status: None,
        }
    }

    fn draft(&self) -> &Draft {
        &self.history[self.at]
    }

    /// The theme as it is now.
    pub(super) fn theme(&self) -> Theme {
        Theme::exact(&self.built.colors, self.built.background)
    }

    fn rows(&self) -> Vec<Row> {
        let mains = (0..self.draft().mains.len()).map(Row::Main);
        [Row::Count, Row::Harmony, Row::Background, Row::Contrast]
            .into_iter()
            .chain(mains)
            .collect()
    }

    /// Shows `draft` from now on, after the one shown, which undo goes back to.
    fn push(&mut self, draft: Draft) {
        self.history.truncate(self.at + 1);
        self.history.push(draft);
        self.show(self.at + 1);
    }

    /// Shows the draft at `at` in the history. The locks stay on their rows, or, when it has
    /// another number of colors, on the colors locked, which a change of the number keeps.
    fn show(&mut self, at: usize) {
        let before = std::mem::replace(&mut self.at, at);
        let (before, now) = (&self.history[before].mains, &self.history[at].mains);
        if before.len() != now.len() {
            let kept: Vec<_> = (before.iter().zip(&self.locked))
                .filter(|(_, locked)| **locked)
                .map(|(main, _)| main)
                .collect();
            self.locked = now.iter().map(|main| kept.contains(&main)).collect();
        }
        self.built = self.draft().build();
        self.rescore();
        self.row = self.row.min(self.rows().len() - 1);
    }

    fn rescore(&mut self) {
        self.score = self.draft().fit(&self.locked).score;
    }

    fn say(&mut self, status: &str, warning: bool) {
        self.status = Some((status.to_owned(), warning));
    }

    /// Changes the theme as `key` asks, or returns how it ended the editor.
    pub(super) fn key(&mut self, key: &Key) -> Option<Ended> {
        self.status = None;
        let rows = self.rows();
        let row = rows[self.row];
        match key {
            _ if key.is(Binding::SelectConfirm) => return Some(Ended::Save),
            _ if key.is(Binding::SelectCancel) => return Some(Ended::Cancel),
            _ if key.is(Binding::SelectUp) => self.row = (self.row + rows.len() - 1) % rows.len(),
            _ if key.is(Binding::SelectDown) => self.row = (self.row + 1) % rows.len(),
            _ if key.is(Binding::CursorLeft) => self.change(row, -1),
            _ if key.is(Binding::CursorRight) => self.change(row, 1),
            _ if key.is(Binding::ThemeRoll) => self.roll(),
            _ if key.is(Binding::ThemeLock) => match row {
                Row::Main(index) => {
                    self.locked[index] = !self.locked[index];
                    self.rescore();
                }
                _ => self.say("Select a main color to lock it", true),
            },
            _ if key.is(Binding::ThemeHarmonize) => self.harmonize(),
            _ if key.is(Binding::ThemeContrastUp) => self.change(Row::Contrast, 1),
            _ if key.is(Binding::ThemeContrastDown) => self.change(Row::Contrast, -1),
            _ if key.is(Binding::ThemeUndo) => match self.at {
                0 => self.say("Nothing to undo", true),
                at => self.show(at - 1),
            },
            _ if key.is(Binding::ThemeRedo) => match self.at + 1 < self.history.len() {
                true => self.show(self.at + 1),
                false => self.say("Nothing to redo", true),
            },
            _ => {}
        }
        None
    }

    /// Rolls the colors not locked, and the background.
    fn roll(&mut self) {
        let mut draft = self.draft().clone();
        draft.roll(&self.locked, &mut self.rng);
        if self.locked.iter().all(|&locked| locked) {
            self.say("Every color is locked: only the background rolled", false);
        }
        self.push(draft);
    }

    fn harmonize(&mut self) {
        let mut draft = self.draft().clone();
        match draft.harmonize(&self.locked) {
            true => self.push(draft),
            false if self.score >= 0.995 => self.say("Already in harmony", false),
            false => self.say("The locked colors keep it from fitting better", true),
        }
    }

    /// Changes `row`'s value a step: `by` -1 or 1.
    fn change(&mut self, row: Row, by: i32) {
        let mut draft = self.draft().clone();
        match row {
            Row::Count => {
                let count = draft.mains.len() as i32 + by;
                if !(1..=MOST as i32).contains(&count) {
                    return self.say(&format!("A theme has 1 to {MOST} main colors"), true);
                }
                let locked = self.locked.iter().filter(|&&locked| locked).count();
                if (count as usize) < locked {
                    return self.say(
                        &format!("{locked} colors are locked: unlock one first"),
                        true,
                    );
                }
                // The locked colors stay, first.
                let mut order: Vec<usize> = (0..draft.mains.len()).collect();
                order.sort_by_key(|&index| !self.locked[index]);
                draft.mains = order
                    .iter()
                    .map(|&index| draft.mains[index].clone())
                    .collect();
                let mut locked: Vec<bool> = order.iter().map(|&index| self.locked[index]).collect();
                draft.mains.truncate(count as usize);
                locked.resize(count as usize, false);
                draft.harmony = harmonies(count as usize)[0];
                draft.roll(&locked, &mut self.rng);
            }
            Row::Harmony => {
                let choices = harmonies(draft.mains.len());
                if choices.len() == 1 {
                    let only = HARMONIES[draft.harmony].name;
                    return self.say(
                        &format!("With this many colors, {only} is the only one"),
                        true,
                    );
                }
                let at = choices.iter().position(|&harmony| harmony == draft.harmony);
                let at = (at.unwrap_or(0) as i32 + by).rem_euclid(choices.len() as i32);
                draft.harmony = choices[at as usize];
                draft.roll(&self.locked, &mut self.rng);
            }
            Row::Background => draft.light = !draft.light,
            Row::Contrast => {
                let steps = draft.contrast + by;
                if steps.abs() > CONTRAST_STEPS {
                    let end = if by > 0 { "highest" } else { "lowest" };
                    return self.say(&format!("Contrast is at its {end}"), true);
                }
                draft.contrast = steps;
            }
            Row::Main(index) => {
                let main = &mut draft.mains[index];
                main.hue = (main.hue + HUE_STEP * f64::from(by)).rem_euclid(360.0);
            }
        }
        self.push(draft);
    }

    /// What each row shows, and the note under it when it is selected.
    fn row(&self, row: Row) -> (String, String, String) {
        let draft = self.draft();
        let left_right = keymap::keys(&[Binding::CursorLeft, Binding::CursorRight]);
        let left_right = left_right.as_deref().unwrap_or("Left and Right");
        match row {
            Row::Count => (
                "Colors".into(),
                draft.mains.len().to_string(),
                format!(
                    "How many main colors the theme is made of, 1 to {MOST}. {left_right} \
                     changes it and rolls the colors not locked."
                ),
            ),
            Row::Harmony => {
                let harmony = &HARMONIES[draft.harmony];
                let about = harmony.about;
                (
                    "Harmony".into(),
                    harmony.name.into(),
                    format!("{about} {left_right} chooses another, rolled anew."),
                )
            }
            Row::Background => (
                "Background".into(),
                if draft.light { "light" } else { "dark" }.into(),
                format!(
                    "Dark or light, tinted toward the first color; every color is lit to read \
                     well on it. {left_right} switches."
                ),
            ),
            Row::Contrast => (
                "Contrast".into(),
                match draft.contrast {
                    0 => "middle".to_owned(),
                    steps if steps == CONTRAST_STEPS => format!("+{steps}, the highest"),
                    steps if steps == -CONTRAST_STEPS => format!("{steps}, the lowest"),
                    steps if steps > 0 => format!("+{steps}"),
                    steps => steps.to_string(),
                },
                format!(
                    "How far text and colors stand out from the background, always within \
                     readable bounds; only their lightness changes. {left_right} steps it."
                ),
            ),
            Row::Main(index) => {
                let rgb = draft.accent(index, self.built.background);
                let (r, g, b) = rgb;
                let swatch = swatch(rgb);
                let name = palette::hue_name(Oklch::of(rgb));
                let lock = match self.locked[index] {
                    true => " · locked",
                    false => "",
                };
                let lock_key = keymap::key(Binding::ThemeLock).unwrap_or_default();
                (
                    format!("Color {}", index + 1),
                    format!("{swatch} #{r:02x}{g:02x}{b:02x} {name}{lock}"),
                    format!(
                        "{left_right} turns its hue a little. {lock_key} locks it: a roll keeps \
                         the locked colors and places the others in the harmony around them."
                    ),
                )
            }
        }
    }

    /// The harmony's score and the faintest text's contrast, in numbers and plain words.
    fn scores(&self) -> String {
        let score = self.score;
        let background = self.built.background;
        let ratio = |token| palette::contrast(self.built.color(token), background);
        let (text, dim) = (ratio(Token::Text), ratio(Token::Dim));
        format!(
            "Harmony {:.0}% {} · text {text:.1}:1 {} · dim text {dim:.1}:1 {}",
            score * 100.0,
            palette::harmony_words(score),
            palette::contrast_words(text, Kind::Text),
            palette::contrast_words(dim, Kind::Dim),
        )
    }

    /// The editor's rows for a dock of `(width, height)`: the title and keys, each row with
    /// its value, the scores, the keys that change the theme and the selected row's note, the
    /// note left out first when there is no room, then the keys.
    pub(super) fn lines(&self, (width, height): (usize, usize)) -> Vec<String> {
        use style::{MUTED, SELECTED, SELECTOR_DONE, SELECTOR_MARK, SELECTOR_TITLE};
        use style::{SELECTOR_WARNING, STATUS};
        let width = width.saturating_sub(1).max(8);
        let hint = keymap::hint([
            keymap::part(&[Binding::SelectUp, Binding::SelectDown], "move"),
            keymap::part(&[Binding::CursorLeft, Binding::CursorRight], "change"),
            keymap::part(&[Binding::SelectConfirm], "save"),
            keymap::part(&[Binding::SelectCancel], "cancel"),
        ]);
        let (status, status_style) = match &self.status {
            Some((status, true)) => (status.as_str(), SELECTOR_WARNING),
            Some((status, false)) => (status.as_str(), SELECTOR_DONE),
            None => (hint.as_str(), MUTED),
        };
        let title = "Theme editor";
        let status = text::clip(status, width.saturating_sub(title.width() + 2));
        let mut lines = vec![format!(
            "{SELECTOR_TITLE}{title}{RESET}  {status_style}{status}{RESET}"
        )];
        let rows = self.rows();
        let shown: Vec<_> = rows.iter().map(|&row| self.row(row)).collect();
        let label = shown
            .iter()
            .map(|(label, ..)| label.width())
            .max()
            .unwrap_or(0);
        for (at, (name, value, _)) in shown.iter().enumerate() {
            let text = match at == self.row {
                true => format!("{name:label$}  ‹ {value} ›"),
                false => format!("{name:label$}  {value}"),
            };
            let text = text::clip(&text, width - 2);
            lines.push(match at == self.row {
                true => {
                    let text = text.replace(RESET, &format!("{RESET}{SELECTED}"));
                    format!("{SELECTOR_MARK}›{RESET} {SELECTED}{text}{RESET}")
                }
                false => format!("  {text}"),
            });
        }
        let scores = text::clip(&self.scores(), width - 2);
        let keys = keymap::hint([
            keymap::part(&[Binding::ThemeRoll], "roll"),
            keymap::part(&[Binding::ThemeLock], "lock"),
            keymap::part(&[Binding::ThemeHarmonize], "toward the harmony"),
            keymap::part(
                &[Binding::ThemeContrastUp, Binding::ThemeContrastDown],
                "contrast",
            ),
            keymap::part(&[Binding::ThemeUndo], "undo"),
            keymap::part(&[Binding::ThemeRedo], "redo"),
        ]);
        let keys = text::clip(&keys, width - 2);
        let mut wrap = Wrap::new(width - 2);
        let note = wrap.push(&shown[self.row].2) + &wrap.finish();
        let note: Vec<&str> = note.split('\n').take(NOTE_ROWS).collect();
        let mut under = vec![format!("  {STATUS}{scores}{RESET}")];
        if height > lines.len() + 2 + note.len() + 1 {
            under.push(format!("  {MUTED}{keys}{RESET}"));
        }
        if height >= lines.len() + under.len() + 1 + note.len() {
            under.insert(0, String::new());
            under.extend(note.iter().map(|row| format!("  {MUTED}{row}{RESET}")));
        }
        lines.extend(under);
        lines.truncate(height.max(1));
        lines
    }

    /// The text of the theme's file, named `name`: the main colors under their hues' names,
    /// with the background and the text's shades, which the tokens refer to where they are
    /// theirs.
    pub(super) fn file_text(&self, name: &str) -> String {
        let draft = self.draft();
        let built = &self.built;
        let mut vars: Vec<(String, Rgb)> = Vec::new();
        let mut add = |name: &str, color: Rgb| {
            let mut unique = name.to_owned();
            let mut number = 2;
            while vars.iter().any(|(known, _)| *known == unique) {
                unique = format!("{name}{number}");
                number += 1;
            }
            vars.push((unique, color));
        };
        for index in 0..draft.mains.len() {
            let color = draft.accent(index, built.background);
            add(palette::hue_name(Oklch::of(color)), color);
        }
        add("background", built.background);
        for (name, token) in [
            ("text", Token::Text),
            ("muted", Token::Muted),
            ("dim", Token::Dim),
            ("faint", Token::BorderMuted),
        ] {
            add(name, built.color(token));
        }
        theme::file_text(name, &vars, &built.colors, built.background)
    }
}

/// A block of `color`, as the terminal shows it.
fn swatch((r, g, b): Rgb) -> String {
    let color = Color::Rgb(r, g, b).foreground(Mode::detect());
    format!("\x1b[{}m████{RESET}", color.unwrap_or_default())
}

/// A sample session with every element the theme colors, in parts as the screen keeps a
/// conversation: a turn that thinks, runs a failing command, edits, runs it again and replies
/// in markdown, a command the user ran and a notice.
pub(super) fn sample() -> Vec<Vec<Event>> {
    let start = Instant::now();
    let after = |seconds: f64| start + Duration::from_secs_f64(seconds);
    let reasoning = "**Looking at the splitter**\n\nThe test expects runs of spaces to collapse, \
                     so `split(' ')` must be the problem.\n\n";
    let failed = "test tests::splits_on_spaces ... FAILED\nassertion `left == right` failed\n\n\
                  Command exited with code 101";
    let diff = " 1 /// Splits `text` into words.\n 2 pub fn words(text: &str) -> Vec<&str> {\n\
                -3     text.split(' ').collect()\n+3     text.split_whitespace().collect()\n 4 }\n";
    let passed = "running 1 test\n.\ntest result: ok. 1 passed; 0 failed";
    let reply = "Fixed: `words` split on **every** space, so `\"a b  c\"` gave an *empty* word.\n\n\
                 ## What changed\n\n\
                 - `src/lib.rs` calls [split_whitespace](https://doc.rust-lang.org/std/primitive.str.html), \
                 which also skips tabs.\n\
                 - The test `splits_on_spaces` passes.\n\n\
                 ```rust\n/// Splits `text` into words.\npub fn words(text: &str) -> Vec<&str> {\n    \
                 text.split_whitespace().collect()\n}\n\n#[test]\nfn splits_on_spaces() {\n    \
                 assert_eq!(words(\"a b  c\").len(), 3);\n}\n```\n\n\
                 > Leading and trailing spaces are dropped too.";
    let call = |name: &str, summary: &str| Event::ToolCall(name.into(), summary.into(), None);
    let turn = vec![
        Event::UserPrompt("The words test fails. Fix it.".into()),
        Event::Progress(StreamProgress::Reasoning(reasoning.into())),
        call("bash", "cargo test -q"),
        Event::ToolRunning(after(0.0)),
        Event::ToolResult("bash".into(), failed.into(), None, true, after(1.4)),
        call("edit", "src/lib.rs"),
        Event::ToolRunning(after(1.5)),
        Event::ToolResult(
            "edit".into(),
            "Successfully replaced 1 block(s) in src/lib.rs.".into(),
            Some(ToolDetail::Diff(diff.into())),
            false,
            after(1.5),
        ),
        call("bash", "cargo test -q"),
        Event::ToolRunning(after(1.6)),
        Event::ToolResult("bash".into(), passed.into(), None, false, after(2.4)),
        Event::Progress(StreamProgress::Text(reply.into())),
        Event::Assistant(reply.into()),
    ];
    let command = UserCommand {
        command: "git status --short".into(),
        include: true,
        output: " M src/lib.rs\n".into(),
        ending: Ending::Done,
    };
    let notice = "/fix is waiting for you: it asked a question. Reply to continue.";
    vec![
        turn,
        vec![
            Event::CommandStarted(command.command.clone(), true),
            Event::CommandResult(command),
        ],
        vec![Event::Notice(notice.into())],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::keys::{Code, Press};
    use crate::agent_cli::text::plain;

    fn press(code: Code) -> Key {
        Key::Press(Press::new(code, 0))
    }

    fn typed(text: &str) -> Key {
        Key::Text(text.into())
    }

    fn lines(editor: &ThemeEditor) -> Vec<String> {
        editor
            .lines((100, 40))
            .iter()
            .map(|line| plain(line))
            .collect()
    }

    #[test]
    fn rolls_change_the_colors_not_locked_and_undo_and_redo_go_through_them() {
        let mut editor = ThemeEditor::new(1, false);
        let first = editor.draft().clone();
        let shown = lines(&editor);
        assert!(shown[0].starts_with("Theme editor  "), "{shown:?}");
        assert_eq!(shown[1], "› Colors      ‹ 3 ›");
        assert_eq!(shown[2], "  Harmony     triadic");
        assert!(shown[5].starts_with("  Color 1     ████ #"), "{shown:?}");
        // Down to the second color, locked; a roll keeps it.
        for _ in 0..5 {
            editor.key(&press(Code::Down));
        }
        assert_eq!(editor.key(&typed("l")), None);
        assert!(
            lines(&editor)[6].ends_with(" · locked ›"),
            "{:?}",
            lines(&editor)
        );
        editor.key(&typed("r"));
        let rolled = editor.draft().clone();
        assert_eq!(rolled.mains[1], first.mains[1]);
        assert_ne!(rolled.mains[0], first.mains[0]);
        assert_ne!(rolled.mains[2], first.mains[2]);
        assert_ne!(editor.theme(), ThemeEditor::new(1, false).theme());
        editor.key(&typed("u"));
        assert_eq!(*editor.draft(), first);
        editor.key(&typed("U"));
        assert_eq!(*editor.draft(), rolled);
        editor.key(&typed("U"));
        assert_eq!(editor.status, Some(("Nothing to redo".into(), true)));
        // A step after an undo drops what redo would bring back.
        editor.key(&typed("u"));
        editor.key(&typed("+"));
        assert_eq!(editor.draft().contrast, 1);
        editor.key(&typed("U"));
        assert_eq!(editor.status, Some(("Nothing to redo".into(), true)));
        assert_eq!(editor.key(&press(Code::Enter)), Some(Ended::Save));
        assert_eq!(editor.key(&press(Code::Escape)), Some(Ended::Cancel));
    }

    #[test]
    fn the_count_and_harmony_rows_roll_anew_and_keep_the_locked_colors() {
        let mut editor = ThemeEditor::new(2, false);
        // Lock the third color, then go down to two colors: it stays, as the first.
        editor.row = 6;
        editor.key(&typed("l"));
        let kept = editor.draft().mains[2].clone();
        editor.row = 0;
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().mains.len(), 2);
        assert_eq!(editor.draft().mains[0], kept);
        assert_eq!(editor.locked, [true, false]);
        assert_eq!(HARMONIES[editor.draft().harmony].name, "complementary");
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().mains, std::slice::from_ref(&kept));
        editor.key(&press(Code::Left));
        assert_eq!(
            editor.status.as_ref().unwrap().0,
            "A theme has 1 to 5 main colors"
        );
        editor.row = 1;
        editor.key(&press(Code::Right));
        assert!(
            editor
                .status
                .as_ref()
                .unwrap()
                .0
                .contains("monochrome is the only one")
        );
        editor.row = 0;
        for _ in 0..4 {
            editor.key(&press(Code::Right));
        }
        assert_eq!(editor.draft().mains.len(), 5);
        assert_eq!(editor.draft().mains[0], kept);
        editor.row = 1;
        editor.key(&press(Code::Right));
        assert_eq!(HARMONIES[editor.draft().harmony].name, "analogous");
        assert_eq!(editor.draft().mains[0], kept);
        // Back to the three colors of the start, the lock on the third again.
        for _ in 0..8 {
            editor.key(&typed("u"));
        }
        assert_eq!(editor.draft().mains.len(), 3);
        assert_eq!(editor.draft().mains[2], kept);
        assert_eq!(editor.locked, [false, false, true]);
    }

    #[test]
    fn stepping_toward_the_harmony_ends_in_harmony_and_contrast_stops_at_its_ends() {
        let mut editor = ThemeEditor::new(3, false);
        editor.row = 5;
        for _ in 0..8 {
            editor.key(&press(Code::Right));
        }
        assert!(editor.draft().fit(&editor.locked).score < 0.9);
        for _ in 0..30 {
            editor.key(&typed("h"));
        }
        assert_eq!(editor.status, Some(("Already in harmony".into(), false)));
        assert!(
            lines(&editor)
                .iter()
                .any(|line| line.contains("Harmony 100% in harmony"))
        );
        for _ in 0..CONTRAST_STEPS {
            editor.key(&typed("+"));
        }
        assert_eq!(editor.draft().contrast, CONTRAST_STEPS);
        editor.key(&typed("+"));
        assert_eq!(
            editor.status,
            Some(("Contrast is at its highest".into(), true))
        );
        editor.key(&typed("-"));
        assert_eq!(editor.draft().contrast, CONTRAST_STEPS - 1);
    }

    #[test]
    fn a_short_dock_leaves_out_the_note_then_the_keys() {
        let editor = ThemeEditor::new(4, true);
        let tall = lines(&editor);
        assert!(
            tall.iter()
                .any(|line| line.contains("How many main colors"))
        );
        assert!(tall.iter().any(|line| line.contains("R roll · L lock")));
        let short: Vec<String> = editor.lines((100, 9)).iter().map(|l| plain(l)).collect();
        assert_eq!(short.len(), 9);
        assert!(short.last().unwrap().contains("Harmony "), "{short:?}");
        let shorter: Vec<String> = editor.lines((100, 4)).iter().map(|l| plain(l)).collect();
        assert_eq!(shorter.len(), 4);
    }

    #[test]
    fn the_saved_file_is_the_theme_shown_in_scramjets_format() {
        let mut editor = ThemeEditor::new(5, false);
        editor.key(&typed("r"));
        let text = editor.file_text("sunset \"two\"");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["name"], "sunset \"two\"");
        assert_eq!(json["colors"]["background"], "background");
        assert_eq!(json["colors"]["text"], "text");
        assert_eq!(json["colors"]["muted"], "muted");
        let vars = json["vars"].as_object().unwrap();
        assert_eq!(vars.len(), 3 + 5);
        assert_eq!(Theme::parse(&text).unwrap(), editor.theme());
    }

    #[test]
    fn the_sample_session_shows_a_failed_and_a_passing_call_a_diff_and_a_reply() {
        let parts = sample();
        assert_eq!(parts.len(), 3);
        let results: Vec<bool> = (parts[0].iter())
            .filter_map(|event| match event {
                Event::ToolResult(_, _, _, failed, _) => Some(*failed),
                _ => None,
            })
            .collect();
        assert_eq!(results, [true, false, false]);
        assert!(parts[0].iter().any(|event| matches!(
            event,
            Event::ToolResult(_, _, Some(ToolDetail::Diff(_)), ..)
        )));
    }
}
