//! Lines a session keeps in memory for want of room (a full disk), copied into a private
//! directory that lives in memory (`$XDG_RUNTIME_DIR`, else `/dev/shm`), so that they outlive
//! the process however it ends, a signal or a kill included, and a later run writes them to
//! their session once there is room. Each copy stays locked while its process lives.

use super::{full, make_directory, write_whole};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
};

const SUFFIX: &str = ".kept";

/// The directory of the copies, where there is one to use.
#[cfg(not(test))]
pub(super) fn directory() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    if let Some(runtime) = runtime.filter(|runtime| runtime.is_absolute() && runtime.is_dir()) {
        return Some(runtime.join("scramjet-rs-kept"));
    }
    #[cfg(unix)]
    {
        let shm = Path::new("/dev/shm");
        let user = rustix::process::geteuid().as_raw();
        shm.is_dir()
            .then(|| shm.join(format!("scramjet-rs-kept-{user}")))
    }
    #[cfg(not(unix))]
    None
}

#[cfg(test)]
thread_local! {
    static TEST_DIRECTORY: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// In tests, none but the one a test names for its thread.
#[cfg(test)]
pub(super) fn directory() -> Option<PathBuf> {
    TEST_DIRECTORY.with(|directory| directory.borrow().clone())
}

/// Has this thread's sessions keep their lines in `directory`, whatever their path.
#[cfg(test)]
pub(in crate::agent) fn keep_in(directory: &Path) {
    TEST_DIRECTORY.with(|kept| *kept.borrow_mut() = Some(directory.to_owned()));
}

/// Where the session saved at `session` copies its kept lines: only a saved session's, not one
/// of a `--no-session` run, which goes with its directory.
pub(super) fn directory_for(session: &Path) -> Option<PathBuf> {
    if super::Unsaved::holds(session) {
        return None;
    }
    directory()
}

/// The copy of one session's kept lines, locked while this process holds it.
pub(in crate::agent) struct Kept {
    path: PathBuf,
    file: File,
}

impl Kept {
    /// A new copy for the session `session` in `directory`, locked before it can be found.
    pub(super) fn create(directory: &Path, session: &Path) -> io::Result<Self> {
        make_directory(directory)?;
        let name = session.file_name().unwrap_or_default().to_string_lossy();
        let id = std::process::id();
        for n in 0u32.. {
            let making = directory.join(format!("{name}.{id}-{n}.making"));
            let opened =
                crate::os::private_file(OpenOptions::new().read(true).write(true).create_new(true))
                    .open(&making);
            let file = match opened {
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                opened => opened?,
            };
            let path = directory.join(format!("{name}.{id}-{n}{SUFFIX}"));
            let made = file.lock().and_then(|()| match path.exists() {
                true => Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                false => std::fs::rename(&making, &path),
            });
            match made {
                Ok(()) => return Ok(Self { path, file }),
                Err(error) => {
                    let _ = std::fs::remove_file(&making);
                    if error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(error);
                    }
                }
            }
        }
        unreachable!("a free name")
    }

    /// Makes the copy hold `held`, the lines to go at byte `size` of the session's file `session`.
    pub(super) fn keep(&mut self, session: &Path, size: u64, held: &[u8]) -> io::Result<()> {
        let mut header = serde_json::to_vec(&json!({"session": session, "size": size}))?;
        header.push(b'\n');
        self.file.set_len(0)?;
        self.file.rewind()?;
        self.file.write_all(&header)?;
        self.file.write_all(held)
    }

    /// Removes the copy, its lines written or no longer wanted.
    pub(super) fn discard(self) {
        let _ = std::fs::remove_file(&self.path);
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

/// A copy a run that ended left, taken by this process.
pub(super) struct Taken {
    pub kept: Kept,
    pub session: PathBuf,
    pub size: u64,
    pub lines: Vec<u8>,
}

/// The copy at `path` unless its process still runs (it holds the lock) or it is gone.
fn take(path: &Path) -> io::Result<Option<Taken>> {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        opened => opened?,
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    // Written out by another run between the open and the lock.
    if crate::os::removed(&file)? {
        return Ok(None);
    }
    let mut reader = BufReader::new(&file);
    let mut header = String::new();
    reader.read_line(&mut header)?;
    let header: Value = serde_json::from_str(&header)
        .map_err(|error| io::Error::other(format!("not a copy of kept lines: {error}")))?;
    let (Some(session), Some(size)) = (header["session"].as_str(), header["size"].as_u64()) else {
        return Err(io::Error::other("not a copy of kept lines"));
    };
    let mut lines = Vec::new();
    reader.read_to_end(&mut lines)?;
    let kept = Kept {
        path: path.to_owned(),
        file,
    };
    Ok(Some(Taken {
        kept,
        session: PathBuf::from(session),
        size,
        lines,
    }))
}

/// Writes a taken copy's lines at the end of its session's file, if the file is as the copy left
/// it, and removes the copy; on failure the file is as it was and the copy stays.
fn write_out(taken: Taken) -> Result<(PathBuf, usize), (Taken, io::Error)> {
    let written = (|| {
        let directory = taken.session.parent().unwrap_or(Path::new("."));
        make_directory(directory)?;
        let _lock = crate::os::lock_directory(directory)?;
        let found = match std::fs::metadata(&taken.session) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        if found != taken.size {
            return Err(io::Error::other(format!(
                "the session's file changed since ({found} bytes, not {})",
                taken.size
            )));
        }
        let mut file =
            crate::os::private_file(OpenOptions::new().append(true).create(taken.size == 0))
                .open(&taken.session)?;
        if let Err(error) = write_whole(&mut file, &taken.lines) {
            let _ = crate::os::truncate(&file, &taken.session, taken.size);
            return Err(error);
        }
        Ok(())
    })();
    match written {
        Ok(()) => {
            let len = taken.lines.len();
            taken.kept.discard();
            Ok((taken.session, len))
        }
        Err(error) => Err((taken, error)),
    }
}

/// What `write_kept` did with the copies runs that ended left.
#[derive(Debug, Default)]
pub struct Written {
    /// The sessions written, each with how many bytes went in.
    pub sessions: Vec<(PathBuf, usize)>,
    /// How many still wait for room.
    pub waiting: usize,
    /// Those that cannot be written, said.
    pub problems: Vec<String>,
}

/// The copies in `directory`, of the session file named `name` only when given.
fn copies(directory: &Path, name: Option<&str>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let file = path.file_name().unwrap_or_default().to_string_lossy();
            file.ends_with(SUFFIX) && name.is_none_or(|name| file.starts_with(&format!("{name}.")))
        })
        .collect();
    found.sort();
    found
}

