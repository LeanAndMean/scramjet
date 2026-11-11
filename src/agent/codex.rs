//! ChatGPT's Codex subscription, as Scramjet logs in to it
//! (`packages/ai/src/utils/oauth/openai-codex.ts`) and sends it requests
//! (`packages/ai/src/providers/openai-codex-responses.ts`). OpenAI's browser login, with PKCE,
//! comes back to a server on 127.0.0.1:1455, or the user pastes the address the browser ended
//! on; it gives an access token, which names the ChatGPT account, and a refresh token. Requests
//! go to ChatGPT's Codex backend. The login also saves the models the backend lists for the
//! account (`GET /codex/models`), which become the provider's. The backend lists them for a
//! release of Codex's own client, and none for others: that request is Codex's current release's
//! (user decision, 2026-11-05: providers' own lists, never lists kept here), while the login and
//! the model requests say they are scramjet-rs's.

use super::client::{self, Failure};
use super::image::decode_base64;
use super::login;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// The provider's name, in `auth.json` and as a provider.
pub const PROVIDER: &str = "openai-codex";
/// Codex's own client, as Scramjet and other open clients log in.
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Where OpenAI sends the browser back: to the server on `CALLBACK`.
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK: &str = "127.0.0.1:1455";
const SCOPE: &str = "openid profile email offline_access";
/// The access token's claim that names the ChatGPT account.
const CLAIM: &str = "https://api.openai.com/auth";
/// ChatGPT's backend; Codex's requests go to `/codex/responses` under it.
pub(super) const BASE_URL: &str = "https://chatgpt.com/backend-api";
/// What the login and the model requests say of their client.
const ORIGINATOR: &str = "scramjet-rs";
/// What Codex's own client says of itself, as the model list's request says it.
const CODEX: &str = "codex_cli_rs";
/// What a listed model keeps of what the backend says of it (each also has its instructions).
const LISTED_FIELDS: [&str; 7] = [
    "slug",
    "display_name",
    "visibility",
    "context_window",
    "max_context_window",
    "supported_reasoning_levels",
    "input_modalities",
];

/// Where the login and the list go.
#[derive(Clone, Debug)]
pub(super) struct Urls {
    authorize: String,
    token: String,
    /// ChatGPT's backend.
    api: String,
    /// npm's record of Codex's current release.
    release: String,
}

#[cfg(not(test))]
impl Default for Urls {
    fn default() -> Self {
        Self {
            authorize: "https://auth.openai.com/oauth/authorize".to_owned(),
            token: "https://auth.openai.com/oauth/token".to_owned(),
            api: BASE_URL.to_owned(),
            release: "https://registry.npmjs.org/%40openai%2Fcodex/latest".to_owned(),
        }
    }
}

/// A test renewing a token, as a request does, reaches none of them.
#[cfg(test)]
impl Default for Urls {
    fn default() -> Self {
        tests::closed()
    }
}

/// A browser login under way: the user opens `url` while `finish` waits for OpenAI's answer.
pub struct BrowserLogin {
    pub url: String,
    verifier: String,
    state: String,
    /// The server the browser comes back to; none when its port is taken.
    listener: Option<TcpListener>,
    urls: Urls,
}

impl BrowserLogin {
    pub fn start() -> Result<Self, String> {
        Self::start_at(Urls::default(), CALLBACK)
    }

    pub(super) fn start_at(urls: Urls, callback: &str) -> Result<Self, String> {
        let verifier = base64url(&random(32)?);
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        let state: String = random(16)?.iter().map(|b| format!("{b:02x}")).collect();
        // Taken, as by another login under way, the answer is to be pasted, as in Scramjet.
        let listener = TcpListener::bind(callback).ok();
        let listener = listener.filter(|listener| listener.set_nonblocking(true).is_ok());
        let query = form(&[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPE),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", ORIGINATOR),
        ]);
        Ok(Self {
            url: format!("{}?{query}", urls.authorize),
            verifier,
            state,
            listener,
            urls,
        })
    }

    /// Whether the browser comes back by itself; else the address it ends on is to be pasted.
    pub fn listening(&self) -> bool {
        self.listener.is_some()
    }

    /// Waits for OpenAI's answer, from the browser or as `pasted`, then sets `answered`, takes
    /// the tokens, lists the account's models and saves the login and its models in `logins`;
    /// with a warning when the list was asked for as Codex's last release known, npm being out
    /// of reach. `cancel` stops it.
    pub fn finish(
        self,
        pasted: &mpsc::Receiver<String>,
        answered: &AtomicBool,
        logins: &login::Logins,
        cancel: &AtomicBool,
    ) -> Result<Option<String>, String> {
        let code = self.code(pasted, cancel)?;
        answered.store(true, Ordering::Relaxed);
        let form = form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", &code),
            ("code_verifier", &self.verifier),
            ("redirect_uri", REDIRECT_URI),
        ]);
        let credential = tokens(&self.urls.token, form, cancel)?;
        // Kept even when the list fails, which needs no new login to be read again.
        login::save(&logins.auth, PROVIDER, credential.clone())?;
        let kept = |error: String| match error == login::CANCELLED {
            true => "logged in, but the model list was cancelled: /login again for it".to_owned(),
            false => format!("logged in, but {error}"),
        };
        save_models(&credential, &self.urls, logins, cancel).map_err(kept)
    }

    /// The authorization code, from whichever comes first: the browser or the user.
    fn code(&self, pasted: &mpsc::Receiver<String>, cancel: &AtomicBool) -> Result<String, String> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(login::CANCELLED.to_owned());
            }
            if let Ok(text) = pasted.try_recv() {
                return self.pasted(&text);
            }
            if let Some(Ok((connection, _))) = self.listener.as_ref().map(TcpListener::accept)
                && let Some(code) = self.callback(connection)
            {
                return Ok(code);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The code in what the user pasted, as Scramjet reads it: the address the browser ended
    /// on, its query, `code#state` or the code alone.
    fn pasted(&self, text: &str) -> Result<String, String> {
        let (code, state) = parse_pasted(text);
        if state.is_some_and(|state| state != self.state) {
            return Err("the pasted address is another login's (its state differs)".to_owned());
        }
        let code = code.filter(|code| !code.is_empty());
        code.ok_or_else(|| "the pasted text holds no authorization code".to_owned())
    }

    /// Answers a request to the server; the code, when it is OpenAI's answer to this login.
    fn callback(&self, mut connection: TcpStream) -> Option<String> {
        let _ = connection.set_nonblocking(false);
        let _ = connection.set_read_timeout(Some(Duration::from_secs(2)));
        let mut head = Vec::new();
        let mut buffer = [0; 2048];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") && head.len() < 16 << 10 {
            match connection.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => head.extend_from_slice(&buffer[..read]),
            }
        }
        let head = String::from_utf8_lossy(&head);
        let line = head.lines().next().unwrap_or_default();
        let target = line.split(' ').nth(1).unwrap_or_default();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let (status, said, code) = match query_value(query, "code") {
            _ if path != "/auth/callback" => ("404 Not Found", "Callback route not found.", None),
            _ if query_value(query, "state").as_deref() != Some(&self.state) => {
                ("400 Bad Request", "State mismatch.", None)
            }
            Some(code) if !code.is_empty() => (
                "200 OK",
                "OpenAI authentication completed. You can close this window.",
                Some(code),
            ),
            _ => ("400 Bad Request", "Missing authorization code.", None),
        };
        let page = format!(
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>Scramjet</title></head>\
             <body><p>{said}</p></body></html>"
        );
        let answer = format!(
            "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{page}",
            page.len()
        );
        let _ = connection.write_all(answer.as_bytes());
        code
    }
}

