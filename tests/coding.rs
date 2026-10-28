#[path = "support/coding.rs"]
mod support;
use scramjet_rs::{
    CodingCapability, CodingError, CodingSession, EditCancellation, OperationState,
    reconstruct_coding,
};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use support::OwnedRoot;

fn session(root: &OwnedRoot) -> CodingSession {
    CodingSession::create(
        &root.path,
        "synthetic-session",
        CodingCapability::ControlledEdit,
    )
    .unwrap()
}

#[test]
fn exact_approval_success_and_stale_or_mismatched_authority() {
    let root = OwnedRoot::new();
    let mut session = session(&root);
    let cancel = EditCancellation::default();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let stale = session.approve_exact(&intent).unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    let mut mismatch = intent.clone();
    mismatch.replacement = "unapproved\n".into();
    assert!(matches!(
        session.authorize(mismatch, approval, &cancel),
        Err(CodingError::Stale)
    ));
    let approval = session.approve_exact(&intent).unwrap();
    session
        .authorize(intent.clone(), approval, &cancel)
        .unwrap();
    assert!(reconstruct_coding(&root.path, "synthetic-session").is_err());
    assert_eq!(
        session.dispatch(&cancel).unwrap().state,
        OperationState::Completed
    );
    assert_eq!(session.read("sample.txt").unwrap(), "after\n");
    assert!(matches!(
        session.authorize(intent, stale, &cancel),
        Err(CodingError::Stale)
    ));
    assert!(matches!(session.dispatch(&cancel), Err(CodingError::Stale)));
    drop(session);
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::Completed
    );
}

#[test]
fn denied_missing_foreign_and_cancelled_approval_have_no_effect() {
    let root = OwnedRoot::new();
    let read_only =
        CodingSession::create(&root.path, "read-only", CodingCapability::ReadOnly).unwrap();
    let intent = read_only
        .intent("sample.txt", "before\n", "after\n")
        .unwrap();
    assert!(matches!(
        read_only.approve_exact(&intent),
        Err(CodingError::Denied)
    ));
    assert_eq!(read_only.read("sample.txt").unwrap(), "before\n");
    drop(read_only);
    let root = OwnedRoot::new();
    let other = OwnedRoot::new();
    let mut owner = session(&root);
    let foreign = session(&other);
    let foreign_intent = foreign.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = foreign.approve_exact(&foreign_intent).unwrap();
    let intent = owner.intent("sample.txt", "before\n", "after\n").unwrap();
    assert!(matches!(
        owner.authorize(intent.clone(), approval, &EditCancellation::default()),
        Err(CodingError::Stale)
    ));
    assert!(matches!(
        owner.dispatch(&EditCancellation::default()),
        Err(CodingError::Stale)
    ));
    let cancel = EditCancellation::default();
    cancel.cancel();
    let approval = owner.approve_exact(&intent).unwrap();
    assert!(matches!(
        owner.authorize(intent, approval, &cancel),
        Err(CodingError::Cancelled)
    ));
    assert_eq!(owner.read("sample.txt").unwrap(), "before\n");
    drop(owner);
    assert!(
        reconstruct_coding(&root.path, "synthetic-session")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cancellation_of_durable_authorization_records_known_no_dispatch() {
    let root = OwnedRoot::new();
    let mut session = session(&root);
    let cancel = EditCancellation::default();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    session.authorize(intent, approval, &cancel).unwrap();
    cancel.cancel();
    assert_eq!(
        session.dispatch(&cancel).unwrap().state,
        OperationState::NotDispatched
    );
    assert_eq!(session.read("sample.txt").unwrap(), "before\n");
    drop(session);
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::NotDispatched
    );
}

#[test]
fn exact_target_is_checked_before_authorization_and_again_before_dispatch() {
    let root = OwnedRoot::new();
    let mut session = session(&root);
    let cancel = EditCancellation::default();
    let wrong = session.intent("sample.txt", "wrong", "after\n").unwrap();
    let approval = session.approve_exact(&wrong).unwrap();
    assert!(matches!(
        session.authorize(wrong, approval, &cancel),
        Err(CodingError::TargetMismatch)
    ));
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    session.authorize(intent, approval, &cancel).unwrap();
    fs::write(root.path.join("sample.txt"), "host changed\n").unwrap();
    assert!(matches!(
        session.dispatch(&cancel),
        Err(CodingError::TargetMismatch)
    ));
    drop(session);
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::Authorized
    );
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "host changed\n"
    );
}

