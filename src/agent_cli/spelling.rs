//! Spelling in the editor, as Scramjet's: words not in the built-in American English dictionary
//! (SCOWL's en_US, from `dictionary-en` 4.0.0; its terms are in `spelling/LICENSE`), leaving
//! code-like tokens alone.

use spellbook::Dictionary;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Mutex, OnceLock};

static DICTIONARY: OnceLock<Option<Dictionary>> = OnceLock::new();
/// Each line checked last, so that a frame checks only lines that changed.
static CHECKED: Mutex<Option<HashMap<String, Vec<Range<usize>>>>> = Mutex::new(None);

/// Loads the dictionary in the background (8 ms in a release build), so that the first frame never
/// waits for it; until it is in, nothing is marked.
pub(super) fn load() {
    if DICTIONARY.get().is_none() {
        std::thread::spawn(dictionary);
    }
}

fn dictionary() -> Option<&'static Dictionary> {
    DICTIONARY
        .get_or_init(|| {
            Dictionary::new(
                include_str!("spelling/en_US.aff"),
                include_str!("spelling/en_US.dic"),
            )
            .ok()
        })
        .as_ref()
}

/// The byte ranges of the misspelled words in `lines`, once the dictionary is in.
pub(super) fn misspelled(lines: &str) -> Vec<Range<usize>> {
    let Some(Some(dictionary)) = DICTIONARY.get() else {
        return Vec::new();
    };
    let mut checked = CHECKED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut last = checked.take().unwrap_or_default();
    let mut now = HashMap::new();
    let mut ranges = Vec::new();
    let mut offset = 0;
    for line in lines.split('\n') {
        let found = last
            .remove(line)
            .or_else(|| now.get(line).cloned())
            .unwrap_or_else(|| check(dictionary, line));
        ranges.extend(found.iter().map(|r| r.start + offset..r.end + offset));
        now.insert(line.to_owned(), found);
        offset += line.len() + 1;
    }
    *checked = Some(now);
    ranges
}

fn check(dictionary: &Dictionary, line: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for (start, token) in line
        .split_whitespace()
        .map(|token| (token.as_ptr() as usize - line.as_ptr() as usize, token))
    {
        let stripped = token.trim_end_matches(['.', ',', '!', '?', ';', ':', ')', ']']);
        if stripped.contains(['_', '/', '\\', '.', '@', '#', ':']) {
            continue;
        }
        for word in words(token) {
            let text = &token[word.clone()];
            if !code_like_casing(text) && !abbreviation(text) && !dictionary.check(text) {
                ranges.push(word.start + start..word.end + start);
            }
        }
    }
    ranges
}

/// The runs of letters and apostrophes in `token`, without apostrophes at their ends, of two
/// letters or more; a run touching a digit (`sha256`, `v2`) is left out.
fn words(token: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut runs = Vec::new();
    let mut start = None;
    for (index, c) in token.char_indices().chain([(token.len(), ' ')]) {
        match (start, c.is_alphanumeric() || c == '\'') {
            (None, true) => start = Some(index),
            (Some(from), false) => {
                runs.push(from..index);
                start = None;
            }
            _ => {}
        }
    }
    runs.into_iter().filter_map(move |run| {
        let text = &token[run.clone()];
        let trimmed = text.trim_start_matches('\'');
        let from = run.start + text.len() - trimmed.len();
        let trimmed = trimmed.trim_end_matches('\'');
        let all_letters = trimmed.chars().all(|c| c.is_alphabetic() || c == '\'');
        (all_letters && trimmed.chars().count() > 1).then(|| from..from + trimmed.len())
    })
}

/// A short word in capitals (`LLM`, `JSON`, `TODO`), which the dictionary mostly lacks.
fn abbreviation(word: &str) -> bool {
    word.chars().count() <= 5 && word.chars().all(|c| c.is_uppercase() || c == '\'')
}

/// Upper case inside a word other than all of it or its first letter (`camelCase`, `iOS`).
fn code_like_casing(word: &str) -> bool {
    let mut chars = word.chars();
    let first = chars.next().unwrap_or_default();
    let rest: String = chars.collect();
    let upper = word.to_uppercase() == word;
    let lower = word.to_lowercase() == word;
    let title = first.is_uppercase() && rest.to_lowercase() == rest;
    !(upper || lower || title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(line: &str) -> Vec<&str> {
        check(dictionary().unwrap(), line)
            .into_iter()
            .map(|range| &line[range])
            .collect()
    }

    #[test]
    fn misspelled_words_are_found_and_code_is_left_alone() {
        assert_eq!(
            marked("Teh quick brown fox's jumpd over the lazy dog."),
            ["Teh", "jumpd"]
        );
        assert_eq!(
            marked("see src/agnt.rs, foo_barr, a@b.c, #hsh and ::wat"),
            Vec::<&str>::new()
        );
        assert_eq!(
            marked("camelCaes iOS sha256 v2 'quoted' LLM JSON PRs HELLO WROLDS Wrold"),
            ["WROLDS", "Wrold"]
        );
        assert_eq!(marked("don't wouldn't o'clock"), Vec::<&str>::new());
        assert_eq!(marked("(misspeled) [braket]"), ["misspeled", "braket"]);
    }

    #[test]
    fn ranges_are_bytes_of_the_whole_text_across_lines() {
        load();
        dictionary();
        let text = "héllo wrld\nfine\nwrld";
        let ranges = misspelled(text);
        let words: Vec<_> = ranges.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(words, ["héllo", "wrld", "wrld"]);
        assert_eq!(ranges[2], 17..21);
    }
}
