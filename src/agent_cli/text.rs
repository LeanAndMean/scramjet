//! Text as the terminal shows it: wrapped between words, cut to a width, made safe to print.
use scramjet_rs::agent::terminal_line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    escape: Escape,
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
            escape: Escape::None,
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
                    out.push('\n');
                }
                ' ' | '\t' => {
                    self.place(&mut out);
                    self.spaces += if c == '\t' { 4 } else { 1 };
                }
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
        *self = Self::new(self.width);
        out
    }
    fn place(&mut self, out: &mut String) {
        if self.word.is_empty() {
            self.continued = false;
            return;
        }
        if self.line_start {
            let marker = matches!(self.word.as_str(), "-" | "*" | "+" | "•" | "●")
                || self.word.strip_suffix(['.', ')']).is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
                });
            let indent = self.spaces + if marker { self.word_width + 1 } else { 0 };
            self.indent = if indent < self.width / 2 { indent } else { 0 };
            self.line_start = false;
        }
        if !self.continued {
            if self.column > self.indent && self.column + self.spaces + self.word_width > self.width
            {
                self.break_line(out);
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
                self.break_line(out);
            }
            out.push(c);
            self.column += cells;
        }
        self.word_width = 0;
        self.continued = false;
    }
    fn break_line(&mut self, out: &mut String) {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', self.indent));
        self.column = self.indent;
    }
}

/// A line as a terminal shows it (`terminal_line`), tabs expanded.
pub(super) fn printable(line: &str) -> String {
    terminal_line(line).replace('\t', "    ")
}

/// As Scramjet shows a call, with every line of a command; later lines go under the first.
pub(super) fn call_line(name: &str, summary: &str) -> String {
    let summary = visible(summary.trim_end_matches('\n'));
    match summary.is_empty() {
        true => format!("● {name}"),
        false => format!("● {name} {}", summary.replace('\n', "\n  ")),
    }
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

pub(super) fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut clipped = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cells = c.width().unwrap_or(0);
        if used + cells + 1 > width {
            break;
        }
        used += cells;
        clipped.push(c);
    }
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            wrapped(20, &["● bash cargo fmt --check && cargo clippy"]),
            "● bash cargo fmt\n  --check && cargo\n  clippy"
        );
        let script = call_line("bash", "python3 - <<'EOF'\nif x:\n    y()\r\nEOF\n");
        assert_eq!(
            script,
            "● bash python3 - <<'EOF'\n  if x:\n      y()\\r\n  EOF"
        );
        assert_eq!(
            wrapped(16, &[&script]),
            "● bash python3 -\n  <<'EOF'\n  if x:\n      y()\\r\n  EOF"
        );
        assert_eq!(wrapped(0, &["no  wrap \n"]), "no  wrap \n");
        assert_eq!(
            wrapped(11, &["\x1b[1mhel", "lo\x1b[0m world \x1b[36mfoo"]),
            "\x1b[1mhello\x1b[0m world\n\x1b[36mfoo"
        );
    }
}
