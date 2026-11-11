//! MCP servers' tools. The servers the settings list start in the background as the agent does;
//! the model finds their tools with `mcp_search`, which shows how to call each, and runs one with
//! `mcp_call`. The tools of servers small enough are also offered directly, as
//! `mcp__SERVER__TOOL`. Which are is settled once no server is starting, so the tools offered,
//! and the cached start of the requests, stay the same while the servers run.
use super::image;
use super::sandbox::{self, Hidden, Sandbox};

/// The sandbox the servers that run as programs start in, with the credential files named
/// besides the common stores; what it hides is looked up again at each start.
pub(super) type Confined = (Sandbox, Vec<PathBuf>);
use super::tools::{Group, MAX_BYTES, MAX_LINES, Output};
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion,
        ServerPeerInfo,
    },
    service::{ClientInitializeError, Peer, RunningService, ServiceError},
    transport::{
        DynamicTransportError, StreamableHttpClientTransport,
        streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
    },
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
use tokio::io::AsyncReadExt;

mod http;

pub const SEARCH: &str = "mcp_search";
pub const CALL: &str = "mcp_call";
pub(super) const TOOLS: [&str; 2] = [SEARCH, CALL];
/// What starts the name of a tool offered directly, `mcp__SERVER__TOOL`, as Claude Code names
/// them.
const DIRECT: &str = "mcp__";
/// The tools offered directly take at most this share of the context window (1/50: 2%; MCP's
/// client best practices suggest 1–5%), in tokens counted as 4 bytes of their definitions.
const DIRECT_SHARE: u64 = 50;
/// The longest tool name OpenAI's API takes.
const NAME_CHARS: usize = 64;

pub(super) const SEARCH_SNIPPET: &str =
    "Find tools of the MCP servers listed below, with how to call each";
pub(super) const CALL_SNIPPET: &str = "Call a tool mcp_search found";

/// How long a server has to start and list its tools: Claude Code's default `MCP_TIMEOUT`
/// (2.1.259).
const START: Duration = Duration::from_secs(30);
/// How long a program stopped has to exit, after its input closes and again after SIGTERM: the
/// MCP SDK's stdio client in Claude Code 2.1.259.
const QUIT: Duration = Duration::from_secs(2);
/// How long listing a server's tools again for a search may take before its last list is used.
const LIST: Duration = Duration::from_secs(10);
/// The last lines of a server's standard error kept to say why it failed.
const STDERR_LINES: usize = 20;
const STDERR_LINE_CHARS: usize = 300;
/// What is read of a line of standard error before the rest of it is dropped.
const STDERR_LINE_BYTES: usize = 4 * STDERR_LINE_CHARS;
const DEFAULT_MATCHES: usize = 5;
const MAX_MATCHES: usize = 20;
/// The tools' names a search that matches none lists, at most.
const LISTED_NAMES: usize = 200;
/// A description in a list of tools, or a server's in the instructions, is cut to this.
const ABOUT_CHARS: usize = 200;

/// A server the settings list (`mcpServers`), as Claude Code's.
#[derive(Clone, Debug, PartialEq)]
pub struct Server {
    pub name: String,
    pub transport: Transport,
}

#[derive(Clone, PartialEq)]
pub enum Transport {
    /// A program speaking MCP over its standard input and output, run in the project directory
    /// with the environment and `env`.
    Program {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// A server at `url` speaking MCP's streamable HTTP, sent `headers` with each request.
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
}

/// Leaves the headers' values out: they hold keys.
impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Program { command, args, env } => (f.debug_struct("Program"))
                .field("command", command)
                .field("args", args)
                .field("env", env)
                .finish(),
            Self::Http { url, headers } => (f.debug_struct("Http"))
                .field("url", url)
                .field(
                    "headers",
                    &headers.iter().map(|(name, _)| name).collect::<Vec<_>>(),
                )
                .finish(),
        }
    }
}

