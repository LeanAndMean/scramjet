//! Text or a file edited with `$VISUAL` or `$EDITOR`, with the terminal as the shell left it.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// `$VISUAL` or `$EDITOR` run on `text` with the terminal as the shell left it: its text, or
/// `None` when it did not save; an error when none is configured or it could not run.
pub(super) fn external(text: &str) -> Result<Option<String>, String> {
    match editor_command() {
        Some(command) => run_editor(&command, text),
        None => Err(NO_EDITOR.to_owned()),
    }
}

const NO_EDITOR: &str = "No editor configured. Set $VISUAL or $EDITOR environment variable.";

/// Runs `$VISUAL` or `$EDITOR` on `file` with the terminal as the shell left it, until it
/// exits; an error when none is configured or it could not start.
pub(super) fn edit_file(file: &Path) -> Result<(), String> {
    let command = editor_command().ok_or_else(|| NO_EDITOR.to_owned())?;
    run_on(&command, file).map(drop)
}

/// `$VISUAL`, else `$EDITOR`, as Scramjet reads them.
fn editor_command() -> Option<String> {
    ["VISUAL", "EDITOR"].iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

/// Runs the editor `command`, its words split at spaces as Scramjet splits them, on `file`.
fn run_on(command: &str, file: &Path) -> Result<std::process::ExitStatus, String> {
    let mut parts = command.split(' ').filter(|part| !part.is_empty());
    let program = parts.next().ok_or_else(|| NO_EDITOR.to_owned())?;
    (Command::new(program).args(parts).arg(file).status())
        .map_err(|error| format!("cannot start {program}: {error}"))
}

/// Runs the editor on a temporary file holding `text`. `Ok(None)` when it exits with another
/// status than 0, which keeps the prompt text. Its file's text, without one trailing newline,
/// replaces the prompt text.
fn run_editor(command: &str, text: &str) -> Result<Option<String>, String> {
    let (path, mut file) = create_prompt_file()
        .map_err(|error| format!("cannot create a file for the prompt: {error}"))?;
    let outcome = file
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())
        .and_then(|()| {
            drop(file);
            let status = run_on(command, &path)?;
            if !status.success() {
                return Ok(None);
            }
            fs::read_to_string(&path)
                .map(Some)
                .map_err(|error| error.to_string())
        });
    let _ = fs::remove_file(&path);
    outcome.map(|edited| edited.map(|text| text.strip_suffix('\n').unwrap_or(&text).to_owned()))
}

/// Creates a new file in the temporary directory; an existing file is never opened.
fn create_prompt_file() -> io::Result<(PathBuf, File)> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut attempt = 0;
    loop {
        let path = std::env::temp_dir().join(format!(
            "scramjet-rs-prompt-{}-{nanos}-{attempt}.md",
            std::process::id()
        ));
        match scramjet_rs::os::private_file(OpenOptions::new().write(true).create_new(true))
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
