/// `println!` and `print!` for the commands' stdout, quiet when it is closed (`| head`).
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout(), $($arg)*);
    }};
}

mod args;
mod background;
mod banner;
mod changelog;
mod children;
mod clipboard;
mod completion;
mod console;
mod design;
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
mod import;
mod install;
mod installed;
mod interactive;
mod job;
mod keymap;
mod keys;
mod lint;
mod login;
mod markdown;
mod packages;
mod palette;
mod programs;
mod record;
mod report;
mod scope;
mod screen;
mod select;
mod selection;
mod sessions;
mod settings;
mod settings_menu;
mod slash;
mod spelling;
mod style;
mod terminal;
mod text;
mod theme;
mod theme_editor;
mod tree;
mod update;
mod usage;

use args::{Options, Start, USAGE};
use scramjet::agent::{
    self, Agent, AgentConfig, AgentUi, Attached, CODING_TOOLS, Found, HARNESS_TOOLS, ModelOption,
    Next, Providers, Sandbox, Session, SessionChoice, SkillOptions,
};
use settings::Settings;
use std::{
    ffi::OsString,
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use terminal::{InputMode, Terminal};

pub fn execute(arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    let mut arguments: Vec<OsString> = arguments.collect();
    // As Scramjet 1's, `--offline` anywhere stands for `SCRAMJET_OFFLINE=1`.
    let before = arguments.len();
    arguments.retain(|argument| argument != "--offline");
    if arguments.len() != before {
        // SAFETY: the process has started no thread yet.
        unsafe { std::env::set_var("SCRAMJET_OFFLINE", "1") };
    }
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
        if let Some(result) = lint::run(command, &rest) {
            return result;
        }
        if let Some(result) = import::run(command, &rest) {
            return result;
        }
    }
    let Some(mut options) = Options::parse(arguments.into_iter())? else {
        outln!("{USAGE}");
        return Ok(());
    };
    if options.version {
        outln!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Some(search) = options.list_models.take() {
        let providers = Providers::read(
            Settings::load().models_files(),
            agent::login::Logins::user(),
        );
        for problem in providers.problems() {
            eprintln!("Warning: {problem}");
        }
        out!(
            "{}",
            scope::listing(&providers.options(), search.as_deref())
        );
        return Ok(());
    }
    if options.list_credential_stores {
        let providers = Providers::read(
            Settings::load().models_files(),
            agent::login::Logins::user(),
        );
        for problem in providers.problems() {
            eprintln!("Warning: {problem}");
        }
        for store in agent::credential_stores(&providers) {
            outln!("{}", store.display());
        }
        return Ok(());
    }
    if let Some(file) = options.export.take() {
        let output = options.messages.first().map(std::path::PathBuf::from);
        let written = export::file(&file, output)?;
        outln!("Exported to: {}", written.display());
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
    let mut settings = Settings::load_cloning();
    settings.retries_given = options.retries;
    scramjet::agent::set_shell(
        settings.shell_path.clone(),
        settings.shell_command_prefix.clone(),
    );
    scramjet::agent::set_user_shell(settings.default_shell);
    scramjet::agent::set_images(settings.resize_images, settings.block_images);
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let session_dir = (options.session_dir.as_deref())
        .or(settings.session_dir.as_deref())
        .map(|directory| given_path(directory, &cwd));
    let first = std::mem::replace(&mut options.start, Start::New);
    let Some(choice) = session_choice(first, &root, session_dir.as_deref())? else {
        eprintln!("Aborted.");
        return Ok(());
    };
    let theme_options = theme::ThemeOptions {
        paths: (options.themes.iter())
            .map(|path| given_path(path, &cwd))
            .collect(),
        no_themes: options.no_themes,
    };
    let themes = theme::Themes::discover(&root, &settings.themes, &theme_options);
    style::set(&themes.theme(settings.theme_name()));
    let (patterns, source) = match options.models.take() {
        Some(patterns) => (patterns, "--models"),
        None => (settings.enabled_models.clone(), "settings: enabledModels"),
    };
    let (mut agent, scope_warnings) = start(
        options,
        (root, session_dir),
        choice,
        &settings,
        (&patterns, source),
    )?;
    let cancel = Arc::new(AtomicBool::new(false));
    let hide_thinking = Arc::new(AtomicBool::new(settings.hide_thinking_block));
    handle_signals(&cancel, &hide_thinking)?;
    let mut ui = Terminal::new(hide_thinking.clone(), true);
    ui.markdown_reads = settings.markdown_reads;
    if !print_only {
        let themes = (themes, theme_options);
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
    for problem in themes.problems(settings.theme_name()) {
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
            && let Err(error) =
                settings.remember_model(agent.root(), agent.model_provider(), agent.model_id())
        {
            eprintln!("Error: settings not saved: {error}");
        }
        result
    });
    if let Err(error) = settings.set_hide_thinking_block(hide_thinking.load(Ordering::Relaxed)) {
        eprintln!("Error: settings not saved: {error}");
    }
    write_held_at_exit(&mut agent);
    result
}

/// The project directory `--project` names, else the current one, as an absolute path.
fn project_root(project: Option<PathBuf>) -> Result<PathBuf, String> {
    let root = match project {
        Some(path) => path,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let root = scramjet::os::canonicalize(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    Ok(root)
}

/// The session the agent starts in for the project `root`, the one `--session` or `--fork` names
/// found as Scramjet finds it; `None` when the user declines to fork one of another project here.
fn session_choice(
    start: Start,
    root: &Path,
    session_dir: Option<&Path>,
) -> Result<Option<SessionChoice>, String> {
    let find = |named: &str| -> Result<Found, String> {
        // As Scramjet's, a word that looks like a path is a session file, wherever it is.
        if named.contains(['/', '\\']) || named.ends_with(".jsonl") {
            return Ok(Found::Here(PathBuf::from(named)));
        }
        let projects = agent::projects_directory().map_err(|e| e.to_string())?;
        let directory = match session_dir {
            Some(directory) => directory.to_owned(),
            None => agent::session_directory(root).map_err(|e| e.to_string())?,
        };
        Session::find(&directory, &projects, named)
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
/// session last used, and a model pattern naming none of the providers' is refused. As in
/// Scramjet, a new session without one starts with the model chosen before (`remembered`) when the scope `patterns` choose it or there is no scope,
/// else with the scope's first model, at the effort its pattern sets; without either, with
/// `gpt-6-luna`. Without an effort either, the effort is the settings' default thinking level,
/// else Scramjet's default, `medium`. Also returns what in `patterns` matched nothing, after the
/// option or setting they come from, the models remembered that the providers lack, and what is
/// wrong in the providers' files.
fn start(
    options: Options,
    (root, session_dir): (PathBuf, Option<PathBuf>),
    choice: SessionChoice,
    settings: &Settings,
    (patterns, source): (&[String], &str),
) -> Result<(Agent, Vec<String>), String> {
    let api_key = options.api_key.clone();
    let providers = Providers::read(settings.models_files(), agent::login::Logins::user());
    // Before the session starts, so that a pattern naming no model leaves none behind.
    let named = match &options.model {
        Some(pattern) => Some(named_model(
            pattern,
            options.provider.as_deref(),
            &providers,
        )?),
        None => None,
    };
    let chosen = named.is_some();
    let chosen_effort = (options.effort.clone()).or_else(|| named.as_ref()?.level.clone());
    let unknown_tools: Vec<String> = (options.only_tools.iter().flatten())
        .filter(|name| {
            !CODING_TOOLS.contains(&name.as_str()) && !HARNESS_TOOLS.contains(&name.as_str())
        })
        .map(|name| format!("--tools: {name} is not one of Scramjet's tools; ignored"))
        .collect();
    let missing_shells =
        agent::missing_shells(options.only_tools.as_ref().unwrap_or(&settings.tools));
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let skill_options = SkillOptions {
        paths: (options.skills.iter())
            .map(|path| given_path(path, &cwd))
            .collect(),
        no_skills: options.no_skills,
    };
    let mut warnings: Vec<String> = providers.problems().to_vec();
    if let (Some(provider), None) = (&options.provider, &options.model) {
        warnings.push(format!(
            "--provider {provider} chooses no model without --model; ignored"
        ));
    }
    let (retention, wrong) = cache_retention(options.cache_retention);
    agent::set_cache_retention(retention);
    warnings.extend(wrong);
    if providers.options().is_empty() {
        let file = (providers.files().first())
            .map_or("models.json".into(), |file| file.display().to_string());
        // As Scramjet's, a run without the full screen stops here: no request could be sent.
        if options.print {
            return Err(format!(
                "no provider offers any model: start scramjet and /login, or add providers to \
{file} (see --help)"
            ));
        }
        warnings.push(format!(
            "No provider offers any model: /login, or add providers to {file} (see --help)"
        ));
    }
    let model = named.map_or_else(|| default_model(&providers), |named| named.model);
    let sandbox = (options.sandbox.unwrap_or(settings.sandbox)).map(|mode| Sandbox {
        mode,
        network: settings.sandbox_network,
    });
    if sandbox.is_some() {
        Sandbox::check()?;
    }
    let config = AgentConfig {
        providers,
        model,
        root,
        effort: chosen_effort.clone(),
        max_output_tokens: options.max_output_tokens,
        ask: options.ask,
        retries: settings.retries(),
        compacting: settings.compacting,
        tools: settings.tools.clone(),
        only_tools: options.only_tools,
        command_sets: settings.command_sets.clone(),
        skills: settings.skills.clone(),
        skill_options,
        prompt: options.prompt,
        mcp_servers: settings.mcp_servers.clone(),
        mcp_disabled: settings.disabled_mcp_servers.clone(),
        sandbox,
        session_dir,
    };
    let mut agent = Agent::start(config, choice).map_err(|e| e.to_string())?;
    if let Some(skipped) = interactive::skipped_lines(&agent) {
        let path = agent.session_path().display();
        warnings.push(format!("session {path}: {skipped}"));
    }
    warnings.extend(agent.saving_notice());
    let (scoped, scope_warnings) = match patterns.is_empty() {
        true => Default::default(),
        false => scope::resolve(patterns, &agent.model_options()),
    };
    warnings.extend((scope_warnings.iter()).map(|warning| format!("{source}: {warning}")));
    warnings.extend(unknown_tools);
    warnings.extend(missing_shells);
    agent.set_model_scope(scoped.iter().map(|scoped| scoped.model.clone()).collect());
    let new = !chosen && agent.last_model().is_none();
    let remembered = new
        .then(|| remembered(settings, &agent, &mut warnings))
        .flatten();
    let in_scope = (scoped.iter())
        .find(|scoped| remembered.as_ref() == Some(&scoped.model))
        .or(scoped.first())
        .filter(|_| new);
    let model = match agent.last_model() {
        _ if let Some(scoped) = in_scope => scoped.model.clone(),
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
    match &api_key {
        Some(key) => agent.give_key(key)?,
        None => {
            agent.load_model_info();
        }
    }
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

/// The model `--model` names, with the level its `:level` sets. When the providers list no
/// models, `pattern` is taken as an id, as before it could be checked.
/// The model `pattern` names, as `scope::named` finds it; with a `provider`, as Scramjet 1's
/// `--provider`, among that provider's models, a `provider/` before it left out.
fn named_model(
    pattern: &str,
    provider: Option<&str>,
    providers: &Providers,
) -> Result<scope::Scoped, String> {
    let pattern = pattern.trim();
    // As Scramjet 1's, `--provider P --model P/M` is `--model M` among P's.
    let pattern = (pattern.split_once('/'))
        .filter(|(before, _)| provider.is_some_and(|p| before.eq_ignore_ascii_case(p)))
        .map_or(pattern, |(_, rest)| rest);
    let models = providers.options();
    if models.is_empty() {
        let model = match provider {
            Some(provider) => format!("{provider}/{pattern}"),
            None => pattern.to_owned(),
        };
        return Ok(scope::Scoped { model, level: None });
    }
    let models = match provider {
        None => models,
        Some(provider) => {
            let theirs: Vec<ModelOption> = (models.iter())
                .filter(|model| {
                    (model.provider.as_deref()).is_some_and(|p| p.eq_ignore_ascii_case(provider))
                })
                .cloned()
                .collect();
            if theirs.is_empty() {
                return Err(format!(
                    "--provider {provider:?} is none of the providers (--list-models lists them)"
                ));
            }
            theirs
        }
    };
    scope::named(pattern, &models).ok_or_else(|| {
        let named = match provider {
            Some(provider) => {
                format!("--provider {provider:?} --model {pattern:?} names none of its")
            }
            None => format!("--model {pattern:?} names none of the providers'"),
        };
        format!("{named} models (--list-models lists them)")
    })
}

/// The cache retention of the run's requests, as Scramjet 1 finds it: `given` by
/// `--cache-retention`, else the variable SCRAMJET_CACHE_RETENTION's or PI_CACHE_RETENTION's,
/// else long. Also says when the variable's is none of them.
fn cache_retention(
    given: Option<agent::CacheRetention>,
) -> (agent::CacheRetention, Option<String>) {
    if let Some(given) = given {
        return (given, None);
    }
    let set = ["SCRAMJET_CACHE_RETENTION", "PI_CACHE_RETENTION"]
        .into_iter()
        .find_map(|name| Some((name, std::env::var(name).ok().filter(|v| !v.is_empty())?)));
    let Some((name, value)) = set else {
        return (agent::CacheRetention::Long, None);
    };
    match agent::CacheRetention::parse(&value) {
        Some(retention) => (retention, None),
        None => (
            agent::CacheRetention::Long,
            Some(format!(
                "{name} {value:?} is not none, short or long; long is used"
            )),
        ),
    }
}

/// The model a new session starts with when nothing chooses one: the first provider's
/// gpt-6-luna, unless the providers offer others but not it, then the first of theirs.
fn default_model(providers: &Providers) -> String {
    const DEFAULT: &str = "gpt-6-luna";
    let models = providers.options();
    let chosen = (models.iter().find(|model| model.id == DEFAULT)).or(models.first());
    chosen.map_or_else(|| DEFAULT.to_owned(), ModelOption::reference)
}

/// The model chosen before that a new session in the agent's project starts with
/// (`Settings::remembered_models`), by reference: the first the providers offer, or either when
/// they list no models. Says in `warnings` which of them they do not offer.
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
        if listed.is_none() {
            return Some(model);
        }
        if let Some(reference) = agent.offered_model(&model) {
            return Some(reference);
        }
        warnings.push(format!("{what} is not one the providers offer; not used"));
    }
    None
}

/// Ctrl-C sets `cancel`, and a second one while it is set exits; Ctrl+T during a turn (SIGQUIT,
/// see `InputMode::turn`) flips `hide_thinking`. Ctrl-Z stops and continues as `job` says, and
/// SIGTERM and SIGHUP exit as `exit` says.
fn handle_signals(cancel: &Arc<AtomicBool>, hide_thinking: &Arc<AtomicBool>) -> Result<(), String> {
    let startup = console::Mode::get();
    job::install(startup.clone()).map_err(|e| e.to_string())?;
    exit::install(startup).map_err(|e| e.to_string())?;
    let flag = cancel.clone();
    // SAFETY: the handler only swaps an atomic and calls `console::flush_input` and `exit::on_signal`, all
    // async-signal-safe.
    unsafe {
        signal_hook::low_level::register(signal_hook::consts::SIGINT, move || {
            // A second Ctrl-C while a cancellation is still pending exits.
            if flag.swap(true, Ordering::Relaxed) {
                exit::on_signal(signal_hook::consts::SIGINT);
                return;
            }
            // Keys typed before it answer nothing and are not left for the shell.
            console::flush_input();
        })
    }
    .map_err(|e| e.to_string())?;
    // Windows has no quit character to give Ctrl+T.
    #[cfg(unix)]
    {
        let flag = hide_thinking.clone();
        // SAFETY: the handler only flips an atomic.
        unsafe {
            signal_hook::low_level::register(signal_hook::consts::SIGQUIT, move || {
                flag.fetch_xor(true, Ordering::Relaxed);
            })
        }
        .map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    let _ = hide_thinking;
    Ok(())
}

/// Before the agent ends, writes once more what its sessions keep in memory for want of room. What
/// is copied where a later run finds it waits there; otherwise, on a terminal the user can free
/// space and try again, as what is still held then is lost.
fn write_held_at_exit(agent: &mut Agent) {
    let asking = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    while agent.held_len() > 0 {
        agent.write_held();
        let held = agent.held_len();
        if held == 0 {
            eprintln!("Session written, with what was kept in memory.");
            return;
        }
        let size = scramjet::agent::byte_size(held);
        if let Some(directory) = agent.held_kept_in() {
            eprintln!(
                "Session not written for want of room: the {size} kept in memory wait in {}, for \
                 the next run that finds room to write them.",
                directory.display()
            );
            return;
        }
        if !asking {
            eprintln!(
                "Error: session not written: the {size} kept in memory for want of room are lost."
            );
            return;
        }
        eprint!(
            "The session keeps {size} in memory that the disk has no room for. Free space, then \
             press Enter to write it, or Ctrl+D to leave without it. "
        );
        let mut line = String::new();
        if !matches!(std::io::stdin().read_line(&mut line), Ok(1..)) {
            eprintln!("\nSession left without the {size} kept in memory.");
            return;
        }
    }
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
    if let Some(notice) = agent.take_interrupted() {
        ui.notice(&notice);
    }
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
