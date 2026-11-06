//! The theme editor's colors: OKLCH, a perceptual space in which lightness and chroma stay as
//! they are while the hue turns; contrast as WCAG 2's ratio; the harmonies, scored on hue; and a
//! whole theme made from a few main colors, every role's contrast with the main background
//! within its kind's band.

use super::theme::{TOKENS, Token};

pub(super) type Rgb = (u8, u8, u8);

/// A color in OKLCH: lightness from 0 to 1, chroma from 0 (gray), hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Oklch {
    pub(super) l: f64,
    pub(super) c: f64,
    pub(super) h: f64,
}

fn linear(channel: u8) -> f64 {
    let c = channel as f64 / 255.0;
    match c <= 0.04045 {
        true => c / 12.92,
        false => ((c + 0.055) / 1.055).powf(2.4),
    }
}

fn encoded(value: f64) -> u8 {
    let v = value.clamp(0.0, 1.0);
    let c = match v <= 0.0031308 {
        true => 12.92 * v,
        false => 1.055 * v.powf(1.0 / 2.4) - 0.055,
    };
    (c * 255.0).round() as u8
}

impl Oklch {
    pub(super) fn of((r, g, b): Rgb) -> Self {
        let (r, g, b) = (linear(r), linear(g), linear(b));
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
        let b = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
        Self {
            l: 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
            c: a.hypot(b),
            h: b.atan2(a).to_degrees().rem_euclid(360.0),
        }
    }

