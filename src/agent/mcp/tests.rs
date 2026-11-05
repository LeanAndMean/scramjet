use super::*;
use crate::agent::tests::{Recorder, ScratchDir, agent, outputs, serve, stream, tool_call};
use crate::agent::{Agent, instructions};
use crate::provider_fixture as fixture;
use std::sync::atomic::AtomicBool;

const SERVER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/mcp_server.py");

fn scratch(name: &str) -> ScratchDir {
    let path = std::env::temp_dir().join(format!("sjrs-mcp-{name}-{}", std::process::id()));
    ScratchDir::new(path)
}

fn test_server(name: &str, env: &[(&str, String)]) -> Server {
    Server {
        name: name.to_owned(),
        command: "python3".to_owned(),
        args: vec![SERVER.to_owned()],
        env: (env.iter())
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
    }
}

/// `configs` started in `root` and settled, each given ten seconds.
fn started(configs: &[Server], root: &Path) -> Servers {
    let servers = Servers::start_within(configs, root, Duration::from_secs(10));
    servers.settle(&AtomicBool::new(false));
    servers
}

fn call(servers: &Servers, tool: &str, arguments: Value) -> Result<Output, String> {
    let arguments = json!({"server": "test", "tool": tool, "arguments": arguments});
    servers.call(&arguments, &AtomicBool::new(false))
}

