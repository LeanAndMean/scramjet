//! The theme editor in `/settings`: a theme made from a few main colors in a harmony, rolled
//! until it is close, its liked colors locked, stepped toward the harmony or toward more or less
//! contrast, and each of its roles, one view away, set to follow another main color or to an
//! exact color, with undo and redo, while a sample session shows it in the conversation's place.
//! It is saved as a theme file in Scramjet's format, which keeps what the editor made it from, so
//! that it opens in the editor again.

use super::keymap::{self, Binding};
use super::keys::Key;
use super::palette::{
    self, Background, Built, CONTRAST_STEPS, Draft, HARMONIES, Kind, Meaning, Oklch, Pick, Rgb,
    Rng, Source, harmonies,
};
use super::record::Event;
use super::style::{self, RESET};
use super::text::{self, Wrap};
use super::theme::{self, Color, KEYS, Mode, TOKENS, Theme, Token};
use scramjet_rs::agent::{Ending, StreamProgress, ToolDetail, UserCommand};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

/// The most main colors a theme has.
const MOST: usize = 5;
/// How far Left and Right turn a main color's hue.
const HUE_STEP: f64 = 5.0;
/// The note under the rows takes at most as many.
const NOTE_ROWS: usize = 3;
/// The roles' view shows at most as many at a time.
const ROLE_ROWS: usize = 10;
/// The key of a theme file under which it keeps what the editor made it from.
const STATE: &str = "themeEditor";

/// What the editor shows: the main colors and what makes the theme of them, or every role.
#[derive(Clone, Copy, Debug, PartialEq)]
enum View {
    Main,
    Roles,
}

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
    view: View,
    /// The role selected in the roles' view, an index into `TOKENS`, and the hex digits of an
    /// exact color being typed for it.
    role: usize,
    typing: Option<String>,
    rng: Rng,
    /// The terminal's own background, when it is known: a theme can be made on it.
    terminal: Option<Rgb>,
    /// Said in place of the keys' hint until the next key (`true`: a warning).
    status: Option<(String, bool)>,
    /// The draft's colors, and how well its main colors fit the harmony.
    built: Built,
    score: f64,
}

impl ThemeEditor {
    /// Three main colors in a triadic harmony, rolled from `seed`, on `background`; `terminal`
    /// is the terminal's own, when known.
    pub(super) fn new(seed: u64, background: Background, terminal: Option<Rgb>) -> Self {
        let mut rng = Rng::new(seed);
        let draft = Draft::rolled(harmonies(3)[0], background, &mut rng);
        let locked = vec![false; draft.mains.len()];
        Self::on(draft, locked, rng, terminal)
    }

    /// The editor on the theme file `text` as it was saved from the editor: its main colors and
    /// locks, harmony, background, contrast and picked roles, the roles changed in the file
    /// since then set to their colors there. On the terminal's background, it is `terminal`'s
    /// when known, else the one it was saved on. `None` when the editor did not make the file.
    pub(super) fn open(text: &str, seed: u64, terminal: Option<Rgb>) -> Option<Self> {
        let file = Theme::parse(text).ok()?;
        let json: Value = serde_json::from_str(text).ok()?;
        let (mut draft, locked) = saved_draft(json.get(STATE)?)?;
        let built = draft.build();
        for (at, &token) in TOKENS.iter().enumerate() {
            if let Color::Rgb(r, g, b) = file.color(token)
                && built.colors[at] != (r, g, b)
            {
                draft.picks[at] = Some(Pick::Exact((r, g, b)));
            }
        }
        let terminal = match draft.background {
            Background::Terminal(saved) => {
                let now = terminal.unwrap_or(saved);
                draft.background = Background::Terminal(now);
                Some(now)
            }
            _ => terminal,
        };
        Some(Self::on(draft, locked, Rng::new(seed), terminal))
    }

    fn on(draft: Draft, locked: Vec<bool>, rng: Rng, terminal: Option<Rgb>) -> Self {
        Self {
            built: draft.build(),
            score: draft.fit(&locked).score,
            locked,
            history: vec![draft],
            at: 0,
            row: 0,
            view: View::Main,
            role: 0,
            typing: None,
            rng,
            terminal,
            status: None,
        }
    }

    fn draft(&self) -> &Draft {
        &self.history[self.at]
    }

    /// The theme as it is now.
    pub(super) fn theme(&self) -> Theme {
        Theme::exact(&self.built.colors, self.built.own_background())
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
        if self.typing.is_some() {
            self.type_color(key);
            return None;
        }
        match key {
            _ if key.is(Binding::SelectConfirm) => return Some(Ended::Save),
            _ if key.is(Binding::SelectCancel) => return Some(Ended::Cancel),
            _ if key.is(Binding::ThemeRoles) => {
                self.view = match self.view {
                    View::Main => View::Roles,
                    View::Roles => View::Main,
                }
            }
            _ if key.is(Binding::ThemeRoll) => self.roll(),
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
            _ => match self.view {
                View::Main => self.main_key(key),
                View::Roles => self.roles_key(key),
            },
        }
        None
    }

