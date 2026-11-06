//! The terminal's own colors: its background, which the full screen asks each time it is shown,
//! as it answers OSC 11, else as `COLORFGBG` or Apple's Terminal says, as Scramjet decides it;
//! and its text color and 16 colors, as it answers OSC 10 and OSC 4, which Scramjet does not
//! ask and `/settings` asks as its themes open, for the theme editor.

use super::palette::Rgb;
use super::theme::Color;
use std::sync::{Mutex, PoisonError};

/// What the terminal is asked: its background, its text color and each of its 16 colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Asked {
    Background,
    Foreground,
    Palette(u8),
}

/// The question for the background; the answers come in with the keys (`Key::Answer`).
pub(super) const BACKGROUND_QUESTION: &str = "\x1b]11;?\x1b\\";

/// The questions for the text color and the 16 colors, each on its own, as not every terminal
/// takes several colors in one OSC 4.
pub(super) fn color_questions() -> String {
    let palette = (0..16).map(|index| format!("\x1b]4;{index};?\x1b\\"));
    std::iter::once("\x1b]10;?\x1b\\".to_owned())
        .chain(palette)
        .collect()
}

/// The terminal's colors, as far as it answered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Answers {
    background: Option<Rgb>,
    foreground: Option<Rgb>,
    palette: [Option<Rgb>; 16],
}

static ANSWERED: Mutex<Answers> = Mutex::new(Answers {
    background: None,
    foreground: None,
    palette: [None; 16],
});

/// Keeps the terminal's answer, the latest replacing any before it.
pub(super) fn answered(asked: Asked, rgb: Rgb) {
    ANSWERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .set(asked, rgb);
}

