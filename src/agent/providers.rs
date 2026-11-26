//! The providers requests go to and what each says of its models, from `models.json` files in
//! Scramjet's format (`providers`, each with `baseUrl`, `api`, `apiKey`, `headers` and `models`):
//! the user's, then each installed package's. A file may also name a program in
//! `providersCommand`, whose output, `{"version": 1, "providers": …}`, adds more providers, as a
//! package lists models it learns at run time, and in `credentialFiles` the files and directories
//! holding its providers' credentials, which the agent's tools do not see (`sandbox::Hidden`).
//! A file's `logins` name the programs that log in to more providers (`login::LoginProgram`).
//! A provider logged in to with `/login` (`login::LOGINS`, or a program) adds the models its
//! login listed, unless a file names it; a provider of that name without an `apiKey` takes the
//! login's key.
//! An API key `/login` saved for a provider the files name is sent in place of its `apiKey`.
//! A provider whose entry lists no models offers those its server lists (`listed`).
//! The model catalogs the settings list (`catalog`) add their providers after the files', unless
//! a file names one.
//! Providers may offer the same id: a model is named by its reference, `provider/id`.

use super::ModelOption;
use super::catalog::Catalog;
use super::client::Protocol;
use super::completions::Compat;
use super::configured::{self, Commanded, Configured};
use super::listed::{self, Listing};
use super::login::{LoginProgram, Logins};
use super::pricing::Prices;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    cell::RefCell,
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// The version of `providersCommand`'s output read here.
const COMMAND_VERSION: u64 = 1;
/// The program runs before the first prompt and each time the models are listed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const COMMAND_OUTPUT_BYTES: u64 = 16 << 20;
/// Scramjet's levels, each with the name efforts give it (`none` is off), and whether a model
/// that reasons has it unless its `thinkingLevelMap` says otherwise.
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
    /// `https://`: the connection is TLS, verified against the system's trusted certificates.
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// The base URL's host and port as written, for the `Host` header.
    pub authority: String,
    /// The base URL's path without a trailing `/`: requests go to `{path}/responses` or
    /// `{path}/v1/messages` or `{path}/chat/completions`, where Scramjet's clients send them.
    pub path: String,
    /// Authentication and the entries' `headers`.
    pub headers: Vec<(String, String)>,
    /// Those of them whose values a command prints at each request (`!command`).
    pub commanded: Vec<Commanded>,
    /// The login whose token each request carries, fresh, besides `headers`.
    pub login: Option<Login>,
}

/// A provider's login (`login::LOGINS`, or a package's `program`), saved in `file`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Login {
    pub provider: String,
    pub file: PathBuf,
    pub program: Option<LoginProgram>,
}

/// Leaves the headers' values out: they hold keys.
impl std::fmt::Debug for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        let commanded: Vec<&str> = (self.commanded.iter())
            .map(|commanded| commanded.header.as_str())
            .collect();
        f.debug_struct("Route")
            .field("provider", &self.provider)
            .field("tls", &self.tls)
            .field("authority", &self.authority)
            .field("path", &self.path)
            .field("headers", &names)
            .field("commanded", &commanded)
            .field("login", &self.login)
            .finish()
    }
}

/// What the agent needs to know about a model to send it requests.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelInfo {
    /// The id requests name it by.
    pub id: String,
    pub protocol: Protocol,
    /// The context window (input and output tokens).
    pub window: Option<u64>,
    /// The most output tokens a response may have.
    pub output: Option<u64>,
    /// The most input tokens a request may have, when less than the window leaves
    /// (`maxInputTokens`): compaction runs past it too.
    pub input: Option<u64>,
    pub reasoning: bool,
    /// The effort levels it takes, in order; empty: any.
    pub levels: Vec<String>,
    /// Levels its `thinkingLevelMap` sends as another text, with that text.
    pub renamed: Vec<(String, String)>,
    pub images: bool,
    pub prices: Prices,
    /// What its server accepts, for Chat Completions (and Responses' cache retention).
    pub compat: Compat,
    /// `None` when no provider offers it.
    pub route: Option<Route>,
    /// The local runtime serving it, as `/login` → Local models saved its provider.
    pub runtime: Option<super::local::Runtime>,
}

impl ModelInfo {
    /// What the model's name says, for a model no provider offers.
    pub(super) fn guess(model: &str) -> Self {
        Self {
            id: model.to_owned(),
            protocol: match model.starts_with("claude-") {
                true => Protocol::Messages,
                false => Protocol::Responses,
            },
            window: None,
            output: None,
            input: None,
            reasoning: true,
            levels: Vec::new(),
            renamed: Vec::new(),
            images: true,
            prices: Prices::default(),
            compat: Compat::default(),
            route: None,
            runtime: None,
        }
    }

    /// `effort` fitted to the model's levels (`fit`).
    pub(super) fn fit<'a>(&'a self, effort: &'a str) -> &'a str {
        fit(&self.levels, effort)
    }

    /// The text requests send for `effort` fitted (`fit`): its `thinkingLevelMap`'s, if it
    /// renames that level.
    pub(super) fn sent<'a>(&'a self, effort: &'a str) -> &'a str {
        let level = self.fit(effort);
        (self.renamed.iter())
            .find(|(renamed, _)| renamed == level)
            .map_or(level, |(_, text)| text)
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

impl Model {
    /// `provider/id`, as the agent, its sessions and settings name the model.
    pub(super) fn reference(&self) -> String {
        reference(&self.provider, &self.id)
    }
}

pub(super) fn reference(provider: &str, id: &str) -> String {
    format!("{provider}/{id}")
}

/// Whether `id` ends in a date, as `claude-haiku-4-5-20251001` does, rather than being an alias.
fn dated(id: &str) -> bool {
    let tail = id.rsplit('-').next().unwrap_or("");
    !id.ends_with("-latest") && tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit())
}

/// Where the key requests to a provider carry comes from, as `/login` shows it.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyFrom {
    /// `/login` saved it.
    Saved,
    /// The variable its entry's `apiKey` names.
    Variable(String),
    /// Its entry's `apiKey` text.
    Entry,
    /// `--api-key`.
    Given,
    /// Its requests carry none; a built-in provider is then not offered.
    Nowhere,
}

/// A provider built in, which `/login` takes an API key for: offered once a key is saved for it,
/// or `--api-key` gives one, unless a file or catalog names a provider of its name. Its models
/// are those its server lists (`listed`). An environment variable alone does not offer it: one
/// set for another program, with that program's own base URL, would send its key here.
#[derive(Debug)]
pub struct Builtin {
    pub name: &'static str,
    /// What the user knows it by.
    pub shown: &'static str,
    /// The start of the id of the model a new session takes of it, the first its list offers.
    pub default: &'static str,
    base_url: &'static str,
    api: &'static str,
    /// The provider of models.dev's catalog (`MODELS_DEV`) that describes the models its list
    /// names, for a list that says nothing else of them, embedders and voices among them: those
    /// it names that the catalog describes as taking tools are offered.
    described: Option<&'static str>,
}

/// The model catalog that describes the models of a built-in provider whose list names them only.
pub const MODELS_DEV: &str = "https://models.dev/api.json";

/// The built-in providers, under Scramjet's names for them, in `/login`'s order.
pub const BUILTIN: [Builtin; 4] = [
    Builtin {
        name: "anthropic",
        shown: "Anthropic",
        default: "claude-sonnet-",
        base_url: "https://api.anthropic.com",
        api: "anthropic-messages",
        described: None,
    },
    Builtin {
        name: "google",
        shown: "Google Gemini",
        default: "gemini-",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        api: "openai-completions",
        described: Some("google"),
    },
    Builtin {
        name: "openai",
        shown: "OpenAI",
        default: "gpt-",
        base_url: "https://api.openai.com/v1",
        api: "openai-responses",
        described: Some("openai"),
    },
    Builtin {
        name: "openrouter",
        shown: "OpenRouter",
        default: "anthropic/claude-sonnet-",
        base_url: "https://openrouter.ai/api/v1",
        api: "openai-completions",
        described: None,
    },
];

/// The built-in provider called `name`.
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    BUILTIN.iter().find(|builtin| builtin.name == name)
}

/// Where providers come from: `models.json` files, then model catalogs' URLs.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub files: Vec<PathBuf>,
    pub catalogs: Vec<String>,
}

impl From<Vec<PathBuf>> for Sources {
    fn from(files: Vec<PathBuf>) -> Self {
        Self {
            files,
            catalogs: Vec::new(),
        }
    }
}

/// The providers' models, each once, in the order of their files, of the providers in each and
/// of each provider's `models`; with what was wrong in the files.
#[derive(Clone, Debug, Default)]
pub struct Providers {
    files: Vec<PathBuf>,
    /// The model catalogs' URLs.
    catalog_urls: Vec<String>,
    /// The catalogs, as last read.
    catalogs: Vec<Catalog>,
    /// The catalogs asked for since these providers were made, by URL, for those not saved.
    tried_catalogs: HashMap<String, Result<String, String>>,
    /// A document given in place of the files.
    #[cfg(test)]
    given: Option<Value>,
    models: Vec<Model>,
    /// The providers the files name that `/login` may save a key for, each once, in order.
    keyed: Vec<(String, KeyFrom)>,
    /// `origins`, of each of `keyed`.
    origins: HashMap<String, Vec<String>>,
    /// Where each provider came from, as its problems name it.
    sources: HashMap<String, String>,
    /// The files' programs that ran and listed no providers, as their problems would name them.
    listed_none: Vec<String>,
    credential_files: Vec<PathBuf>,
    problems: Vec<String>,
    /// Where `/login` saves what it does.
    logins: Option<Logins>,
    /// The providers that list their models themselves.
    listings: Vec<Listing>,
    /// The lists asked for since these providers were made, by `Listing::key`, for those not
    /// saved: one that could not be read is not asked again at each reading.
    tried: HashMap<String, Result<Vec<Value>, String>>,
    /// The logins the files' programs do.
    programs: Vec<LoginProgram>,
    /// `--api-key`: a provider and the key its requests carry for the run.
    given_key: Option<(String, String)>,
}

