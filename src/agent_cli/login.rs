//! `/login` and `/logout`: scramjet-rs's own logins, whose credentials it keeps in its own file
//! (`agent::login`). GitHub Copilot's is a device login: the user enters a code on GitHub's page
//! while the screen waits, as Scramjet's login dialog does. ChatGPT's is a browser login, which
//! comes back by itself, or the user pastes the address the browser ended on. While the full
//! screen is open, the models each login lists are read again in the background as they age.
//! A provider of the `models.json` files takes an API key, which requests to it then carry.

use super::interactive::Interactive;
use super::screen::{Answer, Dialog, Optional, Relisting};
use super::select::{self, Picker};
use super::text::{clip, visible};
use super::{clipboard, editor, style};
use scramjet_rs::agent::{KeyFrom, codex, copilot, login};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

/// How old a login's model list grows before it is read again, at start or while the full
/// screen is open; also how long one that could not be read waits before it is tried again.
const RELIST_AFTER: Duration = Duration::from_secs(15 * 60);
/// How often the lists' age is looked at.
const LOOK_EVERY: Duration = Duration::from_secs(60);

/// A row of `/login`'s list.
enum Row<'a> {
    Heading(&'static str),
    /// A provider's own login: its name, and what the user knows it by.
    Login(&'static str, &'static str),
    /// A provider of the files, which takes an API key.
    Key(&'a str, &'a KeyFrom),
}

/// `/login`'s rows: the logins, then, under headings, the providers that take an API key.
fn rows(keyed: &[(String, KeyFrom)]) -> Vec<Row<'_>> {
    let logins = (login::LOGINS.iter()).map(|(name, shown)| Row::Login(name, shown));
    if keyed.is_empty() {
        return logins.collect();
    }
    let mut rows = vec![Row::Heading("Subscriptions")];
    rows.extend(logins);
    rows.push(Row::Heading("API keys"));
    rows.extend(keyed.iter().map(|(name, from)| Row::Key(name, from)));
    rows
}

impl Interactive {
    /// `/login`: the providers to log in to, each marked when logged in, and those that take an
    /// API key, each with where its key comes from; then its login.
    pub(super) fn login(&mut self) {
        let Some(logins) = login::Logins::user() else {
            return self.say("Error: neither XDG_CONFIG_HOME nor HOME is set");
        };
        let Some(screen) = &self.screen else {
            return self.say("Error: /login needs the full screen");
        };
        let logged_in = login::logged_in(&logins.auth).unwrap_or_default();
        let is_logged_in = |name: &str| logged_in.iter().any(|known| known == name);
        let keyed = self.agent.providers().keyed().to_vec();
        let rows = rows(&keyed);
        let notes = (rows.iter())
            .map(|row| match row {
                Row::Login(name, _) if is_logged_in(name) => {
                    "Logged in; logging in again replaces the saved login"
                }
                Row::Key(_, KeyFrom::Saved) => {
                    "Saved in auth.json, as logins are; entering another replaces it"
                }
                Row::Key(_, KeyFrom::Variable(_) | KeyFrom::Entry) => {
                    "A key saved here goes to auth.json, as logins do, and replaces models.json's"
                }
                Row::Key(_, KeyFrom::Nowhere) => {
                    "Its requests carry no key. One saved here is kept in auth.json, as logins are"
                }
                _ => "",
            })
            .map(str::to_owned)
            .collect();
        let headed = matches!(rows.first(), Some(Row::Heading(_)));
        let indent = if headed { "  " } else { "" };
        let row = |index: usize, width| {
            let mark = |text: &str| format!(" {}✓ {text}{}", style::MUTED, style::RESET);
            let text = match &rows[index] {
                Row::Heading(heading) => heading.to_string(),
                Row::Login(name, shown) if is_logged_in(name) => {
                    format!("{indent}{shown}{}", mark("logged in"))
                }
                Row::Login(_, shown) => format!("{indent}{shown}"),
                Row::Key(name, from) => {
                    let from = match from {
                        KeyFrom::Saved => mark("key saved"),
                        KeyFrom::Variable(variable) => mark(&format!("key from {variable}")),
                        KeyFrom::Entry => mark("key in models.json"),
                        KeyFrom::Nowhere => String::new(),
                    };
                    format!("{indent}{}{from}", visible(name))
                }
            };
            clip(&text, width)
        };
        let keys = select::hint("log in", &[], "cancel");
        // The headings take rows.
        let visible = (editor::terminal_size().1 / 2).max(select::VISIBLE);
        let picker = Picker::select("Log in to", visible, rows.len(), 0, None, row);
        let headings = rows.iter().map(|row| matches!(row, Row::Heading(_)));
        let picker = picker.noting(keys, notes).headed(headings.collect());
        let Some(index) = screen.pick(picker) else {
            return;
        };
        match rows[index] {
            Row::Login(copilot::PROVIDER, shown) => self.copilot_login(&logins, shown),
            Row::Login(codex::PROVIDER, shown) => self.codex_login(&logins, shown),
            Row::Key(provider, _) => self.key_login(&logins, provider),
            _ => {}
        }
    }

