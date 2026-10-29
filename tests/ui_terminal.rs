use std::process::Command;

#[test]
fn non_tty_haiku_ui_starts_no_provider_project_or_storage_work() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args([
            "haiku-ui-demo",
            "--fixture",
            "--project",
            "/synthetic-missing-project",
            "--storage",
            "/synthetic-missing-storage",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"haiku-ui-demo: requires TTY stdin and stdout\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn retained_haiku_owned_pty_exact_approvals_denial_and_gated_settlement() {
    let output = Command::new("timeout")
        .args([
            "150",
            "python3",
            "tests/haiku_ui_pty.py",
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
fn non_tty_project_ui_starts_no_project_or_runtime_work() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args([
            "project-ui-demo",
            "--project",
            "/synthetic-missing-project",
            "--storage",
            "/synthetic-missing-storage",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"project-ui-demo: requires TTY stdin and stdout\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn retained_project_owned_pty_requests_readers_approval_verification_history_and_restoration() {
    let output = Command::new("timeout")
        .args([
            "150",
            "python3",
            "tests/project_ui_pty.py",
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

#[cfg(target_os = "linux")]
#[test]
fn command_reader_owned_pty_intent_facts_and_gated_restoration() {
    let output = Command::new("timeout")
        .args([
            "100",
            "python3",
            "tests/command_reader_pty.py",
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
#[cfg(target_os = "linux")]
#[test]
fn command_reader_owned_pty_original_watchdog_releases_only_after_restoration() {
    let output = Command::new("timeout")
        .args([
            "45",
            "python3",
            "tests/command_reader_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
            "watchdog",
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
fn non_tty_command_reader_ui_starts_nothing() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["reader-ui-demo", "--command"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"reader-ui-demo: requires TTY stdin and stdout\n"
    );
}

#[test]
fn non_tty_command_ui_starts_no_work() {
    for automatic in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"));
        command.arg("command-ui-demo");
        if automatic {
            command.arg("--automatic");
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            b"command-ui-demo: requires TTY stdin and stdout\n"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn command_ui_owned_pty_invocation_and_blocked_read_restoration() {
    let output = Command::new("timeout")
        .args([
            "45",
            "python3",
            "tests/command_ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn non_tty_coding_ui_starts_no_work() {
    for arguments in [
        vec!["coding-ui-demo"],
        vec!["coding-ui-demo", "--follow-up"],
        vec!["coding-ui-demo", "--command"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr,
            b"coding-ui-demo: requires TTY stdin and stdout\n"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn command_coding_owned_pty_input_approval_cancel_and_uncertain_journeys() {
    let output = Command::new("timeout")
        .args([
            "60",
            "python3",
            "tests/command_coding_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn coding_follow_up_owned_pty_context_approval_and_failure_journeys() {
    let output = Command::new("timeout")
        .args([
            "60",
            "python3",
            "tests/coding_follow_up_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn non_tty_ui_fails_without_terminal_mutation() {
    for arguments in [vec!["ui-demo"], vec!["ui-demo", "--diagnostic"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"ui-demo: requires TTY stdin and stdout\n");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn diagnostic_input_ownership_and_failure_reports_follow_cleanup() {
    let output = Command::new("timeout")
        .args([
            "15",
            "python3",
            "tests/ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
            "diagnostic_ownership",
            "diagnostic_invalid_suffix",
            "diagnostic_quit_suffix",
            "diagnostic_cursor_fault_quit",
            "diagnostic_cursor_fault_invalid",
            "diagnostic_deadline",
            "diagnostic_restore_fault",
            "diagnostic_report_fault",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn queued_input_is_discarded_before_handoff() {
    let output = Command::new("timeout")
        .args([
            "15",
            "python3",
            "tests/ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
            "invalid_suffix",
            "escape_suffix",
            "paste_suffix",
            "quit_suffix",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn hidden_cursor_write_failures_are_reported_after_restoration() {
    let output = Command::new("timeout")
        .args([
            "15",
            "python3",
            "tests/ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
            "cursor_fault_quit",
            "cursor_fault_invalid",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn non_tty_runtime_ui_starts_no_work() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("runtime-ui-demo")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"runtime-ui-demo: requires TTY stdin and stdout\n"
    );
}

#[test]
fn non_tty_reader_ui_starts_no_work() {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .arg("reader-ui-demo")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"reader-ui-demo: requires TTY stdin and stdout\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn reader_ui_owned_pty_fact_child_tree_and_failure_journeys() {
    let output = Command::new("timeout")
        .args([
            "45",
            "python3",
            "tests/reader_ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn coding_ui_owned_pty_exact_approval_and_fail_closed_journeys() {
    let output = Command::new("timeout")
        .args([
            "60",
            "python3",
            "tests/coding_ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn coding_failure_settlement_gap_preserves_original_watchdog_and_suppresses_reentry() {
    let output = Command::new("timeout")
        .args([
            "40",
            "python3",
            "tests/coding_ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
            "transport_gap_deadline",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_ui_ascii_observer_reconstructs_fragmented_differential_updates() {
    let output = Command::new("python3")
        .args(["tests/runtime_ui_pty.py", "--observer-only"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn runtime_ui_owned_pty_stream_control_error_and_stall_journeys() {
    let output = Command::new("timeout")
        .args([
            "30",
            "python3",
            "tests/runtime_ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn owned_pty_journeys_restore_terminal() {
    let output = Command::new("timeout")
        .args([
            "15",
            "python3",
            "tests/ui_pty.py",
            env!("CARGO_BIN_EXE_scramjet-rs"),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
