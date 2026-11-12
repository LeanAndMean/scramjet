//! Finds an edit's `oldText` in a file as Scramjet's edit tool does: with line endings and a byte
//! order mark aside, else also trailing whitespace and typographic quotes, dashes and spaces.
//! Scramjet then writes the whole file in that normalized form; here the match is mapped back to
//! the file, so only the text it covers changes. Last, as Codex's `apply_patch`, each line's
//! indentation is set aside too.

use std::cell::OnceCell;

/// A text in normalized form, with where its bytes came from.
struct View {
    text: String,
    /// `(offset in text, offset in the original)` from which both advance together.
    marks: Vec<(usize, usize)>,
}

impl View {
    /// `original` without a leading byte order mark and with `\r\n` as `\n`; when `Loose`, also
    /// without each line's trailing whitespace and with typographic quotes, dashes and spaces as
    /// ASCII ones; when `Flush`, also without its indentation. Text dropped before a line's `\n`
    /// belongs to it.
    fn new(original: &str, level: Level) -> View {
        let start = original
            .strip_prefix('\u{feff}')
            .map_or(0, |_| '\u{feff}'.len_utf8());
        let mut view = View {
            text: String::with_capacity(original.len()),
            marks: vec![(0, start)],
        };
        let mut at = start;
        for line in original[start..].split_inclusive('\n') {
            let ending = line.strip_suffix('\n');
            let mut body = ending.map_or(line, |line| line.strip_suffix('\r').unwrap_or(line));
            if level != Level::Exact {
                body = body.trim_end();
                let indent = match level {
                    Level::Flush => indentation(body).len(),
                    _ => 0,
                };
                for (offset, c) in body[indent..].char_indices() {
                    let char = &body[indent + offset..indent + offset + c.len_utf8()];
                    let from = at + indent + offset;
                    view.push(from, plain(c).unwrap_or(char), from + char.len());
                }
            } else {
                view.push(at, body, at + body.len());
            }
            if ending.is_some() {
                view.push(at + body.len(), "\n", at + line.len());
            }
            at += line.len();
        }
        view
    }

    /// Adds `piece` as the normalized form of the original's `from..to`.
    fn push(&mut self, from: usize, piece: &str, to: usize) {
        self.mark(from);
        self.text.push_str(piece);
        self.mark(to);
    }

    fn mark(&mut self, origin: usize) {
        let at = self.text.len();
        if self.origin(at) != origin {
            if self.marks.last().is_some_and(|mark| mark.0 == at) {
                self.marks.pop();
            }
            self.marks.push((at, origin));
        }
    }

    fn origin(&self, at: usize) -> usize {
        let (text, original) = self.marks[self.marks.partition_point(|mark| mark.0 <= at) - 1];
        original + (at - text)
    }

    /// The original range of the one place `old` is in the text, or of every place when `all`,
    /// else the lines of `original` where each place starts.
    fn find(&self, old: &str, original: &str, all: bool) -> Result<Vec<(usize, usize)>, Miss> {
        if old.is_empty() {
            return Err(Miss::Absent);
        }
        let starts: Vec<usize> = self.text.match_indices(old).map(|(at, _)| at).collect();
        let range = |&at: &usize| (self.origin(at), self.origin(at + old.len()));
        match starts[..] {
            [] => Err(Miss::Absent),
            [_] => Ok(starts.iter().map(range).collect()),
            _ if all => Ok(starts.iter().map(range).collect()),
            _ => {
                let (mut line, mut counted) = (1, 0);
                let lines = starts.iter().map(|&at| {
                    let at = self.origin(at);
                    line += original.as_bytes()[counted..at]
                        .iter()
                        .filter(|&&b| b == b'\n')
                        .count();
                    counted = at;
                    line
                });
                Err(Miss::Many(lines.collect()))
            }
        }
    }
}

