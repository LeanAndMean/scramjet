use super::*;
fn submit(bridge: &Bridge, prompt: &str) {
    let offer = Some(Offer {
        id: bridge.offer,
        revision: 0,
    });
    bridge.painted(offer);
    assert!(bridge.submit(offer, 0, prompt));
}
#[test]
fn sticky_stop_reaches_control_attached_after_submission() {
    let (bridge, _) = Bridge::new();
    submit(&bridge, "never execute");
    bridge.stop();
    let root = std::env::temp_dir().join(format!(
        "scramjet-s46-attach-{}-{}",
        std::process::id(),
        bridge.offer
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    std::fs::write(root.join("sample.txt"), fixture::BEFORE).unwrap();
    let cancel = EditCancellation::default();
    let session =
        CodingSession::create(&root, fixture::SESSION, CodingCapability::ControlledEdit).unwrap();
    let agent = CodingAgent::start_interactive(session, cancel.clone(), "never execute").unwrap();
    bridge.execution().attach(agent.control());
    assert!(cancel.is_cancelled());
    agent.finish().unwrap();
    assert!(
        reconstruct_coding(&root, fixture::SESSION)
            .unwrap()
            .is_empty()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn submission_identity_rejections_and_stop_are_bounded_and_one_shot() {
    let (bridge, _) = Bridge::new();
    let offer = Some(Offer {
        id: bridge.offer,
        revision: 2,
    });
    assert!(!bridge.submit(offer, 2, "draft"));
    bridge.painted(offer);
    for _ in 0..100 {
        for prompt in ["", " \n", "bad\u{1b}", &"x".repeat(8192)] {
            assert!(!bridge.submit(offer, 2, prompt));
            assert!(bridge.snapshot().submission.is_none());
        }
        assert!(!bridge.submit(offer, 3, "changed"));
        assert!(!bridge.submit(
            Some(Offer {
                id: bridge.offer + 1,
                revision: 2
            }),
            2,
            "foreign"
        ));
    }
    assert!(bridge.submit(offer, 2, "exact\n界"));
    let receipt = bridge.snapshot().submission.unwrap();
    assert_eq!(receipt.bytes, "exact\n界".len());
    assert!(!bridge.submit(offer, 2, "other"));
    assert_eq!(
        bridge.submission.lock().unwrap().prompt.as_deref(),
        Some("exact\n界")
    );
    assert_eq!(bridge.snapshot().submission.unwrap().offer, receipt.offer);
    let (stopped, _) = Bridge::new();
    let offer = Some(Offer {
        id: stopped.offer,
        revision: 0,
    });
    stopped.painted(offer);
    stopped.stop();
    assert!(!stopped.submit(offer, 0, "never"));
}

#[tokio::test]
async fn actual_submitted_prompts_are_consumed_and_late_edits_cannot_replace_them() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    let mut prior: Option<(
        Bridge,
        Arc<CodingProposal>,
        Arc<CodingHistory>,
        CodingControl,
    )> = None;
    for prompt in ["first\n界 e\u{301}", "second\n🙂 \\"] {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let (bridge, intent) = Bridge::new();
        let task = tokio::spawn(journey(bridge.execution(), intent, None));
        let offered = Some(Offer {
            id: bridge.offer,
            revision: 0,
        });
        bridge.painted(offered);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            bridge.snapshot().observation.is_none() && bridge.snapshot().retained_root.is_none()
        );
        assert!(!task.is_finished());
        if let Some((old, _, _, _)) = &prior {
            assert_ne!(old.session, bridge.session);
            assert!(!bridge.submit(old.snapshot().submission.map(|r| r.offer), 0, prompt));
            assert!(bridge.snapshot().submission.is_none());
        }
        submit(&bridge, prompt);
        assert!(bridge.snapshot().submission.is_some());
        assert!(!bridge.submit(offered, 1, "late edited draft"));
        bridge.restored();
        let (proposal, control) = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(o) = bridge.snapshot().observation
                    && let Some(proposal) = &o.proposal
                {
                    assert_eq!(proposal.intent.session, bridge.session.as_ref());
                    assert_eq!(proposal.intent.replacement, prompt);
                    if let Some((old, previous, _, control)) = &prior {
                        assert_ne!(previous.owner, proposal.owner);
                        assert_ne!(previous.request_id, proposal.request_id);
                        bridge.decide(previous, CodingDecision::Approve);
                        old.decide(proposal, CodingDecision::Approve);
                        assert!(control.decide(proposal, CodingDecision::Approve).is_err());
                        assert!(bridge.snapshot().observation.unwrap().decision.is_none());
                        assert!(!bridge.snapshot().observation.unwrap().authorized);
                    }
                    assert!(o.attempts[0].recorded && o.attempts[1].recorded);
                    assert!(o.decision.is_none() && !o.authorized && !o.dispatched);
                    let control = bridge.submission.lock().unwrap().control.clone().unwrap();
                    bridge.decide(proposal, CodingDecision::Approve);
                    break (proposal.clone(), control);
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap();
        let p = bridge.snapshot();
        assert!(p.error.is_none(), "{p:?}");
        assert!(p.ended && p.reconstructed && p.retained_root.is_none());
        assert_eq!(p.fixture.as_ref().unwrap().prompt.as_deref(), Some(prompt));
        assert_eq!(p.fixture.as_ref().unwrap().session, bridge.session);
        let history = p.history.as_ref().unwrap();
        assert_eq!(history.session(), bridge.session.as_ref());
        let first = history
            .records()
            .iter()
            .find_map(|r| match &r.entry {
                scramjet_rs::CodingHistoryEntry::Material(m) if m.event == "Request" => Some(m),
                _ => None,
            })
            .unwrap();
        let context: serde_json::Value = serde_json::from_str(&first.text).unwrap();
        assert_eq!(context["session"], bridge.session.as_ref());
        assert_eq!(context["prompt"], prompt);
        assert_eq!(context["turn"], 1);
        assert_eq!(context["previous"], "");
        if let Some((old, _, previous, _)) = &prior {
            assert_ne!(previous.session(), history.session());
            assert!(Arc::ptr_eq(
                previous,
                old.snapshot().history.as_ref().unwrap()
            ));
            assert!(old.snapshot().ended && old.snapshot().reconstructed);
        }
        let retained_history = history.clone();
        let o = p.observation.unwrap();
        let mut foreign_report = p.fixture.as_ref().unwrap().clone();
        foreign_report.session = Arc::from("foreign-session");
        assert!(fixture::reconcile(&o, &foreign_report).is_err());
        assert!(o.verified && o.worker_joined);
        for (slot, receipt) in o.attempts.iter().zip(p.fixture.unwrap().receipts).take(4) {
            let receipt = receipt.unwrap();
            assert!(slot.recorded && receipt.consumed && receipt.joined);
            assert_eq!(slot.request_id, Some(receipt.request_id));
        }
        assert_eq!(p.post.unwrap().synthetic_measured, Some(4));
        prior = Some((bridge, proposal, retained_history, control));
    }
}

#[tokio::test]
async fn finite_coordinator_requires_restored_join_and_new_explicit_intent_for_two_journeys() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    for deny in [None, Some(0), Some(1)] {
        let mut state = ui::CodingUi::new().unwrap();
        let mut intervals = 0;
        let mut old: Option<Arc<CodingProposal>> = None;
        let prompts = ["first\n界 e\u{301}", "second\n🙂 \\"];
        let (bridge, errors) = coordinate(&mut state, |bridge, state| {
            let index = intervals;
            intervals += 1;
            let p = bridge.snapshot();
            if state.read_only() {
                assert_eq!(index, 2);
                assert!(p.settled() && p.previous.as_ref().unwrap().settled());
                assert!(p.offer.is_none());
                assert!(!bridge.submit(None, 0, "third"));
                return Ok(ui::CodingReturn::Exit);
            }
            assert!(index < 2);
            if index == 1 {
                let first = p.previous.as_ref().unwrap();
                assert!(first.settled() && first.previous.is_none());
                assert!(p.observation.is_none() && p.retained_root.is_none());
                assert!(p.offer.is_some());
                assert_eq!(Runtime::process_snapshot().active, 0);
                assert_eq!(Runtime::process_snapshot().pending, 0);
                assert!(!bridge.submit(first.submission.map(|r| r.offer), 0, "old intent"));
            }
            submit(bridge, prompts[index]);
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut decided = false;
            loop {
                let p = bridge.snapshot();
                if let Some(o) = &p.observation {
                    if let Some(proposal) = &o.proposal
                        && !decided
                    {
                        if let Some(old) = &old {
                            bridge.decide(old, CodingDecision::Approve);
                            assert!(bridge.snapshot().observation.unwrap().decision.is_none());
                            assert_ne!(old.owner, proposal.owner);
                            assert_ne!(old.request_id, proposal.request_id);
                            assert_ne!(old.intent.session, proposal.intent.session);
                        }
                        assert_eq!(proposal.intent.replacement, prompts[index]);
                        bridge.decide(
                            proposal,
                            if deny == Some(index) {
                                CodingDecision::Deny
                            } else {
                                CodingDecision::Approve
                            },
                        );
                        old = Some(proposal.clone());
                        decided = true;
                    }
                    if p.needs_settlement {
                        assert!(!p.ended && !o.worker_joined);
                        assert!(p.retained_root.as_ref().unwrap().exists());
                        return Ok(ui::CodingReturn::Settlement);
                    }
                }
                assert!(std::time::Instant::now() < deadline, "{p:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(intervals, 3);
        let p = bridge.snapshot();
        assert!(p.settled(), "{p:?}");
        let first = p.previous.as_ref().unwrap();
        assert_ne!(first.session, p.session);
        for (index, p) in [first.as_ref(), &p].into_iter().enumerate() {
            assert_eq!(
                p.fixture.as_ref().unwrap().prompt.as_deref(),
                Some(prompts[index])
            );
            assert_eq!(
                p.observation.as_ref().unwrap().verified,
                deny != Some(index)
            );
            assert!(
                p.retained_root.is_none()
                    && p.history.as_ref().unwrap().session() == p.session.as_deref().unwrap()
            );
            let first_context = p
                .history
                .as_ref()
                .unwrap()
                .records()
                .iter()
                .find_map(|r| match &r.entry {
                    scramjet_rs::CodingHistoryEntry::Material(m) if m.event == "Request" => Some(m),
                    _ => None,
                })
                .unwrap();
            let context: serde_json::Value = serde_json::from_str(&first_context.text).unwrap();
            assert_eq!(context["previous"], "");
            assert_eq!(context["prompt"], prompts[index]);
            assert_eq!(context["session"], p.session.as_deref().unwrap());
        }
    }
}

#[tokio::test]
async fn settlement_signal_or_invalid_history_prevents_second_execution_and_retains_truthful_facts()
{
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    for corrupt in [false, true] {
        let mut state = ui::CodingUi::new().unwrap();
        let mut intervals = 0;
        let (bridge, errors) = coordinate(&mut state, |bridge, _| {
            intervals += 1;
            submit(bridge, "completed before intermission");
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut approved = false;
            loop {
                let p = bridge.snapshot();
                if let Some(o) = &p.observation {
                    if let Some(proposal) = &o.proposal
                        && !approved
                    {
                        bridge.decide(proposal, CodingDecision::Approve);
                        approved = true;
                    }
                    if p.needs_settlement {
                        assert!(o.verified && !o.worker_joined);
                        if corrupt {
                            std::fs::write(
                                p.retained_root.as_ref().unwrap().join(".coding-journal"),
                                b"invalid synthetic history\n",
                            )
                            .unwrap();
                        } else {
                            bridge
                                .submission
                                .lock()
                                .unwrap()
                                .lifecycle
                                .as_ref()
                                .unwrap()
                                .0
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        return Ok(ui::CodingReturn::Settlement);
                    }
                }
                assert!(std::time::Instant::now() < deadline, "{p:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .unwrap();
        assert!(errors.is_empty());
        assert_eq!(intervals, 1);
        let p = bridge.snapshot();
        assert!(p.previous.is_none() && p.observation.as_ref().unwrap().worker_joined);
        assert_eq!(
            p.observation.as_ref().unwrap().effect,
            Some(OperationState::Completed)
        );
        if corrupt {
            assert!(p.history.is_none() && p.history_error.is_some() && p.settlement_failed);
            let root = p.retained_root.unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join("sample.txt")).unwrap(),
                "completed before intermission"
            );
            std::fs::remove_dir_all(root).unwrap();
        } else {
            assert!(p.settled() && p.retained_root.is_none());
        }
    }
}

#[tokio::test]
async fn prepared_executor_has_no_coding_effect_and_closes_only_after_restoration() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    let (bridge, intent) = Bridge::new();
    bridge.store.lock().unwrap().offer = None;
    let state = ui::CodingUi::new().unwrap();
    state.bind(&bridge);
    let (worker, ready) = spawn_executor(&bridge, intent, true).unwrap();
    assert!(ready.recv_timeout(Duration::from_secs(3)).unwrap());
    let p = bridge.snapshot();
    assert!(
        p.offer.is_some()
            && p.submission.is_none()
            && p.retained_root.is_none()
            && p.observation.is_none()
    );
    bridge.stop();
    std::thread::sleep(Duration::from_millis(20));
    assert!(!worker.is_finished());
    bridge.restored();
    worker.join().unwrap();
    let p = bridge.snapshot();
    assert!(p.ended && p.error.is_none(), "{p:?}");
    assert_eq!(p.fixture.as_ref().unwrap().accepts, 0);
    assert!(p.retained_root.is_none() && p.history.is_none() && p.observation.is_none());
    assert_eq!(
        p.post.unwrap().active
            + p.post.unwrap().pending
            + p.post.unwrap().handles
            + p.post.unwrap().usage_obligations,
        0
    );
}
#[tokio::test]
async fn actual_replacement_gate_refuses_held_runtime_without_retry_or_root_creation() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    let fixture = Fixture::consuming_session(Arc::from("held-owner"))
        .await
        .unwrap();
    let runtime = new_runtime(&fixture).unwrap();
    let (bridge, intent) = Bridge::new();
    bridge.store.lock().unwrap().offer = None;
    let state = ui::CodingUi::new().unwrap();
    state.bind(&bridge);
    let (worker, ready) = spawn_executor(&bridge, intent, true).unwrap();
    assert!(!ready.recv_timeout(Duration::from_secs(3)).unwrap());
    worker.join().unwrap();
    let p = bridge.snapshot();
    assert!(p.ended && p.offer.is_none() && p.observation.is_none() && p.retained_root.is_none());
    assert!(p.error.as_ref().unwrap().contains("no retry"));
    assert_eq!(p.fixture.unwrap().accepts, 0);
    runtime.shutdown().await;
    fixture.shutdown().await.unwrap();
}
#[tokio::test]
async fn protected_execution_panic_keeps_actual_coding_owner_until_restoration() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    let fixture = Fixture::consuming_session(Arc::from("panic-ui"))
        .await
        .unwrap();
    let runtime = new_runtime(&fixture).unwrap();
    let root = std::env::temp_dir().join(format!(
        "scramjet-s48-panic-{}-{}",
        std::process::id(),
        OFFERS.load(std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    std::fs::write(root.join("sample.txt"), fixture::BEFORE).unwrap();
    let session =
        CodingSession::create(&root, "panic-ui", CodingCapability::ControlledEdit).unwrap();
    let mut agent =
        CodingAgent::start_interactive(session, EditCancellation::default(), "synthetic panic")
            .unwrap();
    let observer = agent.observer();
    let mut run = Box::pin(agent.run_unjoined(&runtime));
    let result = protected(async {
        loop {
            if observer.snapshot().proposal.is_some() {
                panic!("synthetic executor poll panic");
            }
            tokio::select! {
                result = &mut run => { panic!("unexpected result: {result:?}"); },
                _ = tokio::time::sleep(Duration::from_millis(1)) => {},
            }
        }
    })
    .await;
    assert!(result.is_err());
    drop(run);
    assert!(!observer.snapshot().worker_joined && !observer.snapshot().worker_terminated);
    assert!(!observer.snapshot().authorized && !observer.snapshot().dispatched);
    agent.finish().unwrap();
    let observation = observer.snapshot();
    assert!(observation.worker_joined && observation.worker_terminated);
    runtime.shutdown().await;
    let report = fixture.shutdown().await.unwrap();
    fixture::reconcile(&observation, &report).unwrap();
    assert!(reconstruct_coding(&root, "panic-ui").unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn enclosing_lifecycle_refuses_submission_at_the_same_bridge_admission_boundary() {
    let (bridge, _) = Bridge::new();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    bridge.bind_lifecycle(
        stop.clone(),
        std::time::Instant::now() + Duration::from_secs(1),
    );
    let offer = Some(Offer {
        id: bridge.offer,
        revision: 0,
    });
    bridge.painted(offer);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(!bridge.submit(offer, 0, "stopped"));
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    bridge.bind_lifecycle(stop, std::time::Instant::now());
    assert!(!bridge.submit(offer, 0, "expired"));
    assert!(bridge.snapshot().submission.is_none());
}

#[tokio::test]
async fn bridge_without_painter_approve_deny_stop_and_closed_input_join_cleanup() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    for mode in 0..6 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let (bridge, intent) = Bridge::new();
        submit(&bridge, "scripted fixture request");
        bridge.restored();
        let execution = bridge.execution();
        let store = execution.store.clone();
        let task = tokio::spawn(journey(execution, intent, Some(fixture::golden())));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if bridge
                    .snapshot()
                    .observation
                    .as_ref()
                    .is_some_and(|o| o.proposal.is_some())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        match mode {
            0 | 1 => {
                let proposal = bridge
                    .snapshot()
                    .observation
                    .unwrap()
                    .proposal
                    .clone()
                    .unwrap();
                bridge.decide(
                    &proposal,
                    if mode == 0 {
                        CodingDecision::Approve
                    } else {
                        CodingDecision::Deny
                    },
                );
            }
            2 => bridge.stop(),
            4 | 5 => {
                let stopped = Arc::new(std::sync::atomic::AtomicBool::new(mode == 4));
                let deadline = if mode == 5 {
                    std::time::Instant::now()
                } else {
                    std::time::Instant::now() + Duration::from_secs(1)
                };
                bridge.bind_lifecycle(stopped, deadline);
                let proposal = bridge
                    .snapshot()
                    .observation
                    .unwrap()
                    .proposal
                    .clone()
                    .unwrap();
                bridge.decide(&proposal, CodingDecision::Approve);
                assert!(!bridge.snapshot().observation.unwrap().authorized);
            }
            _ => {}
        }
        let retained = if mode == 3 {
            drop(bridge);
            None
        } else {
            Some(bridge)
        };
        tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap();
        drop(retained);
        let p = store.lock().unwrap().clone();
        assert!(p.error.is_none(), "{p:?}");
        assert!(p.ended && p.reconstructed && p.fixture.is_some() && p.retained_root.is_none());
        let history = p.history.as_ref().unwrap();
        assert_eq!(
            history.operations().last().map(|f| f.state),
            p.durable_effect
        );
        assert_eq!(
            history.ending(),
            if mode == 0 {
                scramjet_rs::CodingHistoryEnding::RecordedFinal
            } else {
                scramjet_rs::CodingHistoryEnding::RecordedToolError
            }
        );
        let o = p.observation.as_ref().unwrap();
        assert!(o.worker_joined && o.worker_terminated);
        assert_eq!(o.effect == Some(OperationState::Completed), mode == 0);
        assert_eq!(o.verified, mode == 0);
        assert_eq!(
            p.post.unwrap().synthetic_measured,
            Some(if mode == 0 { 4 } else { 2 })
        );
        let mut report = Vec::new();
        write_report(&p, &mut report).unwrap();
        assert!(report.len() < 4096);
    }
}
#[tokio::test]
async fn transport_failure_publishes_before_join_and_waits_for_terminal_restoration() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (bridge, intent) = Bridge::new();
    submit(&bridge, "scripted fixture request");
    let mut replies = fixture::golden();
    replies[0].status = 500;
    let task = tokio::spawn(journey(bridge.execution(), intent, Some(replies)));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if bridge
                .snapshot()
                .observation
                .as_ref()
                .is_some_and(|o| o.error.is_some() && o.run_ended)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let waiting = bridge.snapshot();
    assert!(waiting.observation.as_ref().unwrap().run_ended);
    assert!(!waiting.observation.as_ref().unwrap().worker_joined);
    assert!(waiting.pre.is_none() && waiting.fixture.is_none() && waiting.retained_root.is_some());
    assert!(!task.is_finished());
    bridge.stop();
    bridge.restored();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    let ended = bridge.snapshot();
    assert!(ended.error.is_some() && ended.ended && ended.reconstructed);
    assert!(ended.observation.as_ref().unwrap().worker_joined);
    assert_eq!(
        ended.history.as_ref().unwrap().ending(),
        scramjet_rs::CodingHistoryEnding::RecordedResponseFailed
    );
    ui::check_reentry_boundaries(&ended);
    assert!(ended.pre.is_some() && ended.fixture.is_some() && ended.retained_root.is_none());
}

#[tokio::test]
async fn stop_or_close_before_ready_creates_no_worker_fixture_root_or_runtime() {
    for close in [false, true] {
        let (bridge, intent) = Bridge::new();
        let execution = bridge.execution();
        let store = execution.store.clone();
        if close {
            drop(bridge);
        } else {
            bridge.stop();
        }
        journey(execution, intent, Some(fixture::golden())).await;
        let p = store.lock().unwrap();
        assert!(
            p.ended && p.observation.is_none() && p.fixture.is_none() && p.retained_root.is_none()
        );
    }
}

#[tokio::test]
async fn same_session_coordinator_retains_owner_and_requires_fresh_explicit_authority() {
    let _lock = crate::RUNTIME_TEST_LOCK.lock().await;
    for case in 0..7 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let mut state = ui::CodingUi::new().unwrap();
        let mut intervals = 0;
        let mut previous: Option<(
            Bridge,
            Arc<CodingProposal>,
            CodingControl,
            std::path::PathBuf,
        )> = None;
        let prompts = ["first\n界 e\u{301}", "second\n🙂 \\"];
        let (bridge, errors) = coordinate_follow_up(&mut state, |bridge, state| {
            let index = intervals;
            intervals += 1;
            let p = bridge.snapshot();
            if state.read_only() {
                assert!(p.settled() && p.previous.is_none());
                assert!(p.history.as_ref().unwrap().operations().len() <= 2);
                assert!(p.offer.is_none());
                assert!(!bridge.submit(None, 0, "third"));
                return Ok(ui::CodingReturn::Exit);
            }
            assert!(index < 2);
            if index == 1 {
                let (old, proposal, _, root) = previous.as_ref().unwrap();
                assert_eq!(bridge.session, old.session);
                assert_ne!(bridge.offer, old.offer);
                assert_eq!(p.retained_root.as_ref(), Some(root));
                assert!(p.checkpoint.is_some() && p.history.is_none() && p.previous.is_none());
                assert_eq!(p.observation.as_ref().unwrap().owner, proposal.owner);
                assert!(!p.observation.as_ref().unwrap().worker_joined);
                assert_eq!(
                    std::fs::read_to_string(root.join("sample.txt")).unwrap(),
                    prompts[0]
                );
                assert!(reconstruct_coding_history(root, &bridge.session).is_err());
                assert!(!bridge.submit(old.snapshot().submission.map(|r| r.offer), 0, "stale"));
                old.stop();
                old.decide(proposal, CodingDecision::Deny);
                old.painted(old.snapshot().submission.map(|r| r.offer));
                assert!(!old.submit(old.snapshot().submission.map(|r| r.offer), 0, "stale"));
                if case == 3 {
                    return Ok(ui::CodingReturn::Exit);
                }
                let offered = Some(Offer {
                    id: bridge.offer,
                    revision: 0,
                });
                bridge.painted(offered);
                assert!(!bridge.submit(offered, 0, &"\n".repeat(1024)));
                assert!(
                    bridge.snapshot().submission.is_none() && bridge.snapshot().offer.is_some()
                );
                if case == 5 {
                    bridge
                        .submission
                        .lock()
                        .unwrap()
                        .lifecycle
                        .as_mut()
                        .unwrap()
                        .1 = std::time::Instant::now();
                    assert!(!bridge.submit(offered, 0, prompts[1]));
                    return Ok(ui::CodingReturn::Exit);
                }
                assert!(
                    p.observation.as_ref().unwrap().attempts[4..]
                        .iter()
                        .all(|s| s.request_id.is_none())
                );
            }
            submit(bridge, prompts[index]);
            let end = std::time::Instant::now() + Duration::from_secs(4);
            let mut decided = false;
            loop {
                let p = bridge.snapshot();
                if let Some(o) = &p.observation {
                    if let Some(proposal) = &o.proposal
                        && proposal.invocation == index as u64 + 1
                        && !decided
                    {
                        let root = p.retained_root.clone().unwrap();
                        assert_eq!(proposal.intent.replacement, prompts[index]);
                        assert_eq!(proposal.intent.operation, index as u64 + 1);
                        assert!(!o.authorized && !o.dispatched);
                        assert_eq!(
                            std::fs::read_to_string(root.join("sample.txt")).unwrap(),
                            if index == 0 {
                                fixture::BEFORE
                            } else {
                                prompts[0]
                            }
                        );
                        if index == 1 {
                            let (old, first, control, old_root) = previous.as_ref().unwrap();
                            assert_eq!(first.owner, proposal.owner);
                            assert_eq!(&root, old_root);
                            assert_ne!(first.request_id, proposal.request_id);
                            assert_eq!(proposal.turn, 6);
                            assert_eq!(first.intent.root, proposal.intent.root);
                            old.decide(proposal, CodingDecision::Approve);
                            bridge.decide(first, CodingDecision::Approve);
                            control.stop();
                            assert!(control.decide(proposal, CodingDecision::Approve).is_err());
                            assert!(bridge.snapshot().observation.unwrap().decision.is_none());
                        } else {
                            previous = Some((
                                bridge.clone(),
                                proposal.clone(),
                                bridge.submission.lock().unwrap().control.clone().unwrap(),
                                root,
                            ));
                        }
                        if case == 6 && index == 1 {
                            bridge
                                .submission
                                .lock()
                                .unwrap()
                                .lifecycle
                                .as_mut()
                                .unwrap()
                                .1 = std::time::Instant::now();
                        }
                        bridge.decide(
                            proposal,
                            if (case == 1 && index == 0) || (case == 2 && index == 1) {
                                CodingDecision::Deny
                            } else {
                                CodingDecision::Approve
                            },
                        );
                        decided = true;
                    }
                    if p.needs_settlement {
                        assert!(!o.worker_joined && p.history.is_none() && p.fixture.is_none());
                        if case == 4 {
                            bridge
                                .submission
                                .lock()
                                .unwrap()
                                .lifecycle
                                .as_ref()
                                .unwrap()
                                .0
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        return Ok(if p.needs_checkpoint {
                            ui::CodingReturn::Checkpoint
                        } else {
                            ui::CodingReturn::Settlement
                        });
                    }
                }
                assert!(std::time::Instant::now() < end, "{p:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .unwrap();
        let p = bridge.snapshot();
        assert!(
            errors.is_empty() && p.error.is_none(),
            "case={case} {p:?} {errors:?}"
        );
        assert!(p.ended && p.reconstructed && p.retained_root.is_none());
        assert!(p.observation.as_ref().unwrap().worker_joined);
        assert!(work_clear(&p.post.unwrap()));
        assert!(p.previous.is_none());
        let operations = p.history.as_ref().unwrap().operations();
        assert_eq!(
            operations.len(),
            if case == 0 {
                2
            } else if case == 1 {
                0
            } else {
                1
            }
        );
        if case == 0 {
            assert_eq!(
                p.fixture.as_ref().unwrap().follow_prompt.as_deref(),
                Some(prompts[1])
            );
            assert_eq!(p.fixture.as_ref().unwrap().accepts, 8);
            assert!(
                p.fixture
                    .as_ref()
                    .unwrap()
                    .supervisor_joined
                    .iter()
                    .all(|v| *v)
            );
            assert!(
                operations
                    .iter()
                    .all(|f| f.state == OperationState::Completed)
            );
        }
    }
}

#[tokio::test]
async fn restored_follow_up_checkpoint_is_bounded_and_never_rearms_after_stop() {
    let _lock = crate::RUNTIME_TEST_LOCK.lock().await;
    for timeout in [false, true] {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let (bridge, intent) = Bridge::new();
        bridge.store.lock().unwrap().follow_up = true;
        let state = ui::CodingUi::new().unwrap();
        state.bind(&bridge);
        let mut fixture = Fixture::consuming_follow_up(bridge.session.clone())
            .await
            .unwrap();
        let _gate = fixture.hold_checkpoint();
        let runtime = new_runtime(&fixture).unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let task = tokio::spawn(execute_owned(
            bridge.execution(),
            intent,
            None,
            Some((fixture, runtime)),
            Some((bridge.clone(), send)),
        ));
        submit(&bridge, "verified first\n界");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let p = bridge.snapshot();
                if let Some(o) = &p.observation {
                    if let Some(proposal) = &o.proposal {
                        bridge.decide(proposal, CodingDecision::Approve);
                    }
                    if p.needs_checkpoint {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert!(receive.try_recv().is_err() && !task.is_finished());
        assert!(!bridge.snapshot().observation.unwrap().worker_joined);
        bridge.restored();
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(receive.try_recv().is_err() && !task.is_finished());
        if !timeout {
            bridge.stop();
        }
        tokio::time::timeout(Duration::from_secs(6), task)
            .await
            .unwrap()
            .unwrap();
        let p = bridge.snapshot();
        assert!(p.ended && p.reconstructed && p.retained_root.is_none());
        assert!(p.error.as_ref().unwrap().contains(if timeout {
            "checkpoint timeout"
        } else {
            "lifecycle"
        }));
        assert_eq!(p.fixture.as_ref().unwrap().accepts, 4);
        assert!(
            p.fixture.as_ref().unwrap().supervisor_joined[..4]
                .iter()
                .all(|v| *v)
        );
        assert_eq!(p.history.as_ref().unwrap().operations().len(), 1);
        assert!(work_clear(&p.post.unwrap()));
    }
}

async fn approve_until_boundary(bridge: &Bridge) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let p = bridge.snapshot();
            if let Some(o) = &p.observation
                && o.decision.is_none()
                && !o.run_ended
                && let Some(proposal) = &o.proposal
            {
                bridge.decide(proposal, CodingDecision::Approve);
            }
            if p.needs_settlement || p.ended {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn full_owner_capacity_and_second_response_faults_preserve_first_facts_without_retry() {
    let _lock = crate::RUNTIME_TEST_LOCK.lock().await;
    for case in 0..5 {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let (first, intent) = Bridge::new();
        first.store.lock().unwrap().follow_up = true;
        let state = ui::CodingUi::new().unwrap();
        state.bind(&first);
        let mut reply = fixture::Reply {
            text: "{\"kind\":\"Read\",\"op\":5,\"target\":\"sample.txt\"}".into(),
            done: true,
            status: 200,
            usage: true,
        };
        match case {
            1 => reply.text = "{\"kind\":\"Replace\",\"op\":5}".into(),
            2 => reply.status = 500,
            3 => reply.done = false,
            4 => reply.usage = false,
            _ => {}
        }
        let fixture = Fixture::consuming_follow_up_replies(
            first.session.clone(),
            if case == 0 { vec![] } else { vec![reply] },
        )
        .await
        .unwrap();
        if case == 0 {
            fixture.pad_responses(16384);
        }
        let runtime = new_runtime(&fixture).unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let task = tokio::spawn(supervised_follow_up(
            first.clone(),
            intent,
            send,
            None,
            Some((fixture, runtime)),
        ));
        submit(&first, "first\n界");
        approve_until_boundary(&first).await;
        assert!(
            first.snapshot().needs_checkpoint
                && !first.snapshot().observation.unwrap().worker_joined
        );
        assert!(receive.try_recv().is_err());
        first.restored();
        let active = if case == 0 {
            first.clone()
        } else {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(next) = receive.try_recv() {
                        break next;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap()
        };
        if case != 0 {
            assert!(
                active.snapshot().observation.unwrap().attempts[4]
                    .request_id
                    .is_none()
            );
            submit(&active, "second\n🙂");
            approve_until_boundary(&active).await;
            assert!(!active.snapshot().observation.unwrap().worker_joined);
            active.restored();
            active.stop();
        }
        tokio::time::timeout(Duration::from_secs(4), task)
            .await
            .unwrap()
            .unwrap();
        let p = active.snapshot();
        assert!(
            p.ended && p.reconstructed && p.retained_root.is_none(),
            "case={case} {p:?}"
        );
        assert!(work_clear(&p.post.unwrap()));
        let h = p.history.as_ref().unwrap();
        assert_eq!(h.operations().len(), if case == 4 { 2 } else { 1 });
        assert_eq!(h.operations()[0].state, OperationState::Completed);
        if case == 0 {
            assert!(p.error.as_ref().unwrap().contains("capacity"));
            assert_eq!(h.records().len(), 47);
            assert_eq!(p.fixture.as_ref().unwrap().accepts, 4);
            assert!(receive.try_recv().is_err());
            assert!(
                h.records()
                    .iter()
                    .all(|r| !matches!(r.entry, scramjet_rs::CodingHistoryEntry::FollowUp { .. }))
            );
        } else {
            assert_eq!(
                p.fixture.as_ref().unwrap().accepts,
                if case == 4 { 8 } else { 5 }
            );
            assert_eq!(p.error.is_none(), case == 4);
            let expected = if case == 1 {
                scramjet_rs::CodingHistoryEnding::RecordedToolError
            } else if case == 4 {
                scramjet_rs::CodingHistoryEnding::RecordedFinal
            } else {
                scramjet_rs::CodingHistoryEnding::RecordedResponseFailed
            };
            assert_eq!(h.ending(), expected);
        }
    }
}

#[tokio::test]
async fn post_handoff_executor_panic_publishes_to_current_bridge_before_owned_drop() {
    let _lock = crate::RUNTIME_TEST_LOCK.lock().await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (first, intent) = Bridge::new();
    first.store.lock().unwrap().follow_up = true;
    let state = ui::CodingUi::new().unwrap();
    state.bind(&first);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let task = tokio::spawn(supervised_follow_up(
        first.clone(),
        intent,
        send,
        None,
        None,
    ));
    submit(&first, "first verified");
    approve_until_boundary(&first).await;
    first.restored();
    let next = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(next) = receive.try_recv() {
                break next;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    next.submission.lock().unwrap().fail_after_handoff = true;
    submit(&next, "second never dispatched");
    approve_until_boundary(&next).await;
    assert!(
        next.snapshot()
            .error
            .as_ref()
            .unwrap()
            .contains("phase panicked")
    );
    assert!(first.snapshot().error.is_none());
    assert!(!next.snapshot().observation.unwrap().worker_joined && !task.is_finished());
    let root = next.snapshot().retained_root.unwrap();
    assert!(reconstruct_coding_history(&root, &next.session).is_err());
    assert_eq!(
        std::fs::read_to_string(root.join("sample.txt")).unwrap(),
        "first verified"
    );
    next.restored();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    let p = next.snapshot();
    assert!(p.ended && p.reconstructed && p.retained_root.is_none());
    assert!(p.fixture.is_some() && p.history.is_some());
    assert!(p.observation.as_ref().unwrap().worker_joined);
    assert_eq!(p.history.as_ref().unwrap().operations().len(), 1);
    assert!(work_clear(&p.post.unwrap()));
}

#[tokio::test]
async fn command_coordinator_separates_normal_restoration_report_and_explicit_stop() {
    let _owner = crate::RUNTIME_TEST_LOCK.lock().await;
    for mode in 0..5 {
        let mut state = ui::CodingUi::new().unwrap();
        let mut intervals = 0;
        let input = "entire command input\n界 e\u{301} 🙂 \\";
        let (bridge, errors) = coordinate_command(&mut state, |bridge, state| {
            intervals += 1;
            let p = bridge.snapshot();
            assert!(p.command.is_some() && p.previous.is_none());
            if state.read_only() {
                assert_eq!(intervals, 2);
                assert!(p.settled() && p.history.is_some());
                let command = p.command.as_ref().unwrap();
                assert!(command.settled && command.invocations[1].is_none());
                let fact = command.invocations[0].as_ref().unwrap();
                assert!(fact.report.is_some());
                assert!(!bridge.submit(p.submission.map(|s| s.offer), 0, "replay"));
                return Ok(ui::CodingReturn::Exit);
            }
            assert_eq!(intervals, 1);
            let offer = Some(Offer {
                id: bridge.offer,
                revision: 0,
            });
            bridge.painted(offer);
            for invalid in [
                "",
                " \n",
                "bad\t",
                &"x".repeat(8192),
                &format!("x{}", "\n".repeat(1023)),
            ] {
                assert!(!bridge.submit(offer, 0, invalid));
                assert!(bridge.snapshot().submission.is_none());
                assert_eq!(bridge.submission.lock().unwrap().painted, offer);
                assert!(bridge.snapshot().command.unwrap().invocations[0].is_none());
            }
            submit(bridge, input);
            assert!(!bridge.submit(offer, 0, "late draft"));
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut decided = false;
            loop {
                let p = bridge.snapshot();
                if let Some(o) = &p.observation {
                    if let Some(proposal) = &o.proposal
                        && !decided
                    {
                        assert_eq!(proposal.intent.replacement, input);
                        assert_eq!(proposal.intent.session, bridge.session.as_ref());
                        if mode == 2 {
                            bridge.stop();
                            return Ok(ui::CodingReturn::Exit);
                        }
                        bridge.decide(
                            proposal,
                            if mode == 1 {
                                CodingDecision::Deny
                            } else {
                                CodingDecision::Approve
                            },
                        );
                        decided = true;
                    }
                    if p.needs_settlement {
                        assert!(!o.worker_joined && !p.ended);
                        assert!(
                            p.command.as_ref().unwrap().invocations[0]
                                .as_ref()
                                .unwrap()
                                .report
                                .is_none()
                        );
                        if mode == 3 {
                            bridge.stop();
                        }
                        if mode == 4 {
                            return Err(io::Error::other("injected restoration failure"));
                        }
                        return Ok(ui::CodingReturn::Settlement);
                    }
                }
                assert!(std::time::Instant::now() < deadline, "{p:?}");
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .unwrap();
        let p = bridge.snapshot();
        let command = p.command.as_ref().unwrap();
        let fact = command.invocations[0].as_ref().unwrap();
        assert_eq!(
            fact.report.as_ref().unwrap().completion,
            match mode {
                0 => CommandCompletion::Completed,
                1 => CommandCompletion::Failed,
                _ => CommandCompletion::Cancelled,
            }
        );
        assert_eq!(command.cancellation_requested, mode >= 2);
        assert!(command.settled && command.invocations[1].is_none());
        assert!(p.observation.as_ref().unwrap().worker_joined);
        assert_eq!(
            p.durable_effect,
            if matches!(mode, 1 | 2) {
                None
            } else {
                Some(OperationState::Completed)
            }
        );
        assert_eq!(errors.is_empty(), mode != 4);
        assert_eq!(intervals, if matches!(mode, 2 | 4) { 1 } else { 2 });
    }
}