impl Providers {
    /// The providers of the `models.json` files `sources` names that exist, in their order, then
    /// its catalogs', then those logged in to, as `logins` saved them.
    pub fn read(sources: impl Into<Sources>, logins: Option<Logins>) -> Self {
        let Sources { files, catalogs } = sources.into();
        let mut providers = Self {
            files,
            catalog_urls: catalogs,
            logins,
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

    /// As `given`, with what `logins` saved.
    #[cfg(test)]
    pub(crate) fn given_logged_in(document: Value, logins: Logins) -> Self {
        let mut providers = Self {
            given: Some(document),
            logins: Some(logins),
            ..Self::default()
        };
        providers.reload();
        providers
    }

    /// Reads the files again and runs their programs again.
    pub(super) fn reload(&mut self) {
        let mut reading = Reading {
            logins: self.logins.clone(),
            given_key: self.given_key.clone(),
            tried: std::mem::take(&mut self.tried),
            tried_catalogs: std::mem::take(&mut self.tried_catalogs),
            ..Reading::default()
        };
        if let Some(logins) = &self.logins {
            match super::login::saved(&logins.auth) {
                Ok(saved) => reading.saved = saved,
                Err(problem) => reading.problems.push(problem),
            }
        }
        let mut documents: Vec<(Vec<u8>, &Path, String)> = Vec::new();
        for file in &self.files {
            let shown = file.display().to_string();
            match std::fs::read(file) {
                Ok(text) => documents.push((text, file.parent().unwrap_or(Path::new(".")), shown)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => reading.problems.push(format!("{shown}: {error}")),
            }
        }
        #[cfg(test)]
        if let Some(document) = &self.given {
            let text = document.to_string().into_bytes();
            documents.push((text, Path::new("."), "models".to_owned()));
        }
        // Every file's providers know the logins, whichever file names them.
        for (text, directory, what) in &documents {
            reading.declare(text, directory, what);
        }
        for (text, directory, what) in &documents {
            reading.document(text, directory, what);
        }
        for url in &self.catalog_urls {
            reading.catalog(url);
        }
        reading.builtins();
        reading.logins();
        self.models = reading.models;
        self.keyed = reading.keyed;
        self.origins = reading.origins;
        self.sources = reading.sources;
        self.listed_none = reading.listed_none;
        self.listings = reading.listings;
        self.tried = reading.tried;
        self.catalogs = reading.catalogs;
        self.tried_catalogs = reading.tried_catalogs;
        self.programs = reading.programs;
        let previous = std::mem::replace(&mut self.credential_files, reading.credential_files);
        self.problems = reading.problems;
        self.keep_credential_files(previous);
    }

    /// Sends `key` as it is, in place of any other, with `provider`'s requests from now on, and
    /// reads the files again for it.
    pub fn give_key(&mut self, provider: &str, key: &str) {
        self.given_key = Some((provider.to_owned(), key.to_owned()));
        self.reload();
    }

    /// The key `give_key` gave, kept by providers read again from other files.
    pub(super) fn keep_given_key(&mut self, previous: &Self) {
        if self.given_key.is_none() && previous.given_key.is_some() {
            self.given_key = previous.given_key.clone();
            self.reload();
        }
    }

    /// Keeps `previous`, the stores named before, hidden while a file has problems: once fixed,
    /// it may name them again.
    pub(super) fn keep_credential_files(&mut self, previous: Vec<PathBuf>) {
        if !self.problems.is_empty() {
            self.credential_files.extend(previous);
        }
        self.credential_files.sort();
        self.credential_files.dedup();
    }

    /// The files' `credentialFiles`, with `~` and relative paths resolved: what the agent's tools
    /// do not see, besides the common credential stores.
    pub(super) fn credential_files(&self) -> &[PathBuf] {
        &self.credential_files
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

    /// The providers that list their models themselves, as their entries list none.
    pub fn listings(&self) -> &[Listing] {
        &self.listings
    }

    /// The model catalogs, as last read.
    pub fn catalogs(&self) -> &[Catalog] {
        &self.catalogs
    }

    /// Forgets the list `provider` gave, so that it is asked again when the providers are next
    /// read, as after its key changed; a built-in one's too, which without a key had no listing.
    pub fn forget_list(&mut self, provider: &str) {
        for listing in self.listings.iter().filter(|l| l.provider() == provider) {
            listing.forget();
            self.tried.remove(&listing.key());
        }
        if let Some(logins) = &self.logins
            && self.keyed.iter().any(|(keyed, _)| keyed == provider)
        {
            let _ = std::fs::remove_file(logins.provided(provider));
        }
    }

    /// The providers the files name, but those `/login` logs in to, then the built-in ones no
    /// file names, with where their key comes from: those `/login` may save a key for.
    pub fn keyed(&self) -> &[(String, KeyFrom)] {
        &self.keyed
    }

    /// The servers (`scheme://host:port`) the entry of `provider`, one of `keyed`'s, sends
    /// requests to: those a key `/login` saves for it is sent to.
    pub fn origins(&self, provider: &str) -> &[String] {
        self.origins.get(provider).map_or(&[], Vec::as_slice)
    }

    /// The logins the files' programs do, in their order.
    pub fn login_programs(&self) -> &[LoginProgram] {
        &self.programs
    }

    /// Why the model `name`, whose requests go by `route`, cannot be asked until a sign-in: its
    /// provider's login has saved no credential, or none offers it and it names a provider
    /// `/login` signs in to, or no provider offers any model.
    pub(super) fn login_needed(&self, name: &str, route: Option<&Route>) -> Option<String> {
        let logins = self.logins.as_ref()?;
        let (provider, file) = match route {
            Some(route) => {
                let login = route.login.as_ref()?;
                (login.provider.as_str(), &login.file)
            }
            None if self.models.is_empty() => {
                return Some("no provider offers any model (/login)".to_owned());
            }
            None => {
                let (provider, _) = name.split_once('/')?;
                (provider, &logins.auth)
            }
        };
        let program = self.programs.iter().find(|p| p.provider == provider);
        if program.is_none() && !super::login::LOGINS.iter().any(|(p, _)| *p == provider) {
            return None;
        }
        let saved = super::login::saved(file).unwrap_or_default();
        if saved.get(provider).is_some_and(|c| c["type"] == "oauth") {
            return None;
        }
        let shown = program.map_or(super::login::shown_name(provider), |p| &p.name);
        Some(format!("not logged in to {shown} (/login)"))
    }

    /// The model `name` names: the one whose reference (`provider/id`) it is, else the first
    /// with that id, as sessions and settings written before references name it.
    /// The files' programs that ran and listed no providers: a server they ask may have moved.
    pub(super) fn listed_none(&self) -> &[String] {
        &self.listed_none
    }

    /// Where the provider of the model `name` names (`find`) came from: a file, its program, a
    /// catalog or a login.
    pub(super) fn source(&self, name: &str) -> Option<&str> {
        let model = self.find(name)?;
        self.sources.get(&model.provider).map(String::as_str)
    }

    pub(super) fn find(&self, name: &str) -> Option<&Model> {
        let models = &self.models;
        (models.iter())
            .find(|model| {
                (name.strip_prefix(model.provider.as_str()))
                    .and_then(|rest| rest.strip_prefix('/'))
                    .is_some_and(|id| id == model.id)
            })
            .or_else(|| models.iter().find(|model| model.id == name))
    }

    /// The reference of the model `name` names (`find`), if a provider offers it.
    pub fn offered(&self, name: &str) -> Option<String> {
        self.find(name).map(Model::reference)
    }

    /// The model an agent file's `model` names: the one `find` finds, else, as Claude Code's
    /// `sonnet`, `opus` and `haiku` name the newest of a family, the newest whose id contains
    /// `name` (an alias before a dated id), `provider`'s first.
    pub(super) fn family(&self, name: &str, provider: &str) -> Option<&Model> {
        if let Some(model) = self.find(name) {
            return Some(model);
        }
        let lower = name.to_lowercase();
        let newest = |of_provider: bool| {
            (self.models.iter())
                .filter(|model| !of_provider || model.provider == provider)
                .filter(|model| model.id.to_lowercase().contains(&lower))
                .max_by(|a, b| (!dated(&a.id), &a.id).cmp(&(!dated(&b.id), &b.id)))
        };
        newest(true).or_else(|| newest(false))
    }

    /// The servers on this machine the models' requests go to, each once: the provider, the
    /// host and the port.
    pub fn local_servers(&self) -> Vec<(String, String, u16)> {
        let mut servers: Vec<(String, String, u16)> = Vec::new();
        for route in self
            .models
            .iter()
            .filter_map(|model| model.info.route.as_ref())
        {
            let host = route.host.to_ascii_lowercase();
            let local = host == "localhost"
                || (host.trim_matches(['[', ']']).parse::<std::net::IpAddr>())
                    .is_ok_and(|ip| ip.is_loopback());
            let server = (route.provider.clone(), route.host.clone(), route.port);
            if local && !servers.contains(&server) {
                servers.push(server);
            }
        }
        servers
    }

    /// The reference of the model `name` names (`find`), else `name`.
    pub(super) fn reference(&self, name: &str) -> String {
        self.find(name)
            .map_or_else(|| name.to_owned(), Model::reference)
    }

    /// What the model `name` names is sent as; one no provider offers, as its name says.
    pub(super) fn info(&self, name: &str) -> ModelInfo {
        self.find(name)
            .map_or_else(|| ModelInfo::guess(name), |model| model.info.clone())
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
    /// Read from files only: a program that fails would leave the stores it names in view.
    #[serde(rename = "credentialFiles")]
    credential_files: Option<Value>,
    version: Option<Value>,
}

/// A `models.json`'s logins, read from files only, before their providers.
#[derive(Deserialize)]
struct Declared {
    logins: Option<Ordered>,
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
    credential_files: Vec<PathBuf>,
    problems: Vec<String>,
    logins: Option<Logins>,
    /// What `/login` saved.
    saved: Map<String, Value>,
    /// `Providers::given_key`.
    given_key: Option<(String, String)>,
    /// The providers the files named.
    named: Vec<String>,
    keyed: Vec<(String, KeyFrom)>,
    listings: Vec<Listing>,
    /// `Providers::tried`.
    tried: HashMap<String, Result<Vec<Value>, String>>,
    catalogs: Vec<Catalog>,
    /// `Providers::tried_catalogs`.
    tried_catalogs: HashMap<String, Result<String, String>>,
    programs: Vec<LoginProgram>,
    /// `Providers::origins`.
    origins: HashMap<String, Vec<String>>,
    /// `Providers::sources`.
    sources: HashMap<String, String>,
    /// `Providers::listed_none`.
    listed_none: Vec<String>,
    /// The servers each login's own entry names, as `login_origins` found them.
    logged_in_origins: RefCell<HashMap<String, Vec<String>>>,
}

impl Reading {
    /// The servers the login of provider `name` names itself, its token's only destinations.
    fn login_origins(&self, name: &str) -> Vec<String> {
        let mut found = self.logged_in_origins.borrow_mut();
        let named = found.entry(name.to_owned()).or_insert_with(|| {
            let Some(logins) = &self.logins else {
                return Vec::new();
            };
            let credential = self.saved.get(name).unwrap_or(&Value::Null);
            let listed = super::login::List::read(logins, name);
            let listed = listed.map_or(Value::Null, |listed| listed.0);
            (super::login::entry(name, credential, &listed).as_ref()).map_or_else(Vec::new, origins)
        });
        named.clone()
    }

    /// Whether `/login` logs in to provider `name`, itself or by a package's program.
    fn logs_in(&self, name: &str) -> bool {
        super::login::LOGINS.iter().any(|(login, _)| *login == name)
            || self.programs.iter().any(|program| program.provider == name)
    }

    /// The logins a `models.json` in `directory` names; problems name it `what`.
    fn declare(&mut self, text: &[u8], directory: &Path, what: &str) {
        // `document` says why.
        if unmet_engine(text).is_some() {
            return;
        }
        let logins = match serde_json::from_slice(text) {
            Ok(Declared {
                logins: Some(logins),
            }) => logins,
            Ok(Declared { logins: None }) => return,
            // One that is no models.json is said so as its providers are read.
            Err(_) if serde_json::from_slice::<Document>(text).is_err() => return,
            Err(_) => {
                return self
                    .problems
                    .push(format!("{what}: logins must be an object"));
            }
        };
        for (provider, entry) in logins.0 {
            let what = format!("{what}: login {provider}");
            let command = entry.get("command").and_then(Value::as_array);
            let command = command.filter(|words| {
                !words.is_empty()
                    && words
                        .iter()
                        .all(|word| word.as_str().is_some_and(|w| !w.is_empty()))
            });
            let name = match entry.get("name") {
                None => Some(provider.as_str()),
                Some(name) => name.as_str().filter(|name| !name.trim().is_empty()),
            };
            let problem = match (command, name) {
                _ if provider.trim().is_empty() => "has no provider name",
                _ if self.logs_in(&provider) => "is logged in to already; not offered here",
                (None, _) => "command must be a program and its arguments, as a list of strings",
                (_, None) => "name must be a text",
                (Some(command), Some(name)) => {
                    let command = command.iter().filter_map(Value::as_str);
                    let command = command.map(str::to_owned).collect();
                    let directory = directory.to_path_buf();
                    let name = name.to_owned();
                    let program = LoginProgram::new(provider, name, command, directory);
                    self.programs.push(program);
                    continue;
                }
            };
            self.problems.push(format!("{what}: {problem}"));
        }
    }

    /// The providers of the model catalog at `url` that no file or catalog before named, as its
    /// copy saved lists them, else as it lists them now.
    fn catalog(&mut self, url: &str) {
        let what = format!("model catalog {url}");
        if let Err(problem) = super::catalog::check_url(url) {
            return self.problems.push(format!("{what}: {problem}"));
        }
        let mut catalog = Catalog::new(url, self.logins.as_ref());
        let text = match catalog.saved() {
            Some(text) => Ok(text),
            None => {
                // A failure waits for /reload or the background to ask again.
                let read =
                    (self.tried_catalogs.get(url).cloned()).unwrap_or_else(|| catalog.read_now());
                self.tried_catalogs.insert(url.to_owned(), read.clone());
                read
            }
        };
        let offered = text.and_then(|text| {
            super::catalog::offered(&text).map_err(|problem| format!("it {problem}"))
        });
        let before = self.models.len();
        match offered {
            Ok(offered) => {
                let (named, entries): (Vec<_>, Vec<_>) = (offered.providers.into_iter())
                    .partition(|(name, _)| self.named.contains(name) || self.logs_in(name));
                // Said by `scramjet catalog add`, not at each start: the user may not own the
                // catalog, and the file or login is theirs.
                catalog.shadowed = named.into_iter().map(|(name, _)| name).collect();
                self.providers(Some(Ordered(entries)), &what);
                catalog.offered = Ok(self.models.len() - before);
            }
            Err(problem) => {
                self.problems.push(format!("{what}: {problem}"));
                catalog.offered = Err(problem);
            }
        }
        self.catalogs.push(catalog);
    }

    /// The built-in providers no file or catalog named, each among `keyed`, and the models of
    /// those with a key.
    fn builtins(&mut self) {
        for builtin in &BUILTIN {
            let name = builtin.name;
            if self.named.iter().any(|named| named == name) || self.logs_in(name) {
                continue;
            }
            let given = (self.given_key.as_ref()).is_some_and(|(given, _)| given == name);
            let from = match super::login::key(&self.saved, name) {
                _ if given => KeyFrom::Given,
                Some(_) => KeyFrom::Saved,
                None => KeyFrom::Nowhere,
            };
            let entry = serde_json::json!({"baseUrl": builtin.base_url, "api": builtin.api});
            self.keyed.push((name.to_owned(), from.clone()));
            self.origins.insert(name.to_owned(), origins(&entry));
            if from == KeyFrom::Nowhere {
                continue;
            }
            let what = format!("{} (/login)", builtin.shown);
            self.sources.insert(name.to_owned(), what.clone());
            self.provider(name, &entry, &what, builtin.described);
        }
    }

    /// The models of the providers logged in to that no file named, as their logins listed
    /// them.
    fn logins(&mut self) {
        let Some(logins) = self.logins.clone() else {
            return;
        };
        let saved = self.saved.clone();
        let builtin = super::login::LOGINS
            .iter()
            .map(|(name, _)| name.to_string());
        let programs = self.programs.iter().map(|program| program.provider.clone());
        let names: Vec<String> = builtin.chain(programs).collect();
        for name in &names {
            let name = name.as_str();
            if self.named.iter().any(|named| named == name) {
                continue;
            }
            let Some(credential) = saved.get(name) else {
                continue;
            };
            let what = format!("{name} (/login)");
            // Missing or unreadable, it lists no models: the login is to be done again.
            let listed = super::login::List::read(&logins, name);
            let listed = listed.map_or(Value::Null, |listed| listed.0);
            let Some(entry) = super::login::entry(name, credential, &listed) else {
                self.problems.push(format!(
                    "{what}: no models were read for the login yet; they are asked for again \
                     in the full screen, or /login again"
                ));
                continue;
            };
            if entry["models"].as_array().is_some_and(Vec::is_empty) {
                self.problems.push(format!(
                    "{what}: the login listed no models that take tools; /login again"
                ));
                continue;
            }
            self.sources
                .entry(name.to_owned())
                .or_insert_with(|| what.clone());
            self.provider(name, &entry, &what, None);
        }
    }

    /// A `models.json`'s `providers`, then those its `providersCommand` lists.
    fn document(&mut self, text: &[u8], directory: &Path, what: &str) {
        let document = match serde_json::from_slice::<Document>(text) {
            Ok(document) => document,
            Err(error) => return self.problems.push(format!("{what}: {error}")),
        };
        match &document.credential_files {
            None => {}
            Some(Value::Array(paths))
                if (paths.iter())
                    .all(|path| path.as_str().is_some_and(|p| !p.trim().is_empty())) =>
            {
                let paths = paths.iter().filter_map(Value::as_str);
                let paths = paths.map(|path| in_directory(path, directory));
                self.credential_files.extend(paths)
            }
            Some(_) => self.problems.push(format!(
                "{what}: credentialFiles must be a list of files and directories"
            )),
        }
        if let Ok(Value::Object(top)) = serde_json::from_slice::<Value>(text) {
            let unread = unread(&top, TOP_KEYS, "");
            (self.problems).extend(
                unread
                    .into_iter()
                    .map(|problem| format!("{what}: {problem}")),
            );
        }
        // Its stores stay hidden all the same.
        if let Some(problem) = unmet_engine(text) {
            return self.problems.push(format!("{what}: {problem}"));
        }
        self.providers(document.providers, what);
        match document.command {
            None => {}
            Some(Value::Array(words))
                if !words.is_empty() && words.iter().all(Value::is_string) =>
            {
                let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
                let program = format!("{what}: {}", words.join(" "));
                let ran = run(&words, directory).map_err(|problem| format!("{what}: {problem}"));
                match ran.and_then(|output| listed(&output, &program)) {
                    Ok(None) => self.listed_none.push(program),
                    Ok(Some(providers)) if providers.0.is_empty() => self.listed_none.push(program),
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
            self.named.push(name.clone());
            (self.sources.entry(name.clone())).or_insert_with(|| what.to_owned());
            if !self.logs_in(&name) && !self.keyed.iter().any(|(keyed, _)| *keyed == name) {
                let given = (self.given_key.as_ref()).is_some_and(|(given, _)| *given == name);
                let from = match (super::login::key(&self.saved, &name), &entry["apiKey"]) {
                    _ if given => KeyFrom::Given,
                    (Some(_), _) => KeyFrom::Saved,
                    (None, Value::String(key))
                        if std::env::var_os(key).is_some_and(|set| !set.is_empty()) =>
                    {
                        KeyFrom::Variable(key.clone())
                    }
                    (None, Value::String(_)) => KeyFrom::Entry,
                    (None, _) => KeyFrom::Nowhere,
                };
                self.keyed.push((name.clone(), from));
                self.origins.insert(name.clone(), origins(&entry));
            }
            self.provider(&name, &entry, &format!("{what}: provider {name}"), None);
        }
    }

    /// Provider `name`'s `entry`, whose list, if its server gives one, models.dev's provider
    /// `described` describes.
    fn provider(&mut self, name: &str, entry: &Value, what: &str, described: Option<&'static str>) {
        let Some(entry) = entry.as_object() else {
            return self.problems.push(format!("{what}: not a JSON object"));
        };
        for problem in unread(entry, PROVIDER_KEYS, "") {
            self.problems.push(format!("{what}: {problem}"));
        }
        if let Some(Value::Object(overrides)) = entry.get("modelOverrides") {
            for (id, given) in overrides {
                let unread = given.as_object().map(|given| unread(given, MODEL_KEYS, ""));
                for problem in unread.into_iter().flatten() {
                    (self.problems).push(format!("{what}: modelOverrides {id}: {problem}"));
                }
            }
        }
        let own = matches!(entry.get("models"), Some(Value::Array(models)) if !models.is_empty());
        let models = match entry.get("models") {
            Some(Value::Array(models)) if !models.is_empty() => models.clone(),
            Some(Value::Array(_)) | None => match self.listed(name, entry, what, described) {
                Some(models) => models,
                None => return,
            },
            Some(_) => return self.problems.push(format!("{what}: models must be a list")),
        };
        for model in &models {
            let mut mistaken = mistyped(model);
            if own && let Some(given) = model.as_object() {
                mistaken.extend(unread(given, MODEL_KEYS, ""));
            }
            match self.model(name, entry, model) {
                Ok(model)
                    if (self.models.iter())
                        .any(|m| m.provider == model.provider && m.id == model.id) =>
                {
                    self.problems.push(format!(
                        "{what}: model {} is listed already; not offered here",
                        model.id
                    ));
                }
                Ok(model) => {
                    for problem in mistaken {
                        (self.problems).push(format!("{what}: model {}: {problem}", model.id));
                    }
                    self.models.push(model)
                }
                Err(problem) => self.problems.push(format!("{what}: {problem}")),
            }
        }
    }

    /// The models provider `name`'s `entry`, which lists none, lists itself, as its server listed
    /// them when last asked, else now; `None`, with the problem said, when it cannot.
    fn listed(
        &mut self,
        name: &str,
        entry: &Map<String, Value>,
        what: &str,
        described: Option<&'static str>,
    ) -> Option<Vec<Value>> {
        let logs_in = self.logs_in(name);
        let api = entry.get("api").and_then(Value::as_str);
        let lists = matches!(
            api,
            Some("openai-responses" | "openai-completions" | "anthropic-messages")
        );
        if logs_in {
            self.problems.push(format!(
                "{what}: lists no models; /login's models for it are offered only when no file \
                 names it"
            ));
            return None;
        }
        if !lists || !entry.contains_key("baseUrl") {
            let problem = match api {
                Some(api) if entry.contains_key("baseUrl") => {
                    let near = near_miss(api, APIS)
                        .map(|known| format!("; did you mean {known}?"))
                        .unwrap_or_default();
                    format!(
                        "lists no models, and its api {api} is not one whose server it asks for \
                         them (openai-responses, anthropic-messages or openai-completions){near}"
                    )
                }
                _ => "lists no models, and has no baseUrl and api (openai-responses, \
                      anthropic-messages or openai-completions) to ask its server for them"
                    .to_owned(),
            };
            self.problems.push(format!("{what}: {problem}"));
            return None;
        }
        let overrides = match entry.get("modelOverrides") {
            None => Map::new(),
            Some(Value::Object(overrides)) => overrides.clone(),
            Some(_) => {
                let problem = format!("{what}: modelOverrides must be an object of model ids");
                self.problems.push(problem);
                Map::new()
            }
        };
        let field = |key: &str| entry.get(key);
        let headed = [entry.get("headers"), None];
        let (protocol, _, route) = match self.route(name, field, headed, "its model list") {
            Ok(route) => route,
            Err(problem) => {
                self.problems.push(format!("{what}: {problem}"));
                return None;
            }
        };
        let messages = protocol == Protocol::Messages;
        let file = self.logins.as_ref().map(|logins| logins.provided(name));
        let mut listing = Listing::new(&route, messages, file);
        listing.described = described.map(|provider| listed::Described {
            catalog: Catalog::new(MODELS_DEV, self.logins.as_ref()),
            provider,
            protocol: api.unwrap_or_default().to_owned(),
        });
        let read = match listing.saved().filter(|_| listing.describable()) {
            Some(models) => Ok(models),
            None => {
                let key = listing.key();
                // A failure waits for /reload, a new key or the background to ask again.
                let read = (self.tried.get(&key).cloned()).unwrap_or_else(|| listing.read_now());
                self.tried.insert(key, read.clone());
                read
            }
        };
        let models: Result<Vec<Value>, String> = read
            .map_err(|error| format!("asking its server for them failed: {error}"))
            .and_then(|read| listing.offers(&read))
            .map(|models| {
                let models = models.into_iter();
                models
                    .map(|model| listed::overridden(model, &overrides))
                    .collect()
            });
        let models = models.and_then(|models| match models.is_empty() {
            true => Err("its server lists none that take tools".to_owned()),
            false => Ok(models),
        });
        listing.offered = models.as_ref().map(Vec::len).map_err(String::clone);
        self.listings.push(listing);
        match models {
            Ok(models) => Some(models),
            Err(problem) => {
                self.problems
                    .push(format!("{what}: lists no models, and {problem}"));
                None
            }
        }
    }

    /// Where requests of a model whose fields are `field`'s go, of provider `name`, with the
    /// `headers` of its provider and its own: their protocol, the base URL as written, and the
    /// route; problems name `subject`.
    fn route<'v>(
        &self,
        name: &str,
        field: impl Fn(&str) -> Option<&'v Value>,
        headed: [Option<&'v Value>; 2],
        subject: &str,
    ) -> Result<(Protocol, &'v str, Route), String> {
        let text = |key: &str| match field(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.as_str())),
            Some(_) => Err(format!("{subject}: {key} must be a string")),
        };
        let protocol = match text("api")? {
            Some("openai-responses") => Protocol::Responses,
            Some("anthropic-messages") => Protocol::Messages,
            Some("openai-completions") => Protocol::Completions,
            Some("openai-codex-responses") => Protocol::Codex,
            Some(api) => {
                let near = near_miss(api, APIS)
                    .map(|known| format!("; did you mean {known}?"))
                    .unwrap_or_default();
                return Err(format!(
                    "{subject}: api {api} is not one Scramjet takes (openai-responses, \
                     anthropic-messages or openai-completions){near}"
                ));
            }
            None => return Err(format!("{subject}: no api")),
        };
        let url = text("baseUrl")?.ok_or_else(|| format!("{subject}: no baseUrl"))?;
        let base = base_url(url).map_err(|problem| format!("{subject}: baseUrl {problem}"))?;
        let origin = base.origin();
        let mut headers = Vec::new();
        // As Scramjet's, a key `/login` saved is sent in place of the entry's, but only to the
        // servers it was saved for: a package's entry of the same name may name another.
        let again = match (self.origins.get(name)).is_some_and(|named| named.contains(&origin)) {
            true => format!("/login again to send it to {origin}"),
            false => format!(
                "/login saves it only for the servers the first entry naming {name} gives, \
                 and {origin} is not one"
            ),
        };
        let given = (self.given_key.as_ref()).filter(|(given, _)| given == name);
        let key = match super::login::key(&self.saved, name) {
            _ if let Some((_, key)) = given => Some((key.as_str(), "--api-key")),
            Some((_, origins)) if origins.is_empty() => {
                return Err(format!(
                    "{subject}: the API key /login saved for {name} names no server, as keys \
                     saved before 0.98.0 do; {again}"
                ));
            }
            Some((_, origins)) if !origins.contains(&origin.as_str()) => {
                return Err(format!(
                    "{subject}: the API key /login saved for {name} is for {}, not {origin}; \
                     {again}",
                    origins.join(", ")
                ));
            }
            Some((saved, _)) => Some((saved, "the API key /login saved")),
            None => text("apiKey")?.map(|key| (key, "apiKey")),
        };
        let login = match (&self.logins, key) {
            (Some(logins), None) if self.logs_in(name) => Some(Login {
                provider: name.to_owned(),
                file: logins.auth.clone(),
                program: (self.programs.iter())
                    .find(|program| program.provider == name)
                    .cloned(),
            }),
            _ => None,
        };
        if login.is_some() {
            if !base.private() {
                return Err(format!(
                    "{subject}: {name}'s login would go over http:// to {origin}, unencrypted; a \
                     login's token goes over http:// only to this machine"
                ));
            }
            // This machine too: a proxy there in front of the login's server, which a package
            // able to run programs could not misuse further.
            let origins = self.login_origins(name);
            if !origins.contains(&origin) && !base.local() {
                let named = match origins.is_empty() {
                    true => "no server yet (/login)".to_owned(),
                    false => origins.join(", "),
                };
                return Err(format!(
                    "{subject}: {name}'s login is for {named}, not {origin}; its token goes only \
                     to the servers its login names and to this machine"
                ));
            }
        }
        let mut commanded = Vec::new();
        if let Some((key, what)) = key {
            let key = match given {
                Some(_) => Configured::Text(key.to_owned()),
                None => configured::read(key),
            };
            if protocol == Protocol::Messages {
                put(&mut headers, &mut commanded, "x-api-key", "", &key, what);
            }
            if protocol != Protocol::Messages || field("authHeader") == Some(&Value::Bool(true)) {
                put(
                    &mut headers,
                    &mut commanded,
                    "authorization",
                    "Bearer ",
                    &key,
                    what,
                );
            }
        }
        for given in headed.into_iter().flatten() {
            let given = given.as_object();
            let given = given.ok_or_else(|| format!("{subject}: headers must be an object"))?;
            for (header, value) in given {
                let value = value.as_str().ok_or_else(|| {
                    format!("{subject}: the header {header}'s value must be a string")
                })?;
                let header = header.to_ascii_lowercase();
                let what = format!("the header {header}");
                let value = configured::read(value);
                put(&mut headers, &mut commanded, &header, "", &value, &what);
            }
        }
        let named = (commanded.iter()).map(|commanded| (&commanded.header, None));
        let given = (headers.iter()).map(|(header, value)| (header, Some(value)));
        for (header, value) in given.chain(named) {
            if hyper::header::HeaderName::from_bytes(header.as_bytes()).is_err()
                || value.is_some_and(|value| hyper::header::HeaderValue::from_str(value).is_err())
            {
                return Err(format!(
                    "{subject}: the header {header} is not valid in HTTP"
                ));
            }
        }
        let BaseUrl {
            tls,
            host,
            port,
            authority,
            path,
        } = base;
        let route = Route {
            provider: name.to_owned(),
            tls,
            host,
            port,
            authority,
            path,
            headers,
            commanded,
            login,
        };
        Ok((protocol, url, route))
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
        let headed = [provider.get("headers"), entry.get("headers")];
        let (protocol, url, route) = self.route(name, field, headed, &format!("model {id}"))?;
        let given = [provider.get("compat"), entry.get("compat")];
        let compat = match protocol {
            Protocol::Completions => Compat::read(name, url, entry, given),
            Protocol::Responses => Compat::read_responses(given),
            _ => Ok(Compat::default()),
        };
        let compat = compat.map_err(|problem| format!("model {id}: {problem}"))?;
        let reasoning = entry["reasoning"].as_bool().unwrap_or(false);
        let (levels, renamed) = match reasoning {
            true => levels(&entry["thinkingLevelMap"]),
            false => Default::default(),
        };
        let input = entry["input"].as_array();
        let info = ModelInfo {
            id: id.clone(),
            protocol,
            window: entry["contextWindow"]
                .as_f64()
                .filter(|w| *w >= 1.0)
                .map(|w| w as u64),
            output: entry["maxTokens"].as_u64().filter(|tokens| *tokens > 0),
            input: entry["maxInputTokens"]
                .as_u64()
                .filter(|tokens| *tokens > 0),
            reasoning,
            levels,
            renamed,
            images: input.is_some_and(|input| input.iter().any(|kind| kind == "image")),
            prices: Prices::of(&entry["cost"]),
            compat,
            route: Some(route),
            runtime: (provider.get("runtime").and_then(Value::as_str))
                .and_then(super::local::Runtime::of),
        };
        Ok(Model {
            id,
            name: entry["name"].as_str().map(str::to_owned),
            provider: name.to_owned(),
            info,
        })
    }
}

/// The keys a `models.json` takes at its top, in a provider and in a model (or one of
/// `modelOverrides`), Scramjet 1's it does not read among them.
const TOP_KEYS: &[&str] = &[
    "providers",
    "providersCommand",
    "credentialFiles",
    "engines",
    "logins",
    "release",
    "version",
];
/// The protocols a `models.json` names in `api`, but for ChatGPT's login's own.
const APIS: &[&str] = &[
    "openai-responses",
    "anthropic-messages",
    "openai-completions",
];
const PROVIDER_KEYS: &[&str] = &[
    "name",
    "baseUrl",
    "apiKey",
    "api",
    "headers",
    "compat",
    "authHeader",
    "models",
    "modelOverrides",
    "runtime",
];
const MODEL_KEYS: &[&str] = &[
    "id",
    "name",
    "api",
    "baseUrl",
    "apiKey",
    "headers",
    "compat",
    "authHeader",
    "reasoning",
    "thinkingLevelMap",
    "input",
    "cost",
    "contextWindow",
    "maxTokens",
    "maxInputTokens",
    "requestLimits",
];
const COST_KEYS: &[&str] = &[
    "input",
    "output",
    "cacheRead",
    "cacheWrite",
    "header",
    "longContext",
    "byHeader",
];

/// The keys of `given` that are not `known` but so near one that they are taken for a mistyped
/// one; any other key is let pass, as Scramjet 1 lets it, for what other versions read.
fn unread(given: &Map<String, Value>, known: &[&str], at: &str) -> Vec<String> {
    let mut found = Vec::new();
    for key in given.keys() {
        if let Some(meant) = near_miss(key, known) {
            found.push(format!("{at}{key} is not read; did you mean {at}{meant}?"));
        }
    }
    if known == MODEL_KEYS
        && let Some(Value::Object(cost)) = given.get("cost")
    {
        found.extend(unread(cost, COST_KEYS, &format!("{at}cost.")));
    }
    found
}

/// The one of `known` that `key`, not one of them, mistypes: the same but for case, `_` and
/// `-`, else at most one edit from it (two once it is over 5 characters).
pub fn near_miss<'k>(key: &str, known: &[&'k str]) -> Option<&'k str> {
    if known.contains(&key) {
        return None;
    }
    let plain = |text: &str| -> Vec<char> {
        (text.chars().filter(|c| !matches!(c, '_' | '-')))
            .flat_map(char::to_lowercase)
            .collect()
    };
    let key = plain(key);
    // Edits are a character added, taken out, changed, or two swapped.
    let distance = |known: &[char]| {
        let mut d = vec![vec![0; known.len() + 1]; key.len() + 1];
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i;
        }
        d[0] = (0..=known.len()).collect();
        for i in 1..=key.len() {
            for j in 1..=known.len() {
                let changed = usize::from(key[i - 1] != known[j - 1]);
                d[i][j] = (d[i - 1][j - 1] + changed)
                    .min(d[i - 1][j] + 1)
                    .min(d[i][j - 1] + 1);
                if i > 1 && j > 1 && key[i - 1] == known[j - 2] && key[i - 2] == known[j - 1] {
                    d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
                }
            }
        }
        d[key.len()][known.len()]
    };
    let allowed = if key.len() > 5 { 2 } else { 1 };
    (known.iter().map(|known| (distance(&plain(known)), *known)))
        .filter(|(distance, _)| *distance <= allowed)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, known)| known)
}

