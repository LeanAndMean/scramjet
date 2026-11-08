//! Typed-query search for the selectors, as Scramjet's session and model selectors search
//! (`fuzzy.ts`, `session-selector-search.ts`). Scores are in tenths of Scramjet's, lower first.

/// A row's text to search, lowercased once.
pub(super) struct Haystack {
    lower: String,
    chars: Vec<char>,
    /// Lowercased with whitespace runs as one space, for phrases.
    collapsed: String,
    /// Among rows of equal scores, the lower comes first.
    tie: usize,
}

impl Haystack {
    /// Equal scores put the shorter text first, as Scramjet's `fuzzyFilter` does.
    pub(super) fn new(text: &str) -> Self {
        Self::ranked(text, text.chars().count())
    }

    /// Equal scores put the lower `rank` first, as Scramjet's session search puts the newest.
    pub(super) fn ranked(text: &str, rank: usize) -> Self {
        let lower = text.to_lowercase();
        Self {
            collapsed: collapse(&lower),
            chars: lower.chars().collect(),
            lower,
            tie: rank,
        }
    }
}

#[derive(Debug)]
pub(super) enum Token {
    /// Lowercased characters that must appear in order.
    Fuzzy(Vec<char>),
    /// Lowercased with collapsed whitespace, matched as a substring.
    Phrase(String),
    /// A case-insensitive regular expression; one that is empty or invalid matches nothing.
    Regex(Option<regex::Regex>),
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Fuzzy(a), Self::Fuzzy(b)) => a == b,
            (Self::Phrase(a), Self::Phrase(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => {
                a.as_ref().map(regex::Regex::as_str) == b.as_ref().map(regex::Regex::as_str)
            }
            _ => false,
        }
    }
}

/// Whitespace-separated tokens, a `"quoted phrase"` being one; with an unclosed quote every
/// whitespace-separated word is a fuzzy token, quotes included. A query starting `re:` is one
/// regular expression, as in Scramjet's session search.
pub(super) fn parse(query: &str) -> Vec<Token> {
    if let Some(pattern) = query.trim().strip_prefix("re:") {
        let pattern = pattern.trim();
        let built = (!pattern.is_empty()).then(|| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()
        });
        return vec![Token::Regex(built.flatten())];
    }
    let query = query.trim().to_lowercase();
    let fuzzy = |word: &str| Token::Fuzzy(word.chars().collect());
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let flush = |word: &mut String, phrase: bool, tokens: &mut Vec<Token>| {
        let text = word.trim();
        if !text.is_empty() {
            tokens.push(if phrase {
                Token::Phrase(collapse(text))
            } else {
                fuzzy(text)
            });
        }
        word.clear();
    };
    for ch in query.chars() {
        if ch == '"' {
            flush(&mut word, quoted, &mut tokens);
            quoted = !quoted;
        } else if ch.is_whitespace() && !quoted {
            flush(&mut word, false, &mut tokens);
        } else {
            word.push(ch);
        }
    }
    if quoted {
        return query.split_whitespace().map(fuzzy).collect();
    }
    flush(&mut word, false, &mut tokens);
    tokens
}

/// The indices of the rows every token matches, best first; equal scores go by the rows' ranks,
/// then keep the rows' order. Without tokens, every row in order.
pub(super) fn filter(tokens: &[Token], rows: &[Haystack]) -> Vec<usize> {
    if tokens.is_empty() {
        return (0..rows.len()).collect();
    }
    let mut matches: Vec<(i64, usize)> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let mut total = 0;
            for token in tokens {
                total += score(token, row)?;
            }
            Some((total, index))
        })
        .collect();
    matches.sort_by_key(|&(score, index)| (score, rows[index].tie));
    matches.into_iter().map(|(_, index)| index).collect()
}

fn score(token: &Token, row: &Haystack) -> Option<i64> {
    match token {
        Token::Phrase(phrase) => row
            .collapsed
            .find(phrase.as_str())
            .map(|at| row.collapsed[..at].chars().count() as i64),
        Token::Regex(regex) => regex
            .as_ref()?
            .find(&row.lower)
            .map(|found| row.lower[..found.start()].chars().count() as i64),
        Token::Fuzzy(query) => fuzzy(query, &row.chars).or_else(|| {
            let swapped = swapped(query)?;
            fuzzy(&swapped, &row.chars).map(|score| score + 50)
        }),
    }
}

/// Scramjet's `fuzzyMatch`: the query's characters in order, consecutive and word-start matches
/// better, gaps and late matches worse, the whole text best.
fn fuzzy(query: &[char], text: &[char]) -> Option<i64> {
    if query.len() > text.len() {
        return None;
    }
    let mut score = 0;
    let mut matched = 0;
    // As in Scramjet, a match at the start continues the one "before" it.
    let mut last: i64 = -1;
    let mut run = 0;
    for (at, &ch) in text.iter().enumerate() {
        if matched == query.len() {
            break;
        }
        if ch != query[matched] {
            continue;
        }
        let at_i = at as i64;
        if last == at_i - 1 {
            run += 1;
            score -= run * 50;
        } else {
            run = 0;
            if last >= 0 {
                score += (at_i - last - 1) * 20;
            }
        }
        if at == 0 || text[at - 1].is_whitespace() || "-_./:".contains(text[at - 1]) {
            score -= 100;
        }
        score += at_i;
        last = at_i;
        matched += 1;
    }
    if matched < query.len() {
        return None;
    }
    Some(if query == text { score - 1000 } else { score })
}

