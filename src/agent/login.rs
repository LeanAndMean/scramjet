//! scramjet-rs's own credentials, which `/login` saves: `auth.json` in the settings' directory,
//! readable by its owner only and hidden from the agent's tools as other credential stores are
//! (`sandbox::Hidden`). It is in Scramjet's format: an object of the providers logged in to,
//! each `{"type": "oauth", "refresh", "access", "expires"}` (milliseconds since the epoch) with
//! what else its login keeps, or `{"type": "api_key", "key", "origins"}` for a provider of the
//! `models.json` files, whose requests to those servers carry that key in place of the one its
//! entry gives. A process changes it holding a lock on `auth.json.lock`, so that
//! two refreshing a token at once do not undo each other's, and reads it holding the lock shared.
//! It is written in place, as Scramjet writes its own: a sandbox hides it by mounting an empty
//! file over it, and a file renamed over that path would show in every sandbox already running.
//! A full disk leaves it as it was (`write_in_place`), and a token renewed then is held in memory
//! until there is room for it (`HELD`), so that requests go on.
//! What a login lists that is not secret, its models, is kept apart, in the state directory.
//! Besides its own, `/login` lists those packages' programs do (`program`).

mod program;

pub use program::{LoginProgram, Said};

use super::client::{Failure, Fetched};
use super::{codex, copilot};
use crate::os;
use serde_json::{Map, Value};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The providers `/login` logs in to: their names, as providers and in `auth.json`, and what
/// the user knows them by, in the order Scramjet lists them.
pub const LOGINS: [(&str, &str); 2] = [
    (codex::PROVIDER, "ChatGPT Plus/Pro (Codex Subscription)"),
    (copilot::PROVIDER, "GitHub Copilot"),
];
/// What a login fails with when it was cancelled.
pub const CANCELLED: &str = "cancelled";

/// `$XDG_CONFIG_HOME/scramjet`, by default `~/.config/scramjet`: the settings, credentials, and
/// the user's agents, skills, themes and context files.
pub fn config_directory() -> Option<PathBuf> {
    crate::os::base_directory("XDG_CONFIG_HOME", ".config").map(|config| config.join("scramjet"))
}

/// The credentials file.
pub fn auth_file() -> Option<PathBuf> {
    config_directory().map(|directory| directory.join("auth.json"))
}

/// Where `/login` keeps what it saves: the credentials, and the models each login listed; and,
/// beside those lists, the models providers list themselves (`provided`).
#[derive(Clone, Debug, PartialEq)]
pub struct Logins {
    pub auth: PathBuf,
    /// A directory with each login's list, `<provider>.json`.
    pub lists: PathBuf,
}

impl Logins {
    /// The user's: `auth_file()`, and `logins` in the state directory.
    pub fn user() -> Option<Self> {
        Some(Self {
            auth: auth_file()?,
            lists: super::session::state_directory().ok()?.join("logins"),
        })
    }

    /// The models `provider`'s login listed, under a name a package's cannot lead out of.
    pub(super) fn listed(&self, provider: &str) -> PathBuf {
        let name = super::listed::file_name(provider);
        self.lists.join(format!("{name}.json"))
    }

    /// The models `provider`, a provider of the files whose entry lists none, lists itself: in
    /// `models` beside `lists`, under a name its own cannot lead out of.
    pub(super) fn provided(&self, provider: &str) -> PathBuf {
        let name = super::listed::file_name(provider);
        self.lists
            .with_file_name("models")
            .join(format!("{name}.json"))
    }
}

/// The providers with credentials saved in `path`, in `LOGINS`' order, then the others it names.
pub fn logged_in(path: &Path) -> Result<Vec<String>, String> {
    let saved = saved(path)?;
    let mut names: Vec<String> = (LOGINS.iter())
        .map(|(name, _)| name.to_string())
        .filter(|name| saved.contains_key(name))
        .collect();
    let others = saved.keys().filter(|name| !names.contains(name)).cloned();
    names.extend(others.collect::<Vec<_>>());
    Ok(names)
}

