//! The models a provider lists itself, for a `models.json` provider entry that lists none: read
//! from its server, `GET {baseUrl}/models` (`{baseUrl}/v1/models` under Messages, as its requests
//! go to `{baseUrl}/v1/messages`), with the headers its requests carry. The list is saved in the
//! state directory, beside the logins' lists, and read again as it ages (`agent_cli::login`).
//! Each model takes what its list says of it in the fields the lists of Anthropic, OpenRouter,
//! Mistral, Groq and vLLM use; what a list does not say stays unknown, as for a model written in a
//! `models.json`, and the entry's `modelOverrides` may say it, as in Scramjet.

use super::client;
use super::login;
use super::providers::Route;
use serde_json::{Map, Value, json};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

/// How long reading a list that is not saved yet may hold up reading the providers.
const WAIT: Duration = Duration::from_secs(5);
/// How long reading a list again in the background may take.
const RELIST_WAIT: Duration = Duration::from_secs(30);
/// Pages read at most of a list that comes in pages (Anthropic's, 1000 models each).
const PAGES: usize = 10;

/// Where a provider's own list of its models is read from, and saved.
#[derive(Clone)]
pub struct Listing {
    provider: String,
    url: String,
    /// Its requests' headers; they hold its key.
    headers: Vec<(String, String)>,
    messages: bool,
    /// `None` without a state directory: the list is then kept only while the process runs.
    file: Option<PathBuf>,
    /// How many models it offered when the providers were read, or what kept it from offering any.
    pub(super) offered: Result<usize, String>,
}

/// Leaves the headers' values out: they hold keys.
impl std::fmt::Debug for Listing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Listing")
            .field("provider", &self.provider)
            .field("url", &self.url)
            .field("headers", &names)
            .field("file", &self.file)
            .field("offered", &self.offered)
            .finish()
    }
}

impl Listing {
    /// The list of the provider whose requests go by `route`, saved in `file`.
    pub(super) fn new(route: &Route, messages: bool, file: Option<PathBuf>) -> Self {
        let scheme = if route.tls { "https" } else { "http" };
        let path = match messages {
            true => "/v1/models",
            false => "/models",
        };
        Self {
            provider: route.provider.clone(),
            url: format!("{scheme}://{}{}{path}", route.authority, route.path),
            headers: route.headers.clone(),
            messages,
            file,
            offered: Ok(0),
        }
    }

    /// The provider's name.
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// How many models it offered when the providers were read, or what kept it from offering any.
    pub fn offered(&self) -> Result<usize, &str> {
        self.offered.as_ref().copied().map_err(String::as_str)
    }

    /// What tells a reading of it apart from another's: its provider, URL and headers, whose
    /// values count by a hash, kept only in memory.
    pub fn key(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.headers.hash(&mut hasher);
        format!("{}\n{}\n{:x}", self.provider, self.url, hasher.finish())
    }

    /// The models the list saved for its URL holds, as the server listed them; `None` when none
    /// is saved, or it was read from another URL.
    pub(super) fn saved(&self) -> Option<Vec<Value>> {
        let saved = self.read_saved()?;
        saved["data"].as_array().cloned()
    }

    fn read_saved(&self) -> Option<Value> {
        let text = std::fs::read(self.file.as_ref()?).ok()?;
        let saved: Value = serde_json::from_slice(&text).ok()?;
        (saved["url"] == self.url.as_str()).then_some(saved)
    }

    /// How long ago the list saved was read; `None` when there is none to go by.
    pub fn age(&self) -> Option<Duration> {
        let fetched = self.read_saved()?["fetched"].as_f64()?;
        Some(Duration::from_millis(
            (login::now_ms() - fetched).max(0.0) as u64
        ))
    }

    /// The models the list saved offers, as a `models.json` gives them, each with its id.
    pub fn models(&self) -> Vec<Value> {
        let saved = self.saved().unwrap_or_default();
        let models = saved
            .iter()
            .filter_map(|listed| model(listed, self.messages));
        models.collect()
    }

