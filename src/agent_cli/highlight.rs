//! Code colored by its language, as Scramjet's `highlightCode` colors fenced code blocks that name
//! one and the files `read` and `write` show. Its highlight.js classes become TextMate scopes here,
//! parsed by syntect with bat's syntax definitions (two-face). The full screen renders a running
//! part again at each change, so lines are kept once highlighted, each under the lines before it.

use super::style::{
    DIFF_ADDED, DIFF_REMOVED, RESET, SYNTAX_COMMENT, SYNTAX_FUNCTION, SYNTAX_KEYWORD,
    SYNTAX_NUMBER, SYNTAX_STRING, SYNTAX_TYPE, SYNTAX_VARIABLE,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;
use syntect::highlighting::{
    Color, HighlightIterator, HighlightState, Highlighter as Styler, StyleModifier, Theme,
    ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

/// Scope selectors and the style of what they select, as Scramjet's theme colors highlight.js's
/// classes; the more specific selector wins. Operators and punctuation stay plain, as
/// highlight.js rarely marks them.
const RULES: &[(&str, &str)] = &[
    ("comment", SYNTAX_COMMENT),
    (
        "meta.annotation, meta.preprocessor, meta.diff.range, meta.diff.header, \
punctuation.definition.tag",
        SYNTAX_COMMENT,
    ),
    ("string, constant.character", SYNTAX_STRING),
    ("constant.numeric, constant.language", SYNTAX_NUMBER),
    (
        "keyword, storage, variable.language, entity.name.tag",
        SYNTAX_KEYWORD,
    ),
    ("keyword.operator", ""),
    (
        "keyword.operator.word, keyword.operator.new, keyword.operator.expression",
        SYNTAX_KEYWORD,
    ),
    (
        "entity.name.function, variable.function, support.function",
        SYNTAX_FUNCTION,
    ),
    (
        "support.function.builtin, support.macro, support.type, support.class, entity.name.type, \
entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, \
entity.name.interface, entity.name.union, entity.other.inherited-class",
        SYNTAX_TYPE,
    ),
    (
        "variable.parameter, entity.other.attribute-name, support.type.property-name, \
meta.mapping.key string, entity.name.tag.yaml, entity.name.tag.toml, \
variable.other.readwrite.shell, punctuation.definition.variable.shell, \
punctuation.section.expansion.parameter",
        SYNTAX_VARIABLE,
    ),
    ("variable.parameter.option", ""),
    ("markup.inserted", DIFF_ADDED),
    ("markup.deleted", DIFF_REMOVED),
];

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// The rules as a theme whose colors carry the index of a rule in `RULES`, from one; zero is
/// plain.
static THEME: LazyLock<Theme> = LazyLock::new(|| {
    let color = |index: usize| Color {
        r: index as u8,
        g: 0,
        b: 0,
        a: 0xff,
    };
    let scopes = (RULES.iter().enumerate())
        .map(|(index, (selectors, _))| ThemeItem {
            scope: selectors.parse().expect("the rules' selectors parse"),
            style: StyleModifier {
                foreground: Some(color(index + 1)),
                background: None,
                font_style: None,
            },
        })
        .collect();
    Theme {
        settings: ThemeSettings {
            foreground: Some(color(0)),
            ..ThemeSettings::default()
        },
        scopes,
        ..Theme::default()
    }
});

static STYLER: LazyLock<Styler<'static>> = LazyLock::new(|| Styler::new(&THEME));

thread_local! {
    /// Highlighted lines and the states after them, under the key of their lines so far; per
    /// thread, as the states hold Oniguruma's regions.
    static LINES: RefCell<HashMap<u64, (String, States)>> = RefCell::default();
}

/// The lines kept before they are all dropped, about 2 KB each.
const KEPT_LINES: usize = 10_000;

type States = Rc<(ParseState, HighlightState)>;

#[derive(Clone, Copy)]
pub(super) struct Language(&'static SyntaxReference);

/// The language a code block's info string names, by its first word, as a name or alias
/// highlight.js knows.
pub(super) fn named(info: &str) -> Option<Language> {
    let word = info
        .split([' ', '\t', ',', '{'])
        .next()?
        .to_ascii_lowercase();
    let token = match word.as_str() {
        "" => return None,
        "shell" | "console" | "shellsession" => "bash",
        "golang" => "go",
        "csharp" => "cs",
        "jsonc" | "json5" => "json",
        "python3" | "py3" => "python",
        "docker" => "dockerfile",
        "text" | "plaintext" => "txt",
        word => word,
    };
    SYNTAXES.find_syntax_by_token(token).map(Language)
}

/// The language of a file, by its name (`Makefile`) or its extension, if it is code.
pub(super) fn of_path(path: &str) -> Option<Language> {
    let name = Path::new(path).file_name()?.to_str()?;
    let extension = || Path::new(name).extension()?.to_str();
    let syntax = (SYNTAXES.find_syntax_by_extension(name))
        .or_else(|| SYNTAXES.find_syntax_by_extension(extension()?))?;
    (syntax.name != "Plain Text").then_some(Language(syntax))
}

/// Whether a file's name shows it is Markdown, as `.md` or `.markdown`.
pub(super) fn is_markdown(path: &str) -> bool {
    of_path(path).is_some_and(|Language(syntax)| syntax.name == "Markdown")
}

/// Highlights the lines of one piece of code in order, styled over `base`.
pub(super) struct Highlighter {
    base: &'static str,
    /// The states after the lines so far, and what identifies those lines.
    states: States,
    key: u64,
}

impl Highlighter {
    pub(super) fn new(Language(syntax): Language, base: &'static str) -> Self {
        let mut key = DefaultHasher::new();
        (&syntax.name, base).hash(&mut key);
        let parse = ParseState::new(syntax);
        let highlight = HighlightState::new(&STYLER, ScopeStack::new());
        Self {
            base,
            states: Rc::new((parse, highlight)),
            key: key.finish(),
        }
    }

    /// `line`, without its line break, styled.
    pub(super) fn line(&mut self, line: &str) -> String {
        let mut key = DefaultHasher::new();
        (self.key, line).hash(&mut key);
        self.key = key.finish();
        LINES.with_borrow_mut(|kept| self.highlight(line, kept))
    }

    fn highlight(&mut self, line: &str, kept: &mut HashMap<u64, (String, States)>) -> String {
        if let Some((shown, states)) = kept.get(&self.key) {
            self.states = states.clone();
            return shown.clone();
        }
        let (mut parse, mut highlight) = (*self.states).clone();
        let text = format!("{line}\n");
        let Ok(operations) = parse.parse_line(&text, &SYNTAXES) else {
            return line.to_owned();
        };
        let mut out = String::new();
        let mut current = "";
        let tokens = HighlightIterator::new(&mut highlight, &operations, &text, &STYLER);
        for (style, token) in tokens {
            let token = token.strip_suffix('\n').unwrap_or(token);
            if token.is_empty() {
                continue;
            }
            let wanted = match style.foreground.r as usize {
                0 => "",
                index => RULES[index - 1].1,
            };
            if wanted != current {
                if !current.is_empty() {
                    out += RESET;
                    out += self.base;
                }
                out += wanted;
                current = wanted;
            }
            out += token;
        }
        if !current.is_empty() {
            out += RESET;
            out += self.base;
        }
        if kept.len() >= KEPT_LINES {
            kept.clear();
        }
        self.states = Rc::new((parse, highlight));
        kept.insert(self.key, (out.clone(), self.states.clone()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighted(language: &str, code: &str) -> Vec<String> {
        let mut highlighter = Highlighter::new(named(language).unwrap(), "");
        code.lines().map(|line| highlighter.line(line)).collect()
    }

    #[test]
    fn code_is_colored_as_scramjets_theme_colors_its_classes() {
        assert_eq!(
            highlighted("rust", "fn main() -> u8 { \"s\".len() + 1 } // done"),
            [format!(
                "{SYNTAX_KEYWORD}fn{RESET} {SYNTAX_FUNCTION}main{RESET}() -> \
{SYNTAX_KEYWORD}u8{RESET} {{ {SYNTAX_STRING}\"s\"{RESET}.{SYNTAX_FUNCTION}len{RESET}() + \
{SYNTAX_NUMBER}1{RESET} }} {SYNTAX_COMMENT}// done{RESET}"
            )]
        );
        // A key, and diff lines.
        assert_eq!(
            highlighted("json", "{\"a\": true}"),
            [format!(
                "{{{SYNTAX_VARIABLE}\"a\"{RESET}: {SYNTAX_NUMBER}true{RESET}}}"
            )]
        );
        assert_eq!(
            highlighted("diff", "-old\n+new"),
            [
                format!("{DIFF_REMOVED}-old{RESET}"),
                format!("{DIFF_ADDED}+new{RESET}")
            ]
        );
        // Styles end in the base style around them.
        let mut highlighter = Highlighter::new(named("py").unwrap(), "\x1b[2m");
        assert_eq!(
            highlighter.line("x = 'a'"),
            format!("x = {SYNTAX_STRING}'a'{RESET}\x1b[2m")
        );
    }

    #[test]
    fn a_line_depends_on_the_lines_before_it_however_often_it_is_highlighted() {
        let code = "/* a\nstill comment */ let x = 1;\nlet y = 2;";
        let first = highlighted("js", code);
        assert_eq!(
            first[1],
            format!(
                "{SYNTAX_COMMENT}still comment */{RESET} {SYNTAX_KEYWORD}let{RESET} x = {SYNTAX_NUMBER}1{RESET};"
            )
        );
        assert_eq!(highlighted("js", code), first);
        // The same line after other lines is highlighted for them.
        let other = highlighted("js", "let a;\nstill comment */ let x = 1;");
        assert_ne!(other[1], first[1]);
        // Kept lines are found after a new line too.
        let longer = highlighted("js", &format!("{code}\n/* b */"));
        assert_eq!(longer[..3], first[..]);
        assert_eq!(longer[3], format!("{SYNTAX_COMMENT}/* b */{RESET}"));
    }

    #[test]
    fn languages_are_found_by_highlight_js_names_and_file_names() {
        let name = |language: Option<Language>| language.map(|Language(syntax)| &*syntax.name);
        assert_eq!(name(named("Rust")), Some("Rust"));
        assert_eq!(name(named("ts {title=x}")), Some("TypeScript"));
        assert_eq!(name(named("rust,ignore")), Some("Rust"));
        assert_eq!(name(named("console")), Some("Bourne Again Shell (bash)"));
        assert_eq!(name(named("text")), Some("Plain Text"));
        assert_eq!(name(named("nonsense")), None);
        assert_eq!(name(named("")), None);
        assert_eq!(name(of_path("src/main.rs")), Some("Rust"));
        assert_eq!(name(of_path("/x/Makefile")), Some("Makefile"));
        assert_eq!(name(of_path("a/b.TOML")), Some("TOML"));
        assert_eq!(name(of_path("notes.txt")), None);
        assert_eq!(name(of_path("README")), None);
        assert!(is_markdown("docs/TASKS.md") && is_markdown("a.markdown"));
        assert!(!is_markdown("a.rs") && !is_markdown("README"));
    }
}
