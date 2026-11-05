//! GitHub Copilot, as Scramjet logs in to it (`packages/ai/src/utils/oauth/github-copilot.ts`):
//! GitHub's device login gives a GitHub token, kept as the credential's `refresh`, which buys
//! Copilot tokens of half an hour, kept as its `access`; requests carry the Copilot token and
//! go to the host the token names. The login also saves the models Copilot lists for the
//! account (`GET /models`, as VS Code's Copilot reads them), which become the provider's.

use super::client::{self, Fetched};
use super::login;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// The provider's name, in `auth.json` and as a provider.
pub const PROVIDER: &str = "github-copilot";
/// What `DeviceLogin::finish` fails with when it was cancelled.
pub const CANCELLED: &str = "cancelled";
/// VS Code's Copilot client, as Scramjet and other open clients log in.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// What Copilot's own client says of itself, as Scramjet sends it.
pub(super) const HEADERS: [(&str, &str); 4] = [
    ("User-Agent", "GitHubCopilotChat/0.35.0"),
    ("Editor-Version", "vscode/1.107.0"),
    ("Editor-Plugin-Version", "copilot-chat/0.35.0"),
    ("Copilot-Integration-Id", "vscode-chat"),
];
/// The version of Copilot's API asked for, as Scramjet's models ask.
const API_VERSION: (&str, &str) = ("X-GitHub-Api-Version", "2026-06-01");
/// Copilot's API for a GitHub.com account, unless its token names another host.
pub(super) const DEFAULT_HOST: &str = "api.individual.githubcopilot.com";
/// A Copilot token is taken as expired this long before it does.
const EARLY: f64 = 5.0 * 60.0 * 1000.0;
/// The least time between two asks whether the code was entered.
#[cfg(not(test))]
const MIN_INTERVAL: f64 = 1.0;
#[cfg(test)]
const MIN_INTERVAL: f64 = 0.01;

/// Where a GitHub's login goes: github.com's or an Enterprise server's.
#[derive(Clone, Debug)]
pub(super) struct Urls {
    device: String,
    token: String,
    copilot: String,
    /// Copilot's API, in place of the host its token names.
    api: Option<String>,
}

impl Urls {
    fn of(domain: &str) -> Self {
        Self {
            device: format!("https://{domain}/login/device/code"),
            token: format!("https://{domain}/login/oauth/access_token"),
            copilot: format!("https://api.{domain}/copilot_internal/v2/token"),
            api: None,
        }
    }
}

/// A device login under way: the user enters `code` at `url` while `finish` waits for it.
pub struct DeviceLogin {
    pub url: String,
    pub code: String,
    device_code: String,
    interval: Duration,
    expires: Instant,
    urls: Urls,
    /// The Enterprise server's domain; none for github.com.
    enterprise: Option<String>,
}

/// The domain an Enterprise URL or domain names, as Scramjet's `normalizeDomain`; `None` for an
/// empty text, which means github.com.
pub fn enterprise_domain(typed: &str) -> Result<Option<String>, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    let rest = typed.split_once("://").map_or(typed, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let valid = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
    match !host.is_empty() && host.chars().all(valid) && !host.starts_with('.') {
        true => Ok(Some(host)),
        false => Err(format!("{typed} is not a GitHub Enterprise URL or domain")),
    }
}

impl DeviceLogin {
    /// Asks GitHub (github.com, or the Enterprise server `enterprise`) for a code to enter.
    pub fn start(enterprise: Option<String>) -> Result<Self, String> {
        let urls = Urls::of(enterprise.as_deref().unwrap_or("github.com"));
        Self::start_at(urls, enterprise)
    }

    pub(super) fn start_at(urls: Urls, enterprise: Option<String>) -> Result<Self, String> {
        let form = format!("client_id={CLIENT_ID}&scope=read:user");
        let answer = blocking(post_form(&urls.device, form))?;
        let field = |name: &str| answer[name].as_str().map(str::to_owned);
        let number = |name: &str| answer[name].as_f64().filter(|n| *n >= 0.0);
        match (
            field("device_code"),
            field("user_code"),
            field("verification_uri"),
            number("interval"),
            number("expires_in"),
        ) {
            (Some(device_code), Some(code), Some(url), Some(interval), Some(expires)) => Ok(Self {
                url,
                code,
                device_code,
                interval: Duration::from_secs_f64(interval.max(MIN_INTERVAL)),
                expires: Instant::now() + Duration::from_secs_f64(expires),
                urls,
                enterprise,
            }),
            _ => Err(format!(
                "GitHub's device code answer lacks a field: {answer}"
            )),
        }
    }

