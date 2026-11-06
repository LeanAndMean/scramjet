//! How the agent's terminal output looks, decided in one place: each kind of text the renderers
//! show has its style here, named for what it shows, as text attributes plus the color of one of
//! the theme's tokens. The theme in use decides the escape sequences, and the base under them on
//! the full screen: its main background, if it has one, with its text color.

use super::theme::{Color, Mode, TERMINAL_COLORS, Theme, Token};
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, RwLock};

pub const RESET: &str = "\x1b[0m";
/// The words a one-line change replaced, within a removed or added line, and where they end
/// (which must leave the line's own style on).
pub const DIFF_CHANGED: &str = "\x1b[7m";
pub const DIFF_CHANGED_END: &str = "\x1b[27m";

const BOLD: u8 = 1;
const FAINT: u8 = 2;
const ITALIC: u8 = 3;
const UNDERLINE: u8 = 4;
const REVERSE: u8 = 7;
const STRIKE: u8 = 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style(u8);

struct Spec {
    attributes: &'static [u8],
    color: Option<Token>,
}

macro_rules! styles {
    ($($(#[$doc:meta])* $name:ident: [$($attribute:ident),*] $($token:ident)?;)*) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        enum Id { $($name),* }
        $($(#[$doc])* pub const $name: Style = Style(Id::$name as u8);)*
        const SPECS: &[Spec] = &[$(Spec {
            attributes: &[$($attribute),*],
            color: styles!(@token $($token)?),
        }),*];
    };
    (@token) => { None };
    (@token $token:ident) => { Some(Token::$token) };
}

styles! {
    /// The status line and the running command's output lines above it.
    STATUS: [] Muted;
    /// The status line's spinner, in the accent color, and while a request waits to be
    /// retried, in the warning color.
    SPINNER: [] Accent;
    SPINNER_RETRYING: [] Warning;
    /// A subagent's glyph while it runs, once it is done and when it failed, and the name of
    /// the agent or of several children's mode beside it.
    SUBAGENT_RUNNING: [] Warning;
    SUBAGENT_DONE: [] Success;
    SUBAGENT_FAILED: [] Error;
    SUBAGENT_NAME: [BOLD] ToolTitle;
    /// The footer under the prompt, and how full the context is there past 70%, then past 90%.
    FOOTER: [] Dim;
    FOOTER_NEARLY_FULL: [] Warning;
    FOOTER_FULL: [] Error;
    /// Secondary notes: `Took 1.2s`, `(cancelled)`, why a compaction ran, editor hints.
    MUTED: [] Muted;
    /// A tool's result preview.
    TOOL_OUTPUT: [] ToolOutput;
    /// A failed tool's result preview.
    TOOL_FAILED: [] Error;
    /// Streamed reasoning summaries.
    REASONING: [] ThinkingText;
    /// The line shown in place of hidden reasoning.
    THINKING_HIDDEN: [ITALIC] ThinkingText;
    /// In the looks other than the classic one (`design`): reasoning, and the margin's label
    /// beside it; the mark beside a prompt, the margin's label and a prompt's text; the rails
    /// beside reasoning and a call's result, a failed one's and a running one's; a call's tool;
    /// how a call went, and the margin's label for it.
    THOUGHT: [ITALIC] ThinkingText;
    THOUGHT_LABEL: [] Dim;
    PROMPT_MARK: [] Accent;
    PROMPT_LABEL: [BOLD] Accent;
    PROMPT_TEXT: [BOLD] UserMessageText;
    RAIL: [] BorderMuted;
    RAIL_FAILED: [] Error;
    RAIL_RUNNING: [] Warning;
    TOOL_NAME: [BOLD] ToolTitle;
    /// The path in a file tool's heading, and a read's line range after it, as Scramjet's.
    TOOL_PATH: [] Accent;
    TOOL_RANGE: [] Warning;
    OUTCOME_RUNNING: [] Warning;
    OUTCOME_DONE: [] Success;
    OUTCOME_FAILED: [] Error;
    LABEL_RUNNING: [BOLD] Warning;
    LABEL_DONE: [BOLD] Success;
    LABEL_FAILED: [BOLD] Error;
    /// The banner a session starts with (`banner`): the name, its version, Scramjet's logo, a
    /// key and what it does, a heading over a list of what was loaded and the list, and the
    /// card's border and labels.
    BANNER_NAME: [BOLD] Accent;
    BANNER_VERSION: [] Dim;
    BANNER_LOGO: [] Accent;
    BANNER_KEY: [] Dim;
    BANNER_ACTION: [] Muted;
    BANNER_HEADING: [] MdHeading;
    BANNER_LIST: [] Dim;
    BANNER_BORDER: [] BorderMuted;
    BANNER_LABEL: [] Dim;
    /// The rules above and below the prompt, as Scramjet's editor border: in the color of the
    /// thinking level, `max` taking `xhigh`'s, and while the prompt is a command (`!`), in the
    /// bash-mode color.
    PROMPT_OFF: [] ThinkingOff;
    PROMPT_MINIMAL: [] ThinkingMinimal;
    PROMPT_LOW: [] ThinkingLow;
    PROMPT_MEDIUM: [] ThinkingMedium;
    PROMPT_HIGH: [] ThinkingHigh;
    PROMPT_XHIGH: [] ThinkingXhigh;
    PROMPT_COMMAND: [] BashMode;
    /// A message of the harness's own, as Scramjet's boxes show one: its text (compaction and
    /// branch summaries, what a command or skill sent) and its bold label (`[compaction]`).
    SUMMARY: [] CustomMessageText;
    MESSAGE_LABEL: [BOLD] CustomMessageLabel;
    /// A section heading in an information block such as `/session`'s.
    INFO_HEADING: [BOLD];
    /// A provider failure that is retried.
    WARNING: [] Warning;
    /// What Scramjet notifies the user of about a command, such as a blocked report.
    NOTICE: [] Warning;
    /// The record of a next step chosen or offered after a command completed.
    NEXT_STEP: [] Muted;
    /// A command the user ran, in the bash-mode color: the rules around its block when run with
    /// `!`, then with `!!` (kept out of the conversation), and its `$ command` line.
    COMMAND_RULE: [] BashMode;
    COMMAND_EXCLUDED_RULE: [] Dim;
    COMMAND_LINE: [BOLD] BashMode;
    /// How a command the user ran ended, if not with exit code 0: failed, or cancelled.
    COMMAND_FAILED: [] Error;
    COMMAND_CANCELLED: [] Warning;

    DIFF_REMOVED: [] ToolDiffRemoved;
    DIFF_ADDED: [] ToolDiffAdded;
    DIFF_CONTEXT: [] ToolDiffContext;

    /// Markdown `**strong**` text, `*emphasis*`, both (`***`), and `~~struck~~` text.
    STRONG: [BOLD];
    EMPHASIS: [ITALIC];
    STRONG_EMPHASIS: [BOLD, ITALIC];
    STRUCK: [STRIKE];
    /// A markdown link's text, and its address after it when that differs.
    LINK: [UNDERLINE] MdLink;
    LINK_URL: [] MdLinkUrl;
    /// A markdown block quote's text, and the `│` border before it.
    QUOTE: [ITALIC] MdQuote;
    QUOTE_BORDER: [] MdQuoteBorder;
    /// A markdown horizontal rule, and a list item's bullet or number.
    HORIZONTAL_RULE: [] MdHr;
    LIST_BULLET: [] MdListBullet;
    /// Markdown code spans.
    CODE: [] MdCode;
    /// The lines of a fenced code block, and its fence lines.
    CODE_BLOCK: [] MdCodeBlock;
    CODE_FENCE: [] MdCodeBlockBorder;
    /// Highlighted code, as Scramjet's theme colors it: comments (and annotations, diff
    /// headers), keywords, function names, variables (and parameters, keys, attributes),
    /// strings, numbers (and literals such as `true`), and types (and built-ins). Diff lines
    /// take `DIFF_ADDED` and `DIFF_REMOVED`; other code is plain.
    SYNTAX_COMMENT: [] SyntaxComment;
    SYNTAX_KEYWORD: [] SyntaxKeyword;
    SYNTAX_FUNCTION: [] SyntaxFunction;
    SYNTAX_VARIABLE: [] SyntaxVariable;
    SYNTAX_STRING: [] SyntaxString;
    SYNTAX_NUMBER: [] SyntaxNumber;
    SYNTAX_TYPE: [] SyntaxType;
    /// Markdown headings: level 1, then the others.
    TITLE: [BOLD, UNDERLINE] MdHeading;
    HEADING: [BOLD] MdHeading;

    /// A selector's title, its selected row and the mark before that row.
    SELECTOR_TITLE: [BOLD];
    SELECTED: [BOLD];
    SELECTOR_MARK: [] Accent;
    /// A heading among a selector's rows, over the rows it groups.
    SELECTOR_HEADING: [] Muted;
    /// The tree lines before the rows of forked sessions in `/resume` and of `/tree`'s entries.
    SELECTOR_TREE: [] Dim;
    /// In `/tree`'s rows, as Scramjet's tree colors them: the mark on the current entry's path,
    /// who wrote a message, the error a turn stopped with, compactions, a branch summary's
    /// label, the entries that are not messages (tool calls, commands the user ran, model and
    /// thinking changes) or replies without text, and the labels the user gave entries, with
    /// when.
    TREE_PATH: [] Accent;
    TREE_USER: [] Accent;
    TREE_ASSISTANT: [] Success;
    TREE_ERROR: [] Error;
    TREE_COMPACTION: [] BorderAccent;
    TREE_SUMMARY: [] Warning;
    TREE_OTHER: [] Dim;
    TREE_LABEL: [] Warning;
    TREE_LABEL_TIME: [] Muted;
    /// What a selector's key asks to confirm with the row it would remove, and why it cannot,
    /// in the error color; what it did, in the accent color.
    SELECTOR_WARNING: [] Error;
    SELECTOR_DONE: [] Accent;
    /// Text selected with the mouse in the full screen, as Scramjet's retained view shows it.
    SELECTION: [REVERSE];
    /// The cursor block after the query typed in a selector, as Scramjet's input draws it.
    SEARCH_CURSOR: [REVERSE];
    /// The selected row of the completion list under the prompt.
    COMPLETION_SELECTED: [] Accent;
    /// The mark after the current session or model in a selector.
    CURRENT: [] Success;
    /// In `/resume`, the current session's prompt or name, and other sessions' names, in the
    /// accent and warning colors.
    SESSION_CURRENT: [] Accent;
    SESSION_NAMED: [] Warning;
    /// A model `/scoped-models` has on, and one it has off.
    MODEL_ON: [] Success;
    MODEL_OFF: [] Dim;
    /// The rules above and below the list of a question the model asks, its title, and the
    /// rows of the selected option, in the border and accent colors.
    QUESTION_BORDER: [] Border;
    /// The rule under a list or another dialog, over the footer, as Scramjet's selectors' borders.
    DIALOG_RULE: [] Border;
    QUESTION_TITLE: [BOLD] Accent;
    QUESTION_SELECTED: [] Accent;
    /// No style.
    NONE: [];
}

impl Style {
    /// The escape sequence that starts the style in the theme in use; empty when it changes
    /// nothing.
    pub fn get(self) -> &'static str {
        let table = *TABLE
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        &table[self.0 as usize]
    }
}

/// The escape sequence that starts the full screen's base in the theme in use: its background
/// and text colors when it colors the background, else empty, the terminal's own.
pub(super) fn base() -> &'static str {
    let table = *TABLE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    &table[SPECS.len()]
}

/// A background across the full screen's rows: those a look puts blocks on (`design`), a
/// prompt's, and a call's while it runs, once done and when it failed; the selected row of
/// `/resume` and `/tree`, as Scramjet's selectors paint it; and the box of a message of the
/// harness's own, as Scramjet's. A row is on one when it holds its `mark`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Band {
    Prompt,
    Running,
    Done,
    Failed,
    Selected,
    Message,
}

const BANDS: [(Band, Token, char); 6] = [
    (Band::Prompt, Token::UserMessageBg, '\x11'),
    (Band::Running, Token::ToolPendingBg, '\x12'),
    (Band::Done, Token::ToolSuccessBg, '\x13'),
    (Band::Failed, Token::ToolErrorBg, '\x14'),
    (Band::Selected, Token::SelectedBg, '\x15'),
    (Band::Message, Token::CustomMessageBg, '\x16'),
];

impl Band {
    pub(super) fn mark(self) -> char {
        BANDS[self as usize].2
    }
    /// The band `mark` stands for.
    pub(super) fn marked(mark: char) -> Option<Self> {
        BANDS.iter().find(|band| band.2 == mark).map(|band| band.0)
    }
    /// As `base`, the escape sequence that starts the band in the theme in use: its background
    /// with the theme's text color, or the base where the theme has no such background.
    pub(super) fn get(self) -> &'static str {
        let table = *TABLE
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        &table[SPECS.len() + 1 + self as usize]
    }
    /// Whether the theme in use gives the band a background of its own.
    pub(super) fn painted(self) -> bool {
        self.get() != base()
    }
}

