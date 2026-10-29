use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};
fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "scramjet-haiku-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    for name in ["project", "storage"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join(name))
            .unwrap();
    }
    root
}
#[test]
fn fixture_cli_owns_execution_and_fresh_history_never_replays() {
    let root = root();
    let project = root.join("project");
    let storage = root.join("storage");
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["haiku-demo", "--fixture", "--project"])
        .arg(&project)
        .arg("--storage")
        .arg(&storage)
        .arg("--approve-effects")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("consumed=5 unknown_usage=0 unresolved_remote=0 local_active=0"));
    assert!(text.contains("finished turns=5 worker_joined=true namespace_dead=true"));
    let records = storage.join("haiku-demo/haiku.jsonl");
    let ledger = storage.join("haiku-demo-provider/provider-attempts.jsonl");
    let before = std::fs::read(&records).unwrap();
    let provider = std::fs::read(&ledger).unwrap();
    let files: Vec<_> = ["haiku.md", "print_haiku.py"]
        .iter()
        .map(|name| {
            assert_eq!(
                std::fs::metadata(project.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
            std::fs::read(project.join(name)).unwrap()
        })
        .collect();
    assert_eq!(files[1], scramjet_rs::HAIKU_SCRIPT.as_bytes());
    let read = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["haiku-demo", "--history", "--storage"])
        .arg(&storage)
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    let history = scramjet_rs::reconstruct_haiku_history(&storage, "haiku-demo").unwrap();
    assert!(history.ending_seen());
    assert!(!history.finished());
    assert!(
        history
            .effects()
            .iter()
            .all(|effect| effect.state == scramjet_rs::HaikuCertainty::Completed)
    );
    assert_eq!(history.process().unwrap().stdout.as_bytes(), files[0]);
    assert_eq!(before, std::fs::read(&records).unwrap());
    assert_eq!(provider, std::fs::read(&ledger).unwrap());
    for (name, bytes) in ["haiku.md", "print_haiku.py"].iter().zip(files) {
        assert_eq!(bytes, std::fs::read(project.join(name)).unwrap());
    }
    let replay = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["haiku-demo", "--fixture", "--project"])
        .arg(&project)
        .arg("--storage")
        .arg(&storage)
        .arg("--approve-effects")
        .output()
        .unwrap();
    assert!(!replay.status.success());
    assert_eq!(provider, std::fs::read(&ledger).unwrap());
    assert_eq!(before, std::fs::read(&records).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn omitted_approval_and_ineligible_live_options_have_no_file_effects() {
    let root = root();
    let project = root.join("project");
    let storage = root.join("storage");
    let denied = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["haiku-demo", "--fixture", "--project"])
        .arg(&project)
        .arg("--storage")
        .arg(&storage)
        .output()
        .unwrap();
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stdout).contains("consumed=1"));
    assert!(!project.join("haiku.md").exists());
    let before = std::fs::read(storage.join("haiku-demo/haiku.jsonl")).unwrap();
    let history = scramjet_rs::reconstruct_haiku_history(&storage, "haiku-demo").unwrap();
    assert_eq!(
        history.effects()[0].state,
        scramjet_rs::HaikuCertainty::NotDispatched
    );
    for options in [
        vec!["--live"],
        vec!["--history", "--approve-effects"],
        vec!["--fixture", "--fixture"],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"));
        child
            .arg("haiku-demo")
            .args(options)
            .arg("--storage")
            .arg(&storage);
        assert!(!child.output().unwrap().status.success());
    }
    assert_eq!(
        before,
        std::fs::read(storage.join("haiku-demo/haiku.jsonl")).unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn torn_corrupt_and_oversized_histories_are_read_only_refusals() {
    let root = root();
    let storage = root.join("storage");
    let session =
        scramjet_rs::HaikuSession::create(&root.join("project"), &storage, "corrupt").unwrap();
    drop(session);
    let file = storage.join("corrupt/haiku.jsonl");
    let original = std::fs::read(&file).unwrap();
    let mut cases = Vec::new();
    cases.push(original[..original.len() - 1].to_vec());
    let mut bad: serde_json::Value = serde_json::from_slice(&original).unwrap();
    bad["sequence"] = serde_json::json!(1);
    cases.push(format!("{bad}\n").into_bytes());
    cases.push(vec![b'x'; 2_097_153]);
    for bytes in cases {
        std::fs::write(&file, &bytes).unwrap();
        assert!(scramjet_rs::reconstruct_haiku_history(&storage, "corrupt").is_err());
        let child = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(["haiku-demo", "--history", "--storage"])
            .arg(&storage)
            .args(["--session", "corrupt"])
            .output()
            .unwrap();
        assert!(!child.status.success());
        assert_eq!(bytes, std::fs::read(&file).unwrap());
        assert!(!root.join("project/haiku.md").exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}
