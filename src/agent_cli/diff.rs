//! Shows an edit's diff as Scramjet renders it: changed lines in full, wrapped under their text,
//! with the changed words of a line replaced by one other line in inverse. Unchanged lines are
//! only context, so they are cut to the width.

use super::{Wrap, clip, printable};
use scramjet_rs::agent::sequence_diff;
use unicode_width::UnicodeWidthStr;

/// Screen rows a diff may take, besides the note about the lines left out.
const DIFF_ROWS: usize = 40;

const REMOVED: &str = "\x1b[31m";
const ADDED: &str = "\x1b[32m";
const CONTEXT: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const INVERSE: &str = "\x1b[7m";
const INVERSE_OFF: &str = "\x1b[27m";

/// The rows showing `diff`, whose lines are `{mark}{number} {text}` as the edit tool writes them,
/// in `width` columns, styled when `color` is set.
pub(super) fn rows(diff: &str, width: usize, color: bool) -> Vec<String> {
    let lines: Vec<&str> = diff.lines().collect();
    let mark = |i: usize| lines.get(i).and_then(|line| line.chars().next());
    let mut rows = Vec::new();
    let mut paired = None;
    for (i, line) in lines.iter().enumerate() {
        let line_rows = match mark(i) {
            Some(changed @ ('-' | '+')) => {
                let (gutter, text) = split(line);
                let lone_pair = changed == '-'
                    && (i == 0 || mark(i - 1) != Some('-'))
                    && mark(i + 1) == Some('+')
                    && mark(i + 2) != Some('+');
                let text = match paired.take() {
                    Some(text) => text,
                    None if color && lone_pair => {
                        let (old, new) =
                            highlighted(&printable(text), &printable(split(lines[i + 1]).1));
                        paired = Some(new);
                        old
                    }
                    None => printable(text),
                };
                let style = if changed == '-' { REMOVED } else { ADDED };
                wrapped(gutter, &text, width, style, color)
            }
            _ => vec![styled(CONTEXT, &clip(&printable(line), width), color)],
        };
        if !rows.is_empty() && rows.len() + line_rows.len() > DIFF_ROWS {
            let note = format!("… ({} more lines, {} total)", lines.len() - i, lines.len());
            rows.push(styled(CONTEXT, &note, color));
            break;
        }
        rows.extend(line_rows);
    }
    rows
}

/// A changed line's gutter (its mark, number and the space after them) and its text.
fn split(line: &str) -> (&str, &str) {
    let number = line[1..].trim_start_matches(' ');
    let text = number.trim_start_matches(|c: char| c.is_ascii_digit());
    line.split_at((line.len() - text.len() + 1).min(line.len()))
}

/// A changed line's rows: the gutter, then its text wrapped under itself. A line longer than
/// all the rows a diff may take ends with `…` in place of its last row.
fn wrapped(gutter: &str, text: &str, width: usize, style: &str, color: bool) -> Vec<String> {
    let blank = " ".repeat(gutter.width());
    let mut wrap = Wrap::new(width.saturating_sub(blank.len()).max(10));
    let text = wrap.push(text) + &wrap.finish();
    let lines: Vec<&str> = text.split('\n').collect();
    let cut = lines.len() > DIFF_ROWS;
    let mut rows = Vec::new();
    let mut inverse = false;
    for (k, line) in lines.iter().enumerate().take(match cut {
        true => DIFF_ROWS - 1,
        false => DIFF_ROWS,
    }) {
        // A highlight that a break interrupts continues after the next row's indentation.
        let (indent, rest) = line.split_at(line.len() - line.trim_start_matches(' ').len());
        let reopen = if inverse { INVERSE } else { "" };
        let gutter = if k == 0 { gutter } else { &blank };
        rows.push(styled(
            style,
            &format!("{gutter}{indent}{reopen}{rest}"),
            color,
        ));
        inverse = match (line.rfind(INVERSE), line.rfind(INVERSE_OFF)) {
            (Some(on), off) => off.is_none_or(|off| off < on),
            (None, off) => inverse && off.is_none(),
        };
    }
    if cut {
        rows.push(styled(style, &format!("{blank}…"), color));
    }
    rows
}

