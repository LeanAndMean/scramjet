#[path = "support/coding_invocation.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::{Dirs, command, server};
#[test]
fn generated_profile_parent_loss_reclaims_and_never_resumes() {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "parent_loss_child", "--nocapture"])
        .env("SCRAMJET_PARENT_LOSS", "1")
        .status()
        .unwrap();
    assert!(status.success());
}
#[test]
fn parent_loss_child() {
    if std::env::var_os("SCRAMJET_PARENT_LOSS").is_none() {
        return;
    }
    rustix::process::set_child_subreaper(Some(rustix::process::getpid())).unwrap();
    let dirs = Dirs::new();
    let library = dirs.base.join("fault.so");
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
    let names = ["input.md", "reader.py"];
    let prompts = [
        "Create input and finish.".to_owned(),
        "Create a reader and run it.".to_owned(),
    ];
    let first = dirs.input("first", prompts[0].as_bytes());
    let second = dirs.input("second", prompts[1].as_bytes());
    let calls = vec![
        Some(("write", json!({"path":names[0],"content":"owned bytes"}))),
        None,
        Some((
            "write",
            json!({"path":names[1],"content":"import time\ntime.sleep(10)\n"}),
        )),
        Some(("python", json!({"path":names[1]}))),
    ];
    let (endpoint, server) = server(
        calls,
        prompts,
        2,
        "owned bytes".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let output = command(&dirs, endpoint, &names, &first, None)
        .output()
        .unwrap();
    assert!(output.status.success());
    let prior = dirs.input("prior", &output.stdout);
    let target = dirs.storage.join("ordinary-run/invocation.jsonl");
    let output = command(&dirs, endpoint, &names, &second, Some(&prior))
        .env("LD_PRELOAD", library)
        .env("SCRAMJET_FAULT_TARGET", &target)
        .env("SCRAMJET_FAULT_KIND", "Started")
        .env("SCRAMJET_FAULT_MODE", "exit")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(73));
    assert_eq!(server.join().unwrap().len(), 4);
    let rows = fs::read_to_string(&target)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    let started = rows
        .iter()
        .find(|r| r["record"]["kind"] == "Started")
        .unwrap();
    assert!(!rows.iter().any(|r| r["record"]["kind"] == "Process"));
    use rustix::{
        event::{PollFd, PollFlags, poll},
        process::{Pid, PidfdFlags, WaitOptions, pidfd_open, waitpid},
    };
    for field in ["launcher", "namespace_init"] {
        let pid = Pid::from_raw(started["record"][field].as_i64().unwrap() as i32).unwrap();
        if let Ok(fd) = pidfd_open(pid, PidfdFlags::empty()) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(7);
            loop {
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                poll(
                    &mut fds,
                    Some(&rustix::event::Timespec {
                        tv_sec: 0,
                        tv_nsec: 10_000_000,
                    }),
                )
                .unwrap();
                if fds[0].revents().contains(PollFlags::IN) {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let status = fs::read_to_string(format!("/proc/{}/status", pid.as_raw_pid()))
                        .unwrap_or_default();
                    let _ = rustix::process::pidfd_send_signal(&fd, rustix::process::Signal::KILL);
                    panic!(
                        "owned {field} {} survived parent loss: {status}",
                        pid.as_raw_pid()
                    );
                }
            }
        }
        match waitpid(Some(pid), WaitOptions::NOHANG) {
            Ok(Some(_)) | Err(rustix::io::Errno::CHILD) => {}
            other => panic!("unreaped owned {field}: {other:?}"),
        }
    }
    let before = (tree(&dirs.project), tree(&dirs.storage));
    let retried = command(&dirs, endpoint, &names, &second, Some(&prior))
        .output()
        .unwrap();
    assert!(!retried.status.success());
    assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
}

fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    fn walk(path: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    walk(path, path, &mut out);
    out
}
#[test]
fn lost_or_failed_generation_charge_creation_result_and_export_never_replay() {
    for (kind, mode) in [
        ("InvocationStarted", "exit"),
        ("Attempt", "exit"),
        ("Charged", "exit"),
        ("Dispatched", "exit"),
        ("file", "exit"),
        ("Created", "error"),
        ("Result", "exit"),
        ("Final", "exit"),
        ("file-export", "exit"),
    ] {
        let dirs = Dirs::new();
        let library = dirs.base.join("fault.so");
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
        let names = ["input.md", "reader.py"];
        let prompt = dirs.input(
            "prompt",
            b"Create a small Markdown file and finish this message.",
        );
        let replies = if kind == "InvocationStarted" || kind == "Attempt" {
            vec![]
        } else if kind == "Final" || kind == "file-export" {
            vec![None]
        } else {
            vec![Some((
                "write",
                json!({"path":names[0],"content":"actual owned bytes"}),
            ))]
        };
        let (endpoint, server) = server(
            replies,
            [
                "Create a small Markdown file and finish this message.".into(),
                "".into(),
            ],
            usize::MAX,
            "actual owned bytes".into(),
            dirs.project.clone(),
            names.map(str::to_owned),
        );
        let target = if kind == "file" {
            dirs.project.join(names[0])
        } else if kind == "file-export" {
            dirs.storage.join("ordinary-run/export-1.json")
        } else {
            dirs.storage.join("ordinary-run/invocation.jsonl")
        };
        let output = command(&dirs, endpoint, &names, &prompt, None)
            .env("LD_PRELOAD", &library)
            .env("SCRAMJET_FAULT_TARGET", &target)
            .env(
                "SCRAMJET_FAULT_KIND",
                if kind.starts_with("file") {
                    "file"
                } else {
                    kind
                },
            )
            .env("SCRAMJET_FAULT_MODE", mode)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{kind}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        server.join().unwrap();
        let before = (tree(&dirs.project), tree(&dirs.storage));
        let retry = command(&dirs, endpoint, &names, &prompt, None)
            .output()
            .unwrap();
        assert!(!retry.status.success());
        assert!(retry.stdout.is_empty());
        assert_eq!(
            (tree(&dirs.project), tree(&dirs.storage)),
            before,
            "{kind}: retry altered effects/accounting"
        );
        let journal =
            fs::read_to_string(dirs.storage.join("ordinary-run/invocation.jsonl")).unwrap();
        let records = journal
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>();
        assert!(!records.iter().any(|r| r["record"]["kind"] == "Export"));
        if kind == "file-export" {
            assert!(target.exists());
        }
        if kind == "file" || kind == "Created" || kind == "Result" {
            assert_eq!(
                fs::read_to_string(dirs.project.join(names[0])).unwrap(),
                "actual owned bytes"
            );
        }
        if kind == "InvocationStarted" {
            assert_eq!(
                fs::read_to_string(
                    dirs.storage
                        .join("ordinary-run/provider/provider-attempts.jsonl")
                )
                .unwrap()
                .lines()
                .count(),
                1
            );
        }
        println!(
            "fault {kind}/{mode}: retained_records={} no_effect_replay=true no_new_reservation=true",
            records.len()
        );
    }
}
