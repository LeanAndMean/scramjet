//! Turns the markdown of streamed replies into terminal styles, as Scramjet renders them: strong,
//! emphasized and struck text, code spans, links, headings, lists, block quotes, rules, fenced and
//! indented code blocks, highlighted when they name their language, tables and line breaks. A
//! marker is shown as typed unless its span closes on the same line, so text after an opening
//! marker waits until the span closes or the line ends. A table's lines wait until it ends, as its
//! columns are fitted to all its rows, and so does a highlighted code line. A blank line waits for
//! the next, as Scramjet shows a run of them as one, and none between a list's items; and, as
//! Scramjet's, blocks the text did not separate are shown with a blank line between them.

use super::highlight::{self, Highlighter};
use super::selection;
use super::style::{
    CODE, CODE_BLOCK, CODE_FENCE, EMPHASIS, HEADING, HORIZONTAL_RULE, LINK, LINK_URL, LIST_BULLET,
    QUOTE, QUOTE_BORDER, RESET, STRONG, STRONG_EMPHASIS, STRUCK, TITLE,
};
use super::text::{FROM, JOINED, RULE, SPACE, Wrap, columns, plain};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq)]
enum Line {
    /// Not yet known what the line is.
    Start,
    /// Text in the style of a paragraph (none), a heading or a quote.
    Text(&'static str),
    Code,
    /// A fence line, shown once it ends.
    Fence,
    /// A table's line, or one that may start a table, kept whole until the table ends.
    Held,
    /// A horizontal rule, already shown.
    Rule,
}

/// What the start of a line shows it is.
enum Kind {
    Text,
    Heading(usize),
    Quote,
    Item,
    Rule,
    Held,
    Code,
    Fence,
    /// The first line of a code block indented by four columns.
    Indented,
}

/// The block a line is in, as far as the blank lines between blocks go.
#[derive(Clone, Copy, PartialEq)]
enum Block {
    Paragraph,
    Heading,
    Quote,
    List,
    Rule,
    Code,
    Table,
}

/// Whether Scramjet shows a blank line between a block after `previous` and the `next` the text
/// did not separate: after anything but a list, unless a paragraph or quote goes on, or a list
/// follows a paragraph.
fn separated(previous: Option<Block>, next: Block) -> bool {
    match previous {
        None | Some(Block::List) => false,
        Some(Block::Paragraph) => !matches!(next, Block::Paragraph | Block::List),
        Some(Block::Quote) => next != Block::Quote,
        Some(_) => true,
    }
}

/// What a task item's text starts with.
const TASKS: [&str; 3] = ["[ ] ", "[x] ", "[X] "];

/// Where an open code block's lines start, as Scramjet indents them under their fence.
const CODE_INDENT: &str = "  ";

/// The fence of an open code block.
#[derive(Clone, Copy)]
struct Fence {
    marker: char,
    length: usize,
    /// The spaces before it, which its lines lose as many of theirs.
    indent: usize,
    /// The spaces before it as shown, which its lines are shown after.
    shown: usize,
    /// A block of lines indented by `indent`, which has no fence and ends at a line indented
    /// less; its blank lines wait, as Scramjet shows only those between its lines.
    indented: bool,
}

/// An open list. As Scramjet's, each level is four columns in, an ordered list is numbered from
/// its first item, and its items' other lines are under their text.
struct List {
    /// What its items' markers end with: `-`, `*`, `+`, or the `.` or `)` after a number.
    marker: char,
    /// The current item's number, in an ordered list.
    number: Option<u64>,
    /// The column the current item's text starts at, as written and as shown.
    written: usize,
    shown: usize,
}

pub struct Markdown {
    enabled: bool,
    /// Code lines' indents are marked as a gutter for the full screen's copy (`text::FROM`).
    marked: bool,
    /// The style of the whole block, restored after each styled part.
    base: &'static str,
    /// The columns rules and tables fit in; zero leaves them unbounded.
    width: usize,
    line: Line,
    /// The part of the current line not yet rendered.
    pending: String,
    /// The character before `pending` on its line, which decides whether a marker opens a span.
    before: Option<char>,
    /// The open code block, and its highlighter if it names a language.
    block: Option<Fence>,
    code: Option<Highlighter>,
    /// The current line was ended (`end_line`) and its newline not yet pushed; and it was held.
    ended: bool,
    held: bool,
    table: Option<Table>,
    lists: Vec<List>,
    /// The line before can go on in a line not indented under its item's text, as a paragraph.
    lazy: bool,
    /// The current line's indent as written.
    indent: usize,
    /// The ended line was blank; and the blank lines whose newlines wait for the next line.
    blank: bool,
    blanks: usize,
    /// The block of the line before, outside the lines of a table not yet shown.
    previous: Option<Block>,
    /// The line starting may be shown after a blank line between blocks: none came before it and
    /// it is not in a list.
    separable: bool,
    /// The quotes the line before's text is in, which a line of text goes on in.
    quoted: usize,
    /// The line before ended in a backslash, a line break if the next line goes on with its
    /// text and shown otherwise: the style it is shown in, and whether its newline came.
    broken: Option<(&'static str, bool)>,
}

/// A table's lines so far, or the one line that may start a table.
struct Table {
    /// The spaces its lines start with.
    indent: usize,
    /// Its lines as written, shown as they are when the table cannot fit.
    lines: Vec<String>,
    header: Vec<String>,
    /// Its rows' cells, once its delimiter row came.
    rows: Option<Vec<Vec<String>>>,
    /// A newline followed its last line.
    newline: bool,
    /// A blank line between blocks can go before it (`Markdown::separable`).
    separable: bool,
}

enum Span {
    Plain,
    /// More text is needed to decide.
    Wait,
    Literal(usize),
    /// A backslash before punctuation, which shows the punctuation as typed.
    Escaped,
    Code {
        content: String,
        end: usize,
    },
    Styled {
        style: &'static str,
        inner: (usize, usize),
        end: usize,
    },
    Link {
        text: (usize, usize),
        url: String,
        end: usize,
    },
    /// An address written as it is linked.
    Url {
        url: String,
        end: usize,
    },
}

impl Markdown {
    /// A disabled renderer passes text through unchanged.
    pub fn new(enabled: bool, base: &'static str, width: usize) -> Self {
        Self {
            enabled,
            marked: false,
            base,
            width,
            line: Line::Start,
            pending: String::new(),
            before: None,
            block: None,
            code: None,
            ended: false,
            held: false,
            table: None,
            lists: Vec::new(),
            lazy: false,
            indent: 0,
            blank: false,
            blanks: 0,
            previous: None,
            separable: false,
            quoted: 0,
            broken: None,
        }
    }

    /// The renderer, marking the indent of code lines for a copy (`text::FROM`) if `marked`; the
    /// text it is given then loses its own marks.
    pub fn marking(self, marked: bool) -> Self {
        Self { marked, ..self }
    }

    pub fn push(&mut self, text: &str) -> String {
        if !self.enabled {
            return text.to_owned();
        }
        let text = match self.marked {
            true => text.replace([FROM, JOINED, SPACE, RULE], ""),
            false => text.to_owned(),
        };
        let mut out = String::new();
        let mut lines = text.split('\n');
        if let Some(first) = lines.next() {
            self.pending.push_str(first);
            out += &self.advance(false);
        }
        for line in lines {
            out += &self.end_line();
            out += self.newline();
            self.pending.push_str(line);
            out += &self.advance(false);
        }
        out
    }

