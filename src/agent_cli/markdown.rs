//! Turns the markdown of streamed replies into terminal styles, as Scramjet renders them: bold,
//! code spans, headings and fenced code blocks. A marker is shown as typed unless its span closes
//! on the same line, so text after an opening marker waits until the span closes or the line ends.

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const CODE: &str = "\x1b[36m";
const CODE_BLOCK: &str = "\x1b[32m";
const FENCE: &str = "\x1b[2m";
const HEADING: &str = "\x1b[1;33m";
const TITLE: &str = "\x1b[1;4;33m";

#[derive(Clone, Copy, PartialEq)]
enum Line {
    /// Not yet known whether the line is text, a heading, code or a fence.
    Start,
    Text(&'static str),
    Code,
    /// A fence line, shown once it ends.
    Fence,
}

pub struct Markdown {
    enabled: bool,
    /// The style of the whole block, restored after each styled part.
    base: &'static str,
    line: Line,
    /// The part of the current line not yet rendered.
    pending: String,
    /// The fence character and length of the open code block.
    block: Option<(char, usize)>,
}

enum Span {
    Plain,
    /// More text is needed to decide.
    Wait,
    Literal(usize),
    Code {
        content: String,
        end: usize,
    },
    Bold {
        inner: (usize, usize),
        end: usize,
    },
}

impl Markdown {
    /// A disabled renderer passes text through unchanged.
    pub fn new(enabled: bool, base: &'static str) -> Self {
        Self {
            enabled,
            base,
            line: Line::Start,
            pending: String::new(),
            block: None,
        }
    }

    pub fn push(&mut self, text: &str) -> String {
        if !self.enabled {
            return text.to_owned();
        }
        let mut out = String::new();
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            self.pending.push_str(first);
            out += &self.advance(false);
        }
        for line in lines {
            out += &self.end_line();
            out.push('\n');
            self.pending.push_str(line);
            out += &self.advance(false);
        }
        out
    }

    /// Renders the rest of the current line, which a newline not yet pushed has ended.
    pub fn end_line(&mut self) -> String {
        if !self.enabled {
            return String::new();
        }
        let mut out = self.advance(true);
        match self.line {
            Line::Start => out += &std::mem::take(&mut self.pending),
            Line::Fence => {
                let fence = std::mem::take(&mut self.pending);
                let marker = fence.trim_start();
                let c = marker.chars().next().unwrap_or('`');
                let run = marker.chars().take_while(|&m| m == c).count();
                self.block = match self.block {
                    Some(_) => None,
                    None => Some((c, run)),
                };
                out += &format!("{FENCE}{fence}{RESET}{}", self.base);
            }
            Line::Text("") => {}
            Line::Text(_) | Line::Code => out += &format!("{RESET}{}", self.base),
        }
        self.line = Line::Start;
        out
    }

    /// Renders what is left at the end of the block.
    pub fn finish(&mut self) -> String {
        let out = self.end_line();
        self.block = None;
        out
    }

    /// The style in effect, to set again after other output reset it.
    pub fn reopen(&self) -> String {
        if !self.enabled {
            return String::new();
        }
        let line = match self.line {
            Line::Text(style) => style,
            Line::Code => CODE_BLOCK,
            Line::Start | Line::Fence => "",
        };
        format!("{}{line}", self.base)
    }

    fn advance(&mut self, ended: bool) -> String {
        let mut out = String::new();
        if self.line == Line::Start {
            let Some(line) = self.decide(ended) else {
                return out;
            };
            self.line = line;
            match line {
                Line::Code => out += CODE_BLOCK,
                Line::Text(style) if !style.is_empty() => {
                    let text = self.pending.trim_start();
                    let level = text.chars().take_while(|&c| c == '#').count();
                    let rest = text[level..].trim_start().to_owned();
                    let indent = self.pending.len() - text.len();
                    out += &self.pending[..indent];
                    out += style;
                    if level > 2 {
                        out += &text[..level];
                        out.push(' ');
                    }
                    self.pending = rest;
                }
                _ => {}
            }
        }
        match self.line {
            Line::Start | Line::Fence => {}
            Line::Code => out += &std::mem::take(&mut self.pending),
            Line::Text(style) => out += &self.inline(ended, style),
        }
        out
    }