/// The ASCII character Scramjet's fuzzy matching takes `c` as, when it is not one.
fn plain(c: char) -> Option<&'static str> {
    match c {
        '\u{2018}'..='\u{201B}' => Some("'"),
        '\u{201C}'..='\u{201F}' => Some("\""),
        '\u{2010}'..='\u{2015}' | '\u{2212}' => Some("-"),
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => Some(" "),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Exact,
    Loose,
    Flush,
}

/// The whitespace starting `line`.
fn indentation(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

pub enum Miss {
    Absent,
    /// The 1-based line where each occurrence starts.
    Many(Vec<usize>),
}

/// Finds edits' old texts in one file.
pub struct Finder<'a> {
    original: &'a str,
    exact: View,
    loose: OnceCell<View>,
    flush: OnceCell<View>,
}

impl<'a> Finder<'a> {
    pub fn new(original: &'a str) -> Finder<'a> {
        Finder {
            original,
            exact: View::new(original, Level::Exact),
            loose: OnceCell::new(),
            flush: OnceCell::new(),
        }
    }

    /// The range of the original that `old` stands for, or every such range when `all`, and
    /// `new` as it is to be written there.
    pub fn find(
        &self,
        old: &str,
        new: &str,
        all: bool,
    ) -> Result<(Vec<(usize, usize)>, String), Miss> {
        // A byte order mark starting `old`, or a `\r` ending it, is the file's, copied from `read`;
        // the match leaves it in place, so `new` must not bring it again.
        let (old, new) = match old.strip_prefix('\u{feff}') {
            Some(old) => (old, new.strip_prefix('\u{feff}').unwrap_or(new)),
            None => (old, new),
        };
        let (old, new) = match old.strip_suffix('\r') {
            Some(old) => (old, new.strip_suffix('\r').unwrap_or(new)),
            None => (old, new),
        };
        match self.locate(old, all) {
            Ok(ranges) => Ok((ranges, self.with_endings(new))),
            Err(Miss::Absent) => {
                let (ranges, new) = self.flush(old, new, all)?;
                Ok((ranges, self.with_endings(&new)))
            }
            Err(miss) => Err(miss),
        }
    }

    /// The places `old` is with each line's indentation set aside, as Codex's `apply_patch` finds
    /// lines; `new` is written as given, as its patch lines are.
    fn flush(
        &self,
        old: &str,
        new: &str,
        all: bool,
    ) -> Result<(Vec<(usize, usize)>, String), Miss> {
        let (mut old, mut new) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
        // A last line of whitespace only is the indentation of the file's next line, left as it is.
        for text in [&mut old, &mut new] {
            if let Some(end) = text
                .rfind('\n')
                .filter(|&end| text[end..].trim().is_empty())
            {
                text.truncate(end + 1);
            }
        }
        let flushed = View::new(&old, Level::Flush).text;
        if flushed.trim().is_empty() {
            return Err(Miss::Absent);
        }
        let view = self
            .flush
            .get_or_init(|| View::new(self.original, Level::Flush));
        let mut ranges = view.find(&flushed, self.original, all)?;
        // Ending at a line's end, it leaves the next line's indentation in place.
        if flushed.ends_with('\n') {
            for range in &mut ranges {
                range.1 = self.original[..range.1]
                    .rfind('\n')
                    .map_or(range.1, |n| n + 1);
            }
        }
        // The match starts at the first line's text, after the file's own indentation.
        let first = indentation(old.split('\n').next().unwrap_or_default());
        let new = new.strip_prefix(first).unwrap_or(&new).to_owned();
        Ok((ranges, new))
    }

    /// The one place (or every place, when `all`) `old` is, line endings aside, else loosely.
    fn locate(&self, old: &str, all: bool) -> Result<Vec<(usize, usize)>, Miss> {
        match self
            .exact
            .find(&View::new(old, Level::Exact).text, self.original, all)
        {
            Err(Miss::Absent) => {}
            found => return found,
        }
        let loose = (self.loose).get_or_init(|| View::new(self.original, Level::Loose));
        let trimmed = View::new(old, Level::Loose).text;
        // Whitespace ending `old` may be the indentation of the file's next line, which `new`
        // brings again; Scramjet's trimmed form would leave it in the file too.
        let last = &old[old.rfind('\n').map_or(0, |n| n + 1)..];
        let tail = &last[last.trim_end().len()..];
        if !tail.is_empty() {
            let mut kept = trimmed.clone();
            kept.extend(tail.chars().map(|c| plain(c).map_or(c, |_| ' ')));
            match loose.find(&kept, self.original, all) {
                Err(Miss::Absent) => {}
                found => return found,
            }
        }
        loose.find(&trimmed, self.original, all)
    }

    /// `new` with the line ending of the file's first line, as Scramjet writes it.
    fn with_endings(&self, new: &str) -> String {
        let new = new.replace("\r\n", "\n");
        let crlf = self
            .original
            .find('\n')
            .is_some_and(|end| self.original[..end].ends_with('\r'));
        if crlf { new.replace('\n', "\r\n") } else { new }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(original: &str, old: &str) -> Option<String> {
        let (ranges, _) = Finder::new(original).find(old, "", false).ok()?;
        let [(start, end)] = ranges[..] else {
            return None;
        };
        Some(original[start..end].to_owned())
    }

    #[test]
    fn old_text_stands_for_the_file_text_it_matches_with_line_endings_aside() {
        let crlf = "\u{feff}one\r\ntwo\r\nthree";
        assert_eq!(found(crlf, "one\ntwo").as_deref(), Some("one\r\ntwo"));
        assert_eq!(
            found(crlf, "one\r\ntwo\n").as_deref(),
            Some("one\r\ntwo\r\n")
        );
        assert_eq!(found(crlf, "\nthree").as_deref(), Some("\r\nthree"));
        assert_eq!(found(crlf, "one").as_deref(), Some("one"));
        assert_eq!(found(crlf, "\u{feff}one").as_deref(), Some("one"));
        assert_eq!(found("a\rb", "a\nb"), None);
    }

    #[test]
    fn old_text_matches_loosely_only_where_it_matches_nowhere_exactly() {
        let text = "say “hi” – now  \n\tthen\u{a0}go\t\nsay \"hi\" - now\n";
        assert_eq!(
            found(text, "say \"hi\" - now").as_deref(),
            Some("say \"hi\" - now")
        );
        // Trailing whitespace inside the match goes with it; after its last line it stays.
        assert_eq!(
            found(text, "say \"hi\" — now\n\tthen go").as_deref(),
            Some("say “hi” – now  \n\tthen\u{a0}go")
        );
        assert_eq!(
            found(text, "now   \n\tthen go  \n").as_deref(),
            Some("now  \n\tthen\u{a0}go\t\n")
        );
        assert!(matches!(
            Finder::new("“a”\r\n\r\n”a“\n").find("\"a\"", "", false).err(),
            Some(Miss::Many(lines)) if lines == [1, 3]
        ));
        assert!(matches!(
            Finder::new("a \n").find("\t", "", false),
            Err(Miss::Absent)
        ));
        assert!(matches!(
            Finder::new("ab").find("b\n", "", false),
            Err(Miss::Absent)
        ));
    }

    #[test]
    fn all_takes_every_place_old_text_is_exactly_else_loosely() {
        let all = |original: &str, old: &str| {
            let (ranges, _) = Finder::new(original).find(old, "", true).ok()?;
            Some(ranges)
        };
        assert_eq!(all("ab ab\nab", "ab"), Some(vec![(0, 2), (3, 5), (6, 8)]));
        assert_eq!(all("ab", "ab"), Some(vec![(0, 2)]));
        assert_eq!(all("aaa", "aa"), Some(vec![(0, 2)]));
        assert_eq!(all("“a” \n“a”\r\n", "\"a\""), Some(vec![(0, 7), (9, 16)]));
        assert_eq!(all("ab", "c"), None);
    }

    #[test]
    fn what_old_text_copies_of_the_file_beyond_its_text_new_text_does_not_add_again() {
        let replaced = |original: &str, old: &str, new: &str| {
            let (ranges, new) = Finder::new(original).find(old, new, false).ok()?;
            let mut text = original.to_owned();
            text.replace_range(ranges[0].0..ranges[0].1, &new);
            Some(text)
        };
        let bom = "\u{feff}one\r\ntwo\r\n";
        assert_eq!(
            replaced(bom, "\u{feff}one", "\u{feff}uno").as_deref(),
            Some("\u{feff}uno\r\ntwo\r\n")
        );
        let crlf = "x\r\ny\r\nw\r\n";
        assert_eq!(
            replaced(crlf, "x\r\ny\r", "x\r\nz\r").as_deref(),
            Some("x\r\nz\r\nw\r\n")
        );
        assert_eq!(
            replaced(crlf, "y\r", "z").as_deref(),
            Some("x\r\nz\r\nw\r\n")
        );
        // A loose match takes the whitespace ending the old text where the file has it, else
        // matches without it.
        let code = "if “a”:\n    go()\n“b” \n";
        assert_eq!(
            replaced(code, "if \"a\":\n  ", "if \"x\":\n  ").as_deref(),
            Some("if \"x\":\n    go()\n“b” \n")
        );
        assert_eq!(
            replaced(code, "\"b\"\u{a0}\t", "c").as_deref(),
            Some("if “a”:\n    go()\nc \n")
        );
    }

    #[test]
    fn indentation_set_aside_last_writes_new_text_as_given() {
        let replaced = |original: &str, old: &str, new: &str| {
            let (ranges, new) = Finder::new(original).find(old, new, false).ok()?;
            let mut text = original.to_owned();
            text.replace_range(ranges[0].0..ranges[0].1, &new);
            Some(text)
        };
        let code = "fn f() {\n    if a {\n        go();\n    }\n}\n";
        assert_eq!(
            replaced(
                code,
                "if a {\n        go();\n    }",
                "if b {\n        stop();\n    }"
            )
            .as_deref(),
            Some("fn f() {\n    if b {\n        stop();\n    }\n}\n")
        );
        assert_eq!(
            replaced(code, "if a {\n    go();\n}", "if b {\n    stop();\n}").as_deref(),
            Some("fn f() {\n    if b {\n    stop();\n}\n}\n")
        );
        assert_eq!(
            replaced("a:\n  b\n", "    a:\n      b", "    a:\n  c").as_deref(),
            Some("a:\n  c\n")
        );
        // Ending at a line's end, or with the next line's indentation, leaves that line as it is.
        let lines = "    a\r\n    b\r\n    c\r\n";
        assert_eq!(
            replaced(lines, "a\nb\n", "x\n").as_deref(),
            Some("    x\r\n    c\r\n")
        );
        assert_eq!(
            replaced(lines, "a\nb\n  ", "x\n  ").as_deref(),
            Some("    x\r\n    c\r\n")
        );
        assert!(matches!(
            Finder::new("  a\n b\n\n a\nb\n").find("a\n  b", "", false),
            Err(Miss::Many(lines)) if lines == [1, 4]
        ));
    }

    #[test]
    fn new_text_takes_the_first_line_ending_of_the_file() {
        assert_eq!(
            Finder::new("a\r\nb\n").with_endings("x\ny\r\n"),
            "x\r\ny\r\n"
        );
        assert_eq!(Finder::new("a\nb\r\n").with_endings("x\r\ny\n"), "x\ny\n");
        assert_eq!(Finder::new("a").with_endings("x\r\ny"), "x\ny");
    }
}
