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

mod acp;
mod args;
mod background;
mod banner;
mod bottom;
mod catalog;
mod changelog;
mod changes;
mod children;
mod clipboard;
mod completion;
mod console;
mod context;
mod design;
mod diff;
mod doctor;
mod drive;
mod editor;
mod exit;
mod export;
mod external;
mod file_search;
mod footer;
mod fuzzy;
mod goal;
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
mod live;
mod local_login;
mod login;
mod looping;
mod markdown;
mod notify;
mod packages;
mod palette;
mod plan;
mod programs;
mod pull_request;
mod record;
mod report;
mod rewind;
mod sandbox_menu;
mod scope;
mod screen;
mod select;
mod selection;
mod sessions;
mod settings;
mod settings_menu;
mod slash;
mod spelling;
mod status_line;
mod style;
mod system_prompt;
mod terminal;
mod text;
mod theme;
mod theme_editor;
mod tree;
mod update;
mod usage;
mod writable;

use args::{Options, Start};
use scramjet::agent::{
    self, Agent, AgentConfig, AgentError, AgentUi, Attached, CODING_TOOLS, Found, HARNESS_TOOLS,
    ModelOption, Next, Providers, Sandbox, Session, SessionChoice, SkillOptions, SystemPrompt,
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
    // The clipboard's selection owners, `__…`, run with an emptied environment.
    if !(arguments.first()).is_some_and(|first| first.to_string_lossy().starts_with("__")) {
        settings::set_environment();
    }
    // Checked below before any subcommand that may write, or else once the options are read.
    let checked = (arguments.first().and_then(|first| first.to_str()))
        .is_some_and(|first| !first.starts_with('-'));
    if let Some((command, rest)) = arguments.split_first()
        && let Some(command) = command.to_str()
    {
        let rest: Option<Vec<String>> = rest.iter().map(|a| a.to_str().map(String::from)).collect();
        let rest = rest.unwrap_or_default();
        if let Some(result) = clipboard::run(command, &rest) {
            return result;
        }
        if let Some(result) = lint::run(command, &rest) {
            return result;
        }
        if let Some(result) = doctor::run(command, &rest) {
            return result;
        }
        if !command.starts_with('-') {
            writable::check()?;
        }
        if command == "acp" {
            return acp::run(&rest);
        }
        if let Some(result) = catalog::run(command, &rest) {
            return result;
        }
        if let Some(result) = install::run(command, &rest) {
            return result;
        }
        if let Some(result) = import::run(command, &rest) {
            return result;
        }
    }
    let mut options = match Options::parse(arguments.into_iter())? {
        Ok(options) => options,
        Err(help) => {
            outln!("{}", help.text());
            return Ok(());
        }
    };
    if options.version {
        outln!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    options.isolate()?;
    if !checked {
        writable::check()?;
    }
    if let Some(search) = options.list_models.take() {
        let mut providers =
            Providers::read(Settings::load().sources(), agent::login::Logins::user());
        if let Some(key) = &options.api_key
            && let Some(provider) = key_provider(&options, &providers)
        {
            providers.give_key(&provider, key);
        }
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
        let providers = Providers::read(Settings::load().sources(), agent::login::Logins::user());
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
    let (max_turns, output_json) = (options.max_turns, options.output_json);
    let root = project_root(options.project.take())?;
    let mut settings = Settings::load_cloning();
    settings.in_project(&root);
    settings.retries_given = options.retries;
    scramjet::agent::set_shell(
        settings.shell_path.clone(),
        settings.shell_command_prefix.clone(),
    );
    scramjet::agent::set_user_shell(settings.default_shell);
    scramjet::agent::set_launcher(settings.sandbox_launcher.clone());
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
    ui.replies_on_stderr = output_json;
    if !print_only {
        let themes = (themes, theme_options);
        let mut interactive =
            interactive::Interactive::new(agent, ui, cancel, hide_thinking, settings, themes)?;
        interactive.session.scope = patterns;
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
    agent.set_hooks(settings.hooks.clone());
    agent.set_unattended(true);
    agent.set_max_turns(max_turns);
    // A message read from stdin (`-`) may start a command.
    let commands = (messages.iter()).any(|message| message == "-" || message.starts_with('/'));
    agent.leave_out_idle_tools(commands);
    if let Some(files) = files {
        agent.attach(files);
    }
    let started = std::time::Instant::now();
    let replies = agent.replies().count();
    let mut reached_max_turns = false;
    let result = (messages.into_iter()).try_for_each(|prompt| {
        let prompt = match prompt.as_str() {
            "-" => std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?,
            _ => prompt,
        };
        let model = agent.model().to_owned();
        let result = print(&mut agent, &mut ui, &prompt, &cancel).map_err(|error| {
            reached_max_turns = matches!(error, AgentError::MaxTurns(_));
            error.to_string()
        });
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
    if output_json {
        let replied = agent.replies().count() > replies;
        println!(
            "{}",
            run_result(&agent, started, &result, reached_max_turns, replied)
        );
    }
    write_held_at_exit(&mut agent);
    result
}

/// `--output-format json`'s object, as Claude Code's `-p` prints it: what the run ended with,
/// its last reply's text when it made one, and what it spent, subagents' included. Its
/// `input_tokens` are those neither read from nor written to the cache, as Anthropic counts them.
fn run_result(
    agent: &Agent,
    started: std::time::Instant,
    result: &Result<(), String>,
    reached_max_turns: bool,
    replied: bool,
) -> serde_json::Value {
    let usage = agent.usage;
    let cached = usage.cached_input_tokens + usage.cache_write_tokens;
    let mut object = serde_json::json!({
        "type": "result",
        "subtype": match result {
            Ok(()) => "success",
            Err(_) if reached_max_turns => "error_max_turns",
            Err(_) => "error_during_execution",
        },
        "is_error": result.is_err(),
        "duration_ms": started.elapsed().as_millis() as u64,
        "num_turns": agent.turns_made(),
        "session_id": agent.session_id(),
        "total_cost_usd": usage.cost,
        "usage": {
            "input_tokens": usage.input_tokens.saturating_sub(cached),
            "cache_creation_input_tokens": usage.cache_write_tokens,
            "cache_read_input_tokens": usage.cached_input_tokens,
            "output_tokens": usage.output_tokens,
        },
    });
    match result {
        Ok(()) => {
            let reply = agent.last_reply().filter(|_| replied).unwrap_or_default();
            object["result"] = reply.into();
        }
        Err(error) => object["errors"] = serde_json::json!([error]),
    }
    object
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
    let directory = || match session_dir {
        Some(directory) => Ok(directory.to_owned()),
        None => agent::session_directory(root).map_err(|e| e.to_string()),
    };
    let find = |named: &str| -> Result<Found, String> {
        // As Scramjet's, a word that looks like a path is a session file, wherever it is.
        if named.contains(['/', '\\']) || named.ends_with(".jsonl") {
            return Ok(Found::Here(PathBuf::from(named)));
        }
        let projects = agent::projects_directory().map_err(|e| e.to_string())?;
        let directory = directory()?;
        Session::find(&directory, &projects, named)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .ok_or_else(|| format!("no session found matching {named:?}"))
    };
    // Another process's session would take both processes' writes: it is forked instead.
    let open = |path: PathBuf, named: &str| match live::holder(&path) {
        None => Ok(Some(SessionChoice::Open(path))),
        Some(holder) => Ok(fork_held(named, &holder)?.then_some(SessionChoice::Fork(path))),
    };
    Ok(Some(match start {
        Start::New | Start::Resume => SessionChoice::New,
        Start::Continue => {
            let directory = directory()?;
            let latest = Session::latest(&directory).map_err(|e| e.to_string())?;
            let written = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
            // A session left with only an unsent prompt, kept as no session file is before its
            // first, goes on if it is newer than the latest written.
            let drafted = agent::draft::newest_unwritten(&directory).filter(|(path, at)| {
                live::holder(path).is_none()
                    && latest
                        .as_deref()
                        .and_then(written)
                        .is_none_or(|then| *at > then)
            });
            let id = |path: &Path| Some(path.file_stem()?.to_str()?.to_owned());
            match (latest, drafted.and_then(|(path, _)| id(&path))) {
                (_, Some(id)) => SessionChoice::Named(id),
                (Some(path), None) if live::holder(&path).is_some() => {
                    let named = path.display().to_string();
                    return open(path, &named);
                }
                _ => SessionChoice::Continue,
            }
        }
        Start::Unsaved => SessionChoice::Unsaved,
        Start::Fork(named) => SessionChoice::Fork(find(&named)?.path().to_owned()),
        Start::Session(named) => match find(&named)? {
            Found::Here(path) => return open(path, &named),
            Found::Elsewhere { path, root } => match fork_here(&named, &root)? {
                true => SessionChoice::Fork(path),
                false => return Ok(None),
            },
        },
    }))
}

/// Asks whether to fork the session `named`, which another process holds; when the input is not
/// a terminal, says how to instead.
fn fork_held(named: &str, holder: &live::Holder) -> Result<bool, String> {
    let open = format!("session {named:?} is open in {holder}");
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{open}; --fork {named:?} continues it in a new session"
        ));
    }
    eprintln!("{}{}", open[..1].to_uppercase(), &open[1..]);
    eprint!("Fork it into a new session? [y/N] ");
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
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
    let mut providers = Providers::read(settings.sources(), agent::login::Logins::user());
    // The provider --provider or --model's `provider/` names takes --api-key's key, before the
    // model is found: a built-in one offers its models only once it has a key.
    let key_for = api_key
        .as_ref()
        .and_then(|_| key_provider(&options, &providers));
    if let (Some(key), Some(provider)) = (&api_key, &key_for) {
        providers.give_key(provider, key);
    }
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
        let keyed: Vec<&str> = agent::BUILTIN.iter().map(|builtin| builtin.shown).collect();
        let keyed = match keyed.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} or {last}", rest.join(", ")),
            _ => keyed.concat(),
        };
        // As Scramjet's, a run without the full screen stops here: no request could be sent.
        if options.print {
            let problems = (providers.problems().iter())
                .map(|problem| format!("\n{problem}"))
                .collect::<String>();
            return Err(format!(
                "no provider offers any model: start scramjet and /login (a subscription, an API \
key for {keyed}, or local models), or add providers to {file} (see --help){problems}"
            ));
        }
        warnings.push(format!(
            "No provider offers any model: /login (a subscription, an API key for {keyed}, or \
local models), or add providers to {file} (see --help)"
        ));
    }
    let model = named.map_or_else(|| default_model(&providers), |named| named.model);
    let mut sandbox = (options.sandbox.unwrap_or(settings.sandbox)).map(|mode| Sandbox {
        mode,
        network: settings.sandbox_network,
        writable: Vec::new(),
    });
    if let Some(sandbox) = &mut sandbox {
        Sandbox::check()?;
        warnings.extend(sandbox.grant_always(&root, &settings.allow_write()));
    }
    for directory in &options.add_dirs {
        if let Some(sandbox) = &mut sandbox {
            match sandbox.grant(&root, directory) {
                Err(error) if error.ends_with("already writable") => {}
                granted => _ = granted.map_err(|error| format!("--add-dir {error}"))?,
            }
        }
    }
    warnings.extend((sandbox.as_ref()).and_then(|sandbox| sandbox.spelling_warning(&root)));
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
        harness_off: settings.harness_off.clone(),
        only_tools: options.only_tools,
        command_sets: settings.command_sets.clone(),
        skills: settings.skills.clone(),
        skill_options,
        prompt: SystemPrompt {
            no_directives: options.prompt.no_directives || !settings.directives,
            ..options.prompt
        },
        mcp_servers: settings.mcp_servers.clone(),
        mcp_disabled: settings.disabled_mcp_servers.clone(),
        sandbox,
        session_dir,
    };
    let mut agent = Agent::start(config, choice).map_err(|e| e.to_string())?;
    agent.set_reads(settings.reads());
    agent.set_subagents(settings.subagents());
    agent.set_offline(!settings.sandbox_network);
    warnings.extend(offline_notice(&agent));
    if agent.sandbox().is_none() {
        let said = agent.grant_writable_always(&settings.allow_write());
        // Kept for a later Can read change; with nothing hidden, everything is writable.
        if agent.reads().reach.kept() {
            warnings.extend(said);
        }
        for directory in &options.add_dirs {
            match agent.grant_writable(directory) {
                Err(error) if error.ends_with("already writable") => {}
                // What the reach hides is all a directory granted without a sandbox changes.
                _ if !agent.reads().reach.kept() => warnings.push(format!(
                    "--add-dir {directory}: everything is writable without --sandbox"
                )),
                granted => _ = granted.map_err(|error| format!("--add-dir {error}"))?,
            }
        }
    }
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
        Some(_)
            if let (Some(named), Some(serving)) = (&key_for, agent.model_provider())
                && !named.eq_ignore_ascii_case(serving) =>
        {
            return Err(format!(
                "--api-key: the key is for {named}, which --model names, but {} is {serving}'s; \
it is not sent there",
                agent.model()
            ));
        }
        Some(key) => agent.give_key(key)?,
        None => {
            agent.load_model_info();
        }
    }
    Ok((agent, warnings))
}