    fn linear(self) -> [f64; 3] {
        let (a, b) = (
            self.c * self.h.to_radians().cos(),
            self.c * self.h.to_radians().sin(),
        );
        let l = (self.l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
        let m = (self.l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
        let s = (self.l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
        [
            4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
            -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
            -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
        ]
    }

    /// As a screen shows it, in sRGB: with less chroma where there is too much for its
    /// lightness and hue.
    pub(super) fn rgb(self) -> Rgb {
        let fits = |color: Oklch| {
            (color.linear().iter()).all(|&value| (-1e-4..=1.0 + 1e-4).contains(&value))
        };
        let mut color = Oklch {
            l: self.l.clamp(0.0, 1.0),
            c: self.c.max(0.0),
            ..self
        };
        if !fits(color) {
            let (mut low, mut high) = (0.0, color.c);
            for _ in 0..24 {
                let middle = (low + high) / 2.0;
                match fits(Oklch { c: middle, ..color }) {
                    true => low = middle,
                    false => high = middle,
                }
            }
            color.c = low;
        }
        let [r, g, b] = color.linear();
        (encoded(r), encoded(g), encoded(b))
    }
}

/// WCAG 2's relative luminance.
pub(super) fn luminance((r, g, b): Rgb) -> f64 {
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG 2's contrast ratio, from 1 (none) to 21 (black on white).
pub(super) fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The turn from hue `from` to hue `to`, the shorter way round: from -180 to 180 degrees.
fn turn(from: f64, to: f64) -> f64 {
    let turn = (to - from).rem_euclid(360.0);
    if turn > 180.0 { turn - 360.0 } else { turn }
}

/// A hue's everyday name, as OKLCH places the named colors: dark cyan is teal and dark magenta
/// purple; `gray` with hardly any chroma.
pub(super) fn hue_name(color: Oklch) -> &'static str {
    const NAMES: [(f64, &str); 12] = [
        (15.0, "pink"),
        (45.0, "red"),
        (70.0, "orange"),
        (100.0, "amber"),
        (125.0, "yellow"),
        (140.0, "lime"),
        (175.0, "green"),
        (215.0, "cyan"),
        (245.0, "azure"),
        (280.0, "blue"),
        (315.0, "violet"),
        (340.0, "magenta"),
    ];
    if color.c < 0.03 {
        return "gray";
    }
    let name = (NAMES.iter())
        .find(|(end, _)| color.h < *end)
        .map_or("pink", |(_, name)| name);
    match name {
        "cyan" if color.l < 0.6 => "teal",
        "magenta" if color.l < 0.55 => "purple",
        name => name,
    }
}

/// Main colors that go together by where their hues are on the wheel, as artists' color
/// schemes do.
pub(super) struct Harmony {
    pub(super) name: &'static str,
    /// Its hues, in degrees from the first.
    offsets: &'static [f64],
    /// What it is, in one plain line.
    pub(super) about: &'static str,
}

impl Harmony {
    pub(super) fn count(&self) -> usize {
        self.offsets.len()
    }
}

/// The harmonies, by how many main colors they have.
pub(super) const HARMONIES: [Harmony; 11] = [
    Harmony {
        name: "monochrome",
        offsets: &[0.0],
        about: "One hue in several shades: calm and unified.",
    },
    Harmony {
        name: "complementary",
        offsets: &[0.0, 180.0],
        about: "Two hues from opposite sides of the wheel: bold, each makes the other stand out.",
    },
    Harmony {
        name: "analogous",
        offsets: &[0.0, 30.0],
        about: "Two neighboring hues: gentle and close.",
    },
    Harmony {
        name: "triadic",
        offsets: &[0.0, 120.0, 240.0],
        about: "Three hues evenly around the wheel: lively and balanced.",
    },
    Harmony {
        name: "split-complementary",
        offsets: &[0.0, 150.0, 210.0],
        about: "One hue and the two beside its opposite: contrast, with less tension than two \
                opposites.",
    },
    Harmony {
        name: "analogous",
        offsets: &[-30.0, 0.0, 30.0],
        about: "Three neighboring hues: smooth, like a gradient.",
    },
    Harmony {
        name: "square",
        offsets: &[0.0, 90.0, 180.0, 270.0],
        about: "Four hues evenly around the wheel: rich and playful.",
    },
    Harmony {
        name: "rectangle",
        offsets: &[0.0, 60.0, 180.0, 240.0],
        about: "Two pairs of opposite hues: varied, one pair warmer than the other.",
    },
    Harmony {
        name: "analogous",
        offsets: &[-45.0, -15.0, 15.0, 45.0],
        about: "Four neighboring hues across a quarter of the wheel: smooth and warm or cool.",
    },
    Harmony {
        name: "pentagon",
        offsets: &[0.0, 72.0, 144.0, 216.0, 288.0],
        about: "Five hues evenly around the wheel: colorful, each one distinct.",
    },
    Harmony {
        name: "analogous",
        offsets: &[-60.0, -30.0, 0.0, 30.0, 60.0],
        about: "Five neighboring hues across a third of the wheel: a rainbow's slice.",
    },
];

/// The harmonies with `count` main colors, as indices into `HARMONIES`.
pub(super) fn harmonies(count: usize) -> Vec<usize> {
    (0..HARMONIES.len())
        .filter(|&index| HARMONIES[index].count() == count)
        .collect()
}

/// How far a hue may be from one of a harmony's own and still be in it: each is a sector.
pub(super) const SECTOR: f64 = 10.0;
/// How far outside its sector a hue is, on average, when the harmony's score is 0.
const MISS: f64 = 60.0;

/// How main colors fit a harmony.
#[derive(Debug, PartialEq)]
pub(super) struct Fit {
    /// How far the harmony's hues are turned around the wheel.
    pub(super) rotation: f64,
    /// Each color's place among the harmony's hues.
    pub(super) places: Vec<usize>,
    /// From 0 to 1, when every color is in its sector.
    pub(super) score: f64,
}

/// Every way to give each of `count` colors a place of its own among `places`.
fn arrangements(count: usize, places: usize) -> Vec<Vec<usize>> {
    let mut all: Vec<Vec<usize>> = vec![Vec::new()];
    for _ in 0..count {
        let mut longer = Vec::new();
        for taken in &all {
            for place in (0..places).filter(|place| !taken.contains(place)) {
                longer.push([taken.as_slice(), &[place]].concat());
            }
        }
        all = longer;
    }
    all
}

/// How well `hues`, each with its weight, fit the harmony at `offsets`, turned as best fits
/// them or as `rotation` gives, each in a place of its own. A hue counts by how far it is outside
/// its sector, by its weight; among turns that fit as well, the one nearest each sector's middle.
pub(super) fn fit(hues: &[(f64, f64)], offsets: &[f64], rotation: Option<f64>) -> Fit {
    let mut arrangements = arrangements(hues.len(), offsets.len());
    let cost = |rotation: f64, places: &[usize]| -> (f64, f64) {
        (hues.iter().zip(places)).fold((0.0, 0.0), |(miss, off), (&(hue, weight), &place)| {
            let away = turn(hue, rotation + offsets[place]).abs();
            let outside = (away - SECTOR).max(0.0);
            (
                miss + weight * outside * outside,
                off + weight * away * away,
            )
        })
    };
    let search = |best: &mut (f64, f64, usize), rotations: &mut dyn Iterator<Item = f64>| {
        for rotation in rotations {
            for (index, places) in arrangements.iter().enumerate() {
                let (miss, off) = cost(rotation, places);
                let total = miss + 1e-4 * off;
                if total < best.0 {
                    *best = (total, rotation, index);
                }
            }
        }
    };
    let mut best = (f64::INFINITY, 0.0, 0);
    match rotation {
        Some(rotation) => search(&mut best, &mut std::iter::once(rotation)),
        None => {
            search(&mut best, &mut (0..360).map(f64::from));
            let around = best.1;
            let near = (-20..=20).map(|step| around + f64::from(step) * 0.05);
            search(&mut best, &mut near.into_iter());
        }
    }
    let places = arrangements.swap_remove(best.2);
    let weight: f64 = hues.iter().map(|&(_, weight)| weight).sum();
    let (miss, _) = cost(best.1, &places);
    let score = match weight <= 1e-9 {
        true => 1.0,
        false => 1.0 - ((miss / weight).sqrt() / MISS).min(1.0),
    };
    Fit {
        rotation: best.1.rem_euclid(360.0),
        places,
        score,
    }
}

/// A seeded generator of numbers that look random, SplitMix64's.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// From 0 to 1.
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            items.swap(index, (self.next() % (index as u64 + 1)) as usize);
        }
    }
}

