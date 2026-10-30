use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};
#[path = "responses_fixture.rs"]
pub(super) mod fixture;
static NEXT: AtomicUsize = AtomicUsize::new(0);
pub(super) struct Dirs {
    pub(super) base: PathBuf,
    pub(super) project: PathBuf,
    pub(super) storage: PathBuf,
}
impl Dirs {
    pub(super) fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "scramjet-invocation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
        let project = base.join("project");
        let storage = base.join("storage");
        for path in [&project, &storage] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        Self {
            base,
            project,
            storage,
        }
    }
    pub(super) fn input(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.base.join(name);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
}
impl Drop for Dirs {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}
pub(super) fn command(
    dirs: &Dirs,
    endpoint: SocketAddr,
    names: &[&str; 2],
    prompt: &Path,
    prior: Option<&Path>,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"));
    command
        .args(["coding", "--endpoint", &endpoint.to_string(), "--project"])
        .arg(&dirs.project)
        .arg("--storage")
        .arg(&dirs.storage)
        .args([
            "--run",
            "ordinary-run",
            "--file",
            names[0],
            "--file",
            names[1],
            "--prompt-file",
        ])
        .arg(prompt)
        .arg("--approve-effects");
    if let Some(prior) = prior {
        command.arg("--record-file").arg(prior);
    }
    command.stdin(Stdio::null());
    command
}
pub(super) fn body(stream: &mut TcpStream) -> Value {
    stream
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
        assert!(head.len() <= 16_384);
    }
    let head = std::str::from_utf8(&head).unwrap();
    assert!(head.starts_with("POST /v1/responses HTTP/1.1\r\n"));
    let size = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|n| n.trim().parse::<usize>().ok())
        })
        .unwrap();
    assert!(size <= 16_384);
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
pub(super) fn events(index: usize, call: Option<(&str, Value)>) -> Vec<Value> {
    let mut events = if let Some((name, args)) = call {
        let mut events = fixture::continuation_events(
            "Model commentary hé🙂",
            &args.to_string(),
            "opaque encrypted /+ hé🙂",
        );
        for event in &mut events {
            if event["item"]["type"] == "function_call" {
                event["item"]["name"] = json!(name);
            }
        }
        for item in events.last_mut().unwrap()["response"]["output"]
            .as_array_mut()
            .unwrap()
        {
            if item["type"] == "function_call" {
                item["name"] = json!(name);
            }
        }
        events
    } else {
        fixture::text_events("This message is complete.", fixture::known_usage())
    };
    fixture::qualify_events(&mut events, index);
    events
}
pub(super) fn server(
    calls: Vec<Option<(&'static str, Value)>>,
    prompts: [String; 2],
    second: usize,
    poem: String,
    project: PathBuf,
    names: [String; 2],
) -> (SocketAddr, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(40);
        let mut requests = Vec::<Value>::new();
        let mut expected = vec![json!({"role":"user","content":prompts[0]})];
        let mut previous_items = Vec::new();
        let mut call_id = None;
        for (index, call) in calls.iter().enumerate() {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing request {index}");
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            let request = body(&mut stream);
            if index > 0 {
                expected.append(&mut previous_items);
                if let Some(id) = call_id.take() {
                    let result = request["input"]
                        .as_array()
                        .unwrap()
                        .get(expected.len())
                        .unwrap()
                        .clone();
                    assert_eq!(result["type"], "function_call_output");
                    assert_eq!(result["call_id"], id);
                    let actual: Value =
                        serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
                    match calls[index - 1].as_ref().unwrap().0 {
                        "write" => {
                            assert_eq!(actual["status"], "created");
                            let path = calls[index - 1].as_ref().unwrap().1["path"]
                                .as_str()
                                .unwrap();
                            assert_eq!(actual["path"], path);
                            assert_eq!(actual["mode"], 384);
                            assert_eq!(
                                fs::read_to_string(project.join(path)).unwrap(),
                                calls[index - 1].as_ref().unwrap().1["content"]
                                    .as_str()
                                    .unwrap()
                            );
                        }
                        "read" => assert_eq!(
                            actual,
                            json!({"status":"read","path":names[0],"content":poem})
                        ),
                        "python" => {
                            assert_eq!(actual["exit_code"], 0);
                            assert_eq!(actual["stdout"], poem);
                            assert_eq!(actual["stderr"], "");
                            assert_eq!(actual["launcher_reaped"], true);
                            assert_eq!(actual["namespace_dead"], true);
                            assert_eq!(actual["stopped"], Value::Null);
                        }
                        _ => panic!(),
                    }
                    expected.push(result);
                }
                if index == second {
                    expected.push(json!({"role":"user","content":prompts[1]}));
                }
            }
            assert_eq!(
                request["input"],
                json!(expected),
                "complete request {index}"
            );
            assert_eq!(request["model"], scramjet_rs::PROVIDER_MODEL);
            assert_eq!(request["max_output_tokens"], 1024);
            assert_eq!(request["parallel_tool_calls"], false);
            assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
            let tools = request["tools"].as_array().unwrap();
            assert_eq!(tools.len(), 3);
            assert_eq!(
                tools
                    .iter()
                    .map(|t| t["name"].as_str().unwrap())
                    .collect::<Vec<_>>(),
                ["write", "read", "python"]
            );
            for tool in tools {
                assert_eq!(tool["strict"], true);
                assert_eq!(
                    tool["parameters"]["properties"]["path"]["enum"],
                    json!(names)
                );
            }
            if let Some(first) = requests.first() {
                let mut expected_request = first.clone();
                expected_request["input"] = json!(expected);
                assert_eq!(request, expected_request);
            }
            requests.push(request);
            let events = events(
                index,
                call.as_ref().map(|(name, args)| (*name, args.clone())),
            );
            previous_items = events.last().unwrap()["response"]["output"]
                .as_array()
                .unwrap()
                .clone();
            call_id = previous_items
                .iter()
                .find_map(|i| i["call_id"].as_str().map(str::to_owned));
            let wire = fixture::wire(&events, true);
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",wire.len()).unwrap();
            stream.write_all(&wire).unwrap();
        }
        requests
    });
    (endpoint, worker)
}
