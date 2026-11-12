//! `/login` and `/logout`: scramjet-rs's own logins, whose credentials it keeps in its own file
//! (`agent::login`). GitHub Copilot's is a device login: the user enters a code on GitHub's page
//! while the screen waits, as Scramjet's login dialog does. ChatGPT's is a browser login, which
//! comes back by itself, or the user pastes the address the browser ended on. While the full
//! screen is open, the models each login lists are read again in the background as they age.
//! A package's login is its program's: the screen opens and shows the pages it names and waits
//! with what it says. A provider of the `models.json` files takes an API key, which requests to
//! it then carry. The lists the providers whose entries list no models give themselves are kept
//! current alike.

use super::interactive::{self, Interactive};
use super::screen::{Answer, Dialog, Optional, Relisting};
use super::select::{self, Picker};
use super::text::{clip, visible};
use super::{clipboard, editor, style};
use scramjet::agent::login::{LoginProgram, Said};
use scramjet::agent::{KeyFrom, Listing, codex, copilot, login};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex, PoisonError,
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
    /// A provider's own login, or a package's: its name, and what the user knows it by.
    Login(&'a str, &'a str),
    /// A provider of the files, which takes an API key.
    Key(&'a str, &'a KeyFrom),
    /// A runtime serving models on this machine or nearby (`local_login`).
    Local,
}

