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
fn scramjet_1s_provider_option_and_others_are_taken_or_refused_saying_why() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-v1-{}", std::process::id())));
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    // Two providers offering gpt-6-luna, at closed ports that tell them apart.
    let models = r#"{"providers": {
        "test": {"baseUrl": "http://127.0.0.1:9/v1", "api": "openai-responses",
            "models": [{"id": "gpt-6-luna"}]},
        "remote": {"baseUrl": "http://127.0.0.1:7/v1", "api": "openai-responses",
            "models": [{"id": "gpt-6-luna"}, {"id": "far"}]}}}"#;
    std::fs::write(config.join("scramjet/models.json"), models).unwrap();
    let run = |arguments: &[&str]| {
        let output = program(&binary())
            .args(["-p", "--no-session", "--retries", "0"])
            .args(arguments)
            .arg("hi")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("SCRAMJET_OFFLINE", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        String::from_utf8(output.stderr).unwrap()
    };
    let reached = |port: u16, provider: &str| format!("127.0.0.1:{port} (provider {provider})");
    let said = run(&["--model", "gpt-6-luna"]);
    assert!(said.contains(&reached(9, "test")), "{said}");
    for given in [
        &["--provider", "remote", "--model", "gpt-6-luna"][..],
        &["--provider", "REMOTE", "--model", "remote/fa"],
        &[
            "--model",
            "gpt-6-luna",
            "--provider",
            "remote",
            "--verbose",
            "-ne",
            "-np",
        ],
        &[
            "--model",
            "far",
            "--mode",
            "text",
            "--no-extensions",
            "--no-prompt-templates",
        ],
    ] {
        let said = run(given);
        assert!(said.contains(&reached(7, "remote")), "{given:?}: {said}");
    }
    let said = run(&["--provider", "test"]);
    assert!(
        said.starts_with("--provider test chooses no model without --model; ignored\n"),
        "{said}"
    );
    let refused = [
        (
            &["--provider", "nowhere", "--model", "far"][..],
            "--provider \"nowhere\" is none of the providers (--list-models lists them)",
        ),
        (
            &["--provider", "test", "--model", "far"],
            "--provider \"test\" --model \"far\" names none of its models (--list-models lists them)",
        ),
        (
            &["--mode", "rpc"],
            "--mode rpc is not in Scramjet 2 yet; -p prints the replies as text",
        ),
        (
            &["--mode", "xml"],
            "invalid --mode \"xml\": valid modes are text, json and rpc",
        ),
    ];
    for (given, error) in refused {
        assert_eq!(run(given), format!("scramjet: {error}\n"), "{given:?}");
    }
    for given in [
        &["-e", "x.ts"][..],
        &["--extension", "x.ts"],
        &["--prompt-template", "p"],
        &["--model-definition", "{}"],
        &["--model-definition-env", "ROUTE"],
    ] {
        let said = run(given);
        assert!(
            said.starts_with(&format!("scramjet: {}", given[0])),
            "{said}"
        );
        assert!(said.contains("Scramjet 2"), "{said}");
        assert!(!said.contains("Usage"), "{said}");
    }
}

#[test]
fn api_key_is_sent_as_given_to_the_models_provider_for_the_run() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-api-key-{}", std::process::id())));
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let (port, requests) = serve_refusals();
    let models = serde_json::json!({"providers": {
        "test": {
            "baseUrl": format!("http://127.0.0.1:{port}"), "api": "anthropic-messages",
            "apiKey": "from-the-entry", "models": [{"id": "claude-test"}],
        },
        "other": {
            "baseUrl": "http://127.0.0.1:9", "api": "anthropic-messages",
            "models": [{"id": "claude-other"}],
        },
    }});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
    let run = |arguments: &[&str]| {
        let output = program(&binary())
            .args([
                "-p",
                "--no-session",
                "--retries",
                "0",
                "--model",
                "test/claude-test",
            ])
            .args(arguments)
            .arg("hi")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("SCRAMJET_OFFLINE", "1")
            .env("SCRAMJET_TEST_KEY_VARIABLE", "from-the-variable")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        let (head, _) = requests.recv_timeout(Duration::from_secs(20)).unwrap();
        let key = (head.lines())
            .find_map(|line| line.strip_prefix("x-api-key: "))
            .map(str::to_owned);
        (key, stderr)
    };
    let (key, _) = run(&[]);
    assert_eq!(key.as_deref(), Some("from-the-entry"));
    // As it is: neither a variable's name nor a command.
    for given in ["sk-given", "SCRAMJET_TEST_KEY_VARIABLE", "!true"] {
        let (key, stderr) = run(&["--api-key", given]);
        assert_eq!(key.as_deref(), Some(given), "{stderr}");
        assert!(!stderr.contains(given), "{stderr}");
    }
    let output = program(&binary())
        .args(["-p", "--no-session", "--api-key", "two words", "hi"])
        .env("XDG_CONFIG_HOME", &config)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let said = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        said,
        "scramjet: --api-key: a key is letters, digits and punctuation, without spaces\n"
    );
}

/// A server answering each request with an error that is not retried, sending on each body.
fn serve_refusals() -> (u16, std::sync::mpsc::Receiver<(String, serde_json::Value)>) {
    use std::io::{BufRead, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let (mut length, mut head) = (0, String::new());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                head.push_str(&line);
                let line = line.trim_end().to_ascii_lowercase();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let _ = sent.send((head, serde_json::from_slice(&body).unwrap()));
            let refusal =
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"no"}}"#;
            let reply = format!(
                "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\n\
content-length: {}\r\nconnection: close\r\n\r\n{refusal}",
                refusal.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (port, received)
}

#[test]
fn cache_retention_marks_requests_as_scramjet_1s() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-retention-{}", std::process::id())),
    );
    let config = scratch.0.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let (port, bodies) = serve_refusals();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://127.0.0.1:{port}"), "api": "anthropic-messages",
        "models": [{"id": "claude-test"}],
    }}});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
    let run = |arguments: &[&str], variables: &[(&str, &str)]| {
        let output = program(&binary())
            .args(["-p", "--no-session", "--retries", "0"])
            .args(arguments)
            .arg("hi")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", scratch.0.join("state"))
            .env("SCRAMJET_OFFLINE", "1")
            .env_remove("SCRAMJET_CACHE_RETENTION")
            .env_remove("PI_CACHE_RETENTION")
            .envs(variables.iter().copied())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        let (_, body) = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
        // The system prompt's marker and the last message's, whose last block is the prompt's.
        let last = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        let marked = (
            body["system"][0].get("cache_control").cloned(),
            last.get("cache_control").cloned(),
        );
        (marked, stderr)
    };
    let long = serde_json::json!({"type": "ephemeral", "ttl": "1h"});
    let short = serde_json::json!({"type": "ephemeral"});
    let both = |control: &serde_json::Value| (Some(control.clone()), Some(control.clone()));
    for (arguments, variables, marked) in [
        (&[][..], &[][..], both(&long)),
        (&["--cache-retention", "short"], &[], both(&short)),
        (&[], &[("SCRAMJET_CACHE_RETENTION", "short")], both(&short)),
        (&[], &[("PI_CACHE_RETENTION", "none")], (None, None)),
        (
            &[],
            &[
                ("SCRAMJET_CACHE_RETENTION", "none"),
                ("PI_CACHE_RETENTION", "short"),
            ],
            (None, None),
        ),
        (
            &["--cache-retention", "long"],
            &[("SCRAMJET_CACHE_RETENTION", "none")],
            both(&long),
        ),
    ] {
        let (given, stderr) = run(arguments, variables);
        assert_eq!(given, marked, "{arguments:?} {variables:?}: {stderr}");
    }
    let (given, stderr) = run(&[], &[("SCRAMJET_CACHE_RETENTION", "forever")]);
    assert_eq!(given, both(&long));
    assert!(
        stderr.starts_with(
            "SCRAMJET_CACHE_RETENTION \"forever\" is not none, short or long; long is used\n"
        ),
        "{stderr}"
    );
    for refused in ["none", "medium"] {
        let output = program(&binary())
            .args(["--cache-retention", refused])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!(
                "scramjet: invalid --cache-retention \"{refused}\": valid values are short and long\n"
            )
        );
    }
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
    // Scramjet 1's options: the packages alone, one of them, or the program alone.
    let packages_only = format!("{source}: up to date ({second})\n");
    assert_eq!(run(&["update", "--extensions"]).1, packages_only);
    assert_eq!(run(&["update", "--extension", "mach12"]).1, packages_only);
    assert_eq!(run(&["update", "--self"]).1, up_to_date);
    let (ok, out, _) = run(&["update", "self", "--extensions"]);
    assert!(ok);
    assert_eq!(out, format!("{packages_only}{up_to_date}"));
    let (ok, _, err) = run(&["update", "mach12", "--extensions"]);
    assert!(!ok);
    assert!(
        err.starts_with("scramjet: Usage: scramjet install"),
        "{err}"
    );
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
    // From git, a Scramjet 1 extension and a package needing a later Scramjet leave no clone.
    let old = scratch.0.join("old");
    std::fs::create_dir_all(&old).unwrap();
    git(&old, &["init", "--quiet"]);
    std::fs::write(
        old.join("package.json"),
        r#"{"pi": {"extensions": ["./x.ts"]}}"#,
    )
    .unwrap();
    git(&old, &["add", "-A"]);
    git(&old, &["commit", "--quiet", "-m", "v1"]);
    git(
        &scratch.0,
        &["clone", "--quiet", "--bare", "old", "old.git"],
    );
    let old_source = url(&scratch.0.join("old.git"));
    let (ok, _, err) = run(&["install", &old_source]);
    assert!(!ok);
    assert!(
        err.ends_with(&format!(
            "scramjet: {old_source} is a Scramjet 1 extension (its package.json has a \"pi\" \
manifest), which Scramjet 2 cannot run; install the package's Scramjet 2 version instead, if it \
has one\n"
        )),
        "{err}"
    );
    std::fs::write(
        old.join("models.json"),
        r#"{"engines": {"scramjet": ">=99.0.0"}, "providers": {}}"#,
    )
    .unwrap();
    git(&old, &["add", "-A"]);
    git(&old, &["commit", "--quiet", "-m", "v2"]);
    git(
        &old,
        &[
            "push",
            "--quiet",
            scratch.0.join("old.git").to_str().unwrap(),
            "HEAD",
        ],
    );
    let (ok, _, err) = run(&["install", &old_source]);
    assert!(!ok);
    assert!(
        err.ends_with(&format!(
            "scramjet: {old_source}: needs Scramjet 99.0.0 or later, and this is {}; \
`scramjet update self` updates it\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{err}"
    );
    assert!(
        !clones
            .join("localhost")
            .join(within.trim_start_matches(['/', '\\']))
            .join("old")
            .exists()
    );
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
    std::fs::write(
        notes.join("package.json"),
        r#"{"name": "old", "pi": {"extensions": ["./src/index.ts"]}}"#,
    )
    .unwrap();
    let (ok, _, err) = run(&["install", notes.to_str().unwrap()]);
    assert!(!ok);
    assert!(
        err.ends_with(&native(
            "scramjet: ~/notes is a Scramjet 1 extension (its package.json has a \"pi\" \
manifest), which Scramjet 2 cannot run; install the package's Scramjet 2 version instead, if it \
has one\n"
        )),
        "{err}"
    );
    std::fs::write(
        notes.join("models.json"),
        r#"{"engines": {"scramjet": ">=99.0.0"}, "providers": {}}"#,
    )
    .unwrap();
    let (ok, _, err) = run(&["install", notes.to_str().unwrap()]);
    assert!(!ok);
    assert!(
        err.ends_with(&native(&format!(
            "scramjet: ~/notes: needs Scramjet 99.0.0 or later, and this is {}; `scramjet update \
self` updates it\n",
            env!("CARGO_PKG_VERSION")
        ))),
        "{err}"
    );
    assert_eq!(packages(), serde_json::json!([]));
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
    serve_events(call_events(name, input))
}

/// A Claude reply calling the tool `name` with `input`, then one with a short text.
fn call_events(name: &str, input: serde_json::Value) -> Vec<Vec<String>> {
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
    vec![call, text]
}

/// A scripted Claude provider answering its requests with `replies`' events in turn, cycling:
/// its port, and each request's body.
fn serve_events(replies: Vec<Vec<String>>) -> (u16, std::sync::mpsc::Receiver<String>) {
    serve_events_after(replies, |_| Duration::ZERO)
}

/// `serve_events`, holding request `n`'s reply for `delay(n)`.
fn serve_events_after(
    replies: Vec<Vec<String>>,
    delay: impl Fn(usize) -> Duration + Send + 'static,
) -> (u16, std::sync::mpsc::Receiver<String>) {
    let replies: Vec<String> = replies.iter().map(|events| sse_reply(events)).collect();
    serve_raw_after(replies, delay)
}

/// An HTTP response streaming `events`, Claude's.
fn sse_reply(events: &[String]) -> String {
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
}

/// A scripted provider answering its requests with `replies`, whole HTTP responses, in turn,
/// cycling, holding request `n`'s for `delay(n)`: its port, and each request's body.
fn serve_raw_after(
    replies: Vec<String>,
    delay: impl Fn(usize) -> Duration + Send + 'static,
) -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (sent, received) = std::sync::mpsc::channel();
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
            std::thread::sleep(delay(n));
            stream
                .write_all(replies[n % replies.len()].as_bytes())
                .unwrap();
            let _ = sent.send(String::from_utf8(body).unwrap());
        }
    });
    (port, received)
}

/// `scramjet acp` in `project` with the provider at `port`, its stdin, and its stdout's messages.
#[cfg(unix)]
fn acp(
    scratch: &std::path::Path,
    port: u16,
) -> (
    Child,
    std::process::ChildStdin,
    std::sync::mpsc::Receiver<serde_json::Value>,
) {
    acp_with(
        scratch,
        port,
        serde_json::json!({"hideCredentials": "never"}),
        &[],
    )
}

/// `acp` with `settings` for `settings.json` and the options `given`.
#[cfg(unix)]
fn acp_with(
    scratch: &std::path::Path,
    port: u16,
    settings: serde_json::Value,
    given: &[&str],
) -> (
    Child,
    std::process::ChildStdin,
    std::sync::mpsc::Receiver<serde_json::Value>,
) {
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000,
        "input": ["text", "image"]}]);
    let given = [&["--no-session"], given].concat();
    acp_models(scratch, port, models, settings, &given)
}

/// `acp_with`, the provider at `port` offering `models`, saving sessions unless `given` has
/// `--no-session`.
#[cfg(unix)]
fn acp_models(
    scratch: &std::path::Path,
    port: u16,
    models: serde_json::Value,
    settings: serde_json::Value,
    given: &[&str],
) -> (
    Child,
    std::process::ChildStdin,
    std::sync::mpsc::Receiver<serde_json::Value>,
) {
    let config = scratch.join("config");
    std::fs::create_dir_all(config.join("scramjet")).unwrap();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://127.0.0.1:{port}"), "api": "anthropic-messages",
        "models": models,
    }}});
    std::fs::write(config.join("scramjet/models.json"), models.to_string()).unwrap();
    std::fs::write(config.join("scramjet/settings.json"), settings.to_string()).unwrap();
    let mut child = program(&binary())
        .args(["acp", "--retries", "0"])
        .args(given)
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_STATE_HOME", scratch.join("state"))
        .env("SCRAMJET_CACHE", scratch.join("cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (sent, messages) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines() {
            let line = line.unwrap();
            let message = serde_json::from_str(&line).unwrap_or_else(|_| panic!("{line}"));
            if sent.send(message).is_err() {
                return;
            }
        }
    });
    (child, stdin, messages)
}

/// The next message, within 20 s.
#[cfg(unix)]
fn next(messages: &std::sync::mpsc::Receiver<serde_json::Value>) -> serde_json::Value {
    messages.recv_timeout(Duration::from_secs(20)).unwrap()
}

/// Messages until the one `found` accepts, which is returned with those before it.
#[cfg(unix)]
fn until(
    messages: &std::sync::mpsc::Receiver<serde_json::Value>,
    found: impl Fn(&serde_json::Value) -> bool,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    let mut before = Vec::new();
    loop {
        let message = next(messages);
        if found(&message) {
            return (message, before);
        }
        before.push(message);
    }
}

#[cfg(unix)]
fn send(stdin: &mut std::process::ChildStdin, message: serde_json::Value) {
    writeln!(stdin, "{message}").unwrap();
}

