use super::*;
#[test]
fn command_reader_aggregate_execution_receipt_projection_and_paint_work_is_bounded() {
    if std::env::var_os("S412_RESOURCE_CHILD").is_none() {
        let child = {
            let _spawn = super::super::tests::SPAWN_GATE.write().unwrap();
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ui::resources::command::command_reader_aggregate_execution_receipt_projection_and_paint_work_is_bounded", "--nocapture", "--test-threads=1"])
                .env("S412_RESOURCE_CHILD", "1").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap()
        };
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        return;
    }
    PROCESS_ON.store(true, Ordering::Relaxed);
    let mut cycles = 0;
    let mut frame = 0;
    {
        let mut model = Model::runtime();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 200, 60));
        let p = crate::command_reader_ui_demo::coordinate_test(|p| {
            if cycles >= 8 {
                return;
            }
            model.command_reader_projection(p, 0, 0, false).unwrap();
            let display = model.project(200, 60);
            frame = frame.max(display.text_bytes);
            buffer.reset();
            model.paint(&display, &mut buffer);
            model.displayed(display);
            cycles += 1;
        });
        assert!(
            p.error.is_none()
                && p.reconstructed
                && p.fixture.is_some()
                && p.retained_root.is_none()
        );
        assert!(p.command.as_ref().unwrap().settled);
        assert_eq!(
            p.command.as_ref().unwrap().invocations[0]
                .as_ref()
                .unwrap()
                .phase,
            scramjet_rs::CommandPhase::Completed
        );
        for _ in 0..80 {
            model.command_reader_projection(&p, 2, 7, true).unwrap();
            let display = model.project(200, 60);
            frame = frame.max(display.text_bytes);
            buffer.reset();
            model.paint(&display, &mut buffer);
            model.displayed(display);
            cycles += 1;
        }
    }
    PROCESS_ON.store(false, Ordering::Relaxed);
    let peak = PROCESS_PEAK.load(Ordering::Relaxed);
    let total = PROCESS_TOTAL.load(Ordering::Relaxed);
    let largest = PROCESS_LARGEST.load(Ordering::Relaxed);
    let residual = PROCESS_CURRENT.load(Ordering::Relaxed);
    assert!(
        peak < 8 * 1024 * 1024
            && total < 32 * 1024 * 1024
            && largest <= 1 << 20
            && residual.abs() < 131072,
        "peak={peak} total={total} largest={largest} residual={residual}"
    );
    assert!(frame <= FRAME_LIMIT);
    println!(
        "command reader aggregate: peak={peak} cumulative={total} largest={largest} residual={residual} cycles={cycles} frame={frame}; parser/admission/three children/logical receipt/snapshots/settlement/reconstruction/projection/paint, not RSS or backend capacity"
    );
}
#[test]
fn command_complete_input_execution_history_and_paint_work_is_bounded() {
    if std::env::var_os("S410_RESOURCE_CHILD").is_none() {
        let child = {
            let _spawn = super::super::tests::SPAWN_GATE.write().unwrap();
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ui::resources::command::command_complete_input_execution_history_and_paint_work_is_bounded", "--nocapture", "--test-threads=1"])
                .env("S410_RESOURCE_CHILD", "1").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap()
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
    let mut cycles = 0;
    let mut records = 0;
    let mut payload = 0;
    {
        let huge = "x".repeat(1 << 20);
        let mut state = CodingUi::new().unwrap();
        let mut model = Model::runtime();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 200, 60));
        let (bridge, errors) =
            crate::coding_ui_demo::coordinate_command(&mut state, |bridge, state| {
                let p = bridge.snapshot();
                if state.read_only() {
                    let h = p.history.as_ref().unwrap();
                    records = h.records().len();
                    payload = h.payload_bytes();
                    for _ in 0..80 {
                        let p = bridge.snapshot();
                        model.coding_projection(&p);
                        let display = model.project(200, 60);
                        frame = frame.max(display.text_bytes);
                        buffer.reset();
                        model.paint(&display, &mut buffer);
                        model.displayed(display);
                        cycles += 1;
                    }
                    return Ok(CodingReturn::Exit);
                }
                let offer = Some(crate::coding_ui_demo::Offer {
                    id: p.offer.unwrap(),
                    revision: 0,
                });
                bridge.painted(offer);
                STATS.with(|stats| stats.set(Some(Stats::default())));
                for _ in 0..1000 {
                    assert!(!bridge.submit(offer, 0, &huge));
                }
                assert_eq!(STATS.with(|stats| stats.replace(None).unwrap()).total, 0);
                let mut input = "\\".repeat(1024);
                while scramjet_rs::CodingAgent::validate_prompt(
                    p.session.as_deref().unwrap(),
                    &input,
                )
                .is_err()
                {
                    input.pop();
                }
                assert!(bridge.submit(offer, 0, &input));
                let mut approval = coding_approval::Approval::default();
                let end = Instant::now() + Duration::from_secs(5);
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
                    frame = frame.max(display.text_bytes);
                    buffer.reset();
                    model.paint(&display, &mut buffer);
                    model.displayed(display);
                    cycles += 1;
                    if approval.pending() {
                        model.focus = Focus::Transcript;
                        document = document.max(approval.document_len());
                        let painted = approval.paint(&model, &mut buffer);
                        approval.painted(painted);
                        if let Some((proposal, decision)) = approval.event(
                            &Event::Approve,
                            &model,
                            &approval.displayed.clone(),
                            200,
                            60,
                        ) {
                            bridge.decide(&proposal, decision);
                        } else {
                            approval.event(&Event::ApprovalNext, &model, &None, 200, 60);
                        }
                    }
                    if p.needs_settlement {
                        return Ok(CodingReturn::Settlement);
                    }
                    assert!(Instant::now() < end);
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .unwrap();
        let p = bridge.snapshot();
        assert!(errors.is_empty() && p.settled(), "{p:?}");
        assert_eq!(p.fixture.as_ref().unwrap().accepts, 4);
        assert_eq!(
            p.command.as_ref().unwrap().invocations[0]
                .as_ref()
                .unwrap()
                .report
                .as_ref()
                .unwrap()
                .completion,
            scramjet_rs::CommandCompletion::Completed
        );
    }
    PROCESS_ON.store(false, Ordering::Relaxed);
    let peak = PROCESS_PEAK.load(Ordering::Relaxed);
    let total = PROCESS_TOTAL.load(Ordering::Relaxed);
    let largest = PROCESS_LARGEST.load(Ordering::Relaxed);
    let residual = PROCESS_CURRENT.load(Ordering::Relaxed);
    assert!(
        peak < 8 * 1024 * 1024
            && total < 32 * 1024 * 1024
            && largest <= 1 << 20
            && residual.abs() < 131072
    );
    assert!(
        document <= coding_approval::DOCUMENT_LIMIT
            && frame <= FRAME_LIMIT
            && records <= 64
            && payload <= scramjet_rs::CODING_HISTORY_PAYLOAD_BYTES
    );
    println!(
        "command aggregate allocation: peak={peak} cumulative={total} largest={largest} residual={residual} cycles={cycles} document={document} frame={frame} records={records} payload={payload}; includes 1MiB borrowed rejection backing, command/coding/HTTP/context/history/paint owners, not RSS or backend capacity"
    );
}
