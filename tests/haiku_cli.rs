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
fn selected_item_contract_is_enforced_in_fresh_read_only_recovery() {
    use serde_json::{Value, json};
    let root = root();
    let storage = root.join("storage");
    let producer = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["haiku-demo", "--fixture", "--project"])
        .arg(root.join("project"))
        .arg("--storage")
        .arg(&storage)
        .arg("--approve-effects")
        .output()
        .unwrap();
    assert!(producer.status.success(), "{producer:?}");
    let path = storage.join("haiku-demo/haiku.jsonl");
    let original = std::fs::read_to_string(&path).unwrap();
    let mut records: Vec<Value> = original
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let end = records
        .iter()
        .position(|entry| entry["record"]["kind"] == "Result")
        .unwrap();
    records.truncate(end + 1);
    records.push(json!({"sequence":records.len(),"record":{"kind":"Stopped","turn":1,"reason":"synthetic producer stop"}}));
    let selected = records
        .iter()
        .position(|entry| entry["record"]["kind"] == "Selected")
        .unwrap();
    let protected = [
        storage.join("haiku-demo-provider/provider-attempts.jsonl"),
        root.join("project/haiku.md"),
        root.join("project/print_haiku.py"),
        root.join("project/.scramjet-project-lock"),
    ];
    let protected: Vec<_> = protected
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    let invalid = [
        ("function_call", "/status", json!("in_progress")),
        ("function_call", "/status", Value::Null),
        ("function_call", "/namespace", json!("foreign")),
        ("function_call", "/namespace", json!({})),
        (
            "function_call",
            "/arguments",
            json!("{\"path\":\"haiku.md\",\"path\":\"haiku.md\"}"),
        ),
        ("function_call", "/arguments", json!("[]")),
        ("message", "/role", json!("user")),
        ("message", "/status", json!("in_progress")),
        ("message", "/phase", json!("foreign")),
        ("message", "/content", json!({})),
        ("message", "/content/0/type", json!("refusal")),
        ("message", "/content/0/text", json!(42)),
        ("message", "/content/0/annotations", Value::Null),
        ("message", "/content/0/logprobs", json!({})),
        ("reasoning", "/status", json!("in_progress")),
        ("reasoning", "/status", Value::Null),
        ("reasoning", "/summary", json!({})),
        (
            "reasoning",
            "/summary",
            json!([{"type":"output_text","text":"x"}]),
        ),
        (
            "reasoning",
            "/summary",
            json!([{"type":"summary_text","text":42}]),
        ),
        ("reasoning", "/encrypted_content", json!({})),
    ];
    for (kind, pointer, value) in invalid {
        let mut changed = records.clone();
        let item = changed[selected]["record"]["items"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|item| item["type"] == kind)
            .unwrap();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        item.pointer_mut(parent).unwrap()[field] = value;
        check_selected_history(&storage, &path, &changed, &protected, false);
    }
    for phase in [
        None,
        Some(Value::Null),
        Some(json!("commentary")),
        Some(json!("final_answer")),
    ] {
        for namespace in [None, Some(Value::Null)] {
            let mut changed = records.clone();
            for (index, item) in changed[selected]["record"]["items"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                let kind = item["type"].as_str().unwrap().to_owned();
                let object = item.as_object_mut().unwrap();
                object.insert(
                    "metadata".into(),
                    json!({"opaque":"é🙂\u{0}\r\n\\\"/foreign"}),
                );
                object.insert(
                    "id".into(),
                    json!(format!("opaque / é🙂\u{0}\r\n {kind} {index}")),
                );
                if kind == "function_call" || kind == "reasoning" {
                    object.remove("status");
                }
                if kind == "function_call" {
                    object.remove("namespace");
                    if let Some(value) = &namespace {
                        object.insert("namespace".into(), value.clone());
                    }
                } else if kind == "message" {
                    object.remove("phase");
                    if let Some(value) = &phase {
                        object.insert("phase".into(), value.clone());
                    }
                } else {
                    object.insert(
                        "summary".into(),
                        json!([{"type":"summary_text","text":"é🙂\u{0}\r\n"}]),
                    );
                    object.remove("encrypted_content");
                    if let Some(phase) = &phase {
                        object.insert(
                            "encrypted_content".into(),
                            if phase.is_null() {
                                Value::Null
                            } else {
                                json!("opaque é🙂\u{0}\r\n\\\"")
                            },
                        );
                    }
                }
            }
            check_selected_history(&storage, &path, &changed, &protected, true);
        }
    }
    check_selected_history(&storage, &path, &records, &protected, true);
    std::fs::remove_dir_all(root).unwrap();
}
fn check_selected_history(
    storage: &std::path::Path,
    path: &std::path::Path,
    records: &[serde_json::Value],
    protected: &[(PathBuf, Vec<u8>)],
    valid: bool,
) {
    let bytes: Vec<u8> = records
        .iter()
        .flat_map(|entry| format!("{entry}\n").into_bytes())
        .collect();
    std::fs::write(path, &bytes).unwrap();
    let history = scramjet_rs::reconstruct_haiku_history(storage, "haiku-demo");
    if valid {
        let history = history.unwrap();
        assert_eq!(history.effects().len(), 1);
        assert_eq!(
            history.effects()[0].state,
            scramjet_rs::HaikuCertainty::Completed
        );
        assert!(!history.ending_seen() && !history.finished());
        assert!(!history.entries.last().unwrap().acknowledged);
    } else {
        assert!(
            matches!(history, Err(scramjet_rs::CodingError::History(_))),
            "{history:?}"
        );
    }
    let child = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["haiku-demo", "--history", "--storage"])
        .arg(storage)
        .output()
        .unwrap();
    assert_eq!(child.status.success(), valid, "{child:?}");
    assert_eq!(bytes, std::fs::read(path).unwrap());
    for (path, bytes) in protected {
        assert_eq!(*bytes, std::fs::read(path).unwrap());
    }
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
