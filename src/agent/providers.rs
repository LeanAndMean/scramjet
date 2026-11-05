//! The providers requests go to and what each says of its models, from `models.json` files in
//! Scramjet's format (`providers`, each with `baseUrl`, `api`, `apiKey`, `headers` and `models`):
//! the user's, then each installed package's. A file may also name a program in
//! `providersCommand`, whose output, `{"version": 1, "providers": …}`, adds more providers, as a
//! package lists models it learns at run time.

use super::ModelOption;
use super::client::Protocol;
use super::pricing::Prices;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The version of `providersCommand`'s output read here.
const COMMAND_VERSION: u64 = 1;
/// The program runs before the first prompt and each time the models are listed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_OUTPUT_BYTES: u64 = 16 << 20;
/// Scramjet's levels, each with the name requests give it, and whether a model that reasons has
/// it unless its `thinkingLevelMap` says otherwise.
const LEVELS: [(&str, &str, bool); 7] = [
    ("off", "none", true),
    ("minimal", "minimal", true),
    ("low", "low", true),
    ("medium", "medium", true),
    ("high", "high", true),
    ("xhigh", "xhigh", false),
    ("max", "max", false),
];

/// Where a model's requests go, and with which headers.
#[derive(Clone, PartialEq)]
pub(super) struct Route {
    /// The provider's name.
    pub provider: String,
    pub host: String,
    pub port: u16,
    /// The base URL's host and port as written, for the `Host` header.
    pub authority: String,
    /// The base URL's path without a trailing `/`: requests go to `{path}/responses` or
    /// `{path}/v1/messages`, where Scramjet's clients send them.
    pub path: String,
    /// Authentication and the entries' `headers`.
    pub headers: Vec<(String, String)>,
}

/// Leaves the headers' values out: they hold keys.
impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Route")
            .field("provider", &self.provider)
            .field("authority", &self.authority)
            .field("path", &self.path)
            .field("headers", &names)
            .finish()
    }
}

/// What the agent needs to know about a model to send it requests.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelInfo {
    pub protocol: Protocol,
    /// The context window (input and output tokens).
    pub window: Option<u64>,
    /// The most output tokens a response may have.
    pub output: Option<u64>,
    pub reasoning: bool,
    /// The effort levels it takes, as requests name them, in order; empty: any.
    pub levels: Vec<String>,
    pub images: bool,
    pub prices: Prices,
    /// `None` when no provider offers it.
    pub route: Option<Route>,
}

impl ModelInfo {
    /// What the model's name says, for a model no provider offers.
    pub(super) fn guess(model: &str) -> Self {
        Self {
            protocol: match model.starts_with("claude-") {
                true => Protocol::Messages,
                false => Protocol::Responses,
            },
            window: None,
            output: None,
            reasoning: true,
            levels: Vec::new(),
            images: true,
            prices: Prices::default(),
            route: None,
        }
    }

    /// `effort` fitted to the model's levels (`fit`).
    pub(super) fn fit<'a>(&'a self, effort: &'a str) -> &'a str {
        fit(&self.levels, effort)
    }
}

/// `effort`, or when `levels` has others but not it, the nearest higher level among them, else
/// the nearest lower one, as Scramjet clamps a thinking level. An effort not in Scramjet's order
/// is left for the provider to judge.
pub(super) fn fit<'a>(levels: &[String], effort: &'a str) -> &'a str {
    const ORDER: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
    let listed = |level: &str| levels.iter().any(|listed| listed == level);
    match ORDER.iter().position(|level| *level == effort) {
        Some(wanted) if !levels.is_empty() && !listed(effort) => ORDER[wanted..]
            .iter()
            .chain(ORDER[..wanted].iter().rev())
            .copied()
            .find(|level| listed(level))
            .unwrap_or(effort),
        _ => effort,
    }
}

/// A model a provider offers.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Model {
    pub id: String,
    pub name: Option<String>,
    pub provider: String,
    pub info: ModelInfo,
}

/// The providers' models, each once, in the order of their files, of the providers in each and
/// of each provider's `models`; with what was wrong in the files.
#[derive(Clone, Debug, Default)]
pub struct Providers {
    files: Vec<PathBuf>,
    /// A document given in place of the files.
    #[cfg(test)]
    given: Option<Value>,
    models: Vec<Model>,
    problems: Vec<String>,
}