/// Saves the models ChatGPT lists for the login `credential`, in place of those listed before,
/// asked for as Codex's current release asks; with a warning when npm, out of reach, left them
/// asked for as the release known last.
fn save_models(
    credential: &Value,
    urls: &Urls,
    logins: &login::Logins,
    cancel: &AtomicBool,
) -> Result<Option<String>, String> {
    let path = logins.listed(PROVIDER);
    let before = std::fs::read(&path).ok();
    let before = before.and_then(|text| serde_json::from_slice::<Value>(&text).ok());
    let (release, warning) = match codex_release(&urls.release, cancel) {
        Ok(release) => (release, None),
        Err(error) if error == login::CANCELLED => return Err(error),
        Err(error) => match before.as_ref().and_then(|list| list["release"].as_str()) {
            Some(known) => {
                let warning = format!(
                    "{error}; ChatGPT's models are those it lists for Codex {known}, the release \
                     known last"
                );
                (known.to_owned(), Some(warning))
            }
            None => return Err(error),
        },
    };
    let models = models(credential, &urls.api, &release, cancel)?;
    let list = json!({"fetched": login::now_ms(), "release": release, "models": models});
    login::save_list(&path, &list)?;
    Ok(warning)
}

/// Reads again the models ChatGPT lists for the login saved in `logins`, as `save_models`.
pub(super) fn relist(logins: &login::Logins) -> Result<Option<String>, String> {
    relist_at(logins, &Urls::default())
}

fn relist_at(logins: &login::Logins, urls: &Urls) -> Result<Option<String>, String> {
    login::access(&logins.auth, PROVIDER, None)?;
    let credential = login::saved(&logins.auth)?.remove(PROVIDER);
    let credential = credential.ok_or("not logged in to ChatGPT (/login)")?;
    save_models(&credential, urls, logins, &AtomicBool::new(false))
}

/// The access token renewed with the refresh token `credential` keeps.
pub(super) fn refresh(credential: &Value) -> Result<Value, Failure> {
    refresh_at(&Urls::default().token, credential)
}

fn refresh_at(url: &str, credential: &Value) -> Result<Value, Failure> {
    let refresh = credential["refresh"]
        .as_str()
        .filter(|token| !token.is_empty());
    let refresh = refresh.ok_or_else(|| {
        Failure::from("the ChatGPT login keeps no refresh token: /login again".to_owned())
    })?;
    let form = form(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", CLIENT_ID),
    ]);
    tokens(url, form, &AtomicBool::new(false)).map_err(|failure| Failure {
        message: format!(
            "{} (renewing the ChatGPT token; /login if it goes on)",
            failure.message
        ),
        ..failure
    })
}

/// The credential OpenAI's token endpoint at `url` answers `form` with, in Scramjet's format.
fn tokens(url: &str, form: String, cancel: &AtomicBool) -> Result<Value, Failure> {
    let headers = [("Accept", "application/json")];
    let body = Some(("application/x-www-form-urlencoded", form.into_bytes()));
    let fetched = cancellable(cancel, client::fetch(PROVIDER, "POST", url, &headers, body));
    let answer = login::renewal(fetched, "OpenAI's token endpoint")?;
    let access = answer["access_token"].as_str();
    let (Some(access), Some(refresh), Some(seconds)) = (
        access,
        answer["refresh_token"].as_str(),
        answer["expires_in"].as_f64(),
    ) else {
        return Err("OpenAI's token answer lacks a token or its expiry"
            .to_owned()
            .into());
    };
    let account = account_id(access).ok_or_else(|| {
        Failure::from("OpenAI's access token names no ChatGPT account".to_owned())
    })?;
    Ok(json!({
        "type": "oauth",
        "access": access,
        "refresh": refresh,
        "expires": login::now_ms() + seconds * 1000.0,
        "accountId": account,
    }))
}

