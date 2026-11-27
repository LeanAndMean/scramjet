//! What a selection in a reply copies of the reply's markdown. The text a copy of the reply
//! holds as shown is aligned with its source, character by character, and the source between
//! the selection's ends is taken, the markers there included; an element whose shown text the
//! selection holds whole (a bold span, a code span, an item, a code block, a table or a cell)
//! comes with the markers around it.

use pulldown_cmark::{Event, Options, Parser};
use std::ops::Range;

/// The edits past which an alignment gives up, bounding its time and memory (about the square
/// of it); a copy then holds the text as shown.
const EDITS: usize = 2048;

/// The part of `source` that `shown[from..to]` shows, where `shown` is the whole reply as a copy
/// holds it; `None` when they are too far apart to align or nothing selected is the source's.
pub(super) fn copied(source: &str, shown: &str, from: usize, to: usize) -> Option<String> {
    let shown: Vec<(usize, char)> = (shown.char_indices()).filter(|&(_, c)| !drawn(c)).collect();
    let text: Vec<(usize, char)> = source.char_indices().collect();
    let chars = |of: &[(usize, char)]| of.iter().map(|&(_, c)| c).collect::<Vec<_>>();
    let pairs = align(&chars(&shown), &chars(&text), EDITS)?;
    let first = pairs.iter().position(|&(at, _)| shown[at].0 >= from)?;
    let last = pairs.iter().rposition(|&(at, _)| shown[at].0 < to)?;
    if first > last {
        return None;
    }
    let start = text[pairs[first].1].0;
    let (end, c) = text[pairs[last].1];
    let matched: Vec<usize> = (pairs.iter().map(|&(_, at)| text[at]))
        .filter(|(_, c)| !c.is_whitespace())
        .map(|(at, _)| at)
        .collect();
    let selected = start..end + c.len_utf8();
    let span = widened(source, &matched, selected.clone());
    // What the widening took in around the selection keeps no spaces at its ends.
    let before = source[span.start..selected.start].trim_start();
    let after = source[selected.end..span.end].trim_end();
    Some([before, &source[selected], after].concat())
}

/// Box drawing, which tables and frames are drawn with and no source holds.
fn drawn(c: char) -> bool {
    ('\u{2500}'..='\u{257f}').contains(&c)
}

/// `span` taking in each element of `source` whose shown characters but spaces (`matched`, by
/// their offsets in order) it holds all of, until no other is held whole.
fn widened(source: &str, matched: &[usize], mut span: Range<usize>) -> Range<usize> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let elements: Vec<Range<usize>> = (Parser::new_ext(source, options).into_offset_iter())
        .filter(|(event, _)| matches!(event, Event::Start(_) | Event::Code(_)))
        .map(|(_, range)| range)
        .collect();
    loop {
        let before = span.clone();
        for element in &elements {
            let inside = matched.partition_point(|&at| at < element.start)
                ..matched.partition_point(|&at| at < element.end);
            if inside.is_empty() {
                continue;
            }
            let (first, last) = (matched[inside.start], matched[inside.end - 1]);
            if span.start <= first && last < span.end {
                span = span.start.min(element.start)..span.end.max(element.end);
            }
        }
        if span == before {
            return span;
        }
    }
}

/// The positions of `a` and `b` that a shortest edit script between them keeps, in order;
/// `None` past `limit` edits.
fn align(a: &[char], b: &[char], limit: usize) -> Option<Vec<(usize, usize)>> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = (a[prefix..].iter().rev())
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_end, b_end) = (a.len() - suffix, b.len() - suffix);
    let middle = myers(&a[prefix..a_end], &b[prefix..b_end], limit)?;
    let mut pairs: Vec<(usize, usize)> = (0..prefix).map(|at| (at, at)).collect();
    pairs.extend(middle.into_iter().map(|(x, y)| (x + prefix, y + prefix)));
    pairs.extend((0..suffix).map(|at| (a_end + at, b_end + at)));
    Some(pairs)
}

