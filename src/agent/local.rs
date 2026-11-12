//! Models served on this machine or at an address given: Ollama, llama.cpp's server and vLLM,
//! found at their default ports or told apart by what they answer, each model's context window
//! read from the runtime itself, and a check that a model calls a harmless tool, for `/login`
//! to save them as a provider of the user's `models.json`.

use super::client;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

/// How long a default port may take to answer when looking around.
const LOOK_WAIT: Duration = Duration::from_millis(1500);
/// How long a runtime may take to describe itself and its models.
const PROBE_WAIT: Duration = Duration::from_secs(10);
/// How long the tool check may take: the runtime may load the model first.
const CHECK_WAIT: Duration = Duration::from_secs(300);
const ANSWER_BYTES: usize = 4 << 20;

/// The tool the check offers; it does nothing, and the model is asked to call it.
pub const CHECK_TOOL: &str = "echo_word";
/// What the check asks the model.
pub const CHECK_PROMPT: &str = "Call the tool echo_word with the word \"scramjet\".";
/// What a model that thinks is asked with its thinking turned off.
const SWITCH_PROMPT: &str = "What is 17 times 23? Answer with the number only.";
const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    Ollama,
    LlamaCpp,
    Vllm,
    /// An OpenAI-compatible server that is none of the others.
    Other,
}

impl Runtime {
    /// Those looked for at their default ports.
    pub const LOOKED_FOR: [Runtime; 3] = [Runtime::Ollama, Runtime::LlamaCpp, Runtime::Vllm];

    pub fn name(self) -> &'static str {
        match self {
            Runtime::Ollama => "Ollama",
            Runtime::LlamaCpp => "llama.cpp",
            Runtime::Vllm => "vLLM",
            Runtime::Other => "OpenAI-compatible server",
        }
    }

    /// Its name in a saved entry's `runtime`, and the provider's name it is saved under.
    pub fn key(self) -> &'static str {
        match self {
            Runtime::Ollama => "ollama",
            Runtime::LlamaCpp => "llama-cpp",
            Runtime::Vllm => "vllm",
            Runtime::Other => "local",
        }
    }

    pub fn of(key: &str) -> Option<Runtime> {
        [
            Runtime::Ollama,
            Runtime::LlamaCpp,
            Runtime::Vllm,
            Runtime::Other,
        ]
        .into_iter()
        .find(|runtime| runtime.key() == key)
    }

    pub fn port(self) -> Option<u16> {
        match self {
            Runtime::Ollama => Some(11434),
            Runtime::LlamaCpp => Some(8080),
            Runtime::Vllm => Some(8000),
            Runtime::Other => None,
        }
    }

    /// What to change when a model answers without calling the tool.
    fn tools_hint(self) -> &'static str {
        match self {
            Runtime::Ollama => {
                "Ollama gives the model's own template; another model may call tools"
            }
            Runtime::LlamaCpp => "llama-server calls tools only when started with --jinja",
            Runtime::Vllm => {
                "vllm serve calls tools only when started with --enable-auto-tool-choice and \
                 the model's --tool-call-parser"
            }
            Runtime::Other => "the server or the model may not support tool calls",
        }
    }

    /// What to change when a model's thinking comes as text in its answer.
    fn thinking_hint(self) -> &'static str {
        match self {
            Runtime::Vllm => {
                "vllm serve sends it apart when started with the model's --reasoning-parser \
                 (qwen3 for Qwen3, deepseek_r1 for DeepSeek-R1)"
            }
            Runtime::LlamaCpp => {
                "llama-server sends it apart when started with --jinja --reasoning-format deepseek"
            }
            Runtime::Ollama | Runtime::Other => "the server does not send it apart",
        }
    }
}

/// How requests turn a model's thinking on and off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// `reasoning_effort`, `none` for off: Ollama's.
    Effort,
    /// `chat_template_kwargs.enable_thinking`: Qwen3's chat templates, on vLLM and llama.cpp.
    ChatTemplate,
}