impl Providers {
    /// The providers of the `models.json` files `files` that exist, in their order.
    pub fn read(files: Vec<PathBuf>) -> Self {
        let mut providers = Self {
            files,
            ..Self::default()
        };
        providers.reload();
        providers
    }

    /// The providers `document` gives in a `models.json`'s format.
    #[cfg(test)]
    pub(crate) fn given(document: Value) -> Self {
        let mut providers = Self {
            given: Some(document),
            ..Self::default()
        };
        providers.reload();
        providers
    }

    /// Reads the files again and runs their programs again.
    pub(super) fn reload(&mut self) {
        let mut reading = Reading::default();
        for file in &self.files {
            reading.file(file);
        }
        #[cfg(test)]
        if let Some(document) = &self.given {
            let text = document.to_string().into_bytes();
            reading.document(&text, Path::new("."), "models");
        }
        self.models = reading.models;
        self.problems = reading.problems;
    }

    /// The `models.json` files read, as given.
    pub fn files(&self) -> Vec<&Path> {
        self.files.iter().map(PathBuf::as_path).collect()
    }

    /// What was wrong in the files or their programs' output, each in one line.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub(super) fn models(&self) -> &[Model] {
        &self.models
    }

    pub(super) fn find(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.id == id)
    }

    /// What a model `id` that no provider offers is sent as.
    pub(super) fn info(&self, id: &str) -> ModelInfo {
        self.find(id)
            .map_or_else(|| ModelInfo::guess(id), |model| model.info.clone())
    }

    /// The models, as `/model` and `--list-models` show them.
    pub fn options(&self) -> Vec<ModelOption> {
        (self.models.iter())
            .map(|model| {
                let provider = Some(model.provider.clone());
                ModelOption::new(
                    model.id.clone(),
                    model.name.clone(),
                    provider,
                    &model.info,
                    None,
                )
            })
            .collect()
    }
}

/// A `models.json`, or a program's output in its format.
#[derive(Deserialize)]
struct Document {
    providers: Option<Ordered>,
    #[serde(rename = "providersCommand")]
    command: Option<Value>,
    version: Option<Value>,
}

/// An object's entries in the order written, which `Value`'s map does not keep, as Scramjet
/// keeps the providers in their order.
struct Ordered(Vec<(String, Value)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> serde::de::Visitor<'de> for Entries {
            type Value = Ordered;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Ordered, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Ordered(entries))
            }
        }
        deserializer.deserialize_map(Entries)
    }
}

#[derive(Default)]
struct Reading {
    models: Vec<Model>,
    problems: Vec<String>,
}

impl Reading {
    fn file(&mut self, path: &Path) {
        let shown = path.display().to_string();
        let text = match std::fs::read(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => return self.problems.push(format!("{shown}: {error}")),
        };
        let directory = path.parent().unwrap_or(Path::new("."));
        self.document(&text, directory, &shown);
    }

    /// A `models.json`'s `providers`, then those its `providersCommand` lists.
    fn document(&mut self, text: &[u8], directory: &Path, what: &str) {
        let document = match serde_json::from_slice::<Document>(text) {
            Ok(document) => document,
            Err(error) => return self.problems.push(format!("{what}: {error}")),
        };
        self.providers(document.providers, what);
        match document.command {
            None => {}
            Some(Value::Array(words))
                if !words.is_empty() && words.iter().all(Value::is_string) =>
            {
                let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
                let program = format!("{what}: {}", words[0]);
                let ran = run(&words, directory).map_err(|problem| format!("{what}: {problem}"));
                match ran.and_then(|output| listed(&output, &program)) {
                    Ok(providers) => self.providers(providers, &program),
                    Err(problem) => self.problems.push(problem),
                }
            }
            Some(_) => self.problems.push(format!(
                "{what}: providersCommand must be a program and its arguments, as a list of strings"
            )),
        }
    }

    fn providers(&mut self, providers: Option<Ordered>, what: &str) {
        for (name, entry) in providers.into_iter().flat_map(|ordered| ordered.0) {
            self.provider(&name, &entry, &format!("{what}: provider {name}"));
        }
    }

