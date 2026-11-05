//! scramjet-rs's own credentials, which `/login` saves: `auth.json` in the settings' directory,
//! readable by its owner only and hidden from the agent's tools as other credential stores are
//! (`sandbox::Hidden`). It is in Scramjet's format: an object of the providers logged in to,
//! each `{"type": "oauth", "refresh", "access", "expires"}` (milliseconds since the epoch) with
//! what else its login keeps, or `{"type": "api_key", "key"}` for a provider of the `models.json`
//! files, whose requests carry that key in place of the one its entry gives. A process changes it holding a lock on `auth.json.lock`, so that
//! two refreshing a token at once do not undo each other's, and reads it holding the lock shared.
//! It is written in place, as Scramjet writes its own: a sandbox hides it by mounting an empty
//! file over it, and a file renamed over that path would show in every sandbox already running.
//! What a login lists that is not secret, its models, is kept apart, in the state directory.

use super::client::Fetched;
use super::{codex, copilot};
use serde_json::{Map, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
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

/// `$XDG_CONFIG_HOME/scramjet-rs`, by default `~/.config/scramjet-rs`: where the settings are.
pub fn config_directory() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|config| !config.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    config.map(|config| config.join("scramjet-rs"))
}

/// The credentials file.
pub fn auth_file() -> Option<PathBuf> {
    config_directory().map(|directory| directory.join("auth.json"))
}

/// Where `/login` keeps what it saves: the credentials, and the models each login listed.
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

    /// The models `provider`'s login listed.
    pub(super) fn listed(&self, provider: &str) -> PathBuf {
        self.lists.join(format!("{provider}.json"))
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

/// The API key saved among `saved` for `provider`.
pub(super) fn key<'a>(saved: &'a Map<String, Value>, provider: &str) -> Option<&'a str> {
    let credential = saved.get(provider).filter(|c| c["type"] == "api_key")?;
    credential["key"].as_str()
}

/// Saves `key`, trimmed, in `path` as the API key requests to `provider` carry.
pub fn save_key(path: &Path, provider: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("the key is empty".to_owned());
    }
    // Read as configured values are, it would be a command.
    if key.starts_with('!') {
        return Err("a key starting with ! is a shell command, which is not run yet".to_owned());
    }
    // A space or a line break inside comes of a paste gone wrong; no provider's keys have one.
    if !key.chars().all(|ch| ch.is_ascii_graphic()) {
        return Err("an API key is letters, digits and punctuation, without spaces".to_owned());
    }
    let credential = serde_json::json!({"type": "api_key", "key": key});
    save(path, provider, credential)
}