/// `/login`'s rows, under headings: its own logins and the `programs`', the providers that take
/// an API key, and local models.
fn rows<'a>(programs: &'a [LoginProgram], keyed: &'a [(String, KeyFrom)]) -> Vec<Row<'a>> {
    let own = (login::LOGINS.iter()).map(|(name, shown)| Row::Login(name, shown));
    let packaged = programs.iter().map(|p| Row::Login(&p.provider, &p.name));
    let mut rows = vec![Row::Heading("Subscriptions")];
    rows.extend(own.chain(packaged));
    if !keyed.is_empty() {
        rows.push(Row::Heading("API keys"));
        rows.extend(keyed.iter().map(|(name, from)| Row::Key(name, from)));
    }
    rows.push(Row::Heading("Local models"));
    rows.push(Row::Local);
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
        // Local models' providers have their own row, which asks for a key when one is needed.
        let local = (self.models_file())
            .and_then(|file| scramjet::agent::local::saved(&file).ok())
            .unwrap_or_default();
        let mut keyed = self.session.agent.providers().keyed().to_vec();
        keyed.retain(|(name, _)| !local.iter().any(|(saved, _)| saved == name));
        let programs = self.session.agent.providers().login_programs().to_vec();
        let rows = rows(&programs, &keyed);
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
                Row::Key(_, KeyFrom::Given) => {
                    "Its requests carry --api-key's for this run; one saved here is kept for later"
                }
                Row::Key(_, KeyFrom::Nowhere) => {
                    "Its requests carry no key. One saved here is kept in auth.json, as logins are"
                }
                Row::Local => "Models a runtime serves here or at an address you give",
                _ => "",
            })
            .map(str::to_owned)
            .collect();
        let indent = "  ";
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
                        KeyFrom::Given => mark("key from --api-key"),
                        KeyFrom::Nowhere => String::new(),
                    };
                    format!("{indent}{}{from}", visible(name))
                }
                Row::Local => format!("{indent}Ollama, llama.cpp, vLLM…"),
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
            Row::Login(provider, _) => {
                let program = programs.iter().find(|program| program.provider == provider);
                if let Some(program) = program {
                    self.program_login(&logins, program.clone());
                }
            }
            Row::Key(provider, _) => self.key_login(&logins, provider),
            Row::Local => self.local_login(),
            Row::Heading(_) => {}
        }
    }

    /// A package's login, its program's: the pages it names are opened and shown, and the
    /// dialog waits with what it last said until it ends; Escape stops it.
    fn program_login(&mut self, logins: &login::Logins, program: LoginProgram) {
        let Some(screen) = &self.screen else {
            return;
        };
        let shown = program.name.clone();
        let (telling, heard) = mpsc::channel();
        let [changed, done, cancel] = [(); 3].map(|()| Arc::new(AtomicBool::new(false)));
        let finishing = {
            let (logins, changed, done, cancel) = (
                logins.clone(),
                changed.clone(),
                done.clone(),
                cancel.clone(),
            );
            std::thread::spawn(move || {
                let finished = program.login(&logins, &cancel, |said| {
                    let _ = telling.send(said);
                    changed.store(true, Ordering::SeqCst);
                });
                done.store(true, Ordering::SeqCst);
                changed.store(true, Ordering::SeqCst);
                finished
            })
        };
        let title = format!("Log in to {shown}");
        let mut note = "Starting the login…".to_owned();
        let tell = |said: Said, note: &mut String| match said {
            Said::Open(url) => {
                let copied = match clipboard::copy(&url, false) {
                    Ok(_) => " (copied)",
                    Err(_) => "",
                };
                open_in_browser(&url);
                screen.say(&format!(
                    "Log in to {shown} in the browser that opened, or open this page in the one \
                     you choose{copied}:\n{url}"
                ));
            }
            Said::Say(text) => *note = text,
        };
        // Shown again with each news, until the login ends.
        while !done.load(Ordering::SeqCst) {
            let answer = screen.ask(Dialog::Wait(title.clone(), note.clone()), Some(&changed));
            let news = changed.swap(false, Ordering::SeqCst);
            // Escape, or the screen gone.
            if answer.is_some() || !news {
                cancel.store(true, Ordering::SeqCst);
                break;
            }
            for said in heard.try_iter() {
                tell(said, &mut note);
            }
        }
        let finished = (finishing.join()).unwrap_or_else(|_| Err("the login failed".to_owned()));
        for said in heard.try_iter() {
            tell(said, &mut note);
        }
        let warning = finished.as_ref().ok().cloned().flatten();
        self.finished_login(&shown, logins, finished.map(drop));
        if let Some(warning) = warning {
            self.say(&format!("Warning: {warning}"));
        }
    }

    /// Asks for `provider`'s API key, and saves it; a provider whose entry lists no models is then
    /// asked for them with it.
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
        let origins = self.session.agent.providers().origins(provider).to_vec();
        match login::save_key(&logins.auth, provider, &typed, &origins) {
            Ok(()) => {
                self.session.agent.forget_list(provider);
                self.logins_changed();
                let saved = logins.auth.display();
                let listing = (self.session.agent.providers().listings().iter())
                    .find(|listing| listing.provider() == provider);
                let said = match listing.map(Listing::offered) {
                    None => format!("✓ Saved the API key for {shown} in {saved}"),
                    Some(Ok(count)) => format!(
                        "✓ Saved the API key for {shown} in {saved}; it offers {count} model{} \
                         (/model)",
                        if count == 1 { "" } else { "s" }
                    ),
                    Some(Err(problem)) => format!(
                        "✓ Saved the API key for {shown} in {saved}\nWarning: {shown} lists no \
                         models, and {problem}"
                    ),
                };
                self.say(&said);
                self.take_offered_model(None);
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
                self.take_offered_model(None);
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
        let programs = self.session.agent.providers().login_programs().to_vec();
        let shown = |provider: &str| match is_key(provider) {
            true => format!("{} API key", visible(provider)),
            false => match programs.iter().find(|program| program.provider == provider) {
                Some(program) => visible(&program.name),
                None => visible(login::shown_name(provider)),
            },
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
        self.session.agent.load_model_info();
        if let Some(screen) = &self.screen {
            screen.models(self.session.agent.models().unwrap_or_default());
        }
    }

    /// When no provider offers the session's model (a first start with none leaves a
    /// placeholder), takes `preferred` if offered, else the model a new session would start with.
    pub(super) fn take_offered_model(&mut self, preferred: Option<&str>) {
        if self
            .session
            .agent
            .offered_model(self.session.agent.model())
            .is_some()
        {
            return;
        }
        let default = super::default_model(self.session.agent.providers());
        let Some(reference) = (preferred.into_iter().chain([default.as_str()]))
            .find_map(|model| self.session.agent.offered_model(model))
        else {
            return;
        };
        match self.session.agent.set_model(&reference) {
            Ok(()) => {
                self.session.agent.load_model_info();
                let line = interactive::model_line(&self.session.agent);
                self.say_styled(style::MUTED, &format!("This session now uses {line}"));
            }
            Err(error) => self.say(&format!("Error: model not set to {reference}: {error}")),
        }
        if let Some(screen) = &self.screen {
            screen.footer(super::footer::Shown::of(&self.session.agent));
        }
    }
}

/// What the background keeps current, as the session last read the providers: the logins
/// packages' programs do, and the providers that list their models themselves.
#[derive(Clone, Default)]
pub(super) struct Kept {
    pub(super) programs: Vec<LoginProgram>,
    pub(super) listings: Vec<Listing>,
}

impl Kept {
    pub(super) fn of(providers: &scramjet::agent::Providers) -> Self {
        Self {
            programs: providers.login_programs().to_vec(),
            listings: providers.listings().to_vec(),
        }
    }
}

/// Keeps the model lists current while the full screen is open, the logins' and those the
/// providers in `kept` give themselves: each is read again in the background once
/// `RELIST_AFTER` old, at start too, and `relisting` hears when what they offer changes, with
/// what to tell the user. A list another process read is taken up as well.
pub(super) fn keep_lists_current(relisting: Relisting, kept: Arc<Mutex<Kept>>) {
    let Some(logins) = login::Logins::user() else {
        return;
    };
    let looking = std::thread::Builder::new().name("relist".into());
    let _ = looking.spawn(move || {
        let kept = || kept.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut lists = Lists::new(&logins, &kept());
        let relist = |source: &Source| match source {
            Source::Login(name, _) => login::relist(&logins, name),
            Source::Program(program) => program.relist(&logins),
            Source::Provider(listing) => listing.relist().map(|()| None),
        };
        loop {
            if let Some(notes) = lists.look(&logins, &kept(), Instant::now(), relist) {
                relisting.changed(notes);
            }
            std::thread::sleep(LOOK_EVERY);
        }
    });
}

/// A model list kept current.
enum Source<'a> {
    /// A login's: its provider, and what the user knows it by.
    Login(&'static str, &'static str),
    /// A package's login's.
    Program(&'a LoginProgram),
    /// One a provider of the files gives itself, as its entry lists no models.
    Provider(&'a Listing),
}

impl Source<'_> {
    /// What `Lists` knows it by: a provider's has a line break, which no login's name has, and
    /// a package's login's is never one of `/login`'s own.
    fn key(&self) -> String {
        match self {
            Source::Login(name, _) => name.to_string(),
            Source::Program(program) => program.provider.clone(),
            Source::Provider(listing) => listing.key(),
        }
    }

    fn shown(&self) -> String {
        match self {
            Source::Login(_, shown) => shown.to_string(),
            Source::Program(program) => visible(&program.name),
            Source::Provider(listing) => visible(listing.provider()),
        }
    }

    /// How long ago it was read; `None` when there is none, or it does not say.
    fn age(&self, logins: &login::Logins) -> Option<Duration> {
        match self {
            Source::Login(name, _) => login::List::read(logins, name).and_then(|l| l.age()),
            Source::Program(program) => {
                login::List::read(logins, &program.provider).and_then(|l| l.age())
            }
            Source::Provider(listing) => listing.age(),
        }
    }

    fn models(&self, logins: &login::Logins) -> Vec<Value> {
        match self {
            Source::Login(name, _) => offered(logins, name),
            Source::Program(program) => offered(logins, &program.provider),
            Source::Provider(listing) => listing.models(),
        }
    }
}

/// The lists to keep current: of the logins saved in `logins`, `/login`'s own then `kept`'s
/// programs', then those `kept`'s providers give themselves.
fn sources<'a>(logins: &login::Logins, kept: &'a Kept) -> Vec<Source<'a>> {
    let logged_in = login::logged_in(&logins.auth).unwrap_or_default();
    let is_logged_in = |name: &str| logged_in.iter().any(|known| known == name);
    let own = (login::LOGINS.iter())
        .filter(|(name, _)| is_logged_in(name))
        .map(|(name, shown)| Source::Login(name, shown));
    let programs = (kept.programs.iter())
        .filter(|program| is_logged_in(&program.provider))
        .map(Source::Program);
    let listings = kept.listings.iter().map(Source::Provider);
    own.chain(programs).chain(listings).collect()
}