/// Headers a server's entry cannot set: the transport sets them.
const OWN_HEADERS: [&str; 7] = [
    "host",
    "content-type",
    "content-length",
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

impl Server {
    /// A server over HTTP, its URL and headers checked.
    pub fn http(name: &str, url: &str, headers: Vec<(String, String)>) -> Result<Self, String> {
        http::target(url).map_err(|why| format!("url: {why}"))?;
        for (name, value) in &headers {
            if hyper::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                return Err(format!("headers: {name:?} is not a header's name"));
            }
            if OWN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                return Err(format!("headers: {name} is set by the transport"));
            }
            if hyper::header::HeaderValue::from_str(value).is_err() {
                return Err(format!("headers: {name}'s value is not a header's"));
            }
        }
        Ok(Self {
            name: name.to_owned(),
            transport: Transport::Http {
                url: url.to_owned(),
                headers,
            },
        })
    }

    /// What `/mcp` shows of where it is, after its label: the program run, or the URL without
    /// its query, which may hold a key.
    pub fn address(&self) -> (&'static str, String) {
        match &self.transport {
            Transport::Program { command, args, .. } => {
                let words = (std::iter::once(command).chain(args)).map(|word| {
                    match word.contains(char::is_whitespace) || word.is_empty() {
                        true => format!("'{word}'"),
                        false => word.clone(),
                    }
                });
                ("Command:", words.collect::<Vec<_>>().join(" "))
            }
            Transport::Http { url, .. } => match url.split_once('?') {
                Some((url, _)) => ("URL:", format!("{url}?…")),
                None => ("URL:", url.clone()),
            },
        }
    }
}

/// What `/mcp` shows of a server.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Starting,
    /// With its tools' names, what it says it is, and whether its tools are offered directly,
    /// once that is settled.
    Ready {
        tools: Vec<String>,
        about: Option<String>,
        direct: Option<bool>,
    },
    /// Why it could not start, or that it ended, with the end of its standard error.
    Failed(String),
}

/// The servers running, each started once; they are killed with this.
pub(super) struct Servers {
    runtime: Option<tokio::runtime::Runtime>,
    servers: Vec<Arc<Running>>,
    /// How long each has to start.
    within: Duration,
    /// The instructions' list of the servers and the tools offered directly, made once none is
    /// starting.
    listing: Mutex<Option<Arc<Listing>>>,
    /// The sandbox the servers that run as programs start in.
    confined: Option<Arc<Confined>>,
}

struct Listing {
    section: String,
    /// The definitions of the tools offered directly, and their servers' names.
    direct: Vec<Value>,
    servers: Vec<String>,
}

struct Running {
    config: Server,
    state: Mutex<Status>,
    changed: Condvar,
    stderr: Mutex<VecDeque<String>>,
    /// The process group it runs in, until its process ended.
    group: Mutex<Option<Group>>,
    /// Whether it was killed, so that a process it starts after is killed too.
    killed: AtomicBool,
    /// What waits for its process to end.
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Its connection once started, closed when it is stopped, which ends an HTTP session.
    service: Mutex<Option<RunningService<RoleClient, ClientConfig>>>,
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
    peer: Peer<RoleClient>,
    about: Option<String>,
    /// Its tools as it last listed them, as MCP describes them (`name`, `description`,
    /// `inputSchema`, …).
    tools: Mutex<Vec<Value>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Servers {
    /// Starts each of `configs` in `root`, in the background, those that run as programs in
    /// `confined`'s sandbox.
    pub fn start(configs: &[Server], root: &Path, confined: Option<Confined>) -> Self {
        Self::start_within(configs, root, START, confined.map(Arc::new))
    }

    /// As `start`, each server given `within` to start.
    fn start_within(
        configs: &[Server],
        root: &Path,
        within: Duration,
        confined: Option<Arc<Confined>>,
    ) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("mcp")
            .enable_all()
            .build();
        let mut servers = Self {
            runtime: None,
            servers: Vec::new(),
            within,
            listing: Mutex::new(None),
            confined,
        };
        match runtime {
            Ok(runtime) => servers.runtime = Some(runtime),
            Err(error) => {
                for config in configs {
                    let server = Running::new(config);
                    server.set(Status::Failed(format!("could not start: {error}")));
                    servers.servers.push(server);
                }
                return servers;
            }
        }
        servers.servers = (configs.iter())
            .map(|config| servers.launch(config, root))
            .collect();
        servers
    }

