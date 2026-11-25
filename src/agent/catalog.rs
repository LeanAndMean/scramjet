//! Providers from a model catalog: a URL (`modelCatalogs` in the settings) serving models.dev's
//! `api.json` shape, an object of providers by id, each with `npm` (the AI SDK package, which
//! names its protocol), `api` (its base URL), `env` (the variables its key may be in) and
//! `models` by id, each with `limit`, `cost` (dollars a million), `modalities`, `reasoning`,
//! `reasoning_options`, `tool_call` and `status`. Each provider becomes a `models.json` entry,
//! read as the files' are. The catalog's last copy is saved in the state directory, so that its
//! models stay offered while its server is down, and read again in the background as it ages
//! (`agent_cli::login`) and as `/model` opens, cheaply by its `ETag`.

use super::client;
use super::login;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// How long reading a catalog not saved yet may hold up reading the providers.
const WAIT: Duration = Duration::from_secs(5);
/// How long reading it again in the background may take.
const RELIST_WAIT: Duration = Duration::from_secs(30);
/// How long reading it again may hold up a list about to be shown.
const SHOWN_WAIT: Duration = Duration::from_secs(2);
/// What a catalog gives beyond models.dev's shape, under each model: `cost_by_header`
/// (`{header, values: {VALUE: cost}}`, read as `pricing`'s `header` and `byHeader`) and
/// `unsupported_request_fields` (body fields its server rejects).
const EXTENSION: &str = "x-gateway";
/// models.dev's whole catalog is 6.6 MB (2026-11-16).
const CATALOG_BYTES: usize = 64 << 20;

/// A catalog's URL, and where its last copy is saved.
#[derive(Clone, Debug)]
pub struct Catalog {
    url: String,
    /// `None` without a state directory: the copy is then kept only while the process runs.
    file: Option<PathBuf>,
    /// How many models it offered when the providers were read, or what kept it from offering any.
    pub(super) offered: Result<usize, String>,
    /// Its providers not offered then, as a file, a login or a catalog before named them.
    pub(super) shadowed: Vec<String>,
}

/// What a catalog offers: its providers as `models.json` entries, in its order, and those it
/// lists that cannot be offered, each with why.
#[derive(Debug, Default, PartialEq)]
pub struct Offered {
    pub providers: Vec<(String, Value)>,
    pub skipped: Vec<(String, String)>,
}

impl Offered {
    /// How many models its providers offer.
    pub fn models(&self) -> usize {
        let models = self.providers.iter().map(|(_, entry)| &entry["models"]);
        models.filter_map(Value::as_array).map(Vec::len).sum()
    }
}

