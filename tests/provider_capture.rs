#[allow(dead_code)]
#[path = "../src/provider_demo.rs"]
mod demo;
use scramjet_rs::{Outcome, replay_provider_data};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    process::Command,
};
fn storage() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "s414-raw-{}-{}",
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
    path
}
#[test]
fn private_raw_capture_and_fresh_process_replay() {
    for case in [
        "text",
        "call",
        "disagreement",
        "write-failure",
        "sync-failure",
    ] {
        let directory = storage();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "raw_capture_child", "--nocapture"])
            .env("S414_RAW_CASE", case)
            .env("S414_RAW_DIRECTORY", &directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-private"));
        let ledger = std::fs::read(directory.join("provider-attempts.jsonl")).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&ledger)
                .lines()
                .filter(|l| l.contains("Reserved"))
                .count(),
            1
        );
        let artifact = directory.join("capture");
        assert_eq!(
            std::fs::metadata(&artifact).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in ["response.data", "diagnostic.json", "provenance.json"] {
            assert_eq!(
                std::fs::metadata(artifact.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        if case.ends_with("failure") {
            assert!(
                std::fs::read(artifact.join("provenance.json"))
                    .unwrap()
                    .is_empty()
            );
            continue;
        }
        let before: Vec<_> = ["response.data", "diagnostic.json", "provenance.json"]
            .iter()
            .map(|name| std::fs::read(artifact.join(name)).unwrap())
            .collect();
        let replay = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(["provider-demo", "--replay"])
            .arg(&artifact)
            .output()
            .unwrap();
        assert!(
            replay.status.success(),
            "{}",
            String::from_utf8_lossy(&replay.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
        assert_eq!(report["execution_authority"], false);
        assert_eq!(report["settlement_authority"], false);
        assert_eq!(report["parser_accepted"], case != "disagreement");
        assert_eq!(report["full_stream"], true);
        assert!(!String::from_utf8_lossy(&replay.stdout).contains("synthetic-private"));
        assert!(!String::from_utf8_lossy(&replay.stdout).contains("resp_1"));
        assert_eq!(
            ledger,
            std::fs::read(directory.join("provider-attempts.jsonl")).unwrap()
        );
        let after: Vec<_> = ["response.data", "diagnostic.json", "provenance.json"]
            .iter()
            .map(|name| std::fs::read(artifact.join(name)).unwrap())
            .collect();
        assert_eq!(before, after);
        let provenance: serde_json::Value = serde_json::from_slice(&before[2]).unwrap();
        assert_eq!(provenance["reservation"], 1);
        assert_eq!(provenance["route"], "fixture-responses");
        assert_eq!(provenance["captured_bytes"], before[0].len());
        assert_eq!(provenance["local_joined"], true);
        assert_eq!(provenance["candidate_sha256"].as_str().unwrap().len(), 64);
        if case == "disagreement" {
            assert!(String::from_utf8_lossy(&before[0]).contains("synthetic-private"));
            assert_eq!(report["failure"]["field"], "response.output.item");
        }
        for (field, value) in [
            ("parser_fed_bytes", serde_json::json!(0)),
            ("parser_accepted", serde_json::json!(true)),
            ("overflow", serde_json::json!(true)),
            ("body_eof", serde_json::json!(false)),
        ] {
            if (case == "disagreement" && matches!(field, "parser_fed_bytes" | "body_eof"))
                || (case != "disagreement" && field == "parser_accepted")
            {
                continue;
            }
            let mut altered = provenance.clone();
            altered[field] = value;
            std::fs::write(
                artifact.join("provenance.json"),
                serde_json::to_vec(&altered).unwrap(),
            )
            .unwrap();
            assert!(demo::capture::replay(&artifact).is_err(), "{case}: {field}");
        }
        std::fs::write(artifact.join("provenance.json"), &before[2]).unwrap();
        if case != "disagreement" {
            let mut prefix = provenance.clone();
            prefix["body_eof"] = serde_json::json!(false);
            prefix["parser_accepted"] = serde_json::json!(false);
            std::fs::write(
                artifact.join("provenance.json"),
                serde_json::to_vec(&prefix).unwrap(),
            )
            .unwrap();
            let report: serde_json::Value =
                serde_json::from_slice(&demo::capture::replay(&artifact).unwrap()).unwrap();
            assert_eq!(report["full_stream"], false);
            assert_eq!(report["parser_accepted"], true);
            std::fs::write(artifact.join("provenance.json"), &before[2]).unwrap();
        }
        let mut corrupted = before[0].clone();
        corrupted[0] ^= 1;
        std::fs::write(artifact.join("response.data"), corrupted).unwrap();
        assert!(demo::capture::replay(&artifact).is_err());
    }
}
#[test]
fn raw_capture_child() {
    let Ok(case) = std::env::var("S414_RAW_CASE") else {
        return;
    };
    let directory = PathBuf::from(std::env::var_os("S414_RAW_DIRECTORY").unwrap());
    let mut prepared = demo::capture::prepare(&directory.join("capture"), false).unwrap();
    if case.ends_with("failure") {
        prepared.fail_raw_for_test(if case == "write-failure" {
            "/dev/full"
        } else {
            "/dev/null"
        });
    }
    let events = if case == "disagreement" {
        Some(demo::fixture::disagreement_events())
    } else {
        None
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(demo::run_captured(
            directory.clone(),
            case == "call",
            false,
            None,
            events,
            Some(prepared),
        ));
    match case.as_str() {
        "disagreement" => assert_eq!(
            result.unwrap_err(),
            "terminal/usage acceptance failed; stop without retry"
        ),
        "write-failure" | "sync-failure" => assert_eq!(
            result.unwrap_err(),
            "raw capture write/sync failed; no retry"
        ),
        _ => assert!(result.is_ok(), "{result:?}"),
    };
    let ledger: serde_json::Value = serde_json::from_str(
        std::fs::read_to_string(directory.join("provider-attempts.jsonl"))
            .unwrap()
            .lines()
            .last()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ledger["terminal"]["response_id"], "resp_1");
    assert_eq!(ledger["terminal"]["status"], "completed");
    assert_eq!(ledger["usage"]["total_tokens"], 15);
}
#[test]
fn preopen_refuses_before_reservation_and_replay_refuses_unsafe_files() {
    let directory = storage();
    let existing = directory.join("existing");
    std::fs::create_dir(&existing).unwrap();
    let symlink = directory.join("symlink");
    std::os::unix::fs::symlink(&existing, &symlink).unwrap();
    for path in [&existing, &symlink] {
        let output = Command::new(env!("CARGO_BIN_EXE_scramjet-rs"))
            .args(["provider-demo", "--fixture", "--storage"])
            .arg(&directory)
            .arg("--capture")
            .arg(path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!directory.join("provider-attempts.jsonl").exists());
    }
    let artifact = directory.join("prepared");
    let prepared = demo::capture::prepare(&artifact, false).unwrap();
    drop(prepared);
    assert!(demo::capture::prepare(&artifact, false).is_err());
    std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(demo::capture::replay(&artifact).is_err());
    std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_file(artifact.join("response.data")).unwrap();
    std::os::unix::fs::symlink("diagnostic.json", artifact.join("response.data")).unwrap();
    assert!(demo::capture::replay(&artifact).is_err());
    assert!(!directory.join("provider-attempts.jsonl").exists());
}
#[test]
fn replay_preserves_strict_failure_and_valid_representations() {
    for crlf in [false, true] {
        for events in [
            demo::fixture::text_events("hé🙂\\\"\n", demo::fixture::known_usage()),
            demo::fixture::call_events("{\"message\":\"hé🙂\"}", demo::fixture::known_usage()),
        ] {
            let body = demo::fixture::wire(&events, crlf);
            let replay = replay_provider_data(&body);
            assert_eq!(replay.outcome, Outcome::Success);
            assert!(replay.parser_accepted);
            for end in [0, 1, body.len() - 1] {
                assert!(!replay_provider_data(&body[..end]).parser_accepted);
            }
            let mut trailing = body.clone();
            trailing.extend_from_slice(b"contradictory");
            assert!(!replay_provider_data(&trailing).parser_accepted);
        }
        let body = demo::fixture::wire(&demo::fixture::disagreement_events(), crlf);
        let replay = replay_provider_data(&body);
        assert_eq!(replay.outcome, Outcome::Unsupported);
        assert!(replay.failure.unwrap().final_difference.is_some());
    }
    assert_eq!(
        replay_provider_data(&vec![b'x'; scramjet_rs::PROVIDER_CAPTURE_BYTES + 1]).outcome,
        Outcome::ResourceLimit
    );
}