    /// Renders the rest of the current line, which a newline not yet pushed has ended.
    pub fn end_line(&mut self) -> String {
        if !self.enabled || self.ended {
            return String::new();
        }
        let backslashes = self
            .pending
            .chars()
            .rev()
            .take_while(|&c| c == '\\')
            .count();
        if let Line::Text(style) = self.line
            && backslashes % 2 == 1
            && self.previous != Some(Block::Heading)
        {
            self.pending.pop();
            self.broken = Some((style, false));
        }
        let mut out = self.advance(true);
        match self.line {
            // A blank line, or one of spaces, which in a code block show after its indent.
            Line::Start => {
                out += &self.flush_table();
                let mut line = std::mem::take(&mut self.pending);
                let Some(fence) = self.block.filter(|fence| !fence.indented) else {
                    self.blank = true;
                    self.lazy = false;
                    self.quoted = 0;
                    return self.ended_line(out);
                };
                line.drain(..line.len().min(fence.indent));
                if !line.is_empty() {
                    out += &self.code_indent(fence);
                }
                match &mut self.code {
                    Some(code) => out += &code.line(&line),
                    None => out += &line,
                }
            }
            // As Scramjet's, a fence shows as ```, with the info after the opening one.
            Line::Fence => {
                let fence = std::mem::take(&mut self.pending);
                let marker = fence.trim_start();
                let c = marker.chars().next().unwrap_or('`');
                let length = marker.chars().take_while(|&m| m == c).count();
                let info = marker[length..].trim();
                let opened = Fence {
                    marker: c,
                    length,
                    indent: self.indent,
                    shown: fence.len() - marker.len(),
                    indented: false,
                };
                let (shown, info) = match self.block.take() {
                    Some(open) => (open, ""),
                    None => {
                        self.block = Some(opened);
                        (opened, info)
                    }
                };
                let language = self.block.and_then(|_| highlight::named(info));
                self.code = language.map(|language| Highlighter::new(language, self.base));
                out += &self.fence(shown, info);
            }
            Line::Held => {
                let line = std::mem::take(&mut self.pending);
                out += &self.hold(line);
                self.held = true;
            }
            Line::Text("") | Line::Rule => {}
            Line::Code if self.code.is_some() => {}
            Line::Text(_) | Line::Code => out += &format!("{RESET}{}", self.base),
        }
        self.ended_line(out)
    }

    fn ended_line(&mut self, out: String) -> String {
        self.line = Line::Start;
        self.before = None;
        self.ended = true;
        out
    }

    /// Renders what is left at the end of the block, and, as Scramjet's, the closing fence of a
    /// code block left open.
    pub fn finish(&mut self) -> String {
        let empty = !self.ended && self.line == Line::Start && self.pending.is_empty();
        let mut out = self.end_line();
        if let Some((style, newline)) = self.broken.take() {
            out += &self.backslash(style);
            out += if newline { "\n" } else { "" };
        }
        out += &self.flush_table();
        if let Some(fence) = self.block.take() {
            if !empty {
                out.push('\n');
            }
            out += &self.fence(fence, "");
        }
        out += &"\n".repeat(self.blanks);
        self.code = None;
        self.ended = false;
        self.held = false;
        self.lists.clear();
        self.lazy = false;
        self.blank = false;
        self.blanks = 0;
        self.previous = None;
        self.quoted = 0;
        out
    }

    /// The style in effect, to set again after other output reset it.
    pub fn reopen(&self) -> String {
        if !self.enabled {
            return String::new();
        }
        let line = match self.line {
            Line::Text(style) => style,
            Line::Code if self.code.is_none() => CODE_BLOCK.get(),
            _ => "",
        };
        format!("{}{line}", self.base)
    }

    /// A line's last backslash, which did not break it, in the line's `style`.
    fn backslash(&self, style: &str) -> String {
        match style {
            "" => "\\".into(),
            style => format!("{style}\\{RESET}{}", self.base),
        }
    }

    /// A fence line: its indent, then ``` and the info string it was given.
    fn fence(&self, fence: Fence, info: &str) -> String {
        let indent = " ".repeat(fence.shown);
        format!("{indent}{CODE_FENCE}```{info}{RESET}{}", self.base)
    }

    /// What a line of the code block under `fence` starts with: its fence's indent and two
    /// columns more, a gutter for a copy.
    fn code_indent(&self, fence: Fence) -> String {
        let gutter = " ".repeat(fence.shown) + CODE_INDENT;
        match self.marked {
            true => format!("{gutter}{FROM}"),
            false => gutter,
        }
    }

    /// The newline after an ended line, which waits with a table's line or a blank line.
    fn newline(&mut self) -> &'static str {
        self.ended = false;
        if std::mem::take(&mut self.blank) {
            self.blanks += 1;
            return "";
        }
        if let Some((_, newline)) = &mut self.broken {
            *newline = true;
            return "";
        }
        if !std::mem::take(&mut self.held) {
            return "\n";
        }
        if let Some(table) = &mut self.table {
            table.newline = true;
        }
        ""
    }

    fn advance(&mut self, ended: bool) -> String {
        let mut out = String::new();
        if self.line == Line::Start {
            let Some(kind) = self.decide(ended) else {
                return out;
            };
            // As CommonMark's hard line break: a backslash before a line its text goes on in.
            if let Some((style, newline)) = self.broken.take() {
                let goes_on = self.blanks == 0
                    && match kind {
                        Kind::Text => true,
                        Kind::Quote => {
                            let rest = self.pending.trim_start_matches(['>', ' ']);
                            self.previous == Some(Block::Quote) && !rest.is_empty()
                        }
                        _ => false,
                    };
                if !goes_on {
                    out += &self.backslash(style);
                }
                out += if newline { "\n" } else { "" };
            }
            if let Some(fence) = self.block.filter(|fence| fence.indented) {
                match kind {
                    Kind::Code => out += &"\n".repeat(std::mem::take(&mut self.blanks)),
                    _ => {
                        out += &self.fence(fence, "");
                        out.push('\n');
                        self.block = None;
                    }
                }
            }
            if !matches!(kind, Kind::Held) {
                out += &self.flush_table();
            }
            let listed = !self.lists.is_empty();
            let block = self.block_of(&kind);
            let next_item = self.place(&kind);
            let blanks = std::mem::take(&mut self.blanks);
            self.separable = blanks == 0 && !listed;
            match blanks {
                0 if !listed && block.is_some_and(|b| separated(self.previous, b)) => {
                    out.push('\n')
                }
                0 => {}
                1 if next_item => {}
                _ => out.push('\n'),
            }
            if let Some(block) = block {
                self.previous = Some(block);
            }
            self.line = self.start(kind, &mut out);
        }
        match self.line {
            Line::Start | Line::Fence | Line::Held | Line::Rule => {}
            Line::Code => match &mut self.code {
                None => out += &std::mem::take(&mut self.pending),
                Some(code) if ended => out += &code.line(&std::mem::take(&mut self.pending)),
                Some(_) => {}
            },
            Line::Text(style) => out += &self.inline(ended, style),
        }
        out
    }

    /// What the current line is, once its start shows it.
    fn decide(&self, ended: bool) -> Option<Kind> {
        let text = self.pending.trim_start_matches(' ');
        if text.is_empty() {
            return None;
        }
        let indent = self.pending.len() - text.len();
        match self.block {
            Some(fence) if fence.indented && indent >= fence.indent => return Some(Kind::Code),
            Some(fence) if fence.indented => {}
            Some(fence) => return self.fenced(fence, text, ended),
            None => {}
        }
        if text.starts_with("```") || text.starts_with("~~~") {
            return Some(Kind::Fence);
        }
        if !ended && ["`", "``", "~", "~~"].contains(&text) {
            return None;
        }
        let item = self.lists[..self.within(indent)].last();
        let relative = indent - item.map_or(0, |list| list.written);
        // As marked's tables, whose rows go on in any line that starts no other block.
        let row = (self.table.as_ref()).is_some_and(|table| table.rows.is_some());
        // As CommonMark's indented code, which cannot go on with a paragraph's text.
        if relative >= 4 && (!self.lazy || row) {
            return Some(Kind::Indented);
        }
        if relative < 4 {
            if text.starts_with('|') {
                return Some(Kind::Held);
            }
            if text.starts_with('>') {
                let markers = text.chars().all(|c| c == '>' || c == ' ');
                return (ended || !markers).then_some(Kind::Quote);
            }
            let first = text.chars().next().filter(|c| matches!(c, '-' | '*' | '_'));
            if let Some(c) = first.filter(|&c| text.chars().all(|m| m == c || m == ' ')) {
                if !ended {
                    return None;
                }
                if text.matches(c).count() >= 3 {
                    return Some(Kind::Rule);
                }
            }
            let level = text.chars().take_while(|&c| c == '#').count();
            if (1..=6).contains(&level) {
                match text[level..].chars().next() {
                    None if !ended => return None,
                    Some(' ') => return Some(Kind::Heading(level)),
                    _ => {}
                }
            }
        }
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        let marker = match text.as_bytes()[0] {
            b'-' | b'*' | b'+' => 1,
            _ if digits > 9 => 0,
            _ if digits == text.len() && !ended => return None,
            _ if digits > 0 && matches!(text.as_bytes().get(digits), Some(b'.' | b')')) => {
                digits + 1
            }
            _ => 0,
        };
        if marker > 0 {
            let content = text[marker..].trim_start_matches(' ');
            let after = |task: &&str| content.strip_prefix(task);
            let partial = content.is_empty()
                || (TASKS.iter())
                    .any(|task| task.len() > content.len() && task.starts_with(content))
                || (TASKS.iter().filter_map(after))
                    .any(|rest| rest.trim_start_matches(' ').is_empty());
            match text[marker..].chars().next() {
                Some(' ') if partial && !ended => return None,
                Some(' ') => return Some(Kind::Item),
                None if !ended => return None,
                _ => {}
            }
        }
        Some(if row { Kind::Held } else { Kind::Text })
    }

