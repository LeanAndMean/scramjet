use super::*;
fn execute(test: impl std::future::Future<Output = ()>) {
    let _gate = TEST_GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(test);
}
async fn finish(
    session: &ProjectSession,
    settlement: ProjectSettlement,
    fixture: model::Fixture,
) -> ProjectFinish {
    let finish = settlement.finish().await.unwrap();
    let snapshot = session.observer().snapshot();
    let fixture = fixture.shutdown().await;
    assert!(fixture.joined);
    fixture.attest(&snapshot).unwrap();
    assert_eq!(Runtime::process_snapshot().unknown_usage, 0);
    assert_eq!(Runtime::process_snapshot().unresolved, 0);
    assert!(
        finish.readers_joined
            && finish.coding_joined
            && finish.storage_joined
            && finish.runtime_joined
    );
    finish
}
#[test]
fn reader_failure_wrong_goal_model_failure_denial_and_oversized_context_never_execute_a_companion()
{
    execute(async {
        for mode in 0..5 {
            let root = project();
            let storage = roots::OwnedRoot::new();
            let behavior = match mode {
                0 => model::Mode::ReaderFailure(0),
                1 => model::Mode::WrongGoal,
                2 => model::Mode::ModelFailure(1),
                3 => model::Mode::Golden,
                _ => model::Mode::LargeSummary,
            };
            let fixture = model::Fixture::start(behavior).await;
            let (mut session, settlement) = ProjectSession::start(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "negative-project",
            );
            settlement.release_host();
            let result = if mode == 3 {
                submit(
                    &mut session,
                    &request("OLD", "NEW"),
                    None,
                    CodingDecision::Deny,
                )
                .await
                .0
            } else {
                session.submit(&request("OLD", "NEW")).await
            };
            match result {
                Ok(mut receipt) => {
                    assert!(
                        session
                            .report_source(
                                &mut receipt,
                                CommandCompletion::Failed,
                                "source failure"
                            )
                            .await
                            .unwrap()
                            .is_none()
                    );
                }
                Err(_) => assert!(mode == 0 || mode == 4),
            }
            assert!(
                session
                    .observer()
                    .snapshot()
                    .records
                    .iter()
                    .all(|r| !matches!(
                        r.as_ref(),
                        ProjectRecord::VerificationReady(_) | ProjectRecord::VerificationRead(_)
                    ))
            );
            assert_eq!(
                fs::read_to_string(root.path.join("sample.txt")).unwrap(),
                BEFORE
            );
            assert_eq!(
                fixture.requests().len(),
                match mode {
                    0 | 4 => 3,
                    1 | 3 => 5,
                    _ => 4,
                }
            );
            let snapshot = session.observer().snapshot();
            if mode == 4 {
                let reader = snapshot.journeys[0].readers.as_ref().unwrap();
                assert!(reader.tree_ended && reader.storage_joined && !reader.runtime_joined);
                assert!(reader.children.iter().all(|c| matches!(&c.fact.as_ref().unwrap().state,ReaderState::Completed {summary} if summary==&"\"".repeat(512))));
            }
            finish(&session, settlement, fixture).await;
            let history = reconstruct_project_history(&storage.path, "negative-project").unwrap();
            assert!(history.records.iter().all(|r| !matches!(r.as_ref(),ProjectRecord::SourceReport(r) if r.completion=="Completed")));
        }
    });
}
#[test]
fn actual_reader_summary_changes_survive_all_coding_contexts_and_history() {
    execute(async {
        let mut summaries = Vec::with_capacity(2);
        for changed in [false, true] {
            let root = project();
            let storage = roots::OwnedRoot::new();
            let fixture = model::Fixture::start(if changed {
                model::Mode::ChangedSummary
            } else {
                model::Mode::Golden
            })
            .await;
            let (mut session, settlement) = ProjectSession::start(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "reader-evidence",
            );
            let mut receipt = submit(
                &mut session,
                &request("OLD", "NEW"),
                None,
                CodingDecision::Approve,
            )
            .await
            .0
            .unwrap();
            summaries.push(summary(&session));
            verify(&mut session, &mut receipt).await;
            let requests = fixture.requests();
            for item in requests.iter().filter(|r| r.input.contains("\"project\"")) {
                let context: serde_json::Value = serde_json::from_str(&item.input).unwrap();
                assert_eq!(context["project"]["readers"].as_array().unwrap().len(), 3);
                assert!(
                    context["project"]["readers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|r| r["summary"]
                            .as_str()
                            .unwrap()
                            .contains("alternate observation")
                            == changed)
                );
                assert!(!item.input.contains("complete support rules.txt"));
                assert!(item.input.len() <= 4096);
            }
            finish(&session, settlement, fixture).await;
            let history = reconstruct_project_history(&storage.path, "reader-evidence").unwrap();
            assert!(history.coding[0].as_ref().unwrap().records().iter().any(|r| matches!(&r.entry,CodingHistoryEntry::Material(m) if m.event=="ProjectGoal"&&m.text.contains("alternate observation")==changed)));
        }
        assert_ne!(summaries[0], summaries[1]);
    });
}
#[test]
fn checkout_changes_at_proposal_and_after_source_verification_are_refused_without_recapture() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "proposal-change",
        );
        settlement.release_host();
        let observer = session.observer();
        let control = session.control();
        let change = async {
            loop {
                if let Some(coding) = &observer.snapshot().journeys[0].coding
                    && let Some(proposal) = &coding.proposal
                {
                    fs::write(root.path.join("rules.txt"), "changed support").unwrap();
                    control.decide(proposal, CodingDecision::Approve).unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        let json = request("OLD", "NEW");
        let (result, ()) = tokio::join!(session.submit(&json), change);
        let mut receipt = result.unwrap();
        assert!(
            session
                .report_source(&mut receipt, CommandCompletion::Failed, "checkout changed")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            BEFORE
        );
        assert_eq!(fixture.requests().len(), 5);
        finish(&session, settlement, fixture).await;
    });
}
#[test]
fn an_aborted_borrowed_submission_cancels_without_joining_before_host_return() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "aborted-submit",
        );
        let observer = session.observer();
        let json = request("OLD", "NEW");
        let mut submission = Box::pin(session.submit(&json));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _=&mut submission=>panic!("submission completed without approval"),
                    _=tokio::time::sleep(Duration::from_millis(1))=>{}
                }
                if observer.snapshot().journeys[0]
                    .coding
                    .as_ref()
                    .is_some_and(|c| c.proposal.is_some())
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        drop(submission);
        tokio::time::sleep(Duration::from_millis(30)).await;
        let snapshot = observer.snapshot();
        assert!(snapshot.cancelled && !snapshot.settled);
        assert!(!snapshot.journeys[0].coding.as_ref().unwrap().worker_joined);
        let finish = finish(&session, settlement, fixture).await;
        assert!(!finish.logical_settled);
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            BEFORE
        );
        let history = reconstruct_project_history(&storage.path, "aborted-submit").unwrap();
        assert!(
            history
                .records
                .iter()
                .all(|r| !matches!(r.as_ref(), ProjectRecord::SourceReport(_)))
        );
    });
}
#[test]
fn child_only_cancel_retains_completed_siblings_without_manufacturing_tree_stop() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "child-cancel",
        );
        settlement.release_host();
        let observer = session.observer();
        let control = session.control();
        let cancel = async {
            loop {
                if let Some(readers) = &observer.snapshot().journeys[0].readers
                    && readers.children[0].request_id.is_some()
                {
                    control
                        .cancel_child(&readers.session, readers.generation, 0)
                        .unwrap();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        let json = request("OLD", "NEW");
        let (result, ()) = tokio::join!(session.submit(&json), cancel);
        assert!(result.is_err());
        let snapshot = observer.snapshot();
        assert!(!snapshot.cancelled);
        let readers = snapshot.journeys[0].readers.as_ref().unwrap();
        assert!(matches!(
            readers.children[0].fact.as_ref().unwrap().state,
            ReaderState::Cancelled
        ));
        assert!(readers.children[1..].iter().all(|c| matches!(
            c.fact.as_ref().unwrap().state,
            ReaderState::Completed { .. }
        )));
        finish(&session, settlement, fixture).await;
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            BEFORE
        );
    });
}
#[test]
fn unknown_usage_stalls_the_same_runtime_until_stop_and_actual_fixture_join_attestation() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::UnknownUsage).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "unknown-usage",
        );
        settlement.release_host();
        let control = session.control();
        let stop = async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while fixture.requests().len() < 4 || Runtime::process_snapshot().unknown_usage < 4
                {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            assert_eq!(fixture.requests().len(), 4);
            assert_eq!(Runtime::process_snapshot().unknown_usage, 4);
            control.stop();
        };
        let json = request("OLD", "NEW");
        let (result, ()) = tokio::join!(session.submit(&json), stop);
        let mut receipt = result.unwrap();
        assert!(
            session
                .report_source(
                    &mut receipt,
                    CommandCompletion::Cancelled,
                    "stopped unknown usage"
                )
                .await
                .unwrap()
                .is_none()
        );
        let before = Runtime::process_snapshot();
        assert_eq!(before.unknown_usage, 4);
        let finish = finish(&session, settlement, fixture).await;
        assert_eq!(finish.runtime.unknown_usage, 4);
        assert_eq!(Runtime::process_snapshot().unknown_usage, 0);
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            BEFORE
        );
    });
}
