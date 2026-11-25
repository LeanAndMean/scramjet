//! The private directories a run makes in the temporary directory, `PREFIX-PID-N`: the sessions
//! of a run that saves none (`session::Unsaved`) and a sandbox's `/tmp` (`sandbox::Scratch`).
//! While its run holds one, a file `PREFIX-PID-N.lock` beside it is locked; the run removes both
//! as it ends, and a later run those of runs that ended without (killed, crashed).

use std::path::{Path, PathBuf};
use std::time::Duration;

const PREFIXES: [&str; 2] = ["scramjet-unsaved-", "scramjet-tmp-"];

/// How long a directory is left after its last change: its run may have made it and not locked
/// it yet.
const SETTLED: Duration = Duration::from_secs(60);

/// The lock beside `directory`. Beside it, as the sessions of a run that saves none lock their
/// directory itself while writing, and a sandbox's `/tmp` would show one in it.
fn lock_of(directory: &Path) -> PathBuf {
    let mut name = directory.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

/// Locks `directory`, made by this run, for as long as the file returned is held.
pub(super) fn hold(directory: &Path) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    crate::os::private_file(options.create(true).write(true).truncate(false));
    let lock = options.open(lock_of(directory)).ok()?;
    lock.try_lock().ok()?;
    Some(lock)
}

/// Removes `directory` and its lock.
pub(super) fn remove(directory: &Path) {
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_file(lock_of(directory));
}

/// Removes, once a process and in the background, the directories left in the temporary
/// directory by runs that have ended; none from unit tests, which must leave the user's alone.
pub(super) fn sweep() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    if cfg!(test) {
        return;
    }
    SWEPT.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("leftovers".into())
            .spawn(|| sweep_in(&std::env::temp_dir(), SETTLED));
    });
}

/// Removes those of `directory`'s entries that are this user's directories named as a run's,
/// unchanged for `settled`, whose process is gone and whose lock is free. Returns what it
/// removed.
fn sweep_in(directory: &Path, settled: Duration) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = run_of(&name.to_string_lossy()) else {
            continue;
        };
        let path = entry.path();
        if pid == std::process::id()
            || !ours(&path, settled)
            || crate::os::process_runs(pid)
            || held(&path)
        {
            continue;
        }
        remove(&path);
        if !path.exists() {
            removed.push(path);
        }
    }
    removed
}

/// The process ID in `name`, if it is `PREFIX-PID-N`.
fn run_of(name: &str) -> Option<u32> {
    let rest = PREFIXES
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))?;
    let (pid, n) = rest.split_once('-')?;
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (digits(pid) && digits(n))
        .then(|| pid.parse().ok())
        .flatten()
}

/// Whether `path` is a directory, not a link to one, of this user's, unchanged for `settled`.
fn ours(path: &Path, settled: Duration) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    let owned =
        std::os::unix::fs::MetadataExt::uid(&metadata) == rustix::process::getuid().as_raw();
    #[cfg(not(unix))]
    let owned = true;
    let changed = metadata.modified().ok().and_then(|at| at.elapsed().ok());
    metadata.is_dir() && owned && changed.is_some_and(|age| age >= settled)
}

/// Whether a run holds `directory`'s lock (`hold`); one made before 2.347.1 has none, and whether
/// its process runs decides.
fn held(directory: &Path) -> bool {
    match std::fs::OpenOptions::new()
        .write(true)
        .open(lock_of(directory))
    {
        Ok(lock) => lock.try_lock().is_err(),
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    /// The ID of a process that has ended.
    fn ended() -> u32 {
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/c", "exit"][..]
            } else {
                &[]
            })
            .spawn()
            .unwrap();
        child.wait().unwrap();
        child.id()
    }

    #[test]
    fn the_directories_of_runs_that_ended_are_removed_and_no_others() {
        let base = std::env::temp_dir().join(format!("scramjet-leftovers-{}", std::process::id()));
        let _base = crate::agent::tests::ScratchDir::new(base.clone());
        let gone = ended();
        let make = |name: &str| {
            let path = base.join(name);
            std::fs::create_dir_all(path.join("inner")).unwrap();
            std::fs::write(path.join("inner/dump.txt"), "kept by a killed run").unwrap();
            path
        };
        let unsaved = make(&format!("scramjet-unsaved-{gone}-0"));
        let scratch = make(&format!("scramjet-tmp-{gone}-3"));
        let held = make(&format!("scramjet-tmp-{gone}-4"));
        let _lock = super::hold(&held).unwrap();
        // As its run left it: closed, so free.
        drop(super::hold(&scratch).unwrap());
        let own = make(&format!("scramjet-tmp-{}-0", std::process::id()));
        let other_name = make(&format!("scramjet-tmpfiles-{gone}-0"));
        let unnumbered = make(&format!("scramjet-unsaved-{gone}"));
        let file = base.join(format!("scramjet-unsaved-{gone}-1"));
        std::fs::write(&file, "a file").unwrap();
        #[cfg(unix)]
        let link = {
            let link = base.join(format!("scramjet-tmp-{gone}-5"));
            std::os::unix::fs::symlink(&other_name, &link).unwrap();
            link
        };
        assert_eq!(
            super::sweep_in(&base, Duration::from_secs(3600)),
            [] as [&Path; 0]
        );
        let mut removed = super::sweep_in(&base, Duration::ZERO);
        removed.sort();
        assert_eq!(removed, [scratch.clone(), unsaved.clone()]);
        assert!(!unsaved.exists() && !scratch.exists());
        assert!(!super::lock_of(&scratch).exists() && super::lock_of(&held).exists());
        for kept in [&held, &own, &other_name, &unnumbered, &file] {
            assert!(kept.exists(), "{}", kept.display());
        }
        #[cfg(unix)]
        assert!(link.symlink_metadata().is_ok() && other_name.join("inner/dump.txt").exists());
        drop(_lock);
        assert_eq!(
            super::sweep_in(&base, Duration::ZERO),
            std::slice::from_ref(&held)
        );
        assert!(!super::lock_of(&held).exists());
    }
}
