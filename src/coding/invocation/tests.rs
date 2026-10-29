use super::*;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    process::Command,
};
#[test]
fn cancellation_and_runtime_affinity_run_in_independent_owners() {
    for case in [
        "cancel-read",
        "cancel-final",
        "publish-cancel",
        "wrong-runtime",
    ] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "coding::invocation::tests::selection_control_child",
                "--nocapture",
            ])
            .env("SCRAMJET_INVOCATION_CONTROL", case)
            .status()
            .unwrap();
        assert!(status.success(), "{case}");
    }
}
#[test]
fn selection_control_child() {
    let Ok(case) = std::env::var("SCRAMJET_INVOCATION_CONTROL") else {
        return;
    };
    let base = std::env::temp_dir().join(format!(
        "scramjet-invocation-control-{}",
        std::process::id()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&base)
        .unwrap();
    let project = base.join("project");
    let storage = base.join("storage");
    for p in [&project, &storage] {
        std::fs::DirBuilder::new().mode(0o700).create(p).unwrap();
    }
    let input = project.join("input.md");
    std::fs::write(&input, "retained input").unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    executor.block_on(async {
        if case == "wrong-runtime" {
            let options = CodingInvocationOptions {
                project: project.clone(),
                storage: storage.clone(),
                run: "control".into(),
                files: ["input.md".into(), "reader.py".into()],
                prompt: "Read the selected file.".into(),
                previous: None,
                endpoint,
                owned_fixture: true,
            };
            let (invocation, original) = CodingInvocation::start(options, |intent, _, _| {
                eprintln!("{}", serde_json::to_string(intent).unwrap());
                Ok(true)
            })
            .unwrap();
            original.shutdown().await;
            let foreign = base.join("foreign-provider");
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&foreign)
                .unwrap();
            let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            other.set_nonblocking(true).unwrap();
            let runtime =
                crate::Runtime::new_provider(other.local_addr().unwrap(), &foreign).unwrap();
            let result = invocation.run(runtime).await.unwrap();
            assert!(!result.success);
            let record: Value = serde_json::from_slice(&result.record).unwrap();
            assert_eq!(record["counters"]["attempts"], 0);
            assert_eq!(record["disposition"], "stopped");
            for p in [
                storage.join("control/provider/provider-attempts.jsonl"),
                foreign.join("provider-attempts.jsonl"),
            ] {
                assert_eq!(std::fs::read_to_string(p).unwrap().lines().count(), 1);
            }
            assert!(matches!(other.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock));
        } else {
            let (mut store, runtime, provider) = store::Store::open(
                &project,
                &storage,
                "control",
                ["input.md".into(), "reader.py".into()],
                None,
                endpoint,
            )
            .unwrap();
            store.begin("Read the selected file.".into()).unwrap();
            let mut session = Session {
                store,
                provider,
                pending: None,
                owner: Arc::new(()),
            };
            session.prepare().unwrap();
            let mut events = if case == "cancel-read" {
                crate::provider_fixture::continuation_events(
                    "read",
                    r#"{"path":"input.md"}"#,
                    "opaque",
                )
            } else {
                crate::provider_fixture::text_events(
                    "final",
                    crate::provider_fixture::known_usage(),
                )
            };
            if case == "cancel-read" {
                for event in &mut events {
                    if event["item"]["type"] == "function_call" {
                        event["item"]["name"] = json!("read");
                    }
                }
                for item in events.last_mut().unwrap()["response"]["output"]
                    .as_array_mut()
                    .unwrap()
                {
                    if item["type"] == "function_call" {
                        item["name"] = json!("read");
                    }
                }
            }
            let mut parser = crate::http::responses::ResponsesParser::new();
            for byte in crate::provider_fixture::wire(&events, true) {
                parser.byte(byte).unwrap();
            }
            let parsed = parser.complete().unwrap();
            let id = RequestId(71);
            let terminal = crate::ProviderTerminalEvidence {
                response_id: parsed.response_id,
                reported_model: parsed.reported_model,
                status: parsed.status,
            };
            let ordinal = {
                let mut ledger = session.provider.lock().unwrap();
                let ordinal = ledger.reserve(id.0).unwrap();
                ledger
                    .joined(ordinal, Some(terminal.clone()), parsed.usage.clone())
                    .unwrap();
                ordinal
            };
            let response = ProviderResponse {
                id,
                reservation: Some(ordinal),
                failure: None,
                outcome: Outcome::Success,
                envelope: parsed.envelope,
                usage: parsed.usage,
                remote_terminal: Some(terminal),
                durable: true,
                wire_bytes: 0,
            };
            let cancel = EditCancellation::default();
            if case == "publish-cancel" {
                assert!(matches!(
                    session.select(id, response, &cancel).unwrap(),
                    Selection::Finished
                ));
                cancel.cancel();
                assert!(matches!(
                    session.store.publish(true, true, &cancel),
                    Err(CodingError::Cancelled)
                ));
            } else {
                cancel.cancel();
                std::fs::remove_file(&input).unwrap();
                assert!(matches!(
                    session.select(id, response, &cancel),
                    Err(CodingError::Cancelled)
                ));
                session.store.stop("cancelled".into()).unwrap();
            }
            assert_eq!(session.store.state.counters.reads, 0);
            assert!(!storage.join("control/export-1.json").exists());
            let journal =
                std::fs::read_to_string(storage.join("control/invocation.jsonl")).unwrap();
            assert!(journal.contains("\"kind\":\"Receipt\""));
            assert!(!journal.contains("\"kind\":\"Charged\""));
            assert!(!journal.contains("\"kind\":\"Final\""));
            let history = runtime.provider_history().unwrap();
            assert_eq!(history.consumed(), 1);
            assert_eq!(history.unknown_usage(), 0);
            assert_eq!(history.unresolved_remote(), 0);
            runtime.shutdown().await;
        }
    });
    assert!(matches!(listener.accept(),Err(e)if e.kind()==std::io::ErrorKind::WouldBlock));
    std::fs::remove_dir_all(base).unwrap();
}