/// What the lists offered when last looked at.
struct Lists {
    models: HashMap<String, Vec<Value>>,
    /// The lists that could not be read again, when to try again.
    retry: HashMap<String, Instant>,
    /// The lists whose trouble reading them was said, until one is read well.
    said: HashSet<String>,
    /// The providers' lists seen.
    seen: HashSet<String>,
}

impl Lists {
    /// As the session read them at start.
    fn new(logins: &login::Logins, kept: &Kept) -> Self {
        let sources = sources(logins, kept);
        let models = sources
            .iter()
            .map(|source| (source.key(), source.models(logins)));
        let mut lists = Self {
            models: models.collect(),
            retry: HashMap::new(),
            said: HashSet::new(),
            seen: HashSet::new(),
        };
        for listing in &kept.listings {
            lists.see(listing, Instant::now());
        }
        lists
    }

    /// Takes note of `listing` the first time, at `now`: one not saved was asked for just now,
    /// as the providers were read, which said why it is not.
    fn see(&mut self, listing: &Listing, now: Instant) {
        let key = listing.key();
        if self.seen.insert(key.clone()) && listing.age().is_none() {
            self.retry.insert(key.clone(), now + RELIST_AFTER);
            self.said.insert(key);
        }
    }

    /// Forgets the list `key` names; returns whether it offered models.
    fn forget(&mut self, key: &str) -> bool {
        self.retry.remove(key);
        self.said.remove(key);
        self.seen.remove(key);
        self.models.remove(key).is_some()
    }

