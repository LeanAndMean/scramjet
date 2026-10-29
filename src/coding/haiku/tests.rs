use super::*;
#[path = "tests_faults.rs"]
mod faults;
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Dirs {
    root: PathBuf,
    storage: PathBuf,
}
impl Dirs {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "scramjet-haiku-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&base)
            .unwrap();
        let root = base.join("project");
        let storage = base.join("storage");
        for path in [&root, &storage] {
            std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        Self { root, storage }
    }
}
impl Drop for Dirs {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.root.parent().unwrap()).unwrap();
    }
}
fn captures(poem: &str) -> [HaikuCapture; 2] {
    [
        HaikuCapture {
            path: "haiku.md".into(),
            identity: [1, 2],
            content: poem.into(),
        },
        HaikuCapture {
            path: "print_haiku.py".into(),
            identity: [1, 3],
            content: HAIKU_SCRIPT.into(),
        },
    ]
}
fn selected(turn: usize, tool: &str, arguments: &str) -> ProviderResponse {
    let mut events = if tool.is_empty() {
        crate::provider_fixture::text_events("done", crate::provider_fixture::known_usage())
    } else {
        crate::provider_fixture::continuation_events("working", arguments, "opaque hé🙂")
    };
    fn rename(value: &mut serde_json::Value, tool: &str) {
        match value {
            serde_json::Value::Object(map) => {
                if map.get("name").is_some_and(|name| name == "inert") {
                    map.insert("name".into(), json!(tool));
                }
                for value in map.values_mut() {
                    rename(value, tool);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    rename(value, tool);
                }
            }
            _ => {}
        }
    }
    for event in &mut events {
        rename(event, tool);
    }
    crate::provider_fixture::qualify_events(&mut events, turn);
    let mut parser = crate::http::responses::ResponsesParser::new();
    for byte in crate::provider_fixture::wire(&events, true) {
        parser.byte(byte).unwrap();
    }
    let parsed = parser.complete().unwrap();
    ProviderResponse {
        id: RequestId(turn as u64),
        reservation: Some(turn),
        failure: None,
        outcome: Outcome::Success,
        envelope: parsed.envelope,
        usage: parsed.usage,
        remote_terminal: Some(crate::ProviderTerminalEvidence {
            response_id: parsed.response_id,
            reported_model: parsed.reported_model,
            status: parsed.status,
        }),
        durable: true,
        wire_bytes: 0,
    }
}
fn select(
    session: &mut HaikuSession,
    turn: usize,
    tool: &str,
    args: serde_json::Value,
) -> HaikuStep {
    session.prepare_request().unwrap();
    session
        .select_response(
            RequestId(turn as u64),
            selected(turn, tool, &args.to_string()),
        )
        .unwrap()
}
fn create(session: &mut HaikuSession, turn: usize, path: &str, content: &str) {
    let HaikuStep::Approval(proposal) = select(
        session,
        turn,
        "write",
        json!({"path":path,"content":content}),
    ) else {
        panic!()
    };
    session
        .decide(proposal, true, &EditCancellation::default())
        .unwrap();
}

#[test]
fn failed_or_blocked_start_ack_never_releases_command_and_still_reaps() {
    let input = captures("poem");
    let mut observed = None;
    let mut identities = Vec::new();
    let result = process::execute(
        &input,
        &EditCancellation::default(),
        |launcher, init| {
            identities = [launcher, init]
                .into_iter()
                .map(|id| {
                    rustix::process::pidfd_open(
                        rustix::process::Pid::from_raw(id as i32).unwrap(),
                        rustix::process::PidfdFlags::empty(),
                    )
                    .unwrap()
                })
                .collect();
            Err(std::io::Error::other("injected start acknowledgement failure").into())
        },
        |result| observed = Some(result.clone()),
    );
    assert!(result.is_err());
    let observed = observed.unwrap();
    assert!(observed.launcher_reaped && observed.namespace_dead);
    assert!(observed.stdout.is_empty());
    for identity in &identities {
        let mut poll = [rustix::event::PollFd::new(
            identity,
            rustix::event::PollFlags::IN,
        )];
        rustix::event::poll(
            &mut poll,
            Some(&rustix::event::Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }),
        )
        .unwrap();
        assert!(poll[0].revents().contains(rustix::event::PollFlags::IN));
    }
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let proof = std::thread::spawn(move || {
        let identities: Vec<std::os::fd::OwnedFd> = receive.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5250));
        for identity in &identities {
            let mut poll = [rustix::event::PollFd::new(
                identity,
                rustix::event::PollFlags::IN,
            )];
            rustix::event::poll(
                &mut poll,
                Some(&rustix::event::Timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )
            .unwrap();
            assert!(
                poll[0].revents().contains(rustix::event::PollFlags::IN),
                "sandbox outlived blocked journal acknowledgement"
            );
        }
    });
    let result = process::execute(
        &input,
        &EditCancellation::default(),
        |launcher, init| {
            let identities: Vec<std::os::fd::OwnedFd> = [launcher, init]
                .into_iter()
                .map(|id| {
                    rustix::process::pidfd_open(
                        rustix::process::Pid::from_raw(id as i32).unwrap(),
                        rustix::process::PidfdFlags::empty(),
                    )
                    .unwrap()
                })
                .collect();
            send.send(identities).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5500));
            Ok(())
        },
        |_| {},
    )
    .unwrap();
    proof.join().unwrap();
    assert!(result.stdout.is_empty());
    assert_eq!(
        result.stopped.as_deref(),
        Some("isolated wall-time resource limit")
    );
    assert!(result.launcher_reaped && result.namespace_dead);
}