    /// What a line in the code block under `fence` is: its closing fence, or code.
    fn fenced(&self, fence: Fence, text: &str, ended: bool) -> Option<Kind> {
        let run = text.chars().take_while(|&m| m == fence.marker).count();
        if text[run..].trim_end_matches(' ').is_empty() && !ended {
            return None;
        }
        let closing = run >= fence.length && text[run..].trim_end_matches(' ').is_empty();
        Some(if closing { Kind::Fence } else { Kind::Code })
    }

    /// The block a line starting as `kind` is in, unless it is in a code block or may be in a
    /// table, which its next line decides.
    fn block_of(&self, kind: &Kind) -> Option<Block> {
        if self.block.is_some() {
            return None;
        }
        Some(match kind {
            Kind::Text if self.quoted > 0 => Block::Quote,
            Kind::Text => Block::Paragraph,
            Kind::Heading(_) => Block::Heading,
            Kind::Quote => Block::Quote,
            Kind::Item => Block::List,
            Kind::Rule => Block::Rule,
            Kind::Fence | Kind::Code | Kind::Indented => Block::Code,
            Kind::Held => return None,
        })
    }

    /// How many of the open lists have a line indented by `indent` within their current item.
    fn within(&self, indent: usize) -> usize {
        let lists = self.lists.iter();
        lists.take_while(|list| indent >= list.written).count()
    }

    /// Places a line that starts as `kind` in the open lists, ending those it is not within, and
    /// puts it under the text of the item it is in; returns whether it is the next item of an
    /// open list.
    fn place(&mut self, kind: &Kind) -> bool {
        let text = self.pending.trim_start_matches(' ');
        let indent = self.pending.len() - text.len();
        self.indent = indent;
        if self.block.is_some() {
            return false;
        }
        let lazy = matches!(kind, Kind::Text | Kind::Item | Kind::Quote | Kind::Held);
        let lazy = std::mem::replace(&mut self.lazy, lazy);
        let within = self.within(indent);
        match kind {
            Kind::Item => {
                let (marker, number, length) = marker(text);
                self.lists.truncate(within + 1);
                let next = match self.lists.get_mut(within) {
                    Some(list) if list.marker == marker => {
                        list.number = list.number.map(|number| number + 1);
                        true
                    }
                    _ => false,
                };
                if !next {
                    self.lists.truncate(within);
                    self.lists.push(List {
                        marker,
                        number,
                        written: 0,
                        shown: 0,
                    });
                }
                let content = text[length..].trim_start_matches(' ');
                let spaces = match text.len() - length - content.len() {
                    spaces if spaces > 4 || content.is_empty() => 1,
                    spaces => spaces,
                };
                if let Some(list) = self.lists.last_mut() {
                    list.written = indent + length + spaces;
                }
                return next;
            }
            Kind::Text | Kind::Quote | Kind::Held if lazy => {}
            _ => self.lists.truncate(within),
        }
        if matches!(kind, Kind::Indented) {
            return false;
        }
        if let Some(list) = self.lists.last() {
            self.pending = " ".repeat(list.shown) + &self.pending[indent..];
        }
        false
    }

    /// Renders the start of a line of `kind`, its heading marker, quote borders or bullet, and
    /// leaves the rest pending; returns what the line is.
    fn start(&mut self, kind: Kind, out: &mut String) -> Line {
        let text = self.pending.trim_start_matches(' ');
        let indent = self.pending.len() - text.len();
        let base = self.base;
        let quoted = std::mem::take(&mut self.quoted);
        let border = format!("{RESET}{base}{QUOTE_BORDER}│{RESET}{base}{QUOTE} ");
        match kind {
            // As CommonMark's lazy lines, text goes on in the quotes of the text before it.
            Kind::Text if quoted > 0 => {
                if !self.lists.is_empty() {
                    *out += &self.pending[..indent];
                }
                *out += &border.repeat(quoted);
                self.pending = text.to_owned();
                self.quoted = quoted;
                Line::Text(QUOTE.get())
            }
            Kind::Text => Line::Text(""),
            Kind::Code => {
                if let Some(fence) = self.block {
                    *out += &self.code_indent(fence);
                    self.pending.drain(..indent.min(fence.indent));
                }
                if self.code.is_none() {
                    *out += CODE_BLOCK.get();
                }
                Line::Code
            }
            Kind::Fence => Line::Fence,
            // As Scramjet's, between fences, its lines losing the four columns that made it.
            Kind::Indented => {
                let list = self.lists.last();
                let fence = Fence {
                    marker: ' ',
                    length: 0,
                    indent: list.map_or(0, |list| list.written) + 4,
                    shown: list.map_or(0, |list| list.shown),
                    indented: true,
                };
                self.block = Some(fence);
                *out += &self.fence(fence, "");
                out.push('\n');
                *out += &self.code_indent(fence);
                self.pending.drain(..fence.indent);
                *out += CODE_BLOCK.get();
                Line::Code
            }
            Kind::Held => Line::Held,
            Kind::Heading(level) => {
                let style = if level == 1 { TITLE } else { HEADING }.get();
                *out += &self.pending[..indent];
                *out += style;
                if level > 2 {
                    *out += &text[..level];
                    out.push(' ');
                }
                self.pending = text[level..].trim_start().to_owned();
                Line::Text(style)
            }
            // As Scramjet's, a border for each level, the text in the quote's style.
            Kind::Quote => {
                if !self.lists.is_empty() {
                    *out += &self.pending[..indent];
                }
                let (mut rest, mut depth) = (text, 0);
                while let Some(after) = rest.strip_prefix('>') {
                    *out += &border;
                    depth += 1;
                    rest = after.strip_prefix(' ').unwrap_or(after);
                    let next = rest.trim_start_matches(' ');
                    if next.starts_with('>') {
                        rest = next;
                    }
                }
                if !rest.trim().is_empty() {
                    self.quoted = depth;
                }
                self.pending = rest.to_owned();
                Line::Text(QUOTE.get())
            }
            // As Scramjet's, bullets show as `-`, numbers end with `.`, and a task's box is part
            // of its marker.
            Kind::Item => {
                let (_, _, length) = marker(text);
                let number = self.lists.last().and_then(|list| list.number);
                let mut bullet = number.map_or("-".into(), |number| format!("{number}."));
                let mut content = text[length..].trim_start_matches(' ');
                if let Some(task) = TASKS.iter().find(|&&task| content.starts_with(task)) {
                    bullet += &format!(" {}", task.trim_end().to_lowercase());
                    content = content[task.len()..].trim_start_matches(' ');
                }
                let depth = self.lists.len().saturating_sub(1);
                let lead = " ".repeat(4 * depth);
                if let Some(list) = self.lists.last_mut() {
                    list.shown = lead.len() + bullet.len() + 1;
                }
                *out += &format!("{lead}{LIST_BULLET}{bullet}{RESET}{base} ");
                self.pending = content.to_owned();
                Line::Text("")
            }
            Kind::Rule => {
                let lead = if self.lists.is_empty() { 0 } else { indent };
                let width = match self.width {
                    0 => 80,
                    width => width.saturating_sub(lead).min(80),
                };
                *out += &" ".repeat(lead);
                *out += &format!("{HORIZONTAL_RULE}{}{RESET}{base}", "─".repeat(width));
                self.pending.clear();
                Line::Rule
            }
        }
    }

