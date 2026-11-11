//! What the agent's tests share, and the tests of sessions, models and efforts.
use super::*;
use crate::provider_fixture as fixture;
use client::Protocol;
use session::Session;
use std::io::{BufRead, BufReader, Read, Write};
use std::{net::SocketAddr, sync::atomic::AtomicBool, time::Duration};

#[derive(Default)]
pub(super) struct Recorder {
    pub(super) retries: Vec<(String, u32, Duration)>,
    /// Each wait for a provider's Retry-After before a request is sent.
    pub(super) cooling: Vec<Duration>,
    pub(super) replies: Vec<String>,
    pub(super) calls: Vec<String>,
    pub(super) compactions: Vec<(u64, usize, Option<AutoCompaction>)>,
    pub(super) failed_compactions: Vec<String>,
    /// What each request asked for.
    pub(super) waits: Vec<Purpose>,
    /// Answers to give, in order; questions beyond them are denied.
    pub(super) answers: Vec<Answer>,
    pub(super) questions: usize,
    /// Options to choose, in order; questions beyond them are cancelled.
    pub(super) choices: Vec<Option<usize>>,
    /// What a replay showed, in order, with the results of calls.
    pub(super) shown: Vec<String>,
    /// What each call of `steering` and then `follow_up` gives, in order; then nothing.
    pub(super) steering: Vec<Vec<String>>,
    pub(super) follow_ups: Vec<Vec<String>>,
    /// What each call of `effort` gives, in order; then nothing.
    pub(super) efforts: Vec<Option<String>>,
    /// What each call of `pause` gives, in order; then no pause.
    pub(super) pauses: Vec<bool>,
    /// Each running `subagent` call's children, as each update showed them.
    pub(super) children: Vec<Children>,
    /// The details of the results shown.
    pub(super) details: Vec<ToolDetail>,
    /// The results shown after they came (`tool_result_at`), with how long after.
    pub(super) ended: Vec<(String, Duration)>,
    /// Each `usage` call's session tokens and context, and the model's output rate.
    pub(super) usage: Vec<(Tokens, Option<u64>)>,
    pub(super) rates: Vec<Option<f64>>,
    /// Shows a reply's calls running at the same time each as it runs, `shown` saying whose
    /// call what follows is (`of N`, `of none`).
    pub(super) together: bool,
    /// Offers the steps the agent suggests (`offers_suggestions`).
    pub(super) suggests: bool,
}
impl AgentUi for Recorder {
    fn waiting(&mut self, purpose: Purpose) {
        self.waits.push(purpose);
    }
    fn progress(&mut self, update: &StreamProgress) {
        if let StreamProgress::Reasoning(text) = update {
            self.shown.push(format!("thinking {text}"));
        }
    }
    fn retrying(&mut self, error: &str, attempt: u32, _: u32, delay: Duration) {
        self.retries.push((error.to_owned(), attempt, delay));
    }
    fn cooling(&mut self, delay: Duration) {
        self.cooling.push(delay);
    }
    fn assistant(&mut self, text: &str) {
        self.shown.push(format!("reply {text}"));
        self.replies.push(text.to_owned());
    }
    fn tool_call(&mut self, name: &str, summary: &str, compact: Option<&CompactRead>) {
        match compact {
            Some(compact) => self
                .shown
                .push(format!("call {name} {summary} {compact:?}")),
            None => self.shown.push(format!("call {name} {summary}")),
        }
        self.calls.push(name.to_owned());
    }
    fn tool_running(&mut self, _: std::time::Instant) {}
    fn running_calls(&mut self, count: usize) -> bool {
        if self.together {
            self.shown.push(format!("{count} calls"));
        }
        self.together
    }
    fn of_call(&mut self, index: Option<usize>) {
        let index = index.map_or_else(|| "none".to_owned(), |index| index.to_string());
        self.shown.push(format!("of {index}"));
    }
    fn subagents(&mut self, children: &Children) {
        self.children.push(children.clone());
    }
    fn tool_result(&mut self, name: &str, output: &str, detail: Option<&ToolDetail>, failed: bool) {
        self.details.extend(detail.cloned());
        let failed = if failed { " failed" } else { "" };
        let detail = detail
            .map(|detail| format!(" {detail:?}"))
            .unwrap_or_default();
        self.shown
            .push(format!("result {name} {output}{failed}{detail}"));
    }
    fn tool_result_at(
        &mut self,
        name: &str,
        output: &str,
        detail: Option<&ToolDetail>,
        failed: bool,
        at: std::time::Instant,
    ) {
        self.ended.push((name.to_owned(), at.elapsed()));
        self.tool_result(name, output, detail, failed);
    }
    fn command_started(&mut self, command: &str, include: bool) {
        self.shown.push(format!("started {command} {include}"));
    }
    fn command_result(&mut self, ran: &UserCommand) {
        let UserCommand {
            command,
            include,
            output,
            ending,
        } = ran;
        let mark = if *include { "!" } else { "!!" };
        self.shown
            .push(format!("{mark}{command} {output:?} {ending:?}"));
    }
    fn stopped(&mut self, error: Option<&str>) {
        self.shown
            .push(format!("stopped {}", error.unwrap_or("(cancelled)")));
    }
    fn compacted(&mut self, compaction: &Compaction, automatic: Option<AutoCompaction>) {
        self.compactions
            .push((compaction.tokens_before, compaction.kept_items, automatic));
    }
    fn compaction_failed(&mut self, error: &AgentError) {
        self.failed_compactions.push(error.to_string());
    }
    fn user_prompt(&mut self, prompt: &str) {
        self.shown.push(format!("prompt {prompt}"));
    }
    fn command_prompt(&mut self, typed: &str, sent: &str) {
        self.shown.push(format!("command {typed} sent {sent}"));
    }
    fn compaction_summary(&mut self, summary: &str) {
        self.shown.push(format!("summary {summary}"));
    }
    fn branch_summary(&mut self, summary: &str) {
        self.shown.push(format!("branch {summary}"));
    }
    fn approve(&mut self, _: &str, _: &AtomicBool) -> Answer {
        self.questions += 1;
        self.answers
            .get(self.questions - 1)
            .copied()
            .unwrap_or(Answer::Deny)
    }
    fn question(&mut self, message: &str) {
        self.shown.push(format!("question {message}"));
    }
    fn notice(&mut self, text: &str) {
        self.shown.push(format!("notice {text}"));
    }
    fn next_step(&mut self, record: &str) {
        self.shown.push(format!("next step {record}"));
    }
    fn steering(&mut self) -> Vec<String> {
        match self.steering.is_empty() {
            true => Vec::new(),
            false => self.steering.remove(0),
        }
    }
    fn follow_up(&mut self) -> Vec<String> {
        match self.follow_ups.is_empty() {
            true => Vec::new(),
            false => self.follow_ups.remove(0),
        }
    }
    fn effort(&mut self) -> Option<String> {
        match self.efforts.is_empty() {
            true => None,
            false => self.efforts.remove(0),
        }
    }
    fn offers_suggestions(&mut self) -> bool {
        self.suggests
    }
    fn pause(&mut self) -> bool {
        !self.pauses.is_empty() && self.pauses.remove(0)
    }
    fn usage(&mut self, tokens: Tokens, context: Option<u64>, rate: Option<f64>) {
        self.usage.push((tokens, context));
        self.rates.push(rate);
    }
    fn model_switched(&mut self, agent: &Agent) {
        self.shown.push(format!("switched {}", agent.model()));
    }
    fn choose(
        &mut self,
        title: &str,
        options: &[Choice],
        recommended: Option<usize>,
    ) -> Option<usize> {
        let labels: Vec<_> = options.iter().map(|o| o.label.as_str()).collect();
        self.shown
            .push(format!("choose {title} {labels:?} {recommended:?}"));
        match self.choices.is_empty() {
            true => None,
            false => self.choices.remove(0),
        }
    }
}

/// Tells what a paused turn shows from another thread (`Agent::show_paused`): `waiting`,
/// `progress`, `output TEXT`, `asked QUESTION` (allowed) and `result NAME`.
pub(super) struct Watching(pub(super) std::sync::mpsc::Sender<String>);
impl Watching {
    fn tell(&self, seen: String) {
        let _ = self.0.send(seen);
    }
}
impl AgentUi for Watching {
    fn waiting(&mut self, _: Purpose) {
        self.tell("waiting".into());
    }
    fn progress(&mut self, _: &StreamProgress) {
        self.tell("progress".into());
    }
    fn retrying(&mut self, _: &str, _: u32, _: u32, _: Duration) {}
    fn assistant(&mut self, _: &str) {}
    fn tool_call(&mut self, _: &str, _: &str, _: Option<&CompactRead>) {}
    fn tool_running(&mut self, _: std::time::Instant) {}
    fn tool_output(&mut self, output: &str) {
        self.tell(format!("output {output}"));
    }
    fn tool_result(&mut self, name: &str, _: &str, _: Option<&ToolDetail>, _: bool) {
        self.tell(format!("result {name}"));
    }
    fn command_started(&mut self, _: &str, _: bool) {}
    fn command_result(&mut self, _: &UserCommand) {}
    fn stopped(&mut self, _: Option<&str>) {}
    fn compacted(&mut self, _: &Compaction, _: Option<AutoCompaction>) {}
    fn compaction_failed(&mut self, _: &AgentError) {}
    fn user_prompt(&mut self, _: &str) {}
    fn compaction_summary(&mut self, _: &str) {}
    fn branch_summary(&mut self, _: &str) {}
    fn approve(&mut self, question: &str, _: &AtomicBool) -> Answer {
        self.tell(format!("asked {question}"));
        Answer::Allow
    }
    fn question(&mut self, _: &str) {}
    fn notice(&mut self, _: &str) {}
    fn next_step(&mut self, _: &str) {}
    fn choose(&mut self, _: &str, _: &[Choice], _: Option<usize>) -> Option<usize> {
        None
    }
}

/// Whether `program` (`bash`, `python3`…), which a test runs, is missing, as on Windows
/// without Git Bash or Python; the test then says so and is passed over.
pub(crate) fn missing(program: &str) -> bool {
    let found = match program {
        "bash" => crate::os::bash().is_ok(),
        _ => std::process::Command::new(program)
            .args(["-c", ""])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success()),
    };
    if !found {
        eprintln!("passed over: {program} is not here");
    }
    !found
}