#[test]
#[cfg(unix)]
fn acp_runs_prompt_turns_with_their_updates_queues_prompts_and_cancels() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, bodies) = serve_bash_call("sleep 2; echo from-acp");
    let (mut child, mut stdin, messages) = acp(&scratch.0, port);
    // Framing: a line that is not JSON, a request before `initialize`.
    writeln!(stdin, "not json").unwrap();
    assert_eq!(next(&messages)["error"]["code"], -32700);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "session/new"}),
    );
    assert_eq!(next(&messages)["error"]["code"], -32600);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    let initialized = next(&messages);
    assert_eq!(initialized["result"]["protocolVersion"], 1, "{initialized}");
    assert_eq!(initialized["result"]["agentInfo"]["name"], "scramjet");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "nothing"}),
    );
    assert_eq!(next(&messages)["error"]["code"], -32601);
    let new = serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/new",
        "params": {"cwd": project, "mcpServers": []}});
    send(&mut stdin, new);
    let created = next(&messages);
    let session = created["result"]["sessionId"].as_str().unwrap().to_owned();
    let commands = next(&messages);
    assert_eq!(
        commands["params"]["update"]["sessionUpdate"], "available_commands_update",
        "{commands}"
    );
    assert!(commands["params"]["_meta"].is_null(), "{commands}");
    let prompt = |id: u64, text: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}})
    };
    // A built-in is refused, naming it.
    send(&mut stdin, prompt(5, "/model claude"));
    let refused = next(&messages);
    assert_eq!(refused["error"]["code"], -32602);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/model")
    );
    // A turn: the call, its progress and result, the reply; a prompt sent meanwhile steers
    // its next request, and both prompts end with the turn.
    send(&mut stdin, prompt(6, "hi"));
    let running =
        |message: &serde_json::Value| message["params"]["update"]["status"] == "in_progress";
    let (_, before) = until(&messages, running);
    let call = (before.iter())
        .find(|message| message["params"]["update"]["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(call["params"]["update"]["name"], "bash", "{call}");
    assert_eq!(call["params"]["update"]["kind"], "execute", "{call}");
    send(&mut stdin, prompt(7, "and also this"));
    let (first, updates) = until(&messages, |message| message.get("id").is_some());
    let second = next(&messages);
    let mut ids = [first["id"].as_u64(), second["id"].as_u64()];
    ids.sort();
    assert_eq!(ids, [Some(6), Some(7)]);
    for answer in [&first, &second] {
        assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    }
    let result = (updates.iter())
        .find(|message| message["params"]["update"]["status"] == "completed")
        .unwrap();
    assert!(result.to_string().contains("from-acp"), "{result}");
    let reply: String = (updates.iter())
        .filter(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .map(|message| {
            message["params"]["update"]["content"]["text"]
                .as_str()
                .unwrap()
        })
        .collect();
    assert_eq!(reply, "Done.");
    let usage = (before.iter().chain(&updates))
        .find(|message| message["params"]["update"]["sessionUpdate"] == "usage_update")
        .unwrap();
    assert_eq!(usage["params"]["update"]["size"], 1000, "{usage}");
    let meta = &usage["params"]["update"]["_meta"]["io.github.leanandmean.scramjet"];
    assert!(meta["tokens"]["output"].is_u64(), "{usage}");
    // The first prompt titles the session, once.
    let titles: Vec<_> = (before.iter().chain(&updates))
        .filter(|message| message["params"]["update"]["sessionUpdate"] == "session_info_update")
        .collect();
    assert_eq!(titles.len(), 1, "{titles:?}");
    assert_eq!(titles[0]["params"]["update"]["title"], "hi", "{titles:?}");
    let _first = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
    let steered = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(steered.contains("and also this"), "{steered}");
    // A cancelled turn ends `cancelled`, not with an error.
    send(&mut stdin, prompt(8, "again"));
    until(&messages, running);
    let cancel = serde_json::json!({"jsonrpc": "2.0", "method": "session/cancel",
        "params": {"sessionId": session}});
    send(&mut stdin, cancel);
    let (cancelled, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(cancelled["id"], 8);
    assert_eq!(
        cancelled["result"]["stopReason"], "cancelled",
        "{cancelled}"
    );
    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

#[test]
#[cfg(unix)]
fn acp_asks_the_client_before_calls_under_ask() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-ask-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, _bodies) = serve_bash_call("echo ran-it");
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_with(&scratch.0, port, settings, &["--ask"]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    let asked = |message: &serde_json::Value| message["method"] == "session/request_permission";
    let answered = |message: &serde_json::Value| {
        message.get("id").is_some() && message.get("method").is_none()
    };
    // Each turn's reply calls `echo ran-it`, then the next request gets `Done.`; the
    // provider's replies alternate so, so a turn cancelled after its call shifts them.
    let mut turn = |id: u64, answer: Option<serde_json::Value>| {
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
                "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
        );
        let (request, before) = until(&messages, |message| asked(message) || answered(message));
        let call = (before.iter())
            .find(|message| message["params"]["update"]["sessionUpdate"] == "tool_call");
        let call_id = call.map(|call| call["params"]["update"]["toolCallId"].clone());
        if asked(&request) {
            let params = &request["params"];
            assert_eq!(params["sessionId"], session);
            assert_eq!(params["toolCall"]["toolCallId"], call_id.clone().unwrap());
            assert!(request.to_string().contains("Run in"), "{request}");
            let kinds: Vec<_> = (params["options"].as_array().unwrap().iter())
                .map(|option| option["kind"].as_str().unwrap())
                .collect();
            assert_eq!(kinds, ["allow_once", "allow_always", "reject_once"]);
            match &answer {
                Some(outcome) => send(
                    &mut stdin,
                    serde_json::json!({"jsonrpc": "2.0", "id": request["id"],
                        "result": {"outcome": outcome}}),
                ),
                None => send(
                    &mut stdin,
                    serde_json::json!({"jsonrpc": "2.0", "method": "session/cancel",
                        "params": {"sessionId": session}}),
                ),
            }
            let (ended, after) = until(&messages, answered);
            return (Some(request), ended, after);
        }
        (None, request, before)
    };
    let selected =
        |option: &str| Some(serde_json::json!({"outcome": "selected", "optionId": option}));
    let output = |updates: &[serde_json::Value]| {
        (updates.iter())
            .find(|message| {
                ["completed", "failed"].contains(
                    &message["params"]["update"]["status"]
                        .as_str()
                        .unwrap_or_default(),
                )
            })
            .map(|message| message["params"]["update"]["content"].to_string())
    };
    let (asked_once, ended, after) = turn(3, selected("allow_once"));
    assert!(asked_once.is_some());
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert!(output(&after).unwrap().contains("ran-it"));
    let (_, ended, after) = turn(4, selected("reject_once"));
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert!(output(&after).unwrap().contains("denied"), "{after:?}");
    // An option not offered is a denial.
    let (_, _, after) = turn(5, selected("whatever"));
    assert!(output(&after).unwrap().contains("denied"), "{after:?}");
    // Cancelled while asking: the request is withdrawn and the turn ends cancelled.
    let (_, ended, after) = turn(6, None);
    assert_eq!(ended["result"]["stopReason"], "cancelled", "{ended}");
    assert!(
        after
            .iter()
            .any(|message| message["method"] == "$/cancel_request")
    );
    // The cancelled turn left the provider's `Done.` for the next one.
    let (asked_none, ended, _) = turn(7, None);
    assert!(asked_none.is_none());
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let (_, _, after) = turn(8, selected("allow_always"));
    assert!(output(&after).unwrap().contains("ran-it"));
    let (asked_again, ended, after) = turn(9, None);
    assert!(asked_again.is_none(), "{asked_again:?}");
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert!(output(&after).unwrap().contains("ran-it"));
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_numbers_what_it_sends_and_gives_the_state_and_events_to_a_client_that_comes_back() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-events-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, _bodies) = serve_bash_call("echo ran-it");
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_with(&scratch.0, port, settings, &["--ask"]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities":
                {"_meta": {"io.github.leanandmean.scramjet": {}}}}}),
    );
    let methods =
        next(&messages)["result"]["_meta"]["io.github.leanandmean.scramjet"]["methods"].to_string();
    assert!(
        methods.contains("/state") && methods.contains("/events"),
        "{methods}"
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    let seq = |message: &serde_json::Value| {
        message["params"]["_meta"]["io.github.leanandmean.scramjet"]["seq"].as_u64()
    };
    let commands = next(&messages);
    assert_eq!(seq(&commands), Some(1), "{commands}");
    let call = |id: u64, method: &str, params: serde_json::Value| {
        let mut params = params;
        params["sessionId"] = session.clone();
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": format!("_io.github.leanandmean.scramjet/{method}"), "params": params})
    };
    send(&mut stdin, call(3, "state", serde_json::json!({})));
    let idle = next(&messages)["result"].clone();
    assert_eq!(idle["running"], false, "{idle}");
    assert_eq!(idle["seq"], 1, "{idle}");
    assert_eq!(idle["ask"], true, "{idle}");
    assert_eq!(idle["sandbox"], "danger-full-access", "{idle}");
    assert_eq!(idle["configOptions"][0]["id"], "model", "{idle}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let asked = |message: &serde_json::Value| message["method"] == "session/request_permission";
    let (request, before) = until(&messages, asked);
    // Every update and request is numbered on from the last, in the order written.
    let sent: Vec<serde_json::Value> = before.into_iter().chain([request.clone()]).collect();
    let numbers: Vec<Option<u64>> = sent.iter().map(seq).collect();
    let expected: Vec<Option<u64>> = (2..2 + sent.len() as u64).map(Some).collect();
    assert_eq!(numbers, expected, "{sent:?}");
    // Coming back during the turn: what is open, and what was sent since a number.
    send(&mut stdin, call(5, "state", serde_json::json!({})));
    let running = next(&messages)["result"].clone();
    assert_eq!(running["running"], true, "{running}");
    assert_eq!(running["request"], request, "{running}");
    assert_eq!(running["seq"], seq(&request).unwrap(), "{running}");
    assert!(running["question"].is_null(), "{running}");
    send(
        &mut stdin,
        call(6, "events", serde_json::json!({"since": 1})),
    );
    let events = next(&messages)["result"].clone();
    assert_eq!(events["complete"], true, "{events}");
    assert_eq!(events["events"], serde_json::json!(sent), "{events}");
    send(
        &mut stdin,
        call(7, "events", serde_json::json!({"since": 3})),
    );
    let later = next(&messages)["result"].clone();
    assert_eq!(later["events"], serde_json::json!(sent[2..]), "{later}");
    // What was sent before the turn began is no longer held.
    send(
        &mut stdin,
        call(8, "events", serde_json::json!({"since": 0})),
    );
    let missed = next(&messages)["result"].clone();
    assert_eq!(missed["complete"], false, "{missed}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": request["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}}}),
    );
    let (ended, after) = until(&messages, |message| message["id"] == 4);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let last = after.iter().filter_map(seq).max().unwrap();
    send(&mut stdin, call(9, "state", serde_json::json!({})));
    let done = next(&messages)["result"].clone();
    assert_eq!(done["running"], false, "{done}");
    assert!(done["request"].is_null(), "{done}");
    assert_eq!(done["seq"], last, "{done}");
    assert_eq!(done["usage"]["size"], 1000, "{done}");
    send(
        &mut stdin,
        call(10, "events", serde_json::json!({"since": last})),
    );
    let none = next(&messages)["result"].clone();
    assert_eq!(none["events"], serde_json::json!([]), "{none}");
    assert_eq!(none["complete"], true, "{none}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// What a client offering `capabilities` sees of one turn whose reply calls `name` with
/// `input`, `answer` giving the result for the request it is sent, if any: that request, the
/// prompt's response, the messages between, and the request bodies after the first.
#[cfg(unix)]
fn acp_asked(
    name: &str,
    input: serde_json::Value,
    given: &[&str],
    capabilities: serde_json::Value,
    answer: impl Fn(&serde_json::Value) -> serde_json::Value,
) -> (
    Option<serde_json::Value>,
    serde_json::Value,
    Vec<serde_json::Value>,
    Vec<String>,
) {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "scramjet-cli-acp-asked-{}-{run}",
        std::process::id()
    )));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, bodies) = serve_call(name, input);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_with(&scratch.0, port, settings, given);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": capabilities}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (mut request, mut seen) = (None, Vec::new());
    let ended = loop {
        let message = next(&messages);
        if message.get("method").is_some() && message.get("id").is_some() {
            assert!(request.is_none(), "asked twice: {message}");
            assert_eq!(message["params"]["sessionId"], session);
            let result = answer(&message);
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0", "id": message["id"], "result": result}),
            );
            request = Some(message);
        } else if message.get("id").is_some() {
            break message;
        } else {
            seen.push(message);
        }
    };
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let bodies: Vec<String> = bodies.try_iter().skip(1).collect();
    (request, ended, seen, bodies)
}