/// How far from its harmony's own a rolled hue may land: at times past its sector, which a
/// step toward the harmony mends.
const JITTER: f64 = 16.0;

/// One of a theme's main colors: its hue and chroma, and where its contrast is in the
/// accents' band, from 0 (the least) to 1.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Main {
    pub(super) hue: f64,
    pub(super) chroma: f64,
    pub(super) lift: f64,
}

/// What the user made of a role: lit as its kind is from another main color than its own, out
/// of any meaning's hue range, or an exact color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Pick {
    Follow(usize),
    Exact(Rgb),
}

/// The main background a theme is lit on: dark or light, tinted toward the first main color, or
/// the terminal's own, which the theme leaves to the terminal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Background {
    Dark,
    Light,
    Terminal(Rgb),
}

/// What the theme editor makes a theme from: its main colors in a harmony, its main
/// background, how much contrast, and what the user picked for single roles.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Draft {
    /// An index into `HARMONIES`, with as many hues as there are main colors.
    pub(super) harmony: usize,
    pub(super) mains: Vec<Main>,
    pub(super) background: Background,
    /// A dark or light background's place in its range of lightness, from 0 (the darkest), and
    /// how much of the first main color's hue it has, from 0 (gray) to 1.
    pub(super) depth: f64,
    pub(super) tint: f64,
    /// Steps of contrast above or below each band's middle.
    pub(super) contrast: i32,
    /// Each token's pick, in `TOKENS`' order; `None` follows its own source.
    pub(super) picks: Vec<Option<Pick>>,
}

/// The highest step of contrast either way, at which every role is at its band's edge.
pub(super) const CONTRAST_STEPS: i32 = 3;

/// What a role is drawn as: its contrast band and how much chroma it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Kind {
    Text,
    /// Secondary text: notes, reasoning, tool output, quotes.
    Secondary,
    /// Dim text: comments, link addresses.
    Dim,
    /// Rules and quiet borders.
    Faint,
    Accent,
    /// Blocks' backgrounds under text.
    Block,
}

impl Kind {
    /// The WCAG 2 contrast ratios with the main background it stays between: readable, and
    /// not harsh.
    pub(super) fn band(self) -> (f64, f64) {
        match self {
            Self::Text => (8.0, 16.0),
            Self::Secondary => (4.8, 8.0),
            Self::Dim => (3.8, 6.0),
            Self::Faint => (2.0, 3.6),
            Self::Accent => (4.5, 11.0),
            Self::Block => (1.05, 1.3),
        }
    }

    /// The chroma it has at most, tinted toward its main color; accents have their main's.
    fn chroma(self) -> f64 {
        match self {
            Self::Text => 0.012,
            Self::Secondary => 0.022,
            Self::Dim | Self::Faint => 0.03,
            Self::Block => 0.035,
            Self::Accent => f64::INFINITY,
        }
    }
}

/// Colors with a meaning, which keep their hue ranges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Meaning {
    Success,
    Error,
    Warning,
}

impl Meaning {
    /// Its hues, in OKLCH degrees.
    fn range(self) -> (f64, f64) {
        match self {
            Self::Error => (10.0, 40.0),
            Self::Warning => (65.0, 100.0),
            Self::Success => (130.0, 165.0),
        }
    }

    /// Its hues and what they mean, in plain words.
    pub(super) fn words(self) -> &'static str {
        match self {
            Self::Error => "red, for errors",
            Self::Warning => "amber, for warnings",
            Self::Success => "green, for success",
        }
    }

    /// Its hue: of the hues in its range, the one nearest a main color's, which is that main
    /// color's own when one is in the range.
    fn hue(self, mains: &[Main]) -> f64 {
        let (from, to) = self.range();
        let nearest = |hue: f64| match hue >= from && hue <= to {
            true => (hue, 0.0),
            false => {
                let (before, after) = (turn(hue, from).abs(), turn(hue, to).abs());
                if before <= after {
                    (from, before)
                } else {
                    (to, after)
                }
            }
        };
        (mains.iter())
            .map(|main| nearest(main.hue))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or((from + to) / 2.0, |(hue, _)| hue)
    }
}

/// Where a role takes its hue and chroma from: a slot of the main colors, the first ones each
/// a main color in turn and the later ones each a shade of one, or a meaning.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Source {
    Main(usize),
    Meaning(Meaning),
}

