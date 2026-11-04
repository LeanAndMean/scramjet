//! Finds an edit's `oldText` in a file as Scramjet's edit tool does: with line endings and a byte
//! order mark aside, else also trailing whitespace and typographic quotes, dashes and spaces.
//! Scramjet then writes the whole file in that normalized form; here the match is mapped back to
//! the file, so only the text it covers changes.

use std::cell::OnceCell;

/// A text in normalized form, with where its bytes came from.
struct View {
    text: String,
    /// `(offset in text, offset in the original)` from which both advance together.
    marks: Vec<(usize, usize)>,
}

impl View {
    /// `original` without a leading byte order mark and with `\r\n` as `\n`; when `loose`, also
    /// without each line's trailing whitespace and with typographic quotes, dashes and spaces as
    /// ASCII ones. Text dropped before a line's `\n` belongs to it.
    fn new(original: &str, loose: bool) -> View {
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
            if loose {
                body = body.trim_end();
                for (offset, c) in body.char_indices() {
                    let char = &body[offset..offset + c.len_utf8()];
                    let from = at + offset;
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

    /// The original range of the one place `old` is in the text.
    fn find(&self, old: &str) -> Result<(usize, usize), Miss> {
        if old.is_empty() {
            return Err(Miss::Absent);
        }
        let mut found = self.text.match_indices(old);
        let Some((start, _)) = found.next() else {
            return Err(Miss::Absent);
        };
        match found.count() {
            0 => Ok((self.origin(start), self.origin(start + old.len()))),
            more => Err(Miss::Many(more + 1)),
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

pub enum Miss {
    Absent,
    Many(usize),
}

/// Finds edits' old texts in one file.
pub struct Finder<'a> {
    original: &'a str,
    exact: View,
    loose: OnceCell<View>,
}

impl<'a> Finder<'a> {
    pub fn new(original: &'a str) -> Finder<'a> {
        Finder {
            original,
            exact: View::new(original, false),
            loose: OnceCell::new(),
        }
    }

    /// The range of the original that `old` stands for, and `new` as it is to be written there.
    pub fn find(&self, old: &str, new: &str) -> Result<(usize, usize, String), Miss> {
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
        let (start, end) = self.locate(old)?;
        Ok((start, end, self.with_endings(new)))
    }

    /// The one place `old` is, line endings aside, else the one place it is loosely.
    fn locate(&self, old: &str) -> Result<(usize, usize), Miss> {
        match self.exact.find(&View::new(old, false).text) {
            Err(Miss::Absent) => {}
            found => return found,
        }
        let loose = self.loose.get_or_init(|| View::new(self.original, true));
        let trimmed = View::new(old, true).text;
        // Whitespace ending `old` may be the indentation of the file's next line, which `new`
        // brings again; Scramjet's trimmed form would leave it in the file too.
        let last = &old[old.rfind('\n').map_or(0, |n| n + 1)..];
        let tail = &last[last.trim_end().len()..];
        if !tail.is_empty() {
            let mut kept = trimmed.clone();
            kept.extend(tail.chars().map(|c| plain(c).map_or(c, |_| ' ')));
            match loose.find(&kept) {
                Err(Miss::Absent) => {}
                found => return found,
            }
        }
        loose.find(&trimmed)
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
        let (start, end, _) = Finder::new(original).find(old, "").ok()?;
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
            Finder::new("“a”\n”a“\n").find("\"a\"", ""),
            Err(Miss::Many(2))
        ));
        assert!(matches!(
            Finder::new("a \n").find("\t", ""),
            Err(Miss::Absent)
        ));
        assert!(matches!(
            Finder::new("ab").find("b\n", ""),
            Err(Miss::Absent)
        ));
    }

    #[test]
    fn what_old_text_copies_of_the_file_beyond_its_text_new_text_does_not_add_again() {
        let replaced = |original: &str, old: &str, new: &str| {
            let (start, end, new) = Finder::new(original).find(old, new).ok()?;
            let mut text = original.to_owned();
            text.replace_range(start..end, &new);
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
    fn new_text_takes_the_first_line_ending_of_the_file() {
        assert_eq!(
            Finder::new("a\r\nb\n").with_endings("x\ny\r\n"),
            "x\r\ny\r\n"
        );
        assert_eq!(Finder::new("a\nb\r\n").with_endings("x\r\ny\n"), "x\ny\n");
        assert_eq!(Finder::new("a").with_endings("x\r\ny"), "x\ny");
    }
}