    /// Waits for the user to enter the code, as often as GitHub allows asking, then buys a Copilot
    /// token with the GitHub token, enables the models that need it and saves the login and its
    /// models in `logins`. `cancel` stops it.
    pub fn finish(self, logins: &login::Logins, cancel: &AtomicBool) -> Result<(), String> {
        let github = self.wait(cancel)?;
        let credential = buy(&self.urls.copilot, &github, self.enterprise.as_deref())?;
        let models = models(&credential, self.urls.api.as_deref())?;
        let list = json!({"fetched": login::now_ms(), "data": models});
        save_list(&logins.listed(PROVIDER), &list)?;
        login::save(&logins.auth, PROVIDER, credential)
    }

    /// The GitHub token, once the user entered the code; Scramjet's polling, a fifth slower than
    /// GitHub's interval, and two fifths slower after a `slow_down`.
    fn wait(&self, cancel: &AtomicBool) -> Result<String, String> {
        let form = format!(
            "client_id={CLIENT_ID}&device_code={}&grant_type=urn:ietf:params:oauth:grant-type:device_code",
            self.device_code
        );
        let (mut interval, mut factor, mut slowed) = (self.interval, 1.2, false);
        loop {
            let wait = interval.mul_f64(factor);
            let left = self.expires.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(match slowed {
                    true => "the code expired after GitHub asked to slow down, which a clock \
                             running behind (common in WSL and virtual machines) causes: sync it \
                             and try again"
                        .to_owned(),
                    false => "the code expired before it was entered".to_owned(),
                });
            }
            let until = Instant::now() + wait.min(left);
            while Instant::now() < until {
                if cancel.load(Ordering::Relaxed) {
                    return Err(CANCELLED.to_owned());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let answer = blocking(async {
                tokio::select! {
                    biased;
                    () = client::cancelled(cancel) => Err(CANCELLED.to_owned()),
                    answer = post_form(&self.urls.token, form.clone()) => answer,
                }
            })?;
            if let Some(token) = answer["access_token"].as_str() {
                return Ok(token.to_owned());
            }
            match answer["error"].as_str() {
                Some("authorization_pending") => {}
                Some("slow_down") => {
                    slowed = true;
                    factor = 1.4;
                    interval = match answer["interval"].as_f64().filter(|n| *n > 0.0) {
                        Some(seconds) => Duration::from_secs_f64(seconds),
                        None => interval + Duration::from_secs(5),
                    };
                }
                Some(error) => {
                    let said = answer["error_description"].as_str();
                    let said = said.map_or_else(String::new, |said| format!(": {said}"));
                    return Err(format!("GitHub refused the login: {error}{said}"));
                }
                None => return Err(format!("GitHub's answer has no token: {answer}")),
            }
        }
    }
}

/// A Copilot token bought anew with the GitHub token `credential` keeps.
pub(super) fn refresh(credential: &Value) -> Result<Value, String> {
    let github = credential["refresh"]
        .as_str()
        .filter(|token| !token.is_empty());
    let github = github.ok_or("the GitHub Copilot login keeps no GitHub token: /login again")?;
    let enterprise = credential["enterpriseUrl"].as_str();
    let domain = enterprise.unwrap_or("github.com");
    let bought = buy(&Urls::of(domain).copilot, github, enterprise);
    bought.map_err(|error| format!("{error} (renewing the Copilot token; /login if it goes on)"))
}

/// The credential of a Copilot token bought with `github`, the GitHub token.
fn buy(url: &str, github: &str, enterprise: Option<&str>) -> Result<Value, String> {
    let authorization = format!("Bearer {github}");
    let mut headers = vec![("Accept", "application/json")];
    headers.extend(HEADERS);
    headers.push(("Authorization", &authorization));
    let fetched = blocking(client::fetch(PROVIDER, "GET", url, &headers, None))?;
    let answer = answer(fetched, "GitHub's Copilot token")?;
    let (Some(token), Some(expires)) = (answer["token"].as_str(), answer["expires_at"].as_f64())
    else {
        return Err("GitHub's Copilot token answer lacks the token or its expiry".to_owned());
    };
    let mut credential = json!({
        "type": "oauth",
        "refresh": github,
        "access": token,
        "expires": expires * 1000.0 - EARLY,
    });
    if let Some(enterprise) = enterprise {
        credential["enterpriseUrl"] = json!(enterprise);
    }
    Ok(credential)
}

/// The chat models Copilot lists for the account, each enabled when it is not yet, as Scramjet
/// does after a login, which accepts the terms of those (Claude, Grok) that ask for it; but those
/// disabled, as the user or their organization can in Copilot's settings.
fn models(credential: &Value, api: Option<&str>) -> Result<Vec<Value>, String> {
    let token = credential["access"].as_str().unwrap_or_default();
    let base = match api {
        Some(api) => api.to_owned(),
        None => format!(
            "https://{}",
            host(token, credential["enterpriseUrl"].as_str())
        ),
    };
    let authorization = format!("Bearer {token}");
    let mut headers = vec![("Accept", "application/json"), API_VERSION];
    headers.extend(HEADERS);
    headers.push(("Authorization", &authorization));
    let url = format!("{base}/models");
    let listed = blocking(client::fetch(PROVIDER, "GET", &url, &headers, None))?;
    let listed = answer(listed, "Copilot's model list")?;
    let mut models = match listed["data"].as_array() {
        Some(models) => models.clone(),
        None => return Err(format!("Copilot's model list has no data: {listed}")),
    };
    models.retain(|model| model["capabilities"]["type"] == "chat");
    let mut policy = headers.clone();
    policy.extend([
        ("openai-intent", "chat-policy"),
        ("x-interaction-type", "chat-policy"),
    ]);
    for model in &mut models {
        let state = model["policy"]["state"].as_str();
        let (Some(id), Some(state)) = (model["id"].as_str(), state) else {
            continue;
        };
        if state == "enabled" || state == "disabled" {
            continue;
        }
        let url = format!("{base}/models/{id}/policy");
        let body = Some(("application/json", br#"{"state":"enabled"}"#.to_vec()));
        let enabled = blocking(client::fetch(PROVIDER, "POST", &url, &policy, body));
        if enabled.is_ok_and(|enabled| (200..300).contains(&enabled.status)) {
            model["policy"]["state"] = json!("enabled");
        }
    }
    Ok(models)
}

fn save_list(path: &Path, list: &Value) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", path.display());
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).map_err(failed)?;
    let written = path.with_extension(format!("{}.tmp", std::process::id()));
    let text = serde_json::to_string_pretty(list).unwrap_or_default() + "\n";
    std::fs::write(&written, text).map_err(failed)?;
    std::fs::rename(&written, path).map_err(failed)
}

/// The provider the models a login `listed` make, as a `models.json` entry: those that take
/// tools and are enabled, with the protocol each speaks (Messages, else Responses, else Chat
/// Completions), its limits, images and effort levels.
pub(super) fn entry(credential: &Value, listed: &Value) -> Value {
    let token = credential["access"].as_str().unwrap_or_default();
    let headers: serde_json::Map<String, Value> = (HEADERS.iter().chain([&API_VERSION]))
        .map(|(name, value)| (name.to_string(), json!(value)))
        .collect();
    let models: Vec<Value> = (listed["data"].as_array().into_iter().flatten())
        .filter_map(model)
        .collect();
    json!({
        "baseUrl": format!("https://{}", host(token, credential["enterpriseUrl"].as_str())),
        "headers": headers,
        "models": models,
    })
}

/// A model Copilot listed, as a `models.json` model; `None` for one a coding agent cannot use,
/// or that Copilot's own model picker does not offer (older versions kept for its own uses).
fn model(listed: &Value) -> Option<Value> {
    let id = listed["id"].as_str()?;
    let capabilities = &listed["capabilities"];
    let (supports, limits) = (&capabilities["supports"], &capabilities["limits"]);
    let disabled = listed["policy"]["state"]
        .as_str()
        .is_some_and(|state| state != "enabled");
    let unlisted = listed["model_picker_enabled"] == false;
    if capabilities["type"] != "chat" || supports["tool_calls"] != true || disabled || unlisted {
        return None;
    }
    let endpoints: Vec<&str> = (listed["supported_endpoints"]
        .as_array()
        .into_iter()
        .flatten())
    .filter_map(Value::as_str)
    .collect();
    let api = match () {
        _ if endpoints.contains(&"/v1/messages") => "anthropic-messages",
        _ if endpoints.contains(&"/responses") => "openai-responses",
        _ if endpoints.is_empty() || endpoints.contains(&"/chat/completions") => {
            "openai-completions"
        }
        _ => return None,
    };
    let number = |value: &Value| value.as_u64().filter(|n| *n > 0);
    let output = number(&limits["max_output_tokens"]);
    let window = number(&limits["max_context_window_tokens"]).or_else(|| {
        let prompt = number(&limits["max_prompt_tokens"])?;
        Some(prompt + output.unwrap_or(0))
    });
    let efforts: Vec<&str> = (supports["reasoning_effort"]
        .as_array()
        .into_iter()
        .flatten())
    .filter_map(Value::as_str)
    .collect();
    let thinks = supports["thinking"] == true || supports["adaptive_thinking"] == true;
    let reasoning = !efforts.is_empty() || thinks;
    let mut entry = json!({
        "id": id,
        "api": api,
        "reasoning": reasoning,
        "input": match supports["vision"] == true {
            true => json!(["text", "image"]),
            false => json!(["text"]),
        },
    });
    if let Some(name) = listed["name"].as_str() {
        entry["name"] = json!(name);
    }
    if let Some(window) = window {
        entry["contextWindow"] = json!(window);
    }
    if let Some(output) = output {
        entry["maxTokens"] = json!(output);
    }
    let prompt = number(&limits["max_prompt_tokens"]);
    if let Some(prompt) = prompt.filter(|prompt| window.is_some_and(|window| *prompt < window)) {
        entry["maxInputTokens"] = json!(prompt);
    }
    if let Some(cost) = cost(
        &listed["billing"]["token_prices"],
        api == "anthropic-messages",
    ) {
        entry["cost"] = cost;
    }
    // The levels it lists, each sent as it is; with none listed, Scramjet's ordinary ones.
    if !efforts.is_empty() {
        let mut map = serde_json::Map::new();
        for (level, sent) in [
            ("off", "none"),
            ("minimal", "minimal"),
            ("low", "low"),
            ("medium", "medium"),
            ("high", "high"),
            ("xhigh", "xhigh"),
            ("max", "max"),
        ] {
            let taken = efforts.contains(&sent);
            map.insert(level.into(), if taken { json!(sent) } else { Value::Null });
        }
        entry["thinkingLevelMap"] = Value::Object(map);
    }
    Some(entry)
}

/// Copilot's `token_prices` (cents per `batch_size` tokens; nothing to pay when that is 0) as
/// Scramjet's `cost` (dollars per million), the long context's rates above the default tier's
/// `context_max`. Messages requests ask for an hour's caching, which its own price has.
fn cost(prices: &Value, messages: bool) -> Option<Value> {
    let batch = prices["batch_size"].as_f64()?;
    let rate = |cents: &Value| {
        let cents = cents.as_f64()?;
        Some(match batch > 0.0 {
            true => cents / 100.0 * 1_000_000.0 / batch,
            false => 0.0,
        })
    };
    let rates = |tier: &Value| {
        let write = match messages {
            true => tier
                .get("cache_write_1h_price")
                .unwrap_or(&tier["cache_write_price"]),
            false => &tier["cache_write_price"],
        };
        let mut rates = serde_json::Map::new();
        for (name, cents) in [
            ("input", &tier["input_price"]),
            ("output", &tier["output_price"]),
            ("cacheRead", &tier["cache_price"]),
            ("cacheWrite", write),
        ] {
            if let Some(rate) = rate(cents) {
                rates.insert(name.into(), json!(rate));
            }
        }
        Value::Object(rates)
    };
    let default = &prices["default"];
    if !default.is_object() {
        return None;
    }
    let mut cost = rates(default);
    let long = &prices["long_context"];
    if let (Some(above), true) = (default["context_max"].as_u64(), long.is_object()) {
        let mut long = rates(long);
        long["above"] = json!(above);
        cost["longContext"] = long;
    }
    Some(cost)
}

/// The headers a request to Copilot carries besides its route's, as Scramjet's: whether the user
/// asked for it (a turn's step after a user message) or the agent did (after a tool's result, or
/// a summary's or subagent's request, as VS Code's Copilot marks those), which Copilot bills
/// differently, and whether it holds images.
pub(super) fn request_headers(request: &Value, replying: bool) -> Vec<(&'static str, String)> {
    let input = request["input"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let user = replying
        && input.last().is_some_and(|item| {
            item["role"] == "user" && (item.get("type").is_none() || item["type"] == "message")
        });
    let mut headers = vec![
        (
            "x-initiator",
            if user { "user" } else { "agent" }.to_owned(),
        ),
        ("openai-intent", "conversation-edits".to_owned()),
    ];
    if input.iter().any(|item| super::tools::item_images(item) > 0) {
        headers.push(("copilot-vision-request", "true".to_owned()));
    }
    headers
}

/// The host a Copilot token's requests go to: its `proxy-ep`'s, `proxy.` made `api.`; else an
/// Enterprise server's Copilot API, else the default.
pub(super) fn host(token: &str, enterprise: Option<&str>) -> String {
    let named = (token.split(';'))
        .find_map(|part| part.strip_prefix("proxy-ep="))
        .filter(|host| !host.is_empty());
    match (named, enterprise) {
        (Some(host), _) => match host.strip_prefix("proxy.") {
            Some(rest) => format!("api.{rest}"),
            None => host.to_owned(),
        },
        (None, Some(domain)) => format!("copilot-api.{domain}"),
        (None, None) => DEFAULT_HOST.to_owned(),
    }
}

async fn post_form(url: &str, form: String) -> Result<Value, String> {
    let headers = [("Accept", "application/json"), ("User-Agent", HEADERS[0].1)];
    let body = Some(("application/x-www-form-urlencoded", form.into_bytes()));
    answer(
        client::fetch(PROVIDER, "POST", url, &headers, body).await?,
        "GitHub",
    )
}

/// A JSON answer, or what failed.
fn answer(fetched: Fetched, what: &str) -> Result<Value, String> {
    let text = String::from_utf8_lossy(&fetched.body);
    if !(200..300).contains(&fetched.status) {
        let said = text.trim().chars().take(400).collect::<String>();
        return Err(format!("{what} answered HTTP {}: {said}", fetched.status));
    }
    serde_json::from_str(&text).map_err(|error| format!("{what}'s answer is not JSON: {error}"))
}

/// Runs `future` to its end on a runtime of its own: logins run apart from any turn.
pub(super) fn blocking<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    runtime.map_err(|error| error.to_string())?.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::ScratchDir;
    use std::io::{BufRead, BufReader, Read, Write};

    /// A GitHub and Copilot of the test's: answers `count` requests, each with what `answer`
    /// makes of its request line, head and body, as JSON; returns them as they came.
    fn github(
        count: usize,
        mut answer: impl FnMut(&str, &str) -> Value + Send + 'static,
    ) -> (String, std::thread::JoinHandle<Vec<(String, String)>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..count {
                let (connection, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(connection);
                let (mut head, mut length) = (String::new(), 0);
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8(body).unwrap();
                let answer = answer(&head, &body).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{answer}",
                    answer.len()
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
                requests.push((head, body));
            }
            requests
        });
        (base, server)
    }