    /// `/reload`: keeps the servers running as `configs` lists them, starts the others, also those
    /// that ended, and kills those it no longer lists.
    pub fn update(&mut self, configs: &[Server], root: &Path, confined: Option<Confined>) {
        self.confined = confined.map(Arc::new);
        if self.runtime.is_none() {
            *self = Self::start_within(configs, root, self.within, self.confined.clone());
            return;
        }
        let mut old = std::mem::take(&mut self.servers);
        let mut same = old.len() == configs.len();
        let kept: Vec<Option<Arc<Running>>> = (configs.iter())
            .map(|config| {
                let index = (old.iter()).position(|server| {
                    server.config == *config && !matches!(*lock(&server.state), Status::Failed(_))
                });
                same &= index == Some(0);
                Some(old.remove(index?))
            })
            .collect();
        // Before the new ones start, which may need what an old one holds (a port, a lock).
        self.end(&old);
        let servers = (configs.iter().zip(kept))
            .map(|(config, kept)| kept.unwrap_or_else(|| self.launch(config, root)))
            .collect();
        self.servers = servers;
        if !same {
            *lock(&self.listing) = None;
        }
    }

    /// `/mcp reconnect`: kills the server named `name`, or every server with `None`, and starts
    /// it again as its entry says. Returns the names started again.
    pub fn reconnect(&mut self, name: Option<&str>, root: &Path) -> Result<Vec<String>, String> {
        if let Some(name) = name {
            self.named(name)?;
        }
        if self.runtime.is_none() {
            let configs: Vec<Server> = (self.servers.iter())
                .map(|server| server.config.clone())
                .collect();
            *self = Self::start_within(&configs, root, self.within, self.confined.clone());
            return Ok(configs.into_iter().map(|config| config.name).collect());
        }
        let chosen = |server: &Running| name.is_none_or(|name| server.config.name == name);
        let old: Vec<Arc<Running>> = (self.servers.iter())
            .filter(|server| chosen(server))
            .cloned()
            .collect();
        self.end(&old);
        let mut names = Vec::new();
        for index in 0..self.servers.len() {
            if chosen(&self.servers[index]) {
                let config = self.servers[index].config.clone();
                names.push(config.name.clone());
                self.servers[index] = self.launch(&config, root);
            }
        }
        *lock(&self.listing) = None;
        Ok(names)
    }

    /// Takes `name`'s failure as said, so that the next request does not say it again.
    pub fn reported(&self, name: &str) {
        if let Ok(server) = self.named(name) {
            server.reported.store(true, Ordering::Relaxed);
        }
    }

    /// Starts `config`'s server in the background.
    fn launch(&self, config: &Server, root: &Path) -> Arc<Running> {
        let server = Running::new(config);
        let runtime = self.runtime.as_ref().expect("launched on a runtime");
        let (started, root, within) = (server.clone(), root.to_owned(), self.within);
        let confined = self.confined.clone();
        runtime.spawn(async move {
            let started_up = start(&started, root, confined.as_deref());
            let status = match tokio::time::timeout(within, started_up).await {
                Ok(Ok(ready)) => Status::Ready(Arc::new(ready)),
                Ok(Err(error)) => match (&confined, &started.config.transport) {
                    (Some(confined), Transport::Program { .. }) => Status::Failed(format!(
                        "in the sandbox (--sandbox {}), which hides credential stores and \
                         variables: {error}",
                        confined.0.mode.name()
                    )),
                    _ => Status::Failed(error),
                },
                Err(_) => {
                    started.kill();
                    Status::Failed(format!("it did not start within {} s", within.as_secs()))
                }
            };
            started.set(status);
        });
        server
    }