/// How the later slots change a main color: a turn of its place in the band (folded back at
/// its ends) and a factor of its chroma, so that one main color gives several accents.
const SHADES: [(f64, f64); 6] = [
    (0.0, 1.0),
    (0.35, 0.7),
    (-0.3, 1.15),
    (0.6, 0.5),
    (-0.5, 0.85),
    (0.2, 0.4),
];

/// Each token's kind, source and, but for accents of a main color, which use its `lift`, its
/// place in its kind's band.
fn role(token: Token) -> (Kind, Source, f64) {
    use Kind as K;
    use Meaning as M;
    use Token as T;
    let main = Source::Main;
    match token {
        T::Accent
        | T::BorderAccent
        | T::MdCode
        | T::MdListBullet
        | T::SyntaxType
        | T::ThinkingMedium => (K::Accent, main(0), 0.0),
        T::Border | T::MdLink | T::MdQuoteBorder | T::SyntaxKeyword | T::ThinkingLow => {
            (K::Accent, main(1), 0.0)
        }
        T::MdHeading | T::SyntaxFunction | T::CustomMessageLabel | T::ThinkingXhigh => {
            (K::Accent, main(2), 0.0)
        }
        T::SyntaxVariable | T::ThinkingHigh => (K::Accent, main(3), 0.0),
        T::SyntaxString => (K::Accent, main(4), 0.0),
        T::SyntaxNumber | T::SyntaxOperator => (K::Accent, main(5), 0.0),
        T::Success | T::ToolDiffAdded | T::BashMode => {
            (K::Accent, Source::Meaning(M::Success), 0.5)
        }
        T::Error | T::ToolDiffRemoved | T::SpellcheckError => {
            (K::Accent, Source::Meaning(M::Error), 0.5)
        }
        T::Warning => (K::Accent, Source::Meaning(M::Warning), 0.5),
        T::Text | T::UserMessageText | T::CustomMessageText | T::ToolTitle | T::MdCodeBlock => {
            (K::Text, main(0), 0.5)
        }
        T::Muted | T::ThinkingText | T::ToolOutput | T::MdQuote | T::ToolDiffContext => {
            (K::Secondary, main(0), 0.5)
        }
        T::Dim | T::SyntaxComment | T::SyntaxPunctuation | T::ThinkingMinimal => {
            (K::Dim, main(0), 0.5)
        }
        T::MdLinkUrl => (K::Dim, main(1), 0.5),
        T::BorderMuted | T::MdCodeBlockBorder | T::MdHr | T::ThinkingOff => {
            (K::Faint, main(0), 0.5)
        }
        T::UserMessageBg => (K::Block, main(0), 0.6),
        T::ToolPendingBg => (K::Block, main(0), 0.3),
        T::SelectedBg => (K::Block, main(1), 0.8),
        T::CustomMessageBg => (K::Block, main(2), 0.5),
        T::ToolSuccessBg => (K::Block, Source::Meaning(M::Success), 0.45),
        T::ToolErrorBg => (K::Block, Source::Meaning(M::Error), 0.45),
    }
}

/// What `token` is drawn as, and where it takes its color from unless a pick says otherwise.
pub(super) fn own(token: Token) -> (Kind, Source) {
    let (kind, source, _) = role(token);
    (kind, source)
}

/// `place` folded back into 0 to 1 at its ends.
fn folded(place: f64) -> f64 {
    let place = place.rem_euclid(2.0);
    if place > 1.0 { 2.0 - place } else { place }
}

/// A theme's colors: each token's, in `TOKENS`' order, and the main background they are lit
/// on, which the theme sets unless it is the terminal's own.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Built {
    pub(super) colors: Vec<Rgb>,
    pub(super) background: Rgb,
    pub(super) terminal: bool,
}

impl Built {
    pub(super) fn color(&self, token: Token) -> Rgb {
        self.colors[token as usize]
    }

    /// The main background the theme sets: none on the terminal's own.
    pub(super) fn own_background(&self) -> Option<Rgb> {
        (!self.terminal).then_some(self.background)
    }
}

impl Draft {
    /// A draft in `harmony`, its main colors and background rolled.
    pub(super) fn rolled(harmony: usize, background: Background, rng: &mut Rng) -> Self {
        let mut draft = Self {
            harmony,
            mains: Vec::new(),
            background,
            depth: 0.5,
            tint: 0.5,
            contrast: 0,
            picks: vec![None; TOKENS.len()],
        };
        draft.roll(&[], rng);
        draft
    }

    /// The main colors' hues, each with its chroma as its weight, so that grays barely count.
    fn hues(&self) -> Vec<(f64, f64)> {
        (self.mains.iter())
            .map(|main| (main.hue, main.chroma))
            .collect()
    }