    /// Reads the list now, waiting at most 5 seconds, and saves it; unsaved, it is kept in memory
    /// (`Providers::tried`), and the background says why when it reads the list again.
    pub(super) fn read_now(&self) -> Result<Vec<Value>, String> {
        let models = self.fetch(WAIT)?;
        let _ = self.save(&models);
        Ok(models)
    }

    /// Reads the list again and saves it in place of the one saved, which stays when that fails.
    pub fn relist(&self) -> Result<(), String> {
        let models = self.fetch(RELIST_WAIT)?;
        self.save(&models)
    }

    /// Removes the list saved, so that it is read again.
    pub(super) fn forget(&self) {
        if let Some(file) = &self.file {
            let _ = std::fs::remove_file(file);
        }
    }

    fn save(&self, models: &[Value]) -> Result<(), String> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let saved = json!({"url": self.url, "fetched": login::now_ms(), "data": models});
        login::save_list(file, &saved)
    }

    /// The models the server lists, in its order, waiting at most `wait` for them all. It runs on
    /// a thread of its own, as the providers may be read where a runtime is running.
    fn fetch(&self, wait: Duration) -> Result<Vec<Value>, String> {
        let seconds = wait.as_secs();
        let late = format!(
            "{}'s model list did not come within {seconds}s",
            self.provider
        );
        let reading = async {
            let read = tokio::time::timeout(wait, self.read());
            read.await.unwrap_or(Err(late))
        };
        std::thread::scope(|scope| {
            let reading = scope.spawn(|| login::blocking(reading));
            let failed = || Err(format!("reading {}'s model list failed", self.provider));
            reading.join().unwrap_or_else(|_| failed())
        })
    }

    async fn read(&self) -> Result<Vec<Value>, String> {
        let mut headers: Vec<(&str, &str)> = (self.headers.iter())
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        headers.push(("accept", "application/json"));
        if self.messages {
            headers.push(("anthropic-version", "2023-06-01"));
        }
        let what = format!("{}'s model list", self.provider);
        let mut models = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..PAGES {
            let url = match (self.messages, &after) {
                (true, None) => format!("{}?limit=1000", self.url),
                (true, Some(after)) => format!("{}?limit=1000&after_id={}", self.url, query(after)),
                (false, _) => self.url.clone(),
            };
            let fetched = client::fetch(&self.provider, "GET", &url, &headers, None).await?;
            let answer = login::answer(fetched, &what)?;
            let Some(listed) = answer["data"].as_array() else {
                return Err(format!("{what} has no data"));
            };
            models.extend(listed.iter().cloned());
            match (answer["has_more"] == true, answer["last_id"].as_str()) {
                (true, Some(last)) if !last.is_empty() => after = Some(last.to_owned()),
                _ => break,
            }
        }
        Ok(models)
    }
}

/// `text` as a URL's query value: unreserved characters as they are, the others' bytes as `%XX`.
fn query(text: &str) -> String {
    escaped(text, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
    })
}

/// `provider` as a file name: its letters, digits, `-` and `_` as they are and its other bytes
/// as `%XX`, so that no name leads out of the directory or names a hidden file.
pub(super) fn file_name(provider: &str) -> String {
    escaped(provider, |byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
    })
}

fn escaped(text: &str, kept: impl Fn(u8) -> bool) -> String {
    let mut escaped = String::new();
    for byte in text.bytes() {
        match kept(byte) {
            true => escaped.push(byte as char),
            false => escaped.push_str(&format!("%{byte:02X}")),
        }
    }
    escaped
}

