//! scramjet-rs's own credentials, which `/login` saves: `auth.json` in the settings' directory,
//! readable by its owner only and hidden from the agent's tools as other credential stores are
//! (`sandbox::Hidden`). It is in Scramjet's format: an object of the providers logged in to,
//! each `{"type": "oauth", "refresh", "access", "expires"}` (milliseconds since the epoch) with
//! what else its login keeps. A process changes it holding a lock on `auth.json.lock`, so that
//! two refreshing a token at once do not undo each other's, and replaces it whole. What a login
//! lists that is not secret, its models, is kept apart, in the state directory.

use super::copilot;
use serde_json::{Map, Value};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// The providers `/login` logs in to: their names, as providers and in `auth.json`, and what
/// the user knows them by.
pub const LOGINS: [(&str, &str); 1] = [(copilot::PROVIDER, "GitHub Copilot")];

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

/// Removes `provider`'s credentials, and what its login listed; returns whether there were any.
pub fn logout(logins: &Logins, provider: &str) -> Result<bool, String> {
    let removed = update(&logins.auth, |saved| Ok(saved.remove(provider).is_some()))?;
    let _ = std::fs::remove_file(logins.listed(provider));
    Ok(removed)
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

pub(super) fn now_ms() -> f64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    since.as_millis() as f64
}

/// The saved credentials; none when the file does not exist.
pub(super) fn saved(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
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
    .open(path.with_file_name("auth.json.lock"))
    .map_err(failed)?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|error| failed(error.into()))?;
    let mut saved = saved(path)?;
    let before = saved.clone();
    let result = change(&mut saved)?;
    if saved != before {
        let mut text = serde_json::to_string_pretty(&Value::Object(saved)).unwrap_or_default();
        text.push('\n');
        let written = path.with_file_name(format!(".auth.json.{}", std::process::id()));
        let write = || {
            let mut file = (std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true))
            .mode(0o600)
            .open(&written)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&written, path)
        };
        if let Err(error) = write() {
            let _ = std::fs::remove_file(&written);
            return Err(failed(error));
        }
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
        // Only the file and its lock are left: the file written is moved into place.
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