/// What the user knows `provider` by.
pub fn shown_name(provider: &str) -> &str {
    let known = LOGINS.iter().find(|(name, _)| *name == provider);
    known.map_or(provider, |(_, shown)| shown)
}

/// The providers with an API key saved in `path`.
pub fn saved_keys(path: &Path) -> Result<Vec<String>, String> {
    let saved = saved(path)?;
    let keyed = saved.keys().filter(|name| key(&saved, name).is_some());
    Ok(keyed.cloned().collect())
}

/// The API key saved in `path` for `provider`.
pub fn saved_key(path: &Path, provider: &str) -> Option<String> {
    let saved = saved(path).ok()?;
    key(&saved, provider).map(|(key, _)| key.to_owned())
}

/// The API key saved among `saved` for `provider`, with the servers it was saved for
/// (`providers::Origin`); none for one saved before keys named them.
pub(super) fn key<'a>(
    saved: &'a Map<String, Value>,
    provider: &str,
) -> Option<(&'a str, Vec<&'a str>)> {
    let credential = saved.get(provider).filter(|c| c["type"] == "api_key")?;
    let origins = credential["origins"].as_array().into_iter().flatten();
    Some((
        credential["key"].as_str()?,
        origins.filter_map(Value::as_str).collect(),
    ))
}

/// Saves `key`, trimmed, in `path` as the API key requests to `provider` carry, to the servers
/// `origins` only: those its entry names now, none of them reached over `http://` but on this
/// machine.
pub fn save_key(path: &Path, provider: &str, key: &str, origins: &[String]) -> Result<(), String> {
    if origins.is_empty() {
        return Err(format!("{provider}'s entry names no server (baseUrl)"));
    }
    if let Some(open) = (origins.iter()).find(|origin| !super::providers::private_to(origin)) {
        return Err(format!(
            "{provider} is reached over http:// at {open}, which would send the key \
             unencrypted; a saved key goes over http:// only to this machine"
        ));
    }
    let key = key.trim();
    if key.is_empty() {
        return Err("the key is empty".to_owned());
    }
    // A space or a line break inside comes of a paste gone wrong; no provider's keys have one.
    // One starting with `!` is a command that prints the key at each request, as in Scramjet.
    if !key.starts_with('!') && !key.chars().all(|ch| ch.is_ascii_graphic()) {
        return Err("an API key is letters, digits and punctuation, without spaces".to_owned());
    }
    let credential = serde_json::json!({"type": "api_key", "key": key, "origins": origins});
    save(path, provider, credential)
}

/// Removes `provider`'s credentials, and what its login listed; returns whether there were any.
pub fn logout(logins: &Logins, provider: &str) -> Result<bool, String> {
    let removed = update::<_, String>(&logins.auth, false, |saved| {
        Ok(saved.remove(provider).is_some())
    })?;
    // What a key's provider listed is asked for again with the key it is left with.
    std::fs::remove_file(logins.listed(provider)).ok();
    if !LOGINS.iter().any(|(login, _)| *login == provider) {
        std::fs::remove_file(logins.provided(provider)).ok();
    }
    Ok(removed)
}

/// The models a login listed, as saved.
#[derive(Clone, Debug, PartialEq)]
pub struct List(pub(super) Value);

impl List {
    /// `provider`'s, unless there is none or it cannot be read.
    pub fn read(logins: &Logins, provider: &str) -> Option<Self> {
        let text = std::fs::read(logins.listed(provider)).ok()?;
        serde_json::from_slice(&text).ok().map(Self)
    }

    /// How long ago it was read; `None` when it does not say.
    pub fn age(&self) -> Option<Duration> {
        let fetched = self.0["fetched"].as_f64()?;
        Some(Duration::from_millis((now_ms() - fetched).max(0.0) as u64))
    }

    /// The models it offers as `provider`'s, as a `models.json` gives them, each with its id.
    pub fn models(&self, provider: &str) -> Vec<Value> {
        let entry = entry(provider, &Value::Null, &self.0);
        let models = entry.and_then(|mut entry| entry["models"].as_array_mut().map(std::mem::take));
        models.unwrap_or_default()
    }
}