#[test]
fn cancelled_or_expired_dispatch_ack_cannot_create_late_targets() {
    for expire in [false, true] {
        let dirs = Dirs::new();
        let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "late-dispatch").unwrap();
        let HaikuStep::Approval(proposal) = select(
            &mut session,
            1,
            "write",
            json!({"path":"haiku.md","content":"poem"}),
        ) else {
            panic!()
        };
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        session.pause = Some(fault::Pause {
            point: fault::Point::Dispatched,
            entered: entered.clone(),
            gate: gate.clone(),
        });
        if expire {
            session.started = std::time::Instant::now() - std::time::Duration::from_secs(299);
        }
        let cancel = EditCancellation::default();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let result = session.decide(proposal, true, &worker_cancel);
            (session, result)
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !entered.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < until,
                "dispatch boundary not reached"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if expire {
            std::thread::sleep(std::time::Duration::from_millis(1050));
        } else {
            cancel.cancel();
        }
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let (session, result) = worker.join().unwrap();
        assert!(if expire {
            matches!(result, Err(CodingError::Deadline))
        } else {
            matches!(result, Err(CodingError::Cancelled))
        });
        assert!(!dirs.root.join("haiku.md").exists());
        assert_eq!(
            session.history().effects()[0].state,
            HaikuCertainty::Uncertain
        );
    }
}

#[test]
fn expired_response_retains_accounting_and_cannot_select_effects() {
    let dirs = Dirs::new();
    let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "deadline").unwrap();
    session.prepare_request().unwrap();
    session.started = std::time::Instant::now() - std::time::Duration::from_secs(301);
    let result = session.select_response(
        RequestId(1),
        selected(1, "write", "{\"path\":\"haiku.md\",\"content\":\"poem\"}"),
    );
    assert!(matches!(result, Err(CodingError::Deadline)));
    assert!(!dirs.root.join("haiku.md").exists());
    let receipts = session.history().attempts();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].request_id, 1);
    assert_eq!(receipts[0].ordinal, Some(1));
    assert!(receipts[0].durable);
    assert!(receipts[0].usage.as_ref().unwrap().complete());
}

#[test]
fn fixed_reader_preserves_all_admitted_bytes_and_reaps_namespace() {
    let captures = captures("# Haiku\r\nRust folds\rstreams answer\nNUL\0é🙂\n");
    let result = process::execute(
        &captures,
        &EditCancellation::default(),
        |_, _| Ok(()),
        |_| {},
    )
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout, captures[0].content);
    assert!(result.stderr.is_empty(), "{result:?}");
    assert!(result.stopped.is_none(), "{result:?}");
    assert!(result.launcher_reaped && result.namespace_dead);
}

