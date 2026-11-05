use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[test]
fn help_shows_the_agents_usage_and_other_words_are_refused() {
    let binary = env!("CARGO_BIN_EXE_scramjet-rs");
    for help in ["--help", "-h"] {
        let output = Command::new(binary).arg(help).output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.starts_with("Usage: scramjet-rs [--project DIR]"),
            "{text}"
        );
    }
    for word in ["--bogus", "-x"] {
        let output = Command::new(binary).arg(word).output().unwrap();
        assert!(!output.status.success());
        let text = String::from_utf8(output.stderr).unwrap();
        assert!(
            text.starts_with(&format!("scramjet-rs: unknown argument \"{word}\"")),
            "{text}"
        );
        assert!(text.contains("Usage: scramjet-rs"), "{text}");
    }
    let refused = |arguments: &[&str]| {
        let output = Command::new(binary).args(arguments).output().unwrap();
        assert!(!output.status.success());
        String::from_utf8(output.stderr).unwrap()
    };
    assert!(refused(&["-p"]).starts_with("scramjet-rs: -p needs a message or an @FILE\n"));
    assert!(refused(&["-"]).starts_with("scramjet-rs: the message - (read from stdin) needs -p\n"));
    // Before a session is started.
    let missing = refused(&["-p", "@/nonexistent/notes.md"]);
    assert_eq!(
        missing,
        "scramjet-rs: file not found: /nonexistent/notes.md\n"
    );
    for option in ["--thinking", "--effort"] {
        let output = Command::new(binary)
            .args([option, "hgih"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!(
                "scramjet-rs: invalid {option} \"hgih\": valid levels are off, minimal, low, \
medium, high, xhigh, max\n"
            )
        );
    }
    for version in ["--version", "-v"] {
        let output = Command::new(binary).arg(version).output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text, format!("{}\n", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn list_models_lists_the_providers_models_and_what_is_wrong_in_their_files() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-models-{}", std::process::id())));
    let list = || {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .arg("--list-models")
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = |bytes| String::from_utf8(bytes).unwrap();
        (text(output.stdout), text(output.stderr))
    };
    let none = ("No provider offers any model\n".to_owned(), String::new());
    assert_eq!(list(), none);
    provide(&scratch.0.join("config"), "127.0.0.1:9");
    let file = scratch.0.join("config/scramjet-rs/models.json");
    let mut models: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    models["providers"]["remote"] = serde_json::json!({"baseUrl": "https://example.com/v1",
        "api": "openai-responses", "models": [{"id": "far"}]});
    models["providers"]["odd"] = serde_json::json!({"baseUrl": "ftp://example.com/v1",
        "api": "openai-responses", "models": [{"id": "elsewhere"}]});
    std::fs::write(&file, models.to_string()).unwrap();
    let (listed, warnings) = list();
    for offered in ["test", "gpt-6-luna", "remote", "far"] {
        assert!(listed.contains(offered), "{listed}");
    }
    assert!(!listed.contains("elsewhere"), "{listed}");
    let problem = format!(
        "Warning: {}: provider odd: model elsewhere: baseUrl ftp://example.com/v1: not an http:// \
or https:// URL\n",
        file.display()
    );
    assert_eq!(warnings, problem);
}

#[test]
fn list_credential_stores_lists_the_common_ones_and_those_files_name() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-stores-{}", std::process::id())));
    let (home, config, package) = (
        scratch.0.join("home"),
        scratch.0.join("config"),
        scratch.0.join("package"),
    );
    for directory in [home.join(".ssh"), home.join(".fake"), package.clone()] {
        std::fs::create_dir_all(directory).unwrap();
    }
    std::fs::write(home.join(".netrc"), "").unwrap();
    std::fs::write(package.join("token"), "").unwrap();
    provide(&config, "127.0.0.1:9");
    // scramjet-rs's own logins, beside the settings.
    std::fs::write(config.join("scramjet-rs/auth.json"), "{}").unwrap();
    let file = config.join("scramjet-rs/models.json");
    let mut models: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    models["credentialFiles"] = serde_json::json!(["~/.fake", "~/.ssh/inside", "~/absent"]);
    std::fs::write(&file, models.to_string()).unwrap();
    let package_models = package.join("models.json");
    std::fs::write(&package_models, r#"{"credentialFiles": ["token"]}"#).unwrap();
    let settings = serde_json::json!({"commandSets": [package]});
    std::fs::write(
        config.join("scramjet-rs/settings.json"),
        settings.to_string(),
    )
    .unwrap();
    let list = || {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .arg("--list-credential-stores")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("SCRAMJET_CODING_AGENT_DIR")
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = |bytes| String::from_utf8(bytes).unwrap();
        (text(output.stdout), text(output.stderr))
    };
    let real = |path: std::path::PathBuf| std::fs::canonicalize(path).unwrap();
    let listed: Vec<String> = [config.join("scramjet-rs/auth.json"), home.join(".fake")]
        .into_iter()
        .chain([
            home.join(".netrc"),
            home.join(".ssh"),
            package.join("token"),
        ])
        .map(|path| format!("{}\n", real(path).display()))
        .collect();
    assert_eq!(list(), (listed.concat(), String::new()));
    std::fs::write(&package_models, r#"{"credentialFiles": "token"}"#).unwrap();
    let (_, warnings) = list();
    let problem = format!(
        "Warning: {}: credentialFiles must be a list of files and directories\n",
        package_models.display()
    );
    assert_eq!(warnings, problem);
}

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes the `models.json` of the configuration directory `config`: a provider at `endpoint`
/// offering gpt-6-luna.
fn provide(config: &std::path::Path, endpoint: &str) {
    std::fs::create_dir_all(config.join("scramjet-rs")).unwrap();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://{endpoint}/v1"), "api": "openai-responses",
        "models": [{"id": "gpt-6-luna", "reasoning": true, "input": ["text", "image"]}],
    }}});
    std::fs::write(config.join("scramjet-rs/models.json"), models.to_string()).unwrap();
}