/// What a model's `entry` gives in a type its reader cannot take, each then ignored; `null` is
/// taken as not given.
fn mistyped(entry: &Value) -> Vec<String> {
    fn number(value: &Value) -> bool {
        value.is_number()
    }
    fn tokens(value: &Value) -> bool {
        value.is_u64()
    }
    fn rates(at: &str, cost: &Value, found: &mut Vec<String>) {
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            given(
                cost,
                key,
                at,
                number,
                "a number of dollars per million tokens",
                found,
            );
        }
    }
    fn given(
        entry: &Value,
        key: &str,
        at: &str,
        fits: fn(&Value) -> bool,
        wanted: &str,
        found: &mut Vec<String>,
    ) -> bool {
        match entry.get(key) {
            Some(value) if !value.is_null() && !fits(value) => {
                found.push(format!("{at}{key} must be {wanted}; ignored"));
                false
            }
            value => value.is_some_and(|value| !value.is_null()),
        }
    }
    let mut found = Vec::new();
    let found_in = &mut found;
    given(entry, "name", "", Value::is_string, "a text", found_in);
    given(
        entry,
        "reasoning",
        "",
        Value::is_boolean,
        "true or false",
        found_in,
    );
    let texts =
        |value: &Value| (value.as_array()).is_some_and(|all| all.iter().all(Value::is_string));
    given(
        entry,
        "input",
        "",
        texts,
        "a list such as [\"text\", \"image\"]",
        found_in,
    );
    given(
        entry,
        "contextWindow",
        "",
        number,
        "a number of tokens",
        found_in,
    );
    for key in ["maxTokens", "maxInputTokens"] {
        given(entry, key, "", tokens, "a whole number of tokens", found_in);
    }
    given(
        entry,
        "thinkingLevelMap",
        "",
        Value::is_object,
        "an object of levels",
        found_in,
    );
    if given(
        entry,
        "cost",
        "",
        Value::is_object,
        "an object of prices",
        found_in,
    ) {
        let cost = &entry["cost"];
        rates("cost.", cost, found_in);
        given(
            cost,
            "header",
            "cost.",
            Value::is_string,
            "a header's name",
            found_in,
        );
        if given(
            cost,
            "longContext",
            "cost.",
            Value::is_object,
            "an object",
            found_in,
        ) {
            let long = &cost["longContext"];
            given(
                long,
                "above",
                "cost.longContext.",
                tokens,
                "a whole number of tokens",
                found_in,
            );
            rates("cost.longContext.", long, found_in);
        }
        if given(
            cost,
            "byHeader",
            "cost.",
            Value::is_object,
            "an object of header values",
            found_in,
        ) {
            for (value, rated) in cost["byHeader"].as_object().into_iter().flatten() {
                let at = format!("cost.byHeader.{value}");
                match rated.is_object() {
                    true => rates(&format!("{at}."), rated, found_in),
                    false => found_in.push(format!("{at} must be an object of prices; ignored")),
                }
            }
        }
    }
    found
}