    fn main_key(&mut self, key: &Key) {
        let rows = self.rows();
        let row = rows[self.row];
        match key {
            _ if key.is(Binding::SelectUp) => self.row = (self.row + rows.len() - 1) % rows.len(),
            _ if key.is(Binding::SelectDown) => self.row = (self.row + 1) % rows.len(),
            _ if key.is(Binding::CursorLeft) => self.change(row, -1),
            _ if key.is(Binding::CursorRight) => self.change(row, 1),
            _ if key.is(Binding::ThemeLock) => match row {
                Row::Main(index) => {
                    self.locked[index] = !self.locked[index];
                    self.rescore();
                }
                _ => self.say("Select a main color to lock it", true),
            },
            _ => {}
        }
    }

    fn roles_key(&mut self, key: &Key) {
        let count = TOKENS.len();
        match key {
            _ if key.is(Binding::SelectUp) => self.role = (self.role + count - 1) % count,
            _ if key.is(Binding::SelectDown) => self.role = (self.role + 1) % count,
            _ if key.is(Binding::SelectFirst) => self.role = 0,
            _ if key.is(Binding::SelectLast) => self.role = count - 1,
            _ if key.is(Binding::CursorLeft) => self.follow(-1),
            _ if key.is(Binding::CursorRight) => self.follow(1),
            _ if key.is(Binding::ThemeExact) => self.typing = Some(String::new()),
            Key::Paste(_) => {
                self.typing = Some(String::new());
                self.type_color(key);
            }
            _ if key.is(Binding::ThemeReset) => match self.draft().picks[self.role] {
                None => self.say("It follows its own color already", false),
                Some(_) => self.pick(None),
            },
            _ if key.is(Binding::ThemeLock) => {
                let roles = keymap::key(Binding::ThemeRoles).unwrap_or_default();
                self.say(
                    &format!("Lock main colors in the first view ({roles})"),
                    true,
                );
            }
            _ => {}
        }
    }

    /// Sets the selected role's pick, as a step undo goes back from.
    fn pick(&mut self, pick: Option<Pick>) {
        let mut draft = self.draft().clone();
        draft.picks[self.role] = pick;
        self.push(draft);
    }

    /// Makes the selected role follow the next main color, `by` -1 or 1, or its own again: the
    /// main colors other than its own, in turn, after its own.
    fn follow(&mut self, by: i32) {
        let (_, own) = palette::own(TOKENS[self.role]);
        let mut choices = vec![None];
        let others = (0..self.draft().mains.len()).filter(|&index| own != Source::Main(index));
        choices.extend(others.map(|index| Some(Pick::Follow(index))));
        if choices.len() == 1 {
            return self.say("With one main color, it can follow only that one", true);
        }
        let count = choices.len() as i32;
        let at = match choices
            .iter()
            .position(|&choice| choice == self.draft().picks[self.role])
        {
            Some(at) => (at as i32 + by).rem_euclid(count),
            None if by > 0 => 0,
            None => count - 1,
        };
        self.pick(choices[at as usize]);
    }