/// Myers' O(ND) difference: the furthest reach of each diagonal, kept for each count of edits
/// to trace the path back.
fn myers(a: &[char], b: &[char], limit: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let snake = |mut x: isize, k: isize| {
        let mut y = x - k;
        while x < n && y < m && a[x as usize] == b[y as usize] {
            (x, y) = (x + 1, y + 1);
        }
        x
    };
    let offset = limit as isize + 1;
    let mut reach = vec![0isize; 2 * limit + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=limit as isize {
        let mut done = false;
        for k in (-d..=d).step_by(2) {
            let at = |k: isize| reach[(offset + k) as usize];
            let down = k == -d || (k != d && at(k - 1) < at(k + 1));
            let x = match down {
                true => at(k + 1),
                false => at(k - 1) + 1,
            };
            let x = snake(x, k);
            reach[(offset + k) as usize] = x;
            if x >= n && x - k >= m {
                done = true;
                break;
            }
        }
        trace.push(reach[(offset - d) as usize..=(offset + d) as usize].to_vec());
        if done {
            return Some(back(&trace, a, b));
        }
    }
    None
}

/// The pairs kept along the path `trace` ends at.
fn back(trace: &[Vec<isize>], a: &[char], b: &[char]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let (mut x, mut y) = (a.len() as isize, b.len() as isize);
    for d in (1..trace.len()).rev() {
        let d = d as isize;
        let at = |k: isize| trace[d as usize - 1][(k + d - 1) as usize];
        let k = x - y;
        let down = k == -d || (k != d && at(k - 1) < at(k + 1));
        let previous = if down { k + 1 } else { k - 1 };
        let (from_x, from_y) = (at(previous), at(previous) - previous);
        let (start_x, start_y) = match down {
            true => (from_x, from_y + 1),
            false => (from_x + 1, from_y),
        };
        while x > start_x && y > start_y {
            (x, y) = (x - 1, y - 1);
            pairs.push((x as usize, y as usize));
        }
        (x, y) = (from_x, from_y);
    }
    while x > 0 && y > 0 {
        (x, y) = (x - 1, y - 1);
        pairs.push((x as usize, y as usize));
    }
    pairs.reverse();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn an_alignment_keeps_what_both_share_in_order() {
        let kept = |a: &str, b: &str| {
            let (a, b) = (chars(a), chars(b));
            let pairs = align(&a, &b, 64).unwrap();
            assert!(pairs.iter().all(|&(x, y)| a[x] == b[y]));
            assert!(pairs.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
            pairs.iter().map(|&(x, _)| a[x]).collect::<String>()
        };
        assert_eq!(kept("use bold here", "use **bold** here"), "use bold here");
        assert_eq!(kept("• one", "- one"), " one");
        assert_eq!(kept("abcabba", "cbabac").len(), 4);
        assert_eq!(kept("", "x"), "");
        assert_eq!(kept("same", "same"), "same");
        assert_eq!(align(&chars("aaaa"), &chars("bbbb"), 3), None);
    }

    /// What selecting `selected` (its first occurrence in `shown`) copies of `source`.
    fn copy(source: &str, shown: &str, selected: &str) -> Option<String> {
        let from = shown.find(selected).unwrap();
        copied(source, shown, from, from + selected.len())
    }

    #[test]
    fn a_selection_copies_the_markdown_it_shows() {
        let source = "Use **bold `x` text** and `a` [the docs](https://d/e \"t\") here.";
        let shown = "Use bold x text and a the docs (https://d/e) here.";
        // The markers within, and those around what is held whole.
        assert_eq!(
            copy(source, shown, "Use bold x text and").unwrap(),
            "Use **bold `x` text** and"
        );
        assert_eq!(copy(source, shown, "x").unwrap(), "`x`");
        assert_eq!(copy(source, shown, "bold x").unwrap(), "bold `x`");
        assert_eq!(copy(source, shown, "ol").unwrap(), "ol");
        assert_eq!(
            copy(source, shown, "the docs (https://d/e)").unwrap(),
            "[the docs](https://d/e \"t\")"
        );
        assert_eq!(copy(source, shown, "the docs").unwrap(), "the docs");
    }

    #[test]
    fn a_block_selected_whole_comes_with_its_markers() {
        let source = "## Plan\n\n- **one**\n- two\n\n> quoted\n\n```rust\nlet x = 1;\n```\n\nafter";
        let shown = "## Plan\n\n- one\n- two\n\n│ quoted\n\n```rust\n  let x = 1;\n```\n\nafter";
        assert_eq!(copy(source, shown, "one").unwrap(), "**one**");
        assert_eq!(copy(source, shown, "- one").unwrap(), "- **one**");
        // The bullet shows, so a selection after it leaves it out.
        assert_eq!(copy(source, shown, "one\n- two").unwrap(), "**one**\n- two");
        assert_eq!(
            copy(source, shown, "- one\n- two").unwrap(),
            "- **one**\n- two"
        );
        assert_eq!(copy(source, shown, "quoted").unwrap(), "> quoted");
        assert_eq!(copy(source, shown, "let x").unwrap(), "let x");
        assert_eq!(copy(source, shown, "let x = 1;").unwrap(), "let x = 1;");
        assert_eq!(
            copy(source, shown, "```rust\n  let x = 1;\n```").unwrap(),
            "```rust\nlet x = 1;\n```"
        );
        assert_eq!(copy(source, shown, "Plan").unwrap(), "Plan");
        assert_eq!(copy(source, shown, "## Plan").unwrap(), "## Plan");
    }

    #[test]
    fn a_table_copies_its_pipes_and_a_cell_its_text() {
        let source = "| Name | Size |\n|------|------|\n| a.rs | 12 |\n| b.rs | 7 |\n\nend";
        let shown = "┌──────┬──────┐\n│ Name │ Size │\n├──────┼──────┤\n│ a.rs │ 12   │\n\
                     │ b.rs │ 7    │\n└──────┴──────┘\n\nend";
        let from = 0;
        let to = shown.find("\n\nend").unwrap();
        assert_eq!(
            copied(source, shown, from, to).unwrap(),
            "| Name | Size |\n|------|------|\n| a.rs | 12 |\n| b.rs | 7 |"
        );
        assert_eq!(copy(source, shown, "a.rs").unwrap(), "a.rs");
        assert_eq!(copy(source, shown, "a.rs │ 1").unwrap(), "a.rs | 1");
        assert_eq!(copy(source, shown, "a.rs │ 12").unwrap(), "| a.rs | 12 |");
    }

    #[test]
    fn nothing_of_the_source_selected_copies_nothing_of_it() {
        assert_eq!(copied("text", "┌──┐ text", 0, 3), None);
        assert_eq!(copied("ab", "ab", 1, 1), None);
    }
}
