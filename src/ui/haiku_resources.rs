use super::*;
use scramjet_rs::{HaikuHistory, HaikuHistoryEntry, HaikuPhase, HaikuRecord, HaikuSnapshot};

#[test]
fn haiku_public_record_documents_navigation_and_maximum_paint_are_bounded() {
    STATS.with(|s| s.set(Some(Stats::default())));
    let mut maximum_frame = 0;
    {
        let large = Arc::new(HaikuRecord::Stopped {
            turn: 8,
            reason: "\u{7f}😀\0".repeat(10000),
        });
        assert!(serde_json::to_vec(large.as_ref()).unwrap().len() < 131072);
        let mut entries = Vec::with_capacity(128);
        for sequence in 0..128 {
            entries.push(HaikuHistoryEntry {
                sequence,
                acknowledged: sequence != 127,
                record: if sequence == 0 {
                    large.clone()
                } else {
                    Arc::new(HaikuRecord::Result {
                        call_id: format!("opaque-{sequence}"),
                        output: "界\0".repeat(128),
                    })
                },
            });
        }
        let history = Arc::new(HaikuHistory {
            session: "synthetic public representation".into(),
            root: [1, 2],
            entries,
        });
        let mut projection = crate::haiku_ui_demo::Projection {
            snapshot: Arc::new(HaikuSnapshot {
                phase: HaikuPhase::Stopped,
                proposal: None,
                history,
                observed_attempt: None,
                attempt_acknowledged: false,
                observed_process: None,
                process_acknowledged: false,
                failure: None,
                request_id: None,
                run_ended: false,
                worker_terminated: false,
                worker_joined: false,
            }),
            offered: false,
            accounting: Some(Arc::new(scramjet_rs::ProviderHistory {
                attempts: (1..=8)
                    .map(|ordinal| scramjet_rs::ProviderAttemptFact {
                        ordinal,
                        request_id: ordinal as u64,
                        terminal: Some(scramjet_rs::ProviderTerminalEvidence {
                            response_id: format!("{ordinal}:{}", "界😀\u{7f}\\\"".repeat(1024)),
                            reported_model: Some("synthetic opaque model".into()),
                            status: "completed".into(),
                        }),
                        usage: Some(scramjet_rs::ProviderUsage {
                            input_tokens: Some(10),
                            output_tokens: Some(5),
                            total_tokens: Some(15),
                            cached_input_tokens: None,
                            reasoning_tokens: None,
                        }),
                        locally_joined: true,
                    })
                    .collect(),
            })),
            recovered: false,
            rejection: None,
            error: None,
            runtime_joined: false,
        };
        let mut model = Model::runtime();
        model.editor = "界".repeat(EDITOR_LIMIT / 3);
        model.caret = model.editor.len();
        model.focus = Focus::Transcript;
        let mut presentation = haiku_projection::Presentation::default();
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        presentation.observe(&projection, true);
        presentation.event(&Event::NextChild, &model, None, 200, 60);
        let concurrent = haiku_document::Document::new(large.as_ref()).unwrap();
        assert!(concurrent.pages() > 64);
        for cycle in 0..200 {
            let painted_snapshot = projection.snapshot.clone();
            Arc::make_mut(&mut projection.snapshot).worker_terminated = cycle % 2 == 0;
            presentation.observe(&projection, true);
            presentation.project(&projection, &mut model, true);
            assert!(model.lines.len() <= 80);
            assert!(model.lines.iter().all(|l| l.text.len() <= 1024));
            assert!(model.lines.iter().map(|l| l.text.len()).sum::<usize>() <= 40960);
            let display = model.project(200, 60);
            let buffer = &mut buffers[cycle % 2];
            model.paint(&display, buffer);
            presentation.paint(&model, buffer);
            maximum_frame = maximum_frame.max(
                buffer
                    .content
                    .iter()
                    .map(|c| c.symbol().len())
                    .sum::<usize>(),
            );
            model.displayed(display);
            for event in [
                Event::ApprovalNext,
                Event::HistoryJourney,
                Event::CancelChild,
                Event::ApprovalPrevious,
            ] {
                assert!(presentation.event(&event, &model, None, 200, 60).is_none());
            }
            assert!(presentation.ingress().is_none());
            assert_eq!(painted_snapshot.history.entries.len(), 128);
        }
    }
    let stats = STATS.with(|s| s.replace(None).unwrap());
    assert!(maximum_frame <= FRAME_LIMIT);
    assert!(stats.peak <= 16 * 1024 * 1024, "peak={}", stats.peak);
    assert!(
        stats.largest <= 2 * 1024 * 1024,
        "largest={}",
        stats.largest
    );
    assert!(stats.current.abs() <= 8192, "residual={}", stats.current);
    println!(
        "haiku public-record UI-thread probe: records=128 accounting=8x10242-byte-opaque-IDs current+painted-snapshots cycles=200 geometry=200x60 peak={} largest={} residual={} frame={}; requested heap, synthetic representation not admitted recovery or whole-host composition",
        stats.peak, stats.largest, stats.current, maximum_frame
    );
}