#[test]
fn a_servers_tools_are_listed_found_by_their_words_and_called() {
    let scratch = scratch("tools");
    let servers = started(&[test_server("test", &[])], &scratch.0);
    assert_eq!(
        servers.section(),
        "\n\n# MCP servers\n\nThese MCP servers' tools are not in your tool list: find them \
with mcp_search, which shows how to call each, then run one with mcp_call.\n- test: Tools for \
scramjet-rs's tests (7 tools)"
    );
    let [(config, State::Ready { tools, about })] = &servers.states()[..] else {
        panic!("{:?}", servers.states());
    };
    assert_eq!(config.name, "test");
    assert_eq!(about.as_deref(), Some("Tools for scramjet-rs's tests"));
    // Listed in pages of three.
    assert_eq!(
        tools,
        &["echo", "add", "forecast", "fail", "slow", "picture", "grow"]
    );

    let found = servers
        .search(&json!({"query": "weather"}), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        found,
        "1 tool matches \"weather\":\n\nServer: test\nTool: forecast\nTell the weather forecast \
for a city.\nArguments (JSON Schema): {\"properties\":{\"city\":{\"type\":\"string\"}},\"type\":\
\"object\"}"
    );
    let found = servers
        .search(
            &json!({"query": "add numbers", "limit": 1}),
            &AtomicBool::new(false),
        )
        .unwrap();
    // `grow`'s description has "Adds".
    assert!(found.starts_with("2 tools match \"add numbers\"; the best 1:\n\nServer: test\nTool: add (Add numbers)\nAdd two numbers.\nArguments"), "{found}");
    let found = servers
        .search(&json!({"query": "test/echo"}), &AtomicBool::new(false))
        .unwrap();
    assert!(found.contains("\nTool: echo\n"), "{found}");
    let listed = servers
        .search(&json!({"query": ""}), &AtomicBool::new(false))
        .unwrap();
    assert!(
        listed.starts_with(
            "Server test:\n- echo: Repeat the text it is given.\n- add: Add two numbers.\n"
        ),
        "{listed}"
    );
    let none = servers
        .search(&json!({"query": "zebra"}), &AtomicBool::new(false))
        .unwrap();
    assert!(
        none.starts_with(
            "No tool matches \"zebra\". The tools (server/tool): test/echo, test/add,"
        ),
        "{none}"
    );
    let unknown = servers
        .search(
            &json!({"query": "x", "server": "other"}),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert_eq!(unknown, "no MCP server is named other; the servers: test");

    let echoed = call(&servers, "echo", json!({"text": "hi"})).unwrap();
    assert_eq!((echoed.text.as_str(), echoed.failed), ("echo: hi", false));
    // As some models send the arguments, as JSON text.
    let echoed = call(&servers, "echo", json!("{\"text\": \"again\"}")).unwrap();
    assert_eq!(echoed.text, "echo: again");
    let added = call(&servers, "add", json!({"a": 2, "b": 3})).unwrap();
    assert_eq!(added.text, "{\n  \"sum\": 5\n}");
    let failed = call(&servers, "fail", Value::Null).unwrap();
    assert_eq!((failed.text.as_str(), failed.failed), ("it failed", true));
    let picture = call(&servers, "picture", json!({})).unwrap();
    assert_eq!(picture.text, "a dot\nImage [image/png]");
    assert_eq!(picture.image.unwrap().mime, "image/png");
    let missing = call(&servers, "nothing", json!({})).unwrap_err();
    assert!(
        missing.starts_with("test has no tool named nothing; its tools: echo, add,"),
        "{missing}"
    );
}

#[test]
fn a_tool_a_server_adds_is_found_once_it_says_its_tools_changed() {
    let scratch = scratch("grow");
    let servers = started(&[test_server("test", &[])], &scratch.0);
    assert!(
        servers
            .search(&json!({"query": "extra"}), &AtomicBool::new(false))
            .unwrap()
            .starts_with("1 tool matches \"extra\":\n\nServer: test\nTool: grow\n")
    );
    assert_eq!(call(&servers, "grow", json!({})).unwrap().text, "grown");
    let found = servers
        .search(&json!({"query": "extra"}), &AtomicBool::new(false))
        .unwrap();
    assert!(
        found.starts_with("2 tools match \"extra\":\n\nServer: test\nTool: extra\nAdded later."),
        "{found}"
    );
    assert_eq!(
        call(&servers, "extra", json!({})).unwrap_err(),
        "test: Mcp error: -32602: Unknown tool: extra"
    );
}

#[test]
fn a_call_stopped_cancels_its_request_and_ending_the_servers_kills_them() {
    let scratch = scratch("cancel");
    let log = scratch.0.join("log");
    let servers = started(
        &[test_server(
            "test",
            &[("MCP_TEST_LOG", log.display().to_string())],
        )],
        &scratch.0,
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let stopper = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stopper.store(true, Ordering::Relaxed);
    });
    let began = std::time::Instant::now();
    let arguments = json!({"server": "test", "tool": "slow"});
    let stopped = servers.call(&arguments, &cancel).unwrap_err();
    assert_eq!(stopped, "cancelled; test's slow may have run in part");
    assert!(began.elapsed() < Duration::from_secs(5));
    let logged = || std::fs::read_to_string(&log).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !logged().contains("notifications/cancelled") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let logged = logged();
    let methods: Vec<&str> = logged.lines().skip(1).collect();
    assert_eq!(
        methods,
        [
            "server/discover",
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/list",
            "tools/list",
            "tools/call",
            "notifications/cancelled"
        ]
    );
    let pid: u32 = logged
        .lines()
        .next()
        .unwrap()
        .strip_prefix("pid ")
        .unwrap()
        .parse()
        .unwrap();
    let running = || {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
    };
    assert!(running());
    drop(servers);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while running() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!running());
}

#[test]
fn servers_that_cannot_start_say_why_once_and_the_others_run() {
    let scratch = scratch("failing");
    let missing = Server {
        name: "missing".to_owned(),
        command: "/nonexistent/mcp-server".to_owned(),
        args: Vec::new(),
        env: Vec::new(),
    };
    let configs = [
        test_server("exits", &[("MCP_TEST_MODE", "exit".to_owned())]),
        missing,
        test_server("test", &[]),
    ];
    let servers = started(&configs, &scratch.0);
    let failures = servers.unreported_failures();
    assert_eq!(
        failures,
        [
            ("exits".to_owned(), "it ended before it answered; it wrote:\ntest server starting\nno configuration found".to_owned()),
            ("missing".to_owned(), "could not run /nonexistent/mcp-server: No such file or directory (os error 2)".to_owned()),
        ]
    );
    assert!(servers.unreported_failures().is_empty());
    let section = servers.section();
    assert!(section.ends_with("\n- exits: not available\n- missing: not available\n- test: Tools for scramjet-rs's tests (7 tools)"), "{section}");
    let found = servers
        .search(&json!({"query": "echo"}), &AtomicBool::new(false))
        .unwrap();
    assert!(found.starts_with("1 tool matches \"echo\":"), "{found}");
    assert!(found.ends_with("\n\nNot available:\n- exits: exits is not available: it ended before it answered\n- missing: missing is not available: could not run /nonexistent/mcp-server: No such file or directory (os error 2)"), "{found}");
    let arguments = json!({"server": "missing", "tool": "echo"});
    let refused = servers
        .call(&arguments, &AtomicBool::new(false))
        .unwrap_err();
    assert!(
        refused.starts_with("missing is not available: could not run"),
        "{refused}"
    );
}