    /// Renders the spans of a text line that are complete, keeping an unclosed one pending.
    fn inline(&mut self, ended: bool, style: &'static str) -> String {
        let restore = format!("{RESET}{}{style}", self.base);
        let mut chars: Vec<char> = self.before.into_iter().collect();
        let from = chars.len();
        chars.extend(self.pending.chars());
        let (out, at) = spans(&chars, from, ended, &restore);
        if at > from {
            self.before = Some(chars[at - 1]);
        }
        self.pending = chars[at..].iter().collect();
        out
    }

    /// Takes a table's line, or one that may start a table; returns what shows now, the line
    /// before when it did not start a table.
    fn hold(&mut self, line: String) -> String {
        match &mut self.table {
            Some(Table {
                rows: Some(rows),
                header,
                lines,
                newline,
                ..
            }) => {
                let mut cells = cells(&line);
                cells.resize(header.len(), String::new());
                rows.push(cells);
                lines.push(line);
                *newline = false;
                return String::new();
            }
            Some(table) if delimiter(&line, table.header.len()) => {
                table.rows = Some(Vec::new());
                table.lines.push(line);
                table.newline = false;
                return String::new();
            }
            _ => {}
        }
        let out = self.flush_table();
        let text = line.trim_start_matches(' ');
        self.table = Some(Table {
            indent: line.len() - text.len(),
            header: cells(text),
            lines: vec![line],
            rows: None,
            newline: false,
            separable: self.separable,
        });
        out
    }

    /// Shows the table so far, or the line that did not start one as text.
    fn flush_table(&mut self) -> String {
        let Some(table) = self.table.take() else {
            return String::new();
        };
        let block = match table.rows {
            Some(_) => Block::Table,
            None => Block::Paragraph,
        };
        let mut out = match table.separable && separated(self.previous, block) {
            true => "\n".to_owned(),
            false => String::new(),
        };
        self.previous = Some(block);
        out += &match &table.rows {
            Some(rows) => self.table_lines(&table, rows).join("\n"),
            None => {
                let chars: Vec<char> = table.lines[0].chars().collect();
                complete(&chars, &format!("{RESET}{}", self.base))
            }
        };
        if table.newline {
            out.push('\n');
        }
        out
    }

    /// A table as Scramjet draws it: bordered, its header bold and a rule between its rows, its
    /// columns narrowed to fit with their cells wrapped; as written when even that cannot fit.
    fn table_lines(&self, table: &Table, rows: &[Vec<String>]) -> Vec<String> {
        let base = self.base;
        let indent = " ".repeat(table.indent);
        let count = table.header.len();
        let borders = 3 * count + 1;
        let room = match self.width {
            0 => usize::MAX,
            width => width.saturating_sub(table.indent),
        };
        if room < borders + count {
            return table.lines.clone();
        }
        let cell = |text: &str, style: &str| {
            let chars: Vec<char> = text.chars().collect();
            let restore = format!("{RESET}{base}{style}");
            format!("{style}{}", complete(&chars, &restore))
        };
        let header: Vec<String> = table
            .header
            .iter()
            .map(|text| cell(text, STRONG.get()))
            .collect();
        let body: Vec<Vec<String>> = (rows.iter())
            .map(|row| row.iter().map(|text| cell(text, "")).collect())
            .collect();
        let widths = column_widths(&header, &body, room - borders);
        let rule = |left: &str, middle: &str, right: &str| {
            let parts: Vec<String> = widths.iter().map(|&width| "─".repeat(width)).collect();
            format!(
                "{indent}{left}─{}─{right}",
                parts.join(&format!("─{middle}─"))
            )
        };
        let row = |cells: &[String], lines: &mut Vec<String>| {
            let wrapped: Vec<Vec<String>> = (cells.iter().zip(&widths))
                .map(|(text, &width)| cell_lines(text, width))
                .collect();
            let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
            for at in 0..height {
                let parts: Vec<String> = (wrapped.iter().zip(&widths))
                    .map(|(cell, &width)| {
                        let text = cell.get(at).map_or("", String::as_str);
                        let pad = " ".repeat(width.saturating_sub(columns(text)));
                        format!("{text}{base}{pad}")
                    })
                    .collect();
                lines.push(format!("{indent}│ {} │", parts.join(" │ ")));
            }
        };
        let mut lines = vec![rule("┌", "┬", "┐")];
        row(&header, &mut lines);
        lines.push(rule("├", "┼", "┤"));
        for (index, cells) in body.iter().enumerate() {
            if index > 0 {
                lines.push(rule("├", "┼", "┤"));
            }
            row(cells, &mut lines);
        }
        lines.push(rule("└", "┴", "┘"));
        lines
    }
}

/// The marker a list item's `text` starts with: what it ends with (`List::marker`), its number,
/// and its length.
fn marker(text: &str) -> (char, Option<u64>, usize) {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let end = text[digits..].chars().next().unwrap_or('-');
    (end, text[..digits].parse().ok(), digits + 1)
}

/// The cells of a table's line, between its unescaped `|`s.
fn cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let line = line.strip_prefix('|').unwrap_or(line);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cell).trim().to_owned()),
            c => cell.push(c),
        }
    }
    if !cell.trim().is_empty() {
        cells.push(cell.trim().to_owned());
    }
    cells
}

/// Whether `line` is the delimiter row of a header of `count` cells: `---`, `:--`, `--:` or
/// `:-:` in each.
fn delimiter(line: &str, count: usize) -> bool {
    let cells = cells(line);
    count > 0
        && cells.len() == count
        && cells.iter().all(|cell| {
            let dashes = cell.strip_prefix(':').unwrap_or(cell);
            let dashes = dashes.strip_suffix(':').unwrap_or(dashes);
            !dashes.is_empty() && dashes.bytes().all(|b| b == b'-')
        })
}