/// A runtime found, with its models.
#[derive(Clone, Debug)]
pub struct Server {
    pub runtime: Runtime,
    /// Its address without `/v1`: `http://localhost:11434`.
    pub url: String,
    pub version: Option<String>,
    pub models: Vec<LocalModel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalModel {
    pub id: String,
    /// The window requests get, as the runtime says; `None` when it does not say.
    pub window: Option<u64>,
    /// The model's own maximum, when the runtime says it apart from the window.
    pub trained: Option<u64>,
    /// It thinks before answering.
    pub reasoning: bool,
    /// How its thinking is turned off; `None` when it does not think or cannot be asked not to.
    pub switch: Option<Switch>,
    pub images: bool,
    /// Why it cannot serve the agent, as the runtime says: it only embeds, or takes no tools.
    pub unfit: Option<String>,
}

impl LocalModel {
    fn named(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            window: None,
            trained: None,
            reasoning: false,
            switch: None,
            images: false,
            unfit: None,
        }
    }
}

/// The step a runtime failed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    NotReachable,
    /// It answered 401 or 403: it asks for a key.
    Key,
    NoModels,
    /// The check's request failed: the model may not have loaded.
    Loading,
    NoToolCalls,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub step: Step,
    pub detail: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let step = match self.step {
            Step::NotReachable => "not reachable",
            Step::Key => "it asks for an API key",
            Step::NoModels => "no models",
            Step::Loading => "the model did not answer",
            Step::NoToolCalls => "no tool call",
        };
        write!(f, "{step}: {}", self.detail)
    }
}

fn problem(step: Step, detail: impl Into<String>) -> Problem {
    Problem {
        step,
        detail: detail.into(),
    }
}

/// What the tool check saw.
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    /// The arguments the model called the tool with.
    pub arguments: String,
    /// The window the runtime loaded the model with, when it says so only once loaded (Ollama).
    pub window: Option<u64>,
    /// It thought before answering. Not judged for Ollama, which says so of its models.
    pub thinks: bool,
    /// How its thinking was turned off, when asking it not to think worked.
    pub switch: Option<Switch>,
    /// Its thinking came as text in the answer, and what would send it apart.
    pub thinking_as_text: Option<&'static str>,
}

/// `typed` as a runtime's address: `http://` when no scheme is given, without a last `/` or
/// `/v1`.
pub fn address(typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("no address given".into());
    }
    let mut url = match typed.contains("://") {
        true => typed.to_owned(),
        false => format!("http://{typed}"),
    };
    while url.ends_with('/') {
        url.pop();
    }
    if let Some(base) = url.strip_suffix("/v1") {
        url = base.to_owned();
    }
    super::providers::base_url(&url)?;
    Ok(url)
}

/// The default gateway in `/proc/net/route`'s `routes`: under WSL's NAT networking, the Windows
/// host.
pub fn default_gateway(routes: &str) -> Option<std::net::Ipv4Addr> {
    let route = routes.lines().skip(1).find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        (fields.get(1) == Some(&"00000000")).then(|| fields.get(2).copied())?
    })?;
    let address = u32::from_str_radix(route, 16).ok().filter(|a| *a != 0)?;
    Some(std::net::Ipv4Addr::from(address.swap_bytes()))
}

/// The runtimes answering at their default ports on this machine, in `LOOKED_FOR`'s order.
pub fn look_around() -> Vec<Server> {
    std::thread::scope(|scope| {
        let looking: Vec<_> = (Runtime::LOOKED_FOR.iter())
            .filter_map(|runtime| runtime.port())
            .map(|port| {
                scope.spawn(move || {
                    let url = format!("http://localhost:{port}");
                    super::login::blocking(probe_within(&url, None, LOOK_WAIT)).ok()
                })
            })
            .collect();
        let found = looking.into_iter().filter_map(|l| l.join().ok().flatten());
        let mut found: Vec<Server> = found.collect();
        found.dedup_by(|a, b| a.url == b.url);
        found
    })
}

/// The runtime at `url` (as `address` gives it) and its models; `key` goes as a bearer token.
pub fn probe(url: &str, key: Option<&str>) -> Result<Server, Problem> {
    super::login::blocking(probe_within(url, key, PROBE_WAIT))
}

impl From<String> for Problem {
    fn from(detail: String) -> Self {
        problem(Step::NotReachable, detail)
    }
}

