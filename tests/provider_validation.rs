use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn storage() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "validation-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path.join("ledger"))
        .unwrap();
    path
}
fn invoke(root: &Path, shim: Option<(&Path, &str)>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"));
    command
        .args([
            "provider-demo",
            "--validation-pair",
            "--fixture",
            "--storage",
        ])
        .arg(root.join("ledger"))
        .arg("--capture")
        .arg(root.join("evidence"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some((library, mode)) = shim {
        command
            .env("LD_PRELOAD", library)
            .env("VALIDATION_FAULT_ROOT", root)
            .env("VALIDATION_FAULT_MODE", mode);
    }
    let mut child = command.spawn().unwrap();
    let start = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "validation CLI timed out: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn observe(root: &Path) -> Value {
    observe_from(root, "evidence")
}
fn observe_from(root: &Path, evidence: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args([
            "provider-demo",
            "--validation-observe",
            "--fixture",
            "--storage",
        ])
        .arg(root.join("ledger"))
        .arg("--capture")
        .arg(root.join(evidence))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn validation_pair_cli_and_fresh_read_only_observation() {
    let root = storage();
    let output = invoke(&root, None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["success"], true);
    assert_eq!(report["consumed"], 6);
    assert_eq!(report["unresolved_remote"], 4);
    assert_eq!(report["unknown_usage"], 4);
    assert_eq!(report["local_active"], 0);
    assert_eq!(report["summary_durable"], true);
    let file = root.join("ledger/provider-attempts.jsonl");
    let bytes = std::fs::read(&file).unwrap();
    let observed = observe(&root);
    assert_eq!(observed["consumed"], 6);
    assert_eq!(observed["dispatch_authority"], false);
    assert_eq!(observed["producer_acknowledgement"], "unknown");
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let retry = invoke(&root, None);
    assert!(!retry.status.success());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    let requests: [Value; 2] = std::array::from_fn(|i| {
        serde_json::from_slice(
            &std::fs::read(root.join(if i == 0 {
                "evidence/first/request.json"
            } else {
                "evidence/second/request.json"
            }))
            .unwrap(),
        )
        .unwrap()
    });
    assert_eq!(requests[0]["tools"], serde_json::json!([]));
    assert_eq!(requests[1]["input"][0], requests[0]["input"][0]);
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 4);
    assert_eq!(requests[1]["input"][1]["type"], "reasoning");
    assert_eq!(requests[1]["input"][2]["phase"], "final_answer");
    assert_eq!(requests[1]["tools"][0]["name"], "inert");
}
#[test]
fn validation_pair_native_faults_and_producer_loss_preserve_accounting_without_retry() {
    let compiler = storage();
    let library = compiler.join("fault.so");
    let compiled = Command::new("cc")
        .args([
            "-shared",
            "-fPIC",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "tests/provider_validation_fault.c",
            "-ldl",
            "-o",
        ])
        .arg(&library)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    for mode in [
        "stale-request",
        "delay-reserve5",
        "delay-reserve6",
        "lose-reserve5",
        "lose-join5",
        "lose-reserve6",
        "lose-join6",
        "fail-reserve5",
        "partial-reserve5",
        "fail-join5",
        "fail-reserve6",
        "fail-join6",
        "raw-write",
        "raw-close",
        "replace-first",
        "replace-directory",
        "replace-request",
        "summary-close",
        "summary-replace-root",
        "summary-mutate-first",
    ] {
        let root = storage();
        let output = invoke(&root, Some((&library, mode)));
        assert!(!output.status.success(), "{mode}");
        if mode.starts_with("lose-") {
            assert_eq!(output.status.code(), Some(17), "{mode}");
        } else {
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["success"], false, "{mode}");
            assert_eq!(report["local_active"], 0, "{mode}");
            if mode.starts_with("fail-") || mode == "partial-reserve5" {
                assert_eq!(report["accounting_observed"], false, "{mode}");
                assert!(report["consumed"].is_null(), "{mode}");
                assert!(report["remaining"].is_null(), "{mode}");
                assert!(report["new_uncertainty"].is_null(), "{mode}");
                assert!(report["after_sha256"].is_null(), "{mode}");
                assert_eq!(
                    report["acknowledged_consumed"],
                    if mode == "fail-reserve5" || mode == "partial-reserve5" {
                        4
                    } else if mode == "fail-join6" {
                        6
                    } else {
                        5
                    },
                    "{mode}"
                );
            } else {
                assert_eq!(report["accounting_observed"], true, "{mode}");
                assert_eq!(
                    report["consumed"],
                    if mode == "replace-request" || mode == "stale-request" {
                        4
                    } else if mode.starts_with("summary-") || mode == "delay-reserve6" {
                        6
                    } else {
                        5
                    },
                    "{mode}"
                );
            }
        }
        if mode == "stale-request" || mode.starts_with("delay-") {
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            let posts = usize::from(mode == "delay-reserve6");
            let consumed = if mode == "stale-request" {
                4
            } else {
                5 + posts
            };
            assert_eq!(report["fixture_posts"], posts, "{mode}");
            assert_eq!(report["consumed"], consumed, "{mode}");
            assert_eq!(report["acknowledged_consumed"], consumed, "{mode}");
            assert_eq!(
                report["unresolved_remote"],
                if posts == 0 && consumed == 4 { 4 } else { 5 },
                "{mode}"
            );
            if mode.starts_with("delay-") {
                let detail: Value = serde_json::from_slice(
                    &std::fs::read(root.join(if posts == 0 {
                        "evidence/first/provenance.json"
                    } else {
                        "evidence/second/provenance.json"
                    }))
                    .unwrap(),
                )
                .unwrap();
                let attempt = &detail["attempt"];
                assert_eq!(attempt["passed"], false, "{mode}");
                assert_eq!(attempt["durable"], true, "{mode}");
                assert_eq!(attempt["local_joined"], true, "{mode}");
                assert!(attempt["terminal"].is_null(), "{mode}");
                assert!(attempt["usage"].is_null(), "{mode}");
            }
        }
        let file = root.join("ledger/provider-attempts.jsonl");
        let before = std::fs::read(&file).unwrap();
        let evidence = if mode == "summary-replace-root" {
            "evidence.saved"
        } else {
            "evidence"
        };
        let checkpoint: Value = serde_json::from_slice(
            &std::fs::read(root.join(evidence).join("checkpoint.json")).unwrap(),
        )
        .unwrap();
        let length = checkpoint["bytes"].as_u64().unwrap() as usize;
        let expected: Vec<u8> = checkpoint["sha256"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(Sha256::digest(&before[..length])[..], expected);
        if mode == "partial-reserve5" {
            let read = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
                .args([
                    "provider-demo",
                    "--validation-observe",
                    "--fixture",
                    "--storage",
                ])
                .arg(root.join("ledger"))
                .arg("--capture")
                .arg(root.join("evidence"))
                .output()
                .unwrap();
            assert!(!read.status.success());
            assert!(before.len() > length);
            assert!(!before.ends_with(b"\n"));
            let retry = invoke(&root, None);
            assert!(!retry.status.success());
            assert_eq!(std::fs::read(&file).unwrap(), before);
            println!(
                "validation native fault: partial-reserve5 unreadable suffix retained; no retry"
            );
            continue;
        }
        let observed = observe_from(&root, evidence);
        let consumed = if mode.contains('6') || mode.starts_with("summary-") {
            6
        } else if mode == "replace-request" || mode == "stale-request" {
            4
        } else {
            5
        };
        assert_eq!(observed["consumed"], consumed, "{mode}");
        assert_eq!(observed["producer_acknowledgement"], "unknown");
        let expected_unknown = if mode.contains("reserve") { 5 } else { 4 };
        assert_eq!(observed["unresolved_remote"], expected_unknown, "{mode}");
        assert_eq!(observed["unknown_usage"], expected_unknown, "{mode}");
        let restart = invoke(&root, None);
        assert!(!restart.status.success(), "{mode}");
        assert_eq!(std::fs::read(file).unwrap(), before, "{mode}");
        println!(
            "validation native fault: {mode} observed_consumed={consumed}; no restart or producer-ack inference"
        );
    }
    let root = storage();
    let mut child = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args([
            "provider-demo",
            "--validation-pair",
            "--fixture",
            "--storage",
        ])
        .arg(root.join("ledger"))
        .arg("--capture")
        .arg(root.join("evidence"))
        .env("LD_PRELOAD", &library)
        .env("VALIDATION_FAULT_ROOT", &root)
        .env("VALIDATION_FAULT_MODE", "hold-first")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while !root.join("holder.ready").exists() {
        if started.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            panic!("held first-persistence checkpoint unavailable");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let contender = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args([
            "provider-demo",
            "--validation-observe",
            "--fixture",
            "--storage",
        ])
        .arg(root.join("ledger"))
        .arg("--capture")
        .arg(root.join("evidence"))
        .output()
        .unwrap();
    assert!(!contender.status.success());
    assert!(contender.stdout.is_empty());
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(root.join("holder.release"))
        .unwrap();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            panic!("held pair failed to join");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["success"],
        true
    );
    assert_eq!(observe(&root)["consumed"], 6);
    println!(
        "validation held-owner proof: competing fresh reader refused during first persistence; pair completed only after release"
    );
}
