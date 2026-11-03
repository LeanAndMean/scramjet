mod args;
mod children;
mod clipboard;
mod diff;
mod editor;
mod file_search;
mod fuzzy;
mod interactive;
mod job;
mod markdown;
mod record;
mod screen;
mod select;
mod selection;
mod settings;
mod slash;
mod style;
mod terminal;
mod text;
mod tree;

use args::{Options, USAGE};
use rustix::termios::{self, OptionalActions, QueueSelector};
use scramjet_rs::agent::{Agent, AgentConfig, AgentUi, Next};
use settings::Settings;
use std::{
    ffi::OsString,
    os::fd::BorrowedFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use terminal::{InputMode, Terminal};

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let Some(mut options) = Options::parse(arguments)? else {
        println!("{USAGE}");
        return Ok(());
    };
    let prompt = options.prompt.take();
    let mut settings = Settings::load();
    let mut agent = start(options, &settings)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let hide_thinking = Arc::new(AtomicBool::new(settings.hide_thinking_block));
    handle_signals(&cancel, &hide_thinking)?;
    let mut ui = Terminal::new(hide_thinking.clone(), true);
    let Some(prompt) = prompt else {
        let interactive =
            interactive::Interactive::new(agent, ui, cancel, hide_thinking, settings)?;
        return interactive.run();
    };
    for warning in std::mem::take(&mut settings.warnings) {
        ui.notice(&format!("settings: {warning}"));
    }
    agent.set_hide_credentials(settings.hide_credentials.hides(true));
    let result = print(&mut agent, &mut ui, prompt, &cancel);
    if let Err(error) = settings.set_hide_thinking_block(hide_thinking.load(Ordering::Relaxed)) {
        eprintln!("error: settings not saved: {error}");
    }
    result
}

/// Starts the agent in the project the options name. A model or effort they give wins over the
/// one a resumed session last used; without either, the effort is the settings' default
/// thinking level, else Scramjet's default, `medium`.
fn start(options: Options, settings: &Settings) -> Result<Agent, String> {
    let root = match options.project {
        Some(path) => path,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let chosen = options.model.is_some();
    let chosen_effort = options.effort.clone();
    let config = AgentConfig {
        endpoint: options.endpoint,
        model: options.model.unwrap_or_else(|| "gpt-6-luna".to_owned()),
        root,
        effort: options.effort,
        max_output_tokens: options.max_output_tokens,
        ask: options.ask,
        retries: options.retries,
        tools: settings.tools.clone(),
    };
    let mut agent = Agent::start(config, options.choice).map_err(|e| e.to_string())?;
    let model = match agent.last_model() {
        Some(last) if !chosen => last.to_owned(),
        _ => agent.model().to_owned(),
    };
    agent.set_model(model).map_err(|e| e.to_string())?;
    let effort = chosen_effort
        .or_else(|| agent.last_effort().map(str::to_owned))
        .or_else(|| settings.default_thinking_level.clone())
        .unwrap_or_else(|| "medium".to_owned());
    agent.set_effort(effort).map_err(|e| e.to_string())?;
    agent.load_model_card();
    Ok(agent)
}

/// Ctrl-C sets `cancel`, and a second one while it is set exits; Ctrl+T during a turn (SIGQUIT,
/// see `InputMode::turn`) flips `hide_thinking`. Ctrl-Z stops and continues as `job` says.
fn handle_signals(cancel: &Arc<AtomicBool>, hide_thinking: &Arc<AtomicBool>) -> Result<(), String> {
    let startup = termios::tcgetattr(std::io::stdin()).ok();
    job::install(startup.clone()).map_err(|e| e.to_string())?;
    let flag = cancel.clone();
    // SAFETY: the handler only swaps an atomic, calls tcflush, tcsetattr and _exit, all
    // async-signal-safe.
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
            // A second Ctrl-C while a cancellation is still pending exits.
            if flag.swap(true, Ordering::Relaxed) {
                if let Some(startup) = &startup {
                    let _ = termios::tcsetattr(
                        BorrowedFd::borrow_raw(0),
                        OptionalActions::Now,
                        startup,
                    );
                }
                signal_hook::low_level::exit(130);
            }
            // Keys typed before it answer nothing and are not left for the shell.
            let _ = termios::tcflush(BorrowedFd::borrow_raw(0), QueueSelector::IFlush);
        })
    }
    .map_err(|e| e.to_string())?;
    let flag = hide_thinking.clone();
    // SAFETY: the handler only flips an atomic.
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGQUIT, move || {
            flag.fetch_xor(true, Ordering::Relaxed);
        })
    }
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// `-p`: runs one prompt (`-` reads it from stdin) and names the next step that would run after
/// it instead of running it.
fn print(
    agent: &mut Agent,
    ui: &mut Terminal,
    prompt: String,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let prompt = if prompt == "-" {
        std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
    } else {
        prompt
    };
    let mode = InputMode::turn();
    let result = agent.turn(&prompt, ui, cancel);
    drop(mode);
    ui.clear();
    ui.usage(agent);
    if let Some(completed) = agent.take_completed() {
        let routing = agent.route(&completed, false);
        let mut notices = routing.notices;
        if let Next::Run(step) = routing.next {
            let fresh = if step.fresh_session {
                " (fresh session)"
            } else {
                ""
            };
            notices.push(format!(
                "scramjet: next would be {}{fresh}",
                text::one_line(&step.message)
            ));
        }
        if !notices.is_empty() {
            eprintln!();
        }
        for notice in &notices {
            ui.notice(notice);
        }
    }
    result.map_err(|e| e.to_string())
}