    fn listed() -> Value {
        json!([
            {"id": "claude-opus-5.5", "name": "Claude Opus 5.5", "policy": {"state": "unconfigured"},
             "model_picker_enabled": true,
             "billing": {"token_prices": {"batch_size": 1000000,
                "default": {"input_price": 400, "output_price": 2000, "cache_price": 20,
                    "cache_write_price": 500, "cache_write_1h_price": 800, "context_max": 200000},
                "long_context": {"input_price": 600, "output_price": 2400, "cache_price": 40,
                    "cache_write_price": 700, "cache_write_1h_price": 900, "context_max": 872000}}},
             "capabilities": {"type": "chat", "limits": {"max_context_window_tokens": 1000000,
                "max_prompt_tokens": 872000, "max_output_tokens": 128000},
              "supports": {"tool_calls": true, "vision": true, "adaptive_thinking": true,
                "reasoning_effort": ["low", "medium", "high", "max"]}},
             "supported_endpoints": ["/chat/completions", "/v1/messages"]},
            {"id": "gpt-6-luna", "name": "GPT-6 Luna", "policy": {"state": "enabled"},
             "billing": {"token_prices": {"batch_size": 0, "default": {"input_price": 0,
                "output_price": 0, "cache_price": 0, "cache_write_price": 0}}},
             "capabilities": {"type": "chat", "limits": {"max_prompt_tokens": 400000,
                "max_output_tokens": 128000},
              "supports": {"tool_calls": true, "reasoning_effort": ["none", "low", "medium"]}},
             "supported_endpoints": ["/chat/completions", "/responses"]},
            {"id": "kimi-k3", "capabilities": {"type": "chat", "supports": {"tool_calls": true}},
             "billing": {"token_prices": {"batch_size": 1000000, "default": {"input_price": 300,
                "output_price": 1500, "cache_price": 30, "cache_write_price": 0}}}},
            {"id": "gpt-4o-mini", "capabilities": {"type": "chat", "supports": {}}},
            {"id": "gpt-4o", "model_picker_enabled": false,
             "capabilities": {"type": "chat", "supports": {"tool_calls": true}}},
            {"id": "grok-4.6", "policy": {"state": "disabled"},
             "capabilities": {"type": "chat", "supports": {"tool_calls": true}}},
            {"id": "embedder", "capabilities": {"type": "embeddings"}},
        ])
    }