/// The two lines of a one-line change with the words that differ in inverse. Spaces between
/// changed words belong to their highlight; indentation never does.
fn highlighted(old: &str, new: &str) -> (String, String) {
    let mut ops = Vec::new();
    sequence_diff(&words(old), &words(new), &mut ops);
    let side = |changed: char| {
        let mut words: Vec<(&str, bool)> = ops
            .iter()
            .filter(|(mark, _)| *mark == ' ' || *mark == changed)
            .map(|(mark, word)| (*word, *mark == changed))
            .collect();
        for k in 1..words.len().saturating_sub(1) {
            if words[k].0.trim().is_empty() && words[k - 1].1 && words[k + 1].1 {
                words[k].1 = true;
            }
        }
        if let Some(first) = words.first_mut()
            && first.0.trim().is_empty()
        {
            first.1 = false;
        }
        let mut out = String::new();
        let mut on = false;
        for (word, changed) in words {
            if changed != on {
                out.push_str(if changed { INVERSE } else { INVERSE_OFF });
                on = changed;
            }
            out.push_str(word);
        }
        if on {
            out.push_str(INVERSE_OFF);
        }
        out
    };
    (side('-'), side('+'))
}

/// Runs of letters, digits and underscores, runs of whitespace, and single other characters.
fn words(text: &str) -> Vec<&str> {
    let kind = |c: char| match c {
        c if c.is_alphanumeric() || c == '_' => 0,
        c if c.is_whitespace() => 1,
        _ => 2,
    };
    let mut words = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let mut end = start + c.len_utf8();
        if kind(c) != 2 {
            while let Some(&(next, d)) = chars.peek()
                && kind(d) == kind(c)
            {
                end = next + d.len_utf8();
                chars.next();
            }
        }
        words.push(&text[start..end]);
    }
    words
}

fn styled(style: &str, text: &str, color: bool) -> String {
    match color {
        true => format!("{style}{text}{RESET}"),
        false => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(diff: &str, width: usize) -> Vec<String> {
        rows(diff, width, false)
    }

    #[test]
    fn changed_lines_wrap_under_their_text_and_context_is_cut() {
        let diff = " 1 unchanged context that is long\n\
                    -2 - **Sessions:** saved as lines\n\
                    +2 - **Sessions:** saved as JSON lines\n";
        assert_eq!(
            plain(diff, 24),
            [
                " 1 unchanged context th…",
                "-2 - **Sessions:** saved",
                "     as lines",
                "+2 - **Sessions:** saved",
                "     as JSON lines",
            ]
        );
        assert_eq!(
            plain("+12 \n+13     indented code\n", 18),
            ["+12 ", "+13     indented", "        code"]
        );
    }

    #[test]
    fn rows_beyond_the_limit_are_counted_and_a_huge_line_is_cut() {
        let diff: String = (1..=50).map(|n| format!("+{n:>2} line {n}\n")).collect();
        let shown = plain(&diff, 80);
        assert_eq!(shown.len(), DIFF_ROWS + 1);
        assert_eq!(shown[DIFF_ROWS - 1], "+40 line 40");
        assert_eq!(shown[DIFF_ROWS], "… (10 more lines, 50 total)");

        let huge = format!("-1 {}\n+1 x\n", "word ".repeat(400));
        let shown = plain(&huge, 23);
        assert_eq!(shown.len(), DIFF_ROWS + 1);
        assert_eq!(shown[DIFF_ROWS - 1], "   …");
        assert_eq!(shown[DIFF_ROWS], "… (1 more lines, 2 total)");
    }

    #[test]
    fn a_one_line_change_highlights_the_words_that_differ() {
        let (old, new) = highlighted("    let total = a + b;", "    let sum = a + b + c;");
        assert_eq!(old, "    let \x1b[7mtotal\x1b[27m = a + b;");
        assert_eq!(
            new,
            "    let \x1b[7msum\x1b[27m = a + b\x1b[7m + c\x1b[27m;"
        );
        let (old, new) = highlighted("  one two three", "    four five three");
        assert_eq!(old, "  \x1b[7mone two\x1b[27m three");
        assert_eq!(new, "    \x1b[7mfour five\x1b[27m three");

        // A highlight broken across rows continues after the next row's indentation.
        let colored = rows("-1 aaaa bbbb cccc dd\n+1 aaaa xxxx yyyy dd\n", 16, true);
        assert_eq!(
            colored,
            [
                "\x1b[31m-1 aaaa \x1b[7mbbbb\x1b[0m",
                "\x1b[31m   \x1b[7mcccc\x1b[27m dd\x1b[0m",
                "\x1b[32m+1 aaaa \x1b[7mxxxx\x1b[0m",
                "\x1b[32m   \x1b[7myyyy\x1b[27m dd\x1b[0m",
            ]
        );
        // Lines of a larger change are shown without highlights.
        let block = rows("-1 a\n-2 b\n+1 c\n", 20, true);
        assert_eq!(
            block,
            [
                "\x1b[31m-1 a\x1b[0m",
                "\x1b[31m-2 b\x1b[0m",
                "\x1b[32m+1 c\x1b[0m"
            ]
        );
    }
}
