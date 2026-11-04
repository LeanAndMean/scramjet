//! What the agent's tests share, and the tests of sessions, models and efforts.
use super::*;
use crate::provider_fixture as fixture;
use client::Protocol;
use session::Session;
use std::io::{BufRead, BufReader, Read, Write};
use std::{sync::atomic::AtomicBool, time::Duration};

#[derive(Default)]
pub(super) struct Recorder {
    pub(super) retries: Vec<(String, u32, Duration)>,
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
    fn assistant(&mut self, text: &str) {
        self.shown.push(format!("reply {text}"));
        self.replies.push(text.to_owned());
    }
    fn tool_call(&mut self, name: &str, summary: &str) {
        self.shown.push(format!("call {name} {summary}"));
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
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..count {
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

pub(super) fn agent(endpoint: SocketAddr, name: &str) -> (Agent, ScratchDir) {
    let root = std::env::temp_dir().join(format!("scramjet-agent-{name}-{}", std::process::id()));
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
        tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
        command_sets: Vec::new(),
        skills: Vec::new(),
    };
    let mut agent = Agent::start(config, SessionChoice::Open(path)).unwrap();
    // The user's own agents and command sets stay out of the tests.
    agent.agents_directory = None;
    agent.approvals = Some(root.join("approved-agents.json"));
    agent.skills = Skills::default();
    project_sets(&mut agent);
    agent.instructions = instructions::agent(
        &root,
        &agent.config.tools,
        &agent.commands,
        &[],
        &agent.skills,
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

/// Answers one GET request per response in turn; returns the request lines.
pub(super) fn serve_gets(
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
fn the_context_window_comes_from_proxy_model_cards() {
    let cards = json!({"models": [
        {"requestId": "other", "provider": "anthropic", "limits": {"context": 1}},
        {"requestId": "m", "provider": "openai",
            "limits": {"context": 1_050_000, "output": 128_000}},
    ]})
    .to_string();
    let alone = json!({"models": [{"requestId": "m", "provider": "openai"}]}).to_string();
    let (endpoint, server) = serve_gets(vec![
        ("200 OK", cards),
        ("200 OK", alone),
        ("404 Not Found", "no".into()),
    ]);
    let (mut cards, _scratch) = agent(endpoint, "cards");
    assert_eq!(cards.load_model_card(), Some(1_050_000));
    assert_eq!(cards.info.output, Some(128_000));
    assert_eq!(cards.provider(), Some("openai"));
    cards.set_model("m").unwrap();
    assert_eq!((cards.info.output, cards.provider()), (None, None));
    // The provider is named only when the endpoint's models have several.
    assert_eq!(cards.load_model_card(), None);
    assert_eq!(cards.provider(), None);
    assert_eq!(cards.load_model_card(), None);
    assert_eq!(server.join().unwrap(), ["GET /api/model-cards HTTP/1.1"; 3]);
    assert_eq!(cards.load_model_card(), None);
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let (mut hung, _scratch) = agent(silent.local_addr().unwrap(), "cards-silent");
    let started = std::time::Instant::now();
    assert_eq!(hung.load_model_card(), None);
    assert!(started.elapsed() < Duration::from_secs(2));
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
        endpoint,
        model: "m".into(),
        root: scratch.0.clone(),
        effort: None,
        max_output_tokens: None,
        ask: false,
        retries: 3,
        tools: crate::agent::DEFAULT_TOOLS.map(String::from).to_vec(),
        command_sets: Vec::new(),
        skills: Vec::new(),
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
        use std::os::unix::fs::PermissionsExt;
        let path = scratch.0.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{text}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
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

    // A session started here goes when the agent moves on before it holds a conversation, as
    // Scramjet would not have written it; model and effort changes do not count.
    agent.new_session().unwrap();
    let unused = agent.session_path().to_owned();
    agent.set_model("other").unwrap();
    agent.set_effort("high").unwrap();
    assert!(unused.exists());
    agent.new_session().unwrap();
    assert!(!unused.exists());
    let used = agent.session_path().to_owned();
    agent
        .add(json!({"role":"user","content":"Ran `ls`","_scramjet_typed":"!ls"}))
        .unwrap();
    agent.resume_session(&current).unwrap();
    assert!(used.exists());
    // The one opened stays, and so does one another process wrote to.
    agent.new_session().unwrap();
    assert!(current.exists());
    let shared = agent.session_path().to_owned();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&shared)
        .unwrap()
        .write_all(b"{\"role\":\"user\",\"content\":\"elsewhere\"}\n")
        .unwrap();
    agent.resume_session(&current).unwrap();
    assert!(shared.exists());
    // One another process has open stays too, as when its `--continue` took it.
    agent.new_session().unwrap();
    let continued = agent.session_path().to_owned();
    let elsewhere = Session::open(&continued).unwrap();
    agent.resume_session(&current).unwrap();
    assert!(continued.exists());
    drop(elsewhere);
    agent.new_session().unwrap();
    let last = agent.session_path().to_owned();
    agent.discard_unused_session().unwrap();
    assert!(!last.exists());
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

    assert_eq!(agent.fork(1).unwrap().as_deref(), Some("two"));
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
    assert_eq!(agent.model(), "a");
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
    assert_eq!(agent.fork(2).unwrap().as_deref(), Some("three"));
    assert_eq!(agent.model(), "b");
    let conversation = agent.conversation();
    assert_eq!(conversation.len(), 4);
    assert_eq!(compaction::previous_summary(conversation), Some("s"));
    assert_eq!(conversation[2]["content"], "two");
    assert_eq!(agent.usage.context_tokens, None);

    // Before the first prompt the conversation is empty, and the current model stays.
    agent.resume_session(&original).unwrap();
    agent.set_model("c").unwrap();
    assert_eq!(agent.fork(0).unwrap().as_deref(), Some("one"));
    assert!(agent.conversation().is_empty());
    assert_eq!(agent.model(), "c");
    let reopened = Session::open(agent.session_path()).unwrap().0;
    assert_eq!(reopened.model(), Some("c"));
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
    assert_eq!(agent.model(), "b");
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
    assert_eq!(agent.model(), "b");
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
fn a_next_step_offers_the_endpoints_models_with_their_cards_and_the_current_one() {
    let list = json!({"data": [{"id": "gpt"}, {"id": "claude"}]}).to_string();
    let cards = json!({"models": [
        {"requestId": "claude", "displayName": "Claude", "provider": "anthropic",
            "reasoning": {"levels": ["low", "high"]}},
        {"requestId": "m", "displayName": "Current", "reasoning": null},
    ]})
    .to_string();
    let (endpoint, server) = serve_gets(vec![
        ("200 OK", list),
        ("200 OK", cards),
        ("404 Not Found", "no".into()),
        ("404 Not Found", "no".into()),
    ]);
    let (agent, _scratch) = agent(endpoint, "model-options");
    let option = |id: &str, name: Option<&str>, reasoning, levels: &[&str]| ModelOption {
        id: id.into(),
        name: name.map(str::to_owned),
        provider: (id == "claude").then(|| "anthropic".to_owned()),
        reasoning,
        levels: levels.iter().map(|level| level.to_string()).collect(),
        default_level: None,
        rate: None,
    };
    // The current model, which the endpoint does not list, goes first, as its card says.
    let options = agent.model_options();
    assert_eq!(
        options,
        [
            option("m", Some("Current"), false, &[]),
            option("gpt", None, true, &[]),
            option("claude", Some("Claude"), true, &["low", "high"]),
        ]
    );
    let claude = &options[2];
    assert_eq!(claude.effort(Some("medium")).as_deref(), Some("high"));
    assert_eq!(claude.next_effort(Some("medium")).as_deref(), Some("low"));
    assert_eq!(
        (claude.effort(None), options[1].next_effort(None)),
        (None, None)
    );
    // Without a list, only the current model, as the agent knows it.
    assert_eq!(agent.model_options(), [option("m", None, true, &[])]);
    let requests = ["GET /v1/models HTTP/1.1", "GET /api/model-cards HTTP/1.1"];
    assert_eq!(server.join().unwrap(), [requests, requests].concat());
}

#[test]
fn model_cards_choose_the_protocol_and_names_do_without_one() {
    let cards = json!({"models": [
        {"requestId": "claude-opus-5-5", "supportedProtocols": ["anthropic-messages"],
         "limits": {"context": 1_000_000, "output": 128_000}, "reasoning": {"levels": ["high"]},
         "input": ["text", "image", "pdf"]},
        {"requestId": "both", "supportedProtocols": ["openai-responses", "anthropic-messages"],
         "reasoning": null, "input": ["text"]},
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
        levels: vec!["high".into()],
        default_level: None,
        images: true,
        prices: Default::default(),
    };
    assert_eq!(agent.info, expected);
    agent.set_model("both").unwrap();
    agent.load_model_card();
    assert_eq!(agent.info.protocol, Protocol::Responses);
    assert!(!agent.info.reasoning);
    assert!(!agent.info.images);
    server.join().unwrap();
    assert_eq!(ModelInfo::guess("claude-new").protocol, Protocol::Messages);
    assert_eq!(ModelInfo::guess("gpt-new").protocol, Protocol::Responses);
}

#[test]
fn shift_tab_cycles_the_card_levels_and_the_effort_is_sent_fitted_saved_and_restored() {
    let card = json!({"models": [
        {"requestId": "gpt-6-luna", "supportedProtocols": ["openai-responses"],
         "reasoning": {"levels": ["none","low","medium","high","xhigh","max"], "default": "medium"}},
        {"requestId": "flash", "supportedProtocols": ["openai-responses"],
         "reasoning": {"levels": ["none","low","high","xhigh","max"]}},
        {"requestId": "plain", "supportedProtocols": ["openai-responses"], "reasoning": {}},
    ]})
    .to_string();
    let (endpoint, server) = serve_gets(vec![("200 OK", card); 4]);
    let (mut agent, scratch) = agent(endpoint, "effort-cycle");
    let saved = |path: &Path| -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|line| line["type"] == "thinking_level_change")
            .map(|line| line["level"].as_str().unwrap().to_owned())
            .collect()
    };
    agent.set_model("gpt-6-luna").unwrap();
    agent.load_model_card();
    // No effort is set: the cycle starts after the card's default, medium.
    let cycled: Vec<_> = (0..5)
        .map(|_| agent.cycle_effort().unwrap().unwrap())
        .collect();
    assert_eq!(cycled, ["high", "xhigh", "max", "none", "low"]);
    assert_eq!(agent.request()["reasoning"]["effort"], "low");
    assert_eq!(saved(agent.session_path()), cycled);

    // A level the next model's card lacks is sent as the nearest higher one it lists, and
    // the cycle goes on from that one.
    agent.set_effort("medium").unwrap();
    agent.set_model("flash").unwrap();
    agent.load_model_card();
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
    agent.load_model_card();
    assert_eq!(agent.effort(), Some("max"));
    assert_eq!(agent.cycle_effort().unwrap().as_deref(), Some("none"));
    assert_eq!(saved(&path), ["max", "none"]);
    // `/tree` lists the change while it is the current entry, as Scramjet does.
    let tree = agent.tree().unwrap();
    let current = tree
        .entries
        .iter()
        .find(|entry| Some(entry.line) == tree.current);
    assert_eq!(current.unwrap().item, TreeItem::Effort("none".into()));

    // A model whose card lists no levels keeps its effort and writes nothing.
    agent.set_model("plain").unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    agent.load_model_card();
    assert_eq!(agent.cycle_effort().unwrap(), None);
    assert_eq!(agent.effort(), Some("none"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    // A new session starts with the effort recorded, as with the model.
    agent.new_session().unwrap();
    assert_eq!(saved(agent.session_path()), ["none"]);
    server.join().unwrap();
}

#[test]
fn an_effort_the_card_lacks_becomes_the_nearest_level_it_lists() {
    let info = |levels: &[&str]| ModelInfo {
        levels: levels.iter().map(|level| level.to_string()).collect(),
        ..ModelInfo::guess("m")
    };
    let opus = info(&["low", "medium", "high", "xhigh", "max"]);
    assert_eq!(opus.fit("none"), "low");
    assert_eq!(opus.fit("high"), "high");
    assert_eq!(info(&["none", "low", "high"]).fit("medium"), "high");
    assert_eq!(info(&["none", "low", "medium", "high"]).fit("max"), "high");
    // A level not in Scramjet's order is left for the endpoint to judge.
    assert_eq!(opus.fit("hgih"), "hgih");
    assert_eq!(info(&[]).fit("minimal"), "minimal");
}

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
            ("ours", &AgentSource::Project),
            ("demo:helper", &AgentSource::Set("demo".into())),
        ]
    );
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

/// Prices of a model's provider card, in effect since 1970, in dollars per million tokens.
pub(super) fn prices(input: f64, cache_read: f64, output: f64) -> pricing::Prices {
    let card = json!({"api":"foundry","pricingKey":"m","effectiveFrom":"1970-01-01T00:00:00.000Z",
        "inputPerMTok":input,"cacheWritePerMTok":input * 1.25,"cacheReadPerMTok":cache_read,
        "outputPerMTok":output});
    pricing::Prices::of(&[card], Some("m"))
}

pub(super) fn close(cost: f64, expected: f64) {
    assert!((cost - expected).abs() < 1e-9, "{cost} != {expected}");
}

#[test]
fn each_response_is_priced_by_the_card_of_its_backend_and_resumes_keep_what_it_cost() {
    let served_by = |backend: Option<&str>| {
        let usage = json!({"input_tokens":1_000_000,"output_tokens":10_000,"total_tokens":1_010_000,
            "input_tokens_details":{"cached_tokens":300_000,"cache_write_tokens":100_000}});
        let response = String::from_utf8(stream(&fixture::text_events("priced", usage))).unwrap();
        let header = backend.map(|backend| format!("\r\nx-llm-backend: {backend}"));
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
    let rate = |input: f64, write: f64, read: f64, output: f64| {
        json!({"inputPerMTok":input,"cacheWritePerMTok":write,"cacheReadPerMTok":read,
            "outputPerMTok":output,"effectiveFrom":"2026-01-01T00:00:00.000Z","pricingKey":"opus"})
    };
    let mut foundry = rate(5.0, 6.25, 0.5, 25.0);
    foundry["api"] = json!("foundry");
    let mut bedrock = rate(5.5, 6.875, 0.55, 27.5);
    (bedrock["api"], bedrock["upstreamProvider"]) = (json!("agentic"), json!("bedrock"));
    agent.info.prices = pricing::Prices::of(&[foundry, bedrock], Some("opus"));
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
    // Proxy has no card for this backend: the cost is not known.
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
            (&json!("m"), &json!(5))
        );
        assert!(sample["duration_ms"].as_f64().unwrap() > 0.0);
    }
    let rate = agent.output_rate().unwrap();
    assert!(rate > 0.0);
    // Each response tells the footer the rate with its usage.
    assert_eq!(ui.rates.len(), 2);
    assert!(ui.rates.iter().all(Option::is_some));
    assert_eq!(ui.rates.last(), Some(&Some(rate)));
    // The endpoint is gone: the list has the current model, with its rate.
    assert_eq!(agent.model_options()[0].rate, Some(rate));
}

#[test]
fn requests_end_with_the_models_identity_and_the_sessions_file() {
    let cards = json!({"models": [
        {"requestId": "m", "displayName": "Model M", "provider": "openai"},
        {"requestId": "c", "displayName": "Claude C", "provider": "anthropic"},
    ]})
    .to_string();
    let (gets, cards_server) = serve_gets(vec![("200 OK", cards), ("404 Not Found", "no".into())]);
    let (mut agent, _scratch) = agent(gets, "identity");
    agent.load_model_card();
    // Cards that cannot be read again keep the names known.
    agent.load_model_card();
    cards_server.join().unwrap();
    let (endpoint, server) = serve(vec![
        stream(&fixture::text_events("one", fixture::known_usage())),
        stream(&fixture::text_events("two", fixture::known_usage())),
        stream(&fixture::text_events("three", fixture::known_usage())),
    ]);
    agent.config.endpoint = endpoint;
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
    assert_eq!(session.contributors(), ["m", "c"]);
    let text = std::fs::read_to_string(&path).unwrap();
    let first_reply = (text.lines())
        .position(|line| line.contains("\"_scramjet_model\":\"m\""))
        .unwrap();
    session.move_to(Some(first_reply)).unwrap();
    assert_eq!(session.contributors(), ["m"]);
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