#[test]
#[cfg(unix)]
fn acp_asks_questions_with_the_clients_forms_and_grants_with_permissions() {
    let forms = serde_json::json!({"elicitation": {"form": {}}});
    let select = serde_json::json!({"type": "select", "message": "Which one?",
        "options": [{"value": "a", "label": "A"},
            {"value": "b", "label": "B", "description": "bee"}],
        "recommended": 1});
    let accept = |answer: serde_json::Value| move |_: &serde_json::Value| serde_json::json!({"action": "accept", "content": {"answer": answer}});
    let result = |seen: &[serde_json::Value]| {
        (seen.iter())
            .find(|message| {
                let status = &message["params"]["update"]["status"];
                status == "completed" || status == "failed"
            })
            .map(|message| message["params"]["update"]["content"].to_string())
            .unwrap()
    };
    // A select: a string with an option per value, the recommended one the default.
    let (asked, ended, seen, bodies) = acp_asked(
        "get_scramjet_user_input",
        select.clone(),
        &[],
        forms.clone(),
        accept(serde_json::json!("b")),
    );
    let asked = asked.unwrap();
    assert_eq!(asked["method"], "elicitation/create");
    let params = &asked["params"];
    assert_eq!(params["mode"], "form");
    assert_eq!(params["message"], "Which one?");
    let call = (seen.iter())
        .find(|message| message["params"]["update"]["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(params["toolCallId"], call["params"]["update"]["toolCallId"]);
    let schema = &params["requestedSchema"];
    assert_eq!(schema["required"], serde_json::json!(["answer"]));
    let property = &schema["properties"]["answer"];
    assert_eq!(property["type"], "string", "{property}");
    assert_eq!(property["default"], "b");
    assert_eq!(
        property["oneOf"],
        serde_json::json!([{"const": "a", "title": "A"},
            {"const": "b", "title": "B", "description": "bee"}])
    );
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert!(
        bodies[0].contains(r#"{\"selected\":\"b\"}"#),
        "{}",
        bodies[0]
    );
    // An answer not offered cancels the question, which ends the turn.
    let (_, ended, seen, bodies) = acp_asked(
        "get_scramjet_user_input",
        select.clone(),
        &[],
        forms.clone(),
        accept(serde_json::json!("c")),
    );
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert!(result(&seen).contains("cancelled"), "{seen:?}");
    assert!(bodies.is_empty(), "{bodies:?}");
    // A confirm: a boolean, `true` for yes.
    let confirm = serde_json::json!({"type": "confirm", "message": "Go on?"});
    let (asked, _, _, bodies) = acp_asked(
        "get_scramjet_user_input",
        confirm.clone(),
        &[],
        forms.clone(),
        accept(serde_json::json!(true)),
    );
    let property = &asked.unwrap()["params"]["requestedSchema"]["properties"]["answer"];
    assert_eq!(property["type"], "boolean", "{property}");
    assert!(
        bodies[0].contains(r#"{\"confirmed\":true}"#),
        "{}",
        bodies[0]
    );
    // Declined: cancelled.
    let (_, _, seen, _) = acp_asked(
        "get_scramjet_user_input",
        confirm,
        &[],
        forms,
        |_| serde_json::json!({"action": "decline"}),
    );
    assert!(result(&seen).contains("cancelled"), "{seen:?}");
    // Without forms the question is a freetext one, its options listed for the next prompt.
    let (asked, ended, seen, bodies) = acp_asked(
        "get_scramjet_user_input",
        select,
        &[],
        serde_json::json!({}),
        |_| unreachable!(),
    );
    assert!(asked.is_none());
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let said: String = (seen.iter())
        .filter(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|message| message["params"]["update"]["content"]["text"].as_str())
        .collect();
    assert!(
        said.contains("Which one?\n\nReply with one of:\n- A\n- B — bee"),
        "{said}"
    );
    assert!(result(&seen).contains("parked"), "{seen:?}");
    assert!(bodies.is_empty(), "{bodies:?}");
    // A client that does not know Scramjet's extensions gets none of its notifications.
    assert!(
        (seen.iter()).all(|message| !message.to_string().contains("_io.github")),
        "{seen:?}"
    );
    // A write grant: a permission request whose last option denies.
    let grant = serde_json::json!({"write": ["/var/tmp"], "reason": "to build"});
    let (asked, _, seen, _) = acp_asked(
        "request_permissions",
        grant,
        &["--sandbox", "workspace-write"],
        serde_json::json!({}),
        |asked| {
            let options = asked["params"]["options"].as_array().unwrap();
            let last = &options[options.len() - 1];
            serde_json::json!({"outcome": {"outcome": "selected", "optionId": last["optionId"]}})
        },
    );
    let asked = asked.unwrap();
    assert_eq!(asked["method"], "session/request_permission");
    assert!(
        asked.to_string().contains("Allow writing there?"),
        "{asked}"
    );
    let kinds: Vec<_> = (asked["params"]["options"].as_array().unwrap().iter())
        .map(|option| option["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.first(), Some(&"allow_once"));
    assert_eq!(kinds.last(), Some(&"reject_once"));
    assert!(result(&seen).contains("stay read-only"), "{seen:?}");
}

#[test]
#[cfg(unix)]
fn acp_tells_a_client_that_knows_the_extension_the_status_and_a_question_left_open() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-status-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let question = serde_json::json!({"type": "freetext", "message": "Which branch?"});
    let busy = "HTTP/1.1 503 Service Unavailable\r\ncontent-type: application/json\r\n\
content-length: 2\r\nconnection: close\r\n\r\n{}";
    let mut replies = vec![busy.to_owned()];
    replies.extend(
        call_events("get_scramjet_user_input", question)
            .iter()
            .map(|e| sse_reply(e)),
    );
    let (port, _bodies) = serve_raw_after(replies, |_| Duration::ZERO);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) =
        acp_with(&scratch.0, port, settings, &["--retries", "1"]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities":
                {"_meta": {"io.github.leanandmean.scramjet": {}}}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (ended, seen) = until(&messages, |message| message["id"] == 3);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let ours = |name: &str| format!("_io.github.leanandmean.scramjet/{name}");
    let statuses: Vec<&serde_json::Value> = (seen.iter())
        .filter(|message| message["method"] == ours("status"))
        .map(|message| &message["params"])
        .collect();
    let states: Vec<&str> = (statuses.iter())
        .map(|status| status["state"].as_str().unwrap())
        .collect();
    // Waiting, retrying after the 503, waiting again for the call, then for the reply after it
    // is not sent: the question ends the turn.
    assert_eq!(states, ["waiting", "retrying", "waiting"], "{statuses:?}");
    assert_eq!(statuses[0]["sessionId"], session);
    assert_eq!(statuses[0]["text"], "Waiting for reply");
    let retrying = statuses[1];
    assert!(
        retrying["text"]
            .as_str()
            .unwrap()
            .starts_with("Retrying (1/1) in "),
        "{retrying}"
    );
    assert!(retrying["delayMs"].as_u64().unwrap() > 0, "{retrying}");
    // The question goes just before the turn's response.
    let last = seen.last().unwrap();
    assert_eq!(last["method"], ours("question"), "{last}");
    assert_eq!(
        last["params"],
        serde_json::json!({"sessionId": session, "question": "Which branch?",
            "_meta": {"io.github.leanandmean.scramjet": {"seq": last["params"]["_meta"]
                ["io.github.leanandmean.scramjet"]["seq"]}}})
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_names_the_entries_a_live_turn_saves_by_the_updates_that_showed_them() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-entries-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("a.txt"), "alpha\n").unwrap();
    let mut replies = call_events("read", serde_json::json!({"path": "a.txt"}));
    replies.extend(std::iter::repeat_n(replies[1].clone(), 4));
    let (port, _bodies) = serve_events(replies);
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never",
        "compaction": {"reserveTokens": 100, "keepRecentTokens": 1}});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities":
                {"_meta": {"io.github.leanandmean.scramjet": {}}}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (ended, seen) = until(&messages, |message| message["id"] == 3);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let ours = |name: &str| format!("_io.github.leanandmean.scramjet/{name}");
    let entries: Vec<&serde_json::Value> = (seen.iter())
        .filter(|message| message["method"] == ours("entry"))
        .map(|message| &message["params"])
        .collect();
    let kinds: Vec<&str> = (entries.iter())
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["prompt", "toolResult", "reply"], "{entries:?}");
    // The client's own prompt showed in no update; the others name theirs.
    assert!(entries[0].get("messageId").is_none(), "{entries:?}");
    let update = |kind: &str| {
        (seen.iter())
            .find(|message| message["params"]["update"]["sessionUpdate"] == kind)
            .map(|message| &message["params"]["update"])
            .unwrap()
    };
    assert_eq!(entries[1]["toolCallId"], update("tool_call")["toolCallId"]);
    assert_eq!(
        entries[2]["messageId"],
        update("agent_message_chunk")["messageId"]
    );
    // Without `session.compaction` a compaction is said in a message, which its entry names.
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "again"}]}}),
    );
    until(&messages, |message| message["id"] == 4);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 5, "method": ours("compact"),
            "params": {"sessionId": session}}),
    );
    let (compacted, said) = until(&messages, |message| message["id"] == 5);
    assert_eq!(compacted["result"]["compacted"], true, "{compacted}");
    let compaction = (said.iter())
        .find(|message| message["method"] == ours("entry"))
        .map(|message| &message["params"])
        .unwrap();
    assert_eq!(compaction["kind"], "compaction", "{compaction}");
    let message = (said.iter())
        .find(|message| message["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .unwrap();
    assert_eq!(
        compaction["messageId"],
        message["params"]["update"]["messageId"]
    );
    let mut entries = entries;
    entries.push(compaction);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 6, "method": ours("tree"),
            "params": {"sessionId": session}}),
    );
    let (tree, _) = until(&messages, |message| message["id"] == 6);
    let listed = tree["result"]["entries"].as_array().unwrap();
    for entry in &entries {
        let found = (listed.iter())
            .find(|listed| listed["entryId"] == entry["entryId"])
            .unwrap_or_else(|| panic!("{entry} not in {listed:?}"));
        assert_eq!(found["kind"], entry["kind"], "{found}");
        assert_eq!(entry["sessionId"], session);
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_on_sigterm_cancels_turns_and_their_commands_then_exits() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-term-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let marker = scratch.0.join("marker");
    let (port, _bodies) = serve_bash_call(&format!("sleep 3; touch '{}'", marker.display()));
    let (mut child, mut stdin, messages) = acp(&scratch.0, port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    until(&messages, |message| {
        message["params"]["update"]["status"] == "in_progress"
    });
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let (answer, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(answer["result"]["stopReason"], "cancelled", "{answer}");
    let started = std::time::Instant::now();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(143), "{status}");
    assert!(started.elapsed() < Duration::from_secs(3));
    std::thread::sleep(Duration::from_secs(4));
    assert!(!marker.exists(), "the command ran on after the exit");
}

#[test]
#[cfg(unix)]
fn acp_turns_stopped_at_the_output_limit_or_by_a_refusal_say_so() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-stop-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let stopped = |reason: &str| {
        [
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#.to_owned(),
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_owned(),
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Part"}}"#.to_owned(),
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": reason},
                "usage": {"output_tokens": 4}}).to_string(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ]
        .to_vec()
    };
    let (port, _bodies) = serve_events(vec![stopped("max_tokens"), stopped("refusal")]);
    let (mut child, mut stdin, messages) = acp(&scratch.0, port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1,
                "clientCapabilities": {"session": {"notices": {}}}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    for (id, reason) in [(3, "max_tokens"), (4, "refusal")] {
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
                "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
        );
        let (answer, updates) = until(&messages, |message| message.get("id").is_some());
        assert_eq!(answer["id"], id);
        assert_eq!(answer["result"]["stopReason"], reason, "{answer}");
        let update = |kind: &str| {
            (updates.iter())
                .map(|message| &message["params"]["update"])
                .find(|update| update["sessionUpdate"] == kind)
                .unwrap_or_else(|| panic!("no {kind} in {updates:?}"))
                .clone()
        };
        assert_eq!(update("agent_message_chunk")["content"]["text"], "Part");
        let notice = update("notice");
        assert_eq!(notice["severity"], "warning", "{notice}");
        assert!(
            notice["title"].as_str().unwrap().contains(match reason {
                "max_tokens" => "output token limit",
                _ => "refusal",
            }),
            "{notice}"
        );
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

/// The `tool_call` update and the completed one of a prompt turn in `project` whose reply
/// calls `name` with `input`.
#[cfg(unix)]
fn acp_call(
    scratch: &std::path::Path,
    project: &std::path::Path,
    name: &str,
    input: &serde_json::Value,
) -> (serde_json::Value, serde_json::Value) {
    let (port, _bodies) = serve_call(name, input.clone());
    let (mut child, mut stdin, messages) = acp(scratch, port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (answer, updates) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let update = |found: &dyn Fn(&serde_json::Value) -> bool| {
        (updates.iter())
            .map(|message| message["params"]["update"].clone())
            .find(|update| found(update))
            .unwrap()
    };
    (
        update(&|update| update["sessionUpdate"] == "tool_call"),
        update(&|update| update["status"] == "completed"),
    )
}

#[test]
#[cfg(unix)]
fn acp_prompts_carry_images() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-image-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, bodies) = serve_bash_call("sleep 2; echo seen");
    let (mut child, mut stdin, messages) = acp(&scratch.0, port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    let initialized = next(&messages);
    let capabilities = &initialized["result"]["agentCapabilities"]["promptCapabilities"];
    assert_eq!(capabilities["image"], true, "{initialized}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    next(&messages);
    let mut png = Vec::new();
    image::RgbImage::new(2, 2)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let encoded = |bytes: &[u8]| {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for chunk in bytes.chunks(3) {
            let word = (chunk.iter().enumerate()).fold(0u32, |word, (at, byte)| {
                word | u32::from(*byte) << (16 - 8 * at)
            });
            for at in 0..4 {
                text.push(match at <= chunk.len() {
                    true => ALPHABET[(word >> (18 - 6 * at) & 63) as usize] as char,
                    false => '=',
                });
            }
        }
        text
    };
    let prompt_with = |id: u64, text: &str, data: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [
            {"type": "text", "text": text},
            {"type": "image", "mimeType": "image/png", "data": data},
        ]}})
    };
    let prompt = |id: u64, data: &str| prompt_with(id, "/model what is this?", data);
    // Not an image: refused before the turn, which sends nothing.
    send(&mut stdin, prompt(3, &encoded(b"plain text")));
    let refused = next(&messages);
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    // With an image, a `/` starts no command: the text and the image go to the model.
    send(&mut stdin, prompt(4, &encoded(&png)));
    until(&messages, |message| {
        message["params"]["update"]["status"] == "in_progress"
    });
    // During the turn, one that is not an image is refused; one that is goes as a file's path
    // before the text, as the full screen pastes it.
    send(&mut stdin, prompt_with(5, "and this", &encoded(b"plain")));
    let (refused, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(refused["id"], 5);
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    send(
        &mut stdin,
        prompt_with(6, "/model and this?", &encoded(&png)),
    );
    for _ in 0..2 {
        let (answer, _) = until(&messages, |message| message.get("id").is_some());
        assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    }
    let body = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(body.contains("/model what is this?"), "{body}");
    assert!(body.contains(&encoded(&png)), "{body}");
    let steered: serde_json::Value =
        serde_json::from_str(&bodies.recv_timeout(Duration::from_secs(20)).unwrap()).unwrap();
    let last = steered["messages"].as_array().unwrap().last().unwrap();
    let text = (last["content"].as_array().unwrap().last().unwrap())["text"]
        .as_str()
        .unwrap();
    let (path, rest) = text.split_once(' ').unwrap();
    assert_eq!(rest, "/model and this?", "{text}");
    assert_eq!(std::fs::read(path).unwrap(), png);
    std::fs::remove_file(path).unwrap();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_tool_calls_carry_their_input_locations_and_diffs() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-edit-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = std::fs::canonicalize(project).unwrap();
    let path = project.join("a.txt");
    std::fs::write(&path, "one\n").unwrap();
    let input =
        serde_json::json!({"path": "a.txt", "edits": [{"oldText": "one", "newText": "two"}]});
    let (call, completed) = acp_call(&scratch.0, &project, "edit", &input);
    assert_eq!(call["kind"], "edit", "{call}");
    assert_eq!(call["rawInput"], input);
    assert_eq!(call["locations"], serde_json::json!([{"path": path}]));
    assert_eq!(completed["content"][0]["type"], "content", "{completed}");
    assert_eq!(
        completed["content"][1],
        serde_json::json!({"type": "diff", "path": path, "oldText": "one", "newText": "two"})
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
    // A write making its file: a diff from nothing; one replacing a file: its output only.
    let input = serde_json::json!({"path": "b.txt", "content": "new\n"});
    let (_, completed) = acp_call(&scratch.0, &project, "write", &input);
    let path = project.join("b.txt");
    assert_eq!(
        completed["content"][1],
        serde_json::json!({"type": "diff", "path": path, "oldText": null, "newText": "new\n"}),
        "{completed}"
    );
    let (_, completed) = acp_call(&scratch.0, &project, "write", &input);
    assert_eq!(
        completed["content"].as_array().unwrap().len(),
        1,
        "{completed}"
    );
}

#[test]
#[cfg(unix)]
fn acp_subagent_calls_carry_their_children() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-child-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let agents = scratch.0.join("config/scramjet/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let agent = "---\nname: helper\ndescription: Helps.\ntools: Read\n---\n\nHelp.\n";
    std::fs::write(agents.join("helper.md"), agent).unwrap();
    let start = |id: &str| {
        serde_json::json!({"type": "message_start", "message": {"id": id,
            "usage": {"input_tokens": 3, "output_tokens": 1}}})
        .to_string()
    };
    let end = |reason: &str| {
        [
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": reason},
                "usage": {"output_tokens": 4}})
            .to_string(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ]
    };
    let text = |id: &str, text: &str| {
        let mut events = vec![
            start(id),
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#
                .to_owned(),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}})
            .to_string(),
        ];
        events.extend(end("end_turn"));
        events
    };
    let input = serde_json::json!({"agent": "helper", "task": "look around"}).to_string();
    let mut call = vec![
        start("msg_1"),
        serde_json::json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "subagent",
                "input": {}}})
        .to_string(),
        serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": input}})
        .to_string(),
    ];
    call.extend(end("tool_use"));
    let (port, _bodies) = serve_events(vec![
        call,
        text("msg_2", "Found it."),
        text("msg_3", "Done."),
    ]);
    let (mut child, mut stdin, messages) = acp(&scratch.0, port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (answer, updates) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let calls: Vec<&serde_json::Value> = (updates.iter())
        .map(|message| &message["params"]["update"])
        .filter(|update| update["sessionUpdate"] == "tool_call_update")
        .collect();
    let children = |update: &serde_json::Value| {
        update["_meta"]["io.github.leanandmean.scramjet"]["children"].clone()
    };
    // While it runs: the child as it starts, then with its reply, and a line of each.
    let running = (calls.iter())
        .find(|update| children(update)["runs"][0]["state"] == "running")
        .unwrap_or_else(|| panic!("{calls:?}"));
    let run = &children(running)["runs"][0];
    assert_eq!(run["agent"], "helper", "{running}");
    assert_eq!(run["task"], "look around", "{running}");
    assert_eq!(children(running)["mode"], "single", "{running}");
    let line = running["content"][0]["content"]["text"].as_str().unwrap();
    assert!(line.contains("helper"), "{running}");
    let completed = (calls.iter())
        .find(|update| update["status"] == "completed")
        .unwrap_or_else(|| panic!("{calls:?}"));
    let run = &children(completed)["runs"][0];
    assert_eq!(run["state"], "done", "{completed}");
    assert_eq!(run["items"], serde_json::json!([{"text": "Found it."}]));
    assert!(completed.to_string().contains("Found it."), "{completed}");
}

