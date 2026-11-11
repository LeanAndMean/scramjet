use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// The binary under test: `SCRAMJET_RS_BINARY` where the build's own path does not run, as
/// on Windows from a build made in Linux (`scripts/test-windows.sh`).
fn binary() -> std::path::PathBuf {
    std::env::var_os("SCRAMJET_RS_BINARY")
        .map_or_else(|| env!("CARGO_BIN_EXE_scramjet").into(), Into::into)
}

/// `binary` to run with the user's directories in a scratch one of the tests', so that no test
/// reads or moves the user's own; a test's own `env` replaces them.
fn program(binary: &std::path::Path) -> Command {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    let home = HOME.get_or_init(|| {
        let home = std::env::temp_dir().join("scramjet-cli-tests-home");
        std::fs::create_dir_all(home.join("runtime")).unwrap();
        home
    });
    let mut command = Command::new(binary);
    for (variable, directory) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_RUNTIME_DIR", "runtime"),
    ] {
        command.env(variable, home.join(directory));
    }
    command
}

/// This repository: the `CARGO_MANIFEST_DIR` the tests run with, else the build's.
fn repository() -> std::path::PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| env!("CARGO_MANIFEST_DIR").into(), Into::into)
}

#[test]
fn a_closed_stdout_ends_the_output_quietly() {
    let binary = &binary();
    for arguments in [&["--help"][..], &["--version"], &["lint", "--help"]] {
        let mut child = program(binary)
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{arguments:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{arguments:?}: {output:?}");
    }
}

#[test]
fn help_shows_the_agents_usage_and_other_words_are_refused() {
    let binary = &binary();
    for help in ["--help", "-h"] {
        let output = program(binary).arg(help).output().unwrap();
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.starts_with("Usage: scramjet [--project DIR]"),
            "{text}"
        );
    }
    for word in ["--bogus", "-x"] {
        let output = program(binary).arg(word).output().unwrap();
        assert!(!output.status.success());
        let text = String::from_utf8(output.stderr).unwrap();
        assert!(
            text.starts_with(&format!("scramjet: unknown argument \"{word}\"")),
            "{text}"
        );
        assert!(text.contains("Usage: scramjet"), "{text}");
    }
    let refused = |arguments: &[&str]| {
        let output = program(binary).args(arguments).output().unwrap();
        assert!(!output.status.success());
        String::from_utf8(output.stderr).unwrap()
    };
    assert!(refused(&["-p"]).starts_with("scramjet: -p needs a message or an @FILE\n"));
    assert!(refused(&["-"]).starts_with("scramjet: the message - (read from stdin) needs -p\n"));
    // Before a session is started.
    let missing = refused(&["-p", "@/nonexistent/notes.md"]);
    let absolute = std::path::absolute("/nonexistent/notes.md").unwrap();
    assert_eq!(
        missing,
        format!("scramjet: file not found: {}\n", absolute.display())
    );
    for option in ["--thinking", "--effort"] {
        let output = program(binary).args([option, "hgih"]).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!(
                "scramjet: invalid {option} \"hgih\": valid levels are off, minimal, low, \
medium, high, xhigh, max\n"
            )
        );
    }
    for version in ["--version", "-v"] {
        let output = program(binary).arg(version).output().unwrap();
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
        let output = program(&binary())
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
    let output = program(&binary())
        .args(["-p", "hello"])
        .env("XDG_CONFIG_HOME", scratch.0.join("config"))
        .env("XDG_STATE_HOME", scratch.0.join("state"))
        .env("SCRAMJET_OFFLINE", "1")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "scramjet: no provider offers any model: start scramjet and /login, or add providers \
to {} (see --help)\n",
            (scratch
                .0
                .join("config")
                .join("scramjet")
                .join("models.json"))
            .display()
        )
    );
    assert!(output.stdout.is_empty());
    assert!(
        !scratch
            .0
            .join("state")
            .join("scramjet")
            .join("sessions")
            .exists()
    );
    provide(&scratch.0.join("config"), "127.0.0.1:9");
    let file = scratch
        .0
        .join("config")
        .join("scramjet")
        .join("models.json");
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
    std::fs::write(config.join("scramjet/auth.json"), "{}").unwrap();
    let file = config.join("scramjet/models.json");
    let mut models: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    models["credentialFiles"] = serde_json::json!(["~/.fake", "~/.ssh/inside", "~/absent"]);
    std::fs::write(&file, models.to_string()).unwrap();
    let package_models = package.join("models.json");
    std::fs::write(&package_models, r#"{"credentialFiles": ["token"]}"#).unwrap();
    let settings = serde_json::json!({"commandSets": [package]});
    std::fs::write(config.join("scramjet/settings.json"), settings.to_string()).unwrap();
    let list = || {
        let output = program(&binary())
            .arg("--list-credential-stores")
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env_remove("SCRAMJET_CODING_AGENT_DIR")
            .output()
            .unwrap();
        assert!(output.status.success());
        let text = |bytes| String::from_utf8(bytes).unwrap();
        (text(output.stdout), text(output.stderr))
    };
    let real = |path: std::path::PathBuf| std::fs::canonicalize(path).unwrap();
    let listed: Vec<String> = [config.join("scramjet/auth.json"), home.join(".fake")]
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
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://{endpoint}/v1"), "api": "openai-responses",
        "models": [{"id": "gpt-6-luna", "reasoning": true, "input": ["text", "image"]}],
    }}});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
}

