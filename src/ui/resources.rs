use super::*;
#[path = "command_resources.rs"]
mod command;
#[path = "haiku_resources.rs"]
mod haiku;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

#[derive(Clone, Copy, Default)]
struct Stats {
    current: isize,
    peak: usize,
    largest: usize,
    total: usize,
}

thread_local! { static STATS: Cell<Option<Stats>> = const { Cell::new(None) }; }

struct Allocator;
static PROCESS_ON: AtomicBool = AtomicBool::new(false);
static PROCESS_CURRENT: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static PROCESS_PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PROCESS_TOTAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PROCESS_LARGEST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn allocate(bytes: usize) {
    if PROCESS_ON.load(Ordering::Relaxed) {
        let current = PROCESS_CURRENT.fetch_add(bytes as isize, Ordering::Relaxed) + bytes as isize;
        PROCESS_PEAK.fetch_max(current.max(0) as usize, Ordering::Relaxed);
        PROCESS_TOTAL.fetch_add(bytes, Ordering::Relaxed);
        PROCESS_LARGEST.fetch_max(bytes, Ordering::Relaxed);
    }
    let _ = STATS.try_with(|stats| {
        if let Some(mut value) = stats.get() {
            value.current += bytes as isize;
            value.total += bytes;
            value.peak = value.peak.max(value.current.max(0) as usize);
            value.largest = value.largest.max(bytes);
            stats.set(Some(value));
        }
    });
}