#[test]
#[cfg(unix)]
fn acp_sessions_start_the_clients_mcp_servers() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-mcp-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let log = scratch.0.join("mcp.log");
    let input = serde_json::json!({"server": "editor", "tool": "echo",
        "arguments": {"text": "from-client"}});
    let (port, bodies) = serve_call("mcp_call", input);
    let (mut child, mut stdin, messages) = acp(&scratch.0, port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    let initialized = next(&messages);
    let capabilities = &initialized["result"]["agentCapabilities"]["mcpCapabilities"];
    assert_eq!(capabilities["http"], true, "{initialized}");
    let new = |id: u64, servers: serde_json::Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/new",
            "params": {"cwd": project, "mcpServers": servers}})
    };
    send(
        &mut stdin,
        new(
            2,
            serde_json::json!([{"type": "sse", "name": "old", "url": "http://127.0.0.1:1/",
            "headers": []}]),
        ),
    );
    let refused = next(&messages);
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    let server = repository().join("tests/support/mcp_server.py");
    let mut web = Command::new("python3")
        .arg(repository().join("tests/support/mcp_http_server.py"))
        .env("MCP_TEST_TOKEN", "client-token")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut port = String::new();
    BufReader::new(web.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    let url = format!("http://127.0.0.1:{}/mcp", port.trim());
    send(
        &mut stdin,
        new(
            3,
            serde_json::json!([
                {"name": "editor", "command": "python3", "args": [server],
                    "env": [{"name": "MCP_TEST_LOG", "value": log}]},
                {"type": "http", "name": "web", "url": url,
                    "headers": [{"name": "Authorization", "value": "Bearer client-token"}]},
            ]),
        ),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (answer, updates) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    let completed = (updates.iter())
        .find(|message| message["params"]["update"]["status"] == "completed")
        .unwrap();
    assert!(
        completed.to_string().contains("echo: from-client"),
        "{completed}"
    );
    let first = bodies.recv_timeout(Duration::from_secs(20)).unwrap();
    assert!(first.contains("- editor: Tools for scramjet-rs's tests"));
    assert!(first.contains("- web: Remote tools for scramjet-rs's tests"));
    drop(stdin);
    assert!(child.wait().unwrap().success());
    web.kill().unwrap();
    web.wait().unwrap();
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("tools/call")
    );
}

#[test]
#[cfg(unix)]
fn acp_shows_compactions_and_what_commands_sent() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-compact-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    let skill = project.join(".scramjet/kit/skills/greet");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::create_dir_all(project.join(".scramjet/kit/commands")).unwrap();
    let file = "---\nname: greet\ndescription: Greets.\n---\n\nSay hello.\n";
    std::fs::write(skill.join("SKILL.md"), file).unwrap();
    let reply = |text: &str, input: u64| {
        [
            serde_json::json!({"type": "message_start", "message": {"id": "msg_1",
                "usage": {"input_tokens": input, "output_tokens": 1}}})
            .to_string(),
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_owned(),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}})
            .to_string(),
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#.to_owned(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ]
        .to_vec()
    };
    let settings = serde_json::json!({"hideCredentials": "never",
        "compaction": {"reserveTokens": 100, "keepRecentTokens": 1}});
    // The second turn fills the window (1000 tokens, less the reserve), so it compacts after.
    for offered in [true, false] {
        let replies = vec![
            reply("Hello.", 3),
            reply("Again.", 950),
            reply("The summary.", 3),
        ];
        let (port, _bodies) = serve_events(replies);
        let (mut child, mut stdin, messages) = acp_with(&scratch.0, port, settings.clone(), &[]);
        let capabilities = match offered {
            true => serde_json::json!({"session": {"compaction": {}}}),
            false => serde_json::json!({}),
        };
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": 1, "clientCapabilities": capabilities}}),
        );
        next(&messages);
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": project, "mcpServers": []}}),
        );
        let session = next(&messages)["result"]["sessionId"].clone();
        let mut turn = |id: u64, text: &str| {
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
                    "params": {"sessionId": session,
                        "prompt": [{"type": "text", "text": text}]}}),
            );
            let (answer, updates) = until(&messages, |message| message.get("id").is_some());
            assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
            (updates.into_iter())
                .map(|message| message["params"]["update"].clone())
                .collect::<Vec<_>>()
        };
        let first = turn(3, "/skill:greet world");
        let sent = (first.iter())
            .find(|update| update["sessionUpdate"] == "user_message_chunk")
            .unwrap_or_else(|| panic!("no expansion in {first:?}"));
        assert!(
            sent["content"]["text"]
                .as_str()
                .unwrap()
                .ends_with("Say hello.\n</skill>\n\nworld"),
            "{sent}"
        );
        assert_eq!(
            sent["_meta"]["io.github.leanandmean.scramjet"]["expands"],
            "/skill:greet world"
        );
        let second = turn(4, "more");
        let compactions: Vec<_> = (second.iter())
            .filter(|update| update["sessionUpdate"] == "compaction_update")
            .collect();
        match offered {
            true => {
                assert_eq!(compactions.len(), 2, "{second:?}");
                assert_eq!(compactions[0]["status"], "in_progress");
                assert_eq!(compactions[1]["status"], "completed");
                assert_eq!(
                    compactions[0]["compactionId"],
                    compactions[1]["compactionId"]
                );
                let summary = compactions[1]["summary"][0]["text"].as_str().unwrap();
                assert!(summary.contains("The summary."), "{summary}");
            }
            false => {
                assert!(compactions.is_empty(), "{second:?}");
                let said = (second.iter())
                    .find(|update| {
                        update["sessionUpdate"] == "agent_message_chunk"
                            && update["content"]["text"]
                                .as_str()
                                .is_some_and(|text| text.starts_with("Context compacted."))
                    })
                    .unwrap_or_else(|| panic!("no compaction message in {second:?}"));
                assert!(
                    said["messageId"]
                        .as_str()
                        .unwrap()
                        .starts_with("compaction-")
                );
            }
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
    }
}

#[test]
#[cfg(unix)]
fn acp_compacts_and_renames_sessions_through_the_extensions() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-rename-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let reply = |text: &str| {
        [
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#.to_owned(),
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_owned(),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}})
            .to_string(),
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#.to_owned(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ]
        .to_vec()
    };
    // Summaries are held, so that a prompt or a cancel comes during the compaction.
    let replies = [reply("Hello."), reply("Again.")]
        .into_iter()
        .chain(std::iter::repeat_n(reply("The summary."), 8))
        .collect();
    let (port, bodies) = serve_events_after(replies, |n| match n {
        0 | 1 => Duration::ZERO,
        _ => Duration::from_millis(500),
    });
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never",
        "compaction": {"reserveTokens": 100, "keepRecentTokens": 1}});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1,
                "clientCapabilities": {"session": {"compaction": {}}}}}),
    );
    let offered = &next(&messages)["result"]["_meta"]["io.github.leanandmean.scramjet"];
    assert_eq!(offered["version"], 1, "{offered}");
    assert!(
        (offered["methods"].as_array().unwrap().iter())
            .any(|method| method == "_io.github.leanandmean.scramjet/compact"),
        "{offered}"
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    let prompt = |id: u64, text: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}})
    };
    let compact = |id: u64| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": "_io.github.leanandmean.scramjet/compact",
            "params": {"sessionId": session, "instructions": "Keep the names."}})
    };
    for (id, text) in [(3, "first"), (4, "second")] {
        send(&mut stdin, prompt(id, text));
        let (answer, _) = until(&messages, |message| message.get("id").is_some());
        assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    }
    send(&mut stdin, compact(5));
    send(&mut stdin, compact(6));
    send(&mut stdin, prompt(7, "after it"));
    let (refused, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(refused["id"], 6, "{refused}");
    assert_eq!(refused["error"]["code"], -32602);
    assert_eq!(refused["error"]["message"], "a turn is running");
    let (compacted, updates) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(compacted["id"], 5, "{compacted}");
    assert_eq!(compacted["result"]["compacted"], true, "{compacted}");
    let statuses: Vec<_> = (updates.iter())
        .map(|message| &message["params"]["update"])
        .filter(|update| update["sessionUpdate"] == "compaction_update")
        .map(|update| update["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["in_progress", "completed"], "{updates:?}");
    let (turned, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(turned["id"], 7, "{turned}");
    assert_eq!(turned["result"]["stopReason"], "end_turn", "{turned}");
    let bodies: Vec<String> =
        std::iter::from_fn(|| bodies.recv_timeout(Duration::from_secs(2)).ok()).collect();
    assert!(bodies[2].contains("Keep the names."), "{}", bodies[2]);
    let last = bodies.last().unwrap();
    assert!(
        last.contains("after it") && last.contains("The summary."),
        "{last}"
    );

    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 8,
            "method": "_io.github.leanandmean.scramjet/rename",
            "params": {"sessionId": session, "name": " "}}),
    );
    assert_eq!(next(&messages)["error"]["message"], "name is required");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 9,
            "method": "_io.github.leanandmean.scramjet/rename",
            "params": {"sessionId": session, "name": "My work"}}),
    );
    let renamed = next(&messages);
    assert_eq!(renamed["id"], 9, "{renamed}");
    assert_eq!(renamed["result"], serde_json::json!({}));
    let info = &next(&messages)["params"]["update"];
    assert_eq!(info["sessionUpdate"], "session_info_update", "{info}");
    assert_eq!(info["title"], "My work");
    assert!(info["updatedAt"].as_str().unwrap().ends_with('Z'), "{info}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 10, "method": "session/list",
            "params": {"cwd": project}}),
    );
    let listed = next(&messages);
    assert_eq!(
        listed["result"]["sessions"][0]["title"], "My work",
        "{listed}"
    );

    send(&mut stdin, compact(11));
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "method": "session/cancel",
            "params": {"sessionId": session}}),
    );
    let (cancelled, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(cancelled["id"], 11, "{cancelled}");
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");
    send(&mut stdin, prompt(12, "still here"));
    let (answer, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_reloads_the_settings_commands_and_models_through_the_extension() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-reload-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let reply = [
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    // Held, so that the reload comes during the turn.
    let (port, _bodies) = serve_events_after(vec![reply], |_| Duration::from_millis(500));
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1,
                "clientCapabilities": {"session": {"notices": {}}}}}),
    );
    let offered = &next(&messages)["result"]["_meta"]["io.github.leanandmean.scramjet"];
    assert!(
        (offered["methods"].as_array().unwrap().iter())
            .any(|method| method == "_io.github.leanandmean.scramjet/reload"),
        "{offered}"
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    let commands = &next(&messages)["params"]["update"];
    assert_eq!(commands["sessionUpdate"], "available_commands_update");
    let greets = |listed: &serde_json::Value| {
        (listed.as_array().unwrap().iter()).any(|command| command["name"] == "skill:greet")
    };
    assert!(!greets(&commands["availableCommands"]), "{commands}");
    let reload = |id: u64| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": "_io.github.leanandmean.scramjet/reload"})
    };
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}}),
    );
    send(&mut stdin, reload(4));
    let (refused, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(refused["id"], 4, "{refused}");
    assert_eq!(refused["error"]["message"], "a turn is running");
    let (answer, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");

    let config = scratch.0.join("config/scramjet");
    let skill = scratch.0.join("skills/greet");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: greet\ndescription: Greets the user.\n---\nSay hello.\n",
    )
    .unwrap();
    let settings = serde_json::json!({"hideCredentials": "never",
        "skills": [scratch.0.join("skills")], "commandSets": 5});
    std::fs::write(config.join("settings.json"), settings.to_string()).unwrap();
    let models = serde_json::json!({"providers": {"test": {
        "baseUrl": format!("http://127.0.0.1:{port}"), "api": "anthropic-messages",
        "models": [{"id": "claude-test", "contextWindow": 1000},
            {"id": "claude-other", "contextWindow": 1000}],
    }}});
    std::fs::write(config.join("models.json"), models.to_string()).unwrap();
    send(&mut stdin, reload(5));
    let (reloaded, updates) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(reloaded["id"], 5, "{reloaded}");
    assert_eq!(reloaded["result"], serde_json::json!({}), "{reloaded}");
    let updates: Vec<_> = (updates.iter())
        .map(|message| &message["params"]["update"])
        .collect();
    let of = |kind: &str| {
        (updates.iter())
            .find(|update| update["sessionUpdate"] == kind)
            .unwrap_or_else(|| panic!("no {kind} in {updates:?}"))
    };
    let warning = of("notice")["title"].as_str().unwrap();
    assert!(
        warning.contains("commandSets must be a list of directories"),
        "{warning}"
    );
    let listed = &of("available_commands_update")["availableCommands"];
    assert!(greets(listed), "{listed}");
    let options = of("config_option_update")["configOptions"].to_string();
    assert!(options.contains("test/claude-other"), "{options}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_saves_api_keys_through_the_extension_without_echoing_them() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-key-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let (port, requests) = serve_refusals();
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let given = ["--no-session"];
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &given);
    let mut seen = Vec::new();
    let mut answer = |stdin: &mut std::process::ChildStdin, message: serde_json::Value| {
        let id = message["id"].clone();
        send(stdin, message);
        let (answer, before) = until(&messages, |message| message["id"] == id);
        seen.extend(before);
        seen.push(answer.clone());
        answer
    };
    let initialized = answer(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    let offered = &initialized["result"]["_meta"]["io.github.leanandmean.scramjet"]["methods"];
    assert!(
        (offered.as_array().unwrap().iter())
            .any(|method| method == "_io.github.leanandmean.scramjet/set_api_key"),
        "{offered}"
    );
    let set = |id: u64, provider: &str, key: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": "_io.github.leanandmean.scramjet/set_api_key",
            "params": {"provider": provider, "key": key}})
    };
    let unknown = answer(&mut stdin, set(2, "nowhere", "sk-unknown"));
    let said = unknown["error"]["message"].as_str().unwrap();
    assert!(said.contains("no provider \"nowhere\""), "{unknown}");
    let spaced = answer(&mut stdin, set(3, "test", "sk two words"));
    let said = spaced["error"]["message"].as_str().unwrap();
    assert!(said.contains("without spaces"), "{spaced}");
    let opened = answer(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = opened["result"]["sessionId"].clone();
    let saved = answer(&mut stdin, set(5, "test", " sk-acp-secret\n"));
    assert_eq!(saved["result"], serde_json::json!({}), "{saved}");
    let auth: serde_json::Value = serde_json::from_slice(
        &std::fs::read(scratch.0.join("config/scramjet/auth.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(auth["test"]["key"], "sk-acp-secret", "{auth}");
    assert_eq!(
        auth["test"]["origins"],
        serde_json::json!([format!("http://127.0.0.1:{port}")])
    );
    // The open session reads the key before its next request.
    answer(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 6, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}}),
    );
    let (head, _) = requests.recv_timeout(Duration::from_secs(20)).unwrap();
    let key = (head.lines()).find_map(|line| line.strip_prefix("x-api-key: "));
    assert_eq!(key, Some("sk-acp-secret"), "{head}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    seen.extend(messages.try_iter());
    for message in &seen {
        let text = message.to_string();
        assert!(
            !text.contains("sk-acp") && !text.contains("sk-unknown") && !text.contains("two"),
            "{text}"
        );
    }
}

#[test]
#[cfg(unix)]
fn acp_requires_authentication_for_a_model_whose_login_is_not_made() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-unauth-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    let set = scratch.0.join("acme");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&set).unwrap();
    let (port, requests) = serve_refusals();
    let declared = serde_json::json!({
        "logins": {"acme": {"name": "Acme", "command": ["./login"]}},
        "providers": {"acme": {"baseUrl": format!("http://127.0.0.1:{port}"),
            "api": "anthropic-messages", "models": [{"id": "acme-model"}]}},
    });
    std::fs::write(set.join("models.json"), declared.to_string()).unwrap();
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"commandSets": [set], "hideCredentials": "never"});
    let state = scratch.0.join("sessions");
    let state = state.to_str().unwrap();
    let given = ["--model", "acme/acme-model", "--session-dir", state];
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &given);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    let open = |id: u64| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}})
    };
    let prompt = |id: u64, session: &serde_json::Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}})
    };
    send(&mut stdin, open(2));
    let refused = until(&messages, |message| message["id"] == 2).0;
    assert_eq!(refused["error"]["code"], -32000, "{refused}");
    assert_eq!(
        refused["error"]["data"]["details"],
        refused["error"]["message"]
    );
    assert_eq!(
        refused["error"]["message"],
        "authentication required: not logged in to Acme (/login)"
    );
    assert!(
        !std::path::Path::new(state).exists() || std::fs::read_dir(state).unwrap().count() == 0
    );

    // Signed in, a session opens; signed out again, its prompt is refused unsent.
    let auth_file = scratch.0.join("config/scramjet/auth.json");
    let saved = serde_json::json!({"acme": {"type": "oauth", "access": "acme-token",
        "expires": 99_999_999_999_999_u64}});
    std::fs::write(&auth_file, saved.to_string()).unwrap();
    std::fs::set_permissions(
        &auth_file,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    send(&mut stdin, open(3));
    let opened = until(&messages, |message| message["id"] == 3).0;
    let session = opened["result"]["sessionId"].clone();
    assert!(session.is_string(), "{opened}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "logout", "params": {}}),
    );
    until(&messages, |message| message["id"] == 4);
    send(&mut stdin, prompt(5, &session));
    let refused = until(&messages, |message| message["id"] == 5).0;
    assert_eq!(refused["error"]["code"], -32000, "{refused}");
    assert!(requests.try_recv().is_err(), "a request was sent");
    drop(stdin);
    assert!(child.wait().unwrap().success());

    // A model of a provider that needs no login is asked as before.
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let given = ["--no-session"];
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &given);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    send(&mut stdin, open(2));
    let opened = until(&messages, |message| message["id"] == 2).0;
    send(&mut stdin, prompt(3, &opened["result"]["sessionId"]));
    let failed = until(&messages, |message| message["id"] == 3).0;
    assert_eq!(failed["error"]["code"], -32603, "{failed}");
    drop(stdin);
    assert!(child.wait().unwrap().success());

    // With no model offered at all, a sign-in is what is missing.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    drop(closed);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let models = serde_json::json!([]);
    let (mut child, mut stdin, messages) =
        acp_models(&scratch.0, closed_port, models, settings, &given);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    next(&messages);
    send(&mut stdin, open(2));
    let refused = until(&messages, |message| message["id"] == 2).0;
    assert_eq!(refused["error"]["code"], -32000, "{refused}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_signs_in_through_url_elicitations_and_logs_out() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-auth-{}", std::process::id())));
    let project = scratch.0.join("project");
    let set = scratch.0.join("acme");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&set).unwrap();
    // The login waits for `go` once its page is open, so that the test sees it waiting.
    let login = set.join("login");
    std::fs::write(
        &login,
        r#"#!/bin/sh
read -r request
case "$request" in
*'"action":"login"'*) echo '{"say":"Waiting for Acme."}'; echo '{"open":"https://sign-in.example/start"}'
  while [ ! -f go ]; do sleep 0.05; done; rm go
  echo '{"credential":{"access":"acme-secret-token","expires":99999999999999}}' ;;
