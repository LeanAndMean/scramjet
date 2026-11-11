//! Scramjet's update lock on an `autonomy.yaml` (`withConfigLock` in its
//! `autonomy-settings.ts`), so that a change saved by Scramjet and one saved here at the same
//! moment do not lose either: `<file>.lock` is made by hard-linking a prepared file that names
//! its owner (`{"pid":…,"token":…}`), and one whose owner is gone is taken back through a
//! `<file>.lock.reclaim` claim made the same way.

use crate::os;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const ATTEMPTS: u32 = 50;
const RETRY: Duration = Duration::from_millis(10);
/// How long a lock or prepared file that names no owner is taken as being written.
const INCOMPLETE: Duration = Duration::from_secs(5);

/// Runs `action` holding the update lock of the file at `config`.
pub(super) fn locked<T>(
    config: &Path,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock = suffixed(config, ".lock");
    let shown = || super::shown_path(&lock);
    if let Some(directory) = config.parent() {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    }
    let owner = acquire(&lock).map_err(|error| format!("update lock {}: {error}", shown()))?;
    let Some(owner) = owner else {
        return Err(format!("update lock is busy: {}", shown()));
    };
    let value = action();
    let released = owner
        .release(&lock)
        .map_err(|error| format!("failed to release the update lock {}: {error}", shown()));
    let value = value?;
    released?;
    Ok(value)
}

/// A link this process made, and what it wrote in it.
struct Owner {
    inode: u64,
    content: String,
}

impl Owner {
    /// Whether `path` is still this link.
    fn holds(&self, path: &Path) -> io::Result<bool> {
        let inode = os::inode(&std::fs::symlink_metadata(path)?);
        Ok(inode == self.inode && std::fs::read_to_string(path)? == self.content)
    }

    fn release(&self, path: &Path) -> io::Result<()> {
        if !self.holds(path)? {
            return Err(io::Error::other("its owner changed"));
        }
        std::fs::remove_file(path)
    }
}

