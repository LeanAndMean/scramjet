//! Text as the terminal shows it: wrapped between words, cut to a width, made safe to print.
use scramjet_rs::agent::terminal_line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Marks in text rendered for the full screen, which takes them out of its lines
/// (`selection::lines`) so that a copy holds what was written rather than how it was laid out.
/// A line's copy starts after `FROM`, the cells before it being a gutter. A line with `JOINED`
/// was wrapped from the one before: its copy starts after the mark and follows the untrimmed
/// copy of the line before with a space for each `SPACE` after the mark, not a line break. A line
/// with `RULE` is a rule between blocks, which a copy leaves out. A line's copy ends at `UNTIL`,
/// before what is shown at its right.
pub(super) const FROM: char = '\x1d';
pub(super) const UNTIL: char = '\x10';
pub(super) const RULE: char = '\x1c';
pub(super) const JOINED: char = '\x1e';
pub(super) const SPACE: char = '\x1f';

/// Breaks streamed text into lines that fit the terminal, between words where it can, instead of
/// leaving the terminal to split words at its edge. A word is held until it is complete.
pub(super) struct Wrap {
    /// Zero passes text through unchanged.
    width: usize,
    column: usize,
    word: String,
    word_width: usize,
    /// Spaces before `word`, dropped if it starts a new line after a break.
    spaces: usize,
    /// `word` continues one already broken at the edge of the line.
    continued: bool,
    /// No word has been placed since the last newline in the text.
    line_start: bool,
    /// Where lines wrapped from the current one start: under the text of a list item, or at
    /// the indentation of an indented line.
    indent: usize,
    /// The quote borders (`│`) the current line starts with, as written, which lines wrapped
    /// from it start with too, and the columns they take.
    lead: String,
    lead_columns: usize,
    /// How much of a task's box (`[ ]`, `[x]`) after a list item's marker came: wrapped lines
    /// start after the box.
    task: Task,
    escape: Escape,
    /// The lines it starts at a break are marked with `JOINED`.
    marked: bool,
    /// The `FROM` marks of its input are kept, where it drops them otherwise.
    gutters: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Task {
    None,
    Marker,
    Open,
}

/// Where the text is in an escape sequence, which takes no columns.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Escape {
    #[default]
    None,
    Start,
    Csi,
}

impl Escape {
    /// Whether `c` is part of an escape sequence.
    pub(super) fn step(&mut self, c: char) -> bool {
        *self = match (*self, c) {
            (Self::None, '\x1b') => Self::Start,
            (Self::None, _) => return false,
            (Self::Start, '[') => Self::Csi,
            (Self::Csi, c) if !('@'..='~').contains(&c) => Self::Csi,
            _ => Self::None,
        };
        true
    }
}