/// A model a provider listed, as a `models.json` model; `None` for one its list says is retired,
/// archived or inactive, or cannot chat or take tools. Requests under Messages ask for an hour's
/// caching, which has its own price.
pub(super) fn model(listed: &Value, messages: bool) -> Option<Value> {
    let id = listed["id"].as_str().filter(|id| !id.trim().is_empty())?;
    let capabilities = &listed["capabilities"];
    let parameters = listed["supported_parameters"].as_array();
    let has =
        |list: Option<&Vec<Value>>, item: &str| list.is_some_and(|l| l.iter().any(|i| i == item));
    let gone =
        listed["lifecycle"] == "retired" || listed["archived"] == true || listed["active"] == false;
    let unfit = (parameters.is_some() && !has(parameters, "tools"))
        || capabilities["function_calling"] == false
        || capabilities["completion_chat"] == false;
    if gone || unfit {
        return None;
    }
    let number = |value: &Value| value.as_u64().filter(|n| *n > 0);
    let window = [
        &listed["max_input_tokens"],
        &listed["context_length"],
        &listed["context_window"],
        &listed["max_context_length"],
        &listed["max_model_len"],
    ];
    let output = [
        &listed["max_tokens"],
        &listed["top_provider"]["max_completion_tokens"],
        &listed["max_completion_tokens"],
    ];
    let modalities = listed["architecture"]["input_modalities"].as_array();
    let images = capabilities["image_input"]["supported"] == true
        || capabilities["vision"] == true
        || has(modalities, "image");
    let (thinking, effort) = (&capabilities["thinking"], &capabilities["effort"]);
    let reasoning_of = &listed["reasoning"];
    let reasoning =
        thinking["supported"] == true || reasoning_of.is_object() || has(parameters, "reasoning");
    let mut efforts: Vec<&str> = Vec::new();
    if thinking["supported"] == true && effort["supported"] == true {
        if thinking["types"]["disabled"]["supported"] == true {
            efforts.push("none");
        }
        let levels = ["minimal", "low", "medium", "high", "xhigh", "max"];
        efforts.extend(
            levels
                .into_iter()
                .filter(|level| effort[*level]["supported"] == true),
        );
    }
    if let Some(supported) = reasoning_of["supported_efforts"].as_array() {
        if reasoning_of["mandatory"] != true {
            efforts.push("none");
        }
        efforts.extend(supported.iter().filter_map(Value::as_str));
    }
    let mut entry = json!({
        "id": id,
        "reasoning": reasoning,
        "input": match images {
            true => json!(["text", "image"]),
            false => json!(["text"]),
        },
    });
    let name = [
        &listed["display_name"],
        &listed["name"],
        &listed["displayName"],
    ];
    if let Some(name) = name.into_iter().find_map(Value::as_str) {
        entry["name"] = json!(name);
    }
    if let Some(window) = window.into_iter().find_map(number) {
        entry["contextWindow"] = json!(window);
    }
    if let Some(output) = output.into_iter().find_map(number) {
        entry["maxTokens"] = json!(output);
    }
    if reasoning && !efforts.is_empty() {
        entry["thinkingLevelMap"] = super::providers::level_map(&efforts);
    }
    if let Some(cost) = cost(&listed["pricing"], messages) {
        entry["cost"] = cost;
    }
    Some(entry)
}

/// OpenRouter's `pricing` (dollars a token, as text) as Scramjet's `cost` (dollars a million),
/// with the rates of its first tier for long prompts (`overrides`, from `min_prompt_tokens`).
fn cost(pricing: &Value, messages: bool) -> Option<Value> {
    let mut cost = rates(pricing, messages)?;
    let tiers = pricing["overrides"].as_array().into_iter().flatten();
    let tiers = tiers.filter_map(|tier| Some((tier["min_prompt_tokens"].as_u64()?, tier)));
    if let Some((from, tier)) = tiers
        .filter(|(from, _)| *from > 0)
        .min_by_key(|(from, _)| *from)
        && let Some(mut long) = rates(tier, messages)
    {
        long["above"] = json!(from - 1);
        cost["longContext"] = long;
    }
    Some(cost)
}

