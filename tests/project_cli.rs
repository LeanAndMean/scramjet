#[path = "support/coding.rs"]
mod roots;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};
fn root() -> roots::OwnedRoot {
    let root = roots::OwnedRoot::new();
    fs::set_permissions(&root.path, fs::Permissions::from_mode(0o755)).unwrap();
    for (name, data) in [
        ("sample.txt", "Header\nOLD body\nFooter\n"),
        ("rules.txt", "complete rules\n"),
        ("notes.txt", "complete notes\n"),
    ] {
        fs::write(root.path.join(name), data).unwrap();
        fs::set_permissions(root.path.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    root
}
fn request(from: &str, to: &str) -> String {
    serde_json::json!({"operation":"replace_once","target":"sample.txt","supporting":["rules.txt","notes.txt"],"needle":from,"replacement":to}).to_string()
}
fn run(
    root: &roots::OwnedRoot,
    storage: &roots::OwnedRoot,
    extra: &[&str],
) -> std::process::Output {
    Command::new("timeout")
        .arg("30")
        .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["project-demo", "--project"])
        .arg(&root.path)
        .arg("--storage")
        .arg(&storage.path)
        .arg("--request")
        .arg(request("OLD", "NEW"))
        .args(extra)
        .output()
        .unwrap()
}
#[test]
fn selected_project_cli_two_requests_and_fresh_process_history_preserve_project_without_replay() {
    let root = root();
    let storage = roots::OwnedRoot::new();
    let second = request("NEW", "NEXT");
    let result = run(
        &root,
        &storage,
        &[
            "--approve-edit",
            "--verify-edited",
            "--second-request",
            &second,
        ],
    );
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert_eq!(output.matches("exact proposal:").count(), 2);
    assert_eq!(output.matches("verification report: Completed").count(), 2);
    assert!(output.contains("attempts=14 fixture_requests=14 fixture_peak=2 fixture_joined=true"));
    assert!(
        output.contains(
            "logical=true readers=true coding=true storage=true runtime=true history=true"
        )
    );
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "Header\nNEXT body\nFooter\n"
    );
    assert_eq!(
        fs::metadata(root.path.join("sample.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o644
    );
    let journal = fs::read(storage.path.join(".project-journal")).unwrap();
    let target = fs::read(root.path.join("sample.txt")).unwrap();
    let moved = root.path.with_extension("moved");
    fs::rename(&root.path, &moved).unwrap();
    for _ in 0..3 {
        let recovered = Command::new("timeout")
            .arg("10")
            .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(["project-demo", "--history", "--storage"])
            .arg(&storage.path)
            .output()
            .unwrap();
        assert!(recovered.status.success(), "{recovered:?}");
        let text = String::from_utf8(recovered.stdout).unwrap();
        assert!(text.contains("no runtime, approval, continuation or replay"));
        assert_eq!(
            fs::read(storage.path.join(".project-journal")).unwrap(),
            journal
        );
        assert_eq!(fs::read(moved.join("sample.txt")).unwrap(), target);
    }
    fs::rename(moved, &root.path).unwrap();
}
#[test]
fn omitted_approval_or_continuation_flags_do_not_authorize_mutation_or_companion() {
    for approve in [false, true] {
        let root = root();
        let storage = roots::OwnedRoot::new();
        let result = run(
            &root,
            &storage,
            if approve { &["--approve-edit"] } else { &[] },
        );
        assert!(result.status.success(), "{result:?}");
        let text = String::from_utf8(result.stdout).unwrap();
        assert!(!text.contains("verification report:"));
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            if approve {
                "Header\nNEW body\nFooter\n"
            } else {
                "Header\nOLD body\nFooter\n"
            }
        );
        assert!(
            !fs::read_to_string(storage.path.join(".project-journal"))
                .unwrap()
                .contains("VerificationReady")
        );
    }
}
#[test]
fn malformed_cli_inputs_and_existing_storage_fail_without_overwriting_project_or_journal() {
    let root = root();
    let storage = roots::OwnedRoot::new();
    for args in [
        vec!["--request", "{}"],
        vec!["--approve-edit", "--approve-edit"],
        vec!["--second-request", "{}"],
        vec!["--unknown"],
    ] {
        let result = run(&root, &storage, &args);
        assert!(!result.status.success());
        assert!(!storage.path.join(".project-journal").exists());
    }
    let output = run(&root, &storage, &[]);
    assert!(output.status.success(), "{output:?}");
    let before = fs::read(storage.path.join(".project-journal")).unwrap();
    let repeated = run(&root, &storage, &["--approve-edit", "--verify-edited"]);
    assert!(!repeated.status.success());
    assert_eq!(
        fs::read(storage.path.join(".project-journal")).unwrap(),
        before
    );
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "Header\nOLD body\nFooter\n"
    );
}