/// Removes `provider`'s credentials, and what its login listed; returns whether there were any.
pub fn logout(logins: &Logins, provider: &str) -> Result<bool, String> {
    let removed = update(&logins.auth, |saved| Ok(saved.remove(provider).is_some()))?;
    // Only logins list models; a key's provider is named by a models.json file, not a file name.
    if LOGINS.iter().any(|(login, _)| *login == provider) {
        let _ = std::fs::remove_file(logins.listed(provider));
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
/// `credential`; `None` for a provider `/login` does not log in to.
pub(super) fn entry(provider: &str, credential: &Value, listed: &Value) -> Option<Value> {
    match provider {
        copilot::PROVIDER => Some(copilot::entry(credential, listed)),
        codex::PROVIDER => Some(codex::entry(listed)),
        _ => None,
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
    update(path, |saved| {
        saved.insert(provider.to_owned(), credential);
        Ok(())
    })
}

/// What a request to `provider` authenticates with: the access token its login keeps in `path`,
/// renewed first when it has expired.
pub(super) fn access(path: &Path, provider: &str) -> Result<String, String> {
    access_with(path, provider, |credential| match provider {
        copilot::PROVIDER => copilot::refresh(credential),
        codex::PROVIDER => codex::refresh(credential),
        _ => Err(format!(
            "not logged in to {} (/login)",
            shown_name(provider)
        )),
    })
}

/// As `access`, `refresh` buying a token for an expired credential.
fn access_with(
    path: &Path,
    provider: &str,
    refresh: impl FnOnce(&Value) -> Result<Value, String>,
) -> Result<String, String> {
    let unusable = || format!("not logged in to {} (/login)", shown_name(provider));
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
    update(path, |saved| {
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
    let made = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory);
    let file = (std::fs::OpenOptions::new().write(true).create_new(true))
        .mode(0o600)
        .open(path);
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
    std::fs::write(&written, text).map_err(failed)?;
    std::fs::rename(&written, path).map_err(failed)
}

/// A login's JSON answer from `what`, or what failed.
pub(super) fn answer(fetched: Fetched, what: &str) -> Result<Value, String> {
    let text = String::from_utf8_lossy(&fetched.body);
    if !(200..300).contains(&fetched.status) {
        let said = text.trim().chars().take(400).collect::<String>();
        return Err(format!("{what} answered HTTP {}: {said}", fetched.status));
    }
    serde_json::from_str(&text).map_err(|error| format!("{what}'s answer is not JSON: {error}"))
}

/// Runs `future` to its end on a runtime of its own: logins run apart from any turn.
pub(super) fn blocking<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    runtime.map_err(|error| error.to_string())?.block_on(future)
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
        let _ = rustix::fs::flock(lock, rustix::fs::FlockOperation::LockShared);
    }
    read(path)
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
/// changed them. A file that cannot be read is left as it is.
fn update<T>(
    path: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> Result<T, String>,
) -> Result<T, String> {
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    let directory = path.parent().unwrap_or(Path::new("."));
    let made = std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory);
    made.map_err(failed)?;
    let lock = (std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600))
    .open(lock_file(path))
    .map_err(failed)?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|error| failed(error.into()))?;
    let mut saved = read(path)?;
    let before = saved.clone();
    let result = change(&mut saved)?;
    if saved != before {
        let mut text = serde_json::to_string_pretty(&Value::Object(saved)).unwrap_or_default();
        text.push('\n');
        let write = || {
            let mut file = (std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true))
            .mode(0o600)
            .open(path)?;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(text.as_bytes())?;
            file.sync_all()
        };
        write().map_err(failed)?;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> ScratchDir {
        let path = std::env::temp_dir().join(format!("scramjet-{name}-{}", std::process::id()));
        ScratchDir::new(path)
    }

    #[test]
    fn credentials_are_saved_for_their_owner_only_and_removed_by_provider() {
        let scratch = scratch("login-store");
        let path = scratch.0.join("config/scramjet-rs/auth.json");
        update(&path, |saved| {
            saved.insert(
                "github-copilot".into(),
                json!({"type": "oauth", "access": "a"}),
            );
            saved.insert("other".into(), json!({"type": "oauth", "access": "b"}));
            Ok(())
        })
        .unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(saved(&path).unwrap().len(), 2);
        let removed = update(&path, |saved| Ok(saved.remove("other").is_some())).unwrap();
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
        let token = access_with(&path, "github-copilot", &mut refresh).unwrap();
        assert_eq!(token, "new");
        let credential = &saved(&path).unwrap()["github-copilot"];
        assert_eq!(credential["access"], "new");
        assert_eq!(credential["enterpriseUrl"], "company.ghe.com");
        // Fresh now, it is used as it is.
        let token = access_with(&path, "github-copilot", &mut refresh).unwrap();
        assert_eq!((token.as_str(), refreshes), ("new", 1));
        let failed = access_with(&path, "other", |_| unreachable!());
        assert_eq!(failed.unwrap_err(), "not logged in to other (/login)");
        // A refresh that fails leaves the login as it was.
        let expired = json!({"type": "oauth", "refresh": "gho", "access": "old", "expires": 0});
        save(&path, "github-copilot", expired.clone()).unwrap();
        let failed = access_with(&path, "github-copilot", |_| Err("HTTP 401".to_owned()));
        assert_eq!(failed.unwrap_err(), "HTTP 401");
        assert_eq!(saved(&path).unwrap()["github-copilot"], expired);
    }

    #[test]
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
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}\n");
        assert_eq!((mode(&path), mode(path.parent().unwrap())), (0o600, 0o700));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save(&path, "github-copilot", json!({"type": "oauth"})).unwrap();
        assert_eq!(mode(&path), 0o600);
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
        save_key(&path, "openrouter", "  sk-or-v1-abc\n").unwrap();
        let saved = saved(&path).unwrap();
        assert_eq!(
            saved["openrouter"],
            json!({"type": "api_key", "key": "sk-or-v1-abc"})
        );
        assert_eq!(key(&saved, "openrouter"), Some("sk-or-v1-abc"));
        assert_eq!(key(&saved, "github-copilot"), None);
        assert_eq!(saved_keys(&path).unwrap(), ["openrouter"]);
        assert_eq!(logged_in(&path).unwrap(), ["github-copilot", "openrouter"]);
        for (refused, why) in [
            ("  ", "the key is empty"),
            ("!pass show key", "a key starting with ! is a shell command"),
            ("sk-or\nv1", "without spaces"),
            ("sk or", "without spaces"),
            ("sk-é", "without spaces"),
        ] {
            let error = save_key(&path, "openrouter", refused).unwrap_err();
            assert!(error.contains(why), "{refused:?}: {error}");
        }
        assert_eq!(
            key(&super::saved(&path).unwrap(), "openrouter"),
            Some("sk-or-v1-abc")
        );
        let logins = Logins {
            auth: path.clone(),
            lists: scratch.0.join("logins"),
        };
        assert!(logout(&logins, "openrouter").unwrap());
        assert!(saved_keys(&path).unwrap().is_empty());
        // A name from a models.json file is not a file's: removing its key removes nothing else.
        std::fs::create_dir_all(&logins.lists).unwrap();
        let outside = scratch.0.join("models.json");
        std::fs::write(&outside, "{}").unwrap();
        save_key(&path, "../models", "sk").unwrap();
        assert!(logout(&logins, "../models").unwrap());
        assert!(outside.exists());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_as_it_is() {
        let scratch = scratch("login-broken");
        let path = scratch.0.join("auth.json");
        std::fs::write(&path, "{not json").unwrap();
        let changed = update(&path, |saved| {
            saved.insert("github-copilot".into(), json!({}));
            Ok(())
        });
        assert!(changed.unwrap_err().contains("auth.json"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
        assert!(saved(&scratch.0.join("missing.json")).unwrap().is_empty());
    }
}
