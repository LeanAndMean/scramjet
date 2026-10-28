use super::*;
use crate::coding::tests::support::OwnedRoot;

#[test]
fn journey_headroom_exhaustion_rejects_before_append_and_poisoned_history_never_retries() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let project = OwnedRoot::new();
    let storage = OwnedRoot::new();
    let workspace = ProjectWorkspace::open(&project.path, &storage.path).unwrap();
    let mut journal = Journal::create(&workspace, "reserved-history", 1).unwrap();
    let original = std::fs::read(storage.path.join(NAME)).unwrap();
    for (records, bytes) in [(18, 0), (1, PROJECT_JOURNAL_BYTES - 98304)] {
        journal.records = records;
        journal.bytes = bytes;
        assert!(journal.reserve_journey().is_err());
        assert!(journal.reservation.is_none());
        assert_eq!(std::fs::read(storage.path.join(NAME)).unwrap(), original);
    }
    journal.records = 1;
    journal.bytes = original.len();
    journal.reserve_journey().unwrap();
    assert!(journal.reserve_journey().is_err());
    journal.release_journey();
    journal.reserve_journey().unwrap();
    journal.poison();
    assert!(journal.reserve_journey().is_err());
    assert!(journal.append(ProjectRecord::Requested(ProjectRequested {
        journey: 1,
        request: ProjectRequest::parse(r#"{"operation":"replace_once","target":"target.txt","supporting":["rules.txt","notes.txt"],"needle":"OLD","replacement":"NEW"}"#).unwrap(),
    })).is_err());
    assert_eq!(std::fs::read(storage.path.join(NAME)).unwrap(), original);
}

#[test]
fn strict_project_journal_rejects_malformed_bounded_fields_sequences_and_exhaustion_without_mutation()
 {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let project = OwnedRoot::new();
    let storage = OwnedRoot::new();
    let workspace = ProjectWorkspace::open(&project.path, &storage.path).unwrap();
    let journal = Journal::create(&workspace, "strict-history", 1).unwrap();
    drop(journal);
    let path = storage.path.join(NAME);
    let original = std::fs::read_to_string(&path).unwrap();
    let header: serde_json::Value = serde_json::from_str(&original).unwrap();
    let mut cases = vec![
        String::new(),
        original.trim_end().into(),
        " ".repeat(PROJECT_JOURNAL_RECORD_BYTES + 1),
        " ".repeat(PROJECT_JOURNAL_BYTES + 1),
    ];
    let mut wrong_sequence = header.clone();
    wrong_sequence["sequence"] = 1.into();
    cases.push(wrong_sequence.to_string() + "\n");
    let mut unknown = header.clone();
    unknown["record"]["Header"]["unknown"] = true.into();
    cases.push(unknown.to_string() + "\n");
    let mut long_session = header.clone();
    long_session["record"]["Header"]["session"] = "s".repeat(65).into();
    cases.push(long_session.to_string() + "\n");
    cases.push(original.replace("\"sequence\":0", "\"sequence\":0,\"sequence\":0"));
    let mut records = String::new();
    for sequence in 0..=PROJECT_JOURNAL_RECORDS {
        let mut row = header.clone();
        row["sequence"] = sequence.into();
        records.push_str(&row.to_string());
        records.push('\n');
    }
    cases.push(records);
    for case in cases {
        std::fs::write(&path, &case).unwrap();
        assert!(read(&workspace.0.storage).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), case);
    }
    std::fs::write(path, original).unwrap();
    assert_eq!(read(&workspace.0.storage).unwrap().len(), 1);
}