#[test]
fn https_providers_are_reached_over_tls_and_untrusted_certificates_refused_once() {
    let binary = env!("CARGO_BIN_EXE_scramjet-rs");
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-https-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let tls = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/tls");
    let (port, connections, heads) = serve_tls(&tls);
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet-rs")).unwrap();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("https://localhost:{port}"), "api": "anthropic-messages",
        "models": [{"id": "claude-test"}],
    }}});
    std::fs::write(config.join("scramjet-rs/models.json"), models.to_string()).unwrap();
    // As if the system trusted only the certificates in `trusted`.
    let run = |trusted: &str| {
        Command::new(binary)
            .args(["--retries", "2", "--no-session", "-p", "hi"])
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", &config)
            .env("SCRAMJET_CODING_AGENT_DIR", scratch.0.join("agent"))
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            .env("SSL_CERT_FILE", tls.join(trusted))
            .env_remove("SSL_CERT_DIR")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    };
    let output = run("ca.pem");
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
    let (stdout, stderr) = (text(output.stdout), text(output.stderr));
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stdout.trim_end(), "Reached over TLS.", "{stderr}");
    let head = heads.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(head.starts_with("post /v1/messages http/1.1\r\n"), "{head}");
    assert!(
        head.contains(&format!("\r\nhost: localhost:{port}\r\n")),
        "{head}"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 1);

    // Trusting only the server's own certificate, which is no CA's: no retry would help.
    let output = run("localhost.pem");
    assert!(!output.status.success());
    let stderr = text(output.stderr);
    assert_eq!(
        stderr.lines().last().unwrap_or_default(),
        format!(
            "scramjet-rs: provider: TLS handshake with localhost:{port} (provider test) failed: \
invalid peer certificate: UnknownIssuer; HTTPS trusts the system's certificates, or those \
SSL_CERT_FILE and SSL_CERT_DIR name"
        ),
        "{stderr}"
    );
    assert_eq!(connections.load(Ordering::SeqCst), 2);
}