*'"action":"models"'*) echo '{"provider":{"baseUrl":"http://127.0.0.1:9","api":"anthropic-messages","models":[{"id":"acme-model"}]}}' ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&login, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let declared =
        serde_json::json!({"logins": {"acme": {"name": "Acme", "command": ["./login"]}}});
    std::fs::write(set.join("models.json"), declared.to_string()).unwrap();
    let (port, _requests) = serve_refusals();
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"commandSets": [set], "hideCredentials": "never"});
    let given = ["--no-session"];
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &given);
    let mut seen = Vec::new();
    let mut wait = |found: &dyn Fn(&serde_json::Value) -> bool| {
        let (answer, before) = until(&messages, found);
        seen.extend(before);
        seen.push(answer.clone());
        answer
    };
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities":
                {"elicitation": {"url": {}}}}}),
    );
    let initialized = wait(&|message| message["id"] == 1);
    let result = &initialized["result"];
    assert_eq!(
        result["agentCapabilities"]["auth"]["logout"],
        serde_json::json!({})
    );
    let ids: Vec<&str> = (result["authMethods"].as_array().unwrap().iter())
        .map(|method| method["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["chatgpt", "copilot", "acme"], "{result}");
    assert_eq!(result["authMethods"][2]["name"], "Acme");
    let authenticate = |id: u64, method: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "authenticate",
            "params": {"methodId": method}})
    };
    send(&mut stdin, authenticate(2, "nowhere"));
    let unknown = wait(&|message| message["id"] == 2);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("nowhere"),
        "{unknown}"
    );

    // The page goes to the client, with what the program said before it.
    send(&mut stdin, authenticate(3, "acme"));
    let asked = wait(&|message| message["method"] == "elicitation/create");
    let params = &asked["params"];
    assert_eq!(
        (
            &params["requestId"],
            &params["mode"],
            &params["url"],
            &params["message"]
        ),
        (
            &serde_json::json!(3),
            &serde_json::json!("url"),
            &serde_json::json!("https://sign-in.example/start"),
            &serde_json::json!("Waiting for Acme.")
        ),
        "{asked}"
    );
    let elicitation = params["elicitationId"].clone();
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": asked["id"], "result": {"action": "accept"}}),
    );
    std::fs::write(set.join("go"), "").unwrap();
    let completed = wait(&|message| message["method"] == "elicitation/complete");
    assert_eq!(completed["params"]["elicitationId"], elicitation);
    let signed_in = wait(&|message| message["id"] == 3);
    assert_eq!(signed_in["result"], serde_json::json!({}), "{signed_in}");
    let auth_file = scratch.0.join("config/scramjet/auth.json");
    let auth: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&auth_file).unwrap()).unwrap();
    assert_eq!(auth["acme"]["access"], "acme-secret-token", "{auth}");
    // A session started now offers the login's models.
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let opened = wait(&|message| message["id"] == 4);
    assert!(
        opened["result"].to_string().contains("acme/acme-model"),
        "{opened}"
    );

    // Declining the page, or cancelling the request, stops the sign-in.
    send(&mut stdin, authenticate(5, "acme"));
    let asked = wait(&|message| message["method"] == "elicitation/create");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": asked["id"], "result": {"action": "decline"}}),
    );
    let declined = wait(&|message| message["id"] == 5);
    assert_eq!(declined["error"]["code"], -32800, "{declined}");
    send(&mut stdin, authenticate(6, "acme"));
    wait(&|message| message["method"] == "elicitation/create");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "method": "$/cancel_request",
            "params": {"requestId": 6}}),
    );
    let cancelled = wait(&|message| message["id"] == 6);
    assert_eq!(cancelled["error"]["code"], -32800, "{cancelled}");

    // Logging out of one method removes its login only.
    let mut saved =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&auth_file).unwrap()).unwrap();
    saved["github-copilot"] = serde_json::json!({"type": "oauth", "access": "gh", "expires": 1});
    std::fs::write(&auth_file, saved.to_string()).unwrap();
    let meta =
        |method: &str| serde_json::json!({"io.github.leanandmean.scramjet": {"methodId": method}});
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "logout",
            "params": {"_meta": meta("acme")}}),
    );
    assert_eq!(
        wait(&|message| message["id"] == 7)["result"],
        serde_json::json!({})
    );
    let auth: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&auth_file).unwrap()).unwrap();
    assert!(
        auth.get("acme").is_none() && auth.get("github-copilot").is_some(),
        "{auth}"
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 8, "method": "logout", "params": {}}),
    );
    assert_eq!(
        wait(&|message| message["id"] == 8)["result"],
        serde_json::json!({})
    );
    let auth: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&auth_file).unwrap()).unwrap();
    assert!(auth.get("github-copilot").is_none(), "{auth}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    seen.extend(messages.try_iter());
    for message in &seen {
        assert!(!message.to_string().contains("acme-secret"), "{message}");
    }

    // A client that opens no pages is told to sign in from a terminal.
    let given = ["--no-session"];
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &given);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {"elicitation": {"form": {}}}}}),
    );
    next(&messages);
    send(&mut stdin, authenticate(2, "copilot"));
    let refused = next(&messages);
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/login"),
        "{refused}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_runs_bang_commands_between_and_during_turns_through_the_extension() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-bash-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let reply = [
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    // The first reply is held, so that a command runs and ends during its turn.
    let (port, bodies) = serve_events_after(vec![reply.clone(), reply], |n| match n {
        0 => Duration::from_millis(1000),
        _ => Duration::ZERO,
    });
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 100000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities":
                {"_meta": {"io.github.leanandmean.scramjet": {}}}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    let bash = |id: u64, command: &str, exclude: bool| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": "_io.github.leanandmean.scramjet/bash",
            "params": {"sessionId": session, "command": command, "exclude": exclude}})
    };
    let prompt = |id: u64, text: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}})
    };
    let answered = |messages: &std::sync::mpsc::Receiver<serde_json::Value>| {
        until(messages, |message| message.get("id").is_some())
    };

    send(&mut stdin, bash(3, "pwd; exit 3", false));
    let (ran, before) = answered(&messages);
    assert_eq!(ran["id"], 3, "{ran}");
    let output = project.canonicalize().unwrap().display().to_string();
    assert_eq!(
        ran["result"],
        serde_json::json!({"output": output, "exitCode": 3, "cancelled": false}),
        "{ran}"
    );
    let told: Vec<_> = (before.iter())
        .filter(|message| message["method"] == "_io.github.leanandmean.scramjet/user_command")
        .map(|message| &message["params"])
        .collect();
    assert_eq!(told.len(), 2, "{before:?}");
    assert_eq!(told[0]["command"], "pwd; exit 3");
    assert_eq!(told[0]["include"], true);
    assert!(told[0].get("result").is_none(), "{}", told[0]);
    assert_eq!(told[1]["result"], ran["result"]);
    send(&mut stdin, bash(4, "echo secret", true));
    assert_eq!(answered(&messages).0["result"]["exitCode"], 0);

    send(&mut stdin, bash(5, "sleep 30", false));
    send(&mut stdin, bash(6, "true", false));
    let (refused, _) = answered(&messages);
    assert_eq!(refused["id"], 6, "{refused}");
    assert_eq!(refused["error"]["message"], "a command is running");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 7,
            "method": "_io.github.leanandmean.scramjet/abort_bash",
            "params": {"sessionId": session}}),
    );
    let started = Instant::now();
    let (first, _) = answered(&messages);
    let (second, _) = answered(&messages);
    let mut both = [first, second];
    both.sort_by_key(|message| message["id"].as_u64());
    assert_eq!(both[0]["id"], 5);
    assert_eq!(both[0]["result"]["cancelled"], true, "{}", both[0]);
    assert_eq!(both[1]["result"], serde_json::json!({}), "{}", both[1]);
    assert!(started.elapsed() < Duration::from_secs(10));

    send(&mut stdin, prompt(8, "first"));
    std::thread::sleep(Duration::from_millis(200));
    send(&mut stdin, bash(9, "echo during", false));
    let (during, _) = answered(&messages);
    assert_eq!(during["id"], 9, "{during}");
    assert_eq!(during["result"]["output"], "during");
    let (turned, _) = answered(&messages);
    assert_eq!(turned["result"]["stopReason"], "end_turn", "{turned}");
    send(&mut stdin, prompt(10, "second"));
    assert_eq!(answered(&messages).0["result"]["stopReason"], "end_turn");
    let first = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    let second = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    let messages = |body: &str| {
        serde_json::from_str::<serde_json::Value>(body).unwrap()["messages"].to_string()
    };
    let (first, second) = (messages(&first), messages(&second));
    assert!(first.contains("Ran `pwd; exit 3`"), "{first}");
    assert!(first.contains("Ran `sleep 30`"), "{first}");
    // `!!`'s output is never sent; one ending during a turn goes with the next prompt.
    assert!(
        !first.contains("secret") && !first.contains("during"),
        "{first}"
    );
    assert!(second.contains("Ran `echo during`"), "{second}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_forks_branches_and_lists_the_tree_through_the_extensions() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-fork-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let reply = |text: &str| {
        [
            r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#.to_owned(),
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_owned(),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}})
            .to_string(),
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#.to_owned(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ]
        .to_vec()
    };
    // The third reply is held, so that a branch comes during its turn.
    let replies = vec![reply("One."), reply("Two."), reply("Three.")];
    let (port, bodies) = serve_events_after(replies, |n| match n {
        2 => Duration::from_millis(500),
        _ => Duration::ZERO,
    });
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1,
                "clientCapabilities": {"_meta": {"io.github.leanandmean.scramjet": {}}}}}),
    );
    let offered = &next(&messages)["result"]["_meta"]["io.github.leanandmean.scramjet"];
    let methods = offered["methods"].as_array().unwrap();
    for method in ["fork", "branch", "tree"] {
        let method = format!("_io.github.leanandmean.scramjet/{method}");
        assert!(methods.iter().any(|m| *m == method), "{offered}");
    }
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let first = next(&messages)["result"]["sessionId"].clone();
    let answer = |messages: &std::sync::mpsc::Receiver<serde_json::Value>| {
        until(messages, |message| message.get("id").is_some())
    };
    let prompt = |id: u64, session: &serde_json::Value, text: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}})
    };
    let extension = |id: u64, method: &str, params: serde_json::Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": format!("_io.github.leanandmean.scramjet/{method}"), "params": params})
    };
    for (id, text) in [(3, "first"), (4, "second")] {
        send(&mut stdin, prompt(id, &first, text));
        assert_eq!(answer(&messages).0["result"]["stopReason"], "end_turn");
    }
    send(
        &mut stdin,
        extension(5, "tree", serde_json::json!({"sessionId": first})),
    );
    let tree = answer(&messages).0["result"].clone();
    let all = tree["entries"].as_array().unwrap();
    assert_eq!(all[0]["kind"], "model", "{tree}");
    assert_eq!(all[0]["model"], "test/claude-test", "{tree}");
    let entries: Vec<_> = (all.iter())
        .filter(|entry| entry["kind"] != "model" && entry["kind"] != "thoughtLevel")
        .collect();
    let shown: Vec<_> = (entries.iter())
        .map(|entry| (entry["kind"].as_str().unwrap(), entry["text"].as_str()))
        .collect();
    assert_eq!(
        shown,
        [
            ("prompt", Some("first")),
            ("reply", Some("One.")),
            ("prompt", Some("second")),
            ("reply", Some("Two.")),
        ],
        "{tree}"
    );
    assert!(all.iter().all(|entry| entry["active"] == true), "{tree}");
    assert_eq!(tree["currentId"], entries[3]["entryId"], "{tree}");
    assert_eq!(entries[2]["parentId"], entries[1]["entryId"], "{tree}");

    let at = |entry: &&serde_json::Value| {
        serde_json::json!({"sessionId": first,
        "entryId": entry["entryId"]})
    };
    send(&mut stdin, extension(6, "fork", at(&entries[1])));
    let refused = answer(&messages).0;
    let line = &entries[1]["entryId"];
    assert_eq!(
        refused["error"]["message"],
        format!("no prompt at entry {line}"),
        "{refused}"
    );
    send(&mut stdin, extension(7, "fork", at(&entries[2])));
    let (forked, before) = answer(&messages);
    let changed = (before.iter())
        .find(|message| message["method"] == "_io.github.leanandmean.scramjet/session_changed")
        .unwrap_or_else(|| panic!("{before:?}"));
    let fork = forked["result"]["sessionId"].clone();
    assert_eq!(changed["params"]["sessionId"], first, "{changed}");
    assert_eq!(changed["params"]["newSessionId"], fork, "{changed}");
    assert_ne!(fork, first);
    assert_eq!(forked["result"]["prompt"], "second", "{forked}");
    send(
        &mut stdin,
        extension(8, "tree", serde_json::json!({"sessionId": fork})),
    );
    let tree = answer(&messages).0;
    let prompts: Vec<_> = (tree["result"]["entries"].as_array().unwrap().iter())
        .filter_map(|entry| entry["text"].as_str())
        .collect();
    assert_eq!(prompts, ["first", "One."], "{tree}");

    send(&mut stdin, prompt(9, &fork, "/fork 1"));
    let refused = answer(&messages).0;
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(message.ends_with("scramjet/fork forks"), "{refused}");
    send(&mut stdin, prompt(10, &fork, "third"));
    send(
        &mut stdin,
        extension(11, "branch", serde_json::json!({"sessionId": fork})),
    );
    let refused = answer(&messages).0;
    assert_eq!(refused["id"], 11, "{refused}");
    assert_eq!(refused["error"]["message"], "a turn is running");
    assert_eq!(answer(&messages).0["result"]["stopReason"], "end_turn");
    let bodies: Vec<String> =
        std::iter::from_fn(|| bodies.recv_timeout(Duration::from_secs(2)).ok()).collect();
    let last = bodies.last().unwrap();
    assert!(
        last.contains("first") && last.contains("third") && !last.contains("second"),
        "{last}"
    );

    send(
        &mut stdin,
        extension(
            12,
            "branch",
            serde_json::json!({"sessionId": fork, "name": "Copy"}),
        ),
    );
    let (branched, _) = answer(&messages);
    let branch = branched["result"]["sessionId"].clone();
    assert!(branch.is_string() && branch != fork, "{branched}");
    let info = &next(&messages)["params"]["update"];
    assert_eq!(info["sessionUpdate"], "session_info_update", "{info}");
    assert_eq!(info["title"], "Copy");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 13, "method": "session/list",
            "params": {"cwd": project}}),
    );
    let listed = answer(&messages).0;
    let sessions = listed["result"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 3, "{listed}");
    assert_eq!(sessions[0]["sessionId"], branch, "{listed}");
    assert_eq!(sessions[0]["title"], "Copy", "{listed}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_labels_and_moves_in_the_tree_through_the_extensions() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-move-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let ended = |stop: &str| {
        [
            r#"{"type":"content_block_stop","index":0}"#.to_owned(),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": stop},
                "usage": {"output_tokens": 4}})
            .to_string(),
            r#"{"type":"message_stop"}"#.to_owned(),
        ]
    };
    let start = r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#;
    let reply = |text: &str| {
        let mut events = vec![
            start.to_owned(),
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#
                .to_owned(),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}})
            .to_string(),
        ];
        events.extend(ended("end_turn"));
        events
    };
    let question = serde_json::json!({"type": "confirm", "message": "Ready?"}).to_string();
    let mut asks = vec![
        start.to_owned(),
        serde_json::json!({"type": "content_block_start", "index": 0, "content_block":
            {"type": "tool_use", "id": "toolu_1", "name": "get_scramjet_user_input", "input": {}}})
        .to_string(),
        serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": question}})
        .to_string(),
    ];
    asks.extend(ended("tool_use"));
    let replies = vec![
        reply("One."),
        asks,
        reply("Two."),
        reply("Again."),
        reply("Summary."),
        reply("Three."),
    ];
    let (port, bodies) = serve_events(replies);
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 100000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    let capabilities = serde_json::json!({"elicitation": {"form": {}},
        "session": {"compaction": {}}, "_meta": {"io.github.leanandmean.scramjet": {}}});
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": capabilities}}),
    );
    let offered = &next(&messages)["result"]["_meta"]["io.github.leanandmean.scramjet"];
    let methods = offered["methods"].as_array().unwrap();
    for method in ["navigate_tree", "set_label"] {
        let method = format!("_io.github.leanandmean.scramjet/{method}");
        assert!(methods.iter().any(|m| *m == method), "{offered}");
    }
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    // The answer to request `id`, the questions asked meanwhile confirmed; with the messages
    // before it and how many questions were asked.
    let answer = |stdin: &mut std::process::ChildStdin, id: u64| {
        let (mut before, mut asked) = (Vec::new(), 0);
        loop {
            let message = next(&messages);
            if message["method"] == "elicitation/create" {
                asked += 1;
                let result = serde_json::json!({"action": "accept", "content": {"answer": true}});
                send(
                    stdin,
                    serde_json::json!({"jsonrpc": "2.0", "id": message["id"], "result": result}),
                );
            } else if message["id"] == id && message.get("method").is_none() {
                return (message, before, asked);
            } else {
                before.push(message);
            }
        }
    };
    let prompt = |id: u64, text: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]}})
    };
    let extension = |id: u64, method: &str, mut params: serde_json::Value| {
        params["sessionId"] = session.clone();
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": format!("_io.github.leanandmean.scramjet/{method}"), "params": params})
    };
    send(&mut stdin, prompt(3, "first"));
    assert_eq!(answer(&mut stdin, 3).0["result"]["stopReason"], "end_turn");
    send(&mut stdin, prompt(4, "second"));
    let (ended, _, asked) = answer(&mut stdin, 4);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert_eq!(asked, 1);
    let tree = |stdin: &mut std::process::ChildStdin, id: u64| {
        send(stdin, extension(id, "tree", serde_json::json!({})));
        let tree = answer(stdin, id).0["result"].clone();
        let entries: Vec<_> = (tree["entries"].as_array().unwrap().iter())
            .filter(|entry| entry["kind"] != "model" && entry["kind"] != "thoughtLevel")
            .cloned()
            .collect();
        (entries, tree["currentId"].clone())
    };
    let (entries, current) = tree(&mut stdin, 5);
    let kinds: Vec<_> = (entries.iter())
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["prompt", "reply", "prompt", "toolResult", "reply"],
        "{entries:?}"
    );
    let id = |at: usize| entries[at]["entryId"].clone();

    send(
        &mut stdin,
        extension(
            6,
            "set_label",
            serde_json::json!({"entryId": id(2), "label": " here "}),
        ),
    );
    assert_eq!(answer(&mut stdin, 6).0["result"]["label"], "here");
    send(
        &mut stdin,
        extension(
            7,
            "set_label",
            serde_json::json!({"entryId": 9999, "label": "x"}),
        ),
    );
    let refused = answer(&mut stdin, 7).0;
    assert_eq!(refused["error"]["code"], -32602, "{refused}");
    let (labeled, _) = tree(&mut stdin, 8);
    assert_eq!(labeled[2]["label"], "here", "{labeled:?}");
    send(
        &mut stdin,
        extension(9, "set_label", serde_json::json!({"entryId": id(2)})),
    );
    assert_eq!(
        answer(&mut stdin, 9).0["result"]["label"],
        serde_json::Value::Null
    );
    let (unlabeled, _) = tree(&mut stdin, 10);
    assert!(unlabeled[2].get("label").is_none(), "{unlabeled:?}");

    send(
        &mut stdin,
        extension(11, "navigate_tree", serde_json::json!({"entryId": current})),
    );
    assert_eq!(answer(&mut stdin, 11).0["result"]["moved"], false);

    // Back at the question, it is asked again and its turn goes on.
    send(
        &mut stdin,
        extension(12, "navigate_tree", serde_json::json!({"entryId": id(3)})),
    );
    let (ended, before, asked) = answer(&mut stdin, 12);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    assert_eq!(asked, 1, "{before:?}");
    let said = |before: &[serde_json::Value], text: &str| {
        before
            .iter()
            .any(|message| message["params"]["update"]["content"]["text"] == text)
    };
    assert!(said(&before, "Again."), "{before:?}");

    // The branch left summarized: shown as a compaction marked as a branch's.
    send(
        &mut stdin,
        extension(
            13,
            "navigate_tree",
            serde_json::json!({"entryId": id(1), "summarize": true}),
        ),
    );
    let (moved, before, _) = answer(&mut stdin, 13);
    assert_eq!(moved["result"]["moved"], true, "{moved}");
    assert_eq!(
        moved["result"]["prompt"],
        serde_json::Value::Null,
        "{moved}"
    );
    let compactions: Vec<_> = (before.iter())
        .map(|message| &message["params"]["update"])
        .filter(|update| update["sessionUpdate"] == "compaction_update")
        .collect();
    assert_eq!(compactions.len(), 2, "{before:?}");
    assert_eq!(compactions[0]["status"], "in_progress");
    assert_eq!(compactions[1]["status"], "completed");
    for update in &compactions {
        assert_eq!(
            update["_meta"]["io.github.leanandmean.scramjet"]["branch"], true,
            "{update}"
        );
    }
    let summary = compactions[1]["summary"][0]["text"].as_str().unwrap();
    assert!(summary.contains("Summary."), "{summary}");

    // At a prompt, it is given back to edit.
    send(
        &mut stdin,
        extension(14, "navigate_tree", serde_json::json!({"entryId": id(2)})),
    );
    let moved = answer(&mut stdin, 14).0;
    assert_eq!(moved["result"]["moved"], true, "{moved}");
    assert_eq!(moved["result"]["prompt"], "second", "{moved}");
    send(&mut stdin, prompt(15, "third"));
    assert_eq!(answer(&mut stdin, 15).0["result"]["stopReason"], "end_turn");
    let bodies: Vec<String> =
        std::iter::from_fn(|| bodies.recv_timeout(Duration::from_secs(2)).ok()).collect();
    assert_eq!(bodies.len(), 6);
    let last = bodies.last().unwrap();
    assert!(
        last.contains("first") && last.contains("third") && !last.contains("second"),
        "{last}"
    );
    send(&mut stdin, prompt(16, "/tree"));
    let refused = answer(&mut stdin, 16).0;
    let message = refused["error"]["message"].as_str().unwrap();
    assert!(
        message.ends_with("scramjet/navigate_tree moves"),
        "{refused}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_exports_sessions_and_gives_the_usage_through_the_extensions() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-export-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let reply = [
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    let (port, _bodies) = serve_events_after(vec![reply], |_| Duration::ZERO);
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1, "clientCapabilities": {}}}),
    );
    let offered = &next(&messages)["result"]["_meta"]["io.github.leanandmean.scramjet"];
    let methods = offered["methods"].as_array().unwrap();
    for method in ["export_html", "usage"] {
        let method = format!("_io.github.leanandmean.scramjet/{method}");
        assert!(methods.iter().any(|m| *m == method), "{offered}");
    }
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let session = next(&messages)["result"]["sessionId"].clone();
    let export = |id: u64, path: Option<&str>| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": "_io.github.leanandmean.scramjet/export_html",
            "params": {"sessionId": session, "path": path}})
    };
    let answer = |messages: &std::sync::mpsc::Receiver<serde_json::Value>| {
        until(messages, |message| message.get("id").is_some()).0
    };
    send(&mut stdin, export(3, None));
    let nothing = answer(&messages);
    assert_eq!(
        nothing["result"],
        serde_json::json!({"path": null}),
        "{nothing}"
    );
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}}),
    );
    assert_eq!(answer(&messages)["result"]["stopReason"], "end_turn");
    send(&mut stdin, export(5, None));
    let page = answer(&messages);
    let page = std::path::PathBuf::from(page["result"]["path"].as_str().unwrap());
    let id = session.as_str().unwrap();
    assert_eq!(page.parent().unwrap(), project.canonicalize().unwrap());
    assert_eq!(
        page.file_name().unwrap().to_str().unwrap(),
        format!("scramjet-session-{id}.html")
    );
    assert!(std::fs::read_to_string(&page).unwrap().contains("Hello."));
    send(&mut stdin, export(6, Some("copy.jsonl")));
    let copy = answer(&messages);
    let copy = std::path::PathBuf::from(copy["result"]["path"].as_str().unwrap());
    assert_eq!(copy, project.canonicalize().unwrap().join("copy.jsonl"));
    assert!(std::fs::read_to_string(&copy).unwrap().contains("Hello."));
    send(&mut stdin, export(7, Some("missing/x.html")));
    assert_eq!(answer(&messages)["error"]["code"], -32603);

    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 8,
            "method": "_io.github.leanandmean.scramjet/usage", "params": {}}),
    );
    let usage = &answer(&messages)["result"];
    assert_eq!(usage["today"]["requests"], 1, "{usage}");
    assert_eq!(usage["today"]["outputTokens"], 4, "{usage}");
    assert_eq!(usage["last30Days"]["requests"], 1, "{usage}");
    assert_eq!(usage["models"][0]["model"], "test/claude-test", "{usage}");
    assert!(usage["quotas"].as_array().unwrap().is_empty(), "{usage}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 9, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "/usage"}]}}),
    );
    let refused = answer(&messages);
    let said = refused["error"]["message"].as_str().unwrap();
    assert!(
        said.contains("_io.github.leanandmean.scramjet/usage"),
        "{refused}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_fresh_session_steps_change_the_session_for_clients_that_follow() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-fresh-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    let flow = project.join(".scramjet/flow/commands");
    std::fs::create_dir_all(&flow).unwrap();
    let step = "---\nnext:\n  mode: closed\n  candidates:\n    - name: flow:done\n---\nStep.\n";
    std::fs::write(flow.join("flow:step.md"), step).unwrap();
    std::fs::write(flow.join("flow:done.md"), "Finish.\n").unwrap();
    let autonomy = "edges:\n  flow:step:\n    flow:done: chain\n";
    std::fs::write(project.join(".scramjet/autonomy.yaml"), autonomy).unwrap();
    for follows in [true, false] {
        let (port, _bodies) = serve_events(vec![
            report_events(
                "Stepped.",
                serde_json::json!({"status": "completed", "summary": "Stepped.",
                    "next_steps": [{"message": "/flow:done", "fresh_session": true,
                        "reason": "Finish."}],
                    "recommended_next_step": 0}),
            ),
            report_events(
                "All done.",
                serde_json::json!({"status": "completed", "summary": "Done."}),
            ),
        ]);
        let (mut child, mut stdin, messages) = acp(&scratch.0, port);
        let capabilities = match follows {
            true => serde_json::json!({"_meta": {"io.github.leanandmean.scramjet": {}}}),
            false => serde_json::json!({}),
        };
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": 1, "clientCapabilities": capabilities}}),
        );
        next(&messages);
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": project, "mcpServers": []}}),
        );
        let session = next(&messages)["result"]["sessionId"].clone();
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
                "params": {"sessionId": session,
                    "prompt": [{"type": "text", "text": "/flow:step"}]}}),
        );
        let (answer, before) = until(&messages, |message| message.get("id").is_some());
        assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
        let changed = (before.iter()).position(|message| {
            message["method"] == "_io.github.leanandmean.scramjet/session_changed"
        });
        let ids = |messages: &[serde_json::Value]| {
            let mut ids: Vec<_> = (messages.iter())
                .filter(|message| message["method"] == "session/update")
                .map(|message| message["params"]["sessionId"].clone())
                .collect();
            ids.dedup();
            ids
        };
        let texts: Vec<&str> = (before.iter())
            .filter_map(|message| message["params"]["update"]["content"]["text"].as_str())
            .collect();
        assert!(texts.contains(&"/flow:done"), "{texts:?}");
        assert!(texts.contains(&"All done."), "{texts:?}");
        let fresh = match follows {
            true => {
                let at = changed.unwrap_or_else(|| panic!("{before:?}"));
                let params = &before[at]["params"];
                assert_eq!(params["sessionId"], session, "{params}");
                let fresh = params["newSessionId"].clone();
                assert_ne!(fresh, session);
                assert_eq!(ids(&before[..at]), std::slice::from_ref(&session));
                assert_eq!(ids(&before[at..]), std::slice::from_ref(&fresh));
                fresh
            }
            false => {
                assert_eq!(changed, None, "{before:?}");
                assert_eq!(ids(&before), std::slice::from_ref(&session));
                assert!(
                    texts
                        .iter()
                        .any(|text| text.starts_with("✓ New session started")),
                    "{texts:?}"
                );
                session.clone()
            }
        };
        // The new id reaches the session, and with a follower so does the old one.
        for id in [&fresh, &session] {
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
                    "params": {"sessionId": id, "prompt": [{"type": "text", "text": "/model"}]}}),
            );
            let refused = next(&messages);
            assert!(
                refused["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("built-in"),
                "{refused}"
            );
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
    }
}

