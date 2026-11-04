//! Themes, in Scramjet's format: a palette of named colors (`vars`) and the color of each of
//! its 52 tokens (`colors`), a palette color's name, an exact `#rrggbb`, a 256-color index or
//! `""` for the terminal's own. Exact colors are sent as 24-bit colors where the terminal shows
//! them, else as the nearest of its 256, as Scramjet sends them.

use serde_json::{Map, Value};

macro_rules! tokens {
    ($($token:ident = $key:literal),* $(,)?) => {
        /// What a theme colors, by Scramjet's names.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub(super) enum Token { $($token),* }

        const KEYS: &[&str] = &[$($key),*];
        const TOKENS: &[Token] = &[$(Token::$token),*];
    };
}

tokens! {
    Accent = "accent",
    Border = "border",
    BorderAccent = "borderAccent",
    BorderMuted = "borderMuted",
    Success = "success",
    Error = "error",
    Warning = "warning",
    Muted = "muted",
    Dim = "dim",
    Text = "text",
    ThinkingText = "thinkingText",
    SelectedBg = "selectedBg",
    UserMessageBg = "userMessageBg",
    UserMessageText = "userMessageText",
    CustomMessageBg = "customMessageBg",
    CustomMessageText = "customMessageText",
    CustomMessageLabel = "customMessageLabel",
    ToolPendingBg = "toolPendingBg",
    ToolSuccessBg = "toolSuccessBg",
    ToolErrorBg = "toolErrorBg",
    ToolTitle = "toolTitle",
    ToolOutput = "toolOutput",
    MdHeading = "mdHeading",
    MdLink = "mdLink",
    MdLinkUrl = "mdLinkUrl",
    MdCode = "mdCode",
    MdCodeBlock = "mdCodeBlock",
    MdCodeBlockBorder = "mdCodeBlockBorder",
    MdQuote = "mdQuote",
    MdQuoteBorder = "mdQuoteBorder",
    MdHr = "mdHr",
    MdListBullet = "mdListBullet",
    ToolDiffAdded = "toolDiffAdded",
    ToolDiffRemoved = "toolDiffRemoved",
    ToolDiffContext = "toolDiffContext",
    SyntaxComment = "syntaxComment",
    SyntaxKeyword = "syntaxKeyword",
    SyntaxFunction = "syntaxFunction",
    SyntaxVariable = "syntaxVariable",
    SyntaxString = "syntaxString",
    SyntaxNumber = "syntaxNumber",
    SyntaxType = "syntaxType",
    SyntaxOperator = "syntaxOperator",
    SyntaxPunctuation = "syntaxPunctuation",
    ThinkingOff = "thinkingOff",
    ThinkingMinimal = "thinkingMinimal",
    ThinkingLow = "thinkingLow",
    ThinkingMedium = "thinkingMedium",
    ThinkingHigh = "thinkingHigh",
    ThinkingXhigh = "thinkingXhigh",
    BashMode = "bashMode",
    SpellcheckError = "spellcheckError",
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Color {
    /// The terminal's own (`""`).
    Default,
    /// The terminal's own text, faint: how the built-in terminal colors show secondary text,
    /// which Scramjet's format cannot say.
    Faint,
    /// One of the terminal's 256 colors; the first 16 are its palette, which it may have changed.
    Index(u8),
    Rgb(u8, u8, u8),
}

/// How the terminal is sent exact colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mode {
    TrueColor,
    Indexed,
}

impl Mode {
    /// As Scramjet decides it: 24-bit colors unless the terminal is known not to show them.
    pub(super) fn detect() -> Self {
        let var = |name| std::env::var(name).unwrap_or_default();
        let (colorterm, term) = (var("COLORTERM"), var("TERM"));
        if colorterm == "truecolor" || colorterm == "24bit" || !var("WT_SESSION").is_empty() {
            return Self::TrueColor;
        }
        let limited = matches!(term.as_str(), "" | "dumb" | "linux")
            || var("TERM_PROGRAM") == "Apple_Terminal"
            || term == "screen"
            || term.starts_with("screen-")
            || term.starts_with("screen.");
        if limited {
            Self::Indexed
        } else {
            Self::TrueColor
        }
    }
}

impl Color {
    /// The SGR parameter that sets it as the text's color; none for the terminal's own, as the
    /// styles are applied after a reset.
    pub(super) fn foreground(self, mode: Mode) -> Option<String> {
        match self {
            Self::Default => None,
            Self::Faint => Some("2".into()),
            // The first 16 by their own codes, as before themes, which every terminal knows.
            Self::Index(index @ 0..8) => Some(format!("{}", 30 + index)),
            Self::Index(index @ 8..16) => Some(format!("{}", 90 + index - 8)),
            Self::Index(index) => Some(format!("38;5;{index}")),
            Self::Rgb(r, g, b) if mode == Mode::TrueColor => Some(format!("38;2;{r};{g};{b}")),
            Self::Rgb(r, g, b) => Some(format!("38;5;{}", nearest_index(r, g, b))),
        }
    }
}

