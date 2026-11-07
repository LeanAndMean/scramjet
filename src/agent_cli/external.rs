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
#[cfg(not(windows))]
fn run_on(command: &str, file: &Path) -> Result<std::process::ExitStatus, String> {
    let mut parts = command.split(' ').filter(|part| !part.is_empty());
    let program = parts.next().ok_or_else(|| NO_EDITOR.to_owned())?;
    (Command::new(program).args(parts).arg(file).status())
        .map_err(|error| format!("cannot start {program}: {error}"))
}

/// Runs the editor `command` on `file` through `cmd.exe`, as Scramjet's `shell: true` does there,
/// so that `code --wait` finds `code.cmd`; the file is quoted, as a profile's path may hold spaces.
#[cfg(windows)]
fn run_on(command: &str, file: &Path) -> Result<std::process::ExitStatus, String> {
    use std::os::windows::process::CommandExt;
    let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into());
    (Command::new(shell)
        .raw_arg(windows_line(command, file))
        .status())
    .map_err(|error| format!("cannot start {command}: {error}"))
}

/// `cmd.exe`'s arguments for `command` on `file`: `/s` strips the outer quotes, keeping the rest.
#[cfg(any(windows, test))]
fn windows_line(command: &str, file: &Path) -> String {
    format!("/d /s /c \"{} \"{}\"\"", command.trim(), file.display())
}

/// Runs the editor on a temporary file holding `text`. `Ok(None)` when it exits with another
/// status than 0, which keeps the prompt text. Its file's text (`taken`) replaces the prompt text.
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
    outcome.map(|edited| edited.map(taken))
}

/// An edited file's text as the prompt takes it: line breaks as `\n`, as Scramjet's `setText`
/// makes them (Windows' editors save `\r\n`), without one trailing line break.
fn taken(text: String) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.strip_suffix('\n').unwrap_or(&text).to_owned()
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

#[cfg(test)]
mod tests {
    #[test]
    fn an_edited_file_has_its_line_breaks_made_newlines_and_one_trailing_dropped() {
        assert_eq!(super::taken("a\r\nb\rc\r\n".into()), "a\nb\nc");
        assert_eq!(super::taken("a\n\n".into()), "a\n");
        assert_eq!(super::taken("a".into()), "a");
    }

    #[test]
    fn cmd_gets_the_editor_and_its_file_quoted_within_one_quoted_line() {
        let file = std::path::Path::new(r"C:\Users\A B\Temp\p.md");
        assert_eq!(
            super::windows_line(" code --wait ", file),
            r#"/d /s /c "code --wait "C:\Users\A B\Temp\p.md"""#
        );
    }
}