    /// Stops `servers` as MCP's stdio shutdown does: closes their connections, which closes a
    /// program's standard input and ends an HTTP session, gives each program `QUIT` to exit, then
    /// SIGTERM and `QUIT` again, then SIGKILL; and waits a moment, so that none is left unreaped.
    /// One still starting is killed at once.
    fn end(&self, servers: &[Arc<Running>]) {
        for server in servers {
            server.killed.store(true, Ordering::Relaxed);
        }
        let mut services = Vec::new();
        for server in servers {
            match lock(&server.service).take() {
                Some(service) if self.runtime.is_some() => services.push(service),
                _ => server.kill(),
            }
        }
        let Some(runtime) = &self.runtime else {
            return;
        };
        let watchers: Vec<_> = (servers.iter())
            .filter_map(|server| lock(&server.watcher).take())
            .collect();
        let ended = || async {
            while !watchers.iter().all(tokio::task::JoinHandle::is_finished) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        runtime.block_on(async {
            let closed =
                futures_util::future::join_all(services.iter_mut().map(RunningService::close));
            let _ = tokio::time::timeout(QUIT, async { tokio::join!(closed, ended()) }).await;
            if !watchers.iter().all(tokio::task::JoinHandle::is_finished) {
                servers.iter().for_each(|server| server.terminate());
                let _ = tokio::time::timeout(QUIT, ended()).await;
            }
            servers.iter().for_each(|server| server.kill());
            let _ = tokio::time::timeout(Duration::from_secs(1), ended()).await;
        });
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
        let listing = lock(&self.listing).clone();
        self.states_of(listing.as_ref().map(|listing| &listing.servers[..]))
    }

    /// Their states, those named in `direct` offered directly.
    fn states_of(&self, direct: Option<&[String]>) -> Vec<(&Server, State)> {
        let state = |server: &Running| match &*lock(&server.state) {
            Status::Starting => State::Starting,
            Status::Ready(ready) => State::Ready {
                tools: (lock(&ready.tools).iter())
                    .map(|tool| tool["name"].as_str().unwrap_or("").to_owned())
                    .collect(),
                about: ready.about.clone(),
                direct: direct.map(|direct| direct.contains(&server.config.name)),
            },
            Status::Failed(why) => State::Failed(server.failure(why)),
        };
        (self.servers.iter())
            .map(|server| (&server.config, state(server)))
            .collect()
    }

    /// The instructions' list of the servers, one line each, as it was once none was starting,
    /// so that it stays the same while they run; empty while one is starting.
    pub fn section(&self, window: Option<u64>) -> String {
        (self.listing(window)).map_or_else(String::new, |listing| listing.section.clone())
    }

    /// The definitions of the tools offered directly, settled with `section`: those of the
    /// servers, in order, that fit in what is left of a fiftieth of a `window` known.
    pub fn direct(&self, window: Option<u64>) -> Vec<Value> {
        (self.listing(window)).map_or_else(Vec::new, |listing| listing.direct.clone())
    }

    fn listing(&self, window: Option<u64>) -> Option<Arc<Listing>> {
        let mut listing = lock(&self.listing);
        if let Some(listing) = &*listing {
            return Some(listing.clone());
        }
        if !self.starting().is_empty() {
            return None;
        }
        let mut left = window.unwrap_or(0) / DIRECT_SHARE;
        let (mut direct, mut servers, mut others) = (Vec::new(), Vec::new(), 0);
        let lines: Vec<String> = (self.states_of(None).into_iter())
            .map(|(config, state)| match state {
                State::Ready { tools, about, .. } => {
                    let count = match tools.len() {
                        1 => "1 tool".to_owned(),
                        count => format!("{count} tools"),
                    };
                    let server = self.named(&config.name).ok().and_then(|server| {
                        let definitions = direct_definitions(server)?;
                        let tokens = serde_json::to_string(&definitions).ok()?.len() as u64 / 4;
                        left = left.checked_sub(tokens)?;
                        Some(definitions)
                    });
                    let count = match server {
                        Some(definitions) => {
                            direct.extend(definitions);
                            servers.push(config.name.clone());
                            format!("{count}, in your tool list as {DIRECT}{}__*", config.name)
                        }
                        None => {
                            others += 1;
                            count
                        }
                    };
                    match about {
                        Some(about) => format!("- {}: {about} ({count})", config.name),
                        None => format!("- {} ({count})", config.name),
                    }
                }
                _ => {
                    others += 1;
                    format!("- {}: not available", config.name)
                }
            })
            .collect();
        let text = match (direct.is_empty(), others == 0) {
            (true, _) => format!(
                "These MCP servers' tools are not in your tool list: find them with {SEARCH}, \
which shows how to call each, then run one with {CALL}."
            ),
            (false, true) => "These MCP servers' tools are in your tool list.".to_owned(),
            (false, false) => format!(
                "The tools of the MCP servers marked as in your tool list are there; find the \
others' with {SEARCH}, which shows how to call each, then run one with {CALL}."
            ),
        };
        let section = format!("\n\n# MCP servers\n\n{text}\n{}", lines.join("\n"));
        let made = Arc::new(Listing {
            section,
            direct,
            servers,
        });
        *listing = Some(made.clone());
        Some(made)
    }

    /// `mcp_search`: the tools whose names and descriptions best match the query's words, each
    /// with its input schema; with an empty query, every tool's name. Servers that do not list
    /// their tools again before `cancel` is set are searched as last listed.
    pub fn search(&self, arguments: &Value, cancel: &AtomicBool) -> Result<String, String> {
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
            match self.listed(server, cancel) {
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
        Ok(head(&text))
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
            let listed = self.listed(server, cancel)?;
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
        match until_cancelled(runtime, ready.peer.call_tool(params), cancel) {
            None => Err(format!("cancelled; {name}'s {tool} may have run in part")),
            Some(Err(error)) => Err(format!("{name}: {}", service_failure(&error))),
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
    /// changed), or as last listed when it does not answer in time or `cancel` is set.
    fn listed(&self, server: &Running, cancel: &AtomicBool) -> Result<Vec<Value>, String> {
        let ready = server.ready()?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or("the MCP servers are not running")?;
        let list = ready.peer.list_all_tools();
        let listed = until_cancelled(
            runtime,
            async { tokio::time::timeout(LIST, list).await },
            cancel,
        );
        let mut tools = lock(&ready.tools);
        if let Some(Ok(Ok(listed))) = listed {
            *tools = (listed.iter())
                .map(|tool| serde_json::to_value(tool).unwrap_or_default())
                .collect();
        }
        Ok(tools.clone())
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        self.end(&self.servers);
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// `future`'s output, or `None` once `cancel` is set, which drops it.
fn until_cancelled<F: Future>(
    runtime: &tokio::runtime::Runtime,
    future: F,
    cancel: &AtomicBool,
) -> Option<F::Output> {
    runtime.block_on(async {
        tokio::pin!(future);
        loop {
            tokio::select! {
                output = &mut future => return Some(output),
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if cancel.load(Ordering::Relaxed) {
                        return None;
                    }
                }
            }
        }
    })
}

impl Running {
    fn new(config: &Server) -> Arc<Self> {
        Arc::new(Self {
            config: config.clone(),
            state: Mutex::new(Status::Starting),
            changed: Condvar::new(),
            stderr: Mutex::new(VecDeque::new()),
            group: Mutex::new(None),
            killed: AtomicBool::new(false),
            watcher: Mutex::new(None),
            service: Mutex::new(None),
            reported: AtomicBool::new(false),
        })
    }

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

    fn terminate(&self) {
        if let Some(group) = &*lock(&self.group) {
            group.terminate();
        }
    }

    fn kill(&self) {
        let mut group = lock(&self.group);
        self.killed.store(true, Ordering::Relaxed);
        if let Some(group) = group.take() {
            group.kill();
        }
    }

    /// Keeps a line of its standard error, cut short.
    fn keep(&self, line: &[u8]) {
        let line = String::from_utf8_lossy(line);
        let mut kept = lock(&self.stderr);
        if kept.len() == STDERR_LINES {
            kept.pop_front();
        }
        let line = line.trim_end_matches('\r').chars().take(STDERR_LINE_CHARS);
        kept.push_back(line.collect());
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

/// Starts the server and connects with the `initialize` handshake, or reaches it over HTTP and
/// connects as MCP's 2026-07-28 revision does, or with `initialize` as the server answers; then
/// lists its tools. A program is not probed first, as Claude Code 2.1.259 does not: rmcp waits 10 s
/// for the probe's answer, which a server still installing (`uvx`, `npx`) gives late, after
/// `initialize` went out.
async fn start(
    server: &Arc<Running>,
    root: PathBuf,
    confined: Option<&Confined>,
) -> Result<Ready, String> {
    let client = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("scramjet", env!("CARGO_PKG_VERSION")),
    );
    let lifecycle = ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::LATEST],
        legacy_version: Some(ProtocolVersion::LATEST_WITH_INITIALIZE),
    };
    let service = match &server.config.transport {
        Transport::Program { command, args, env } => {
            let program = (command.as_str(), &args[..], &env[..]);
            let client = client.with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE);
            let lifecycle = ClientLifecycleMode::Initialize;
            run(server, program, root, confined, client, lifecycle).await?
        }
        Transport::Http { url, headers } => {
            let headers = (headers.iter())
                .filter_map(|(name, value)| {
                    let name = hyper::header::HeaderName::from_bytes(name.as_bytes()).ok()?;
                    Some((name, hyper::header::HeaderValue::from_str(value).ok()?))
                })
                .collect();
            let config =
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).custom_headers(headers);
            let transport = StreamableHttpClientTransport::with_client(http::Https, config);
            (client.serve_with_lifecycle(transport, lifecycle).await).map_err(
                |error| match error {
                    ClientInitializeError::TransportError { error, .. } => {
                        transport_failure(&error)
                    }
                    error => format!("it did not answer as an MCP server: {error}"),
                },
            )?
        }
    };
    let info = service.peer_info();
    let tools = match info
        .as_ref()
        .is_some_and(|info| info.capabilities.tools.is_some())
    {
        true => (service.list_all_tools().await).map_err(|error| {
            format!("its tools could not be listed: {}", service_failure(&error))
        })?,
        false => Vec::new(),
    };
    let ready = Ready {
        peer: service.peer().clone(),
        about: info.as_deref().and_then(about),
        tools: Mutex::new(
            (tools.iter())
                .map(|tool| serde_json::to_value(tool).unwrap_or_default())
                .collect(),
        ),
    };
    let mut held = lock(&server.service);
    if server.killed.load(Ordering::Relaxed) {
        return Err("it was stopped as it started".to_owned());
    }
    *held = Some(service);
    Ok(ready)
}

/// Runs the server's program, in `confined`'s sandbox if any, keeps the end of its standard
/// error, and connects to it over its standard input and output.
async fn run(
    server: &Arc<Running>,
    (program, args, env): (&str, &[String], &[(String, String)]),
    root: PathBuf,
    confined: Option<&Confined>,
    client: ClientConfig,
    lifecycle: ClientLifecycleMode,
) -> Result<RunningService<RoleClient, ClientConfig>, String> {
    let mut command = match confined {
        Some((sandbox, named)) => {
            sandbox::server(&root, program, &Hidden::current(named), *sandbox)?.into()
        }
        None => {
            let path = env
                .iter()
                .rev()
                .find(|(key, _)| key.eq_ignore_ascii_case("PATH"));
            let path = path.map(|(_, path)| std::ffi::OsStr::new(path));
            tokio::process::Command::new(crate::os::program(program, path))
        }
    };
    command
        .args(args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::os::own_group_async(&mut command);
    let (mut child, group) = Group::started(|| {
        let child = command.spawn()?;
        let id = child
            .id()
            .ok_or_else(|| std::io::Error::other("it ended at once"))?;
        Ok((child, id))
    })
    .map_err(|error| format!("could not run {program}: {error}"))?;
    {
        let mut held = lock(&server.group);
        if server.killed.load(Ordering::Relaxed) {
            group.kill();
            return Err("it was stopped as it started".to_owned());
        }
        *held = Some(group);
    }
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        server.kill();
        return Err("its standard input and output could not be read".to_owned());
    };
    let reader = server.clone();
    // Read to its end, whatever its bytes: a server whose standard error is closed fails writing.
    tokio::spawn(async move {
        let (mut stderr, mut chunk, mut line) = (stderr, [0; 4096], Vec::new());
        while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
            for &byte in &chunk[..read] {
                match byte {
                    b'\n' => reader.keep(&std::mem::take(&mut line)),
                    _ if line.len() < STDERR_LINE_BYTES => line.push(byte),
                    _ => {}
                }
            }
        }
        if !line.is_empty() {
            reader.keep(&line);
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
    match client
        .serve_with_lifecycle((stdout, stdin), lifecycle)
        .await
    {
        Ok(service) => Ok(service),
        Err(error) => {
            // Its standard error says more once the process ended.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let why = match ended.load(Ordering::Relaxed) {
                true => "it ended before it answered".to_owned(),
                false => format!("it did not answer as an MCP server: {error}"),
            };
            server.kill();
            Err(why)
        }
    }
}

/// What a transport's failure says, without the transport's type, which rmcp names.
fn transport_failure(error: &DynamicTransportError) -> String {
    match error
        .error
        .downcast_ref::<StreamableHttpError<http::Failed>>()
    {
        Some(StreamableHttpError::Client(failed)) => failed.to_string(),
        Some(other) => other.to_string(),
        None => error.error.to_string(),
    }
}

fn service_failure(error: &ServiceError) -> String {
    match error {
        ServiceError::TransportSend(error) => transport_failure(error),
        error => error.to_string(),
    }
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

/// The server and tool a direct tool's name names, if it is one.
pub(super) fn direct_name(name: &str) -> Option<(&str, &str)> {
    let (server, tool) = name.strip_prefix(DIRECT)?.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

/// The definitions of the server's tools offered directly, if each can be: its name and theirs
/// fit in a tool's name that `direct_name` reads back, and each takes an object.
fn direct_definitions(server: &Running) -> Option<Vec<Value>> {
    let name = server.config.name.as_str();
    let word = |text: &str| {
        !text.is_empty()
            && (text.chars()).all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    if !word(name) || name.contains("__") || name.starts_with('_') || name.ends_with('_') {
        return None;
    }
    let ready = server.ready().ok()?;
    let tools = lock(&ready.tools);
    if tools.is_empty() {
        return None;
    }
    (tools.iter())
        .map(|tool| {
            let tool_name = tool["name"].as_str().filter(|tool| word(tool))?;
            let full = format!("{DIRECT}{name}__{tool_name}");
            let schema = tool
                .get("inputSchema")
                .filter(|schema| schema["type"] == "object")?;
            let description = (tool["description"].as_str())
                .or(tool["title"].as_str())
                .unwrap_or("");
            (full.len() <= NAME_CHARS).then(|| {
                json!({"type":"function","name":full,"strict":false,
                    "description":description.trim(),"parameters":schema})
            })
        })
        .collect()
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
            let given = given(arguments);
            format!(
                "{}/{}{}",
                text("server"),
                text("tool"),
                cut(&given, SHOWN_CHARS)
            )
        }
    }
}

/// How much of a call's arguments its call line shows.
const SHOWN_CHARS: usize = 200;

fn given(arguments: &Value) -> String {
    match &arguments["arguments"] {
        Value::Null => String::new(),
        Value::Object(object) if object.is_empty() => String::new(),
        given => format!(" {given}"),
    }
}

/// A call's arguments in full, for its approval, when its call line shows only their start.
pub(super) fn unseen_arguments(arguments: &Value) -> Option<String> {
    let cut = given(arguments).chars().nth(SHOWN_CHARS).is_some();
    let pretty = serde_json::to_string_pretty(&arguments["arguments"]).ok()?;
    cut.then(|| format!("The call's arguments in full:\n\n```json\n{pretty}\n```"))
}

#[cfg(test)]
mod tests;