/// Writes the shell script `body` to `path`, executable. A child process writes it: a file this
/// process held open for writing would be inherited by any child another test forks meanwhile,
/// and running the script while that child holds it fails with `ETXTBSY`.
pub(crate) fn script(path: &Path, body: &str) {
    let mut writer = std::process::Command::new("sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = writer.stdin.take().unwrap();
    input
        .write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
        .unwrap();
    drop(input);
    assert!(writer.wait().unwrap().success(), "{}", path.display());
}

/// Creates a scratch directory and removes it when dropped, also when a test panics.
pub(super) struct ScratchDir(pub(super) PathBuf);

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

pub(super) fn stream(events: &[Value]) -> Vec<u8> {
    let body = fixture::wire(events, false);
    let mut response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    response
}

/// A streamed Messages reply with these events.
pub(super) fn messages_stream(events: &[Value]) -> Vec<u8> {
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

/// Answers each connection with the next scripted response; returns the request bodies.
pub(super) fn serve(responses: Vec<Vec<u8>>) -> (SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    let (address, server) = serve_requests(responses);
    let bodies = std::thread::spawn(move || {
        let requests = server.join().unwrap();
        requests.into_iter().map(|(_, body)| body).collect()
    });
    (address, bodies)
}

/// As `serve`, returning each request's head (request line and headers, lowercased) too.
pub(super) fn serve_requests(
    responses: Vec<Vec<u8>>,
) -> (SocketAddr, std::thread::JoinHandle<Vec<(String, Value)>>) {
    let count = responses.len();
    let mut responses = responses.into_iter();
    serve_answering(count, move |_| responses.next().unwrap())
}

/// Answers `count` connections, one at a time, with what `answer` makes of each request's
/// body; returns the requests in the order they were answered.
pub(super) fn serve_answering(
    count: usize,
    mut answer: impl FnMut(&Value) -> Vec<u8> + Send + 'static,
) -> (SocketAddr, std::thread::JoinHandle<Vec<(String, Value)>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    // A test whose agent makes fewer requests fails rather than waiting for good.
    listener.set_nonblocking(true).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..count {
            let connection = loop {
                match listener.accept() {
                    Ok((connection, _)) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "the scripted server got {} of {count} requests",
                            requests.len()
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            // Windows' accepted sockets inherit the listener's mode.
            connection.set_nonblocking(false).unwrap();
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
            // A GET has none.
            let body: Value = match length {
                0 => Value::Null,
                _ => serde_json::from_slice(&body).unwrap(),
            };
            let response = answer(&body);
            requests.push((head, body));
            reader.get_mut().write_all(&response).unwrap();
        }
        requests
    });
    (address, server)
}

/// A login's servers (GitHub's, OpenAI's): answers `count` requests, each with what `answer`
/// makes of its head (request line and headers, as sent) and body, as JSON; returns them as
/// they came, with the server's base URL.
pub(super) fn serve_json(
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

/// The models the tests use.
const SERVED: [&str; 16] = [
    "m",
    "a",
    "b",
    "c",
    "other",
    "plain",
    "old",
    "new",
    "flash",
    "both",
    "gpt-6-luna",
    "gpt-6-terra",
    "claude-opus-5-5",
    "claude-sonnet-4-5",
    "claude-haiku-5-5",
    "claude-haiku-4-5-20251001",
];

/// A provider at `endpoint` offering the models the tests use, each as its name suggests
/// (`ModelInfo::guess`): Claude on Messages, the others on Responses, all reasoning at any level
/// and taking images, with nothing known of their limits or prices.
pub(super) fn serving(endpoint: SocketAddr) -> Providers {
    Providers::given(served(endpoint))
}

/// `serving`'s providers in a `models.json`'s format.
pub(super) fn served(endpoint: SocketAddr) -> Value {
    let any: serde_json::Map<String, Value> =
        ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
            .iter()
            .map(|level| (level.to_string(), json!(settings_level(level))))
            .collect();
    let models: Vec<Value> = (SERVED.iter())
        .map(|id| {
            let (api, base) = match id.starts_with("claude-") {
                true => ("anthropic-messages", format!("http://{endpoint}")),
                false => ("openai-responses", format!("http://{endpoint}/v1")),
            };
            json!({"id": id, "api": api, "baseUrl": base, "reasoning": true,
                "input": ["text", "image"], "thinkingLevelMap": any})
        })
        .collect();
    json!({"providers": {"test": {"models": models}}})
}

fn settings_level(level: &str) -> &str {
    if level == "off" { "none" } else { level }
}

/// The configuration of an agent of the project `root`, with the model `m`.
pub(super) fn config(endpoint: SocketAddr, root: &Path) -> AgentConfig {
    AgentConfig {
        providers: serving(endpoint),
        model: "m".into(),
        root: root.to_owned(),
        effort: None,
        max_output_tokens: None,
        ask: false,
        retries: Default::default(),
        compacting: Default::default(),
        tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
        only_tools: None,
        command_sets: Vec::new(),
        skills: Vec::new(),
        skill_options: Default::default(),
        prompt: Default::default(),
        mcp_servers: Vec::new(),
        mcp_disabled: Vec::new(),
        sandbox: None,
        session_dir: None,
    }
}

pub(super) fn agent(endpoint: SocketAddr, name: &str) -> (Agent, ScratchDir) {
    let root = std::env::temp_dir().join(format!("scramjet-agent-{name}-{}", std::process::id()));
    let scratch = ScratchDir::new(root.clone());
    let path = root.join("session.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"version\":1}\n").unwrap();
    let config = config(endpoint, &root);
    let mut agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
    // The user's own agents and command sets stay out of the tests.
    agent.agents_directory = None;
    agent.agent_directory = None;
    agent.approvals = Some(root.join("approved-agents.json"));
    agent.skills = Skills::default();
    project_sets(&mut agent);
    agent.instructions = instructions::agent(
        &root,
        &agent.offered(),
        &agent.commands,
        &[],
        &agent.skills,
        &Default::default(),
    );
    (agent, scratch)
}

/// Gives `agent` the command sets of its project's `.scramjet` directory alone.
pub(super) fn project_sets(agent: &mut Agent) {
    agent.sets = Sets::find(&[], None, &agent.root().join(".scramjet"));
    agent.commands = Commands::discover(&agent.sets);
}

/// The events of a reply that calls tool `name` once.
pub(super) fn tool_call(id: &str, name: &str, arguments: Value) -> Vec<u8> {
    let events = fixture::call_events(&arguments.to_string(), fixture::known_usage());
    let text = serde_json::to_string(&events).unwrap();
    let text = text
        .replace("\"inert\"", &format!("\"{name}\""))
        .replace("call_1", id);
    stream(&serde_json::from_str::<Vec<Value>>(&text).unwrap())
}

/// The events of a reply that makes each of `calls` (a tool's name and arguments), as GPT makes
/// them with `parallel_tool_calls`; their ids are `call_0`, `call_1` and so on.
pub(super) fn tool_calls(calls: &[(&str, Value)]) -> Vec<u8> {
    let first = fixture::call_events("{}", Value::Null);
    let (mut events, mut output) = (vec![first[0].clone()], Vec::new());
    for (index, (name, arguments)) in calls.iter().enumerate() {
        let call = fixture::call_events(&arguments.to_string(), Value::Null);
        let text = serde_json::to_string(&call[1..5]).unwrap();
        let text = (text.replace("\"inert\"", &format!("\"{name}\"")))
            .replace("fc_1", &format!("fc_{index}"))
            .replace("call_1", &format!("call_{index}"));
        for mut event in serde_json::from_str::<Vec<Value>>(&text).unwrap() {
            event["output_index"] = json!(index);
            if event["type"] == "response.output_item.done" {
                output.push(event["item"].clone());
            }
            events.push(event);
        }
    }
    let mut completed = first[5].clone();
    completed["response"]["output"] = json!(output);
    completed["response"]["usage"] = fixture::known_usage();
    events.push(completed);
    stream(&events)
}

/// The results a request sends, by call id.
pub(super) fn outputs(body: &Value) -> Vec<(String, String)> {
    let items = body["input"].as_array().unwrap().iter();
    let results = items.filter(|item| item["type"] == "function_call_output");
    let result = |item: &Value| {
        (
            item["call_id"].as_str().unwrap().to_owned(),
            tools::result_text(&item["output"]),
        )
    };
    results.map(result).collect()
}

pub(super) fn last_output(body: &Value) -> String {
    let input = body["input"].as_array().unwrap();
    input.last().unwrap()["output"].as_str().unwrap().to_owned()
}

#[test]
fn what_the_model_takes_comes_from_its_provider_read_again_each_time() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "provider-info");
    let file = scratch.0.join("models.json");
    let write = |document: Value| std::fs::write(&file, document.to_string()).unwrap();
    let openai = |m: Value| {
        json!({"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
        "models": [m]})
    };
    write(json!({"providers": {
        "anthropic": {"baseUrl": "http://127.0.0.1:9", "api": "anthropic-messages",
            "models": [{"id": "other", "contextWindow": 1}]},
        "openai": openai(json!({"id": "m", "name": "Model M", "contextWindow": 1_050_000,
            "maxTokens": 128_000})),
    }}));
    // The model the agent started with is the test provider's, which these files do not offer.
    agent.set_providers(Providers::read(vec![file.clone()], None));
    assert_eq!((agent.info.route.as_ref(), agent.provider()), (None, None));
    agent.set_model("m").unwrap();
    assert_eq!(agent.model(), "openai/m");
    assert_eq!((agent.info.output, agent.provider()), (Some(128_000), None));
    assert_eq!(agent.load_model_info(), Some(1_050_000));
    assert_eq!(agent.provider(), Some("openai"));
    // The provider is named only when the models are those of several.
    write(json!({"providers": {"openai": openai(json!({"id": "m"}))}}));
    assert_eq!(agent.load_model_info(), None);
    assert_eq!(agent.provider(), None);
    // A model no provider offers any longer keeps what was known of it, and where it went while
    // a program fails, and the names stay known for the Model Identity.
    write(json!({"providers": {"openai": openai(json!({"id": "m", "contextWindow": 5}))}}));
    assert_eq!(agent.load_model_info(), Some(5));
    write(json!({"providersCommand": ["./absent"]}));
    assert_eq!(agent.load_model_info(), Some(5));
    assert!(agent.info.route.is_some());
    assert!(agent.identities.contains_key("openai/m"));
    // Removed from files read without problems, its requests go nowhere.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(agent.load_model_info(), Some(5));
    assert_eq!(agent.info.route, None);
    let error = agent.turn("Hi", &mut Recorder::default(), &AtomicBool::new(false));
    assert!(
        matches!(&error, Err(AgentError::Provider(message)) if message.starts_with("no provider offers the model openai/m;")),
        "{error:?}"
    );
}

#[test]
fn providers_read_again_from_a_broken_file_keep_hiding_the_stores_it_named() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "kept-stores");
    let file = scratch.0.join("models.json");
    let write = |text: &str| std::fs::write(&file, text).unwrap();
    let named = vec![scratch.0.join("store")];
    write(r#"{"credentialFiles": ["store"]}"#);
    agent.set_providers(Providers::read(vec![file.clone()], None));
    assert_eq!(agent.providers().credential_files(), named);
    // As /reload reads them: anew.
    write(r#"{"credentialFiles": "store"}"#);
    agent.set_providers(Providers::read(vec![file.clone()], None));
    assert_eq!(agent.providers().problems().len(), 1);
    assert_eq!(agent.providers().credential_files(), named);
    write("{}");
    agent.set_providers(Providers::read(vec![file.clone()], None));
    assert_eq!(agent.providers().credential_files(), [] as [PathBuf; 0]);
}

#[test]
fn a_model_that_does_not_reason_is_asked_nothing_of_reasoning() {
    let (endpoint, server) = serve(vec![stream(&fixture::text_events(
        "plain",
        fixture::known_usage(),
    ))]);
    let (mut agent, _scratch) = agent(endpoint, "no-reasoning");
    agent.set_providers(Providers::given(json!({"providers": {"p": {
        "baseUrl": format!("http://{endpoint}/v1"), "api": "openai-responses",
        "models": [{"id": "plain"}],
    }}})));
    agent.config.effort = Some("high".into());
    agent.set_model("plain").unwrap();
    agent
        .turn("Hi", &mut Recorder::default(), &AtomicBool::new(false))
        .unwrap();
    let body = &server.join().unwrap()[0];
    assert_eq!((body.get("reasoning"), body.get("include")), (None, None));
    assert_eq!(body["input"][0]["content"], "Hi");
}

pub(super) fn write_session(path: &Path, lines: &[Value], modified: std::time::SystemTime) {
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(path, text).unwrap();
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(modified).unwrap();
}

#[test]
fn sessions_are_listed_and_continued_by_their_last_write() {
    let directory = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-list-{}", std::process::id())),
    );
    let now = std::time::SystemTime::now();
    let header = json!({"type":"session","version":1});
    let written_last = directory.0.join("1000-1.jsonl");
    write_session(
        &written_last,
        &[
            header.clone(),
            json!({"role":"user","content":"first prompt"}),
            json!({"type":"message","role":"assistant",
                "content":[{"type":"output_text","text":"the reply"}]}),
            json!({"type":"compaction","first_kept":1,"summary":"s"}),
            json!({"role":"user","content":"later"}),
            json!({"role":"user","content":"Ran `ls`\nlisting","_scramjet_typed":"!ls"}),
        ],
        now,
    );
    let created_last = directory.0.join("2000-1.jsonl");
    write_session(
        &created_last,
        &[header, json!({"role":"user","content":"second\nsession"})],
        now - Duration::from_secs(3600),
    );
    std::fs::write(directory.0.join("1500-1.jsonl"), "not a session\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(directory.0.join("1500-1.jsonl"))
        .unwrap()
        .set_modified(now - Duration::from_secs(7200))
        .unwrap();

    let sessions = Session::list(&directory.0).unwrap();
    let listed: Vec<_> = sessions
        .iter()
        .map(|s| (&s.path, s.first_prompt.as_deref(), s.item_count))
        .collect();
    assert_eq!(
        listed,
        [
            (&written_last, Some("first prompt"), 4),
            (&created_last, Some("second\nsession"), 1)
        ]
    );
    // `/resume` searches prompts and replies, compacted ones too, but not `!` commands.
    assert_eq!(sessions[0].text, "first prompt the reply later");
    assert_eq!(Session::latest(&directory.0).unwrap(), Some(written_last));
}

#[test]
fn a_session_goes_on_past_unreadable_lines_and_a_cut_last_line() {
    let directory = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-damaged-{}", std::process::id())),
    );
    let path = directory.0.join("1000-1.jsonl");
    let lines = [
        r#"{"type":"session","version":1}"#,
        r#"{"role":"user","content":"first"}"#,
        r#"{"role":"user","cont"#,
        r#"{"type":"branch","parent":7}"#,
        r#"{"role":"user","content":"second"}"#,
    ];
    std::fs::write(&path, format!("{}\n{{\"role\":\"us", lines.join("\n"))).unwrap();

    let listed = Session::list(&directory.0).unwrap();
    assert_eq!(listed[0].first_prompt.as_deref(), Some("first"));
    let (mut session, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(session.skipped(), [2, 3]);
    let prompts: Vec<_> = conversation.iter().map(|item| &item["content"]).collect();
    assert_eq!(prompts, ["first", "second"]);

    session
        .append(&json!({"role":"user","content":"third"}))
        .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.ends_with("\"second\"}\n{\"content\":\"third\",\"role\":\"user\"}\n"),
        "{text}"
    );
    let (session, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(session.skipped(), [2, 3]);
    assert_eq!(conversation.len(), 3);
    assert_eq!(session.leaf(), Some(5));

    // A whole last line without its newline, as an editor may leave one, is kept.
    std::fs::write(&path, format!("{}\n{}", lines[0], lines[1])).unwrap();
    let (mut session, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(conversation.len(), 1);
    session
        .append(&json!({"role":"user","content":"next"}))
        .unwrap();
    let (_, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(conversation.len(), 2);
}

#[test]
fn a_session_whose_file_could_not_be_created_creates_it_once_there_is_room() {
    let directory = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-uncreated-{}", std::process::id())),
    );
    let mut session = Session::create(&directory.0, Path::new("/project"), None).unwrap();
    let path = directory.0.join("later/1-1.jsonl");
    session.not_created_at(&path);
    assert!(session.holding());
    assert!(!session.is_file(&path));
    session.set_name("kept").unwrap();
    assert_eq!(session.branch().unwrap().entries.len(), 0);
    session
        .append(&json!({"role":"user","content":"first"}))
        .unwrap();
    assert!(!session.holding());
    assert!(session.is_file(&path));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory.0.join("later"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let (reopened, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(conversation, [json!({"role":"user","content":"first"})]);
    assert_eq!(reopened.name(), Some("kept"));

    // A name another user could have made in the shared temporary directory is not written into.
    #[cfg(unix)]
    {
        let elsewhere = directory.0.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, directory.0.join("planted")).unwrap();
        let mut session = Session::create(&directory.0, Path::new("/project"), None).unwrap();
        session.not_created_at(&directory.0.join("planted/1-1.jsonl"));
        let error = (session.append(&json!({"role":"user","content":"x"}))).unwrap_err();
        assert!(
            error.to_string().contains("not a directory of this user's"),
            "{error}"
        );
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }
}

#[test]
#[cfg_attr(windows, ignore = "writes to /dev/full, which Windows lacks")]
fn a_copy_on_a_full_disk_holds_its_lines_until_there_is_room() {
    let directory = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-full-copy-{}", std::process::id())),
    );
    let mut copy = Session::create(&directory.0, Path::new("/project"), None).unwrap();
    let path = copy.path().to_owned();
    copy.write_to(
        std::fs::OpenOptions::new()
            .append(true)
            .open("/dev/full")
            .unwrap(),
    );
    let lines = concat!(
        "{\"role\":\"user\",\"content\":\"one\"}\n",
        "{\"type\":\"session_info\",\"name\":\"Work\"}\n",
        "{\"role\":\"user\",\"content\":\"two\"}\n",
        "{\"role\":\"user\",\"cont",
    );
    let (mut copy, conversation, prompts) = copy.filled(lines.as_bytes()).unwrap();
    assert!(copy.holding());
    assert!(matches!(copy.take_change(), Some(session::Saving::Held(_))));
    assert_eq!(prompts, ["one", "two"]);
    assert_eq!(conversation.len(), 2);
    assert_eq!(copy.name(), Some("Work"));
    assert_eq!(copy.leaf(), Some(3));
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);

    copy.write_to(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
    );
    copy.append(&json!({"role":"user","content":"three"}))
        .unwrap();
    assert_eq!(copy.take_change(), Some(session::Saving::Saved));
    let (reopened, conversation, _) = Session::open(&path).unwrap();
    let prompts: Vec<_> = conversation.iter().map(|item| &item["content"]).collect();
    assert_eq!(prompts, ["one", "two", "three"]);
    assert_eq!(reopened.name(), Some("Work"));
    assert!(reopened.skipped().is_empty());
}

#[test]
#[cfg_attr(windows, ignore = "writes to /dev/full, which Windows lacks")]
fn a_session_on_a_full_disk_keeps_its_lines_until_there_is_room() {
    let directory = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-full-{}", std::process::id())),
    );
    let mut session = Session::create(&directory.0, Path::new("/project"), None).unwrap();
    let path = session.path().to_owned();
    let say = |text: &str| json!({"role":"user","content":text});
    session.append(&say("first")).unwrap();
    assert_eq!(session.take_change(), None);

    let full = || {
        std::fs::OpenOptions::new()
            .append(true)
            .open("/dev/full")
            .unwrap()
    };
    session.write_to(full());
    session.append(&say("second")).unwrap();
    let held = session.take_change();
    assert!(
        matches!(&held, Some(session::Saving::Held(error)) if error.contains("No space left")),
        "{held:?}"
    );
    session.set_name("kept").unwrap();
    session.append(&say("third")).unwrap();
    assert_eq!(session.take_change(), None);
    assert!(session.holding());
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk.lines().count(), 2, "{on_disk}");
    // What it reads of itself has the held lines.
    let prompts: Vec<_> = (session.branch().unwrap().entries.iter())
        .map(|entry| entry["content"].clone())
        .collect();
    assert_eq!(prompts, ["first", "second", "third"]);
    assert_eq!(session.leaf(), Some(4));

    session.write_to(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
    );
    session.write_held().unwrap();
    assert_eq!(session.take_change(), Some(session::Saving::Saved));
    assert!(!session.holding());
    session.write_held().unwrap();
    session.append(&say("fourth")).unwrap();
    assert_eq!(session.take_change(), None);
    assert!(!session.holding());
    let (reopened, conversation, _) = Session::open(&path).unwrap();
    let prompts: Vec<_> = conversation.iter().map(|item| &item["content"]).collect();
    assert_eq!(prompts, ["first", "second", "third", "fourth"]);
    assert_eq!(reopened.name(), Some("kept"));
    assert_eq!(reopened.leaf(), Some(5));
    assert!(reopened.skipped().is_empty());
    drop(reopened);

    // Lines still held when it ends are tried once more.
    session.write_to(full());
    session.append(&say("fifth")).unwrap();
    assert!(matches!(
        session.take_change(),
        Some(session::Saving::Held(_))
    ));
    // A try on a disk still full keeps them, saying nothing new.
    session.write_held().unwrap();
    assert_eq!(session.take_change(), None);
    assert!(session.holding());
    session.write_to(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
    );
    drop(session);
    assert_eq!(Session::open(&path).unwrap().1.len(), 5);
}

