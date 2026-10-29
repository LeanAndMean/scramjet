#[path = "support/coding_invocation.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use support::{Dirs, command, fixture, server};
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
fn eight_attempts_are_shared_by_eight_fresh_processes_and_ninth_is_inert() {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut prefix = Vec::new();
        for index in 0..8 {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            let request = support::body(&mut stream);
            prefix.push(json!({"role":"user","content":format!("A new short message {index}." )}));
            assert_eq!(request["input"], json!(prefix));
            let events = support::events(index, None);
            prefix.extend(
                events.last().unwrap()["response"]["output"]
                    .as_array()
                    .unwrap()
                    .clone(),
            );
            let wire = fixture::wire(&events, true);
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",wire.len()).unwrap();
            stream.write_all(&wire).unwrap();
        }
    });
    let mut prior = None;
    let mut original = None;
    for index in 0..8 {
        let prompt = dirs.input(
            &format!("prompt-{index}"),
            format!("A new short message {index}.").as_bytes(),
        );
        let output = command(&dirs, endpoint, &names, &prompt, prior.as_deref())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let record: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(record["generation"], index + 1);
        assert_eq!(record["counters"]["attempts"], index + 1);
        if let Some(original) = &original {
            assert_eq!(&record["header"], original);
        } else {
            original = Some(record["header"].clone());
        }
        prior = Some(dirs.input(&format!("record-{index}.json"), &output.stdout));
    }
    server.join().unwrap();
    let before = (tree(&dirs.project), tree(&dirs.storage));
    let prompt = dirs.input("ninth", b"No restart can renew the run.");
    let output = command(&dirs, endpoint, &names, &prompt, prior.as_deref())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
    assert_eq!(
        fs::read_to_string(
            dirs.storage
                .join("ordinary-run/provider/provider-attempts.jsonl")
        )
        .unwrap()
        .lines()
        .count(),
        17
    );
}
#[test]
fn full_next_request_refuses_intact_before_generation_or_reservation() {
    let dirs = Dirs::new();
    let names = ["input.md", "reader.py"];
    let prompt = dirs.input("prompt", b"A short message.");
    let (endpoint, server) = server(
        vec![None],
        ["A short message.".into(), "".into()],
        usize::MAX,
        "".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let output = command(&dirs, endpoint, &names, &prompt, None)
        .output()
        .unwrap();
    assert!(output.status.success());
    server.join().unwrap();
    let prior = dirs.input("prior.json", &output.stdout);
    let next = dirs.input("large", &vec![b'x'; 16_384]);
    let before = (tree(&dirs.project), tree(&dirs.storage));
    let refused = command(&dirs, endpoint, &names, &next, Some(&prior))
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("resource limit"));
    assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
}
#[test]
fn boot_contradiction_and_original_expiry_are_not_repaired_or_renewed() {
    for expired in [false, true] {
        let dirs = Dirs::new();
        let names = ["input.md", "reader.py"];
        let prompt = dirs.input("prompt", b"A short message.");
        let (endpoint, server) = server(
            vec![None],
            ["A short message.".into(), "".into()],
            usize::MAX,
            "".into(),
            dirs.project.clone(),
            names.map(str::to_owned),
        );
        let output = command(&dirs, endpoint, &names, &prompt, None)
            .output()
            .unwrap();
        assert!(output.status.success());
        server.join().unwrap();
        let mut record: Value = serde_json::from_slice(&output.stdout).unwrap();
        if expired {
            let start = record["header"]["started_ns"].as_u64().unwrap();
            if start <= 301_000_000_000 {
                continue;
            }
            record["header"]["started_ns"] = json!(start - 301_000_000_000);
            record["header"]["deadline_ns"] = json!(start - 1_000_000_000);
        } else {
            record["header"]["boot"] = json!("foreign boot");
        }
        let journal = dirs.storage.join("ordinary-run/invocation.jsonl");
        let mut rows = fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        rows[0]["record"]["header"] = record["header"].clone();
        let mut bytes = Vec::new();
        for row in rows {
            bytes.extend_from_slice(&serde_json::to_vec(&row).unwrap());
            bytes.push(b'\n');
        }
        fs::write(&journal, bytes).unwrap();
        fs::write(
            dirs.storage.join("ordinary-run/export-1.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let prior = dirs.input("prior.json", &serde_json::to_vec(&record).unwrap());
        let next = dirs.input("next", b"Another prompt.");
        let before = (tree(&dirs.project), tree(&dirs.storage));
        let refused = command(&dirs, endpoint, &names, &next, Some(&prior))
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(refused.stdout.is_empty());
        assert_eq!((tree(&dirs.project), tree(&dirs.storage)), before);
        if expired {
            assert!(String::from_utf8_lossy(&refused.stderr).contains("deadline"));
        }
    }
}
