use super::*;
use crate::coding::haiku::fault::{Fault, Point};
use std::process::Command;

fn point(name: &str) -> Point {
    match name {
        "reserve" => Point::Reserve,
        "append" => Point::Append,
        "short-append" => Point::ShortAppend,
        "sync" => Point::Sync,
        "authorize" => Point::Authorize,
        "authorized" => Point::Authorized,
        "dispatch" => Point::Dispatch,
        "dispatched" => Point::Dispatched,
        "open" => Point::CreateOpen,
        "partial" => Point::PartialWrite,
        "file-sync" => Point::FileSync,
        "directory-sync" => Point::DirectorySync,
        "created" => Point::Created,
        "start" => Point::Start,
        "reaped" => Point::Reaped,
        "process-ack" => Point::ProcessAck,
        "result" => Point::Result,
        _ => panic!("unknown fixture point"),
    }
}
#[test]
fn producer_exits_at_owned_creation_process_and_journal_boundaries() {
    let Some(path) = std::env::var_os("HAIKU_FAULT_ROOT") else {
        return;
    };
    let name = std::env::var("HAIKU_FAULT_POINT").unwrap();
    let root = PathBuf::from(path);
    let mut session =
        HaikuSession::create(&root.join("project"), &root.join("storage"), "fault").unwrap();
    let fault = Fault {
        point: point(&name),
        exit: true,
    };
    if name == "reserve" {
        session.journal.fault = Some(fault);
        let _ = session.prepare_request();
        panic!("fault did not execute");
    }
    let process = matches!(
        fault.point,
        Point::Start | Point::Reaped | Point::ProcessAck
    );
    let proposal = if process {
        create(&mut session, 1, "haiku.md", "poem\r\n");
        select(&mut session, 2, "read", json!({"path":"haiku.md"}));
        create(&mut session, 3, "print_haiku.py", HAIKU_SCRIPT);
        let HaikuStep::Approval(proposal) =
            select(&mut session, 4, "bash", json!({"command":HAIKU_COMMAND}))
        else {
            panic!()
        };
        proposal
    } else {
        let HaikuStep::Approval(proposal) = select(
            &mut session,
            1,
            "write",
            json!({"path":"haiku.md","content":"poem\r\n"}),
        ) else {
            panic!()
        };
        proposal
    };
    if matches!(
        fault.point,
        Point::Append | Point::ShortAppend | Point::Sync
    ) {
        session.journal.fault = Some(fault);
    } else {
        session.fault = Some(fault);
    }
    let _ = session.decide(proposal, true, &EditCancellation::default());
    panic!("producer fault did not execute");
}
#[test]
fn fresh_reader_never_replays_and_retains_boundary_uncertainty() {
    let Some(path) = std::env::var_os("HAIKU_READ_ROOT") else {
        return;
    };
    let root = PathBuf::from(path);
    let name = std::env::var("HAIKU_FAULT_POINT").unwrap();
    let journal = root.join("storage/fault/haiku.jsonl");
    let before = std::fs::read(&journal).unwrap();
    let targets: Vec<_> = NAMES
        .iter()
        .map(|name| std::fs::read(root.join("project").join(name)).ok())
        .collect();
    let recovered = crate::reconstruct_haiku_history(&root.join("storage"), "fault");
    if name == "short-append" {
        assert!(recovered.is_err());
    } else {
        let history = recovered.unwrap();
        assert!(!history.finished());
        let effects = history.effects();
        if matches!(
            name.as_str(),
            "dispatched"
                | "open"
                | "partial"
                | "file-sync"
                | "directory-sync"
                | "created"
                | "result"
                | "start"
                | "reaped"
                | "process-ack"
        ) {
            assert_eq!(
                effects.last().unwrap().state,
                crate::HaikuCertainty::Uncertain,
                "{name}"
            );
        }
        if name == "sync" {
            assert_eq!(
                effects.last().unwrap().state,
                crate::HaikuCertainty::NotDispatched
            );
        }
        assert!(
            history
                .entries
                .last()
                .is_some_and(|entry| !entry.acknowledged)
        );
    }
    assert_eq!(before, std::fs::read(&journal).unwrap());
    for (name, before) in NAMES.iter().zip(targets) {
        assert_eq!(before, std::fs::read(root.join("project").join(name)).ok());
    }
    assert!(HaikuSession::create(&root.join("project"), &root.join("storage"), "fault").is_err());
}
#[test]
fn isolated_parent_loss_and_fresh_process_recovery_cases() {
    let Some(_) = std::env::var_os("HAIKU_FAULT_PARENT") else {
        let _fork_gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
        let output = Command::new(std::env::current_exe().unwrap()).args(["--exact", "coding::haiku::tests::faults::isolated_parent_loss_and_fresh_process_recovery_cases", "--nocapture"]).env("HAIKU_FAULT_PARENT", "1").output().unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    };
    rustix::process::set_child_subreaper(Some(rustix::process::getpid())).unwrap();
    for name in [
        "reserve",
        "append",
        "short-append",
        "sync",
        "authorize",
        "authorized",
        "dispatch",
        "dispatched",
        "open",
        "partial",
        "file-sync",
        "directory-sync",
        "created",
        "result",
        "start",
        "reaped",
        "process-ack",
    ] {
        let dirs = Dirs::new();
        let base = dirs.root.parent().unwrap();
        let child = Command::new(std::env::current_exe().unwrap()).args(["--exact", "coding::haiku::tests::faults::producer_exits_at_owned_creation_process_and_journal_boundaries"]).env("HAIKU_FAULT_ROOT", base).env("HAIKU_FAULT_POINT", name).status().unwrap();
        assert_eq!(child.code(), Some(29), "{name}");
        let bytes = std::fs::read(dirs.storage.join("fault/haiku.jsonl")).unwrap();
        if name != "short-append" {
            for line in bytes
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
            {
                let value: serde_json::Value = serde_json::from_slice(line).unwrap();
                if value["record"]["kind"] == "Started" {
                    for field in ["launcher", "namespace_init"] {
                        let id = value["record"][field].as_u64().unwrap() as i32;
                        let pid = rustix::process::Pid::from_raw(id).unwrap();
                        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
                        loop {
                            match rustix::process::waitpid(
                                Some(pid),
                                rustix::process::WaitOptions::NOHANG,
                            ) {
                                Ok(Some(_)) => break,
                                Err(rustix::io::Errno::CHILD)
                                    if !std::path::Path::new(&format!("/proc/{id}")).exists() =>
                                {
                                    break;
                                }
                                result => {
                                    assert!(
                                        std::time::Instant::now() < until,
                                        "owned descendant not reaped: {name} {id} {result:?}"
                                    );
                                    std::thread::sleep(std::time::Duration::from_millis(10));
                                }
                            }
                        }
                    }
                }
            }
        }
        let reader = Command::new(std::env::current_exe().unwrap()).args(["--exact", "coding::haiku::tests::faults::fresh_reader_never_replays_and_retains_boundary_uncertainty", "--nocapture"]).env("HAIKU_READ_ROOT", base).env("HAIKU_FAULT_POINT", name).output().unwrap();
        assert!(
            reader.status.success(),
            "{name}: {} {}",
            String::from_utf8_lossy(&reader.stdout),
            String::from_utf8_lossy(&reader.stderr)
        );
        assert_eq!(
            bytes,
            std::fs::read(dirs.storage.join("fault/haiku.jsonl")).unwrap()
        );
    }
}