fn release(bytes: usize) {
    if PROCESS_ON.load(Ordering::Relaxed) {
        PROCESS_CURRENT.fetch_sub(bytes as isize, Ordering::Relaxed);
    }
    let _ = STATS.try_with(|stats| {
        if let Some(mut value) = stats.get() {
            value.current -= bytes as isize;
            stats.set(Some(value));
        }
    });
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocate(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocate(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe {
            System.dealloc(pointer, layout);
        }
        release(layout.size());
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            allocate(size);
            release(layout.size());
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

pub(super) fn start_haiku_meter() {
    assert!(!PROCESS_ON.load(Ordering::Relaxed));
    PROCESS_CURRENT.store(0, Ordering::Relaxed);
    PROCESS_PEAK.store(0, Ordering::Relaxed);
    PROCESS_LARGEST.store(0, Ordering::Relaxed);
    PROCESS_TOTAL.store(0, Ordering::Relaxed);
    STATS.with(|s| s.set(Some(Stats::default())));
    PROCESS_ON.store(true, Ordering::Relaxed);
}
pub(super) fn finish_haiku_meter() {
    PROCESS_ON.store(false, Ordering::Relaxed);
    let ui = STATS.with(|s| s.replace(None).unwrap());
    let peak = PROCESS_PEAK.load(Ordering::Relaxed);
    let largest = PROCESS_LARGEST.load(Ordering::Relaxed);
    let residual = PROCESS_CURRENT.load(Ordering::Relaxed);
    assert!(peak <= 24 * 1024 * 1024, "composed peak={peak}");
    assert!(
        peak <= 16 * 1024 * 1024,
        "whole workload no longer supplies the UI peak upper bound={peak}"
    );
    assert!(ui.largest <= 2 * 1024 * 1024, "UI largest={}", ui.largest);
    assert!(
        residual.abs() <= 2 * 1024 * 1024,
        "retained runtime accounting exceeds the accepted running-headless budget: {residual}"
    );
    println!(
        "HAIKU_APPLICATION_METER peak={peak} largest={largest} residual={residual} ui_largest={} ui_requested_peak={}; all threads, requested incremental heap, not RSS; whole peak bounds UI for this workload",
        ui.largest, ui.peak
    );
}

pub(super) fn project_presentation_probe(p: &crate::project_ui_demo::Projection) {
    let mut maximum = crate::project_ui_demo::Projection {
        snapshot: p.snapshot.clone(),
        offer: None,
        error: None,
        rejection: None,
        history: p.history.clone(),
    };
    maximum.snapshot.journeys[1] = maximum.snapshot.journeys[0].clone();
    for journey in &mut maximum.snapshot.journeys {
        if let Some(readers) = &mut journey.readers {
            for slot in &mut readers.children {
                if let Some(fact) = &mut slot.fact {
                    fact.state = scramjet_rs::ReaderState::Completed {
                        summary: "\"".repeat(512),
                    };
                }
            }
        }
    }
    let mut captured = maximum
        .snapshot
        .records
        .iter()
        .find_map(|r| match r.as_ref() {
            scramjet_rs::ProjectRecord::Captured(c) => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    for file in &mut captured.files {
        file.content = "\n".repeat(1024);
    }
    captured.desired = "\n".repeat(1024);
    maximum.snapshot.records = (0..32)
        .map(|_| Arc::new(scramjet_rs::ProjectRecord::Captured(captured.clone())))
        .collect();
    let mut model = Model::runtime();
    model.editor = "界".repeat(EDITOR_LIMIT / 3);
    model.caret = model.editor.len();
    let mut history = project_projection::History::default();
    let mut buffers = [
        Buffer::empty(Rect::new(0, 0, 200, 60)),
        Buffer::empty(Rect::new(0, 0, 200, 60)),
    ];
    let mut frame = 0;
    STATS.with(|s| s.set(Some(Stats::default())));
    for cycle in 0..200 {
        history.observe(&maximum);
        model.project_projection(&maximum, 0);
        if cycle == 0 {
            history.event(&Event::NextChild, &mut model);
        }
        assert!(model.lines.len() <= 160);
        assert!(model.lines.iter().map(|l| l.text.len()).sum::<usize>() <= 65536);
        let display = model.project(200, 60);
        model.paint(&display, &mut buffers[cycle % 2]);
        history.paint(&model, &mut buffers[cycle % 2]);
        frame = frame.max(
            buffers[cycle % 2]
                .content
                .iter()
                .map(|c| c.symbol().len())
                .sum::<usize>(),
        );
        assert!(history.document_len() <= 196608);
        model.displayed(display);
    }
    let stats = STATS.with(|s| s.replace(None).unwrap());
    assert!(frame <= FRAME_LIMIT);
    assert!(
        stats.largest <= 524288 && stats.peak <= 2097152 && stats.total <= 536870912,
        "{} {} {}",
        stats.largest,
        stats.peak,
        stats.total
    );
    println!(
        "project presentation synthetic maximum: cycles=200 records=32 reader_summaries=6x512 captured_contents=4x1024 editor={} document={} frame={} largest={} peak={} total={}; UI-thread allocations only, not RSS/composed admission",
        model.editor.len(),
        history.document_len(),
        frame,
        stats.largest,
        stats.peak,
        stats.total
    );
}

#[test]
fn same_session_owned_context_history_and_paint_allocation_work_is_bounded() {
    if std::env::var_os("S49_RESOURCE_METER_CHILD").is_none() {
        let child = {
            let _spawn = super::tests::SPAWN_GATE.write().unwrap();
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ui::resources::same_session_owned_context_history_and_paint_allocation_work_is_bounded", "--nocapture", "--test-threads=1"])
                .env("S49_RESOURCE_METER_CHILD", "1")
                .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
                .spawn().unwrap()
        };
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    PROCESS_ON.store(true, Ordering::Relaxed);
    let mut document = 0;
    let mut frame = 0;
    let mut records = 0;
    let mut payload = 0;
    let mut paint_cycles = 0;
    {
        let huge = "x".repeat(1_048_576);
        let mut state = CodingUi::new().unwrap();
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        let mut index = 0;
        let (bridge, errors) =
            crate::coding_ui_demo::coordinate_follow_up(&mut state, |bridge, state| {
                let read_only = state.read_only();
                let model = state.model.get_or_insert_with(Model::runtime);
                let p = bridge.snapshot();
                if read_only {
                    assert!(p.previous.is_none() && p.checkpoint.is_some());
                    let snapshot = p.history.as_ref().unwrap();
                    records = snapshot.records().len();
                    payload = snapshot.payload_bytes();
                    let mut history = coding_history::History::default();
                    model.editor = "x".repeat(EDITOR_LIMIT);
                    model.caret = EDITOR_LIMIT;
                    history.observe(&p);
                    history.event(&Event::NextChild, model);
                    for _ in 0..records {
                        for _ in 0..10 {
                            let copy = p.clone();
                            history.observe(&copy);
                            model.coding_projection(&copy);
                            document = document.max(history.document_len());
                            let display = model.project(200, 60);
                            paint_cycles += 1;
                            frame = frame.max(display.text_bytes);
                            for b in &mut buffers {
                                b.reset();
                                model.paint(&display, b);
                                history.paint(model, b);
                            }
                            model.displayed(display);
                        }
                        history.event(&Event::CancelChild, model);
                    }
                    return Ok(CodingReturn::Exit);
                }
                model.editor = if index == 0 {
                    "x".repeat(1024)
                } else {
                    "second\n🙂".into()
                };
                model.caret = model.editor.len();
                model.focus = Focus::Editor;
                model.revision += 1;
                model.coding_projection(&p);
                let display = model.project(200, 60);
                model.displayed(display);
                let offer = Some(crate::coding_ui_demo::Offer {
                    id: p.offer.unwrap(),
                    revision: model.revision,
                });
                bridge.painted(offer);
                if index == 1 {
                    let before = PROCESS_TOTAL.load(Ordering::Relaxed);
                    for _ in 0..1000 {
                        assert!(!bridge.submit(offer, model.revision, &huge));
                    }
                    assert_eq!(PROCESS_TOTAL.load(Ordering::Relaxed), before);
                }
                assert!(bridge.submit(offer, model.revision, &model.editor));
                let mut approval = coding_approval::Approval::default();
                let end = Instant::now() + Duration::from_secs(4);
                loop {
                    let p = bridge.snapshot();
                    if let Some(o) = &p.observation {
                        approval.observe(if o.decision.is_none() && !o.run_ended {
                            o.proposal.clone()
                        } else {
                            None
                        });
                    }
                    model.coding_projection(&p);
                    let display = model.project(200, 60);
                    paint_cycles += 1;
                    frame = frame.max(display.text_bytes);
                    buffers[0].reset();
                    model.paint(&display, &mut buffers[0]);
                    model.displayed(display);
                    if approval.pending() {
                        model.focus = Focus::Transcript;
                        document = document.max(approval.document_len());
                        let painted = approval.paint(model, &mut buffers[0]);
                        approval.painted(painted);
                        if let Some((proposal, decision)) = approval.event(
                            &Event::Approve,
                            model,
                            &approval.displayed.clone(),
                            200,
                            60,
                        ) {
                            bridge.decide(&proposal, decision);
                        } else {
                            approval.event(&Event::ApprovalNext, model, &None, 200, 60);
                        }
                    }
                    if p.needs_settlement {
                        index += 1;
                        return Ok(if p.needs_checkpoint {
                            CodingReturn::Checkpoint
                        } else {
                            CodingReturn::Settlement
                        });
                    }
                    assert!(Instant::now() < end);
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .unwrap();
        let p = bridge.snapshot();
        assert!(errors.is_empty() && p.error.is_none(), "{p:?}");
        assert_eq!(p.fixture.as_ref().unwrap().accepts, 8);
        assert_eq!(p.history.as_ref().unwrap().operations().len(), 2);
        for _ in 0..1000 {
            let copy = bridge.snapshot();
            assert!(Arc::ptr_eq(
                copy.history.as_ref().unwrap(),
                p.history.as_ref().unwrap()
            ));
        }
    }
    PROCESS_ON.store(false, Ordering::Relaxed);
    let peak = PROCESS_PEAK.load(Ordering::Relaxed);
    let total = PROCESS_TOTAL.load(Ordering::Relaxed);
    let largest = PROCESS_LARGEST.load(Ordering::Relaxed);
    let residual = PROCESS_CURRENT.load(Ordering::Relaxed);
    assert!(
        peak < 8 * 1024 * 1024
            && total < 8 * 1024 * 1024 + paint_cycles * 3 * 1024 * 1024
            && largest <= 2 * 1024 * 1024,
        "peak={peak} total={total} largest={largest}"
    );
    assert!(residual.abs() < 32768, "residual={residual}");
    assert!(document <= coding_approval::DOCUMENT_LIMIT && frame <= FRAME_LIMIT);
    assert_eq!(records, 39);
    assert!(payload <= 512 * 1024);
    println!(
        "same-session process allocation delta: peak={peak} total_requested={total} largest={largest} residual={residual} paint_cycles={paint_cycles} records={records} payload={payload} document={document} frame={frame}; 1MiB borrowed caller + all owned coding/HTTP/context/history/editor/dual-buffer paint setup+work+joins in isolated child, not RSS or preexisting test-harness heap"
    );
}

#[test]
fn settled_history_selected_documents_shared_snapshots_and_unchanged_paints_are_bounded() {
    let snapshot = coding_history::tests::snapshot(&"\x7f".repeat(4096), true);
    STATS.with(|s| s.set(Some(Stats::default())));
    let (document, frame) = coding_history::tests::exercise(&snapshot);
    let stats = STATS.with(|s| s.replace(None)).unwrap();
    println!(
        "history presentation: records={} payload={} document={} frame={} peak={} largest={} total_requested={}",
        snapshot.records().len(),
        snapshot.payload_bytes(),
        document,
        frame,
        stats.peak,
        stats.largest,
        stats.total
    );
    assert_eq!(snapshot.records().len(), 63);
    assert!(snapshot.payload_bytes() <= scramjet_rs::CODING_HISTORY_PAYLOAD_BYTES);
    assert!(document <= 32768 && frame <= FRAME_LIMIT);
    assert!(stats.peak <= 4_194_304 && stats.largest <= 1_048_576 && stats.total <= 671_088_640);
}

#[test]
fn two_maximal_history_slots_load_select_and_repaint_with_bounded_work() {
    STATS.with(|s| s.set(Some(Stats::default())));
    let (document, frame, payload) = coding_history::tests::two_history_workload();
    let stats = STATS.with(|s| s.replace(None)).unwrap();
    println!(
        "two history slots: envelopes=128 facts=126 payload={payload} document={document} frame={frame} peak={} largest={} total_requested={}",
        stats.peak, stats.largest, stats.total
    );
    assert!(payload <= 2 * scramjet_rs::CODING_HISTORY_PAYLOAD_BYTES);
    assert!(document <= 32768 && frame <= FRAME_LIMIT);
    assert!(stats.peak <= 2_097_152 && stats.largest <= 1_048_576 && stats.total <= 67_108_864);
}

#[test]
fn command_maximum_payload_snapshots_projection_and_paint_backing_are_bounded() {
    STATS.with(|s| s.set(Some(Stats::default())));
    let mut line_bytes = 0;
    let mut line_count = 0;
    let mut scan = 0;
    {
        let projection = crate::command_ui_demo::tests::maximum_projection();
        let mut model = Model::runtime();
        model.editor = "x".repeat(EDITOR_LIMIT);
        model.caret = EDITOR_LIMIT;
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        for cycle in 0..40 {
            let snapshot = projection.clone();
            model.command_projection(&snapshot).unwrap();
            line_bytes = line_bytes.max(model.lines.iter().map(|l| l.text.len()).sum::<usize>());
            line_count = line_count.max(model.lines.len());
            model.clear_selection();
            let (w, h) = if cycle % 2 == 0 { (200, 60) } else { (8, 6) };
            let display = model.project(w, h);
            scan = scan.max(display.scanned);
            assert!(display.text_bytes <= FRAME_LIMIT);
            for buffer in &mut buffers {
                buffer.resize(Rect::new(0, 0, w, h));
                buffer.reset();
                model.paint(&display, buffer);
            }
            model.displayed(display);
        }
    }
    let stats = STATS.with(|s| s.replace(None).unwrap());
    assert!(line_bytes <= 32768 && line_count <= 512 && scan <= 40960);
    assert!(
        stats.peak < 16 * 1024 * 1024
            && stats.largest < 2 * 1024 * 1024
            && stats.current.abs() < 8192,
        "peak={} largest={} residual={}",
        stats.peak,
        stats.largest,
        stats.current
    );
    println!(
        "command projection: cycles=40 read_bytes=4096 line_bytes={line_bytes} lines={line_count} scan={scan} peak_requested={} largest={} residual={}; owned snapshots/paint backing, not RSS or arbitrary clones",
        stats.peak, stats.largest, stats.current
    );
}

#[test]
fn saturated_projection_backing_and_work_are_bounded() {
    saturation(false);
}

#[test]
fn diagnostic_projection_backing_and_work_are_bounded() {
    saturation(true);
}

#[test]
fn coding_observation_approval_pages_clones_and_paint_work_are_bounded() {
    STATS.with(|s| s.set(Some(Stats::default())));
    let mut scan = 0;
    let mut text = 0;
    let document_bytes;
    {
        let proposal = Arc::new(scramjet_rs::CodingProposal {
            owner: u64::MAX,
            invocation: 2,
            request_id: scramjet_rs::RequestId(u64::MAX),
            turn: 8,
            intent: scramjet_rs::EditIntent {
                session: "s".repeat(64),
                root: scramjet_rs::RootIdentity {
                    device: u64::MAX,
                    inode: u64::MAX,
                },
                operation: 8,
                target: "t".repeat(64),
                expected: "\u{34f}".repeat(512),
                replacement: "\u{34f}".repeat(512),
            },
        });
        let observation = Arc::new(scramjet_rs::CodingObservation {
            owner: u64::MAX,
            invocation: 2,
            attempts: std::array::from_fn(|i| scramjet_rs::CodingAttempt {
                recorded: true,
                request_id: Some(scramjet_rs::RequestId(i as u64 + 1)),
                outcome: Some(scramjet_rs::Outcome::Success),
                accounting: Some(scramjet_rs::CodingAccounting {
                    request_id: scramjet_rs::RequestId(i as u64 + 1),
                    unknown_usage: true,
                    unresolved_exposure: true,
                }),
                material: Some("Read"),
                output: Some(Arc::from("x".repeat(1024))),
            }),
            proposal: Some(proposal.clone()),
            decision: None,
            authorized: true,
            dispatched: true,
            effect: Some(scramjet_rs::OperationState::Completed),
            verified: true,
            verified_identity: None,
            final_summary: Some(Arc::from("x".repeat(1024))),
            error: Some(Arc::from("x".repeat(4096))),
            run_ended: false,
            worker_terminated: false,
            worker_joined: false,
        });
        let projection = crate::coding_ui_demo::Projection {
            observation: Some(observation),
            error: Some("x".repeat(4096)),
            ..Default::default()
        };
        let mut approval = coding_approval::Approval::default();
        approval.observe(Some(proposal));
        let mut model = Model::runtime();
        model.focus = Focus::Transcript;
        model.editor = "x".repeat(8192);
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        for _ in 0..80 {
            let snapshot = projection.clone();
            model.coding_projection(&snapshot);
            let display = model.project(200, 60);
            scan = scan.max(display.scanned);
            text = text.max(display.text_bytes);
            for buffer in &mut buffers {
                buffer.reset();
                model.paint(&display, buffer);
                approval.paint(&model, buffer);
            }
            model.displayed(display);
            approval.painted(true);
            approval.event(&Event::ApprovalNext, &model, &None, 200, 60);
            assert!(model.lines.len() <= 80);
            assert!(model.lines.iter().all(|line| line.text.len() <= 1024));
        }
        document_bytes = approval.document_len();
    }
    let stats = STATS.with(|s| s.replace(None).unwrap());
    assert!(
        document_bytes <= coding_approval::DOCUMENT_LIMIT && scan <= 40960 && text <= FRAME_LIMIT
    );
    assert!(
        stats.peak < 16 * 1024 * 1024
            && stats.largest < 2 * 1024 * 1024
            && stats.current.abs() < 8192,
        "peak={} largest={} residual={}",
        stats.peak,
        stats.largest,
        stats.current
    );
    println!(
        "coding projection: cycles=80 attempts=8 doc_bytes={document_bytes} scan={scan} paint_text={text} peak_requested={} largest={} residual={}; bounded owned snapshots, not RSS or caller-held clones",
        stats.peak, stats.largest, stats.current
    );
}

#[test]
fn submission_borrowed_rejection_and_snapshot_backing_are_bounded() {
    let (bridge, _) = crate::coding_ui_demo::Bridge::new();
    let offer = Some(crate::coding_ui_demo::Offer {
        id: bridge.snapshot().offer.unwrap(),
        revision: 0,
    });
    bridge.painted(offer);
    let huge = "x".repeat(1_048_576);
    STATS.with(|s| s.set(Some(Stats::default())));
    for _ in 0..1000 {
        assert!(!bridge.submit(offer, 0, &huge));
        assert!(scramjet_rs::CodingAgent::validate_prompt("s", &huge).is_err());
        assert!(bridge.snapshot().submission.is_none());
    }
    let rejected = STATS.with(|s| s.replace(None).unwrap());
    assert_eq!(rejected.peak, 0);
    assert_eq!(rejected.largest, 0);
    let prompt = "x".repeat(1024);
    STATS.with(|s| s.set(Some(Stats::default())));
    assert!(bridge.submit(offer, 0, &prompt));
    for _ in 0..1000 {
        assert!(bridge.snapshot().submission.is_some());
        assert!(!bridge.submit(offer, 0, &prompt));
    }
    let admitted = STATS.with(|s| s.replace(None).unwrap());
    assert!(admitted.peak < 16384 && admitted.largest <= 4096);
    println!(
        "submission: 1000 borrowed 1MiB rejections peak={} largest={}; one1024-byte latch +1000 snapshots/late rejects peak={} largest={}",
        rejected.peak, rejected.largest, admitted.peak, admitted.largest
    );
}

#[test]
fn integrated_projection_backing_and_work_are_bounded() {
    STATS.with(|stats| stats.set(Some(Stats::default())));
    let mut maximum_scan = 0;
    let mut maximum_text = 0;
    {
        let mut projection = crate::runtime_ui_demo::Projection::default();
        for slot in &mut projection.slots {
            slot.prefix = "\x1b".repeat(4096);
            slot.received = 40960;
            slot.clipped = true;
        }
        let mut model = Model::runtime();
        model.editor = "x".repeat(EDITOR_LIMIT);
        model.caret = EDITOR_LIMIT;
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        for turn in 0..40 {
            let copy = projection.clone();
            model.runtime_projection(&copy, turn > 20);
            assert!(model.lines.len() <= 40);
            assert!(model.lines.iter().all(|line| line.text.len() <= 1024));
            model.clear_selection();
            let (width, height) = if turn % 2 == 0 { (200, 60) } else { (10, 12) };
            let display = model.project(width, height);
            maximum_scan = maximum_scan.max(display.scanned);
            maximum_text = maximum_text.max(display.text_bytes);
            for buffer in &mut buffers {
                buffer.resize(Rect::new(0, 0, width, height));
                buffer.reset();
                model.paint(&display, buffer);
            }
            model.displayed(display);
        }
    }
    let stats = STATS.with(|stats| stats.replace(None).unwrap());
    assert!(maximum_scan <= 40960);
    assert!(maximum_text <= FRAME_LIMIT);
    assert!(stats.peak < 16 * 1024 * 1024, "peak={}", stats.peak);
    assert!(stats.largest < 2 * 1024 * 1024, "largest={}", stats.largest);
    assert!(stats.current.abs() < 8192, "residual={}", stats.current);
    println!(
        "integrated projection: cycles=40 scan_peak={maximum_scan} text_peak={maximum_text} requested_peak={} largest={} residual={}; not RSS",
        stats.peak, stats.largest, stats.current
    );
}

#[test]
fn reader_projection_three_slot_clones_and_safe_paint_are_bounded() {
    STATS.with(|stats| stats.set(Some(Stats::default())));
    let mut scan_peak = 0;
    let mut text_peak = 0;
    {
        let observation = scramjet_rs::ReaderObservation {
            session: "x".repeat(64),
            generation: u64::MAX,
            children: std::array::from_fn(|child| scramjet_rs::ReaderSlot {
                request_id: Some(scramjet_rs::RequestId(child as u64 + 1)),
                received: 2048,
                phase: scramjet_rs::ReaderPhase::Terminal,
                accounting: None,
                acceptance_ordinal: Some(child as u8 + 1),
                fact: Some(scramjet_rs::ReaderFact {
                    child: child as u8,
                    request_id: Some(scramjet_rs::RequestId(child as u64 + 1)),
                    state: scramjet_rs::ReaderState::Completed {
                        summary: "\u{202e}".repeat(170),
                    },
                }),
            }),
            tree_ended: true,
            storage_joined: true,
            runtime_joined: true,
            error: None,
        };
        let projection = crate::reader_ui_demo::Projection {
            observation: Some(observation),
            error: Some("x".repeat(4096)),
            ..Default::default()
        };
        let mut model = Model::runtime();
        model.editor = "x".repeat(EDITOR_LIMIT);
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        for turn in 0..40 {
            let copy = projection.clone();
            model.reader_projection(&copy, (turn % 3) as u8, 7, true);
            assert!(model.lines.len() <= 24);
            assert!(
                model
                    .lines
                    .iter()
                    .all(|line| line.text.len() <= 1024 && !line.text.contains('\u{202e}'))
            );
            let (width, height) = if turn % 2 == 0 { (200, 60) } else { (10, 12) };
            model.clear_selection();
            let display = model.project(width, height);
            scan_peak = scan_peak.max(display.scanned);
            text_peak = text_peak.max(display.text_bytes);
            for buffer in &mut buffers {
                buffer.resize(Rect::new(0, 0, width, height));
                buffer.reset();
                model.paint(&display, buffer);
            }
            model.displayed(display);
        }
    }
    let stats = STATS.with(|stats| stats.replace(None).unwrap());
    assert!(scan_peak <= 40960 && text_peak <= FRAME_LIMIT);
    assert!(stats.peak < 16 * 1024 * 1024, "peak={}", stats.peak);
    assert!(stats.largest < 2 * 1024 * 1024, "largest={}", stats.largest);
    assert!(stats.current.abs() < 8192, "residual={}", stats.current);
    println!(
        "reader projection: cycles=40 scan_peak={scan_peak} text_peak={text_peak} requested_peak={} largest={} residual={}; not RSS",
        stats.peak, stats.largest, stats.current
    );
}

fn saturation(diagnostic_enabled: bool) {
    STATS.with(|stats| stats.set(Some(Stats::default())));
    let mut maximum_scan = 0;
    let mut maximum_text = 0;
    let mut maximum_cells = 0;
    {
        let mut model = Model::new();
        model.lines.clear();
        let cluster = format!("e{}", "\u{301}".repeat(30));
        for _ in 0..256 {
            model.append(&cluster.repeat(16)).unwrap();
        }
        model.editor = "x".repeat(EDITOR_LIMIT);
        model.caret = model.editor.len();
        let mut observer = diagnostic_enabled.then(|| diagnostic::Diagnostic::new(&model));
        let mut report = Vec::with_capacity(if diagnostic_enabled { 10240 } else { 0 });
        let mut buffers = [
            Buffer::empty(Rect::new(0, 0, 200, 60)),
            Buffer::empty(Rect::new(0, 0, 200, 60)),
        ];
        for turn in 0..40 {
            let (width, height) = if turn % 2 == 0 { (200, 60) } else { (10, 12) };
            model.clear_selection();
            let display = model.project(width, height);
            maximum_scan = maximum_scan.max(display.scanned);
            maximum_text = maximum_text.max(display.text_bytes);
            for buffer in &mut buffers {
                buffer.resize(Rect::new(0, 0, width, height));
                buffer.reset();
                model.paint(&display, buffer);
                if let Some(observer) = &observer {
                    observer.paint_footer(&model, &display, buffer);
                }
                maximum_cells = maximum_cells.max(
                    buffer
                        .content
                        .iter()
                        .map(|c| c.symbol().len())
                        .sum::<usize>(),
                );
            }
            model.displayed(display);
            if let Some(observer) = &mut observer {
                observer.displayed(&model);
            }
            for _ in 0..64 {
                dispatch(&mut model, &mut observer, Event::Unknown, &mut io::sink()).unwrap();
            }
            if let Some(observer) = &observer {
                report.clear();
                observer.report(&mut report, "ok", true, true).unwrap();
                assert!(report.len() <= 10240);
            }
            model.selection = Some((
                (0, 0),
                (width - 1, model.display.rows.len().saturating_sub(1) as u16),
            ));
            let selected_frame = model.project(width, height);
            model.displayed(selected_frame);
            let _payload = osc52(&model.copied).unwrap();
            model.tick();
        }
    }
    let stats = STATS.with(|stats| stats.replace(None).unwrap());
    assert!(maximum_scan <= 262144);
    assert!(maximum_text <= FRAME_LIMIT);
    assert!(maximum_cells <= FRAME_LIMIT);
    assert!(stats.peak < 16 * 1024 * 1024, "peak={}", stats.peak);
    assert!(stats.largest < 2 * 1024 * 1024, "largest={}", stats.largest);
    assert!(stats.current.abs() < 8192, "residual={}", stats.current);
    println!(
        "UI saturation diagnostic={diagnostic_enabled}: 40 cycles, scan_peak={maximum_scan} map_text_peak={maximum_text} cell_text_peak={maximum_cells} requested_peak={} largest={} residual={}; not RSS guarantees",
        stats.peak, stats.largest, stats.current
    );
}
