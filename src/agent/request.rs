//! What a model request sends, and sending it again after transient failures as Scramjet
//! does.
use super::client::{self, ClientError, Protocol};
use super::completions;
use super::providers::{Login, ModelInfo};
use super::{
    Agent, AgentError, AgentUi, PRODUCER, Purpose, StreamProgress, messages, rates, sent, tools,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

/// Scramjet's retry backoff: 2s, 4s, 8s, ...
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Like Scramjet, a longer wait (backoff or the provider's Retry-After) fails the request instead.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

impl Agent {
    pub(super) fn request(&self) -> Value {
        let mut request = json!({
            "model": self.info.id,
            "instructions": self.instructions(),
            "input": self.replay_conversation(),
            "tools": self.tool_definitions(),
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            // GPT-5.6 and later only reuse cached prefixes reliably when requests share a key.
            "prompt_cache_key": self.session.id(),
        });
        request["reasoning"] = json!({"summary": "auto"});
        if let Some(effort) = self.effort() {
            request["reasoning"]["effort"] = json!(effort);
        }
        if let Some(tokens) = self.config.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        // As Pi's, a request with no tools (`--no-tools`) says nothing of them.
        if request["tools"].as_array().is_some_and(Vec::is_empty) {
            let object = request.as_object_mut().unwrap();
            for key in ["tools", "tool_choice", "parallel_tool_calls"] {
                object.remove(key);
            }
        }
        request
    }

    /// The conversation as sent to the current model: the items a model is sent (`sent`). As in
    /// Scramjet, items another model produced lose what only that model accepts: encrypted
    /// reasoning, and `fc_` ids paired with it. The same id from another provider is another
    /// model.
    fn replay_conversation(&self) -> Vec<Value> {
        self.conversation
            .iter()
            .filter(|item| sent(item))
            .map(|item| {
                let mut item = item.clone();
                let producer = item.as_object_mut().and_then(|object| {
                    let producer = object.remove(PRODUCER);
                    object.retain(|key, _| !key.starts_with("_scramjet_"));
                    producer
                });
                let current =
                    |producer: &Value| producer.as_str().is_some_and(|p| self.produced(p));
                if producer.is_some_and(|producer| !current(&producer)) {
                    if item["type"] == "reasoning" {
                        return Value::Null;
                    }
                    if item["type"] == "function_call"
                        && item["id"].as_str().is_some_and(|id| id.starts_with("fc_"))
                    {
                        item.as_object_mut().unwrap().remove("id");
                    }
                }
                item
            })
            .filter(|item| !item.is_null())
            .collect()
    }

    /// Fails when no provider offers the current model, after looking again.
    fn routed(&mut self) -> Result<(), AgentError> {
        if self.info.route.is_none() {
            // A provider that could not list its models before may list them now.
            self.config.providers.reload();
            if let Some(model) = self.config.providers.find(&self.config.model) {
                self.info = model.info.clone();
            }
        }
        match self.info.route {
            Some(_) => Ok(()),
            None => Err(AgentError::Provider(self.unoffered())),
        }
    }

    /// Counts a response's usage, and its output rate when it is a step of the turn.
    fn count(&mut self, model: &str, reply: &client::Reply, purpose: Purpose) {
        self.usage.count(reply);
        if purpose == Purpose::Reply
            && let Some(path) = &self.rates
        {
            // The rates are only shown; a session goes on without them.
            let _ = rates::record(path, model, reply);
        }
    }

    /// Starts sending `request`, the turn's next step, in a thread of its own, so that the turn
    /// can pause for the user's commands while the reply streams (`wait_reply`).
    pub(super) fn start_reply(&mut self, request: Value) -> Result<Replying, AgentError> {
        self.routed()?;
        let (info, retries) = (self.info.clone(), self.config.retries);
        let cancel = Arc::new(AtomicBool::new(false));
        let (to, from) = mpsc::channel();
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let ended = match executor {
                Ok(executor) => {
                    let mut on = |event: Sending| {
                        let _ = to.send(match event {
                            Sending::Attempt => Streamed::Attempt,
                            Sending::Sent => Streamed::Sent,
                            Sending::Progress(update) => Streamed::Progress(update.clone()),
                            Sending::Retrying(error, attempt, delay) => {
                                Streamed::Retrying(error.to_owned(), attempt, delay)
                            }
                        });
                    };
                    send(&executor, &info, retries, &request, true, &stop, &mut on)
                }
                Err(error) => Err(error.into()),
            };
            let _ = to.send(Streamed::Ended(Box::new(ended)));
        });
        Ok(Replying {
            from,
            cancel,
            model: self.config.model.clone(),
            held: VecDeque::new(),
        })
    }

    /// Shows the reply `replying` streams until it ends, stopping it once `cancel` is set. When
    /// `ui` asks for a pause, it is kept in `self.replying`, going on meanwhile, and this returns
    /// `AgentError::Paused`.
    pub(super) fn wait_reply(
        &mut self,
        mut replying: Replying,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<client::Reply, AgentError> {
        let mut looked = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                replying.cancel.store(true, Ordering::Relaxed);
            } else if looked.elapsed() >= LOOK {
                looked = Instant::now();
                if ui.pause() {
                    self.replying = Some(replying);
                    return Err(AgentError::Paused);
                }
            }
            let next = match replying.held.pop_front() {
                Some(held) => Ok(held),
                None => replying.from.recv_timeout(LOOK),
            };
            match next {
                Ok(Streamed::Attempt) => ui.waiting(Purpose::Reply),
                Ok(Streamed::Sent) => self.usage.requests += 1,
                Ok(Streamed::Progress(update)) => ui.progress(&update),
                Ok(Streamed::Retrying(error, attempt, delay)) => {
                    ui.retrying(&error, attempt, self.config.retries, delay)
                }
                Ok(Streamed::Ended(ended)) => {
                    let reply = (*ended)?;
                    self.count(&replying.model, &reply, Purpose::Reply);
                    return Ok(reply);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let ended = "the request's thread ended without a reply";
                    return Err(AgentError::Provider(ended.into()));
                }
            }
        }
    }

    /// Sends `request` as `send` does, and counts its usage.
    pub(super) fn respond(
        &mut self,
        request: &Value,
        purpose: Purpose,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<client::Reply, AgentError> {
        self.routed()?;
        let retries = self.config.retries;
        let requests = &mut self.usage.requests;
        let reply = send(
            &self.executor,
            &self.info,
            retries,
            request,
            purpose == Purpose::Reply,
            cancel,
            &mut |event| match event {
                Sending::Attempt => ui.waiting(purpose),
                Sending::Sent => *requests += 1,
                Sending::Progress(update) if purpose == Purpose::Reply => ui.progress(update),
                Sending::Progress(_) => {}
                Sending::Retrying(error, attempt, delay) => {
                    ui.retrying(error, attempt, retries, delay)
                }
            },
        )?;
        let model = self.config.model.clone();
        self.count(&model, &reply, purpose);
        Ok(reply)
    }
}

/// How long waiting for a reply's stream goes before it looks whether to pause.
const LOOK: Duration = Duration::from_millis(50);

