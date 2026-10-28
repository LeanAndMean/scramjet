use super::*;

#[test]
fn actual_receipts_continuation_and_ready_from_a_closed_foreign_owner_cannot_authorize_another_project()
 {
    let _gate = TEST_GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for foreign in 0..4 {
                let root = project();
                let storage = roots::OwnedRoot::new();
                let fixture = model::Fixture::start(model::Mode::Golden).await;
                let (mut prior, settlement) = ProjectSession::start(
                    runtime(fixture.endpoint),
                    &root.path,
                    &storage.path,
                    "foreign-owner",
                );
                let mut source = submit(
                    &mut prior,
                    &request("OLD", "NEW"),
                    None,
                    CodingDecision::Approve,
                )
                .await
                .0
                .unwrap();
                let mut continuation = None;
                let mut ready = None;
                let mut verification = None;
                if foreign > 0 {
                    continuation = prior
                        .report_source(&mut source, CommandCompletion::Completed, &summary(&prior))
                        .await
                        .unwrap();
                }
                if foreign > 1 {
                    ready = Some(prior.resume(continuation.take().unwrap()).await.unwrap());
                }
                if foreign > 2 {
                    verification = Some(prior.verify(ready.take().unwrap()).await.unwrap());
                }
                let finish = settlement.finish().await.unwrap();
                assert!(finish.runtime_joined && finish.storage_joined);
                fixture
                    .shutdown()
                    .await
                    .attest(&prior.observer().snapshot())
                    .unwrap();

                let other = project();
                let other_storage = roots::OwnedRoot::new();
                let fixture = model::Fixture::start(model::Mode::Golden).await;
                let (mut session, settlement) = ProjectSession::start(
                    runtime(fixture.endpoint),
                    &other.path,
                    &other_storage.path,
                    "foreign-owner",
                );
                let mut actual = submit(
                    &mut session,
                    &request("OLD", "NEW"),
                    None,
                    CodingDecision::Approve,
                )
                .await
                .0
                .unwrap();
                let text = summary(&session);
                assert!(
                    session
                        .report_source(&mut source, CommandCompletion::Completed, &text)
                        .await
                        .is_err()
                );
                assert!(
                    session
                        .report_source(&mut actual, CommandCompletion::Completed, &"s".repeat(513))
                        .await
                        .is_err()
                );
                let handle = session
                    .report_source(&mut actual, CommandCompletion::Completed, &text)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    session
                        .report_source(&mut actual, CommandCompletion::Completed, &text)
                        .await
                        .is_err()
                );
                if let Some(handle) = continuation {
                    assert!(session.resume(handle).await.is_err());
                }
                let admitted = session.resume(handle).await.unwrap();
                if let Some(ready) = ready {
                    assert!(session.verify(ready).await.is_err());
                }
                let mut receipt = session.verify(admitted).await.unwrap();
                if let Some(mut receipt) = verification {
                    assert!(
                        session
                            .report_verification(
                                &mut receipt,
                                CommandCompletion::Completed,
                                "matched"
                            )
                            .await
                            .is_err()
                    );
                }
                session
                    .report_verification(&mut receipt, CommandCompletion::Completed, "matched")
                    .await
                    .unwrap();
                assert!(
                    session
                        .report_verification(&mut receipt, CommandCompletion::Completed, "matched")
                        .await
                        .is_err()
                );
                let finish = settlement.finish().await.unwrap();
                assert!(finish.logical_settled);
                fixture
                    .shutdown()
                    .await
                    .attest(&session.observer().snapshot())
                    .unwrap();
                assert_eq!(finish.runtime.attempts, 7);
                assert_eq!(
                    fs::read_to_string(other.path.join("sample.txt")).unwrap(),
                    BEFORE.replace("OLD", "NEW")
                );
            }
        });
}

#[test]
fn stop_after_source_report_suppresses_paused_admission_without_rewriting_source_success() {
    let _gate = TEST_GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let root = project(); let storage = roots::OwnedRoot::new(); let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(runtime(fixture.endpoint), &root.path, &storage.path, "stop-paused");
        let mut receipt = submit(&mut session, &request("OLD", "NEW"), None, CodingDecision::Approve).await.0.unwrap();
        let handle = session.report_source(&mut receipt, CommandCompletion::Completed, &summary(&session)).await.unwrap().unwrap();
        session.control().stop();
        assert!(session.resume(handle).await.is_err());
        assert!(session.observer().snapshot().records.iter().all(|r| !matches!(r.as_ref(), ProjectRecord::VerificationReady(_) | ProjectRecord::VerificationRead(_))));
        let finish = settlement.finish().await.unwrap(); assert!(finish.logical_settled);
        fixture.shutdown().await.attest(&session.observer().snapshot()).unwrap();
        assert!(reconstruct_project_history(&storage.path, "stop-paused").unwrap().records.iter().any(|r| matches!(r.as_ref(), ProjectRecord::SourceReport(r) if r.completion == "Completed")));
    });
}