    /// Asks for `provider`'s API key, and saves it.
    fn key_login(&mut self, logins: &login::Logins, provider: &str) {
        let Some(screen) = &self.screen else {
            return;
        };
        let title = format!("API key for {}", visible(provider));
        let typed = match screen.ask(Dialog::Text(title, String::new(), Optional::No), None) {
            Some(Answer::Text(Some(typed))) => typed,
            _ => return,
        };
        let shown = visible(provider);
        match login::save_key(&logins.auth, provider, &typed) {
            Ok(()) => {
                let saved = logins.auth.display();
                self.say(&format!("✓ Saved the API key for {shown} in {saved}"));
                self.logins_changed();
            }
            Err(error) => self.say(&format!(
                "Error: the API key for {shown} was not saved: {error}"
            )),
        }
    }

    /// ChatGPT's browser login: the page is opened and copied, and the address the browser ends
    /// on comes back by itself or is pasted, whichever comes first, as in Scramjet's dialog.
    fn codex_login(&mut self, logins: &login::Logins, shown: &str) {
        let Some(screen) = &self.screen else {
            return;
        };
        let browser = match codex::BrowserLogin::start() {
            Ok(browser) => browser,
            Err(error) => return self.say(&format!("Error: login to {shown} failed: {error}")),
        };
        let copied = match clipboard::copy(&browser.url, false) {
            Ok(_) => " (copied)",
            Err(_) => "",
        };
        open_in_browser(&browser.url);
        screen.say(&format!(
            "Log in to {shown} in the browser that opened, or open this page in the one you \
             choose, such as a private window{copied}:\n{}",
            browser.url
        ));
        let title = match browser.listening() {
            true => "Paste the browser's last address, or log in there",
            false => "Port 1455 is in use: paste the browser's last address",
        };
        let (paste, pasted) = mpsc::channel();
        let [answered, done, cancel] = [(); 3].map(|()| Arc::new(AtomicBool::new(false)));
        let finishing = {
            let (logins, answered, done, cancel) = (
                logins.clone(),
                answered.clone(),
                done.clone(),
                cancel.clone(),
            );
            std::thread::spawn(move || {
                let finished = browser.finish(&pasted, &answered, &logins, &cancel);
                // Ended without an answer, as for a pasted text that is none, it ends the dialog.
                answered.store(true, Ordering::Relaxed);
                done.store(true, Ordering::Relaxed);
                finished
            })
        };
        let dialog = Dialog::Text(title.to_owned(), String::new(), Optional::No);
        match screen.ask(dialog, Some(&answered)) {
            Some(Answer::Text(Some(text))) => {
                let _ = paste.send(text);
            }
            // Escape, or the screen gone.
            _ if !answered.load(Ordering::Relaxed) => cancel.store(true, Ordering::Relaxed),
            _ => {}
        }
        if !cancel.load(Ordering::Relaxed) {
            let note = "Taking the tokens from OpenAI and the models from ChatGPT…".to_owned();
            screen.ask(
                Dialog::Wait(format!("Log in to {shown}"), note),
                Some(&done),
            );
            if !done.load(Ordering::Relaxed) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        let finished = (finishing.join()).unwrap_or_else(|_| Err("the login failed".to_owned()));
        let warning = finished.as_ref().ok().cloned().flatten();
        self.finished_login(shown, logins, finished.map(drop));
        if let Some(warning) = warning {
            self.say(&format!("Warning: {warning}"));
        }
    }

    /// GitHub Copilot's device login, for github.com or an Enterprise server.
    fn copilot_login(&mut self, logins: &login::Logins, shown: &str) {
        let Some(screen) = &self.screen else {
            return;
        };
        let title = "GitHub Enterprise URL or domain (empty for github.com)".to_owned();
        let typed = match screen.ask(Dialog::Text(title, String::new(), Optional::Yes), None) {
            Some(Answer::Text(Some(typed))) => typed,
            _ => return,
        };
        let enterprise = match copilot::enterprise_domain(&typed) {
            Ok(enterprise) => enterprise,
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        let device = match copilot::DeviceLogin::start(enterprise) {
            Ok(device) => device,
            Err(error) => return self.say(&format!("Error: login to {shown} failed: {error}")),
        };
        let copied = match clipboard::copy(&device.code, false) {
            Ok(_) => " (copied)",
            Err(_) => "",
        };
        let note = format!(
            "Open {} and enter the code {}{copied}. Waiting for GitHub…",
            device.url, device.code
        );
        let (done, cancel) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let finishing = {
            let (logins, done, cancel) = (logins.clone(), done.clone(), cancel.clone());
            std::thread::spawn(move || {
                let finished = device.finish(&logins, &cancel);
                done.store(true, Ordering::Relaxed);
                finished
            })
        };
        let title = format!("Log in to {shown}");
        // Escape, or the screen gone: the login stops.
        screen.ask(Dialog::Wait(title, note), Some(&done));
        if !done.load(Ordering::Relaxed) {
            cancel.store(true, Ordering::Relaxed);
        }
        let finished = (finishing.join()).unwrap_or_else(|_| Err("the login failed".to_owned()));
        self.finished_login(shown, logins, finished);
    }

    /// Says how a login ended.
    fn finished_login(
        &mut self,
        shown: &str,
        logins: &login::Logins,
        finished: Result<(), String>,
    ) {
        match finished {
            Ok(()) => {
                let saved = logins.auth.display();
                self.say(&format!(
                    "✓ Logged in to {shown}. Credentials saved to {saved}"
                ));
                self.logins_changed();
            }
            Err(error) if error == login::CANCELLED => {
                self.say_styled(style::MUTED, "Login cancelled")
            }
            Err(error) => self.say(&format!("Error: login to {shown} failed: {error}")),
        }
    }

    /// `/logout`: the logins saved, one of which is removed.
    pub(super) fn logout(&mut self) {
        let Some(logins) = login::Logins::user() else {
            return self.say("Error: neither XDG_CONFIG_HOME nor HOME is set");
        };
        let logged_in = match login::logged_in(&logins.auth) {
            Ok(logged_in) => logged_in,
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        if logged_in.is_empty() {
            return self.say(
                "No saved logins to remove. /logout removes only what /login saved; environment \
                 variables and models.json files are unchanged.",
            );
        }
        let keys = login::saved_keys(&logins.auth).unwrap_or_default();
        let is_key = |provider: &str| keys.iter().any(|keyed| keyed == provider);
        let shown = |provider: &str| match is_key(provider) {
            true => format!("{} API key", visible(provider)),
            false => visible(login::shown_name(provider)),
        };
        let chosen = match &self.screen {
            Some(screen) => {
                let row = |index: usize, width| clip(&shown(&logged_in[index]), width);
                let keys = select::hint("log out", &[], "cancel");
                let picker =
                    Picker::select("Log out of", select::VISIBLE, logged_in.len(), 0, None, row);
                let notes = vec![String::new(); logged_in.len()];
                screen.pick(picker.noting(keys, notes))
            }
            None if logged_in.len() == 1 => Some(0),
            None => return self.say("Error: /logout needs the full screen to choose a login"),
        };
        let Some(index) = chosen else {
            return;
        };
        let provider = &logged_in[index];
        match login::logout(&logins, provider) {
            Ok(_) if is_key(provider) => {
                self.say(&format!(
                    "✓ Removed the saved API key for {}; environment variables and models.json \
                     files are unchanged",
                    visible(provider)
                ));
                self.logins_changed();
            }
            Ok(_) => {
                self.say(&format!("✓ Logged out of {}", shown(provider)));
                self.logins_changed();
            }
            Err(error) => self.say(&format!("Error: {error}")),
        }
    }

    /// The providers' models change with the logins.
    fn logins_changed(&mut self) {
        self.agent.load_model_info();
        if let Some(screen) = &self.screen {
            screen.models(self.agent.models().unwrap_or_default());
        }
    }
}

/// Keeps the logins' model lists current while the full screen is open: each is read again in
/// the background once `RELIST_AFTER` old, at start too, and `relisting` hears when what they
/// offer changes, with what to tell the user. A list another process read is taken up as well.
pub(super) fn keep_lists_current(relisting: Relisting) {
    let Some(logins) = login::Logins::user() else {
        return;
    };
    let looking = std::thread::Builder::new().name("relist".into());
    let _ = looking.spawn(move || {
        let mut lists = Lists::new(&logins);
        loop {
            if let Some(notes) = lists.look(&logins, Instant::now(), login::relist) {
                relisting.changed(notes);
            }
            std::thread::sleep(LOOK_EVERY);
        }
    });
}

/// What the logins' lists offered when last looked at.
struct Lists {
    models: HashMap<String, Vec<Value>>,
    /// The logins whose list could not be read again, when to try again.
    retry: HashMap<String, Instant>,
    /// The logins whose trouble reading their list was said, until one is read well.
    said: HashSet<String>,
}

impl Lists {
    /// As the session read them at start.
    fn new(logins: &login::Logins) -> Self {
        let logged_in = login::logged_in(&logins.auth).unwrap_or_default();
        let models = (login::LOGINS.iter())
            .filter(|(name, _)| logged_in.iter().any(|known| known == name))
            .map(|(name, _)| (name.to_string(), offered(logins, name)))
            .collect();
        Self {
            models,
            retry: HashMap::new(),
            said: HashSet::new(),
        }
    }

    /// Reads again with `relist` each list due at `now`; returns what to tell the user when
    /// what the lists offer changed, or there is trouble not yet said.
    fn look(
        &mut self,
        logins: &login::Logins,
        now: Instant,
        relist: impl Fn(&login::Logins, &str) -> Result<Option<String>, String>,
    ) -> Option<Vec<String>> {
        let logged_in = login::logged_in(&logins.auth).unwrap_or_default();
        let (mut notes, mut changed) = (Vec::new(), false);
        for (name, shown) in login::LOGINS {
            if !logged_in.iter().any(|known| known == name) {
                changed |= self.models.remove(name).is_some();
                self.retry.remove(name);
                self.said.remove(name);
                continue;
            }
            let before = login::List::read(logins, name);
            let due =
                (before.as_ref().and_then(login::List::age)).is_none_or(|age| age >= RELIST_AFTER);
            let waiting = self.retry.get(name).is_some_and(|retry| now < *retry);
            if due && !waiting {
                let before = before.map(|list| list.models(name)).unwrap_or_default();
                match relist(logins, name) {
                    Ok(warning) => {
                        self.retry.remove(name);
                        match warning {
                            Some(warning) if self.said.insert(name.to_owned()) => {
                                notes.push(format!("Warning: {warning}"));
                            }
                            Some(_) => {}
                            None => {
                                self.said.remove(name);
                            }
                        }
                        notes.extend(differences(shown, &before, &offered(logins, name)));
                    }
                    Err(error) => {
                        self.retry.insert(name.to_owned(), now + RELIST_AFTER);
                        if self.said.insert(name.to_owned()) {
                            notes.push(format!(
                                "Warning: {shown}'s models could not be read again, so those it \
                                 listed before are offered: {error}"
                            ));
                        }
                    }
                }
            }
            let models = offered(logins, name);
            if self.models.get(name) != Some(&models) {
                self.models.insert(name.to_owned(), models);
                changed = true;
            }
        }
        (changed || !notes.is_empty()).then_some(notes)
    }
}

/// The models `provider`'s saved list offers.
fn offered(logins: &login::Logins, provider: &str) -> Vec<Value> {
    let list = login::List::read(logins, provider);
    list.map(|list| list.models(provider)).unwrap_or_default()
}

/// What to say of the models `shown` offers now, `after`, that it did not, `before`, and of
/// those it no longer offers.
fn differences(shown: &str, before: &[Value], after: &[Value]) -> Vec<String> {
    let ids = |models: &[Value]| -> Vec<String> {
        let ids = models.iter().filter_map(|model| model["id"].as_str());
        ids.map(str::to_owned).collect()
    };
    let (before, after) = (ids(before), ids(after));
    let added: Vec<&str> = (after.iter())
        .filter(|id| !before.contains(id))
        .map(String::as_str)
        .collect();
    let removed: Vec<&str> = (before.iter())
        .filter(|id| !after.contains(id))
        .map(String::as_str)
        .collect();
    let mut notes = Vec::new();
    if !added.is_empty() {
        notes.push(format!("{shown} now offers {} (/model)", added.join(", ")));
    }
    if !removed.is_empty() {
        notes.push(format!("{shown} no longer offers {}", removed.join(", ")));
    }
    notes
}

/// Opens `url` in the user's browser, as Scramjet's login dialog does (`open` on macOS, else
/// `xdg-open`, which WSL's `wslview` serves); the page is shown and copied besides.
fn open_in_browser(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let opened = (Command::new(program).arg(url))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut opener) = opened {
        std::thread::spawn(move || opener.wait());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn now_ms() -> f64 {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        since.unwrap().as_millis() as f64
    }

    /// Saves a Copilot list of `ids`, read `minutes` ago.
    fn list(logins: &login::Logins, minutes: f64, ids: &[&str]) {
        let models: Vec<Value> = (ids.iter())
            .map(|id| {
                json!({"id": id, "supported_endpoints": ["/responses"],
                    "capabilities": {"type": "chat", "supports": {"tool_calls": true}}})
            })
            .collect();
        let list = json!({"fetched": now_ms() - minutes * 60_000.0, "data": models});
        std::fs::create_dir_all(&logins.lists).unwrap();
        let path = logins.lists.join("github-copilot.json");
        std::fs::write(path, list.to_string()).unwrap();
    }

    #[test]
    fn lists_are_read_again_as_they_age_and_changes_and_trouble_are_said_once() {
        let scratch = Scratch(
            std::env::temp_dir().join(format!("scramjet-rs-relist-{}", std::process::id())),
        );
        let logins = login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        std::fs::create_dir_all(&scratch.0).unwrap();
        let credential = r#"{"github-copilot": {"type": "oauth", "access": "a", "expires": 0}}"#;
        std::fs::write(&logins.auth, credential).unwrap();
        list(&logins, 1.0, &["a", "b"]);
        let mut lists = Lists::new(&logins);
        let start = Instant::now();
        let never = |_: &login::Logins, _: &str| -> Result<Option<String>, String> {
            panic!("read again while fresh")
        };
        assert_eq!(lists.look(&logins, start, never), None);
        // Old enough, it is read again, and what it offers now and no longer is said.
        list(&logins, 16.0, &["a", "b"]);
        let relisted = |logins: &login::Logins, name: &str| {
            assert_eq!(name, "github-copilot");
            list(logins, 0.0, &["a", "c"]);
            Ok(None)
        };
        assert_eq!(
            lists.look(&logins, start, relisted),
            Some(vec![
                "GitHub Copilot now offers c (/model)".to_owned(),
                "GitHub Copilot no longer offers b".to_owned(),
            ])
        );
        // Another process read it: taken up, nothing said.
        list(&logins, 0.0, &["a", "c", "d"]);
        assert_eq!(lists.look(&logins, start, never), Some(vec![]));
        // Trouble is said once, the list kept, and tried again only after a while.
        list(&logins, 16.0, &["a", "c", "d"]);
        let tries = Cell::new(0);
        let failing = |_: &login::Logins, _: &str| {
            tries.set(tries.get() + 1);
            Err("offline".to_owned())
        };
        assert_eq!(
            lists.look(&logins, start, failing),
            Some(vec![
                "Warning: GitHub Copilot's models could not be read again, so those it listed \
                 before are offered: offline"
                    .to_owned()
            ])
        );
        assert_eq!(lists.look(&logins, start, failing), None);
        let later = start + RELIST_AFTER;
        assert_eq!(lists.look(&logins, later, failing), None);
        assert_eq!(tries.get(), 2);
        // Read well, a later trouble is said again.
        let unchanged = |logins: &login::Logins, _: &str| {
            list(logins, 0.0, &["a", "c", "d"]);
            Ok(None)
        };
        assert_eq!(lists.look(&logins, later + RELIST_AFTER, unchanged), None);
        list(&logins, 16.0, &["a", "c", "d"]);
        let warned = |_: &login::Logins, _: &str| Ok(Some("npm is out of reach".to_owned()));
        let said = lists.look(&logins, later + RELIST_AFTER, warned);
        assert_eq!(said, Some(vec!["Warning: npm is out of reach".to_owned()]));
        // Logged out, its models are gone.
        std::fs::write(&logins.auth, "{}").unwrap();
        assert_eq!(lists.look(&logins, later, never), Some(vec![]));
        assert!(lists.models.is_empty());
    }
}