/// The terminal's colors as it answered.
pub(super) fn answers() -> Answers {
    *ANSWERED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The terminal's background as it answered, else as the environment says; `None` when
/// nothing says.
pub(super) fn terminal() -> Option<Rgb> {
    let var = |name| std::env::var(name).unwrap_or_default();
    (answers().background).or_else(|| from_environment(&var("COLORFGBG"), &var("TERM_PROGRAM")))
}

impl Answers {
    /// The terminal answered its text color and all its 16 colors.
    pub(super) fn has_colors(&self) -> bool {
        self.foreground.is_some() && self.palette.iter().all(Option::is_some)
    }

    fn set(&mut self, asked: Asked, rgb: Rgb) {
        let slot = match asked {
            Asked::Background => &mut self.background,
            Asked::Foreground => &mut self.foreground,
            Asked::Palette(index) => &mut self.palette[usize::from(index)],
        };
        *slot = Some(rgb);
    }

    /// The exact color the terminal shows `color` in, as it answered: one of its 16 colors,
    /// another of its 256 as xterm's, which terminals rarely change, or faint text halfway from
    /// its text color to its background, as terminals dim it toward the background or black.
    /// `None` for its own text color and background, which a theme leaves to the terminal, and
    /// what it did not answer.
    pub(super) fn shown(&self, color: Color) -> Option<Rgb> {
        match color {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Index(index @ 0..16) => self.palette[usize::from(index)],
            Color::Index(index) => Some(xterm(index)),
            Color::Faint => {
                let ((r1, g1, b1), (r2, g2, b2)) = (self.foreground?, self.background?);
                let half = |a: u8, b: u8| ((u16::from(a) + u16::from(b)).div_ceil(2)) as u8;
                Some((half(r1, r2), half(g1, g2), half(b1, b2)))
            }
            Color::Default => None,
        }
    }
}

/// The question an OSC answer's text, after `ESC ]`, answers, and the color it names.
pub(super) fn answer(text: &str) -> Option<(Asked, Rgb)> {
    let (asked, color) = match text.split_once(';')? {
        ("11", color) => (Asked::Background, color),
        ("10", color) => (Asked::Foreground, color),
        ("4", rest) => {
            let (index, color) = rest.split_once(';')?;
            let index = index.parse::<u8>().ok().filter(|&index| index < 16)?;
            (Asked::Palette(index), color)
        }
        _ => return None,
    };
    Some((asked, parse(color)?))
}

/// `COLORFGBG`'s last field, the background's index among xterm's 256 colors, in xterm's own
/// colors, as the terminal's palette is not known; else white for Apple's Terminal.
fn from_environment(colorfgbg: &str, program: &str) -> Option<Rgb> {
    let index = (colorfgbg.rsplit_once(';')).and_then(|(_, last)| last.parse::<u8>().ok());
    match index {
        Some(index) => Some(xterm(index)),
        None => (program == "Apple_Terminal").then_some((255, 255, 255)),
    }
}

/// The color of xterm's 256-color `index`, as Scramjet takes it.
fn xterm(index: u8) -> Rgb {
    const BASIC: [Rgb; 16] = [
        (0, 0, 0),
        (128, 0, 0),
        (0, 128, 0),
        (128, 128, 0),
        (0, 0, 128),
        (128, 0, 128),
        (0, 128, 128),
        (192, 192, 192),
        (128, 128, 128),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (0, 0, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let level = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
    match index {
        0..16 => BASIC[usize::from(index)],
        16..232 => {
            let cube = index - 16;
            (level(cube / 36), level(cube / 6 % 6), level(cube % 6))
        }
        _ => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

/// The color an answer names after its question's number: `rgb:R/G/B`, each 1 to 4 hex digits,
/// or `#` and 3, 6, 9 or 12 of them.
fn parse(text: &str) -> Option<Rgb> {
    let channels: Vec<&str> = match (text.strip_prefix("rgb:"), text.strip_prefix('#')) {
        (Some(rgb), _) => rgb.split('/').collect(),
        (_, Some(hash)) if matches!(hash.len(), 3 | 6 | 9 | 12) && hash.is_ascii() => {
            let length = hash.len() / 3;
            (0..3).map(|at| &hash[at * length..][..length]).collect()
        }
        _ => return None,
    };
    let scaled = |digits: &str| {
        let fits = (1..=4).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_hexdigit());
        let value = u32::from_str_radix(digits, 16).ok().filter(|_| fits)?;
        let most = (1u32 << (4 * digits.len())) - 1;
        Some(((value * 255 + most / 2) / most) as u8)
    };
    match channels[..] {
        [r, g, b] => Some((scaled(r)?, scaled(g)?, scaled(b)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_name_their_color_in_xterms_forms() {
        assert_eq!(parse("rgb:0c0c/0c0c/0c0c"), Some((12, 12, 12)));
        assert_eq!(parse("rgb:ff/80/0"), Some((255, 128, 0)));
        assert_eq!(parse("rgb:f/8/0"), Some((255, 136, 0)));
        assert_eq!(parse("rgb:fafa/f4f4/e3e3"), Some((250, 244, 227)));
        assert_eq!(parse("#1e1e2e"), Some((30, 30, 46)));
        assert_eq!(parse("#fff"), Some((255, 255, 255)));
        assert_eq!(parse("#ffff00000000"), Some((255, 0, 0)));
        for wrong in [
            "?",
            "rgb:ff/ff",
            "rgb:ff/ff/ff/ff",
            "rgb:fffff/0/0",
            "rgb:g0/0/0",
            "rgb://0",
            "#ffff",
            "#éé00",
            "red",
        ] {
            assert_eq!(parse(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn answers_say_which_question_they_answer() {
        assert_eq!(
            answer("11;rgb:0c0c/0c0c/0c0c"),
            Some((Asked::Background, (12, 12, 12)))
        );
        assert_eq!(
            answer("10;rgb:cccc/cccc/cccc"),
            Some((Asked::Foreground, (204, 204, 204)))
        );
        assert_eq!(
            answer("4;1;rgb:c5c5/0f0f/1f1f"),
            Some((Asked::Palette(1), (197, 15, 31)))
        );
        assert_eq!(
            answer("4;15;#f2f2f2"),
            Some((Asked::Palette(15), (242, 242, 242)))
        );
        for wrong in [
            "4;16;rgb:0/0/0",
            "4;rgb:0/0/0",
            "4;x;#000",
            "12;#000",
            "11;?",
            "11",
        ] {
            assert_eq!(answer(wrong), None, "{wrong}");
        }
        let questions = color_questions();
        assert!(questions.starts_with("\x1b]10;?\x1b\\\x1b]4;0;?\x1b\\"));
        assert!(questions.ends_with("\x1b]4;15;?\x1b\\"));
        assert_eq!(questions.matches("\x1b]4;").count(), 16);
    }

    #[test]
    fn the_colors_answered_say_how_the_terminal_shows_a_themes_colors() {
        let mut answers = Answers::default();
        assert_eq!(answers.shown(Color::Index(1)), None);
        assert_eq!(answers.shown(Color::Faint), None);
        answers.set(Asked::Palette(1), (197, 15, 31));
        answers.set(Asked::Foreground, (204, 204, 204));
        assert_eq!(answers.shown(Color::Index(1)), Some((197, 15, 31)));
        assert_eq!(answers.shown(Color::Index(2)), None);
        assert_eq!(answers.shown(Color::Index(67)), Some((95, 135, 175)));
        assert_eq!(answers.shown(Color::Rgb(1, 2, 3)), Some((1, 2, 3)));
        // Faint text needs both the text color and the background.
        assert_eq!(answers.shown(Color::Faint), None);
        answers.set(Asked::Background, (12, 12, 12));
        assert_eq!(answers.shown(Color::Faint), Some((108, 108, 108)));
        assert_eq!(answers.shown(Color::Default), None);
        assert!(!answers.has_colors());
        for index in 0..16 {
            answers.set(Asked::Palette(index), (index, index, index));
        }
        assert!(answers.has_colors());
        // The latest answer replaces one before it.
        answers.set(Asked::Palette(1), (255, 0, 0));
        assert_eq!(answers.shown(Color::Index(1)), Some((255, 0, 0)));
    }

    #[test]
    fn the_environment_names_the_background_as_scramjet_reads_it() {
        assert_eq!(from_environment("15;0", ""), Some((0, 0, 0)));
        assert_eq!(from_environment("0;15", ""), Some((255, 255, 255)));
        assert_eq!(from_environment("15;default;7", ""), Some((192, 192, 192)));
        assert_eq!(from_environment("0;231", ""), Some((255, 255, 255)));
        assert_eq!(from_environment("0;67", ""), Some((95, 135, 175)));
        assert_eq!(from_environment("0;235", ""), Some((38, 38, 38)));
        assert_eq!(
            from_environment("15;default", "Apple_Terminal"),
            Some((255, 255, 255))
        );
        for (colorfgbg, program) in [("", ""), ("0", ""), ("15;256", ""), ("15;default", "")] {
            assert_eq!(from_environment(colorfgbg, program), None, "{colorfgbg}");
        }
    }
}