/// A provider on a port of 127.0.0.1 speaking TLS with `tls`' certificate for localhost,
/// answering each request with the same short Messages reply: its port, the connections it
/// took, and the head (request line and headers, lowercased) of each request it answered.
fn serve_tls(tls: &std::path::Path) -> (u16, Arc<AtomicUsize>, std::sync::mpsc::Receiver<String>) {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    let chain = CertificateDer::pem_file_iter(tls.join("localhost.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(tls.join("localhost.key")).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let config = Arc::new(config);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let connections = Arc::new(AtomicUsize::new(0));
    let (heads, received) = std::sync::mpsc::channel();
    let counted = connections.clone();
    let events = [
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Reached over TLS."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ];
    let body: String = (events.iter())
        .map(|event| {
            let kind: serde_json::Value = serde_json::from_str(event).unwrap();
            format!(
                "event: {}\ndata: {event}\n\n",
                kind["type"].as_str().unwrap()
            )
        })
        .collect();
    let reply = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, socket.unwrap());
            let mut reader = BufReader::new(&mut stream);
            let (mut head, mut length) = (String::new(), 0);
            // A client that does not trust the certificate ends the handshake here.
            while let Ok(read) = reader.read_line(&mut head) {
                let line = &head[head.len() - read..];
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if read == 0 || line == "\r\n" {
                    break;
                }
            }
            if !head.ends_with("\r\n\r\n") {
                continue;
            }
            let mut body = vec![0; length];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            stream.conn.send_close_notify();
            let _ = stream.flush();
            let _ = heads.send(head.to_lowercase());
        }
    });
    (port, connections, received)
}

#[test]
fn session_and_fork_find_a_session_by_the_start_of_its_id() {
    let binary = env!("CARGO_BIN_EXE_scramjet-rs");
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-sessions-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let sessions = scratch.0.join("state/scramjet-rs/sessions");
    let slug = project
        .to_str()
        .unwrap()
        .trim_matches('/')
        .replace('/', "-");
    let (here, other) = (sessions.join(slug), sessions.join("work-other"));
    let source = other.join("1700000000000-7.jsonl");
    let theirs = "{\"role\":\"user\",\"content\":\"one\"}\n";
    let ours = here.join("1600000000000-3.jsonl");
    for (path, root, lines) in [
        (&source, "/work/other", theirs),
        (
            &ours,
            project.to_str().unwrap(),
            "{\"role\":\"user\",\"content\":\"here\"}\n",
        ),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let header = format!("{{\"type\":\"session\",\"version\":1,\"root\":{root:?}}}\n");
        std::fs::write(path, header + lines).unwrap();
    }
    provide(&scratch.0.join("config"), "127.0.0.1:9");
    let run = |arguments: &[&str]| {
        let output = Command::new(binary)
            .args(["--retries", "0"])
            .args(arguments)
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
            .env("SCRAMJET_CODING_AGENT_DIR", scratch.0.join("agent"))
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        error.lines().last().unwrap_or_default().to_owned()
    };
    let first_line = |arguments: &[&str]| {
        let output = Command::new(binary).args(arguments).output().unwrap();
        let error = String::from_utf8(output.stderr).unwrap();
        error.lines().next().unwrap_or_default().to_owned()
    };
    assert_eq!(
        first_line(&["--fork", "x", "-c"]),
        "scramjet-rs: --fork cannot be combined with --continue"
    );
    assert_eq!(
        first_line(&["-r", "--session", "a", "--fork", "b", "-c"]),
        "scramjet-rs: --fork cannot be combined with --session, --continue, --resume"
    );
    for option in ["--session", "--fork"] {
        assert_eq!(
            run(&[option, "1999", "-p", "hi"]),
            "scramjet-rs: no session found matching \"1999\""
        );
    }
    // Another project's is forked here only once the user agrees on a terminal.
    assert_eq!(
        run(&["--session", "1700", "-p", "hi"]),
        "scramjet-rs: session \"1700\" belongs to another project, /work/other; --fork \"1700\" \
continues it in a new session of this one"
    );
    let listed = || std::fs::read_dir(&here).unwrap().count();
    assert_eq!(listed(), 1);

    // The requests fail (nothing listens), after the prompt is saved in the session.
    let refused = "scramjet-rs: provider: cannot connect to 127.0.0.1:9";
    assert!(run(&["--session", "16", "-p", "again"]).starts_with(refused));
    let saved = std::fs::read_to_string(&ours).unwrap();
    assert!(saved.contains("\"content\":\"again\""), "{saved}");
    assert!(run(&["--fork", "1700", "-p", "more"]).starts_with(refused));
    assert_eq!(listed(), 2);
    let forked = (std::fs::read_dir(&here).unwrap())
        .map(|entry| entry.unwrap().path())
        .find(|path| *path != ours)
        .unwrap();
    let text = std::fs::read_to_string(forked).unwrap();
    let (header, lines) = text.split_once('\n').unwrap();
    assert!(lines.starts_with(theirs), "{text}");
    assert!(lines.contains("\"content\":\"more\""), "{text}");
    let parent = format!("\"parentSession\":{:?}", source.to_str().unwrap());
    let root = format!("\"root\":{:?}", project.to_str().unwrap());
    assert!(
        header.contains(&parent) && header.contains(&root),
        "{header}"
    );
    assert!(!std::fs::read_to_string(&source).unwrap().contains("more"));
}