#[test]
fn exact_approved_effect_owner_completes_five_response_journey() {
    let dirs = Dirs::new();
    let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "journey").unwrap();
    let poem = "Rust unfolds\r\nStreams answer\rFiles remember\0é🙂\n";
    create(&mut session, 1, "haiku.md", poem);
    let HaikuStep::Read { content, .. } =
        select(&mut session, 2, "read", json!({"path":"haiku.md"}))
    else {
        panic!()
    };
    assert_eq!(content, poem);
    create(&mut session, 3, "print_haiku.py", HAIKU_SCRIPT);
    let HaikuStep::Approval(proposal) =
        select(&mut session, 4, "bash", json!({"command":HAIKU_COMMAND}))
    else {
        panic!()
    };
    session
        .decide(proposal, true, &EditCancellation::default())
        .unwrap();
    assert!(session.successful());
    assert!(matches!(
        select(&mut session, 5, "", json!({})),
        HaikuStep::Finished
    ));
    for (name, bytes) in [("haiku.md", poem), ("print_haiku.py", HAIKU_SCRIPT)] {
        let path = dirs.root.join(name);
        assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
    }
    let history = std::fs::read_to_string(dirs.storage.join("journey/haiku.jsonl")).unwrap();
    let records: Vec<serde_json::Value> = history
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for (sequence, record) in records.iter().enumerate() {
        assert_eq!(record["sequence"], sequence);
    }
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"]["kind"] == "Created")
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"]["kind"] == "Process")
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"]["kind"] == "Result")
            .count(),
        4
    );
    let encoded: serde_json::Value = serde_json::from_str(
        session
            .history
            .iter()
            .filter_map(|input| {
                if let ProviderInput::ToolResult { output, .. } = input {
                    Some(output)
                } else {
                    None
                }
            })
            .next_back()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(encoded["stdout"], poem);
}

#[test]
fn foreign_owner_changed_root_lock_and_file_metadata_cannot_authorize() {
    for case in 0..5 {
        let dirs = Dirs::new();
        let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "binding").unwrap();
        let HaikuStep::Approval(proposal) = select(
            &mut session,
            1,
            "write",
            json!({"path":"haiku.md","content":"poem"}),
        ) else {
            panic!()
        };
        if case == 0 {
            let other = Dirs::new();
            let mut foreign = HaikuSession::create(&other.root, &other.storage, "binding").unwrap();
            let _ = select(
                &mut foreign,
                1,
                "write",
                json!({"path":"haiku.md","content":"poem"}),
            );
            assert!(matches!(
                foreign.decide(proposal, true, &EditCancellation::default()),
                Err(CodingError::Stale)
            ));
            assert!(!other.root.join("haiku.md").exists());
            continue;
        }
        if case == 1 {
            std::fs::rename(&dirs.root, dirs.root.with_file_name("old-root")).unwrap();
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dirs.root)
                .unwrap();
        }
        if case == 2 {
            std::fs::rename(
                dirs.root.join(crate::PROJECT_LOCK_NAME),
                dirs.root.join("old-lock"),
            )
            .unwrap();
            std::fs::write(dirs.root.join(crate::PROJECT_LOCK_NAME), b"").unwrap();
            std::fs::set_permissions(
                dirs.root.join(crate::PROJECT_LOCK_NAME),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        if case == 3 {
            std::os::unix::fs::symlink("absent", dirs.root.join("haiku.md")).unwrap();
        }
        if case == 4 {
            std::fs::set_permissions(&dirs.root, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(
            session
                .decide(proposal, true, &EditCancellation::default())
                .is_err()
        );
        assert!(std::fs::read(dirs.root.join("haiku.md")).is_err());
    }
    for mode in [0o644, 0o600] {
        let dirs = Dirs::new();
        let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "metadata").unwrap();
        create(&mut session, 1, "haiku.md", "poem");
        if mode == 0o600 {
            std::fs::hard_link(dirs.root.join("haiku.md"), dirs.root.join("other.md")).unwrap();
        } else {
            std::fs::set_permissions(
                dirs.root.join("haiku.md"),
                std::fs::Permissions::from_mode(mode),
            )
            .unwrap();
        }
        session.prepare_request().unwrap();
        assert!(
            session
                .select_response(RequestId(2), selected(2, "read", "{\"path\":\"haiku.md\"}"))
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(dirs.root.join("haiku.md")).unwrap(),
            "poem"
        );
    }
}

#[test]
fn complete_public_envelope_limits_refuse_before_proposal_or_effect() {
    let dirs = Dirs::new();
    let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "envelope").unwrap();
    session.prepare_request().unwrap();
    let mut response = selected(1, "write", "{\"path\":\"haiku.md\",\"content\":\"poem\"}");
    let id = "opaque/\"\0é".repeat(3000);
    response.envelope.as_mut().unwrap().response_id = id.clone();
    response.remote_terminal.as_mut().unwrap().response_id = id;
    assert!(matches!(
        session.select_response(RequestId(1), response),
        Err(CodingError::ResourceLimit(_))
    ));
    assert!(!dirs.root.join("haiku.md").exists());
}