#[test]
fn a_server_that_does_not_answer_is_given_up_and_killed() {
    let scratch = scratch("silent");
    let log = scratch.0.join("log");
    let env = [
        ("MCP_TEST_MODE", "silent".to_owned()),
        ("MCP_TEST_LOG", log.display().to_string()),
    ];
    let servers = Servers::start_within(
        &[test_server("test", &env)],
        &scratch.0,
        Duration::from_secs(1),
    );
    assert_eq!(servers.starting(), ["test"]);
    servers.settle(&AtomicBool::new(false));
    let [(_, State::Failed(why))] = &servers.states()[..] else {
        panic!("{:?}", servers.states());
    };
    assert_eq!(
        why,
        "it did not start within 1 s; it wrote:\ntest server starting"
    );
    let logged = std::fs::read_to_string(&log).unwrap();
    let pid = logged.lines().next().unwrap().strip_prefix("pid ").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let alive = || {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat
                .rsplit(')')
                .next()
                .unwrap_or("")
                .trim_start()
                .starts_with('Z')
        })
    };
    while alive() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!alive());
}

#[test]
fn outputs_keep_text_resources_and_what_they_cannot_show() {
    let result = json!({"content": [
        {"type": "resource_link", "uri": "file:///a.txt", "name": "a.txt"},
        {"type": "resource", "resource": {"uri": "file:///b.txt", "text": "bee"}},
        {"type": "resource", "resource": {"uri": "file:///c.bin", "mimeType": "application/octet-stream", "blob": "AAAA"}},
        {"type": "audio", "data": "AAAA", "mimeType": "audio/wav"},
        {"type": "image", "data": "not base64!", "mimeType": "image/png"},
    ]});
    let output = output(result);
    assert_eq!(
        output.text,
        "[Resource a.txt: file:///a.txt]\nbee\n[Resource file:///c.bin [application/octet-stream] \
not shown]\n[Audio [audio/wav] not shown]\n[Image [image/png] not shown: it could not be read]"
    );
    assert!(output.image.is_none() && !output.failed);
    assert_eq!(super::output(json!({"content": []})).text, "(no output)");
    let long = "line\n".repeat(MAX_LINES + 5);
    let cut = super::output(json!({"content": [{"type": "text", "text": long}]})).text;
    assert!(
        cut.ends_with(&format!(
            "line\n[Output cut: showing {MAX_LINES} of {} lines (50 KB at most)]",
            MAX_LINES + 5
        )),
        "{}",
        &cut[cut.len() - 100..]
    );
}

