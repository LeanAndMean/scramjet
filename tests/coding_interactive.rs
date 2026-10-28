#[path = "support/coding_ui_fixture.rs"]
mod fixture;
#[path = "support/coding.rs"]
mod roots;
use scramjet_rs::*;
use std::{sync::Mutex, time::Duration};
static GATE: Mutex<()> = Mutex::new(());
fn executor() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
async fn setup(
    replies: Vec<fixture::Reply>,
) -> (roots::OwnedRoot, fixture::Fixture, Runtime, CodingAgent) {
    let root = roots::OwnedRoot::new();
    let fixture =
        fixture::Fixture::replies_session(replies, std::sync::Arc::from(fixture::SESSION))
            .await
            .unwrap();
    let runtime = Runtime::new(
        fixture.address,
        1,
        RequestRate::new(4, Duration::from_millis(1)).unwrap(),
        SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
        ConcurrencyMode::Fixed,
    )
    .unwrap();
    let session = CodingSession::create(
        &root.path,
        fixture::SESSION,
        CodingCapability::ControlledEdit,
    )
    .unwrap();
    let agent = CodingAgent::start_interactive(session, EditCancellation::default(), "").unwrap();
    (root, fixture, runtime, agent)
}
async fn settle(runtime: Runtime, fixture: fixture::Fixture, observer: &CodingObserver) {
    let snapshot = runtime.shutdown().await;
    assert_eq!(snapshot.active + snapshot.pending + snapshot.handles, 0);
    let report = fixture.shutdown().await.unwrap();
    fixture::reconcile(&observer.snapshot(), &report).unwrap();
    let post = Runtime::process_snapshot();
    assert_eq!(post.unresolved + post.usage_obligations + post.potential, 0);
    assert!(observer.snapshot().worker_joined && observer.snapshot().worker_terminated);
    tokio::time::sleep(Duration::from_millis(5)).await;
}
async fn pending(observer: &CodingObserver) -> std::sync::Arc<CodingProposal> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(p) = &observer.snapshot().proposal {
                return p.clone();
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap()
}
#[test]
fn admission_refusal_retains_synced_prompt_without_inventing_a_runtime_id() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (captured, received) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let fixture = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 8192];
            let mut len = 0;
            loop {
                let n = socket.read(&mut bytes[len..]).await.unwrap();
                assert!(n > 0);
                len += n;
                if let Some(end) = bytes[..len].windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&bytes[..end])
                        .unwrap()
                        .to_ascii_lowercase();
                    let length: usize = header
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if len < end + 4 + length {
                        continue;
                    }
                    assert_eq!(&bytes[end + 4..end + 4 + length], b"fixture probe");
                    let id = RequestId(
                        header
                            .lines()
                            .find_map(|l| l.strip_prefix("x-fixture-id:"))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap(),
                    );
                    captured.send(id).unwrap();
                    released.await.unwrap();
                    return id;
                }
                assert!(len < bytes.len());
            }
        });
        let runtime = Runtime::new(
            endpoint,
            1,
            RequestRate::new(4, Duration::from_millis(1)).unwrap(),
            SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
            ConcurrencyMode::Fixed,
        )
        .unwrap();
        let mut blockers: Vec<_> = (0..9)
            .map(|_| {
                runtime
                    .submit("fixture probe", SyntheticCost::new(1, 1).unwrap())
                    .unwrap()
            })
            .collect();
        let accepted = received.await.unwrap();
        assert_eq!(runtime.snapshot().pending, 8);
        let root = roots::OwnedRoot::new();
        let session =
            CodingSession::create(&root.path, "full", CodingCapability::ControlledEdit).unwrap();
        let agent = CodingAgent::start_interactive(
            session,
            EditCancellation::default(),
            "not admitted\n界",
        )
        .unwrap();
        let observer = agent.observer();
        assert!(matches!(
            agent.run(&runtime).await,
            Err(CodingError::Invalid("model admission refused"))
        ));
        let o = observer.snapshot();
        assert!(o.attempts[0].recorded && o.attempts[0].request_id.is_none());
        assert!(o.run_ended && o.worker_joined && o.error.is_some());
        assert_eq!(
            std::fs::read(root.path.join("sample.txt")).unwrap(),
            b"before\n"
        );
        for handle in &mut blockers {
            handle.cancel();
        }
        for mut handle in blockers {
            while handle.next_delta().await.is_some() {}
            let terminal = handle.terminal().await.unwrap();
            assert_eq!(
                terminal.synthetic_usage,
                if handle.id() == accepted {
                    SyntheticUsage::Unknown
                } else {
                    SyntheticUsage::NotAttempted
                }
            );
        }
        let pre = runtime.shutdown().await;
        assert_eq!(pre.active + pre.pending + pre.handles, 0);
        release.send(()).unwrap();
        assert_eq!(fixture.await.unwrap(), accepted);
        assert_eq!(pre.unknown_usage, 1);
        assert_eq!(pre.potential + pre.unresolved, 1);
        Runtime::reconcile_usage_fixture(accepted, 1).unwrap();
        Runtime::reconcile_fixture(accepted).unwrap();
        assert_eq!(
            Runtime::process_snapshot().unresolved + Runtime::process_snapshot().usage_obligations,
            0
        );
        let before = std::fs::read(root.path.join(".coding-journal")).unwrap();
        for _ in 0..3 {
            assert!(reconstruct_coding(&root.path, "full").unwrap().is_empty());
            assert_eq!(
                std::fs::read(root.path.join(".coding-journal")).unwrap(),
                before
            );
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    });
}