impl Catalog {
    /// The catalog at `url`, its copy saved under `logins`' state directory.
    pub fn new(url: &str, logins: Option<&login::Logins>) -> Self {
        let file = logins.map(|logins| {
            let directory = logins.lists.with_file_name("catalogs");
            directory.join(format!("{}.json", file_name(url)))
        });
        Self {
            url: url.to_owned(),
            file,
            offered: Ok(0),
            shadowed: Vec::new(),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// How many models it offered when the providers were read, or what kept it from offering any.
    pub fn offered(&self) -> Result<usize, &str> {
        self.offered.as_ref().copied().map_err(String::as_str)
    }

    /// Its providers not offered when the providers were read, as a `models.json`, a login or a
    /// catalog before named them.
    pub fn shadowed(&self) -> &[String] {
        &self.shadowed
    }

    /// What tells it apart from the logins' and providers' lists kept current, which have no
    /// space in their keys.
    pub fn key(&self) -> String {
        format!("catalog {}", self.url)
    }

    fn read_saved(&self) -> Option<Value> {
        let text = std::fs::read(self.file.as_ref()?).ok()?;
        let saved: Value = serde_json::from_slice(&text).ok()?;
        (saved["url"] == self.url.as_str() && saved["body"].is_string()).then_some(saved)
    }

    /// The catalog's text as last read; `None` when none is saved.
    pub(super) fn saved(&self) -> Option<String> {
        Some(self.read_saved()?["body"].as_str()?.to_owned())
    }

    /// How long ago the copy saved was read; `None` when there is none.
    pub fn age(&self) -> Option<Duration> {
        let fetched = self.read_saved()?["fetched"].as_f64()?;
        Some(Duration::from_millis(
            (login::now_ms() - fetched).max(0.0) as u64
        ))
    }

    /// What the copy saved offers; nothing when there is none or it is no catalog.
    pub fn offers(&self) -> Offered {
        let saved = self.saved().unwrap_or_default();
        offered(&saved).unwrap_or_default()
    }

    /// The models the copy saved offers, each named `provider/id`, for saying what changed.
    pub fn models(&self) -> Vec<Value> {
        let offered = self.offers();
        let models = offered.providers.iter().flat_map(|(provider, entry)| {
            let models = entry["models"].as_array().into_iter().flatten();
            models.map(move |model| {
                let id = model["id"].as_str().unwrap_or_default();
                json!({"id": format!("{provider}/{id}")})
            })
        });
        models.collect()
    }

    /// Reads it now, waiting at most 5 seconds, and saves it.
    pub fn read_now(&self) -> Result<String, String> {
        let (body, etag) = self
            .fetch(WAIT, None)?
            .ok_or("its server sent no catalog")?;
        let _ = self.save(&body, etag.as_deref());
        Ok(body)
    }

    /// Reads it again and saves it in place of the copy saved, which stays when that fails;
    /// asks with the copy's `ETag`, so that an unchanged one is not sent again.
    pub fn relist(&self) -> Result<(), String> {
        self.relist_within(RELIST_WAIT)
    }

    /// As `relist`, waiting at most 2 seconds, for a list of models about to be shown.
    pub fn relist_for_showing(&self) -> Result<(), String> {
        self.relist_within(SHOWN_WAIT)
    }

    fn relist_within(&self, wait: Duration) -> Result<(), String> {
        let saved = self.read_saved();
        let etag = saved.as_ref().and_then(|saved| saved["etag"].as_str());
        let old = saved.as_ref().and_then(|saved| saved["body"].as_str());
        match (self.fetch(wait, etag)?, old) {
            (Some((body, etag)), _) => self.save(&body, etag.as_deref()),
            (None, Some(old)) => self.save(old, etag),
            (None, None) => Err("its server sent no catalog".to_owned()),
        }
    }

    /// Removes the copy saved.
    pub fn forget(&self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }

    pub(super) fn save(&self, body: &str, etag: Option<&str>) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut saved = json!({"url": self.url, "fetched": login::now_ms(), "body": body});
        if let Some(etag) = etag {
            saved["etag"] = json!(etag);
        }
        login::save_list(file, &saved)
    }

    /// The catalog's text and `ETag`, or `None` when the server says the copy whose `ETag` is
    /// `etag` is current; a catalog that is not one is refused, so the copy saved stays.
    fn fetch(
        &self,
        wait: Duration,
        etag: Option<&str>,
    ) -> Result<Option<(String, Option<String>)>, String> {
        let what = format!("the model catalog at {}", self.url);
        let reading = async {
            let mut headers = vec![("accept", "application/json")];
            if let Some(etag) = etag {
                headers.push(("if-none-match", etag));
            }
            let fetched =
                client::fetch_within(&what, "GET", &self.url, &headers, None, CATALOG_BYTES, wait);
            let fetched = fetched
                .await
                .map_err(|failure| (failure.message).replace(&format!(" (provider {what})"), ""))?;
            if fetched.status == 304 && etag.is_some() {
                return Ok(None);
            }
            if !(200..300).contains(&fetched.status) {
                let said = fetched.said();
                let said = said.trim().chars().take(400).collect::<String>();
                return Err(format!("{what} answered HTTP {}: {said}", fetched.status));
            }
            let body =
                String::from_utf8(fetched.body).map_err(|_| format!("{what} is not UTF-8 text"))?;
            offered(&body).map_err(|problem| format!("{what} {problem}"))?;
            Ok(Some((body, fetched.etag)))
        };
        std::thread::scope(|scope| {
            let reading = scope.spawn(|| login::blocking(reading));
            let failed = || Err(format!("reading {what} failed"));
            reading.join().unwrap_or_else(|_| failed())
        })
    }
}

/// Whether `url` can be a catalog's: an `http://` or `https://` URL with a host.
pub fn check_url(url: &str) -> Result<(), String> {
    super::providers::base_url(url).map(drop)
}

/// An object's entries in the order written.
struct Keyed<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Keyed<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Entries<T> {
            type Value = Keyed<T>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Keyed<T>, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Keyed(entries))
            }
        }
        deserializer.deserialize_map(Entries(std::marker::PhantomData))
    }
}

