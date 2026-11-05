mod args;
mod children;
mod clipboard;
mod completion;
mod diff;
mod editor;
mod exit;
mod export;
mod external;
mod file_search;
mod footer;
mod fuzzy;
mod highlight;
mod hotkeys;
mod install;
mod installed;
mod interactive;
mod job;
mod keymap;
mod keys;
mod markdown;
mod record;
mod scope;
mod screen;
mod select;
mod selection;
mod sessions;
mod settings;
mod settings_menu;
mod slash;
mod style;
mod terminal;
mod text;
mod theme;
mod tree;

use args::{Options, Start, USAGE};
use rustix::termios::{self, QueueSelector};
use scramjet_rs::agent::{
    self, Agent, AgentConfig, AgentUi, Attached, CODING_TOOLS, Found, HARNESS_TOOLS, ModelOption,
    Next, Session, SessionChoice, SkillOptions,
};
use settings::Settings;
use std::{
    ffi::OsString,
    io::IsTerminal,
    os::fd::BorrowedFd,
    path::{Path, PathBuf},
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
        let rest = rest.unwrap_or_default();
        if let Some(result) = clipboard::run(command, &rest) {
            return result;
        }
        if let Some(result) = install::run(command, &rest) {
            return result;
        }
    }
    let Some(mut options) = Options::parse(arguments.into_iter())? else {
        println!("{USAGE}");
        return Ok(());
    };
    if options.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Some(search) = options.list_models.take() {
        let models = endpoint_models(options.endpoint)?;
        print!("{}", scope::listing(&models, search.as_deref()));
        return Ok(());
    }
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
    let (print_only, resume) = (options.print, matches!(options.start, Start::Resume));
    let root = project_root(options.project.take())?;
    let Some(choice) = session_choice(std::mem::replace(&mut options.start, Start::New), &root)?
    else {
        eprintln!("Aborted.");
        return Ok(());
    };
    let mut settings = Settings::load();
    let themes = theme::Themes::discover(&root);
    style::set(&themes.theme(&settings.theme));
    let (patterns, source) = match options.models.take() {
        Some(patterns) => (patterns, "--models"),
        None => (settings.enabled_models.clone(), "settings: enabledModels"),
    };
    let (mut agent, scope_warnings) = start(options, root, choice, &settings, (&patterns, source))?;
    let cancel = Arc::new(AtomicBool::new(false));
    let hide_thinking = Arc::new(AtomicBool::new(settings.hide_thinking_block));
    handle_signals(&cancel, &hide_thinking)?;
    let mut ui = Terminal::new(hide_thinking.clone(), true);
    ui.markdown_reads = settings.markdown_reads;
    if !print_only {
        let mut interactive =
            interactive::Interactive::new(agent, ui, cancel, hide_thinking, settings, themes)?;
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
    for problem in themes.problems(&settings.theme) {
        ui.notice(&problem.text);
    }
    for warning in &scope_warnings {
        ui.notice(warning);
    }
    agent.set_hide_credentials(settings.hide_credentials.hides(true));
    if let Some(files) = files {
        agent.attach(files);
    }
    let result = (messages.into_iter()).try_for_each(|prompt| {
        let model = agent.model().to_owned();
        let result = print(&mut agent, &mut ui, prompt, &cancel);
        // The agent's `switch_scramjet_model`, remembered as the user's choices are.
        if agent.model() != model
            && let Err(error) = settings.remember_model(agent.root(), agent.model())
        {
            eprintln!("Error: settings not saved: {error}");
        }
        result
    });
    if let Err(error) = settings.set_hide_thinking_block(hide_thinking.load(Ordering::Relaxed)) {
        eprintln!("Error: settings not saved: {error}");
    }
    result
}