impl Wrap {
    pub(super) fn new(width: usize) -> Self {
        Self {
            width,
            column: 0,
            word: String::new(),
            word_width: 0,
            spaces: 0,
            continued: false,
            line_start: true,
            indent: 0,
            lead: String::new(),
            lead_columns: 0,
            task: Task::None,
            escape: Escape::None,
            marked: false,
            gutters: false,
        }
    }
    /// The wrap, its breaks marked for a copy (`JOINED`) if `marked`.
    pub(super) fn marking(self, marked: bool) -> Self {
        Self { marked, ..self }
    }
    /// The wrap, keeping the gutters (`FROM`) its input marks, for text whose renderer removed
    /// the marks from what it was given (`Markdown::marking`).
    pub(super) fn keeping_gutters(self) -> Self {
        Self {
            gutters: true,
            ..self
        }
    }
    pub(super) fn push(&mut self, text: &str) -> String {
        if self.width == 0 {
            return text.to_owned();
        }
        let mut out = String::new();
        for c in text.chars() {
            if self.escape.step(c) {
                self.word.push(c);
                continue;
            }
            match c {
                '\n' => {
                    self.place(&mut out);
                    self.spaces = 0;
                    self.column = 0;
                    self.line_start = true;
                    self.indent = 0;
                    self.task = Task::None;
                    self.lead.clear();
                    self.lead_columns = 0;
                    out.push('\n');
                }
                ' ' | '\t' => {
                    self.place(&mut out);
                    self.spaces += if c == '\t' { 4 } else { 1 };
                }
                // After the spaces before it, which do not decide where wrapped lines start.
                FROM if self.marked && self.gutters => {
                    self.place(&mut out);
                    out.extend(std::iter::repeat_n(' ', self.spaces));
                    self.column += std::mem::take(&mut self.spaces);
                    out.push(FROM);
                }
                FROM | JOINED | SPACE if self.marked => {}
                c => {
                    self.word.push(c);
                    self.word_width += c.width().unwrap_or(0);
                    if self.word_width > self.width {
                        self.place(&mut out);
                        self.continued = true;
                    }
                }
            }
        }
        out
    }
    /// The held word, once a newline that is not yet passed on has ended it.
    pub(super) fn end_word(&mut self) -> String {
        let mut out = String::new();
        self.place(&mut out);
        out
    }
    /// The held word, at the end of the text.
    pub(super) fn finish(&mut self) -> String {
        let mut out = String::new();
        self.place(&mut out);
        *self = Self {
            marked: self.marked,
            gutters: self.gutters,
            ..Self::new(self.width)
        };
        out
    }
    fn place(&mut self, out: &mut String) {
        if self.word.is_empty() {
            self.continued = false;
            return;
        }
        let shown: String = match self.line_start || self.task != Task::None {
            true => {
                let mut escape = Escape::None;
                self.word.chars().filter(|&c| !escape.step(c)).collect()
            }
            false => String::new(),
        };
        if self.task != Task::None {
            let boxed = self.spaces == 1 && self.indent + 4 < self.width / 2;
            self.task = match (self.task, shown.as_str()) {
                (Task::Marker, "[") if boxed => Task::Open,
                (Task::Marker, "[x]") | (Task::Open, "]") if boxed => {
                    self.indent += 4;
                    Task::None
                }
                _ => Task::None,
            };
        }
        if self.line_start {
            if shown == "│" && self.column + self.spaces + self.word_width < self.width / 2 {
                let spaces = " ".repeat(std::mem::take(&mut self.spaces));
                let word = std::mem::take(&mut self.word);
                *out += &format!("{spaces}{word}");
                self.lead += &format!("{spaces}{word}");
                self.column += spaces.len() + std::mem::take(&mut self.word_width);
                self.lead_columns = self.column;
                self.continued = false;
                return;
            }
            let marker = matches!(shown.as_str(), "-" | "*" | "+" | "•" | "●")
                || shown.strip_suffix(['.', ')']).is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                });
            let indent = self.column + self.spaces + if marker { self.word_width + 1 } else { 0 };
            let under_lead = self.lead_columns + usize::from(self.lead_columns > 0);
            self.indent = if indent < self.width / 2 {
                indent
            } else {
                under_lead
            };
            if marker && indent < self.width / 2 {
                self.task = Task::Marker;
            }
            self.line_start = false;
        }
        if !self.continued {
            if self.column > self.indent && self.column + self.spaces + self.word_width > self.width
            {
                self.break_line(out, self.spaces);
                self.spaces = 0;
            }
            out.extend(std::iter::repeat_n(' ', self.spaces));
            self.column += self.spaces;
            self.spaces = 0;
        }
        let word = std::mem::take(&mut self.word);
        let mut escape = Escape::None;
        for c in word.chars() {
            let cells = match escape.step(c) {
                true => 0,
                false => c.width().unwrap_or(0),
            };
            if self.column > self.indent && self.column + cells > self.width {
                self.break_line(out, 0);
            }
            out.push(c);
            self.column += cells;
        }
        self.word_width = 0;
        self.continued = false;
    }
    /// Starts a new line in place of `spaces` spaces.
    fn break_line(&mut self, out: &mut String, spaces: usize) {
        out.push('\n');
        *out += &self.lead;
        out.extend(std::iter::repeat_n(
            ' ',
            self.indent.saturating_sub(self.lead_columns),
        ));
        if self.marked {
            out.push(JOINED);
            out.extend(std::iter::repeat_n(SPACE, spaces));
        }
        self.column = self.indent;
    }
}

