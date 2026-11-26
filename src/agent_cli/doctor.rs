//! `scramjet doctor` and `/doctor`, as Claude Code's and Codex's: whether Scramjet can work here,
//! each check a line with what to do when it cannot. Nothing is sent to a provider; the only
//! connections tried are to the providers' servers on this machine.
use super::settings::Settings;
use super::{keymap, theme, update, writable};
use scramjet::agent::{McpState, Providers, Sandbox};
use std::{
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const USAGE: &str = "Usage: scramjet doctor

Checks whether Scramjet can work here: the program and the scramjet on PATH, a newer release,
the files it writes and the disk's room, the settings and what they load, the models, the
providers' servers on this machine, the sandbox, git and the shell, the terminal and the MCP
servers. Nothing is sent to a provider. Exits with an error when a check fails.
In a session, /doctor shows the same with what the session loaded.";

const LOW_DISK: u64 = 1 << 30;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mark {
    Good,
    Warn,
    Bad,
}

#[derive(Debug, PartialEq)]
pub(super) struct Check {
    pub(super) mark: Mark,
    pub(super) name: &'static str,
    pub(super) summary: String,
    pub(super) details: Vec<String>,
}

impl Check {
    fn new(mark: Mark, name: &'static str, summary: impl Into<String>) -> Self {
        Self {
            mark,
            name,
            summary: summary.into(),
            details: Vec::new(),
        }
    }

    fn with(mut self, details: impl IntoIterator<Item = String>) -> Self {
        self.details.extend(details);
        self
    }
}

/// What the checks look at.
pub(super) struct Setup<'a> {
    pub(super) settings: &'a Settings,
    pub(super) providers: &'a Providers,
    pub(super) root: &'a Path,
    /// What is wrong in the settings and in what they load, as said at start.
    pub(super) problems: Vec<String>,
    /// The session's model, in a session.
    pub(super) model: Option<String>,
    /// The MCP servers' states, once a session started them.
    pub(super) mcp: Option<Vec<(String, McpState)>>,
}

/// Runs `scramjet doctor`; `None` for another command.
pub(super) fn run(command: &str, arguments: &[String]) -> Option<Result<(), String>> {
    if command != "doctor" {
        return None;
    }
    Some(match arguments {
        [] => doctor(),
        [help] if help == "--help" || help == "-h" => {
            outln!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    })
}

fn doctor() -> Result<(), String> {
    let root = std::env::current_dir().map_err(|error| error.to_string())?;
    let mut settings = Settings::load();
    settings.in_project(&root);
    scramjet::agent::set_launcher(settings.sandbox_launcher.clone());
    let providers = Providers::read(settings.sources(), scramjet::agent::login::Logins::user());
    let found = scramjet::agent::inventory(&settings.command_sets, &settings.skills, &root);
    let themes = theme::Themes::discover(&root, &settings.themes, &Default::default());
    let problems = (settings.warnings.iter())
        .map(|warning| format!("settings: {warning}"))
        .chain(
            themes
                .problems(settings.theme_name())
                .into_iter()
                .map(|p| p.text),
        )
        .chain(keymap::load(settings.path()).into_iter().map(|p| p.text))
        .chain(found.warnings.iter().map(|p| p.text.clone()))
        .chain(found.skills.warnings().iter().map(|p| p.text.clone()))
        .collect();
    let setup = Setup {
        settings: &settings,
        providers: &providers,
        root: &root,
        problems,
        model: None,
        mcp: None,
    };
    let checks = checks(&setup);
    out!("{}", text(&checks));
    match checks
        .iter()
        .filter(|check| check.mark == Mark::Bad)
        .count()
    {
        0 => Ok(()),
        1 => Err("1 check failed".to_owned()),
        failed => Err(format!("{failed} checks failed")),
    }
}

pub(super) fn checks(setup: &Setup) -> Vec<Check> {
    vec![
        program(),
        newer(),
        files(),
        settings(setup),
        models(setup),
        servers(setup.providers, starting(setup).0.as_deref()),
        sandbox(setup.settings),
        tools(setup.root),
        terminal(),
        mcp(setup),
    ]
}

/// The report: a heading, a line per check with its details under it, and a count of what
/// failed. Ends with a newline.
pub(super) fn text(checks: &[Check]) -> String {
    let mut text = format!(
        "Scramjet {} · {} {}{}\n\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if !cfg!(windows) && std::env::var_os("WSL_DISTRO_NAME").is_some() {
            " (WSL)"
        } else {
            ""
        }
    );
    let width = checks
        .iter()
        .map(|check| check.name.len())
        .max()
        .unwrap_or(0);
    for check in checks {
        let mark = match check.mark {
            Mark::Good => '✓',
            Mark::Warn => '⚠',
            Mark::Bad => '✗',
        };
        let summary = shown(&check.summary);
        text += &format!("{mark} {:width$}  {summary}\n", check.name);
        for detail in &check.details {
            text += &format!("  {:width$}    {}\n", "", shown(detail));
        }
    }
    let count = |mark: Mark| checks.iter().filter(|check| check.mark == mark).count();
    let plural = |n: usize, what: &str| match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    };
    text += &match (count(Mark::Bad), count(Mark::Warn)) {
        (0, 0) => "\nAll checks passed.\n".to_owned(),
        (0, warn) => format!("\n{}.\n", plural(warn, "warning")),
        (bad, 0) => format!("\n{} failed.\n", plural(bad, "check")),
        (bad, warn) => format!(
            "\n{} failed, {}.\n",
            plural(bad, "check"),
            plural(warn, "warning")
        ),
    };
    text
}