#[test]
fn a_session_is_found_by_the_start_of_its_id_here_before_elsewhere() {
    let sessions = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-find-{}", std::process::id())),
    );
    let [here, other, third] = ["here", "other", "third"].map(|name| sessions.0.join(name));
    for directory in [&here, &other, &third] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let now = std::time::SystemTime::now();
    let ago = |minutes: u64| now - Duration::from_secs(60 * minutes);
    let save = |path: &Path, root: Option<&str>, modified| {
        let mut header = json!({"type":"session","version":1});
        if let Some(root) = root {
            header["root"] = json!(root);
        }
        write_session(
            path,
            &[header, json!({"role":"user","content":"hello"})],
            modified,
        );
    };
    save(&here.join("1000-1.jsonl"), Some("/work/here"), ago(30));
    save(&here.join("1001-1.jsonl"), Some("/work/here"), ago(20));
    // Without a conversation (another process's, just started), or not a session at all.
    write_session(
        &here.join("1002-1.jsonl"),
        &[json!({"type":"session","version":1})],
        ago(1),
    );
    std::fs::write(here.join("1003-1.jsonl"), "not a session\n").unwrap();
    save(&other.join("2000-5.jsonl"), Some("/work/other"), ago(30));
    save(&third.join("2000-6.jsonl"), None, ago(10));
    // Newer elsewhere, but one of the project's matches.
    save(&here.join("3000-1.jsonl"), Some("/work/here"), ago(40));
    save(&other.join("3000-2.jsonl"), Some("/work/other"), ago(5));
    std::fs::write(sessions.0.join("2000-7.jsonl"), "").unwrap();

    let find = |prefix| Session::find(&here, &sessions.0, prefix).unwrap();
    assert_eq!(find("100"), Some(Found::Here(here.join("1001-1.jsonl"))));
    assert_eq!(find("1000"), Some(Found::Here(here.join("1000-1.jsonl"))));
    assert_eq!(find("1000-1"), Some(Found::Here(here.join("1000-1.jsonl"))));
    assert_eq!(find("1002"), None);
    assert_eq!(find("3"), Some(Found::Here(here.join("3000-1.jsonl"))));
    // The newest of the other projects'; a header without the project names its directory.
    assert_eq!(
        find("2000"),
        Some(Found::Elsewhere {
            path: third.join("2000-6.jsonl"),
            root: third.clone()
        })
    );
    assert_eq!(
        find("2000-5"),
        Some(Found::Elsewhere {
            path: other.join("2000-5.jsonl"),
            root: PathBuf::from("/work/other")
        })
    );
    assert_eq!(find("9"), None);
    // A project without sessions yet.
    let new = sessions.0.join("new");
    assert_eq!(
        Session::find(&new, &sessions.0, "3000").unwrap(),
        Some(Found::Elsewhere {
            path: other.join("3000-2.jsonl"),
            root: PathBuf::from("/work/other")
        })
    );
}

#[test]
fn a_fork_from_a_session_file_holds_all_of_it_in_a_new_session_of_the_project() {
    let scratch = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-fork-from-{}", std::process::id())),
    );
    let reply = |text: &str| {
        json!({"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}],"_scramjet_model":"a"})
    };
    let source = crate::os::join(&scratch.0, "other/1700000000000-1.jsonl");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    write_session(
        &source,
        &[
            json!({"type":"session","version":1,"root":"/work/other","parentSession":"/old.jsonl"}),
            json!({"role":"user","content":"one"}),
            reply("first"),
            json!({"type":"session_info","name":"Work"}),
            json!({"role":"user","content":"two"}),
            reply("second"),
            json!({"type":"branch","parent":2}),
            json!({"role":"user","content":"other"}),
            reply("another"),
            json!({"type":"label","target":4,"label":"kept","time":1}),
        ],
        std::time::SystemTime::now(),
    );
    let saved = std::fs::read_to_string(&source).unwrap();
    let here = scratch.0.join("here");
    let root = Path::new("/work/here");

    let (session, conversation, prompts) = Session::fork_from(&here, root, &source).unwrap();
    let forked = session.path().to_owned();
    assert_eq!(forked.parent(), Some(here.as_path()));
    let text = std::fs::read_to_string(&forked).unwrap();
    let (header, lines) = text.split_once('\n').unwrap();
    let header: Value = serde_json::from_str(header).unwrap();
    assert_eq!(header["root"], "/work/here");
    assert_eq!(header["parentSession"], json!(source));
    assert_eq!(lines, saved.split_once('\n').unwrap().1);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), saved);

    let reopened = Session::open(&forked).unwrap();
    assert_eq!((&reopened.1, &reopened.2), (&conversation, &prompts));
    assert_eq!(reopened.0.leaf(), session.leaf());
    assert_eq!(session.name(), Some("Work"));
    // Up/Down recalls the prompts of every branch.
    assert_eq!(prompts, ["one", "two", "other"]);
    let texts: Vec<_> = (conversation.iter())
        .map(|item| match &item["content"] {
            Value::String(text) => text.as_str(),
            parts => parts[0]["text"].as_str().unwrap(),
        })
        .collect();
    assert_eq!(texts, ["one", "first", "other", "another"]);
    let (entries, labels) = session.labeled_entries().unwrap();
    assert_eq!(entries.len(), 6);
    assert_eq!(labels[&4].text, "kept");
    assert_eq!(
        Session::list(&here).unwrap()[0].parent.as_ref(),
        Some(&source)
    );

    // What is not a session file is not copied.
    let notes = scratch.0.join("notes.jsonl");
    std::fs::write(&notes, "{\"type\":\"note\"}\n").unwrap();
    let error = Session::fork_from(&here, root, &notes)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("not a scramjet-rs session"), "{error}");
    let missing = scratch.0.join("missing.jsonl");
    let error = Session::fork_from(&here, root, &missing)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("missing.jsonl"), "{error}");
    assert_eq!(std::fs::read_dir(&here).unwrap().count(), 1);
}

#[test]
fn every_projects_sessions_are_listed_and_another_projects_is_forked_here() {
    let scratch = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-all-{}", std::process::id())),
    );
    let (here, other) = (scratch.0.join("here"), scratch.0.join("other"));
    std::fs::create_dir_all(&here).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let now = std::time::SystemTime::now();
    let ago = |minutes: u64| now - Duration::from_secs(60 * minutes);
    let save = |path: &Path, root: &str, prompt: &str, modified| {
        let header = json!({"type":"session","version":1,"root":root});
        write_session(
            path,
            &[header, json!({"role":"user","content":prompt})],
            modified,
        );
    };
    save(&here.join("1000-1.jsonl"), "/work/here", "mine", ago(30));
    save(
        &other.join("1000-2.jsonl"),
        "/work/other",
        "theirs",
        ago(20),
    );
    save(&other.join("1000-3.jsonl"), "/work/other", "older", ago(40));
    // Not a project's directory, and a file beside them.
    std::fs::write(scratch.0.join("1000-4.jsonl"), "").unwrap();
    let current = here.join("1001-1.jsonl");
    std::fs::write(&current, "{\"type\":\"session\",\"version\":1}\n").unwrap();
    let endpoint = "127.0.0.1:9".parse().unwrap();
    let mut agent =
        Agent::start(config(endpoint, &scratch.0), SessionChoice::Open(current)).unwrap();

    let prompts = |sessions: Vec<SessionInfo>| -> Vec<(String, Option<PathBuf>)> {
        (sessions.into_iter())
            .map(|session| (session.first_prompt.unwrap(), session.root))
            .collect()
    };
    let root = |root: &str| Some(PathBuf::from(root));
    assert_eq!(
        prompts(agent.sessions().unwrap()),
        [("mine".to_owned(), root("/work/here"))]
    );
    assert_eq!(
        prompts(agent.all_sessions().unwrap()),
        [
            ("theirs".to_owned(), root("/work/other")),
            ("mine".to_owned(), root("/work/here")),
            ("older".to_owned(), root("/work/other")),
        ]
    );
    assert!(agent.session_here(&here.join("1000-1.jsonl")));
    let theirs = other.join("1000-2.jsonl");
    assert!(!agent.session_here(&theirs));

    // Theirs continues in a new session of this project, and stays as it was.
    let before = std::fs::read(&theirs).unwrap();
    agent.fork_session_file(&theirs).unwrap();
    assert_eq!(agent.session_path().parent(), Some(here.as_path()));
    assert_eq!(std::fs::read(&theirs).unwrap(), before);
    let forked = agent.sessions().unwrap();
    let fork = (forked.iter())
        .find(|session| agent.is_session(&session.path))
        .unwrap();
    assert_eq!(fork.first_prompt.as_deref(), Some("theirs"));
    assert_eq!(fork.parent.as_deref(), Some(theirs.as_path()));
    assert_eq!(fork.root.as_deref(), Some(scratch.0.as_path()));
}