/// A step's request sent in a thread of its own (`Agent::start_reply`). Dropped, it stops.
pub(super) struct Replying {
    from: mpsc::Receiver<Streamed>,
    cancel: Arc<AtomicBool>,
    /// The model asked, which produces the reply, though the session may change it meanwhile.
    pub(super) model: String,
    /// What `pump` left for the turn: the requests sent and how the reply ended.
    held: VecDeque<Streamed>,
}

impl Replying {
    /// Shows the reply's stream on `ui`, which another thread than the turn's shows while the
    /// turn is paused, until `stop` is set or it ends; what the turn counts waits for it.
    pub(super) fn pump(&mut self, ui: &mut dyn AgentUi, retries: u32, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match self.from.recv_timeout(LOOK) {
                Ok(Streamed::Attempt) => ui.waiting(Purpose::Reply),
                Ok(Streamed::Progress(update)) => ui.progress(&update),
                Ok(Streamed::Retrying(error, attempt, delay)) => {
                    ui.retrying(&error, attempt, retries, delay)
                }
                Ok(Streamed::Sent) => self.held.push_back(Streamed::Sent),
                Ok(ended @ Streamed::Ended(_)) => {
                    self.held.push_back(ended);
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

impl Drop for Replying {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// What a reply's thread tells the turn, as `Sending` says it, then how it ended.
enum Streamed {
    Attempt,
    Sent,
    Progress(StreamProgress),
    Retrying(String, u32, Duration),
    Ended(Box<Result<client::Reply, AgentError>>),
}

/// What happens while a request is sent.
pub(super) enum Sending<'a> {
    /// An attempt starts; a login's token is read, and renewed when it expired, before its
    /// request is sent.
    Attempt,
    /// The attempt's request is sent.
    Sent,
    Progress(&'a StreamProgress),
    /// An attempt failed transiently; the next one, the given attempt, starts after the delay.
    Retrying(&'a str, u32, Duration),
}

/// Sends `request` to the model `info` describes, sending it again after transient failures. A
/// failed response's items are discarded, so a retry never runs a tool call from it. `replying`:
/// it is a step of the user's turn, not a summary or a subagent's, which Copilot bills apart.
pub(super) fn send(
    executor: &tokio::runtime::Runtime,
    info: &ModelInfo,
    retries: u32,
    request: &Value,
    replying: bool,
    cancel: &AtomicBool,
    on: &mut dyn FnMut(Sending),
) -> Result<client::Reply, AgentError> {
    let Some(route) = &info.route else {
        let model = request["model"].as_str().unwrap_or_default();
        return Err(AgentError::Provider(format!(
            "no provider offers the model {model}"
        )));
    };
    let without_images = (!info.images).then(|| without_images(request)).flatten();
    let request = without_images.as_ref().unwrap_or(request);
    let mut headers = Vec::new();
    let body = match info.protocol {
        Protocol::Responses | Protocol::Codex => {
            let mut request = match info.protocol {
                Protocol::Codex => super::codex::body(request),
                _ => request.clone(),
            };
            // As Scramjet's, a model that does not reason is asked nothing of reasoning.
            if !info.reasoning
                && let Some(object) = request.as_object_mut()
            {
                object.remove("reasoning");
                object.remove("include");
            }
            serde_json::to_vec(&request)
        }
        Protocol::Messages => {
            let body = messages::request(
                request,
                info.output.unwrap_or(messages::DEFAULT_MAX_TOKENS),
                info.reasoning,
            )
            .map_err(AgentError::Provider)?;
            headers.extend(messages::betas(&body).map(|betas| ("anthropic-beta", betas)));
            serde_json::to_vec(&body)
        }
        Protocol::Completions => {
            let session = request["prompt_cache_key"].as_str();
            if let Some(session) = session.filter(|_| info.compat.session_affinity) {
                for name in ["session_id", "x-client-request-id", "x-session-affinity"] {
                    headers.push((name, session.to_owned()));
                }
            }
            serde_json::to_vec(&completions::request(request, info))
        }
    };
    let body = body.map_err(std::io::Error::from)?;
    let mut attempt = 0;
    let mut failed = Vec::new();
    loop {
        let mut sent_at = None;
        on(Sending::Attempt);
        // A login's token, renewed when it expired, which it may have during a retry's wait.
        let token = (route.login.as_ref())
            .map(|login| login_token(login, cancel))
            .transpose();
        let failure = match token {
            Err(ClientError::Cancelled) => return Err(AgentError::Cancelled),
            Err(ClientError::Failed(failure)) => failure,
            Ok(token) => {
                let mut headers = headers.clone();
                if let (Some(login), Some(token)) = (&route.login, &token) {
                    headers.push(("authorization", format!("Bearer {token}")));
                    if login.provider == super::copilot::PROVIDER {
                        headers.extend(super::copilot::request_headers(request, replying));
                    }
                }
                if info.protocol == Protocol::Codex {
                    // A models.json's key, else the login's, names the account.
                    let keyed = (route.headers.iter())
                        .find(|(name, _)| name == "authorization")
                        .and_then(|(_, value)| value.strip_prefix("Bearer "));
                    let token = keyed.or(token.as_deref()).unwrap_or_default();
                    let codex = super::codex::request_headers(token, request);
                    headers.extend(codex.map_err(AgentError::Provider)?);
                }
                on(Sending::Sent);
                let started = Instant::now();
                sent_at = Some((SystemTime::now(), started));
                let sent = client::Sent {
                    route,
                    protocol: info.protocol,
                    headers: &headers,
                    priced_by: info.prices.header(),
                    compat: &info.compat,
                };
                let mut progress = |update: StreamProgress| on(Sending::Progress(&update));
                match executor.block_on(client::respond(sent, body.clone(), cancel, &mut progress))
                {
                    Ok(mut reply) => {
                        reply.elapsed = started.elapsed();
                        reply.timing = Some(client::Timing {
                            started_at: sent_at.map_or_else(SystemTime::now, |(at, _)| at),
                            headers: reply.headers_at.map(|at| at - started),
                            first_output: reply.first_output_at.map(|at| at - started),
                            elapsed: reply.elapsed,
                            failed,
                        });
                        reply.cost = (reply.usage.as_ref())
                            .and_then(|usage| info.prices.cost(usage, reply.priced_by.as_deref()));
                        return Ok(reply);
                    }
                    Err(ClientError::Cancelled) => return Err(AgentError::Cancelled),
                    Err(ClientError::Failed(failure)) => failure,
                }
            }
        };
        if let Some((started_at, started)) = sent_at {
            failed.push(client::FailedAttempt {
                started_at,
                elapsed: started.elapsed(),
                error: (failure.message.lines().next().unwrap_or_default())
                    .chars()
                    .take(300)
                    .collect(),
            });
        }
        let backoff = RETRY_DELAY * 2u32.pow(attempt.min(5));
        let delay = backoff
            .min(MAX_RETRY_WAIT)
            .max(failure.wait.unwrap_or_default());
        // A context overflow is not retried as is: only compaction can let the request through.
        if !failure.transient || failure.overflow || attempt == retries || delay > MAX_RETRY_WAIT {
            let message = match attempt {
                0 => failure.message,
                n => format!("{} (after {n} retries)", failure.message),
            };
            return Err(match failure.overflow {
                true => AgentError::ContextOverflow(message),
                false => AgentError::Provider(message),
            });
        }
        attempt += 1;
        on(Sending::Retrying(&failure.message, attempt, delay));
        let interrupted = executor.block_on(async {
            tokio::select! {
                biased;
                () = client::cancelled(cancel) => true,
                () = tokio::time::sleep(delay) => false,
            }
        });
        if interrupted {
            return Err(AgentError::Cancelled);
        }
    }
}

/// The token `login` keeps, renewed when it expired. Escape stops the wait at once, but not the
/// renewal, which goes on by itself and saves what it gets: OpenAI's renewal answers with a new
/// refresh token, and the old one may not be taken again.
fn login_token(login: &Login, cancel: &AtomicBool) -> Result<String, ClientError> {
    let login = login.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let program = login.program.as_ref();
        sender.send(super::login::access(&login.file, &login.provider, program))
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ClientError::Cancelled);
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(token) => return token.map_err(ClientError::Failed),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let failed = "reading the login's token failed".to_owned();
                return Err(ClientError::Failed(failed.into()));
            }
        }
    }
}

/// `request` for a model that takes no images, when it has any: each tool result's images become
/// a note after its text, and a user message's a note in its place. `None` when it has none.
fn without_images(request: &Value) -> Option<Value> {
    const NOTE: &str = "[Image omitted: the current model does not support images.]";
    let has_images = |item: &Value| tools::item_images(item) > 0;
    if !request["input"].as_array()?.iter().any(has_images) {
        return None;
    }
    let mut request = request.clone();
    for item in request["input"].as_array_mut()? {
        let images = tools::result_images(&item["output"]);
        if images > 0 {
            let mut text = tools::result_text(&item["output"]);
            for _ in 0..images {
                text.push('\n');
                text.push_str(NOTE);
            }
            item["output"] = json!(text);
        }
        let parts = item.get_mut("content").and_then(Value::as_array_mut);
        for part in parts.into_iter().flatten() {
            if part["type"] == "input_image" {
                *part = json!({"type":"input_text","text":format!("\n{NOTE}")});
            }
        }
    }
    Some(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests::*;
    use crate::agent::{AgentConfig, Session, SessionChoice};
    use crate::provider_fixture as fixture;

    #[test]
    fn a_request_without_tools_says_nothing_of_them() {
        let (mut agent, _scratch) = agent(([127, 0, 0, 1], 9).into(), "no-tools");
        agent.config.only_tools = Some(vec!["read".into()]);
        let request = agent.request();
        assert_eq!(request["tools"][0]["name"], "read");
        assert_eq!(request["tools"].as_array().unwrap().len(), 1);
        assert_eq!(request["tool_choice"], "auto");
        agent.config.only_tools = Some(Vec::new());
        let request = agent.request();
        for key in ["tools", "tool_choice", "parallel_tool_calls"] {
            assert!(request.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn transient_failures_are_sent_again_without_running_their_tool_calls() {
        let mut failed = fixture::call_events("{}", fixture::known_usage());
        let terminal = failed.last_mut().unwrap();
        terminal["type"] = json!("response.failed");
        terminal["response"]["status"] = json!("failed");
        // Observed through Proxy on 2026-10-30.
        terminal["response"]["error"] = json!({"message": "Upstream model provider failed before \
the response completed (ReadError: ConnectionResetError(104, 'Connection reset by peer')). The \
response is incomplete; retry the request."});
        let (endpoint, server) = serve(vec![
            stream(&failed),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "retry");
        let mut ui = Recorder::default();
        agent.turn("hi", &mut ui, &AtomicBool::new(false)).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], bodies[1]);
        assert_eq!(ui.retries.len(), 1);
        assert!(ui.retries[0].0.contains("retry the request"));
        assert_eq!((ui.retries[0].1, ui.retries[0].2), (1, RETRY_DELAY));
        assert_eq!(ui.replies, ["done"]);
        assert!(ui.calls.is_empty());
        assert_eq!(agent.usage.requests, 2);
        assert_eq!(agent.usage.context_tokens, Some(15));
        assert_eq!(agent.conversation()[1]["_scramjet_model"], "test/m");
        assert!(bodies[1]["input"][0].get("_scramjet_model").is_none());
        let kinds: Vec<_> = agent
            .conversation()
            .iter()
            .map(|item| item["type"].as_str().or(item["role"].as_str()).unwrap())
            .collect();
        assert_eq!(kinds, ["user", "message"]);
        // The reply's timing is saved, with the attempt that failed before it.
        let timing = &agent.conversation()[1]["_scramjet_timing"];
        let failed = &timing["failed"][0];
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .contains("Connection reset by peer")
        );
        assert!(timing["failed"].get(1).is_none());
        let ms = |value: &Value| value.as_u64().unwrap();
        let waited = ms(&timing["started"]) - ms(&failed["started"]);
        assert!(waited >= RETRY_DELAY.as_millis() as u64, "{timing}");
        let (headers, first) = (ms(&timing["headers_ms"]), ms(&timing["first_output_ms"]));
        assert!(
            headers <= first && first <= ms(&timing["duration_ms"]),
            "{timing}"
        );
    }

    #[test]
    fn permanent_failures_and_long_retry_waits_end_the_turn_at_once() {
        let (endpoint, server) = serve(vec![
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 15\r\n\r\ninvalid request".to_vec(),
            b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 120\r\ncontent-length: 4\r\n\r\nslow"
                .to_vec(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "permanent");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let errors: Vec<_> = (0..2)
            .map(|_| match agent.turn("hi", &mut ui, &cancel) {
                Err(AgentError::Provider(message)) => message,
                other => panic!("{:?}", other.err()),
            })
            .collect();
        assert_eq!(
            errors,
            [
                "HTTP 400 Bad Request: invalid request",
                "HTTP 429 Too Many Requests: slow",
            ]
        );
        assert!(ui.retries.is_empty());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn failures_are_classified_by_code_then_wording() {
        let failure = |code, message: &str| client::Failure::provider(code, message.into());
        let transient = |code, message: &str| failure(code, message).transient;
        assert!(transient(Some("server_error"), "anything"));
        assert!(!transient(Some("insufficient_quota"), "rate limit"));
        assert!(transient(None, "Rate limit reached for requests"));
        assert!(transient(Some("unknown_code"), "The server is overloaded"));
        assert!(!transient(
            None,
            "This model's maximum context length is 128000 tokens; timeout"
        ));
        assert!(!transient(None, "Invalid value for 'input'"));
        assert!(failure(Some("context_length_exceeded"), "anything").overflow);
        assert!(failure(None, "Your input exceeds the context window of this model.").overflow);
        assert!(failure(None, "prompt is too long: 213462 tokens > 200000 maximum").overflow);
        assert!(!failure(None, "Rate limit reached: too many tokens").overflow);
        assert!(!failure(None, "Invalid value for 'input'").overflow);
        // Scramjet's provider examples, and deepseek-v4.1-flash's gateway through Proxy.
        for overflow in [
            "This model's maximum context length is 327680 tokens. However, you requested 330001",
            "Requested token count exceeds the model's maximum context length of 131,072 tokens",
            "The input token count (1196265) exceeds the maximum number of tokens allowed",
            "prompt token count of 140000 exceeds the limit of 128000",
            "The input (140000 tokens) is longer than the model's context length (131072 tokens).",
            "Prompt contains 140000 tokens, too large for model with 131072 maximum context length",
            "prompt too long; exceeded max context length by 12 tokens",
        ] {
            assert!(failure(None, overflow).overflow, "{overflow}");
        }
        // The same words without what makes them an overflow in Scramjet's patterns.
        for other in [
            "The upload exceeds the limit of the plan",
            "max_tokens must be less than the maximum context length",
            "Invalid input token count",
            "ThrottlingException: Too many tokens, please wait before trying again.",
        ] {
            assert!(!failure(None, other).overflow, "{other}");
        }
    }

    #[test]
    fn replay_sends_only_messages_and_the_items_of_replies() {
        let (mut agent, scratch) = agent(([127, 0, 0, 1], 9).into(), "allowlist");
        let path = scratch.0.join("entries.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"version\":1}\n",
                "{\"role\":\"user\",\"content\":\"go\"}\n",
                "{\"type\":\"message\",\"role\":\"assistant\",\"content\":\"Done.\",\"_scramjet_model\":\"test-model\"}\n",
                "{\"type\":\"turn_stopped\",\"error\":\"HTTP 500\"}\n",
                "{\"type\":\"bang_excluded\",\"command\":\"echo secret\",\"output\":\"secret\\n\"}\n",
                "{\"type\":\"from_a_later_build\",\"role\":\"user\",\"content\":\"unknown\"}\n",
                "{\"type\":null,\"role\":\"user\",\"content\":\"untyped\"}\n",
                "{\"role\":\"assistant\",\"content\":\"not a reply\"}\n",
                "{\"role\":\"user\",\"content\":\"next\"}\n",
            ),
        )
        .unwrap();
        let (_, conversation, _) = Session::open(&path).unwrap();
        assert_eq!(conversation.len(), 8);
        agent.conversation = conversation;
        assert_eq!(
            agent.replay_conversation(),
            [
                json!({"role":"user","content":"go"}),
                json!({"type":"message","role":"assistant","content":"Done."}),
                json!({"role":"user","content":"next"}),
            ]
        );
    }

    #[test]
    fn replay_drops_other_models_reasoning_and_unpairs_function_calls_after_resume() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-model-{}", std::process::id()));
        let _scratch = ScratchDir::new(root.clone());
        let path = root.join("1700000000000-42.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"session\",\"version\":1}\n",
                "{\"type\":\"reasoning\",\"encrypted_content\":\"opaque\",\"_scramjet_model\":\"old\"}\n",
                "{\"type\":\"function_call\",\"id\":\"fc_item\",\"call_id\":\"call_1\",\"_scramjet_model\":\"old\"}\n",
                "{\"type\":\"function_call\",\"id\":\"item_other\",\"call_id\":\"call_2\",\"_scramjet_model\":\"old\"}\n",
                "{\"type\":\"function_call_output\",\"call_id\":\"call_1\",\"output\":\"ok\"}\n",
                "{\"type\":\"function_call_output\",\"call_id\":\"call_2\",\"output\":\"ok\"}\n"
            ),
        )
        .unwrap();
        let config = AgentConfig {
            providers: crate::agent::tests::serving(([127, 0, 0, 1], 9).into()),
            model: "new".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
            only_tools: None,
            command_sets: Vec::new(),
            skills: Vec::new(),
            skill_options: Default::default(),
            prompt: Default::default(),
            mcp_servers: Vec::new(),
        };
        let mut agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        let replay = agent.replay_conversation();
        assert_eq!(replay.len(), 4);
        assert_eq!(replay[0]["type"], "function_call");
        assert!(replay[0].get("id").is_none());
        assert_eq!(replay[0]["call_id"], "call_1");
        assert_eq!(replay[1]["id"], "item_other");
        assert_eq!(replay[2]["type"], "function_call_output");
        assert!(
            replay
                .iter()
                .all(|item| item.get("_scramjet_model").is_none())
        );

        assert_eq!(agent.last_model(), Some("old"));
        agent.set_model("old").unwrap();
        let same_model = agent.replay_conversation();
        assert_eq!(same_model.len(), 5);
        assert_eq!(same_model[0]["type"], "reasoning");
        assert_eq!(same_model[1]["id"], "fc_item");
    }

    #[test]
    fn older_claude_models_send_budget_thinking_with_the_interleaved_beta() {
        let (endpoint, server) = serve_requests(vec![messages_stream(&[
            json!({"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":3,"output_tokens":1}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
            json!({"type":"message_stop"}),
        ])]);
        let (mut agent, _scratch) = agent(endpoint, "budget");
        agent.config.effort = Some("low".into());
        agent.set_model("claude-haiku-4-5-20251001").unwrap();
        let mut ui = Recorder::default();
        agent.turn("Hi", &mut ui, &AtomicBool::new(false)).unwrap();
        let requests = server.join().unwrap();
        let (head, body) = &requests[0];
        assert!(
            head.contains(
                "anthropic-beta: fine-grained-tool-streaming-2025-05-14,interleaved-thinking-2025-05-14\r\n"
            ),
            "{head}"
        );
        assert_eq!(
            body["thinking"],
            json!({"type":"enabled","budget_tokens":2048,"display":"summarized"})
        );
    }

    #[test]
    fn budget_thinking_without_room_fails_before_sending() {
        // Nothing listens on the endpoint: the request must not be sent.
        let endpoint = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let (mut agent, _scratch) = agent(endpoint, "no-room");
        agent.config.effort = Some("high".into());
        agent.config.max_output_tokens = Some(2000);
        agent.set_model("claude-sonnet-4-5").unwrap();
        let mut ui = Recorder::default();
        let error = agent
            .turn("Hi", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert!(
            matches!(&error, AgentError::Provider(message) if message.starts_with("Insufficient output space for Anthropic thinking")),
            "{error}"
        );
    }

    #[test]
    fn an_image_read_goes_to_the_model_with_its_result_and_stays_in_the_session() {
        let (endpoint, server) = serve(vec![
            tool_call("call_1", "read", json!({"path":"shot.png"})),
            stream(&fixture::text_events("A gradient.", fixture::known_usage())),
            stream(&fixture::text_events("Noted.", fixture::known_usage())),
        ]);
        let (mut agent, scratch) = agent(endpoint, "image-read");
        let png = crate::agent::image::tests::png(3, 2);
        std::fs::write(scratch.0.join("shot.png"), &png).unwrap();
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent
            .turn("What is in shot.png?", &mut ui, &cancel)
            .unwrap();
        // A model whose card lists no image input gets the text with a note.
        agent.info.images = false;
        agent.turn("And now?", &mut ui, &cancel).unwrap();
        let shown = "result read Read image file [image/png] Read(\"shot.png\")";
        assert_eq!(ui.shown[1], shown);
        let bodies = server.join().unwrap();
        let result = |body: &Value| {
            let input = body["input"].as_array().unwrap();
            let result = input
                .iter()
                .find(|item| item["type"] == "function_call_output");
            result.unwrap()["output"].clone()
        };
        let sent = result(&bodies[1]);
        assert_eq!(
            sent,
            json!([
                {"type":"input_text","text":"Read image file [image/png]"},
                {"type":"input_image","detail":"auto",
                 "image_url":format!("data:image/png;base64,{}", crate::agent::image::base64(&png))},
            ])
        );
        assert_eq!(
            result(&bodies[2]),
            "Read image file [image/png]\n[Image omitted: the current model does not support images.]"
        );
        let saved = std::fs::read_to_string(agent.session_path()).unwrap();
        let saved: Vec<Value> = saved
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let saved = saved
            .iter()
            .find(|line| line["type"] == "function_call_output");
        assert_eq!(saved.unwrap()["output"], sent);
        let mut replay = Recorder::default();
        crate::agent::show_items(agent.conversation(), None, &mut replay);
        assert!(
            replay.shown.iter().any(|line| line == shown),
            "{:?}",
            replay.shown
        );
    }

    #[test]
    fn attached_files_go_with_the_next_prompt_only_and_show_as_typed() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("A dot.", fixture::known_usage())),
            stream(&fixture::text_events(
                "Still a dot.",
                fixture::known_usage(),
            )),
        ]);
        let (mut agent, scratch) = agent(endpoint, "attached-turn");
        let png = crate::agent::image::tests::png(3, 2);
        std::fs::write(scratch.0.join("dot.png"), &png).unwrap();
        let files = crate::agent::Attached::read(&["dot.png".to_owned()], &scratch.0).unwrap();
        agent.attach(files);
        let element = format!(
            "<file name=\"{}\"></file>\n",
            scratch.0.join("dot.png").display()
        );
        let sent = format!("{element}What is it?");
        assert_eq!(
            agent.attached("What is it?"),
            Some(("@dot.png What is it?".to_owned(), sent.clone()))
        );
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("What is it?", &mut ui, &cancel).unwrap();
        assert_eq!(agent.attached("And now?"), None);
        agent.info.images = false;
        agent.turn("And now?", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        let image = json!({"type":"input_image","detail":"auto",
            "image_url":format!("data:image/png;base64,{}", crate::agent::image::base64(&png))});
        let text = json!({"type":"input_text","text":sent});
        assert_eq!(
            bodies[0]["input"],
            json!([{"role":"user","content":[text, image]}])
        );
        // A model whose card lists no image input gets a note in the image's place.
        let note = "\n[Image omitted: the current model does not support images.]";
        assert_eq!(
            bodies[1]["input"][0],
            json!({"role":"user","content":[text, {"type":"input_text","text":note}]})
        );
        assert_eq!(
            bodies[1]["input"][2],
            json!({"role":"user","content":"And now?"})
        );
        // Recalled, listed by /tree and shown on a resume as typed.
        assert_eq!(agent.prompts(), ["@dot.png What is it?", "And now?"]);
        let tree = agent.tree().unwrap();
        assert_eq!(
            tree.entries[0].item,
            crate::agent::TreeItem::Prompt("@dot.png What is it?".into())
        );
        let mut replay = Recorder::default();
        crate::agent::show_items(agent.conversation(), None, &mut replay);
        assert_eq!(
            replay.shown[0],
            format!("command @dot.png What is it? sent {sent}")
        );
    }

    #[test]
    fn claude_models_use_messages_and_replay_their_thinking_calls_and_results() {
        let start = |id: &str, input: u64| {
            json!({"type":"message_start","message":{"id":id,"usage":{"input_tokens":input,
                "cache_read_input_tokens":5,"cache_creation_input_tokens":2,"output_tokens":1}}})
        };
        let stop = |index: u64| json!({"type":"content_block_stop","index":index});
        let delta = |index: u64, delta: Value| json!({"type":"content_block_delta","index":index,"delta":delta});
        let (endpoint, server) = serve_requests(vec![
            messages_stream(&[
                start("msg_1", 10),
                json!({"type":"ping"}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
                delta(
                    0,
                    json!({"type":"thinking_delta","thinking":"Need the file."}),
                ),
                delta(0, json!({"type":"signature_delta","signature":"sig1"})),
                stop(0),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                delta(1, json!({"type":"text_delta","text":"Reading"})),
                stop(1),
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read","input":{}}}),
                delta(
                    2,
                    json!({"type":"input_json_delta","partial_json":"{\"path\":"}),
                ),
                delta(
                    2,
                    json!({"type":"input_json_delta","partial_json":"\"notes.txt\"}"}),
                ),
                stop(2),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}),
                json!({"type":"message_stop"}),
            ]),
            messages_stream(&[
                start("msg_2", 30),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                delta(0, json!({"type":"text_delta","text":"It says hello."})),
                stop(0),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
                json!({"type":"message_stop"}),
            ]),
        ]);
        let (mut agent, scratch) = agent(endpoint, "claude");
        std::fs::write(scratch.0.join("notes.txt"), "hello\n").unwrap();
        agent.set_model("claude-opus-5-5").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("What is in notes.txt?", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["Reading", "It says hello."]);
        assert_eq!(ui.calls, ["read"]);
        let requests = server.join().unwrap();
        for (head, _) in &requests {
            assert!(head.starts_with("post /v1/messages http/1.1\r\n"), "{head}");
            assert!(head.contains("anthropic-version: 2023-06-01\r\n"), "{head}");
            assert!(
                head.contains("anthropic-beta: fine-grained-tool-streaming-2025-05-14\r\n"),
                "{head}"
            );
        }
        let first = &requests[0].1;
        assert_eq!(first["model"], "claude-opus-5-5");
        assert_eq!(first["max_tokens"], messages::DEFAULT_MAX_TOKENS);
        assert_eq!(
            first["thinking"],
            json!({"type":"adaptive","display":"summarized"})
        );
        assert_eq!(first["system"][0]["text"], agent.instructions());
        assert_eq!(first["tools"][0]["name"], "read");
        assert_eq!(
            first["tools"][0]["input_schema"]["required"],
            json!(["path"])
        );
        assert_eq!(
            first["messages"],
            json!([{"role":"user","content":[{"type":"text","text":"What is in notes.txt?",
                "cache_control":{"type":"ephemeral","ttl":"1h"}}]}])
        );
        let second = &requests[1].1["messages"];
        assert_eq!(
            second[1],
            json!({"role":"assistant","content":[
                {"type":"thinking","thinking":"Need the file.","signature":"sig1"},
                {"type":"text","text":"Reading"},
                {"type":"tool_use","id":"toolu_1","name":"read","input":{"path":"notes.txt"}},
            ]})
        );
        let result = &second[2]["content"][0];
        assert_eq!(second[2]["role"], "user");
        assert_eq!(result["tool_use_id"], "toolu_1");
        assert!(result["content"].as_str().unwrap().contains("hello"));
        assert_eq!(
            result["cache_control"],
            json!({"type":"ephemeral","ttl":"1h"})
        );
        assert!(second[0]["content"][0].get("cache_control").is_none());
        // Input counts cache reads and writes, as Responses input does.
        assert_eq!(agent.usage.input_tokens, 17 + 37);
        assert_eq!(agent.usage.cached_input_tokens, 10);
        assert_eq!(agent.usage.cache_write_tokens, 4);
        assert_eq!(agent.usage.output_tokens, 24);
        // Saved apart: the input neither read from the cache nor written to it.
        let saved: Vec<_> = (agent.conversation().iter())
            .filter_map(|item| item.get("_scramjet_usage"))
            .collect();
        assert_eq!(
            saved[0],
            &json!({"input":10,"output":20,"cache_read":5,"cache_write":2})
        );
        assert_eq!(agent.usage.context_tokens, Some(41));
        assert!(
            agent.conversation()[1..]
                .iter()
                .filter(|item| item.get("type").is_some())
                .all(|item| item["_scramjet_model"] == "test/claude-opus-5-5"
                    || item["type"] == "function_call_output")
        );
    }

    /// A streamed Chat Completions reply with these chunks.
    fn completions_stream(chunks: &[Value]) -> Vec<u8> {
        let mut body: String = (chunks.iter())
            .map(|chunk| format!("data: {chunk}\n\n"))
            .collect();
        body.push_str("data: [DONE]\n\n");
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    #[test]
    fn a_provider_logged_in_to_sends_the_logins_token_and_copilots_headers() {
        let (endpoint, server) = serve_requests(vec![
            tool_call("call_1", "read", json!({"path": "notes.txt"})),
            stream(&fixture::text_events(
                "It says hello.",
                fixture::known_usage(),
            )),
        ]);
        let (mut agent, scratch) = agent(endpoint, "copilot-login");
        std::fs::write(scratch.0.join("notes.txt"), "hello\n").unwrap();
        let logins = super::super::login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let auth = logins.auth.clone();
        let expires = super::super::login::now_ms() + 600_000.0;
        let credential = json!({"type": "oauth", "refresh": "gho", "access": "tid=9;exp=1",
            "expires": expires});
        super::super::login::save(&auth, "github-copilot", credential).unwrap();
        // As a models.json names it, at the test's server; without an apiKey, the login's.
        agent.config.providers = crate::agent::Providers::given_logged_in(
            json!({"providers": {"github-copilot": {
                "baseUrl": format!("http://{endpoint}/v1"), "api": "openai-responses",
                "headers": {"Editor-Version": "vscode/1.107.0"},
                "models": [{"id": "gpt-6-luna", "reasoning": true}],
            }}}),
            logins.clone(),
        );
        agent.set_model("gpt-6-luna").unwrap();
        let mut ui = Recorder::default();
        let said = agent.turn("What is in notes.txt?", &mut ui, &AtomicBool::new(false));
        said.unwrap();
        assert_eq!(ui.replies, ["It says hello."]);
        let requests = server.join().unwrap();
        for (head, _) in &requests {
            assert!(
                head.starts_with("post /v1/responses http/1.1\r\n"),
                "{head}"
            );
            assert!(
                head.contains("authorization: bearer tid=9;exp=1\r\n"),
                "{head}"
            );
            assert!(
                head.contains("openai-intent: conversation-edits\r\n"),
                "{head}"
            );
            assert!(
                head.contains("editor-version: vscode/1.107.0\r\n"),
                "{head}"
            );
            assert!(!head.contains("copilot-vision-request"), "{head}");
        }
        // The user asked for the first; the agent for the one after the tool's result.
        assert!(requests[0].0.contains("x-initiator: user\r\n"));
        assert!(requests[1].0.contains("x-initiator: agent\r\n"));
        // Logged out, the request is not sent.
        super::super::login::logout(&logins, "github-copilot").unwrap();
        let failed = agent.turn("Again?", &mut ui, &AtomicBool::new(false));
        assert!(
            matches!(&failed, Err(AgentError::Provider(message))
                if message == "not logged in to GitHub Copilot (/login)"),
            "{failed:?}"
        );
    }

    #[test]
    fn a_packages_login_renews_its_token_before_the_request_that_carries_it() {
        let (endpoint, server) = serve_requests(vec![stream(&fixture::text_events(
            "Hi.",
            fixture::known_usage(),
        ))]);
        let (mut agent, scratch) = agent(endpoint, "program-login");
        let logins = super::super::login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let program = scratch.0.join("login");
        let renewed = r#"{"credential":{"access":"renewed","expires":99999999999999}}"#;
        script(&program, &format!("read -r r\necho '{renewed}'"));
        let file = scratch.0.join("models.json");
        let declared = r#"{"logins": {"acme": {"name": "Acme", "command": ["./login"]}}}"#;
        std::fs::write(&file, declared).unwrap();
        let expired = json!({"type": "oauth", "access": "old", "refresh": "r", "expires": 0});
        super::super::login::save(&logins.auth, "acme", expired).unwrap();
        // The login listed the test's server.
        let entry = json!({"baseUrl": format!("http://{endpoint}/v1"), "api": "openai-responses",
            "models": [{"id": "m1"}]});
        let list = json!({"fetched": super::super::login::now_ms(), "provider": entry});
        super::super::login::save_list(&logins.listed("acme"), &list).unwrap();
        agent.config.providers = crate::agent::Providers::read(vec![file], Some(logins.clone()));
        agent.set_model("acme/m1").unwrap();
        let mut ui = Recorder::default();
        agent
            .turn("Hello?", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["Hi."]);
        let requests = server.join().unwrap();
        let head = &requests[0].0;
        assert!(
            head.starts_with("post /v1/responses http/1.1\r\n"),
            "{head}"
        );
        assert!(head.contains("authorization: bearer renewed\r\n"), "{head}");
        let saved = super::super::login::saved(&logins.auth).unwrap();
        assert_eq!(
            (&saved["acme"]["access"], &saved["acme"]["refresh"]),
            (&json!("renewed"), &json!("r"))
        );
    }

    #[test]
    fn a_codex_login_sends_its_account_and_session_to_codexs_backend() {
        let limit = json!({"error": {"code": "usage_limit_reached", "plan_type": "pro",
            "message": "The usage limit has been reached"}})
        .to_string();
        let (endpoint, server) = serve_requests(vec![
            stream(&fixture::text_events("Hi.", fixture::known_usage())),
            format!(
                "HTTP/1.1 429 Too Many Requests\r\ncontent-length: {}\r\n\r\n{limit}",
                limit.len()
            )
            .into_bytes(),
        ]);
        let (mut agent, scratch) = agent(endpoint, "codex-login");
        let logins = super::super::login::Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        // An access token naming the account `acc-9`.
        let claims = json!({"https://api.openai.com/auth": {"chatgpt_account_id": "acc-9"}});
        let claims = crate::agent::image::base64(claims.to_string().as_bytes());
        let token = format!("e30.{}.c2ln", claims.trim_end_matches('='));
        let expires = super::super::login::now_ms() + 600_000.0;
        let credential = json!({"type": "oauth", "refresh": "rt", "access": token,
            "expires": expires, "accountId": "acc-9"});
        super::super::login::save(&logins.auth, "openai-codex", credential).unwrap();
        agent.config.providers = crate::agent::Providers::given_logged_in(
            json!({"providers": {"openai-codex": {
                "baseUrl": format!("http://{endpoint}/backend-api"),
                "api": "openai-codex-responses", "headers": {"originator": "scramjet-rs"},
                "models": [{"id": "gpt-6-luna", "reasoning": true}],
            }}}),
            logins,
        );
        agent.set_model("openai-codex/gpt-6-luna").unwrap();
        agent.config.max_output_tokens = Some(500);
        let mut ui = Recorder::default();
        agent
            .turn("Hello", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["Hi."]);
        // A usage limit ends the turn at once, saying when, not sent again.
        let failed = agent.turn("Again", &mut ui, &AtomicBool::new(false));
        assert!(
            matches!(&failed, Err(AgentError::Provider(message)) if message
                == "HTTP 429 Too Many Requests: You have hit your ChatGPT usage limit (pro plan)."),
            "{failed:?}"
        );
        assert!(ui.retries.is_empty());
        let requests = server.join().unwrap();
        let session = agent.session.id().to_lowercase();
        for (head, body) in &requests {
            for header in [
                "post /backend-api/codex/responses http/1.1\r\n".to_owned(),
                format!("authorization: bearer {}\r\n", token.to_lowercase()),
                "chatgpt-account-id: acc-9\r\n".to_owned(),
                format!("session_id: {session}\r\n"),
                format!("x-client-request-id: {session}\r\n"),
                "originator: scramjet-rs\r\n".to_owned(),
            ] {
                assert!(head.contains(&header), "{header} in {head}");
            }
            assert!(body.get("max_output_tokens").is_none(), "{body}");
            assert_eq!(body["text"], json!({"verbosity": "low"}));
            assert_eq!(body["store"], false);
            assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        }
    }

    #[test]
    fn a_failed_token_renewal_is_retried_when_it_may_succeed_and_escape_stops_waiting_for_one() {
        use super::super::login::{self, Logins, now_ms};
        let (endpoint, server) = serve_requests(vec![
            stream(&fixture::text_events("Renewed.", fixture::known_usage())),
            stream(&fixture::text_events(
                "After Escape.",
                fixture::known_usage(),
            )),
        ]);
        let (mut agent, scratch) = agent(endpoint, "login-renewal");
        let logins = Logins {
            auth: scratch.0.join("auth.json"),
            lists: scratch.0.join("logins"),
        };
        let auth = logins.auth.clone();
        agent.config.providers = crate::agent::Providers::given_logged_in(
            json!({"providers": {"openai-codex": {
                "baseUrl": format!("http://{endpoint}/v1"), "api": "openai-responses",
                "models": [{"id": "gpt-6-luna"}],
            }}}),
            logins,
        );
        agent.set_model("openai-codex/gpt-6-luna").unwrap();
        let request = agent.request();
        let fresh = |access: &str| {
            json!({"type": "oauth", "refresh": "rt-2", "access": access,
                "expires": now_ms() + 600_000.0})
        };
        let expired = json!({"type": "oauth", "refresh": "rt", "access": "old", "expires": 0});
        let mut events = Vec::new();
        let send_it = |cancel: &AtomicBool, events: &mut Vec<String>| {
            send(
                &agent.executor,
                &agent.info,
                2,
                &request,
                true,
                cancel,
                &mut |event| {
                    match event {
                        Sending::Attempt => events.push("attempt".to_owned()),
                        Sending::Sent => events.push("sent".to_owned()),
                        Sending::Retrying(error, attempt, delay) => {
                            events.push(format!("retry {attempt} in {delay:?}: {error}"));
                            // Another process renews it during the wait.
                            login::save(&auth, "openai-codex", fresh("fresh")).unwrap();
                        }
                        Sending::Progress(_) => {}
                    }
                },
            )
        };
        // OpenAI's token endpoint is out of reach in tests (`codex::Urls`): it may answer later.
        login::save(&auth, "openai-codex", expired.clone()).unwrap();
        let reply = send_it(&AtomicBool::new(false), &mut events).unwrap();
        assert_eq!(reply.items[0]["content"][0]["text"], "Renewed.");
        assert_eq!(events.len(), 4, "{events:?}");
        assert!(
            events[1].starts_with("retry 1 in 2s: cannot connect to 127.0.0.1:1 ")
                && events[1].ends_with(" (renewing the ChatGPT token; /login if it goes on)"),
            "{events:?}"
        );
        assert_eq!(
            [&events[0], &events[2], &events[3]],
            ["attempt", "attempt", "sent"]
        );
        // A login that cannot be renewed fails at once.
        events.clear();
        let unrenewable = json!({"type": "oauth", "access": "old", "expires": 0});
        login::save(&auth, "openai-codex", unrenewable).unwrap();
        let failed = send_it(&AtomicBool::new(false), &mut events);
        assert!(
            matches!(&failed, Err(AgentError::Provider(message))
                if message == "the ChatGPT login keeps no refresh token: /login again"),
            "{:?}",
            failed.err()
        );
        assert_eq!(events, ["attempt"]);
        events.clear();
        // Escape stops the wait while another process holds the login to renew it.
        login::save(&auth, "openai-codex", expired).unwrap();
        let lock = std::fs::File::open(scratch.0.join("auth.json.lock")).unwrap();
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let failed = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            });
            send_it(&cancel, &mut events)
        });
        assert!(matches!(failed, Err(AgentError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(1));
        // Waiting for the reply shows from the start; nothing was sent.
        assert_eq!(events, ["attempt"]);
        // Its renewal saved, the next request carries it.
        let renewed = json!({"openai-codex": fresh("after")}).to_string();
        std::fs::write(scratch.0.join("auth.json"), renewed).unwrap();
        drop(lock);
        let reply = send_it(&AtomicBool::new(false), &mut events).unwrap();
        assert_eq!(reply.items[0]["content"][0]["text"], "After Escape.");
        let requests = server.join().unwrap();
        assert!(requests[0].0.contains("authorization: bearer fresh\r\n"));
        assert!(requests[1].0.contains("authorization: bearer after\r\n"));
    }

    #[test]
    fn chat_completions_models_run_tools_and_replay_their_results() {
        let choice = |delta: Value, finish: Value| json!({"id":"chatcmpl-1","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        let usage = |prompt: u64| {
            json!({"id":"chatcmpl-1","choices":[],
                "usage":{"prompt_tokens":prompt,"completion_tokens":5}})
        };
        let (endpoint, server) = serve_requests(vec![
            completions_stream(&[
                choice(
                    json!({"role":"assistant","reasoning_content":"Need the file."}),
                    json!(null),
                ),
                choice(
                    json!({"tool_calls":[{"index":0,"id":"call_9","type":"function",
                    "function":{"name":"read","arguments":"{\"path\":\"notes.txt\"}"}}]}),
                    json!(null),
                ),
                choice(json!({}), json!("tool_calls")),
                usage(20),
            ]),
            completions_stream(&[
                choice(json!({"content":"It says hello."}), json!(null)),
                choice(json!({}), json!("stop")),
                usage(40),
            ]),
        ]);
        let (mut agent, scratch) = agent(endpoint, "completions");
        std::fs::write(scratch.0.join("notes.txt"), "hello\n").unwrap();
        agent.config.providers = crate::agent::Providers::given(json!({"providers": {"local": {
            "baseUrl": format!("http://{endpoint}/v1"), "api": "openai-completions",
            "apiKey": "local-key",
            "models": [{"id": "qwen3", "reasoning": true, "maxTokens": 8192}],
        }}}));
        agent.set_model("qwen3").unwrap();
        agent.config.effort = Some("low".into());
        let mut ui = Recorder::default();
        agent
            .turn("What is in notes.txt?", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["It says hello."]);
        assert_eq!(ui.calls, ["read"]);
        let requests = server.join().unwrap();
        for (head, body) in &requests {
            assert!(
                head.starts_with("post /v1/chat/completions http/1.1\r\n"),
                "{head}"
            );
            assert!(
                head.contains("authorization: bearer local-key\r\n"),
                "{head}"
            );
            assert_eq!(body["model"], "qwen3");
            assert_eq!(body["reasoning_effort"], "low");
            assert_eq!(body["max_completion_tokens"], 8192);
            assert_eq!(body["prompt_cache_key"], agent.session.id());
        }
        let messages = requests[1].1["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "developer");
        assert_eq!(messages[0]["content"], agent.instructions());
        assert_eq!(
            messages[2],
            json!({"role":"assistant","content":null,"reasoning_content":"Need the file.",
                "tool_calls":[{"id":"call_9","type":"function",
                "function":{"name":"read","arguments":"{\"path\":\"notes.txt\"}"}}]})
        );
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call_9");
        assert!(messages[3]["content"].as_str().unwrap().contains("hello"));
        assert_eq!(messages.len(), 4);
        assert_eq!(agent.usage.input_tokens, 60);
        assert_eq!(agent.usage.output_tokens, 10);
        assert_eq!(agent.usage.context_tokens, Some(45));
        // Saved as Responses items, which another model is sent without this one's reasoning.
        assert_eq!(agent.conversation()[1]["type"], "reasoning");
        assert_eq!(agent.conversation()[1]["_scramjet_model"], "local/qwen3");
        agent.set_model("m").unwrap();
        let replay = agent.replay_conversation();
        assert_eq!(replay[1]["type"], "function_call");
        assert_eq!(replay[1]["call_id"], "call_9");
    }

    #[test]
    fn a_claude_reply_at_the_output_limit_keeps_its_text_and_ends_the_turn_without_its_call() {
        let (endpoint, server) = serve_requests(vec![
            messages_stream(&[
                json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Writing notes.txt"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"write","input":{}}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"notes.txt\",\"content\":\"hel"}}),
                json!({"type":"content_block_stop","index":1}),
                json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":20}}),
                json!({"type":"message_stop"}),
            ]),
            messages_stream(&[
                json!({"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":9,"output_tokens":1}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Shorter then."}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}),
                json!({"type":"message_stop"}),
            ]),
        ]);
        let (mut agent, scratch) = agent(endpoint, "limit");
        agent.set_model("claude-opus-5-5").unwrap();
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let error = agent.turn("Write notes", &mut ui, &cancel).unwrap_err();
        assert!(
            matches!(&error, AgentError::Provider(message) if message == "response incomplete: it \
reached the output token limit (max_tokens); the tool call it was making did not run"),
            "{error}"
        );
        assert_eq!(ui.replies, ["Writing notes.txt"]);
        assert!(ui.calls.is_empty());
        assert!(!scratch.0.join("notes.txt").exists());
        assert_eq!(agent.usage.output_tokens, 20);
        agent.turn("Continue", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        assert_eq!(
            requests[1].1["messages"],
            json!([
                {"role":"user","content":[{"type":"text","text":"Write notes"}]},
                {"role":"assistant","content":[{"type":"text","text":"Writing notes.txt"}]},
                {"role":"user","content":[{"type":"text","text":"Continue",
                    "cache_control":{"type":"ephemeral","ttl":"1h"}}]},
            ])
        );
    }

    #[test]
    fn a_gpt_reply_at_the_output_limit_keeps_its_text_and_ends_the_turn() {
        // Stopped while reasoning after a message: the reasoning, which a replay would need
        // followed by an item, is left out.
        let mut reasoning = fixture::cut_events("Looking", fixture::known_usage());
        let thought = json!({"type":"reasoning","id":"rs_2","summary":[],"status":"incomplete"});
        reasoning.insert(
            8,
            json!({"type":"response.output_item.added","output_index":1,
                "item":{"type":"reasoning","id":"rs_2","summary":[]}}),
        );
        reasoning.insert(
            9,
            json!({"type":"response.output_item.done","output_index":1,"item":thought}),
        );
        let (endpoint, server) = serve_requests(vec![
            stream(&fixture::cut_events("At the edge", fixture::known_usage())),
            stream(&reasoning),
            stream(&fixture::text_events(
                "Shorter then.",
                fixture::known_usage(),
            )),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "gpt-limit");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let error = agent.turn("Write a story", &mut ui, &cancel).unwrap_err();
        assert!(
            matches!(&error, AgentError::Provider(message) if message == "response incomplete: it \
reached the output token limit (max_output_tokens)"),
            "{error}"
        );
        assert_eq!(ui.replies, ["At the edge"]);
        assert_eq!(agent.usage.output_tokens, 5);
        agent.turn("Go on", &mut ui, &cancel).unwrap_err();
        agent.turn("Continue", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        let input = &requests[2].1["input"];
        let shape: Vec<_> = (input.as_array().unwrap().iter())
            .map(|item| (item["role"].as_str(), item["status"].as_str()))
            .collect();
        let (user, reply) = ((Some("user"), None), (Some("assistant"), Some("completed")));
        assert_eq!(shape, [user, reply, user, reply, user]);
        assert_eq!(input[1]["content"][0]["text"], "At the edge");
        assert_eq!(input[3]["content"][0]["text"], "Looking");
        assert_eq!(input[4]["content"], "Continue");
    }

    #[test]
    fn requests_share_the_session_cache_key_across_resumes() {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-cache-{}", std::process::id()));
        let _scratch = ScratchDir::new(root.clone());
        let path = root.join("1700000000000-42.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"session\",\"version\":1}\n{\"role\":\"user\",\"content\":\"hi\"}\n",
        )
        .unwrap();
        let config = AgentConfig {
            providers: crate::agent::tests::serving(([127, 0, 0, 1], 9).into()),
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
            tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
            only_tools: None,
            command_sets: Vec::new(),
            skills: Vec::new(),
            skill_options: Default::default(),
            prompt: Default::default(),
            mcp_servers: Vec::new(),
        };
        let agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        assert_eq!(agent.request()["prompt_cache_key"], "1700000000000-42");
    }
}
