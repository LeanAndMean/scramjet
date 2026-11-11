//! The waits providers asked for with Retry-After, shared by every request sent to the same
//! endpoint and model, in this process and in the user's other scramjet-rs processes: once one
//! is told to wait, the others wait too rather than being refused in turn. They then start one
//! after another, `GAP` apart, rather than all at the wait's end. Each wait is kept in a small
//! locked file in a private directory (`$XDG_RUNTIME_DIR`, else the temporary directory); where
//! none can be used, in this process only.

use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::PathBuf,
    sync::{LazyLock, Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Between two requests' starts after a shared wait, in milliseconds.
const GAP: u64 = 250;

/// An endpoint, by host, port and path, and a model there.
type Key = (String, u16, String, String);

/// The wait's end, and the first start not yet given to a request, in milliseconds since the
/// Unix epoch, which other processes read alike.
#[derive(Clone, Copy)]
struct Held {
    end: u64,
    next: u64,
}

static HELD: LazyLock<Mutex<HashMap<Key, Held>>> = LazyLock::new(Mutex::default);

fn key(route: &super::providers::Route, model: &str) -> Key {
    let (host, path) = (route.host.clone(), route.path.clone());
    (host, route.port, path, model.to_owned())
}

fn now() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |since| since.as_millis().try_into().unwrap_or(u64::MAX))
}

/// The directory of the waits' files.
#[cfg(not(test))]
fn directory() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    if let Some(runtime) = runtime.filter(|runtime| runtime.is_absolute() && runtime.is_dir()) {
        return Some(runtime.join("scramjet-cooldown"));
    }
    #[cfg(unix)]
    let name = format!("scramjet-cooldown-{}", rustix::process::geteuid().as_raw());
    #[cfg(not(unix))]
    let name = "scramjet-cooldown".to_owned();
    Some(std::env::temp_dir().join(name))
}