/// The models ChatGPT's backend at `api` lists for the account, asked for as Codex's `release`
/// asks, with what is used of each.
fn models(
    credential: &Value,
    api: &str,
    release: &str,
    cancel: &AtomicBool,
) -> Result<Vec<Value>, String> {
    let token = credential["access"].as_str().unwrap_or_default();
    let account = credential["accountId"].as_str().unwrap_or_default();
    let (authorization, agent) = (format!("Bearer {token}"), user_agent(CODEX, release));
    let headers = [
        ("Accept", "application/json"),
        ("Authorization", authorization.as_str()),
        ("chatgpt-account-id", account),
        ("originator", CODEX),
        ("User-Agent", agent.as_str()),
    ];
    let url = format!("{api}/codex/models?client_version={release}");
    let fetched = cancellable(cancel, client::fetch(PROVIDER, "GET", &url, &headers, None))?;
    let listed = login::answer(fetched, "ChatGPT's model list")?;
    let models = listed["models"].as_array();
    let models = models.ok_or("ChatGPT's model list has no models field")?;
    let kept = models.iter().map(|model| {
        let fields = LISTED_FIELDS.iter().filter_map(|field| {
            let value = model.get(*field)?;
            Some((field.to_string(), value.clone()))
        });
        Value::Object(fields.collect())
    });
    Ok(kept.collect())
}

/// Codex's current release (`0.166.0`), as npm's record at `url` names it.
fn codex_release(url: &str, cancel: &AtomicBool) -> Result<String, String> {
    let headers = [("Accept", "application/json")];
    let fetched = cancellable(cancel, client::fetch(PROVIDER, "GET", url, &headers, None))?;
    let record = login::answer(fetched, "npm's record of Codex")?;
    let version = record["version"].as_str().unwrap_or_default();
    let parts: Vec<&str> = version.split('.').collect();
    match parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        true => Ok(version.to_owned()),
        false => Err(format!("npm names no release of Codex: {version:?}")),
    }
}

/// The provider the models a login `listed` make, as a `models.json` entry: Codex's backend with
/// the headers Scramjet's requests carry, and the models Codex's own picker offers.
pub(super) fn entry(listed: &Value) -> Value {
    let models: Vec<Value> = (listed["models"].as_array().into_iter().flatten())
        .filter_map(model)
        .collect();
    json!({
        "baseUrl": BASE_URL,
        "api": "openai-codex-responses",
        "headers": {
            "originator": ORIGINATOR,
            "User-Agent": user_agent("scramjet-rs", env!("CARGO_PKG_VERSION")),
            "OpenAI-Beta": "responses=experimental",
        },
        "models": models,
    })
}

/// A model the backend listed, as a `models.json` model; `None` for one Codex's picker hides
/// (its reviews' model, models held back). Its window is the most the backend allows, as
/// Scramjet's catalog gives it, rather than Codex's default.
fn model(listed: &Value) -> Option<Value> {
    let id = listed["slug"].as_str().filter(|id| !id.is_empty())?;
    if listed["visibility"]
        .as_str()
        .is_some_and(|shown| shown != "list")
    {
        return None;
    }
    let efforts: Vec<&str> = (listed["supported_reasoning_levels"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|level| level["effort"].as_str())
        .collect();
    // Codex takes a model that says nothing of its input to take images.
    let images = (listed["input_modalities"].as_array())
        .is_none_or(|kinds| kinds.iter().any(|kind| kind == "image"));
    let number = |value: &Value| value.as_u64().filter(|n| *n > 0);
    let window =
        number(&listed["max_context_window"]).or_else(|| number(&listed["context_window"]));
    let mut entry = json!({
        "id": id,
        "reasoning": !efforts.is_empty(),
        "input": match images {
            true => json!(["text", "image"]),
            false => json!(["text"]),
        },
    });
    if let Some(name) = listed["display_name"].as_str() {
        entry["name"] = json!(name);
    }
    if let Some(window) = window {
        entry["contextWindow"] = json!(window);
    }
    if !efforts.is_empty() {
        entry["thinkingLevelMap"] = super::providers::level_map(&efforts);
    }
    Some(entry)
}

/// Where a request to Codex's backend at `base` (a base URL's path) goes, as Scramjet's
/// client resolves it: `{base}/codex/responses`, unless `base` ends with either part.
pub(super) fn path(base: &str) -> String {
    let base = base.trim_end_matches('/');
    match () {
        _ if base.ends_with("/codex/responses") => base.to_owned(),
        _ if base.ends_with("/codex") => format!("{base}/responses"),
        _ => format!("{base}/codex/responses"),
    }
}

/// `request` as Codex's backend takes it, as Scramjet's client sends it: without an output
/// limit, which Codex's own client never sets, and asking for short text.
pub(super) fn body(request: &Value) -> Value {
    let mut body = request.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("max_output_tokens");
    }
    body["text"] = json!({"verbosity": "low"});
    body
}