/// Writes to their sessions the lines that runs which ended kept for want of room.
pub fn write_kept() -> Written {
    let mut written = Written::default();
    let Some(directory) = directory() else {
        return written;
    };
    for path in copies(&directory, None) {
        match take(&path) {
            Ok(None) => {}
            Ok(Some(taken)) => match write_out(taken) {
                Ok(session) => written.sessions.push(session),
                Err((_, error)) if full(&error) => written.waiting += 1,
                Err((taken, error)) => written.problems.push(format!(
                    "Error: session {} not written: {error}; the lines an earlier run kept for it \
                     are in {}",
                    taken.session.display(),
                    path.display()
                )),
            },
            Err(error) => written
                .problems
                .push(format!("Error: {}: {error}", path.display())),
        }
    }
    written
}

/// The copy of `session`'s kept lines that a run which ended left, its lines written there when
/// there is room now; one left for want of room is taken over, to be kept on with this run's, with
/// why it could not be written.
pub(super) fn adopt(session: &Path) -> Option<(Taken, io::Error)> {
    let directory = directory()?;
    let name = session.file_name()?.to_string_lossy().into_owned();
    for path in copies(&directory, Some(&name)) {
        let Ok(Some(taken)) = take(&path) else {
            continue;
        };
        if taken.session != session {
            continue;
        }
        match write_out(taken) {
            Ok(_) => {}
            Err((taken, error)) if full(&error) => return Some((taken, error)),
            Err(_) => {}
        }
    }
    None
}

/// Removes the copies runs that ended left of `session`'s lines, the session deleted.
pub(super) fn forget(session: &Path) {
    let Some(directory) = directory() else {
        return;
    };
    let Some(name) = session.file_name() else {
        return;
    };
    for path in copies(&directory, Some(&name.to_string_lossy())) {
        if let Ok(Some(taken)) = take(&path)
            && taken.session == session
        {
            taken.kept.discard();
        }
    }
}
