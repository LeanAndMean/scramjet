use super::*;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    process::Command,
    time::Duration,
};
#[test]
fn cancellation_and_runtime_affinity_run_in_independent_owners() {
    let shim_base =
        std::env::temp_dir().join(format!("scramjet-publication-shim-{}", std::process::id()));
    std::fs::create_dir(&shim_base).unwrap();
    let library = shim_base.join("fault.so");
    assert!(
        Command::new("cc")
            .args([
                "-shared",
                "-fPIC",
                "-Wall",
                "-Wextra",
                "-Werror",
                "tests/coding_invocation_fault.c",
                "-ldl",
                "-o"
            ])
            .arg(&library)
            .status()
            .unwrap()
            .success()
    );
    for case in [
        "abort-publication",
        "abort-completed-publication",
        "drop-cancelled-result",
        "publish-final-deadline",
        "publish-export-deadline",
        "delivery-partial",
        "delivery-flush",
        "delivery-cancel",
        "delivery-flush-cancel",
        "delivery-deadline",
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
            .env("LD_PRELOAD", &library)
            .env("SCRAMJET_FAULT_SUFFIX", "/control/invocation.jsonl")
            .env(
                "SCRAMJET_FAULT_KIND",
                if case == "publish-final-deadline" {
                    "Final"
                } else {
                    "Export"
                },
            )
            .env(
                "SCRAMJET_FAULT_MODE",
                if case.starts_with("abort-")
                    || (case.starts_with("publish-") && case.ends_with("deadline"))
                {
                    "delay"
                } else {
                    "none"
                },
            )
            .status()
            .unwrap();
        assert!(status.success(), "{case}");
    }
    std::fs::remove_dir_all(shim_base).unwrap();
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
        if case.starts_with("abort-") || case == "drop-cancelled-result" {
            aborted_publication(&project, &storage, &case).await;
        } else if case == "wrong-runtime" {
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
            if case.ends_with("deadline") || case.starts_with("delivery-") {
                assert!(matches!(
                    session.select(id, response, &cancel).unwrap(),
                    Selection::Finished
                ));
                if case.starts_with("publish-") {
                    session.store.state.header.deadline_ns = short_deadline();
                    assert!(matches!(
                        session.store.publish(true, true, &cancel),
                        Err(CodingError::Deadline)
                    ));
                } else {
                    let record = session.store.publish(true, true, &cancel).unwrap();
                    if case == "delivery-deadline" {
                        session.store.state.header.deadline_ns = short_deadline();
                    }
                    let mut writer = FailingWriter {
                        case: &case,
                        cancellation: cancel.clone(),
                        bytes: Vec::new(),
                        writes: 0,
                    };
                    let result = CodingInvocationResult {
                        record,
                        success: true,
                        failure: None,
                        publication: Some(agent::PublicationOwner {
                            store: session.store,
                            cancellation: cancel,
                        }),
                    };
                    assert!(result.deliver(&mut writer).is_err());
                    if case == "delivery-partial" {
                        assert_eq!(writer.bytes.len(), 11);
                    }
                }
                let journal =
                    std::fs::read_to_string(storage.join("control/invocation.jsonl")).unwrap();
                let rows = journal
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(rows.last().unwrap()["record"]["kind"], "Stopped");
                let copied = storage.join("control/export-1.json");
                let prior = copied.exists().then(|| std::fs::read(copied).unwrap());
                drop(session.provider);
                runtime.shutdown().await;
                assert!(
                    store::Store::open(
                        &project,
                        &storage,
                        "control",
                        ["input.md".into(), "reader.py".into()],
                        prior.as_deref(),
                        endpoint
                    )
                    .is_err()
                );
                return;
            } else if case == "publish-cancel" {
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
async fn aborted_publication(project: &std::path::Path, storage: &std::path::Path, case: &str) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() <= 16_384);
        }
        let header = String::from_utf8(header).unwrap();
        let size = header
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|n| n.trim().parse::<usize>().ok())
            })
            .unwrap();
        assert!(size <= 16_384);
        stream.read_exact(&mut vec![0; size]).unwrap();
        let events = crate::provider_fixture::text_events(
            "complete",
            crate::provider_fixture::known_usage(),
        );
        let wire = crate::provider_fixture::wire(&events, true);
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", wire.len()).unwrap();
        stream.write_all(&wire).unwrap();
    });
    let (invocation, runtime) = CodingInvocation::start(
        CodingInvocationOptions {
            project: project.into(),
            storage: storage.into(),
            run: "control".into(),
            files: ["input.md".into(), "reader.py".into()],
            prompt: "Finish without tools.".into(),
            previous: None,
            endpoint,
            owned_fixture: true,
        },
        |_, _, _| Ok(false),
    )
    .unwrap();
    let cancel = invocation.cancellation();
    let journal = storage.join("control/invocation.jsonl");
    let mut run = Box::pin(invocation.run(runtime));
    let limit = Instant::now() + Duration::from_secs(5);
    if case == "drop-cancelled-result" {
        let result = run.await.unwrap();
        assert!(result.success);
        cancel.cancel();
        drop(result);
    } else {
        loop {
            tokio::select! {
                result = &mut run => panic!("publication returned before abort: {:?}", result.err()),
                _ = tokio::time::sleep(Duration::from_millis(1)) => {
                    if std::fs::read_to_string(&journal).unwrap().lines().last().is_some_and(|row| row.contains("\"kind\":\"Export\"")) { break; }
                    assert!(Instant::now() < limit);
                }
            }
        }
        if case == "abort-completed-publication" {
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert!(
                std::fs::read_to_string(&journal)
                    .unwrap()
                    .lines()
                    .last()
                    .unwrap()
                    .contains("\"kind\":\"Export\"")
            );
        }
        drop(run);
    }
    assert!(cancel.is_cancelled());
    loop {
        if std::fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .last()
            .is_some_and(|row| row.contains("\"kind\":\"Stopped\""))
        {
            break;
        }
        assert!(
            Instant::now() < limit,
            "aborted publication did not durably stop"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    server.join().unwrap();
    let prior = std::fs::read(storage.join("control/export-1.json")).unwrap();
    let before = std::fs::read(&journal).unwrap();
    assert!(
        store::Store::open(
            project,
            storage,
            "control",
            ["input.md".into(), "reader.py".into()],
            Some(&prior),
            endpoint
        )
        .is_err()
    );
    assert_eq!(std::fs::read(journal).unwrap(), before);
}
fn short_deadline() -> u64 {
    let time = rustix::time::clock_gettime_dynamic(rustix::time::DynamicClockId::Boottime).unwrap();
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64 + 50_000_000
}
struct FailingWriter<'a> {
    case: &'a str,
    cancellation: EditCancellation,
    bytes: Vec<u8>,
    writes: usize,
}
impl std::io::Write for FailingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if self.case == "delivery-partial" {
            if self.writes > 1 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.bytes.extend_from_slice(&bytes[..11]);
            return Ok(11);
        }
        if self.case == "delivery-cancel" {
            self.cancellation.cancel();
        }
        if self.case == "delivery-deadline" {
            std::thread::sleep(Duration::from_millis(100));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.case == "delivery-flush" {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        if self.case == "delivery-flush-cancel" {
            self.cancellation.cancel();
        }
        Ok(())
    }
}
