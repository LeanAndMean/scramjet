//! MCP servers' tools. The servers the settings list start in the background as the agent does;
//! the model finds their tools with `mcp_search`, which shows how to call each, and runs one with
//! `mcp_call`. So the tools it is offered, and the cached start of its requests, stay the same
//! however many servers and tools there are.
use super::image;
use super::tools::{Group, MAX_BYTES, MAX_LINES, Output};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion,
        ServerPeerInfo,
    },
    service::RunningService,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncBufReadExt;

pub const SEARCH: &str = "mcp_search";
pub const CALL: &str = "mcp_call";
pub(super) const TOOLS: [&str; 2] = [SEARCH, CALL];

pub(super) const SEARCH_SNIPPET: &str =
    "Find tools of the MCP servers listed below, with how to call each";
pub(super) const CALL_SNIPPET: &str = "Call a tool mcp_search found";

/// How long a server has to start and list its tools: Claude Code's default `MCP_TIMEOUT`
/// (2.1.259).
const START: Duration = Duration::from_secs(30);
/// How long listing a server's tools again for a search may take before its last list is used.
const LIST: Duration = Duration::from_secs(10);
/// The last lines of a server's standard error kept to say why it failed.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 300;
const DEFAULT_MATCHES: usize = 5;
const MAX_MATCHES: usize = 20;
/// The tools' names a search that matches none lists, at most.
const LISTED_NAMES: usize = 200;
/// A description in a list of tools, or a server's in the instructions, is cut to this.
const ABOUT_CHARS: usize = 200;

/// A server the settings list (`mcpServers`), as Claude Code's: a program speaking MCP over its
/// standard input and output, run in the project directory with the environment and `env`.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Server {
    /// How it is run, as `/mcp` shows it.
    pub fn command_line(&self) -> String {
        std::iter::once(&self.command)
            .chain(&self.args)
            .map(
                |word| match word.contains(char::is_whitespace) || word.is_empty() {
                    true => format!("'{word}'"),
                    false => word.clone(),
                },
            )
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// What `/mcp` shows of a server.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Starting,
    /// With its tools' names and what it says it is.
    Ready {
        tools: Vec<String>,
        about: Option<String>,
    },
    /// Why it could not start, or that it ended, with the end of its standard error.
    Failed(String),
}

/// The servers running, each started once; they are killed with this.
pub(super) struct Servers {
    runtime: Option<tokio::runtime::Runtime>,
    servers: Vec<Arc<Running>>,
    /// The instructions' list of the servers, made once none is starting.
    section: Mutex<Option<String>>,
}

struct Running {
    config: Server,
    state: Mutex<Status>,
    changed: Condvar,
    stderr: Mutex<VecDeque<String>>,
    /// The process group it runs in, until its process ended.
    group: Mutex<Option<Group>>,
    /// What waits for its process to end.
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Whether the user was told it failed.
    reported: AtomicBool,
}

enum Status {
    Starting,
    Ready(Arc<Ready>),
    /// Why, without its standard error, which only the user is shown, as it may hold secrets.
    Failed(String),
}

struct Ready {
    service: RunningService<RoleClient, ClientConfig>,
    about: Option<String>,
    /// Its tools as it last listed them, as MCP describes them (`name`, `description`,
    /// `inputSchema`, …).
    tools: Mutex<Vec<Value>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Servers {
    /// Starts each of `configs` in `root`, in the background.
    pub fn start(configs: &[Server], root: &Path) -> Self {
        Self::start_within(configs, root, START)
    }

    /// As `start`, each server given `within` to start.
    fn start_within(configs: &[Server], root: &Path, within: Duration) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("mcp")
            .enable_all()
            .build();
        let servers: Vec<Arc<Running>> = (configs.iter())
            .map(|config| {
                Arc::new(Running {
                    config: config.clone(),
                    state: Mutex::new(Status::Starting),
                    changed: Condvar::new(),
                    stderr: Mutex::new(VecDeque::new()),
                    group: Mutex::new(None),
                    watcher: Mutex::new(None),
                    reported: AtomicBool::new(false),
                })
            })
            .collect();
        let runtime = match runtime {
            Ok(runtime) => runtime,
            Err(error) => {
                for server in &servers {
                    server.set(Status::Failed(format!("could not start: {error}")));
                }
                return Self {
                    runtime: None,
                    servers,
                    section: Mutex::new(None),
                };
            }
        };
        for server in &servers {
            let (server, root) = (server.clone(), root.to_owned());
            runtime.spawn(async move {
                let status = match tokio::time::timeout(within, start(&server, root)).await {
                    Ok(Ok(ready)) => Status::Ready(Arc::new(ready)),
                    Ok(Err(error)) => Status::Failed(error),
                    Err(_) => {
                        server.kill();
                        Status::Failed(format!("it did not start within {} s", within.as_secs()))
                    }
                };
                server.set(status);
            });
        }
        Self {
            runtime: Some(runtime),
            servers,
            section: Mutex::new(None),
        }
    }