#[test]
fn invalid_targets_and_arguments_are_atomic_and_nonblocking() {
    let root = OwnedRoot::new();
    let session = session(&root);
    for name in [
        "",
        ".",
        "..",
        "...",
        "a/b",
        "../sample.txt",
        "a\\b",
        ".coding-journal",
        ".coding-stage",
        "bad\nname",
        "界",
        &"a".repeat(65),
    ] {
        assert!(session.intent(name, "before\n", "after\n").is_err());
        assert!(session.read(name).is_err());
    }
    for content in [
        "x".repeat(1_025),
        "\u{001b}[31m".into(),
        "\0".into(),
        "\r".into(),
    ] {
        assert!(session.intent("sample.txt", &content, "after\n").is_err());
        assert!(session.intent("sample.txt", "before\n", &content).is_err());
    }
    symlink("sample.txt", root.path.join("symlink")).unwrap();
    fs::hard_link(root.path.join("sample.txt"), root.path.join("hardlink")).unwrap();
    assert!(session.read("symlink").is_err());
    assert!(session.read("hardlink").is_err());
    assert!(session.read("sample.txt").is_err());
    fs::create_dir(root.path.join("directory")).unwrap();
    assert!(session.read("directory").is_err());
    rustix::fs::mknodat(
        rustix::fs::CWD,
        root.path.join("fifo"),
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(session.read("fifo").is_err());
    for (name, bytes) in [
        ("oversized", vec![b'x'; 1_025]),
        ("invalid-utf8", vec![0xff]),
        ("control", vec![0x1b]),
    ] {
        fs::write(root.path.join(name), bytes).unwrap();
        assert!(session.read(name).is_err());
    }
    assert_eq!(
        fs::read_to_string(root.path.join("sample.txt")).unwrap(),
        "before\n"
    );
    assert!(!root.path.join(".coding-stage").exists());
}

#[test]
fn root_and_journal_identity_and_cooperative_owner_exclusion() {
    let root = OwnedRoot::new();
    let session = session(&root);
    assert!(CodingSession::create(&root.path, "second", CodingCapability::ControlledEdit).is_err());
    assert!(reconstruct_coding(&root.path, "synthetic-session").is_err());
    drop(session);
    assert!(reconstruct_coding(&root.path, "wrong-session").is_err());
    let other = OwnedRoot::new();
    fs::copy(
        root.path.join(".coding-journal"),
        other.path.join(".coding-journal"),
    )
    .unwrap();
    assert!(reconstruct_coding(&other.path, "synthetic-session").is_err());
    fs::set_permissions(root.path.clone(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(reconstruct_coding(&root.path, "synthetic-session").is_err());
    fs::set_permissions(root.path.clone(), fs::Permissions::from_mode(0o700)).unwrap();
    let alias = OwnedRoot::new();
    symlink(&root.path, alias.path.join("alias")).unwrap();
    assert!(reconstruct_coding(&alias.path.join("alias"), "synthetic-session").is_err());
}

#[test]
fn preexisting_stage_is_not_overwritten_or_cleaned_after_uncertainty() {
    let root = OwnedRoot::new();
    let mut session = session(&root);
    fs::write(root.path.join(".coding-stage"), "retained stage").unwrap();
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    let cancel = EditCancellation::default();
    session.authorize(intent, approval, &cancel).unwrap();
    assert_eq!(
        session.dispatch(&cancel).unwrap().state,
        OperationState::Uncertain
    );
    assert_eq!(
        fs::read_to_string(root.path.join(".coding-stage")).unwrap(),
        "retained stage"
    );
    assert_eq!(session.read("sample.txt").unwrap(), "before\n");
}

#[test]
fn corrupt_truncated_duplicate_out_of_order_and_oversized_history_fails_closed() {
    let root = OwnedRoot::new();
    let mut session = session(&root);
    let intent = session.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = session.approve_exact(&intent).unwrap();
    let cancel = EditCancellation::default();
    session.authorize(intent, approval, &cancel).unwrap();
    session.dispatch(&cancel).unwrap();
    drop(session);
    let path = root.path.join(".coding-journal");
    let original = fs::read_to_string(&path).unwrap();
    let lines: Vec<_> = original.lines().collect();
    let mut cases = vec![
        String::new(),
        original[..original.len() - 1].into(),
        original[..original.len() / 2].into(),
        original.replace("\"version\":1", "\"version\":2"),
        original.replace("\"version\":1", "\"version\":1,\"version\":1"),
        original.replace("\"version\":1", "\"version\":1,\"unknown\":true"),
        original.replace("\"operation\":1", "\"operation\":1,\"operation\":1"),
        original.replace(
            "\"kind\":\"Completed\"",
            "\"kind\":\"Completed\",\"kind\":\"Completed\"",
        ),
        original.replace("\"operation\":1", "\"operation\":2"),
        format!("{}\n{}\n{}\n{}\n", lines[0], lines[2], lines[1], lines[3]),
        format!("{original}{}\n", lines[3]),
        format!("{}\n{}\n", lines[0], "x".repeat(16_385)),
        "x".repeat(1_048_577),
        format!("{original}{{}} trailing\n"),
    ];
    cases.push(format!(
        "{}\n{}\n{}\n{}\n",
        lines[0],
        lines[1],
        lines[2],
        lines[2].replace("\"sequence\":2", "\"sequence\":3")
    ));
    for corrupt in cases {
        fs::write(&path, &corrupt).unwrap();
        assert!(
            reconstruct_coding(&root.path, "synthetic-session").is_err(),
            "accepted corrupt case: {}",
            &corrupt[..corrupt.len().min(100)]
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            "after\n"
        );
    }
    fs::write(&path, original).unwrap();
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::Completed
    );
}

#[test]
fn journal_objects_reject_every_positional_substitution() {
    let root = OwnedRoot::new();
    let mut owner = session(&root);
    let intent = owner.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = owner.approve_exact(&intent).unwrap();
    let cancel = EditCancellation::default();
    owner.authorize(intent, approval, &cancel).unwrap();
    owner.dispatch(&cancel).unwrap();
    drop(owner);
    let path = root.path.join(".coding-journal");
    let original = fs::read_to_string(&path).unwrap();
    let records: Vec<serde_json::Value> = original
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for level in ["envelope", "record", "root", "intent"] {
        let mut changed = records.clone();
        match level {
            "envelope" => changed[3] = serde_json::json!([3, ["Completed", 1]]),
            "record" => changed[3]["record"] = serde_json::json!(["Completed", 1]),
            "root" => {
                let identity = &records[0]["record"]["root"];
                changed[0]["record"]["root"] =
                    serde_json::json!([identity["device"], identity["inode"]]);
            }
            "intent" => {
                let intent = &records[1]["record"]["intent"];
                changed[1]["record"]["intent"] = serde_json::json!([
                    intent["session"],
                    intent["root"],
                    intent["operation"],
                    intent["target"],
                    intent["expected"],
                    intent["replacement"]
                ]);
            }
            _ => unreachable!(),
        }
        let corrupt: String = changed.iter().map(|v| format!("{v}\n")).collect();
        fs::write(&path, &corrupt).unwrap();
        assert!(
            reconstruct_coding(&root.path, "synthetic-session").is_err(),
            "{level}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            "after\n"
        );
    }
    fs::write(&path, &original).unwrap();
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::Completed
    );
}

#[test]
fn malformed_schema_values_have_bounded_decoder_backing_and_never_repair_history() {
    let root = OwnedRoot::new();
    let mut owner = session(&root);
    let intent = owner.intent("sample.txt", "before\n", "after\n").unwrap();
    let approval = owner.approve_exact(&intent).unwrap();
    let cancel = EditCancellation::default();
    owner.authorize(intent, approval, &cancel).unwrap();
    owner.dispatch(&cancel).unwrap();
    drop(owner);
    let path = root.path.join(".coding-journal");
    let original = fs::read_to_string(&path).unwrap();
    let prefix: String = original.lines().take(3).map(|v| format!("{v}\n")).collect();
    let array = format!("[{}0]", "0,".repeat(7_999));
    let map = format!("{{{}\"z\":0}}", "\"x\":0,".repeat(2_500));
    let nested = format!("{}0{}", "[".repeat(512), "]".repeat(512));
    let cases = vec![
        format!(r#"{{"kind":"Completed","operation":1,"unknown":{array}}}"#),
        format!(r#"{{"unknown":{map},"operation":1,"kind":"Completed"}}"#),
        format!(r#"{{"operation":1,"unknown":{nested},"kind":"Completed"}}"#),
        format!(r#"{{"operation":{array},"kind":"Completed"}}"#),
        format!(r#"{{"intent":{map},"kind":"Authorized"}}"#),
        format!(r#"{{"root":{nested},"kind":"Header"}}"#),
        format!(r#"{{"session":{array},"kind":"Header"}}"#),
        format!(r#"{{"operation":1,"operation":{array},"kind":"Completed"}}"#),
        format!(r#"{{"kind":"Completed","kind":{nested},"operation":1}}"#),
        format!(
            r#"{{"kind":"Completed","session":"{}","operation":1}}"#,
            "x".repeat(15_000)
        ),
        format!(
            r#"{{"{}":0,"kind":"Completed","operation":1}}"#,
            "\\u0078".repeat(2_500)
        ),
        format!(
            r#"{{"kind":"Header","session":"{}"}}"#,
            "\\u0078".repeat(2_500)
        ),
        format!(
            r#"{{"{}\t":0,"kind":"Completed","operation":1}}"#,
            "x".repeat(15_000)
        ),
        format!(
            r#"{{"session":"{}\t","kind":"Header"}}"#,
            "x".repeat(15_000)
        ),
        r#"{"operation":1,"version":1,"kind":"Completed"}"#.into(),
        r#"{"operation":1,"kind":"Unknown"}"#.into(),
        r#"{"operation":1}"#.into(),
    ];
    let mut max_peak = 0;
    let mut max_largest = 0;
    for record in cases {
        let corrupt = format!("{prefix}{{\"sequence\":3,\"record\":{record}}}\n");
        assert!(corrupt.lines().all(|line| line.len() < 16_384));
        fs::write(&path, &corrupt).unwrap();
        METER.with(|meter| meter.set(Some((0, 0, 0))));
        let rejected = reconstruct_coding(&root.path, "synthetic-session").is_err();
        let (residual, peak, largest) = METER.with(|meter| meter.replace(None).unwrap());
        assert!(rejected);
        assert_eq!(residual, 0);
        assert!(peak < 98_304, "peak={peak}");
        assert!(largest <= 32_768, "largest={largest}");
        max_peak = max_peak.max(peak);
        max_largest = max_largest.max(largest);
        assert_eq!(fs::read_to_string(&path).unwrap(), corrupt);
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            "after\n"
        );
    }
    println!("coding malformed decoder: peak={max_peak} largest={max_largest} residual=0");
    let tag_last = original.replace(
        r#"{"kind":"Completed","operation":1}"#,
        r#"{"operation":1,"kind":"Completed"}"#,
    );
    assert_ne!(tag_last, original);
    fs::write(&path, &tag_last).unwrap();
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::Completed
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), tag_last);
    fs::write(&path, original).unwrap();
    assert_eq!(
        reconstruct_coding(&root.path, "synthetic-session").unwrap()[0].state,
        OperationState::Completed
    );
}

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static METER: Cell<Option<(usize, usize, usize)>> = const { Cell::new(None) };
}
struct Metered;
fn account(add: usize, remove: usize) {
    let _ = METER.try_with(|meter| {
        if let Some((live, peak, largest)) = meter.get() {
            let overlap = live + add;
            meter.set(Some((
                overlap.saturating_sub(remove),
                peak.max(overlap),
                largest.max(add),
            )));
        }
    });
}
unsafe impl GlobalAlloc for Metered {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let result = unsafe { System.alloc(layout) };
        if !result.is_null() {
            account(layout.size(), 0);
        }
        result
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(0, layout.size());
        unsafe { System.dealloc(pointer, layout) };
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, bytes) };
        if !result.is_null() {
            account(bytes, layout.size());
        }
        result
    }
}
#[global_allocator]
static ALLOCATOR: Metered = Metered;

#[test]
fn maximum_escaping_eight_operation_history_has_bounded_backing_and_work() {
    let root = OwnedRoot::new();
    let payload = "\t".repeat(1_024);
    fs::write(root.path.join("sample.txt"), &payload).unwrap();
    METER.with(|meter| meter.set(Some((0, 0, 0))));
    {
        let mut session = session(&root);
        let cancel = EditCancellation::default();
        for _ in 0..8 {
            let intent = session.intent("sample.txt", &payload, &payload).unwrap();
            let approval = session.approve_exact(&intent).unwrap();
            session.authorize(intent, approval, &cancel).unwrap();
            assert_eq!(
                session.dispatch(&cancel).unwrap().state,
                OperationState::Completed
            );
        }
        assert!(session.intent("sample.txt", &payload, &payload).is_err());
        drop(session);
        for _ in 0..3 {
            let facts = reconstruct_coding(&root.path, "synthetic-session").unwrap();
            assert_eq!(facts.len(), 8);
            assert!(
                facts
                    .iter()
                    .all(|fact| fact.state == OperationState::Completed)
            );
        }
    }
    let (residual, peak, largest) = METER.with(|meter| meter.replace(None).unwrap());
    println!(
        "coding max-escaping/eight-op/three-reconstruction requested allocations: peak={peak} largest={largest} residual={residual}"
    );
    assert!(peak < 131_072);
    assert!(largest <= 16_384);
    assert!(residual < 4_096);
    let journal = fs::read(root.path.join(".coding-journal")).unwrap();
    assert_eq!(journal.iter().filter(|&&b| b == b'\n').count(), 25);
    assert!(journal.len() < 40_000);
}
