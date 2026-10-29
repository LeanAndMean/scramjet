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
fn supported_terminal_delivers_complete_record() {
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
    let invocation = command(&dirs, endpoint, &names, &prompt, None);
    let output = Command::new("python3")
        .args(["-c", r#"import errno, os, pty, subprocess, sys
master, slave = pty.openpty()
child = subprocess.Popen(sys.argv[1:], stdin=subprocess.DEVNULL, stdout=slave, stderr=subprocess.PIPE)
os.close(slave)
while True:
 try:
  data = os.read(master, 4096)
 except OSError as error:
  if error.errno == errno.EIO: break
  raise
 if not data: break
 sys.stdout.buffer.write(data)
os.close(master)
_, errors = child.communicate(timeout=5)
sys.stderr.buffer.write(errors)
sys.exit(child.returncode)
"#])
        .arg(invocation.get_program())
        .args(invocation.get_args())
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        record,
        serde_json::from_slice::<Value>(
            &fs::read(dirs.storage.join("ordinary-run/export-1.json")).unwrap()
        )
        .unwrap()
    );
    assert_eq!(worker.join().unwrap().len(), 1);
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

#[test]
fn saturated_stdout_cancellation_stops_without_reader_drain_and_refuses_successor() {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
    use std::{
        os::unix::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };
    for (kind, shared_stderr) in [
        ("pipe", false),
        ("socket", false),
        ("pipe", true),
        ("socket", true),
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
        let (reader, output): (fs::File, fs::File) = if kind == "pipe" {
            let path = dirs.base.join("output-pipe");
            rustix::fs::mkfifoat(
                rustix::fs::CWD,
                &path,
                rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            )
            .unwrap();
            let reader = fs::OpenOptions::new()
                .read(true)
                .custom_flags(OFlags::NONBLOCK.bits() as i32)
                .open(&path)
                .unwrap();
            let output = fs::OpenOptions::new().write(true).open(path).unwrap();
            (reader, output)
        } else {
            let (reader, output) = std::os::unix::net::UnixStream::pair().unwrap();
            (
                std::os::fd::OwnedFd::from(reader).into(),
                std::os::fd::OwnedFd::from(output).into(),
            )
        };
        let flags = fcntl_getfl(&output).unwrap();
        fcntl_setfl(&output, flags | OFlags::NONBLOCK).unwrap();
        loop {
            match rustix::io::write(&output, &[b'x'; 4096]) {
                Ok(_) => {}
                Err(rustix::io::Errno::AGAIN) => break,
                Err(error) => panic!("{error}"),
            }
        }
        fcntl_setfl(&output, flags).unwrap();
        let mut child = command(&dirs, endpoint, &names, &prompt, None)
            .stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(if shared_stderr {
                Stdio::from(output.try_clone().unwrap())
            } else {
                Stdio::piped()
            })
            .spawn()
            .unwrap();
        let artifact = dirs.storage.join("ordinary-run/export-1.json");
        let journal = dirs.storage.join("ordinary-run/invocation.jsonl");
        let started = Instant::now();
        while !fs::read_to_string(&journal).is_ok_and(|text| {
            text.lines().last().is_some_and(|row| {
                serde_json::from_str::<Value>(row).unwrap()["record"]["kind"] == "Export"
            })
        }) {
            if started.elapsed() > Duration::from_secs(5) {
                child.kill().unwrap();
                panic!("export timeout");
            }
            assert!(child.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(child.try_wait().unwrap().is_none());
        assert!(fcntl_getfl(&output).unwrap().contains(OFlags::NONBLOCK));
        assert!(
            Command::new("kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let stopped = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if stopped.elapsed() > Duration::from_secs(2) {
                child.kill().unwrap();
                panic!("blocked cancellation required drainage");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        if !shared_stderr {
            assert!(String::from_utf8_lossy(&result.stderr).contains("publication failed"));
        }
        assert_eq!(fcntl_getfl(&output).unwrap(), flags);
        assert_eq!(worker.join().unwrap().len(), 1);
        let rows = fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .map(|row| serde_json::from_str::<Value>(row).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.last().unwrap()["record"]["kind"], "Stopped");
        let prior = dirs.input("copied", &fs::read(&artifact).unwrap());
        let before = (tree(&dirs.project), tree(&dirs.storage));
        let retry = command(&dirs, endpoint, &names, &prompt, Some(&prior))
            .output()
            .unwrap();
        assert!(!retry.status.success());
        assert!(retry.stdout.is_empty());
        assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
        drop(reader);
        println!(
            "saturated {kind}, shared stderr={shared_stderr}: SIGTERM stopped in {:?} without drainage; copied export refused without new reservation/replay",
            stopped.elapsed()
        );
    }
}