#[test]
fn https_providers_are_reached_over_tls_and_untrusted_certificates_refused_once() {
    let binary = &binary();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-https-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let tls = repository().join("tests").join("tls");
    let (port, connections, heads) = serve_tls(&tls);
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("https://localhost:{port}"), "api": "anthropic-messages",
        "models": [{"id": "claude-test"}],
    }}});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
    // As if the system trusted only the certificates in `trusted`.
    let run = |trusted: &str| {
        program(binary)
            .args(["--retries", "2", "--no-session", "-p", "hi"])
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", &config)
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
            "scramjet: provider: TLS handshake with localhost:{port} (provider test) failed: \
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
    let binary = &binary();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-sessions-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    // As the program takes it from the working directory, and names its sessions' directory.
    let project = scramjet::os::canonicalize(&project).unwrap();
    let separators: &[char] = match cfg!(windows) {
        true => &['/', '\\', ':'],
        false => &['/'],
    };
    let sessions = scratch.0.join("state").join("scramjet").join("sessions");
    let slug = project
        .to_str()
        .unwrap()
        .trim_matches(separators)
        .replace(separators, "-");
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
        let output = program(binary)
            .args(["--retries", "0"])
            .args(arguments)
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        error.lines().last().unwrap_or_default().to_owned()
    };
    let first_line = |arguments: &[&str]| {
        let output = program(binary).args(arguments).output().unwrap();
        let error = String::from_utf8(output.stderr).unwrap();
        error.lines().next().unwrap_or_default().to_owned()
    };
    assert_eq!(
        first_line(&["--fork", "x", "-c"]),
        "scramjet: --fork cannot be combined with --continue"
    );
    assert_eq!(
        first_line(&["-r", "--session", "a", "--fork", "b", "-c"]),
        "scramjet: --fork cannot be combined with --session, --continue, --resume"
    );
    for option in ["--session", "--fork"] {
        assert_eq!(
            run(&[option, "1999", "-p", "hi"]),
            "scramjet: no session found matching \"1999\""
        );
    }
    // Another project's is forked here only once the user agrees on a terminal.
    assert_eq!(
        run(&["--session", "1700", "-p", "hi"]),
        "scramjet: session \"1700\" belongs to another project, /work/other; --fork \"1700\" \
continues it in a new session of this one"
    );
    // Sessions, not Windows' `.lock` (`os::lock_directory`).
    let sessions = || {
        (std::fs::read_dir(&here).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
    };
    let listed = || sessions().count();
    assert_eq!(listed(), 1);

    // The requests fail (nothing listens), after the prompt is saved in the session.
    let refused = "scramjet: provider: cannot connect to 127.0.0.1:9";
    let said = run(&["--session", "16", "-p", "again"]);
    assert!(said.starts_with(refused), "{said}");
    let saved = std::fs::read_to_string(&ours).unwrap();
    assert!(saved.contains("\"content\":\"again\""), "{saved}");
    assert!(run(&["--fork", "1700", "-p", "more"]).starts_with(refused));
    assert_eq!(listed(), 2);
    let forked = sessions().find(|path| *path != ours).unwrap();
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
fn a_chains_session_resumed_without_a_terminal_shows_the_earlier_ones_first() {
    let binary = &binary();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-chain-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let slug = (project.to_str().unwrap().trim_matches('/')).replace('/', "-");
    let here = scratch.0.join("state/scramjet/sessions").join(slug);
    std::fs::create_dir_all(&here).unwrap();
    let report = |summary: &str| {
        let arguments = format!("{{\"status\":\"completed\",\"summary\":{summary:?}}}");
        format!(
            "{{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\
\"report_scramjet_command_status\",\"arguments\":{arguments:?}}}\n"
        )
    };
    let mut parent: Option<std::path::PathBuf> = None;
    for (at, (prompt, summary)) in [
        ("/first", Some("Did the first.")),
        ("/second", None),
        ("third", None),
    ]
    .into_iter()
    .enumerate()
    {
        let path = here.join(format!("160000000{at}000-1.jsonl"));
        let link = (parent.as_ref())
            .map(|parent| format!(",\"parentSession\":{:?},\"nextStep\":true", parent))
            .unwrap_or_default();
        let mut text = format!(
            "{{\"type\":\"session\",\"version\":1,\"root\":{:?}{link}}}\n\
{{\"role\":\"user\",\"content\":{prompt:?}}}\n",
            project.to_str().unwrap()
        );
        text += &summary.map(report).unwrap_or_default();
        std::fs::write(&path, text).unwrap();
        parent = Some(path);
    }
    provide(&scratch.0.join("config"), "127.0.0.1:9");
    let output = program(binary)
        .arg("--session")
        .arg(parent.unwrap())
        .current_dir(&project)
        .env("XDG_STATE_HOME", scratch.0.join("state"))
        .env("XDG_CONFIG_HOME", scratch.0.join("config"))
        .env("SCRAMJET_CACHE", scratch.0.join("cache"))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let error = String::from_utf8(output.stderr).unwrap();
    let at = |text: &str| {
        error
            .find(text)
            .unwrap_or_else(|| panic!("{text}: {error}"))
    };
    assert!(at("[session] /first") < at("Did the first."), "{error}");
    assert!(at("Did the first.") < at("[session] /second"), "{error}");
    assert!(at("[session] /second") < at("> third"), "{error}");
}

#[test]
fn session_dir_saves_and_finds_sessions_there_whatever_their_project() {
    let binary = &binary();
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-session-dir-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config = scratch.0.join("config");
    provide(&config, "127.0.0.1:9");
    let run_all = |arguments: &[&str]| {
        let output = program(binary)
            .args(["--retries", "0"])
            .args(arguments)
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", &config)
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        String::from_utf8(output.stderr).unwrap()
    };
    let run = |arguments: &[&str]| {
        run_all(arguments)
            .lines()
            .last()
            .unwrap_or_default()
            .to_owned()
    };
    let sessions = |directory: &std::path::Path| -> Vec<std::path::PathBuf> {
        let listed = std::fs::read_dir(directory).into_iter().flatten().flatten();
        (listed.map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
            .collect()
    };
    let read = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();
    let refused = "scramjet: provider: cannot connect to 127.0.0.1:9";
    let projects = scratch.0.join("state/scramjet/sessions");

    // Relative to the current directory, as Scramjet's.
    let custom = project.join("custom");
    assert!(run(&["--session-dir", "custom", "-p", "first"]).starts_with(refused));
    let [saved] = sessions(&custom).try_into().unwrap();
    assert!(read(&saved).contains("\"content\":\"first\""));
    assert!(!projects.exists());
    assert!(run(&["--session-dir", "custom", "-c", "-p", "again"]).starts_with(refused));
    assert_eq!(sessions(&custom), std::slice::from_ref(&saved));
    assert!(read(&saved).contains("\"content\":\"again\""));

    // Another project's session there is this one's to resume, not to fork.
    let theirs = custom.join("1700000000000-7.jsonl");
    std::fs::write(
        &theirs,
        "{\"type\":\"session\",\"version\":1,\"root\":\"/work/other\"}\n\
{\"role\":\"user\",\"content\":\"one\"}\n",
    )
    .unwrap();
    assert!(
        run(&["--session-dir", "custom", "--session", "1700", "-p", "more"]).starts_with(refused)
    );
    assert!(read(&theirs).contains("\"content\":\"more\""));
    assert_eq!(sessions(&custom).len(), 2);

    // The settings' sessionDir, which the option replaces.
    let settings = config.join("scramjet/settings.json");
    let elsewhere = scratch.0.join("elsewhere");
    let setting = serde_json::json!({"sessionDir": elsewhere});
    std::fs::write(&settings, setting.to_string()).unwrap();
    assert!(run(&["-p", "set"]).starts_with(refused));
    let [set] = sessions(&elsewhere).try_into().unwrap();
    assert!(read(&set).contains("\"content\":\"set\""));
    assert!(run(&["--session-dir", "custom", "-p", "option"]).starts_with(refused));
    assert_eq!(sessions(&elsewhere).len(), 1);
    assert_eq!(sessions(&custom).len(), 3);
    // Found elsewhere, in the projects' directories, a session is another project's.
    let other = projects.join("work-other").join("1800000000000-7.jsonl");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::copy(&theirs, &other).unwrap();
    assert_eq!(
        run(&["--session", "1800", "-p", "hi"]),
        "scramjet: session \"1800\" belongs to another project, /work/other; --fork \"1800\" \
continues it in a new session of this one"
    );
    std::fs::write(&settings, r#"{"sessionDir": 7}"#).unwrap();
    let output = run_all(&["-p", "wrong"]);
    assert!(
        output.contains("sessionDir must be a directory; ignored"),
        "{output}"
    );
    assert_eq!(sessions(&elsewhere).len(), 1);
}

#[test]
fn no_session_saves_none_and_its_directory_goes_at_exit() {
    let binary = &binary();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-unsaved-{}", std::process::id())));
    let (project, temporary) = (scratch.0.join("project"), scratch.0.join("tmp"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&temporary).unwrap();
    let command = |endpoint: &str| {
        provide(&scratch.0.join("config"), endpoint);
        let mut command = program(binary);
        command
            .args(["--retries", "0"])
            .args(["--no-session", "-p", "hi"])
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
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
    assert!(!scratch.0.join("state/scramjet/sessions").exists());

    // Accepts connections into its backlog and never answers, so the request waits.
    #[cfg(unix)]
    {
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
}

#[cfg(target_os = "linux")]
#[test]
fn prompt_editor_and_running_turns_keep_input_and_restore_the_terminal() {
    // A guard against a hang: the journeys take about 180 seconds.
    let output = Command::new("timeout")
        .args([
            "300",
            "python3",
            "tests/agent_editor_pty.py",
            env!("CARGO_BIN_EXE_scramjet"),
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
        let mut owner = program(&binary())
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

#[test]
fn packages_from_git_are_cloned_listed_updated_and_removed() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("passed over: git is not here");
        return;
    }
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-packages-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&scratch.0);
    let (work, project) = (scratch.0.join("work"), scratch.0.join("project"));
    std::fs::create_dir_all(&project).unwrap();
    let git = |directory: &std::path::Path, arguments: &[&str]| {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-C",
            ])
            .arg(directory)
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {arguments:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let file = |path: &str, text: &str| {
        let path = work.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let commit = |message: &str| {
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "--quiet", "-m", message]);
    };
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet"]);
    file("README.md", "A kit\n");
    file("packages/kit/mach12/set.yaml", "name: mach12\n");
    file("packages/kit/mach12/commands/hello.md", "Say hello\n");
    file("packages/kit/tests/fixture/commands/x.md", "Not a set\n");
    file(
        "providers/models.json",
        r#"{"providersCommand": ["bin/models", "--all"]}"#,
    );
    commit("First");
    let first = git(&work, &["rev-parse", "--short", "HEAD"]);
    let upstream = scratch.0.join("upstream.git");
    git(
        &scratch.0,
        &["clone", "--quiet", "--bare", "work", "upstream.git"],
    );
    git(
        &work,
        &["remote", "add", "origin", upstream.to_str().unwrap()],
    );
    let branch = git(&work, &["rev-parse", "--abbrev-ref", "HEAD"]);

    // A plain `update` updates the program too: a release list whose newest release is older.
    let listing = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let releases = format!("http://{}/releases", listing.local_addr().unwrap());
    std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        for stream in listing.incoming() {
            let mut stream = stream.unwrap();
            let reader = std::io::BufReader::new(stream.try_clone().unwrap());
            for line in reader.lines() {
                if line.unwrap().is_empty() {
                    break;
                }
            }
            let body = r#"[{"tag_name": "v0.0.0", "assets": []}]"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    let up_to_date = format!(
        "scramjet {} is up to date (newest release v0.0.0)\n",
        env!("CARGO_PKG_VERSION")
    );
    let run = |arguments: &[&str]| {
        let output = program(&binary())
            .args(arguments)
            .current_dir(&project)
            .env("SCRAMJET_RELEASES", &releases)
            .env("HOME", &scratch.0)
            .env("USERPROFILE", &scratch.0)
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
            .env("XDG_DATA_HOME", scratch.0.join("data"))
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env_remove("SCRAMJET_CACHE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = |bytes| String::from_utf8(bytes).unwrap();
        (
            output.status.success(),
            text(output.stdout),
            text(output.stderr),
        )
    };
    // Git for Windows takes file:///C:/path.
    let url = |path: &std::path::Path| {
        format!(
            "file://{}{}",
            if cfg!(windows) { "/" } else { "" },
            path.display().to_string().replace('\\', "/")
        )
    };
    let source = url(&upstream);
    let clones = scratch.0.join("data/scramjet/git");
    let within = scratch.0.display().to_string().replace(':', "");
    let clone = clones
        .join("localhost")
        .join(within.trim_start_matches(['/', '\\']))
        .join("upstream");
    // A path as the program shows it: with `\` on Windows.
    let native = |path: &str| match cfg!(windows) {
        true => path.replace('/', "\\"),
        false => path.to_owned(),
    };
    let shown_clone = native(&format!(
        "~/{}",
        clone.strip_prefix(&scratch.0).unwrap().display()
    ));
    let settings = scratch.0.join("config/scramjet/settings.json");
    let packages = || {
        let text = std::fs::read_to_string(&settings).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["packages"].clone()
    };

    let kit = native("packages/kit/mach12");
    let (ok, out, err) = run(&["install", &source]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        out,
        format!(
            "Cloning {source} into {shown_clone}\nInstalled {source} from {shown_clone}:\n  \
mach12 ({kit}): 1 command, 0 agents\n  providers (providers): 0 commands, \
0 agents, and providers (models.json)\nIt runs at each start:\n  bin/models --all \
(providersCommand in {models}, run in its directory)\n",
            models = native("providers/models.json"),
        )
    );
    assert_eq!(packages(), serde_json::json!([source]));
    let (ok, out, _) = run(&["list"]);
    assert!(ok);
    assert!(
        out.contains(&format!(
            "mach12     {source}  {shown_clone}{}  1 command",
            native("/packages/kit/mach12")
        )),
        "{out}"
    );
    assert!(!out.contains("fixture"), "{out}");
    assert!(out.contains("(commandSets, packages)"), "{out}");

    let (ok, _, err) = run(&["install", &source]);
    assert!(!ok);
    assert_eq!(err, format!("scramjet: {source} is already installed\n"));
    let pinned = format!("{source}@{first}");
    let (ok, _, err) = run(&["install", &pinned]);
    assert!(!ok);
    let identity = url(&scratch.0.join("upstream")).replacen("file://", "git:localhost", 1);
    assert_eq!(
        err,
        format!("scramjet: {identity} is installed as {source}; remove it first\n")
    );
    let (ok, _, err) = run(&["remove", "mach12"]);
    assert!(!ok);
    assert!(
        err.contains(&format!(
            "mach12 came with {source}; `scramjet remove {source}`"
        )),
        "{err}"
    );

    // An update pulls what was pushed and keeps what was changed in the clone.
    file("packages/kit/mach12/commands/second.md", "Again\n");
    commit("Second");
    let second = git(&work, &["rev-parse", "--short", "HEAD"]);
    git(&work, &["push", "--quiet", "origin", &branch]);
    std::fs::write(clone.join("README.md"), "Changed here\n").unwrap();
    let (ok, out, err) = run(&["update"]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        out,
        format!("Updated {source}: {first}..{second}\n{up_to_date}")
    );
    assert_eq!(
        std::fs::read_to_string(clone.join("README.md")).unwrap(),
        "Changed here\n"
    );
    let (ok, out, _) = run(&["update", "mach12"]);
    assert!(ok);
    assert_eq!(out, format!("{source}: up to date ({second})\n"));
    assert!(run(&["list"]).1.contains("2 commands"));

    // A clone gone is said at start and cloned again by an update.
    std::fs::remove_dir_all(&clone).unwrap();
    let (ok, _, err) = run(&["list"]);
    assert!(ok);
    assert!(
        err.contains(&format!(
            "{source}: not cloned into {shown_clone}; `scramjet update` clones it"
        )),
        "{err}"
    );
    let (ok, out, _) = run(&["update", &source]);
    assert!(ok);
    assert_eq!(out, format!("Cloned {source} into {shown_clone}\n"));

    // A session's start clones it too, as Scramjet's does, unless SCRAMJET_OFFLINE is set or
    // --offline given.
    let start = |offline: &str| {
        let output = program(&binary())
            .args(["-p", "--no-session", "--model", "nowhere/none", "hi"])
            .args((offline == "--offline").then_some("--offline"))
            .current_dir(&project)
            .env("HOME", &scratch.0)
            .env("USERPROFILE", &scratch.0)
            .env("XDG_CONFIG_HOME", scratch.0.join("config"))
            .env("XDG_DATA_HOME", scratch.0.join("data"))
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("SCRAMJET_OFFLINE", offline)
            .env_remove("PI_OFFLINE")
            .env_remove("SCRAMJET_CACHE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        String::from_utf8(output.stderr).unwrap()
    };
    std::fs::remove_dir_all(&clone).unwrap();
    for offline in ["1", "--offline"] {
        let err = start(offline);
        assert!(!err.contains("Cloning"), "{err}");
        assert!(!err.contains("unknown argument"), "{err}");
        assert!(!clone.exists());
    }
    let err = start("");
    assert!(
        err.starts_with(&format!("Cloning {source} into {shown_clone}…\n")),
        "{err}"
    );
    assert!(!err.contains("not cloned"), "{err}");
    assert!(clone.join(".git").exists());

    let (ok, out, err) = run(&["remove", &source]);
    assert!(ok, "{err}");
    assert_eq!(
        out,
        format!("Removed {source} and its clone in {shown_clone}\n")
    );
    assert!(!clone.exists());
    assert_eq!(std::fs::read_dir(&clones).unwrap().count(), 0);
    assert_eq!(packages(), serde_json::json!([]));

    // A ref pins a package.
    let (ok, out, err) = run(&["install", &pinned]);
    assert!(ok, "{out}{err}");
    assert!(out.contains(&format!("mach12 ({kit}): 1 command")), "{out}");
    let (ok, out, _) = run(&["update"]);
    assert!(ok);
    assert_eq!(
        out,
        format!("{pinned}: pinned to {first}; left as it is\n{up_to_date}")
    );
    assert!(run(&["uninstall", &source]).0);

    // Clones do not nest, so that removing one does not delete another.
    git(
        &scratch.0,
        &["clone", "--quiet", "--bare", "work", "upstream/inner.git"],
    );
    let inner = url(&scratch.0.join("upstream/inner.git"));
    assert!(run(&["install", &source]).0);
    let (ok, _, err) = run(&["install", &inner]);
    assert!(!ok);
    assert_eq!(
        err,
        format!(
            "scramjet: {inner} would be cloned into {shown_clone}{}, which nests with the \
             clone of {source}; remove that first\n",
            native("/inner")
        )
    );
    assert!(!clone.join("inner").exists());
    std::fs::write(
        &settings,
        serde_json::json!({ "packages": [&source, &inner] }).to_string(),
    )
    .unwrap();
    assert!(run(&["update", &inner]).0);
    let (ok, _, err) = run(&["remove", &source]);
    assert!(!ok);
    assert_eq!(
        err,
        format!(
            "scramjet: {source}'s clone in {shown_clone} holds the clone of {inner}; remove \
             that first\n"
        )
    );
    assert!(clone.join("inner/.git").exists());
    assert!(run(&["remove", &inner]).0);
    assert!(run(&["remove", &source]).0);

    // A directory in a clone's place that is not one is neither used nor pulled.
    std::fs::create_dir_all(clone.join("notes")).unwrap();
    let (ok, _, err) = run(&["install", &source]);
    assert!(!ok);
    assert_eq!(
        err,
        format!("scramjet: {shown_clone} exists and is not a clone; left as it is\n")
    );
    std::fs::write(
        &settings,
        serde_json::json!({ "packages": [&source] }).to_string(),
    )
    .unwrap();
    let (ok, _, err) = run(&["update"]);
    assert!(!ok);
    assert!(
        err.contains(&format!(
            "{source}: {} is not a clone",
            native(&clone.display().to_string())
        )),
        "{err}"
    );
    std::fs::write(&settings, r#"{"packages": []}"#).unwrap();
    std::fs::remove_dir_all(clones.join("localhost")).unwrap();

    // A repository without a set, or one that cannot be cloned, leaves nothing.
    let plain = scratch.0.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    git(&plain, &["init", "--quiet"]);
    std::fs::write(plain.join("notes.md"), "Notes\n").unwrap();
    git(&plain, &["add", "-A"]);
    git(&plain, &["commit", "--quiet", "-m", "Notes"]);
    let plain = url(&plain);
    let (ok, _, err) = run(&["install", &plain]);
    assert!(!ok);
    assert!(err.ends_with(&format!("scramjet: {plain} holds no command set (no commands, agents or models.json at its top or in its top directories, and no set.yaml deeper) and no skills or themes directory\n")), "{err}");
    let missing = url(&scratch.0.join("missing.git"));
    let (ok, _, err) = run(&["install", &missing]);
    assert!(!ok);
    assert!(err.ends_with("scramjet: git clone failed\n"), "{err}");
    assert_eq!(std::fs::read_dir(&clones).unwrap().count(), 0);
    assert_eq!(packages(), serde_json::json!([]));

    // A local directory holding sets is a package, as Pi takes a local path: its sets, skills
    // and themes, and sets installed from it before move into it.
    let command_sets = || {
        let text = std::fs::read_to_string(&settings).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["commandSets"].clone()
    };
    file(
        "skills/greeting/SKILL.md",
        "---\nname: greeting\ndescription: Greets.\n---\nSay hi.\n",
    );
    let dark = repository().join("src/agent_cli/themes/scramjet-dark.json");
    file(
        "themes/scramjet-dark.json",
        &std::fs::read_to_string(dark).unwrap(),
    );
    let providers = work.join("providers");
    let (ok, _, err) = run(&["install", providers.to_str().unwrap()]);
    assert!(ok, "{err}");
    let (ok, out, err) = run(&["install", work.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    assert_eq!(
        out,
        native(
            "Installed the package in ~/work:\n  mach12 (packages/kit/mach12): 2 commands, 0 \
agents\n  providers (providers): 0 commands, 0 agents, and providers (models.json)\n  skills: 1 \
skill (greeting)\n  themes: 1 theme (scramjet-dark)\nIt now brings providers, taken out of \
commandSets.\n"
        )
    );
    assert_eq!(command_sets(), serde_json::json!([]));
    assert_eq!(packages(), serde_json::json!([native("~/work")]));
    let (ok, _, err) = run(&["install", work.to_str().unwrap()]);
    assert!(!ok);
    assert!(
        err.ends_with(&native("~/work is already installed\n")),
        "{err}"
    );
    // A set that appears later comes with it.
    file("tools/commands/x.md", "X\n");
    let (ok, out, err) = run(&["list"]);
    assert!(ok, "{err}");
    assert!(
        out.contains(&native("tools      ~/work    ~/work/tools")),
        "{out}"
    );
    assert!(
        out.contains(&native(
            "greeting                   ~/work/skills/greeting/SKILL.md"
        )),
        "{out}"
    );
    assert!(out.contains("\nscramjet   built in  "), "{out}");
    assert!(!out.contains("Warnings"), "{out}");
    let (ok, _, err) = run(&["remove", "mach12"]);
    assert!(!ok);
    assert!(
        err.ends_with(&native(
            "mach12 came with ~/work; `scramjet remove ~/work` removes it with the package's \
other command sets\n"
        )),
        "{err}"
    );
    let (ok, out, err) = run(&["remove", work.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    assert_eq!(out, native("Removed the package in ~/work\n"));
    assert_eq!(packages(), serde_json::json!([]));
    std::fs::remove_dir_all(work.join("skills")).unwrap();
    std::fs::remove_dir_all(work.join("themes")).unwrap();
    let notes = scratch.0.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let (ok, _, err) = run(&["install", notes.to_str().unwrap()]);
    assert!(!ok);
    assert!(
        err.ends_with(&format!(
            "scramjet: {} is not a command set: it has no commands or agents directory, nor \
a models.json, and holds no set (none in its top directories, nor one with a set.yaml deeper) \
and no skills or themes directory\n",
            notes.display()
        )),
        "{err}"
    );
}

#[test]
#[cfg(unix)]
fn shell_path_and_shell_command_prefix_run_the_agents_commands() {
    let binary = &binary();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-shell-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let (port, bodies) = serve_bash_call("greet");
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://127.0.0.1:{port}"), "api": "anthropic-messages",
        "models": [{"id": "claude-test"}],
    }}});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
    let marker = scratch.0.join("ran");
    let shell = scratch.0.join("shell");
    let script = format!(
        "#!/bin/sh\necho \"$@\" > '{}'\nexec bash \"$@\"\n",
        marker.display()
    );
    std::fs::write(&shell, script).unwrap();
    std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let run = |settings: serde_json::Value| {
        let file = config.join("scramjet/settings.json");
        std::fs::write(file, settings.to_string()).unwrap();
        let output = program(binary)
            .args(["--retries", "0", "--no-session", "-p", "hi"])
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", &config)
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{stderr}");
        let _first = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
        let second = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
        let second: serde_json::Value = serde_json::from_str(&second).unwrap();
        let messages = second["messages"].as_array().unwrap();
        let result = &messages.last().unwrap()["content"][0];
        assert_eq!(result["type"], "tool_result", "{second}");
        (result["content"].to_string(), stderr)
    };
    let prefix = "shopt -s expand_aliases\nalias greet='echo hello from the prefix'";
    // In a turn nobody watches, the commands run with the credential stores hidden; without.
    for hide in ["unattended", "never"] {
        let _ = std::fs::remove_file(&marker);
        let (result, stderr) = run(serde_json::json!({
            "shellPath": shell, "shellCommandPrefix": prefix, "hideCredentials": hide,
        }));
        if hide == "unattended" && result.contains("bwrap") {
            eprintln!("passed over with the stores hidden: {result}");
            continue;
        }
        assert!(
            result.contains("hello from the prefix"),
            "{result} {stderr}"
        );
        let ran = std::fs::read_to_string(&marker).unwrap();
        assert!(ran.starts_with("-c "), "{ran}");
    }
    // In the sandbox, whose /tmp is its own, so that the scratch shell is not there.
    let (result, stderr) = run(serde_json::json!({
        "shellPath": "/bin/bash", "shellCommandPrefix": prefix, "sandbox": "workspace-write",
    }));
    if result.contains("bwrap") || stderr.contains("bwrap") {
        eprintln!("passed over in the sandbox: {result} {stderr}");
    } else {
        assert!(
            result.contains("hello from the prefix"),
            "{result} {stderr}"
        );
    }
    // A shellPath that is not there is said in the call's result.
    let (result, _) = run(serde_json::json!({
        "shellPath": scratch.0.join("missing"), "hideCredentials": "never",
    }));
    assert!(
        result.contains("shellPath") && result.contains("is not a file"),
        "{result}"
    );
    let (result, stderr) = run(serde_json::json!({"shellPath": 7, "hideCredentials": "never"}));
    assert!(result.contains("command not found"), "{result}");
    assert!(
        stderr.contains("shellPath must be a program; ignored"),
        "{stderr}"
    );
}

#[test]
fn images_are_resized_sent_as_they_are_or_blocked_as_the_settings_say() {
    let binary = &binary();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-images-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let wide = image::RgbImage::from_fn(2100, 10, |x, y| image::Rgb([x as u8, y as u8, 7]));
    let mut png = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut png),
        &wide,
        2100,
        10,
        image::ExtendedColorType::Rgb8,
    )
    .unwrap();
    std::fs::write(project.join("wide.png"), &png).unwrap();
    let (port, bodies) = serve_call("read", serde_json::json!({"path": "wide.png"}));
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://127.0.0.1:{port}"), "api": "anthropic-messages",
        "models": [{"id": "claude-test", "input": ["text", "image"]}],
    }}});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
    // The read call's result, as the second request sends it.
    let run = |images: serde_json::Value| {
        let file = config.join("scramjet/settings.json");
        std::fs::write(file, serde_json::json!({"images": images}).to_string()).unwrap();
        let output = program(binary)
            .args(["--retries", "0", "--no-session", "-p", "hi"])
            .current_dir(&project)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("XDG_CONFIG_HOME", &config)
            .env("SCRAMJET_CACHE", scratch.0.join("cache"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{stderr}");
        assert!(!stderr.contains("images"), "{stderr}");
        let _first = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
        let second = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
        let second: serde_json::Value = serde_json::from_str(&second).unwrap();
        let messages = second["messages"].as_array().unwrap();
        let result = messages.last().unwrap()["content"][0].clone();
        assert_eq!(result["type"], "tool_result", "{second}");
        match &result["content"] {
            serde_json::Value::String(text) => {
                vec![serde_json::json!({"type": "text", "text": text})]
            }
            content => content.as_array().unwrap().clone(),
        }
    };
    let image_data = |content: &[serde_json::Value]| {
        let image = content.iter().find(|part| part["type"] == "image").unwrap();
        image["source"]["data"].as_str().unwrap().to_owned()
    };
    let texts = |content: &[serde_json::Value]| {
        (content.iter())
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let decoded = |data: &str| {
        let value = |c: u8| match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        let digits: Vec<u8> = data.bytes().filter(|&c| c != b'=').map(value).collect();
        (digits.chunks(4))
            .flat_map(|chunk| {
                let bits = (chunk.iter().enumerate())
                    .fold(0u32, |bits, (i, &d)| bits | u32::from(d) << (18 - 6 * i));
                (0..chunk.len() - 1).map(move |i| (bits >> (16 - 8 * i)) as u8)
            })
            .collect::<Vec<u8>>()
    };
    // By default the image is scaled to 2000 pixels wide, and the model told so.
    let content = run(serde_json::json!({}));
    assert!(
        texts(&content).contains("original 2100x10, displayed at 2000x10"),
        "{content:?}"
    );
    assert_ne!(decoded(&image_data(&content)), png);
    // Without resizing it goes as read.
    let content = run(serde_json::json!({"autoResize": false}));
    assert_eq!(texts(&content), "Read image file [image/png]");
    assert_eq!(decoded(&image_data(&content)), png);
    // Blocked, no image goes, and the model is told why.
    let content = run(serde_json::json!({"blockImages": true}));
    assert!(
        content.iter().all(|part| part["type"] != "image"),
        "{content:?}"
    );
    assert_eq!(
        texts(&content),
        "Read image file [image/png]\n[Image: original 2100x10, displayed at 2000x10. Multiply \
coordinates by 1.05 to map to original image.]\nImage reading is disabled."
    );
}

/// A Messages provider on a port of 127.0.0.1 whose replies alternate between a bash call
/// running `command` and a short text: its port, and each request's body.
#[cfg(unix)]
fn serve_bash_call(command: &str) -> (u16, std::sync::mpsc::Receiver<String>) {
    serve_call("bash", serde_json::json!({"command": command}))
}

/// As `serve_bash_call`, with a call of the tool `name` with `input`.
fn serve_call(name: &str, input: serde_json::Value) -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sent, received) = std::sync::mpsc::channel();
    let input = input.to_string();
    let call = vec![
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#.to_owned(),
        serde_json::json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": name, "input": {}}}).to_string(),
        serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": input}}).to_string(),
        r#"{"type":"content_block_stop","index":0}"#.to_owned(),
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}"#.to_owned(),
        r#"{"type":"message_stop"}"#.to_owned(),
    ];
    let text = [
        r#"{"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    let reply = |events: &[String]| {
        let body: String = (events.iter())
            .map(|event| {
                let kind: serde_json::Value = serde_json::from_str(event).unwrap();
                let kind = kind["type"].as_str().unwrap().to_owned();
                format!("event: {kind}\ndata: {event}\n\n")
            })
            .collect();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\
connection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let replies = [reply(&call), reply(&text)];
    std::thread::spawn(move || {
        for (n, socket) in listener.incoming().enumerate() {
            let mut stream = socket.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut head, mut length) = (String::new(), 0);
            while let Ok(read) = reader.read_line(&mut head) {
                let line = &head[head.len() - read..];
                if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if read == 0 || line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            std::io::Read::read_exact(&mut reader, &mut body).unwrap();
            stream.write_all(replies[n % 2].as_bytes()).unwrap();
            let _ = sent.send(String::from_utf8(body).unwrap());
        }
    });
    (port, received)
}
