#[path = "support/coding_invocation.rs"]
mod journey;
use journey::{Dirs, command, server};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
};
fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    fn walk(path: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(&path, root, out);
            } else if metadata.file_type().is_symlink() {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read_link(&path)
                        .unwrap()
                        .as_os_str()
                        .as_encoded_bytes()
                        .to_vec(),
                );
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    walk(path, path, &mut out);
    out
}
fn seed() -> (Dirs, std::net::SocketAddr, PathBuf, PathBuf, Value) {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let first = dirs.input("first.txt", b"Finish without tools.");
    let next = dirs.input("next.txt", b"Please continue with another message.");
    let (endpoint, server) = server(
        vec![None],
        ["Finish without tools.".into(), "".into()],
        usize::MAX,
        "".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let output = command(&dirs, endpoint, &names, &first, None)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    let prior = dirs.input("prior.json", &output.stdout);
    (dirs, endpoint, next, prior, record)
}
fn refusal(dirs: &Dirs, command: &mut std::process::Command) {
    let before = (tree(&dirs.project), tree(&dirs.storage));
    let output = command.output().unwrap();
    assert!(
        !output.status.success(),
        "unexpected success {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stdout.is_empty());
    assert_eq!(
        (tree(&dirs.project), tree(&dirs.storage)),
        before,
        "refusal changed source/accounting: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn supplied_record_mutations_cannot_change_authority_accounting_or_material() {
    let (dirs, endpoint, next, _, record) = seed();
    let names = ["input.md", "reader.py"];
    for case in 0..13 {
        let mut changed = record.clone();
        match case {
            0 => changed["version"] = json!(2),
            1 => changed["generation"] = json!(0),
            2 => changed["header"]["root"][1] = json!(123),
            3 => changed["header"]["provider"]["file"][1] = json!(123),
            4 => changed["header"]["deadline_ns"] = json!(0),
            5 => changed["tools"][0]["description"] = json!("changed"),
            6 => changed["conversation"][1]["value"]["phase"] = json!("commentary"),
            7 => changed["conversation"][0]["text"] = json!("foreign prompt"),
            8 => changed["receipts"][0]["request_id"] = json!(123),
            9 => changed["counters"]["attempts"] = json!(0),
            10 => changed["worker_joined"] = json!(false),
            11 => changed["export_producer_acknowledgement"] = json!("settled"),
            _ => changed["conversation"].as_array_mut().unwrap().reverse(),
        }
        let prior = dirs.input(
            &format!("tampered-{case}.json"),
            &serde_json::to_vec(&changed).unwrap(),
        );
        refusal(
            &dirs,
            &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
        );
    }
    for (name, bytes) in [
        ("duplicate", b"{\"version\":1,\"version\":1}".as_slice()),
        ("torn", b"{".as_slice()),
        ("unsupported", b"[]".as_slice()),
    ] {
        let prior = dirs.input(name, bytes);
        refusal(
            &dirs,
            &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
        );
    }
    let prior = dirs.input("oversized", &vec![b' '; 131_073]);
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
    );
}
#[test]
fn artifact_anchor_store_scope_and_missing_record_refuse_without_repair() {
    let (dirs, endpoint, next, prior, record) = seed();
    let names = ["input.md", "reader.py"];
    let artifact = dirs.storage.join("ordinary-run/export-1.json");
    let original = fs::read(&artifact).unwrap();
    for bytes in [b"{".as_slice(), b"{}".as_slice()] {
        fs::write(&artifact, bytes).unwrap();
        refusal(
            &dirs,
            &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
        );
    }
    fs::remove_file(&artifact).unwrap();
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
    );
    fs::write(&artifact, &original).unwrap();
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600)).unwrap();
    let orphan = dirs.storage.join("ordinary-run/export-77.json");
    fs::write(&orphan, &original).unwrap();
    fs::set_permissions(&orphan, fs::Permissions::from_mode(0o600)).unwrap();
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
    );
    fs::remove_file(orphan).unwrap();
    refusal(&dirs, &mut command(&dirs, endpoint, &names, &next, None));
    let swapped = [names[1], names[0]];
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &swapped, &next, Some(&prior)),
    );
    let alternate = dirs.base.join("alternate-storage");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&alternate)
        .unwrap();
    let other = command(&dirs, endpoint, &names, &next, Some(&prior));
    let args = other.get_args().map(|a| a.to_owned()).collect::<Vec<_>>();
    let mut other = std::process::Command::new(env!("CARGO_BIN_EXE_scramjet-rs"));
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--storage" {
            args.next();
            other.arg("--storage").arg(&alternate);
        } else {
            other.arg(arg);
        }
    }
    other.stdin(Stdio::null());
    refusal(&dirs, &mut other);
    assert_eq!(fs::read_dir(&alternate).unwrap().count(), 0);
    let anchor = dirs.project.join(".coding-run");
    let saved = fs::read(&anchor).unwrap();
    fs::remove_file(&anchor).unwrap();
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
    );
    refusal(&dirs, &mut command(&dirs, endpoint, &names, &next, None));
    fs::write(&anchor, &saved).unwrap();
    fs::set_permissions(&anchor, fs::Permissions::from_mode(0o600)).unwrap();
    let clock = record["header"]["deadline_ns"].as_u64().unwrap();
    assert!(clock > 0);
}
#[test]
fn appeared_missing_changed_and_foreign_capture_cannot_be_recaptured() {
    let (dirs, endpoint, next, prior, _) = seed();
    let names = ["input.md", "reader.py"];
    let appeared = dirs.project.join("input.md");
    fs::write(&appeared, b"appeared").unwrap();
    fs::set_permissions(&appeared, fs::Permissions::from_mode(0o600)).unwrap();
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
    );
    fs::remove_file(&appeared).unwrap();
    std::os::unix::fs::symlink("missing", &appeared).unwrap();
    refusal(
        &dirs,
        &mut command(&dirs, endpoint, &names, &next, Some(&prior)),
    );
}
#[test]
fn durable_maximum_output_is_kept_when_complete_next_context_cannot_fit() {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let prompt = dirs.input(
        "prompt",
        b"Write and execute Python that emits synthetic NUL bytes.",
    );
    let calls = vec![
        Some((
            "write",
            json!({"path":names[1],"content":"import sys\nsys.stdout.buffer.write(bytes(16384))\n"}),
        )),
        Some(("python", json!({"path":names[1]}))),
    ];
    let (endpoint, server) = server(
        calls,
        [
            "Write and execute Python that emits synthetic NUL bytes.".into(),
            "".into(),
        ],
        usize::MAX,
        "".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let output = command(&dirs, endpoint, &names, &prompt, None)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["disposition"], "stopped");
    let result: Value = serde_json::from_str(
        record["conversation"].as_array().unwrap().last().unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        result["stdout"].as_str().unwrap().as_bytes(),
        vec![0; 16384]
    );
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["namespace_dead"], true);
    assert_eq!(server.join().unwrap().len(), 2);
    assert_eq!(record["counters"]["processes"], 1);
    let journal = fs::read_to_string(dirs.storage.join("ordinary-run/invocation.jsonl")).unwrap();
    assert!(journal.contains("\"kind\":\"Process\""));
    assert!(journal.contains("\"kind\":\"Result\""));
    assert_eq!(
        fs::read_to_string(
            dirs.storage
                .join("ordinary-run/provider/provider-attempts.jsonl")
        )
        .unwrap()
        .lines()
        .count(),
        5
    );
}
#[test]
fn nonzero_python_keeps_real_output_and_stops_without_a_scenario_predicate() {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let prompts = [
        "Create input and finish.".to_owned(),
        "Create your own Python and execute it.".to_owned(),
    ];
    let first = dirs.input("first", prompts[0].as_bytes());
    let second = dirs.input("second", prompts[1].as_bytes());
    let calls = vec![
        Some(("write", json!({"path":names[0],"content":"original input"}))),
        None,
        Some((
            "write",
            json!({"path":names[1],"content":"import sys\nprint('different output')\nsys.exit(3)\n"}),
        )),
        Some(("python", json!({"path":names[1]}))),
    ];
    let (endpoint, server) = server(
        calls,
        prompts,
        2,
        "original input".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let output = command(&dirs, endpoint, &names, &first, None)
        .output()
        .unwrap();
    assert!(output.status.success());
    let prior = dirs.input("prior", &output.stdout);
    let output = command(&dirs, endpoint, &names, &second, Some(&prior))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(record["disposition"], "stopped");
    let result: Value = serde_json::from_str(
        record["conversation"].as_array().unwrap().last().unwrap()["output"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["exit_code"], 3);
    assert_eq!(result["stdout"], "different output\n");
    assert_eq!(result["status_source"], "program_wait_status");
    assert_eq!(result["launcher_reaped"], true);
    assert_eq!(result["namespace_dead"], true);
    assert_eq!(server.join().unwrap().len(), 4);
}
#[test]
fn exact_intent_reply_finishes_with_stdin_writer_open_and_stale_reply_denies() {
    for stale in [false, true] {
        let dirs = Dirs::new();
        let names = ["input.md", "reader.py"];
        let prompt = dirs.input("prompt", b"Create the selected file and finish.");
        let (endpoint, server) = server(
            if stale {
                vec![Some((
                    "write",
                    json!({"path":names[0],"content":"approved bytes"}),
                ))]
            } else {
                vec![
                    Some(("write", json!({"path":names[0],"content":"approved bytes"}))),
                    None,
                ]
            },
            ["Create the selected file and finish.".into(), "".into()],
            usize::MAX,
            "approved bytes".into(),
            dirs.project.clone(),
            names.map(str::to_owned),
        );
        let base = command(&dirs, endpoint, &names, &prompt, None);
        let args = base
            .get_args()
            .filter(|a| *a != "--approve-effects")
            .map(|a| a.to_owned())
            .collect::<Vec<_>>();
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let driver = std::thread::spawn(move || {
            let mut log = String::new();
            for line in BufReader::new(stderr).lines() {
                let line = line.unwrap();
                if let Some(proposal) = line.strip_prefix("proposal ") {
                    let mut intent: Value = serde_json::from_str(proposal).unwrap();
                    if stale {
                        intent["generation"] = json!(999);
                    }
                    writeln!(stdin, "{}", json!({"approve":true,"intent":intent})).unwrap();
                    stdin.flush().unwrap();
                }
                log.push_str(&line);
                log.push('\n');
            }
            log
        });
        let start = std::time::Instant::now();
        let output = child.wait_with_output().unwrap();
        let log = driver.join().unwrap();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "approval waited for EOF or extra input"
        );
        assert_eq!(output.status.success(), !stale, "{log}");
        let record: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            record["disposition"],
            if stale { "stopped" } else { "complete" }
        );
        assert_eq!(dirs.project.join(names[0]).exists(), !stale);
        server.join().unwrap();
    }
}