    pub fn configs(&self) -> Vec<&Server> {
        self.servers.iter().map(|server| &server.config).collect()
    }

    /// The names of the servers still starting.
    pub fn starting(&self) -> Vec<&str> {
        (self.servers.iter())
            .filter(|server| matches!(*lock(&server.state), Status::Starting))
            .map(|server| server.config.name.as_str())
            .collect()
    }

    /// Waits until no server is starting, or `cancel` is set.
    pub fn settle(&self, cancel: &AtomicBool) {
        for server in &self.servers {
            let mut state = lock(&server.state);
            while matches!(*state, Status::Starting) && !cancel.load(Ordering::Relaxed) {
                state = (server.changed)
                    .wait_timeout(state, Duration::from_millis(50))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
    }

    /// The servers that failed and were not said to, each said once: their names and why, with
    /// the end of their standard error.
    pub fn unreported_failures(&self) -> Vec<(String, String)> {
        let mut failures = Vec::new();
        for server in &self.servers {
            if let Status::Failed(why) = &*lock(&server.state)
                && !server.reported.swap(true, Ordering::Relaxed)
            {
                failures.push((server.config.name.clone(), server.failure(why)));
            }
        }
        failures
    }

    pub fn states(&self) -> Vec<(&Server, State)> {
        let state = |server: &Running| match &*lock(&server.state) {
            Status::Starting => State::Starting,
            Status::Ready(ready) => State::Ready {
                tools: (lock(&ready.tools).iter())
                    .map(|tool| tool["name"].as_str().unwrap_or("").to_owned())
                    .collect(),
                about: ready.about.clone(),
            },
            Status::Failed(why) => State::Failed(server.failure(why)),
        };
        (self.servers.iter())
            .map(|server| (&server.config, state(server)))
            .collect()
    }

    /// The instructions' list of the servers, one line each, as it was once none was starting,
    /// so that it stays the same while they run; empty while one is starting.
    pub fn section(&self) -> String {
        let mut section = lock(&self.section);
        if let Some(section) = &*section {
            return section.clone();
        }
        if !self.starting().is_empty() {
            return String::new();
        }
        let lines: Vec<String> = (self.states().into_iter())
            .map(|(config, state)| match state {
                State::Ready { tools, about } => {
                    let count = match tools.len() {
                        1 => "1 tool".to_owned(),
                        count => format!("{count} tools"),
                    };
                    match about {
                        Some(about) => format!("- {}: {about} ({count})", config.name),
                        None => format!("- {} ({count})", config.name),
                    }
                }
                _ => format!("- {}: not available", config.name),
            })
            .collect();
        let text = format!(
            "\n\n# MCP servers\n\nThese MCP servers' tools are not in your tool list: find them \
with {SEARCH}, which shows how to call each, then run one with {CALL}.\n{}",
            lines.join("\n")
        );
        *section = Some(text.clone());
        text
    }

    /// `mcp_search`: the tools whose names and descriptions best match the query's words, each
    /// with its input schema; with an empty query, every tool's name.
    pub fn search(&self, arguments: &Value) -> Result<String, String> {
        let query = arguments["query"].as_str().unwrap_or("").trim();
        let only = arguments["server"].as_str().filter(|name| !name.is_empty());
        if let Some(name) = only {
            self.named(name)?;
        }
        let limit = (arguments["limit"].as_u64())
            .map_or(DEFAULT_MATCHES, |limit| limit as usize)
            .clamp(1, MAX_MATCHES);
        let searched: Vec<&Arc<Running>> = (self.servers.iter())
            .filter(|server| only.is_none_or(|name| server.config.name == name))
            .collect();
        let mut tools = Vec::new();
        let mut unavailable = Vec::new();
        for server in &searched {
            match self.listed(server) {
                Ok(listed) => tools.extend(listed.into_iter().map(|tool| (*server, tool))),
                Err(why) => unavailable.push(format!("{}: {why}", server.config.name)),
            }
        }
        let mut text = match query {
            "" => list_tools(&tools),
            _ => matching(query, &tools, limit),
        };
        if !unavailable.is_empty() {
            text += &format!("\n\nNot available:\n- {}", unavailable.join("\n- "));
        }
        Ok(text)
    }

    /// `mcp_call`: runs the tool on its server until it answers or `cancel` is set, which cancels
    /// the request.
    pub fn call(&self, arguments: &Value, cancel: &AtomicBool) -> Result<Output, String> {
        let name = arguments["server"]
            .as_str()
            .ok_or("missing string argument `server`")?;
        let tool = arguments["tool"]
            .as_str()
            .ok_or("missing string argument `tool`")?;
        let given = match &arguments["arguments"] {
            Value::Null => serde_json::Map::new(),
            Value::Object(object) => object.clone(),
            // Some models send the object as JSON text.
            Value::String(text) => match serde_json::from_str(text) {
                Ok(Value::Object(object)) => object,
                _ => return Err("`arguments` must be an object".to_owned()),
            },
            _ => return Err("`arguments` must be an object".to_owned()),
        };
        let server = self.named(name)?;
        let ready = server.ready()?;
        let known = |tools: &[Value]| tools.iter().any(|listed| listed["name"] == tool);
        if !known(&lock(&ready.tools)) {
            let listed = self.listed(server)?;
            if !known(&listed) {
                let names: Vec<&str> = (listed.iter())
                    .filter_map(|tool| tool["name"].as_str())
                    .collect();
                return Err(format!(
                    "{name} has no tool named {tool}; its tools: {}",
                    names.join(", ")
                ));
            }
        }
        let runtime = self
            .runtime
            .as_ref()
            .ok_or("the MCP servers are not running")?;
        let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(given);
        let result = runtime.block_on(async {
            let call = ready.service.call_tool(params);
            tokio::pin!(call);
            loop {
                tokio::select! {
                    result = &mut call => return Some(result),
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {
                        if cancel.load(Ordering::Relaxed) {
                            return None;
                        }
                    }
                }
            }
        });
        match result {
            None => Err(format!("cancelled; {name}'s {tool} may have run in part")),
            Some(Err(error)) => Err(format!("{name}: {error}")),
            Some(Ok(result)) => Ok(output(serde_json::to_value(result).unwrap_or_default())),
        }
    }

    fn named(&self, name: &str) -> Result<&Arc<Running>, String> {
        (self.servers.iter())
            .find(|server| server.config.name == name)
            .ok_or_else(|| {
                let names: Vec<&str> = (self.servers.iter())
                    .map(|server| server.config.name.as_str())
                    .collect();
                format!(
                    "no MCP server is named {name}; the servers: {}",
                    names.join(", ")
                )
            })
    }

    /// The server's tools, listed again (the client keeps a list until the server says it
    /// changed), or as last listed when it does not answer in time.
    fn listed(&self, server: &Running) -> Result<Vec<Value>, String> {
        let ready = server.ready()?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or("the MCP servers are not running")?;
        let list = ready.service.list_all_tools();
        let listed = runtime.block_on(async { tokio::time::timeout(LIST, list).await });
        let mut tools = lock(&ready.tools);
        if let Ok(Ok(listed)) = listed {
            *tools = (listed.iter())
                .map(|tool| serde_json::to_value(tool).unwrap_or_default())
                .collect();
        }
        Ok(tools.clone())
    }
}

impl Drop for Servers {
    /// Kills the servers, and waits a moment for their processes so that none is left unreaped.
    fn drop(&mut self) {
        for server in &self.servers {
            server.kill();
        }
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        let watchers: Vec<_> = (self.servers.iter())
            .filter_map(|server| lock(&server.watcher).take())
            .collect();
        runtime.block_on(async {
            let ended = async {
                for watcher in watchers {
                    let _ = watcher.await;
                }
            };
            let _ = tokio::time::timeout(Duration::from_secs(1), ended).await;
        });
        runtime.shutdown_background();
    }
}

impl Running {
    fn set(&self, status: Status) {
        *lock(&self.state) = status;
        self.changed.notify_all();
    }

    fn ready(&self) -> Result<Arc<Ready>, String> {
        match &*lock(&self.state) {
            Status::Ready(ready) => Ok(ready.clone()),
            Status::Starting => Err(format!("{} is still starting", self.config.name)),
            Status::Failed(why) => Err(format!("{} is not available: {why}", self.config.name)),
        }
    }

    fn kill(&self) {
        if let Some(group) = lock(&self.group).take() {
            group.kill();
        }
    }

    /// `why`, with the end of what the server wrote to its standard error.
    fn failure(&self, why: &str) -> String {
        let stderr = lock(&self.stderr);
        match stderr.is_empty() {
            true => why.to_owned(),
            false => format!("{why}; it wrote:\n{}", Vec::from(stderr.clone()).join("\n")),
        }
    }
}

/// Runs the server's program, keeps the end of its standard error, and connects to it as MCP's
/// 2026-07-28 revision does, or with the `initialize` handshake of those before, as the server
/// answers; then lists its tools.
async fn start(server: &Arc<Running>, root: PathBuf) -> Result<Ready, String> {
    let config = &server.config;
    let mut command = tokio::process::Command::new(&config.command);
    command
        .args(&config.args)
        .envs(config.env.iter().map(|(key, value)| (key, value)))
        .current_dir(root)
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (mut child, group) = Group::started(|| {
        let child = command.spawn()?;
        let id = child
            .id()
            .ok_or_else(|| std::io::Error::other("it ended at once"))?;
        Ok((child, id))
    })
    .map_err(|error| format!("could not run {}: {error}", config.command))?;
    *lock(&server.group) = Some(group);
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        server.kill();
        return Err("its standard input and output could not be read".to_owned());
    };
    let reader = server.clone();
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut kept = lock(&reader.stderr);
            if kept.len() == STDERR_LINES {
                kept.pop_front();
            }
            kept.push_back(line.chars().take(STDERR_LINE_CHARS).collect());
        }
    });
    let ended = Arc::new(AtomicBool::new(false));
    let (watched, watcher) = (server.clone(), ended.clone());
    *lock(&server.watcher) = Some(tokio::spawn(async move {
        let status = child.wait().await;
        watcher.store(true, Ordering::Relaxed);
        // Its process was waited for, so its group's id may go to another once the group is
        // empty: what is left of it is killed now, and never later.
        watched.kill();
        let why = match status {
            Ok(status) => format!("it ended ({status})"),
            Err(error) => format!("it ended: {error}"),
        };
        // One still starting fails on the closed connection and says so.
        let ready = matches!(*lock(&watched.state), Status::Ready(_));
        if ready {
            watched.set(Status::Failed(why));
        }
    }));
    let client = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("scramjet-rs", env!("CARGO_PKG_VERSION")),
    );
    let lifecycle = ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::LATEST],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    };
    let service = match client
        .serve_with_lifecycle((stdout, stdin), lifecycle)
        .await
    {
        Ok(service) => service,
        Err(error) => {
            // Its standard error says more once the process ended.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let why = match ended.load(Ordering::Relaxed) {
                true => "it ended before it answered".to_owned(),
                false => format!("it did not answer as an MCP server: {error}"),
            };
            server.kill();
            return Err(why);
        }
    };
    let info = service.peer_info();
    let tools = match info
        .as_ref()
        .is_some_and(|info| info.capabilities.tools.is_some())
    {
        true => (service.list_all_tools().await)
            .map_err(|error| format!("its tools could not be listed: {error}"))?,
        false => Vec::new(),
    };
    Ok(Ready {
        about: info.as_deref().and_then(about),
        tools: Mutex::new(
            (tools.iter())
                .map(|tool| serde_json::to_value(tool).unwrap_or_default())
                .collect(),
        ),
        service,
    })
}