#[test]
fn consuming_prompt_is_exact_on_all_requests_and_durable_without_replay() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for (prompt, approve) in [
            ("first\n界 e\u{301}", true),
            ("second\n🙂 \\", true),
            ("denied draft", false),
            ("cancelled draft", false),
        ] {
            let root = roots::OwnedRoot::new();
            let fixture =
                fixture::Fixture::consuming_session(std::sync::Arc::from(fixture::SESSION))
                    .await
                    .unwrap();
            let runtime = Runtime::new(
                fixture.address,
                1,
                RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
                ConcurrencyMode::Fixed,
            )
            .unwrap();
            let session = CodingSession::create(
                &root.path,
                fixture::SESSION,
                CodingCapability::ControlledEdit,
            )
            .unwrap();
            let agent =
                CodingAgent::start_interactive(session, EditCancellation::default(), prompt)
                    .unwrap();
            let observer = agent.observer();
            let control = agent.control();
            let (result, ()) = tokio::join!(agent.run(&runtime), async {
                let proposal = pending(&observer).await;
                assert_eq!(proposal.intent.replacement, prompt);
                assert!(!observer.snapshot().authorized);
                if prompt == "cancelled draft" {
                    control.stop();
                } else {
                    control
                        .decide(
                            &proposal,
                            if approve {
                                CodingDecision::Approve
                            } else {
                                CodingDecision::Deny
                            },
                        )
                        .unwrap();
                }
            });
            assert_eq!(result.is_ok(), approve);
            let o = observer.snapshot();
            let pre = runtime.shutdown().await;
            let report = fixture.shutdown().await.unwrap();
            fixture::reconcile(&o, &report).unwrap();
            assert_eq!(report.prompt.as_deref(), Some(prompt));
            assert_eq!(report.accepts, if approve { 4 } else { 2 });
            assert_eq!(pre.synthetic_measured, Some(report.accepts as u64));
            for (slot, receipt) in o.attempts.iter().zip(report.receipts).take(report.accepts) {
                let receipt = receipt.unwrap();
                assert!(slot.recorded && receipt.consumed && receipt.joined);
                assert_eq!(slot.request_id, Some(receipt.request_id));
            }
            let journal_path = root.path.join(".coding-journal");
            let journal = std::fs::read_to_string(&journal_path).unwrap();
            let requests: Vec<_> = journal
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .map(|v| v["record"].clone())
                .filter(|v| v["entry"]["event"] == "Request")
                .collect();
            assert_eq!(requests.len(), report.accepts);
            for (index, entry) in requests.iter().enumerate() {
                let text = entry["entry"]["text"].as_str().unwrap();
                assert!(text.len() <= 4096);
                let context: serde_json::Value = serde_json::from_str(text).unwrap();
                assert_eq!(context["session"], fixture::SESSION);
                assert_eq!(context["turn"], index + 1);
                assert_eq!(context["prompt"], prompt);
                assert_eq!(
                    context["previous"],
                    [
                        "",
                        fixture::BEFORE,
                        "replacement durably completed",
                        "exact content verified"
                    ][index]
                );
            }
            for _ in 0..3 {
                let facts = reconstruct_coding(&root.path, fixture::SESSION).unwrap();
                assert_eq!(facts.len(), usize::from(approve));
                assert_eq!(std::fs::read_to_string(&journal_path).unwrap(), journal);
                assert_eq!(
                    std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                    if approve { prompt } else { fixture::BEFORE }
                );
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
}

#[test]
fn approve_deny_and_external_stop_without_any_painter() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for mode in 0..3 {
            let (root, fixture, runtime, agent) = setup(fixture::golden()).await;
            let observer = agent.observer();
            let control = agent.control();
            let run = agent.run(&runtime);
            let decision = async {
                let proposal = pending(&observer).await;
                assert_eq!(proposal.turn, 2);
                assert_eq!(proposal.intent.replacement, fixture::AFTER);
                let snapshot = runtime.snapshot();
                assert_eq!(snapshot.active + snapshot.pending + snapshot.handles, 0);
                if mode == 2 {
                    control.stop();
                } else {
                    control
                        .decide(
                            &proposal,
                            if mode == 0 {
                                CodingDecision::Approve
                            } else {
                                CodingDecision::Deny
                            },
                        )
                        .unwrap();
                }
            };
            let (result, ()) = tokio::join!(run, decision);
            let o = observer.snapshot();
            assert!(o.run_ended);
            if mode == 0 {
                assert_eq!(result.unwrap().turns, 4);
                assert!(o.authorized && o.dispatched && o.verified && o.final_summary.is_some());
                assert_eq!(o.effect, Some(OperationState::Completed));
                assert_eq!(
                    o.attempts.iter().filter(|s| s.accounting.is_some()).count(),
                    4
                );
                assert_eq!(
                    reconstruct_coding(&root.path, fixture::SESSION).unwrap()[0].state,
                    OperationState::Completed
                );
                assert_eq!(
                    std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                    fixture::AFTER
                );
            } else {
                assert!(result.is_err());
                assert!(!o.authorized && !o.dispatched && o.effect.is_none());
                assert!(
                    reconstruct_coding(&root.path, fixture::SESSION)
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                    fixture::BEFORE
                );
            }
            settle(runtime, fixture, &observer).await;
        }
    });
}
#[test]
fn changed_foreign_duplicate_and_concurrent_decisions_fail_closed() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, fixture, runtime, agent) = setup(fixture::golden()).await;
        let observer = agent.observer();
        let control = agent.control();
        let (result, ()) = tokio::join!(agent.run(&runtime), async {
            let proposal = pending(&observer).await;
            for field in 0..9 {
                let mut changed = (*proposal).clone();
                match field {
                    0 => changed.owner += 1,
                    1 => changed.request_id.0 += 1,
                    2 => changed.turn += 1,
                    3 => changed.intent.session.push('x'),
                    4 => changed.intent.root.inode += 1,
                    5 => changed.intent.operation += 1,
                    6 => changed.intent.target.push('x'),
                    7 => changed.intent.expected.push('x'),
                    _ => changed.intent.replacement.push('x'),
                }
                assert!(control.decide(&changed, CodingDecision::Approve).is_err());
                assert!(observer.snapshot().decision.is_none());
            }
            let outcomes = std::thread::scope(|scope| {
                let threads: Vec<_> = (0..8)
                    .map(|index| {
                        let control = &control;
                        let proposal = &proposal;
                        scope.spawn(move || {
                            control
                                .decide(
                                    proposal,
                                    if index % 2 == 0 {
                                        CodingDecision::Approve
                                    } else {
                                        CodingDecision::Deny
                                    },
                                )
                                .is_ok()
                        })
                    })
                    .collect();
                threads
                    .into_iter()
                    .map(|t| t.join().unwrap())
                    .filter(|ok| *ok)
                    .count()
            });
            assert_eq!(outcomes, 1);
            assert!(control.decide(&proposal, CodingDecision::Approve).is_err());
        });
        assert_eq!(
            result.is_ok(),
            observer.snapshot().decision == Some(CodingDecision::Approve)
        );
        assert_eq!(observer.snapshot().dispatched, result.is_ok());
        assert!(std::fs::read_to_string(root.path.join("sample.txt")).is_ok());
        settle(runtime, fixture, &observer).await;
    });
}
#[test]
fn rejected_responses_never_publish_proposals_or_effects() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for mode in 0..5 {
            let mut replies = fixture::golden();
            match mode { 0 => replies[1].done=false, 1 => replies[1].status=500, 2 => replies[1].text="{\"kind\":\"Replace\"}".into(), 3 => replies[1].text="x".repeat(17000), _ => replies[1].text = serde_json::json!({"kind":"Replace","op":2,"target":"sample.txt","expected":"before\n","replacement":"bad\u{1b}"}).to_string() }
            let (root, fixture, runtime, agent) = setup(replies).await;
            let observer = agent.observer();
            assert!(agent.run(&runtime).await.is_err());
            assert!(observer.snapshot().proposal.is_none());
            assert!(!observer.snapshot().authorized);
            assert_eq!(observer.snapshot().attempts[0].material,Some("Read"));
            assert_eq!(std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),fixture::BEFORE);
            settle(runtime,fixture,&observer).await;
        }
    });
}
#[test]
fn completed_effect_survives_later_failed_verification_and_final() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for turn in [2, 3] {
            let mut replies = fixture::golden();
            replies[turn].status = 500;
            let (root, fixture, runtime, agent) = setup(replies).await;
            let observer = agent.observer();
            let control = agent.control();
            let (result, ()) = tokio::join!(agent.run(&runtime), async {
                let proposal = pending(&observer).await;
                control.decide(&proposal, CodingDecision::Approve).unwrap();
            });
            assert!(result.is_err());
            let o = observer.snapshot();
            assert_eq!(o.effect, Some(OperationState::Completed));
            assert!(o.authorized && o.dispatched);
            assert!(o.final_summary.is_none());
            assert_eq!(o.verified, turn == 3);
            assert_eq!(
                reconstruct_coding(&root.path, fixture::SESSION).unwrap()[0].state,
                OperationState::Completed
            );
            assert_eq!(
                std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                fixture::AFTER
            );
            settle(runtime, fixture, &observer).await;
        }
    });
}
#[test]
fn dropping_pending_run_joins_owner_and_keeps_observer_clones_non_cancelling() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, fixture, runtime, agent) = setup(fixture::golden()).await;
        let observer = agent.observer(); let control = agent.control();
        drop(observer.clone()); drop(control.clone());
        let mut run = Box::pin(agent.run(&runtime));
        tokio::select! { result = &mut run => panic!("unexpected result: {result:?}"), _ = pending(&observer) => {} }
        drop(run);
        assert!(observer.snapshot().worker_joined);
        assert!(observer.snapshot().decision.is_none());
        assert_eq!(std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),fixture::BEFORE);
        settle(runtime,fixture,&observer).await;
    });
}
#[test]
fn dropped_borrowed_run_finish_cancels_pending_decision_without_external_stop() {
    const CHILD: &str = "SCRAMJET_BORROWED_FINISH_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new("timeout")
            .args(["15s"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dropped_borrowed_run_finish_cancels_pending_decision_without_external_stop",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "borrowed-run finish watchdog failed: {:?}\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let (root, fixture, runtime, mut agent) = setup(fixture::golden()).await;
        let observer = agent.observer();
        let mut run = Box::pin(agent.run_unjoined(&runtime));
        tokio::select! {
            result = &mut run => panic!("unexpected result: {result:?}"),
            _ = pending(&observer) => {},
        }
        drop(run);
        let before = observer.snapshot();
        assert!(!before.run_ended && !before.worker_terminated && !before.worker_joined);
        assert!(before.decision.is_none());
        assert_eq!(
            runtime.snapshot().active + runtime.snapshot().pending + runtime.snapshot().handles,
            0
        );
        let start = std::time::Instant::now();
        agent.finish().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
        let after = observer.snapshot();
        assert!(after.run_ended && after.worker_terminated && after.worker_joined);
        assert!(after.decision.is_none() && after.error.is_none());
        assert!(!after.authorized && !after.dispatched && after.effect.is_none());
        assert!(!after.verified && after.final_summary.is_none());
        assert_eq!(
            after
                .attempts
                .iter()
                .filter(|s| s.request_id.is_some())
                .count(),
            2
        );
        for slot in &after.attempts[..2] {
            assert_eq!(slot.outcome, Some(Outcome::Success));
            let accounting = slot.accounting.unwrap();
            assert_eq!(Some(accounting.request_id), slot.request_id);
            assert!(!accounting.unknown_usage && !accounting.unresolved_exposure);
        }
        let journal_path = root.path.join(".coding-journal");
        let journal = std::fs::read(&journal_path).unwrap();
        use std::os::unix::fs::MetadataExt;
        let inode = std::fs::metadata(&journal_path).unwrap().ino();
        for _ in 0..3 {
            assert!(
                reconstruct_coding(&root.path, fixture::SESSION)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
            assert_eq!(std::fs::metadata(&journal_path).unwrap().ino(), inode);
            assert_eq!(
                std::fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                fixture::BEFORE
            );
        }
        let pre = runtime.shutdown().await;
        assert_eq!(pre.active + pre.pending + pre.handles, 0);
        let report = fixture.shutdown().await.unwrap();
        assert_eq!(report.accepts, 2);
        assert_eq!(report.receipts.iter().flatten().count(), 2);
        for (index, receipt) in report.receipts.iter().flatten().enumerate() {
            assert_eq!(receipt.turn, index as u64 + 1);
            assert_eq!(Some(receipt.request_id), after.attempts[index].request_id);
            assert_eq!(receipt.cost, 1);
            assert!(receipt.joined);
        }
        fixture::reconcile(&after, &report).unwrap();
        let post = Runtime::process_snapshot();
        assert_eq!(post.unresolved + post.usage_obligations + post.potential, 0);
    });
}

#[test]
fn target_change_at_live_approval_and_concurrent_stop_preserve_actual_certainty() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        for mode in 0..2 {
            let (root, fixture, runtime, agent) = setup(fixture::golden()).await;
            let observer = agent.observer();
            let control = agent.control();
            let (result, ()) = tokio::join!(agent.run(&runtime), async {
                let proposal = pending(&observer).await;
                if mode == 0 {
                    std::fs::write(root.path.join("sample.txt"), "foreign\n").unwrap();
                    control.decide(&proposal, CodingDecision::Approve).unwrap();
                } else {
                    std::thread::scope(|scope| {
                        scope.spawn(|| {
                            let _ = control.decide(&proposal, CodingDecision::Approve);
                        });
                        scope.spawn(|| control.stop());
                    });
                }
            });
            let o = observer.snapshot();
            if mode == 0 {
                assert!(matches!(result, Err(CodingError::TargetMismatch)));
                assert!(!o.authorized && !o.dispatched);
            } else {
                assert!(result.is_err());
                assert_eq!(
                    o.effect,
                    if o.dispatched {
                        Some(OperationState::Completed)
                    } else if o.authorized {
                        Some(OperationState::NotDispatched)
                    } else {
                        None
                    }
                );
            }
            let content = std::fs::read_to_string(root.path.join("sample.txt")).unwrap();
            assert_eq!(
                content,
                if mode == 0 {
                    "foreign\n"
                } else if o.dispatched {
                    fixture::AFTER
                } else {
                    fixture::BEFORE
                }
            );
            settle(runtime, fixture, &observer).await;
        }
    });
}

