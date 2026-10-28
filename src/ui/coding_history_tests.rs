use super::*;
use crate::ui::{Display, Rect};

pub(crate) fn snapshot(text: &str, maximal: bool) -> Arc<CodingHistory> {
    snapshot_for(text, maximal, "history-ui")
}
fn snapshot_for(text: &str, maximal: bool, session: &str) -> Arc<CodingHistory> {
    let _spawn = super::super::tests::SPAWN_GATE.read().unwrap();
    static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "scramjet-history-ui-test-{}-{}",
        std::process::id(),
        IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    std::fs::write(root.join("sample.txt"), "before\n").unwrap();
    drop(
        scramjet_rs::CodingSession::create(&root, session, scramjet_rs::CodingCapability::ReadOnly)
            .unwrap(),
    );
    let path = root.join(".coding-journal");
    let mut journal = std::fs::read_to_string(&path).unwrap();
    let mut seq = 1;
    let mut add = |turn, request, operation, event, status, text: &str| {
        let line = serde_json::json!({"sequence":seq,"record":{"kind":"Material","entry":{"turn":turn,"request":request,"operation":operation,"event":event,"status":status,"text":text}}}).to_string();
        assert!(line.len() < 16384);
        journal.push_str(&line);
        journal.push('\n');
        seq += 1;
    };
    for turn in 1..=if maximal { 7 } else { 1 } {
        add(turn, 0, 0, "Request", "Pending", text);
        if !maximal {
            break;
        }
        let action = serde_json::json!({"kind":"Read","op":turn,"target":"sample.txt"}).to_string();
        let response = format!("{}{}", action, "\t".repeat(12288 - action.len()));
        for chunk in response.as_bytes().chunks(2048) {
            add(
                turn,
                turn,
                0,
                "ResponsePart",
                "Pending",
                std::str::from_utf8(chunk).unwrap(),
            );
        }
        add(turn, turn, 0, "Response", "Success", "");
        add(turn, turn, turn, "Tool", "Read", &"\u{202e}".repeat(341));
    }
    std::fs::write(path, journal).unwrap();
    let h = scramjet_rs::reconstruct_coding_history(&root, session).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    Arc::new(h)
}
pub(crate) fn exercise(snapshot: &Arc<CodingHistory>) -> (usize, usize) {
    let p = projection(snapshot.clone());
    let mut h = History::default();
    let mut model = Model::runtime();
    model.editor = "x".repeat(super::super::EDITOR_LIMIT);
    model.caret = model.editor.len();
    let mut b = Buffer::empty(Rect::new(0, 0, 200, 60));
    h.observe(&p);
    h.event(&Event::NextChild, &mut model);
    let mut max_document = 0;
    let mut max_frame = 0;
    for _ in 0..63 {
        max_document = max_document.max(h.document.len());
        let cached = h.document.as_ptr();
        for _ in 0..10 {
            let p = p.clone();
            h.observe(&p);
            model.coding_projection(&p);
            let display = model.project(200, 60);
            max_frame = max_frame.max(display.text_bytes);
            b.reset();
            model.paint(&display, &mut b);
            h.paint(&model, &mut b);
            model.displayed(display);
            assert_eq!(cached, h.document.as_ptr());
            assert!(Arc::ptr_eq(snapshot, h.snapshot.as_ref().unwrap()));
        }
        h.event(&Event::CancelChild, &mut model);
    }
    (max_document, max_frame)
}