/// What a server says it is, in one line: its description, title, or the first line of its
/// instructions.
fn about(info: &ServerPeerInfo) -> Option<String> {
    let implementation = info.server_info.as_ref();
    let text = (implementation.and_then(|server| server.description.clone()))
        .or_else(|| implementation.and_then(|server| server.title.clone()))
        .or_else(|| info.instructions.clone())?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    Some(cut(line, ABOUT_CHARS))
}

fn cut(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// The first line of a tool's description, cut short.
fn summary(tool: &Value) -> String {
    let description = tool["description"].as_str().unwrap_or("");
    cut(description.lines().next().unwrap_or("").trim(), ABOUT_CHARS)
}

/// Every tool's name and the first line of its description, by server.
fn list_tools(tools: &[(&Arc<Running>, Value)]) -> String {
    if tools.is_empty() {
        return "No MCP server offers any tool.".to_owned();
    }
    let mut text = String::new();
    let mut server = None;
    for (running, tool) in tools {
        if server != Some(&running.config.name) {
            server = Some(&running.config.name);
            text += &format!(
                "{}Server {}:\n",
                if text.is_empty() { "" } else { "\n" },
                running.config.name
            );
        }
        let name = tool["name"].as_str().unwrap_or("");
        text += &match summary(tool).as_str() {
            "" => format!("- {name}\n"),
            line => format!("- {name}: {line}\n"),
        };
    }
    text + "\nSearch for a tool by its name to see how to call it."
}

/// The `limit` tools that match `query` best, with their full descriptions and input schemas.
fn matching(query: &str, tools: &[(&Arc<Running>, Value)], limit: usize) -> String {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut scored: Vec<(usize, usize)> = (tools.iter().enumerate())
        .map(|(index, (server, tool))| (score(query, &words, &server.config.name, tool), index))
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    if scored.is_empty() {
        let names: Vec<String> = (tools.iter().take(LISTED_NAMES))
            .map(|(server, tool)| {
                format!(
                    "{}/{}",
                    server.config.name,
                    tool["name"].as_str().unwrap_or("")
                )
            })
            .collect();
        let more = tools.len().saturating_sub(LISTED_NAMES);
        let more = if more > 0 {
            format!(" and {more} more")
        } else {
            String::new()
        };
        return match names.is_empty() {
            true => format!("No tool matches \"{query}\": no MCP server offers any tool."),
            false => format!(
                "No tool matches \"{query}\". The tools (server/tool): {}{more}",
                names.join(", ")
            ),
        };
    }
    let shown = scored.len().min(limit);
    let mut text = match scored.len() {
        1 => format!("1 tool matches \"{query}\":"),
        count if count > shown => format!("{count} tools match \"{query}\"; the best {shown}:"),
        count => format!("{count} tools match \"{query}\":"),
    };
    for &(_, index) in &scored[..shown] {
        let (server, tool) = &tools[index];
        let name = tool["name"].as_str().unwrap_or("");
        text += &format!("\n\nServer: {}\nTool: {name}", server.config.name);
        if let Some(title) = tool["title"].as_str() {
            text += &format!(" ({title})");
        }
        if let Some(description) = tool["description"]
            .as_str()
            .filter(|d| !d.trim().is_empty())
        {
            text += &format!("\n{}", description.trim());
        }
        let schema = tool.get("inputSchema").unwrap_or(&Value::Null);
        text += &format!("\nArguments (JSON Schema): {schema}");
    }
    text
}

/// How well a tool matches: its name or `server/name` exactly, else each of the query's words
/// found in its name, title, description or server's name.
fn score(query: &str, words: &[String], server: &str, tool: &Value) -> usize {
    let name = tool["name"].as_str().unwrap_or("");
    if query == name || query == format!("{server}/{name}") {
        return 1000;
    }
    let field = |key: &str| tool[key].as_str().unwrap_or("").to_lowercase();
    let (lower, title, description) = (name.to_lowercase(), field("title"), field("description"));
    let server = server.to_lowercase();
    (words.iter())
        .map(|word| {
            let found = |text: &str, weight| {
                if text.contains(word.as_str()) {
                    weight
                } else {
                    0
                }
            };
            found(&lower, 3) + found(&title, 2) + found(&description, 1) + found(&server, 1)
        })
        .sum()
}

/// A tool's result as the model receives it: its text, what other content it holds, its
/// structured content when it has nothing else, and its first image.
fn output(result: Value) -> Output {
    let mut parts: Vec<String> = Vec::new();
    let mut attached = None;
    for block in result["content"].as_array().into_iter().flatten() {
        let mime = block["mimeType"].as_str().unwrap_or("");
        match block["type"].as_str().unwrap_or("") {
            "text" => parts.push(block["text"].as_str().unwrap_or("").to_owned()),
            "image" if attached.is_some() => parts.push(format!(
                "[Image [{mime}] not shown: one image is sent per result]"
            )),
            "image" => {
                let bytes = block["data"].as_str().and_then(image::decode_base64);
                let fitted = bytes.as_deref().and_then(|bytes| {
                    let mime = image::mime(bytes)?;
                    image::fit(bytes, mime)
                });
                match fitted {
                    Some(fitted) => {
                        let mut text = format!("Image [{}]", fitted.image.mime);
                        if let Some(note) = fitted.note() {
                            text = format!("{text}\n{note}");
                        }
                        parts.push(text);
                        attached = Some(fitted.image);
                    }
                    None => parts.push(format!("[Image [{mime}] not shown: it could not be read]")),
                }
            }
            "audio" => parts.push(format!("[Audio [{mime}] not shown]")),
            "resource_link" => {
                let (name, uri) = (
                    block["name"].as_str().unwrap_or(""),
                    block["uri"].as_str().unwrap_or(""),
                );
                parts.push(format!("[Resource {name}: {uri}]"));
            }
            "resource" => {
                let resource = &block["resource"];
                match resource["text"].as_str() {
                    Some(text) => parts.push(text.to_owned()),
                    None => parts.push(format!(
                        "[Resource {} [{}] not shown]",
                        resource["uri"].as_str().unwrap_or(""),
                        resource["mimeType"].as_str().unwrap_or("")
                    )),
                }
            }
            other => parts.push(format!("[{other} content not shown]")),
        }
    }
    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        parts.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    let text = match parts.is_empty() {
        true => "(no output)".to_owned(),
        false => head(&parts.join("\n")),
    };
    Output {
        text,
        detail: None,
        failed: result["isError"] == json!(true),
        image: attached,
    }
}

/// The start of `text`, at most `MAX_LINES` lines and `MAX_BYTES` bytes, saying what was cut.
fn head(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut kept = String::new();
    let mut shown = 0;
    for line in &lines {
        if shown == MAX_LINES || kept.len() + line.len() > MAX_BYTES {
            break;
        }
        kept.push_str(line);
        shown += 1;
    }
    if shown == lines.len() {
        return kept;
    }
    if shown == 0 {
        let mut end = MAX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        kept.push_str(&text[..end]);
    }
    let total = lines.len();
    format!(
        "{}\n[Output cut: showing {shown} of {total} lines ({} KB at most)]",
        kept.trim_end_matches('\n'),
        MAX_BYTES / 1024
    )
}

/// The definitions of `mcp_search` and `mcp_call`.
pub(super) fn definitions() -> [Value; 2] {
    [
        json!({"type":"function","name":SEARCH,"strict":false,
        "description":"Search the tools of the MCP servers the instructions list. Returns the best matches, each with its server, name, description and input schema, as mcp_call takes them; an empty query lists every tool's name.",
        "parameters":{"type":"object","properties":{
            "query":{"type":"string","description":"Words to look for in tools' names and descriptions, or a tool's exact name"},
            "server":{"type":"string","description":"Search only this server's tools"},
            "limit":{"type":"integer","description":format!("Maximum number of tools to return (default {DEFAULT_MATCHES}, at most {MAX_MATCHES})")}},
          "required":["query"]}}),
        json!({"type":"function","name":CALL,"strict":false,
        "description":"Call a tool of an MCP server, as mcp_search shows it, and return its result.",
        "parameters":{"type":"object","properties":{
            "server":{"type":"string","description":"The server's name"},
            "tool":{"type":"string","description":"The tool's name"},
            "arguments":{"type":"object","description":"The tool's arguments, as its input schema describes them"}},
          "required":["server","tool"]}}),
    ]
}

/// What a call line shows of a search or call.
pub(super) fn summary_line(name: &str, arguments: &Value) -> String {
    let text = |key: &str| arguments[key].as_str().unwrap_or("");
    match name {
        SEARCH => match text("server") {
            "" => format!("\"{}\"", text("query")),
            server => format!("\"{}\" in {server}", text("query")),
        },
        _ => {
            let given = match &arguments["arguments"] {
                Value::Null => String::new(),
                Value::Object(object) if object.is_empty() => String::new(),
                given => format!(" {given}"),
            };
            format!("{}/{}{}", text("server"), text("tool"), cut(&given, 200))
        }
    }
}

#[cfg(test)]
mod tests;
