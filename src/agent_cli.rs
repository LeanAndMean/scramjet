mod args;
mod children;
mod clipboard;
mod diff;
mod editor;
mod export;
mod file_search;
mod fuzzy;
mod install;
mod installed;
mod interactive;
mod job;
mod markdown;
mod record;
mod scope;
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
use scramjet_rs::agent::{Agent, AgentConfig, AgentUi, Attached, Next};
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
    let arguments: Vec<OsString> = arguments.collect();
    if let Some((command, rest)) = arguments.split_first()
        && let Some(command) = command.to_str()
    {
        let rest: Option<Vec<String>> = rest.iter().map(|a| a.to_str().map(String::from)).collect();
        if let Some(result) = install::run(command, &rest.unwrap_or_default()) {
            return result;
        }
    }
    let Some(mut options) = Options::parse(arguments.into_iter())? else {
        println!("{USAGE}");
        return Ok(());
    };
    if let Some(file) = options.export.take() {
        let output = options.messages.first().map(std::path::PathBuf::from);
        let written = export::file(&file, output)?;
        println!("Exported to: {}", written.display());
        return Ok(());
    }
    let mut messages = std::mem::take(&mut options.messages);
    let files = match options.files.is_empty() {
        true => None,
        false => {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            Some(Attached::read(&options.files, &cwd)?)
        }
    };
    // Files alone are a prompt, as in Scramjet.
    if files.is_some() && messages.is_empty() {
        messages.push(String::new());
    }
    let (print_only, resume) = (options.print, options.resume);
    let mut settings = Settings::load();
    let (patterns, source) = match options.models.take() {
        Some(patterns) => (patterns, "--models"),
        None => (settings.enabled_models.clone(), "settings: enabledModels"),
    };
    let (mut agent, scope_warnings) = start(options, &settings, &patterns)?;
    let scope_warnings: Vec<String> = (scope_warnings.iter())
        .map(|warning| format!("{source}: {warning}"))
        .collect();
    let cancel = Arc::new(AtomicBool::new(false));
    let hide_thinking = Arc::new(AtomicBool::new(settings.hide_thinking_block));
    handle_signals(&cancel, &hide_thinking)?;
    let mut ui = Terminal::new(hide_thinking.clone(), true);
    if !print_only {
        let mut interactive =
            interactive::Interactive::new(agent, ui, cancel, hide_thinking, settings)?;
        interactive.scope = patterns;
        return interactive.run(resume, messages, files, scope_warnings);
    }
    let settings_warnings = std::mem::take(&mut settings.warnings);
    for warning in settings_warnings
        .iter()
        .map(|warning| format!("settings: {warning}"))
    {
        ui.notice(&warning);
    }
    for warning in &scope_warnings {
        ui.notice(warning);
    }
    agent.set_hide_credentials(settings.hide_credentials.hides(true));
    if let Some(files) = files {
        agent.attach(files);
    }
    let result =
        (messages.into_iter()).try_for_each(|prompt| print(&mut agent, &mut ui, prompt, &cancel));
    if let Err(error) = settings.set_hide_thinking_block(hide_thinking.load(Ordering::Relaxed)) {
        eprintln!("error: settings not saved: {error}");
    }
    result
}

/// Starts the agent in the project the options name. A model or effort they give wins over the
/// one a resumed session last used; a new session without one starts with the first model the
/// scope `patterns` choose, as Scramjet's, and with the effort its pattern sets. Without one
/// either, the effort is the settings' default thinking level, else Scramjet's default,
/// `medium`. Also returns what in `patterns` matched nothing.
fn start(
    options: Options,
    settings: &Settings,
    patterns: &[String],
) -> Result<(Agent, Vec<String>), String> {
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
        command_sets: settings.command_sets.clone(),
        skills: settings.skills.clone(),
    };
    let mut agent = Agent::start(config, options.choice).map_err(|e| e.to_string())?;
    let (scoped, warnings) = match patterns.is_empty() {
        true => Default::default(),
        false => scope::resolve(patterns, &agent.model_options()),
    };
    let first = scoped
        .first()
        .filter(|_| !chosen && agent.last_model().is_none());
    let model = match agent.last_model() {
        _ if let Some(first) = first => first.id.clone(),
        Some(last) if !chosen => last.to_owned(),
        _ => agent.model().to_owned(),
    };
    agent.set_model(model).map_err(|e| e.to_string())?;
    let effort = chosen_effort
        .or_else(|| agent.last_effort().map(str::to_owned))
        .or_else(|| first.and_then(|first| first.level.clone()))
        .or_else(|| settings.default_thinking_level.clone())
        .unwrap_or_else(|| "medium".to_owned());
    agent.set_effort(effort).map_err(|e| e.to_string())?;
    agent.load_model_card();
    Ok((agent, warnings))
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

/// `-p`: runs a prompt (`-` reads it from stdin) and names the next step that would run after it,
/// with `/autopilot on` too, instead of running it.
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
        let routing = agent.route(&completed, false, true);
        let mut notices = routing.notices;
        if let Next::Run { steps, chosen } = routing.next {
            let step = &steps[chosen];
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