    /// Reads again with `relist` each list due at `now`, of the logins and of `kept`'s
    /// providers; returns what to tell the user when what the lists offer changed, or there is
    /// trouble not yet said.
    fn look(
        &mut self,
        logins: &login::Logins,
        kept: &Kept,
        now: Instant,
        relist: impl Fn(&Source) -> Result<Option<String>, String>,
    ) -> Option<Vec<String>> {
        let (mut notes, mut changed) = (Vec::new(), false);
        let sources = sources(logins, kept);
        // Those logged out of, and those gone from the files.
        let keys: HashSet<String> = sources.iter().map(Source::key).collect();
        let gone: Vec<String> = (self.models.keys().chain(&self.seen))
            .filter(|key| !keys.contains(*key))
            .cloned()
            .collect();
        for key in gone {
            changed |= self.forget(&key);
        }
        for listing in &kept.listings {
            self.see(listing, now);
        }
        for source in &sources {
            let (key, shown) = (source.key(), source.shown());
            let due = source.age(logins).is_none_or(|age| age >= RELIST_AFTER);
            let waiting = self.retry.get(&key).is_some_and(|retry| now < *retry);
            if due && !waiting {
                let before = source.models(logins);
                match relist(source) {
                    Ok(warning) => {
                        self.retry.remove(&key);
                        match warning {
                            Some(warning) if self.said.insert(key.clone()) => {
                                notes.push(format!("Warning: {warning}"));
                            }
                            Some(_) => {}
                            None => {
                                self.said.remove(&key);
                            }
                        }
                        notes.extend(differences(&shown, &before, &source.models(logins)));
                    }
                    Err(error) => {
                        self.retry.insert(key.clone(), now + RELIST_AFTER);
                        if self.said.insert(key.clone()) {
                            notes.push(match before.is_empty() {
                                true => {
                                    format!("Warning: {shown}'s models could not be read: {error}")
                                }
                                false => format!(
                                    "Warning: {shown}'s models could not be read again, so \
                                     those it listed before are offered: {error}"
                                ),
                            });
                        }
                    }
                }
            }
            let models = source.models(logins);
            let differs = (self.models.get(&key)).map_or(!models.is_empty(), |m| *m != models);
            if differs {
                changed = true;
            }
            self.models.insert(key, models);
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
        notes.push(format!("{shown} now offers {} (/model)", few(&added)));
    }
    if !removed.is_empty() {
        notes.push(format!("{shown} no longer offers {}", few(&removed)));
    }
    notes
}

/// `ids`, the first ten of a longer list followed by how many more there are.
fn few(ids: &[&str]) -> String {
    const SHOWN: usize = 10;
    match ids.len() > SHOWN + 1 {
        true => format!("{} and {} more", ids[..SHOWN].join(", "), ids.len() - SHOWN),
        false => ids.join(", "),
    }
}

/// Opens `url` in the user's browser, as Scramjet's login dialog does (`open` on macOS,
/// `xdg-open` elsewhere, which WSL's `wslview` serves); the page is shown and copied besides.
fn open_in_browser(url: &str) {
    let opened = browser_opener(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut opener) = opened {
        std::thread::spawn(move || opener.wait());
    }
}

/// On Windows `rundll32`'s URL handler, as Claude Code opens pages there: no shell sits between,
/// where Scramjet's `start` through `cmd.exe` would expand a `%…%` in the address.
fn browser_opener(url: &str) -> Command {
    if cfg!(windows) {
        let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from);
        let rundll32 = system.map_or("rundll32.exe".into(), |root| {
            root.join("System32").join("rundll32.exe")
        });
        let mut command = Command::new(rundll32);
        command.arg("url.dll,FileProtocolHandler").arg(url);
        return command;
    }
    let mut command = Command::new(if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    });
    command.arg(url);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use scramjet::agent::Providers;
    use serde_json::json;
    use std::cell::Cell;
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn kept(listings: &[Listing]) -> Kept {
        let listings = listings.to_vec();
        Kept {
            listings,
            ..Kept::default()
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
    fn the_lists_providers_give_themselves_are_kept_current_too() {
        let scratch = Scratch(
            std::env::temp_dir().join(format!("scramjet-rs-relist-given-{}", std::process::id())),
        );
        let logins = login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        std::fs::create_dir_all(scratch.0.join("models")).unwrap();
        let file = scratch.0.join("models.json");
        let entry = json!({"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-completions"});
        std::fs::write(&file, json!({"providers": {"local": entry}}).to_string()).unwrap();
        let saved = |minutes: f64, ids: &[&str]| {
            let data: Vec<Value> = ids.iter().map(|id| json!({"id": id})).collect();
            let list = json!({"url": "http://127.0.0.1:9/v1/models", "data": data,
                "fetched": now_ms() - minutes * 60_000.0});
            std::fs::write(scratch.0.join("models/local.json"), list.to_string()).unwrap();
        };
        saved(1.0, &["a"]);
        let providers = Providers::read(vec![file.clone()], Some(logins.clone()));
        let listings = providers.listings().to_vec();
        assert_eq!(
            listings.iter().map(Listing::offered).collect::<Vec<_>>(),
            [Ok(1)]
        );
        let mut lists = Lists::new(&logins, &kept(&listings));
        let start = Instant::now();
        let never =
            |_: &Source| -> Result<Option<String>, String> { panic!("read again while fresh") };
        assert_eq!(lists.look(&logins, &kept(&listings), start, never), None);
        // Old enough, it is read again, and what changed is said.
        saved(16.0, &["a"]);
        let relisted = |source: &Source| {
            assert!(matches!(source, Source::Provider(listing) if listing.provider() == "local"));
            saved(0.0, &["a", "b"]);
            Ok(None)
        };
        let said = lists.look(&logins, &kept(&listings), start, relisted);
        assert_eq!(said, Some(vec!["local now offers b (/model)".to_owned()]));
        // Gone from the files, its models go.
        assert_eq!(
            lists.look(&logins, &Kept::default(), start, never),
            Some(vec![])
        );
        // One the providers could not read just now, which they said, waits before it is tried
        // again, and its trouble is not said twice.
        std::fs::remove_file(scratch.0.join("models/local.json")).unwrap();
        let unread = Providers::read(vec![file], Some(logins.clone()));
        let listings = unread.listings().to_vec();
        assert!(listings[0].offered().is_err());
        assert_eq!(lists.look(&logins, &kept(&listings), start, never), None);
        let failing = |_: &Source| Err("offline".to_owned());
        let later = start + RELIST_AFTER;
        assert_eq!(lists.look(&logins, &kept(&listings), later, failing), None);
        let read = |_: &Source| {
            saved(0.0, &["c"]);
            Ok(None)
        };
        let said = lists.look(&logins, &kept(&listings), later + RELIST_AFTER, read);
        assert_eq!(said, Some(vec!["local now offers c (/model)".to_owned()]));
        // A long list is cut short.
        let ids = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"];
        assert_eq!(few(&ids), "a, b, c, d, e, f, g, h, i, j and 2 more");
        assert_eq!(few(&ids[..11]), ids[..11].join(", "));
    }

    #[test]
    fn the_lists_of_packages_logins_are_kept_current_while_logged_in() {
        let scratch = Scratch(
            std::env::temp_dir().join(format!("scramjet-rs-relist-program-{}", std::process::id())),
        );
        let logins = login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        std::fs::create_dir_all(&logins.lists).unwrap();
        let file = scratch.0.join("models.json");
        let declared = json!({"logins": {"acme": {"name": "Acme", "command": ["./login"]}}});
        std::fs::write(&file, declared.to_string()).unwrap();
        let saved = |minutes: f64, ids: &[&str]| {
            let models: Vec<Value> = ids.iter().map(|id| json!({"id": id})).collect();
            let entry = json!({"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
                "models": models});
            let list = json!({"fetched": now_ms() - minutes * 60_000.0, "provider": entry});
            std::fs::write(logins.lists.join("acme.json"), list.to_string()).unwrap();
        };
        std::fs::write(&logins.auth, r#"{"acme": {"type": "oauth"}}"#).unwrap();
        saved(1.0, &["a"]);
        let providers = Providers::read(vec![file], Some(logins.clone()));
        let kept = Kept::of(&providers);
        assert_eq!(kept.programs.len(), 1);
        let mut lists = Lists::new(&logins, &kept);
        let start = Instant::now();
        let never =
            |_: &Source| -> Result<Option<String>, String> { panic!("read again while fresh") };
        assert_eq!(lists.look(&logins, &kept, start, never), None);
        saved(16.0, &["a"]);
        let relisted = |source: &Source| {
            assert!(matches!(source, Source::Program(program) if program.provider == "acme"));
            saved(0.0, &["a", "b"]);
            Ok(Some("one source is stale".to_owned()))
        };
        assert_eq!(
            lists.look(&logins, &kept, start, relisted),
            Some(vec![
                "Warning: one source is stale".to_owned(),
                "Acme now offers b (/model)".to_owned(),
            ])
        );
        // Gone from the files, or logged out of, its models go.
        assert_eq!(
            lists.look(&logins, &Kept::default(), start, never),
            Some(vec![])
        );
        assert_eq!(lists.look(&logins, &kept, start, never), Some(vec![]));
        std::fs::write(&logins.auth, "{}").unwrap();
        assert_eq!(lists.look(&logins, &kept, start, never), Some(vec![]));
        assert!(lists.models.is_empty());
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
        let mut lists = Lists::new(&logins, &Kept::default());
        let start = Instant::now();
        let never =
            |_: &Source| -> Result<Option<String>, String> { panic!("read again while fresh") };
        assert_eq!(lists.look(&logins, &Kept::default(), start, never), None);
        // Old enough, it is read again, and what it offers now and no longer is said.
        list(&logins, 16.0, &["a", "b"]);
        let relisted = |source: &Source| {
            assert_eq!(source.key(), "github-copilot");
            list(&logins, 0.0, &["a", "c"]);
            Ok(None)
        };
        assert_eq!(
            lists.look(&logins, &Kept::default(), start, relisted),
            Some(vec![
                "GitHub Copilot now offers c (/model)".to_owned(),
                "GitHub Copilot no longer offers b".to_owned(),
            ])
        );
        // Another process read it: taken up, nothing said.
        list(&logins, 0.0, &["a", "c", "d"]);
        assert_eq!(
            lists.look(&logins, &Kept::default(), start, never),
            Some(vec![])
        );
        // Trouble is said once, the list kept, and tried again only after a while.
        list(&logins, 16.0, &["a", "c", "d"]);
        let tries = Cell::new(0);
        let failing = |_: &Source| {
            tries.set(tries.get() + 1);
            Err("offline".to_owned())
        };
        assert_eq!(
            lists.look(&logins, &Kept::default(), start, failing),
            Some(vec![
                "Warning: GitHub Copilot's models could not be read again, so those it listed \
                 before are offered: offline"
                    .to_owned()
            ])
        );
        assert_eq!(lists.look(&logins, &Kept::default(), start, failing), None);
        let later = start + RELIST_AFTER;
        assert_eq!(lists.look(&logins, &Kept::default(), later, failing), None);
        assert_eq!(tries.get(), 2);
        // Read well, a later trouble is said again.
        let unchanged = |_: &Source| {
            list(&logins, 0.0, &["a", "c", "d"]);
            Ok(None)
        };
        assert_eq!(
            lists.look(&logins, &Kept::default(), later + RELIST_AFTER, unchanged),
            None
        );
        list(&logins, 16.0, &["a", "c", "d"]);
        let warned = |_: &Source| Ok(Some("npm is out of reach".to_owned()));
        let said = lists.look(&logins, &Kept::default(), later + RELIST_AFTER, warned);
        assert_eq!(said, Some(vec!["Warning: npm is out of reach".to_owned()]));
        // Logged out, its models are gone.
        std::fs::write(&logins.auth, "{}").unwrap();
        assert_eq!(
            lists.look(&logins, &Kept::default(), later, never),
            Some(vec![])
        );
        assert!(lists.models.is_empty());
    }
}