#[test]
fn denial_stale_proposal_and_appeared_target_never_create() {
    for case in 0..3 {
        let dirs = Dirs::new();
        let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "negative").unwrap();
        let HaikuStep::Approval(mut proposal) = select(
            &mut session,
            1,
            "write",
            json!({"path":"haiku.md","content":"poem"}),
        ) else {
            panic!()
        };
        if case == 1 {
            Arc::make_mut(&mut proposal.intent).request_id += 1;
        }
        if case == 2 {
            std::fs::write(dirs.root.join("haiku.md"), "appeared").unwrap();
            std::fs::set_permissions(
                dirs.root.join("haiku.md"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        assert!(
            session
                .decide(proposal, case != 0, &EditCancellation::default())
                .is_err()
        );
        if case == 2 {
            assert_eq!(
                std::fs::read_to_string(dirs.root.join("haiku.md")).unwrap(),
                "appeared"
            );
        } else {
            assert!(!dirs.root.join("haiku.md").exists());
        }
        assert!(session.prepare_request().is_err());
    }
}

#[test]
fn invalid_failed_or_foreign_envelopes_have_no_effect_authority() {
    assert!(
        serde_json::from_str::<WriteArgs>(
            "{\"path\":\"haiku.md\",\"path\":\"haiku.md\",\"content\":\"poem\"}"
        )
        .is_err()
    );
    for (tool, arguments, fault) in [
        (
            "write",
            "{\"path\":\"../haiku.md\",\"content\":\"poem\"}",
            0,
        ),
        (
            "write",
            "{\"path\":\"haiku.md\",\"content\":\"poem\",\"extra\":1}",
            0,
        ),
        ("write", "{\"path\":\"haiku.md\",\"content\":\"poem\"}", 1),
        ("write", "{\"path\":\"haiku.md\",\"content\":\"poem\"}", 2),
        ("write", "{\"path\":\"haiku.md\",\"content\":\"poem\"}", 3),
        ("", "{}", 0),
    ] {
        let dirs = Dirs::new();
        let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "invalid").unwrap();
        session.prepare_request().unwrap();
        let mut response = selected(1, tool, arguments);
        if fault == 1 {
            response.durable = false;
        }
        if fault == 2 {
            response.outcome = Outcome::Incomplete;
        }
        if fault == 3 {
            response.id = RequestId(77);
        }
        assert!(session.select_response(RequestId(1), response).is_err());
        assert!(!dirs.root.join("haiku.md").exists());
        assert!(session.prepare_request().is_err());
    }
}

#[test]
fn changed_process_capture_stops_before_dispatch() {
    let dirs = Dirs::new();
    let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "changed").unwrap();
    create(&mut session, 1, "haiku.md", "poem");
    select(&mut session, 2, "read", json!({"path":"haiku.md"}));
    create(&mut session, 3, "print_haiku.py", HAIKU_SCRIPT);
    let HaikuStep::Approval(proposal) =
        select(&mut session, 4, "bash", json!({"command":HAIKU_COMMAND}))
    else {
        panic!()
    };
    std::fs::write(dirs.root.join("haiku.md"), "changed").unwrap();
    assert!(
        session
            .decide(proposal, true, &EditCancellation::default())
            .is_err()
    );
    let history = std::fs::read_to_string(dirs.storage.join("changed/haiku.jsonl")).unwrap();
    assert!(!history.contains("\"operation\":4"));
    assert!(session.process_result().is_none());
}

#[test]
fn installed_cpu_address_and_descriptor_refusals_reclaim_namespace() {
    for (script, expected) in [
        ("x = bytearray(400*1024*1024)\n", "MemoryError"),
        (
            "import os\nx=[os.open('/dev/null',os.O_RDONLY) for _ in range(100)]\n",
            "Too many open files",
        ),
    ] {
        let result =
            process::control(&captures("poem"), &EditCancellation::default(), script).unwrap();
        assert_eq!(result.exit_code, Some(1));
        assert!(result.stderr.contains(expected), "{result:?}");
        assert!(result.launcher_reaped && result.namespace_dead);
    }
    let result = process::control(
        &captures("poem"),
        &EditCancellation::default(),
        "while True: pass\n",
    )
    .unwrap();
    assert_eq!(result.exit_code, Some(137), "{result:?}");
    assert!(result.signal.is_none());
    assert!(result.stopped.is_none());
    assert!(result.launcher_reaped && result.namespace_dead);
}