/// A catalog's provider; one of another shape is skipped, not the whole catalog.
#[derive(Deserialize)]
#[serde(untagged)]
enum Listed {
    Provider {
        #[serde(default)]
        npm: Value,
        #[serde(default)]
        api: Value,
        #[serde(default)]
        env: Value,
        models: Keyed<Value>,
    },
    Other(#[allow(dead_code)] Value),
}

/// The protocols of the AI SDK's packages, with the base URL each takes without `api`.
fn protocol(npm: &str) -> Option<(&'static str, Option<&'static str>)> {
    match npm {
        "@ai-sdk/anthropic" => Some(("anthropic-messages", Some("https://api.anthropic.com/v1"))),
        "@ai-sdk/openai" => Some(("openai-responses", Some("https://api.openai.com/v1"))),
        "@ai-sdk/openai-compatible" => Some(("openai-completions", None)),
        _ => None,
    }
}

/// `api` as a `models.json`'s `baseUrl` for `protocol`: Messages requests go to
/// `{baseUrl}/v1/messages`, where the AI SDK's go to `{api}/messages`.
fn base_url(api: &str, protocol: &str) -> String {
    let api = api.trim_end_matches('/');
    match protocol {
        "anthropic-messages" => api.strip_suffix("/v1").unwrap_or(api).to_owned(),
        _ => api.to_owned(),
    }
}

/// What the catalog `text` offers; an error when it is not one.
pub(super) fn offered(text: &str) -> Result<Offered, String> {
    let providers: Keyed<Listed> = serde_json::from_str(text)
        .map_err(|error| format!("is not a model catalog (models.dev's shape): {error}"))?;
    let mut offered = Offered::default();
    for (id, listed) in providers.0 {
        match provider(&id, listed) {
            Ok(entry) => offered.providers.push((id, entry)),
            Err(why) => offered.skipped.push((id, why)),
        }
    }
    Ok(offered)
}

/// The models the catalog `text` lists of its provider `id`, as `models.json` models, newest
/// first by release date, but those its own server or protocol routes elsewhere: what describes
/// a provider's own list, which names its models only.
pub(super) fn described(text: &str, id: &str) -> Result<Vec<Value>, String> {
    let mut providers: Map<String, Value> = serde_json::from_str(text)
        .map_err(|error| format!("is not a model catalog (models.dev's shape): {error}"))?;
    let listed = (providers.remove(id)).ok_or_else(|| format!("lists no provider {id}"))?;
    let released: Vec<(String, String)> = (listed["models"].as_object().into_iter().flatten())
        .map(|(key, model)| {
            let id = model["id"].as_str().unwrap_or(key).to_owned();
            let date = model["release_date"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            (id, date)
        })
        .collect();
    let released = |model: &Value| {
        let found = released.iter().find(|(id, _)| model["id"] == id.as_str());
        found.map_or("", |(_, date)| date.as_str()).to_owned()
    };
    let listed = Listed::deserialize(listed).map_err(|error| format!("lists {id} as {error}"))?;
    let entry = provider(id, listed).map_err(|why| format!("lists {id}, but {why}"))?;
    let models = entry["models"].as_array().into_iter().flatten();
    let mut models: Vec<Value> = (models
        .filter(|m| m.get("baseUrl").is_none() && m.get("api").is_none()))
    .cloned()
    .collect();
    models.sort_by_cached_key(|model| std::cmp::Reverse(released(model)));
    Ok(models)
}

/// A catalog's provider as a `models.json` entry, or why it cannot be offered.
fn provider(id: &str, listed: Listed) -> Result<Value, String> {
    let Listed::Provider {
        npm,
        api,
        env,
        models,
    } = listed
    else {
        return Err("it is not a provider with models".to_owned());
    };
    if id.trim().is_empty() {
        return Err("it has no id".to_owned());
    }
    let npm = npm.as_str().unwrap_or_default();
    let (protocol, default) =
        protocol(npm).ok_or_else(|| format!("its protocol ({npm}) is not supported yet"))?;
    let api = api.as_str().or(default);
    // models.dev's `${VARIABLE}`s are the user's to fill in.
    if api.is_some_and(|api| api.contains("${")) {
        return Err("its base URL has variables to fill in".to_owned());
    }
    let mut entry = Map::new();
    entry.insert("api".into(), json!(protocol));
    if let Some(api) = api {
        entry.insert("baseUrl".into(), json!(base_url(api, protocol)));
    }
    // A variable named but not set would be sent as the key itself.
    let variables = env
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    let set = variables
        .into_iter()
        .find(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if let Some(variable) = set {
        entry.insert("apiKey".into(), json!(variable));
    }
    let mut offered = Vec::new();
    for (key, listed) in models.0 {
        let model = model(&key, &listed, protocol);
        let model = model.filter(|model| api.is_some() || model.get("baseUrl").is_some());
        offered.extend(model);
    }
    if offered.is_empty() {
        return Err("it lists no models that take tools".to_owned());
    }
    entry.insert("models".into(), Value::Array(offered));
    Ok(Value::Object(entry))
}

/// A catalog's model, listed as `key`, as a `models.json` model of a provider whose protocol is
/// `protocol`; `None` for one that takes no tools, is deprecated, or whose own protocol or base
/// URL cannot be used.
fn model(key: &str, listed: &Value, protocol: &str) -> Option<Value> {
    let id = listed["id"].as_str().unwrap_or(key);
    if id.trim().is_empty() || listed["tool_call"] == false || listed["status"] == "deprecated" {
        return None;
    }
    let mut entry = json!({"id": id});
    let own = &listed["provider"];
    let mut protocol = protocol;
    if let Some(npm) = own["npm"].as_str() {
        protocol = self::protocol(npm)?.0;
        entry["api"] = json!(protocol);
    }
    if let Some(api) = own["api"].as_str() {
        if api.contains("${") {
            return None;
        }
        entry["baseUrl"] = json!(base_url(api, protocol));
    }
    if let Some(name) = listed["name"].as_str() {
        entry["name"] = json!(name);
    }
    let reasoning = listed["reasoning"] == true;
    entry["reasoning"] = json!(reasoning);
    let images = (listed["modalities"]["input"].as_array())
        .is_some_and(|input| input.iter().any(|kind| kind == "image"));
    entry["input"] = match images {
        true => json!(["text", "image"]),
        false => json!(["text"]),
    };
    let number = |value: &Value| value.as_u64().filter(|n| *n > 0);
    let limit = &listed["limit"];
    for (field, value) in [
        ("contextWindow", &limit["context"]),
        ("maxTokens", &limit["output"]),
        ("maxInputTokens", &limit["input"]),
    ] {
        if let Some(value) = number(value) {
            entry[field] = json!(value);
        }
    }
    let options = listed["reasoning_options"].as_array().into_iter().flatten();
    let efforts = options
        .filter(|option| option["type"] == "effort")
        .find_map(|option| option["values"].as_array());
    if reasoning && let Some(efforts) = efforts {
        let efforts: Vec<&str> = efforts.iter().filter_map(Value::as_str).collect();
        entry["thinkingLevelMap"] = super::providers::level_map(&efforts);
    }
    let messages = protocol == "anthropic-messages";
    if let Some(cost) = cost(&listed["cost"], messages) {
        entry["cost"] = cost;
    }
    let extension = &listed[EXTENSION];
    let by_header = &extension["cost_by_header"];
    if let Some(header) = by_header["header"]
        .as_str()
        .filter(|h| !h.trim().is_empty())
    {
        let values = by_header["values"].as_object().into_iter().flatten();
        let values: Map<String, Value> = values
            .filter_map(|(value, rates)| Some((value.clone(), cost(rates, messages)?)))
            .collect();
        if !values.is_empty() {
            if !entry["cost"].is_object() {
                entry["cost"] = json!({});
            }
            entry["cost"]["header"] = json!(header);
            entry["cost"]["byHeader"] = Value::Object(values);
        }
    }
    // Of the fields a server rejects, scramjet-rs sends only this one, and only when allowed.
    let unsupported = extension["unsupported_request_fields"].as_array();
    if unsupported.is_some_and(|fields| fields.iter().any(|f| f == "prompt_cache_retention")) {
        entry["compat"] = json!({"supportsLongCacheRetention": false});
    }
    Some(entry)
}

/// models.dev's `cost` as Scramjet's, with the rates of its least context tier for long
/// prompts (`tiers`, else `context_over_200k`). Messages requests ask for an hour's caching,
/// which Anthropic prices at twice the input where five minutes' is 1.25 times it: a catalog's
/// `cache_write` at that five minutes' rate is taken as the hour's.
fn cost(cost: &Value, messages: bool) -> Option<Value> {
    let mut rates = rates(cost, messages)?;
    let tiers = cost["tiers"].as_array().into_iter().flatten();
    let tiers = tiers.filter(|tier| tier["tier"]["type"] == "context");
    let tiers = tiers.filter_map(|tier| Some((tier["tier"]["size"].as_u64()?, tier)));
    let long = match tiers.min_by_key(|(size, _)| *size) {
        Some((size, tier)) => Some((size, tier)),
        None => {
            (cost["context_over_200k"].is_object()).then(|| (200_000, &cost["context_over_200k"]))
        }
    };
    if let Some((above, tier)) = long
        && let Some(mut long) = self::rates(tier, messages)
    {
        long["above"] = json!(above);
        rates["longContext"] = long;
    }
    Some(rates)
}

fn rates(cost: &Value, messages: bool) -> Option<Value> {
    let rate = |key: &str| {
        cost[key]
            .as_f64()
            .filter(|rate| rate.is_finite() && *rate >= 0.0)
    };
    let mut rates = Map::new();
    for (name, key) in [
        ("input", "input"),
        ("output", "output"),
        ("cacheRead", "cache_read"),
        ("cacheWrite", "cache_write"),
    ] {
        if let Some(rate) = rate(key) {
            rates.insert(name.into(), json!(rate));
        }
    }
    if messages
        && let (Some(input), Some(write)) = (rate("input"), rate("cache_write"))
        && (write - input * 1.25).abs() <= input * 1e-6
    {
        rates.insert("cacheWrite".into(), json!(input * 2.0));
    }
    (!rates.is_empty()).then_some(Value::Object(rates))
}

/// `url` as a file name, as `listed::file_name` makes one, cut to a length every file system
/// takes, with a digest of the whole URL.
fn file_name(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let name = super::listed::file_name(url);
    if name.len() <= 120 {
        return name;
    }
    let digest = Sha256::digest(url.as_bytes());
    let digest: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{}-{digest}", &name[..100])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_offers_its_providers_in_their_protocols() {
        let catalog = r#"{
            "proxy-messages": {"id": "proxy-messages", "npm": "@ai-sdk/anthropic",
                "api": "http://127.0.0.1:9/v1", "env": [], "name": "Proxy", "models": {
                "claude-b": {"id": "claude-b", "name": "Claude B", "reasoning": true,
                    "tool_call": true, "reasoning_options": [{"type": "toggle"},
                        {"type": "effort", "values": ["none", "low", "high", "max"]}],
                    "modalities": {"input": ["text", "image", "pdf"], "output": ["text"]},
                    "limit": {"context": 1000000, "output": 128000},
                    "cost": {"input": 5, "output": 25, "cache_read": 0.5, "cache_write": 6.25,
                        "tiers": [{"input": 10, "output": 37.5, "tier": {"type": "context",
                            "size": 200000}}]}},
                "claude-a": {"id": "claude-a", "tool_call": true, "reasoning": false,
                    "limit": {"context": 200000, "output": 0}, "status": "beta"},
                "old": {"id": "old", "tool_call": true, "status": "deprecated"},
                "embed": {"id": "embed", "tool_call": false}}},
            "proxy-responses": {"npm": "@ai-sdk/openai", "api": "http://127.0.0.1:9/v1/",
                "env": ["SCRAMJET_TEST_UNSET_KEY"], "models": {
                "gpt": {"id": "gpt", "tool_call": true, "reasoning": true,
                    "limit": {"context": 400000, "output": 128000, "input": 272000},
                    "cost": {"input": 2, "output": 12, "context_over_200k": {"input": 4}}}}},
            "elsewhere": {"npm": "@ai-sdk/openai-compatible", "models": {
                "m": {"id": "m", "tool_call": true}}},
            "routed": {"npm": "@ai-sdk/openai-compatible", "api": "https://r.example/v1",
                "models": {
                    "own": {"id": "own", "tool_call": true,
                        "provider": {"npm": "@ai-sdk/anthropic", "api": "https://a.example/v1"}},
                    "filled": {"id": "filled", "tool_call": true,
                        "provider": {"api": "${ACCOUNT}/v1"}},
                    "gateway": {"id": "gateway", "tool_call": true,
                        "provider": {"npm": "@ai-sdk/gateway"}}}},
            "gateway": {"npm": "@ai-sdk/gateway", "models": {}},
            "broken": {"npm": "@ai-sdk/openai", "models": []}
        }"#;
        let offered = offered(catalog).unwrap();
        let names: Vec<&str> = offered.providers.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["proxy-messages", "proxy-responses", "routed"]);
        let skipped: Vec<(&str, &str)> = (offered.skipped.iter())
            .map(|(name, why)| (name.as_str(), why.as_str()))
            .collect();
        assert_eq!(
            skipped,
            [
                ("elsewhere", "it lists no models that take tools"),
                (
                    "gateway",
                    "its protocol (@ai-sdk/gateway) is not supported yet"
                ),
                ("broken", "it is not a provider with models"),
            ]
        );
        assert_eq!(offered.models(), 4);
        let (_, messages) = &offered.providers[0];
        // In the catalog's order, under Messages from the root, an hour's cache writes at twice
        // the input; Anthropic's levels as it lists them, `none` for off.
        assert_eq!(
            messages,
            &json!({"api": "anthropic-messages", "baseUrl": "http://127.0.0.1:9", "models": [
                {"id": "claude-b", "name": "Claude B", "reasoning": true,
                    "input": ["text", "image"], "contextWindow": 1_000_000, "maxTokens": 128_000,
                    "thinkingLevelMap": {"off": "none", "minimal": null, "low": "low",
                        "medium": null, "high": "high", "xhigh": null, "max": "max"},
                    "cost": {"input": 5.0, "output": 25.0, "cacheRead": 0.5, "cacheWrite": 10.0,
                        "longContext": {"input": 10.0, "output": 37.5, "above": 200_000}}},
                {"id": "claude-a", "reasoning": false, "input": ["text"],
                    "contextWindow": 200_000}]})
        );
        // A variable not set is no key.
        let (_, responses) = &offered.providers[1];
        assert_eq!(responses.get("apiKey"), None);
        assert_eq!(responses["baseUrl"], "http://127.0.0.1:9/v1");
        assert_eq!(
            responses["models"][0]["cost"],
            json!({"input": 2.0, "output": 12.0, "longContext": {"input": 4.0, "above": 200_000}})
        );
        assert_eq!(responses["models"][0]["maxInputTokens"], 272_000);
        let (_, routed) = &offered.providers[2];
        assert_eq!(
            routed["models"],
            json!([{"id": "own", "api": "anthropic-messages", "baseUrl": "https://a.example",
                "reasoning": false, "input": ["text"]}])
        );
        assert!(super::offered("[]").is_err());
        assert!(super::offered("{\"a\": 1}").unwrap().providers.is_empty());
    }

