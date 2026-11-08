//! Spelling in the editor, as Scramjet's: words not in the built-in American English dictionary
//! (SCOWL's en_US, from `dictionary-en` 4.0.0; its terms are in `spelling/LICENSE`), leaving
//! code-like tokens alone. Beyond Scramjet's, common developer words (`spelling/developer.dic`)
//! and the user's own (`words.txt` beside the settings) are known too, and `spellCheck: false`
//! turns it off.

use spellbook::Dictionary;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

/// The user's words file, beside the settings file.
pub(super) const WORDS: &str = "words.txt";

static DICTIONARY: RwLock<Option<Dictionary>> = RwLock::new(None);
static ENABLED: AtomicBool = AtomicBool::new(true);
/// The user's words the dictionary is (being) built with, and that build's number.
static BUILT: Mutex<Option<(Vec<String>, u64)>> = Mutex::new(None);
/// Each line checked last, so that a frame checks only lines that changed.
static CHECKED: Mutex<Option<HashMap<String, Vec<Range<usize>>>>> = Mutex::new(None);

/// Turns spelling on or off, and with it on, builds the dictionary with the words in `words`
/// in the background when they changed (8 ms in a release build), so that no frame waits for
/// it; until the first is in, nothing is marked.
pub(super) fn configure(enabled: bool, words: Option<&Path>) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        return;
    }
    let words = words.map(read_words).unwrap_or_default();
    let mut built = BUILT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let generation = match &*built {
        Some((same, _)) if *same == words => return,
        Some((_, generation)) => generation + 1,
        None => 0,
    };
    *built = Some((words.clone(), generation));
    std::thread::spawn(move || {
        let dictionary = build(&words);
        let built = BUILT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // A later build was asked for meanwhile; it replaces this one.
        if built
            .as_ref()
            .is_some_and(|(_, latest)| *latest == generation)
        {
            *DICTIONARY
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = dictionary;
            *CHECKED
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
    });
}

/// The entries of a words file: one word a line, in Hunspell's syntax (`word/flags` takes the
/// dictionary's affixes); none when there is no file.
fn read_words(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    (text.lines().map(str::trim))
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// The dictionary with the developer words and `words`; a word that does not parse is left out.
fn build(words: &[String]) -> Option<Dictionary> {
    let mut dictionary = Dictionary::new(
        include_str!("spelling/en_US.aff"),
        include_str!("spelling/en_US.dic"),
    )
    .ok()?;
    for word in include_str!("spelling/developer.dic").lines() {
        let _ = dictionary.add(word);
    }
    for word in words {
        // en_US's `M` takes `'s`, which a word of one's own is expected to take too.
        let _ = match word.contains('/') {
            true => dictionary.add(word),
            false => dictionary.add(&format!("{word}/M")),
        };
    }
    Some(dictionary)
}

/// The byte ranges of the misspelled words in `lines`, once the dictionary is in.
pub(super) fn misspelled(lines: &str) -> Vec<Range<usize>> {
    if !ENABLED.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let dictionary = DICTIONARY
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(dictionary) = dictionary.as_ref() else {
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
    use std::sync::OnceLock;

    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn marked(line: &str) -> Vec<&str> {
        static PLAIN: OnceLock<Dictionary> = OnceLock::new();
        let dictionary = PLAIN.get_or_init(|| build(&[]).unwrap());
        check(dictionary, line)
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
    fn common_developer_words_are_known() {
        assert_eq!(
            marked("The repo's configs: async stdin, tmux and Clippy, refactored subagents ok"),
            Vec::<&str>::new()
        );
        assert_eq!(marked("repoz confg"), ["repoz", "confg"]);
    }

    /// Spelling's state is the process's, so one test goes through it all.
    #[test]
    fn the_users_words_are_known_and_spelling_can_be_turned_off() {
        let ready = |text: &str, want: &[&str]| {
            for _ in 0..500 {
                let ranges = misspelled(text);
                let words: Vec<_> = ranges.iter().map(|r| &text[r.clone()]).collect();
                if words == want {
                    return ranges;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("{text:?} never marked {want:?}");
        };
        let directory = std::env::temp_dir().join(format!("rs-words-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let _scratch = Scratch(directory.clone());
        let file = directory.join(WORDS);
        configure(true, Some(&file));
        let text = "héllo wrld\nfine\nwrld Aidd's";
        let ranges = ready(text, &["héllo", "wrld", "wrld", "Aidd's"]);
        assert_eq!(ranges[2], 17..21);
        std::fs::write(&file, "# mine\nwrld\n\n aidd \n").unwrap();
        configure(true, Some(&file));
        ready(text, &["héllo"]);
        configure(false, Some(&file));
        assert!(misspelled(text).is_empty());
        configure(true, Some(&file));
        ready(text, &["héllo"]);
    }
}