/// The 256-color index nearest `r`, `g`, `b`, as Scramjet picks it: in the 6×6×6 cube, or on the
/// gray ramp for a color without a noticeable hue.
fn nearest_index(r: u8, g: u8, b: u8) -> u8 {
    const CUBE: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |values: &mut dyn Iterator<Item = i32>, value: i32| {
        let mut best = (0, i32::MAX);
        for (index, candidate) in values.enumerate() {
            if (value - candidate).abs() < best.1 {
                best = (index, (value - candidate).abs());
            }
        }
        best.0
    };
    let distance = |(r1, g1, b1): (i32, i32, i32), (r2, g2, b2): (i32, i32, i32)| {
        let (dr, dg, db) = ((r1 - r2) as f64, (g1 - g2) as f64, (b1 - b2) as f64);
        dr * dr * 0.299 + dg * dg * 0.587 + db * db * 0.114
    };
    let color = (r as i32, g as i32, b as i32);
    let (ri, gi, bi) = (
        nearest(&mut CUBE.into_iter(), color.0),
        nearest(&mut CUBE.into_iter(), color.1),
        nearest(&mut CUBE.into_iter(), color.2),
    );
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let gray = (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64).round() as i32;
    let step = nearest(&mut (0..24).map(|step| 8 + step * 10), gray);
    let level = 8 + step as i32 * 10;
    let spread = color.0.max(color.1).max(color.2) - color.0.min(color.1).min(color.2);
    if spread < 10 && distance(color, (level, level, level)) < distance(color, cube) {
        return 232 + step as u8;
    }
    16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
}

#[derive(Clone, Debug)]
pub(super) struct Theme {
    colors: Vec<Color>,
}

pub(super) const TERMINAL_COLORS: &str = "terminal-colors";

/// The built-in themes' names, in the order `/settings` lists them.
pub(super) const BUILT_IN: [&str; 3] = [TERMINAL_COLORS, "scramjet-dark", "pi-light"];

impl Theme {
    pub(super) fn color(&self, token: Token) -> Color {
        self.colors[token as usize]
    }

    /// The built-in theme `name`.
    pub(super) fn built_in(name: &str) -> Option<Self> {
        let text = match name {
            TERMINAL_COLORS => return Some(terminal_colors()),
            "scramjet-dark" => include_str!("themes/scramjet-dark.json"),
            "pi-light" => include_str!("themes/pi-light.json"),
            _ => return None,
        };
        Some(Self::parse(text).expect("the built-in themes parse"))
    }