/// `text` with the home directory as `~`.
fn shown(text: &str) -> String {
    match scramjet::os::home_prefix() {
        Some(home) => text.replace(&home, &format!("~{}", std::path::MAIN_SEPARATOR)),
        None => text.to_owned(),
    }
}

/// This program, and which `scramjet` typing it runs.
fn program() -> Check {
    let exe = std::env::current_exe().ok();
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let this = exe.as_deref().map(real);
    let Some(this) = this else {
        return Check::new(Mark::Warn, "program", "where this program is is not known");
    };
    let found = on_path("scramjet", std::env::var_os("PATH"));
    let shown = |path: &Path| match real(path).extension().and_then(|e| e.to_str()) {
        Some("js" | "cmd" | "ps1") => format!("{} (Scramjet 1)", path.display()),
        _ => path.display().to_string(),
    };
    let others: Vec<String> = (found.iter().skip(1))
        .filter(|path| real(path) != this)
        .map(|path| format!("also on PATH: {}", shown(path)))
        .collect();
    match found.first() {
        Some(first) if real(first) != this => Check::new(
            Mark::Warn,
            "program",
            format!(
                "{}; typing scramjet runs {}, which comes first on PATH",
                this.display(),
                shown(first)
            ),
        ),
        Some(_) => Check::new(Mark::Good, "program", this.display().to_string()),
        None => Check::new(
            Mark::Warn,
            "program",
            format!("{}; no scramjet on PATH: add its directory", this.display()),
        ),
    }
    .with(others)
}

/// The executables `name` stands for in the directories of `path`, in its order.
fn on_path(name: &str, path: Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let Some(path) = path else {
        return Vec::new();
    };
    let names: Vec<String> = match cfg!(windows) {
        true => [".exe", ".cmd", ".bat", ".ps1"]
            .iter()
            .map(|extension| format!("{name}{extension}"))
            .collect(),
        false => vec![name.to_owned()],
    };
    let mut found: Vec<PathBuf> = Vec::new();
    for directory in std::env::split_paths(&path) {
        for name in &names {
            let file = directory.join(name);
            let runs = std::fs::metadata(&file)
                .is_ok_and(|metadata| metadata.is_file() && scramjet::os::executable(&metadata));
            if runs && !found.contains(&file) {
                found.push(file);
            }
        }
    }
    found
}