/// The project directory `--project` names, else the current one, as an absolute path.
fn project_root(project: Option<PathBuf>) -> Result<PathBuf, String> {
    let root = match project {
        Some(path) => path,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    Ok(root)
}

/// The session the agent starts in for the project `root`, the one `--session` or `--fork` names
/// found as Scramjet finds it; `None` when the user declines to fork one of another project here.
fn session_choice(start: Start, root: &Path) -> Result<Option<SessionChoice>, String> {
    let find = |named: &str| -> Result<Found, String> {
        // As Scramjet's, a word that looks like a path is a session file, wherever it is.
        if named.contains(['/', '\\']) || named.ends_with(".jsonl") {
            return Ok(Found::Here(PathBuf::from(named)));
        }
        let directory = agent::session_directory(root).map_err(|e| e.to_string())?;
        Session::find(&directory, named)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .ok_or_else(|| format!("no session found matching {named:?}"))
    };
    Ok(Some(match start {
        Start::New | Start::Resume => SessionChoice::New,
        Start::Continue => SessionChoice::Continue,
        Start::Unsaved => SessionChoice::Unsaved,
        Start::Fork(named) => SessionChoice::Fork(find(&named)?.path().to_owned()),
        Start::Session(named) => match find(&named)? {
            Found::Here(path) => SessionChoice::Open(path),
            Found::Elsewhere { path, root } => match fork_here(&named, &root)? {
                true => SessionChoice::Fork(path),
                false => return Ok(None),
            },
        },
    }))
}

/// Asks, as Scramjet does, whether to fork the session `named` of the project `other` into this
/// one; when the input is not a terminal, says how to instead.
fn fork_here(named: &str, other: &Path) -> Result<bool, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "session {named:?} belongs to another project, {}; --fork {named:?} continues it in \
a new session of this one",
            other.display()
        ));
    }
    eprintln!("Session found in different project: {}", other.display());
    eprint!("Fork this session into this project? [y/N] ");
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Starts the agent in the project `root`, in the session `choice`. A model or effort the options
/// give (`--model`'s `:level` too, unless `--thinking` gives one) wins over the one a resumed
/// session last used, and a model pattern naming none of the endpoint's is refused. As in
/// Scramjet, a new session without one starts with the model chosen before (`remembered`) when the scope `patterns` choose it or there is no scope,
/// else with the scope's first model, at the effort its pattern sets; without either, with
/// `gpt-6-luna`. Without an effort either, the effort is the settings' default thinking level,
/// else Scramjet's default, `medium`. Also returns what in `patterns` matched nothing, after the
/// option or setting they come from, and the models remembered that the endpoint lacks.
fn start(
    options: Options,
    root: PathBuf,
    choice: SessionChoice,
    settings: &Settings,
    (patterns, source): (&[String], &str),
) -> Result<(Agent, Vec<String>), String> {
    // Before the session starts, so that a pattern naming no model leaves none behind.
    let named = match &options.model {
        Some(pattern) => Some(named_model(pattern, options.endpoint)?),
        None => None,
    };
    let chosen = named.is_some();
    let chosen_effort = (options.effort.clone()).or_else(|| named.as_ref()?.level.clone());
    let unknown_tools: Vec<String> = (options.only_tools.iter().flatten())
        .filter(|name| {
            !CODING_TOOLS.contains(&name.as_str()) && !HARNESS_TOOLS.contains(&name.as_str())
        })
        .map(|name| format!("--tools: {name} is not one of scramjet-rs's tools; ignored"))
        .collect();
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let skill_options = SkillOptions {
        paths: (options.skills.iter())
            .map(|path| given_path(path, &cwd))
            .collect(),
        no_skills: options.no_skills,
    };
    let config = AgentConfig {
        endpoint: options.endpoint,
        model: named.map_or_else(|| "gpt-6-luna".to_owned(), |named| named.id),
        root,
        effort: chosen_effort.clone(),
        max_output_tokens: options.max_output_tokens,
        ask: options.ask,
        retries: options.retries,
        tools: settings.tools.clone(),
        only_tools: options.only_tools,
        command_sets: settings.command_sets.clone(),
        skills: settings.skills.clone(),
        skill_options,
        prompt: options.prompt,
    };
    let mut agent = Agent::start(config, choice).map_err(|e| e.to_string())?;
    let (scoped, warnings) = match patterns.is_empty() {
        true => Default::default(),
        false => scope::resolve(patterns, &agent.model_options()),
    };
    let mut warnings: Vec<String> = (warnings.iter())
        .map(|warning| format!("{source}: {warning}"))
        .chain(unknown_tools)
        .collect();
    agent.set_model_scope(scoped.iter().map(|scoped| scoped.id.clone()).collect());
    let new = !chosen && agent.last_model().is_none();
    let remembered = new
        .then(|| remembered(settings, &agent, &mut warnings))
        .flatten();
    let in_scope = (scoped.iter())
        .find(|scoped| remembered.as_ref() == Some(&scoped.id))
        .or(scoped.first())
        .filter(|_| new);
    let model = match agent.last_model() {
        _ if let Some(scoped) = in_scope => scoped.id.clone(),
        _ if let Some(remembered) = remembered => remembered,
        Some(last) if !chosen => last.to_owned(),
        _ => agent.model().to_owned(),
    };
    agent.set_model(model).map_err(|e| e.to_string())?;
    let effort = chosen_effort
        .or_else(|| agent.last_effort().map(str::to_owned))
        .or_else(|| in_scope.and_then(|scoped| scoped.level.clone()))
        .or_else(|| settings.default_thinking_level.clone())
        .unwrap_or_else(|| "medium".to_owned());
    agent.set_effort(effort).map_err(|e| e.to_string())?;
    agent.load_model_card();
    Ok((agent, warnings))
}

