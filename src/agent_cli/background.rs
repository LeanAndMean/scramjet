//! The terminal's own background: as the terminal answers OSC 11, which the full screen asks it
//! each time it is shown, else as `COLORFGBG` or Apple's Terminal says, as Scramjet decides it.

use super::palette::Rgb;
use std::sync::{Mutex, PoisonError};

/// The question; the answer comes in with the keys (`Key::Background`).
pub(super) const QUERY: &str = "\x1b]11;?\x1b\\";

static ANSWERED: Mutex<Option<Rgb>> = Mutex::new(None);

/// Keeps the terminal's answer, the latest replacing any before it.
pub(super) fn answered(rgb: Rgb) {
    *ANSWERED.lock().unwrap_or_else(PoisonError::into_inner) = Some(rgb);
}

/// The terminal's background as it answered, else as the environment says; `None` when
/// nothing says.
pub(super) fn terminal() -> Option<Rgb> {
    let answered = *ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
    let var = |name| std::env::var(name).unwrap_or_default();
    answered.or_else(|| from_environment(&var("COLORFGBG"), &var("TERM_PROGRAM")))
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

/// The color an OSC 11 answer names after its `11;`: `rgb:R/G/B`, each 1 to 4 hex digits, or
/// `#` and 3, 6, 9 or 12 of them.
pub(super) fn parse(text: &str) -> Option<Rgb> {
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
