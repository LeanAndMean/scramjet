//! A coding agent that talks to Proxy (Responses for GPT models, Messages for Claude) and works in
//! a project.

mod client;
mod compaction;
mod messages;
mod session;
mod tools;

pub use client::StreamProgress;
pub use session::Session;
pub use tools::sequence_diff;

use client::{ClientError, ModelInfo, Protocol};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tools::{Approval, Call};

/// Guards against a model that never stops calling tools.
const MAX_STEPS: usize = 200;
const CONTEXT_FILE_BYTES: u64 = 256 << 10;
/// Scramjet's retry backoff: 2s, 4s, 8s, ...
const RETRY_DELAY: Duration = Duration::from_secs(2);
/// Like Scramjet, a longer wait (backoff or the provider's Retry-After) fails the request instead.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);
/// Stored with each response item: the model that produced it. Never sent to the endpoint.
const PRODUCER: &str = "_scramjet_model";
/// Stored with a user message whose text is not its prompt: the `!command` it ran, which is recalled
/// as typed. Never sent to the endpoint.
pub(super) const TYPED: &str = "_scramjet_typed";
/// Compaction keeps about this much of the latest conversation, as Scramjet does.
const KEEP_RECENT_TOKENS: u64 = 20_000;

pub struct AgentConfig {
    pub endpoint: SocketAddr,
    pub model: String,
    pub root: PathBuf,
    pub effort: Option<String>,
    pub max_output_tokens: Option<u64>,
    /// Ask before each bash command and each write or edit outside the project.
    pub ask: bool,
    /// Extra attempts for a model request that failed transiently (rate limit, overload, network).
    pub retries: u32,
}

pub enum SessionChoice {
    New,
    Continue,
    Open(PathBuf),
}

/// How the agent presents progress and asks for approval.
pub trait AgentUi {
    /// A model request is sent; `compacting` when it asks for a compaction summary.
    fn waiting(&mut self, compacting: bool);
    /// Text or reasoning summary as it streams; may never be called for a response.
    fn progress(&mut self, update: &StreamProgress);
    /// A model request failed transiently and will be sent again after `delay`.
    fn retrying(&mut self, error: &str, attempt: u32, retries: u32, delay: Duration);
    /// The complete text of an assistant message, after any streamed progress for it.
    fn assistant(&mut self, text: &str);
    fn tool_call(&mut self, name: &str, summary: &str);
    /// The tool call announced last starts running, after any approval.
    fn tool_running(&mut self);
    /// The end of the running tool's output so far.
    fn tool_output(&mut self, _output: &str) {}
    /// `output` is what the model receives; `detail`, if any, is better shown to the user.
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool);
    /// The result of a command the user ran with `!` or `!!`.
    fn command_result(&mut self, output: &str, failed: bool);
    /// Shows a finished compaction; `automatic` says why the agent ran it without `/compact`.
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>);
    /// An automatic compaction before a prompt failed; the prompt is sent anyway.
    fn compaction_failed(&mut self, error: &AgentError);
    /// Asks whether a tool call may run; returns `Deny` without an answer once `cancel` is set.
    fn approve(&mut self, question: &str, cancel: &AtomicBool) -> Answer;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    Deny,
    Allow,
    /// Allow this call and every later one until the agent exits.
    AllowAll,
}

/// What the user sees of a successful tool call instead of the text the model receives.
#[derive(Debug, PartialEq)]
pub enum ToolDetail {
    /// The lines an edit changed, each marked ` ` (unchanged), `-` or `+` and numbered, with
    /// `...` for skipped lines.
    Diff(String),
    /// The content a write put in the file.
    Written(String),
}

#[derive(Debug)]
pub enum AgentError {
    Cancelled,
    Provider(String),
    /// A request failed because its input exceeded the model's context window.
    ContextOverflow(String),
    /// A compaction the agent started by itself failed.
    CompactionFailed(Box<AgentError>),
    Session(std::io::Error),
    TooManySteps,
}
impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Provider(message) => write!(f, "provider: {message}"),
            Self::ContextOverflow(message) => write!(f, "context overflow: {message}"),
            Self::CompactionFailed(error) => write!(f, "automatic compaction failed: {error}"),
            Self::Session(error) => write!(f, "session: {error}"),
            Self::TooManySteps => write!(f, "stopped after {MAX_STEPS} model requests in one turn"),
        }
    }
}
impl From<std::io::Error> for AgentError {
    fn from(error: std::io::Error) -> Self {
        Self::Session(error)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Usage {
    pub requests: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    /// The input and output tokens of the last response, when it reported them: how full the
    /// context was, as Scramjet's footer shows it.
    pub context_tokens: Option<u64>,
}

pub struct Compaction {
    /// The last response's context size and an estimate of the items after it, or without a
    /// response of the current model an estimate of the whole conversation (chars/4).
    pub tokens_before: u64,
    pub kept_items: usize,
    pub summary: String,
}

/// Why compaction ran without `/compact`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AutoCompaction {
    /// The last response's context passed the model's window minus `RESERVE_TOKENS`.
    Threshold,
    /// A request failed because its input exceeded the model's window.
    Overflow,
}

impl AutoCompaction {
    /// A short line saying why compaction ran, for the user.
    pub fn reason(self) -> &'static str {
        match self {
            Self::Threshold => "Automatic: the context was nearing the model's window.",
            Self::Overflow => "Automatic: a request exceeded the model's context window.",
        }
    }
}

pub struct Agent {
    config: AgentConfig,
    instructions: String,
    conversation: Vec<Value>,
    prompts: Vec<String>,
    session: Session,
    executor: tokio::runtime::Runtime,
    /// The current model's protocol and limits, from its name until `load_model_card`.
    info: ModelInfo,
    pub usage: Usage,
}

impl Agent {
    pub fn start(config: AgentConfig, choice: SessionChoice) -> Result<Self, AgentError> {
        let opened = match choice {
            SessionChoice::New => None,
            SessionChoice::Continue => Session::latest(&config.root)?,
            SessionChoice::Open(path) => Some(path),
        };
        let (session, conversation, prompts) = match opened {
            Some(path) => Session::open(&path)?,
            None => (
                Session::create(&session::directory(&config.root)?, &config.root)?,
                Vec::new(),
                Vec::new(),
            ),
        };
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let mut agent = Self {
            instructions: instructions(&config.root),
            info: ModelInfo::guess(&config.model),
            config,
            conversation,
            prompts,
            session,
            executor,
            usage: Usage::default(),
        };
        agent.answer_interrupted_calls()?;
        Ok(agent)
    }

    /// Continues in an empty session saved next to the current one; on failure nothing changes.
    /// Approvals given with `a` still apply.
    pub fn new_session(&mut self) -> Result<(), AgentError> {
        let directory = self.session.path().parent().unwrap_or(Path::new("."));
        let mut session = Session::create(directory, &self.config.root)?;
        session.record_model(&self.config.model)?;
        self.session = session;
        self.conversation.clear();
        self.prompts.clear();
        self.usage = Usage::default();
        Ok(())
    }