/// `row` on `base`, which each reset in it starts again.
pub(super) fn painted(row: &str, base: &str) -> String {
    match base.is_empty() {
        true => row.to_owned(),
        false => base.to_owned() + &row.replace(RESET, &(RESET.to_owned() + base)),
    }
}

impl fmt::Display for Style {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.get())
    }
}

/// The styles' escape sequences in a theme, then its base's and its bands'.
fn table(theme: &Theme, mode: Mode) -> Vec<String> {
    let sequence = |spec: &Spec| {
        let mut attributes = spec.attributes.to_vec();
        let color = match spec.color.map(|token| theme.color(token)) {
            Some(Color::Faint) => {
                attributes.push(FAINT);
                None
            }
            color => color.and_then(|color| color.foreground(mode)),
        };
        attributes.sort();
        let attributes = attributes.iter().map(u8::to_string);
        let parameters: Vec<String> = attributes.chain(color).collect();
        match parameters.is_empty() {
            true => String::new(),
            false => format!("\x1b[{}m", parameters.join(";")),
        }
    };
    let on = |background: Color| {
        background.background(mode).map(|background| {
            let text = theme.color(Token::Text).foreground(mode);
            let parameters: Vec<String> = text.into_iter().chain([background]).collect();
            format!("\x1b[{}m", parameters.join(";"))
        })
    };
    let base = on(theme.background()).unwrap_or_default();
    let bands = BANDS.map(|(_, token, _)| on(theme.color(token)).unwrap_or_else(|| base.clone()));
    let mut table: Vec<String> = SPECS.iter().map(sequence).collect();
    table.push(base);
    table.extend(bands);
    table
}