/// The levels a model that reasons takes, as Scramjet reads its `thinkingLevelMap`, and those it
/// sends as another text: a level the map gives a text is sent as that text, one it gives `null`
/// is not taken, and those it leaves out are taken but `xhigh` and `max`.
fn levels(map: &Value) -> (Vec<String>, Vec<(String, String)>) {
    let taken: Vec<(&str, &str)> = (LEVELS.iter())
        .filter_map(|(level, name, ordinary)| match map.get(level) {
            Some(Value::String(text)) => Some((*name, text.as_str())),
            Some(_) => None,
            None => ordinary.then_some((*name, *name)),
        })
        .collect();
    // Scramjet's own maps send two levels as one (`minimal` as `low`); Shift+Tab steps once, on
    // the level named as it is sent.
    let doubled = |name: &str, text: &str| {
        name != text
            && taken
                .iter()
                .any(|(other, sent)| other != &name && *sent == text)
    };
    let kept = (taken.iter()).filter(|(name, text)| !doubled(name, text));
    let levels = kept.clone().map(|(name, _)| name.to_string()).collect();
    let renamed = (kept.filter(|(name, text)| name != text))
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect();
    (levels, renamed)
}

/// The `thinkingLevelMap` of a model a login lists with the effort levels `efforts`, as
/// requests name them: each of Scramjet's levels sent as it is when listed, else not taken.
pub(super) fn level_map(efforts: &[&str]) -> Value {
    let map = LEVELS.iter().map(|(level, sent, _)| {
        let taken = efforts.contains(sent);
        (
            level.to_string(),
            if taken {
                Value::from(*sent)
            } else {
                Value::Null
            },
        )
    });
    Value::Object(map.collect())
}

/// Sets `header` to `prefix` and `value`, in place of any value it had: in `headers` when it is
/// a text, in `commanded` when a command prints it at each request.
fn put(
    headers: &mut Vec<(String, String)>,
    commanded: &mut Vec<Commanded>,
    header: &str,
    prefix: &'static str,
    value: &Configured,
    what: &str,
) {
    headers.retain(|(name, _)| name != header);
    commanded.retain(|commanded| commanded.header != header);
    match value {
        Configured::Text(text) => headers.push((header.to_owned(), format!("{prefix}{text}"))),
        Configured::Command(command) => commanded.push(Commanded {
            header: header.to_owned(),
            prefix,
            command: command.clone(),
            what: what.to_owned(),
        }),
    }
}

#[derive(Debug, PartialEq)]
pub(super) struct BaseUrl {
    pub tls: bool,
    pub host: String,
    /// 80 for `http://` and 443 for `https://` unless the URL names one.
    pub port: u16,
    pub authority: String,
    /// Without a trailing `/`.
    pub path: String,
}

impl BaseUrl {
    /// The server credentials are tied to: `scheme://host:port`, the host in lower case.
    pub fn origin(&self) -> String {
        let host = self.host.to_ascii_lowercase();
        let host = match host.contains(':') {
            true => format!("[{host}]"),
            false => host,
        };
        let scheme = if self.tls { "https" } else { "http" };
        format!("{scheme}://{host}:{}", self.port)
    }

    /// Whether it is on this machine.
    fn local(&self) -> bool {
        let host = self.host.to_ascii_lowercase();
        host == "localhost" || (host.parse::<std::net::IpAddr>()).is_ok_and(|ip| ip.is_loopback())
    }

    /// Whether what is sent there stays private: over TLS, or to this machine.
    fn private(&self) -> bool {
        self.tls || self.local()
    }
}

/// Whether what is sent to the server `origin` stays private (`BaseUrl::private`).
pub(super) fn private_to(origin: &str) -> bool {
    base_url(origin).is_ok_and(|url| url.private())
}

/// The servers a provider's `entry` sends requests to: its `baseUrl` and its models' and
/// `modelOverrides`', each once.
fn origins(entry: &Value) -> Vec<String> {
    let models = entry["models"].as_array().into_iter().flatten();
    let overrides = entry["modelOverrides"].as_object().into_iter().flatten();
    let models = models.chain(overrides.map(|(_, model)| model));
    let urls = std::iter::once(&entry["baseUrl"]).chain(models.map(|model| &model["baseUrl"]));
    let mut origins: Vec<String> = (urls.filter_map(Value::as_str))
        .filter_map(|url| base_url(url).ok())
        .map(|url| url.origin())
        .collect();
    origins.sort();
    origins.dedup();
    origins
}

/// An `http://` or `https://` URL's parts.
pub(super) fn base_url(url: &str) -> Result<BaseUrl, String> {
    let scheme = |prefix: &str| {
        url.get(..prefix.len())
            .is_some_and(|s| s.eq_ignore_ascii_case(prefix))
    };
    let (tls, rest) = match (scheme("https://"), scheme("http://")) {
        (true, _) => (true, &url["https://".len()..]),
        (_, true) => (false, &url["http://".len()..]),
        _ => return Err(format!("{url}: not an http:// or https:// URL")),
    };
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
        None if tls => 443,
        None => 80,
        Some(port) => port
            .parse()
            .map_err(|_| format!("{url}: the port is not a number"))?,
    };
    if host.is_empty() {
        return Err(format!("{url}: no host"));
    }
    Ok(BaseUrl {
        tls,
        host: host.to_owned(),
        port,
        authority: authority.to_owned(),
        path: path.trim_end_matches('/').to_owned(),
    })
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

/// What keeps this Scramjet from taking a `models.json`'s `text`: its `engines.scramjet`, as
/// npm's `engines`, asking for a later version or in a form not read here.
pub fn unmet_engine(text: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(text).ok()?;
    let wanted = document.get("engines")?;
    let unread = || {
        Some(
            "engines must be {\"scramjet\": \">=X.Y.Z\"}, the earliest Scramjet it works \
             with; not read"
                .to_owned(),
        )
    };
    let Some(wanted) = wanted.as_object().map(|engines| engines.get("scramjet")) else {
        return unread();
    };
    let wanted = wanted?
        .as_str()
        .and_then(|wanted| wanted.strip_prefix(">="));
    let Some(earliest) = wanted.map(str::trim).and_then(version) else {
        return unread();
    };
    let running = env!("CARGO_PKG_VERSION");
    (version(running)? < earliest).then(|| {
        format!(
            "needs Scramjet {}.{}.{} or later, and this is {running}; `scramjet update self` \
             updates it",
            earliest[0], earliest[1], earliest[2]
        )
    })
}

/// `X.Y.Z`'s numbers.
fn version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.split('.').map(|part| {
        let digits = !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        digits.then(|| part.parse().ok()).flatten()
    });
    let numbers = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(numbers)
}

/// `path` as a settings path is taken: `~` is the home directory, and a relative one is from
/// `directory`, the file's.
fn in_directory(path: &str, directory: &Path) -> PathBuf {
    let path = path.trim();
    match crate::os::after_tilde(path).zip(std::env::home_dir()) {
        Some((rest, home)) => crate::os::join(&home, rest),
        None => crate::os::join(directory, path),
    }
}

/// The program a file in `directory` names: `~/` starts a path in the home directory, a path
/// with a `/` is from `directory`, and a name is looked for in `PATH`.
/// Why `program`, at `path`, could not be started: for one that is not there, where it was
/// looked for, as a relative path's directory is otherwise not said.
pub(super) fn cannot_run(program: &str, path: &Path, error: &std::io::Error) -> String {
    match error.kind() == std::io::ErrorKind::NotFound && path.is_absolute() && !path.exists() {
        true => {
            let path: PathBuf = path.components().collect();
            format!("cannot run {program}: there is no {}", path.display())
        }
        false => format!("cannot run {program}: {error}"),
    }
}

pub(super) fn program_path(program: &str, directory: &Path) -> PathBuf {
    match crate::os::after_tilde(program).zip(std::env::home_dir()) {
        Some((rest, home)) => crate::os::join(&home, rest),
        None if program.contains(std::path::is_separator) => crate::os::join(directory, program),
        None => crate::os::program(program, None),
    }
}