    /// Replaces the conversation before its last ~20k tokens with a summary the model writes, as
    /// Scramjet's compaction does, followed by the files read and modified in all compacted
    /// history, and records that in the session. On failure or cancellation
    /// nothing changes. `None`: nothing is old enough to summarize.
    pub fn compact(
        &mut self,
        focus: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<Option<Compaction>, AgentError> {
        let previous = compaction::previous_summary(&self.conversation).map(str::to_owned);
        let start = usize::from(previous.is_some());
        let cut = compaction::cut_point(&self.conversation, start, KEEP_RECENT_TOKENS);
        if cut == start {
            return Ok(None);
        }
        // As in Scramjet, a turn cut in two is summarized apart from the history before it.
        let turn = compaction::turn_start(&self.conversation, start, cut);
        let history = &self.conversation[start..turn.unwrap_or(cut)];
        let previous_files = self.session.compacted_files();
        let files = previous_files.with_calls(&self.conversation[start..cut]);
        let mut summary = match previous {
            Some(previous) if history.is_empty() => previous
                .strip_suffix(&previous_files.format())
                .unwrap_or(&previous)
                .to_owned(),
            _ if history.is_empty() => "No prior history.".into(),
            previous => {
                let prompt = compaction::prompt(history, previous.as_deref(), focus);
                self.summarize(prompt, ui, cancel)?
            }
        };
        if let Some(turn) = turn {
            let prompt = compaction::turn_prefix_prompt(&self.conversation[turn..cut]);
            let prefix = self.summarize(prompt, ui, cancel)?;
            summary = compaction::split_turn_summary(&summary, &prefix);
        }
        summary.push_str(&files.format());
        let tokens_before = self
            .context_size()
            .unwrap_or_else(|| compaction::estimate(&self.conversation));
        self.session.append(
            &json!({"type":"compaction", "summary":summary, "first_kept":cut,
                "read_files":files.read, "modified_files":files.modified}),
        )?;
        let kept = self.conversation.split_off(cut);
        self.conversation = vec![compaction::summary_message(&summary)];
        self.conversation.extend(kept);
        // Unknown until the next response reports it.
        self.usage.context_tokens = None;
        Ok(Some(Compaction {
            tokens_before,
            kept_items: self.conversation.len() - 1,
            summary,
        }))
    }

    /// One request without tools for the text a summarization `prompt` asks for.
    fn summarize(
        &mut self,
        prompt: String,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<String, AgentError> {
        let mut request = json!({
            "model": self.config.model,
            "instructions": compaction::SYSTEM_PROMPT,
            "input": [{"role":"user", "content":prompt}],
            "stream": true,
            "store": false,
            "prompt_cache_key": self.session.id(),
            "reasoning": {"summary": "auto"},
        });
        if let Some(effort) = &self.config.effort {
            request["reasoning"]["effort"] = json!(effort);
        }
        if let Some(tokens) = self.config.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        let reply = self.respond(&request, true, ui, cancel)?;
        let summary = reply
            .items
            .iter()
            .filter(|item| item["type"] == "message")
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n");
        if summary.trim().is_empty() {
            return Err(AgentError::Provider("the model returned no summary".into()));
        }
        Ok(summary)
    }

    /// The user's prompts in this session, oldest first, including those compacted away.
    pub fn prompts(&self) -> &[String] {
        &self.prompts
    }

    pub fn session_path(&self) -> &Path {
        self.session.path()
    }

    pub fn root(&self) -> &Path {
        &self.config.root
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Changes the model used for subsequent requests and records it in the session, so a resume
    /// continues with it; on failure nothing changes.
    pub fn set_model(&mut self, model: impl Into<String>) -> Result<(), AgentError> {
        let model = model.into();
        self.session.record_model(&model)?;
        self.info = ModelInfo::guess(&model);
        self.config.model = model;
        Ok(())
    }

    /// The window automatic compaction checks; `None` skips the check.
    pub fn set_context_window(&mut self, window: Option<u64>) {
        self.info.window = window;
    }

    /// The model the session last recorded: its latest switch or response item's producer.
    pub fn last_model(&self) -> Option<&str> {
        self.session.model()
    }

    /// The endpoint's models, or `None` if they cannot be listed.
    pub fn models(&self) -> Option<Vec<String>> {
        self.executor.block_on(client::models(self.config.endpoint))
    }

    pub fn conversation(&self) -> &[Value] {
        &self.conversation
    }

    /// Reads the current model's Proxy model card: which protocol its requests use, its output
    /// limit and its context window, which is returned. Without a card the model's name decides
    /// the protocol, and the window is unknown (`None`).
    pub fn load_model_card(&mut self) -> Option<u64> {
        if let Some(info) = self
            .executor
            .block_on(client::model_info(self.config.endpoint, &self.config.model))
        {
            self.info = info;
        }
        self.info.window
    }

    /// A tool call left without a result (the process stopped mid-call) is answered, never re-run.
    fn answer_interrupted_calls(&mut self) -> Result<(), AgentError> {
        let answered: std::collections::HashSet<_> = self
            .conversation
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .filter_map(|item| item["call_id"].as_str().map(str::to_owned))
            .collect();
        let missing: Vec<_> = self
            .conversation
            .iter()
            .filter(|item| item["type"] == "function_call")
            .filter_map(|item| item["call_id"].as_str())
            .filter(|id| !answered.contains(*id))
            .map(str::to_owned)
            .collect();
        for call_id in missing {
            self.add(json!({"type":"function_call_output","call_id":call_id,
                "output":"Error: the previous session ended before this tool call reported a result. It may or may not have taken effect, and it was not re-run."}))?;
        }
        Ok(())
    }

    fn add(&mut self, item: Value) -> Result<(), AgentError> {
        self.session.append(&item)?;
        self.conversation.push(item);
        Ok(())
    }

    fn request(&self) -> Value {
        let mut request = json!({
            "model": self.config.model,
            "instructions": self.instructions,
            "input": self.replay_conversation(),
            "tools": tools::definitions(),
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            // GPT-5.6 and later only reuse cached prefixes reliably when requests share a key.
            "prompt_cache_key": self.session.id(),
        });
        request["reasoning"] = json!({"summary": "auto"});
        if let Some(effort) = &self.config.effort {
            request["reasoning"]["effort"] = json!(effort);
        }
        if let Some(tokens) = self.config.max_output_tokens {
            request["max_output_tokens"] = json!(tokens);
        }
        request
    }

    /// The conversation as sent to the current model. As in Scramjet, items another model produced
    /// lose what only that model accepts: encrypted reasoning, and `fc_` ids paired with it.
    fn replay_conversation(&self) -> Vec<Value> {
        self.conversation
            .iter()
            .map(|item| {
                let mut item = item.clone();
                let producer = item.as_object_mut().and_then(|object| {
                    object.remove(TYPED);
                    object.remove(PRODUCER)
                });
                if producer.is_some_and(|producer| producer != self.config.model.as_str()) {
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

    /// Runs one user prompt to completion: model requests and tool calls until a final answer.
    /// As Scramjet does, the conversation is compacted before the prompt and after the reply if
    /// its context passed the model's window minus `compaction::RESERVE_TOKENS`. A request that
    /// fails because its input exceeded the window is compacted and sent again, once per turn.
    pub fn turn(
        &mut self,
        prompt: &str,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        self.compact_before_prompt(ui, cancel)?;
        self.add(json!({"role":"user","content":prompt}))?;
        self.prompts.push(prompt.to_owned());
        let mut overflow_recovered = false;
        for _ in 0..MAX_STEPS {
            if cancel.load(Ordering::Relaxed) {
                return Err(AgentError::Cancelled);
            }
            let request = self.request();
            let reply = match self.respond(&request, false, ui, cancel) {
                Ok(reply) => reply,
                Err(AgentError::ContextOverflow(message)) if !overflow_recovered => {
                    overflow_recovered = true;
                    if !self.auto_compact(AutoCompaction::Overflow, ui, cancel)? {
                        return Err(AgentError::ContextOverflow(format!(
                            "{message} (nothing is old enough to compact)"
                        )));
                    }
                    continue;
                }
                Err(AgentError::ContextOverflow(message)) => {
                    return Err(AgentError::ContextOverflow(format!(
                        "{message} (again after compacting once; reduce the context or switch \
to a larger-context model)"
                    )));
                }
                Err(error) => return Err(error),
            };
            self.usage.context_tokens = reply.usage.as_ref().and_then(|usage| usage.total_tokens);
            let mut calls = Vec::new();
            for mut item in reply.items {
                if let Some(object) = item.as_object_mut() {
                    object.insert(PRODUCER.into(), json!(self.config.model));
                }
                match item["type"].as_str() {
                    Some("message") => ui.assistant(&message_text(&item)),
                    Some("function_call") => calls.push(item.clone()),
                    _ => {}
                }
                self.add(item)?;
            }
            if calls.is_empty() {
                self.compact_when_full(ui, cancel)?;
                return Ok(());
            }
            for item in calls {
                let output = self.call(&item, ui, cancel);
                self.add(json!({"type":"function_call_output","call_id":item["call_id"],"output":output}))?;
                if cancel.load(Ordering::Relaxed) {
                    return Err(AgentError::Cancelled);
                }
            }
        }
        Err(AgentError::TooManySteps)
    }

    /// A `!` or `!!` command the user runs without a model turn: `include` adds it to the
    /// conversation as a user message, as Scramjet's `!` does, and `!!` leaves it out.
    pub fn shell(
        &mut self,
        command: &str,
        include: bool,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        ui.tool_call("bash", command);
        ui.tool_running();
        let ran =
            match tools::run_command(&self.config.root, command, None, cancel, &mut |output| {
                ui.tool_output(output)
            }) {
                Ok(ran) => ran,
                Err(error) => {
                    ui.command_result(&format!("Error: {error}"), true);
                    return Ok(());
                }
            };
        let shown = ran.tool_output();
        ui.command_result(&shown.text, shown.failed);
        if include {
            let prompt = format!("!{command}");
            self.add(json!({"role":"user","content":ran.message(command),TYPED:prompt}))?;
            self.prompts.push(prompt);
        }
        Ok(())
    }

    /// Compacts as `/compact` without a focus does and shows the result as `/compact` does,
    /// saying why it ran. Returns whether there was anything to compact.
    fn auto_compact(
        &mut self,
        reason: AutoCompaction,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<bool, AgentError> {
        match self.compact("", ui, cancel) {
            Ok(Some(compaction)) => {
                ui.compacted(&compaction, Some(reason));
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(AgentError::Cancelled) => Err(AgentError::Cancelled),
            Err(error) => Err(AgentError::CompactionFailed(Box::new(error))),
        }
    }

    /// As Scramjet's threshold check: compacts after a turn once the context passed the model's
    /// window minus `compaction::RESERVE_TOKENS`. Without a known window it does nothing.
    fn compact_when_full(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let (Some(tokens), Some(window)) = (self.usage.context_tokens, self.info.window) else {
            return Ok(());
        };
        if compaction::should_compact(tokens, window) {
            self.auto_compact(AutoCompaction::Threshold, ui, cancel)?;
        }
        Ok(())
    }

    /// As Scramjet counts the context: the last response's, if the current model gave it, and an
    /// estimate of the items added after it, such as tool output from a cancelled turn.
    fn context_size(&self) -> Option<u64> {
        let tokens = self.usage.context_tokens?;
        let last = self
            .conversation
            .iter()
            .rposition(|item| item.get(PRODUCER).is_some())?;
        (self.conversation[last][PRODUCER] == self.config.model.as_str())
            .then(|| tokens + compaction::estimate(&self.conversation[last + 1..]))
    }

    /// As Scramjet's check before a new prompt, which catches a turn that ended without a reply.
    fn compact_before_prompt(
        &mut self,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<(), AgentError> {
        let (Some(tokens), Some(window)) = (self.context_size(), self.info.window) else {
            return Ok(());
        };
        if !compaction::should_compact(tokens, window) {
            return Ok(());
        }
        match self.auto_compact(AutoCompaction::Threshold, ui, cancel) {
            Err(AgentError::CompactionFailed(error)) => {
                ui.compaction_failed(&error);
                Ok(())
            }
            result => result.map(drop),
        }
    }

    /// Sends `request`, sending it again after transient failures, and counts its usage. A failed
    /// response's items are discarded, so a retry never runs a tool call from it.
    fn respond(
        &mut self,
        request: &Value,
        compacting: bool,
        ui: &mut dyn AgentUi,
        cancel: &AtomicBool,
    ) -> Result<client::Reply, AgentError> {
        let (body, interleaved) = match self.info.protocol {
            Protocol::Responses => (serde_json::to_vec(request), false),
            Protocol::Messages => {
                let body = messages::request(
                    request,
                    self.info.output.unwrap_or(messages::DEFAULT_MAX_TOKENS),
                    self.info.reasoning,
                )
                .map_err(AgentError::Provider)?;
                let interleaved = messages::budget_thinking(&body);
                (serde_json::to_vec(&body), interleaved)
            }
        };
        let body = body.map_err(std::io::Error::from)?;
        let mut attempt = 0;
        loop {
            ui.waiting(compacting);
            self.usage.requests += 1;
            let failure = match self.executor.block_on(client::respond(
                self.config.endpoint,
                self.info.protocol,
                body.clone(),
                interleaved,
                cancel,
                &mut |update| {
                    if !compacting {
                        ui.progress(&update)
                    }
                },
            )) {
                Ok(reply) => {
                    if let Some(usage) = &reply.usage {
                        self.usage.input_tokens += usage.input_tokens.unwrap_or(0);
                        self.usage.cached_input_tokens += usage.cached_input_tokens.unwrap_or(0);
                        self.usage.output_tokens += usage.output_tokens.unwrap_or(0);
                    }
                    return Ok(reply);
                }
                Err(ClientError::Cancelled) => return Err(AgentError::Cancelled),
                Err(ClientError::Failed(failure)) => failure,
            };
            let backoff = RETRY_DELAY * 2u32.pow(attempt.min(5));
            let delay = backoff
                .min(MAX_RETRY_WAIT)
                .max(failure.wait.unwrap_or_default());
            // A context overflow is not retried as is: only compaction can let the request through.
            if !failure.transient
                || failure.overflow
                || attempt == self.config.retries
                || delay > MAX_RETRY_WAIT
            {
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
            ui.retrying(&failure.message, attempt, self.config.retries, delay);
            let interrupted = self.executor.block_on(async {
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

    fn call(&mut self, item: &Value, ui: &mut dyn AgentUi, cancel: &AtomicBool) -> String {
        let call = match Call::parse(item) {
            Ok(call) => call,
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(item["name"].as_str().unwrap_or(""), &output, None, true);
                return output;
            }
        };
        ui.tool_call(call.name, &call.summary());
        let answer = match call.approval(&self.config.root) {
            Approval::Ask(question) if self.config.ask => ui.approve(&question, cancel),
            _ => Answer::Allow,
        };
        if answer == Answer::AllowAll {
            self.config.ask = false;
        }
        if answer == Answer::Deny {
            let output = if cancel.load(Ordering::Relaxed) {
                "The user cancelled the turn before this tool call ran."
            } else {
                "The user denied this tool call."
            };
            ui.tool_result(call.name, output, None, true);
            return output.to_owned();
        }
        ui.tool_running();
        match call.run(&self.config.root, cancel, &mut |output| {
            ui.tool_output(output)
        }) {
            Ok(output) => {
                ui.tool_result(
                    call.name,
                    &output.text,
                    output.detail.as_ref(),
                    output.failed,
                );
                output.text
            }
            Err(error) => {
                let output = format!("Error: {error}");
                ui.tool_result(call.name, &output, None, true);
                output
            }
        }
    }
}

fn message_text(item: &Value) -> String {
    item["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str().or(part["refusal"].as_str()))
        .collect()
}

fn instructions(root: &Path) -> String {
    let mut text = format!(
        "You are scramjet-rs, a coding agent. You help the user with software engineering tasks \
in the project directory {root}, using tools to read, write and edit files and to run bash commands.

Available tools:
- read: Read file contents
- bash: Execute bash commands (ls, grep, find, etc.)
- edit: Make precise file edits with exact text replacement, including multiple disjoint edits in one call
- write: Create or overwrite files

Guidelines:
- Use bash for file operations like ls, rg, find, and for git, builds and tests
- Use read to examine files instead of cat or sed.
- Use edit for precise changes (edits[].oldText must match exactly)
- When changing multiple separate locations in one file, use one edit call with multiple entries in \
edits[] instead of multiple edit calls
- Each edits[].oldText is matched against the original file, not after earlier edits are applied. \
Do not emit overlapping or nested edits. Merge nearby changes into one edit.
- Keep edits[].oldText as small as possible while still being unique in the file. Do not pad with \
large unchanged regions.
- Use write only for new files or complete rewrites.
- Prefer editing existing files. Keep changes focused on the task, and verify them with relevant checks.
- Be concise in your responses
- Show file paths clearly when working with files

Current date: {date}
Current working directory: {root}
",
        root = root.display(),
        date = today(),
    );
    let mut files = Vec::new();
    for directory in root.ancestors() {
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = directory.join(name);
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= CONTEXT_FILE_BYTES)
                && let Ok(contents) = std::fs::read_to_string(&path)
            {
                files.push((path, contents));
            }
        }
    }
    if !files.is_empty() {
        text.push_str("\n# Project Context\n\nProject-specific instructions and guidelines:\n");
        for (path, contents) in files.iter().rev() {
            text.push_str(&format!(
                "\n## {}\n\n{}\n",
                path.display(),
                contents.trim_end()
            ));
        }
    }
    text
}

/// UTC date as YYYY-MM-DD, from days since the Unix epoch.
fn today() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        / 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_fixture as fixture;
    use session::Session;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::Arc;

    #[derive(Default)]
    struct Recorder {
        retries: Vec<(String, u32, Duration)>,
        replies: Vec<String>,
        calls: Vec<String>,
        compactions: Vec<(u64, usize, Option<AutoCompaction>)>,
        failed_compactions: Vec<String>,
        /// Whether each request asked for a compaction summary.
        waits: Vec<bool>,
        /// Answers to give, in order; questions beyond them are denied.
        answers: Vec<Answer>,
        questions: usize,
    }
    impl AgentUi for Recorder {
        fn waiting(&mut self, compacting: bool) {
            self.waits.push(compacting);
        }
        fn progress(&mut self, _: &StreamProgress) {}
        fn retrying(&mut self, error: &str, attempt: u32, _: u32, delay: Duration) {
            self.retries.push((error.to_owned(), attempt, delay));
        }
        fn assistant(&mut self, text: &str) {
            self.replies.push(text.to_owned());
        }
        fn tool_call(&mut self, name: &str, _: &str) {
            self.calls.push(name.to_owned());
        }
        fn tool_running(&mut self) {}
        fn tool_result(&mut self, _: &str, _: &str, _: Option<&ToolDetail>, _: bool) {}
        fn command_result(&mut self, _: &str, _: bool) {}
        fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>) {
            self.compactions
                .push((compaction.tokens_before, compaction.kept_items, automatic));
        }
        fn compaction_failed(&mut self, error: &AgentError) {
            self.failed_compactions.push(error.to_string());
        }
        fn approve(&mut self, _: &str, _: &AtomicBool) -> Answer {
            self.questions += 1;
            self.answers
                .get(self.questions - 1)
                .copied()
                .unwrap_or(Answer::Deny)
        }
    }

    /// Creates a scratch directory and removes it when dropped, also when a test panics.
    pub(super) struct ScratchDir(PathBuf);

    impl ScratchDir {
        pub(super) fn new(path: PathBuf) -> Self {
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0).or_else(|_| std::fs::remove_file(&self.0));
        }
    }

    fn stream(events: &[Value]) -> Vec<u8> {
        let body = fixture::wire(events, false);
        let mut response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend(body);
        response
    }

    /// Answers each connection with the next scripted response; returns the request bodies.
    fn serve(responses: Vec<Vec<u8>>) -> (SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
        let (address, server) = serve_requests(responses);
        let bodies = std::thread::spawn(move || {
            let requests = server.join().unwrap();
            requests.into_iter().map(|(_, body)| body).collect()
        });
        (address, bodies)
    }

    /// As `serve`, returning each request's head (request line and headers, lowercased) too.
    fn serve_requests(
        responses: Vec<Vec<u8>>,
    ) -> (SocketAddr, std::thread::JoinHandle<Vec<(String, Value)>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (connection, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(connection);
                let mut length = 0;
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line.to_lowercase());
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                requests.push((head, serde_json::from_slice(&body).unwrap()));
                reader.get_mut().write_all(&response).unwrap();
            }
            requests
        });
        (address, server)
    }

    fn agent(endpoint: SocketAddr, name: &str) -> (Agent, ScratchDir) {
        let root =
            std::env::temp_dir().join(format!("scramjet-agent-{name}-{}", std::process::id()));
        let scratch = ScratchDir::new(root.clone());
        let path = root.join("session.jsonl");
        std::fs::write(&path, "{\"type\":\"session\",\"version\":1}\n").unwrap();
        let config = AgentConfig {
            endpoint,
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
        };
        (
            Agent::start(config, SessionChoice::Open(path)).unwrap(),
            scratch,
        )
    }

    #[test]
    fn compaction_summarizes_older_items_and_resumes_compacted() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events(
                "first summary",
                fixture::known_usage(),
            )),
            stream(&fixture::text_events(
                "updated summary",
                fixture::known_usage(),
            )),
            stream(&fixture::text_events("continued", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "compact");
        let add_call = |agent: &mut Agent, name: &str, path: &str| {
            let arguments = json!({"path":path}).to_string();
            let id = format!("{name}-{path}");
            let call =
                json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments});
            agent.add(call).unwrap();
            let output = json!({"type":"function_call_output","call_id":id,"output":"ok"});
            agent.add(output).unwrap();
        };
        let old = json!({"role":"user", "content":"remember the goal"});
        let recent = json!({"role":"user", "content":"recent work ".repeat(7000)});
        agent.add(old).unwrap();
        add_call(&mut agent, "read", "a.rs");
        add_call(&mut agent, "edit", "b.rs");
        add_call(&mut agent, "bash", "ignored.rs");
        agent.add(recent.clone()).unwrap();
        // No response of the current model gave this count, so the conversation is estimated.
        agent.usage.context_tokens = Some(30_000);
        let estimate = compaction::estimate(agent.conversation());
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        let first = agent
            .compact("file paths", &mut ui, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!((first.kept_items, first.tokens_before), (1, estimate));
        assert_eq!(
            first.summary,
            "first summary\n\n<read-files>\na.rs\n</read-files>\n\n<modified-files>\nb.rs\n</modified-files>"
        );
        assert_eq!(agent.usage.context_tokens, None);
        assert_eq!(agent.conversation()[1], recent);
        assert_eq!(
            agent.conversation()[0]["content"],
            format!(
                "{}{}{}",
                compaction::SUMMARY_PREFIX,
                first.summary,
                compaction::SUMMARY_SUFFIX
            )
        );
        let (session, resumed, _) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, agent.conversation());
        agent.session = session;
        add_call(&mut agent, "read", "c.rs");
        add_call(&mut agent, "read", "b.rs");
        let newest = json!({"role":"user", "content":"newer work ".repeat(8000)});
        agent.add(newest.clone()).unwrap();
        let second = agent.compact("", &mut ui, &cancel).unwrap().unwrap();
        assert_eq!(second.kept_items, 1);
        assert!(second.tokens_before > 20_000);
        assert_eq!(
            second.summary,
            "updated summary\n\n<read-files>\na.rs\nc.rs\n</read-files>\n\n<modified-files>\nb.rs\n</modified-files>"
        );
        let (session, resumed, prompts) = Session::open(agent.session_path()).unwrap();
        assert_eq!(resumed, agent.conversation());
        assert_eq!(prompts.len(), 3);
        agent.session = session;
        agent.conversation = resumed;
        agent.turn("continue", &mut ui, &cancel).unwrap();
        let requests = server.join().unwrap();
        for request in &requests[..2] {
            assert_eq!(request["instructions"], compaction::SYSTEM_PROMPT);
            assert!(request.get("tools").is_none());
            assert!(request.get("tool_choice").is_none());
            assert_eq!(request["input"].as_array().unwrap().len(), 1);
        }
        let first_prompt = requests[0]["input"][0]["content"].as_str().unwrap();
        assert!(first_prompt.contains("[User]: remember the goal"));
        assert!(!first_prompt.contains("recent work"));
        assert!(first_prompt.ends_with("Additional focus: file paths"));
        let second_prompt = requests[1]["input"][0]["content"].as_str().unwrap();
        assert!(second_prompt.contains(&format!(
            "<previous-summary>\n{}\n</previous-summary>",
            first.summary
        )));
        assert!(second_prompt.contains("[User]: recent work"));
        assert!(!second_prompt.contains("newer work"));
        assert_eq!(requests[2]["input"][1], newest);
        assert_eq!(
            requests[2]["input"][0]["content"],
            format!(
                "{}{}{}",
                compaction::SUMMARY_PREFIX,
                second.summary,
                compaction::SUMMARY_SUFFIX
            )
        );
        assert!(requests[2]["input"][0].get(PRODUCER).is_none());
        assert_eq!(ui.replies, ["continued"]);
        assert!(ui.calls.is_empty());
    }

    #[test]
    fn a_turn_cut_in_two_is_summarized_apart_from_the_history_before_it() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("history", fixture::known_usage())),
            stream(&fixture::text_events("prefix", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "compact-split");
        let call = |id: &str, name: &str| json!({"type":"function_call","call_id":id,"name":name,"arguments":json!({"path":format!("{id}.rs")}).to_string()});
        let output = |id: &str, text: String| json!({"type":"function_call_output","call_id":id,"output":text});
        for item in [
            json!({"role":"user","content":"old goal"}),
            json!({"type":"message","role":"assistant","content":[{"text":"noted"}]}),
            json!({"role":"user","content":"long task"}),
            call("a", "read"),
            output("a", "x".repeat(90_000)),
            call("b", "edit"),
            output("b", "y".repeat(1000)),
            json!({"type":"message","role":"assistant","content":[{"text":"done"}]}),
        ] {
            agent.add(item).unwrap();
        }
        let mut ui = Recorder::default();
        let compacted = agent
            .compact("focus", &mut ui, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(compacted.kept_items, 3);
        assert_eq!(
            compacted.summary,
            "history\n\n---\n\n**Turn Context (split turn):**\n\nprefix\n\n<read-files>\na.rs\n</read-files>"
        );
        assert_eq!(agent.conversation()[1]["call_id"], "b");
        let requests = server.join().unwrap();
        let prompt = |n: usize| {
            requests[n]["input"][0]["content"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert!(prompt(0).contains("[User]: old goal\n\n[Assistant]: noted\n</conversation>"));
        assert!(prompt(0).ends_with("Additional focus: focus"));
        assert!(prompt(1).starts_with(
            "<conversation>\n[User]: long task\n\n[Assistant tool calls]: read(path=\"a.rs\")"
        ));
        assert!(prompt(1).contains("[... 88000 more characters truncated]\n</conversation>"));
        assert!(prompt(1).ends_with("Focus on what's needed to understand the kept suffix."));
    }

    #[test]
    fn a_reused_summary_keeps_one_file_list() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "prefix",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "compact-reuse");
        let previous = "earlier\n\n<read-files>\na.rs\n</read-files>";
        agent
            .session
            .append(&json!({"type":"compaction","summary":previous,"first_kept":0,"read_files":["a.rs"]}))
            .unwrap();
        agent.conversation = vec![compaction::summary_message(previous)];
        let call = |id: &str| json!({"type":"function_call","call_id":id,"name":"write","arguments":json!({"path":"a.rs"}).to_string()});
        let output = |id: &str, text: String| json!({"type":"function_call_output","call_id":id,"output":text});
        for item in [
            json!({"role":"user","content":"long task"}),
            call("a"),
            output("a", "x".repeat(90_000)),
            call("b"),
            output("b", "ok".into()),
        ] {
            agent.add(item).unwrap();
        }
        let compacted = agent
            .compact("", &mut Recorder::default(), &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(
            compacted.summary,
            "earlier\n\n---\n\n**Turn Context (split turn):**\n\nprefix\n\n<modified-files>\na.rs\n</modified-files>"
        );
        assert_eq!(server.join().unwrap().len(), 1);
    }

    #[test]
    fn failed_or_textless_compactions_leave_history_unchanged() {
        let mut failed = fixture::text_events("partial", fixture::known_usage());
        let terminal = failed.last_mut().unwrap();
        terminal["type"] = json!("response.failed");
        terminal["response"]["status"] = json!("failed");
        terminal["response"]["error"] = json!({"code":"invalid_prompt","message":"Invalid prompt"});
        let (endpoint, server) = serve(vec![
            stream(&failed),
            stream(&fixture::text_events(" ", fixture::known_usage())),
            stream(&fixture::call_events("{}", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "compact-failure");
        agent.add(json!({"role":"user","content":"old"})).unwrap();
        agent
            .add(json!({"role":"user","content":"recent ".repeat(12000)}))
            .unwrap();
        let before = agent.conversation().to_vec();
        let log = std::fs::read(agent.session_path()).unwrap();
        let mut ui = Recorder::default();
        for _ in 0..3 {
            assert!(agent.compact("", &mut ui, &AtomicBool::new(false)).is_err());
            assert_eq!(agent.conversation(), before);
            assert_eq!(std::fs::read(agent.session_path()).unwrap(), log);
        }
        assert!(matches!(
            agent.compact("", &mut ui, &AtomicBool::new(true)),
            Err(AgentError::Cancelled)
        ));
        assert_eq!(agent.conversation(), before);
        assert_eq!(server.join().unwrap().len(), 3);
        assert!(ui.calls.is_empty());
        assert!(ui.replies.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn short_history_compaction_is_a_no_op_without_a_request() {
        let (mut agent, _scratch) = agent(([127, 0, 0, 1], 9).into(), "compact-short");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        assert!(agent.compact("", &mut ui, &cancel).unwrap().is_none());
        agent.add(json!({"role":"user","content":"hi"})).unwrap();
        let log = std::fs::read(agent.session_path()).unwrap();
        assert!(agent.compact("", &mut ui, &cancel).unwrap().is_none());
        assert_eq!(std::fs::read(agent.session_path()).unwrap(), log);
        assert_eq!(agent.usage.requests, 0);
    }

    /// Usage of a response that filled `total` tokens of context.
    fn full_usage(total: u64) -> Value {
        json!({"input_tokens": total - 10, "output_tokens": 10, "total_tokens": total})
    }

    /// A conversation with a small old prompt and ~21k tokens of recent work to keep.
    fn seed_long_conversation(agent: &mut Agent) {
        agent
            .add(json!({"role":"user", "content":"remember the goal"}))
            .unwrap();
        agent
            .add(json!({"role":"user", "content":"recent work ".repeat(7000)}))
            .unwrap();
    }

    #[test]
    fn a_full_context_is_compacted_after_the_turn() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("done", full_usage(10_000))),
            stream(&fixture::text_events("summary", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-threshold");
        seed_long_conversation(&mut agent);
        agent.set_context_window(Some(20_000));
        let mut ui = Recorder::default();
        agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["done"]);
        assert_eq!(
            ui.compactions,
            [(10_000, 3, Some(AutoCompaction::Threshold))]
        );
        assert_eq!(ui.waits, [false, true]);
        // The turn ends after compacting: no further request is sent.
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["instructions"], compaction::SYSTEM_PROMPT);
        assert!(
            requests[1]["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains("[User]: remember the goal")
        );
        assert_eq!(agent.usage.context_tokens, None);
        assert!(ui.calls.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn a_failed_automatic_compaction_is_reported_as_such_after_the_reply() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("done", full_usage(10_000))),
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 4\r\n\r\nnope".to_vec(),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-threshold-failed");
        seed_long_conversation(&mut agent);
        agent.set_context_window(Some(20_000));
        let mut ui = Recorder::default();
        let error = agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "automatic compaction failed: provider: HTTP 400 Bad Request: nope"
        );
        assert_eq!(server.join().unwrap().len(), 2);
        assert_eq!(ui.replies, ["done"]);
        assert!(ui.compactions.is_empty());
        assert_eq!(agent.conversation().len(), 4);
    }

    #[test]
    fn a_turn_left_without_a_reply_is_compacted_before_the_next_prompt() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events(
                "counted by another model",
                full_usage(100),
            )),
            b"HTTP/1.1 400 Bad Request\r\ncontent-length: 4\r\n\r\nnope".to_vec(),
            stream(&fixture::text_events("sent anyway", full_usage(100))),
            stream(&fixture::text_events("summary", full_usage(100))),
            stream(&fixture::text_events("compacted first", full_usage(100))),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-before-prompt");
        agent.set_context_window(Some(40_000));
        // A turn cancelled after a tool call: its output is not in the last response's count.
        let cancelled = |producer: &str| {
            vec![
                json!({"role":"user","content":"old goal"}),
                json!({"type":"message","role":"assistant","content":[{"text":"noted"}],"_scramjet_model":producer}),
                json!({"role":"user","content":"big task ".repeat(9000)}),
                json!({"type":"function_call","call_id":"a","name":"read","arguments":"{}","_scramjet_model":producer}),
                json!({"type":"function_call_output","call_id":"a","output":"y".repeat(20_000)}),
            ]
        };
        let mut ui = Recorder::default();
        for (producer, prompt) in [("other", "one"), ("m", "two"), ("m", "three")] {
            agent.conversation = cancelled(producer);
            agent.usage.context_tokens = Some(20_000);
            agent
                .turn(prompt, &mut ui, &AtomicBool::new(false))
                .unwrap();
        }
        assert_eq!(
            ui.replies,
            ["counted by another model", "sent anyway", "compacted first"]
        );
        assert_eq!(
            ui.failed_compactions,
            ["provider: HTTP 400 Bad Request: nope"]
        );
        assert_eq!(
            ui.compactions,
            [(25_000, 3, Some(AutoCompaction::Threshold))]
        );
        assert_eq!(ui.waits, [false, true, false, true, false]);
        let requests = server.join().unwrap();
        assert_eq!(requests[0]["input"].as_array().unwrap().len(), 6);
        assert_eq!(requests[2]["input"].as_array().unwrap().len(), 6);
        assert_eq!(requests[2]["input"][5]["content"], "two");
        assert!(
            requests[3]["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains("[User]: old goal\n\n[Assistant]: noted\n</conversation>")
        );
        let input = requests[4]["input"].as_array().unwrap();
        assert_eq!(input.len(), 5);
        assert_eq!(input[0], compaction::summary_message("summary"));
        assert_eq!(input[4]["content"], "three");
    }

    #[test]
    fn the_threshold_needs_a_known_window_and_a_full_enough_context() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "done",
            full_usage(1_000_000),
        ))]);
        let (mut unknown, _scratch) = agent(endpoint, "auto-threshold-unknown");
        let mut ui = Recorder::default();
        unknown
            .turn("hi", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());

        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "done",
            fixture::known_usage(),
        ))]);
        let (mut low, _scratch) = agent(endpoint, "auto-threshold-low");
        low.set_context_window(Some(20_000));
        low.turn("hi", &mut ui, &AtomicBool::new(false)).unwrap();
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());
    }

    /// A stream that ends with the provider saying the input exceeded the context window.
    fn overflow_events() -> Vec<Value> {
        let mut events = fixture::text_events("partial", fixture::known_usage());
        events.truncate(4);
        events.push(json!({"type":"error","code":"context_length_exceeded",
            "message":"Your input exceeds the context window of this model."}));
        events
    }

    #[test]
    fn an_overflowing_request_is_compacted_and_sent_again() {
        let (endpoint, server) = serve(vec![
            stream(&overflow_events()),
            stream(&fixture::text_events("summary", fixture::known_usage())),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-overflow");
        seed_long_conversation(&mut agent);
        let mut ui = Recorder::default();
        agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(ui.replies, ["done"]);
        assert!(matches!(
            ui.compactions[..],
            [(_, 2, Some(AutoCompaction::Overflow))]
        ));
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1]["instructions"], compaction::SYSTEM_PROMPT);
        let retried = requests[2]["input"].as_array().unwrap();
        assert_eq!(
            retried[0]["content"],
            format!(
                "{}summary{}",
                compaction::SUMMARY_PREFIX,
                compaction::SUMMARY_SUFFIX
            )
        );
        assert_eq!(retried[2], json!({"role":"user","content":"continue"}));
        assert!(ui.calls.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn a_second_overflow_fails_the_turn_with_a_clear_message() {
        let (endpoint, server) = serve(vec![
            stream(&overflow_events()),
            stream(&fixture::text_events("summary", fixture::known_usage())),
            stream(&overflow_events()),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "auto-overflow-twice");
        seed_long_conversation(&mut agent);
        let mut ui = Recorder::default();
        let error = agent
            .turn("continue", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "context overflow: provider error: Your input exceeds the context window of this \
model. (again after compacting once; reduce the context or switch to a larger-context model)"
        );
        assert_eq!(server.join().unwrap().len(), 3);
        assert_eq!(ui.compactions.len(), 1);
        assert!(ui.replies.is_empty());
        assert!(ui.retries.is_empty());
    }

    #[test]
    fn an_overflow_that_cannot_compact_fails_the_turn() {
        let (endpoint, server) = serve(vec![stream(&overflow_events())]);
        let (mut agent, _scratch) = agent(endpoint, "auto-overflow-nothing");
        let mut ui = Recorder::default();
        let error = agent
            .turn("hi", &mut ui, &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "context overflow: provider error: Your input exceeds the context window of this \
model. (nothing is old enough to compact)"
        );
        assert_eq!(server.join().unwrap().len(), 1);
        assert!(ui.compactions.is_empty());
        assert!(ui.replies.is_empty());
        assert!(ui.retries.is_empty());
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
        assert_eq!(agent.conversation()[1]["_scramjet_model"], "m");
        assert!(bodies[1]["input"][0].get("_scramjet_model").is_none());
        let kinds: Vec<_> = agent
            .conversation()
            .iter()
            .map(|item| item["type"].as_str().or(item["role"].as_str()).unwrap())
            .collect();
        assert_eq!(kinds, ["user", "message"]);
    }

    #[test]
    fn the_context_window_comes_from_proxy_model_cards() {
        let cards = json!({"models": [
            {"requestId": "other", "limits": {"context": 1}},
            {"requestId": "m", "limits": {"context": 1_050_000, "output": 128_000}},
        ]})
        .to_string();
        let (endpoint, server) =
            serve_gets(vec![("200 OK", cards), ("404 Not Found", "no".into())]);
        let (mut cards, _scratch) = agent(endpoint, "cards");
        assert_eq!(cards.load_model_card(), Some(1_050_000));
        assert_eq!(cards.info.output, Some(128_000));
        cards.set_model("m").unwrap();
        assert_eq!(cards.info.output, None);
        assert_eq!(cards.load_model_card(), None);
        assert_eq!(server.join().unwrap(), ["GET /api/model-cards HTTP/1.1"; 2]);
        assert_eq!(cards.load_model_card(), None);
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (mut hung, _scratch) = agent(silent.local_addr().unwrap(), "cards-silent");
        let started = std::time::Instant::now();
        assert_eq!(hung.load_model_card(), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// Answers one GET request per response in turn; returns the request lines.
    fn serve_gets(
        responses: Vec<(&'static str, String)>,
    ) -> (SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (connection, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(connection);
                let mut head = String::new();
                while !head.ends_with("\r\n\r\n") {
                    reader.read_line(&mut head).unwrap();
                }
                requests.push(head.lines().next().unwrap().to_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
            }
            requests
        });
        (endpoint, server)
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
    }

    #[test]
    fn allowing_all_stops_later_approval_questions() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let bash = |id: &str| {
            let events = fixture::call_events(r#"{"command":"true"}"#, fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text.replace("\"inert\"", "\"bash\"").replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let (endpoint, server) = serve(vec![
            stream(&bash("call_1")),
            stream(&bash("call_2")),
            stream(&fixture::text_events("done", fixture::known_usage())),
            stream(&bash("call_3")),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "allow-all");
        agent.config.ask = true;
        let mut ui = Recorder {
            answers: vec![Answer::AllowAll],
            ..Recorder::default()
        };
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        agent.turn("two", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (1, 3));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(outputs, ["(no output)"; 3]);
    }

    #[test]
    fn tool_calls_run_without_questions_unless_asked() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let bash = |id: &str, command: &str| {
            let arguments = json!({ "command": command }).to_string();
            let events = fixture::call_events(&arguments, fixture::known_usage());
            let text = serde_json::to_string(&events).unwrap();
            let text = text.replace("\"inert\"", "\"bash\"").replace("call_1", id);
            serde_json::from_str::<Vec<Value>>(&text).unwrap()
        };
        let (endpoint, server) = serve(vec![
            stream(&bash("call_1", "ls -a > listing")),
            stream(&fixture::text_events("done", fixture::known_usage())),
            stream(&bash("call_2", "ls -a")),
            stream(&fixture::text_events("done", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "unasked");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("list", &mut ui, &cancel).unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (0, 1));
        assert!(agent.root().join("listing").exists());
        agent.config.ask = true;
        agent.turn("list again", &mut ui, &cancel).unwrap();
        server.join().unwrap();
        assert_eq!((ui.questions, ui.calls.len()), (1, 2));
        let outputs: Vec<_> = agent
            .conversation()
            .iter()
            .filter_map(|item| item["output"].as_str())
            .collect();
        assert_eq!(outputs, ["(no output)", "The user denied this tool call."]);
    }

    #[test]
    fn bang_commands_run_without_a_turn_and_are_recalled_as_typed() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "ok",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "bang");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent
            .shell("echo hi; exit 3", true, &mut ui, &cancel)
            .unwrap();
        agent.shell("echo secret", false, &mut ui, &cancel).unwrap();
        assert_eq!(ui.calls, ["bash", "bash"]);
        assert_eq!(agent.usage.requests, 0);
        let message = "Ran `echo hi; exit 3`\n```\nhi\n```\n\nCommand exited with code 3";
        assert_eq!(
            agent.conversation(),
            [json!({"role":"user","content":message,"_scramjet_typed":"!echo hi; exit 3"})]
        );
        assert_eq!(agent.prompts(), ["!echo hi; exit 3"]);
        agent.turn("next", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(
            bodies[0]["input"],
            json!([{"role":"user","content":message},{"role":"user","content":"next"}])
        );
        assert!(!bodies[0].to_string().contains("_scramjet_typed"));
        let path = agent.session_path().to_owned();
        let (_, _, prompts) = Session::open(&path).unwrap();
        assert_eq!(prompts, ["!echo hi; exit 3", "next"]);
        assert!(!std::fs::read_to_string(&path).unwrap().contains("secret"));
    }

    #[test]
    fn a_cancelled_bang_command_is_recorded_as_cancelled() {
        let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let (endpoint, _server) = serve(vec![]);
        let (mut agent, _scratch) = agent(endpoint, "bang-cancel");
        let mut ui = Recorder::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            setter.store(true, Ordering::Relaxed);
        });
        agent.shell("sleep 30", true, &mut ui, &cancel).unwrap();
        assert_eq!(
            agent.conversation()[0]["content"],
            "Ran `sleep 30`\n(no output)\n\n(command cancelled)"
        );
    }

    #[test]
    fn a_new_session_starts_an_empty_conversation_next_to_the_old_one() {
        let (endpoint, server) = serve(vec![
            stream(&fixture::text_events("first", fixture::known_usage())),
            stream(&fixture::text_events("second", fixture::known_usage())),
        ]);
        let (mut agent, _scratch) = agent(endpoint, "new-session");
        let mut ui = Recorder::default();
        let cancel = AtomicBool::new(false);
        agent.turn("one", &mut ui, &cancel).unwrap();
        let old = agent.session_path().to_owned();
        agent.new_session().unwrap();
        assert_eq!(agent.usage.requests, 0);
        assert!(agent.conversation().is_empty());
        agent.turn("two", &mut ui, &cancel).unwrap();
        let bodies = server.join().unwrap();
        assert_eq!(bodies[1]["input"], json!([{"role":"user","content":"two"}]));
        assert_ne!(bodies[0]["prompt_cache_key"], bodies[1]["prompt_cache_key"]);
        let new = agent.session_path().to_owned();
        assert_eq!(new.parent(), old.parent());
        let lines = |path: &Path| std::fs::read_to_string(path).unwrap().lines().count();
        assert_eq!((lines(&old), lines(&new)), (3, 4));

        let directory = new.parent().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::write(directory, "").unwrap();
        assert!(matches!(agent.new_session(), Err(AgentError::Session(_))));
        assert_eq!(agent.session_path(), new);
        assert_eq!(agent.conversation().len(), 2);
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
            endpoint: ([127, 0, 0, 1], 9).into(),
            model: "new".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
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
    fn model_switches_are_saved_at_once_and_resumed() {
        let (endpoint, server) = serve(vec![stream(&fixture::text_events(
            "hi",
            fixture::known_usage(),
        ))]);
        let (mut agent, _scratch) = agent(endpoint, "model-switch");
        let path = agent.session_path().to_owned();
        let changes = || {
            std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|line| line.contains("model_change"))
                .count()
        };
        assert_eq!(agent.last_model(), None);
        agent.set_model("m").unwrap();
        agent.set_model("m").unwrap();
        agent.set_model("other").unwrap();
        assert_eq!(changes(), 2);
        let (session, items, _) = Session::open(&path).unwrap();
        assert_eq!((session.model(), items.len()), (Some("other"), 0));

        agent.set_model("m").unwrap();
        agent
            .turn("one", &mut Recorder::default(), &AtomicBool::new(false))
            .unwrap();
        server.join().unwrap();
        agent.set_model("m").unwrap();
        assert_eq!(changes(), 3);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"model_change\",\"model\":\"later\"}\n")
            .unwrap();
        let (session, items, _) = Session::open(&path).unwrap();
        assert_eq!((session.model(), items.len()), (Some("later"), 2));

        agent.new_session().unwrap();
        assert_eq!(agent.last_model(), Some("m"));
        let (session, _, _) = Session::open(agent.session_path()).unwrap();
        assert_eq!(session.model(), Some("m"));
    }

    #[test]
    fn the_model_list_is_the_endpoints() {
        let list = json!({"data": [{"id": "claude"}, {"id": "gpt"}]}).to_string();
        let (endpoint, server) = serve_gets(vec![("200 OK", list), ("404 Not Found", "no".into())]);
        let (agent, _scratch) = agent(endpoint, "model-list");
        let ids = Some(vec!["claude".to_owned(), "gpt".to_owned()]);
        assert_eq!(agent.models(), ids);
        assert_eq!(agent.models(), None);
        assert_eq!(server.join().unwrap(), ["GET /v1/models HTTP/1.1"; 2]);
    }

    #[test]
    fn model_cards_choose_the_protocol_and_names_do_without_one() {
        let cards = json!({"models": [
            {"requestId": "claude-opus-5-5", "supportedProtocols": ["anthropic-messages"],
             "limits": {"context": 1_000_000, "output": 128_000}, "reasoning": {"levels": ["high"]}},
            {"requestId": "both", "supportedProtocols": ["openai-responses", "anthropic-messages"],
             "reasoning": null},
        ]})
        .to_string();
        let (endpoint, server) = serve_gets(vec![("200 OK", cards.clone()), ("200 OK", cards)]);
        let (mut agent, _scratch) = agent(endpoint, "protocols");
        agent.set_model("claude-opus-5-5").unwrap();
        assert_eq!(agent.load_model_card(), Some(1_000_000));
        let expected = ModelInfo {
            protocol: Protocol::Messages,
            window: Some(1_000_000),
            output: Some(128_000),
            reasoning: true,
        };
        assert_eq!(agent.info, expected);
        agent.set_model("both").unwrap();
        agent.load_model_card();
        assert_eq!(agent.info.protocol, Protocol::Responses);
        assert!(!agent.info.reasoning);
        server.join().unwrap();
        assert_eq!(ModelInfo::guess("claude-new").protocol, Protocol::Messages);
        assert_eq!(ModelInfo::guess("gpt-new").protocol, Protocol::Responses);
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
            head.contains("anthropic-beta: interleaved-thinking-2025-05-14\r\n"),
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

    /// A streamed Messages reply with these events.
    fn messages_stream(events: &[Value]) -> Vec<u8> {
        let body: String = events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap()
                )
            })
            .collect();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
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
            assert!(!head.contains("anthropic-beta"), "{head}");
        }
        let first = &requests[0].1;
        assert_eq!(first["model"], "claude-opus-5-5");
        assert_eq!(first["max_tokens"], messages::DEFAULT_MAX_TOKENS);
        assert_eq!(
            first["thinking"],
            json!({"type":"adaptive","display":"summarized"})
        );
        assert_eq!(first["system"][0]["text"], agent.instructions);
        assert_eq!(first["tools"][0]["name"], "read");
        assert_eq!(
            first["tools"][0]["input_schema"]["required"],
            json!(["path"])
        );
        assert_eq!(
            first["messages"],
            json!([{"role":"user","content":[{"type":"text","text":"What is in notes.txt?",
                "cache_control":{"type":"ephemeral"}}]}])
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
        assert_eq!(result["cache_control"], json!({"type":"ephemeral"}));
        assert!(second[0]["content"][0].get("cache_control").is_none());
        // Input counts cache reads and writes, as Responses input does.
        assert_eq!(agent.usage.input_tokens, 17 + 37);
        assert_eq!(agent.usage.cached_input_tokens, 10);
        assert_eq!(agent.usage.output_tokens, 24);
        assert_eq!(agent.usage.context_tokens, Some(41));
        assert!(
            agent.conversation()[1..]
                .iter()
                .filter(|item| item.get("type").is_some())
                .all(|item| item["_scramjet_model"] == "claude-opus-5-5"
                    || item["type"] == "function_call_output")
        );
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
            endpoint: ([127, 0, 0, 1], 9).into(),
            model: "m".into(),
            root: root.clone(),
            effort: None,
            max_output_tokens: None,
            ask: false,
            retries: 3,
        };
        let agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
        assert_eq!(agent.request()["prompt_cache_key"], "1700000000000-42");
    }
}