    #[test]
    fn the_gateway_extension_gives_rates_by_header_and_rejected_fields() {
        let catalog = r#"{"gw": {"npm": "@ai-sdk/anthropic", "models": {
            "claude": {"id": "claude", "tool_call": true,
                "cost": {"input": 3, "output": 15},
                "provider": {"npm": "@ai-sdk/anthropic", "api": "http://127.0.0.1:9/v1"},
                "x-gateway": {"cost_by_header": {"header": "X-LLM-Backend", "values": {
                    "a": {"input": 3, "output": 15, "cache_write": 3.75,
                        "tiers": [{"input": 6, "tier": {"type": "context", "size": 200000}}]},
                    "b": {"input": 4},
                    "broken": {"input": "free"}}}}},
            "gpt": {"id": "gpt", "tool_call": true,
                "provider": {"npm": "@ai-sdk/openai", "api": "http://127.0.0.1:9/v1"},
                "x-gateway": {"unsupported_request_fields":
                    ["prompt_cache_retention", "prompt_cache_options"],
                    "cost_by_header": {"header": "x", "values": {}}}}}}}"#;
        let offered = offered(catalog).unwrap();
        let models = &offered.providers[0].1["models"];
        assert_eq!(
            models[0]["cost"],
            json!({"input": 3.0, "output": 15.0, "header": "X-LLM-Backend", "byHeader": {
                "a": {"input": 3.0, "output": 15.0, "cacheWrite": 6.0,
                    "longContext": {"input": 6.0, "above": 200_000}},
                "b": {"input": 4.0}}})
        );
        let prices = super::super::pricing::Prices::of(&models[0]["cost"]);
        assert_eq!(prices.header(), Some("x-llm-backend"));
        assert_eq!(models[0].get("compat"), None);
        assert_eq!(models[1].get("cost"), None);
        assert_eq!(
            models[1]["compat"],
            json!({"supportsLongCacheRetention": false})
        );
    }

    #[test]
    fn long_urls_make_short_file_names() {
        assert_eq!(
            file_name("http://127.0.0.1:8/c"),
            "http%3A%2F%2F127%2E0%2E0%2E1%3A8%2Fc"
        );
        let long = format!("https://example.com/{}", "a".repeat(300));
        let name = file_name(&long);
        assert_eq!(name.len(), 117);
        assert_ne!(name, file_name(&format!("{long}b")));
    }
}
