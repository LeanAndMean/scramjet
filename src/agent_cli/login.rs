//! `/login` and `/logout`: scramjet-rs's own logins, whose credentials it keeps in its own file
//! (`agent::login`). GitHub Copilot's is a device login: the user enters a code on GitHub's page
//! while the screen waits, as Scramjet's login dialog does. ChatGPT's is a browser login, which
//! comes back by itself, or the user pastes the address the browser ended on.

use super::interactive::Interactive;
use super::screen::{Answer, Dialog, Optional};
use super::select::{self, Picker};
use super::text::clip;
use super::{clipboard, style};
use scramjet_rs::agent::{codex, copilot, login};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

impl Interactive {
    /// `/login`: the providers to log in to, each marked when logged in, then its login.
    pub(super) fn login(&mut self) {
        let Some(logins) = login::Logins::user() else {
            return self.say("Error: neither XDG_CONFIG_HOME nor HOME is set");
        };
        let Some(screen) = &self.screen else {
            return self.say("Error: /login needs the full screen");
        };
        let logged_in = login::logged_in(&logins.auth).unwrap_or_default();
        let notes = (login::LOGINS.iter())
            .map(
                |(name, _)| match logged_in.iter().any(|known| known == name) {
                    true => "Logged in; logging in again replaces the saved login".to_owned(),
                    false => String::new(),
                },
            )
            .collect();
        let row = |index: usize, width| {
            let (name, shown) = login::LOGINS[index];
            let mark = match logged_in.iter().any(|known| known == name) {
                true => format!(" {}✓ logged in{}", style::MUTED, style::RESET),
                false => String::new(),
            };
            clip(&format!("{shown}{mark}"), width)
        };
        let keys = select::hint("log in", &[], "cancel");
        let picker = Picker::select(
            "Log in to",
            select::VISIBLE,
            login::LOGINS.len(),
            0,
            None,
            row,
        );
        let Some(index) = screen.pick(picker.noting(keys, notes)) else {
            return;
        };
        match login::LOGINS[index] {
            (copilot::PROVIDER, shown) => self.copilot_login(&logins, shown),
            (codex::PROVIDER, shown) => self.codex_login(&logins, shown),
            _ => {}
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
        let chosen = match &self.screen {
            Some(screen) => {
                let row = |index: usize, width| clip(login::shown_name(&logged_in[index]), width);
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
            Ok(_) => {
                self.say(&format!("✓ Logged out of {}", login::shown_name(provider)));
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