/// A line as a terminal shows it (`terminal_line`), tabs expanded.
pub(super) fn printable(line: &str) -> String {
    terminal_line(line).replace('\t', "    ")
}

/// As Scramjet shows a call, with every line of a command, wrapped to `width` (zero: not
/// wrapped); later lines go under the first, the two columns before them a gutter (`FROM`) if
/// `marked`, so that a copy holds the command as written.
pub(super) fn call_line(name: &str, summary: &str, width: usize, marked: bool) -> String {
    let wrapped = |text: &str, width| {
        let mut wrap = Wrap::new(width).marking(marked);
        wrap.push(text) + &wrap.finish()
    };
    let summary = visible(summary.trim_end_matches('\n'));
    let mut lines = summary.split('\n');
    let mut shown = match lines.next() {
        Some(first) if !summary.is_empty() => wrapped(&format!("● {name} {first}"), width),
        _ => format!("● {name}"),
    };
    let gutter = if marked {
        format!("  {FROM}")
    } else {
        "  ".into()
    };
    for line in lines {
        for row in wrapped(line, width.saturating_sub(2)).split('\n') {
            shown += &format!("\n{gutter}{row}");
        }
    }
    shown
}

/// Shows control and invisible formatting characters (other than newlines and tabs) as escapes,
/// so that a command cannot make its approval question display something else.
pub(super) fn visible(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\t' => c.to_string(),
            '\u{061c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}' => c.escape_default().to_string(),
            c if c.is_control() => c.escape_default().to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// A prompt on one line for a list: line breaks and other control characters as spaces.
pub(super) fn one_line(prompt: &str) -> String {
    let line: String = prompt
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    line.trim().to_owned()
}

/// `text` in at most `width` columns, ending with `…` where it is cut. Its style sequences take
/// none, and a cut text's styles end after the `…`.
pub(super) fn clip(text: &str, width: usize) -> String {
    if columns(text) <= width {
        return text.to_owned();
    }
    let (mut clipped, mut used, mut styled) = (String::new(), 0, false);
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(sequence) = style_at(rest) {
            (clipped, styled) = (clipped + sequence, true);
            rest = &rest[sequence.len()..];
            continue;
        }
        let cells = c.width().unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        used += cells;
        clipped.push(c);
        rest = &rest[c.len_utf8()..];
    }
    clipped.push('…');
    if styled {
        clipped += super::style::RESET;
    }
    clipped
}

/// The columns `text` takes, but for its style sequences.
pub(super) fn columns(text: &str) -> usize {
    match text.contains('\x1b') {
        true => plain(text).width(),
        false => text.width(),
    }
}

/// `text` without its style sequences.
pub(super) fn plain(text: &str) -> String {
    let (mut plain, mut rest) = (String::new(), text);
    while let Some(c) = rest.chars().next() {
        let length = match style_at(rest) {
            Some(sequence) => sequence.len(),
            None => {
                plain.push(c);
                c.len_utf8()
            }
        };
        rest = &rest[length..];
    }
    plain
}