#[cfg(test)]
thread_local! {
    static TEST_DIRECTORY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// In tests, none but the one a test names for its thread.
#[cfg(test)]
fn directory() -> Option<PathBuf> {
    TEST_DIRECTORY.with(|directory| directory.borrow().clone())
}

/// Opens and locks the file of `key`'s wait, made if `create`; `None` when there is none.
fn open(key: &Key, create: bool) -> io::Result<Option<File>> {
    let Some(directory) = directory() else {
        return Err(io::ErrorKind::Unsupported.into());
    };
    let (host, port, path, model) = key;
    let named = Sha256::digest(format!("{host}\0{port}\0{path}\0{model}"));
    let name: String = named[..12].iter().map(|b| format!("{b:02x}")).collect();
    let path = directory.join(name);
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    let file = match crate::os::private_file(options.create(create)).open(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            super::session::make_directory(&directory)?;
            options.open(&path)?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        opened => opened?,
    };
    file.lock()?;
    Ok(Some(file))
}

fn read(file: &mut File) -> io::Result<Option<Held>> {
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut numbers = text.split_whitespace().map(str::parse);
    Ok(match (numbers.next(), numbers.next()) {
        (Some(Ok(end)), Some(Ok(next))) => Some(Held { end, next }),
        _ => None,
    })
}

fn write(file: &mut File, held: Held) -> io::Result<()> {
    file.rewind()?;
    file.set_len(0)?;
    file.write_all(format!("{} {}\n", held.end, held.next).as_bytes())
}

/// Runs `change` on what is held for `key`, in its file where one can be used, else in this
/// process's table, and keeps what it leaves. Without a file, nothing is made unless `create`.
fn with_held<R>(key: Key, create: bool, change: impl FnOnce(&mut Option<Held>) -> R) -> R {
    let filed = open(&key, create).and_then(|file| {
        let Some(mut file) = file else {
            return Ok(None);
        };
        let held = read(&mut file)?;
        Ok(Some((file, held)))
    });
    match filed {
        Ok(None) => change(&mut None),
        Ok(Some((mut file, mut held))) => {
            let was = held.map(|held| (held.end, held.next));
            let result = change(&mut held);
            if let Some(held) = held.filter(|held| was != Some((held.end, held.next))) {
                // Not kept for the others, it still holds this request.
                let _ = write(&mut file, held);
            }
            result
        }
        Err(_) => {
            let mut table = HELD.lock().unwrap_or_else(PoisonError::into_inner);
            let now = now();
            table.retain(|_, held| held.end > now);
            let mut held = table.get(&key).copied();
            let result = change(&mut held);
            if let Some(held) = held {
                table.insert(key, held);
            }
            result
        }
    }
}

/// Keeps `wait`, from now, as the one requests to `model` on `route` keep, unless a later end
/// is kept already. A refused request that `retries` at the end takes its first start.
pub(super) fn hold(route: &super::providers::Route, model: &str, wait: Duration, retries: bool) {
    let wait: u64 = wait.as_millis().try_into().unwrap_or(u64::MAX);
    let end = now().saturating_add(wait);
    with_held(key(route, model), true, |held| {
        let held = match held {
            Some(held) if held.end > now() => held,
            _ => held.insert(Held { end, next: end }),
        };
        held.end = held.end.max(end);
        held.next = held.next.max(held.end);
        if retries {
            held.next += GAP;
        }
    });
}

/// How long a request to `model` on `route` is to wait before it starts, if at all, taking
/// that start; none is taken when the wait left is over `max_wait`.
pub(super) fn take(
    route: &super::providers::Route,
    model: &str,
    max_wait: Duration,
) -> Option<Duration> {
    with_held(key(route, model), false, |held| {
        let held = held.as_mut()?;
        let now = now();
        if held.end <= now || Duration::from_millis(held.end - now) > max_wait {
            return None;
        }
        let start = held.next.max(held.end);
        held.next = start + GAP;
        Some(Duration::from_millis(start - now))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> super::super::providers::Route {
        super::super::providers::Route {
            provider: "local".into(),
            tls: false,
            host: "127.0.0.1".into(),
            port: 9,
            authority: "127.0.0.1:9".into(),
            path: "/v1".into(),
            headers: Vec::new(),
            commanded: Vec::new(),
            login: None,
        }
    }

    #[test]
    fn requests_held_by_one_wait_start_a_gap_apart_after_the_refused_one() {
        let (route, long) = (route(), Duration::from_secs(60));
        hold(&route, "spaced", Duration::from_secs(1), true);
        let starts: Vec<Duration> = (0..3)
            .map(|_| take(&route, "spaced", long).unwrap())
            .collect();
        let near = |start: Duration, millis: u64| {
            let expected = Duration::from_millis(millis);
            start <= expected && expected - start < Duration::from_millis(100)
        };
        assert!(near(starts[0], 1250), "{starts:?}");
        assert!(near(starts[1], 1500), "{starts:?}");
        assert!(near(starts[2], 1750), "{starts:?}");
        // One that will not retry leaves the first start to the others.
        hold(&route, "unclaimed", Duration::from_secs(1), false);
        assert!(near(take(&route, "unclaimed", long).unwrap(), 1000));
        // A wait over the settings' limit is not waited for, and takes no start.
        assert_eq!(take(&route, "unclaimed", Duration::from_millis(500)), None);
        assert!(near(take(&route, "unclaimed", long).unwrap(), 1250));
        assert_eq!(take(&route, "never held", long), None);
    }

    #[test]
    fn a_longer_wait_given_meanwhile_moves_the_starts_after_it() {
        let (route, long) = (route(), Duration::from_secs(60));
        hold(&route, "extended", Duration::from_secs(1), false);
        hold(&route, "extended", Duration::from_secs(3), true);
        let start = take(&route, "extended", long).unwrap();
        assert!(start > Duration::from_millis(3150) && start <= Duration::from_millis(3250));
        // A shorter one does not shorten it.
        hold(&route, "extended", Duration::from_secs(1), false);
        assert!(take(&route, "extended", long).unwrap() > Duration::from_millis(3400));
    }

    #[test]
    fn a_wait_is_shared_through_its_file_with_other_processes() {
        let scratch =
            std::env::temp_dir().join(format!("scramjet-cooldown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let scratch = super::super::tests::ScratchDir::new(scratch);
        let directory = scratch.0.join("cooldown");
        let (route, long) = (route(), Duration::from_secs(60));
        // Each thread names its own directory, as another process finds it.
        let shared = |directory: Option<PathBuf>, f: fn(&super::super::providers::Route)| {
            let route = route.clone();
            std::thread::spawn(move || {
                TEST_DIRECTORY.with(|named| *named.borrow_mut() = directory);
                f(&route);
            })
            .join()
            .unwrap();
        };
        shared(Some(directory.clone()), |route| {
            hold(route, "filed", Duration::from_secs(2), true)
        });
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        shared(Some(directory.clone()), |route| {
            let start = take(route, "filed", Duration::from_secs(60)).unwrap();
            assert!(start > Duration::from_millis(2100) && start <= Duration::from_millis(2250));
            let after = take(route, "filed", Duration::from_secs(60)).unwrap();
            assert!(after > Duration::from_millis(2350) && after <= Duration::from_millis(2500));
            assert_eq!(take(route, "unheld", Duration::from_secs(60)), None);
        });
        // A process without the directory keeps its own.
        shared(None, |route| {
            assert_eq!(take(route, "filed", Duration::from_secs(60)), None)
        });
        assert_eq!(take(&route, "filed", long), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&directory).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
    }
}