    /// How the main colors fit the harmony. Those `locked` marks anchor it: it is turned as
    /// best fits them, and the others go to the places left.
    pub(super) fn fit(&self, locked: &[bool]) -> Fit {
        let offsets = HARMONIES[self.harmony].offsets;
        let anchors: Vec<(f64, f64)> = (self.hues().into_iter().enumerate())
            .filter(|&(index, _)| locked.get(index) == Some(&true))
            .map(|(_, hue)| hue)
            .collect();
        let rotation = match anchors.is_empty() {
            true => None,
            false => Some(fit(&anchors, offsets, None).rotation),
        };
        fit(&self.hues(), offsets, rotation)
    }

    /// Rolls the main colors but those `locked` marks, each a hue of the harmony around the
    /// locked ones, with its chroma and contrast, and the main background's depth and tint.
    pub(super) fn roll(&mut self, locked: &[bool], rng: &mut Rng) {
        let offsets = HARMONIES[self.harmony].offsets;
        let count = offsets.len();
        let kept: Vec<usize> = (0..self.mains.len().min(count))
            .filter(|&index| locked.get(index) == Some(&true))
            .collect();
        let (rotation, taken) = match kept.is_empty() {
            true => (rng.between(0.0, 360.0), Vec::new()),
            false => {
                let hues: Vec<_> = (kept.iter())
                    .map(|&index| (self.mains[index].hue, self.mains[index].chroma))
                    .collect();
                let fit = fit(&hues, offsets, None);
                (fit.rotation, fit.places)
            }
        };
        let mut free: Vec<usize> = (0..count).filter(|place| !taken.contains(place)).collect();
        rng.shuffle(&mut free);
        let chroma = rng.between(0.07, 0.19);
        let mains = (0..count)
            .map(|index| match kept.contains(&index) {
                true => self.mains[index].clone(),
                false => {
                    let place = free.pop().expect("a place for each color not kept");
                    let hue = rotation + offsets[place] + rng.between(-JITTER, JITTER);
                    Main {
                        hue: hue.rem_euclid(360.0),
                        chroma: (chroma + rng.between(-0.03, 0.03)).clamp(0.04, 0.24),
                        lift: rng.unit(),
                    }
                }
            })
            .collect();
        self.mains = mains;
        self.depth = rng.unit();
        self.tint = rng.unit();
    }

    /// Turns each main color `locked` leaves free a step toward its hue in the harmony: a third
    /// of the way into its sector, at least 2 degrees, so that steps end in it. `false` when
    /// none had a step to take.
    pub(super) fn harmonize(&mut self, locked: &[bool]) -> bool {
        let offsets = HARMONIES[self.harmony].offsets;
        let fit = self.fit(locked);
        let mut moved = false;
        for (index, main) in self.mains.iter_mut().enumerate() {
            if locked.get(index) == Some(&true) {
                continue;
            }
            let off = turn(main.hue, fit.rotation + offsets[fit.places[index]]);
            let outside = off.abs() - SECTOR;
            if outside <= 1e-9 {
                continue;
            }
            let step = (outside / 3.0).max(2.0).min(outside);
            main.hue = (main.hue + step * off.signum()).rem_euclid(360.0);
            moved = true;
        }
        moved
    }

    /// The main background, then each token's color.
    pub(super) fn build(&self) -> Built {
        let first = &self.mains[0];
        let ((low, high), tint) = match self.background {
            Background::Light => ((0.955, 0.995), 0.02),
            Background::Dark => ((0.14, 0.23), 0.035),
            Background::Terminal(background) => return self.built(background, true),
        };
        let background = Oklch {
            l: low + (high - low) * self.depth,
            c: tint * self.tint,
            h: first.hue,
        }
        .rgb();
        self.built(background, false)
    }

    fn built(&self, background: Rgb, terminal: bool) -> Built {
        let colors = (TOKENS.iter())
            .map(|&token| self.color(token, background))
            .collect();
        Built {
            colors,
            background,
            terminal,
        }
    }

    /// The color of `slot` as an accent: what the editor shows for a main color.
    pub(super) fn accent(&self, slot: usize, background: Rgb) -> Rgb {
        self.lit(Kind::Accent, Source::Main(slot), 0.0, background)
    }

    fn color(&self, token: Token, background: Rgb) -> Rgb {
        let (kind, source, place) = role(token);
        match self.picks[token as usize] {
            None => self.lit(kind, source, place, background),
            Some(Pick::Follow(index)) => self.lit(kind, Source::Main(index), place, background),
            Some(Pick::Exact(rgb)) => rgb,
        }
    }

