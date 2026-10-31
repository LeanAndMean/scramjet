//! Shell commands that run without an approval question: one plain command from a short
//! read-only list, with no option that makes it write files or run other programs.

/// `find` expressions that write files or run programs.
const FIND_EFFECTS: [&str; 9] = [
    "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fls", "-fprint", "-fprint0", "-fprintf",
];

pub(super) fn read_only(command: &str) -> bool {
    let Some(Words {
        words,
        leading_glob,
    }) = words(command.trim_end())
    else {
        return false;
    };
    let Some((name, args)) = words.split_first() else {
        return false;
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match name.as_str() {
        "ls" | "cat" | "head" | "tail" | "wc" | "pwd" | "grep" | "stat" | "du" | "diff"
        | "realpath" | "basename" | "dirname" | "which" => true,
        // A glob at the start of a word can expand to a file named like an option.
        _ if leading_glob => false,
        "rg" => !args
            .iter()
            .any(|arg| option(arg, "--pre") || option(arg, "--hostname-bin")),
        "find" => !args.iter().any(|arg| FIND_EFFECTS.contains(arg)),
        "sed" => sed_print(&args),
        "git" => git(&args),
        _ => false,
    }
}

/// `arg` is the long option `name`, with or without an attached value.
fn option(arg: &str, name: &str) -> bool {
    arg.strip_prefix(name)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
}

/// `sed -n 'N,Mp' FILE...`: prints a line range and does nothing else.
fn sed_print(args: &[&str]) -> bool {
    let ["-n", script, files @ ..] = args else {
        return false;
    };
    let Some(range) = script.strip_suffix('p') else {
        return false;
    };
    let bound = |b: &str| b == "$" || (!b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()));
    let range = match range.split_once(',') {
        Some((first, last)) => bound(first) && bound(last),
        None => bound(range),
    };
    // GNU sed also takes options after the script, such as `-i`.
    range && files.iter().all(|file| !file.starts_with('-'))
}

fn git(mut args: &[&str]) -> bool {
    while let ["--no-pager", rest @ ..] | ["-C", _, rest @ ..] = args {
        args = rest;
    }
    let [subcommand, args @ ..] = args else {
        return false;
    };
    matches!(*subcommand, "status" | "diff" | "log" | "show")
        && !args.iter().any(|arg| {
            // Any prefix of `--output`, in case a git version accepts abbreviated options.
            let name = arg.split('=').next().unwrap_or(arg);
            name.len() > 2 && "--output".starts_with(name)
        })
}

struct Words {
    words: Vec<String>,
    /// Some word starts with a glob or is an option containing one, so its expansion is unknown.
    leading_glob: bool,
}

/// Splits a command into words as bash would, with quotes removed, or `None` if bash would do
/// more than run one command with literal arguments: operators, redirection, expansion,
/// substitution, grouping, comments or control characters (a second line would not be shown
/// on the call line).
fn words(command: &str) -> Option<Words> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut leading_glob = false;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => words.extend(word.take()),
            '\'' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c if c.is_control() => return None,
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                let word = word.get_or_insert_default();
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' | '`' => return None,
                        '\\' => match chars.next()? {
                            c @ ('"' | '\\' | '$' | '`') => word.push(c),
                            c if c.is_control() => return None,
                            c => word.extend(['\\', c]),
                        },
                        c if c.is_control() => return None,
                        c => word.push(c),
                    }
                }
            }
            '\\' => match chars.next()? {
                c if c.is_control() => return None,
                c => word.get_or_insert_default().push(c),
            },
            '*' | '?' | '[' => {
                let word = word.get_or_insert_default();
                leading_glob |= word.is_empty() || word.starts_with('-');
                word.push(c);
            }
            '|' | '&' | ';' | '(' | ')' | '<' | '>' | '$' | '`' | '#' | '{' | '}' => return None,
            c if c.is_control() => return None,
            c => word.get_or_insert_default().push(c),
        }
    }
    words.extend(word);
    Some(Words {
        words,
        leading_glob,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_read_only_commands_run_without_asking() {
        for command in [
            "ls -la",
            "ls -la\n",
            "cat src/main.rs README.md",
            "rg -n 'fn (main|run)' src",
            "rg \"a|b > c\" -g '!*.lock'",
            "rg -n \"\\$HOME\" src",
            "rg 'x' src/*.rs",
            "grep -rn foo *",
            "head -n 50 ~/repos/scramjet/README.md",
            "wc -l src/agent/*.rs",
            "find . -name '*.rs' -newer Cargo.toml",
            "sed -n '120,180p' src/agent.rs",
            "sed -n 5p a b",
            "sed -n '1,$p' a",
            "git status --short",
            "git diff HEAD~1 -- src",
            "git -C ../other --no-pager log --oneline -5",
            "git log --output-indicator-new=+ -p",
            "git show HEAD:src/main.rs",
            "pwd",
        ] {
            assert!(read_only(command), "{command:?} should run without asking");
        }
    }

    #[test]
    fn anything_that_may_write_run_programs_or_hide_work_asks() {
        for command in [
            "",
            "ls; rm -rf x",
            "ls && rm x",
            "ls & rm x",
            "cat a | sh",
            "cat a > b",
            "cat < a",
            "cat $(ls)",
            "cat `ls`",
            "cat \"$(ls)\"",
            "cat $HOME/x",
            "cat {a,b}",
            "ls # hidden",
            "ls\nrm x",
            "ls 'a\nb'",
            "(ls)",
            "cat 'unterminated",
            "rm -rf target",
            "cargo test",
            "true",
            "GIT_EXTERNAL_DIFF=x git diff",
            "awk '{print}' a",
            "rg --pre cat x",
            "rg --pre=cat x",
            "rg --hostname-bin=x --hyperlink-format=default x",
            "rg x *",
            "rg x --pr*",
            "find . -name x -delete",
            "find . -exec cat {} \\;",
            "find . -fprint out",
            "find * -name x",
            "sed -i 's/a/b/' f",
            "sed -n 'w out' f",
            "sed -n '1,5p' f -i",
            "sed '1,5p' f",
            "git diff --output=x",
            "git diff --output x",
            "git log -p --outp=x",
            "git -c core.pager=sh log",
            "git commit -m x",
            "git branch new",
            "git -C",
            "git",
            "git diff *",
        ] {
            assert!(!read_only(command), "{command:?} should ask");
        }
    }

    #[test]
    fn words_follow_bash_quoting() {
        let split = |command| words(command).map(|words| words.words);
        assert_eq!(
            split("rg  'a b'\"c\\\"d\\e\" x\\ y").unwrap(),
            ["rg", "a bc\"d\\e", "x y"]
        );
        assert_eq!(split("ls ''").unwrap(), ["ls", ""]);
        assert!(split("ls \\*").is_some_and(|w| w == ["ls", "*"]));
        assert!(!words("ls \\*").unwrap().leading_glob);
        assert!(words("ls *").unwrap().leading_glob);
        assert!(!words("ls src/*").unwrap().leading_glob);
    }
}