async fn get(url: &str, key: Option<&str>, wait: Duration) -> Result<(u16, Value), Problem> {
    let bearer = key.map(|key| format!("Bearer {key}"));
    let mut headers = vec![("accept", "application/json")];
    if let Some(bearer) = &bearer {
        headers.push(("authorization", bearer));
    }
    let fetched = client::fetch_within("local", "GET", url, &headers, None, ANSWER_BYTES, wait);
    let fetched = fetched.await.map_err(|failure| {
        let message = failure.message;
        problem(Step::NotReachable, message.replace("local's ", ""))
    })?;
    let value = serde_json::from_slice(&fetched.body).unwrap_or(Value::Null);
    Ok((fetched.status, value))
}

/// The answer of a request that must succeed for the runtime to be told apart.
async fn answer(url: &str, key: Option<&str>, wait: Duration) -> Result<Value, Problem> {
    match get(url, key, wait).await? {
        (401 | 403, _) => Err(problem(Step::Key, format!("{url} answered without a key"))),
        (200..300, value) if value.is_object() => Ok(value),
        (200..300, _) => Err(problem(Step::NotReachable, format!("{url}: not JSON"))),
        (status, _) => Err(problem(
            Step::NotReachable,
            format!("{url} answered HTTP {status}: not an OpenAI-compatible server"),
        )),
    }
}

/// An answer the runtime may not give; `Null` when it does not.
async fn optional(url: String, key: Option<&str>, wait: Duration) -> Value {
    match get(&url, key, wait).await {
        Ok((200..300, value)) => value,
        _ => Value::Null,
    }
}

async fn probe_within(url: &str, key: Option<&str>, wait: Duration) -> Result<Server, Problem> {
    let listed = answer(&format!("{url}/v1/models"), key, wait).await?;
    let data = listed["data"].as_array().cloned().unwrap_or_default();
    let owned = |owner: &str| data.iter().any(|model| model["owned_by"] == owner);
    let props = |path: &'static str| optional(format!("{url}{path}"), key, wait);
    let (runtime, version, models) = if owned("llamacpp") {
        let props = props("/props").await;
        let version = props["build_info"].as_str().map(str::to_owned);
        (Runtime::LlamaCpp, version, llama_cpp(&data, &props))
    } else if owned("vllm") || data.iter().any(|model| model["max_model_len"].is_u64()) {
        let version = props("/version").await["version"]
            .as_str()
            .map(str::to_owned);
        (Runtime::Vllm, version, vllm(&data))
    } else {
        let ollama = props("/api/version").await;
        match ollama["version"].as_str() {
            Some(version) => {
                let tags = props("/api/tags").await;
                let loaded = props("/api/ps").await;
                let models = ollama_models(&tags, &loaded);
                (Runtime::Ollama, Some(version.to_owned()), models)
            }
            None => (Runtime::Other, None, other(&data)),
        }
    };
    if models.is_empty() {
        return Err(problem(
            Step::NoModels,
            format!("{} at {url} lists none", runtime.name()),
        ));
    }
    Ok(Server {
        runtime,
        url: url.to_owned(),
        version,
        models,
    })
}

fn positive(value: &Value) -> Option<u64> {
    value.as_u64().filter(|n| *n > 0)
}

/// Ollama's models from `/api/tags`, each's window from `/api/ps` when it is loaded: Ollama loads
/// a model with a window of its own choosing (by the GPU's memory, or `OLLAMA_CONTEXT_LENGTH`),
/// at most the model's own.
fn ollama_models(tags: &Value, loaded: &Value) -> Vec<LocalModel> {
    let tags = tags["models"].as_array().into_iter().flatten();
    let models = tags.filter_map(|tag| {
        let id = tag["model"].as_str().or(tag["name"].as_str())?;
        let mut model = LocalModel::named(id);
        model.window = ollama_window(loaded, id);
        model.trained = positive(&tag["details"]["context_length"]);
        if let Some(capabilities) = tag["capabilities"].as_array() {
            let has = |name: &str| capabilities.iter().any(|c| c == name);
            model.reasoning = has("thinking");
            model.switch = model.reasoning.then_some(Switch::Effort);
            model.images = has("vision");
            model.unfit = match (has("completion"), has("tools")) {
                (false, _) => Some("Ollama says it does not chat".into()),
                (true, false) => Some("Ollama says its template takes no tools".into()),
                _ => None,
            };
        }
        Some(model)
    });
    models.collect()
}