/// A scripted Claude reply: `text`, then a `report_scramjet_command_status` call with `input`.
#[cfg(unix)]
fn report_events(text: &str, input: serde_json::Value) -> Vec<String> {
    let mut events = vec![
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#.to_owned(),
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#.to_owned(),
        serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text}})
        .to_string(),
        r#"{"type":"content_block_stop","index":0}"#.to_owned(),
        serde_json::json!({"type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_1",
                "name": "report_scramjet_command_status", "input": {}}})
        .to_string(),
        serde_json::json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": input.to_string()}})
        .to_string(),
        r#"{"type":"content_block_stop","index":1}"#.to_owned(),
    ];
    events.extend([
        r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":4}}"#.to_owned(),
        r#"{"type":"message_stop"}"#.to_owned(),
    ]);
    events
}

#[test]
#[cfg(unix)]
fn acp_offers_next_steps_with_the_extension_or_as_text_and_counts_down_under_autopilot() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-next-{}", std::process::id())));
    let project = scratch.0.join("project");
    let flow = project.join(".scramjet/flow/commands");
    std::fs::create_dir_all(&flow).unwrap();
    let step = "---\nnext:\n  mode: closed\n  candidates:\n    - name: flow:done\n---\nStep.\n";
    std::fs::write(flow.join("flow:step.md"), step).unwrap();
    std::fs::write(flow.join("flow:done.md"), "Finish.\n").unwrap();
    let stepped = report_events(
        "Stepped.",
        serde_json::json!({"status": "completed", "summary": "Stepped.",
            "next_steps": [{"message": "/flow:done", "fresh_session": false, "reason": "Finish."}],
            "recommended_next_step": 0}),
    );
    let done = report_events(
        "All done.",
        serde_json::json!({"status": "completed", "summary": "Done."}),
    );
    let instead = [
        r#"{"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Instead."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    let scramjet = serde_json::json!({"_meta": {"io.github.leanandmean.scramjet": {}}});
    let booleans = serde_json::json!({"session": {"configOptions": {"boolean": {}}}});
    // A session in `project` for a client offering `capabilities`, autopilot set to `autopilot`
    // when given, and `/flow:step` sent: the process, its stdin and messages, the session's id,
    // the provider's bodies.
    let start = |capabilities: &serde_json::Value,
                 autopilot: Option<serde_json::Value>,
                 replies: Vec<Vec<String>>| {
        let (port, bodies) = serve_events(replies);
        let (child, mut stdin, messages) = acp(&scratch.0, port);
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": 1, "clientCapabilities": capabilities}}),
        );
        next(&messages);
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": project, "mcpServers": []}}),
        );
        let started = next(&messages);
        let session = started["result"]["sessionId"].clone();
        let options = &started["result"]["configOptions"];
        let autopilot_of = |options: &serde_json::Value| {
            (options.as_array().unwrap().iter())
                .find(|option| option["id"] == "autopilot")
                .cloned()
                .unwrap()
        };
        autopilot_of(options);
        if let Some(value) = autopilot {
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0", "id": 5, "method": "session/set_config_option",
                    "params": {"sessionId": session, "configId": "autopilot", "value": value}}),
            );
            let (set, _) = until(&messages, |message| message["id"] == 5);
            assert!(set["result"]["configOptions"].is_array(), "{set}");
            let current = &autopilot_of(&set["result"]["configOptions"])["currentValue"];
            assert!(*current == true || *current == "on", "{set}");
        }
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
                "params": {"sessionId": session,
                    "prompt": [{"type": "text", "text": "/flow:step"}]}}),
        );
        (child, stdin, messages, session, bodies)
    };
    let is_list = |message: &serde_json::Value| {
        message["method"] == "_io.github.leanandmean.scramjet/next_step"
    };
    let texts = |messages: &[serde_json::Value]| -> Vec<String> {
        (messages.iter())
            .filter_map(|message| message["params"]["update"]["content"]["text"].as_str())
            .map(str::to_owned)
            .collect()
    };

    // The extension: the step chosen runs; a list cancelled ends the turn.
    for (answer, runs) in [
        (serde_json::json!({"index": 0}), true),
        (serde_json::json!({"cancelled": true}), false),
    ] {
        let (mut child, mut stdin, messages, session, bodies) =
            start(&scramjet, None, vec![stepped.clone(), done.clone()]);
        let (list, _) = until(&messages, is_list);
        let params = &list["params"];
        assert_eq!(params["sessionId"], session);
        assert_eq!(params["kind"], "nextStep");
        assert_eq!(params["recommended"], 0);
        assert_eq!(params["model"], "test/claude-test");
        assert_eq!(params.get("autoselectMs"), None, "{list}");
        let offered = serde_json::json!([{"message": "/flow:done", "reason": "Finish.",
            "freshSession": false}]);
        assert_eq!(params["steps"], offered);
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": list["id"], "result": answer}),
        );
        let (ended, after) = until(&messages, |message| message["id"] == 3);
        assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
        let shown = texts(&after);
        assert_eq!(
            shown.iter().any(|text| text == "All done."),
            runs,
            "{shown:?}"
        );
        drop(stdin);
        assert!(child.wait().unwrap().success());
        let bodies: Vec<String> = bodies.try_iter().collect();
        assert_eq!(bodies.len(), if runs { 2 } else { 1 });
        if runs {
            assert!(bodies[1].contains("→ /flow:done"), "{}", bodies[1]);
        }
    }

    // Under autopilot the countdown takes the recommended step and withdraws the request,
    // unless the client holds it.
    for hold in [false, true] {
        let (mut child, mut stdin, messages, session, _bodies) = start(
            &scramjet,
            Some(serde_json::json!("on")),
            vec![stepped.clone(), done.clone()],
        );
        let (list, _) = until(&messages, is_list);
        assert_eq!(list["params"]["autoselectMs"], 3000, "{list}");
        if hold {
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0",
                    "method": "_io.github.leanandmean.scramjet/hold",
                    "params": {"sessionId": session}}),
            );
            std::thread::sleep(Duration::from_secs(4));
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0", "id": list["id"],
                    "result": {"cancelled": true}}),
            );
        }
        let (ended, after) = until(&messages, |message| message["id"] == 3);
        assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
        let withdrawn = (after.iter()).any(|message| {
            message["method"] == "$/cancel_request" && message["params"]["requestId"] == list["id"]
        });
        assert_eq!(withdrawn, !hold, "{after:?}");
        let shown = texts(&after);
        assert_eq!(
            shown.iter().any(|text| text == "All done."),
            !hold,
            "{shown:?}"
        );
        drop(stdin);
        assert!(child.wait().unwrap().success());
    }

    // Without the extension the list is said; autopilot's countdown runs the same, and a
    // prompt sent during it stops it and runs.
    for (autopilot, prompted) in [(None, false), (Some(true), false), (Some(true), true)] {
        let replies = match prompted {
            true => vec![stepped.clone(), instead.clone()],
            false => vec![stepped.clone(), done.clone()],
        };
        let capabilities = match autopilot {
            Some(_) => &booleans,
            None => &serde_json::json!({}),
        };
        let (mut child, mut stdin, messages, session, bodies) = start(
            capabilities,
            autopilot.map(serde_json::Value::Bool),
            replies,
        );
        let (list, _) = until(&messages, |message| {
            (message["params"]["update"]["content"]["text"].as_str())
                .is_some_and(|text| text.starts_with("Select next step:"))
        });
        let list = list["params"]["update"]["content"]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            list.contains("/flow:done (recommended) — Finish."),
            "{list}"
        );
        let counts = autopilot.is_some();
        assert_eq!(list.contains("runs in 3 s"), counts, "{list}");
        if prompted {
            send(
                &mut stdin,
                serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "session/prompt",
                    "params": {"sessionId": session,
                        "prompt": [{"type": "text", "text": "do this instead"}]}}),
            );
        }
        let (ended, after) = until(&messages, |message| message["id"] == 3);
        assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
        assert!(!after.iter().any(is_list), "{after:?}");
        let shown = texts(&after);
        let ran = shown.iter().any(|text| text == "All done.");
        assert_eq!(ran, counts && !prompted, "{shown:?}");
        if prompted {
            assert!(shown.iter().any(|text| text == "Instead."), "{shown:?}");
            assert_eq!(next(&messages)["result"]["stopReason"], "end_turn");
        }
        drop(stdin);
        assert!(child.wait().unwrap().success());
        let bodies: Vec<String> = bodies.try_iter().collect();
        assert_eq!(bodies.len(), if counts || prompted { 2 } else { 1 });
        if prompted {
            assert!(bodies[1].contains("do this instead"), "{}", bodies[1]);
        }
    }
}