    fn provider(&mut self, name: &str, entry: &Value, what: &str) {
        let Some(entry) = entry.as_object() else {
            return self.problems.push(format!("{what}: not a JSON object"));
        };
        let models = match entry.get("models") {
            Some(Value::Array(models)) if !models.is_empty() => models,
            Some(Value::Array(_)) | None => {
                return self.problems.push(format!(
                    "{what}: lists no models (scramjet-rs has no built-in ones)"
                ));
            }
            Some(_) => return self.problems.push(format!("{what}: models must be a list")),
        };
        for model in models {
            match self.model(name, entry, model) {
                Ok(model) if let Some(first) = self.models.iter().find(|m| m.id == model.id) => {
                    self.problems.push(format!(
                        "{what}: model {} is {}'s already; not offered here",
                        model.id, first.provider
                    ));
                }
                Ok(model) => self.models.push(model),
                Err(problem) => self.problems.push(format!("{what}: {problem}")),
            }
        }
    }

    /// A model entry of `provider`, whose fields it falls back to.
    fn model(
        &self,
        name: &str,
        provider: &Map<String, Value>,
        entry: &Value,
    ) -> Result<Model, String> {
        let id = entry["id"].as_str().filter(|id| !id.trim().is_empty());
        let id = id.ok_or("a model has no id")?.to_owned();
        let field = |key: &str| entry.get(key).or_else(|| provider.get(key));
        let text = |key: &str| match field(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.as_str())),
            Some(_) => Err(format!("model {id}: {key} must be a string")),
        };
        let protocol = match text("api")? {
            Some("openai-responses") => Protocol::Responses,
            Some("anthropic-messages") => Protocol::Messages,
            Some(api) => return Err(format!("model {id}: api {api} is not supported yet")),
            None => return Err(format!("model {id}: no api")),
        };
        let url = text("baseUrl")?.ok_or_else(|| format!("model {id}: no baseUrl"))?;
        let (host, port, authority, path) =
            base_url(url).map_err(|problem| format!("model {id}: baseUrl {problem}"))?;
        let mut headers = Vec::new();
        if let Some(key) = text("apiKey")? {
            let key = resolve(key).map_err(|problem| format!("model {id}: apiKey {problem}"))?;
            if protocol == Protocol::Messages {
                headers.push(("x-api-key".to_owned(), key.clone()));
            }
            if protocol == Protocol::Responses || field("authHeader") == Some(&Value::Bool(true)) {
                headers.push(("authorization".to_owned(), format!("Bearer {key}")));
            }
        }
        for given in [provider.get("headers"), entry.get("headers")]
            .into_iter()
            .flatten()
        {
            let given = given.as_object();
            let given = given.ok_or_else(|| format!("model {id}: headers must be an object"))?;
            for (header, value) in given {
                let value = value.as_str().ok_or_else(|| {
                    format!("model {id}: the header {header}'s value must be a string")
                })?;
                let value = resolve(value)
                    .map_err(|problem| format!("model {id}: the header {header} {problem}"))?;
                let header = header.to_ascii_lowercase();
                headers.retain(|(name, _)| *name != header);
                headers.push((header, value));
            }
        }
        for (header, value) in &headers {
            if hyper::header::HeaderName::from_bytes(header.as_bytes()).is_err()
                || hyper::header::HeaderValue::from_str(value).is_err()
            {
                return Err(format!(
                    "model {id}: the header {header} is not valid in HTTP"
                ));
            }
        }
        let reasoning = entry["reasoning"].as_bool().unwrap_or(false);
        let levels = match reasoning {
            true => levels(&entry["thinkingLevelMap"]),
            false => Vec::new(),
        };
        let input = entry["input"].as_array();
        let info = ModelInfo {
            protocol,
            window: entry["contextWindow"]
                .as_f64()
                .filter(|w| *w >= 1.0)
                .map(|w| w as u64),
            output: entry["maxTokens"].as_u64().filter(|tokens| *tokens > 0),
            reasoning,
            levels,
            images: input.is_some_and(|input| input.iter().any(|kind| kind == "image")),
            prices: Prices::of(&entry["cost"]),
            route: Some(Route {
                provider: name.to_owned(),
                host,
                port,
                authority,
                path,
                headers,
            }),
        };
        Ok(Model {
            id,
            name: entry["name"].as_str().map(str::to_owned),
            provider: name.to_owned(),
            info,
        })
    }
}

/// The levels a model that reasons takes, as Scramjet reads its `thinkingLevelMap`: a level the
/// map gives a text is sent as that text, one it gives `null` is not taken, and those it leaves
/// out are taken but `xhigh` and `max`.
fn levels(map: &Value) -> Vec<String> {
    (LEVELS.iter())
        .filter_map(|(level, sent, ordinary)| match map.get(level) {
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => None,
            None => ordinary.then(|| (*sent).to_owned()),
        })
        .collect()
}