fn ollama_window(loaded: &Value, id: &str) -> Option<u64> {
    let mut loaded = loaded["models"].as_array().into_iter().flatten();
    let model = loaded.find(|model| model["model"] == id || model["name"] == id)?;
    positive(&model["context_length"])
}

/// llama.cpp's models, each's window from its list (`meta.n_ctx`) or else `/props`.
fn llama_cpp(data: &[Value], props: &Value) -> Vec<LocalModel> {
    let caps = &props["chat_template_caps"];
    let models = data.iter().filter_map(|listed| {
        let mut model = LocalModel::named(listed["id"].as_str()?);
        model.window = positive(&listed["meta"]["n_ctx"])
            .or_else(|| positive(&props["default_generation_settings"]["n_ctx"]));
        model.trained = positive(&listed["meta"]["n_ctx_train"]);
        model.images = props["modalities"]["vision"] == true;
        if caps["supports_tool_calls"] == false {
            model.unfit = Some("its chat template takes no tools".into());
        }
        Some(model)
    });
    models.collect()
}

fn vllm(data: &[Value]) -> Vec<LocalModel> {
    let models = data.iter().filter_map(|listed| {
        let mut model = LocalModel::named(listed["id"].as_str()?);
        model.window = positive(&listed["max_model_len"]);
        Some(model)
    });
    models.collect()
}

fn other(data: &[Value]) -> Vec<LocalModel> {
    let models = data.iter().filter_map(|listed| {
        let model = super::listed::model(listed, false)?;
        let mut local = LocalModel::named(model["id"].as_str()?);
        local.window = positive(&model["contextWindow"]);
        local.images = model["input"].as_array().is_some_and(|i| i.len() > 1);
        Some(local)
    });
    models.collect()
}

/// Asks `model` of `server` to call a tool that does nothing, waiting while the runtime loads
/// it. A pass shows that the model called the tool once, with arguments that parse; not that it
/// calls tools well.
pub fn check_tools(server: &Server, model: &str, key: Option<&str>) -> Result<Checked, Problem> {
    super::login::blocking(check(server, model, key))
}

/// The answer of a non-streamed Chat Completions request `body` to `server`, or its HTTP status
/// and what the server said.
async fn complete(server: &Server, key: Option<&str>, body: Value) -> Result<Value, Problem> {
    let url = format!("{}/v1/chat/completions", server.url);
    let bearer = key.map(|key| format!("Bearer {key}"));
    let mut headers = vec![("accept", "application/json")];
    if let Some(bearer) = &bearer {
        headers.push(("authorization", bearer));
    }
    let body = Some(("application/json", body.to_string().into_bytes()));
    let fetched = client::fetch_within(
        "local",
        "POST",
        &url,
        &headers,
        body,
        ANSWER_BYTES,
        CHECK_WAIT,
    );
    let fetched = fetched.await.map_err(|failure| {
        let message = failure.message.replace("local's ", "");
        problem(Step::Loading, message)
    })?;
    let answer: Value = serde_json::from_slice(&fetched.body).unwrap_or(Value::Null);
    let said = |answer: &Value| {
        let error = &answer["error"];
        let text = (error["message"].as_str().or(error.as_str()))
            .or(answer["detail"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(&fetched.body).into_owned());
        clipped(text.trim(), 300)
    };
    match fetched.status {
        200..300 => Ok(answer),
        401 | 403 => Err(problem(Step::Key, said(&answer))),
        status => {
            let text = said(&answer);
            // vLLM refuses a request with tools when not started to call them.
            let step = match text.contains("tool") {
                true => Step::NoToolCalls,
                false => Step::Loading,
            };
            let hint = match step {
                Step::NoToolCalls => format!(". {}", server.runtime.tools_hint()),
                _ => String::new(),
            };
            Err(problem(step, format!("HTTP {status}: {text}{hint}")))
        }
    }
}

/// Whether `message` carries thinking: apart, in a reasoning field, or as text between `<think>`
/// tags (a template may give the opening tag in the prompt).
fn thinking(message: &Value) -> (bool, bool) {
    let apart = (REASONING_FIELDS.iter()).any(|field| {
        message[field]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())
    });
    let content = message["content"].as_str().unwrap_or_default();
    let thought = match content.find("</think>") {
        Some(end) => &content[..end],
        None => content.split_once("<think>").map_or("", |(_, after)| after),
    };
    let thought = thought
        .split_once("<think>")
        .map_or(thought, |(_, after)| after);
    (apart, !thought.trim().is_empty())
}