pub(crate) fn two_history_workload() -> (usize, usize, usize) {
    let first = snapshot_for(&"\x7f".repeat(4096), true, "history-first");
    let second = snapshot_for(&"\x7f".repeat(4096), true, "history-second");
    assert_eq!(first.records().len() + second.records().len(), 126);
    let first_projection = Arc::new(crate::coding_ui_demo::Projection {
        session: Some(Arc::from(first.session())),
        history: Some(first.clone()),
        ended: true,
        ..Default::default()
    });
    let p = crate::coding_ui_demo::Projection {
        session: Some(Arc::from(second.session())),
        history: Some(second.clone()),
        ended: true,
        previous: Some(first_projection.clone()),
        ..Default::default()
    };
    let mut h = History::default();
    let mut model = Model::runtime();
    let mut b = Buffer::empty(Rect::new(0, 0, 200, 60));
    h.observe(&p);
    h.event(&Event::NextChild, &mut model);
    let mut document = 0;
    let mut frame = 0;
    for _ in 0..2 {
        for selected in 0..2 {
            assert_eq!(h.selected, selected);
            for fact in 0..63 {
                assert_eq!(h.fact, fact);
                document = document.max(h.document.len());
                let cached = h.document.as_ptr();
                for _ in 0..3 {
                    h.observe(&p.clone());
                    model.coding_projection(&p);
                    let display = model.project(200, 60);
                    frame = frame.max(display.text_bytes);
                    b.reset();
                    model.paint(&display, &mut b);
                    h.paint(&model, &mut b);
                    model.displayed(display);
                    assert_eq!(h.document.as_ptr(), cached);
                }
                h.event(&Event::CancelChild, &mut model);
            }
            h.event(&Event::HistoryJourney, &mut model);
            assert_eq!((h.fact, h.page), (0, 0));
            assert!(Arc::ptr_eq(&first, h.sources[0].as_ref().unwrap()));
            assert!(Arc::ptr_eq(&second, h.sources[1].as_ref().unwrap()));
            assert!(Arc::ptr_eq(&first_projection, p.previous.as_ref().unwrap()));
        }
    }
    (
        document,
        frame,
        first.payload_bytes() + second.payload_bytes(),
    )
}
#[test]
fn two_keyed_histories_cycle_without_substitution_or_live_input_authority() {
    let first = snapshot_for("FIRST distinct context", false, "history-first");
    let second = snapshot_for("SECOND distinct context", false, "history-second");
    let mut p = crate::coding_ui_demo::Projection {
        session: Some(Arc::from(second.session())),
        previous: Some(Arc::new(crate::coding_ui_demo::Projection {
            session: Some(Arc::from(first.session())),
            history: Some(first.clone()),
            ended: true,
            ..Default::default()
        })),
        ..Default::default()
    };
    let mut h = History::default();
    let mut model = Model::runtime();
    h.observe(&p);
    h.event(&Event::NextChild, &mut model);
    assert!(h.document.contains("FIRST distinct context"));
    h.event(&Event::HistoryJourney, &mut model);
    assert!(h.snapshot.is_none() && h.document.is_empty() && !h.settled);
    for event in [Event::Submit, Event::Approve, Event::Deny, Event::Copy] {
        assert!(h.suppresses(&event));
    }
    p.ended = true;
    p.history_error = Some("corrupt second history");
    h.observe(&p);
    assert!(h.snapshot.is_none() && h.unavailable == p.history_error);
    h.event(&Event::HistoryJourney, &mut model);
    assert!(Arc::ptr_eq(&first, h.snapshot.as_ref().unwrap()));
    h.event(&Event::HistoryJourney, &mut model);
    p.history_error = None;
    p.history = Some(second.clone());
    h.observe(&p);
    assert!(Arc::ptr_eq(&second, h.snapshot.as_ref().unwrap()));
    assert!(
        h.document.contains("SECOND distinct context")
            && !h.document.contains("FIRST distinct context")
    );
    model.focus = Focus::Editor;
    assert!(!h.event(&Event::HistoryJourney, &mut model));
    assert_eq!(h.selected, 1);
}