#[test]
fn no_session_saves_none_and_its_directory_goes_at_exit() {
    let binary = env!("CARGO_BIN_EXE_scramjet-rs");
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-unsaved-{}", std::process::id())));
    let (project, temporary) = (scratch.0.join("project"), scratch.0.join("tmp"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&temporary).unwrap();
    let command = |endpoint: &str| {
        provide(&scratch.0.join("config"), endpoint);
        let mut command = Command::new(binary);
        command
            .args(["--retries", "0"])
            .args(["--no-session", "-p", "hi"])
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
            .env("SCRAMJET_CODING_AGENT_DIR", scratch.0.join("agent"))
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            // Where the run keeps its sessions, so that this test finds its own.
            .env("TMPDIR", &temporary)
            .stdin(std::process::Stdio::null());
        command
    };
    let output = command("127.0.0.1:9").output().unwrap();
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot connect to 127.0.0.1:9"), "{error}");
    assert_eq!(std::fs::read_dir(&temporary).unwrap().count(), 0);
    assert!(!scratch.0.join("state/scramjet-rs/sessions").exists());

    // Accepts connections into its backlog and never answers, so the request waits.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let hanging = silent.local_addr().unwrap().to_string();
    for signal in [libc::SIGTERM, libc::SIGHUP] {
        let mut child = command(&hanging)
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        let directory = loop {
            let found = std::fs::read_dir(&temporary).unwrap().flatten().next();
            let files = found
                .as_ref()
                .and_then(|entry| std::fs::read_dir(entry.path()).ok());
            let texts = (files.into_iter().flatten().flatten())
                .filter_map(|file| std::fs::read_to_string(file.path()).ok());
            if texts
                .into_iter()
                .any(|text| text.contains("\"content\":\"hi\""))
            {
                break found.unwrap().path();
            }
            assert!(
                started.elapsed().as_secs() < 20,
                "no session holds the prompt"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        // SAFETY: a plain kill(2) of the child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        // As a shell reports a process the signal killed, and as Scramjet's print mode exits.
        assert_eq!(child.wait().unwrap().code(), Some(128 + signal));
        assert!(!directory.exists(), "{}", directory.display());
    }
    drop(silent);
}

#[cfg(target_os = "linux")]
#[test]
fn prompt_editor_and_running_turns_keep_input_and_restore_the_terminal() {
    // A guard against a hang: the journeys take about 150 seconds.
    let output = Command::new("timeout")
        .args([
            "240",
            "python3",
            "tests/agent_editor_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_copys_owner_serves_until_another_takes_the_selection() {
    let selection = format!("SCRAMJET_RS_TEST_{}_owner", std::process::id());
    let start = |text: &str| -> (Child, String) {
        let mut owner = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(["__x11-selection-owner", &selection])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        owner
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        let mut said = String::new();
        let stdout = owner.stdout.take().unwrap();
        BufReader::new(stdout).read_line(&mut said).unwrap();
        (owner, said)
    };
    let (mut first, said) = start("first");
    if said != "ok\n" {
        eprintln!("no X server reached ({said:?}): the X11 owner is not tested");
        let _ = first.wait();
        return;
    }
    assert_eq!(first.try_wait().unwrap(), None);
    let (mut second, said) = start("second");
    assert_eq!(said, "ok\n");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match first.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            None => panic!("the first owner kept serving"),
        }
    };
    assert!(status.success());
    second.kill().unwrap();
    second.wait().unwrap();
}