async fn check(server: &Server, model: &str, key: Option<&str>) -> Result<Checked, Problem> {
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": CHECK_PROMPT}],
        "tools": [{"type": "function", "function": {
            "name": CHECK_TOOL,
            "description": "Echoes a word back. It does nothing else.",
            "parameters": {"type": "object", "properties": {"word": {"type": "string"}},
                "required": ["word"]},
        }}],
        "max_tokens": 2048,
        "stream": false,
    });
    let answer = complete(server, key, body).await?;
    let message = &answer["choices"][0]["message"];
    let call = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .next();
    let function = call.map(|call| &call["function"]);
    match function {
        Some(function) if function["name"] == CHECK_TOOL => {
            let arguments = match &function["arguments"] {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            if serde_json::from_str::<Value>(&arguments).is_err() {
                return Err(problem(
                    Step::NoToolCalls,
                    format!("it called {CHECK_TOOL} with arguments that are not JSON: {arguments}"),
                ));
            }
            let mut checked = Checked {
                arguments,
                window: None,
                thinks: false,
                switch: None,
                thinking_as_text: None,
            };
            if server.runtime == Runtime::Ollama {
                let loaded = optional(format!("{}/api/ps", server.url), key, PROBE_WAIT);
                checked.window = ollama_window(&loaded.await, model);
                return Ok(checked);
            }
            let (apart, as_text) = thinking(message);
            checked.thinks = apart || as_text;
            checked.thinking_as_text = as_text.then(|| server.runtime.thinking_hint());
            if checked.thinks && switches_off(server, model, key).await {
                checked.switch = Some(Switch::ChatTemplate);
            }
            Ok(checked)
        }
        Some(function) => Err(problem(
            Step::NoToolCalls,
            format!("it called {} instead of {CHECK_TOOL}", function["name"]),
        )),
        None => {
            let content = message["content"].as_str().unwrap_or_default().trim();
            let content = match content.is_empty() {
                true => "nothing".to_owned(),
                false => format!("“{}”", clipped(content, 200)),
            };
            Err(problem(
                Step::NoToolCalls,
                format!("it answered {content}. {}", server.runtime.tools_hint()),
            ))
        }
    }
}

/// Whether `model` answers without thinking when its chat template is asked not to.
async fn switches_off(server: &Server, model: &str, key: Option<&str>) -> bool {
    let body = json!({
        "model": model,
        "messages": [{"role": "user", "content": SWITCH_PROMPT}],
        "chat_template_kwargs": {"enable_thinking": false},
        "max_tokens": 256,
        "stream": false,
    });
    match complete(server, key, body).await {
        Ok(answer) => {
            let message = &answer["choices"][0]["message"];
            message.is_object() && thinking(message) == (false, false)
        }
        Err(_) => false,
    }
}