fn projection(snapshot: Arc<CodingHistory>) -> crate::coding_ui_demo::Projection {
    crate::coding_ui_demo::Projection {
        history: Some(snapshot),
        ended: true,
        ..Default::default()
    }
}
#[test]
fn complete_hostile_pages_navigation_focus_editor_and_non_authority_are_separate() {
    let text = format!("{}\"\\\n\t\x1b\u{202e}界🙂", "\x7f".repeat(4000));
    let p = projection(snapshot(&text, false));
    let mut h = History::default();
    h.observe(&p);
    let mut model = Model::runtime();
    model.editor = "é界🙂".into();
    model.caret = 2;
    model.copied = "stale selection".into();
    assert!(h.event(&Event::NextChild, &mut model));
    assert!(model.copied.is_empty() && model.focus == Focus::Transcript);
    assert!(
        h.document.len() <= DOCUMENT_BYTES
            && h.document
                .contains(&super::super::coding_approval::encode(&text))
    );
    assert!(h.document.bytes().all(|b| (32..=126).contains(&b)));
    let mut assembled = String::new();
    let mut b = Buffer::empty(Rect::new(0, 0, 100, 24));
    for page in 0..h.pages() {
        assert_eq!(h.page, page);
        h.paint(&model, &mut b);
        let start = page * 512;
        let end = (start + 512).min(h.document.len());
        for n in 0..(end - start).div_ceil(64) {
            let count = (end - start - n * 64).min(64);
            for x in 6..6 + count {
                assembled.push_str(b[(x as u16, n as u16 + 2)].symbol());
            }
        }
        h.event(&Event::ApprovalNext, &mut model);
    }
    assert_eq!(assembled, h.document);
    let last = h.page;
    h.event(&Event::ApprovalNext, &mut model);
    assert_eq!(h.page, last);
    for _ in 0..100 {
        h.event(&Event::ApprovalPrevious, &mut model);
        h.event(&Event::HistoryPrevious, &mut model);
    }
    assert_eq!((h.page, h.fact), (0, 0));
    model.event(Event::Tab);
    model.event(Event::Paste("e\u{301}\n".into()));
    assert_eq!(model.editor, "ée\u{301}\n界🙂");
    let editor = model.editor.clone();
    let caret = model.caret;
    model.event(Event::Focus(false));
    assert!(!h.event(&Event::NextChild, &mut model));
    model.event(Event::Focus(true));
    model.event(Event::Tab);
    for (w, height) in [(8, 6), (80, 24), (200, 60)] {
        let display: Display = model.project(w, height);
        assert!(display.text_bytes <= super::super::FRAME_LIMIT);
        b.resize(Rect::new(0, 0, w, height));
        model.paint(&display, &mut b);
        h.paint(&model, &mut b);
        model.displayed(display);
        assert_eq!((h.fact, h.page), (0, 0));
    }
    assert_eq!((model.editor.clone(), model.caret), (editor, caret));
    for event in [
        Event::Approve,
        Event::Deny,
        Event::Submit,
        Event::Copy,
        Event::Mouse {
            button: 0,
            x: 0,
            y: 0,
            release: false,
        },
    ] {
        assert!(h.suppresses(&event));
    }
    assert!(h.event(&Event::NextChild, &mut model));
    assert!(!h.active());
}
#[test]
fn fact_indices_snapshot_identity_and_unavailable_history_are_stable() {
    let snapshot = snapshot(&"\x7f".repeat(4096), true);
    let p = projection(snapshot.clone());
    let mut h = History::default();
    let mut model = Model::runtime();
    h.observe(&p);
    h.event(&Event::NextChild, &mut model);
    for fact in 0..63 {
        assert_eq!(h.fact, fact);
        assert!(h.document.contains(&format!("sequence={}", fact + 1)));
        h.event(&Event::CancelChild, &mut model);
    }
    assert_eq!(h.fact, 62);
    for _ in 0..100 {
        h.event(&Event::CancelChild, &mut model);
        h.observe(&p.clone());
    }
    assert!(Arc::ptr_eq(&snapshot, h.snapshot.as_ref().unwrap()));
    assert_eq!(h.fact, 62);
    model.copied = "stale".into();
    h.event(&Event::HistoryPrevious, &mut model);
    assert!(model.copied.is_empty());
    let mut pending = History::default();
    pending.event(&Event::NextChild, &mut model);
    let mut b = Buffer::empty(Rect::new(0, 0, 100, 24));
    pending.paint(&model, &mut b);
    assert!(
        b.content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .contains("Pending: history unavailable")
    );
    pending.observe(&crate::coding_ui_demo::Projection {
        ended: true,
        history_error: Some("corrupt snapshot"),
        ..Default::default()
    });
    pending.paint(&model, &mut b);
    assert!(
        b.content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .contains("corrupt snapshot")
    );
    assert!(pending.snapshot.is_none());
}
