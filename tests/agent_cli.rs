use std::process::Command;

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
        let output = Command::new(binary)
            .args([word, "--endpoint", "127.0.0.1:9"])
            .output()
            .unwrap();
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
    let missing = refused(&["-p", "@/nonexistent/notes.md", "--endpoint", "127.0.0.1:9"]);
    assert_eq!(
        missing,
        "scramjet-rs: file not found: /nonexistent/notes.md\n"
    );
    for option in ["--thinking", "--effort"] {
        let output = Command::new(binary)
            .args([option, "hgih", "--endpoint", "127.0.0.1:9"])
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
    // Before a session is started.
    let output = Command::new(binary)
        .args(["--list-models", "--endpoint", "127.0.0.1:9"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "scramjet-rs: could not list the models of the endpoint 127.0.0.1:9\n"
    );
}

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
    let run = |arguments: &[&str]| {
        let output = Command::new(binary)
            .args(["--endpoint", "127.0.0.1:9", "--retries", "0"])
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

    // The requests fail (no endpoint), after the prompt is saved in the session.
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
    let output = Command::new(binary)
        .args(["--endpoint", "127.0.0.1:9", "--retries", "0"])
        .args(["--no-session", "-p", "hi"])
        .current_dir(&project)
        .env("XDG_STATE_HOME", scratch.0.join("state"))
        .env("XDG_CONFIG_HOME", scratch.0.join("config"))
        .env("SCRAMJET_CODING_AGENT_DIR", scratch.0.join("agent"))
        .env("SCRAMJET_CACHE", scratch.0.join("cache"))
        // Where the run keeps its sessions, so that this test finds its own.
        .env("TMPDIR", &temporary)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot connect to 127.0.0.1:9"), "{error}");
    assert_eq!(std::fs::read_dir(&temporary).unwrap().count(), 0);
    assert!(!scratch.0.join("state/scramjet-rs/sessions").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn prompt_editor_and_running_turns_keep_input_and_restore_the_terminal() {
    // A guard against a hang: the journeys take about 130 seconds.
    let output = Command::new("timeout")
        .args([
            "180",
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