    #[test]
    fn a_device_login_waits_for_the_code_then_saves_a_copilot_token_and_the_models() {
        let expires = (login::now_ms() / 1000.0) as u64 + 1800;
        let mut polls = 0;
        let (base, server) = github(7, move |head, _| {
            let path = head.split(' ').nth(1).unwrap_or_default();
            match path {
                "/login/device/code" => json!({"device_code": "dc-1", "user_code": "ABCD-1234",
                    "verification_uri": "https://github.com/login/device", "interval": 0,
                    "expires_in": 900}),
                "/login/oauth/access_token" => {
                    polls += 1;
                    match polls {
                        1 => json!({"error": "authorization_pending"}),
                        2 => json!({"error": "slow_down", "interval": 0.01}),
                        _ => json!({"access_token": "gho_github", "token_type": "bearer"}),
                    }
                }
                "/copilot_internal/v2/token" => json!({"expires_at": expires,
                    "token": "tid=1;exp=2;proxy-ep=proxy.business.githubcopilot.com;st=dotcom"}),
                "/models" => json!({"data": listed(), "object": "list"}),
                _ => json!({}),
            }
        });
        let urls = Urls {
            device: format!("{base}/login/device/code"),
            token: format!("{base}/login/oauth/access_token"),
            copilot: format!("{base}/copilot_internal/v2/token"),
            api: Some(base.clone()),
        };
        let device = DeviceLogin::start_at(urls, None).unwrap();
        assert_eq!(
            (device.url.as_str(), device.code.as_str()),
            ("https://github.com/login/device", "ABCD-1234")
        );
        let scratch = ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-copilot-login-{}", std::process::id())),
        );
        let logins = login::Logins {
            auth: scratch.0.join("config/auth.json"),
            lists: scratch.0.join("state/logins"),
        };
        device.finish(&logins, &AtomicBool::new(false)).unwrap();
        let requests = server.join().unwrap();
        let lines: Vec<&str> = (requests.iter())
            .map(|(head, _)| head.lines().next().unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                "POST /login/device/code HTTP/1.1",
                "POST /login/oauth/access_token HTTP/1.1",
                "POST /login/oauth/access_token HTTP/1.1",
                "POST /login/oauth/access_token HTTP/1.1",
                "GET /copilot_internal/v2/token HTTP/1.1",
                "GET /models HTTP/1.1",
                "POST /models/claude-opus-5.5/policy HTTP/1.1",
            ]
        );
        assert_eq!(
            requests[0].1,
            format!("client_id={CLIENT_ID}&scope=read:user")
        );
        assert!(
            requests[1].1.contains("device_code=dc-1&grant_type="),
            "{}",
            requests[1].1
        );
        assert!(
            requests[4]
                .0
                .contains("authorization: Bearer gho_github\r\n")
        );
        assert!(
            requests[5]
                .0
                .contains("authorization: Bearer tid=1;exp=2;proxy-ep=")
        );
        assert!(
            requests[5]
                .0
                .contains("copilot-integration-id: vscode-chat\r\n")
        );
        assert_eq!(requests[6].1, r#"{"state":"enabled"}"#);
        let saved = login::saved(&logins.auth).unwrap();
        let credential = &saved[PROVIDER];
        assert_eq!(credential["refresh"], "gho_github");
        assert_eq!(
            credential["expires"],
            json!(expires as f64 * 1000.0 - EARLY)
        );
        // The models are saved apart; the embedding model is not kept, the one enabled marked so.
        let text = std::fs::read_to_string(logins.listed(PROVIDER)).unwrap();
        let list: Value = serde_json::from_str(&text).unwrap();
        let ids: Vec<&str> = (list["data"].as_array().unwrap().iter())
            .filter_map(|model| model["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "claude-opus-5.5",
                "gpt-6-luna",
                "kimi-k3",
                "gpt-4o-mini",
                "gpt-4o",
                "grok-4.6"
            ]
        );
        assert_eq!(list["data"][0]["policy"]["state"], "enabled");
        let entry = entry(credential, &list);
        assert_eq!(entry["baseUrl"], "https://api.business.githubcopilot.com");
        assert_eq!(entry["headers"]["X-GitHub-Api-Version"], "2026-06-01");
    }

    #[test]
    fn a_device_login_stops_when_cancelled_even_while_github_has_not_answered() {
        let (base, _) = github(1, |_, _| {
            json!({"device_code": "dc", "user_code": "C", "verification_uri": "u",
                "interval": 0, "expires_in": 900})
        });
        // Takes the poll and never answers it.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let token = format!(
            "http://{}/login/oauth/access_token",
            silent.local_addr().unwrap()
        );
        let held = std::thread::spawn(move || silent.accept().map(|(connection, _)| connection));
        let urls = Urls {
            device: format!("{base}/login/device/code"),
            token,
            copilot: String::new(),
            api: None,
        };
        let device = DeviceLogin::start_at(urls, None).unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let stop = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                let _connection = held.join().unwrap().unwrap();
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(500));
            })
        };
        let started = Instant::now();
        let nowhere = login::Logins {
            auth: "/nonexistent/auth.json".into(),
            lists: "/nonexistent".into(),
        };
        let finished = device.finish(&nowhere, &cancel);
        assert_eq!(finished.unwrap_err(), CANCELLED);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        stop.join().unwrap();
    }

    #[test]
    fn the_models_listed_become_a_provider_of_those_that_take_tools() {
        let credential = json!({"access": "tid=1"});
        let mut list = json!({"data": listed()});
        // One not enabled is left out, as its requests would fail.
        assert_eq!(entry(&credential, &list)["models"][0]["id"], "gpt-6-luna");
        list["data"][0]["policy"]["state"] = json!("enabled");
        let entry = entry(&credential, &list);
        assert_eq!(entry["baseUrl"], format!("https://{DEFAULT_HOST}"));
        let levels = |levels: &[(&str, Value)]| {
            Value::Object(
                levels
                    .iter()
                    .map(|(level, sent)| (level.to_string(), sent.clone()))
                    .collect(),
            )
        };
        assert_eq!(
            entry["models"],
            json!([
                {"id": "claude-opus-5.5", "name": "Claude Opus 5.5", "api": "anthropic-messages",
                 "reasoning": true, "input": ["text", "image"], "contextWindow": 1000000,
                 "maxTokens": 128000, "maxInputTokens": 872000,
                 "thinkingLevelMap": levels(&[("off", Value::Null),
                    ("minimal", Value::Null), ("low", json!("low")), ("medium", json!("medium")),
                    ("high", json!("high")), ("xhigh", Value::Null), ("max", json!("max"))]),
                 "cost": {"input": 4.0, "output": 20.0, "cacheRead": 0.2, "cacheWrite": 8.0,
                    "longContext": {"above": 200000, "input": 6.0, "output": 24.0,
                        "cacheRead": 0.4, "cacheWrite": 9.0}}},
                {"id": "gpt-6-luna", "name": "GPT-6 Luna", "api": "openai-responses",
                 "reasoning": true, "input": ["text"], "contextWindow": 528000,
                 "maxTokens": 128000, "maxInputTokens": 400000,
                 "thinkingLevelMap": levels(&[("off", json!("none")),
                    ("minimal", Value::Null), ("low", json!("low")), ("medium", json!("medium")),
                    ("high", Value::Null), ("xhigh", Value::Null), ("max", Value::Null)]),
                 "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0}},
                {"id": "kimi-k3", "api": "openai-completions", "reasoning": false,
                 "input": ["text"],
                 "cost": {"input": 3.0, "output": 15.0, "cacheRead": 0.3, "cacheWrite": 0.0}},
            ])
        );
        // As a provider: Copilot's headers, the login's key, each model on its protocol.
        let providers = super::super::Providers::given(json!({"providers": {PROVIDER: entry}}));
        let model = providers.find("claude-opus-5.5").unwrap();
        let route = model.info.route.as_ref().unwrap();
        assert_eq!(route.authority, DEFAULT_HOST);
        assert!(
            route
                .headers
                .contains(&("copilot-integration-id".into(), "vscode-chat".into()))
        );
        assert_eq!(model.info.levels, ["low", "medium", "high", "max"]);
        assert!(model.info.images);
        assert_eq!(model.info.input, Some(872000));
    }

    #[test]
    fn a_request_says_who_asked_for_it_and_whether_it_holds_images() {
        let asked = |input: Value| request_headers(&json!({"input": input}), true);
        let user = json!({"role": "user", "content": [{"type": "input_text", "text": "hi"}]});
        let result = json!({"type": "function_call_output", "call_id": "c", "output": "x"});
        assert_eq!(
            asked(json!([user])),
            [
                ("x-initiator", "user".to_owned()),
                ("openai-intent", "conversation-edits".to_owned())
            ]
        );
        assert_eq!(
            asked(json!([user, result]))[0],
            ("x-initiator", "agent".to_owned())
        );
        // A summary's or a subagent's request ends with a user message too.
        let summary = request_headers(&json!({"input": [user]}), false);
        assert_eq!(summary[0], ("x-initiator", "agent".to_owned()));
        let image = json!({"role": "user", "content": [{"type": "input_image",
            "image_url": "data:image/png;base64,AAAA"}]});
        assert_eq!(
            asked(json!([image]))[2],
            ("copilot-vision-request", "true".to_owned())
        );
    }

    #[test]
    fn an_enterprise_url_or_domain_names_its_host() {
        assert_eq!(enterprise_domain("  ").unwrap(), None);
        for typed in [
            "company.ghe.com",
            "https://company.ghe.com/",
            "https://user@Company.ghe.com:8443/login?x",
        ] {
            assert_eq!(
                enterprise_domain(typed).unwrap().as_deref(),
                Some("company.ghe.com")
            );
        }
        assert!(enterprise_domain("not a domain").is_err());
    }

    #[test]
    fn a_token_names_the_host_its_requests_go_to() {
        let token = "tid=1;exp=2;proxy-ep=proxy.business.githubcopilot.com;st=dotcom";
        assert_eq!(host(token, None), "api.business.githubcopilot.com");
        assert_eq!(
            host("tid=1;exp=2", Some("company.ghe.com")),
            "copilot-api.company.ghe.com"
        );
        assert_eq!(host("tid=1", None), DEFAULT_HOST);
    }
}
