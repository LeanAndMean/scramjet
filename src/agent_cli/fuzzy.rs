//! Typed-query search for the selectors, as Scramjet's session and model selectors search
//! (`fuzzy.ts`, `session-selector-search.ts`). Scores are in tenths of Scramjet's, lower first.

/// A row's text to search, lowercased once.
pub(super) struct Haystack {
    chars: Vec<char>,
    /// Lowercased with whitespace runs as one space, for phrases.
    collapsed: String,
}

impl Haystack {
    pub(super) fn new(text: &str) -> Self {
        let lower = text.to_lowercase();
        Self {
            collapsed: collapse(&lower),
            chars: lower.chars().collect(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub(super) enum Token {
    /// Lowercased characters that must appear in order.
    Fuzzy(Vec<char>),
    /// Lowercased with collapsed whitespace, matched as a substring.
    Phrase(String),
}

/// Whitespace-separated tokens, a `"quoted phrase"` being one; with an unclosed quote every
/// whitespace-separated word is a fuzzy token, quotes included.
pub(super) fn parse(query: &str) -> Vec<Token> {
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

/// The indices of the rows every token matches, best first; equal scores keep the rows' order.
pub(super) fn filter(tokens: &[Token], rows: &[Haystack]) -> Vec<usize> {
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
    matches.sort_by_key(|&(score, _)| score);
    matches.into_iter().map(|(_, index)| index).collect()
}

fn score(token: &Token, row: &Haystack) -> Option<i64> {
    match token {
        Token::Phrase(phrase) => row
            .collapsed
            .find(phrase.as_str())
            .map(|at| row.collapsed[..at].chars().count() as i64),
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
        assert_eq!(filter(&parse("parser"), &rows), [0, 3, 2]);
        assert_eq!(filter(&parse("parser again"), &rows), [3]);
        assert_eq!(filter(&parse(r#""the   PARSER""#), &rows), [0, 3]);
        assert_eq!(filter(&parse(r#""parser and""#), &rows), [0]);
        assert!(filter(&parse("zzz"), &rows).is_empty());
        // Equal scores keep the rows' order.
        let same: Vec<_> = ["same", "same"]
            .iter()
            .map(|text| Haystack::new(text))
            .collect();
        assert_eq!(filter(&parse("same"), &same), [0, 1]);
    }
}