#[test]
fn the_model_finds_a_servers_tool_and_calls_it_through_the_pair_of_tools() {
    let (endpoint, server) = serve(vec![
        tool_call("call_1", SEARCH, json!({"query": "repeat text"})),
        tool_call(
            "call_2",
            CALL,
            json!({"server": "test", "tool": "echo", "arguments": {"text": "hello"}}),
        ),
        stream(&fixture::text_events("done", fixture::known_usage())),
    ]);
    let (mut agent, scratch) = agent(endpoint, "mcp");
    agent.config.mcp_servers = vec![test_server("test", &[])];
    agent.start_mcp();
    agent.instructions = instructions::agent(
        &scratch.0,
        &agent.offered(),
        &agent.commands,
        &[],
        &agent.skills,
        &Default::default(),
    );
    let mut ui = Recorder::default();
    agent
        .turn("say hello", &mut ui, &AtomicBool::new(false))
        .unwrap();
    let bodies = server.join().unwrap();
    let names: Vec<&str> = (bodies[0]["tools"].as_array().unwrap().iter())
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.ends_with(&[SEARCH, CALL]), "{names:?}");
    let instructions = bodies[0]["instructions"].as_str().unwrap();
    assert!(instructions.contains("\n- mcp_search: Find tools of the MCP servers listed below, with how to call each\n- mcp_call: Call a tool mcp_search found\n"), "{instructions}");
    assert!(
        instructions
            .contains("\n- test: Tools for scramjet-rs's tests (7 tools)\n\n# Model Identity"),
        "{instructions}"
    );
    assert_eq!(bodies[1]["instructions"], bodies[0]["instructions"]);
    let results = outputs(&bodies[2]);
    assert!(
        results[0]
            .1
            .starts_with("1 tool matches \"repeat text\":\n\nServer: test\nTool: echo\n"),
        "{results:?}"
    );
    assert_eq!(results[1], ("call_2".to_owned(), "echo: hello".to_owned()));
    let calls: Vec<&String> = (ui.shown.iter())
        .filter(|shown| shown.starts_with("call "))
        .collect();
    assert_eq!(
        calls,
        [
            "call mcp_search \"repeat text\"",
            "call mcp_call test/echo {\"text\":\"hello\"}"
        ]
    );
}

#[test]
fn without_servers_the_model_has_no_mcp_tools_and_none_start() {
    let (endpoint, _server) = serve(Vec::new());
    let (mut agent, _scratch) = agent(endpoint, "no-mcp");
    assert!(agent.mcp.is_none());
    let definitions = agent.tool_definitions();
    let names: Vec<&str> = (definitions.as_array().unwrap().iter())
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&SEARCH) && !names.contains(&CALL),
        "{names:?}"
    );
    // Nor with servers when the command line leaves their tools out.
    agent.config.mcp_servers = vec![test_server("test", &[])];
    agent.config.only_tools = Some(vec!["read".to_owned()]);
    agent.start_mcp();
    assert!(agent.mcp.is_none());
    assert!(agent.mcp_servers().is_empty());
}

/// The process ids `log` holds, in the order the servers started.
fn pids(log: &Path) -> Vec<u32> {
    (std::fs::read_to_string(log).unwrap_or_default().lines())
        .filter_map(|line| line.strip_prefix("pid ")?.parse().ok())
        .collect()
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|stat| !(stat.rsplit(')').next().unwrap_or("").trim_start()).starts_with('Z'))
}