fn clipped(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// The provider entries of `models.json` (in `file`) that `/login` saved, by name, with their
/// runtime and address.
pub fn saved(file: &Path) -> Result<Vec<(String, Value)>, String> {
    let Some(Json::Object(providers)) = read(file)?.get("providers").cloned() else {
        return Ok(Vec::new());
    };
    let entries = providers.into_iter().filter_map(|(name, entry)| {
        let entry = entry.to_value();
        Runtime::of(entry["runtime"].as_str()?)?;
        Some((name, entry))
    });
    Ok(entries.collect())
}

/// The name `server`'s models are saved under: that of the entry saved for its address, else
/// its runtime's, or with the port when another entry has that.
pub fn name_for(file: &Path, server: &Server) -> Result<String, String> {
    let document = read(file)?;
    let providers = match document.get("providers") {
        Some(Json::Object(entries)) => entries.as_slice(),
        _ => &[],
    };
    let base = format!("{}/v1", server.url);
    let at =
        |entry: &Json| entry.get("baseUrl").map(Json::to_value) == Some(Value::from(base.as_str()));
    if let Some((name, _)) = providers.iter().find(|(_, entry)| at(entry)) {
        return Ok(name.clone());
    }
    let taken = |name: &str| providers.iter().any(|(taken, _)| taken == name);
    let name = server.runtime.key().to_owned();
    if !taken(&name) {
        return Ok(name);
    }
    let port = super::providers::base_url(&server.url).map(|parts| parts.port);
    let mut numbered = (2..).map(|n| format!("{name}-{n}"));
    let first = port.ok().map(|port| format!("{name}-{port}"));
    let name = first.filter(|name| !taken(name));
    Ok(name.unwrap_or_else(|| numbered.find(|name| !taken(name)).unwrap_or_default()))
}

/// Saves `model` of `server` in `file` under provider `name`: its entry made or its address
/// renewed, the model put in place of one with its id. The file's other contents keep their
/// order.
pub fn save(file: &Path, name: &str, server: &Server, model: &LocalModel) -> Result<(), String> {
    let mut document = read(file)?;
    let providers = document.entry("providers", || Json::Object(Vec::new()));
    let Json::Object(providers) = providers else {
        return Err(format!("{}: providers is not an object", file.display()));
    };
    let fresh = Json::Object(Vec::new());
    let at = match providers.iter().position(|(taken, _)| taken == name) {
        Some(at) => at,
        None => {
            providers.push((name.to_owned(), fresh));
            providers.len() - 1
        }
    };
    let entry = &mut providers[at].1;
    if !matches!(entry, Json::Object(_)) {
        *entry = Json::Object(Vec::new());
    }
    entry.set("baseUrl", Json::from(json!(format!("{}/v1", server.url))));
    entry.set("api", Json::from(json!("openai-completions")));
    entry.set("runtime", Json::from(json!(server.runtime.key())));
    entry.entry("compat", || {
        Json::from(json!({"supportsDeveloperRole": false, "supportsReasoningEffort": false}))
    });
    let models = entry.entry("models", || Json::Array(Vec::new()));
    if !matches!(models, Json::Array(_)) {
        *models = Json::Array(Vec::new());
    }
    let Json::Array(models) = models else {
        unreachable!()
    };
    let saved = model_entry(model);
    let same = |entry: &Json| entry.get("id").map(Json::to_value) == Some(json!(model.id));
    match models.iter().position(same) {
        Some(at) => models[at] = saved,
        None => models.push(saved),
    }
    write(file, &document)
}

/// Removes provider `name` from `file`.
pub fn remove(file: &Path, name: &str) -> Result<(), String> {
    let mut document = read(file)?;
    if let Some(Json::Object(providers)) = document.get_mut("providers") {
        providers.retain(|(taken, _)| taken != name);
    }
    write(file, &document)
}

/// Removes model `id` from provider `name` in `file`, and the provider when it was its last.
pub fn remove_model(file: &Path, name: &str, id: &str) -> Result<(), String> {
    let mut document = read(file)?;
    let Some(Json::Object(providers)) = document.get_mut("providers") else {
        return Ok(());
    };
    let Some(at) = providers.iter().position(|(taken, _)| taken == name) else {
        return Ok(());
    };
    if let Some(Json::Array(models)) = providers[at].1.get_mut("models") {
        models.retain(|model| model.get("id").map(Json::to_value) != Some(json!(id)));
        if models.is_empty() {
            providers.remove(at);
        }
    }
    write(file, &document)
}

/// A model's entry, its fields in the order a person writes them.
fn model_entry(model: &LocalModel) -> Json {
    let mut entry = vec![
        ("id".to_owned(), Json::from(json!(model.id))),
        (
            "reasoning".to_owned(),
            Json::from(json!(model.switch.is_some())),
        ),
    ];
    if let Some(window) = model.window {
        entry.push(("contextWindow".to_owned(), Json::from(json!(window))));
    }
    let input = match model.images {
        true => json!(["text", "image"]),
        false => json!(["text"]),
    };
    entry.push(("input".to_owned(), Json::from(input)));
    let free = json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0});
    entry.push(("cost".to_owned(), Json::from(free)));
    let (levels, compat) = match model.switch {
        // Ollama takes every level, and `none` stops a model thinking (0.40.16).
        Some(Switch::Effort) => (
            vec![("off", json!("none"))],
            json!({"supportsReasoningEffort": true}),
        ),
        // The template knows thinking or none.
        Some(Switch::ChatTemplate) => (
            vec![
                ("off", json!("none")),
                ("minimal", Value::Null),
                ("low", Value::Null),
                ("medium", Value::Null),
            ],
            json!({"thinkingFormat": "qwen-chat-template"}),
        ),
        None => return Json::Object(entry),
    };
    let levels = levels.into_iter();
    let levels = levels.map(|(level, sent)| (level.to_owned(), Json::Plain(sent)));
    entry.push((
        "thinkingLevelMap".to_owned(),
        Json::Object(levels.collect()),
    ));
    entry.push(("compat".to_owned(), Json::from(compat)));
    Json::Object(entry)
}

