//! `/login` → Local models: a runtime found on this machine (Ollama, llama.cpp's server, vLLM at
//! their default ports) or at an address given, then one of its models, checked to call a
//! harmless tool and saved as a provider of the user's `models.json`; saved ones are checked
//! again, or removed, the same way. The runtime itself is never started, changed or stopped.

use super::interactive::Interactive;
use super::screen::{Answer, Dialog, Optional};
use super::select::{self, Picker};
use super::style;
use super::text::{clip, visible};
use scramjet::agent::local::{self, LocalModel, Problem, Runtime, Server, Step, Switch};
use scramjet::agent::{Providers, login};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A window under which the first request may not fit: the system prompt and tools of a session
/// took about 9.5k tokens (2026-11-12).
const SMALL_WINDOW: u64 = 16_384;

/// A row of the runtimes' list.
enum Place {
    Found(Server),
    Saved(String, Value),
    Address,
}

impl Interactive {
    /// The runtimes found and the providers saved before, then an address to give.
    pub(super) fn local_login(&mut self) {
        let Some(file) = self.models_file() else {
            return self.say("Error: neither XDG_CONFIG_HOME nor HOME is set");
        };
        let ports = (Runtime::LOOKED_FOR.iter())
            .map(|runtime| format!("{} ({})", runtime.name(), runtime.port().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(", ");
        let note = format!("Looking on this machine for {ports}…");
        let Some(found) = self.waiting("Local models", &note, local::look_around) else {
            return;
        };
        let saved = match local::saved(&file) {
            Ok(saved) => saved,
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        let at = |entry: &Value| {
            entry["baseUrl"]
                .as_str()
                .map(|url| url.trim_end_matches("/v1").to_owned())
        };
        let mut places: Vec<Place> = Vec::new();
        for server in found {
            let saved = saved
                .iter()
                .find(|(_, entry)| at(entry).as_deref() == Some(&server.url));
            places.push(match saved {
                Some((name, entry)) => Place::Saved(name.clone(), entry.clone()),
                None => Place::Found(server),
            });
        }
        for (name, entry) in &saved {
            let shown = places
                .iter()
                .any(|place| matches!(place, Place::Saved(n, _) if n == name));
            if !shown {
                places.push(Place::Saved(name.clone(), entry.clone()));
            }
        }
        places.push(Place::Address);
        let count = |entry: &Value| entry["models"].as_array().map_or(0, Vec::len);
        let texts: Vec<String> = (places.iter())
            .map(|place| match place {
                Place::Found(server) => {
                    let version = server
                        .version
                        .as_deref()
                        .map(|v| format!(" {v}"))
                        .unwrap_or_default();
                    let models = plural(server.models.len(), "model");
                    format!(
                        "{}{version} at {} · {models}",
                        server.runtime.name(),
                        server.url
                    )
                }
                Place::Saved(name, entry) => format!(
                    "{} at {} {}✓ saved: {}{}",
                    visible(name),
                    at(entry).unwrap_or_default(),
                    style::MUTED,
                    plural(count(entry), "model"),
                    style::RESET
                ),
                Place::Address => "Another address…".to_owned(),
            })
            .collect();
        let notes = (places.iter())
            .map(|place| match place {
                Place::Found(_) => {
                    "Choose a model to check and save; the runtime is left as it is".into()
                }
                Place::Saved(..) => "Add a model, check one again, or remove them".into(),
                Place::Address => {
                    "A runtime elsewhere, or at another port: its address, as host:port".into()
                }
            })
            .collect();
        let row = |index: usize, width| clip(&texts[index], width);
        let keys = select::hint("choose", &[], "cancel");
        let picker = Picker::select("Local models", select::VISIBLE, places.len(), 0, None, row);
        let Some(index) = self.pick(picker.noting(keys, notes)) else {
            return;
        };
        match places.swap_remove(index) {
            Place::Found(server) => self.local_models(&file, server, None),
            Place::Saved(name, entry) => self.local_saved(&file, &name, &entry),
            Place::Address => self.local_address(&file),
        }
    }

    /// Asks for a runtime's address and reads it, with a key when it asks for one.
    fn local_address(&mut self, file: &Path) {
        if let Some(hint) = windows_hint() {
            self.say_styled(style::MUTED, &hint);
        }
        let Some(typed) = self.typed("Runtime address (host:port)", Optional::No) else {
            return;
        };
        let url = match local::address(&typed) {
            Ok(url) => url,
            Err(error) => return self.say(&format!("Error: {error}")),
        };
        self.local_read(file, &url, None);
    }

    /// Reads the runtime at `url` and goes on to its models; asks for a key when it wants one.
    fn local_read(&mut self, file: &Path, url: &str, key: Option<String>) {
        let (reading, given) = (url.to_owned(), key.clone());
        let note = format!("Asking {url} what it is and what it serves…");
        let read = self.waiting("Local models", &note, move || {
            local::probe(&reading, given.as_deref())
        });
        match read {
            None => {}
            Some(Ok(server)) => self.local_models(file, server, key),
            Some(Err(Problem {
                step: Step::Key, ..
            })) if key.is_none() => {
                let title = format!("{url} asks for an API key");
                if let Some(key) = self.typed(&title, Optional::No) {
                    self.local_read(file, url, Some(key));
                }
            }
            Some(Err(problem)) => self.say(&format!("Error: {url}: {problem}")),
        }
    }

    /// A saved provider: its runtime read again to add or check a model, or a model or the
    /// whole provider removed.
    fn local_saved(&mut self, file: &Path, name: &str, entry: &Value) {
        let url = entry["baseUrl"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches("/v1")
            .to_owned();
        let models: Vec<String> = (entry["models"].as_array().into_iter().flatten())
            .filter_map(|model| model["id"].as_str().map(str::to_owned))
            .collect();
        let mut texts = vec![format!("Add or check a model of {url}")];
        texts.extend(models.iter().map(|id| format!("Remove {}", visible(id))));
        texts.push(format!("Remove {} and its models", visible(name)));
        let mut notes = vec!["Reads the runtime again; a model checked is saved anew".to_owned()];
        notes.extend(
            models
                .iter()
                .map(|_| "Only from models.json; the runtime keeps it".to_owned()),
        );
        notes.push("Only from models.json; the runtime is left as it is".into());
        let row = |index: usize, width| clip(&texts[index], width);
        let keys = select::hint("choose", &[], "cancel");
        let picker = Picker::select(name, select::VISIBLE, texts.len(), 0, None, row);
        let Some(index) = self.pick(picker.noting(keys, notes)) else {
            return;
        };
        let removed = match index {
            0 => {
                let key =
                    login::Logins::user().and_then(|logins| login::saved_key(&logins.auth, name));
                return self.local_read(file, &url, key);
            }
            _ if index <= models.len() => {
                let id = &models[index - 1];
                local::remove_model(file, name, id).map(|()| format!("{name}/{id}"))
            }
            _ => local::remove(file, name).map(|()| {
                // A key saved for it would be left for nothing.
                if let Some(logins) = login::Logins::user()
                    && login::saved_key(&logins.auth, name).is_some()
                {
                    let _ = login::logout(&logins, name);
                }
                name.to_owned()
            }),
        };
        match removed {
            Ok(removed) => {
                self.say(&format!(
                    "✓ Removed {} from {}",
                    visible(&removed),
                    file.display()
                ));
                self.providers_changed();
            }
            Err(error) => self.say(&format!("Error: {error}")),
        }
    }

    /// `server`'s models: one chosen is checked to call a tool, then saved. The list comes back
    /// after each, until Escape.
    fn local_models(&mut self, file: &Path, mut server: Server, key: Option<String>) {
        let mut chosen = 0;
        loop {
            let name = match local::name_for(file, &server) {
                Ok(name) => name,
                Err(error) => return self.say(&format!("Error: {error}")),
            };
            let saved = local::saved(file).unwrap_or_default();
            let saved: Vec<String> = (saved.iter().find(|(n, _)| *n == name))
                .and_then(|(_, entry)| entry["models"].as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|model| model["id"].as_str().map(str::to_owned))
                .collect();
            let texts: Vec<String> = (server.models.iter())
                .map(|model| {
                    let mark = match (&model.unfit, saved.contains(&model.id)) {
                        (Some(_), _) => format!(" {}left out{}", style::MUTED, style::RESET),
                        (None, true) => format!(" {}✓ saved{}", style::MUTED, style::RESET),
                        (None, false) => String::new(),
                    };
                    format!("{}{mark}", visible(&model.id))
                })
                .collect();
            let notes = server
                .models
                .iter()
                .map(|model| described(server.runtime, model))
                .collect();
            let row = |index: usize, width| clip(&texts[index], width);
            let keys = select::hint("check and save", &[], "done");
            let title = format!("{} at {}", server.runtime.name(), server.url);
            let picker = Picker::select(&title, select::VISIBLE, texts.len(), chosen, None, row);
            let Some(index) = self.pick(picker.noting(keys, notes)) else {
                return;
            };
            chosen = index;
            let model = server.models[index].clone();
            if let Some(unfit) = &model.unfit {
                self.say(&format!("{} is left out: {unfit}", visible(&model.id)));
                continue;
            }
            if let Some(model) = self.local_check(&server, model, key.as_deref()) {
                self.local_save(file, &name, &server, &model, key.as_deref());
                server.models[index] = model;
            }
        }
    }

    /// Checks that `model` calls a tool; the model to save, with the window the runtime said or
    /// the user gave and how it thinks; `None` when it failed or was stopped.
    fn local_check(
        &mut self,
        server: &Server,
        mut model: LocalModel,
        key: Option<&str>,
    ) -> Option<LocalModel> {
        let id = visible(&model.id);
        self.say_styled(
            style::MUTED,
            &format!(
                "Checking {id}: it is offered one tool, {}, which does nothing, and asked: {}",
                local::CHECK_TOOL,
                local::CHECK_PROMPT
            ),
        );
        let note = format!(
            "Waiting for {id}'s answer; {} may load it first (at most 5 minutes)…",
            server.runtime.name()
        );
        let (checked, asked, key) = (server.clone(), model.id.clone(), key.map(str::to_owned));
        let check = move || local::check_tools(&checked, &asked, key.as_deref());
        let checked = match self.waiting(&format!("Checking {id}"), &note, check)? {
            Ok(checked) => checked,
            Err(problem) => {
                self.say(&format!("Error: {id} was not saved: {problem}"));
                return None;
            }
        };
        self.say(&format!(
            "✓ {id} called {} with {}. One call shows it can call a tool; not how well it uses \
             tools in real work",
            local::CHECK_TOOL,
            checked.arguments
        ));
        model.reasoning = checked.thinks;
        model.switch = checked.switch;
        if let Some(hint) = checked.thinking_as_text {
            self.say(&format!(
                "Note: {id}'s thinking came as text in its answer, where it will show and be \
                 sent back; {hint}"
            ));
        }
        match (model.reasoning, model.switch) {
            (_, Some(Switch::Effort)) => self.say(&format!(
                "{id} thinks; its thinking level is sent as reasoning_effort, off as none"
            )),
            (_, Some(Switch::EffortNoOff)) => self.say(&format!(
                "Note: {id} thinks, and asking it not to (reasoning_effort none) did not stop it: \
                 its thinking level is sent as reasoning_effort, and off is not offered"
            )),
            (_, Some(Switch::ChatTemplate)) => self.say(&format!(
                "{id} thinks, and stopped when asked not to (chat_template_kwargs \
                 enable_thinking): its thinking level turns it on and off"
            )),
            (true, None) => self.say(&format!(
                "Note: {id} thinks, and asking it not to (chat_template_kwargs enable_thinking) \
                 did not stop it, so no thinking level is offered for it"
            )),
            (false, None) => {}
        }
        model.window = checked.window.or(model.window);
        if model.window.is_some() {
            return Some(model);
        }
        let title = format!(
            "{}'s context window in tokens; empty leaves it unknown{}",
            id,
            model
                .trained
                .map(|t| format!(" (the model's own: {t})"))
                .unwrap_or_default()
        );
        loop {
            let typed = self.typed(&title, Optional::Yes)?;
            let typed = typed.trim().replace([',', '_'], "");
            if typed.is_empty() {
                return Some(model);
            }
            match typed.parse::<u64>() {
                Ok(window) if window > 0 => {
                    model.window = Some(window);
                    return Some(model);
                }
                _ => self.say(&format!("Error: {typed} is not a number of tokens")),
            }
        }
    }

    fn local_save(
        &mut self,
        file: &Path,
        name: &str,
        server: &Server,
        model: &LocalModel,
        key: Option<&str>,
    ) {
        if let Err(error) = local::save(file, name, server, model) {
            return self.say(&format!(
                "Error: {} was not saved: {error}",
                visible(&model.id)
            ));
        }
        if let Some(key) = key
            && let Some(logins) = login::Logins::user()
            && let Err(error) =
                login::save_key(&logins.auth, name, key, std::slice::from_ref(&server.url))
        {
            self.say(&format!(
                "Error: the API key for {name} was not saved: {error}"
            ));
        }
        let window = match model.window {
            Some(window) => format!("window {window} tokens"),
            None => "window unknown".to_owned(),
        };
        self.say(&format!(
            "✓ Saved {}/{} in {} ({window}); choose it in /model",
            visible(name),
            visible(&model.id),
            file.display()
        ));
        if model.window.is_some_and(|window| window < SMALL_WINDOW) {
            let over = match server.runtime {
                Runtime::Ollama => {
                    "Ollama cuts a longer prompt from its start without saying so \
                     (OLLAMA_CONTEXT_LENGTH sets its window)"
                }
                _ => "a runtime refuses a request over its window",
            };
            self.say(&format!(
                "Warning: that window is small for an agent: the instructions and tools alone \
                 take about 10k tokens here, and {over}"
            ));
        }
        self.providers_changed();
        self.take_offered_model(Some(&format!("{name}/{}", model.id)));
    }

    /// The user's own `models.json`, beside the settings.
    pub(super) fn models_file(&self) -> Option<PathBuf> {
        let settings = self.settings.path().map(PathBuf::from);
        let directory = settings.and_then(|path| path.parent().map(PathBuf::from));
        let directory = directory.or_else(login::config_directory)?;
        Some(directory.join("models.json"))
    }

    /// Reads the providers again, as `/reload` does.
    fn providers_changed(&mut self) {
        let logins = login::Logins::user();
        self.agent
            .set_providers(Providers::read(self.settings.models_files(), logins));
        if let Some(screen) = &self.screen {
            screen.models(self.agent.models().unwrap_or_default());
        }
        for problem in self.agent.providers().problems().to_vec() {
            self.say(&format!("Warning: {problem}"));
        }
    }

    /// Runs `work` on a thread of its own while a dialog waits with `note`; `None` when Escape
    /// stopped the waiting (the work is then left to end by itself, its result dropped).
    fn waiting<T: Send + 'static>(
        &self,
        title: &str,
        note: &str,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let screen = self.screen.as_ref()?;
        let done = Arc::new(AtomicBool::new(false));
        let (sending, result) = std::sync::mpsc::channel();
        {
            let done = done.clone();
            std::thread::spawn(move || {
                let _ = sending.send(work());
                done.store(true, Ordering::SeqCst);
            });
        }
        screen.ask(Dialog::Wait(title.to_owned(), note.to_owned()), Some(&done));
        match done.load(Ordering::SeqCst) {
            true => result.recv().ok(),
            false => {
                self.say_styled(style::MUTED, "Stopped waiting");
                None
            }
        }
    }

    fn pick(&self, picker: Picker) -> Option<usize> {
        self.screen.as_ref()?.pick(picker)
    }

    fn typed(&self, title: &str, optional: Optional) -> Option<String> {
        let dialog = Dialog::Text(title.to_owned(), String::new(), optional);
        match self.screen.as_ref()?.ask(dialog, None) {
            Some(Answer::Text(Some(typed))) => Some(typed),
            _ => None,
        }
    }
}

/// What the runtime says of `model`, under its row.
fn described(runtime: Runtime, model: &LocalModel) -> String {
    if let Some(unfit) = &model.unfit {
        return format!("Left out: {unfit}");
    }
    let window = match (model.window, model.trained, runtime) {
        (Some(window), _, Runtime::Ollama) => {
            format!("window {window} tokens, as Ollama loaded it")
        }
        (Some(window), _, _) => format!("window {window} tokens"),
        (None, Some(trained), Runtime::Ollama) => {
            format!("window known once loaded (the model's own: {trained}); the check loads it")
        }
        (None, _, _) => "window unknown; you can give it after the check".to_owned(),
    };
    let mut also = Vec::new();
    if model.reasoning {
        also.push("thinks");
    }
    if model.images {
        also.push("sees images");
    }
    match also.is_empty() {
        true => window,
        false => format!("{window}; {}", also.join(", ")),
    }
}

fn plural(count: usize, what: &str) -> String {
    format!("{count} {what}{}", if count == 1 { "" } else { "s" })
}

/// Under WSL, where a runtime on Windows is: `localhost` under mirrored networking, the Windows
/// host's address under NAT (the default gateway), as `wslinfo` tells the mode.
fn windows_hint() -> Option<String> {
    let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if !set("WSL_DISTRO_NAME") && !set("WSL_INTEROP") {
        return None;
    }
    let mode = std::process::Command::new("wslinfo")
        .arg("--networking-mode")
        .stderr(std::process::Stdio::null())
        .output();
    let mode = mode
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
    let gateway = std::fs::read_to_string("/proc/net/route").ok();
    let gateway = gateway.as_deref().and_then(local::default_gateway);
    let nat = |gateway: Option<std::net::Ipv4Addr>| match gateway {
        Some(gateway) => format!("the Windows host's address, {gateway}"),
        None => "the Windows host's address".to_owned(),
    };
    let listen = "and the runtime must listen beyond localhost (Ollama: OLLAMA_HOST=0.0.0.0)";
    Some(match mode.as_deref() {
        Some("mirrored") => {
            "A runtime on Windows is reached at localhost here (WSL's mirrored networking)".into()
        }
        Some("nat") => format!(
            "A runtime on Windows is reached at {}, {listen}",
            nat(gateway)
        ),
        _ => format!(
            "A runtime on Windows is reached at localhost under WSL's mirrored networking, or \
             under NAT at {}, {listen}",
            nat(None)
        ),
    })
}
