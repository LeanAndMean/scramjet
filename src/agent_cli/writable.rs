//! Whether Scramjet can write where it keeps its files, checked at start. A command run with
//! `sudo` (Scramjet 1's npm package installed with it, or Scramjet run with it once) leaves them
//! owned by root, and every later write then fails far from its cause.

#[cfg(unix)]
use std::path::{Path, PathBuf};

/// `Err` with what cannot be written and the one command that gives it back, when a directory
/// Scramjet writes in, or a file or directory directly in it that another user owns, cannot be
/// written. Not checked on Windows, where `sudo` leaves no such files.
pub(super) fn check() -> Result<(), String> {
    #[cfg(unix)]
    {
        let roots = [
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_STATE_HOME", ".local/state"),
            ("XDG_DATA_HOME", ".local/share"),
        ]
        .into_iter()
        .filter_map(|(variable, under)| scramjet::os::base_directory(variable, under))
        .map(|base| base.join("scramjet"));
        let blocked: Vec<Blocked> = roots.flat_map(|root| blocked(&root)).collect();
        match blocked.is_empty() {
            true => Ok(()),
            false => Err(message(
                &blocked,
                std::env::home_dir().as_deref(),
                (
                    "cannot write where Scramjet keeps its files",
                    "start Scramjet again",
                ),
            )),
        }
    }
    #[cfg(not(unix))]
    Ok(())
}

#[cfg(unix)]
struct Blocked {
    path: PathBuf,
    /// Its owner when that is another user.
    owner: Option<u32>,
}

/// What in `root` cannot be written: the nearest directory of it or above it that exists, when
/// this user cannot write in it, and what `root` holds directly that another user owns and this
/// one cannot write. One the user made read-only for themselves, or a link, is theirs to keep.
#[cfg(unix)]
fn blocked(root: &Path) -> Vec<Blocked> {
    use rustix::fs::Access;
    if let Some(blocked) = directory(root) {
        return vec![blocked];
    }
    let usable = Access::READ_OK | Access::WRITE_OK | Access::EXEC_OK;
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Some(owner) = stranger(&metadata).filter(|_| !metadata.is_symlink()) else {
            continue;
        };
        let needed = match metadata.is_dir() {
            true => usable,
            false => Access::WRITE_OK,
        };
        if rustix::fs::access(entry.path(), needed).is_err() {
            found.push(Blocked {
                path: entry.path(),
                owner: Some(owner),
            });
        }
    }
    found
}

/// `directory`, or the nearest directory above it that exists, when this user cannot make
/// files in it.
#[cfg(unix)]
fn directory(directory: &Path) -> Option<Blocked> {
    use rustix::fs::Access;
    let existing = directory.ancestors().find(|path| path.exists())?;
    let usable = Access::READ_OK | Access::WRITE_OK | Access::EXEC_OK;
    rustix::fs::access(existing, usable).err()?;
    let owner = std::fs::metadata(existing)
        .ok()
        .and_then(|found| stranger(&found));
    Some(Blocked {
        path: existing.to_owned(),
        owner,
    })
}

/// `Err` saying what keeps this user from replacing `program`, and how to fix it, when they
/// cannot write in its directory: in the home directory the command that gives it back, else
/// to update it as its owner, who installed it there.
#[cfg(unix)]
pub(super) fn program(program: &Path) -> Result<(), String> {
    let Some(blocked) = program.parent().and_then(directory) else {
        return Ok(());
    };
    let home = std::env::home_dir();
    let heading = format!("cannot replace {}", scramjet::os::tilde(program));
    match (blocked.owner, &home) {
        (Some(_), Some(home)) if !blocked.path.starts_with(home) => Err(format!(
            "{heading}: {} belongs to another user; update it as they installed it, or install \
             into your own ~/.local/bin (see install.sh)",
            blocked.path.display()
        )),
        _ => Err(message(
            &[blocked],
            home.as_deref(),
            (&heading, "run `scramjet update self` again"),
        )),
    }
}

/// The owner of what `metadata` describes, when that is not this user.
#[cfg(unix)]
fn stranger(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    (!scramjet::os::owned(metadata)).then(|| metadata.uid())
}

