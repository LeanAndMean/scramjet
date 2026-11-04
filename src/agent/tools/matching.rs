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

    /// The range of the original that `old` stands for: the one place it is, line endings
    /// aside, else the one place it is loosely.
    pub fn find(&self, old: &str) -> Result<(usize, usize), Miss> {
        match self.exact.find(&View::new(old, false).text) {
            Err(Miss::Absent) => {}
            found => return found,
        }
        let loose = self.loose.get_or_init(|| View::new(self.original, true));
        loose.find(&View::new(old, true).text)
    }

    /// `new` with the line ending of the file's first line, as Scramjet writes it.
    pub fn with_endings(&self, new: &str) -> String {
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
        let (start, end) = Finder::new(original).find(old).ok()?;
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
            Finder::new("“a”\n”a“\n").find("\"a\""),
            Err(Miss::Many(2))
        ));
        assert!(matches!(Finder::new("a \n").find("\t"), Err(Miss::Absent)));
        assert!(matches!(Finder::new("ab").find("b\n"), Err(Miss::Absent)));
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