/// A path the command line gives, as Scramjet resolves `--skill`'s: `~` is the home directory,
/// and a relative path is from `cwd`.
fn given_path(path: &str, cwd: &Path) -> PathBuf {
    let path = path.trim();
    match std::env::home_dir() {
        Some(home) if path == "~" || path.starts_with("~/") => {
            home.join(path[1..].trim_start_matches('/'))
        }
        _ => cwd.join(path),
    }
}

/// The endpoint's models, or why they cannot be listed.
fn endpoint_models(endpoint: std::net::SocketAddr) -> Result<Vec<ModelOption>, String> {
    match agent::endpoint_models(endpoint) {
        Ok(Some(models)) => Ok(models),
        Ok(None) => Err(format!(
            "could not list the models of the endpoint {endpoint}"
        )),
        Err(error) => Err(error.to_string()),
    }
}

/// The model `--model` names, with the level its `:level` sets. When the endpoint cannot list
/// its models, `pattern` is taken as an id, as before it could be checked.
fn named_model(pattern: &str, endpoint: std::net::SocketAddr) -> Result<scope::Scoped, String> {
    let Ok(models) = endpoint_models(endpoint) else {
        let id = pattern.trim().to_owned();
        return Ok(scope::Scoped { id, level: None });
    };
    scope::named(pattern, &models).ok_or_else(|| {
        format!(
            "--model {pattern:?} names none of the endpoint's models (--list-models lists them)"
        )
    })
}

/// The model chosen before that a new session in the agent's project starts with
/// (`Settings::remembered_models`): the first the endpoint lists, or either when it cannot list
/// its models. Says in `warnings` which of them it does not list.
fn remembered(settings: &Settings, agent: &Agent, warnings: &mut Vec<String>) -> Option<String> {
    let (here, anywhere) = settings.remembered_models(agent.root());
    let anywhere = anywhere.filter(|model| here.as_ref() != Some(model));
    if here.is_none() && anywhere.is_none() {
        return None;
    }
    let listed = agent.models();
    let path = settings.shown_path().unwrap_or_default();
    let remembered = [
        here.map(|model| {
            (
                format!("{model}, the model last chosen in this project,"),
                model,
            )
        }),
        anywhere.map(|model| (format!("settings: {path}: defaultModel {model}"), model)),
    ];
    for (what, model) in remembered.into_iter().flatten() {
        if listed.as_ref().is_none_or(|ids| ids.contains(&model)) {
            return Some(model);
        }
        warnings.push(format!("{what} is not one the endpoint lists; not used"));
    }
    None
}

/// Ctrl-C sets `cancel`, and a second one while it is set exits; Ctrl+T during a turn (SIGQUIT,
/// see `InputMode::turn`) flips `hide_thinking`. Ctrl-Z stops and continues as `job` says, and
/// SIGTERM and SIGHUP exit as `exit` says.
fn handle_signals(cancel: &Arc<AtomicBool>, hide_thinking: &Arc<AtomicBool>) -> Result<(), String> {
    let startup = termios::tcgetattr(std::io::stdin()).ok();
    job::install(startup.clone()).map_err(|e| e.to_string())?;
    exit::install(startup).map_err(|e| e.to_string())?;
    let flag = cancel.clone();
    // SAFETY: the handler only swaps an atomic and calls tcflush and `exit::on_signal`, all
    // async-signal-safe.
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
            // A second Ctrl-C while a cancellation is still pending exits.
            if flag.swap(true, Ordering::Relaxed) {
                exit::on_signal(signal_hook::consts::SIGINT);
                return;
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
    ui.print_usage(agent);
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