#[test]
fn another_projects_session_resumed_in_its_folder_moves_the_agent_there() {
    let scratch = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-moved-{}", std::process::id())),
    );
    let (here, there) = (scratch.0.join("here"), scratch.0.join("there"));
    let sessions = scratch.0.join("sessions");
    for directory in [&here, &there, &sessions] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(there.join("AGENTS.md"), "Their rules.\n").unwrap();
    let theirs = sessions.join("1000-2.jsonl");
    let header = json!({"type":"session","version":1,"root":there});
    let prompt = json!({"role":"user","content":"theirs"});
    write_session(&theirs, &[header, prompt], std::time::SystemTime::now());
    let current = here.join("1001-1.jsonl");
    std::fs::write(&current, "{\"type\":\"session\",\"version\":1}\n").unwrap();
    let endpoint = "127.0.0.1:9".parse().unwrap();
    let mut agent = Agent::start(config(endpoint, &here), SessionChoice::Open(current)).unwrap();
    assert!(agent.installed().context_files.is_empty());

    agent.resume_session_in(&theirs, &there).unwrap();
    assert!(agent.is_session(&theirs));
    assert_eq!(agent.root(), there.as_path());
    assert_eq!(agent.installed().context_files, [there.join("AGENTS.md")]);
    // A new session is one of that project's, next to the one resumed.
    agent.new_session().unwrap();
    assert_eq!(agent.session_path().parent(), Some(sessions.as_path()));
}

#[test]
fn a_session_name_and_stats_stay_out_of_the_conversation() {
    let (endpoint, server) = serve(vec![
        stream(&fixture::call_events("{}", fixture::known_usage())),
        stream(&fixture::text_events("A", fixture::known_usage())),
        stream(&fixture::text_events("B", fixture::known_usage())),
        stream(&fixture::text_events("C", fixture::known_usage())),
    ]);
    let (mut agent, scratch) = agent(endpoint, "name");
    let mut ui = Recorder::default();
    let cancel = AtomicBool::new(false);
    agent.turn("one", &mut ui, &cancel).unwrap();
    agent.turn("two", &mut ui, &cancel).unwrap();
    assert_eq!(agent.session_name(), None);

    // Named while a move waits for the next entry, the branch still starts at the reply.
    agent.navigate(4, None, &mut ui, &cancel).unwrap();
    agent.set_session_name("  Parser work ").unwrap();
    assert_eq!(agent.session_name(), Some("Parser work"));
    agent
        .add(json!({"role":"user","content":"Ran `ls`","_scramjet_typed":"!ls"}))
        .unwrap();
    agent.turn("three", &mut ui, &cancel).unwrap();
    let text = std::fs::read_to_string(agent.session_path()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[7], r#"{"name":"Parser work","type":"session_info"}"#);
    assert_eq!(lines[8], r#"{"parent":4,"type":"branch"}"#);
    let bodies = server.join().unwrap();
    assert!(!bodies[3].to_string().contains("\"_scramjet_"));

    // Counted over the current branch (one, call, result, A, !ls, three, C), as saved.
    let stats = Stats {
        user: 2,
        assistant: 3,
        tool_calls: 1,
        tool_results: 1,
        total: 7,
        tokens: Tokens {
            input: 21,
            output: 15,
            cache_read: 9,
            // The model has no card, so no price.
            unpriced: true,
            ..Tokens::default()
        },
    };
    assert_eq!(agent.stats(), stats);
    assert_eq!(agent.session_id(), "session");
    let path = agent.session_path().to_owned();
    let config = AgentConfig {
        providers: serving(endpoint),
        model: "m".into(),
        root: scratch.0.clone(),
        effort: None,
        max_output_tokens: None,
        ask: false,
        retries: Default::default(),
        compacting: Default::default(),
        tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
        only_tools: None,
        command_sets: Vec::new(),
        skills: Vec::new(),
        skill_options: Default::default(),
        prompt: Default::default(),
        mcp_servers: Vec::new(),
        mcp_disabled: Vec::new(),
        sandbox: None,
        session_dir: None,
    };
    let resumed = Agent::start(config, SessionChoice::Open(path.clone())).unwrap();
    assert_eq!(resumed.session_name(), Some("Parser work"));
    assert_eq!(resumed.stats(), stats);
    let listed = Session::list(&scratch.0).unwrap();
    assert_eq!(listed[0].name.as_deref(), Some("Parser work"));
    assert_eq!(listed[0].first_prompt.as_deref(), Some("one"));

    // The latest name wins; an empty one clears it.
    agent.set_session_name("").unwrap();
    assert_eq!(agent.session_name(), None);
    let (session, _, _) = Session::open(&path).unwrap();
    assert_eq!(session.name(), None);
    assert_eq!(Session::list(&scratch.0).unwrap()[0].name, None);
}

#[test]
fn resume_lists_sessions_with_a_conversation_and_switches_to_one() {
    let (endpoint, _server) = serve(vec![]);
    let (mut agent, scratch) = agent(endpoint, "resume-session");
    agent.usage.requests = 7;
    let current = agent.session_path().to_owned();
    let later = std::time::SystemTime::now() + Duration::from_secs(60);
    let header = json!({"type":"session","version":1});
    write_session(
        &scratch.0.join("empty.jsonl"),
        &[
            header.clone(),
            json!({"type":"model_change","model":"other"}),
        ],
        later,
    );
    let path = scratch.0.join("resumed.jsonl");
    write_session(
        &path,
        &[
            header,
            json!({"type":"model_change","model":"restored"}),
            json!({"role":"user","content":"Ran `ls`","_scramjet_typed":"!ls"}),
            json!({"type":"function_call","call_id":"unfinished","name":"bash","arguments":"{}"}),
        ],
        later,
    );
    let listed: Vec<_> = agent
        .sessions()
        .unwrap()
        .into_iter()
        .map(|s| s.path)
        .collect();
    // The current session has no conversation yet, so it is not listed either.
    assert_eq!(listed, std::slice::from_ref(&path));

    assert!(agent.is_session(&scratch.0.join(".").join("session.jsonl")));
    assert!(
        agent
            .resume_session(&scratch.0.join(".").join("session.jsonl"))
            .is_err()
    );
    assert!(
        agent
            .resume_session(&scratch.0.join("missing.jsonl"))
            .is_err()
    );
    assert_eq!(agent.session_path(), current);
    assert_eq!(agent.usage.requests, 7);

    agent.resume_session(&path).unwrap();
    assert_eq!(agent.session_path(), path);
    assert_eq!(agent.model(), "restored");
    assert_eq!(agent.prompts(), ["!ls"]);
    assert_eq!(agent.usage.requests, 0);
    assert_eq!(agent.conversation().len(), 3);
    assert_eq!(agent.conversation()[2]["type"], "function_call_output");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"call_id\":\"unfinished\",\"output\"")
    );
}

#[test]
fn a_new_session_is_written_at_its_first_prompt_with_what_came_before() {
    let (mut agent, _scratch) = agent("127.0.0.1:9".parse().unwrap(), "written-at-prompt");
    agent.new_session().unwrap();
    let path = agent.session_path().to_owned();
    agent.set_model("other").unwrap();
    agent.set_effort("high").unwrap();
    agent.set_session_name("Named").unwrap();
    assert!(!path.exists());
    assert!(agent.sessions().unwrap().is_empty());
    assert!(!agent.session_holding());
    assert_eq!(agent.write_held(), None);
    assert!(!path.exists());

    agent.add(json!({"role":"user","content":"one"})).unwrap();
    let lines: Vec<Value> = (std::fs::read_to_string(&path).unwrap().lines())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let kinds: Vec<_> = (lines.iter())
        .map(|line| line["type"].as_str().or(line["role"].as_str()).unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "session",
            "model_change",
            "model_change",
            "thinking_level_change",
            "session_info",
            "user"
        ]
    );
    assert_eq!(agent.saving_notice(), None);
    let (session, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(
        (session.model(), session.effort(), session.name()),
        (Some("test/other"), Some("high"), Some("Named"))
    );
    assert_eq!(conversation.len(), 1);
}

#[test]
#[cfg_attr(windows, ignore = "runs `sh` scripts as programs")]
fn sessions_are_renamed_and_deleted_and_unused_new_ones_are_not_left_behind() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "manage-sessions");
    let current = agent.session_path().to_owned();
    let header = json!({"type":"session","version":1});
    let earlier = std::time::SystemTime::now() - Duration::from_secs(3600);
    let other = scratch.0.join("other.jsonl");
    let prompt = json!({"role":"user","content":"hello"});
    write_session(&other, &[header.clone(), prompt.clone()], earlier);

    // Another session's name goes in its file, which keeps its time; the current one's in its own.
    agent.rename_session(&other, "  Old work ").unwrap();
    assert_eq!(
        agent.sessions().unwrap()[0].name.as_deref(),
        Some("Old work")
    );
    assert_eq!(
        std::fs::metadata(&other).unwrap().modified().unwrap(),
        earlier
    );
    let current_too = scratch.0.join(".").join("session.jsonl");
    agent.rename_session(&current_too, "Mine").unwrap();
    assert_eq!(agent.session_name(), Some("Mine"));
    assert!(
        agent
            .rename_session(&scratch.0.join("missing.jsonl"), "x")
            .is_err()
    );

    // Neither the current session nor a file that is not a session is deleted.
    assert!(agent.delete_session(&current_too).is_err());
    assert!(current.exists());
    let notes = scratch.0.join("notes.jsonl");
    std::fs::write(&notes, "not a session\n").unwrap();
    assert!(Session::delete_with(&notes, "true").is_err());
    assert!(notes.exists());

    // `trash` moves a session away; without it, or when it fails, the file is removed.
    let script = |name: &str, text: &str| {
        let path = scratch.0.join(name);
        script(&path, text);
        path.to_str().unwrap().to_owned()
    };
    let trash = script("trash", "mv \"$1\" \"$1.trashed\"");
    assert_eq!(
        Session::delete_with(&other, &trash).unwrap(),
        Removal::Trashed
    );
    assert!(!other.exists() && scratch.0.join("other.jsonl.trashed").exists());
    let session = [header.clone(), prompt.clone()];
    write_session(&other, &session, earlier);
    let missing = "scramjet-rs-no-such-trash";
    assert_eq!(
        Session::delete_with(&other, missing).unwrap(),
        Removal::Deleted
    );
    assert!(!other.exists());
    let failing = script("failing", "echo 'no trash here' >&2\necho more >&2\nexit 1");
    write_session(&other, &session, earlier);
    assert_eq!(
        Session::delete_with(&other, &failing).unwrap(),
        Removal::Deleted
    );
    // A removal that fails says what the trash said.
    #[cfg(unix)]
    {
        let locked = ScratchDir::new(scratch.0.join("locked"));
        let kept = locked.0.join("kept.jsonl");
        write_session(&kept, &session, earlier);
        let mode = |mode| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&locked.0, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        mode(0o555);
        let error = Session::delete_with(&kept, &failing)
            .unwrap_err()
            .to_string();
        mode(0o755);
        assert!(error.ends_with("(trash: no trash here)"), "{error}");
        assert!(kept.exists());
    }

    // A session started here goes when the agent moves on before it holds a conversation, as
    // Scramjet would not have written it; model and effort changes do not count.
    agent.new_session().unwrap();
    let unused = agent.session_path().to_owned();
    agent.set_model("other").unwrap();
    agent.set_effort("high").unwrap();
    assert!(!unused.exists());
    agent.new_session().unwrap();
    assert!(!unused.exists());
    let used = agent.session_path().to_owned();
    agent
        .add(json!({"role":"user","content":"Ran `ls`","_scramjet_typed":"!ls"}))
        .unwrap();
    agent.resume_session(&current).unwrap();
    assert!(used.exists());
    // The one opened stays; one ended before a conversation leaves nothing.
    agent.new_session().unwrap();
    assert!(current.exists());
    let last = agent.session_path().to_owned();
    drop(agent);
    assert!(!last.exists());
}

