#[path = "support/coding_invocation.rs"]
mod support;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use support::{Dirs, command, server};

fn shim(dirs: &Dirs) -> PathBuf {
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
    library
}
fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(tree(&path));
        } else {
            out.insert(path.clone(), fs::read(path).unwrap());
        }
    }
    out
}
#[test]
fn deterministic_capture_refusal_leaves_same_run_usable_without_repair() {
    for case in [
        "oversized",
        "utf8",
        "mode",
        "directory",
        "fifo",
        "symlink",
        "multilink",
    ] {
        let dirs = Dirs::new();
        let names = ["input.md", "reader.py"];
        let input = dirs.project.join(names[0]);
        match case {
            "directory" => fs::create_dir(&input).unwrap(),
            "fifo" => rustix::fs::mkfifoat(
                rustix::fs::CWD,
                &input,
                rustix::fs::Mode::from_raw_mode(0o600),
            )
            .unwrap(),
            "symlink" => {
                std::os::unix::fs::symlink(dirs.input("synthetic-target", b"input"), &input)
                    .unwrap()
            }
            _ => {
                fs::write(
                    &input,
                    match case {
                        "oversized" => vec![b'x'; 513],
                        "utf8" => vec![255],
                        _ => b"input".to_vec(),
                    },
                )
                .unwrap();
                fs::set_permissions(
                    &input,
                    fs::Permissions::from_mode(if case == "mode" { 0o644 } else { 0o600 }),
                )
                .unwrap();
                if case == "multilink" {
                    fs::hard_link(&input, dirs.base.join("other-link")).unwrap();
                }
            }
        }
        let metadata = fs::symlink_metadata(&input).unwrap();
        let original = if metadata.is_file() {
            Some(fs::read(&input).unwrap())
        } else {
            None
        };
        let prompt = dirs.input("prompt", b"Finish this message without tools.");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let output = command(&dirs, listener.local_addr().unwrap(), &names, &prompt, None)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{case}");
        assert!(output.stdout.is_empty());
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        assert_eq!(fs::read_dir(&dirs.storage).unwrap().count(), 0, "{case}");
        assert!(!dirs.project.join(".coding-run").exists());
        assert!(
            fs::read(dirs.project.join(scramjet_rs::PROJECT_LOCK_NAME))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fs::symlink_metadata(&input).unwrap().permissions().mode(),
            metadata.permissions().mode()
        );
        if let Some(bytes) = original {
            assert_eq!(fs::read(&input).unwrap(), bytes);
        }
        if case == "directory" {
            fs::remove_dir(&input).unwrap();
        } else {
            fs::remove_file(&input).unwrap();
        }
        fs::write(&input, vec![b'x'; 512]).unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o600)).unwrap();
        let (endpoint, worker) = server(
            vec![None],
            ["Finish this message without tools.".into(), "".into()],
            usize::MAX,
            "".into(),
            dirs.project.clone(),
            names.map(str::to_owned),
        );
        let corrected = command(&dirs, endpoint, &names, &prompt, None)
            .output()
            .unwrap();
        assert!(
            corrected.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&corrected.stderr)
        );
        assert_eq!(worker.join().unwrap().len(), 1);
        println!("capture {case}: intact refusal, no initialized run, same ID usable");
    }
}
#[test]
fn observed_late_publication_stop_prevents_copied_export_continuation() {
    for (kind, mode) in [
        ("Export", "cancel"),
        ("Export", "cancel-stop-error"),
        ("artifact", "read-cancel"),
        ("Export", "stdout-partial"),
        ("Export", "full"),
    ] {
        let dirs = Dirs::new();
        let names = ["input.md", "reader.py"];
        let prompt = dirs.input("prompt", b"Finish this message without tools.");
        let (endpoint, worker) = server(
            vec![None],
            ["Finish this message without tools.".into(), "".into()],
            usize::MAX,
            "".into(),
            dirs.project.clone(),
            names.map(str::to_owned),
        );
        let journal = dirs.storage.join("ordinary-run/invocation.jsonl");
        let artifact = dirs.storage.join("ordinary-run/export-1.json");
        let mut invoke = command(&dirs, endpoint, &names, &prompt, None);
        invoke
            .env("LD_PRELOAD", shim(&dirs))
            .env(
                "SCRAMJET_FAULT_TARGET",
                if kind == "artifact" {
                    &artifact
                } else {
                    &journal
                },
            )
            .env("SCRAMJET_FAULT_KIND", kind)
            .env("SCRAMJET_FAULT_MODE", mode);
        if mode == "full" {
            invoke.stdout(Stdio::from(
                fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            ));
        }
        let output = invoke.output().unwrap();
        assert!(!output.status.success(), "{kind}/{mode}");
        if mode == "stdout-partial" {
            assert_eq!(output.stdout.len(), 11);
        }
        if mode == "cancel-stop-error" {
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("publication stop persistence incomplete; no retry")
            );
        }
        assert_eq!(worker.join().unwrap().len(), 1);
        let rows = fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            rows.last().unwrap()["record"]["kind"],
            "Stopped",
            "{kind}/{mode}"
        );
        let prior = artifact
            .exists()
            .then(|| dirs.input("copied", &fs::read(&artifact).unwrap()));
        let before = (tree(&dirs.project), tree(&dirs.storage));
        let retry = command(&dirs, endpoint, &names, &prompt, prior.as_deref())
            .output()
            .unwrap();
        assert!(!retry.status.success());
        assert!(retry.stdout.is_empty());
        assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
        println!("publication {kind}/{mode}: observed stop retained, zero new reservation/replay");
    }
}
#[test]
fn clean_export_producer_loss_keeps_unknown_acknowledgement_and_inert_continuation() {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let prompt = dirs.input("prompt", b"Finish this message without tools.");
    let next = dirs.input("next", b"Finish a different message.");
    let (endpoint, worker) = server(
        vec![None, None],
        [
            "Finish this message without tools.".into(),
            "Finish a different message.".into(),
        ],
        1,
        "".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let output = command(&dirs, endpoint, &names, &prompt, None)
        .env("LD_PRELOAD", shim(&dirs))
        .env(
            "SCRAMJET_FAULT_TARGET",
            dirs.storage.join("ordinary-run/invocation.jsonl"),
        )
        .env("SCRAMJET_FAULT_KIND", "Export")
        .env("SCRAMJET_FAULT_MODE", "exit")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(73));
    let bytes = fs::read(dirs.storage.join("ordinary-run/export-1.json")).unwrap();
    let record: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        record["export_producer_acknowledgement"],
        "unknown_on_recovery"
    );
    let prior = dirs.input("copied", &bytes);
    let successor = command(&dirs, endpoint, &names, &next, Some(&prior))
        .output()
        .unwrap();
    assert!(
        successor.status.success(),
        "{}",
        String::from_utf8_lossy(&successor.stderr)
    );
    assert_eq!(worker.join().unwrap().len(), 2);
}
#[test]
fn initialization_loss_and_eio_preserve_torn_store_without_reset() {
    for (path, kind) in [
        ("provider/provider-attempts.jsonl", "Header"),
        ("lock", "marker"),
        ("invocation.jsonl", "Header"),
        ("anchor", "file"),
    ] {
        for mode in ["exit", "error"] {
            let dirs = Dirs::new();
            let names = ["input.md", "reader.py"];
            let prompt = dirs.input("prompt", b"Finish this message without tools.");
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint = listener.local_addr().unwrap();
            let target = match path {
                "lock" => dirs.project.join(scramjet_rs::PROJECT_LOCK_NAME),
                "anchor" => dirs.project.join(".coding-run"),
                _ => dirs.storage.join("ordinary-run").join(path),
            };
            let output = command(&dirs, endpoint, &names, &prompt, None)
                .env("LD_PRELOAD", shim(&dirs))
                .env("SCRAMJET_FAULT_TARGET", target)
                .env("SCRAMJET_FAULT_KIND", kind)
                .env("SCRAMJET_FAULT_MODE", mode)
                .output()
                .unwrap();
            assert!(!output.status.success(), "{path}/{mode}");
            let before = (tree(&dirs.project), tree(&dirs.storage));
            let retry = command(&dirs, endpoint, &names, &prompt, None)
                .output()
                .unwrap();
            assert!(!retry.status.success());
            assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
            );
            println!("initialization {path}/{mode}: torn facts retained, no dispatch/reset");
        }
    }
}