fn rates(pricing: &Value, messages: bool) -> Option<Value> {
    let rate = |value: &Value| {
        let rate = match value {
            Value::String(text) => text.trim().parse::<f64>().ok()?,
            Value::Number(number) => number.as_f64()?,
            _ => return None,
        };
        // OpenRouter's router models price at -1: it depends on the model routed to.
        (rate.is_finite() && rate >= 0.0).then(|| (rate * 1e12).round() / 1e6)
    };
    let write = match messages {
        true => (pricing.get("input_cache_write_1h"))
            .filter(|rate| !rate.is_null())
            .unwrap_or(&pricing["input_cache_write"]),
        false => &pricing["input_cache_write"],
    };
    let mut cost = Map::new();
    for (name, value) in [
        ("input", &pricing["prompt"]),
        ("output", &pricing["completion"]),
        ("cacheRead", &pricing["input_cache_read"]),
        ("cacheWrite", write),
    ] {
        if let Some(rate) = rate(value) {
            cost.insert(name.into(), json!(rate));
        }
    }
    (!cost.is_empty()).then_some(Value::Object(cost))
}

/// `model` with what `overrides`, an entry's `modelOverrides`, says of its id, as Scramjet
/// applies them: each field in place of the list's, `cost`, `headers` and `compat` field by field.
pub(super) fn overridden(mut model: Value, overrides: &Map<String, Value>) -> Value {
    let id = model["id"].as_str().unwrap_or_default().to_owned();
    let Some(given) = overrides.get(&id).and_then(Value::as_object) else {
        return model;
    };
    for (key, value) in given {
        match (key.as_str(), model.get_mut(key), value) {
            ("id", _, _) => {}
            ("cost" | "headers" | "compat", Some(Value::Object(listed)), Value::Object(value)) => {
                listed.extend(
                    value
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
            _ => model[key] = value.clone(),
        }
    }
    model
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_list_says_what_it_knows_of_its_models() {
        // Anthropic's: limits, thinking, its effort levels and images.
        let claude = json!({"id": "claude-opus-5-5", "type": "model", "display_name": "Claude Opus 5.5",
            "max_input_tokens": 1_000_000, "max_tokens": 128_000, "lifecycle": "active",
            "capabilities": {"image_input": {"supported": true},
                "thinking": {"supported": true, "types": {"adaptive": {"supported": true},
                    "disabled": {"supported": true}}},
                "effort": {"supported": true, "low": {"supported": true},
                    "medium": {"supported": true}, "high": {"supported": true},
                    "xhigh": {"supported": true}, "max": {"supported": false}}}});
        let model = model(&claude, true).unwrap();
        assert_eq!(
            model,
            json!({"id": "claude-opus-5-5", "name": "Claude Opus 5.5", "reasoning": true,
                "input": ["text", "image"], "contextWindow": 1_000_000, "maxTokens": 128_000,
                "thinkingLevelMap": {"off": "none", "minimal": null, "low": "low",
                    "medium": "medium", "high": "high", "xhigh": "xhigh", "max": null}})
        );
        let retired = json!({"id": "claude-2", "lifecycle": "retired"});
        assert_eq!(super::model(&retired, true), None);
        // OpenRouter's: prices a token, those that take tools, and its levels.
        let routed = json!({"id": "openai/gpt-6-luna", "name": "OpenAI: GPT-6 Luna",
            "context_length": 400_000, "top_provider": {"max_completion_tokens": 128_000},
            "architecture": {"input_modalities": ["text", "image", "file"]},
            "supported_parameters": ["tools", "reasoning", "max_tokens"],
            "reasoning": {"mandatory": true, "supported_efforts": ["low", "medium", "high"]},
            "pricing": {"prompt": "0.0000025", "completion": "0.00002",
                "input_cache_read": "0.00000025", "input_cache_write": null, "request": "0"}});
        let model = super::model(&routed, false).unwrap();
        assert_eq!(
            (
                &model["contextWindow"],
                &model["maxTokens"],
                &model["input"]
            ),
            (&json!(400_000), &json!(128_000), &json!(["text", "image"]))
        );
        assert_eq!(
            model["cost"],
            json!({"input": 2.5, "output": 20.0, "cacheRead": 0.25})
        );
        assert_eq!(model["thinkingLevelMap"]["off"], Value::Null);
        assert_eq!(model["thinkingLevelMap"]["medium"], "medium");
        let chat_only = json!({"id": "x/chat", "supported_parameters": ["temperature"]});
        assert_eq!(super::model(&chat_only, false), None);
        let auto = json!({"id": "openrouter/auto", "supported_parameters": ["tools"],
            "pricing": {"prompt": "-1", "completion": "-1"}});
        assert_eq!(super::model(&auto, false).unwrap().get("cost"), None);
        // Messages requests ask for an hour's caching.
        let pricing =
            json!({"input_cache_write": "0.00000375", "input_cache_write_1h": "0.000006"});
        assert_eq!(cost(&pricing, true).unwrap()["cacheWrite"], json!(6.0));
        assert_eq!(cost(&pricing, false).unwrap()["cacheWrite"], json!(3.75));
        // Its rates for long prompts, from their least number of tokens.
        let tiered = json!({"prompt": "0.0000001", "completion": "0.0000005", "overrides": [
            {"min_prompt_tokens": 200_000, "prompt": "0.0000004"},
            {"min_prompt_tokens": 100_000, "prompt": "0.0000002", "completion": "0.0000025"}]});
        assert_eq!(
            cost(&tiered, false).unwrap(),
            json!({"input": 0.1, "output": 0.5,
                "longContext": {"input": 0.2, "output": 2.5, "above": 99_999}})
        );
        // Mistral's, Groq's and vLLM's windows; Mistral's embedders and Groq's inactive models go.
        let mistral = json!({"id": "mistral-large", "max_context_length": 131_072,
            "capabilities": {"completion_chat": true, "function_calling": true, "vision": true}});
        let mistral = super::model(&mistral, false).unwrap();
        assert_eq!(
            (&mistral["contextWindow"], &mistral["input"][1]),
            (&json!(131_072), &json!("image"))
        );
        let embedder = json!({"id": "mistral-embed",
            "capabilities": {"completion_chat": false, "function_calling": false}});
        assert_eq!(super::model(&embedder, false), None);
        let groq =
            json!({"id": "llama", "context_window": 131_072, "max_completion_tokens": 32_768});
        let groq = super::model(&groq, false).unwrap();
        assert_eq!(
            (&groq["contextWindow"], &groq["maxTokens"]),
            (&json!(131_072), &json!(32_768))
        );
        assert_eq!(
            super::model(&json!({"id": "old", "active": false}), false),
            None
        );
        let vllm = super::model(&json!({"id": "qwen", "max_model_len": 32_768}), false).unwrap();
        assert_eq!(vllm["contextWindow"], 32_768);
        // OpenAI's and Ollama's say nothing but the id: Scramjet's defaults.
        let plain = super::model(
            &json!({"id": "gpt-6", "object": "model", "owned_by": "openai"}),
            false,
        );
        assert_eq!(
            plain,
            Some(json!({"id": "gpt-6", "reasoning": false, "input": ["text"]}))
        );
        assert_eq!(super::model(&json!({"object": "model"}), false), None);
    }

    #[test]
    fn overrides_say_what_a_list_does_not() {
        let listed = json!({"id": "gpt-6", "reasoning": false, "input": ["text"],
            "cost": {"input": 1.0, "output": 8.0}});
        let overrides = json!({"gpt-6": {"id": "renamed", "reasoning": true, "contextWindow": 400_000,
            "cost": {"output": 10.0}}, "other": {"reasoning": true}});
        let overridden = overridden(listed, overrides.as_object().unwrap());
        assert_eq!(
            overridden,
            json!({"id": "gpt-6", "reasoning": true, "input": ["text"], "contextWindow": 400_000,
                "cost": {"input": 1.0, "output": 10.0}})
        );
    }

    #[test]
    fn names_are_kept_inside_the_directory() {
        assert_eq!(file_name("openrouter"), "openrouter");
        assert_eq!(file_name("../my.provider"), "%2E%2E%2Fmy%2Eprovider");
        assert_eq!(query("a b/c"), "a%20b%2Fc");
    }
}