fn acquire(lock: &Path) -> io::Result<Option<Owner>> {
    remove_orphans(lock)?;
    for _ in 0..ATTEMPTS {
        let token = uuid()?;
        let content = format!(r#"{{"pid":{},"token":"{token}"}}"#, std::process::id());
        if let Some(owner) = link(lock, &token, content)? {
            return Ok(Some(owner));
        }
        if !reclaim(lock)? {
            std::thread::sleep(RETRY);
        }
    }
    Ok(None)
}

/// Makes `path` a link to a new file holding `content`; `None` when `path` already exists.
fn link(path: &Path, token: &str, content: String) -> io::Result<Option<Owner>> {
    let prepared = suffixed(path, &format!(".owner.{}.{token}", std::process::id()));
    let mut file = os::private_file(std::fs::OpenOptions::new().write(true).create_new(true))
        .open(&prepared)?;
    let written = file.write_all(content.as_bytes());
    drop(file);
    let linked = written.and_then(|()| std::fs::hard_link(&prepared, path));
    let removed = std::fs::remove_file(&prepared);
    let owner = match linked {
        Ok(()) => Owner {
            inode: os::inode(&std::fs::symlink_metadata(path)?),
            content,
        },
        Err(error) if error.kind() == ErrorKind::AlreadyExists || pending(&error) => {
            return removed.map(|()| None);
        }
        Err(error) => return Err(error),
    };
    if let Err(error) = removed {
        let _ = owner.release(path);
        return Err(error);
    }
    Ok(Some(owner))
}

/// Who a lock or claim names: its process and token, if it names them.
fn named(content: &str) -> (Option<u32>, Option<String>) {
    let value: serde_json::Value = serde_json::from_str(content).unwrap_or_default();
    let pid = match value.get("pid") {
        Some(pid) => pid.as_u64(),
        None => content.trim().parse().ok(),
    };
    let token = value
        .get("token")
        .and_then(|t| t.as_str())
        .map(str::to_owned);
    (pid.and_then(|pid| u32::try_from(pid).ok()), token)
}

/// Whether a lock or claim with `content` and `metadata` may still be in use: its process runs,
/// or it names none and is too new to be given up.
fn in_use(content: &str, metadata: &std::fs::Metadata) -> bool {
    match named(content).0.filter(|&pid| pid > 0) {
        Some(pid) => os::process_runs(pid),
        None => young(metadata),
    }
}

fn young(metadata: &std::fs::Metadata) -> bool {
    let age = metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    age.is_none_or(|age| age < INCOMPLETE)
}

/// Removes the lock when its owner is gone; whether to try taking it again at once.
fn reclaim(lock: &Path) -> io::Result<bool> {
    let (metadata, content) = match existing(lock) {
        Ok(Some(existing)) => existing,
        Ok(None) => return Ok(true),
        Err(error) if pending(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    if in_use(&content, &metadata) {
        return Ok(false);
    }
    let lock_token =
        (named(&content).1).unwrap_or_else(|| format!("legacy-{}", os::inode(&metadata)));
    let claim = suffixed(lock, ".reclaim");
    let token = uuid()?;
    let claim_content = format!(
        r#"{{"pid":{},"token":"{token}","lockToken":"{lock_token}"}}"#,
        std::process::id()
    );
    let Some(claimed) = link(&claim, &token, claim_content)? else {
        return reclaim_dead_claim(lock, &claim);
    };
    // A link of its own keeps the lock's inode from being reused while it is compared.
    let snapshot = suffixed(lock, &format!(".reclaim-snapshot.{token}"));
    let unchanged = |path: &Path| -> io::Result<bool> {
        Ok(
            os::inode(&std::fs::symlink_metadata(path)?) == os::inode(&metadata)
                && std::fs::read_to_string(path)? == content,
        )
    };
    let reclaimed = std::fs::hard_link(lock, &snapshot)
        .and_then(|()| Ok(unchanged(&snapshot)? && unchanged(lock)?))
        .and_then(|same| match same {
            true => std::fs::remove_file(lock).map(|()| true),
            false => Ok(false),
        });
    let reclaimed = match reclaimed {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        reclaimed => reclaimed,
    };
    let snapshot_removed = match std::fs::remove_file(&snapshot) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    let released = claimed.release(&claim);
    let reclaimed = reclaimed?;
    snapshot_removed?;
    released?;
    Ok(reclaimed)
}

/// Removes a claim whose owner is gone, with the snapshot it may have left.
fn reclaim_dead_claim(lock: &Path, claim: &Path) -> io::Result<bool> {
    let Some((metadata, content)) = existing(claim)? else {
        return Ok(true);
    };
    if in_use(&content, &metadata) {
        return Ok(false);
    }
    if let Some(token) = named(&content).1 {
        match std::fs::remove_file(suffixed(lock, &format!(".reclaim-snapshot.{token}"))) {
            Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    let owner = Owner {
        inode: os::inode(&metadata),
        content,
    };
    match owner.holds(claim) {
        Ok(true) => std::fs::remove_file(claim).map(|()| true),
        Ok(false) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

/// Removes the prepared files of owners that are gone, which a kill can leave.
fn remove_orphans(lock: &Path) -> io::Result<()> {
    let (Some(directory), Some(base)) = (lock.parent(), lock.file_name()) else {
        return Ok(());
    };
    let base = base.to_string_lossy();
    let prefixes = [format!("{base}.owner."), format!("{base}.reclaim.owner.")];
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if !prefixes
            .iter()
            .any(|p| name.to_string_lossy().starts_with(p))
        {
            continue;
        }
        let path = entry.path();
        let (metadata, content) = match existing(&path) {
            Ok(Some(existing)) => existing,
            Err(error) if !pending(&error) => return Err(error),
            _ => continue,
        };
        if in_use(&content, &metadata) {
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            Ok(now) if os::inode(&now) == os::inode(&metadata) => {
                match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != ErrorKind::NotFound && !pending(&error) => {
                        return Err(error);
                    }
                    _ => {}
                }
            }
            Err(error) if error.kind() != ErrorKind::NotFound && !pending(&error) => {
                return Err(error);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether `error` comes from a file another process is removing: Windows refuses to open or
/// link one until its last handle closes.
fn pending(error: &io::Error) -> bool {
    cfg!(windows)
        && (error.kind() == ErrorKind::PermissionDenied || error.raw_os_error() == Some(32))
}

/// A file's metadata and text, `None` when it is gone.
fn existing(path: &Path) -> io::Result<Option<(std::fs::Metadata, String)>> {
    let read = std::fs::symlink_metadata(path).and_then(|metadata| {
        let bytes = std::fs::read(path)?;
        Ok((metadata, String::from_utf8_lossy(&bytes).into_owned()))
    });
    match read {
        Ok(read) => Ok(Some(read)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// A random version 4 UUID, as Node's `randomUUID` makes the tokens.
fn uuid() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| io::Error::other(error.to_string()))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;

    fn scratch(name: &str) -> (ScratchDir, PathBuf) {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-rs-lock-{name}-{}", std::process::id())),
        );
        let config = scratch.0.join("autonomy.yaml");
        (scratch, config)
    }

    /// The id of a process that has ended.
    fn ended() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_lock_names_this_process_while_held_and_goes_after() {
        let (scratch, config) = scratch("held");
        let lock = scratch.0.join("autonomy.yaml.lock");
        let held = locked(&config, || Ok(std::fs::read_to_string(&lock).unwrap())).unwrap();
        let owner: serde_json::Value = serde_json::from_str(&held).unwrap();
        assert_eq!(owner["pid"], std::process::id());
        assert_eq!(owner["token"].as_str().unwrap().len(), 36);
        assert!(names(&scratch.0).is_empty(), "{:?}", names(&scratch.0));
        let failed: Result<(), _> = locked(&config, || Err("no".to_owned()));
        assert_eq!(failed.unwrap_err(), "no");
        assert!(names(&scratch.0).is_empty());
    }

    #[test]
    fn a_lock_held_by_a_running_process_is_waited_for_then_busy() {
        let (scratch, config) = scratch("busy");
        let lock = scratch.0.join("autonomy.yaml.lock");
        let content = format!(r#"{{"pid":{},"token":"t"}}"#, std::process::id());
        std::fs::write(&lock, &content).unwrap();
        let started = std::time::Instant::now();
        let error = locked(&config, || Ok(())).unwrap_err();
        assert!(error.starts_with("update lock is busy: "), "{error}");
        assert!(started.elapsed() >= ATTEMPTS * RETRY);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), content);
        assert_eq!(names(&scratch.0), ["autonomy.yaml.lock"]);
    }

    #[test]
    fn a_lock_whose_process_ended_is_taken_back() {
        let (scratch, config) = scratch("stale");
        let lock = scratch.0.join("autonomy.yaml.lock");
        let pid = ended();
        for content in [format!(r#"{{"pid":{pid},"token":"t"}}"#), pid.to_string()] {
            std::fs::write(&lock, &content).unwrap();
            let held = locked(&config, || Ok(std::fs::read_to_string(&lock).unwrap())).unwrap();
            assert_ne!(held, content);
            assert!(names(&scratch.0).is_empty(), "{:?}", names(&scratch.0));
        }
    }

    #[test]
    fn a_lock_naming_no_process_is_kept_until_it_is_old() {
        let (scratch, config) = scratch("unnamed");
        let lock = scratch.0.join("autonomy.yaml.lock");
        std::fs::write(&lock, "").unwrap();
        assert!(locked(&config, || Ok(())).is_err());
        let old = SystemTime::now() - 2 * INCOMPLETE;
        std::fs::File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(old)
            .unwrap();
        locked(&config, || Ok(())).unwrap();
        assert!(names(&scratch.0).is_empty());
    }

    #[test]
    fn a_claim_whose_process_ended_is_removed_with_its_snapshot() {
        let (scratch, config) = scratch("claim");
        let pid = ended();
        let lock = scratch.0.join("autonomy.yaml.lock");
        std::fs::write(&lock, format!(r#"{{"pid":{pid},"token":"a"}}"#)).unwrap();
        let claim = format!(r#"{{"pid":{pid},"token":"b","lockToken":"a"}}"#);
        std::fs::write(scratch.0.join("autonomy.yaml.lock.reclaim"), claim).unwrap();
        std::fs::write(scratch.0.join("autonomy.yaml.lock.reclaim-snapshot.b"), "").unwrap();
        locked(&config, || Ok(())).unwrap();
        assert!(names(&scratch.0).is_empty(), "{:?}", names(&scratch.0));
    }

    #[test]
    fn prepared_files_of_ended_processes_are_removed_and_others_kept() {
        let (scratch, config) = scratch("orphans");
        let dead = ended();
        let live = std::process::id();
        for (name, pid) in [
            (format!("autonomy.yaml.lock.owner.{dead}.x"), dead),
            (format!("autonomy.yaml.lock.reclaim.owner.{dead}.y"), dead),
            (format!("autonomy.yaml.lock.owner.{live}.z"), live),
        ] {
            let content = format!(r#"{{"pid":{pid},"token":"t"}}"#);
            std::fs::write(scratch.0.join(name), content).unwrap();
        }
        locked(&config, || Ok(())).unwrap();
        assert_eq!(
            names(&scratch.0),
            [format!("autonomy.yaml.lock.owner.{live}.z")]
        );
    }

    #[test]
    fn changes_made_at_once_under_the_lock_are_all_kept() {
        let (scratch, config) = scratch("counter");
        std::fs::write(&config, "0").unwrap();
        std::thread::scope(|threads| {
            for _ in 0..4 {
                threads.spawn(|| {
                    // A loaded machine can keep a thread waiting past the lock's half second:
                    // refused as busy, its change is not made, and it tries again.
                    let mut made = 0;
                    while made < 25 {
                        let changed = locked(&config, || {
                            let n: u32 = std::fs::read_to_string(&config).unwrap().parse().unwrap();
                            std::thread::yield_now();
                            std::fs::write(&config, (n + 1).to_string()).map_err(|e| e.to_string())
                        });
                        match changed {
                            Ok(()) => made += 1,
                            Err(error) => {
                                assert!(error.starts_with("update lock is busy"), "{error}")
                            }
                        }
                    }
                });
            }
        });
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "100");
        assert_eq!(names(&scratch.0), ["autonomy.yaml"]);
    }
}
