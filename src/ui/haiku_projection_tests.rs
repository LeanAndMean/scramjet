use super::*;
use scramjet_rs::{
    HaikuAttempt, HaikuHistoryEntry, HaikuOutcome, HaikuPhase, HaikuProcessResult,
    ProviderAttemptFact, ProviderUsage,
};

fn projection() -> crate::haiku_ui_demo::Projection {
    let process = HaikuProcessResult {
        exit_code: None,
        signal: Some(9),
        stdout: "界😀\u{7f}\0".repeat(1024),
        stderr: String::new(),
        stopped: Some("cancelled".into()),
        launcher_reaped: true,
        namespace_dead: true,
    };
    let partial = ProviderUsage {
        input_tokens: Some(10),
        output_tokens: None,
        total_tokens: None,
        cached_input_tokens: None,
        reasoning_tokens: None,
    };
    let attempt = HaikuAttempt {
        turn: 1,
        request_id: 1,
        response_request_id: 1,
        ordinal: Some(1),
        outcome: HaikuOutcome::Cancelled,
        usage: Some(partial),
        terminal: None,
        durable: false,
    };
    crate::haiku_ui_demo::Projection {
        offered: false,
        rejection: None,
        error: Some("complete host error\u{7f}\0".into()),
        runtime_joined: false,
        recovered: false,
        accounting: Some(Arc::new(ProviderHistory {
            attempts: vec![ProviderAttemptFact {
                ordinal: 1,
                request_id: 1,
                terminal: None,
                usage: None,
                locally_joined: false,
            }],
        })),
        snapshot: Arc::new(HaikuSnapshot {
            phase: HaikuPhase::Stopped,
            proposal: None,
            history: Arc::new(HaikuHistory {
                session: "synthetic representation".into(),
                root: [1, 2],
                entries: vec![
                    HaikuHistoryEntry {
                        sequence: 0,
                        acknowledged: true,
                        record: Arc::new(HaikuRecord::Header {
                            version: 3,
                            session: "synthetic representation".into(),
                            root: [1, 2],
                        }),
                    },
                    HaikuHistoryEntry {
                        sequence: 1,
                        acknowledged: false,
                        record: Arc::new(HaikuRecord::Result {
                            call_id: "opaque\0界".into(),
                            output: "😀\u{7f}".repeat(4096),
                        }),
                    },
                ],
            }),
            observed_attempt: Some(Arc::new(attempt)),
            attempt_acknowledged: false,
            observed_process: Some(Arc::new(process)),
            process_acknowledged: false,
            failure: Some("complete failure\u{7f}\0".into()),
            request_id: Some(scramjet_rs::RequestId(1)),
            run_ended: false,
            worker_terminated: false,
            worker_joined: false,
        }),
    }
}
#[test]
fn complete_nonjournal_facts_round_trip_without_acknowledgement_or_authority() {
    let mut projection = projection();
    let document = facts_document(&projection).unwrap();
    let value: serde_json::Value = serde_json::from_slice(document.bytes()).unwrap();
    assert_eq!(
        value["observed_process"]["stdout"],
        projection
            .snapshot
            .observed_process
            .as_ref()
            .unwrap()
            .stdout
    );
    assert_eq!(value["observed_process"]["signal"], 9);
    assert_eq!(
        value["host_error"],
        projection.error.as_ref().unwrap().as_str()
    );
    assert_eq!(
        value["failure"],
        projection.snapshot.failure.as_ref().unwrap().as_str()
    );
    assert_eq!(
        value["observed_attempt"]["usage"]["output_tokens"],
        serde_json::Value::Null
    );
    assert_eq!(value["process_acknowledged"], false);
    assert_eq!(value["unknown_usage"], 1);
    assert_eq!(value["unresolved_remote"], 1);
    let mut model = Model::runtime();
    model.focus = Focus::Transcript;
    let mut presentation = Presentation::default();
    presentation.observe(&projection, true);
    presentation.event(&Event::NextChild, &model, None, 200, 60);
    presentation.event(&Event::CancelChild, &model, None, 200, 60);
    assert!(presentation.facts);
    for event in [Event::Approve, Event::Deny, Event::Submit, Event::Follow] {
        assert!(
            presentation
                .event(&event, &model, Some(1), 200, 60)
                .is_none()
        );
    }
    assert!(presentation.ingress().is_none());
    projection.accounting = None;
    presentation.observe(&projection, true);
    let value: serde_json::Value = serde_json::from_slice(
        presentation
            .detail
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .bytes(),
    )
    .unwrap();
    assert_eq!(value["unknown_usage"], serde_json::Value::Null);
    assert_eq!(value["consumed"], serde_json::Value::Null);
}
#[test]
fn complete_request_overlay_never_copies_hidden_summary_cells() {
    let mut projection = projection();
    projection.offered = true;
    let mut model = Model::runtime();
    model.focus = Focus::Editor;
    model.editor = scramjet_rs::haiku_prompt();
    let mut presentation = Presentation::default();
    presentation.observe(&projection, false);
    presentation.project(&projection, &mut model, false);
    let mut buffer = Buffer::empty(super::super::Rect::new(0, 0, 80, 24));
    assert!(presentation.paint_submission(&model, &mut buffer));
    presentation.submission_painted(true);
    assert!(presentation.suppresses(&Event::Copy));
    assert!(presentation.suppresses(&Event::Mouse {
        button: 0,
        x: 1,
        y: 1,
        release: false
    }));
    assert!(!presentation.suppresses(&Event::CancelAll));
    model.focus = Focus::Transcript;
    assert!(!presentation.paint_submission(&model, &mut buffer));
    presentation.submission_painted(false);
    assert!(!presentation.suppresses(&Event::Copy));
}

#[test]
fn selected_immutable_record_keeps_its_page_across_snapshot_updates() {
    let mut projection = projection();
    let mut model = Model::runtime();
    model.focus = Focus::Transcript;
    let mut presentation = Presentation::default();
    presentation.observe(&projection, true);
    presentation.event(&Event::NextChild, &model, None, 200, 60);
    presentation.event(&Event::HistoryJourney, &model, None, 200, 60);
    presentation.event(&Event::ApprovalNext, &model, None, 200, 60);
    assert_eq!(
        presentation
            .detail
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .page(),
        1
    );
    let history = Arc::new((*projection.snapshot.history).clone());
    Arc::make_mut(&mut projection.snapshot).history = history;
    presentation.observe(&projection, true);
    assert_eq!(
        presentation
            .detail
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .page(),
        1
    );
    let history = Arc::make_mut(&mut Arc::make_mut(&mut projection.snapshot).history);
    history.entries[1].acknowledged = true;
    presentation.observe(&projection, true);
    assert_eq!(
        presentation
            .detail
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .page(),
        0
    );
}