/// The headers a request to Codex's backend carries besides its route's, as Scramjet's: the
/// account its token `token` names, and the session.
pub(super) fn request_headers(
    token: &str,
    request: &Value,
) -> Result<Vec<(&'static str, String)>, String> {
    let account = account_id(token).ok_or("the ChatGPT token names no account: /login again")?;
    let mut headers = vec![("chatgpt-account-id", account)];
    if let Some(session) = request["prompt_cache_key"].as_str() {
        headers.push(("session_id", session.to_owned()));
        headers.push(("x-client-request-id", session.to_owned()));
    }
    Ok(headers)
}

/// What an error answer says of a ChatGPT usage limit, as Scramjet words it; `None` for any
/// other answer. Waiting a few seconds does not lift it.
pub(super) fn usage_limit(text: &str) -> Option<String> {
    let answer: Value = serde_json::from_str(text).ok()?;
    let error = &answer["error"];
    let code = error["code"].as_str().or(error["type"].as_str())?;
    let limits = [
        "usage_limit_reached",
        "usage_not_included",
        "insufficient_quota",
        "billing_hard_limit_reached",
    ];
    if !limits.contains(&code) {
        return None;
    }
    let plan = (error["plan_type"].as_str()).map_or_else(String::new, |plan| {
        format!(" ({} plan)", plan.to_lowercase())
    });
    let when = error["resets_at"].as_f64().map_or_else(String::new, |at| {
        let minutes = ((at * 1000.0 - login::now_ms()) / 60_000.0)
            .round()
            .max(0.0);
        format!(" Try again in ~{minutes} min.")
    });
    Some(format!(
        "You have hit your ChatGPT usage limit{plan}.{when}"
    ))
}

/// The ChatGPT account an access token names, from its claims.
pub(super) fn account_id(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    let [_, payload, _] = parts[..] else {
        return None;
    };
    let claims: Value = serde_json::from_slice(&decode_base64(payload)?).ok()?;
    let account = claims[CLAIM]["chatgpt_account_id"].as_str();
    account.filter(|id| !id.is_empty()).map(str::to_owned)
}

/// What client `name` at `version` says it is, as Codex's own says it.
fn user_agent(name: &str, version: &str) -> String {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease");
    let release = release.map_or_else(|_| String::new(), |release| format!(" {}", release.trim()));
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    format!("{name}/{version} ({os}{release}; {arch})")
}

/// `future`, unless `cancel` is set first.
fn cancellable<T, E: From<String>>(
    cancel: &AtomicBool,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    login::blocking(async {
        tokio::select! {
            biased;
            () = client::cancelled(cancel) => Err(E::from(login::CANCELLED.to_owned())),
            done = future => done,
        }
    })
}

/// The code and state in what the user pasted.
fn parse_pasted(text: &str) -> (Option<String>, Option<String>) {
    let text = text.trim();
    if text.is_empty() {
        return (None, None);
    }
    // An address, also without its scheme (`localhost:1455/…`), or its query (`?code=…`).
    if text.contains("://") || text.contains('?') {
        let query = text.split_once('?').map_or("", |(_, query)| query);
        let query = query.split('#').next().unwrap_or_default();
        return (query_value(query, "code"), query_value(query, "state"));
    }
    if let Some((code, state)) = text.split_once('#') {
        return (Some(code.to_owned()), Some(state.to_owned()));
    }
    if text.contains("code=") {
        return (query_value(text, "code"), query_value(text, "state"));
    }
    (Some(text.to_owned()), None)
}

/// The value of `name` in a URL's query.
fn query_value(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(key) == name).then(|| decode(value))
    })
}

/// `pairs` as a form encodes them (`application/x-www-form-urlencoded`).
fn form(pairs: &[(&str, &str)]) -> String {
    let pairs = pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)));
    pairs.collect::<Vec<_>>().join("&")
}

fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let hex = text
            .get(at + 1..at + 3)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
                continue;
            }
            (b'+', _) => decoded.push(b' '),
            (byte, _) => decoded.push(byte),
        }
        at += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn random(count: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; count];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| format!("cannot get random bytes: {error}"))?;
    Ok(bytes)
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `bytes` in base64url without padding, as PKCE takes them.
fn base64url(bytes: &[u8]) -> String {
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let bits = chunk
            .iter()
            .fold(0u32, |bits, byte| bits << 8 | u32::from(*byte));
        let bits = bits << (8 * (3 - chunk.len()));
        for index in 0..=chunk.len() {
            text.push(BASE64URL[(bits >> (18 - 6 * index) & 63) as usize] as char);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::{ScratchDir, serve_json};

    /// An access token naming the ChatGPT account `account`, as OpenAI's do.
    fn token(account: &str) -> String {
        let claims = json!({CLAIM: {"chatgpt_account_id": account}, "exp": 1, "sub": "s?>"});
        let header = base64url(br#"{"alg":"RS256"}"#);
        format!("{header}.{}.c2ln", base64url(claims.to_string().as_bytes()))
    }

    fn listed() -> Value {
        let levels = |levels: &[&str]| -> Vec<Value> {
            (levels.iter())
                .map(|effort| json!({"effort": effort, "description": "…"}))
                .collect()
        };
        json!([
            {"slug": "gpt-6.1-sol", "display_name": "GPT-6.1-Sol", "visibility": "list",
             "context_window": 500000, "max_context_window": 872000,
             "supported_reasoning_levels": levels(&["low", "medium", "high", "xhigh", "max", "ultra"]),
             "input_modalities": ["text", "image"], "priority": 1,
             "model_messages": {"persistent_instructions": "## Overview"}},
            {"slug": "codex-auto-review", "visibility": "hide", "context_window": 272000,
             "supported_reasoning_levels": levels(&["low"])},
            {"slug": "plain", "display_name": "Plain", "visibility": "list",
             "context_window": 128000, "supported_reasoning_levels": [],
             "input_modalities": ["text"]},
            {"slug": "bare"},
        ])
    }

    /// OpenAI, npm and ChatGPT on a closed port: a test that should send nothing reaches none.
    pub(super) fn closed() -> Urls {
        Urls {
            authorize: "http://127.0.0.1:1/oauth/authorize".to_owned(),
            token: "http://127.0.0.1:1/oauth/token".to_owned(),
            api: "http://127.0.0.1:1/backend-api".to_owned(),
            release: "http://127.0.0.1:1/codex".to_owned(),
        }
    }

    fn scratch(name: &str) -> ScratchDir {
        ScratchDir::new(
            std::env::temp_dir().join(format!("scramjet-codex-{name}-{}", std::process::id())),
        )
    }

    #[test]
    fn pkce_and_tokens_use_base64url_as_rfc_7636_shows() {
        let octets = [
            116, 24, 223, 180, 151, 153, 224, 37, 79, 250, 96, 125, 216, 173, 187, 186, 22, 212,
            37, 77, 105, 214, 191, 240, 91, 88, 5, 88, 83, 132, 141, 121,
        ];
        let verifier = base64url(&octets);
        assert_eq!(verifier, "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(decode_base64(&verifier).unwrap(), octets);
        for length in 0..7 {
            let bytes: Vec<u8> = (0..length).map(|n| 250 - n as u8).collect();
            assert_eq!(decode_base64(&base64url(&bytes)).unwrap(), bytes);
        }
        assert_eq!(
            decode_base64("a+b/").unwrap(),
            decode_base64("a-b_").unwrap()
        );
        assert_eq!(decode_base64("abcde"), None);
        assert_eq!(decode_base64("ab$d"), None);
        // The account is the token's claim; anything else names none.
        assert_eq!(account_id(&token("acc-1")).as_deref(), Some("acc-1"));
        assert_eq!(account_id(&token("")), None);
        assert_eq!(account_id("a.b"), None);
        assert_eq!(account_id("x.!!!.z"), None);
    }

    #[test]
    fn a_browser_login_takes_the_browsers_answer_then_saves_the_tokens_and_the_models() {
        let access = token("acc-1");
        let served = access.clone();
        let (base, server) = serve_json(3, move |head, _| {
            let path = head.split(' ').nth(1).unwrap_or_default();
            match path {
                "/oauth/token" => json!({"access_token": served, "refresh_token": "rt-1",
                    "id_token": "id", "expires_in": 864000}),
                "/%40openai%2Fcodex/latest" => {
                    json!({"name": "@openai/codex", "version": "0.170.0"})
                }
                "/backend-api/codex/models?client_version=0.170.0" => json!({"models": listed()}),
                _ => json!({}),
            }
        });
        let urls = Urls {
            authorize: "https://auth.example/oauth/authorize".to_owned(),
            token: format!("{base}/oauth/token"),
            api: format!("{base}/backend-api"),
            release: format!("{base}/%40openai%2Fcodex/latest"),
        };
        let browser = BrowserLogin::start_at(urls, "127.0.0.1:0").unwrap();
        assert!(browser.listening());
        let (page, query) = browser.url.split_once('?').unwrap();
        assert_eq!(page, "https://auth.example/oauth/authorize");
        let asked = |name| query_value(query, name).unwrap();
        assert_eq!(asked("client_id"), CLIENT_ID);
        assert_eq!(asked("redirect_uri"), REDIRECT_URI);
        assert!(
            query.contains("&scope=openid+profile+email+offline_access&"),
            "{query}"
        );
        assert_eq!(asked("code_challenge_method"), "S256");
        let challenge = base64url(&Sha256::digest(browser.verifier.as_bytes()));
        assert_eq!(asked("code_challenge"), challenge);
        assert_eq!(asked("state"), browser.state);
        assert_eq!(asked("state").len(), 32);
        assert_eq!(asked("originator"), "scramjet-rs");
        assert_eq!(asked("codex_cli_simplified_flow"), "true");
        let callback = browser.listener.as_ref().unwrap().local_addr().unwrap();
        let (state, verifier) = (browser.state.clone(), browser.verifier.clone());
        // The browser asks for another page, brings another login's answer, then this one's.
        let browsing = std::thread::spawn(move || {
            let get = |target: &str| {
                let mut connection = TcpStream::connect(callback).unwrap();
                write!(
                    connection,
                    "GET {target} HTTP/1.1\r\nHost: localhost:1455\r\n\r\n"
                )
                .unwrap();
                let mut page = String::new();
                connection.read_to_string(&mut page).unwrap();
                page
            };
            [
                get("/favicon.ico"),
                get("/auth/callback?code=c-0&state=other"),
                get(&format!("/auth/callback?state={state}")),
                get(&format!(
                    "/auth/callback?code=c%2B1&state={state}&scope=openid"
                )),
            ]
        });
        let scratch = scratch("login");
        let logins = login::Logins {
            auth: scratch.0.join("config/auth.json"),
            lists: scratch.0.join("state/logins"),
        };
        let (_paste, pasted) = mpsc::channel();
        let answered = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        let warned = browser
            .finish(&pasted, &answered, &logins, &cancel)
            .unwrap();
        assert_eq!(warned, None);
        assert!(answered.load(Ordering::Relaxed));
        let pages = browsing.join().unwrap();
        let said = |page: &str| {
            (
                page.lines().next().unwrap().to_owned(),
                page.contains("You can close"),
            )
        };
        assert_eq!(
            said(&pages[0]),
            ("HTTP/1.1 404 Not Found".to_owned(), false)
        );
        assert!(pages[1].starts_with("HTTP/1.1 400") && pages[1].contains("State mismatch."));
        assert!(
            pages[2].contains("Missing authorization code."),
            "{}",
            pages[2]
        );
        assert_eq!(said(&pages[3]), ("HTTP/1.1 200 OK".to_owned(), true));
        let requests = server.join().unwrap();
        let lines: Vec<&str> = (requests.iter())
            .map(|(head, _)| head.lines().next().unwrap())
            .collect();
        // The list is asked for as Codex's current release asks for it.
        assert_eq!(
            lines,
            [
                "POST /oauth/token HTTP/1.1",
                "GET /%40openai%2Fcodex/latest HTTP/1.1",
                "GET /backend-api/codex/models?client_version=0.170.0 HTTP/1.1"
            ]
        );
        let redirect = "http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback";
        assert_eq!(
            requests[0].1,
            format!(
                "grant_type=authorization_code&client_id={CLIENT_ID}&code=c%2B1&code_verifier={verifier}&redirect_uri={redirect}"
            )
        );
        assert!(!requests[1].0.to_lowercase().contains("authorization"));
        let head = &requests[2].0;
        for header in [
            format!("authorization: Bearer {access}\r\n"),
            "chatgpt-account-id: acc-1\r\n".to_owned(),
            "originator: codex_cli_rs\r\n".to_owned(),
            "user-agent: codex_cli_rs/0.170.0 (".to_owned(),
        ] {
            assert!(head.contains(&header), "{header} in {head}");
        }
        let saved = login::saved(&logins.auth).unwrap();
        let credential = &saved[PROVIDER];
        assert_eq!(credential["type"], "oauth");
        assert_eq!(
            (
                credential["access"].as_str(),
                credential["refresh"].as_str()
            ),
            (Some(access.as_str()), Some("rt-1"))
        );
        assert_eq!(credential["accountId"], "acc-1");
        let expires = credential["expires"].as_f64().unwrap() - login::now_ms();
        assert!(
            (863_990_000.0..=864_000_000.0).contains(&expires),
            "{expires}"
        );
        // The list keeps what is used of each model, not its instructions.
        let text = std::fs::read_to_string(logins.listed(PROVIDER)).unwrap();
        let list: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(list["models"].as_array().unwrap().len(), 4);
        assert_eq!(list["release"], "0.170.0");
        assert!(list["models"][0].get("model_messages").is_none());
        assert!(list["models"][0].get("priority").is_none());
        assert_eq!(list["models"][0]["max_context_window"], 872000);
    }

    #[test]
    fn a_pasted_address_or_code_is_the_answer_unless_it_is_another_logins() {
        let browser = BrowserLogin::start_at(closed(), "127.0.0.1:0").unwrap();
        let state = browser.state.clone();
        let address =
            format!("http://localhost:1455/auth/callback?code=ac_1%2F2&scope=x&state={state}");
        for (text, code) in [
            (address.as_str(), "ac_1/2"),
            (&format!(" code=ab&state={state} "), "ab"),
            (&format!("?code=ab&state={state}"), "ab"),
            (
                &format!("localhost:1455/auth/callback?code=ab&state={state}"),
                "ab",
            ),
            (&format!("ab#{state}"), "ab"),
            ("just-the-code", "just-the-code"),
            ("http://localhost:1455/auth/callback?code=c#fragment", "c"),
        ] {
            assert_eq!(browser.pasted(text).unwrap(), code, "{text}");
        }
        for text in [
            "",
            "  ",
            "http://localhost:1455/auth/callback?state=x&code=c",
            "ab#other",
        ] {
            assert!(browser.pasted(text).is_err(), "{text}");
        }
        // Another login's answer ends the login before anything is sent.
        let (paste, pasted) = mpsc::channel();
        paste.send("code=c&state=other".to_owned()).unwrap();
        let answered = AtomicBool::new(false);
        let nowhere = login::Logins {
            auth: "/nonexistent/auth.json".into(),
            lists: "/nonexistent".into(),
        };
        let finished = browser.finish(&pasted, &answered, &nowhere, &AtomicBool::new(false));
        assert_eq!(
            finished.unwrap_err(),
            "the pasted address is another login's (its state differs)"
        );
        assert!(!answered.load(Ordering::Relaxed));
        // Cancelled, it stops waiting.
        let browser = BrowserLogin::start_at(closed(), "127.0.0.1:0").unwrap();
        let (_paste, pasted) = mpsc::channel();
        let finished = browser.finish(&pasted, &answered, &nowhere, &AtomicBool::new(true));
        assert_eq!(finished.unwrap_err(), login::CANCELLED);
        // With the port taken, as by another login, the answer is to be pasted.
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = taken.local_addr().unwrap().to_string();
        assert!(
            !BrowserLogin::start_at(closed(), &address)
                .unwrap()
                .listening()
        );
    }

    #[test]
    fn a_list_that_fails_keeps_the_login_and_says_so() {
        let served = token("acc-1");
        let (base, server) = serve_json(3, move |head, _| match head.split(' ').nth(1) {
            Some("/oauth/token") => {
                json!({"access_token": served, "refresh_token": "rt", "expires_in": 60})
            }
            Some("/codex") => json!({"version": "0.170.0"}),
            _ => json!({"detail": "not here"}),
        });
        let urls = Urls {
            authorize: String::new(),
            token: format!("{base}/oauth/token"),
            api: base.clone(),
            release: format!("{base}/codex"),
        };
        let browser = BrowserLogin::start_at(urls, "127.0.0.1:0").unwrap();
        let (paste, pasted) = mpsc::channel();
        paste.send(format!("c#{}", browser.state)).unwrap();
        let scratch = scratch("unlisted");
        let logins = login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let answered = AtomicBool::new(false);
        let finished = browser.finish(&pasted, &answered, &logins, &AtomicBool::new(false));
        assert_eq!(
            finished.unwrap_err(),
            "logged in, but ChatGPT's model list has no models field"
        );
        assert_eq!(login::logged_in(&logins.auth).unwrap(), [PROVIDER]);
        assert!(!logins.listed(PROVIDER).exists());
        server.join().unwrap();
        // Cancelled once the tokens are saved, the login is kept, and says so.
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (served, cancelling) = (token("acc-2"), cancel.clone());
        let (base, _server) = serve_json(2, move |head, _| match head.split(' ').nth(1) {
            Some("/oauth/token") => {
                json!({"access_token": served, "refresh_token": "rt", "expires_in": 60})
            }
            _ => {
                cancelling.store(true, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(300));
                json!({})
            }
        });
        let urls = Urls {
            authorize: String::new(),
            token: format!("{base}/oauth/token"),
            api: base.clone(),
            release: format!("{base}/codex"),
        };
        let browser = BrowserLogin::start_at(urls, "127.0.0.1:0").unwrap();
        let (paste, pasted) = mpsc::channel();
        paste.send(format!("c#{}", browser.state)).unwrap();
        std::fs::remove_file(&logins.auth).unwrap();
        let finished = browser.finish(&pasted, &answered, &logins, &cancel);
        assert_eq!(
            finished.unwrap_err(),
            "logged in, but the model list was cancelled: /login again for it"
        );
        assert_eq!(login::logged_in(&logins.auth).unwrap(), [PROVIDER]);
        // npm's record must name a release, which goes into the list's request.
        let (base, server) = serve_json(1, |_, _| json!({"version": "1.2.3&x=y"}));
        let named = codex_release(&format!("{base}/codex"), &AtomicBool::new(false));
        assert_eq!(
            named.unwrap_err(),
            r#"npm names no release of Codex: "1.2.3&x=y""#
        );
        server.join().unwrap();
    }

    #[test]
    fn without_npm_the_list_is_asked_for_as_the_release_known_last() {
        let served = token("acc-1");
        let (base, server) = serve_json(5, move |head, _| match head.split(' ').nth(1) {
            Some("/oauth/token") => {
                json!({"access_token": served, "refresh_token": "rt", "expires_in": 60})
            }
            Some("/codex/models?client_version=0.165.0") => json!({"models": listed()}),
            _ => json!({"error": "Not found"}),
        });
        let urls = Urls {
            authorize: String::new(),
            token: format!("{base}/oauth/token"),
            api: base.clone(),
            release: format!("{base}/%40openai%2Fcodex/latest"),
        };
        let scratch = scratch("known-release");
        let logins = login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let log_in = || {
            let browser = BrowserLogin::start_at(urls.clone(), "127.0.0.1:0").unwrap();
            let (paste, pasted) = mpsc::channel();
            paste.send(format!("c#{}", browser.state)).unwrap();
            let answered = AtomicBool::new(false);
            browser.finish(&pasted, &answered, &logins, &AtomicBool::new(false))
        };
        // None known yet: the login is kept, without models.
        assert_eq!(
            log_in().unwrap_err(),
            r#"logged in, but npm names no release of Codex: """#
        );
        assert_eq!(login::logged_in(&logins.auth).unwrap(), [PROVIDER]);
        let known = json!({"fetched": 0, "release": "0.165.0", "models": []});
        login::save_list(&logins.listed(PROVIDER), &known).unwrap();
        assert_eq!(
            log_in().unwrap().unwrap(),
            r#"npm names no release of Codex: ""; ChatGPT's models are those it lists for Codex 0.165.0, the release known last"#
        );
        let list = std::fs::read_to_string(logins.listed(PROVIDER)).unwrap();
        let list: Value = serde_json::from_str(&list).unwrap();
        let models = list["models"].as_array().unwrap().len();
        assert_eq!((list["release"].as_str(), models), (Some("0.165.0"), 4));
        server.join().unwrap();
    }

    #[test]
    fn a_saved_login_reads_its_models_again_as_codexs_current_release() {
        let scratch = scratch("relist");
        let logins = login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let credential = json!({"type": "oauth", "access": token("acc-1"), "refresh": "rt",
            "expires": login::now_ms() + 60_000.0, "accountId": "acc-1"});
        login::save(&logins.auth, PROVIDER, credential).unwrap();
        let before = json!({"fetched": 0, "release": "0.165.0", "models": []});
        login::save_list(&logins.listed(PROVIDER), &before).unwrap();
        let read = || {
            let text = std::fs::read_to_string(logins.listed(PROVIDER)).unwrap();
            serde_json::from_str::<Value>(&text).unwrap()
        };
        // Neither npm nor ChatGPT answering, the list stays as it was.
        assert!(relist_at(&logins, &closed()).is_err());
        assert_eq!(read(), before);
        let (base, server) = serve_json(2, |head, _| match head.split(' ').nth(1) {
            Some("/codex/latest") => json!({"version": "0.171.0"}),
            Some("/codex/models?client_version=0.171.0") => json!({"models": listed()}),
            _ => json!({}),
        });
        let urls = Urls {
            api: base.clone(),
            release: format!("{base}/codex/latest"),
            ..closed()
        };
        assert_eq!(relist_at(&logins, &urls), Ok(None));
        let requests = server.join().unwrap();
        assert!(requests[1].0.contains("chatgpt-account-id: acc-1\r\n"));
        let list = read();
        let models = list["models"].as_array().unwrap().len();
        assert_eq!((list["release"].as_str(), models), (Some("0.171.0"), 4));
        assert!(list["fetched"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn a_token_is_renewed_with_the_refresh_token_which_changes() {
        let access = token("acc-2");
        let served = access.clone();
        let (base, server) = serve_json(2, move |_, body| match body.contains("rt-1") {
            true => json!({"access_token": served, "refresh_token": "rt-2", "expires_in": 3600}),
            false => json!({"error": "invalid_grant"}),
        });
        let url = format!("{base}/oauth/token");
        let credential = json!({"type": "oauth", "access": "old", "refresh": "rt-1", "expires": 0});
        let renewed = refresh_at(&url, &credential).unwrap();
        assert_eq!(renewed["access"], access);
        assert_eq!(
            (renewed["refresh"].as_str(), renewed["accountId"].as_str()),
            (Some("rt-2"), Some("acc-2"))
        );
        let failed = refresh_at(&url, &json!({"refresh": "rt-0"})).unwrap_err();
        assert_eq!(
            (failed.message.as_str(), failed.transient),
            (
                "OpenAI's token answer lacks a token or its expiry (renewing the ChatGPT token; /login if it goes on)",
                false
            )
        );
        let requests = server.join().unwrap();
        assert_eq!(
            requests[0].1,
            format!("grant_type=refresh_token&refresh_token=rt-1&client_id={CLIENT_ID}")
        );
        // OpenAI out of reach, it may answer later.
        let failed = refresh(&credential).unwrap_err();
        assert!(failed.transient, "{failed:?}");
        assert!(
            failed
                .message
                .ends_with(" (renewing the ChatGPT token; /login if it goes on)")
        );
        let failed = refresh(&json!({"type": "oauth"})).unwrap_err();
        assert!(failed.message.contains("/login again") && !failed.transient);
    }

    #[test]
    fn the_models_listed_become_a_provider_on_codexs_backend() {
        let entry = entry(&json!({"models": listed()}));
        let levels = super::super::providers::level_map(&["low", "medium", "high", "xhigh", "max"]);
        assert_eq!(
            entry["models"],
            json!([
                {"id": "gpt-6.1-sol", "name": "GPT-6.1-Sol", "reasoning": true,
                 "input": ["text", "image"], "contextWindow": 872000, "thinkingLevelMap": levels},
                {"id": "plain", "name": "Plain", "reasoning": false, "input": ["text"],
                 "contextWindow": 128000},
                {"id": "bare", "reasoning": false, "input": ["text", "image"]},
            ])
        );
        let providers = super::super::Providers::given(json!({"providers": {PROVIDER: entry}}));
        let model = providers.find("openai-codex/gpt-6.1-sol").unwrap();
        assert_eq!(model.info.protocol, client::Protocol::Codex);
        assert_eq!(model.info.levels, ["low", "medium", "high", "xhigh", "max"]);
        assert_eq!((model.info.window, model.info.images), (Some(872000), true));
        let route = model.info.route.as_ref().unwrap();
        assert_eq!(
            (route.authority.as_str(), route.path.as_str(), route.tls),
            ("chatgpt.com", "/backend-api", true)
        );
        for (name, value) in [
            ("originator", "scramjet-rs"),
            ("openai-beta", "responses=experimental"),
        ] {
            assert!(
                route.headers.contains(&(name.into(), value.into())),
                "{name}"
            );
        }
        assert_eq!(path(&route.path), "/backend-api/codex/responses");
        assert_eq!(path("/x/codex/"), "/x/codex/responses");
        assert_eq!(path("/x/codex/responses"), "/x/codex/responses");
    }

    #[test]
    fn a_usage_limit_says_the_plan_and_when_it_lifts() {
        let resets = (login::now_ms() / 1000.0) + 42.0 * 60.0;
        let answer = json!({"error": {"code": "usage_limit_reached", "message": "The usage limit has been reached",
            "plan_type": "PLUS", "resets_at": resets}});
        assert_eq!(
            usage_limit(&answer.to_string()).unwrap(),
            "You have hit your ChatGPT usage limit (plus plan). Try again in ~42 min."
        );
        let answer = json!({"error": {"type": "usage_not_included"}});
        assert_eq!(
            usage_limit(&answer.to_string()).unwrap(),
            "You have hit your ChatGPT usage limit."
        );
        let other = json!({"error": {"code": "rate_limit_exceeded", "message": "slow down"}});
        assert_eq!(usage_limit(&other.to_string()), None);
        assert_eq!(usage_limit("Too Many Requests"), None);
    }

    #[test]
    fn a_request_names_its_account_and_session_and_asks_for_no_output_limit() {
        let request = json!({"model": "m", "prompt_cache_key": "s-1", "max_output_tokens": 9,
            "reasoning": {"effort": "low"}});
        let headers = request_headers(&token("acc-3"), &request).unwrap();
        let expected = [
            ("chatgpt-account-id", "acc-3"),
            ("session_id", "s-1"),
            ("x-client-request-id", "s-1"),
        ];
        assert_eq!(
            headers,
            expected.map(|(name, value)| (name, value.to_owned()))
        );
        assert!(
            request_headers("not-a-token", &request)
                .unwrap_err()
                .contains("/login again")
        );
        let body = body(&request);
        assert!(body.get("max_output_tokens").is_none());
        assert_eq!(body["text"], json!({"verbosity": "low"}));
        assert_eq!(body["reasoning"], request["reasoning"]);
    }
}