/// A configured value, as Scramjet resolves it: the environment variable of that name when one
/// is set, else the text itself.
fn resolve(value: &str) -> Result<String, String> {
    if value.starts_with('!') {
        return Err("is a shell command (`!…`), which is not run yet".to_owned());
    }
    match std::env::var(value) {
        Ok(set) if !set.is_empty() => Ok(set),
        _ => Ok(value.to_owned()),
    }
}

/// An `http://` URL's host, port (80 by default), authority and path without a trailing `/`.
fn base_url(url: &str) -> Result<(String, u16, String, String), String> {
    let scheme = |prefix: &str| {
        url.get(..prefix.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
    };
    if scheme("https://") {
        return Err(format!("{url}: HTTPS is not supported yet"));
    }
    if !scheme("http://") {
        return Err(format!("{url}: not an http:// URL"));
    }
    let rest = &url["http://".len()..];
    if rest
        .chars()
        .any(|c| matches!(c, '?' | '#' | '@' | '\\') || c.is_whitespace() || c.is_control())
    {
        return Err(format!(
            "{url}: must have no credentials, query or fragment"
        ));
    }
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']').ok_or(format!("{url}: no host"))?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let port = match port {
        None => 80,
        Some(port) => port
            .parse()
            .map_err(|_| format!("{url}: the port is not a number"))?,
    };
    if host.is_empty() {
        return Err(format!("{url}: no host"));
    }
    let path = path.trim_end_matches('/').to_owned();
    Ok((host.to_owned(), port, authority.to_owned(), path))
}

/// The providers a program's `output` lists, in the version read here; `what` names it.
fn listed(output: &[u8], what: &str) -> Result<Option<Ordered>, String> {
    let output = serde_json::from_slice::<Document>(output)
        .map_err(|error| format!("{what}: its output is not in models.json's format: {error}"))?;
    match output.version.as_ref().and_then(Value::as_u64) {
        Some(COMMAND_VERSION) => Ok(output.providers),
        Some(version) => Err(format!(
            "{what} speaks version {version}, not {COMMAND_VERSION}"
        )),
        None => Err(format!("{what}: its output has no version")),
    }
}