#[test]
fn a_run_that_saves_no_session_keeps_its_sessions_apart_until_it_ends() {
    let (endpoint, server) = serve(vec![
        stream(&fixture::text_events("A", fixture::known_usage())),
        stream(&fixture::text_events("B", fixture::known_usage())),
    ]);
    let (saved, scratch) = agent(endpoint, "unsaved");
    let mut agent = Agent::start(config(endpoint, &scratch.0), SessionChoice::Unsaved).unwrap();
    let directory = agent.session_path().parent().unwrap().to_owned();
    let name = directory
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(directory.parent(), Some(std::env::temp_dir().as_path()));
    assert!(name.starts_with("scramjet-unsaved-"), "{name}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&directory).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    assert!(saved.session_saved() && !agent.session_saved());
    // As Scramjet's instructions name no journal for a session in memory.
    assert!(saved.instructions().contains("\nCurrent session journal: "));
    assert!(!agent.instructions().contains("\nCurrent session journal: "));

    let mut ui = Recorder::default();
    let cancel = AtomicBool::new(false);
    agent.turn("one", &mut ui, &cancel).unwrap();
    let first = agent.session_path().to_owned();
    // Those made from it are kept with it, and `/resume` lists only them.
    agent.new_session().unwrap();
    assert_eq!(agent.session_path().parent(), Some(directory.as_path()));
    agent.turn("two", &mut ui, &cancel).unwrap();
    assert!(agent.fork(0).unwrap().is_some());
    assert_eq!(agent.session_path().parent(), Some(directory.as_path()));
    let listed: Vec<_> = (agent.sessions().unwrap().into_iter())
        .map(|session| session.first_prompt)
        .collect();
    assert_eq!(listed, [Some("two".to_owned()), Some("one".to_owned())]);
    agent.resume_session(&first).unwrap();
    server.join().unwrap();

    drop(agent);
    assert!(!directory.exists());
    // An agent that ends otherwise takes its directory with it too.
    let other = Agent::start(config(endpoint, &scratch.0), SessionChoice::Unsaved).unwrap();
    let directory = other.session_path().parent().unwrap().to_owned();
    assert!(directory.exists());
    drop(other);
    assert!(!directory.exists());
}

#[test]
fn lines_another_process_appends_keep_entries_and_branches_right() {
    let scratch = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-shared-{}", std::process::id())),
    );
    let path = scratch.0.join("shared.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"version\":1}\n").unwrap();
    let (mut session, _, _) = Session::open(&path).unwrap();
    session
        .append(&json!({"role":"user","content":"one"}))
        .unwrap();
    // `/resume` in another process names it.
    Session::name_file(&path, "Elsewhere").unwrap();
    session
        .append(&json!({"role":"user","content":"two"}))
        .unwrap();
    assert_eq!(
        (session.leaf(), session.name()),
        (Some(3), Some("Elsewhere"))
    );
    let left = session.left_behind(Some(1)).unwrap();
    assert_eq!(left, [json!({"role":"user","content":"two"})]);
    session.move_to(Some(1)).unwrap();
    session
        .append(&json!({"role":"user","content":"again"}))
        .unwrap();
    let (reopened, items, _) = Session::open(&path).unwrap();
    let prompts: Vec<_> = items.iter().map(|item| item["content"].clone()).collect();
    assert_eq!(prompts, ["one", "again"]);
    assert_eq!(reopened.leaf(), session.leaf());
}

#[test]
fn names_another_process_gives_while_entries_go_in_keep_the_file_readable() {
    let scratch = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-renamed-{}", std::process::id())),
    );
    let path = scratch.0.join("shared.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"version\":1}\n").unwrap();
    let (mut session, _, _) = Session::open(&path).unwrap();
    let appending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let renamer = {
        let (path, appending) = (path.clone(), appending.clone());
        std::thread::spawn(move || {
            let mut renamed = 0;
            while renamed == 0 || appending.load(std::sync::atomic::Ordering::Relaxed) {
                Session::name_file(&path, &format!("Name {renamed}")).unwrap();
                renamed += 1;
                std::thread::sleep(Duration::from_millis(1));
            }
            renamed
        })
    };
    for n in 0..2000 {
        session
            .append(&json!({"role":"user","content":n.to_string()}))
            .unwrap();
    }
    appending.store(false, std::sync::atomic::Ordering::Relaxed);
    let last = format!("Name {}", renamer.join().unwrap() - 1);
    let (reopened, items, _) = Session::open(&path).unwrap();
    assert_eq!((items.len(), reopened.name()), (2000, Some(last.as_str())));
    assert_eq!(reopened.leaf(), session.leaf());
}

#[test]
#[cfg_attr(windows, ignore = "writes to /dev/full, which Windows lacks")]
fn a_session_left_while_holding_lines_writes_them_once_there_is_room() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "left-held");
    let original = scratch.0.join("1700000000000-1.jsonl");
    write_session(
        &original,
        &[
            json!({"type":"session","version":1}),
            json!({"role":"user","content":"one"}),
        ],
        std::time::SystemTime::now(),
    );
    agent.resume_session(&original).unwrap();
    let full = std::fs::OpenOptions::new().append(true).open("/dev/full");
    agent.session.write_to(full.unwrap());
    let said = json!({"role":"user","content":"two"});
    agent.session.append(&said).unwrap();
    assert!(agent.saving_notice().is_some());
    let held = agent.held_len();
    assert!(held > said.to_string().len(), "{held}");

    assert!(agent.clone_session().unwrap());
    assert_ne!(agent.session_path(), original);
    assert_eq!(agent.conversation.len(), 2);
    assert!(agent.session_holding());
    // Still full: nothing more to say.
    assert_eq!(agent.write_held(), None);
    assert!(agent.session_holding());

    let room = std::fs::OpenOptions::new().append(true).open(&original);
    agent.left[0].write_to(room.unwrap());
    let notice = agent.write_held().unwrap();
    assert!(notice.contains("1700000000000-1.jsonl written"), "{notice}");
    assert!(!agent.session_holding());
    assert_eq!(agent.held_len(), 0);
    let (_, conversation, _) = Session::open(&original).unwrap();
    assert_eq!(conversation, [json!({"role":"user","content":"one"}), said]);
}

#[test]
#[cfg_attr(windows, ignore = "writes to /dev/full, which Windows lacks")]
fn a_session_left_while_holding_lines_resumes_with_them() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "resume-held");
    let original = scratch.0.join("1700000000000-1.jsonl");
    write_session(
        &original,
        &[
            json!({"type":"session","version":1}),
            json!({"role":"user","content":"one"}),
        ],
        std::time::SystemTime::now(),
    );
    agent.resume_session(&original).unwrap();
    let full = std::fs::OpenOptions::new().append(true).open("/dev/full");
    agent.session.write_to(full.unwrap());
    let two = json!({"role":"user","content":"two"});
    agent.session.append(&two).unwrap();
    assert!(agent.clone_session().unwrap());
    agent.new_session().unwrap();
    assert_eq!(agent.left.len(), 1);

    agent.resume_session(&original).unwrap();
    assert!(agent.left.is_empty());
    assert_eq!(agent.session_path(), original);
    assert_eq!(
        agent.conversation,
        [json!({"role":"user","content":"one"}), two.clone()]
    );
    assert_eq!(agent.prompts, ["one", "two"]);
    let three = json!({"role":"user","content":"three"});
    agent.session.append(&three).unwrap();

    let room = std::fs::OpenOptions::new().append(true).open(&original);
    agent.session.write_to(room.unwrap());
    agent.write_held();
    assert!(!agent.session_holding());
    let (_, conversation, _) = Session::open(&original).unwrap();
    assert_eq!(
        conversation,
        [json!({"role":"user","content":"one"}), two, three]
    );
}

