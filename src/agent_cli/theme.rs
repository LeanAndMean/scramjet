//! Themes, in Scramjet's format: a palette of named colors (`vars`) and the color of each of
//! its 52 tokens (`colors`), a palette color's name, an exact `#rrggbb`, a 256-color index or
//! `""` for the terminal's own. Exact colors are sent as 24-bit colors where the terminal shows
//! them, else as the nearest of its 256, as Scramjet sends them. Beyond Scramjet's format, a
//! theme may color the main `background` among its `colors`.

use scramjet::agent::{Problem, config_directory};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

macro_rules! tokens {
    ($($token:ident = $key:literal),* $(,)?) => {
        /// What a theme colors, by Scramjet's names.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub(super) enum Token { $($token),* }

        pub(super) const KEYS: &[&str] = &[$($key),*];
        pub(super) const TOKENS: &[Token] = &[$(Token::$token),*];
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    /// The SGR parameter that sets it as the background; none for the terminal's own.
    pub(super) fn background(self, mode: Mode) -> Option<String> {
        match self {
            Self::Default | Self::Faint => None,
            Self::Index(index @ 0..8) => Some(format!("{}", 40 + index)),
            Self::Index(index @ 8..16) => Some(format!("{}", 100 + index - 8)),
            Self::Index(index) => Some(format!("48;5;{index}")),
            Self::Rgb(r, g, b) if mode == Mode::TrueColor => Some(format!("48;2;{r};{g};{b}")),
            Self::Rgb(r, g, b) => Some(format!("48;5;{}", nearest_index(r, g, b))),
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Theme {
    colors: Vec<Color>,
    background: Color,
}

/// A theme to choose from: a built-in one, or one read from `file`.
#[derive(Debug)]
pub(super) struct Listed {
    pub(super) name: String,
    pub(super) file: Option<PathBuf>,
    pub(super) theme: Theme,
}

/// The themes to choose from, and what was wrong with the files skipped.
#[derive(Debug)]
pub(super) struct Themes {
    pub(super) listed: Vec<Listed>,
    problems: Vec<Problem>,
}

/// The command line's themes: `--theme`'s files and directories, and `--no-themes`.
#[derive(Clone, Debug, Default)]
pub(super) struct ThemeOptions {
    pub(super) paths: Vec<PathBuf>,
    pub(super) no_themes: bool,
}

/// Where themes are looked for, which says how a path that holds none is told.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Place {
    Directory,
    /// In the settings' `themes`.
    Listed,
    /// Given with `--theme`.
    Given,
}

impl Themes {
    /// The built-in themes, then, as Scramjet finds its own, the files in the `themes`
    /// directory of the user's (`config_directory`) and in the project's `.scramjet/themes`, the
    /// settings' `listed` files and directories, and those `options` give; with `--no-themes`,
    /// only those it gives.
    pub(super) fn discover(root: &Path, listed: &[PathBuf], options: &ThemeOptions) -> Self {
        let mut places = Vec::new();
        if !options.no_themes {
            let user = config_directory().map(|directory| directory.join("themes"));
            places.extend(user.map(|directory| (directory, Place::Directory)));
            places.push((
                scramjet::os::join(root, ".scramjet/themes"),
                Place::Directory,
            ));
            places.extend(listed.iter().map(|path| (path.clone(), Place::Listed)));
        }
        places.extend((options.paths.iter()).map(|path| (path.clone(), Place::Given)));
        Self::load(&places)
    }

    /// The built-in themes, then those in the `.json` files of `places`; of two with one name,
    /// the first is kept.
    fn load(places: &[(PathBuf, Place)]) -> Self {
        let built_in = BUILT_IN.map(|name| Listed {
            name: name.to_owned(),
            file: None,
            theme: Theme::built_in(name).expect("built in"),
        });
        let mut themes = Self {
            listed: built_in.into(),
            problems: Vec::new(),
        };
        let mut loaded = std::collections::HashSet::new();
        for (path, place) in places {
            let files = if *place == Place::Directory || path.is_dir() {
                match themes.files(path) {
                    Some(files) => files,
                    None => continue,
                }
            } else if is_json(path) {
                vec![path.clone()]
            } else {
                let wrong = match path.exists() {
                    true => "is not a json file",
                    false => "does not exist",
                };
                let shown = path.display();
                themes.problems.push(match place {
                    Place::Given => {
                        Problem::new(format!("theme path given with --theme {wrong}: {shown}"))
                    }
                    _ => Problem::in_settings(format!("theme path {wrong}: {shown}")),
                });
                continue;
            };
            for file in files {
                // The same file reached again, through a link or another directory, is no collision.
                if loaded.insert(file.canonicalize().unwrap_or_else(|_| file.clone())) {
                    themes.add(&file);
                }
            }
        }
        themes
    }

    /// The `.json` files in `directory`, in order; none when it does not exist.
    fn files(&mut self, directory: &Path) -> Option<Vec<PathBuf>> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                self.problems.push(Problem::new(format!(
                    "{}: failed to read theme directory ({error})",
                    directory.display()
                )));
                return None;
            }
        };
        let mut files: Vec<PathBuf> = (entries.filter_map(Result::ok))
            .map(|entry| entry.path())
            .filter(|path| is_json(path))
            .collect();
        files.sort();
        Some(files)
    }

    fn add(&mut self, file: &Path) {
        let skipping = |error: String| {
            Problem::in_file(format!("skipping theme {}: {error}", file.display()), file)
        };
        let read = std::fs::read_to_string(file).map_err(|error| error.to_string());
        let (name, theme) = match read.and_then(|text| Theme::parse_named(&text)) {
            Ok(parsed) => parsed,
            Err(error) => return self.problems.push(skipping(error)),
        };
        let stem = || {
            file.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let name = name.unwrap_or_else(stem);
        match self.listed.iter().find(|known| known.name == name) {
            // The same theme again, as a package's copy of a built-in one, is no collision.
            Some(known) if known.theme == theme => {}
            Some(known) => {
                let from = match &known.file {
                    Some(path) => format!("loaded from {}", path.display()),
                    None => "a built-in theme's".to_owned(),
                };
                self.problems
                    .push(skipping(format!("name {name} is already {from}")));
            }
            None => self.listed.push(Listed {
                name,
                file: Some(file.to_path_buf()),
                theme,
            }),
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<&Listed> {
        self.listed.iter().find(|listed| listed.name == name)
    }

    /// The theme `name`, else the terminal colors.
    pub(super) fn theme(&self, name: &str) -> Theme {
        self.get(name)
            .map_or_else(terminal_colors, |listed| listed.theme.clone())
    }

    /// What is wrong with the themes when the setting names `chosen`.
    pub(super) fn problems(&self, chosen: &str) -> Vec<Problem> {
        let missing = self.get(chosen).is_none().then(|| {
            Problem::in_settings(format!(
                "settings: theme {chosen} is not one of the themes found; using {TERMINAL_COLORS}"
            ))
        });
        missing.into_iter().chain(self.problems.clone()).collect()
    }
}

/// The names of the themes in `directory`'s files that read as themes.
pub(super) fn names_in(directory: &Path) -> Vec<String> {
    let mut themes = Themes {
        listed: Vec::new(),
        problems: Vec::new(),
    };
    let files = themes.files(directory).unwrap_or_default();
    (files.iter())
        .filter_map(|file| {
            let text = std::fs::read_to_string(file).ok()?;
            let (name, _) = Theme::parse_named(&text).ok()?;
            let stem = || {
                file.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into()
            };
            Some(name.unwrap_or_else(stem))
        })
        .collect()
}

fn is_json(path: &Path) -> bool {
    path.extension() == Some("json".as_ref()) && path.is_file()
}

pub(super) const TERMINAL_COLORS: &str = "terminal-colors";
/// The key of the main background among a theme's colors, which Scramjet's format lacks.
const BACKGROUND: &str = "background";

/// The built-in themes' names, in the order `/settings` lists them.
pub(super) const BUILT_IN: [&str; 3] = [TERMINAL_COLORS, "scramjet-dark", "pi-light"];

/// The theme shown when the settings name none, as Scramjet's `resolveThemeName` picks it:
/// `pi-light` on a light background, `scramjet-dark` on a dark or unknown one.
pub(super) fn following(background: Option<(u8, u8, u8)>) -> &'static str {
    match background.map(super::palette::luminance) {
        Some(luminance) if luminance >= 0.2 => "pi-light",
        _ => "scramjet-dark",
    }
}

impl Theme {
    pub(super) fn color(&self, token: Token) -> Color {
        self.colors[token as usize]
    }

    /// The main background, under everything; the terminal's own unless the theme colors it.
    pub(super) fn background(&self) -> Color {
        self.background
    }

    /// A theme of exact colors: each token's, in `TOKENS`' order, on the main `background`, or
    /// on the terminal's own without one.
    pub(super) fn exact(colors: &[(u8, u8, u8)], background: Option<(u8, u8, u8)>) -> Self {
        let exact = |(r, g, b)| Color::Rgb(r, g, b);
        Self {
            colors: colors.iter().copied().map(exact).collect(),
            background: background.map_or(Color::Default, exact),
        }
    }

    /// The theme with each color `shown` gives an exact color for in that color, its other
    /// colors as they are.
    pub(super) fn exactly(&self, shown: impl Fn(Color) -> Option<(u8, u8, u8)>) -> Self {
        let exact = |color| shown(color).map_or(color, |(r, g, b)| Color::Rgb(r, g, b));
        Self {
            colors: self.colors.iter().copied().map(exact).collect(),
            background: exact(self.background),
        }
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
        Self::parse_named(text).map(|(_, theme)| theme)
    }

    /// A theme in Scramjet's format, and its `name` when it has one.
    fn parse_named(text: &str) -> Result<(Option<String>, Self), String> {
        let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let name = match json.get("name") {
            None => None,
            Some(Value::String(name)) if !name.trim().is_empty() => Some(name.clone()),
            Some(_) => return Err("name must be a theme's name".into()),
        };
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
        let color = |key: &str| {
            resolve(&colors[key], vars, &mut Vec::new()).map_err(|e| format!("{key}: {e}"))
        };
        let background = match colors.contains_key(BACKGROUND) {
            true => color(BACKGROUND)?,
            false => Color::Default,
        };
        let colors = KEYS
            .iter()
            .map(|key| color(key))
            .collect::<Result<_, _>>()?;
        Ok((name, Self { colors, background }))
    }
}

/// The text of a theme file in Scramjet's format, named `name`, of exact colors: `vars` names
/// some, and each token (in `TOKENS`' order) and the main `background`, if the theme sets one,
/// take the first of those that is theirs by its name, or else their own; `extra` is one more
/// key at the top, which Scramjet's loader passes over.
pub(super) fn file_text(
    name: &str,
    vars: &[(String, (u8, u8, u8))],
    colors: &[(u8, u8, u8)],
    background: Option<(u8, u8, u8)>,
    extra: Option<(&str, &Value)>,
) -> String {
    let hex = |(r, g, b): (u8, u8, u8)| format!("#{r:02x}{g:02x}{b:02x}");
    let quoted = |text: &str| Value::String(text.to_owned()).to_string();
    let value = |color| match vars.iter().find(|(_, var)| *var == color) {
        Some((name, _)) => quoted(name),
        None => quoted(&hex(color)),
    };
    let vars: Vec<String> = (vars.iter())
        .map(|(name, color)| format!("    {}: {}", quoted(name), quoted(&hex(*color))))
        .collect();
    let keys = KEYS.iter().copied().chain([BACKGROUND]);
    let colors: Vec<String> = (keys.zip(colors.iter().copied().chain(background)))
        .map(|(key, color)| format!("    {}: {}", quoted(key), value(color)))
        .collect();
    let extra = extra.map_or_else(String::new, |(key, value)| {
        let value = serde_json::to_string_pretty(value).unwrap_or_default();
        format!(",\n  {}: {}", quoted(key), value.replace('\n', "\n  "))
    });
    format!(
        "{{\n  \"name\": {},\n  \"vars\": {{\n{}\n  }},\n  \"colors\": {{\n{}\n  }}{extra}\n}}\n",
        quoted(name),
        vars.join(",\n"),
        colors.join(",\n")
    )
}

/// How often the file of the theme in use is looked at, as the screen looks at the terminal's
/// size, and how long it stays as it changed before it is read, so that a file still being
/// written is not (Scramjet waits as long).
const CHECK: Duration = Duration::from_millis(250);
const SETTLE: Duration = Duration::from_millis(100);

/// What tells a file's change: its modification time, length and inode, which a save that
/// replaces the file changes.
type Stamp = (Option<std::time::SystemTime>, u64, u64);

fn stamp(file: &Path) -> Option<Stamp> {
    let metadata = std::fs::metadata(file).ok()?;
    Some((
        metadata.modified().ok(),
        metadata.len(),
        scramjet::os::inode(&metadata),
    ))
}

/// The file of the theme in use, read again when it changes, as Scramjet's own are.
#[derive(Debug)]
pub(super) struct Watched {
    pub(super) name: String,
    file: PathBuf,
    /// The file as it was read last.
    read: Option<Stamp>,
    /// The file as it was when it was first seen changed, and when.
    changed: Option<(Stamp, Instant)>,
    checked: Instant,
    /// It failed to read the last time.
    pub(super) failed: bool,
}

impl Watched {
    /// The listed theme's file, if it has one.
    pub(super) fn new(listed: &Listed) -> Option<Self> {
        let file = listed.file.clone()?;
        Some(Self {
            name: listed.name.clone(),
            read: stamp(&file),
            file,
            changed: None,
            checked: Instant::now(),
            failed: false,
        })
    }

    /// How long until it is looked at next.
    pub(super) fn due(&self, now: Instant) -> Duration {
        let at = match self.changed {
            Some((_, changed)) => changed + SETTLE,
            None => self.checked + CHECK,
        };
        at.saturating_duration_since(now)
    }

    /// The theme read again once its file changed and then stayed as it was for `SETTLE`, or
    /// why it could not be. A file gone, as while a save replaces it, changes nothing.
    pub(super) fn check(&mut self, now: Instant) -> Option<Result<Theme, String>> {
        if !self.due(now).is_zero() {
            return None;
        }
        self.checked = now;
        let Some(stamp) = stamp(&self.file) else {
            self.changed = None;
            return None;
        };
        if self.read == Some(stamp) {
            self.changed = None;
            return None;
        }
        match self.changed {
            Some((changed, _)) if changed == stamp => self.changed = None,
            _ => {
                self.changed = Some((stamp, now));
                return None;
            }
        }
        self.read = Some(stamp);
        let read = std::fs::read_to_string(&self.file).map_err(|error| error.to_string());
        let theme = read.and_then(|text| Theme::parse(&text));
        self.failed = theme.is_err();
        Some(theme)
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
        background: Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet::agent::ProblemFile;

    #[test]
    fn without_a_setting_the_theme_follows_the_background_as_scramjets() {
        assert_eq!(following(None), "scramjet-dark");
        assert_eq!(following(Some((12, 12, 12))), "scramjet-dark");
        assert_eq!(following(Some((118, 118, 118))), "scramjet-dark");
        assert_eq!(following(Some((128, 128, 128))), "pi-light");
        assert_eq!(following(Some((250, 244, 227))), "pi-light");
    }

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
    fn a_theme_may_color_the_main_background() {
        let with = |background: &str| {
            let colors: Vec<String> = (KEYS.iter()).map(|key| format!(r#""{key}": """#)).collect();
            format!(
                r##"{{"vars": {{"night": "#10141c"}}, "colors": {{{}{background}}}}}"##,
                colors.join(", ")
            )
        };
        let background = |text: &str| Theme::parse(text).map(|theme| theme.background());
        assert_eq!(background(&with("")), Ok(Color::Default));
        assert_eq!(
            background(&with(r#", "background": """#)),
            Ok(Color::Default)
        );
        assert_eq!(
            background(&with(r#", "background": "night""#)),
            Ok(Color::Rgb(0x10, 0x14, 0x1c))
        );
        assert_eq!(
            background(&with(r#", "background": "day""#)),
            Err("background: day is not in vars".into())
        );
        for name in BUILT_IN {
            assert_eq!(Theme::built_in(name).unwrap().background(), Color::Default);
        }
        assert_eq!(Color::Index(4).background(Mode::Indexed), Some("44".into()));
        assert_eq!(
            Color::Index(12).background(Mode::Indexed),
            Some("104".into())
        );
        assert_eq!(
            Color::Rgb(16, 20, 28).background(Mode::TrueColor),
            Some("48;2;16;20;28".into())
        );
        assert_eq!(
            Color::Rgb(16, 20, 28).background(Mode::Indexed),
            Some("48;5;16".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn themes_are_found_in_their_directories_the_first_of_a_name_kept() {
        let scratch = Scratch::new("found");
        let root = scratch.0.clone();
        let (user, project) = (root.join("user"), root.join("project"));
        for directory in [&user, &project, &user.join("folder.json")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let theme = |name: Option<&str>, accent: &str| {
            let colors: Vec<String> = (KEYS.iter())
                .map(|key| {
                    format!(
                        r#""{key}": "{}""#,
                        if *key == "accent" { accent } else { "" }
                    )
                })
                .collect();
            let name = name.map_or(String::new(), |name| format!(r#""name": "{name}", "#));
            format!(r#"{{{name}"colors": {{{}}}}}"#, colors.join(", "))
        };
        let write = |path: &Path, text: &str| std::fs::write(path, text).unwrap();
        write(&user.join("ocean.json"), &theme(Some("Ocean"), "#0000ff"));
        write(&user.join("plain.json"), &theme(None, "#00ff00"));
        write(&user.join("broken.json"), "{");
        write(&user.join("light.json"), &theme(Some("pi-light"), ""));
        write(&user.join("notes.txt"), &theme(Some("notes"), ""));
        write(&project.join("sea.json"), &theme(Some("Ocean"), "#ff0000"));
        write(
            &project.join("forest.json"),
            &theme(Some("Forest"), "#00ff00"),
        );
        std::os::unix::fs::symlink(project.join("forest.json"), project.join("link.json")).unwrap();
        std::fs::create_dir(root.join("elsewhere")).unwrap();
        write(&root.join("elsewhere/r.json"), &theme(Some("River"), ""));
        std::os::unix::fs::symlink(root.join("elsewhere/r.json"), project.join("river.json"))
            .unwrap();

        let directories = [user.clone(), root.join("missing"), project.clone()];
        let places: Vec<_> = (directories.into_iter())
            .map(|directory| (directory, Place::Directory))
            .collect();
        let themes = Themes::load(&places);
        let names: Vec<&str> = themes.listed.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "terminal-colors",
                "scramjet-dark",
                "pi-light",
                "Ocean",
                "plain",
                "Forest",
                "River"
            ]
        );
        let ocean = themes.get("Ocean").unwrap();
        assert_eq!(
            ocean.file.as_deref(),
            Some(user.join("ocean.json").as_path())
        );
        assert_eq!(
            themes.theme("Ocean").color(Token::Accent),
            Color::Rgb(0, 0, 255)
        );
        assert_eq!(themes.theme("gone"), terminal_colors());
        let problems: Vec<String> = (themes.problems("gone").into_iter())
            .map(|problem| problem.text)
            .collect();
        let shown = |path: PathBuf| path.display().to_string();
        assert_eq!(problems.len(), 4, "{problems:#?}");
        assert_eq!(
            problems[0],
            "settings: theme gone is not one of the themes found; using terminal-colors"
        );
        assert!(
            problems[1].starts_with(&format!(
                "skipping theme {}: EOF",
                shown(user.join("broken.json"))
            )),
            "{}",
            problems[1]
        );
        assert_eq!(
            problems[2],
            format!(
                "skipping theme {}: name pi-light is already a built-in theme's",
                shown(user.join("light.json"))
            )
        );
        assert_eq!(
            problems[3],
            format!(
                "skipping theme {}: name Ocean is already loaded from {}",
                shown(project.join("sea.json")),
                shown(user.join("ocean.json"))
            )
        );
        assert_eq!(themes.problems("River").len(), 3);
        drop(scratch);
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("scramjet-rs-themes-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A theme whose accent is `accent` and whose other colors are the terminal's.
    fn accented(name: &str, accent: &str) -> String {
        let colors: Vec<String> = (KEYS.iter())
            .map(|key| match *key {
                "accent" => format!(r#""{key}": "{accent}""#),
                _ => format!(r#""{key}": """#),
            })
            .collect();
        format!(
            r#"{{"name": "{name}", "colors": {{{}}}}}"#,
            colors.join(", ")
        )
    }

    #[test]
    fn the_settings_and_the_command_line_add_files_and_directories_after_the_others() {
        let scratch = Scratch::new("given");
        let root = &scratch.0;
        let (listed, given) = (root.join("listed"), root.join("given"));
        for directory in [&listed, &given, &root.join(".scramjet/themes")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let write = |path: PathBuf, text: &str| std::fs::write(path, text).unwrap();
        write(
            root.join(".scramjet/themes/a.json"),
            &accented("Project", ""),
        );
        write(listed.join("b.json"), &accented("Listed", ""));
        write(root.join("one.json"), &accented("File", ""));
        write(root.join("notes.txt"), "");
        write(given.join("c.json"), &accented("Given", "#010203"));
        write(root.join("two.json"), &accented("Project", "#040506"));
        write(root.join("same.json"), &accented("Project", ""));
        let settings = [
            listed,
            root.join("one.json"),
            root.join("notes.txt"),
            root.join("gone"),
        ];
        let options = ThemeOptions {
            paths: vec![
                given,
                root.join("two.json"),
                root.join("same.json"),
                root.join("missing.json"),
            ],
            no_themes: false,
        };
        // Leaving out the user's own themes, in the agent directory.
        let names = |themes: &Themes| -> Vec<String> {
            (themes.listed.iter().skip(BUILT_IN.len()))
                .filter(|listed| {
                    listed
                        .file
                        .as_ref()
                        .is_some_and(|file| file.starts_with(root))
                })
                .map(|listed| listed.name.clone())
                .collect()
        };
        let problems = |themes: &Themes| -> Vec<(String, Option<ProblemFile>)> {
            (themes.problems.iter())
                .map(|problem| (problem.text.clone(), problem.file.clone()))
                .collect()
        };
        let shown = |name: &str| scramjet::os::join(root, name).display().to_string();

        let themes = Themes::discover(root, &settings, &options);
        assert_eq!(names(&themes), ["Project", "Listed", "File", "Given"]);
        assert_eq!(
            themes.theme("Given").color(Token::Accent),
            Color::Rgb(1, 2, 3)
        );
        let skipped = format!(
            "skipping theme {}: name Project is already loaded from {}",
            shown("two.json"),
            shown(".scramjet/themes/a.json")
        );
        assert_eq!(
            problems(&themes),
            [
                (
                    format!("theme path is not a json file: {}", shown("notes.txt")),
                    Some(ProblemFile::Settings)
                ),
                (
                    format!("theme path does not exist: {}", shown("gone")),
                    Some(ProblemFile::Settings)
                ),
                (skipped, Some(ProblemFile::Path(root.join("two.json")))),
                (
                    format!(
                        "theme path given with --theme does not exist: {}",
                        shown("missing.json")
                    ),
                    None
                ),
            ]
        );

        // `--no-themes` leaves only those `--theme` gives.
        let none = ThemeOptions {
            no_themes: true,
            ..options
        };
        let themes = Themes::discover(root, &settings, &none);
        assert_eq!(names(&themes), ["Given", "Project"]);
        // `missing.json`, and `same.json`, which now differs from the `Project` loaded first.
        assert_eq!(problems(&themes).len(), 2);
    }

    #[test]
    fn the_theme_in_use_is_read_again_once_its_file_has_changed_and_settled() {
        let scratch = Scratch::new("watched");
        let file = scratch.0.join("ocean.json");
        // Each write of its own length: two within one tick of the clock that dates files
        // would otherwise look the same.
        let write = |accent: &str, version: usize| {
            let text = accented("Ocean", accent) + &" ".repeat(version);
            std::fs::write(&file, text).unwrap();
        };
        write("#000001", 1);
        let themes = Themes::load(&[(scratch.0.clone(), Place::Directory)]);
        let mut watched = Watched::new(themes.get("Ocean").unwrap()).unwrap();
        assert!(Watched::new(themes.get(TERMINAL_COLORS).unwrap()).is_none());
        let start = Instant::now();
        watched.checked = start;
        let at = |millis| start + Duration::from_millis(millis);
        assert_eq!(watched.due(start), CHECK);
        assert!(watched.check(at(0)).is_none());
        // Unchanged, and looked at no sooner than `CHECK` after the last time.
        assert!(watched.check(at(250)).is_none());
        write("#000002", 2);
        assert!(watched.check(at(300)).is_none());
        assert!(watched.check(at(500)).is_none());
        assert_eq!(watched.due(at(500)), SETTLE);
        // Changed again before it settled: it waits again.
        write("#000003", 3);
        assert!(watched.check(at(600)).is_none());
        let theme = watched.check(at(700)).unwrap().unwrap();
        assert_eq!(theme.color(Token::Accent), Color::Rgb(0, 0, 3));
        assert!(!watched.failed);
        assert!(watched.check(at(950)).is_none());

        // A file that is no theme says why, once, and is read again when it changes.
        write("#00000x", 4);
        assert!(watched.check(at(1200)).is_none());
        let error = watched.check(at(1300)).unwrap().unwrap_err();
        assert_eq!(error, "accent: #00000x is not #rrggbb");
        assert!(watched.failed);
        assert!(watched.check(at(1550)).is_none());
        // Gone, as while a save replaces it: nothing changes until it is back, nor is it looked
        // at sooner meanwhile.
        write("#000005", 5);
        assert!(watched.check(at(1800)).is_none());
        std::fs::remove_file(&file).unwrap();
        assert!(watched.check(at(1900)).is_none());
        assert_eq!(watched.due(at(1900)), CHECK);
        write("#000006", 6);
        assert!(watched.check(at(2150)).is_none());
        let theme = watched.check(at(2250)).unwrap().unwrap();
        assert_eq!(theme.color(Token::Accent), Color::Rgb(0, 0, 6));
        assert!(!watched.failed);
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