/// The provider, as a `models.json` entry, that what `provider`'s login `listed` makes with its
/// `credential`; a package's program lists the entry itself. `None` when there is none.
pub(super) fn entry(provider: &str, credential: &Value, listed: &Value) -> Option<Value> {
    match provider {
        copilot::PROVIDER => Some(copilot::entry(credential, listed)),
        codex::PROVIDER => Some(codex::entry(listed)),
        _ => listed
            .get("provider")
            .filter(|entry| entry.is_object())
            .cloned(),
    }
}

/// Reads again the models `provider`'s login lists, in place of those saved, which stay when
/// that fails; with a warning when they are not quite current.
pub fn relist(logins: &Logins, provider: &str) -> Result<Option<String>, String> {
    match provider {
        copilot::PROVIDER => copilot::relist(logins).map(|()| None),
        codex::PROVIDER => codex::relist(logins),
        _ => Err(format!("{provider} is not a login that lists models")),
    }
}

/// Saves `credential` in `path` as `provider`'s, in place of any it had.
pub(super) fn save(path: &Path, provider: &str, credential: Value) -> Result<(), String> {
    update(path, false, |saved| {
        saved.insert(provider.to_owned(), credential);
        Ok(())
    })
}

/// Scramjet 1's logins in `from` (its `auth.json`, in the same format) that `/login` makes the
/// same way, saved in `to` where it has none of that provider's; nothing saved when `dry`.
/// Returns the logins carried over (or that would be), then those left, each with why.
pub fn import(from: &Path, to: &Path, dry: bool) -> Result<(Vec<String>, Vec<String>), String> {
    let theirs = read(from)?;
    let ours = read(to)?;
    let (mut taken, mut left) = (Map::new(), Vec::new());
    for (provider, credential) in theirs {
        let shown = shown_name(&provider).to_owned();
        let ours_already = ours.contains_key(&provider);
        match credential["type"].as_str() {
            _ if ours_already => left.push(format!("{shown}: logged in here already")),
            Some("oauth") if LOGINS.iter().any(|(name, _)| *name == provider) => {
                taken.insert(provider, credential);
            }
            Some("api_key") => left.push(format!(
                "{shown}'s API key: Scramjet 2 keeps with each key the servers it may go to; \
                 enter it again under /login"
            )),
            _ => left.push(format!("{shown}: not a login Scramjet 2 makes; /login")),
        }
    }
    let names = taken
        .keys()
        .map(|name| shown_name(name).to_owned())
        .collect();
    if !dry && !taken.is_empty() {
        update(to, false, |saved| {
            for (provider, credential) in taken {
                saved.entry(provider).or_insert(credential);
            }
            Ok::<(), String>(())
        })?;
    }
    Ok((names, left))
}

/// What a request to `provider` authenticates with: the access token its login keeps in `path`,
/// renewed first when it has expired, by `program` when a package's does the login. A renewal
/// that failed transiently may succeed if retried.
pub(super) fn access(
    path: &Path,
    provider: &str,
    program: Option<&LoginProgram>,
) -> Result<String, Failure> {
    let shown = program.map_or(shown_name(provider), |program| &program.name);
    access_with(path, provider, shown, |credential| {
        match (program, provider) {
            (Some(program), _) => program.refresh(credential),
            (None, copilot::PROVIDER) => copilot::refresh(credential),
            (None, codex::PROVIDER) => codex::refresh(credential),
            (None, _) => Err(format!("not logged in to {shown} (/login)").into()),
        }
    })
}