#[test]
fn a_fork_continues_from_before_a_prompt_in_a_new_session() {
    let (endpoint, server) = serve(vec![stream(&fixture::text_events(
        "again",
        fixture::known_usage(),
    ))]);
    let (mut agent, scratch) = agent(endpoint, "fork");
    let reply = |text: &str, model: &str| {
        json!({"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}],"_scramjet_model":model})
    };
    let mut answered = reply("first", "a");
    answered["_scramjet_context_tokens"] = json!(100);
    let original = scratch.0.join("1700000000000-1.jsonl");
    write_session(
        &original,
        &[
            json!({"type":"session","version":1}),
            json!({"type":"model_change","model":"a"}),
            json!({"role":"user","content":"one"}),
            answered,
            json!({"role":"user","content":"Ran `ls`","_scramjet_typed":"!ls"}),
            json!({"role":"user","content":"two"}),
            reply("second", "a"),
            json!({"type":"model_change","model":"b"}),
            json!({"type":"compaction","first_kept":2,"summary":"s"}),
            json!({"role":"user","content":"three"}),
            reply("third", "b"),
        ],
        std::time::SystemTime::now(),
    );
    let saved = std::fs::read_to_string(&original).unwrap();
    agent.resume_session(&original).unwrap();
    assert_eq!(agent.fork_prompts().unwrap(), ["one", "two", "three"]);
    assert_eq!(agent.fork(3).unwrap(), None);
    assert_eq!(agent.session_path(), original);

    assert_eq!(agent.fork(1).unwrap(), Some(("two".into(), None)));
    let forked = agent.session_path().to_owned();
    assert_ne!(forked, original);
    assert_eq!(forked.parent(), original.parent());
    assert_eq!(std::fs::read_to_string(&original).unwrap(), saved);
    let lines: Vec<String> = std::fs::read_to_string(&forked)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines[1..], saved.lines().collect::<Vec<_>>()[1..5]);
    let header: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(header["parentSession"], json!(original));
    let listed = agent.sessions().unwrap();
    let fork = listed
        .iter()
        .find(|session| session.path == forked)
        .unwrap();
    assert_eq!(fork.parent.as_deref(), Some(original.as_path()));
    assert_eq!(agent.model(), "test/a");
    assert_eq!(agent.prompts(), ["one", "!ls"]);
    assert_eq!(agent.usage.context_tokens, Some(100));
    let mut ui = Recorder::default();
    agent
        .turn("two again", &mut ui, &AtomicBool::new(false))
        .unwrap();
    let bodies = server.join().unwrap();
    assert_eq!(bodies[0]["model"], "a");
    let input = bodies[0]["input"].as_array().unwrap();
    assert_eq!(input.len(), 4, "{input:?}");
    assert_eq!(input[1]["content"][0]["text"], "first");
    assert_eq!(input[3], json!({"role":"user","content":"two again"}));
    assert!(!bodies[0].to_string().contains("\"_scramjet_"));

    // A compaction before the prompt is copied, and later model changes are not.
    agent.resume_session(&original).unwrap();
    assert_eq!(agent.fork(2).unwrap(), Some(("three".into(), None)));
    assert_eq!(agent.model(), "test/b");
    let conversation = agent.conversation();
    assert_eq!(conversation.len(), 4);
    assert_eq!(compaction::previous_summary(conversation), Some("s"));
    assert_eq!(conversation[2]["content"], "two");
    assert_eq!(agent.usage.context_tokens, None);

    // Before the first prompt the conversation is empty, and the current model stays.
    agent.resume_session(&original).unwrap();
    agent.set_model("c").unwrap();
    assert_eq!(agent.fork(0).unwrap(), Some(("one".into(), None)));
    assert!(agent.conversation().is_empty());
    assert_eq!(agent.model(), "test/c");
    let empty = agent.session_path().to_owned();
    // As Scramjet writes no fork without a reply, it is not written before a prompt, so neither
    // leaving it nor quitting leaves it, and `--continue` takes the conversation before it.
    assert!(!empty.exists());
    agent.resume_session(&original).unwrap();
    assert!(!empty.exists());
    let saved = std::fs::read_to_string(&original).unwrap();
    agent.fork(0).unwrap();
    let empty = agent.session_path().to_owned();
    drop(agent);
    assert!(!empty.exists());
    assert_eq!(std::fs::read_to_string(&original).unwrap(), saved);
}

#[test]
fn a_clone_continues_the_conversation_where_it_is_in_a_new_session() {
    let (endpoint, server) = serve(vec![stream(&fixture::text_events(
        "cloned",
        fixture::known_usage(),
    ))]);
    let (mut agent, scratch) = agent(endpoint, "clone");
    let fresh = agent.session_path().to_owned();
    assert!(!agent.clone_session().unwrap());
    assert_eq!(agent.session_path(), fresh);

    let reply = |text: &str, model: &str| {
        json!({"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}],"_scramjet_model":model})
    };
    let original = scratch.0.join("1700000000000-1.jsonl");
    write_session(
        &original,
        &[
            json!({"type":"session","version":1}),
            json!({"type":"model_change","model":"a"}),
            json!({"role":"user","content":"one"}),
            reply("first", "a"),
            json!({"type":"session_info","name":"Work"}),
            json!({"role":"user","content":"two"}),
            reply("second", "a"),
            json!({"type":"branch","parent":3}),
            json!({"type":"model_change","model":"b"}),
            json!({"role":"user","content":"other"}),
            reply("another", "b"),
        ],
        std::time::SystemTime::now(),
    );
    let saved = std::fs::read_to_string(&original).unwrap();
    agent.resume_session(&original).unwrap();
    assert!(agent.clone_session().unwrap());
    let cloned = agent.session_path().to_owned();
    assert_ne!(cloned, original);
    assert_eq!(cloned.parent(), original.parent());
    assert_eq!(std::fs::read_to_string(&original).unwrap(), saved);
    let saved: Vec<_> = saved.lines().collect();
    let lines: Vec<String> = std::fs::read_to_string(&cloned)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    // The path to the leaf, without the branch left behind, then the name.
    let path = [saved[1], saved[2], saved[3], saved[8], saved[9], saved[10]];
    assert_eq!(lines[1..7], path);
    assert_eq!(lines[7], r#"{"name":"Work","type":"session_info"}"#);
    assert_eq!(lines.len(), 8);
    let header: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(header["parentSession"], json!(original));
    assert_eq!(agent.session_name(), Some("Work"));
    assert_eq!(agent.model(), "test/b");
    assert_eq!(agent.prompts(), ["one", "other"]);

    let mut ui = Recorder::default();
    agent
        .turn("more", &mut ui, &AtomicBool::new(false))
        .unwrap();
    let bodies = server.join().unwrap();
    assert_eq!(bodies[0]["model"], "b");
    let input = bodies[0]["input"].as_array().unwrap();
    let texts: Vec<_> = input
        .iter()
        .map(|item| match &item["content"] {
            Value::String(text) => text.as_str(),
            parts => parts[0]["text"].as_str().unwrap(),
        })
        .collect();
    assert_eq!(texts, ["one", "first", "other", "another", "more"]);
    assert_eq!(
        std::fs::read_to_string(&original).unwrap(),
        saved.join("\n") + "\n"
    );
}

#[test]
fn an_export_holds_the_current_branch_and_an_import_continues_in_a_copy() {
    let (endpoint, server) = serve(vec![stream(&fixture::text_events(
        "again",
        fixture::known_usage(),
    ))]);
    let (mut agent, scratch) = agent(endpoint, "export");
    let exported = scratch.0.join("shared/work.jsonl");
    assert!(!agent.export_session(&exported).unwrap());
    assert!(!exported.exists());

    let reply = |text: &str, model: &str| {
        json!({"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":text}],"_scramjet_model":model})
    };
    let original = scratch.0.join("1700000000000-1.jsonl");
    write_session(
        &original,
        &[
            json!({"type":"session","version":1,"root":"/work","parentSession":"/old.jsonl"}),
            json!({"type":"model_change","model":"a"}),
            json!({"role":"user","content":"one"}),
            reply("first", "a"),
            json!({"type":"session_info","name":"Work"}),
            json!({"role":"user","content":"two"}),
            reply("second", "a"),
            json!({"type":"branch","parent":3}),
            json!({"type":"model_change","model":"b"}),
            json!({"role":"user","content":"other"}),
            reply("another", "b"),
        ],
        std::time::SystemTime::now(),
    );
    let saved = std::fs::read_to_string(&original).unwrap();
    agent.resume_session(&original).unwrap();
    assert!(agent.export_session(&exported).unwrap());
    let lines: Vec<String> = (std::fs::read_to_string(&exported).unwrap().lines())
        .map(str::to_owned)
        .collect();
    // The path to the leaf, without the branch left behind, then the name; the header no longer
    // names the session it was forked from.
    let saved_lines: Vec<_> = saved.lines().collect();
    let path = [1, 2, 3, 8, 9, 10].map(|line| saved_lines[line]);
    assert_eq!(lines[1..7], path);
    assert_eq!(lines[7], r#"{"name":"Work","type":"session_info"}"#);
    assert_eq!(lines.len(), 8);
    assert_eq!(
        serde_json::from_str::<Value>(&lines[0]).unwrap(),
        json!({"type":"session","version":1,"root":"/work"})
    );
    let branch = agent.branch().unwrap();
    let values: Vec<Value> = (path.iter())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(branch.entries, values);
    assert_eq!(branch.models(), ["a", "b"]);
    assert_eq!(branch.name.as_deref(), Some("Work"));

    agent.import_session(&exported).unwrap();
    let copy = agent.session_path().to_owned();
    assert_ne!(copy, original);
    assert_eq!(copy.parent(), original.parent());
    assert_eq!(
        std::fs::read_to_string(&copy).unwrap(),
        std::fs::read_to_string(&exported).unwrap()
    );
    assert_eq!(agent.session_name(), Some("Work"));
    assert_eq!(agent.model(), "test/b");
    assert_eq!(agent.prompts(), ["one", "other"]);
    let mut ui = Recorder::default();
    agent
        .turn("more", &mut ui, &AtomicBool::new(false))
        .unwrap();
    let bodies = server.join().unwrap();
    let input = bodies[0]["input"].as_array().unwrap();
    let texts: Vec<_> = (input.iter())
        .map(|item| match &item["content"] {
            Value::String(text) => text.as_str(),
            parts => parts[0]["text"].as_str().unwrap(),
        })
        .collect();
    assert_eq!(texts, ["one", "first", "other", "another", "more"]);
    assert_eq!(std::fs::read_to_string(&original).unwrap(), saved);
    assert_eq!(
        std::fs::read_to_string(&exported).unwrap().lines().count(),
        8
    );

    // What is not a session file is not copied, and the session stays.
    let sessions = || std::fs::read_dir(&scratch.0).unwrap().count();
    let before = sessions();
    let notes = scratch.0.join("shared/notes.jsonl");
    std::fs::write(&notes, "{\"type\":\"note\"}\n").unwrap();
    let error = agent.import_session(&notes).unwrap_err().to_string();
    assert!(error.contains("not a scramjet-rs session"), "{error}");
    let missing = scratch.0.join("shared/missing.jsonl");
    let error = agent.import_session(&missing).unwrap_err().to_string();
    assert!(error.contains("missing.jsonl"), "{error}");
    assert_eq!(sessions(), before);
    assert_eq!(agent.session_path(), copy);
}

#[test]
fn the_last_reply_is_the_text_of_the_latest_reply_that_has_any() {
    let (endpoint, server) = serve(vec![stream(&fixture::text_events(
        "\n**Done**: it works.\n\n- one\n- two\n\n",
        fixture::known_usage(),
    ))]);
    let (mut agent, _scratch) = agent(endpoint, "last-reply");
    assert_eq!(agent.last_reply(), None);
    let mut ui = Recorder::default();
    agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
    server.join().unwrap();
    let reply = "**Done**: it works.\n\n- one\n- two";
    assert_eq!(agent.last_reply().as_deref(), Some(reply));
    // A later reply without text, its calls and the prompts after it leave it the last.
    agent.conversation.extend([
        json!({"role":"user","content":"and now?"}),
        json!({"type":"reasoning","summary":[{"type":"summary_text","text":"Thinking."}]}),
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":" \n"}]}),
        json!({"type":"function_call","call_id":"call_2","name":"read","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"call_2","output":"text"}),
    ]);
    assert_eq!(agent.last_reply().as_deref(), Some(reply));
    agent
        .conversation
        .push(json!({"type":"message","role":"assistant","content":[
            {"type":"output_text","text":"Read "},
            {"type":"output_text","text":"it."},
        ]}));
    assert_eq!(agent.last_reply().as_deref(), Some("Read it."));
    agent.new_session().unwrap();
    assert_eq!(agent.last_reply(), None);
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
    assert_eq!((session.model(), items.len()), (Some("test/other"), 0));

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
    assert_eq!(agent.last_model(), Some("test/m"));
    assert_eq!(agent.session.model(), Some("test/m"));
    assert!(agent.session.text().contains("\"model\":\"test/m\""));
}

#[test]
fn the_same_id_from_two_providers_is_two_models() {
    let (mut agent, _scratch) = agent("127.0.0.1:9".parse().unwrap(), "same-id");
    let provider = |port: u16| {
        json!({"baseUrl": format!("http://127.0.0.1:{port}/v1"), "api": "openai-responses",
            "models": [{"id": "gpt", "name": "GPT"}]})
    };
    agent.set_providers(Providers::given(json!({"providers": {
        "first": provider(9), "second": provider(8),
    }})));
    assert_eq!(
        agent.models(),
        Some(vec!["first/gpt".to_owned(), "second/gpt".to_owned()])
    );
    // An id alone names the first provider's, as a session or setting saved before says it.
    agent.set_model("gpt").unwrap();
    assert_eq!(agent.model(), "first/gpt");
    agent.set_model("second/gpt").unwrap();
    agent.load_model_info();
    assert_eq!(
        (agent.model(), agent.model_id(), agent.model_provider()),
        ("second/gpt", "gpt", Some("second"))
    );
    assert_eq!(agent.info.route.as_ref().unwrap().port, 8);
    assert_eq!(agent.request()["model"], "gpt");
    // Reasoning is sent back only to the model that wrote it; a reply saved with an id alone is
    // the first provider's.
    let reasoning = |producer: &str| json!({"type": "reasoning", "encrypted_content": producer, "_scramjet_model": producer});
    agent.conversation = ["first/gpt", "gpt", "second/gpt"].map(reasoning).to_vec();
    let replayed = |agent: &Agent| -> Vec<Value> {
        let request = agent.request();
        let input = request["input"].as_array().unwrap();
        input
            .iter()
            .map(|item| item["encrypted_content"].clone())
            .collect()
    };
    assert_eq!(replayed(&agent), ["second/gpt"]);
    agent.set_model("first/gpt").unwrap();
    assert_eq!(replayed(&agent), ["first/gpt", "gpt"]);
}

#[test]
fn the_model_list_is_the_providers() {
    let (mut agent, _scratch) = agent("127.0.0.1:9".parse().unwrap(), "model-list");
    let references: Vec<String> = SERVED.iter().map(|id| format!("test/{id}")).collect();
    assert_eq!(agent.models(), Some(references));
    agent.set_providers(Providers::given(json!({})));
    assert_eq!(agent.models(), None);
}

#[test]
fn a_next_step_offers_the_providers_models_and_the_current_one() {
    let (mut agent, _scratch) = agent("127.0.0.1:9".parse().unwrap(), "model-options");
    let current = agent.info.clone();
    agent.set_providers(Providers::given(json!({"providers": {
        "openai": {"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
            "models": [{"id": "gpt", "reasoning": true, "input": ["text", "image"]}]},
        "anthropic": {"baseUrl": "http://127.0.0.1:9", "api": "anthropic-messages",
            "models": [{"id": "claude", "name": "Claude", "reasoning": true, "input": ["text"],
                "contextWindow": 200_000, "maxTokens": 64_000,
                "thinkingLevelMap": {"off": null, "minimal": null, "medium": null}}]},
    }})));
    let option = |id: &str, provider: Option<&str>, levels: &[&str]| ModelOption {
        id: id.into(),
        name: None,
        provider: provider.map(str::to_owned),
        reasoning: true,
        levels: levels.iter().map(|level| level.to_string()).collect(),
        window: None,
        output: None,
        images: true,
        rate: None,
    };
    // The current model, which no provider offers, goes first, as the agent knows it.
    let options = agent.model_options();
    assert_eq!(
        options,
        [
            ModelOption {
                levels: current.levels.clone(),
                ..option("test/m", None, &[])
            },
            ModelOption {
                name: Some("Claude".into()),
                window: Some(200_000),
                output: Some(64_000),
                images: false,
                ..option("claude", Some("anthropic"), &["low", "high"])
            },
            option(
                "gpt",
                Some("openai"),
                &["none", "minimal", "low", "medium", "high"]
            ),
        ]
    );
    let claude = &options[1];
    assert_eq!(claude.effort(Some("medium")).as_deref(), Some("high"));
    assert_eq!(claude.next_effort(Some("medium")).as_deref(), Some("low"));
    assert_eq!(claude.effort(None), None);
    // Without an explicit level, Scramjet's xhigh and max are taken as high.
    assert_eq!(options[2].effort(Some("max")).as_deref(), Some("high"));
    // Without models, only the current one.
    agent.set_providers(Providers::given(json!({})));
    assert_eq!(agent.model_options().len(), 1);
}

#[test]
fn entries_choose_the_protocol_and_names_do_without_one() {
    let (mut agent, _scratch) = agent("127.0.0.1:9".parse().unwrap(), "protocols");
    let only_high: serde_json::Map<String, Value> =
        ["off", "minimal", "low", "medium", "xhigh", "max"]
            .iter()
            .map(|level| (level.to_string(), Value::Null))
            .collect();
    agent.set_providers(Providers::given(json!({"providers": {"p": {
        "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
        "models": [
            {"id": "claude-opus-5-5", "api": "anthropic-messages", "contextWindow": 1_000_000,
                "maxTokens": 128_000, "reasoning": true, "thinkingLevelMap": only_high,
                "input": ["text", "image"]},
            {"id": "both", "input": ["text"]},
        ],
    }}})));
    agent.set_model("claude-opus-5-5").unwrap();
    assert_eq!(agent.load_model_info(), Some(1_000_000));
    let info = &agent.info;
    assert_eq!(
        (info.protocol, info.output),
        (Protocol::Messages, Some(128_000))
    );
    assert!(info.reasoning && info.images);
    assert_eq!(info.levels, ["high"]);
    agent.set_model("both").unwrap();
    assert_eq!(agent.info.protocol, Protocol::Responses);
    assert!(!agent.info.reasoning);
    assert!(!agent.info.images);
    assert_eq!(ModelInfo::guess("claude-new").protocol, Protocol::Messages);
    assert_eq!(ModelInfo::guess("gpt-new").protocol, Protocol::Responses);
}

#[test]
fn shift_tab_cycles_the_models_levels_and_the_effort_is_sent_fitted_saved_and_restored() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "effort-cycle");
    let entry =
        |id: &str, map: Value| json!({"id": id, "reasoning": true, "thinkingLevelMap": map});
    agent.set_providers(Providers::given(json!({"providers": {"p": {
        "baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
        "models": [
            entry("gpt-6-luna", json!({"minimal": null, "xhigh": "xhigh", "max": "max"})),
            entry("flash", json!({"minimal": null, "medium": null, "xhigh": "xhigh", "max": "max"})),
            {"id": "plain"},
        ],
    }}})));
    let saved = |text: String| -> Vec<String> {
        text.lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|line| line["type"] == "thinking_level_change")
            .map(|line| line["level"].as_str().unwrap().to_owned())
            .collect()
    };
    agent.set_model("gpt-6-luna").unwrap();
    agent.set_effort("medium").unwrap();
    let cycled: Vec<_> = (0..5)
        .map(|_| agent.cycle_effort().unwrap().unwrap())
        .collect();
    assert_eq!(cycled, ["high", "xhigh", "max", "none", "low"]);
    assert_eq!(agent.request()["reasoning"]["effort"], "low");
    assert_eq!(
        saved(agent.session.text()),
        [&["medium".to_owned()], &cycled[..]].concat()
    );

    // A level the next model lacks is sent as the nearest higher one it takes, and the cycle
    // goes on from that one.
    agent.set_effort("medium").unwrap();
    agent.set_model("flash").unwrap();
    agent.load_model_info();
    assert_eq!(agent.effort(), Some("high"));
    assert_eq!(agent.request()["reasoning"]["effort"], "high");
    assert_eq!(agent.cycle_effort().unwrap().as_deref(), Some("xhigh"));

    // A resumed session restores the level its last change recorded.
    let path = scratch.0.join("resumed.jsonl");
    write_session(
        &path,
        &[
            json!({"type":"session","version":1}),
            json!({"type":"model_change","model":"gpt-6-luna"}),
            json!({"type":"thinking_level_change","level":"max"}),
        ],
        std::time::SystemTime::now(),
    );
    agent.resume_session(&path).unwrap();
    agent.load_model_info();
    assert_eq!(agent.effort(), Some("max"));
    assert_eq!(agent.cycle_effort().unwrap().as_deref(), Some("none"));
    assert_eq!(
        saved(std::fs::read_to_string(&path).unwrap()),
        ["max", "none"]
    );
    // `/tree` lists the change while it is the current entry, as Scramjet does.
    let tree = agent.tree().unwrap();
    let current = tree
        .entries
        .iter()
        .find(|entry| Some(entry.line) == tree.current);
    assert_eq!(current.unwrap().item, TreeItem::Effort("none".into()));

    // A model without levels keeps its effort and writes nothing.
    agent.set_model("plain").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    agent.load_model_info();
    assert_eq!(agent.cycle_effort().unwrap(), None);
    assert_eq!(agent.effort(), Some("none"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    // A new session starts with the effort recorded, as with the model.
    agent.new_session().unwrap();
    assert_eq!(saved(agent.session.text()), ["none"]);
}

#[test]
fn an_effort_the_model_lacks_becomes_the_nearest_level_it_takes() {
    let info = |levels: &[&str]| ModelInfo {
        levels: levels.iter().map(|level| level.to_string()).collect(),
        ..ModelInfo::guess("m")
    };
    let opus = info(&["low", "medium", "high", "xhigh", "max"]);
    assert_eq!(opus.fit("none"), "low");
    assert_eq!(opus.fit("high"), "high");
    assert_eq!(info(&["none", "low", "high"]).fit("medium"), "high");
    assert_eq!(info(&["none", "low", "medium", "high"]).fit("max"), "high");
    // A level not in Scramjet's order is left for the provider to judge.
    assert_eq!(opus.fit("hgih"), "hgih");
    assert_eq!(info(&[]).fit("minimal"), "minimal");
}

// The broken link it reports needs a symlink, which Windows makes only with a privilege.
#[cfg(unix)]
#[test]
fn installed_says_where_each_comes_from_and_what_is_wrong() {
    let (mut agent, scratch) = agent("127.0.0.1:9".parse().unwrap(), "installed");
    let root = scratch.0.clone();
    let write = |path: &Path, text: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let agent_file =
        |name: &str| format!("---\nname: {name}\ndescription: Does {name}.\n---\nBe.\n");
    let set = root.join(".scramjet/demo");
    write(&set.join("commands/demo:go.md"), "Go.\n");
    write(
        &set.join("agents/demo:helper.md"),
        &agent_file("demo:helper"),
    );
    let user = root.join("user");
    write(&user.join("mine.md"), &agent_file("mine"));
    std::os::unix::fs::symlink(root.join("gone.md"), user.join("demo:old.md")).unwrap();
    write(&root.join(".scramjet/agents/ours.md"), &agent_file("ours"));
    write(
        &root.join(".scramjet/autonomy.yaml"),
        "edges:\n  demo:gone:\n    demo:go: chain\n",
    );
    write(&root.join("AGENTS.md"), "Be careful.\n");
    agent.agents_directory = Some(user.clone());
    agent.autonomy = None;
    project_sets(&mut agent);
    agent.instruct();
    let found = agent.installed();
    assert_eq!(found.sets, agent.sets.list());
    let agents: Vec<(&str, &AgentSource)> = (found.agents.iter())
        .map(|agent| (agent.name.as_str(), &agent.source))
        .collect();
    assert_eq!(
        agents,
        [
            ("mine", &AgentSource::User),
            ("ours", &AgentSource::Project(ProjectApproval::NotApproved)),
            ("demo:helper", &AgentSource::Set("demo".into())),
        ]
    );
    // The project agent's row follows the approval as the agent file changes, until forgotten.
    let ours = root.join(".scramjet/agents/ours.md");
    let approval = |agent: &Agent| match &agent.installed().agents[1].source {
        AgentSource::Project(approval) => *approval,
        source => panic!("{source:?}"),
    };
    let digest = approvals::digest(&agent_file("ours"));
    let remembered = json!({ours.display().to_string(): digest}).to_string();
    std::fs::write(root.join("approved-agents.json"), remembered).unwrap();
    assert_eq!(approval(&agent), ProjectApproval::Approved);
    write(&ours, &(agent_file("ours") + "Changed.\n"));
    assert_eq!(approval(&agent), ProjectApproval::Changed);
    assert_eq!(agent.forget_project_agents().unwrap(), 1);
    assert_eq!(approval(&agent), ProjectApproval::NotApproved);
    assert_eq!(agent.forget_project_agents().unwrap(), 0);
    assert_eq!(found.context_files, [root.join("AGENTS.md")]);
    assert_eq!(found.autonomy, (root.join(".scramjet/autonomy.yaml"), None));
    let autonomy = root.join(".scramjet/autonomy.yaml");
    assert_eq!(
        found.problems,
        [
            Problem::in_file(
                format!(
                    "agents: skipping user agent {} (a broken link to {}): names starting with \
                     \"demo:\" belong to the project command set {}",
                    user.join("demo:old.md").display(),
                    root.join("gone.md").display(),
                    set.display()
                ),
                &user.join("demo:old.md")
            ),
            Problem::in_file(
                format!(
                    "scramjet: project ({}): unknown source command \"demo:gone\"",
                    autonomy.display()
                ),
                &autonomy
            ),
        ]
    );
}

/// A model's prices in dollars per million tokens, cache writes at 1.25 times its input.
pub(super) fn prices(input: f64, cache_read: f64, output: f64) -> pricing::Prices {
    pricing::Prices::of(&json!({"input": input, "cacheWrite": input * 1.25,
        "cacheRead": cache_read, "output": output}))
}

pub(super) fn close(cost: f64, expected: f64) {
    assert!((cost - expected).abs() < 1e-9, "{cost} != {expected}");
}

#[test]
fn each_response_is_priced_by_its_backends_rates_and_resumes_keep_what_it_cost() {
    let served_by = |backend: Option<&str>| {
        let usage = json!({"input_tokens":1_000_000,"output_tokens":10_000,"total_tokens":1_010_000,
            "input_tokens_details":{"cached_tokens":300_000,"cache_write_tokens":100_000}});
        let response = String::from_utf8(stream(&fixture::text_events("priced", usage))).unwrap();
        let header = backend.map(|backend| format!("\r\nx-served-by: {backend}"));
        response
            .replacen("\r\n", &(header.unwrap_or_default() + "\r\n"), 1)
            .into_bytes()
    };
    let (endpoint, server) = serve(vec![
        served_by(None),
        served_by(Some(" Bedrock ")),
        served_by(Some("onehpc")),
    ]);
    let (mut agent, _scratch) = agent(endpoint, "priced");
    agent.info.prices = pricing::Prices::of(&json!({
        "input": 5, "cacheWrite": 6.25, "cacheRead": 0.5, "output": 25,
        "header": "x-served-by",
        "byHeader": {"bedrock": {"input": 5.5, "cacheWrite": 6.875, "cacheRead": 0.55,
            "output": 27.5}},
    }));
    let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
    agent.turn("one", &mut ui, &cancel).unwrap();
    // 600k input tokens neither read from the cache nor written to it, 300k read, 100k written.
    let saved = &agent.conversation().last().unwrap()[USAGE];
    assert_eq!(
        (&saved["input"], &saved["cache_read"], &saved["cache_write"]),
        (&json!(600_000), &json!(300_000), &json!(100_000))
    );
    close(saved["cost"].as_f64().unwrap(), 3.0 + 0.625 + 0.15 + 0.25);
    agent.turn("two", &mut ui, &cancel).unwrap();
    let both = 4.025 + 3.3 + 0.6875 + 0.165 + 0.275;
    let tokens = agent.session_tokens();
    close(tokens.cost, both);
    assert!(!tokens.unpriced);
    assert_eq!((tokens.input, tokens.cache_write), (1_200_000, 200_000));
    // No rates for this backend: the cost is not known.
    agent.turn("three", &mut ui, &cancel).unwrap();
    server.join().unwrap();
    assert!(
        agent.conversation().last().unwrap()[USAGE]
            .get("cost")
            .is_none()
    );
    let tokens = agent.session_tokens();
    close(tokens.cost, both);
    assert!(tokens.unpriced);
    assert_eq!(ui.usage.last().map(|(shown, _)| *shown), Some(tokens));
    assert_eq!(agent.stats().tokens, tokens);
    close(agent.usage.cost, both);
    assert!(agent.usage.unpriced && agent.usage.cache_write_tokens == 300_000);
    let (session, ..) = Session::open(agent.session_path()).unwrap();
    assert_eq!(session.tokens(), tokens);
}

#[test]
fn subagents_costs_count_in_the_sessions_and_unknown_ones_leave_it_open() {
    let mut tokens = Tokens::default();
    tokens.add(
        &json!({"type":"function_call_output","_scramjet_subagents":{"mode":"parallel",
        "runs":[{"usage":{"turns":2,"input_tokens":10,"cached_input_tokens":0,"output_tokens":1,
            "cost":0.25}},
            {"usage":{"turns":0,"input_tokens":0,"cached_input_tokens":0,"output_tokens":0}}]}}),
    );
    // Their tokens stay theirs, as Scramjet's footer counts the session's own.
    let quarter = Tokens {
        cost: 0.25,
        ..Tokens::default()
    };
    assert_eq!(tokens, quarter);
    // Saved before costs were.
    tokens.add(&json!({"_scramjet_subagents":{"runs":[{"usage":{"turns":1}}]}}));
    assert!(tokens.unpriced);
}

#[test]
fn session_tokens_count_every_response_on_every_branch_and_are_read_again_on_resume() {
    let other = json!({"input_tokens":100,"output_tokens":20,"total_tokens":120});
    let (endpoint, server) = serve(vec![
        stream(&fixture::text_events("one", fixture::known_usage())),
        stream(&fixture::text_events("two", fixture::known_usage())),
        stream(&fixture::text_events("three", other)),
    ]);
    let (mut agent, _scratch) = agent(endpoint, "session-tokens");
    assert_eq!(agent.session_tokens(), Tokens::default());
    // An empty conversation fills none of the context.
    assert_eq!(agent.context_tokens(), Some(0));
    let (mut ui, cancel) = (Recorder::default(), AtomicBool::new(false));
    agent.turn("first", &mut ui, &cancel).unwrap();
    agent.turn("second", &mut ui, &cancel).unwrap();
    // Each known usage: 10 input tokens, 3 of them read from the cache, and 5 output tokens.
    let two = Tokens {
        input: 14,
        output: 10,
        cache_read: 6,
        unpriced: true,
        ..Tokens::default()
    };
    assert_eq!(ui.usage.last(), Some(&(two, Some(15))));
    // Another branch from the first reply adds its response; leaving the second keeps it.
    let tree = agent.tree().unwrap();
    let first = (tree.entries.iter())
        .find(|entry| entry.item == TreeItem::Reply("one".into()))
        .unwrap();
    agent.navigate(first.line, None, &mut ui, &cancel).unwrap();
    assert_eq!(agent.session_tokens(), two);
    agent.turn("elsewhere", &mut ui, &cancel).unwrap();
    server.join().unwrap();
    let three = Tokens {
        input: 114,
        output: 30,
        cache_read: 6,
        unpriced: true,
        ..Tokens::default()
    };
    assert_eq!(ui.usage.last(), Some(&(three, Some(120))));
    assert_eq!(agent.session_tokens(), three);
    assert_eq!(agent.stats().tokens.input, 107);
    let (session, ..) = Session::open(agent.session_path()).unwrap();
    assert_eq!(session.tokens(), three);
}

#[test]
fn replies_keep_their_models_output_rate_for_the_footer_and_the_model_list() {
    let (endpoint, server) = serve(vec![
        stream(&fixture::call_events("{}", fixture::known_usage())),
        stream(&fixture::text_events("done", fixture::known_usage())),
    ]);
    let (mut agent, scratch) = agent(endpoint, "rates");
    let path = scratch.0.join("state/output-rates.json");
    agent.rates = Some(path.clone());
    assert_eq!(agent.output_rate(), None);
    let mut ui = Recorder::default();
    agent.turn("go", &mut ui, &AtomicBool::new(false)).unwrap();
    server.join().unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let samples = saved["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 2);
    for sample in samples {
        assert_eq!(
            (&sample["model"], &sample["output_tokens"]),
            (&json!("test/m"), &json!(5))
        );
        assert!(sample["duration_ms"].as_f64().unwrap() > 0.0);
    }
    let rate = agent.output_rate().unwrap();
    assert!(rate > 0.0);
    // Each response tells the footer the rate with its usage.
    assert_eq!(ui.rates.len(), 2);
    assert!(ui.rates.iter().all(Option::is_some));
    assert_eq!(ui.rates.last(), Some(&Some(rate)));
    // The list has the model with its rate.
    assert_eq!(agent.model_options()[0].rate, Some(rate));
}

#[test]
fn requests_end_with_the_models_identity_and_the_sessions_file() {
    let (endpoint, server) = serve(vec![
        stream(&fixture::text_events("one", fixture::known_usage())),
        stream(&fixture::text_events("two", fixture::known_usage())),
        stream(&fixture::text_events("three", fixture::known_usage())),
    ]);
    let (mut agent, _scratch) = agent(endpoint, "identity");
    let provider = |id: &str, name: &str| {
        json!({"baseUrl": format!("http://{endpoint}/v1"),
        "api": "openai-responses", "models": [{"id": id, "name": name}]})
    };
    agent.set_providers(Providers::given(json!({"providers": {
        "openai": provider("m", "Model M"),
        "anthropic": provider("c", "Claude C"),
    }})));
    agent.set_model("m").unwrap();
    let mut ui = Recorder::default();
    let cancel = AtomicBool::new(false);
    agent.turn("first", &mut ui, &cancel).unwrap();
    agent.set_model("c").unwrap();
    agent.turn("second", &mut ui, &cancel).unwrap();
    agent.set_model("m").unwrap();
    agent.turn("third", &mut ui, &cancel).unwrap();
    let bodies = server.join().unwrap();
    let m = "Model M (ID: m, provider: openai)";
    let c = "Claude C (ID: c, provider: anthropic)";
    let environment = format!(
        "\nCurrent working directory: {}\nCurrent session journal: {}",
        agent.root().display(),
        agent.session_path().display()
    );
    // The current model is listed from the first request, so the text stays after its reply.
    let expected = [(m, vec![m]), (c, vec![m, c]), (m, vec![m, c])];
    for (body, (current, listed)) in bodies.iter().zip(expected) {
        let instructions = body["instructions"].as_str().unwrap();
        let listed: Vec<String> = listed.iter().map(|model| format!("- {model}")).collect();
        let identity = format!(
            "\n\n# Model Identity\nYour model for the imminent response is: {current}.\nModels \
with material responses in this session branch, the imminent one included:\n{}\nWhen posting",
            listed.join("\n")
        );
        assert!(instructions.contains(&identity), "{instructions}");
        assert!(instructions.ends_with(&environment), "{instructions}");
        assert!(instructions.starts_with(&agent.instructions));
    }
    assert!(
        bodies[0]["instructions"]
            .as_str()
            .unwrap()
            .contains("\"Reviewed by Model M\"")
    );
    assert!(
        bodies[1]["instructions"]
            .as_str()
            .unwrap()
            .contains("\"Reviewed by Claude C\"")
    );

    // A resume knows them from the file; a move to before `c` replied knows only `m`.
    let path = agent.session_path().to_owned();
    let (mut session, _, _) = Session::open(&path).unwrap();
    assert_eq!(session.contributors(), ["openai/m", "anthropic/c"]);
    let text = std::fs::read_to_string(&path).unwrap();
    let first_reply = (text.lines())
        .position(|line| line.contains("\"_scramjet_model\":\"openai/m\""))
        .unwrap();
    session.move_to(Some(first_reply)).unwrap();
    assert_eq!(session.contributors(), ["openai/m"]);
}

#[test]
fn models_that_replied_before_a_compaction_are_still_on_the_branch() {
    let scratch = ScratchDir::new(std::env::temp_dir().join(format!(
        "scramjet-agent-contributors-{}",
        std::process::id()
    )));
    let path = scratch.0.join("compacted.jsonl");
    let lines = [
        json!({"type":"session","version":1}),
        json!({"role":"user","content":"one"}),
        json!({"type":"message","role":"assistant","content":[],"_scramjet_model":"a"}),
        json!({"role":"user","content":"two"}),
        json!({"type":"compaction","summary":"s","first_kept":2}),
        json!({"type":"message","role":"assistant","content":[],"_scramjet_model":"b"}),
        json!({"type":"message","role":"assistant","content":[],"_scramjet_model":"a"}),
    ];
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(&path, text).unwrap();
    let (mut session, items, _) = Session::open(&path).unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(session.contributors(), ["a", "b"]);
    session
        .append(&json!({"type":"message","role":"assistant","content":[],"_scramjet_model":"c"}))
        .unwrap();
    assert_eq!(session.contributors(), ["a", "b", "c"]);
}

#[test]
#[cfg_attr(windows, ignore = "writes to /dev/full, which Windows lacks")]
fn lines_held_for_want_of_room_outlive_the_run_and_a_later_one_writes_them() {
    let directory = ScratchDir::new(
        std::env::temp_dir().join(format!("scramjet-agent-kept-{}", std::process::id())),
    );
    let copies = directory.0.join("kept");
    session::kept::keep_in(&copies);
    let say = |text: &str| json!({"role":"user","content":text});
    let full = || {
        std::fs::OpenOptions::new()
            .append(true)
            .open("/dev/full")
            .unwrap()
    };
    let listed = || std::fs::read_dir(&copies).unwrap().count();
    // A dropped session's copy, once no child that another test forked shares its lock: the
    // descriptor is the child's too until its exec closes it.
    let released = |copy: &Path| {
        for _ in 0..250 {
            if std::fs::File::open(copy).unwrap().try_lock().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{} stays locked", copy.display());
    };

    let mut session = Session::create(&directory.0, Path::new("/project"), None).unwrap();
    let path = session.path().to_owned();
    session.append(&say("first")).unwrap();
    assert_eq!(session.kept_path(), None);
    session.write_to(full());
    session.append(&say("second")).unwrap();
    session.append(&say("third")).unwrap();
    let copy = session.kept_path().unwrap().to_owned();
    assert!(copy.starts_with(&copies));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&copies).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    // Locked while its run lives: nothing writes it from under the session.
    let written = session::write_kept();
    assert!(written.sessions.is_empty() && written.problems.is_empty());
    assert_eq!(written.waiting, 0);
    // The run ends with them still held, as on a signal or Ctrl+D.
    drop(session);
    assert!(copy.exists());
    assert!(!std::fs::read_to_string(&path).unwrap().contains("second"));
    released(&copy);

    let written = session::write_kept();
    assert!(written.problems.is_empty(), "{:?}", written.problems);
    let len = std::fs::read(&path).unwrap().len();
    assert_eq!(written.sessions, [(path.clone(), written.sessions[0].1)]);
    assert!(written.sessions[0].1 > 0 && written.sessions[0].1 < len);
    assert_eq!(listed(), 0);
    let (mut reopened, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(conversation, [say("first"), say("second"), say("third")]);
    reopened.append(&say("fourth")).unwrap();
    let (_, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(conversation.len(), 4);

    // Opening the session writes them in first.
    let mut session = Session::open(&path).unwrap().0;
    session.write_to(full());
    session.append(&say("held")).unwrap();
    let copy = session.kept_path().unwrap().to_owned();
    drop(session);
    assert_eq!(listed(), 1);
    released(&copy);
    let (_, conversation, _) = Session::open(&path).unwrap();
    assert_eq!(conversation.last(), Some(&say("held")));
    assert_eq!(listed(), 0);

    // A file changed since keeps its copy, said, rather than lines numbered for another file.
    let mut session = Session::open(&path).unwrap().0;
    session.write_to(full());
    session.append(&say("fifth")).unwrap();
    let copy = session.kept_path().unwrap().to_owned();
    drop(session);
    released(&copy);
    let mut other = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(other, "{}", say("elsewhere")).unwrap();
    let written = session::write_kept();
    assert!(written.sessions.is_empty());
    assert!(
        written.problems[0].contains("changed since"),
        "{:?}",
        written.problems
    );
    assert_eq!(listed(), 1);

    // Deleting the session removes its copy.
    session::Session::delete_with(&path, "scramjet-no-such-trash").unwrap();
    assert_eq!(listed(), 0);
}

#[cfg(unix)]
#[test]
fn what_another_user_owns_where_the_project_looks_is_left_out_and_said() {
    let scratch =
        ScratchDir::new(std::env::temp_dir().join(format!("sjrs-stranger-{}", std::process::id())));
    let project = scratch.0.join("shared/project");
    let file = |at: &str, text: &str| {
        let path = project.join(at);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    file("../AGENTS.md", "Planted above.");
    file("CLAUDE.md", "In the project.");
    file(".scramjet/SYSTEM.md", "Planted opening.");
    file(
        ".scramjet/autonomy.yaml",
        "edges:\n  demo:greet:\n    demo:greet: chain\n",
    );
    file(".scramjet/demo/commands/demo:greet.md", "Greet.");
    file(
        ".scramjet/agents/helper.md",
        "---\nname: helper\ndescription: Helps.\n---\nHelp.",
    );
    file(
        ".agents/skills/tidy/SKILL.md",
        "---\nname: tidy\ndescription: Tidies.\n---\nTidy.",
    );
    let found = || {
        let loaded = instructions::load(
            &SystemPrompt::default(),
            &project,
            None,
            &sandbox::Hidden::default(),
        );
        let sets = Sets::find(&[], None, &project.join(".scramjet"));
        let commands = Commands::discover(&sets);
        let edges = autonomy::Edges::load(None, &project, &commands);
        let sources = subagent::Sources {
            user: None,
            sets: &sets,
        };
        let agents = subagent::discover(&sources, &project, subagent::Scope::Both);
        let skills = Skills::discover(&[], sets.list(), &SkillOptions::default(), &project);
        let mut said: Vec<String> = (loaded.problems.iter())
            .chain(sets.warnings())
            .chain(&edges.diagnostics)
            .chain(&agents.diagnostics)
            .chain(skills.warnings())
            .map(|problem| problem.text.clone())
            .collect();
        said.sort();
        let context: Vec<String> = (loaded.context.iter())
            .filter(|(path, _)| path.starts_with(&scratch.0))
            .map(|(_, text)| text.clone())
            .collect();
        let taken = (
            context,
            loaded.opening,
            edges.resolve("demo:greet", "demo:greet"),
            (commands.list().iter())
                .map(|c| c.name.clone())
                .collect::<Vec<_>>(),
            (agents.agents.iter())
                .map(|a| a.name.clone())
                .collect::<Vec<_>>(),
            (skills.list().iter())
                .map(|s| s.name.clone())
                .collect::<Vec<_>>(),
        );
        (taken, said)
    };
    let (taken, said) = found();
    assert!(said.is_empty(), "{said:?}");
    assert_eq!(taken.0, ["Planted above.", "In the project."]);
    assert_eq!(taken.1.as_deref(), Some("Planted opening."));
    assert_eq!(taken.2, Some(autonomy::Edge::Chain));
    assert_eq!(
        (taken.3, taken.4, taken.5),
        (
            vec!["demo:greet".to_owned()],
            vec!["helper".to_owned()],
            vec!["tidy".to_owned()]
        )
    );

    let me = rustix::process::geteuid().as_raw();
    crate::os::PRETEND_USER.set(Some(me + 1));
    let (taken, said) = found();
    crate::os::PRETEND_USER.set(None);
    assert_eq!(taken.0, Vec::<String>::new());
    assert_eq!((&taken.1, taken.2), (&None, None));
    assert!(
        taken.3.is_empty() && taken.4.is_empty() && taken.5.is_empty(),
        "{taken:?}"
    );
    let left_out = |at: &str| {
        let path = std::path::absolute(project.join(at)).unwrap();
        let path = path.to_string_lossy().replace("project/../", "");
        format!("{path}: left out, as another user (uid {me}) owns it")
    };
    let mut expected: Vec<String> = [
        "../AGENTS.md",
        "CLAUDE.md",
        ".scramjet/SYSTEM.md",
        ".scramjet/autonomy.yaml",
        ".scramjet/demo",
        ".scramjet/agents",
        ".agents/skills",
    ]
    .into_iter()
    .map(left_out)
    .collect();
    expected.sort();
    assert_eq!(said, expected);
}