/// What the last look for a newer release found; none is looked for here.
fn newer() -> Check {
    let current = env!("CARGO_PKG_VERSION");
    if scramjet::agent::offline() {
        return Check::new(Mark::Good, "update", "not looked for (offline)");
    }
    let Some((latest, checked, error)) = update::last_check() else {
        return Check::new(
            Mark::Good,
            "update",
            "not looked for yet: the full screen looks once a day",
        );
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let ago = ago(now.saturating_sub(checked));
    let newer = latest.as_deref().filter(|latest| {
        let parse = semver::Version::parse;
        matches!((parse(latest), parse(current)), (Ok(l), Ok(c)) if l.cmp_precedence(&c).is_gt())
    });
    match (newer, error) {
        (Some(latest), _) => Check::new(
            Mark::Warn,
            "update",
            format!("{latest} is out (this is {current}): scramjet update self installs it"),
        ),
        (None, Some(error)) => Check::new(
            Mark::Warn,
            "update",
            format!("the look for a newer release {ago} failed: {error}"),
        ),
        (None, None) => Check::new(
            Mark::Good,
            "update",
            format!("no newer release (looked {ago})"),
        ),
    }
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..120 => "just now".to_owned(),
        120..7200 => format!("{} minutes ago", seconds / 60),
        7200..172800 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86400),
    }
}

/// Whether Scramjet can write where it keeps its files, and the room left there.
fn files() -> Check {
    if let Err(error) = writable::check() {
        return Check::new(Mark::Bad, "files", error);
    }
    let state = match scramjet::agent::state_directory() {
        Ok(state) => state,
        Err(error) => return Check::new(Mark::Bad, "files", error.to_string()),
    };
    let place = state
        .ancestors()
        .find(|path| path.exists())
        .unwrap_or(&state);
    let free = free_space(place);
    let shown = state.display();
    match free {
        Some(free) if free < LOW_DISK => Check::new(
            Mark::Warn,
            "files",
            format!(
                "only {} free for {shown}: sessions are held in memory once the disk is full",
                size(free)
            ),
        ),
        Some(free) => Check::new(
            Mark::Good,
            "files",
            format!("writable; {} free for {shown}", size(free)),
        ),
        None => Check::new(Mark::Good, "files", "writable"),
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        0..0x10_0000 => format!("{} KB", bytes >> 10),
        0x10_0000..0x4000_0000 => format!("{} MB", bytes >> 20),
        _ => format!("{:.1} GB", bytes as f64 / f64::from(1 << 30)),
    }
}