    fn lit(&self, kind: Kind, source: Source, place: f64, background: Rgb) -> Rgb {
        let count = self.mains.len();
        let mean = self.mains.iter().map(|main| main.chroma).sum::<f64>() / count as f64;
        let (hue, chroma, place) = match source {
            Source::Main(slot) => {
                let main = &self.mains[slot % count];
                let (turn, factor) = SHADES[(slot / count).min(SHADES.len() - 1)];
                match kind {
                    Kind::Accent => (main.hue, main.chroma * factor, folded(main.lift + turn)),
                    kind => (main.hue, kind.chroma().min(main.chroma), place),
                }
            }
            Source::Meaning(meaning) => {
                let chroma = match kind {
                    Kind::Accent => mean.max(0.13),
                    kind => kind.chroma(),
                };
                (meaning.hue(&self.mains), chroma, place)
            }
        };
        let place = (place + 0.15 * f64::from(self.contrast)).clamp(0.0, 1.0);
        let (low, high) = kind.band();
        let target = (low.ln() + (high.ln() - low.ln()) * place).exp();
        lit(hue, chroma, background, target)
    }
}

/// The color of `hue` and `chroma` whose contrast with `background` is nearest `target`: only
/// its lightness is chosen, lighter than a dark background or darker than a light one.
pub(super) fn lit(hue: f64, chroma: f64, background: Rgb, target: f64) -> Rgb {
    let dark = is_dark(background);
    let from = Oklch::of(background).l;
    let (mut low, mut high) = if dark { (from, 1.0) } else { (0.0, from) };
    let at = |l: f64| {
        Oklch {
            l,
            c: chroma,
            h: hue,
        }
        .rgb()
    };
    for _ in 0..32 {
        let middle = (low + high) / 2.0;
        match (contrast(at(middle), background) < target) == dark {
            true => low = middle,
            false => high = middle,
        }
    }
    // Of the two colors around the target, the one nearer it.
    let (low, high) = (at(low), at(high));
    let miss = |color| (contrast(color, background) / target).ln().abs();
    if miss(low) <= miss(high) { low } else { high }
}

/// Whether text on `background` is light: it is darker than the middle gray.
pub(super) fn is_dark(background: Rgb) -> bool {
    luminance(background) < 0.18
}

/// A contrast ratio in plain words, by where it is in `kind`'s band.
pub(super) fn contrast_words(ratio: f64, kind: Kind) -> &'static str {
    let (low, high) = kind.band();
    let place = (ratio.ln() - low.ln()) / (high.ln() - low.ln());
    match place {
        place if place < -0.02 => "too faint",
        place if place < 0.25 => "a bit faint",
        place if place <= 0.75 => "good",
        place if place <= 1.02 => "strong",
        _ => "harsh",
    }
}