/// While commands are kept off the network with no sandbox: that they are, from a setting the
/// user may have made for the sandbox alone, and why they cannot run if they cannot.
pub(super) fn offline_notice(agent: &Agent) -> Option<String> {
    if agent.sandbox().is_some() || !agent.offline() {
        return None;
    }
    let said = "Commands run off the network: sandbox.network is false; /sandbox network on \
puts them on it";
    Some(match Sandbox::check() {
        Ok(()) => said.to_owned(),
        Err(error) => format!("{said}. They cannot run: {error}"),
    })
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

/// The provider `--api-key`'s key is for when the options name one: `--provider`, else the
/// `provider/` before `--model`'s id when a provider of that name takes a key.
fn key_provider(options: &Options, providers: &Providers) -> Option<String> {
    let keyed = |name: &str| {
        (providers.keyed().iter())
            .find(|(keyed, _)| keyed.eq_ignore_ascii_case(name))
            .map(|(keyed, _)| keyed.clone())
    };
    match &options.provider {
        Some(provider) => Some(keyed(provider).unwrap_or_else(|| provider.clone())),
        None => keyed(options.model.as_deref()?.trim().split_once('/')?.0),
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
    // A built-in provider's first, newest, model may be any of its own or, routed, another's.
    let first = models.first().map(|first| {
        let builtin = first.provider.as_deref().and_then(agent::builtin);
        let default = builtin.and_then(|builtin| {
            (models.iter())
                .find(|m| m.provider == first.provider && m.id.starts_with(builtin.default))
        });
        default.unwrap_or(first)
    });
    let chosen = (models.iter().find(|model| model.id == DEFAULT)).or(first);
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
    let key = settings.default_model_key();
    let remembered = [
        here.map(|model| {
            (
                format!("{model}, the model last chosen in this project,"),
                model,
                false,
            )
        }),
        anywhere.map(|model| {
            let what = format!("settings: {path}: {key} {model}");
            (what, model, key == "model")
        }),
    ];
    for (what, model, named) in remembered.into_iter().flatten() {
        if listed.is_none() {
            return Some(model);
        }
        if let Some(reference) = offered(agent.providers(), &model, named) {
            return Some(reference);
        }
        warnings.push(format!("{what} is not one the providers offer; not used"));
    }
    None
}

/// The reference of the model `model`, chosen before, if the providers offer it: as written, or
/// when `named` (Claude Code's `model` setting, which takes aliases) as `--model` finds one.
pub(super) fn offered(providers: &Providers, model: &str, named: bool) -> Option<String> {
    (providers.offered(model)).or_else(|| {
        let found = named.then(|| scope::named(model, &providers.options()))??;
        Some(found.model)
    })
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
    prompt: &str,
    cancel: &AtomicBool,
) -> Result<(), AgentError> {
    let mode = InputMode::turn();
    let result = agent.turn(prompt, ui, cancel);
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
    result
}