#[test]
fn owned_namespace_bounds_output_time_and_cancellation() {
    for (script, cancel) in [
        ("import sys\nsys.stdout.write('x'*20000)\n", false),
        ("import time\ntime.sleep(10)\n", true),
        ("import time\ntime.sleep(10)\n", false),
    ] {
        let cancellation = EditCancellation::default();
        let child_cancel = cancellation.clone();
        let (started, receive) = std::sync::mpsc::sync_channel(1);
        let signal = std::thread::spawn(move || {
            receive.recv().unwrap();
            if cancel {
                std::thread::sleep(std::time::Duration::from_millis(100));
                child_cancel.cancel();
            }
        });
        let result = process::control_started(&captures("poem"), &cancellation, script, |_, _| {
            started.send(()).unwrap();
            Ok(())
        })
        .unwrap();
        signal.join().unwrap();
        assert!(result.stopped.is_some(), "{result:?}");
        assert!(result.launcher_reaped && result.namespace_dead);
        assert!(result.stdout.len() + result.stderr.len() <= 16384);
    }
}

#[test]
fn detached_journal_chain_cannot_authorize_effects() {
    for case in 0..3 {
        let dirs = Dirs::new();
        let mut session = HaikuSession::create(&dirs.root, &dirs.storage, "detached").unwrap();
        let HaikuStep::Approval(proposal) = select(
            &mut session,
            1,
            "write",
            json!({"path":"haiku.md","content":"poem"}),
        ) else {
            panic!()
        };
        match case {
            0 => std::fs::rename(
                dirs.storage.join("detached/haiku.jsonl"),
                dirs.storage.join("detached/saved.jsonl"),
            )
            .unwrap(),
            1 => {
                std::fs::rename(dirs.storage.join("detached"), dirs.storage.join("saved")).unwrap()
            }
            _ => {
                std::fs::rename(&dirs.storage, dirs.storage.with_file_name("saved-storage"))
                    .unwrap();
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&dirs.storage)
                    .unwrap();
            }
        }
        assert!(
            session
                .decide(proposal, true, &EditCancellation::default())
                .is_err()
        );
        assert!(!dirs.root.join("haiku.md").exists());
        assert!(session.prepare_request().is_err());
    }
}

#[test]
fn installed_isolation_has_read_only_files_no_host_paths_or_leaked_descriptors() {
    let _unrelated_inheritable =
        rustix::fs::open("/dev/null", OFlags::RDONLY, Mode::empty()).unwrap();
    let script = r#"import os, resource, json, sys
from pathlib import Path
try:
    Path('haiku.md').write_bytes(b'changed')
    writable = True
except OSError:
    writable = False
print(json.dumps({'writable':writable,'paths':[Path(p).exists() for p in ['/home','/etc','/root','/workspace/storage']], 'fds':[int(p) for p in os.listdir('/proc/self/fd')], 'environment':dict(os.environ), 'limits':[resource.getrlimit(r) for r in [resource.RLIMIT_CPU,resource.RLIMIT_AS,resource.RLIMIT_NOFILE,resource.RLIMIT_CORE]], 'net':os.readlink('/proc/self/ns/net'), 'isolated':sys.flags.isolated, 'customized':any(n in sys.modules for n in ['sitecustomize','usercustomize'])}))
"#;
    let result = process::control(&captures("poem"), &EditCancellation::default(), script).unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.stderr.is_empty());
    let observed: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(observed["writable"], false);
    assert_eq!(observed["isolated"], 1);
    assert_eq!(observed["customized"], false);
    assert_eq!(observed["paths"], json!([false, false, false, false]));
    assert!(
        observed["fds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|fd| fd.as_u64().unwrap() <= 3)
    );
    assert_eq!(
        observed["limits"],
        json!([[2, 2], [268435456, 268435456], [32, 32], [0, 0]])
    );
    let environment = observed["environment"].as_object().unwrap();
    assert_eq!(environment["PATH"], "/usr/bin:/bin");
    assert!(
        environment
            .keys()
            .all(|key| ["PATH", "PWD", "SHLVL", "_", "LC_CTYPE"].contains(&key.as_str()))
    );
    assert_ne!(
        observed["net"],
        std::fs::read_link("/proc/self/ns/net")
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(result.launcher_reaped && result.namespace_dead);
}

#[test]
fn short_process_waits_for_supervision_and_descendants_die_with_namespace() {
    let result = process::control(
        &captures("poem"),
        &EditCancellation::default(),
        "import os, time\nif os.fork() == 0: time.sleep(30)\n",
    )
    .unwrap();
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.launcher_reaped && result.namespace_dead);
    assert!(result.stopped.is_none());
}