/// The style sequence (`ESC [ … m`) `text` starts with.
pub(super) fn style_at(text: &str) -> Option<&str> {
    let parameters = text.strip_prefix("\x1b[")?;
    let end = parameters.find(|c: char| !c.is_ascii_digit() && c != ';')?;
    parameters[end..].starts_with('m').then(|| &text[..end + 3])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_counts_the_text_of_styled_rows() {
        let styled = "\x1b[2m├─ \x1b[0m\x1b[32mab\x1b[0mcd";
        assert_eq!(clip(styled, 7), styled);
        assert_eq!(
            clip(styled, 6),
            "\x1b[2m├─ \x1b[0m\x1b[32mab\x1b[0m…\x1b[0m"
        );
        assert_eq!(clip(styled, 4), "\x1b[2m├─ \x1b[0m\x1b[32m…\x1b[0m");
        assert_eq!(clip("\x1b[2mabc", 2), "\x1b[2ma…\x1b[0m");
        // Escaped text, as `visible` shows it, is text.
        assert_eq!(clip("\\u{1b}[2m", 4), "\\u{…");
    }

    #[test]
    fn streamed_text_wraps_between_words() {
        let wrapped = |width, pieces: &[&str]| {
            let mut wrap = Wrap::new(width);
            let mut out: String = pieces.iter().map(|piece| wrap.push(piece)).collect();
            out += &wrap.finish();
            out
        };
        assert_eq!(
            wrapped(11, &["hel", "lo wo", "rld foo"]),
            "hello world\nfoo"
        );
        assert_eq!(wrapped(11, &["hello   ", "  world"]), "hello\nworld");
        assert_eq!(
            wrapped(20, &["- a\n    indented  \nend"]),
            "- a\n    indented\nend"
        );
        let long = format!("a {} b", "x".repeat(25));
        let broken = format!("a\n{0}\n{0}\n{1} b", "x".repeat(10), "x".repeat(5));
        assert_eq!(wrapped(10, &[&long]), broken);
        assert_eq!(
            wrapped(10, &[&format!("{} b", "x".repeat(11))]),
            "xxxxxxxxxx\nx b"
        );
        assert_eq!(wrapped(6, &["日本語 です"]), "日本語\nです");
        assert_eq!(
            wrapped(12, &["- one two three\n12. four five six\n  seven eight"]),
            "- one two\n  three\n12. four\n    five six\n  seven\n  eight"
        );
        assert_eq!(wrapped(12, &["-flag one two"]), "-flag one\ntwo");
        // A task's box is part of its item's marker.
        assert_eq!(
            wrapped(
                14,
                &["\x1b[36m- [ ]\x1b[0m one two three\n- [x] three four"]
            ),
            "\x1b[36m- [ ]\x1b[0m one two\n      three\n- [x] three\n      four"
        );
        assert_eq!(wrapped(10, &["- [ ] a b c"]), "- [ ] a b\n  c");
        assert_eq!(wrapped(12, &["- [  ] a b cd"]), "- [  ] a b\n  cd");
        // A style before the marker takes no columns.
        assert_eq!(
            wrapped(12, &["\x1b[2m- one two three"]),
            "\x1b[2m- one two\n  three"
        );
        assert_eq!(
            wrapped(20, &["● bash cargo fmt --check && cargo clippy"]),
            "● bash cargo fmt\n  --check && cargo\n  clippy"
        );
        let script = "python3 - <<'EOF'\nif x:\n    y()\r\nEOF\n";
        assert_eq!(
            call_line("bash", script, 0, false),
            "● bash python3 - <<'EOF'\n  if x:\n      y()\\r\n  EOF"
        );
        assert_eq!(
            call_line("bash", script, 16, false),
            "● bash python3 -\n  <<'EOF'\n  if x:\n      y()\\r\n  EOF"
        );
        assert_eq!(call_line("read", "", 16, false), "● read");
        // Marked, the copy leaves out the columns that put later lines under the first.
        assert_eq!(
            call_line("bash", "echo one two\n  echo three four", 14, true),
            format!(
                "● bash echo\n  {JOINED}{SPACE}one two\n  {FROM}  echo three\n  {FROM}  {JOINED}{SPACE}four"
            )
        );
        // Lines wrapped from a quote start with its borders, styled as on its first line.
        let border = "\x1b[34m│\x1b[0m\x1b[3m";
        assert_eq!(
            wrapped(
                12,
                &[&format!("{border} {border} one two three\nfour five six")]
            ),
            format!("{border} {border} one two\n{border} {border} three\nfour five\nsix")
        );
        assert_eq!(
            wrapped(12, &["│ - one two three"]),
            "│ - one two\n│   three"
        );
        assert_eq!(wrapped(4, &["│ ab cd"]), "│ ab\n│ cd");
        assert_eq!(wrapped(0, &["no  wrap \n"]), "no  wrap \n");
        assert_eq!(
            wrapped(11, &["\x1b[1mhel", "lo\x1b[0m world \x1b[36mfoo"]),
            "\x1b[1mhello\x1b[0m world\n\x1b[36mfoo"
        );
    }
}