/// As `access`, `refresh` buying a token for an expired credential; problems name the login
/// `shown`.
fn access_with(
    path: &Path,
    provider: &str,
    shown: &str,
    refresh: impl FnOnce(&Value) -> Result<Value, Failure>,
) -> Result<String, Failure> {
    let unusable = || Failure::from(format!("not logged in to {shown} (/login)"));
    let current = |saved: &Map<String, Value>| {
        let credential = saved.get(provider).filter(|c| c["type"] == "oauth");
        credential.cloned().ok_or_else(unusable)
    };
    let fresh = |credential: &Value| {
        let token = credential["access"]
            .as_str()
            .filter(|token| !token.is_empty());
        let expires = credential["expires"].as_f64().unwrap_or(0.0);
        token.filter(|_| now_ms() < expires).map(str::to_owned)
    };
    if let Some(token) = fresh(&current(&saved(path)?)?) {
        return Ok(token);
    }
    // Another process may have refreshed it meanwhile: it is looked at again under the lock.
    update(path, true, |saved| {
        let credential = current(saved)?;
        if let Some(token) = fresh(&credential) {
            return Ok(token);
        }
        let bought = refresh(&credential)?;
        // What else the login keeps stays.
        let mut refreshed = credential;
        for (key, value) in bought.as_object().into_iter().flatten() {
            refreshed[key] = value.clone();
        }
        let token = refreshed["access"].as_str().unwrap_or_default().to_owned();
        saved.insert(provider.to_owned(), refreshed);
        Ok(token)
    })
}

/// Makes the credentials file, with no logins, when there is none, as Scramjet does: a sandbox
/// hides only the files that exist when it starts, and a tmux server started in one keeps it.
pub(super) fn make(path: &Path) {
    let directory = path.parent().unwrap_or(Path::new("."));
    let made = os::private_directory(std::fs::DirBuilder::new().recursive(true)).create(directory);
    let file =
        os::private_file(std::fs::OpenOptions::new().write(true).create_new(true)).open(path);
    if let (Ok(()), Ok(mut file)) = (made, file) {
        let _ = file.write_all(b"{}\n");
    }
}

/// Saves what a login listed, `list`, in `path`, replacing it whole: it is no secret.
pub(super) fn save_list(path: &Path, list: &Value) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(failed)?;
    let written = path.with_extension(format!("{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(list).unwrap_or_default() + "\n";
    let saved = std::fs::write(&written, text).and_then(|()| std::fs::rename(&written, path));
    if saved.is_err() {
        let _ = std::fs::remove_file(&written);
    }
    saved.map_err(failed)
}

/// A renewal's JSON answer from `what`, as `answer` reads it, or what failed; transiently when
/// asking again may succeed: no answer, or HTTP 408, 429 or 5xx.
pub(super) fn renewal(fetched: Result<Fetched, Failure>, what: &str) -> Result<Value, Failure> {
    let fetched = fetched?;
    let transient = matches!(fetched.status, 408 | 429 | 500..);
    answer(fetched, what).map_err(|message| Failure::new(message, transient))
}

/// A login's JSON answer from `what`, or what failed.
pub(super) fn answer(fetched: Fetched, what: &str) -> Result<Value, String> {
    if !(200..300).contains(&fetched.status) {
        let said = fetched.said();
        let said = said.trim().chars().take(400).collect::<String>();
        return Err(format!("{what} answered HTTP {}: {said}", fetched.status));
    }
    serde_json::from_slice(&fetched.body)
        .map_err(|error| format!("{what}'s answer is not JSON: {error}"))
}

/// Runs `future` to its end on a runtime of its own: logins run apart from any turn.
pub(super) fn blocking<T, E: From<String>>(
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    runtime
        .map_err(|error| E::from(error.to_string()))?
        .block_on(future)
}

pub(super) fn now_ms() -> f64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    since.as_millis() as f64
}

/// The saved credentials, read holding the lock shared, so that a change under way is not read
/// half made; none when the file does not exist.
pub(super) fn saved(path: &Path) -> Result<Map<String, Value>, String> {
    // Without the lock, in a sandbox that cannot open it, the file is read anyway.
    let lock = std::fs::File::open(lock_file(path)).ok();
    if let Some(lock) = &lock {
        let _ = lock.lock_shared();
    }
    let mut saved = read(path)?;
    let held = held(path);
    drop(lock);
    if !held.is_empty() {
        // Written as soon as the disk has room for them.
        let _ = update(path, false, |_| Ok::<(), String>(()));
        saved.extend(held);
    }
    Ok(saved)
}