    /// The kind of the current line, once its start shows it.
    fn decide(&self, ended: bool) -> Option<Line> {
        let text = self.pending.trim_start_matches(' ');
        if text.is_empty() {
            return None;
        }
        if let Some((c, length)) = self.block {
            let run = text.chars().take_while(|&m| m == c).count();
            if text[run..].trim_end_matches(' ').is_empty() && !ended {
                return None;
            }
            let closing = run >= length && text[run..].trim_end_matches(' ').is_empty();
            return Some(if closing { Line::Fence } else { Line::Code });
        }
        if text.starts_with("```") || text.starts_with("~~~") {
            return Some(Line::Fence);
        }
        if !ended && ["`", "``", "~", "~~"].contains(&text) {
            return None;
        }
        let level = text.chars().take_while(|&c| c == '#').count();
        let indent = self.pending.len() - text.len();
        if (1..=6).contains(&level) && indent < 4 {
            match text[level..].chars().next() {
                None if !ended => return None,
                Some(' ') => return Some(Line::Text(if level == 1 { TITLE } else { HEADING })),
                _ => {}
            }
        }
        Some(Line::Text(""))
    }

    /// Renders the spans of a text line that are complete, keeping an unclosed one pending.
    fn inline(&mut self, ended: bool, style: &'static str) -> String {
        let chars: Vec<char> = self.pending.chars().collect();
        let restore = format!("{RESET}{}{style}", self.base);
        let mut out = String::new();
        let mut i = 0;
        let mut plain = 0;
        while i < chars.len() {
            match span(&chars, i, ended) {
                Span::Plain => i += 1,
                Span::Literal(n) => i += n,
                Span::Wait => break,
                Span::Code { content, end } => {
                    out.extend(&chars[plain..i]);
                    out += &format!("{CODE}{content}{restore}");
                    i = end;
                    plain = i;
                }
                Span::Bold { inner, end } => {
                    out.extend(&chars[plain..i]);
                    let bold = format!("{restore}{BOLD}");
                    out += BOLD;
                    out += &codes(&chars[inner.0..inner.1], &bold);
                    out += &restore;
                    i = end;
                    plain = i;
                }
            }
        }
        out.extend(&chars[plain..i]);
        self.pending = chars[i..].iter().collect();
        out
    }
}