#[test]
#[cfg(unix)]
fn acp_offers_again_the_next_step_list_a_fork_or_move_goes_back_to() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-reopen-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    let flow = project.join(".scramjet/flow/commands");
    std::fs::create_dir_all(&flow).unwrap();
    let step = "---\nnext:\n  mode: closed\n  candidates:\n    - name: flow:done\n---\nStep.\n";
    std::fs::write(flow.join("flow:step.md"), step).unwrap();
    std::fs::write(flow.join("flow:done.md"), "Finish.\n").unwrap();
    let stepped = report_events(
        "Stepped.",
        serde_json::json!({"status": "completed", "summary": "Stepped.",
            "next_steps": [{"message": "/flow:done", "fresh_session": false, "reason": "Finish."}],
            "recommended_next_step": 0}),
    );
    let done = report_events(
        "All done.",
        serde_json::json!({"status": "completed", "summary": "Done."}),
    );
    let other = [
        r#"{"type":"message_start","message":{"id":"msg_2","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Other."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    let (port, bodies) = serve_events(vec![stepped, other, done]);
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": 1,
                "clientCapabilities": {"_meta": {"io.github.leanandmean.scramjet": {}}}}}),
    );
    next(&messages);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
            "params": {"cwd": project, "mcpServers": []}}),
    );
    let first = next(&messages)["result"]["sessionId"].clone();
    let prompt = |id: u64, text: &str| {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/prompt",
            "params": {"sessionId": first, "prompt": [{"type": "text", "text": text}]}})
    };
    let extension = |id: u64, method: &str, params: serde_json::Value| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
            "method": format!("_io.github.leanandmean.scramjet/{method}"), "params": params})
    };
    let is_list = |message: &serde_json::Value| {
        message["method"] == "_io.github.leanandmean.scramjet/next_step"
    };
    let cancelled = |list: &serde_json::Value| serde_json::json!({"jsonrpc": "2.0", "id": list["id"], "result": {"cancelled": true}});
    send(&mut stdin, prompt(3, "/flow:step"));
    let (list, _) = until(&messages, is_list);
    send(&mut stdin, cancelled(&list));
    let (ended, _) = until(&messages, |message| message["id"] == 3);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    send(&mut stdin, prompt(4, "other"));
    let (ended, _) = until(&messages, |message| message["id"] == 4);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    send(
        &mut stdin,
        extension(5, "tree", serde_json::json!({"sessionId": first})),
    );
    let (tree, _) = until(&messages, |message| message["id"] == 5);
    let entry = (tree["result"]["entries"].as_array().unwrap().iter())
        .find(|entry| entry["kind"] == "prompt" && entry["text"] == "other")
        .unwrap_or_else(|| panic!("{tree}"))["entryId"]
        .clone();

    // A move back there offers the list again; cancelled, the session is left idle.
    send(
        &mut stdin,
        extension(
            6,
            "navigate_tree",
            serde_json::json!({"sessionId": first, "entryId": entry}),
        ),
    );
    let (list, _) = until(&messages, is_list);
    assert_eq!(
        list["params"]["steps"][0]["message"], "/flow:done",
        "{list}"
    );
    send(&mut stdin, cancelled(&list));
    let (moved, _) = until(&messages, |message| message["id"] == 6);
    assert_eq!(moved["result"]["moved"], true, "{moved}");
    assert_eq!(moved["result"]["prompt"], "other", "{moved}");
    assert_eq!(moved["result"].get("stopReason"), None, "{moved}");

    // A fork there offers it again too, and the step chosen runs in the fork.
    send(
        &mut stdin,
        extension(
            7,
            "fork",
            serde_json::json!({"sessionId": first, "entryId": entry}),
        ),
    );
    let (list, _) = until(&messages, is_list);
    let fork = list["params"]["sessionId"].clone();
    assert_ne!(fork, first, "{list}");
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": list["id"], "result": {"index": 0}}),
    );
    let (forked, during) = until(&messages, |message| message["id"] == 7);
    assert_eq!(forked["result"]["sessionId"], fork, "{forked}");
    assert_eq!(forked["result"]["prompt"], "other", "{forked}");
    assert_eq!(forked["result"]["stopReason"], "end_turn", "{forked}");
    let shown = |text: &str| {
        (during.iter()).any(|message| message["params"]["update"]["content"]["text"] == text)
    };
    assert!(shown("/flow:done") && shown("All done."), "{during:?}");
    send(
        &mut stdin,
        extension(8, "tree", serde_json::json!({"sessionId": fork})),
    );
    let (tree, _) = until(&messages, |message| message["id"] == 8);
    let texts: Vec<_> = (tree["result"]["entries"].as_array().unwrap().iter())
        .filter_map(|entry| entry["text"].as_str())
        .collect();
    assert!(!texts.contains(&"other"), "{tree}");
    assert_eq!(texts.last(), Some(&"All done."), "{tree}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let bodies: Vec<String> = bodies.try_iter().collect();
    assert_eq!(bodies.len(), 3);
    assert!(bodies[2].contains("→ /flow:done"), "{}", bodies[2]);
    assert!(!bodies[2].contains(r#""text":"other""#), "{}", bodies[2]);
}

#[test]
#[cfg(unix)]
fn acp_sets_the_model_and_thinking_level_between_and_during_turns() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-config-{}", std::process::id())),
    );
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let models = serde_json::json!([
        {"id": "claude-test", "contextWindow": 1000, "reasoning": true},
        {"id": "claude-other", "name": "Other", "contextWindow": 2000, "reasoning": true},
        {"id": "claude-plain", "contextWindow": 2000},
    ]);
    let booleans = serde_json::json!({"session": {"configOptions": {"boolean": {}}}});
    // A session with the provider at `port`: the process, its stdin and messages, the session's
    // id and the options it started with.
    let start = |port: u16| {
        let (child, mut stdin, messages) = acp_models(
            &scratch.0,
            port,
            models.clone(),
            serde_json::json!({"hideCredentials": "never"}),
            &[
                "--no-session",
                "--model",
                "test/claude-test",
                "--thinking",
                "low",
            ],
        );
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": 1, "clientCapabilities": booleans}}),
        );
        next(&messages);
        send(
            &mut stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "session/new",
                "params": {"cwd": project, "mcpServers": []}}),
        );
        let started = next(&messages);
        let session = started["result"]["sessionId"].clone();
        let options = started["result"]["configOptions"].clone();
        (child, stdin, messages, session, options)
    };
    let option = |options: &serde_json::Value, id: &str| {
        (options.as_array().unwrap().iter())
            .find(|option| option["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no {id} in {options}"))
    };
    let set = |stdin: &mut std::process::ChildStdin,
               id: u64,
               session: &serde_json::Value,
               option: &str,
               value: serde_json::Value| {
        send(
            stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "session/set_config_option",
                "params": {"sessionId": session, "configId": option, "value": value}}),
        );
    };

    // Set while a bash call runs, the model is taken at the turn's pause and answered before
    // the turn ends; the request after the call uses it.
    let (port, bodies) = serve_bash_call("sleep 1");
    let (mut child, mut stdin, messages, session, options) = start(port);
    let model = option(&options, "model");
    assert_eq!(model["currentValue"], "test/claude-test", "{options}");
    assert_eq!(model["category"], "model");
    let group = &model["options"][0];
    assert_eq!(group["group"], "test", "{options}");
    let names: Vec<_> = (group["options"].as_array().unwrap().iter())
        .map(|value| (value["value"].clone(), value["name"].clone()))
        .collect();
    assert_eq!(
        names,
        [
            ("test/claude-test".into(), "claude-test".into()),
            ("test/claude-other".into(), "Other".into()),
            ("test/claude-plain".into(), "claude-plain".into())
        ]
    );
    let level = option(&options, "thought_level");
    assert_eq!(level["currentValue"], "low", "{options}");
    let levels: Vec<_> = (level["options"].as_array().unwrap().iter())
        .map(|value| value["value"].as_str().unwrap())
        .collect();
    assert_eq!(levels, ["none", "minimal", "low", "medium", "high"]);
    assert_eq!(level["options"][0]["name"], "Off");
    assert_eq!(option(&options, "autopilot")["currentValue"], false);

    set(&mut stdin, 4, &session, "thought_level", "high".into());
    let (answer, _) = until(&messages, |message| message["id"] == 4);
    let options = &answer["result"]["configOptions"];
    assert_eq!(
        option(options, "thought_level")["currentValue"],
        "high",
        "{answer}"
    );
    let saved = std::fs::read_to_string(scratch.0.join("config/scramjet/settings.json")).unwrap();
    assert!(
        saved.contains(r#""defaultThinkingLevel": "high""#),
        "{saved}"
    );
    for (id, option, value) in [
        (5, "model", serde_json::json!("test/claude-none")),
        (6, "thought_level", "xhigh".into()),
        (7, "autopilot", "maybe".into()),
        (8, "colour", "red".into()),
    ] {
        set(&mut stdin, id, &session, option, value);
        let (answer, _) = until(&messages, |message| message["id"] == id);
        assert_eq!(answer["error"]["code"], -32602, "{answer}");
    }
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    until(&messages, |message| {
        message["params"]["update"]["status"] == "in_progress"
    });
    set(&mut stdin, 9, &session, "model", "test/claude-other".into());
    let (answer, _) = until(&messages, |message| message.get("id").is_some());
    assert_eq!(
        answer["id"], 9,
        "the change waits for the turn's end: {answer}"
    );
    let options = &answer["result"]["configOptions"];
    assert_eq!(
        option(options, "model")["currentValue"],
        "test/claude-other"
    );
    let (answer, updates) = until(&messages, |message| message["id"] == 3);
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    assert!(
        !(updates.iter())
            .any(|message| message["params"]["update"]["sessionUpdate"] == "config_option_update"),
        "the client's own change is in its answer only: {updates:?}"
    );
    let sent: Vec<serde_json::Value> = (0..2)
        .map(|_| {
            serde_json::from_str(&bodies.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
        })
        .collect();
    assert_eq!(sent[0]["model"], "claude-test");
    assert_eq!(sent[1]["model"], "claude-other");
    let saved = std::fs::read_to_string(scratch.0.join("config/scramjet/settings.json")).unwrap();
    assert!(saved.contains("claude-other"), "remembered: {saved}");
    drop(stdin);
    assert!(child.wait().unwrap().success());

    // The agent's own switch is told in a `config_option_update`. A model with no levels keeps
    // the level's option, with the model's own only.
    let (port, _bodies) = serve_call(
        "switch_scramjet_model",
        serde_json::json!({"provider": "test", "model": "claude-plain"}),
    );
    let (mut child, mut stdin, messages, session, _) = start(port);
    send(
        &mut stdin,
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "session/prompt",
            "params": {"sessionId": session, "prompt": [{"type": "text", "text": "go"}]}}),
    );
    let (answer, updates) = until(&messages, |message| message["id"] == 3);
    assert_eq!(answer["result"]["stopReason"], "end_turn", "{answer}");
    let update = (updates.iter())
        .map(|message| &message["params"]["update"])
        .find(|update| update["sessionUpdate"] == "config_option_update")
        .unwrap_or_else(|| panic!("no config_option_update: {updates:?}"));
    let options = &update["configOptions"];
    assert_eq!(
        option(options, "model")["currentValue"],
        "test/claude-plain",
        "{update}"
    );
    let level = option(options, "thought_level");
    assert_eq!(level["currentValue"], "default", "{update}");
    assert_eq!(
        level["options"],
        serde_json::json!([{"value": "default", "name": "Model's default"}])
    );
    for (id, value, code) in [(4, "default", None), (5, "high", Some(-32602))] {
        set(&mut stdin, id, &session, "thought_level", value.into());
        let (answer, _) = until(&messages, |message| message["id"] == id);
        assert_eq!(answer["error"]["code"].as_i64(), code, "{answer}");
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_lists_resumes_closes_and_deletes_saved_sessions() {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("scramjet-cli-acp-sessions-{}", std::process::id())),
    );
    let (project, other) = (scratch.0.join("project"), scratch.0.join("other"));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let (project, other) = (
        std::fs::canonicalize(&project).unwrap(),
        std::fs::canonicalize(&other).unwrap(),
    );
    let text = [
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":3,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Done."}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .map(str::to_owned)
    .to_vec();
    let (port, bodies) = serve_events(vec![text]);
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    let mut ids = 0;
    let mut request = |stdin: &mut std::process::ChildStdin, method: &str, params| {
        ids += 1;
        send(
            stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": ids, "method": method, "params": params}),
        );
        until(&messages, |message| message["id"] == ids)
    };
    let initialized = request(
        &mut stdin,
        "initialize",
        serde_json::json!({"protocolVersion": 1}),
    );
    let capabilities = &initialized.0["result"]["agentCapabilities"]["sessionCapabilities"];
    assert_eq!(
        *capabilities,
        serde_json::json!({"list": {}, "resume": {}, "close": {}, "delete": {}})
    );
    // Three sessions with a turn each, two in `project`, the newest last.
    let mut started = Vec::new();
    for (cwd, prompt) in [
        (&project, "first prompt\nmore of it"),
        (&other, "elsewhere"),
        (&project, "second"),
    ] {
        let params = serde_json::json!({"cwd": cwd, "mcpServers": []});
        let session = request(&mut stdin, "session/new", params).0["result"]["sessionId"].clone();
        let prompt = serde_json::json!([{"type": "text", "text": prompt}]);
        let params = serde_json::json!({"sessionId": session, "prompt": prompt});
        let (ended, _) = request(&mut stdin, "session/prompt", params);
        assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
        bodies.recv_timeout(Duration::from_secs(5)).unwrap();
        started.push(session);
        // Their files' times differ.
        std::thread::sleep(Duration::from_millis(20));
    }
    let answer = request(
        &mut stdin,
        "session/list",
        serde_json::json!({"cwd": project}),
    )
    .0;
    let sessions = answer["result"]["sessions"].as_array().unwrap().clone();
    assert_eq!(answer["result"].get("nextCursor"), None, "{answer}");
    let shown: Vec<_> = (sessions.iter())
        .map(|session| (session["sessionId"].clone(), session["title"].clone()))
        .collect();
    assert_eq!(
        shown,
        [
            (started[2].clone(), "second".into()),
            (started[0].clone(), "first prompt".into())
        ]
    );
    assert_eq!(sessions[0]["cwd"], project.to_str().unwrap());
    let updated = sessions[0]["updatedAt"].as_str().unwrap();
    assert!(
        updated.len() == 20 && updated.starts_with("20") && updated.ends_with('Z'),
        "{updated}"
    );
    let path = &sessions[0]["_meta"]["io.github.leanandmean.scramjet"]["path"];
    assert!(
        std::path::Path::new(path.as_str().unwrap()).is_file(),
        "{path}"
    );
    let answer = request(&mut stdin, "session/list", serde_json::json!({})).0;
    assert_eq!(answer["result"]["sessions"].as_array().unwrap().len(), 3);
    let answer = request(
        &mut stdin,
        "session/list",
        serde_json::json!({"cursor": "2"}),
    )
    .0;
    let sessions = answer["result"]["sessions"].as_array().unwrap().clone();
    assert_eq!(sessions.len(), 1, "{answer}");
    assert_eq!(sessions[0]["sessionId"], started[0]);
    let answer = request(
        &mut stdin,
        "session/list",
        serde_json::json!({"cursor": "page two"}),
    )
    .0;
    assert_eq!(answer["error"]["code"], -32602, "{answer}");

    // Closing twice answers both times; the closed session takes no prompts.
    for _ in 0..2 {
        let params = serde_json::json!({"sessionId": started[0]});
        let (answer, _) = request(&mut stdin, "session/close", params);
        assert_eq!(answer["result"], serde_json::json!({}), "{answer}");
    }
    let prompt = serde_json::json!([{"type": "text", "text": "again"}]);
    let params = serde_json::json!({"sessionId": started[0], "prompt": prompt});
    let (answer, _) = request(&mut stdin, "session/prompt", params);
    assert_eq!(answer["error"]["code"], -32002, "{answer}");
    assert_eq!(answer["error"]["data"]["sessionId"], started[0], "{answer}");

    // Resumed in another folder, unknown or not an id: refused, an unknown one named.
    let resume = |session: &serde_json::Value, cwd: &std::path::Path| serde_json::json!({"sessionId": session, "cwd": cwd, "mcpServers": []});
    for (params, code) in [
        (resume(&started[0], &other), -32602),
        (resume(&"1-2".into(), &project), -32002),
        (resume(&"../other".into(), &project), -32002),
        (resume(&started[2], &other), -32602),
    ] {
        for method in ["session/resume", "session/load"] {
            let (answer, _) = request(&mut stdin, method, params.clone());
            assert_eq!(answer["error"]["code"], code, "{params}: {answer}");
            if code == -32002 {
                assert_eq!(answer["error"]["data"]["sessionId"], params["sessionId"]);
            }
        }
    }
    // One open already goes on: resumed it shows nothing, loaded its branch again.
    let (answer, before) = request(&mut stdin, "session/resume", resume(&started[2], &project));
    assert_eq!(answer["result"]["sessionId"], started[2], "{answer}");
    assert!(answer["result"]["configOptions"].is_array(), "{answer}");
    assert!(before.is_empty(), "{before:?}");
    let (answer, before) = request(&mut stdin, "session/load", resume(&started[2], &project));
    assert_eq!(answer["result"]["sessionId"], started[2], "{answer}");
    let shown: Vec<_> = (before.iter())
        .map(|message| &message["params"]["update"])
        .filter(|update| {
            update["sessionUpdate"]
                .as_str()
                .is_some_and(|kind| kind.ends_with("message_chunk"))
        })
        .map(|update| {
            (
                update["sessionUpdate"].as_str().unwrap(),
                update["content"]["text"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("user_message_chunk", "second"),
            ("agent_message_chunk", "Done.")
        ],
        "{before:?}"
    );
    // Resumed, it replays nothing and goes on from its conversation.
    let (answer, before) = request(&mut stdin, "session/resume", resume(&started[0], &project));
    assert!(answer["result"]["configOptions"].is_array(), "{answer}");
    assert!(before.is_empty(), "{before:?}");
    let prompt = serde_json::json!([{"type": "text", "text": "and then"}]);
    let params = serde_json::json!({"sessionId": started[0], "prompt": prompt});
    let (ended, _) = request(&mut stdin, "session/prompt", params);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let body = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        body.contains("more of it") && body.contains("and then"),
        "{body}"
    );

    // Deleting an open session is refused; a closed one goes from the list, and deleting it
    // again, or one never saved, succeeds.
    let params = serde_json::json!({"sessionId": started[1]});
    let (answer, _) = request(&mut stdin, "session/delete", params.clone());
    assert_eq!(answer["error"]["code"], -32602, "{answer}");
    request(&mut stdin, "session/close", params.clone());
    for params in [
        params.clone(),
        params,
        serde_json::json!({"sessionId": "1-2"}),
        serde_json::json!({"sessionId": "../other"}),
    ] {
        let (answer, _) = request(&mut stdin, "session/delete", params.clone());
        assert_eq!(
            answer["result"],
            serde_json::json!({}),
            "{params}: {answer}"
        );
    }
    let answer = request(&mut stdin, "session/list", serde_json::json!({})).0;
    let listed: Vec<_> = (answer["result"]["sessions"].as_array().unwrap().iter())
        .map(|session| session["sessionId"].clone())
        .collect();
    assert_eq!(listed, [started[0].clone(), started[2].clone()], "{answer}");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
#[cfg(unix)]
fn acp_loads_a_saved_session_replaying_its_branch() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("scramjet-cli-acp-load-{}", std::process::id())));
    let project = scratch.0.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let project = std::fs::canonicalize(&project).unwrap();
    std::fs::write(project.join("a.txt"), "alpha\n").unwrap();
    let event = |value: serde_json::Value| value.to_string();
    let start = |id: &str| {
        event(serde_json::json!({"type": "message_start",
            "message": {"id": id, "usage": {"input_tokens": 3, "output_tokens": 1}}}))
    };
    let end = |reason: &str| {
        [
            event(serde_json::json!({"type": "message_delta",
                "delta": {"stop_reason": reason}, "usage": {"output_tokens": 4}})),
            event(serde_json::json!({"type": "message_stop"})),
        ]
    };
    let mut call = vec![
        start("msg_1"),
        event(
            serde_json::json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "thinking", "thinking": ""}}),
        ),
        event(
            serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "Need the file."}}),
        ),
        event(
            serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "signature_delta", "signature": "s"}}),
        ),
        event(serde_json::json!({"type": "content_block_stop", "index": 0})),
        event(
            serde_json::json!({"type": "content_block_start", "index": 1,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {}}}),
        ),
        event(
            serde_json::json!({"type": "content_block_delta", "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": r#"{"path":"a.txt"}"#}}),
        ),
        event(serde_json::json!({"type": "content_block_stop", "index": 1})),
    ];
    call.extend(end("tool_use"));
    let mut text = vec![
        start("msg_2"),
        event(
            serde_json::json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}}),
        ),
        event(
            serde_json::json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": "It says alpha."}}),
        ),
        event(serde_json::json!({"type": "content_block_stop", "index": 0})),
    ];
    text.extend(end("end_turn"));
    let (port, bodies) = serve_events(vec![call, text]);
    let models = serde_json::json!([{"id": "claude-test", "contextWindow": 1000}]);
    let settings = serde_json::json!({"hideCredentials": "never"});
    let (mut child, mut stdin, messages) = acp_models(&scratch.0, port, models, settings, &[]);
    let mut ids = 0;
    let mut request = |stdin: &mut std::process::ChildStdin, method: &str, params| {
        ids += 1;
        send(
            stdin,
            serde_json::json!({"jsonrpc": "2.0", "id": ids, "method": method, "params": params}),
        );
        until(&messages, |message| message["id"] == ids)
    };
    let initialized = request(
        &mut stdin,
        "initialize",
        serde_json::json!({"protocolVersion": 1}),
    );
    assert_eq!(
        initialized.0["result"]["agentCapabilities"]["loadSession"],
        true
    );
    let params = serde_json::json!({"cwd": project, "mcpServers": []});
    let session = request(&mut stdin, "session/new", params).0["result"]["sessionId"].clone();
    let prompt = serde_json::json!([{"type": "text", "text": "what does a.txt say?"}]);
    let params = serde_json::json!({"sessionId": session, "prompt": prompt});
    let (ended, live) = request(&mut stdin, "session/prompt", params);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    for _ in 0..2 {
        bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    let params = serde_json::json!({"sessionId": session});
    request(&mut stdin, "session/close", params);

    let load = |cwd: &std::path::Path| serde_json::json!({"sessionId": session, "cwd": cwd, "mcpServers": []});
    let (answer, _) = request(&mut stdin, "session/load", load(&scratch.0));
    assert_eq!(answer["error"]["code"], -32602, "{answer}");
    let (answer, replayed) = request(&mut stdin, "session/load", load(&project));
    assert!(answer["result"]["configOptions"].is_array(), "{answer}");
    let updates: Vec<serde_json::Value> = (replayed.iter())
        .inspect(|message| assert_eq!(message["params"]["sessionId"], session, "{message}"))
        .map(|message| message["params"]["update"].clone())
        .collect();
    let kinds: Vec<&str> = (updates.iter())
        .map(|update| update["sessionUpdate"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "user_message_chunk",
            "agent_thought_chunk",
            "agent_thought_chunk",
            "tool_call",
            "agent_message_chunk"
        ],
        "{updates:?}"
    );
    assert_eq!(updates[0]["content"]["text"], "what does a.txt say?");
    assert_eq!(updates[1]["content"]["text"], "Need the file.");
    // Reasoning and its reply share a response; the user's prompt and the call's reply do not.
    assert_eq!(updates[1]["messageId"], updates[2]["messageId"]);
    assert_ne!(updates[0]["messageId"], updates[1]["messageId"]);
    assert_ne!(updates[1]["messageId"], updates[4]["messageId"]);
    let call = &updates[3];
    assert_eq!(call["status"], "completed", "{call}");
    assert_eq!(call["kind"], "read");
    assert_eq!(call["rawInput"], serde_json::json!({"path": "a.txt"}));
    assert_eq!(
        call["locations"],
        serde_json::json!([{"path": project.join("a.txt")}])
    );
    assert!(
        call["content"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("alpha"),
        "{call}"
    );
    assert_eq!(updates[4]["content"]["text"], "It says alpha.");
    // Each replayed update names its entry, as `tree` lists them.
    let entry = |update: &serde_json::Value| {
        update["_meta"]["io.github.leanandmean.scramjet"]["entryId"]
            .as_u64()
            .unwrap_or_else(|| panic!("no entryId: {update}"))
    };
    let params = serde_json::json!({"sessionId": session});
    let (tree, _) = request(&mut stdin, "_io.github.leanandmean.scramjet/tree", params);
    let listed = |kind: &str| {
        (tree["result"]["entries"].as_array().unwrap().iter())
            .find(|listed| listed["kind"] == kind)
            .and_then(|listed| listed["entryId"].as_u64())
            .unwrap_or_else(|| panic!("no {kind}: {tree}"))
    };
    assert_eq!(entry(&updates[0]), listed("prompt"));
    assert_eq!(entry(&updates[3]), listed("toolResult"));
    assert_eq!(entry(&updates[4]), listed("reply"));
    assert_eq!(entry(&updates[1]), entry(&updates[2]));
    assert!((entry(&updates[0]) + 1..entry(&updates[3])).contains(&entry(&updates[1])));
    // The live turn's call carried the same input.
    let live_call = (live.iter())
        .map(|message| &message["params"]["update"])
        .find(|update| update["sessionUpdate"] == "tool_call")
        .unwrap();
    assert_eq!(live_call["rawInput"], call["rawInput"]);
    // Loaded again while open, it shows the same entries again, then goes on from its
    // conversation.
    let (answer, again) = request(&mut stdin, "session/load", load(&project));
    assert_eq!(answer["result"]["sessionId"], session, "{answer}");
    let again: Vec<_> = (again.iter())
        .map(|message| entry(&message["params"]["update"]))
        .collect();
    assert_eq!(again, updates.iter().map(entry).collect::<Vec<_>>());
    let prompt = serde_json::json!([{"type": "text", "text": "and then"}]);
    let params = serde_json::json!({"sessionId": session, "prompt": prompt});
    let (ended, _) = request(&mut stdin, "session/prompt", params);
    assert_eq!(ended["result"]["stopReason"], "end_turn", "{ended}");
    let body = bodies.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        body.contains("what does a.txt say?") && body.contains("and then"),
        "{body}"
    );
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