    /// Takes `key` toward the exact color being typed: its hex digits, Backspace, Enter to set
    /// it, Escape to leave it.
    fn type_color(&mut self, key: &Key) {
        let mut typed = self.typing.take().unwrap_or_default();
        match key {
            Key::Text(text) | Key::Paste(text) => {
                let text = text.trim();
                let text = text.strip_prefix('#').unwrap_or(text);
                let fits = text.chars().all(|c| c.is_ascii_hexdigit());
                if fits && typed.len() + text.len() <= 6 {
                    typed.push_str(&text.to_ascii_lowercase());
                } else {
                    self.say("A color is six hex digits, such as ff8800", true);
                }
            }
            _ if key.is(Binding::DeleteCharBackward) => {
                typed.pop();
            }
            _ if key.is(Binding::SelectConfirm) => match hex(&typed) {
                Some(rgb) => return self.pick(Some(Pick::Exact(rgb))),
                None => self.say("Type six hex digits, or three", true),
            },
            _ if key.is(Binding::SelectCancel) => return,
            _ => {}
        }
        self.typing = Some(typed);
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
                // Roles following a main color follow it where it went, or their own if it went.
                for pick in &mut draft.picks {
                    if let Some(Pick::Follow(index)) = *pick {
                        let to = order.iter().position(|&from| from == index);
                        *pick = (to.filter(|&to| to < count as usize)).map(Pick::Follow);
                    }
                }
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
            Row::Background => {
                let choices: Vec<Background> = (self.terminal.map(Background::Terminal))
                    .into_iter()
                    .chain([Background::Dark, Background::Light])
                    .collect();
                let kind = std::mem::discriminant(&draft.background);
                let at = (choices.iter())
                    .position(|choice| std::mem::discriminant(choice) == kind)
                    .unwrap_or(0);
                let at = (at as i32 + by).rem_euclid(choices.len() as i32);
                draft.background = choices[at as usize];
            }
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
                match draft.background {
                    Background::Dark => "dark".to_owned(),
                    Background::Light => "light".to_owned(),
                    Background::Terminal((r, g, b)) => {
                        format!("the terminal's, #{r:02x}{g:02x}{b:02x}")
                    }
                },
                match self.terminal {
                    Some(_) => format!(
                        "The terminal's own, which the theme leaves as it is, or dark or light, \
                         tinted toward the first color; every color is lit to read well on it. \
                         {left_right} switches."
                    ),
                    None => format!(
                        "Dark or light, tinted toward the first color; every color is lit to \
                         read well on it. The terminal did not say what its own background is, \
                         so no theme can be made on it. {left_right} switches."
                    ),
                },
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

    /// The harmony's score, the faintest text's contrast, in numbers and plain words, and how
    /// many roles were picked.
    fn scores(&self) -> String {
        let score = self.score;
        let background = self.built.background;
        let ratio = |token| palette::contrast(self.built.color(token), background);
        let (text, dim) = (ratio(Token::Text), ratio(Token::Dim));
        let picked = match self.draft().picks.iter().flatten().count() {
            0 => String::new(),
            1 => " · 1 role picked".to_owned(),
            count => format!(" · {count} roles picked"),
        };
        format!(
            "Harmony {:.0}% {} · text {text:.1}:1 {} · dim text {dim:.1}:1 {}{picked}",
            score * 100.0,
            palette::harmony_words(score),
            palette::contrast_words(text, Kind::Text),
            palette::contrast_words(dim, Kind::Dim),
        )
    }

    /// What the role at `at` takes its color from, in plain words.
    fn follows(&self, at: usize) -> String {
        let draft = self.draft();
        let (kind, own) = palette::own(TOKENS[at]);
        let count = draft.mains.len();
        let named = |source| match source {
            Source::Main(slot) => {
                let main = slot % count + 1;
                match (kind, slot < count) {
                    (Kind::Accent, true) => format!("color {main}"),
                    (Kind::Accent, false) => format!("a shade of color {main}"),
                    _ => format!("a tint of color {main}"),
                }
            }
            Source::Meaning(meaning) => meaning.words().to_owned(),
        };
        match draft.picks[at] {
            None => named(own),
            Some(Pick::Follow(index)) => named(Source::Main(index)) + ", picked",
            Some(Pick::Exact(_)) => "set by hand".to_owned(),
        }
    }

    /// The row of the role at `at`: its name, its color, its contrast with the background in
    /// its kind's words and what it follows, which the keys change, or the color being typed.
    fn role_row(&self, at: usize, selected: bool) -> String {
        let name = KEYS.iter().map(|key| key.len()).max().unwrap_or(0);
        let rgb = self.built.colors[at];
        let (r, g, b) = rgb;
        let (kind, _) = palette::own(TOKENS[at]);
        let ratio = palette::contrast(rgb, self.built.background);
        let words = palette::contrast_words(ratio, kind);
        let source = match (&self.typing, selected) {
            (Some(typed), true) => {
                let cursor = style::SEARCH_CURSOR;
                let typed_swatch =
                    hex(typed).map_or(String::new(), |rgb| format!(" {}", swatch(rgb)));
                format!("‹ #{typed}{cursor} {RESET}{typed_swatch} ›")
            }
            (None, true) => format!("‹ {} ›", self.follows(at)),
            _ => self.follows(at),
        };
        format!(
            "{:name$}  {} #{r:02x}{g:02x}{b:02x}  {ratio:>4.1}:1 {words:11}  {source}",
            KEYS[at],
            swatch(rgb),
        )
    }

    /// The note under the roles: what the selected one colors and what the keys do with it.
    fn role_note(&self) -> String {
        if self.typing.is_some() {
            return "Type the color as six hex digits, such as ff8800, or three, such as f80. \
                    Its contrast shows once it is set; undo takes it back."
                .to_owned();
        }
        let token = TOKENS[self.role];
        let range = match palette::own(token).1 {
            Source::Meaning(meaning) => {
                let hues = match meaning {
                    Meaning::Error => "errors' reds",
                    Meaning::Warning => "warnings' ambers",
                    Meaning::Success => "success's greens",
                };
                format!(" Its hue stays within {hues} unless it follows a main color.")
            }
            Source::Main(_) => String::new(),
        };
        let left_right = keymap::keys(&[Binding::CursorLeft, Binding::CursorRight]);
        let left_right = left_right.as_deref().unwrap_or("Left and Right");
        let exact = keymap::key(Binding::ThemeExact).unwrap_or_default();
        let reset = keymap::key(Binding::ThemeReset).unwrap_or_default();
        format!(
            "{}{range} {left_right} picks the main color it follows, {exact} an exact color, \
             {reset} its own again.",
            about(token)
        )
    }

    /// The editor's rows for a dock of `(width, height)`: the title and keys, the main colors'
    /// rows each with its value or a window of the roles' rows, the scores, the keys that change
    /// the theme and the selected row's note, the note left out first when there is no room,
    /// then the keys.
    pub(super) fn lines(&self, (width, height): (usize, usize)) -> Vec<String> {
        use style::{MUTED, SELECTED, SELECTOR_DONE, SELECTOR_MARK, SELECTOR_TITLE};
        use style::{SELECTOR_WARNING, STATUS};
        let width = width.saturating_sub(1).max(8);
        let part = keymap::part;
        let hint = match (self.view, &self.typing) {
            (View::Main, _) => keymap::hint([
                part(&[Binding::SelectUp, Binding::SelectDown], "move"),
                part(&[Binding::CursorLeft, Binding::CursorRight], "change"),
                part(&[Binding::ThemeRoles], "every role"),
                part(&[Binding::SelectConfirm], "save"),
                part(&[Binding::SelectCancel], "cancel"),
            ]),
            (View::Roles, None) => keymap::hint([
                part(&[Binding::SelectUp, Binding::SelectDown], "move"),
                part(&[Binding::CursorLeft, Binding::CursorRight], "follow"),
                part(&[Binding::ThemeExact], "exact"),
                part(&[Binding::ThemeRoles], "back"),
                part(&[Binding::SelectConfirm], "save"),
                part(&[Binding::SelectCancel], "cancel"),
            ]),
            (View::Roles, Some(_)) => keymap::hint([
                part(&[Binding::SelectConfirm], "set the color"),
                part(&[Binding::SelectCancel], "leave it"),
            ]),
        };
        let (status, status_style) = match &self.status {
            Some((status, true)) => (status.as_str(), SELECTOR_WARNING),
            Some((status, false)) => (status.as_str(), SELECTOR_DONE),
            None => (hint.as_str(), MUTED),
        };
        let title = match self.view {
            View::Main => "Theme editor".to_owned(),
            View::Roles => format!("Theme editor · role {} of {}", self.role + 1, TOKENS.len()),
        };
        let status = text::clip(status, width.saturating_sub(title.width() + 2));
        let mut lines = vec![format!(
            "{SELECTOR_TITLE}{title}{RESET}  {status_style}{status}{RESET}"
        )];
        let row = |text: &str, selected: bool| {
            let text = text::clip(text, width - 2);
            match selected {
                true => {
                    let text = text.replace(RESET, &format!("{RESET}{SELECTED}"));
                    format!("{SELECTOR_MARK}›{RESET} {SELECTED}{text}{RESET}")
                }
                false => format!("  {text}"),
            }
        };
        let mut wrap = Wrap::new(width - 2);
        let note = match self.view {
            View::Main => {
                let rows = self.rows();
                let shown: Vec<_> = rows.iter().map(|&row| self.row(row)).collect();
                let label = (shown.iter())
                    .map(|(label, ..)| label.width())
                    .max()
                    .unwrap_or(0);
                for (at, (name, value, _)) in shown.iter().enumerate() {
                    let text = match at == self.row {
                        true => format!("{name:label$}  ‹ {value} ›"),
                        false => format!("{name:label$}  {value}"),
                    };
                    lines.push(row(&text, at == self.row));
                }
                wrap.push(&shown[self.row].2) + &wrap.finish()
            }
            View::Roles => {
                let note = wrap.push(&self.role_note()) + &wrap.finish();
                // The title and the scores, then the keys, then the blank row and the note,
                // each once the rows above them have room enough.
                let full = 4 + note.split('\n').take(NOTE_ROWS).count();
                let room = match height {
                    height if height >= full + 5 => height - full,
                    height if height >= 6 => height - 3,
                    height => height.saturating_sub(2),
                };
                let room = room.clamp(1, ROLE_ROWS);
                let first = (self.role.saturating_sub(room / 2)).min(TOKENS.len() - room);
                for at in first..first + room {
                    lines.push(row(&self.role_row(at, at == self.role), at == self.role));
                }
                note
            }
        };
        let note: Vec<&str> = note.split('\n').take(NOTE_ROWS).collect();
        let scores = text::clip(&self.scores(), width - 2);
        let keys = keymap::hint([
            part(&[Binding::ThemeRoll], "roll"),
            match self.view {
                View::Main => part(&[Binding::ThemeLock], "lock"),
                View::Roles => None,
            },
            part(&[Binding::ThemeHarmonize], "toward the harmony"),
            part(
                &[Binding::ThemeContrastUp, Binding::ThemeContrastDown],
                "contrast",
            ),
            part(&[Binding::ThemeUndo], "undo"),
            part(&[Binding::ThemeRedo], "redo"),
        ]);
        let keys = text::clip(&keys, width - 2);
        let mut under = vec![format!("  {STATUS}{scores}{RESET}")];
        if height >= lines.len() + 2 {
            under.push(format!("  {MUTED}{keys}{RESET}"));
        }
        if height >= lines.len() + 3 + note.len() {
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
        if let Some(background) = built.own_background() {
            add("background", background);
        }
        for (name, token) in [
            ("text", Token::Text),
            ("muted", Token::Muted),
            ("dim", Token::Dim),
            ("faint", Token::BorderMuted),
        ] {
            add(name, built.color(token));
        }
        let state = self.state();
        let extra = Some((STATE, &state));
        theme::file_text(name, &vars, &built.colors, built.own_background(), extra)
    }

    /// What the theme is made from, as its file keeps it: the main colors with their locks,
    /// numbered from 1 where a role follows one, and the roles picked by their keys.
    fn state(&self) -> Value {
        let draft = self.draft();
        let mains: Vec<Value> = (draft.mains.iter().zip(&self.locked))
            .map(|(main, locked)| {
                json!({"hue": main.hue, "chroma": main.chroma, "lift": main.lift, "locked": locked})
            })
            .collect();
        let picks: Map<String, Value> = (KEYS.iter().zip(&draft.picks))
            .filter_map(|(key, pick)| {
                let pick = match (*pick)? {
                    Pick::Follow(index) => json!({"follow": index + 1}),
                    Pick::Exact(rgb) => json!(hex_text(rgb)),
                };
                Some(((*key).to_owned(), pick))
            })
            .collect();
        let mut state = json!({
            "harmony": HARMONIES[draft.harmony].name,
            "background": match draft.background {
                Background::Dark => "dark",
                Background::Light => "light",
                Background::Terminal(_) => "terminal",
            },
            "depth": draft.depth,
            "tint": draft.tint,
            "contrast": draft.contrast,
            "mains": mains,
            "picks": picks,
        });
        if let Background::Terminal(rgb) = draft.background {
            state["terminal"] = json!(hex_text(rgb));
        }
        state
    }
}

/// The draft and locks a theme file keeps (`ThemeEditor::state`); `None` unless every part of
/// it is one the editor makes.
fn saved_draft(state: &Value) -> Option<(Draft, Vec<bool>)> {
    let unit = |value: &Value| value.as_f64().filter(|value| (0.0..=1.0).contains(value));
    let name = state.get("harmony")?.as_str()?;
    let harmony = HARMONIES.iter().position(|harmony| harmony.name == name)?;
    let mut mains = Vec::new();
    let mut locked = Vec::new();
    for main in state.get("mains")?.as_array()? {
        let hue = main.get("hue")?.as_f64().filter(|hue| hue.is_finite())?;
        let chroma = main
            .get("chroma")?
            .as_f64()
            .filter(|c| (0.0..=0.4).contains(c))?;
        mains.push(palette::Main {
            hue: hue.rem_euclid(360.0),
            chroma,
            lift: unit(main.get("lift")?)?,
        });
        locked.push(main.get("locked").and_then(Value::as_bool).unwrap_or(false));
    }
    if mains.len() != HARMONIES[harmony].count() {
        return None;
    }
    let background = match state.get("background")?.as_str()? {
        "dark" => Background::Dark,
        "light" => Background::Light,
        "terminal" => {
            Background::Terminal(hex(state.get("terminal")?.as_str()?.strip_prefix('#')?)?)
        }
        _ => return None,
    };
    let contrast = state.get("contrast")?.as_i64()?;
    if contrast.abs() > i64::from(CONTRAST_STEPS) {
        return None;
    }
    let mut picks = vec![None; TOKENS.len()];
    for (key, pick) in state.get("picks")?.as_object()? {
        let at = KEYS.iter().position(|known| known == key)?;
        picks[at] = Some(match pick {
            Value::String(text) => Pick::Exact(hex(text.strip_prefix('#')?)?),
            pick => {
                let main = pick.get("follow")?.as_u64()? as usize;
                (1..=mains.len()).contains(&main).then_some(())?;
                Pick::Follow(main - 1)
            }
        });
    }
    let draft = Draft {
        harmony,
        background,
        depth: unit(state.get("depth")?)?,
        tint: unit(state.get("tint")?)?,
        contrast: contrast as i32,
        picks,
        mains,
    };
    Some((draft, locked))
}

fn hex_text((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The color of hex digits typed: six, or three, each doubled.
fn hex(digits: &str) -> Option<Rgb> {
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let digits = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect(),
        6 => digits.to_owned(),
        _ => return None,
    };
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// What `token` colors here, in a line, Scramjet's meaning where scramjet-rs does not show it.
#[rustfmt::skip]
fn about(token: Token) -> &'static str {
    use Token as T;
    match token {
        T::Accent => "The main accent: the spinner, the mark of a list's selected row, the banner's name, a question's title.",
        T::Border => "Borders: the rules around a question the model asks.",
        T::BorderAccent => "Highlighted borders: compactions in /tree.",
        T::BorderMuted => "Quiet borders: the rails beside reasoning and calls in the looks other than the classic one, the card banner's border.",
        T::Success => "Success: calls and subagents that went well, the mark of the current session or model, models switched on.",
        T::Error => "Errors: failed calls, commands and subagents, a full context, what a list's key cannot do.",
        T::Warning => "Warnings: retries, notices, running calls and subagents, a filling context, labels and named sessions.",
        T::Muted => "Secondary text: the status line, notes such as Took 1.4s, hints, next steps, headings in lists.",
        T::Dim => "Faint text: the footer, the banner's keys and lists, the tree lines in lists, models switched off.",
        T::Text => "Text: replies, and everything without a color of its own.",
        T::ThinkingText => "The reasoning a model shows.",
        T::SelectedBg => "Behind the selected row in /resume and /tree.",
        T::UserMessageBg => "Behind your prompts, in the panels look.",
        T::UserMessageText => "Your prompts, in the looks other than the classic one.",
        T::CustomMessageBg => "Behind extensions' messages, in Scramjet; scramjet-rs does not show it.",
        T::CustomMessageText => "Compaction summaries.",
        T::CustomMessageLabel => "Extensions' message labels, in Scramjet; scramjet-rs does not show it.",
        T::ToolPendingBg => "Behind a call while it runs, in the panels look.",
        T::ToolSuccessBg => "Behind a call that went well, in the panels look.",
        T::ToolErrorBg => "Behind a call that failed, in the panels look.",
        T::ToolTitle => "A subagent's name, and a call's tool in the looks other than the classic one.",
        T::ToolOutput => "What a call returned, under it.",
        T::MdHeading => "Headings in replies, and the banner's.",
        T::MdLink => "A link's text in replies.",
        T::MdLinkUrl => "A link's address, after its text.",
        T::MdCode => "Code in a line of text.",
        T::MdCodeBlock => "A code block where it is not highlighted.",
        T::MdCodeBlockBorder => "The fence lines around a code block.",
        T::MdQuote => "Quoted text in replies.",
        T::MdQuoteBorder => "The bar before quoted text.",
        T::MdHr => "Horizontal rules in replies.",
        T::MdListBullet => "List bullets and numbers.",
        T::ToolDiffAdded => "Added lines in a diff.",
        T::ToolDiffRemoved => "Removed lines in a diff.",
        T::ToolDiffContext => "The unchanged lines around a diff's changes.",
        T::SyntaxComment => "Comments in highlighted code, and a diff's headers.",
        T::SyntaxKeyword => "Keywords in highlighted code.",
        T::SyntaxFunction => "Function names in highlighted code.",
        T::SyntaxVariable => "Variables, parameters and keys in highlighted code.",
        T::SyntaxString => "Strings in highlighted code.",
        T::SyntaxNumber => "Numbers, and values such as true, in highlighted code.",
        T::SyntaxType => "Types and built-ins in highlighted code.",
        T::SyntaxOperator => "Operators in Scramjet's highlighted code; scramjet-rs leaves them plain.",
        T::SyntaxPunctuation => "Punctuation in Scramjet's highlighted code; scramjet-rs leaves it plain.",
        T::ThinkingOff => "The rules above and below the prompt with thinking off, or for a model that does not reason.",
        T::ThinkingMinimal => "The rules above and below the prompt at minimal thinking.",
        T::ThinkingLow => "The rules above and below the prompt at low thinking.",
        T::ThinkingMedium => "The rules above and below the prompt at medium thinking.",
        T::ThinkingHigh => "The rules above and below the prompt at high thinking.",
        T::ThinkingXhigh => "The rules above and below the prompt at extra high and max thinking.",
        T::BashMode => "Commands you run with !: the prompt's rules while it starts with !, the rules around their output and the command's line.",
        T::SpellcheckError => "Misspelled words in the prompt, in Scramjet; scramjet-rs does not check spelling yet.",
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
        let mut editor = ThemeEditor::new(1, Background::Dark, None);
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
        assert_ne!(
            editor.theme(),
            ThemeEditor::new(1, Background::Dark, None).theme()
        );
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
        let mut editor = ThemeEditor::new(2, Background::Dark, None);
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
        let mut editor = ThemeEditor::new(3, Background::Dark, None);
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
        let editor = ThemeEditor::new(4, Background::Light, None);
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
        let mut editor = ThemeEditor::new(5, Background::Dark, None);
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
    fn a_theme_on_the_terminals_background_leaves_it_to_the_terminal() {
        let campbell = (12, 12, 12);
        let mut editor = ThemeEditor::new(11, Background::Terminal(campbell), Some(campbell));
        editor.row = 2;
        let shown = lines(&editor);
        assert_eq!(shown[3], "› Background  ‹ the terminal's, #0c0c0c ›");
        assert!(
            (shown.iter()).any(|line| line.contains("The terminal's own, which the theme leaves"))
        );
        // Its colors are lit and scored on it, and the theme and its file leave it out.
        assert_eq!(editor.built.background, campbell);
        assert_eq!(editor.theme().background(), Color::Default);
        let text = editor.file_text("on-terminal");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(json["colors"].get("background").is_none(), "{text}");
        assert!(json["vars"].get("background").is_none(), "{text}");
        assert_eq!(Theme::parse(&text).unwrap(), editor.theme());
        // Right goes to dark, light, then the terminal's again.
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().background, Background::Dark);
        assert_ne!(editor.theme().background(), Color::Default);
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().background, Background::Light);
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().background, Background::Terminal(campbell));
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().background, Background::Light);
        // Unknown, it is not offered, and the note says why.
        let mut editor = ThemeEditor::new(11, Background::Dark, None);
        editor.row = 2;
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().background, Background::Light);
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().background, Background::Dark);
        let shown = lines(&editor).join(" ");
        let shown = shown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(shown.contains("The terminal did not say"), "{shown}");
    }

    fn token_at(token: Token) -> usize {
        TOKENS.iter().position(|&t| t == token).unwrap()
    }

    #[test]
    fn every_role_can_follow_another_main_color_and_its_own_again() {
        let mut editor = ThemeEditor::new(6, Background::Dark, None);
        editor.key(&press(Code::Tab));
        let shown = lines(&editor);
        assert!(
            shown[0].starts_with("Theme editor · role 1 of 52  "),
            "{shown:?}"
        );
        assert!(shown[1].starts_with("› accent  "), "{shown:?}");
        assert!(shown[1].ends_with("‹ color 1 ›"), "{shown:?}");
        assert!(shown.iter().any(|line| line.contains("The main accent")));
        // The heading follows color 3; Right makes it follow color 1, then color 2, then its own.
        editor.role = token_at(Token::MdHeading);
        let accent =
            |editor: &ThemeEditor, index| editor.draft().accent(index, editor.built.background);
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().picks[editor.role], Some(Pick::Follow(0)));
        assert_eq!(editor.built.color(Token::MdHeading), accent(&editor, 0));
        assert!(
            lines(&editor)
                .iter()
                .any(|line| line.contains("‹ color 1, picked ›"))
        );
        assert!(
            lines(&editor)
                .iter()
                .any(|line| line.contains("1 role picked"))
        );
        editor.key(&press(Code::Right));
        assert_eq!(editor.built.color(Token::MdHeading), accent(&editor, 1));
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().picks[editor.role], None);
        assert_eq!(editor.built.color(Token::MdHeading), accent(&editor, 2));
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().picks[editor.role], Some(Pick::Follow(1)));
        // A roll gives the colors anew; the role follows color 2 as it is now.
        editor.key(&typed("r"));
        assert_eq!(editor.built.color(Token::MdHeading), accent(&editor, 1));
        editor.key(&press(Code::Backspace));
        assert_eq!(editor.draft().picks[editor.role], None);
        editor.key(&press(Code::Backspace));
        assert_eq!(
            editor.status,
            Some(("It follows its own color already".into(), false))
        );
        editor.key(&typed("u"));
        assert_eq!(editor.draft().picks[editor.role], Some(Pick::Follow(1)));
        // Locks are for the main colors' view.
        editor.key(&typed("l"));
        assert!(
            editor
                .status
                .as_ref()
                .unwrap()
                .0
                .contains("first view (Tab)")
        );
        // Up from the first role goes round to the last; End and Home go there too.
        editor.key(&press(Code::Home));
        editor.key(&press(Code::Up));
        assert_eq!(TOKENS[editor.role], Token::SpellcheckError);
        editor.key(&press(Code::Tab));
        assert!(lines(&editor)[0].starts_with("Theme editor  "));
    }

    #[test]
    fn a_color_with_a_meaning_leaves_its_hue_range_when_it_follows_a_main_color() {
        let mut editor = ThemeEditor::new(7, Background::Dark, None);
        editor.view = View::Roles;
        editor.role = token_at(Token::Error);
        assert!(
            lines(&editor)
                .iter()
                .any(|line| line.contains("‹ red, for errors ›"))
        );
        assert!(
            editor
                .role_note()
                .contains("Its hue stays within errors' reds unless")
        );
        // A main color far from red: the error color goes with it.
        let mut draft = editor.draft().clone();
        draft.mains[0].hue = 250.0;
        draft.picks[editor.role] = Some(Pick::Follow(0));
        editor.push(draft);
        let hue = Oklch::of(editor.built.color(Token::Error)).h;
        assert!((hue - 250.0).abs() < 5.0, "{hue}");
        // With one main color, the diff's added lines follow it or their own.
        editor.view = View::Main;
        editor.row = 0;
        editor.key(&press(Code::Left));
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().mains.len(), 1);
        editor.view = View::Roles;
        editor.role = token_at(Token::ToolDiffAdded);
        editor.key(&press(Code::Right));
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().picks[editor.role], None);
        editor.role = token_at(Token::Accent);
        editor.key(&press(Code::Right));
        assert_eq!(
            editor.status.as_ref().unwrap().0,
            "With one main color, it can follow only that one"
        );
    }

    #[test]
    fn a_role_set_to_an_exact_color_keeps_it_through_rolls_and_into_the_file() {
        let mut editor = ThemeEditor::new(8, Background::Dark, None);
        editor.key(&press(Code::Tab));
        editor.role = token_at(Token::SyntaxNumber);
        editor.key(&typed("#"));
        editor.key(&typed("F8"));
        assert!(
            lines(&editor).iter().any(|line| line.contains("‹ #f8  ›")),
            "{:?}",
            lines(&editor)
        );
        // Not a hex digit, or a seventh: refused, and what was typed stays.
        editor.key(&typed("g"));
        assert!(editor.status.as_ref().unwrap().0.contains("six hex digits"));
        editor.key(&press(Code::Enter));
        assert_eq!(
            editor.status,
            Some(("Type six hex digits, or three".into(), true))
        );
        editor.key(&typed("0"));
        // Keys that change the theme are typed text meanwhile.
        editor.key(&typed("r"));
        assert_eq!(editor.typing.as_deref(), Some("f80"));
        editor.key(&press(Code::Enter));
        assert_eq!(editor.typing, None);
        let orange = (0xff, 0x88, 0x00);
        assert_eq!(editor.built.color(Token::SyntaxNumber), orange);
        assert!(
            lines(&editor)
                .iter()
                .any(|line| line.contains("‹ set by hand ›"))
        );
        editor.key(&typed("r"));
        assert_eq!(editor.built.color(Token::SyntaxNumber), orange);
        // A pasted color, `#` and all; Escape leaves one being typed, and the editor stays.
        editor.role = token_at(Token::SyntaxString);
        editor.key(&Key::Paste("#1E90FF\n".into()));
        assert_eq!(editor.typing.as_deref(), Some("1e90ff"));
        editor.key(&press(Code::Backspace));
        assert_eq!(editor.key(&press(Code::Escape)), None);
        assert_eq!(editor.typing, None);
        assert_eq!(editor.draft().picks[editor.role], None);
        let text = editor.file_text("exact");
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["colors"]["syntaxNumber"], "#ff8800");
        assert_eq!(Theme::parse(&text).unwrap(), editor.theme());
    }

    #[test]
    fn roles_following_a_main_color_follow_it_where_a_change_of_count_moves_it() {
        let mut editor = ThemeEditor::new(9, Background::Dark, None);
        let (heading, link) = (token_at(Token::MdHeading), token_at(Token::MdLink));
        let mut draft = editor.draft().clone();
        draft.picks[heading] = Some(Pick::Follow(2));
        draft.picks[link] = Some(Pick::Follow(0));
        editor.push(draft);
        // The third color locked goes first when there are two; the first is dropped.
        editor.row = 6;
        editor.key(&typed("l"));
        let kept = editor.draft().mains[2].clone();
        editor.row = 0;
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().mains[0], kept);
        assert_eq!(editor.draft().picks[heading], Some(Pick::Follow(0)));
        let first = editor.draft().accent(0, editor.built.background);
        assert_eq!(editor.built.color(Token::MdHeading), first);
        assert_eq!(editor.draft().picks[link], Some(Pick::Follow(1)));
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().picks[link], None);
    }

    #[test]
    fn the_roles_view_shows_a_window_around_the_selected_role() {
        let mut editor = ThemeEditor::new(10, Background::Dark, None);
        editor.view = View::Roles;
        editor.role = token_at(Token::MdCode);
        let shown = lines(&editor);
        let roles: Vec<&String> = (shown.iter())
            .filter(|line| line.contains("████ #"))
            .collect();
        assert_eq!(roles.len(), ROLE_ROWS, "{shown:?}");
        assert!(roles[ROLE_ROWS / 2].starts_with("› mdCode "), "{roles:?}");
        assert!(
            shown
                .iter()
                .any(|line| line.contains("Code in a line of text."))
        );
        editor.role = TOKENS.len() - 1;
        let shown = lines(&editor);
        let last = (shown.iter())
            .rfind(|line| line.contains("████ #"))
            .unwrap();
        assert!(last.starts_with("› spellcheckError "), "{shown:?}");
        let short: Vec<String> = editor.lines((100, 8)).iter().map(|l| plain(l)).collect();
        assert_eq!(short.len(), 8);
        assert!(
            short[1..6].iter().all(|line| line.contains("████ #")),
            "{short:?}"
        );
        assert!(short[7].contains("R roll"), "{short:?}");
        for (at, &token) in TOKENS.iter().enumerate() {
            let note = about(token);
            assert!(note.ends_with('.') && note.len() < 130, "{}", KEYS[at]);
        }
    }

    #[test]
    fn a_saved_theme_opens_as_it_was_saved_with_the_roles_changed_in_its_file() {
        let mut editor = ThemeEditor::new(12, Background::Dark, None);
        editor.row = 0;
        editor.key(&press(Code::Right));
        editor.row = 5;
        editor.key(&typed("l"));
        editor.key(&typed("r"));
        editor.key(&typed("+"));
        let mut draft = editor.draft().clone();
        draft.picks[token_at(Token::MdHeading)] = Some(Pick::Follow(3));
        draft.picks[token_at(Token::Error)] = Some(Pick::Exact((0xff, 0x22, 0x11)));
        editor.push(draft);
        let text = editor.file_text("mine");
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json[STATE]["mains"][1]["locked"], true);
        assert_eq!(json[STATE]["picks"]["mdHeading"]["follow"], 4);
        assert_eq!(json[STATE]["picks"]["error"], "#ff2211");
        let opened = ThemeEditor::open(&text, 1, None).unwrap();
        assert_eq!(opened.draft(), editor.draft());
        assert_eq!(opened.locked, editor.locked);
        assert_eq!(opened.theme(), editor.theme());
        assert_eq!(opened.at, 0);
        assert_eq!(opened.file_text("mine"), text);
        // A role changed by hand in the file is set to its color there; the others still follow.
        let changed = text.replace("\"mdLink\": ", "\"mdLink\": \"#123456\", \"_was\": ");
        let opened = ThemeEditor::open(&changed, 1, None).unwrap();
        let link = token_at(Token::MdLink);
        assert_eq!(
            opened.draft().picks[link],
            Some(Pick::Exact((0x12, 0x34, 0x56)))
        );
        let mut expected = editor.draft().clone();
        expected.picks[link] = Some(Pick::Exact((0x12, 0x34, 0x56)));
        assert_eq!(*opened.draft(), expected);
        // Themes the editor did not make, or whose state it would not make, do not open.
        let built_in = include_str!("themes/scramjet-dark.json");
        assert!(ThemeEditor::open(built_in, 1, None).is_none());
        for (from, to) in [
            ("\"harmony\": \"", "\"harmony\": \"no "),
            ("\"contrast\": 1", "\"contrast\": 4"),
            ("\"follow\": 4", "\"follow\": 5"),
            ("\"#ff2211\"", "\"#ff22é\""),
            ("\"mdHeading\": {", "\"mdHeadline\": {"),
        ] {
            assert!(text.contains(from), "{from}");
            let broken = text.replacen(from, to, 1);
            assert!(ThemeEditor::open(&broken, 1, None).is_none(), "{to}");
        }
    }

    #[test]
    fn a_theme_on_the_terminals_background_opens_on_the_terminals_as_it_is_now() {
        let (campbell, solarized) = ((12, 12, 12), (0, 43, 54));
        let editor = ThemeEditor::new(13, Background::Terminal(campbell), Some(campbell));
        let text = editor.file_text("on-terminal");
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json[STATE]["terminal"], "#0c0c0c");
        // Its colors are lit for the terminal's background now, none taken as changed by hand.
        let opened = ThemeEditor::open(&text, 1, Some(solarized)).unwrap();
        assert_eq!(opened.draft().background, Background::Terminal(solarized));
        assert_eq!(opened.terminal, Some(solarized));
        assert!(opened.draft().picks.iter().all(Option::is_none));
        assert_ne!(opened.theme(), editor.theme());
        // A terminal that does not say: the one it was saved on.
        let opened = ThemeEditor::open(&text, 1, None).unwrap();
        assert_eq!(opened.terminal, Some(campbell));
        assert_eq!(opened.theme(), editor.theme());
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