/// `abc12` as `12abc` and `12abc` as `abc12`, as Scramjet retries such a query.
fn swapped(query: &[char]) -> Option<Vec<char>> {
    let split = query
        .iter()
        .position(|ch| ch.is_ascii_digit() != query[0].is_ascii_digit())?;
    let (head, tail) = query.split_at(split);
    let letters = |part: &[char]| part.iter().all(char::is_ascii_lowercase);
    let digits = |part: &[char]| part.iter().all(char::is_ascii_digit);
    ((letters(head) && digits(tail)) || (digits(head) && letters(tail)))
        .then(|| [tail, head].concat())
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_of(query: &str, text: &str) -> Option<i64> {
        let [token] = &parse(query)[..] else {
            panic!("one token expected in {query:?}");
        };
        score(token, &Haystack::new(text))
    }

    #[test]
    fn fuzzy_scores_are_scramjets() {
        // From Scramjet's `fuzzyMatch` (packages/tui/src/fuzzy.ts), times ten.
        for (query, text, expected) in [
            ("parser", "Fix the parser then run the tests", Some(-787)),
            ("ftp", "Fix the parser", Some(-218)),
            ("luna", "gpt-6-luna", Some(-370)),
            ("GPT", "gpt", Some(-1397)),
            ("6gpt", "gpt-6-luna", Some(-423)),
            ("gpt6", "gpt-6", Some(-473)),
            ("xyz", "gpt-6-luna", None),
            ("ab", "a-b", Some(-228)),
            ("sol", "gpt-6.1-sol", Some(-223)),
            ("longertext", "text", None),
        ] {
            assert_eq!(score_of(query, text), expected, "{query} in {text}");
        }
    }

    #[test]
    fn queries_split_into_words_and_quoted_phrases() {
        let fuzzy = |word: &str| Token::Fuzzy(word.chars().collect());
        assert_eq!(parse("  Foo bar "), [fuzzy("foo"), fuzzy("bar")]);
        assert_eq!(
            parse(r#"foo "Node   CVE" bar"#),
            [fuzzy("foo"), Token::Phrase("node cve".into()), fuzzy("bar")]
        );
        assert_eq!(
            parse(r#"a"b c"d"#),
            [fuzzy("a"), Token::Phrase("b c".into()), fuzzy("d")]
        );
        assert_eq!(
            parse(r#"foo "bar baz"#),
            [fuzzy("foo"), fuzzy("\"bar"), fuzzy("baz")]
        );
        assert_eq!(parse(r#""" x"#), [fuzzy("x")]);
        assert!(parse("   ").is_empty());
    }

    #[test]
    fn rows_matching_every_token_come_best_first() {
        let rows: Vec<_> = [
            "Fix the parser and its tests",
            "Write the haiku",
            "Plan a release: parse args",
            "Run the parser tests again",
        ]
        .iter()
        .map(|text| Haystack::new(text))
        .collect();
        assert_eq!(filter(&parse("parser"), &rows), [3, 0, 2]);
        assert_eq!(filter(&parse(" "), &rows), [0, 1, 2, 3]);
        assert_eq!(filter(&parse("parser again"), &rows), [3]);
        assert_eq!(filter(&parse(r#""the   PARSER""#), &rows), [3, 0]);
        assert_eq!(filter(&parse(r#""parser and""#), &rows), [0]);
        assert!(filter(&parse("zzz"), &rows).is_empty());
        // Equal scores put the shorter text first, as Scramjet's `fuzzyFilter`, or the lower
        // rank, then keep the rows' order.
        let same: Vec<_> = ["same", "same"]
            .iter()
            .map(|text| Haystack::new(text))
            .collect();
        assert_eq!(filter(&parse("same"), &same), [0, 1]);
        let names: Vec<_> = ["rs:review-all", "rs:review", "rs:loop"]
            .iter()
            .map(|text| Haystack::new(text))
            .collect();
        assert_eq!(filter(&parse("rs:"), &names), [2, 1, 0]);
        assert_eq!(filter(&parse("review"), &names), [1, 0]);
        let ranked: Vec<_> = [("fix it", 2), ("fix the parser", 0), ("fix", 1)]
            .iter()
            .map(|&(text, rank)| Haystack::ranked(text, rank))
            .collect();
        assert_eq!(filter(&parse("fix"), &ranked), [2, 1, 0]);
        assert_eq!(filter(&parse(r#""fix""#), &ranked), [1, 2, 0]);
    }

    #[test]
    fn a_query_starting_re_is_a_case_insensitive_regular_expression() {
        let rows: Vec<_> = [
            "Fix the parser and its tests",
            "Write the haiku",
            "Now run all the PARSER tests",
            "release v0.99.3",
        ]
        .iter()
        .map(|text| Haystack::new(text))
        .collect();
        // Earlier matches come first, as Scramjet scores a match by where it starts.
        assert_eq!(filter(&parse("re:Parser\\s+(and|tests)"), &rows), [0, 2]);
        assert_eq!(filter(&parse("  re: ^write  "), &rows), [1]);
        assert_eq!(filter(&parse(r"re:v\d+\.\d+"), &rows), [3]);
        // Empty or invalid, it matches nothing; `RE:` is not the prefix.
        assert!(filter(&parse("re:"), &rows).is_empty());
        assert!(filter(&parse("re:(parser"), &rows).is_empty());
        assert_eq!(parse("RE:x"), [Token::Fuzzy("re:x".chars().collect())]);
    }
}