#[test]
fn missing_and_foreign_attestations_cannot_settle_unknown_usage() {
    let _gate = GATE.lock().unwrap();
    executor().block_on(async {
        let mut replies = fixture::golden();
        replies.iter_mut().for_each(|r| r.usage = false);
        let (_root, fixture, runtime, agent) = setup(replies).await;
        let observer = agent.observer();
        let control = agent.control();
        let (result, ()) = tokio::join!(agent.run(&runtime), async {
            let p = pending(&observer).await;
            control.decide(&p, CodingDecision::Approve).unwrap();
        });
        result.unwrap();
        let pre = runtime.shutdown().await;
        assert_eq!(pre.unknown_usage, 4);
        let report = fixture.shutdown().await.unwrap();
        let mut missing = report.clone();
        missing.receipts[0] = None;
        assert!(fixture::reconcile(&observer.snapshot(), &missing).is_err());
        assert_eq!(Runtime::process_snapshot().unknown_usage, 4);
        let mut foreign = report.clone();
        foreign.receipts[0].as_mut().unwrap().request_id.0 += 1;
        assert!(fixture::reconcile(&observer.snapshot(), &foreign).is_err());
        assert_eq!(Runtime::process_snapshot().unknown_usage, 4);
        fixture::reconcile(&observer.snapshot(), &report).unwrap();
        assert_eq!(Runtime::process_snapshot().unknown_usage, 0);
    });
}