/// Credentials renewed while the disk was too full to save them, by `auth.json`'s path: used
/// in place of the file's until written, at the next read or change with room for them.
static HELD: Mutex<BTreeMap<PathBuf, Map<String, Value>>> = Mutex::new(BTreeMap::new());

fn held(path: &Path) -> Map<String, Value> {
    let held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    held.get(path).cloned().unwrap_or_default()
}

fn lock_file(path: &Path) -> PathBuf {
    path.with_file_name("auth.json.lock")
}

/// The credentials in `path`; none when it does not exist or is empty, as it reads in a sandbox.
fn read(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if text.trim_ascii().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_slice(&text) {
        Ok(Value::Object(saved)) => Ok(saved),
        Ok(_) => Err(format!("{}: not a JSON object", path.display())),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Runs `change` on the saved credentials holding the file's lock, and writes them when it
/// changed them or some are held. A file that cannot be read is left as it is. With `hold`, what
/// a full disk cannot take is held in memory (`HELD`) rather than failing the change.
fn update<T, E: From<String>>(
    path: &Path,
    hold: bool,
    change: impl FnOnce(&mut Map<String, Value>) -> Result<T, E>,
) -> Result<T, E> {
    let failed = |error: std::io::Error| E::from(format!("{}: {error}", path.display()));
    let directory = path.parent().unwrap_or(Path::new("."));
    let made = os::private_directory(std::fs::DirBuilder::new().recursive(true)).create(directory);
    made.map_err(failed)?;
    // Windows locks only a handle that can read or write; one that only appends cannot.
    let lock = os::private_file(
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false),
    )
    .open(lock_file(path))
    .map_err(failed)?;
    lock.lock().map_err(failed)?;
    let file = read(path)?;
    let held = held(path);
    let mut saved = file.clone();
    saved.extend(held.clone());
    let before = saved.clone();
    let result = change(&mut saved)?;
    if saved == before && held.is_empty() {
        return Ok(result);
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(saved.clone())).unwrap_or_default();
    text.push('\n');
    let mut all_held = HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match write_in_place(path, text.as_bytes()) {
        Ok(()) => {
            all_held.remove(path);
        }
        Err(error) if hold && super::session::full(&error) => {
            let changed = saved
                .into_iter()
                .filter(|(name, credential)| file.get(name) != Some(credential));
            all_held.insert(path.to_owned(), changed.collect());
        }
        Err(error) => return Err(failed(error)),
    }
    Ok(result)
}

/// Writes `text` over the contents of `path`, in place (the module says why), once the disk has
/// given it room (`os::overwrite`).
fn write_in_place(path: &Path, text: &[u8]) -> std::io::Result<()> {
    let mut file = os::private_file(
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false),
    )
    .open(path)?;
    os::make_private(&file)?;
    os::overwrite(&mut file, text)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;
    use serde_json::json;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> ScratchDir {
        let path = std::env::temp_dir().join(format!("scramjet-{name}-{}", std::process::id()));
        ScratchDir::new(path)
    }

    #[test]
    fn credentials_are_saved_for_their_owner_only_and_removed_by_provider() {
        let scratch = scratch("login-store");
        let path = scratch.0.join("config/scramjet-rs/auth.json");
        update::<_, String>(&path, false, |saved| {
            saved.insert(
                "github-copilot".into(),
                json!({"type": "oauth", "access": "a"}),
            );
            saved.insert("other".into(), json!({"type": "oauth", "access": "b"}));
            Ok(())
        })
        .unwrap();
        #[cfg(unix)]
        {
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        assert_eq!(saved(&path).unwrap().len(), 2);
        let removed =
            update::<_, String>(&path, false, |saved| Ok(saved.remove("other").is_some()));
        let removed = removed.unwrap();
        assert!(removed);
        let saved = saved(&path).unwrap();
        assert_eq!(
            Value::Object(saved),
            json!({"github-copilot": {"type": "oauth", "access": "a"}})
        );
        // Only the file and its lock are left: it is written in place.
        let mut names: Vec<String> = (std::fs::read_dir(path.parent().unwrap()).unwrap())
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["auth.json", "auth.json.lock"]);
    }

    /// Runs in a mount namespace of its own, where `SCRAMJET_TEST_FULL_DIR` is a tmpfs that this
    /// test fills; outside, it starts itself there, and passes where that cannot be done.
    #[test]
    fn a_full_disk_keeps_the_saved_logins_and_holds_a_renewed_token_until_there_is_room() {
        const NAME: &str = "agent::login::tests::\
            a_full_disk_keeps_the_saved_logins_and_holds_a_renewed_token_until_there_is_room";
        let Some(directory) = std::env::var_os("SCRAMJET_TEST_FULL_DIR") else {
            let scratch = scratch("login-full");
            std::fs::create_dir_all(&scratch.0).unwrap();
            let script = r#"mount -t tmpfs -o size=64k tmpfs "$1" || exit 77
                SCRAMJET_TEST_FULL_DIR="$1" exec "$2" --exact "$3" --nocapture"#;
            let status = std::process::Command::new("unshare")
                .args(["-r", "-m", "sh", "-c", script, "sh"])
                .arg(&scratch.0)
                .arg(std::env::current_exe().unwrap())
                .arg(NAME)
                .status();
            match status.map(|status| status.code()) {
                Ok(Some(0)) => {}
                Ok(Some(101)) => panic!("the test failed on a full tmpfs"),
                other => eprintln!("skipped: no tmpfs of its own ({other:?})"),
            }
            return;
        };
        let directory = PathBuf::from(directory);
        let path = directory.join("auth.json");
        let expired = json!({"type": "oauth", "refresh": "gho", "access": "old", "expires": 0});
        save(&path, "github-copilot", expired).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let mut fill = std::fs::File::create(directory.join("fill")).unwrap();
        while fill.write_all(&[0; 4096]).is_ok() {}
        // Larger than the page the file has, so that it needs room the disk does not have.
        let access = "new".repeat(3000);
        let bought = json!({"access": access, "expires": now_ms() + 60_000.0});
        let token = access_with(&path, "github-copilot", "GitHub Copilot", |_| {
            Ok(bought.clone())
        });
        assert_eq!(token.unwrap(), access);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert_eq!(saved(&path).unwrap()["github-copilot"]["access"], access);
        let token = access_with(
            &path,
            "github-copilot",
            "GitHub Copilot",
            |_| unreachable!(),
        );
        assert_eq!(token.unwrap(), access);
        // A login is not held: it says it was not saved.
        let other = json!({"type": "api_key", "key": "k".repeat(5000)});
        let failed = save(&path, "other", other).unwrap_err();
        assert!(failed.contains("No space left on device"), "{failed}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        drop(fill);
        std::fs::remove_file(directory.join("fill")).unwrap();
        let saved = saved(&path).unwrap();
        assert_eq!(saved["github-copilot"]["access"], access);
        assert!(saved.get("other").is_none());
        let file: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(file["github-copilot"]["access"], access);
        assert_eq!(file["github-copilot"]["refresh"], "gho");
        assert!(held(&path).is_empty());
    }

    #[test]
    fn an_expired_token_is_refreshed_once_and_saved_with_what_the_login_keeps() {
        let scratch = scratch("login-refresh");
        let path = scratch.0.join("auth.json");
        let expired = json!({"type": "oauth", "refresh": "gho", "access": "old",
            "expires": now_ms() - 1.0, "enterpriseUrl": "company.ghe.com"});
        save(&path, "github-copilot", expired).unwrap();
        let bought = json!({"type": "oauth", "refresh": "gho", "access": "new",
            "expires": now_ms() + 60_000.0});
        let mut refreshes = 0;
        let mut refresh = |credential: &Value| {
            refreshes += 1;
            assert_eq!(credential["refresh"], "gho");
            Ok(bought.clone())
        };
        let token = access_with(&path, "github-copilot", "GitHub Copilot", &mut refresh).unwrap();
        assert_eq!(token, "new");
        let credential = &saved(&path).unwrap()["github-copilot"];
        assert_eq!(credential["access"], "new");
        assert_eq!(credential["enterpriseUrl"], "company.ghe.com");
        // Fresh now, it is used as it is.
        let token = access_with(&path, "github-copilot", "GitHub Copilot", &mut refresh).unwrap();
        assert_eq!((token.as_str(), refreshes), ("new", 1));
        let failed = access_with(&path, "other", "other", |_| unreachable!()).unwrap_err();
        assert_eq!(failed.message, "not logged in to other (/login)");
        assert!(!failed.transient);
        // A refresh that fails leaves the login as it was, and says whether to try again.
        let expired = json!({"type": "oauth", "refresh": "gho", "access": "old", "expires": 0});
        save(&path, "github-copilot", expired.clone()).unwrap();
        for transient in [false, true] {
            let failure = Failure::new("HTTP 5xx or 401", transient);
            let failed = access_with(&path, "github-copilot", "GitHub Copilot", |_| Err(failure))
                .unwrap_err();
            assert_eq!(
                (failed.message.as_str(), failed.transient),
                ("HTTP 5xx or 401", transient)
            );
            assert_eq!(saved(&path).unwrap()["github-copilot"], expired);
        }
    }

    #[test]
    fn a_renewal_that_asking_again_may_cure_fails_transiently() {
        let fetched = |status, body: &str| {
            let body = body.as_bytes().to_vec();
            Ok(Fetched {
                status,
                body,
                location: None,
                secrets: Vec::new(),
            })
        };
        for (status, transient) in [(408, true), (429, true), (500, true), (503, true)]
            .into_iter()
            .chain([(400, false), (401, false), (403, false), (404, false)])
        {
            let failed = renewal(
                fetched(status, r#"{"error":"invalid_grant"}"#),
                "The endpoint",
            );
            let failed = failed.unwrap_err();
            assert_eq!(failed.transient, transient, "{status}");
            let said =
                format!("The endpoint answered HTTP {status}: {{\"error\":\"invalid_grant\"}}");
            assert_eq!(failed.message, said);
        }
        let unanswered = Failure::new("x gave no answer within 30s", true);
        let unanswered = renewal(Err(unanswered), "The endpoint");
        assert!(unanswered.unwrap_err().transient);
        assert!(
            !renewal(fetched(200, "<html>"), "The endpoint")
                .unwrap_err()
                .transient
        );
        let answered = renewal(fetched(200, r#"{"token":"t"}"#), "The endpoint").unwrap();
        assert_eq!(answered["token"], "t");
    }

    #[test]
    #[cfg_attr(windows, ignore = "needs the sandbox, which Windows lacks")]
    fn a_sandbox_running_while_a_login_is_saved_still_sees_the_file_empty() {
        use crate::agent::sandbox;
        let scratch = scratch("login-hidden");
        let path = scratch.0.join("config/auth.json");
        make(&path);
        let hidden = sandbox::Hidden::under(&scratch.0, vec![path.clone()]);
        let script = format!(
            "cat {0}; : > ready; while [ ! -e go ]; do sleep 0.02; done; cat {0}; echo end",
            path.display()
        );
        let running = sandbox::hiding(&scratch.0, &script, &hidden)
            .unwrap()
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        while !scratch.0.join("ready").exists() {
            assert!(
                started.elapsed().as_secs() < 10,
                "the sandbox did not start"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let credential = json!({"type": "oauth", "access": "SECRET"});
        save(&path, "github-copilot", credential).unwrap();
        std::fs::write(scratch.0.join("go"), "").unwrap();
        let output = running.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "end\n");
        assert_eq!(saved(&path).unwrap()["github-copilot"]["access"], "SECRET");
    }

    #[test]
    fn the_file_is_made_without_logins_and_kept_for_its_owner() {
        let scratch = scratch("login-made");
        let path = scratch.0.join("config/auth.json");
        make(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
        #[cfg(unix)]
        {
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode(&path), mode(path.parent().unwrap())), (0o600, 0o700));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            save(&path, "github-copilot", json!({"type": "oauth"})).unwrap();
            assert_eq!(mode(&path), 0o600);
        }
        #[cfg(not(unix))]
        save(&path, "github-copilot", json!({"type": "oauth"})).unwrap();
        make(&path);
        assert_eq!(logged_in(&path).unwrap(), ["github-copilot"]);
        // Empty, as a sandbox shows it, it holds no logins.
        std::fs::write(&path, "").unwrap();
        assert!(saved(&path).unwrap().is_empty());
    }

    #[test]
    fn api_keys_are_saved_trimmed_and_refused_when_they_cannot_be_sent() {
        let scratch = scratch("login-keys");
        let path = scratch.0.join("auth.json");
        save(
            &path,
            "github-copilot",
            json!({"type": "oauth", "access": "a"}),
        )
        .unwrap();
        let server = ["https://openrouter.ai:443".to_owned()];
        save_key(&path, "openrouter", "  sk-or-v1-abc\n", &server).unwrap();
        let saved = saved(&path).unwrap();
        assert_eq!(
            saved["openrouter"],
            json!({"type": "api_key", "key": "sk-or-v1-abc", "origins": server})
        );
        let only = Some(("sk-or-v1-abc", vec!["https://openrouter.ai:443"]));
        assert_eq!(key(&saved, "openrouter"), only);
        assert_eq!(key(&saved, "github-copilot"), None);
        assert_eq!(saved_keys(&path).unwrap(), ["openrouter"]);
        assert_eq!(logged_in(&path).unwrap(), ["github-copilot", "openrouter"]);
        for (refused, why) in [
            ("  ", "the key is empty"),
            ("sk-or\nv1", "without spaces"),
            ("sk or", "without spaces"),
            ("sk-é", "without spaces"),
        ] {
            let error = save_key(&path, "openrouter", refused, &server).unwrap_err();
            assert!(error.contains(why), "{refused:?}: {error}");
        }
        // Over plain http only to this machine, and only to a server the entry names.
        for (servers, why) in [
            (
                &["https://a:443", "http://10.0.0.5:80"][..],
                "at http://10.0.0.5:80, which",
            ),
            (
                &["http://localhost.example:80"],
                "at http://localhost.example:80",
            ),
            (&[], "names no server (baseUrl)"),
        ] {
            let servers: Vec<String> = servers.iter().map(|s| s.to_string()).collect();
            let error = save_key(&path, "openrouter", "sk", &servers).unwrap_err();
            assert!(error.contains(why), "{servers:?}: {error}");
        }
        let local = [
            "http://localhost:80",
            "http://127.0.0.2:8",
            "http://[::1]:9",
        ];
        let local: Vec<String> = local.iter().map(|s| s.to_string()).collect();
        save_key(&path, "local", "sk", &local).unwrap();
        assert_eq!(key(&super::saved(&path).unwrap(), "openrouter"), only);
        // A command that prints the key, as typed.
        save_key(&path, "local", " !pass show local key\n", &local).unwrap();
        let saved = super::saved(&path).unwrap();
        assert_eq!(key(&saved, "local").unwrap().0, "!pass show local key");
        let logins = Logins {
            auth: path.clone(),
            lists: scratch.0.join("logins"),
        };
        assert!(logout(&logins, "openrouter").unwrap());
        assert_eq!(saved_keys(&path).unwrap(), ["local"]);
        // A name from a models.json file is not a file's: removing its key removes nothing else.
        std::fs::create_dir_all(&logins.lists).unwrap();
        let outside = scratch.0.join("models.json");
        std::fs::write(&outside, "{}").unwrap();
        save_key(&path, "../models", "sk", &server).unwrap();
        assert!(logout(&logins, "../models").unwrap());
        assert!(outside.exists());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_as_it_is() {
        let scratch = scratch("login-broken");
        let path = scratch.0.join("auth.json");
        std::fs::write(&path, "{not json").unwrap();
        let changed = update::<_, String>(&path, false, |saved| {
            saved.insert("github-copilot".into(), json!({}));
            Ok(())
        });
        assert!(changed.unwrap_err().contains("auth.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        assert!(saved(&scratch.0.join("missing.json")).unwrap().is_empty());
    }
}
