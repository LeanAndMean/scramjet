//! The theme editor in `/settings`: a theme made from a few main colors in a harmony, rolled
//! until it is close, its liked colors locked, stepped toward the harmony, toward more or less
//! contrast or a lighter or darker background, and each of its roles, one view away, set to
//! follow another main color or to an exact color, with undo and redo, while a sample session
//! shows it in the conversation's place. It can start from another theme's colors. It is saved
//! as a theme file in Scramjet's format, which keeps what the editor made it from, so that it
//! opens in the editor again.

use super::keymap::{self, Binding};
use super::keys::Key;
use super::palette::{
    self, Background, Built, CONTRAST_STEPS, Draft, HARMONIES, Kind, LIGHTNESS_STEPS, Meaning,
    Oklch, Pick, Rgb, Rng, Source, harmonies,
};
use super::record::Event;
use super::style::{self, Band, RESET};
use super::text::{self, Wrap};
use super::theme::{self, Color, KEYS, Mode, TOKENS, Theme, Token};
use scramjet_rs::agent::{Compaction, Ending, StreamProgress, ToolDetail, UserCommand};
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
/// Colors nearer each other than this in OKLab are taken as one main color.
const SAME: f64 = 0.02;

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
    Lightness,
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
    /// A background of the theme's own, which it started on, offered beside the others.
    own: Option<Rgb>,
    /// The theme it started from, whose colors roles keep until they are lit anew.
    from: Option<String>,
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
        let state = json.get(STATE)?;
        let (mut draft, locked) = saved_draft(state)?;
        let built = draft.build();
        for (at, &token) in TOKENS.iter().enumerate() {
            if let Some(rgb) = rgb(file.color(token))
                && built.colors[at] != rgb
            {
                draft.picks[at] = Some(Pick::Exact(rgb));
            }
        }
        // A background changed in the file is kept, the roles not changed there lit on it.
        let terminal = match (draft.background, rgb(file.background())) {
            (_, Some(own)) if built.own_background() != Some(own) => {
                draft.background = Background::Own(own);
                terminal
            }
            (Background::Terminal(saved), _) => {
                let now = terminal.unwrap_or(saved);
                draft.background = Background::Terminal(now);
                Some(now)
            }
            _ => terminal,
        };
        let mut editor = Self::on(draft, locked, Rng::new(seed), terminal);
        editor.from = state.get("from").and_then(Value::as_str).map(str::to_owned);
        Some(editor)
    }

    /// The editor on a new theme starting from `theme`, named `name`: its main colors those of
    /// its accent, border, headings, variables and strings, the distinct ones in turn, on its
    /// background, else the terminal's, else dark or light as its secondary text suits; each
    /// role keeps `theme`'s color until the color it would follow changes. `None` when it has
    /// none of those colors exactly, as the terminal's 16 colors until the terminal says what
    /// they are (`Theme::exactly`).
    pub(super) fn from_theme(
        name: &str,
        theme: &Theme,
        seed: u64,
        terminal: Option<Rgb>,
    ) -> Option<Self> {
        let of = |token| rgb(theme.color(token));
        let mut accents: Vec<Rgb> = Vec::new();
        for token in [
            Token::Accent,
            Token::Border,
            Token::MdHeading,
            Token::SyntaxVariable,
            Token::SyntaxString,
        ] {
            if let Some(color) = of(token)
                && (accents.iter()).all(|&known| palette::distance(known, color) > SAME)
            {
                accents.push(color);
            }
        }
        if accents.is_empty() {
            return None;
        }
        let background = match (rgb(theme.background()), terminal) {
            (Some(own), _) => Background::Own(own),
            (None, Some(terminal)) => Background::Terminal(terminal),
            _ if of(Token::Muted).is_some_and(palette::is_dark) => Background::Light,
            _ => Background::Dark,
        };
        let mut draft = Draft::of_accents(&accents, background);
        let built = draft.build();
        for (at, &token) in TOKENS.iter().enumerate() {
            draft.picks[at] =
                (of(token).filter(|&color| color != built.colors[at])).map(Pick::Kept);
        }
        let locked = vec![false; accents.len()];
        let mut editor = Self::on(draft, locked, Rng::new(seed), terminal);
        editor.from = Some(name.to_owned());
        editor.say(
            &format!("From {name}: its colors stay until what they follow changes hue"),
            false,
        );
        Some(editor)
    }

    fn on(draft: Draft, locked: Vec<bool>, rng: Rng, terminal: Option<Rgb>) -> Self {
        Self {
            own: match draft.background {
                Background::Own(own) => Some(own),
                _ => None,
            },
            from: None,
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
        [
            Row::Count,
            Row::Harmony,
            Row::Background,
            Row::Lightness,
            Row::Contrast,
        ]
        .into_iter()
        .chain(mains)
        .collect()
    }

    /// Shows `draft` from now on, after the one shown, which undo goes back to.
    fn push(&mut self, mut draft: Draft) {
        let released = draft.carry(self.draft());
        self.history.truncate(self.at + 1);
        self.history.push(draft);
        self.show(self.at + 1);
        if released > 0 && self.status.is_none() {
            let from = self.started_from();
            let roles = match released {
                1 => "1 role follows".to_owned(),
                count => format!("{count} roles follow"),
            };
            self.say(&format!("{roles} the main colors now, not {from}'s"), false);
        }
    }

    fn started_from(&self) -> &str {
        self.from.as_deref().unwrap_or("the theme it started from")
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
            }
            Row::Background => {
                let choices: Vec<Background> = (self.terminal.map(Background::Terminal))
                    .into_iter()
                    .chain([Background::Dark, Background::Light])
                    .chain(self.own.map(Background::Own))
                    .collect();
                let kind = std::mem::discriminant(&draft.background);
                let at = (choices.iter())
                    .position(|choice| std::mem::discriminant(choice) == kind)
                    .unwrap_or(0);
                let at = (at as i32 + by).rem_euclid(choices.len() as i32);
                draft.background = choices[at as usize];
            }
            Row::Lightness => {
                let steps = draft.lightness + by;
                if steps.abs() > LIGHTNESS_STEPS {
                    let end = if by > 0 { "lightest" } else { "darkest" };
                    return self.say(&format!("The background is at its {end}"), true);
                }
                draft.lightness = steps;
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
                let toward = keymap::key(Binding::ThemeHarmonize).unwrap_or_default();
                (
                    "Harmony".into(),
                    harmony.name.into(),
                    format!(
                        "{about} {left_right} chooses another and keeps the colors; {toward} \
                         steps them toward it."
                    ),
                )
            }
            Row::Background => (
                "Background".into(),
                match draft.background {
                    Background::Dark => "dark".to_owned(),
                    Background::Light => "light".to_owned(),
                    Background::Terminal(rgb) => format!("the terminal's, {}", hex_text(rgb)),
                    Background::Own(rgb) => format!("its own, {}", hex_text(rgb)),
                },
                {
                    let own = match self.own {
                        Some(_) => ", or its own, which it started on",
                        None => "",
                    };
                    match self.terminal {
                        Some(_) => format!(
                            "The terminal's own, which the theme leaves as it is, or dark or \
                             light, tinted toward the first color{own}; every color is lit to \
                             read well on it. {left_right} switches."
                        ),
                        None => format!(
                            "Dark or light, tinted toward the first color{own}; every color is \
                             lit to read well on it. The terminal did not say what its own \
                             background is, so no theme can be made on it. {left_right} \
                             switches."
                        ),
                    }
                },
            ),
            Row::Lightness => (
                "Lightness".into(),
                match draft.lightness {
                    0 => "as rolled".to_owned(),
                    steps if steps == LIGHTNESS_STEPS => format!("+{steps}, the lightest"),
                    steps if steps == -LIGHTNESS_STEPS => format!("{steps}, the darkest"),
                    steps if steps > 0 => format!("+{steps}, lighter"),
                    steps => format!("{steps}, darker"),
                },
                {
                    let on = match draft.background {
                        Background::Terminal(_) => {
                            " A step from the terminal's own makes the theme set its own."
                        }
                        _ => "",
                    };
                    format!(
                        "How much lighter or darker the background is than the one rolled or \
                         chosen; rolls keep the steps, and every color is lit anew to read well \
                         on it.{on} {left_right} steps it."
                    )
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
    /// many roles were picked and kept from the theme it started from.
    fn scores(&self) -> String {
        let score = self.score;
        let background = self.built.background;
        let ratio = |token| palette::contrast(self.built.color(token), background);
        let (text, dim) = (ratio(Token::Text), ratio(Token::Dim));
        let picks = self.draft().picks.iter().flatten();
        let kept = (picks.clone())
            .filter(|pick| matches!(pick, Pick::Kept(_)))
            .count();
        let picked = match picks.count() - kept {
            0 => String::new(),
            1 => " · 1 role picked".to_owned(),
            count => format!(" · {count} roles picked"),
        };
        let kept = match kept {
            0 => String::new(),
            1 => " · 1 role kept".to_owned(),
            kept => format!(" · {kept} roles kept"),
        };
        format!(
            "Harmony {:.0}% {} · text {text:.1}:1 {} · dim text {dim:.1}:1 {}{picked}{kept}",
            score * 100.0,
            palette::harmony_words(score),
            palette::contrast_words(text, Kind::Text),
            palette::contrast_words(dim, Kind::Dim),
        )
    }

    /// What the role at `at` takes its color from, in plain words.
    fn follows(&self, at: usize) -> String {
        let named = |source| self.source_words(at, source);
        match self.draft().picks[at] {
            None => named(palette::own(TOKENS[at]).1),
            Some(Pick::Follow(index)) => named(Source::Main(index)) + ", picked",
            Some(Pick::Exact(_)) => "set by hand".to_owned(),
            Some(Pick::Kept(_)) => format!("{}'s", self.started_from()),
        }
    }

    /// `source` in plain words, as the role at `at` takes its color from it.
    fn source_words(&self, at: usize, source: Source) -> String {
        let (kind, _) = palette::own(TOKENS[at]);
        let count = self.draft().mains.len();
        match source {
            Source::Main(slot) => {
                let main = slot % count + 1;
                match (kind, slot < count) {
                    (Kind::Accent, true) => format!("color {main}"),
                    (Kind::Accent, false) => format!("a shade of color {main}"),
                    _ => format!("a tint of color {main}"),
                }
            }
            Source::Meaning(meaning) => meaning.words().to_owned(),
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
        let kept = match self.draft().picks[self.role] {
            Some(Pick::Kept(_)) => format!(
                " It has {}'s color, lighter or darker as the contrast and the background \
                 change, until {} changes.",
                self.started_from(),
                self.source_words(self.role, palette::own(token).1),
            ),
            _ => String::new(),
        };
        let left_right = keymap::keys(&[Binding::CursorLeft, Binding::CursorRight]);
        let left_right = left_right.as_deref().unwrap_or("Left and Right");
        let exact = keymap::key(Binding::ThemeExact).unwrap_or_default();
        let reset = keymap::key(Binding::ThemeReset).unwrap_or_default();
        let shown = match (prompt_rule(token), token) {
            (Some(_), _) => " Shown on the rule under the editor while selected.",
            (None, Token::SelectedBg) => " Shown behind the selected row while selected.",
            _ => "",
        };
        format!(
            "{}{range}{kept}{shown} {left_right} picks the main color it follows, {exact} an \
             exact color, {reset} its own again.",
            about(token)
        )
    }

    /// The style of the rule under the editor: a dialog's, or, while a role of the prompt's
    /// rules is selected, that role's, which the sample does not show.
    pub(super) fn rule(&self) -> style::Style {
        (self.view == View::Roles)
            .then(|| prompt_rule(TOKENS[self.role]))
            .flatten()
            .unwrap_or(style::DIALOG_RULE)
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
                // As `/resume` and `/tree` paint theirs, which the sample does not show.
                let painted = TOKENS[self.role] == Token::SelectedBg;
                for at in first..first + room {
                    let line = row(&self.role_row(at, at == self.role), at == self.role);
                    lines.push(match at == self.role && painted {
                        true => format!("{}{line}", Band::Selected.mark()),
                        false => line,
                    });
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
                    Pick::Kept(rgb) => json!({"kept": hex_text(rgb)}),
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
                Background::Own(_) => "own",
            },
            "depth": draft.depth,
            "tint": draft.tint,
            "contrast": draft.contrast,
            "lightness": draft.lightness,
            "mains": mains,
            "picks": picks,
        });
        match draft.background {
            Background::Terminal(rgb) => state["terminal"] = json!(hex_text(rgb)),
            Background::Own(rgb) => state["own"] = json!(hex_text(rgb)),
            _ => {}
        }
        if let Some(from) = &self.from {
            state["from"] = json!(from);
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
        "terminal" => Background::Terminal(hex_value(state.get("terminal")?)?),
        "own" => Background::Own(hex_value(state.get("own")?)?),
        _ => return None,
    };
    let contrast = state.get("contrast")?.as_i64()?;
    if contrast.abs() > i64::from(CONTRAST_STEPS) {
        return None;
    }
    let lightness = match state.get("lightness") {
        None => 0,
        Some(steps) => steps.as_i64()?,
    };
    if lightness.abs() > i64::from(LIGHTNESS_STEPS) {
        return None;
    }
    let mut picks = vec![None; TOKENS.len()];
    for (key, pick) in state.get("picks")?.as_object()? {
        let at = KEYS.iter().position(|known| known == key)?;
        picks[at] = Some(match pick {
            Value::String(_) => Pick::Exact(hex_value(pick)?),
            pick if pick.get("kept").is_some() => Pick::Kept(hex_value(&pick["kept"])?),
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
        lightness: lightness as i32,
        picks,
        mains,
    };
    Some((draft, locked))
}

fn hex_text((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The color of a text such as `"#ff8800"`.
fn hex_value(value: &Value) -> Option<Rgb> {
    hex(value.as_str()?.strip_prefix('#')?)
}

/// The exact color `color` is, if it is one.
fn rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
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
        T::Accent => "The main accent: the spinner, the mark of a list's selected row, the banner's name, a question's title, a file tool's path.",
        T::Border => "Borders: the rule under lists and dialogs, this editor's too, and around a question the model asks.",
        T::BorderAccent => "Highlighted borders: compactions in /tree.",
        T::BorderMuted => "Quiet borders: the rails beside reasoning and calls in the looks other than the classic one, the card banner's border.",
        T::Success => "Success: calls and subagents that went well, the mark of the current session or model, models switched on.",
        T::Error => "Errors: failed calls, commands and subagents, a full context, what a list's key cannot do.",
        T::Warning => "Warnings: retries, notices, running calls and subagents, a filling context, labels and named sessions, a read's lines.",
        T::Muted => "Secondary text: the status line, notes such as Took 1.4s, hints, next steps, headings in lists.",
        T::Dim => "Faint text: the footer, the banner's keys and lists, the tree lines in lists, models switched off.",
        T::Text => "Text: replies, and everything without a color of its own.",
        T::ThinkingText => "The reasoning a model shows.",
        T::SelectedBg => "Behind the selected row in /resume and /tree.",
        T::UserMessageBg => "Behind your prompts, in the panels look.",
        T::UserMessageText => "Your prompts, in the looks other than the classic one.",
        T::CustomMessageBg => "Behind compaction and branch summaries and what a command or skill sent.",
        T::CustomMessageText => "Compaction and branch summaries, and what a command or skill sent.",
        T::CustomMessageLabel => "The labels of compaction and branch summaries and of what a command or skill sent: [compaction].",
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

/// The style of the prompt's rules that `token` colors, if it colors them.
fn prompt_rule(token: Token) -> Option<style::Style> {
    use Token as T;
    Some(match token {
        T::ThinkingOff => style::PROMPT_OFF,
        T::ThinkingMinimal => style::PROMPT_MINIMAL,
        T::ThinkingLow => style::PROMPT_LOW,
        T::ThinkingMedium => style::PROMPT_MEDIUM,
        T::ThinkingHigh => style::PROMPT_HIGH,
        T::ThinkingXhigh => style::PROMPT_XHIGH,
        T::BashMode => style::PROMPT_COMMAND,
        _ => return None,
    })
}

/// A block of `color`, as the terminal shows it.
fn swatch((r, g, b): Rgb) -> String {
    let color = Color::Rgb(r, g, b).foreground(Mode::detect());
    format!("\x1b[{}m████{RESET}", color.unwrap_or_default())
}

/// A sample session with every element the theme colors, in parts as the screen keeps a
/// conversation: a turn that thinks, runs a failing command, edits, runs it again and replies
/// in markdown, a command the user ran, a compaction and a notice.
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
                 What `words(\"a b  c\")` gives:\n\n\
                 ```\nbefore: [\"a\", \"b\", \"\", \"c\"]\nafter:  [\"a\", \"b\", \"c\"]\n```\n\n\
                 ---\n\n\
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
    let compaction = Compaction {
        tokens_before: 48_200,
        kept_items: 6,
        summary: "The words test failed: `words` split on **single** spaces. It splits on any \
                  whitespace now, and the test passes."
            .into(),
    };
    let notice = "/fix is waiting for you: it asked a question. Reply to continue.";
    let running = vec![
        Event::UserPrompt("Yes, and run clippy too.".into()),
        call("bash", "cargo clippy -q"),
        Event::ToolRunning(after(3.0)),
        Event::ToolOutput("    Checking words v0.1.0 (/home/me/words)".into()),
    ];
    vec![
        turn,
        vec![
            Event::CommandStarted(command.command.clone(), true),
            Event::CommandResult(command),
        ],
        vec![Event::Compacted(compaction, None)],
        vec![Event::Notice(notice.into())],
        running,
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
        assert!(shown[6].starts_with("  Color 1     ████ #"), "{shown:?}");
        // Down to the second color, locked; a roll keeps it.
        for _ in 0..6 {
            editor.key(&press(Code::Down));
        }
        assert_eq!(editor.key(&typed("l")), None);
        assert!(
            lines(&editor)[7].ends_with(" · locked ›"),
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
    fn the_count_row_rolls_anew_and_keeps_the_locked_colors_and_the_harmony_row_keeps_all() {
        let mut editor = ThemeEditor::new(2, Background::Dark, None);
        // Lock the third color, then go down to two colors: it stays, as the first.
        editor.row = 7;
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
        let (mains, score) = (editor.draft().mains.clone(), editor.score);
        editor.row = 1;
        editor.key(&press(Code::Right));
        assert_eq!(HARMONIES[editor.draft().harmony].name, "analogous");
        assert_eq!(editor.draft().mains, mains);
        assert_ne!(editor.score, score);
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
        editor.row = 6;
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
    fn the_lightness_row_steps_the_background_through_rolls_undo_and_the_file() {
        let mut editor = ThemeEditor::new(8, Background::Dark, None);
        editor.row = 3;
        let rolled = editor.built.background;
        let text_before = editor.built.color(Token::Text);
        assert_eq!(lines(&editor)[4], "› Lightness   ‹ as rolled ›");
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().lightness, 1);
        assert_eq!(lines(&editor)[4], "› Lightness   ‹ +1, lighter ›");
        let lighter = editor.built.background;
        assert!(Oklch::of(lighter).l > Oklch::of(rolled).l + 0.01);
        // Every color is lit anew on it: text keeps its contrast, so it is lighter too.
        let ratio = |editor: &ThemeEditor| {
            palette::contrast(editor.built.color(Token::Text), editor.built.background)
        };
        assert!((ratio(&editor) / palette::contrast(text_before, rolled) - 1.0).abs() < 0.03);
        for _ in 0..LIGHTNESS_STEPS {
            editor.key(&press(Code::Left));
        }
        assert_eq!(editor.draft().lightness, 1 - LIGHTNESS_STEPS);
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().lightness, -LIGHTNESS_STEPS);
        assert_eq!(lines(&editor)[4], "› Lightness   ‹ -3, the darkest ›");
        editor.key(&press(Code::Left));
        assert_eq!(
            editor.status,
            Some(("The background is at its darkest".into(), true))
        );
        // A roll keeps the steps; undo goes back through them.
        editor.key(&typed("r"));
        assert_eq!(editor.draft().lightness, -LIGHTNESS_STEPS);
        editor.key(&typed("u"));
        editor.key(&typed("u"));
        assert_eq!(editor.draft().lightness, 1 - LIGHTNESS_STEPS);
        // The file keeps them, and a file saved before they were kept opens at none.
        let text = editor.file_text("dim");
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json[STATE]["lightness"], 1 - LIGHTNESS_STEPS);
        let opened = ThemeEditor::open(&text, 1, None).unwrap();
        assert_eq!(opened.draft(), editor.draft());
        assert_eq!(opened.theme(), editor.theme());
        let older = text.replacen("\"lightness\": -2,", "", 1);
        assert_ne!(older, text);
        assert_eq!(
            ThemeEditor::open(&older, 1, None)
                .unwrap()
                .draft()
                .lightness,
            0
        );
        let broken = text.replacen("\"lightness\": -2", "\"lightness\": -4", 1);
        assert!(ThemeEditor::open(&broken, 1, None).is_none());

        // A step from the terminal's background makes the theme set its own, starting from it.
        let campbell = (12, 12, 12);
        let mut editor = ThemeEditor::new(11, Background::Terminal(campbell), Some(campbell));
        editor.row = 3;
        let shown = lines(&editor).join(" ");
        let shown = shown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(shown.contains("makes the theme set its own"), "{shown}");
        editor.key(&press(Code::Right));
        let own = editor.theme().background();
        assert_ne!(own, Color::Default);
        assert!(Oklch::of(editor.built.background).l > Oklch::of(campbell).l);
        editor.key(&press(Code::Left));
        assert_eq!(editor.theme().background(), Color::Default);
        assert_eq!(editor.built.background, campbell);
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
        let short: Vec<String> = editor.lines((100, 10)).iter().map(|l| plain(l)).collect();
        assert_eq!(short.len(), 10);
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
        editor.row = 7;
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
    fn roles_the_sample_does_not_show_show_on_the_dock_while_selected() {
        let mut editor = ThemeEditor::new(10, Background::Dark, None);
        editor.role = token_at(Token::ThinkingHigh);
        assert_eq!(editor.rule(), style::DIALOG_RULE);
        editor.view = View::Roles;
        assert_eq!(editor.rule(), style::PROMPT_HIGH);
        let note = lines(&editor).join(" ");
        assert!(
            note.contains("Shown on the rule under the editor"),
            "{note}"
        );
        editor.role = token_at(Token::BashMode);
        assert_eq!(editor.rule(), style::PROMPT_COMMAND);
        editor.role = token_at(Token::Border);
        assert_eq!(editor.rule(), style::DIALOG_RULE);
        let painted = |editor: &ThemeEditor| {
            let rows = editor.lines((100, 40));
            (crate::agent_cli::selection::rows(&rows).into_iter())
                .filter(|line| line.band == Some(Band::Selected))
                .map(|line| plain(&line.shown))
                .collect::<Vec<_>>()
        };
        assert_eq!(painted(&editor), Vec::<String>::new());
        editor.role = token_at(Token::SelectedBg);
        let rows = painted(&editor);
        assert!(
            rows.len() == 1 && rows[0].starts_with("› selectedBg "),
            "{rows:?}"
        );
        editor.view = View::Main;
        assert_eq!(painted(&editor), Vec::<String>::new());
    }

    #[test]
    fn a_saved_theme_opens_as_it_was_saved_with_the_roles_changed_in_its_file() {
        let mut editor = ThemeEditor::new(12, Background::Dark, None);
        editor.row = 0;
        editor.key(&press(Code::Right));
        editor.row = 6;
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

    fn kept(editor: &ThemeEditor, token: Token) -> bool {
        matches!(editor.draft().picks[token_at(token)], Some(Pick::Kept(_)))
    }

    #[test]
    fn a_theme_the_editor_did_not_make_starts_a_new_one_showing_its_colors() {
        let campbell = (12, 12, 12);
        let dark = Theme::built_in("scramjet-dark").unwrap();
        let editor = ThemeEditor::from_theme("scramjet-dark", &dark, 1, Some(campbell)).unwrap();
        // Its accent, border, heading, variable and string colors are the main colors, each
        // lit as it is, on the terminal's background, which it leaves to the terminal.
        assert_eq!(editor.draft().mains.len(), 5);
        assert_eq!(editor.draft().background, Background::Terminal(campbell));
        let slots = [
            Token::Accent,
            Token::Border,
            Token::MdHeading,
            Token::SyntaxVariable,
            Token::SyntaxString,
        ];
        // Each is lit as it is where its contrast is within the accents' band; the cyan, above
        // it, keeps its hue and chroma.
        for (index, token) in slots.into_iter().enumerate() {
            let theirs = rgb(dark.color(token)).unwrap();
            let lit = editor.draft().accent(index, campbell);
            match token {
                Token::Accent => {
                    let (lit, theirs) = (Oklch::of(lit), Oklch::of(theirs));
                    assert!((lit.h - theirs.h).abs() < 1.0 && (lit.c - theirs.c).abs() < 0.005);
                    assert!(lit.l < theirs.l);
                }
                _ => assert_eq!(lit, theirs, "{}", KEYS[token as usize]),
            }
        }
        // It shows scramjet-dark, but for the colors that theme leaves to the terminal.
        let shown = editor.theme();
        assert_eq!(shown.background(), Color::Default);
        for (at, &token) in TOKENS.iter().enumerate() {
            match dark.color(token) {
                Color::Rgb(..) => assert_eq!(shown.color(token), dark.color(token), "{}", KEYS[at]),
                _ => assert_eq!(editor.draft().picks[at], None, "{}", KEYS[at]),
            }
        }
        let shown = lines(&editor);
        assert!(
            shown[0].contains("From scramjet-dark: its colors stay until"),
            "{shown:?}"
        );
        assert!(
            (shown.iter()).any(|line| line.ends_with(" · 38 roles kept")),
            "{shown:?}"
        );
        // Without the terminal's background, a light one for a theme with dark secondary text.
        let light = Theme::built_in("pi-light").unwrap();
        let editor = ThemeEditor::from_theme("pi-light", &light, 1, None).unwrap();
        assert_eq!(editor.draft().background, Background::Light);
        assert_eq!(
            editor.theme().color(Token::Muted),
            light.color(Token::Muted)
        );
        let editor = ThemeEditor::from_theme("scramjet-dark", &dark, 1, None).unwrap();
        assert_eq!(editor.draft().background, Background::Dark);
        // The terminal's 16 colors have nothing exact to start from.
        let terminal = Theme::built_in(theme::TERMINAL_COLORS).unwrap();
        assert!(ThemeEditor::from_theme("terminal-colors", &terminal, 1, None).is_none());
        // In the colors a terminal answered it shows them in, they do: Campbell's here, its
        // faint text halfway to the background, its own text color left to the editor.
        let campbell_colors: [Rgb; 16] = [
            (12, 12, 12),
            (197, 15, 31),
            (19, 161, 14),
            (193, 156, 0),
            (0, 55, 218),
            (136, 23, 152),
            (58, 150, 221),
            (204, 204, 204),
            (118, 118, 118),
            (231, 72, 86),
            (22, 198, 12),
            (249, 241, 165),
            (59, 120, 255),
            (180, 0, 158),
            (97, 214, 214),
            (242, 242, 242),
        ];
        let shown = terminal.exactly(|color| match color {
            Color::Index(index) => Some(campbell_colors[usize::from(index)]),
            Color::Faint => Some((108, 108, 108)),
            _ => None,
        });
        let editor = ThemeEditor::from_theme("terminal-colors", &shown, 1, Some(campbell)).unwrap();
        assert_eq!(editor.draft().mains.len(), 5);
        assert_eq!(editor.draft().background, Background::Terminal(campbell));
        assert_eq!(editor.theme().background(), Color::Default);
        for (token, color) in [
            (Token::Accent, (58, 150, 221)),
            (Token::Border, (0, 55, 218)),
            (Token::Error, (197, 15, 31)),
            (Token::MdHeading, (193, 156, 0)),
            (Token::Muted, (108, 108, 108)),
        ] {
            assert_eq!(
                editor.theme().color(token),
                Color::Rgb(color.0, color.1, color.2),
                "{}",
                KEYS[token as usize]
            );
        }
        for token in [Token::Text, Token::SelectedBg, Token::UserMessageBg] {
            assert_eq!(editor.draft().picks[token as usize], None);
        }
        // A color used again is one main color.
        let text = include_str!("themes/scramjet-dark.json")
            .replace(r#""border": "shockBlue""#, r#""border": "plasmaCyan""#);
        let twice = Theme::parse(&text).unwrap();
        let editor = ThemeEditor::from_theme("twice", &twice, 1, Some(campbell)).unwrap();
        assert_eq!(editor.draft().mains.len(), 4);
        assert_eq!(
            editor.theme().color(Token::Border),
            dark.color(Token::Accent)
        );
    }

    #[test]
    fn kept_colors_follow_the_main_colors_once_what_they_follow_changes() {
        let campbell = (12, 12, 12);
        let dark = Theme::built_in("scramjet-dark").unwrap();
        let mut editor =
            ThemeEditor::from_theme("scramjet-dark", &dark, 2, Some(campbell)).unwrap();
        let start = editor.draft().clone();
        // Color 1 locked, a roll leaves its roles as they were and lights the others anew.
        editor.row = 5;
        editor.key(&typed("l"));
        editor.key(&typed("r"));
        assert!(kept(&editor, Token::Accent) || start.picks[token_at(Token::Accent)].is_none());
        assert!(kept(&editor, Token::Muted) && kept(&editor, Token::MdCodeBlockBorder));
        assert!(!kept(&editor, Token::Border) && !kept(&editor, Token::MdHeading));
        assert_eq!(editor.theme().color(Token::Muted), dark.color(Token::Muted));
        assert_ne!(
            editor.theme().color(Token::Border),
            dark.color(Token::Border)
        );
        let status = editor.status.clone().unwrap().0;
        assert!(
            status.ends_with(" roles follow the main colors now, not scramjet-dark's"),
            "{status}"
        );
        // In the roles' view, Backspace lets one follow its own; the others stay.
        editor.key(&press(Code::Tab));
        editor.role = token_at(Token::Muted);
        let shown = lines(&editor).join(" ");
        let shown = shown.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(shown.contains("‹ scramjet-dark's ›"), "{shown}");
        assert!(
            shown.contains("It has scramjet-dark's color, lighter or darker as the contrast and the background \
             change, until a tint of color 1 changes."),
            "{shown}"
        );
        editor.key(&press(Code::Backspace));
        assert!(!kept(&editor, Token::Muted) && kept(&editor, Token::MdCodeBlockBorder));
        // A step of contrast keeps them, each lighter on this dark background, with its hue;
        // undo brings them back.
        let was: Vec<_> = (TOKENS.iter())
            .filter(|&&token| kept(&editor, token))
            .map(|&token| (token, editor.built.color(token)))
            .collect();
        editor.key(&typed("+"));
        assert!(editor.status.is_none(), "{:?}", editor.status);
        let on = editor.built.background;
        let mut changed = 0;
        for &(token, before) in &was {
            assert!(kept(&editor, token), "{token:?}");
            let now = editor.built.color(token);
            assert!(palette::contrast(now, on) >= palette::contrast(before, on) - 0.05);
            if now != before {
                changed += 1;
                let (before, now) = (Oklch::of(before), Oklch::of(now));
                assert!(now.l > before.l, "{token:?}");
                if before.c > 0.02 {
                    assert!(
                        (now.h - before.h).abs() < 3.0,
                        "{token:?} {before:?} {now:?}"
                    );
                }
            }
        }
        assert!(changed > 10, "{changed} of {}", was.len());
        assert_ne!(
            editor.theme().color(Token::MdCodeBlockBorder),
            dark.color(Token::MdCodeBlockBorder)
        );
        // A lighter background keeps them too, each at its contrast on the one before.
        let was: Vec<_> = (was.iter())
            .map(|&(token, _)| (token, palette::contrast(editor.built.color(token), on)))
            .collect();
        editor.view = View::Main;
        editor.row = 3;
        editor.key(&press(Code::Right));
        assert!(editor.status.is_none(), "{:?}", editor.status);
        let lighter = editor.built.background;
        assert!(Oklch::of(lighter).l > Oklch::of(on).l);
        for (token, ratio) in was {
            assert!(kept(&editor, token), "{token:?}");
            let now = palette::contrast(editor.built.color(token), lighter);
            assert!((now / ratio - 1.0).abs() < 0.06, "{token:?} {ratio} {now}");
        }
        editor.view = View::Roles;
        for _ in 0..4 {
            editor.key(&typed("u"));
        }
        assert_eq!(*editor.draft(), start);
        assert_eq!(
            editor.theme().color(Token::Border),
            dark.color(Token::Border)
        );
        // Saved, it opens again with the colors kept and where they came from.
        let text = editor.file_text("mine");
        let json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json[STATE]["from"], "scramjet-dark");
        assert_eq!(json[STATE]["picks"]["muted"]["kept"], "#8b95a7");
        let opened = ThemeEditor::open(&text, 1, Some(campbell)).unwrap();
        assert_eq!(opened.draft(), editor.draft());
        assert_eq!(opened.from.as_deref(), Some("scramjet-dark"));
        assert_eq!(opened.theme(), editor.theme());
    }

    #[test]
    fn a_background_of_a_themes_own_is_kept_and_offered_beside_the_others() {
        let campbell = (12, 12, 12);
        let text = include_str!("themes/scramjet-dark.json").replace(
            r#""text": "","#,
            r##""text": "", "background": "#10141c","##,
        );
        let own = Theme::parse(&text).unwrap();
        let mut editor = ThemeEditor::from_theme("own", &own, 1, Some(campbell)).unwrap();
        assert_eq!(editor.draft().background, Background::Own((16, 20, 28)));
        assert_eq!(editor.theme().background(), Color::Rgb(16, 20, 28));
        editor.row = 2;
        let shown = lines(&editor);
        assert_eq!(shown[3], "› Background  ‹ its own, #10141c ›");
        // After the terminal's, dark and light; saved and opened again, it is still there.
        editor.key(&press(Code::Right));
        assert_eq!(editor.draft().background, Background::Terminal(campbell));
        editor.key(&press(Code::Left));
        assert_eq!(editor.draft().background, Background::Own((16, 20, 28)));
        let file = editor.file_text("mine");
        let opened = ThemeEditor::open(&file, 1, Some(campbell)).unwrap();
        assert_eq!(opened.draft().background, Background::Own((16, 20, 28)));
        assert_eq!(opened.own, Some((16, 20, 28)));
        assert_eq!(opened.theme(), editor.theme());
        // A theme the editor made whose background was changed in its file opens on that one,
        // its roles still following their main colors, lit on it.
        let made = ThemeEditor::new(3, Background::Dark, None);
        let file = made.file_text("made");
        let json: Value = serde_json::from_str(&file).unwrap();
        let was = json["vars"]["background"].as_str().unwrap().to_owned();
        let changed = file.replace(
            &format!(r#""background": "{was}""#),
            r##""background": "#203040""##,
        );
        let opened = ThemeEditor::open(&changed, 1, None).unwrap();
        assert_eq!(
            opened.draft().background,
            Background::Own((0x20, 0x30, 0x40))
        );
        assert!(opened.draft().picks.iter().all(Option::is_none));
        assert_eq!(opened.theme().background(), Color::Rgb(0x20, 0x30, 0x40));
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
    fn the_sample_session_shows_a_failed_and_a_passing_call_a_diff_a_reply_and_a_running_call() {
        let parts = sample();
        assert_eq!(parts.len(), 5);
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
        let reply = (parts[0].iter())
            .find_map(|event| match event {
                Event::Assistant(reply) => Some(reply.as_str()),
                _ => None,
            })
            .unwrap();
        // Highlighted code, code that is not (`mdCodeBlock`) and a rule (`mdHr`).
        assert!(reply.contains("```rust\n") && reply.contains("\n\n```\nbefore:"));
        assert!(reply.contains("\n\n---\n\n"));
        // The last part's call still runs, on `toolPendingBg` in the panels look.
        assert!(matches!(parts[4].last(), Some(Event::ToolOutput(_))));
    }
}