#[cfg(unix)]
fn free_space(path: &Path) -> Option<u64> {
    let stat = rustix::fs::statvfs(path).ok()?;
    Some(stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path; the other outputs may be null.
    let done = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (done != 0).then_some(free)
}

/// The settings file, and what is wrong in it and in what it loads.
fn settings(setup: &Setup) -> Check {
    let file = (setup.settings.shown_path()).unwrap_or_else(|| "no settings file".to_owned());
    match setup.problems.len() {
        0 => Check::new(Mark::Good, "settings", file),
        count => Check::new(
            Mark::Warn,
            "settings",
            format!(
                "{file}: {count} problem{}",
                if count == 1 { "" } else { "s" }
            ),
        ),
    }
    .with(setup.problems.iter().cloned())
}

/// The models the providers offer, those signed in to, and whether the model a session starts
/// with is among them.
fn models(setup: &Setup) -> Check {
    let options = setup.providers.options();
    let mut providers: Vec<&str> = Vec::new();
    for provider in options
        .iter()
        .filter_map(|option| option.provider.as_deref())
    {
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    let mut details: Vec<String> = (setup.providers.problems().iter())
        .map(|problem| format!("models: {problem}"))
        .collect();
    let (starts, not_offered) = starting(setup);
    details.extend(not_offered);
    let mark = match details.is_empty() {
        true => Mark::Good,
        false => Mark::Warn,
    };
    let auth = scramjet::agent::login::auth_file();
    let signed_in = auth.and_then(|auth| scramjet::agent::login::logged_in(&auth).ok());
    if let Some(names) = signed_in.filter(|names| !names.is_empty()) {
        details.push(format!("signed in or keys saved: {}", names.join(", ")));
    }
    if options.is_empty() {
        let summary = "no models: /login in a session signs in or saves a provider's key";
        return Check::new(Mark::Bad, "models", summary).with(details);
    }
    let count = format!(
        "{} model{} from {} provider{}",
        options.len(),
        if options.len() == 1 { "" } else { "s" },
        providers.len(),
        if providers.len() == 1 { "" } else { "s" }
    );
    let summary = match (&setup.model, starts) {
        (Some(model), _) => format!("{count}; this session's is {model}"),
        (None, Some(model)) => format!("{count}; a session here starts with {model}"),
        (None, None) => format!("{count}; a session here starts without one: /model picks it"),
    };
    Check::new(mark, "models", summary).with(details)
}

/// The model a session here has, or starts with: the one last chosen in the project, else the
/// settings' `defaultModel`; with a note for each of those no provider offers.
fn starting(setup: &Setup) -> (Option<String>, Vec<String>) {
    if let Some(model) = &setup.model {
        return (Some(model.clone()), Vec::new());
    }
    let (here, anywhere) = setup.settings.remembered_models(setup.root);
    let mut notes = Vec::new();
    let starts = here.into_iter().chain(anywhere).find_map(|model| {
        let offered = setup.providers.offered(&model);
        if offered.is_none() {
            notes.push(format!("{model}, chosen before, is not offered now"));
        }
        offered
    });
    (starts, notes)
}

/// Whether something answers at the providers' servers on this machine: a local proxy or a
/// runtime for local models not started is the commonest reason a first prompt fails. Fails
/// for the server of `model`, the one a session uses; another one not answering is a warning.
fn servers(providers: &Providers, model: Option<&str>) -> Check {
    let used = model
        .and_then(|model| model.split_once('/'))
        .map(|(provider, _)| provider);
    let servers = providers.local_servers();
    if servers.is_empty() {
        return Check::new(Mark::Good, "servers", "no provider on this machine");
    }
    // Each server: how it is shown, whether the model in use is sent there, and why nothing
    // answered there.
    let tried: Vec<(String, bool, Option<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (servers.iter())
            .map(|(provider, host, port)| {
                let shown = format!("{provider} at {host}:{port}");
                let in_use = used == Some(provider.as_str());
                (shown, in_use, scope.spawn(move || connect(host, *port)))
            })
            .collect();
        (handles.into_iter())
            .map(|(shown, in_use, handle)| {
                let result = handle.join().unwrap_or_else(|_| Err("failed".to_owned()));
                (shown, in_use, result.err())
            })
            .collect()
    });
    let used_failed = (tried.iter()).any(|(_, in_use, error)| *in_use && error.is_some());
    let failed: Vec<String> = (tried.iter())
        .filter_map(|(shown, in_use, error)| {
            let note = if *in_use { " (the model in use)" } else { "" };
            error
                .as_ref()
                .map(|error| format!("{shown}{note}: {error}"))
        })
        .collect();
    let answering: Vec<&str> = (tried.iter())
        .filter(|(_, _, error)| error.is_none())
        .map(|(shown, _, _)| shown.as_str())
        .collect();
    let answered = answering.iter().map(|shown| format!("answers: {shown}"));
    match failed.is_empty() {
        true => Check::new(Mark::Good, "servers", answering.join(", ")),
        false => Check::new(
            if used_failed { Mark::Bad } else { Mark::Warn },
            "servers",
            "nothing answers at a provider's server on this machine: start it, or check its port",
        )
        .with(failed)
        .with(answered),
    }
}

fn connect(host: &str, port: u16) -> Result<(), String> {
    let host = host.trim_matches(['[', ']']);
    let addresses = (host, port).to_socket_addrs().map_err(|e| e.to_string())?;
    let mut last = "no address".to_owned();
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(_) => return Ok(()),
            Err(error) => last = error.to_string(),
        }
    }
    Err(last)
}

/// Whether the sandbox can start here; a failure matters when the settings ask for one.
fn sandbox(settings: &Settings) -> Check {
    let asked = settings.sandbox.is_some();
    match Sandbox::check() {
        Ok(()) => Check::new(Mark::Good, "sandbox", "can confine commands (--sandbox)"),
        Err(error) => Check::new(if asked { Mark::Bad } else { Mark::Warn }, "sandbox", error),
    }
}

