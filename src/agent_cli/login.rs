//! `/login` and `/logout`: scramjet-rs's own logins, whose credentials it keeps in its own file
//! (`agent::login`). GitHub Copilot's is a device login: the user enters a code on GitHub's page
//! while the screen waits, as Scramjet's login dialog does.

use super::interactive::Interactive;
use super::screen::{Answer, Dialog, Optional};
use super::select::{self, Picker};
use super::text::clip;
use super::{clipboard, style};
use scramjet_rs::agent::{copilot, login};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
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
        let (provider, shown) = login::LOGINS[index];
        if provider == copilot::PROVIDER {
            self.copilot_login(&logins, shown);
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
        match finished {
            Ok(()) => {
                let saved = logins.auth.display();
                self.say(&format!(
                    "✓ Logged in to {shown}. Credentials saved to {saved}"
                ));
                self.logins_changed();
            }
            Err(error) if error == copilot::CANCELLED => {
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