fn read(file: &Path) -> Result<Json, String> {
    let shown = file.display();
    match std::fs::read(file) {
        Ok(text) => match serde_json::from_slice::<Json>(&text) {
            Ok(document @ Json::Object(_)) => Ok(document),
            Ok(_) => Err(format!("{shown}: not a JSON object")),
            Err(error) => Err(format!("{shown}: {error}")),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Json::Object(Vec::new())),
        Err(error) => Err(format!("{shown}: {error}")),
    }
}

fn write(file: &Path, document: &Json) -> Result<(), String> {
    let shown = file.display();
    let text = serde_json::to_string_pretty(document).map_err(|e| format!("{shown}: {e}"))?;
    if let Some(directory) = file.parent() {
        std::fs::create_dir_all(directory).map_err(|e| format!("{shown}: {e}"))?;
    }
    let temporary = file.with_extension("json.saving");
    std::fs::write(&temporary, text + "\n").map_err(|e| format!("{shown}: {e}"))?;
    std::fs::rename(&temporary, file).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("{shown}: {e}")
    })
}

/// JSON whose objects keep their keys' order, so that a file written back keeps the user's.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Object(Vec<(String, Json)>),
    Array(Vec<Json>),
    Plain(Value),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value at `key` of an object, put there by `made` when it has none.
    fn entry(&mut self, key: &str, made: impl FnOnce() -> Json) -> &mut Json {
        let Json::Object(entries) = self else {
            panic!("entry of a JSON value that is not an object");
        };
        let at = match entries.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => {
                entries.push((key.to_owned(), made()));
                entries.len() - 1
            }
        };
        &mut entries[at].1
    }

    fn set(&mut self, key: &str, value: Json) {
        *self.entry(key, || Json::Plain(Value::Null)) = value;
    }

    fn to_value(&self) -> Value {
        match self {
            Json::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_value()))
                    .collect(),
            ),
            Json::Array(items) => Value::Array(items.iter().map(Json::to_value).collect()),
            Json::Plain(value) => value.clone(),
        }
    }
}

impl From<Value> for Json {
    fn from(value: Value) -> Self {
        match value {
            Value::Object(map) => {
                Json::Object(map.into_iter().map(|(k, v)| (k, v.into())).collect())
            }
            Value::Array(items) => Json::Array(items.into_iter().map(Json::from).collect()),
            plain => Json::Plain(plain),
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Json::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Plain(value) => value.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Any;
        impl<'de> Visitor<'de> for Any {
            type Value = Json;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
                let mut entries: Vec<(String, Json)> = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, Json>()? {
                    // A key given twice keeps its last value, as serde_json's.
                    entries.retain(|(k, _)| *k != key);
                    entries.push((key, value));
                }
                Ok(Json::Object(entries))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Json::Array(items))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Json, E> {
                Ok(Json::Plain(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Json, E> {
                Ok(Json::Plain(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Json, E> {
                Ok(Json::Plain(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Json, E> {
                Ok(Json::Plain(v.into()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Json, E> {
                Ok(Json::Plain(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Json, E> {
                Ok(Json::Plain(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Json, E> {
                Ok(Json::Plain(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Json, E> {
                Ok(Json::Plain(Value::Null))
            }
        }
        deserializer.deserialize_any(Any)
    }
}

#[cfg(test)]
mod tests;