/// git, and the shell the agent's commands run in.
fn tools(root: &Path) -> Check {
    let mut mark = Mark::Good;
    let mut found = Vec::new();
    let mut details = Vec::new();
    match version("git", root) {
        Some(git) => found.push(git),
        None => {
            mark = Mark::Warn;
            details.push("git: not found; /diff, /review and packages need it".to_owned());
        }
    }
    match scramjet::os::bash() {
        Ok(bash) => {
            let path = match bash.is_absolute() {
                true => Some(bash),
                false => {
                    (on_path(&bash.to_string_lossy(), std::env::var_os("PATH")).into_iter()).next()
                }
            };
            let path = path.map_or_else(|| "bash".to_owned(), |path| path.display().to_string());
            found.push(format!("bash {path}"));
        }
        Err(error) if cfg!(windows) => details.push(format!("bash: {error}")),
        Err(error) => {
            mark = Mark::Bad;
            details.push(format!("bash: {error}"));
        }
    }
    #[cfg(windows)]
    {
        let shells = [
            ("powershell", scramjet::os::powershell()),
            ("cmd", scramjet::os::cmd()),
        ];
        for (name, path) in shells {
            match path {
                Some(path) => found.push(format!("{name} {}", path.display())),
                None => details.push(format!("{name}: not found")),
            }
        }
        if found.len() == usize::from(found.first().is_some_and(|f| f.starts_with("git"))) {
            mark = Mark::Bad;
        }
    }
    match on_path("gh", std::env::var_os("PATH")).first() {
        Some(gh) => found.push(format!("gh {}", gh.display())),
        None => {
            details.push("gh: not found; pull requests and private packages need it".to_owned())
        }
    }
    Check::new(mark, "tools", found.join(", ")).with(details)
}

/// `name --version`'s first line, e.g. `git 2.43.0`.
fn version(name: &str, directory: &Path) -> Option<String> {
    let output = Command::new(scramjet::os::program(name, None))
        .arg("--version")
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    Some(line.replacen(" version ", " ", 1))
}

/// What the terminal says it is.
fn terminal() -> Check {
    let variable = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let mut parts = Vec::new();
    match variable("TERM") {
        Some(term) => parts.push(format!("TERM={term}")),
        None if !cfg!(windows) => parts.push("TERM not set".to_owned()),
        None => {}
    }
    if let Some(program) = variable("TERM_PROGRAM") {
        let version = variable("TERM_PROGRAM_VERSION").map_or(String::new(), |v| format!(" {v}"));
        parts.push(format!("{program}{version}"));
    }
    if variable("TMUX").is_some() {
        parts.push("in tmux".to_owned());
    }
    if variable("WT_SESSION").is_some() {
        parts.push("Windows Terminal".to_owned());
    }
    if variable("SSH_CONNECTION").is_some() {
        parts.push("over SSH".to_owned());
    }
    let dumb = variable("TERM").as_deref() == Some("dumb");
    let mark = match dumb {
        true => Mark::Warn,
        false => Mark::Good,
    };
    let check = Check::new(mark, "terminal", parts.join(", "));
    match dumb {
        true => check.with(["TERM=dumb: the full screen needs a terminal that draws".to_owned()]),
        false => check,
    }
}