/// What `blocked` names, and the commands that fix it: `chown` of the outermost directory in the
/// home directory that another user holds, so that everything under it is given back at once,
/// and `chmod` of what is the user's own. Each path is named once, with none under it.
#[cfg(unix)]
fn message(blocked: &[Blocked], home: Option<&Path>, (heading, again): (&str, &str)) -> String {
    let mut fixes: Vec<(PathBuf, Option<u32>)> = (blocked.iter())
        .map(|item| match item.owner {
            Some(_) => (outermost(&item.path, home), item.owner),
            None => (item.path.clone(), None),
        })
        .collect();
    fixes.sort();
    fixes.dedup_by(|later, earlier| later.0 == earlier.0);
    let all = fixes.clone();
    fixes.retain(|(path, _)| {
        !all.iter()
            .any(|(other, _)| other != path && path.starts_with(other))
    });
    let mut lines = vec![format!("{heading}:")];
    for (path, owner) in &fixes {
        let shown = scramjet::os::tilde(path);
        lines.push(match owner {
            Some(0) => format!("  {shown} is owned by root, not you"),
            Some(other) => format!("  {shown} is owned by user {other}, not you"),
            None => format!("  {shown} is yours but not writable"),
        });
    }
    let (given_back, opened): (Vec<_>, Vec<_>) =
        fixes.iter().partition(|(_, owner)| owner.is_some());
    if !given_back.is_empty() {
        lines.push("A command run with sudo most likely made it so.".to_owned());
    }
    lines.push(format!("Run this, then {again}:"));
    for (command, paths) in [
        ("sudo chown -R \"$USER\"", given_back),
        ("chmod u+rwx", opened),
    ] {
        if !paths.is_empty() {
            let shown: Vec<String> = paths.iter().map(|(path, _)| quoted(path)).collect();
            lines.push(format!("  {command} {}", shown.join(" ")));
        }
    }
    lines.join("\n")
}

/// `path`, or the outermost directory above it, below the home directory, that another user
/// also owns.
#[cfg(unix)]
fn outermost(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home.filter(|home| path.starts_with(home)) else {
        return path.to_owned();
    };
    let mut outermost = path;
    while let Some(parent) = outermost.parent()
        && parent != home
        && parent.starts_with(home)
        && std::fs::metadata(parent).is_ok_and(|found| stranger(&found).is_some())
    {
        outermost = parent;
    }
    outermost.to_owned()
}

/// `path` as a shell reads it back: as `~/…` in the home directory, quoted when it holds more
/// than plain characters.
#[cfg(unix)]
fn quoted(path: &Path) -> String {
    let shown = scramjet::os::tilde(path);
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-+@%:,=".contains(c);
    let (start, rest) = match shown.strip_prefix("~/") {
        Some(rest) => ("~/", rest),
        None => ("", shown.as_str()),
    };
    match rest.chars().all(plain) {
        true => shown.clone(),
        false => format!("{start}'{}'", rest.replace('\'', r"'\''")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn names_each_path_and_gives_one_command_per_kind() {
        let home = Path::new("/home/u");
        let blocked = [
            Blocked {
                path: "/home/u/.local/state".into(),
                owner: Some(0),
            },
            Blocked {
                path: "/home/u/.config/scramjet/auth.json".into(),
                owner: Some(501),
            },
            Blocked {
                path: "/home/u/.local/state/scramjet/x".into(),
                owner: Some(0),
            },
            Blocked {
                path: "/srv/my data".into(),
                owner: None,
            },
        ];
        let message = message(&blocked, Some(home), ("cannot write here", "start again"));
        let lines: Vec<&str> = message.lines().collect();
        assert_eq!(lines[0], "cannot write here:");
        assert_eq!(lines[5], "Run this, then start again:");
        assert_eq!(
            lines[1],
            "  /home/u/.config/scramjet/auth.json is owned by user 501, not you"
        );
        assert_eq!(lines[2], "  /home/u/.local/state is owned by root, not you");
        assert_eq!(lines[3], "  /srv/my data is yours but not writable");
        assert_eq!(lines[4], "A command run with sudo most likely made it so.");
        assert_eq!(
            lines[6],
            "  sudo chown -R \"$USER\" /home/u/.config/scramjet/auth.json /home/u/.local/state"
        );
        assert_eq!(lines[7], "  chmod u+rwx '/srv/my data'");
        assert_eq!(lines.len(), 8);
    }

    #[test]
    fn quotes_what_a_shell_would_split() {
        assert_eq!(quoted(Path::new("/a/b.c")), "/a/b.c");
        assert_eq!(quoted(Path::new("/a/it's")), r"'/a/it'\''s'");
    }

    #[test]
    fn a_directory_of_ones_own_is_not_blocked() {
        let scratch = std::env::temp_dir().join(format!("writable-{}", std::process::id()));
        std::fs::create_dir_all(scratch.join("root")).unwrap();
        std::fs::write(scratch.join("root/settings.json"), "{}").unwrap();
        assert!(blocked(&scratch.join("root")).is_empty());
        assert!(blocked(&scratch.join("missing/root")).is_empty());
        std::fs::remove_dir_all(&scratch).unwrap();
    }
}
