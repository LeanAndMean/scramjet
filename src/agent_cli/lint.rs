//! `scramjet lint [--strict] [--json] DIR...`: checks command sets without loading a session,
//! in place of Scramjet's `scramjet-command-lint`, whose output it writes.
use scramjet::agent::{Diagnostic, Origin, Set, Severity, lint, sets_in};
use std::path::{Path, PathBuf};

pub(super) const USAGE: &str = "Usage: scramjet lint [--strict] [--json] DIR...
DIR: a command set, or a package holding sets. Errors are files the harness would skip, warnings
what it would ignore and commands without an early ## Goals list or with their arguments outside
one <user-context> (delegate-only: <caller-context>) block. --strict fails on warnings too.";

pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    (command == "lint").then(|| lint_command(arguments))
}

fn lint_command(arguments: &[String]) -> Result<(), String> {
    let (mut strict, mut json, mut targets) = (false, false, Vec::new());
    for argument in arguments {
        match argument.as_str() {
            "--strict" => strict = true,
            "--json" => json = true,
            "--help" | "-h" => {
                outln!("{USAGE}");
                return Ok(());
            }
            option if option.starts_with('-') => {
                return Err(format!("unknown option {option}\n{USAGE}"));
            }
            target => targets.push(target),
        }
    }
    if targets.is_empty() {
        return Err(USAGE.to_owned());
    }
    let mut sets = Vec::new();
    for target in targets {
        let found = sets_in(Path::new(target));
        if found.is_empty() {
            return Err(format!("{target} is not a command set and holds none"));
        }
        for directory in found {
            sets.push(Set::at(&directory, Origin::Installed)?);
        }
    }
    let (mut diagnostics, mut files) = (Vec::new(), Vec::new());
    for set in &sets {
        let (found, read) = lint(set);
        diagnostics.extend(found);
        files.extend(read);
    }
    let count = |severity| {
        (diagnostics.iter())
            .filter(|d| d.severity == severity)
            .count()
    };
    let (errors, warnings) = (count(Severity::Error), count(Severity::Warning));
    match json {
        true => outln!("{}", report(&diagnostics, &files, errors, warnings)),
        false => {
            for diagnostic in &diagnostics {
                outln!("{}", shown(diagnostic));
            }
            outln!(
                "Checked {}: {}, {}.",
                plural(files.len(), "file"),
                plural(errors, "error"),
                plural(warnings, "warning")
            );
        }
    }
    match (errors, warnings) {
        (0, 0) => Ok(()),
        (0, _) if !strict => Ok(()),
        (0, _) => Err(format!("{} (--strict)", plural(warnings, "warning"))),
        _ => Err(plural(errors, "error")),
    }
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    }
}

/// `text` with control characters written as `\uXXXX`, so a file cannot drive the terminal.
fn safe(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for character in text.chars() {
        match character.is_control() {
            true => shown.push_str(&format!("\\u{:04x}", character as u32)),
            false => shown.push(character),
        }
    }
    shown
}

fn shown(diagnostic: &Diagnostic) -> String {
    format!(
        "{}:{}: {} {}: {}",
        safe(&diagnostic.path.display().to_string()),
        diagnostic.line.unwrap_or(1),
        diagnostic.severity.name(),
        diagnostic.code,
        safe(&diagnostic.message)
    )
}

fn report(diagnostics: &[Diagnostic], files: &[PathBuf], errors: usize, warnings: usize) -> String {
    use serde_json::{Value, json};
    let diagnostics: Vec<Value> = (diagnostics.iter())
        .map(|diagnostic| {
            let mut entry = json!({
                "code": diagnostic.code,
                "severity": diagnostic.severity.name(),
                "filePath": diagnostic.path.display().to_string(),
                "message": diagnostic.message,
            });
            if let Some(line) = diagnostic.line {
                entry["line"] = json!(line);
            }
            entry
        })
        .collect();
    let files: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    let report = json!({
        "checkedFiles": files,
        "diagnostics": diagnostics,
        "summary": {"files": files.len(), "errors": errors, "warnings": warnings},
    });
    serde_json::to_string_pretty(&report).expect("JSON")
}