/// The output of the program `words` names with its arguments (`program_path`), run in
/// `directory`.
fn run(words: &[&str], directory: &Path) -> Result<Vec<u8>, String> {
    let program = words[0];
    let path = program_path(program, directory);
    // In a group of its own, so that the processes it starts end with it.
    let mut command = Command::new(&path);
    let mut child = crate::os::own_group(&mut command)
        .args(&words[1..])
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| cannot_run(program, &path, &error))?;
    let group = child.id();
    let (mut stdout, mut stderr) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    // What is past the limits is read and dropped: a closed pipe would end the program.
    let (sender, output) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = (&mut stdout)
            .take(COMMAND_OUTPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .and_then(|_| std::io::copy(&mut stdout, &mut std::io::sink()));
        let _ = sender.send(read.map(|_| bytes));
    });
    let (sender, errors) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = (&mut stderr).take(64 << 10).read_to_end(&mut bytes);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        let _ = sender.send(bytes);
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let timed_out = |child: &mut std::process::Child| {
        crate::os::kill_group(group);
        let _ = child.kill();
        let _ = child.wait();
        let seconds = COMMAND_TIMEOUT.as_secs();
        format!("{program} gave no answer within {seconds}s")
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Ok(None) => return Err(timed_out(&mut child)),
            Err(error) => return Err(format!("{program}: {error}")),
        }
    };
    // A process it left running may hold the pipes open: the limit stands until they close.
    let left = || deadline.saturating_duration_since(Instant::now());
    let (Ok(errors), Ok(output)) = (errors.recv_timeout(left()), output.recv_timeout(left()))
    else {
        return Err(timed_out(&mut child));
    };
    let said = String::from_utf8_lossy(&errors);
    let said = said.lines().map(str::trim).find(|line| !line.is_empty());
    let said = said.map(|line| format!(": {line}")).unwrap_or_default();
    if !status.success() {
        return Err(format!("{program} failed ({status}){said}"));
    }
    let output = output.map_err(|error| format!("{program}: {error}"))?;
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
    use crate::agent::tests::{ScratchDir, serve_json};
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
                        "contextWindow": 1_050_000, "maxTokens": 128_000, "maxInputTokens": 922_000,
                        "thinkingLevelMap": {"minimal": null, "xhigh": "xhigh"}},
                    {"id": "claude", "api": "anthropic-messages", "baseUrl": "http://[::1]:9",
                        "authHeader": true, "headers": {"x-team": "messages"}},
                    {"id": "plain"},
                    {"id": "chat", "api": "openai-completions", "baseUrl": "http://localhost:11434/v1",
                        "compat": {"maxTokensField": "max_tokens"}},
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
        assert_eq!(info.input, Some(922_000));
        assert!(info.reasoning && info.images);
        assert_eq!(info.levels, ["none", "low", "medium", "high", "xhigh"]);
        // A level sent as another's is taken once, as in Scramjet's maps for GPT-5.
        let (shared, renamed) = levels(&json!({"xhigh": "xhigh", "minimal": "low"}));
        assert_eq!(shared, ["none", "low", "medium", "high", "xhigh"]);
        assert_eq!(renamed, []);
        // A level sent as a text of the map's own keeps Scramjet's name, and is sent as that text.
        let (named, renamed) = levels(&json!({"off": "disabled", "xhigh": "max"}));
        assert_eq!(named, ["none", "minimal", "low", "medium", "high", "xhigh"]);
        let pair = |level: &str, text: &str| (level.to_owned(), text.to_owned());
        assert_eq!(renamed, [pair("none", "disabled"), pair("xhigh", "max")]);
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
        // Chat Completions takes the key as Responses does.
        let chat = providers.find("chat").unwrap();
        assert_eq!(chat.info.protocol, Protocol::Completions);
        assert_eq!(route(&providers, "chat").headers[0], headers[0]);
        assert_ne!(chat.info.compat, Compat::default());
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
    fn commands_are_kept_to_run_at_each_request_and_a_later_value_replaces_one() {
        let providers = Providers::given(json!({"providers": {"p": {
            "baseUrl": "http://localhost", "api": "anthropic-messages", "apiKey": "!pass key", "authHeader": true,
            "headers": {"x-team": "!pass team", "X-Api-Key": "literal"},
            "models": [{"id": "m", "headers": {"x-team": "t", "x-user": "!whoami"}}],
        }}}));
        assert_eq!(providers.problems(), [] as [&str; 0]);
        let route = route(&providers, "m");
        let header = |name: &str, value: &str| (name.to_owned(), value.to_owned());
        assert_eq!(
            route.headers,
            [header("x-api-key", "literal"), header("x-team", "t")]
        );
        let commanded: Vec<(&str, &str, &str, &str)> = (route.commanded.iter())
            .map(|c| {
                (
                    c.header.as_str(),
                    c.prefix,
                    c.command.as_str(),
                    c.what.as_str(),
                )
            })
            .collect();
        assert_eq!(
            commanded,
            [
                ("authorization", "Bearer ", "pass key", "apiKey"),
                ("x-user", "", "whoami", "the header x-user"),
            ]
        );
        assert!(!format!("{route:?}").contains("pass"));
    }

    /// `tests/compatibility`'s `models.json` files, the user's and packages', read with no
    /// problem, each key still one the reader takes: in its place, a value of no field's type is
    /// said, where a key no longer read would be passed over in silence (the compatibility
    /// promise).
    #[test]
    fn earlier_versions_models_files_are_read_as_they_were() {
        fn leaves(value: &Value, at: Vec<String>, found: &mut Vec<Vec<String>>) {
            match value {
                Value::Object(object) if !object.is_empty() => {
                    for (key, value) in object {
                        let mut path = at.clone();
                        path.push(key.clone());
                        leaves(value, path, found);
                    }
                }
                Value::Array(items) if items.iter().any(Value::is_object) => {
                    for (n, item) in items.iter().enumerate() {
                        let mut path = at.clone();
                        path.push(n.to_string());
                        leaves(item, path, found);
                    }
                }
                _ => found.push(at),
            }
        }
        fn models_files(directory: &Path, found: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                match path.is_dir() {
                    true => models_files(&path, found),
                    false if path.ends_with("models.json") => found.push(path),
                    false => {}
                }
            }
        }
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/compatibility");
        let mut files = Vec::new();
        models_files(&fixtures, &mut files);
        assert!(files.len() >= 2, "{files:?}");
        let mut unread = Vec::new();
        for path in files {
            let text = std::fs::read_to_string(&path).unwrap().replace("PORT", "1");
            let document: Value = serde_json::from_str(&text).unwrap();
            let providers = Providers::given(document.clone());
            assert!(
                providers.problems().is_empty(),
                "{}: {:?}",
                path.display(),
                providers.problems()
            );
            assert!(!providers.models().is_empty(), "{}", path.display());
            let mut found = Vec::new();
            leaves(&document, Vec::new(), &mut found);
            for leaf in found {
                let mut changed = document.clone();
                let mut value = &mut changed;
                for key in &leaf {
                    value = match value {
                        Value::Array(items) => &mut items[key.parse::<usize>().unwrap()],
                        _ => &mut value[key],
                    };
                }
                *value = json!([["no field's type"]]);
                if Providers::given(changed).problems().is_empty() {
                    unread.push(format!("{}: {leaf:?}", path.display()));
                }
            }
        }
        assert!(unread.is_empty(), "not read: {unread:#?}");
    }

    #[test]
    fn a_models_json_needing_a_later_scramjet_is_not_read_but_hides_its_stores() {
        let running = env!("CARGO_PKG_VERSION");
        let wanting = |wanted: Value| {
            let document = json!({"engines": wanted, "credentialFiles": ["/stores/a"],
                "logins": {"co": {"command": ["./login"]}},
                "providers": {"a": {"baseUrl": "http://a/v1", "api": "openai-responses",
                    "models": [{"id": "m"}]}}});
            Providers::given(document)
        };
        for met in [
            json!({"scramjet": format!(">={running}")}),
            json!({"scramjet": ">= 2.0.0"}),
            json!({"node": ">=99"}),
        ] {
            let providers = wanting(met);
            assert_eq!(providers.models().len(), 1);
            assert_eq!(providers.login_programs().len(), 1);
            assert!(
                providers.problems().is_empty(),
                "{:?}",
                providers.problems()
            );
        }
        let later = wanting(json!({"scramjet": ">=99.0.0"}));
        assert!(later.models().is_empty() && later.login_programs().is_empty());
        assert!((later.credential_files().iter()).any(|file| file.ends_with("stores/a")));
        assert_eq!(
            later.problems(),
            [format!(
                "models: needs Scramjet 99.0.0 or later, and this is {running}; \
                 `scramjet update self` updates it"
            )]
        );
        for unread in [
            json!("2.0.0"),
            json!({"scramjet": "^2.0.0"}),
            json!({"scramjet": 2}),
            json!({"scramjet": ">=2.0"}),
            json!({"scramjet": ">=2.0.x"}),
        ] {
            let providers = wanting(unread);
            assert!(providers.models().is_empty());
            assert_eq!(
                providers.problems(),
                [
                    "models: engines must be {\"scramjet\": \">=X.Y.Z\"}, the earliest \
                  Scramjet it works with; not read"
                ]
            );
        }
    }

    #[test]
    fn a_models_field_of_the_wrong_type_is_said_and_ignored() {
        let providers = Providers::given(json!({"providers": {"a": {
        "baseUrl": "http://a/v1", "api": "openai-responses", "models": [
            {"id": "m", "name": 7, "reasoning": "yes", "input": "image",
                "contextWindow": "200000", "maxTokens": 1.5, "maxInputTokens": null,
                "cost": {"input": "1.25", "output": 10, "longContext": {"above": -1},
                    "byHeader": {"eu": 2}}},
            {"id": "n", "cost": [1, 2]},
        ]}}}));
        let ids: Vec<String> = (providers.models().iter()).map(Model::reference).collect();
        assert_eq!(ids, ["a/m", "a/n"]);
        let m = &providers.models()[0];
        assert_eq!(
            (m.name.as_deref(), m.info.window, m.info.output),
            (None, None, None)
        );
        assert!(!m.info.reasoning && !m.info.images);
        let said = |problem: &str| format!("models: provider a: model {problem}; ignored");
        assert_eq!(
            providers.problems(),
            [
                said("m: name must be a text"),
                said("m: reasoning must be true or false"),
                said("m: input must be a list such as [\"text\", \"image\"]"),
                said("m: contextWindow must be a number of tokens"),
                said("m: maxTokens must be a whole number of tokens"),
                said("m: cost.input must be a number of dollars per million tokens"),
                said("m: cost.longContext.above must be a whole number of tokens"),
                said("m: cost.byHeader.eu must be an object of prices"),
                said("n: cost must be an object of prices"),
            ]
        );
    }

    #[test]
    fn a_mistyped_key_is_said_with_the_one_meant_and_others_pass() {
        let providers = Providers::given(json!({
            "providersComand": ["./list"], "$schema": "x",
            "providers": {
                "a": {"base_url": "http://a/v1", "baseUrl": "http://a/v1", "api": "openai-responses",
                    "modelOverides": {}, "somethingNew": 1, "models": [
                    {"id": "m", "contextWindw": 5, "MaxTokens": 3, "requestLimits": {},
                        "cost": {"inptu": 1, "output": 2}},
                    {"id": "n", "api": "openai-respones"},
                ]},
                "b": {"baseUrl": "http://b/v1", "api": "anthropic-mesages"},
            },
        }));
        let ids: Vec<String> = (providers.models().iter()).map(Model::reference).collect();
        assert_eq!(ids, ["a/m"]);
        assert_eq!(
            providers.problems(),
            [
                "models: providersComand is not read; did you mean providersCommand?",
                "models: provider a: base_url is not read; did you mean baseUrl?",
                "models: provider a: modelOverides is not read; did you mean modelOverrides?",
                "models: provider a: model m: MaxTokens is not read; did you mean maxTokens?",
                "models: provider a: model m: contextWindw is not read; did you mean \
                 contextWindow?",
                "models: provider a: model m: cost.inptu is not read; did you mean cost.input?",
                "models: provider a: model n: api openai-respones is not one Scramjet takes \
                 (openai-responses, anthropic-messages or openai-completions); did you mean \
                 openai-responses?",
                "models: provider b: lists no models, and its api anthropic-mesages is not one \
                 whose server it asks for them (openai-responses, anthropic-messages or \
                 openai-completions); did you mean anthropic-messages?",
            ]
        );
    }

    #[test]
    fn an_agent_files_model_names_the_newest_of_its_family() {
        let providers = Providers::given(json!({
            "providers": {
                "a": {"baseUrl": "http://a/v1", "api": "anthropic-messages", "models": [
                    {"id": "claude-sonnet-4-5"}, {"id": "claude-sonnet-5-20261001"},
                    {"id": "claude-haiku-4-5"},
                ]},
                "b": {"baseUrl": "http://b/v1", "api": "anthropic-messages", "models": [
                    {"id": "claude-sonnet-5"}, {"id": "claude-opus-5-5"},
                ]},
            },
        }));
        let family = |name, provider| providers.family(name, provider).map(Model::reference);
        assert_eq!(
            family("sonnet", "a").as_deref(),
            Some("a/claude-sonnet-4-5")
        );
        assert_eq!(family("Sonnet", "b").as_deref(), Some("b/claude-sonnet-5"));
        assert_eq!(family("opus", "a").as_deref(), Some("b/claude-opus-5-5"));
        assert_eq!(
            family("a/claude-haiku-4-5", "b").as_deref(),
            Some("a/claude-haiku-4-5")
        );
        assert_eq!(family("gpt", "a"), None);
    }

    #[test]
    fn what_is_wrong_is_said_and_the_rest_is_offered() {
        let providers = Providers::given(json!({
            "providers": {
                "a": {"baseUrl": "http://a/v1", "api": "openai-responses", "models": [
                    {"id": "m"}, {"name": "no id"}, {"id": "chat", "api": "google-generative-ai"},
                    {"id": "odd", "api": "openai-completions", "compat": {"supportsStore": 1}},
                    {"id": "shell", "headers": {"x shell": "!pass show key"}},
                    {"id": "bad", "headers": {"x-bad": "line\nbreak"}},
                    {"id": "m", "name": "again"},
                ]},
                "b": {"baseUrl": "http://b/v1", "api": "openai-responses",
                    "models": [{"id": "m"}, {"id": "n"}]},
                "c": {"baseUrl": "http://c"},
            },
            "providersCommand": "not a list",
        }));
        let ids: Vec<String> = (providers.models().iter()).map(Model::reference).collect();
        assert_eq!(ids, ["a/m", "b/m", "b/n"]);
        // Its reference names a provider's model; its id, the first's.
        let provider = |name| providers.find(name).map(|model| model.provider.as_str());
        assert_eq!((provider("m"), provider("b/m")), (Some("a"), Some("b")));
        assert_eq!(
            (provider("c/m"), provider("a/n"), provider("/m")),
            (None, None, None)
        );
        assert_eq!(providers.reference("m"), "a/m");
        assert_eq!(providers.reference("z/m"), "z/m");
        assert_eq!(providers.info("b/m").id, "m");
        assert_eq!(providers.info("z/m"), ModelInfo::guess("z/m"));
        assert_eq!(
            providers.problems(),
            [
                "models: provider a: a model has no id",
                "models: provider a: model chat: api google-generative-ai is not one Scramjet takes \
                 (openai-responses, anthropic-messages or openai-completions)",
                "models: provider a: model odd: compat.supportsStore must be true or false",
                "models: provider a: model shell: the header x shell is not valid in HTTP",
                "models: provider a: model bad: the header x-bad is not valid in HTTP",
                "models: provider a: model m is listed already; not offered here",
                "models: provider c: lists no models, and has no baseUrl and api \
                 (openai-responses, anthropic-messages or openai-completions) to ask its server \
                 for them",
                "models: providersCommand must be a program and its arguments, as a list of strings",
            ]
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn a_program_lists_more_providers_and_its_failures_are_said() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-rs-providers-{}", std::process::id())),
        );
        let script = |name: &str, body: &str| {
            crate::agent::tests::script(&scratch.0.join(name), body);
        };
        let listed = json!({"version": 1, "providers": {"served": {
            "baseUrl": "http://127.0.0.1:1/v1", "api": "openai-responses",
            "models": [{"id": "from-program"}],
        }}});
        script(
            "list",
            &format!(
                "[ \"$1\" = arg ] && [ \"$PWD\" = \"{}\" ] && echo '{listed}'",
                // macOS's temporary directory is under /var, a link to /private/var.
                std::fs::canonicalize(&scratch.0).unwrap().display()
            ),
        );
        script("fail", "echo 'cannot reach the server' >&2; exit 3");
        script("old", "echo '{\"version\": 2}'");
        script("slow", "sleep 10 & echo $! > slow.pid; wait");
        script(
            "background",
            &format!("sleep 10 & echo $! > background.pid; echo '{listed}'"),
        );
        script(
            "noisy",
            &format!(
                "i=0; while [ $i -lt 3000 ]; do echo \"progress line $i of many\" >&2; i=$((i+1)); done; echo '{listed}'"
            ),
        );
        let file = scratch.0.join("models.json");
        let write = |document: Value| std::fs::write(&file, document.to_string()).unwrap();
        write(json!({
            "providers": {"own": {"baseUrl": "http://x", "api": "openai-responses",
                "models": [{"id": "own"}]}},
            "providersCommand": ["./list", "arg"],
        }));
        let mut providers =
            Providers::read(vec![file.clone(), scratch.0.join("absent.json")], None);
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
                format!(
                    "cannot run ./absent: there is no {}",
                    scratch.0.join("absent").display()
                ),
            ),
        ] {
            write(json!({"providersCommand": [program]}));
            providers.reload();
            assert_eq!(providers.problems(), [format!("{shown}: {problem}")]);
            assert!(providers.models().is_empty());
        }
        // More than the 64 KiB of errors kept.
        write(json!({"providersCommand": ["./noisy"]}));
        providers.reload();
        assert_eq!(providers.problems(), [] as [String; 0]);
        assert_eq!(providers.models().len(), 1);
        // The limit holds for a program and what it leaves running on its output, which ends.
        let gone = |name: &str| {
            let pid = std::fs::read_to_string(scratch.0.join(name)).unwrap();
            let process = PathBuf::from(format!("/proc/{}", pid.trim()));
            let until = Instant::now() + Duration::from_secs(2);
            while process.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            !process.exists()
        };
        for program in ["slow", "background"] {
            write(json!({"providersCommand": [format!("./{program}")]}));
            let started = Instant::now();
            providers.reload();
            assert!(started.elapsed() < COMMAND_TIMEOUT + Duration::from_secs(1));
            assert_eq!(
                providers.problems(),
                [format!("{shown}: ./{program} gave no answer within 2s")]
            );
            assert!(gone(&format!("{program}.pid")), "{program}");
        }
        std::fs::write(&file, "{").unwrap();
        providers.reload();
        assert!(providers.problems()[0].starts_with(&format!("{shown}: EOF while parsing")));
        assert_eq!(
            providers.files(),
            [file.as_path(), &scratch.0.join("absent.json")]
        );
    }

    #[test]
    fn a_login_adds_its_models_after_the_files_unless_a_file_names_its_provider() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-providers-login-{}", std::process::id())),
        );
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let auth = logins.auth.clone();
        let listed = |id: &str| {
            json!({"id": id, "capabilities": {"type": "chat", "supports": {"tool_calls": true}},
                "supported_endpoints": ["/responses"]})
        };
        let credential = json!({"type": "oauth", "access": "tid=1;proxy-ep=proxy.x.example"});
        super::super::login::save(&auth, "github-copilot", credential).unwrap();
        // Logged in without the list, the login is to be done again.
        let providers = Providers::given_logged_in(json!({}), logins.clone());
        assert_eq!(
            providers.problems(),
            ["github-copilot (/login): the login listed no models that take tools; /login again"]
        );
        let list = json!({"data": [listed("gpt-6-luna"), listed("grok-4.6"), listed("gpt-6-sol")]});
        std::fs::create_dir_all(&logins.lists).unwrap();
        std::fs::write(logins.listed("github-copilot"), list.to_string()).unwrap();
        let openai = json!({"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
            "apiKey": "k", "models": [{"id": "gpt-6-luna"}, {"id": "gpt-6-sol"}]});
        let providers =
            Providers::given_logged_in(json!({"providers": {"openai": openai}}), logins.clone());
        // Models another provider offers too are the login's as well.
        let ids: Vec<String> = (providers.models().iter()).map(Model::reference).collect();
        assert_eq!(
            ids,
            [
                "openai/gpt-6-luna",
                "openai/gpt-6-sol",
                "github-copilot/gpt-6-luna",
                "github-copilot/grok-4.6",
                "github-copilot/gpt-6-sol"
            ]
        );
        assert_eq!(providers.problems(), [] as [String; 0]);
        let luna = route(&providers, "github-copilot/gpt-6-luna");
        assert_eq!((luna.provider.as_str(), luna.tls), ("github-copilot", true));
        let grok = route(&providers, "grok-4.6");
        assert_eq!((grok.tls, grok.authority.as_str()), (true, "api.x.example"));
        assert_eq!(grok.login.unwrap().file, auth);
        // A file that names the provider replaces the login's list, and takes its token to the
        // login's own server only.
        let named = json!({"providers": {"github-copilot": {"baseUrl": "https://API.x.example",
            "api": "openai-completions", "models": [{"id": "kimi-k3"},
                {"id": "local", "baseUrl": "http://127.0.0.1:9"},
                {"id": "proxied", "baseUrl": "https://proxy.x.example"},
                {"id": "open", "baseUrl": "http://api.x.example:443"}]}}});
        let providers = Providers::given_logged_in(named, logins.clone());
        let ids: Vec<&str> = providers
            .models()
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(ids, ["kimi-k3", "local"]);
        assert!(route(&providers, "kimi-k3").login.is_some());
        assert_eq!(
            providers.problems(),
            [
                "models: provider github-copilot: model proxied: github-copilot's login is for \
                 https://api.x.example:443, not https://proxy.x.example:443; its token goes only \
                 to the servers its login names and to this machine",
                "models: provider github-copilot: model open: github-copilot's login would go \
                 over http:// to http://api.x.example:443, unencrypted; a login's token goes \
                 over http:// only to this machine"
            ]
        );
        // Logged out, the login's models go.
        super::super::login::logout(&logins, "github-copilot").unwrap();
        assert!(!logins.listed("github-copilot").exists());
        assert!(
            Providers::given_logged_in(json!({}), logins)
                .models()
                .is_empty()
        );
    }

    #[test]
    fn a_listed_provider_routes_each_model_by_the_protocol_its_pattern_gives() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-providers-routed-{}", std::process::id())),
        );
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let (base, server) = serve_json(
            1,
            |_, _| json!({"data": [{"id": "claude-a", "display_name": "Claude A"}, {"id": "gpt-b"}]}),
        );
        let document = json!({"providers": {"proxy": {
            "baseUrl": base, "api": "anthropic-messages", "apiKey": "k",
            "modelOverrides": {
                "claude-*": {"reasoning": true, "contextWindow": 1_000_000},
                "gpt-*": {"api": "openai-responses", "baseUrl": format!("{base}/v1"),
                    "contextWindow": 400_000}}}}});
        let providers = Providers::given_logged_in(document, logins);
        assert_eq!(providers.problems(), [] as [String; 0]);
        let claude = &providers.find("proxy/claude-a").unwrap().info;
        assert_eq!(
            (claude.protocol, claude.window, claude.reasoning),
            (Protocol::Messages, Some(1_000_000), true)
        );
        let gpt = &providers.find("proxy/gpt-b").unwrap().info;
        assert_eq!(
            (gpt.protocol, gpt.window),
            (Protocol::Responses, Some(400_000))
        );
        assert!(route(&providers, "gpt-b").path.ends_with("/v1"));
        assert_eq!(
            route(&providers, "gpt-b").headers[0],
            ("authorization".into(), "Bearer k".into())
        );
        assert_eq!(
            route(&providers, "claude-a").headers[0],
            ("x-api-key".into(), "k".into())
        );
        server.join().unwrap();
    }

    #[test]
    fn built_in_providers_are_offered_once_a_key_is_saved_or_given() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-providers-builtin-{}", std::process::id())),
        );
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let none = json!({"providers": {}});
        let providers = Providers::given_logged_in(none.clone(), logins.clone());
        let nowhere = [
            ("anthropic".to_owned(), KeyFrom::Nowhere),
            ("google".to_owned(), KeyFrom::Nowhere),
            ("openai".to_owned(), KeyFrom::Nowhere),
            ("openrouter".to_owned(), KeyFrom::Nowhere),
        ];
        assert_eq!(providers.keyed(), nowhere);
        assert!(providers.models().is_empty() && providers.listings().is_empty());
        assert_eq!(providers.problems(), [] as [String; 0]);
        assert_eq!(
            providers.origins("anthropic"),
            ["https://api.anthropic.com:443"]
        );
        assert_eq!(
            providers.origins("openrouter"),
            ["https://openrouter.ai:443"]
        );
        // A file naming one replaces it.
        let file = json!({"providers": {"anthropic": {"baseUrl": "http://127.0.0.1:9",
            "api": "anthropic-messages", "models": [{"id": "own"}]}}});
        let replaced = Providers::given_logged_in(file, logins.clone());
        let keyed: Vec<&str> = replaced.keyed().iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(keyed, ["anthropic", "google", "openai", "openrouter"]);
        assert_eq!(replaced.origins("anthropic"), ["http://127.0.0.1:9"]);
        let references: Vec<String> = replaced.models().iter().map(Model::reference).collect();
        assert_eq!(references, ["anthropic/own"]);
        // Their servers' lists, as read before: nothing is asked of them here.
        let listed = |name: &str, url: &str, data: Value| {
            let list = json!({"url": url, "fetched": super::super::login::now_ms(), "data": data});
            std::fs::create_dir_all(scratch.0.join("models")).unwrap();
            let file = scratch.0.join("models").join(format!("{name}.json"));
            std::fs::write(file, list.to_string()).unwrap();
        };
        let claude = json!([{"id": "claude-a", "display_name": "Claude A",
            "max_input_tokens": 1_000_000}]);
        listed("anthropic", "https://api.anthropic.com/v1/models", claude);
        let routed = json!([{"id": "x/y", "supported_parameters": ["tools"]},
            {"id": "x/chat", "supported_parameters": ["temperature"]}]);
        listed("openrouter", "https://openrouter.ai/api/v1/models", routed);
        let anthropic = providers.origins("anthropic").to_vec();
        super::super::login::save_key(&logins.auth, "anthropic", "sk-ant", &anthropic).unwrap();
        let providers = Providers::given_logged_in(none.clone(), logins.clone());
        assert_eq!(providers.problems(), [] as [String; 0]);
        assert_eq!(
            providers.keyed()[0],
            ("anthropic".to_owned(), KeyFrom::Saved)
        );
        let claude = providers.find("anthropic/claude-a").unwrap();
        assert_eq!(
            (claude.name.as_deref(), claude.info.window),
            (Some("Claude A"), Some(1_000_000))
        );
        assert_eq!(claude.info.protocol, Protocol::Messages);
        let route = route(&providers, "anthropic/claude-a");
        assert_eq!(
            (route.host.as_str(), route.tls),
            ("api.anthropic.com", true)
        );
        assert_eq!(route.headers, [("x-api-key".into(), "sk-ant".into())]);
        assert_eq!(
            providers.source("anthropic/claude-a"),
            Some("Anthropic (/login)")
        );
        assert!(providers.find("openrouter/x/y").is_none());
        // --api-key offers one for the run.
        let mut given = Providers::given_logged_in(none.clone(), logins.clone());
        given.give_key("openrouter", "sk-or");
        assert_eq!(given.keyed()[3], ("openrouter".to_owned(), KeyFrom::Given));
        let routed = given.find("openrouter/x/y").unwrap();
        assert_eq!(routed.info.protocol, Protocol::Completions);
        assert!(given.find("openrouter/x/chat").is_none());
        // OpenAI's list names its models only, embedders among them: models.dev's catalog
        // describes them, and those it names that the catalog describes as taking tools are
        // offered, newest first.
        let named = json!([{"id": "gpt-old", "object": "model", "owned_by": "system"},
            {"id": "text-embedding-9"}, {"id": "gpt-new"}, {"id": "gpt-undescribed"},
            {"id": "gpt-retired"}]);
        listed("openai", "https://api.openai.com/v1/models", named);
        let catalog = json!({
            "other": {"npm": "@ai-sdk/openai-compatible", "api": "http://127.0.0.1:9",
                "models": {"gpt-old": {"tool_call": true}}},
            "openai": {"npm": "@ai-sdk/openai", "env": ["OPENAI_API_KEY"], "models": {
                "gpt-old": {"id": "gpt-old", "tool_call": true, "release_date": "2026-01-01",
                    "limit": {"context": 400_000, "output": 128_000}},
                "gpt-new": {"id": "gpt-new", "name": "GPT New", "tool_call": true,
                    "reasoning": true, "release_date": "2026-11-01",
                    "limit": {"context": 1_050_000, "input": 922_000, "output": 128_000},
                    "reasoning_options": [{"type": "effort", "values": ["low", "high"]}],
                    "cost": {"input": 2.5, "output": 15}},
                "text-embedding-9": {"tool_call": false},
                "gpt-retired": {"tool_call": true, "status": "deprecated"},
                "gpt-unnamed": {"tool_call": true},
                "gpt-elsewhere": {"tool_call": true,
                    "provider": {"npm": "@ai-sdk/openai", "api": "https://example.com/v1"}}}},
            "google": {"npm": "@ai-sdk/google", "env": ["GEMINI_API_KEY"], "models": {
                "gemini-new": {"id": "gemini-new", "name": "Gemini New", "tool_call": true,
                    "reasoning": true, "release_date": "2026-11-18",
                    "limit": {"context": 1_048_576, "output": 65_536},
                    "reasoning_options": [{"type": "effort",
                        "values": ["minimal", "low", "medium", "high"]}]},
                "gemini-old": {"id": "gemini-old", "tool_call": true,
                    "release_date": "2026-02-19"},
                "gemini-embedding-2": {"id": "gemini-embedding-2", "tool_call": false}}}});
        let models_dev = Catalog::new(MODELS_DEV, Some(&logins));
        models_dev.save(&catalog.to_string(), None).unwrap();
        let mut given = Providers::given_logged_in(none.clone(), logins.clone());
        given.give_key("openai", "sk-oa");
        assert_eq!(given.problems(), [] as [String; 0]);
        let openai: Vec<String> = (given.models().iter())
            .filter(|model| model.provider == "openai")
            .map(Model::reference)
            .collect();
        assert_eq!(openai, ["openai/gpt-new", "openai/gpt-old"]);
        let new = given.find("openai/gpt-new").unwrap();
        assert_eq!(
            (new.name.as_deref(), new.info.protocol, new.info.window),
            (Some("GPT New"), Protocol::Responses, Some(1_050_000))
        );
        let to = super::tests::route(&given, "openai/gpt-new");
        assert_eq!((to.host.as_str(), to.tls), ("api.openai.com", true));
        assert!(to.path.ends_with("/v1"), "{}", to.path);
        assert_eq!(
            to.headers,
            [("authorization".into(), "Bearer sk-oa".into())]
        );
        let listing = (given.listings().iter())
            .find(|listing| listing.provider() == "openai")
            .unwrap();
        let kept: Vec<Value> = listing.models().iter().map(|m| m["id"].clone()).collect();
        assert_eq!(kept, ["gpt-new", "gpt-old"]);
        // Gemini's list, at its OpenAI-compatible route, names its models as `models/{id}`;
        // models.dev describes them under a protocol not spoken here, so the route's is taken.
        let named = json!([{"id": "models/gemini-old", "object": "model", "owned_by": "google"},
            {"id": "models/gemini-embedding-2"}, {"id": "models/gemini-new"},
            {"id": "models/imagen-9"}]);
        let url = "https://generativelanguage.googleapis.com/v1beta/openai/models";
        listed("google", url, named);
        let mut gemini = Providers::given_logged_in(none.clone(), logins.clone());
        gemini.give_key("google", "AIza-g");
        assert_eq!(gemini.problems(), [] as [String; 0]);
        let google: Vec<String> = (gemini.models().iter())
            .filter(|model| model.provider == "google")
            .map(Model::reference)
            .collect();
        assert_eq!(google, ["google/gemini-new", "google/gemini-old"]);
        let new = gemini.find("google/gemini-new").unwrap();
        assert_eq!(
            (new.name.as_deref(), new.info.protocol, new.info.window),
            (Some("Gemini New"), Protocol::Completions, Some(1_048_576))
        );
        let to = super::tests::route(&gemini, "google/gemini-new");
        assert_eq!(to.host, "generativelanguage.googleapis.com");
        assert!(to.path.ends_with("/v1beta/openai"), "{}", to.path);
        assert_eq!(
            to.headers,
            [("authorization".into(), "Bearer AIza-g".into())]
        );
        // Without the catalog's copy, nothing is offered, and why is said.
        models_dev.forget();
        assert!(listing.models().is_empty());
        let problem = listing.offers(&[json!({"id": "gpt-new"})]).unwrap_err();
        assert_eq!(
            problem,
            "the model catalog at https://models.dev/api.json, which describes them, is not read"
        );
        // A key saved reads the list again, though a built-in without one had no listing.
        assert!(logins.provided("openai").exists());
        Providers::given_logged_in(none.clone(), logins.clone()).forget_list("openai");
        assert!(!logins.provided("openai").exists());
        // A key saved for another server is not sent to the provider's.
        let local = ["http://127.0.0.1:9".to_owned()];
        super::super::login::save_key(&logins.auth, "anthropic", "sk-local", &local).unwrap();
        let moved = Providers::given_logged_in(none, logins);
        assert!(moved.models().is_empty());
        let problems = moved.problems().join("\n");
        assert!(
            problems.contains("is for http://127.0.0.1:9, not https://api.anthropic.com:443"),
            "{problems}"
        );
    }

    #[test]
    fn a_provider_that_lists_no_models_offers_those_its_server_lists() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-providers-listed-{}", std::process::id())),
        );
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let page = |ids: &[&str], more: bool| {
            let models = ids.iter().map(|id| {
                json!({"id": id, "display_name": id.to_uppercase(), "max_input_tokens": 200_000})
            });
            json!({"data": models.collect::<Vec<_>>(), "has_more": more, "last_id": ids.last()})
        };
        let (base, server) = serve_json(3, move |head, _| {
            match head.split(' ').nth(1).unwrap_or_default() {
                "/v1/models" => json!({"object": "list", "data": [
                    {"id": "gpt-6", "object": "model", "owned_by": "openai"},
                    {"id": "embedder", "supported_parameters": ["dimensions"]},
                ]}),
                "/anthropic/v1/models?limit=1000" => page(&["claude-a"], true),
                "/anthropic/v1/models?limit=1000&after_id=claude-a" => page(&["claude-b"], false),
                _ => json!({}),
            }
        });
        let document = |openai: &str| {
            json!({"providers": {
                "openai": {"baseUrl": openai, "api": "openai-responses", "apiKey": "sk-test",
                    "modelOverrides": {"gpt-6": {"reasoning": true, "contextWindow": 400_000}}},
                "anthropic": {"baseUrl": format!("{base}/anthropic"), "api": "anthropic-messages",
                    "apiKey": "ant-test"},
            }})
        };
        let listed = document(&format!("{base}/v1"));
        let providers = Providers::given_logged_in(listed.clone(), logins.clone());
        assert_eq!(providers.problems(), [] as [String; 0]);
        let references = |providers: &Providers| -> Vec<String> {
            providers.models().iter().map(Model::reference).collect()
        };
        // The test's document has its keys sorted.
        let offered = ["anthropic/claude-a", "anthropic/claude-b", "openai/gpt-6"];
        assert_eq!(references(&providers), offered);
        let gpt = &providers.find("openai/gpt-6").unwrap().info;
        assert_eq!((gpt.reasoning, gpt.window), (true, Some(400_000)));
        let headers = &route(&providers, "gpt-6").headers;
        assert_eq!(
            headers[0],
            ("authorization".into(), "Bearer sk-test".into())
        );
        let claude = providers.find("claude-a").unwrap();
        assert_eq!(
            (claude.name.as_deref(), claude.info.window),
            (Some("CLAUDE-A"), Some(200_000))
        );
        assert_eq!(claude.info.protocol, Protocol::Messages);
        let counts: Vec<_> = providers.listings().iter().map(Listing::offered).collect();
        assert_eq!(counts, [Ok(2), Ok(1)]);
        // Each with its requests' key; Anthropic's in pages.
        let requests = server.join().unwrap();
        let heads: Vec<String> = requests
            .iter()
            .map(|(head, _)| head.to_lowercase())
            .collect();
        assert!(heads[1].contains("after_id=claude-a"));
        assert!(heads[1].contains("x-api-key: ant-test\r\n"));
        assert!(heads[1].contains("anthropic-version: 2023-06-01\r\n"));
        assert!(
            heads[2].starts_with("get /v1/models http/1.1\r\n"),
            "{}",
            heads[2]
        );
        assert!(heads[2].contains("authorization: bearer sk-test\r\n"));
        assert!(heads[2].contains("accept: application/json\r\n"));
        // Saved, the lists are not asked for again, though the server is gone.
        assert!(logins.provided("openai").exists());
        let again = Providers::given_logged_in(listed, logins.clone());
        assert_eq!(
            (references(&again), again.problems()),
            (offered.map(str::to_owned).to_vec(), &[] as &[String])
        );
        // A list read from another URL is not its: it is asked for, and what failed is said, but
        // not asked again until it is forgotten.
        let (other, server) = serve_json(2, |_, _| json!({"error": "no"}));
        let mut moved = Providers::given_logged_in(document(&other), logins.clone());
        let failed = "models: provider openai: lists no models, and asking its server for them \
                      failed: openai's model list has no data";
        assert_eq!(moved.problems(), [failed]);
        moved.reload();
        assert_eq!(moved.problems(), [failed]);
        moved.forget_list("openai");
        moved.reload();
        assert_eq!(moved.problems(), [failed]);
        assert_eq!(server.join().unwrap().len(), 2);
        // A key saved, or logged out of, has what its provider lists asked for again.
        let saved = logins.provided("anthropic");
        assert!(saved.exists());
        let origins = moved.origins("anthropic");
        super::super::login::save_key(&logins.auth, "anthropic", "k", origins).unwrap();
        super::super::login::logout(&logins, "anthropic").unwrap();
        assert!(!saved.exists());
    }

    #[test]
    fn a_catalog_adds_its_providers_and_its_copy_saved_stays_while_its_server_is_down() {
        use std::io::{BufRead, BufReader, Write};
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-providers-catalog-{}", std::process::id())),
        );
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let file = scratch.0.join("models.json");
        let named = json!({"providers": {"named": {"baseUrl": "http://127.0.0.1:9",
            "api": "openai-completions", "models": [{"id": "own"}]}}});
        std::fs::write(&file, named.to_string()).unwrap();
        // In its order, not the keys' sorted one.
        let catalog = r#"{"proxy": {"npm": "@ai-sdk/openai", "api": "http://127.0.0.1:9/v1",
            "models": {"z": {"id": "z", "tool_call": true, "limit": {"context": 400000}},
                "a": {"id": "a", "tool_call": true}}},
            "named": {"npm": "@ai-sdk/openai", "api": "http://127.0.0.1:9/v1",
                "models": {"m": {"id": "m", "tool_call": true}}}}"#;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/api/model-catalog",
            listener.local_addr().unwrap()
        );
        let server = std::thread::spawn(move || {
            let mut heads = Vec::new();
            for answer in [
                format!(
                    "HTTP/1.1 200 OK\r\netag: \"v1\"\r\nconnection: close\r\n\
                     content-length: {}\r\n\r\n{catalog}",
                    catalog.len()
                ),
                "HTTP/1.1 304 Not Modified\r\netag: \"v1\"\r\nconnection: close\r\n\r\n".to_owned(),
            ] {
                let (connection, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(connection);
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line.to_lowercase());
                }
                reader.get_mut().write_all(answer.as_bytes()).unwrap();
                heads.push(head);
            }
            heads
        });
        let sources = || Sources {
            files: vec![file.clone()],
            catalogs: vec![url.clone()],
        };
        let references = |providers: &Providers| -> Vec<String> {
            providers.models().iter().map(Model::reference).collect()
        };
        let providers = Providers::read(sources(), Some(logins.clone()));
        let offered = ["named/own", "proxy/z", "proxy/a"];
        assert_eq!(references(&providers), offered);
        assert_eq!(
            providers.find("proxy/z").unwrap().info.window,
            Some(400_000)
        );
        assert_eq!(providers.problems(), [] as [String; 0]);
        let catalogs = providers.catalogs();
        assert_eq!(catalogs[0].shadowed(), ["named"]);
        assert_eq!(catalogs[0].offered(), Ok(2));
        // Read again by its ETag: unchanged, the copy saved stays.
        catalogs[0].relist().unwrap();
        let heads = server.join().unwrap();
        assert!(heads[0].starts_with("get /api/model-catalog http/1.1\r\n"));
        assert!(!heads[0].contains("if-none-match"));
        assert!(
            heads[1].contains("if-none-match: \"v1\"\r\n"),
            "{}",
            heads[1]
        );
        assert!(catalogs[0].age().unwrap() < Duration::from_secs(60));
        // Its server gone, its models stay offered.
        let again = Providers::read(sources(), Some(logins.clone()));
        assert_eq!(references(&again), offered);
        assert_eq!(again.catalogs()[0].offered(), Ok(2));
        // One never read is not offered, and why is said once. The freed port may be another
        // test's by now, so what failed is checked on the closed port 9.
        catalogs[0].forget();
        let mut gone = Providers::read(sources(), Some(logins.clone()));
        assert_eq!(references(&gone), ["named/own"]);
        let problems = gone.problems().to_vec();
        assert!(
            problems[0].starts_with(&format!("model catalog {url}: ")),
            "{problems:?}"
        );
        assert!(!problems[0].contains("(provider"), "{problems:?}");
        gone.reload();
        assert_eq!(gone.problems().len(), 1);
        assert!(gone.catalogs()[0].offered().is_err());
        let closed = "http://127.0.0.1:9/api/model-catalog";
        let refused = Providers::read(
            Sources {
                files: Vec::new(),
                catalogs: vec![closed.into()],
            },
            Some(logins),
        );
        assert_eq!(refused.problems().len(), 1, "{:?}", refused.problems());
        assert!(
            refused.problems()[0].starts_with(&format!(
                "model catalog {closed}: cannot connect to 127.0.0.1:9"
            )),
            "{:?}",
            refused.problems()
        );
        let bad = Providers::read(
            Sources {
                files: Vec::new(),
                catalogs: vec!["ftp://x".into()],
            },
            None,
        );
        assert_eq!(bad.problems().len(), 1);
    }

    #[test]
    fn a_key_login_saved_is_sent_in_place_of_the_entrys() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-providers-keys-{}", std::process::id())),
        );
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var("SCRAMJET_RS_PROVIDERS_KEYED_TEST", "from-variable") };
        let entry = |key: Option<&str>, api: &str| {
            let mut entry = json!({"baseUrl": "http://127.0.0.1:9", "api": api,
                "models": [{"id": format!("m-{api}")}]});
            if let Some(key) = key {
                entry["apiKey"] = key.into();
            }
            entry
        };
        let document = json!({"providers": {
            "openrouter": entry(None, "openai-completions"),
            "anthropic": entry(Some("literal"), "anthropic-messages"),
            "named": entry(Some("SCRAMJET_RS_PROVIDERS_KEYED_TEST"), "openai-responses"),
            "github-copilot": entry(None, "openai-responses"),
        }});
        let headers = |providers: &Providers, model: &str| route(providers, model).headers;
        let pair = |name: &str, value: &str| (name.to_owned(), value.to_owned());
        let providers = Providers::given_logged_in(document.clone(), logins.clone());
        assert_eq!(
            providers.keyed(),
            [
                ("anthropic".to_owned(), KeyFrom::Entry),
                (
                    "named".to_owned(),
                    KeyFrom::Variable("SCRAMJET_RS_PROVIDERS_KEYED_TEST".to_owned())
                ),
                ("openrouter".to_owned(), KeyFrom::Nowhere),
                ("google".to_owned(), KeyFrom::Nowhere),
                ("openai".to_owned(), KeyFrom::Nowhere),
            ]
        );
        assert!(headers(&providers, "openrouter/m-openai-completions").is_empty());
        assert_eq!(
            headers(&providers, "anthropic/m-anthropic-messages"),
            [pair("x-api-key", "literal")]
        );
        assert_eq!(providers.origins("openrouter"), ["http://127.0.0.1:9"]);
        let login = |name: &str, key: &str| {
            let origins = providers.origins(name);
            super::super::login::save_key(&logins.auth, name, key, origins).unwrap()
        };
        login("openrouter", "sk-or");
        login("anthropic", "sk-ant");
        // A saved key that names a variable is read from it, as the entry's is.
        login("named", "SCRAMJET_RS_PROVIDERS_KEYED_TEST");
        let providers = Providers::given_logged_in(document.clone(), logins.clone());
        let saved: Vec<KeyFrom> = providers
            .keyed()
            .iter()
            .map(|(_, from)| from.clone())
            .collect();
        let nowhere = KeyFrom::Nowhere;
        assert_eq!(
            saved,
            [
                KeyFrom::Saved,
                KeyFrom::Saved,
                KeyFrom::Saved,
                nowhere.clone(),
                nowhere
            ]
        );
        assert_eq!(
            headers(&providers, "openrouter/m-openai-completions"),
            [pair("authorization", "Bearer sk-or")]
        );
        assert_eq!(
            headers(&providers, "anthropic/m-anthropic-messages"),
            [pair("x-api-key", "sk-ant")]
        );
        assert_eq!(
            headers(&providers, "named/m-openai-responses"),
            [pair("authorization", "Bearer from-variable")]
        );
        // A provider `/login` logs in to keeps its login: this machine may take its token.
        let copilot = route(&providers, "github-copilot/m-openai-responses");
        assert!(copilot.headers.is_empty() && copilot.login.is_some());
        assert_eq!(providers.problems(), [] as [String; 0]);
        // `--api-key`'s goes, as it is, to its provider alone, and stays when read again.
        let mut given = providers.clone();
        given.give_key("named", "SCRAMJET_RS_PROVIDERS_KEYED_TEST");
        let mut again = Providers::given_logged_in(document.clone(), logins.clone());
        again.keep_given_key(&given);
        for providers in [&given, &again] {
            assert_eq!(
                headers(providers, "named/m-openai-responses"),
                [pair(
                    "authorization",
                    "Bearer SCRAMJET_RS_PROVIDERS_KEYED_TEST"
                )]
            );
            assert_eq!(
                headers(providers, "anthropic/m-anthropic-messages"),
                [pair("x-api-key", "sk-ant")]
            );
            let from = providers.keyed().iter().map(|(_, from)| from.clone());
            let from: Vec<KeyFrom> = from.collect();
            let nowhere = KeyFrom::Nowhere;
            assert_eq!(
                from,
                [
                    KeyFrom::Saved,
                    KeyFrom::Given,
                    KeyFrom::Saved,
                    nowhere.clone(),
                    nowhere
                ]
            );
        }
        // A saved key goes only to the servers it was saved for; one saved before keys named
        // them goes nowhere until saved again.
        let mut moved = document.clone();
        moved["providers"]["openrouter"]["baseUrl"] = "http://127.0.0.1:10".into();
        moved["providers"]["openrouter"]["models"][0]["baseUrl"] = "http://127.0.0.1:9/v2".into();
        let models = moved["providers"]["openrouter"]["models"]
            .as_array_mut()
            .unwrap();
        models.push(json!({"id": "elsewhere"}));
        let old = json!({"type": "api_key", "key": "sk-ant"});
        super::super::login::save(&logins.auth, "anthropic", old).unwrap();
        let providers = Providers::given_logged_in(moved, logins.clone());
        assert_eq!(
            headers(&providers, "openrouter/m-openai-completions"),
            [pair("authorization", "Bearer sk-or")]
        );
        assert!(providers.find("openrouter/elsewhere").is_none());
        let problems = providers.problems();
        assert_eq!(
            [&problems[0], &problems[1]],
            [
                "models: provider anthropic: model m-anthropic-messages: the API key /login saved \
                 for anthropic names no server, as keys saved before 0.98.0 do; /login again to \
                 send it to http://127.0.0.1:9",
                "models: provider openrouter: model elsewhere: the API key /login saved for \
                 openrouter is for http://127.0.0.1:9, not http://127.0.0.1:10; /login again to \
                 send it to http://127.0.0.1:10"
            ]
        );
        assert_eq!(problems.len(), 2);
        // A later file's entry of the same name cannot take the key elsewhere, and /login
        // would not change that; an entry's modelOverrides name its servers too.
        let (first, later) = (scratch.0.join("first.json"), scratch.0.join("later.json"));
        std::fs::write(&first, document.to_string()).unwrap();
        let other = json!({"providers": {"openrouter": {"baseUrl": "http://127.0.0.1:11",
            "api": "openai-completions", "models": [{"id": "other"}]}}});
        std::fs::write(&later, other.to_string()).unwrap();
        let providers = Providers::read(vec![first, later], Some(logins.clone()));
        assert!(providers.find("openrouter/m-openai-completions").is_some());
        assert!(providers.find("openrouter/other").is_none());
        assert_eq!(providers.origins("openrouter"), ["http://127.0.0.1:9"]);
        assert!(
            (providers.problems().iter()).any(|problem| problem.ends_with(
                "model other: the API key /login saved for openrouter is for http://127.0.0.1:9, \
                 not http://127.0.0.1:11; /login saves it only for the servers the first entry \
                 naming openrouter gives, and http://127.0.0.1:11 is not one"
            )),
            "{:?}",
            providers.problems()
        );
        let overridden = json!({"baseUrl": "https://a.example",
            "modelOverrides": {"m": {"baseUrl": "https://b.example:8443/v1"}}});
        assert_eq!(
            origins(&overridden),
            ["https://a.example:443", "https://b.example:8443"]
        );
    }

    #[test]
    #[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
    fn credential_files_come_from_files_and_stay_while_one_has_problems() {
        let scratch = ScratchDir::new(std::env::temp_dir().join(format!(
            "scramjet-rs-providers-credentials-{}",
            std::process::id()
        )));
        let file = scratch.0.join("models.json");
        let write = |text: &str| std::fs::write(&file, text).unwrap();
        let program = scratch.0.join("list");
        let listed = r#"{"version": 1, "credentialFiles": ["/from/program"]}"#;
        crate::agent::tests::script(&program, &format!("echo '{listed}'"));
        write(
            r#"{"credentialFiles": ["~/.config/x", "~", " keys ", "/abs/token"],
                "providersCommand": ["./list"]}"#,
        );
        let mut providers = Providers::read(vec![file.clone()], None);
        let home = std::env::home_dir().unwrap();
        let mut named = vec![
            home.join(".config/x"),
            home.clone(),
            scratch.0.join("keys"),
            PathBuf::from("/abs/token"),
        ];
        named.sort();
        assert_eq!(providers.problems(), [] as [String; 0]);
        assert_eq!(providers.credential_files(), named);

        // A file that cannot be read, or names them wrongly, may name them again once fixed.
        write("{");
        providers.reload();
        assert_eq!(providers.problems().len(), 1);
        assert_eq!(providers.credential_files(), named);
        for wrong in [r#""keys""#, r#"["keys", 1]"#, r#"[" "]"#] {
            write(&format!(r#"{{"credentialFiles": {wrong}}}"#));
            providers.reload();
            let problem = format!(
                "{}: credentialFiles must be a list of files and directories",
                file.display()
            );
            assert_eq!(providers.problems(), [problem]);
            assert_eq!(providers.credential_files(), named);
        }
        write(r#"{"credentialFiles": ["keys"]}"#);
        providers.reload();
        assert_eq!(providers.credential_files(), [scratch.0.join("keys")]);
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
        let providers = Providers::read(vec![file], None);
        let ids: Vec<&str> = providers.models().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["z", "a"]);
    }

    #[test]
    fn only_http_and_https_urls_with_a_host_are_taken() {
        let parsed = |url: &str| {
            base_url(url).map(|url| (url.tls, url.host, url.port, url.authority, url.path))
        };
        assert_eq!(
            parsed("HTTP://h:8/a/b/"),
            Ok((false, "h".into(), 8, "h:8".into(), "/a/b".into()))
        );
        assert_eq!(
            parsed("https://api.example.com/v1"),
            Ok((
                true,
                "api.example.com".into(),
                443,
                "api.example.com".into(),
                "/v1".into()
            ))
        );
        assert_eq!(
            parsed("HTTPS://[::1]:8443"),
            Ok((true, "::1".into(), 8443, "[::1]:8443".into(), "".into()))
        );
        for url in [
            "http://",
            "http://:80",
            "http://u:p@h",
            "http://h?q",
            "http://h:x",
            "https://",
            "https://u@h/v1",
            "ftp://h",
            "h:80",
        ] {
            assert!(parsed(url).is_err(), "{url}");
        }
    }

    #[test]
    fn files_declare_logins_that_every_files_providers_take() {
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-rs-declared-{}", std::process::id())),
        );
        let package = scratch.0.join("package");
        std::fs::create_dir_all(&package).unwrap();
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let write = |path: &Path, document: Value| std::fs::write(path, document.to_string());
        let own = scratch.0.join("models.json");
        // The user's file names the provider before the package declares its login.
        let fixed = json!({"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
            "models": [{"id": "fixed"}]});
        write(&own, json!({"providers": {"acme": fixed}})).unwrap();
        let declared = package.join("models.json");
        write(
            &declared,
            json!({"logins": {
                "acme": {"command": ["bin/login", "--direct"]},
                "github-copilot": {"command": ["bin/login"]},
                "nameless": {"name": 3, "command": ["bin/login"]},
                "bare": {},
                "wordy": {"command": ["bin/login", 2]},
                "other": {"name": "Other Co", "command": ["other-login"]},
            }}),
        )
        .unwrap();
        let again = scratch.0.join("again.json");
        write(
            &again,
            json!({"logins": {"acme": {"command": ["elsewhere"]}}}),
        )
        .unwrap();
        super::super::login::save(&logins.auth, "other", json!({"type": "oauth"})).unwrap();
        let listed = scratch.0.join("listed.json");
        write(&listed, json!({"logins": [{"acme": {}}]})).unwrap();
        let files = vec![own, declared.clone(), again.clone(), listed.clone()];
        let providers = Providers::read(files, Some(logins.clone()));
        let programs = providers.login_programs();
        let shown: Vec<(&str, &str)> = (programs.iter())
            .map(|program| (program.provider.as_str(), program.name.as_str()))
            .collect();
        assert_eq!(shown, [("acme", "acme"), ("other", "Other Co")]);
        let (declared, again, listed) = (declared.display(), again.display(), listed.display());
        assert_eq!(
            providers.problems(),
            // In the order written, which json! sorts.
            [
                format!(
                    "{declared}: login bare: command must be a program and its arguments, as a \
                     list of strings"
                ),
                format!(
                    "{declared}: login github-copilot: is logged in to already; not offered here"
                ),
                format!("{declared}: login nameless: name must be a text"),
                format!(
                    "{declared}: login wordy: command must be a program and its arguments, as a \
                     list of strings"
                ),
                format!("{again}: login acme: is logged in to already; not offered here"),
                format!("{listed}: logins must be an object"),
                "other (/login): no models were read for the login yet; they are asked for \
                 again in the full screen, or /login again"
                    .to_owned(),
            ]
        );
        let fixed = route(&providers, "acme/fixed");
        let login = fixed.login.unwrap();
        assert_eq!(
            (login.provider.as_str(), &login.file),
            ("acme", &logins.auth)
        );
        assert_eq!(login.program.as_ref(), Some(&programs[0]));
        // A login, it takes no key.
        assert!(!providers.keyed().iter().any(|(name, _)| name == "acme"));
    }
}