/// The widths of a table's columns, as Scramjet's: as wide as their cells when they fit in
/// `room`, else each narrowed toward its longest word (at most 30 columns), growing from there
/// in proportion to what more it needs.
fn column_widths(header: &[String], body: &[Vec<String>], room: usize) -> Vec<usize> {
    let count = header.len();
    let column =
        |at: usize| std::iter::once(&header[at]).chain(body.iter().map(move |row| &row[at]));
    let natural: Vec<usize> = (0..count)
        .map(|at| column(at).map(|cell| columns(cell)).max().unwrap_or(0))
        .collect();
    let words: Vec<usize> = (0..count)
        .map(|at| {
            column(at)
                .map(|cell| longest_word(cell))
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    let fitting: Vec<usize> = natural
        .iter()
        .zip(&words)
        .map(|(&n, &w)| n.max(w))
        .collect();
    if fitting.iter().sum::<usize>() <= room {
        return fitting;
    }
    let mut least = words.clone();
    if least.iter().sum::<usize>() > room {
        // Not even the longest words fit: one column each, the rest shared by their lengths.
        let spare = room - count;
        let weight: usize = words.iter().map(|word| word - 1).sum();
        least = (words.iter())
            .map(|word| 1 + (word - 1) * spare / weight.max(1))
            .collect();
        let left = room - least.iter().sum::<usize>();
        for width in least.iter_mut().take(left) {
            *width += 1;
        }
    }
    let wants: usize = natural
        .iter()
        .zip(&least)
        .map(|(n, l)| n.saturating_sub(*l))
        .sum();
    let spare = room.saturating_sub(least.iter().sum());
    let mut widths: Vec<usize> = (least.iter().zip(&natural))
        .map(|(&l, &n)| l + n.saturating_sub(l) * spare / wants.max(1))
        .collect();
    let mut left = room.saturating_sub(widths.iter().sum());
    while left > 0 {
        let mut grew = false;
        for (width, &n) in widths.iter_mut().zip(&natural) {
            if left > 0 && *width < n {
                *width += 1;
                left -= 1;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    widths
}

/// The columns of the longest word of a rendered cell, at most 30.
fn longest_word(cell: &str) -> usize {
    let words = plain(cell);
    let longest = words.split_whitespace().map(UnicodeWidthStr::width).max();
    longest.unwrap_or(0).min(30)
}

/// A rendered cell wrapped to `width`, each line ending its styles and starting with those
/// still on from the lines before.
fn cell_lines(text: &str, width: usize) -> Vec<String> {
    let mut wrap = Wrap::new(width);
    let wrapped = wrap.push(text) + &wrap.finish();
    let lines: Vec<&str> = wrapped.split('\n').collect();
    (selection::rows(&lines).into_iter())
        .map(|line| line.shown)
        .collect()
}

/// The spans of `chars` from `from` that are complete, rendered, restoring `restore` after each,
/// and where the first that is not starts.
fn spans(chars: &[char], from: usize, ended: bool, restore: &str) -> (String, usize) {
    let mut out = String::new();
    let mut at = from;
    while at < chars.len() {
        match span(chars, at, ended) {
            Span::Plain => {
                out.push(chars[at]);
                at += 1;
            }
            Span::Literal(n) => {
                out.extend(&chars[at..at + n]);
                at += n;
            }
            Span::Escaped => {
                out.push(chars[at + 1]);
                at += 2;
            }
            Span::Wait => break,
            Span::Code { content, end } => {
                out += &format!("{CODE}{content}{restore}");
                at = end;
            }
            Span::Styled { style, inner, end } => {
                out += style;
                out += &complete(&chars[inner.0..inner.1], &format!("{restore}{style}"));
                out += restore;
                at = end;
            }
            // As Scramjet shows a link where the terminal is not known to open one: its
            // address after its text when they differ.
            Span::Link { text, url, end } => {
                let inside = &chars[text.0..text.1];
                out += LINK.get();
                out += &complete(inside, &format!("{restore}{LINK}"));
                out += restore;
                let written: String = inside.iter().collect();
                if written != url && url.strip_prefix("mailto:") != Some(written.as_str()) {
                    out += &format!("{LINK_URL} ({url}){restore}");
                }
                at = end;
            }
            Span::Url { url, end } => {
                out += &format!("{LINK}{url}{restore}");
                at = end;
            }
        }
    }
    (out, at)
}

/// Complete text with its spans rendered.
fn complete(chars: &[char], restore: &str) -> String {
    spans(chars, 0, true, restore).0
}

fn run(chars: &[char], at: usize) -> usize {
    chars[at..].iter().take_while(|&&c| c == chars[at]).count()
}

fn span(chars: &[char], at: usize, ended: bool) -> Span {
    match chars[at] {
        '\\' => match chars.get(at + 1) {
            None if !ended => Span::Wait,
            Some(c) if c.is_ascii_punctuation() => Span::Escaped,
            _ => Span::Plain,
        },
        '`' => code_span(chars, at, ended),
        '*' | '_' | '~' => delimited(chars, at, ended),
        '[' => link(chars, at, ended),
        // An image, of which Scramjet shows the description.
        '!' => match chars.get(at + 1) {
            None if !ended => Span::Wait,
            Some('[') => match link(chars, at + 1, ended) {
                Span::Wait => Span::Wait,
                Span::Link { text, end, .. } => Span::Styled {
                    style: "",
                    inner: text,
                    end,
                },
                _ => Span::Plain,
            },
            _ => Span::Plain,
        },
        '<' => autolink(chars, at, ended),
        'h' => bare_url(chars, at, ended),
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

/// Emphasis (`*` or `_`), strong (two) or both (three), or struck text (`~~`): a run of the
/// marker after a non-word character and before text, closed by the same run after text and
/// before a non-word character. Code spans and escapes inside are skipped.
fn delimited(chars: &[char], at: usize, ended: bool) -> Span {
    let marker = chars[at];
    let n = run(chars, at);
    if at + n == chars.len() && !ended {
        return Span::Wait;
    }
    let style = match (marker, n) {
        ('~', 2) => STRUCK,
        ('~', _) => return Span::Literal(n),
        (_, 1) => EMPHASIS,
        (_, 2) => STRONG,
        (_, 3) => STRONG_EMPHASIS,
        _ => return Span::Literal(n),
    }
    .get();
    let opens = (at == 0 || !chars[at - 1].is_alphanumeric())
        && chars.get(at + n).is_some_and(|c| !c.is_whitespace());
    if !opens {
        return Span::Literal(n);
    }
    let mut k = at + n;
    while k < chars.len() {
        match chars[k] {
            '`' => match code_span(chars, k, ended) {
                Span::Wait => return Span::Wait,
                Span::Code { end, .. } => k = end,
                _ => k += run(chars, k),
            },
            '\\' => k += 2,
            c if c == marker => {
                let m = run(chars, k);
                if k + m == chars.len() && !ended {
                    return Span::Wait;
                }
                let closes = m == n
                    && !chars[k - 1].is_whitespace()
                    && chars.get(k + m).is_none_or(|c| !c.is_alphanumeric());
                if closes {
                    return Span::Styled {
                        style,
                        inner: (at + n, k),
                        end: k + m,
                    };
                }
                k += m;
            }
            _ => k += 1,
        }
    }
    if ended { Span::Literal(n) } else { Span::Wait }
}

/// `[text](address)`, perhaps with a title after the address.
fn link(chars: &[char], at: usize, ended: bool) -> Span {
    let unclosed = if ended { Span::Literal(1) } else { Span::Wait };
    let mut depth = 0;
    let mut k = at;
    let close = loop {
        let Some(&c) = chars.get(k) else {
            return unclosed;
        };
        match c {
            '[' => depth += 1,
            ']' if depth == 1 => break k,
            ']' => depth -= 1,
            '\\' => k += 1,
            '`' => match code_span(chars, k, ended) {
                Span::Wait => return Span::Wait,
                Span::Code { end, .. } => {
                    k = end;
                    continue;
                }
                _ => k += run(chars, k) - 1,
            },
            _ => {}
        }
        k += 1;
    };
    match chars.get(close + 1) {
        None if !ended => return Span::Wait,
        Some('(') => {}
        _ => return Span::Literal(1),
    }
    let mut k = close + 2;
    let mut url = String::new();
    let mut parens = 0;
    loop {
        match chars.get(k) {
            None => return unclosed,
            Some(')') if parens == 0 => break,
            Some(c) if c.is_whitespace() => break,
            Some(&c) => {
                parens += usize::from(c == '(');
                parens -= usize::from(c == ')');
                url.push(c);
            }
        }
        k += 1;
    }
    if chars[k] != ')' {
        let skip = |k: &mut usize| {
            while chars.get(*k).is_some_and(|c| c.is_whitespace()) {
                *k += 1;
            }
        };
        skip(&mut k);
        if let Some(&quote) = chars.get(k).filter(|&&c| c == '"' || c == '\'') {
            k += 1;
            while chars.get(k).is_some_and(|&c| c != quote) {
                k += 1;
            }
            k += 1;
            skip(&mut k);
        }
        match chars.get(k) {
            Some(')') => {}
            None if !ended => return Span::Wait,
            _ => return Span::Literal(1),
        }
    }
    Span::Link {
        text: (at + 1, close),
        url,
        end: k + 1,
    }
}

const WEB: [&str; 2] = ["https://", "http://"];

/// `<address>`, of a web page or a mail address.
fn autolink(chars: &[char], at: usize, ended: bool) -> Span {
    let rest = &chars[at + 1..];
    let schemes = [WEB[0], WEB[1], "mailto:"];
    if !schemes.iter().any(|scheme| starts_with(rest, scheme)) {
        let partial = schemes.iter().any(|scheme| begins(rest, scheme));
        return if partial && !ended {
            Span::Wait
        } else {
            Span::Plain
        };
    }
    match rest
        .iter()
        .position(|&c| c == '>' || c == '<' || c.is_whitespace())
    {
        Some(end) if rest[end] == '>' => Span::Url {
            url: rest[..end].iter().collect(),
            end: at + end + 2,
        },
        None if !ended => Span::Wait,
        _ => Span::Plain,
    }
}

/// An address of a web page in the text, which GFM links, without the punctuation after it.
fn bare_url(chars: &[char], at: usize, ended: bool) -> Span {
    if at > 0 && chars[at - 1].is_alphanumeric() {
        return Span::Plain;
    }
    let rest = &chars[at..];
    let Some(scheme) = WEB.into_iter().find(|scheme| starts_with(rest, scheme)) else {
        let partial = WEB.iter().any(|scheme| begins(rest, scheme));
        return if partial && !ended {
            Span::Wait
        } else {
            Span::Plain
        };
    };
    let mut end = (rest.iter())
        .position(|&c| c.is_whitespace() || c == '<')
        .unwrap_or(rest.len());
    if end == rest.len() && !ended {
        return Span::Wait;
    }
    while end > scheme.len() {
        let url = &rest[..end];
        let count = |c| url.iter().filter(|&&u| u == c).count();
        let last = url[end - 1];
        if !"?!.,:*_~'\";".contains(last) && (last != ')' || count('(') >= count(')')) {
            break;
        }
        end -= 1;
    }
    match end > scheme.len() {
        true => Span::Url {
            url: rest[..end].iter().collect(),
            end: at + end,
        },
        false => Span::Plain,
    }
}

fn starts_with(chars: &[char], prefix: &str) -> bool {
    let mut chars = chars.iter();
    prefix.chars().all(|p| chars.next() == Some(&p))
}

/// Whether `chars` are the start of `word`, but not all of it.
fn begins(chars: &[char], word: &str) -> bool {
    chars.len() < word.chars().count() && word.chars().zip(chars).all(|(w, &c)| w == c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(pieces: &[&str]) -> String {
        render_in(80, pieces)
    }

    fn render_in(width: usize, pieces: &[&str]) -> String {
        let mut markdown = Markdown::new(true, "", width);
        let mut out: String = pieces.iter().map(|piece| markdown.push(piece)).collect();
        out += &markdown.finish();
        out
    }

    /// The text rendered as a streamed reply's deltas are (`Terminal::stream`): newlines at
    /// the end of a delta wait for more text, its line ended meanwhile.
    fn streamed(width: usize, pieces: &[&str]) -> String {
        let mut markdown = Markdown::new(true, "", width);
        let (mut out, mut held) = (String::new(), 0);
        for piece in pieces {
            let body = piece.trim_end_matches('\n');
            if !body.is_empty() {
                out += &markdown.push(&("\n".repeat(std::mem::take(&mut held)) + body));
            }
            held += piece.len() - body.len();
            if held > 0 {
                out += &markdown.end_line();
            }
        }
        out + &markdown.finish()
    }

    /// The text rendered however it arrives: whole, and in three pieces split anywhere, pushed
    /// as they are and as a stream's deltas.
    fn rendered_alike(width: usize, text: &str) -> String {
        let whole = render_in(width, &[text]);
        let chars: Vec<char> = text.chars().collect();
        for a in 0..chars.len() {
            for b in a..chars.len() {
                let pieces: Vec<String> = [&chars[..a], &chars[a..b], &chars[b..]]
                    .map(|part| part.iter().collect())
                    .into();
                let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
                assert_eq!(render_in(width, &pieces), whole, "split at {a} and {b}");
                if !text.ends_with('\n') {
                    assert_eq!(
                        streamed(width, &pieces),
                        whole,
                        "streamed, split at {a}, {b}"
                    );
                }
            }
        }
        whole
    }

    #[test]
    fn spans_render_the_same_however_the_text_arrives() {
        let text = "Use **bold `x` text** and `a ** b` here.\n## Plan `p`\n- **Note:** done";
        assert_eq!(
            rendered_alike(80, text),
            "Use \x1b[1mbold \x1b[36mx\x1b[0m\x1b[1m text\x1b[0m and \x1b[36ma ** b\x1b[0m here.\n\n\
\x1b[1;33mPlan \x1b[36mp\x1b[0m\x1b[1;33m\x1b[0m\n\n\
\x1b[36m-\x1b[0m \x1b[1mNote:\x1b[0m done"
        );
        let text = "*it **b** \\* x* and ~~gone~~, x**y** [a *b*](u \"t\") <http://h/_x_>.";
        assert_eq!(
            rendered_alike(80, text),
            "\x1b[3mit \x1b[1mb\x1b[0m\x1b[3m * x\x1b[0m and \x1b[9mgone\x1b[0m, x**y** \
\x1b[4;34ma \x1b[3mb\x1b[0m\x1b[4;34m\x1b[0m\x1b[2m (u)\x1b[0m \x1b[4;34mhttp://h/_x_\x1b[0m."
        );
    }

    #[test]
    fn unclosed_markers_stay_as_typed() {
        for text in [
            "f(**kwargs) and 2**10 and a ` tick",
            "** spaced** and **not bold ** and * star *",
            "x**y** and snake_case_name and 3*4*5 and ****four****",
            "#hashtag and ####### seven and [not a link] (x) and [open",
            "a ~one~ and ~~~three~~~ and <notalink> and C:\\Users\\me and ![x]",
            "--- after text, 1.5 and 2) x",
        ] {
            assert_eq!(render(&[text, "\nnext"]), format!("{text}\nnext"));
        }
        assert_eq!(render(&["`a``b`"]), "\x1b[36ma``b\x1b[0m");
    }

    #[test]
    fn text_after_an_open_span_waits_for_it_to_close() {
        let mut markdown = Markdown::new(true, "", 80);
        assert_eq!(markdown.push("see **this"), "see ");
        assert_eq!(markdown.push(" part** now"), "\x1b[1mthis part\x1b[0m now");
        assert_eq!(markdown.push(" `open"), " ");
        assert_eq!(markdown.end_line(), "`open");
        assert_eq!(markdown.push("\n# T"), "\n\n\x1b[1;4;33mT");
        // Whether a marker opens depends on the text before it, already shown.
        assert_eq!(markdown.push("\nx**"), "\x1b[0m\n\nx");
        assert_eq!(markdown.push("y** z"), "**y** z");
    }

    #[test]
    fn code_blocks_show_their_lines_unstyled_inside() {
        // As Scramjet's: lines two columns in from their fence, which is shown as ```; lines lose
        // as many spaces as the fence has before it.
        let text =
            "Run:\n  ```\n  echo **x** `y`\n\n# not a heading\n     deep\n  ```\n### Three\nafter";
        assert_eq!(
            rendered_alike(80, text),
            "Run:\n\n  \x1b[2m```\x1b[0m\n    \x1b[32mecho **x** `y`\x1b[0m\n\n    \
\x1b[32m# not a heading\x1b[0m\n    \x1b[32m   deep\x1b[0m\n  \x1b[2m```\x1b[0m\n\n\
\x1b[1;33m### Three\x1b[0m\n\nafter"
        );
        assert_eq!(
            render(&["~~~~  nonsense\n```\nstill code\n~~~~\n**b**"]),
            "\x1b[2m```nonsense\x1b[0m\n  \x1b[32m```\x1b[0m\n  \x1b[32mstill code\x1b[0m\n\
\x1b[2m```\x1b[0m\n\n\x1b[1mb\x1b[0m"
        );
        assert_eq!(
            render(&["```\n| a |\n|---|\n> q\n```"]),
            "\x1b[2m```\x1b[0m\n  \x1b[32m| a |\x1b[0m\n  \x1b[32m|---|\x1b[0m\n  \
\x1b[32m> q\x1b[0m\n\x1b[2m```\x1b[0m"
        );
    }

    #[test]
    fn a_code_block_left_open_is_closed_at_the_end() {
        let closed = "\x1b[2m```\x1b[0m";
        assert_eq!(
            rendered_alike(80, "```\nx"),
            format!("{closed}\n  \x1b[32mx\x1b[0m\n{closed}")
        );
        assert_eq!(
            render(&["```\nx\n"]),
            format!("{closed}\n  \x1b[32mx\x1b[0m\n{closed}")
        );
        assert_eq!(render(&["  ```"]), format!("  {closed}\n  {closed}"));
        // Ended while its newline waits, as a streamed reply's line does.
        let mut markdown = Markdown::new(true, "", 80);
        markdown.push("```\nx");
        markdown.end_line();
        assert_eq!(markdown.finish(), format!("\n{closed}"));
    }

    #[test]
    fn marked_code_lines_start_their_copy_after_the_indent() {
        let mut markdown = Markdown::new(true, "", 80).marking(true);
        // The text's own marks are not taken as the renderer's.
        let text = format!("a{FROM}b{JOINED}{SPACE}{RULE}\n```\n  x\n\n```");
        assert_eq!(
            markdown.push(&text) + &markdown.finish(),
            format!("ab\n\n\x1b[2m```\x1b[0m\n  {FROM}\x1b[32m  x\x1b[0m\n\n\x1b[2m```\x1b[0m")
        );
        // Lines wrapped from one go under its text, and a copy joins them.
        let mut wrap = Wrap::new(12).marking(true).keeping_gutters();
        let rows = wrap.push(&format!("  {FROM}  a b c d e")) + &wrap.finish();
        assert_eq!(rows, format!("  {FROM}  a b c d\n    {JOINED}{SPACE}e"));
        let lines = selection::lines(&rows);
        assert_eq!(selection::text(&lines, 0, 99), "  a b c d e");
        // Other wraps drop them.
        let mut wrap = Wrap::new(8).marking(true);
        assert_eq!(wrap.push(&format!("  {FROM}a")) + &wrap.finish(), "  a");
    }

    #[test]
    fn code_blocks_that_name_their_language_are_highlighted_line_by_line() {
        use crate::agent_cli::style::{SYNTAX_COMMENT, SYNTAX_KEYWORD, SYNTAX_STRING};
        let text =
            "```python title\ns = \"\"\"a\n\nb\"\"\" # c\n```\n`x` ~~~ *y*\n~~~nonsense\nif\n~~~";
        assert_eq!(
            rendered_alike(80, text),
            format!(
                "\x1b[2m```python title\x1b[0m\n  s = {SYNTAX_STRING}\"\"\"a{RESET}\n\n  \
{SYNTAX_STRING}b\"\"\"{RESET} {SYNTAX_COMMENT}# c{RESET}\n\
\x1b[2m```\x1b[0m\n\n{CODE}x{RESET} ~~~ {EMPHASIS}y{RESET}\n\n\x1b[2m```nonsense\x1b[0m\n  \
{CODE_BLOCK}if{RESET}\n\x1b[2m```\x1b[0m"
            )
        );
        // A line shows once it ends, in the block's style.
        let mut markdown = Markdown::new(true, "\x1b[2m", 80);
        assert_eq!(
            markdown.push("```rs\nlet"),
            "\x1b[2m```rs\x1b[0m\x1b[2m\n  "
        );
        assert_eq!(markdown.reopen(), "\x1b[2m");
        assert_eq!(
            markdown.push(" x\n"),
            format!("{SYNTAX_KEYWORD}let{RESET}\x1b[2m x\n")
        );
    }

    #[test]
    fn indented_code_shows_between_fences_unless_it_goes_on_with_text() {
        let (fence, code) = ("\x1b[2m```\x1b[0m", "\x1b[32m");
        assert_eq!(
            rendered_alike(80, "Run:\n\n    let x;\n\n\n      **y**\n\nafter"),
            format!(
                "Run:\n\n{fence}\n  {code}let x;\x1b[0m\n\n\n  {code}  **y**\x1b[0m\n{fence}\n\nafter"
            )
        );
        assert_eq!(
            rendered_alike(80, "# T\n    c\nnext"),
            format!("\x1b[1;4;33mT\x1b[0m\n\n{fence}\n  {code}c\x1b[0m\n{fence}\n\nnext")
        );
        // In a list, four columns past its item's text.
        assert_eq!(
            rendered_alike(80, "- a\n\n        c\n\n    text"),
            format!("\x1b[36m-\x1b[0m a\n\n  {fence}\n    {code}  c\x1b[0m\n  {fence}\n\n  text")
        );
        assert_eq!(render(&["text\n    more"]), "text\n    more");
        assert_eq!(
            render(&["    c\n\n"]),
            format!("{fence}\n  {code}c\x1b[0m\n{fence}\n")
        );
    }

    #[test]
    fn a_backslash_ending_a_line_breaks_it_when_its_text_goes_on() {
        assert_eq!(
            rendered_alike(80, "one\\\ntwo\\\n- a\\\n  b"),
            "one\ntwo\\\n\x1b[36m-\x1b[0m a\n  b"
        );
        let quote = "\x1b[0m\x1b[34m│\x1b[0m\x1b[2;3m ";
        assert_eq!(
            rendered_alike(80, "> q\\\n> r\\\n>\n> s\\"),
            format!(
                "{quote}q\x1b[0m\n{quote}r\x1b[0m\x1b[2;3m\\\x1b[0m\n{quote}\x1b[0m\n{quote}s\x1b[0m\x1b[2;3m\\\x1b[0m"
            )
        );
        assert_eq!(render(&["end\\\n\nnext"]), "end\\\n\nnext");
        assert_eq!(render(&["# H\\\nx"]), "\x1b[1;4;33mH\\\x1b[0m\n\nx");
        assert_eq!(render(&["two\\\\\nx", "\\\\\\\ny"]), "two\\\nx\\\ny");
    }

    #[test]
    fn styles_restore_the_block_style() {
        let mut markdown = Markdown::new(true, "\x1b[2m", 80);
        assert_eq!(
            markdown.push("**Title** `c`\n"),
            "\x1b[1mTitle\x1b[0m\x1b[2m \x1b[36mc\x1b[0m\x1b[2m\n"
        );
        assert_eq!(
            markdown.push("```\nx"),
            "\n\x1b[2m```\x1b[0m\x1b[2m\n  \x1b[32mx"
        );
        assert_eq!(markdown.reopen(), "\x1b[2m\x1b[32m");
        let mut plain = Markdown::new(false, "", 80);
        assert_eq!(plain.push("**a** `b`\n```"), "**a** `b`\n```");
        assert_eq!(plain.finish() + &plain.reopen(), "");
    }

    #[test]
    fn quotes_rules_and_list_bullets_render_as_scramjet_shows_them() {
        let text = "> a *b*\n>> deep\n>\n---\n* one\n  + two\n12. three\n***\n- [x] done";
        let border = "\x1b[0m\x1b[34m│\x1b[0m\x1b[2;3m ";
        let rule = format!("\x1b[2m{}\x1b[0m", "─".repeat(20));
        assert_eq!(
            rendered_alike(20, text),
            format!(
                "{border}a \x1b[3mb\x1b[0m\x1b[2;3m\x1b[0m\n{border}{border}deep\x1b[0m\n\
{border}\x1b[0m\n\n{rule}\n\n\x1b[36m-\x1b[0m one\n    \x1b[36m-\x1b[0m two\n\
\x1b[36m12.\x1b[0m three\n{rule}\n\n\x1b[36m- [x]\x1b[0m done"
            )
        );
        assert!(render(&["___"]).contains(&"─".repeat(80)));
        assert!(render_in(0, &["- - -"]).contains(&"─".repeat(80)));
    }

    #[test]
    fn lists_render_as_scramjet_shows_them() {
        // Each level four columns in, numbered from the first item, a task's box in its marker,
        // an item's other lines under its text, and no blank line between its items.
        let text = "1. **a**\n   - b\n     lazy\n1. c\n\n   more c\n\n7. d\n- [ ] e\n  - [X]  f\n\n\nafter";
        let bullet = |marker: &str| format!("\x1b[36m{marker}\x1b[0m ");
        assert_eq!(
            rendered_alike(80, text),
            format!(
                "{}\x1b[1ma\x1b[0m\n    {}b\n      lazy\n{}c\n\n   more c\n{}d\n{}e\n    {}f\n\nafter",
                bullet("1."),
                bullet("-"),
                bullet("2."),
                bullet("3."),
                bullet("- [ ]"),
                bullet("- [x]"),
            )
        );
        // Blocks in an item go under its text, and code lines lose the indent of their fence.
        let text = "- run:\n  ```\n    x\n  ```\n  > q\n  - b\n    ```\n      y\n    ```\n* new";
        let (fence, border) = ("\x1b[2m```\x1b[0m", "\x1b[0m\x1b[34m│\x1b[0m\x1b[2;3m ");
        assert_eq!(
            rendered_alike(80, text),
            format!(
                "{0}run:\n  {fence}\n    \x1b[32m  x\x1b[0m\n  {fence}\n  {border}q\x1b[0m\n    {0}b\n      \
{fence}\n        \x1b[32m  y\x1b[0m\n      {fence}\n{0}new",
                bullet("-")
            )
        );
        // Outside lists, a run of blank lines shows as one, except in a code block.
        assert_eq!(
            rendered_alike(80, "a\n\n  \n\nb\n```\n\n\n```"),
            format!("a\n\nb\n\n{fence}\n\n\n{fence}")
        );
        // The blank line before a later item waits for it.
        let mut markdown = Markdown::new(true, "", 80);
        assert_eq!(markdown.push("- a\n\n"), format!("{}a\n", bullet("-")));
        assert_eq!(markdown.push("- b"), format!("{}b", bullet("-")));
        assert_eq!(markdown.push("\n\n") + &markdown.finish(), "\n\n");
    }

    #[test]
    fn blocks_the_text_did_not_separate_show_a_blank_line_between() {
        // As Scramjet's: after anything but a list, unless a paragraph or quote goes on, or a
        // list follows a paragraph; none between the blocks in an item.
        let text = "# T\ntext\n- a\n# U\n```\nx\n```\n---\n> q\nlazy\n# V\n| h |\n|-|\n- b\n  ```\n  y\n  ```\nz\n| not |\nmore";
        let rule = "─".repeat(30);
        assert_eq!(
            plain(&rendered_alike(30, text)),
            format!(
                "T\n\ntext\n- a\nU\n\n```\n  x\n```\n\n{rule}\n\n│ q\n│ lazy\n\nV\n\n┌───┐\n│ h │\n├───┤\n└───┘\n\n- b\n  ```\n    y\n  ```\nz\n| not |\nmore"
            )
        );
        // A line of text goes on in the quotes of the text before it, in their style.
        let border = "\x1b[0m\x1b[34m│\x1b[0m\x1b[2;3m ";
        assert_eq!(
            rendered_alike(80, "> > r\n lazy\n>\nnew"),
            format!(
                "{border}{border}r\x1b[0m\n{border}{border}lazy\x1b[0m\n{border}\x1b[0m\n\nnew"
            )
        );
    }

    #[test]
    fn tables_are_drawn_once_complete_with_their_columns_fitted() {
        let text = "Before\n| Name | Note |\n|:--|--:|\n| **a** | x |\n| b \\| c | `y` |\n\nafter";
        assert_eq!(
            rendered_alike(40, text),
            "Before\n\n\
┌───────┬──────┐\n\
│ \x1b[1mName\x1b[0m  │ \x1b[1mNote\x1b[0m │\n\
├───────┼──────┤\n\
│ \x1b[1ma\x1b[0m     │ x    │\n\
├───────┼──────┤\n\
│ b | c │ \x1b[36my\x1b[0m    │\n\
└───────┴──────┘\n\nafter"
        );
        // Cells wrap when the table is too wide, between words, longest words first.
        let wide = "| key | what it means |\n|---|---|\n| a | one two three four |";
        let lines = render_in(20, &[wide]);
        assert_eq!(
            lines,
            "┌─────┬────────────┐\n\
│ \x1b[1mkey\x1b[0m │ \x1b[1mwhat it\x1b[0m    │\n\
│     │ \x1b[1mmeans\x1b[0m      │\n\
├─────┼────────────┤\n\
│ a   │ one two    │\n\
│     │ three four │\n\
└─────┴────────────┘"
        );
        assert!(lines.lines().all(|line| columns(line) <= 20));
        // Too narrow for its columns: as written.
        assert_eq!(render_in(8, &["|a|b|\n|-|-|"]), "|a|b|\n|-|-|");
    }

    #[test]
    fn a_line_that_does_not_start_a_table_is_text() {
        assert_eq!(
            render(&["| a **b** |\nnext"]),
            "| a \x1b[1mb\x1b[0m |\nnext"
        );
        assert_eq!(
            render(&["| a |\n| b | c |\n|--|--|"]),
            "| a |\n\n┌───┬───┐\n│ \x1b[1mb\x1b[0m │ \x1b[1mc\x1b[0m │\n├───┼───┤\n└───┴───┘"
        );
        // Shown as the line ends, while the reply streams.
        let mut markdown = Markdown::new(true, "", 80);
        assert_eq!(markdown.push("| a |\n|-|\n| 1 |"), "");
        assert_eq!(markdown.end_line(), "");
        assert_eq!(markdown.push("\n"), "");
        assert_eq!(markdown.push("text"), "");
        assert_eq!(
            markdown.push("\n\nafter"),
            "┌──────┐\n│ \x1b[1ma\x1b[0m    │\n├──────┤\n│ 1    │\n├──────┤\n│ text │\n└──────┘\n\nafter"
        );
    }

    #[test]
    fn a_table_goes_on_in_lines_that_start_no_other_block() {
        let text = "| a | b |\n|---|---|\n1 | 2\n   x\n-|y|z\n#h";
        assert_eq!(
            rendered_alike(40, text),
            "┌────┬───┐\n│ \x1b[1ma\x1b[0m  │ \x1b[1mb\x1b[0m │\n├────┼───┤\n│ 1  │ 2 │\n\
├────┼───┤\n│ x  │   │\n├────┼───┤\n│ -  │ y │\n├────┼───┤\n│ #h │   │\n└────┴───┘"
        );
        let table = "┌───┐\n│ \x1b[1ma\x1b[0m │\n├───┤\n│ 1 │\n└───┘\n\n";
        for (after, shown) in [
            ("- i", "\x1b[36m-\x1b[0m i"),
            ("1. i", "\x1b[36m1.\x1b[0m i"),
            ("---", &format!("\x1b[2m{}\x1b[0m", "─".repeat(40))),
            (
                "    c",
                "\x1b[2m```\x1b[0m\n  \x1b[32mc\x1b[0m\n\x1b[2m```\x1b[0m",
            ),
        ] {
            let text = format!("|a|\n|-|\n|1|\n{after}");
            assert_eq!(
                render_in(40, &[&text]),
                format!("{table}{shown}"),
                "{after}"
            );
        }
    }

    #[test]
    fn column_widths_share_what_room_there_is() {
        let row = |cells: &[&str]| {
            cells
                .iter()
                .map(|cell| cell.to_string())
                .collect::<Vec<_>>()
        };
        let header = row(&["ab", "cdefgh"]);
        assert_eq!(column_widths(&header, &[], 20), [2, 6]);
        assert_eq!(column_widths(&header, &[row(&["x", "y z"])], 5), [2, 3]);
        let long = "w".repeat(40);
        assert_eq!(column_widths(&row(&[&long, "a"]), &[], 20), [19, 1]);
        assert_eq!(column_widths(&row(&[&long, "a"]), &[], 40), [39, 1]);
    }
}
