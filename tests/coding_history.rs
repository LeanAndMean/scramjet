#[path = "support/coding_ui_fixture.rs"]
mod fixture;
#[path = "support/coding.rs"]
mod roots;
use scramjet_rs::*;
use std::{fs, sync::Arc, time::Duration};

#[test]
fn actual_settled_golden_denied_transport_tool_and_nonterminal_histories_are_read_only() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        for mode in 0..5 {
            let root = roots::OwnedRoot::new();
            let mut replies = fixture::golden();
            if mode == 2 { replies[0].status = 500; }
            if mode == 3 { replies[2].text = r#"{"kind":"Verify","op":3,"target":"sample.txt","expected":"wrong"}"#.into(); }
            if mode == 4 {
                replies.truncate(3);
                for turn in 4..=8 { replies.push(fixture::Reply { text: serde_json::json!({"kind":"Read","op":turn,"target":"sample.txt"}).to_string(), done: true, status:200, usage:true }); }
            }
            let f = if mode == 0 { fixture::Fixture::consuming_session(Arc::from(fixture::SESSION)).await } else { fixture::Fixture::replies_session(replies, Arc::from(fixture::SESSION)).await }.unwrap();
            let runtime = Runtime::new(f.address, 1, RequestRate::new(4, Duration::from_millis(1)).unwrap(), SyntheticTokenBudget::new(65536,65536,Duration::from_millis(1)).unwrap(), ConcurrencyMode::Fixed).unwrap();
            let session = CodingSession::create(&root.path, fixture::SESSION, CodingCapability::ControlledEdit).unwrap();
            assert!(reconstruct_coding_history(&root.path, fixture::SESSION).is_err());
            let agent = CodingAgent::start_interactive(session, EditCancellation::default(), "history prompt\n界").unwrap();
            let observer = agent.observer();
            let control = agent.control();
            let run = agent.run(&runtime);
            let decide = async {
                if mode == 2 { return; }
                tokio::time::timeout(Duration::from_secs(3), async {
                    loop {
                        if let Some(p) = observer.snapshot().proposal.as_ref() {
                            control.decide(p, if mode == 1 { CodingDecision::Deny } else { CodingDecision::Approve }).unwrap();
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                }).await.unwrap();
            };
            let (result, ()) = tokio::join!(run, decide);
            assert_eq!(result.is_ok(), mode == 0);
            runtime.shutdown().await;
            let report = f.shutdown().await.unwrap();
            fixture::reconcile(&observer.snapshot(), &report).unwrap();
            let path = root.path.join(".coding-journal");
            let journal = fs::read(&path).unwrap();
            let target = fs::read(root.path.join("sample.txt")).unwrap();
            let mut members: Vec<_> = fs::read_dir(&root.path).unwrap().map(|e| e.unwrap().file_name()).collect();
            members.sort();
            for _ in 0..3 {
                let h = reconstruct_coding_history(&root.path, fixture::SESSION).unwrap();
                assert_eq!(h.operations(), reconstruct_coding(&root.path, fixture::SESSION).unwrap());
                assert_eq!(h.ending(), match mode {0 => CodingHistoryEnding::RecordedFinal, 2 => CodingHistoryEnding::RecordedResponseFailed, 4 => CodingHistoryEnding::Incomplete, _ => CodingHistoryEnding::RecordedToolError});
                assert!(h.payload_bytes() < CODING_HISTORY_PAYLOAD_BYTES);
                assert!(h.records().windows(2).all(|w| w[0].sequence + 1 == w[1].sequence));
                assert!(h.records().iter().any(|r| matches!(&r.entry, CodingHistoryEntry::Material(m) if m.event == "Request" && m.request == 0 && m.text.contains("history prompt"))));
                if mode == 3 { assert_eq!(h.operations().last().unwrap().state, OperationState::Completed); }
                if mode == 0 {
                    assert!(h.records().iter().any(|r| matches!(&r.entry, CodingHistoryEntry::Authorized(i) if i.expected == fixture::BEFORE && i.replacement == "history prompt\n界")));
                }
            }
            assert_eq!(journal, fs::read(&path).unwrap());
            assert_eq!(target, fs::read(root.path.join("sample.txt")).unwrap());
            let mut after: Vec<_> = fs::read_dir(&root.path).unwrap().map(|e| e.unwrap().file_name()).collect(); after.sort();
            assert_eq!(members, after);
            let mut end = 0;
            for line in journal.split_inclusive(|b| *b == b'\n') {
                end += line.len(); fs::write(&path, &journal[..end]).unwrap();
                let h = reconstruct_coding_history(&root.path, fixture::SESSION).unwrap();
                assert_eq!(h.operations(), reconstruct_coding(&root.path, fixture::SESSION).unwrap());
                if end == line.len() { assert!(h.records().is_empty()); assert_eq!(h.ending(), CodingHistoryEnding::Incomplete); }
                if matches!(h.records().last().map(|r| &r.entry), Some(CodingHistoryEntry::Dispatched { .. })) { assert_eq!(h.operations().last().unwrap().state, OperationState::Uncertain); }
                if !h.records().iter().any(|r| matches!(&r.entry, CodingHistoryEntry::Material(m) if m.event == "Final" || m.status == "Error" || m.status == "Failed")) { assert_eq!(h.ending(), CodingHistoryEnding::Incomplete); }
            }
            fs::write(&path, &journal[..journal.len()-1]).unwrap();
            assert!(reconstruct_coding_history(&root.path, fixture::SESSION).is_err());
            assert_eq!(fs::read(&path).unwrap(), journal[..journal.len()-1]);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
}

#[test]
fn same_strict_scanner_rejects_schema_framing_identity_and_correlation_without_repair() {
    let root = roots::OwnedRoot::new();
    let session =
        CodingSession::create(&root.path, fixture::SESSION, CodingCapability::ReadOnly).unwrap();
    drop(session);
    let path = root.path.join(".coding-journal");
    let header = fs::read_to_string(&path).unwrap();
    let h: serde_json::Value = serde_json::from_str(&header).unwrap();
    let request = serde_json::json!({"sequence":1,"record":{"kind":"Material","entry":{"turn":1,"request":0,"operation":0,"event":"Request","status":"Pending","text":"x"}}});
    let valid = format!("{}{}\n", header, request);
    let mut cases = vec![
        String::new(),
        header.trim_end().into(),
        valid.trim_end().into(),
        format!("{}{{}}\n", header),
        valid.replace("\"sequence\":1", "\"sequence\":2"),
        valid.replace("\"request\":0", "\"request\":1"),
        valid.replace("\"turn\":1", "\"turn\":9"),
        valid.replace("\"event\":\"Request\"", "\"event\":\"Final\""),
        valid.replace("\"status\":\"Pending\"", "\"status\":\"Success\""),
        valid.replace(
            "\"text\":\"x\"",
            &format!("\"text\":\"{}\"", "x".repeat(4097)),
        ),
        valid.replace("\"text\":\"x\"", "\"text\":\"x\",\"text\":\"x\""),
        valid.replace("\"text\":\"x\"", "\"text\":\"x\",\"unknown\":0"),
        format!("{}{}\n{}\n", header, request, request),
        "x".repeat(1_048_577),
    ];
    for field in ["version", "session", "root"] {
        let mut bad = h.clone();
        bad["record"][field] = serde_json::json!(999);
        cases.push(format!("{bad}\n"));
    }
    let mut positional = request.clone();
    positional["record"]["entry"] = serde_json::json!([1, 0, 0, "Request", "Pending", "x"]);
    cases.push(format!("{header}{positional}\n"));
    for data in cases {
        fs::write(&path, &data).unwrap();
        assert!(
            reconstruct_coding_history(&root.path, fixture::SESSION).is_err(),
            "accepted {} bytes",
            data.len()
        );
        assert!(reconstruct_coding(&root.path, fixture::SESSION).is_err());
        assert_eq!(fs::read(&path).unwrap(), data.as_bytes());
    }
    fs::remove_file(&path).unwrap();
    assert!(reconstruct_coding_history(&root.path, fixture::SESSION).is_err());
}

#[test]
fn sixty_four_control_heavy_records_retain_complete_payload_and_shared_snapshot() {
    let root = roots::OwnedRoot::new();
    drop(CodingSession::create(&root.path, fixture::SESSION, CodingCapability::ReadOnly).unwrap());
    let path = root.path.join(".coding-journal");
    let mut journal = fs::read_to_string(&path).unwrap();
    let mut sequence = 1;
    let mut add = |entry: serde_json::Value| {
        let line =
            serde_json::json!({"sequence":sequence,"record":{"kind":"Material","entry":entry}})
                .to_string();
        assert!(line.len() < 16384);
        journal.push_str(&line);
        journal.push('\n');
        sequence += 1;
    };
    for turn in 1..=7 {
        add(
            serde_json::json!({"turn":turn,"request":0,"operation":0,"event":"Request","status":"Pending","text":format!("{}{}", "\u{1}".repeat(2000), "x".repeat(2096))}),
        );
        let action = serde_json::json!({"kind":"Read","op":turn,"target":"sample.txt"}).to_string();
        let response = format!("{}{}", action, "\t".repeat(12288 - action.len()));
        for part in response.as_bytes().chunks(2048) {
            add(
                serde_json::json!({"turn":turn,"request":turn,"operation":0,"event":"ResponsePart","status":"Pending","text":std::str::from_utf8(part).unwrap()}),
            );
        }
        add(
            serde_json::json!({"turn":turn,"request":turn,"operation":0,"event":"Response","status":"Success","text":""}),
        );
        add(
            serde_json::json!({"turn":turn,"request":turn,"operation":turn,"event":"Tool","status":"Read","text":"\u{1}".repeat(1024)}),
        );
    }
    assert_eq!(sequence, 64);
    fs::write(&path, &journal).unwrap();
    let h = Arc::new(reconstruct_coding_history(&root.path, fixture::SESSION).unwrap());
    assert_eq!(h.records().len(), 63);
    assert_eq!(h.ending(), CodingHistoryEnding::Incomplete);
    assert!(h.payload_bytes() > 120000 && h.payload_bytes() < CODING_HISTORY_PAYLOAD_BYTES);
    for _ in 0..1000 {
        let shared = h.clone();
        assert!(Arc::ptr_eq(&h, &shared));
    }
    let extra = journal
        .lines()
        .last()
        .unwrap()
        .replace("\"sequence\":63", "\"sequence\":64");
    fs::write(&path, format!("{journal}{extra}\n")).unwrap();
    assert!(reconstruct_coding_history(&root.path, fixture::SESSION).is_err());
    assert_eq!(h.records().len(), 63);
}
