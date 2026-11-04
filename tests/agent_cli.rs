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
    for word in ["agent", "smoke", "--bogus"] {
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
    let output = Command::new(binary)
        .args(["--effort", "hgih", "--endpoint", "127.0.0.1:9"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "scramjet-rs: invalid --effort \"hgih\": valid levels are off, minimal, low, medium, \
high, xhigh, max\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn prompt_editor_and_running_turns_keep_input_and_restore_the_terminal() {
    // A guard against a hang: the journeys take about 90 seconds.
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
