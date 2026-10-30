use std::process::Command;

#[cfg(target_os = "linux")]
#[test]
fn prompt_editor_pastes_without_sending_recalls_history_and_restores_the_terminal() {
    let output = Command::new("timeout")
        .args([
            "60",
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