/// `chars` with its code spans styled, restoring `restore` after each.
fn codes(chars: &[char], restore: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => match code_span(chars, i, true) {
                Span::Code { content, end } => {
                    out += &format!("{CODE}{content}{restore}");
                    i = end;
                }
                _ => {
                    let n = run(chars, i);
                    out.extend(&chars[i..i + n]);
                    i += n;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

fn run(chars: &[char], at: usize) -> usize {
    chars[at..].iter().take_while(|&&c| c == chars[at]).count()
}

fn span(chars: &[char], at: usize, ended: bool) -> Span {
    match chars[at] {
        '`' => code_span(chars, at, ended),
        '*' => bold_span(chars, at, ended),
        _ => Span::Plain,
    }
}

/// A run of backticks closed by the next run of the same length, as in CommonMark.
fn code_span(chars: &[char], at: usize, ended: bool) -> Span {
    let n = run(chars, at);
    if at + n == chars.len() && !ended {
        return Span::Wait;
    }
    let mut j = at + n;
    while j < chars.len() {
        if chars[j] != '`' {
            j += 1;
            continue;
        }
        let m = run(chars, j);
        if m == n {
            if j + m == chars.len() && !ended {
                return Span::Wait;
            }
            let mut content: String = chars[at + n..j].iter().collect();
            if content.len() > 2
                && content.starts_with(' ')
                && content.ends_with(' ')
                && !content.trim().is_empty()
            {
                content = content[1..content.len() - 1].to_owned();
            }
            return Span::Code {
                content,
                end: j + m,
            };
        }
        j += m;
    }
    if ended { Span::Literal(n) } else { Span::Wait }
}

/// `**` after a non-word character and before text, closed by `**` after text and before a
/// non-word character. Code spans inside are skipped.
fn bold_span(chars: &[char], at: usize, ended: bool) -> Span {
    let n = run(chars, at);
    if at + n == chars.len() && !ended {
        return Span::Wait;
    }
    let opens = n == 2
        && (at == 0 || !chars[at - 1].is_alphanumeric())
        && chars.get(at + 2).is_some_and(|c| !c.is_whitespace());
    if !opens {
        return Span::Literal(n);
    }
    let mut k = at + 2;
    while k < chars.len() {
        match chars[k] {
            '`' => match code_span(chars, k, ended) {
                Span::Wait => return Span::Wait,
                Span::Code { end, .. } => k = end,
                _ => k += run(chars, k),
            },
            '*' => {
                let m = run(chars, k);
                if k + m == chars.len() && !ended {
                    return Span::Wait;
                }
                let closes = m == 2
                    && !chars[k - 1].is_whitespace()
                    && chars.get(k + 2).is_none_or(|c| !c.is_alphanumeric());
                if closes {
                    return Span::Bold {
                        inner: (at + 2, k),
                        end: k + 2,
                    };
                }
                k += m;
            }
            _ => k += 1,
        }
    }
    if ended { Span::Literal(2) } else { Span::Wait }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(pieces: &[&str]) -> String {
        let mut markdown = Markdown::new(true, "");
        let mut out: String = pieces.iter().map(|piece| markdown.push(piece)).collect();
        out += &markdown.finish();
        out
    }

    #[test]
    fn spans_render_the_same_however_the_text_arrives() {
        let text = "Use **bold `x` text** and `a ** b` here.\n## Plan `p`\n- **Note:** done";
        let whole = render(&[text]);
        assert_eq!(
            whole,
            "Use \x1b[1mbold \x1b[36mx\x1b[0m\x1b[1m text\x1b[0m and \x1b[36ma ** b\x1b[0m here.\n\
\x1b[1;33mPlan \x1b[36mp\x1b[0m\x1b[1;33m\x1b[0m\n\
- \x1b[1mNote:\x1b[0m done"
        );
        let chars: Vec<char> = text.chars().collect();
        for a in 0..chars.len() {
            for b in a..chars.len() {
                let pieces: Vec<String> = [&chars[..a], &chars[a..b], &chars[b..]]
                    .map(|part| part.iter().collect())
                    .into();
                let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
                assert_eq!(render(&pieces), whole, "split at {a} and {b}");
            }
        }
    }

    #[test]
    fn unclosed_markers_stay_as_typed() {
        for text in [
            "f(**kwargs) and 2**10 and a ` tick",
            "** spaced** and **not bold **",
            "x**y** and *one* and ***three***",
            "#hashtag and ####### seven",
        ] {
            assert_eq!(render(&[text, "\nnext"]), format!("{text}\nnext"));
        }
        assert_eq!(render(&["`a``b`"]), "\x1b[36ma``b\x1b[0m");
    }

    #[test]
    fn text_after_an_open_span_waits_for_it_to_close() {
        let mut markdown = Markdown::new(true, "");
        assert_eq!(markdown.push("see **this"), "see ");
        assert_eq!(markdown.push(" part** now"), "\x1b[1mthis part\x1b[0m now");
        assert_eq!(markdown.push(" `open"), " ");
        assert_eq!(markdown.end_line(), "`open");
        assert_eq!(markdown.push("\n# T"), "\n\x1b[1;4;33mT");
    }

    #[test]
    fn code_blocks_show_their_lines_unstyled_inside() {
        let text = "Run:\n  ```sh\n  echo **x** `y`\n\n# not a heading\n  ```\n### Three\nafter";
        assert_eq!(
            render(&[text]),
            "Run:\n\x1b[2m  ```sh\x1b[0m\n\x1b[32m  echo **x** `y`\x1b[0m\n\n\
\x1b[32m# not a heading\x1b[0m\n\x1b[2m  ```\x1b[0m\n\x1b[1;33m### Three\x1b[0m\nafter"
        );
        assert_eq!(
            render(&["````\n```\nstill code\n````\n**b**"]),
            "\x1b[2m````\x1b[0m\n\x1b[32m```\x1b[0m\n\x1b[32mstill code\x1b[0m\n\
\x1b[2m````\x1b[0m\n\x1b[1mb\x1b[0m"
        );
    }

    #[test]
    fn styles_restore_the_block_style() {
        let mut markdown = Markdown::new(true, "\x1b[2m");
        assert_eq!(
            markdown.push("**Title** `c`\n"),
            "\x1b[1mTitle\x1b[0m\x1b[2m \x1b[36mc\x1b[0m\x1b[2m\n"
        );
        assert_eq!(
            markdown.push("```\nx"),
            "\x1b[2m```\x1b[0m\x1b[2m\n\x1b[32mx"
        );
        assert_eq!(markdown.reopen(), "\x1b[2m\x1b[32m");
        let mut plain = Markdown::new(false, "");
        assert_eq!(plain.push("**a** `b`\n```"), "**a** `b`\n```");
        assert_eq!(plain.finish() + &plain.reopen(), "");
    }
}