/// The MCP servers: how each is in a session, else which the settings list.
fn mcp(setup: &Setup) -> Check {
    let servers = &setup.settings.mcp_servers;
    if servers.is_empty() {
        return Check::new(Mark::Good, "mcp", "no servers");
    }
    let disabled = &setup.settings.disabled_mcp_servers;
    let off: Vec<&str> = (servers.iter())
        .map(|server| server.name.as_str())
        .filter(|name| disabled.iter().any(|d| d == name))
        .collect();
    let off = match off.is_empty() {
        true => String::new(),
        false => format!("; disabled: {}", off.join(", ")),
    };
    let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
    let listed = format!(
        "{} server{} ({}){off}",
        servers.len(),
        if servers.len() == 1 { "" } else { "s" },
        names.join(", ")
    );
    let Some(states) = &setup.mcp else {
        let check = Check::new(Mark::Good, "mcp", listed);
        return check.with(["not started here: /mcp in a session says how each is".to_owned()]);
    };
    let failed: Vec<String> = (states.iter())
        .filter_map(|(name, state)| match state {
            McpState::Failed(why) => Some(format!("{name}: {why}")),
            _ => None,
        })
        .collect();
    let starting = (states.iter()).filter(|(_, state)| matches!(state, McpState::Starting));
    let starting: Vec<String> = starting
        .map(|(name, _)| format!("{name}: starting"))
        .collect();
    match failed.is_empty() {
        true => Check::new(Mark::Good, "mcp", listed).with(starting),
        false => Check::new(Mark::Warn, "mcp", listed)
            .with(failed)
            .with(starting)
            .with(["/mcp reconnect starts them again".to_owned()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_marks_each_check_and_counts_what_failed() {
        let checks = [
            Check::new(Mark::Good, "program", "/usr/bin/scramjet"),
            Check::new(Mark::Warn, "settings", "settings.json: 1 problem")
                .with(["settings: thme: not a key; did you mean theme?".to_owned()]),
            Check::new(Mark::Bad, "servers", "nothing answers"),
        ];
        let text = text(&checks);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with(&format!("Scramjet {}", env!("CARGO_PKG_VERSION"))));
        assert_eq!(
            &lines[2..],
            [
                "✓ program   /usr/bin/scramjet",
                "⚠ settings  settings.json: 1 problem",
                "              settings: thme: not a key; did you mean theme?",
                "✗ servers   nothing answers",
                "",
                "1 check failed, 1 warning.",
            ]
        );
        let good = [Check::new(Mark::Good, "files", "writable")];
        assert!(super::text(&good).ends_with("\nAll checks passed.\n"));
    }

    #[test]
    fn a_server_on_this_machine_answers_or_is_named_with_why_not() {
        let listening = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listening.local_addr().unwrap().port();
        // The discard port, which nothing here listens on, as in the other tests.
        let closed = 9;
        let directory = std::env::temp_dir().join(format!("doctor-servers-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let read = |name: &str, providers: serde_json::Value| {
            let file = directory.join(name);
            let document = serde_json::json!({"providers": providers});
            std::fs::write(&file, document.to_string()).unwrap();
            Providers::read(vec![file], None)
        };
        let entry = |url: String, id: &str| {
            serde_json::json!({"baseUrl": url, "api": "openai-completions", "apiKey": "k",
                "models": [{"id": id}]})
        };
        let providers = read(
            "three.json",
            serde_json::json!({
                "up": entry(format!("http://127.0.0.1:{open}/v1"), "a"),
                "down": entry(format!("http://localhost:{closed}"), "b"),
                "far": entry("https://example.com".to_owned(), "c"),
            }),
        );
        let check = servers(&providers, Some("down/b"));
        assert_eq!(check.mark, Mark::Bad);
        assert_eq!(check.details.len(), 2, "{check:?}");
        let failed = format!("down at localhost:{closed} (the model in use): ");
        assert!(check.details[0].starts_with(&failed), "{check:?}");
        assert_eq!(check.details[1], format!("answers: up at 127.0.0.1:{open}"));
        let unused = servers(&providers, Some("up/a"));
        assert_eq!(unused.mark, Mark::Warn);
        assert!(unused.details[0].starts_with(&format!("down at localhost:{closed}: ")));
        let only_up = read(
            "one.json",
            serde_json::json!({"up": entry(format!("http://127.0.0.1:{open}"), "a")}),
        );
        let check = servers(&only_up, None);
        std::fs::remove_dir_all(&directory).unwrap();
        assert_eq!(check.mark, Mark::Good);
        assert_eq!(check.summary, format!("up at 127.0.0.1:{open}"));
        let far = servers(&Providers::default(), None);
        assert_eq!(far.summary, "no provider on this machine");
    }

    #[test]
    fn the_programs_on_path_are_found_in_its_order() {
        let directory = std::env::temp_dir().join(format!("doctor-path-{}", std::process::id()));
        let (first, second) = (directory.join("a"), directory.join("b"));
        for place in [&first, &second] {
            std::fs::create_dir_all(place).unwrap();
            let file = place.join(scramjet::os::program_file("doctor-probe"));
            std::fs::write(&file, "").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let path = std::env::join_paths([&first, &second]).unwrap();
        let found = on_path("doctor-probe", Some(path));
        std::fs::remove_dir_all(&directory).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.parent().unwrap().to_owned())
            .collect();
        assert_eq!(names, [first, second]);
    }
}