    /// A theme in Scramjet's format, which names all its tokens' colors.
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let empty = Map::new();
        let vars = match json.get("vars") {
            None => &empty,
            Some(Value::Object(vars)) => vars,
            Some(_) => return Err("vars must be an object".into()),
        };
        let Some(Value::Object(colors)) = json.get("colors") else {
            return Err("it has no colors".into());
        };
        let missing: Vec<&str> = (KEYS.iter())
            .filter(|key| !colors.contains_key(**key))
            .copied()
            .collect();
        if !missing.is_empty() {
            return Err(format!("colors lacks {}", missing.join(", ")));
        }
        let colors = (KEYS.iter())
            .map(|key| {
                resolve(&colors[*key], vars, &mut Vec::new()).map_err(|e| format!("{key}: {e}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { colors })
    }
}

/// A color in the theme, following the palette's names to a color.
fn resolve<'a>(
    value: &'a Value,
    vars: &'a Map<String, Value>,
    seen: &mut Vec<&'a str>,
) -> Result<Color, String> {
    let text = match value {
        Value::Number(number) => {
            let index = number.as_u64().and_then(|n| u8::try_from(n).ok());
            return index
                .map(Color::Index)
                .ok_or(format!("{number} is not 0 to 255"));
        }
        Value::String(text) => text.as_str(),
        _ => return Err(format!("{value} is not a color")),
    };
    if text.is_empty() {
        return Ok(Color::Default);
    }
    if let Some(hex) = text.strip_prefix('#') {
        let channel = |at: usize| {
            hex.get(at..at + 2)
                .and_then(|c| u8::from_str_radix(c, 16).ok())
        };
        return match (hex.len(), channel(0), channel(2), channel(4)) {
            (6, Some(r), Some(g), Some(b)) => Ok(Color::Rgb(r, g, b)),
            _ => Err(format!("{text} is not #rrggbb")),
        };
    }
    if seen.contains(&text) {
        return Err(format!("{text} refers to itself"));
    }
    let Some(next) = vars.get(text) else {
        return Err(format!("{text} is not in vars"));
    };
    seen.push(text);
    resolve(next, vars, seen)
}

/// The terminal's own 16 colors, as scramjet-rs showed everything before it had themes.
fn terminal_colors() -> Theme {
    use Color::{Default, Faint, Index};
    use Token::*;
    let (red, green, yellow, blue, magenta, cyan) =
        (Index(1), Index(2), Index(3), Index(4), Index(5), Index(6));
    let color = |token| match token {
        Accent | BorderAccent | CustomMessageLabel | MdCode | MdListBullet | SyntaxType
        | ThinkingMedium => cyan,
        Border | MdLink | MdQuoteBorder | SyntaxKeyword | ThinkingLow => blue,
        Success | MdCodeBlock | ToolDiffAdded | SyntaxString | BashMode => green,
        Error | ToolDiffRemoved | ThinkingXhigh | SpellcheckError => red,
        Warning | MdHeading | SyntaxFunction | SyntaxNumber => yellow,
        SyntaxVariable | ThinkingHigh => magenta,
        BorderMuted | Muted | Dim | ThinkingText | CustomMessageText | ToolOutput | MdLinkUrl
        | MdCodeBlockBorder | MdQuote | MdHr | ToolDiffContext | SyntaxComment | ThinkingOff => {
            Faint
        }
        Text | SelectedBg | UserMessageBg | UserMessageText | CustomMessageBg | ToolPendingBg
        | ToolSuccessBg | ToolErrorBg | ToolTitle | SyntaxOperator | SyntaxPunctuation
        | ThinkingMinimal => Default,
    };
    Theme {
        colors: TOKENS.iter().map(|&token| color(token)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_themes_color_every_token() {
        for name in BUILT_IN {
            assert_eq!(Theme::built_in(name).unwrap().colors.len(), KEYS.len());
        }
        let dark = Theme::built_in("scramjet-dark").unwrap();
        assert_eq!(dark.color(Token::Accent), Color::Rgb(0x38, 0xe1, 0xff));
        assert_eq!(dark.color(Token::Text), Color::Default);
        assert_eq!(
            dark.color(Token::SyntaxKeyword),
            Color::Rgb(0x56, 0x9c, 0xd6)
        );
        assert_eq!(KEYS.len(), 52);
    }

    #[test]
    fn a_theme_follows_its_palette_and_says_what_is_wrong() {
        let all = |value: &str| {
            let colors: Vec<String> = (KEYS.iter())
                .map(|key| format!(r#""{key}": {value}"#))
                .collect();
            colors.join(", ")
        };
        let theme = format!(
            r##"{{"vars": {{"a": "b", "b": "#0a0B0c"}}, "colors": {{{}}}}}"##,
            all(r#""a""#)
        );
        let theme = Theme::parse(&theme).unwrap();
        assert_eq!(theme.color(Token::MdHr), Color::Rgb(10, 11, 12));
        let indexed = format!(r#"{{"colors": {{{}}}}}"#, all("39"));
        assert_eq!(
            Theme::parse(&indexed).unwrap().color(Token::Dim),
            Color::Index(39)
        );

        let wrong = |vars: &str, value: &str| {
            let text = format!(r#"{{"vars": {vars}, "colors": {{{}}}}}"#, all(value));
            Theme::parse(&text).unwrap_err()
        };
        assert_eq!(wrong("{}", r#""a""#), "accent: a is not in vars");
        assert_eq!(
            wrong(r#"{"a": "b", "b": "a"}"#, r#""a""#),
            "accent: a refers to itself"
        );
        assert_eq!(
            wrong("{}", r##""#12345""##),
            "accent: #12345 is not #rrggbb"
        );
        assert_eq!(wrong("{}", "256"), "accent: 256 is not 0 to 255");
        assert_eq!(wrong("[]", "1"), "vars must be an object");
        let short = r#"{"colors": {"accent": "", "border": ""}}"#;
        let error = Theme::parse(short).unwrap_err();
        assert!(
            error.starts_with("colors lacks borderAccent, borderMuted, "),
            "{error}"
        );
    }

    #[test]
    fn exact_colors_fall_back_to_the_nearest_of_256_as_scramjets() {
        // Expected values from Scramjet's rgbTo256, run under Node.
        for (rgb, index) in [
            ((0, 0, 0), 16),
            ((255, 255, 255), 231),
            ((128, 128, 128), 244),
            ((0x38, 0xe1, 0xff), 81),
            ((0x8b, 0x95, 0xa7), 103),
            ((0x16, 0x1b, 0x26), 16),
            ((0xff, 0x5f, 0x6d), 203),
            ((0x59, 0x59, 0x59), 240),
        ] {
            assert_eq!(nearest_index(rgb.0, rgb.1, rgb.2), index, "{rgb:?}");
        }
        let color = Color::Rgb(0x38, 0xe1, 0xff);
        assert_eq!(
            color.foreground(Mode::TrueColor).unwrap(),
            "38;2;56;225;255"
        );
        assert_eq!(color.foreground(Mode::Indexed).unwrap(), "38;5;81");
        assert_eq!(Color::Index(3).foreground(Mode::TrueColor).unwrap(), "33");
        assert_eq!(Color::Index(9).foreground(Mode::TrueColor).unwrap(), "91");
        assert_eq!(
            Color::Index(100).foreground(Mode::Indexed).unwrap(),
            "38;5;100"
        );
        assert_eq!(Color::Default.foreground(Mode::TrueColor), None);
    }
}