/// The output of the program `words` names with its arguments, run in `directory`: `~/` starts a
/// path in the home directory, and a path with a `/` is from `directory`.
fn run(words: &[&str], directory: &Path) -> Result<Vec<u8>, String> {
    let program = words[0];
    let path = match program.strip_prefix("~/").zip(std::env::home_dir()) {
        Some((rest, home)) => home.join(rest),
        None if program.contains('/') => directory.join(program),
        None => PathBuf::from(program),
    };
    let mut child = Command::new(&path)
        .args(&words[1..])
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(COMMAND_OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stderr).take(64 << 10).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let seconds = COMMAND_TIMEOUT.as_secs();
                return Err(format!("{program} gave no answer within {seconds}s"));
            }
            Err(error) => return Err(format!("{program}: {error}")),
        }
    };
    let errors = errors.join().unwrap_or_default();
    let said = String::from_utf8_lossy(&errors);
    let said = said.lines().map(str::trim).find(|line| !line.is_empty());
    let said = said.map(|line| format!(": {line}")).unwrap_or_default();
    if !status.success() {
        return Err(format!("{program} failed ({status}){said}"));
    }
    let output = output.join().ok().and_then(Result::ok).unwrap_or_default();
    if output.len() as u64 > COMMAND_OUTPUT_BYTES {
        return Err(format!(
            "{program}: its output is over {} MB",
            COMMAND_OUTPUT_BYTES >> 20
        ));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;
    use serde_json::json;

    fn route(models: &Providers, id: &str) -> Route {
        models.find(id).unwrap().info.route.clone().unwrap()
    }

    #[test]
    fn entries_say_where_requests_go_and_what_each_model_takes() {
        let providers = Providers::given(json!({"providers": {
            "local": {
                "baseUrl": "http://127.0.0.1:18080/v1/",
                "api": "openai-responses",
                "apiKey": "literal-key",
                "headers": {"X-Team": "core"},
                "models": [
                    {"id": "luna", "name": "Luna", "reasoning": true, "input": ["text", "image"],
                        "contextWindow": 1_050_000, "maxTokens": 128_000,
                        "thinkingLevelMap": {"minimal": null, "xhigh": "xhigh"}},
                    {"id": "claude", "api": "anthropic-messages", "baseUrl": "http://[::1]:9",
                        "authHeader": true, "headers": {"x-team": "messages"}},
                    {"id": "plain"},
                ],
            },
        }}));
        assert_eq!(providers.problems(), [] as [String; 0]);
        let luna = providers.find("luna").unwrap();
        assert_eq!(
            (luna.name.as_deref(), luna.provider.as_str()),
            (Some("Luna"), "local")
        );
        let info = &luna.info;
        assert_eq!(info.protocol, Protocol::Responses);
        assert_eq!((info.window, info.output), (Some(1_050_000), Some(128_000)));
        assert!(info.reasoning && info.images);
        assert_eq!(info.levels, ["none", "low", "medium", "high", "xhigh"]);
        let local = route(&providers, "luna");
        assert_eq!((local.host.as_str(), local.port), ("127.0.0.1", 18080));
        assert_eq!(
            (local.authority.as_str(), local.path.as_str()),
            ("127.0.0.1:18080", "/v1")
        );
        let headers = [("authorization", "Bearer literal-key"), ("x-team", "core")];
        let headers = headers.map(|(name, value)| (name.to_owned(), value.to_owned()));
        assert_eq!(local.headers, headers);
        // A model's own fields come first; Messages has its key in x-api-key.
        let claude = route(&providers, "claude");
        assert_eq!(
            providers.find("claude").unwrap().info.protocol,
            Protocol::Messages
        );
        assert_eq!(
            (claude.host.as_str(), claude.port, claude.path.as_str()),
            ("::1", 9, "")
        );
        let names: Vec<&str> = claude
            .headers
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["x-api-key", "authorization", "x-team"]);
        assert_eq!(claude.headers[2].1, "messages");
        // Scramjet's defaults: no reasoning, text only, nothing known of its limits or prices.
        let plain = &providers.find("plain").unwrap().info;
        assert!(!plain.reasoning && !plain.images && plain.levels.is_empty());
        assert_eq!(
            (plain.window, plain.output, plain.prices.header()),
            (None, None, None)
        );
        // Keys are not shown.
        assert!(!format!("{:?}", providers.models()).contains("literal-key"));
        assert_eq!(providers.options()[0].provider.as_deref(), Some("local"));
        assert_eq!(providers.info("unknown"), ModelInfo::guess("unknown"));
    }

    #[test]
    fn keys_and_headers_are_read_from_the_variables_they_name() {
        // SAFETY: no other test reads or writes this variable.
        unsafe { std::env::set_var("SCRAMJET_RS_PROVIDERS_TEST_KEY", "from-the-environment") };
        let providers = Providers::given(json!({"providers": {"p": {
            "baseUrl": "http://localhost", "api": "anthropic-messages",
            "apiKey": "SCRAMJET_RS_PROVIDERS_TEST_KEY", "models": [{"id": "m"}],
        }}}));
        let route = route(&providers, "m");
        assert_eq!((route.port, route.authority.as_str()), (80, "localhost"));
        let key = ("x-api-key".to_owned(), "from-the-environment".to_owned());
        assert_eq!(route.headers, [key]);
    }

    #[test]
    fn what_is_wrong_is_said_and_the_rest_is_offered() {
        let providers = Providers::given(json!({
            "providers": {
                "a": {"baseUrl": "http://a/v1", "api": "openai-responses", "models": [
                    {"id": "m"}, {"name": "no id"}, {"id": "chat", "api": "openai-completions"},
                    {"id": "secure", "baseUrl": "https://a/v1"},
                    {"id": "shell", "apiKey": "!pass show key"},
                    {"id": "bad", "headers": {"x-bad": "line\nbreak"}},
                ]},
                "b": {"baseUrl": "http://b/v1", "api": "openai-responses",
                    "models": [{"id": "m"}, {"id": "n"}]},
                "c": {"baseUrl": "http://c"},
            },
            "providersCommand": "not a list",
        }));
        let ids: Vec<&str> = providers.models().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m", "n"]);
        assert_eq!(providers.find("m").unwrap().provider, "a");
        assert_eq!(
            providers.problems(),
            [
                "models: provider a: a model has no id",
                "models: provider a: model chat: api openai-completions is not supported yet",
                "models: provider a: model secure: baseUrl https://a/v1: HTTPS is not supported yet",
                "models: provider a: model shell: apiKey is a shell command (`!…`), which is not \
                 run yet",
                "models: provider a: model bad: the header x-bad is not valid in HTTP",
                "models: provider b: model m is a's already; not offered here",
                "models: provider c: lists no models (scramjet-rs has no built-in ones)",
                "models: providersCommand must be a program and its arguments, as a list of strings",
            ]
        );
    }

    #[test]
    fn a_program_lists_more_providers_and_its_failures_are_said() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-rs-providers-{}", std::process::id())),
        );
        let script = |name: &str, body: &str| {
            let path = scratch.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
            std::fs::set_permissions(&path, mode).unwrap();
        };
        let listed = json!({"version": 1, "providers": {"served": {
            "baseUrl": "http://127.0.0.1:1/v1", "api": "openai-responses",
            "models": [{"id": "from-program"}],
        }}});
        script(
            "list",
            &format!(
                "[ \"$1\" = arg ] && [ \"$PWD\" = \"{}\" ] && echo '{listed}'",
                scratch.0.display()
            ),
        );
        script("fail", "echo 'cannot reach the server' >&2; exit 3");
        script("old", "echo '{\"version\": 2}'");
        script("slow", "exec sleep 10");
        let file = scratch.0.join("models.json");
        let write = |document: Value| std::fs::write(&file, document.to_string()).unwrap();
        write(json!({
            "providers": {"own": {"baseUrl": "http://x", "api": "openai-responses",
                "models": [{"id": "own"}]}},
            "providersCommand": ["./list", "arg"],
        }));
        let mut providers = Providers::read(vec![file.clone(), scratch.0.join("absent.json")]);
        let ids: Vec<&str> = providers.models().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            (ids, providers.problems()),
            (vec!["own", "from-program"], &[] as &[String])
        );
        assert_eq!(route(&providers, "from-program").provider, "served");
        let shown = file.display();
        for (program, problem) in [
            (
                "./fail",
                "./fail failed (exit status: 3): cannot reach the server".to_owned(),
            ),
            ("./old", "./old speaks version 2, not 1".to_owned()),
            (
                "./absent",
                "cannot run ./absent: No such file or directory (os error 2)".to_owned(),
            ),
        ] {
            write(json!({"providersCommand": [program]}));
            providers.reload();
            assert_eq!(providers.problems(), [format!("{shown}: {problem}")]);
            assert!(providers.models().is_empty());
        }
        write(json!({"providersCommand": ["./slow"]}));
        let started = Instant::now();
        providers.reload();
        assert!(started.elapsed() < COMMAND_TIMEOUT + Duration::from_secs(1));
        assert_eq!(
            providers.problems(),
            [format!("{shown}: ./slow gave no answer within 2s")]
        );
        std::fs::write(&file, "{").unwrap();
        providers.reload();
        assert!(providers.problems()[0].starts_with(&format!("{shown}: EOF while parsing")));
        assert_eq!(
            providers.files(),
            [file.as_path(), &scratch.0.join("absent.json")]
        );
    }

    #[test]
    fn providers_keep_the_order_written() {
        let scratch = ScratchDir::new(std::env::temp_dir().join(format!(
            "scramjet-rs-providers-order-{}",
            std::process::id()
        )));
        let file = scratch.0.join("models.json");
        let provider = |id: &str| {
            format!(
                r#"{{"baseUrl": "http://h", "api": "openai-responses", "models": [{{"id": "{id}"}}]}}"#
            )
        };
        let text = format!(
            r#"{{"providers": {{"zeta": {}, "alpha": {}}}}}"#,
            provider("z"),
            provider("a")
        );
        std::fs::write(&file, text).unwrap();
        let providers = Providers::read(vec![file]);
        let ids: Vec<&str> = providers.models().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["z", "a"]);
    }

    #[test]
    fn only_plain_http_urls_with_a_host_are_taken() {
        let parsed = |url: &str| base_url(url).map(|(host, port, _, path)| (host, port, path));
        assert_eq!(
            parsed("HTTP://h:8/a/b/"),
            Ok(("h".into(), 8, "/a/b".into()))
        );
        for url in [
            "http://",
            "http://:80",
            "http://u:p@h",
            "http://h?q",
            "http://h:x",
            "ftp://h",
        ] {
            assert!(parsed(url).is_err(), "{url}");
        }
    }
}