static TABLE: LazyLock<RwLock<&'static [String]>> = LazyLock::new(|| {
    let theme = Theme::built_in(TERMINAL_COLORS).expect("built in");
    RwLock::new(table(&theme, Mode::TrueColor).leak())
});
static GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Shows everything from now on in `theme`'s colors.
pub(super) fn set(theme: &Theme) {
    let table = table(theme, Mode::detect());
    let mut current = TABLE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if **current == table[..] {
        return;
    }
    // A few hundred bytes for each theme chosen, which strings already shown may still borrow.
    *current = table.leak();
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// A number that changes with each theme set, for what keeps text rendered in one.
pub(super) fn generation() -> usize {
    GENERATION.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_cli::theme::BUILT_IN;

    #[test]
    fn the_terminal_colors_are_the_16_colors_shown_before_themes() {
        let theme = Theme::built_in(TERMINAL_COLORS).unwrap();
        let table = table(&theme, Mode::TrueColor);
        let shown = |style: Style| table[style.0 as usize].as_str();
        assert_eq!(shown(STATUS), "\x1b[2m");
        assert_eq!(shown(SPINNER), "\x1b[36m");
        assert_eq!(shown(SUBAGENT_NAME), "\x1b[1m");
        assert_eq!(shown(THINKING_HIDDEN), "\x1b[2;3m");
        assert_eq!(shown(QUOTE), "\x1b[2;3m");
        assert_eq!(shown(TITLE), "\x1b[1;4;33m");
        assert_eq!(shown(LINK), "\x1b[4;34m");
        assert_eq!(shown(COMMAND_LINE), "\x1b[1;32m");
        assert_eq!(shown(SYNTAX_VARIABLE), "\x1b[35m");
        assert_eq!(shown(STRONG_EMPHASIS), "\x1b[1;3m");
        assert_eq!(shown(SELECTION), "\x1b[7m");
        assert_eq!(shown(NONE), "");
        assert_eq!(STATUS.get(), "\x1b[2m");
    }

    #[test]
    fn an_exact_theme_sends_24_bit_colors_or_the_nearest_of_256() {
        let theme = Theme::built_in("scramjet-dark").unwrap();
        let exact = table(&theme, Mode::TrueColor);
        let indexed = table(&theme, Mode::Indexed);
        let shown = |table: &[String], style: Style| table[style.0 as usize].clone();
        assert_eq!(shown(&exact, SPINNER), "\x1b[38;2;56;225;255m");
        assert_eq!(shown(&indexed, SPINNER), "\x1b[38;5;81m");
        assert_eq!(shown(&exact, TITLE), "\x1b[1;4;38;2;255;158;90m");
        assert_eq!(shown(&exact, SUBAGENT_NAME), "\x1b[1m");
        assert_eq!(shown(&exact, CODE_BLOCK), "");
    }

    #[test]
    fn a_theme_with_a_background_paints_it_under_its_text_color() {
        let base = |text: &str, mode| {
            let theme = include_str!("themes/scramjet-dark.json").replace(
                r#""text": "","#,
                &format!(r##""text": {text}, "background": "#10141c","##),
            );
            let theme = Theme::parse(&theme).unwrap();
            table(&theme, mode).swap_remove(SPECS.len())
        };
        assert_eq!(
            base(r##""#e0e4ec""##, Mode::TrueColor),
            "\x1b[38;2;224;228;236;48;2;16;20;28m"
        );
        assert_eq!(base(r#""""#, Mode::TrueColor), "\x1b[48;2;16;20;28m");
        assert_eq!(base("15", Mode::Indexed), "\x1b[97;48;5;16m");
        for name in BUILT_IN {
            let theme = Theme::built_in(name).unwrap();
            assert_eq!(table(&theme, Mode::TrueColor)[SPECS.len()], "");
        }
        // A selected row on the theme's selectedBg, or on the base where it has none.
        let selected = |name| {
            let theme = Theme::built_in(name).unwrap();
            table(&theme, Mode::TrueColor).swap_remove(SPECS.len() + 1 + Band::Selected as usize)
        };
        assert_eq!(selected("scramjet-dark"), "\x1b[48;2;31;39;54m");
        assert_eq!(selected("pi-light"), "\x1b[48;2;212;212;224m");
        assert_eq!(selected(TERMINAL_COLORS), "");

        let row = "plain \x1b[1mbold\x1b[0m end\x1b[0m";
        assert_eq!(painted(row, ""), row);
        assert_eq!(
            painted(row, "\x1b[44m"),
            "\x1b[44mplain \x1b[1mbold\x1b[0m\x1b[44m end\x1b[0m\x1b[44m"
        );
    }
}