fn until(done: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !done() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_server_writing_bytes_that_are_not_utf8_keeps_its_standard_error() {
    let scratch = scratch("latin1");
    let servers = started(
        &[test_server(
            "test",
            &[("MCP_TEST_MODE", "latin1".to_owned())],
        )],
        &scratch.0,
    );
    for text in ["one", "two"] {
        let echoed = call(&servers, "echo", json!({"text": text})).unwrap();
        assert_eq!(echoed.text, format!("echo: {text}"));
    }
    assert!(matches!(servers.states()[0].1, State::Ready { .. }));
    let kept = servers.servers[0].failure("why");
    assert!(kept.contains("\ncaf\u{fffd}\n"), "{kept}");
    assert!(kept.ends_with("\ngot tools/call"), "{kept}");
}

#[test]
fn reload_starts_again_the_servers_that_ended_or_changed_and_keeps_the_others() {
    let (endpoint, _server) = serve(Vec::new());
    let (mut agent, scratch) = agent(endpoint, "mcp-reload");
    let (log_a, log_b) = (scratch.0.join("a.log"), scratch.0.join("b.log"));
    let a = test_server("a", &[("MCP_TEST_LOG", log_a.display().to_string())]);
    let b = test_server("b", &[("MCP_TEST_LOG", log_b.display().to_string())]);
    agent.config.mcp_servers = vec![a.clone(), b.clone()];
    agent.start_mcp();
    let settle = |agent: &Agent| agent.mcp.as_ref().unwrap().settle(&AtomicBool::new(false));
    settle(&agent);
    let (first_a, first_b) = (pids(&log_a), pids(&log_b));
    assert_eq!((first_a.len(), first_b.len()), (1, 1));
    // `a` ends.
    std::process::Command::new("kill")
        .args(["-9", &first_a[0].to_string()])
        .status()
        .unwrap();
    until(|| matches!(agent.mcp_servers()[0].1, State::Failed(_)));
    assert!(matches!(agent.mcp_servers()[0].1, State::Failed(_)));
    let reload = |agent: &mut Agent, servers: Vec<Server>| {
        let config = &agent.config;
        let (tools, sets, skills) = (
            config.tools.clone(),
            config.command_sets.clone(),
            config.skills.clone(),
        );
        agent.reload(tools, sets, skills, servers);
    };
    reload(&mut agent, vec![a.clone(), b.clone()]);
    settle(&agent);
    assert!(matches!(agent.mcp_servers()[0].1, State::Ready { .. }));
    assert_eq!(pids(&log_a).len(), 2);
    assert_eq!(pids(&log_b), first_b);
    // Only `b` changed.
    let mut changed = b.clone();
    changed.env.push(("EXTRA".to_owned(), "1".to_owned()));
    reload(&mut agent, vec![a.clone(), changed]);
    settle(&agent);
    assert_eq!(pids(&log_a).len(), 2);
    let second_b = pids(&log_b);
    assert_eq!(second_b.len(), 2);
    until(|| !alive(first_b[0]));
    assert!(!alive(first_b[0]));
    assert!(alive(second_b[1]));
    // `b` removed.
    reload(&mut agent, vec![a]);
    until(|| !alive(second_b[1]));
    assert!(!alive(second_b[1]));
    let section = agent.mcp.as_ref().unwrap().section();
    assert!(
        section.ends_with("mcp_call.\n- a: Tools for scramjet-rs's tests (7 tools)"),
        "{section}"
    );
}

#[test]
fn a_search_returning_more_than_a_tool_output_holds_is_cut() {
    let scratch = scratch("many");
    let servers = started(
        &[test_server(
            "test",
            &[("MCP_TEST_TOOLS", "1000".to_owned())],
        )],
        &scratch.0,
    );
    let listed = servers
        .search(&json!({"query": ""}), &AtomicBool::new(false))
        .unwrap();
    assert!(listed.len() <= MAX_BYTES + 100, "{}", listed.len());
    assert!(listed.contains("\n[Output cut: showing "), "{listed}");
}

#[test]
fn escape_stops_a_search_waiting_for_a_server_to_list_its_tools() {
    let scratch = scratch("stuck");
    let servers = started(
        &[test_server(
            "test",
            &[("MCP_TEST_MODE", "stuck".to_owned())],
        )],
        &scratch.0,
    );
    let began = std::time::Instant::now();
    let cancel = AtomicBool::new(true);
    let found = servers.search(&json!({"query": "weather"}), &cancel);
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "{:?}",
        began.elapsed()
    );
    assert!(found.unwrap().contains("\nTool: forecast\n"));
}

#[test]
fn servers_ended_as_they_start_leave_no_process() {
    let scratch = scratch("ended-early");
    let log = scratch.0.join("log");
    // A server that never reads its input, so that only killing it ends it.
    let server = Server {
        name: "sleeper".to_owned(),
        command: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            format!("echo pid $$ >> '{}'; exec sleep 30", log.display()),
        ],
        env: Vec::new(),
    };
    for round in 0..40 {
        let servers = Servers::start(std::slice::from_ref(&server), &scratch.0);
        std::thread::sleep(Duration::from_micros(round * 50));
        drop(servers);
    }
    let started = pids(&log);
    until(|| started.iter().all(|pid| !alive(*pid)));
    let left: Vec<&u32> = started.iter().filter(|pid| alive(**pid)).collect();
    for pid in &left {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    assert!(left.is_empty(), "{} of {} left", left.len(), started.len());
}