/// A harmony's score in plain words.
pub(super) fn harmony_words(score: f64) -> &'static str {
    match score {
        score if score >= 0.995 => "in harmony",
        score if score >= 0.9 => "cohesive",
        score if score >= 0.75 => "a little loose",
        score if score >= 0.5 => "loose",
        _ => "scattered",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, within: f64) -> bool {
        (a - b).abs() <= within
    }

    #[test]
    fn colors_go_to_oklch_and_back_as_they_were() {
        let white = Oklch::of((255, 255, 255));
        assert!(close(white.l, 1.0, 1e-3) && white.c < 1e-3, "{white:?}");
        let red = Oklch::of((255, 0, 0));
        assert!(close(red.l, 0.628, 1e-3), "{red:?}");
        assert!(
            close(red.c, 0.2577, 1e-3) && close(red.h, 29.23, 0.05),
            "{red:?}"
        );
        for rgb in [
            (0, 0, 0),
            (56, 225, 255),
            (255, 158, 90),
            (16, 20, 28),
            (128, 128, 128),
        ] {
            assert_eq!(Oklch::of(rgb).rgb(), rgb);
        }
        // Past what sRGB shows, chroma gives way and the lightness and hue stay.
        let vivid = Oklch {
            l: 0.9,
            c: 0.3,
            h: 264.0,
        }
        .rgb();
        let shown = Oklch::of(vivid);
        assert!(
            close(shown.l, 0.9, 0.01) && close(shown.h, 264.0, 2.0),
            "{shown:?}"
        );
        assert!(shown.c < 0.1, "{shown:?}");
    }

    #[test]
    fn contrast_is_wcag_2s_ratio() {
        assert!(close(contrast((0, 0, 0), (255, 255, 255)), 21.0, 1e-9));
        assert!(close(contrast((255, 255, 255), (255, 255, 255)), 1.0, 1e-9));
        assert!(close(
            contrast((118, 118, 118), (255, 255, 255)),
            4.54,
            0.01
        ));
        assert!(is_dark((16, 20, 28)) && !is_dark((250, 250, 250)));
    }

    #[test]
    fn a_color_is_lit_to_the_contrast_asked_for_on_dark_and_light_backgrounds() {
        for background in [(16, 20, 28), (40, 30, 50), (250, 248, 240)] {
            for target in [1.1, 4.5, 7.0, 12.0] {
                let color = lit(200.0, 0.12, background, target);
                let ratio = contrast(color, background);
                assert!(
                    close(ratio, target, target * 0.02),
                    "{background:?} {target}: {ratio}"
                );
                let (lighter, dark) = (
                    Oklch::of(color).l > Oklch::of(background).l,
                    is_dark(background),
                );
                assert_eq!(lighter, dark, "{background:?} {target}");
            }
        }
    }

    #[test]
    fn hues_have_everyday_names() {
        let name = |rgb| hue_name(Oklch::of(rgb));
        assert_eq!(name((255, 0, 0)), "red");
        assert_eq!(name((0, 255, 0)), "green");
        assert_eq!(name((0, 0, 255)), "blue");
        assert_eq!(name((0, 255, 255)), "cyan");
        assert_eq!(name((255, 158, 90)), "orange");
        assert_eq!(name((185, 140, 255)), "violet");
        assert_eq!(name((0, 128, 128)), "teal");
        assert_eq!(name((128, 0, 128)), "purple");
        assert_eq!(name((255, 105, 180)), "pink");
        assert_eq!(name((128, 130, 128)), "gray");
    }

    #[test]
    fn each_count_of_main_colors_has_harmonies_described_in_a_line() {
        for count in 1..=5 {
            let found = harmonies(count);
            assert!(!found.is_empty(), "{count}");
            for index in found {
                let harmony = &HARMONIES[index];
                assert!(harmony.about.ends_with('.') && !harmony.about.contains('\n'));
            }
        }
        assert_eq!(harmonies(3).len(), 3);
    }

    #[test]
    fn hues_in_their_sectors_score_full_and_drift_lowers_the_score_by_chroma() {
        let triadic = HARMONIES[3].offsets;
        let perfect = fit(&[(20.0, 0.1), (140.0, 0.1), (260.0, 0.1)], triadic, None);
        assert!(close(perfect.score, 1.0, 1e-9), "{perfect:?}");
        // In any order, each in a place of its own: three hues together are no triad.
        let mixed = fit(&[(265.0, 0.1), (25.0, 0.1), (138.0, 0.1)], triadic, None);
        assert!(close(mixed.score, 1.0, 1e-9), "{mixed:?}");
        let mut places = mixed.places.clone();
        places.sort();
        assert_eq!(places, [0, 1, 2]);
        let bunched = fit(&[(20.0, 0.1), (30.0, 0.1), (40.0, 0.1)], triadic, None);
        assert!(bunched.score < 0.2, "{bunched:?}");
        // A hue 30 degrees out, which the best turn shares among them, counts for less as it
        // grays.
        let off = |chroma| fit(&[(20.0, 0.1), (170.0, chroma), (260.0, 0.1)], triadic, None);
        assert!(off(0.1).score < 0.95, "{:?}", off(0.1));
        let held = |chroma| {
            fit(
                &[(20.0, 0.1), (170.0, chroma), (260.0, 0.1)],
                triadic,
                Some(20.0),
            )
        };
        assert!(held(0.01).score > held(0.1).score + 0.1);
        // A turn given is kept, and the hues are scored against it.
        let turned = fit(
            &[(20.0, 0.1), (140.0, 0.1), (260.0, 0.1)],
            triadic,
            Some(50.0),
        );
        assert_eq!(turned.rotation, 50.0);
        assert!(turned.score < 0.9, "{turned:?}");
    }

    #[test]
    fn a_roll_keeps_the_locked_colors_and_places_the_others_around_them() {
        let mut rng = Rng::new(7);
        for (index, harmony) in HARMONIES.iter().enumerate() {
            let mut draft = Draft::rolled(index, Background::Dark, &mut rng);
            let count = harmony.count();
            assert_eq!(draft.mains.len(), count);
            let locked: Vec<bool> = (0..count).map(|index| index == 0).collect();
            let first = draft.mains[0].clone();
            for _ in 0..20 {
                draft.roll(&locked, &mut rng);
                assert_eq!(draft.mains[0], first, "{}", harmony.name);
                // Rolled within a few degrees of their sectors, around the locked one.
                let fit = draft.fit(&locked);
                assert!(fit.score > 0.85, "{} {fit:?}", harmony.name);
            }
        }
    }

    #[test]
    fn steps_toward_the_harmony_end_in_a_perfect_score_and_leave_locked_colors() {
        let mut draft = Draft::rolled(4, Background::Dark, &mut Rng::new(3));
        draft.mains[1].hue = (draft.mains[0].hue + 100.0).rem_euclid(360.0);
        draft.mains[2].hue = (draft.mains[0].hue + 250.0).rem_euclid(360.0);
        let locked = [true, false, false];
        let first = draft.mains[0].clone();
        let before = draft.fit(&locked).score;
        assert!(before < 0.8, "{before}");
        let mut steps = 0;
        while draft.harmonize(&locked) {
            steps += 1;
            assert!(steps < 30);
        }
        assert!(steps > 1);
        assert_eq!(draft.mains[0], first);
        assert!(close(draft.fit(&locked).score, 1.0, 1e-9));
        assert!(close(
            fit(&draft.hues(), HARMONIES[4].offsets, None).score,
            1.0,
            1e-9
        ));
    }

    /// Every token's contrast with the main background, with the band of its kind.
    fn contrasts(built: &Built) -> Vec<(Token, f64, (f64, f64))> {
        (TOKENS.iter())
            .map(|&token| {
                let ratio = contrast(built.color(token), built.background);
                (token, ratio, role(token).0.band())
            })
            .collect()
    }

    #[test]
    fn every_role_of_a_rolled_theme_keeps_its_contrast_within_its_band() {
        let mut rng = Rng::new(11);
        // Terminals' own: Windows Terminal's Campbell, Solarized's, Catppuccin Mocha's, One Half
        // Light's, black and white.
        let terminals = [
            (12, 12, 12),
            (0, 43, 54),
            (30, 30, 46),
            (0, 0, 0),
            (250, 250, 250),
            (253, 246, 227),
            (255, 255, 255),
        ];
        let backgrounds = [Background::Dark, Background::Light]
            .into_iter()
            .chain(terminals.map(Background::Terminal));
        for background in backgrounds {
            let rolls = match background {
                Background::Terminal(_) => 3,
                _ => 8,
            };
            for harmony in 0..HARMONIES.len() {
                for _ in 0..rolls {
                    let mut draft = Draft::rolled(harmony, background, &mut rng);
                    for steps in [-CONTRAST_STEPS, 0, CONTRAST_STEPS] {
                        draft.contrast = steps;
                        let built = draft.build();
                        match background {
                            Background::Dark => assert!(is_dark(built.background)),
                            Background::Light => assert!(!is_dark(built.background)),
                            Background::Terminal(rgb) => {
                                assert_eq!((built.background, built.own_background()), (rgb, None))
                            }
                        }
                        for (token, ratio, (low, high)) in contrasts(&built) {
                            assert!(
                                ratio >= low * 0.98 && ratio <= high * 1.02,
                                "{token:?} {ratio} not in {low}-{high}: {draft:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn contrast_steps_change_only_lightness_and_meanings_keep_their_hues() {
        let mut draft = Draft::rolled(3, Background::Dark, &mut Rng::new(5));
        let middle = draft.build();
        draft.contrast = 2;
        let higher = draft.build();
        assert_eq!(middle.background, higher.background);
        for token in [Token::Text, Token::Muted, Token::Accent, Token::Border] {
            let (before, after) = (middle.color(token), higher.color(token));
            let (b, a) = (Oklch::of(before), Oklch::of(after));
            assert!(a.l > b.l, "{token:?}");
            // Grayer colors' hues move more as they are rounded to 8 bits a channel.
            let within = if b.c > 0.05 { 3.0 } else { 15.0 };
            assert!(close(a.h, b.h, within), "{token:?} {b:?} {a:?}");
            let bg = middle.background;
            assert!(contrast(after, bg) > contrast(before, bg), "{token:?}");
        }
        for (token, meaning) in [
            (Token::Error, Meaning::Error),
            (Token::Success, Meaning::Success),
            (Token::Warning, Meaning::Warning),
        ] {
            let hue = Oklch::of(middle.color(token)).h;
            let (from, to) = meaning.range();
            assert!(hue >= from - 3.0 && hue <= to + 3.0, "{token:?} {hue}");
        }
    }

    #[test]
    fn a_meaning_takes_a_main_colors_hue_in_its_range_else_the_nearest_end() {
        let main = |hue| Main {
            hue,
            chroma: 0.1,
            lift: 0.5,
        };
        assert_eq!(Meaning::Success.hue(&[main(300.0), main(150.0)]), 150.0);
        assert_eq!(Meaning::Success.hue(&[main(200.0)]), 165.0);
        assert_eq!(Meaning::Error.hue(&[main(350.0)]), 10.0);
        assert_eq!(Meaning::Warning.hue(&[main(250.0), main(55.0)]), 65.0);
    }

    #[test]
    fn scores_are_said_in_plain_words() {
        assert_eq!(harmony_words(1.0), "in harmony");
        assert_eq!(harmony_words(0.82), "a little loose");
        assert_eq!(contrast_words(4.0, Kind::Dim), "a bit faint");
        assert_eq!(contrast_words(11.3, Kind::Text), "good");
        assert_eq!(contrast_words(2.5, Kind::Secondary), "too faint");
        assert_eq!(contrast_words(19.0, Kind::Text), "harsh");
    }
}
