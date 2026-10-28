use super::*;

fn execute(test: impl std::future::Future<Output = ()>) {
    let _gate = TEST_GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(test);
}
async fn first(session: &mut ProjectSession) -> ProjectSourceReceipt {
    submit(
        session,
        &request("OLD", "NEW"),
        None,
        CodingDecision::Approve,
    )
    .await
    .0
    .unwrap()
}
async fn close(
    session: &ProjectSession,
    settlement: ProjectSettlement,
    fixture: model::Fixture,
) -> ProjectFinish {
    let attempts = fixture.requests().len();
    fixture.settle(attempts).await;
    let finish = settlement.finish().await.unwrap();
    assert!(fixture.shutdown().await.joined);
    assert!(
        finish.readers_joined
            && finish.coding_joined
            && finish.storage_joined
            && finish.runtime_joined
            && finish.history_acknowledged
    );
    assert_eq!(
        session.observer().snapshot().settled,
        finish.logical_settled
    );
    finish
}
#[test]
fn missing_source_or_verification_report_preserves_physical_joins_but_is_logically_incomplete() {
    execute(async {
        for verification in [false, true] {
            let root = project();
            let storage = roots::OwnedRoot::new();
            let fixture = model::Fixture::start(model::Mode::Golden).await;
            let (mut session, settlement) = ProjectSession::start(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "missing-report",
            );
            let mut source = first(&mut session).await;
            if verification {
                let text = summary(&session);
                let continuation = session
                    .report_source(&mut source, CommandCompletion::Completed, &text)
                    .await
                    .unwrap()
                    .unwrap();
                let ready = session.resume(continuation).await.unwrap();
                let _unreported = session.verify(ready).await.unwrap();
            }
            let finish = close(&session, settlement, fixture).await;
            assert!(!finish.logical_settled);
            assert!(finish.error.is_some());
            assert_eq!(
                session.observer().snapshot().phase,
                ProjectPhase::Incomplete
            );
            let history = reconstruct_project_history(&storage.path, "missing-report").unwrap();
            assert_eq!(
                history.coding[0].as_ref().unwrap().operations()[0].state,
                OperationState::Completed
            );
            assert!(
                matches!(history.records.last().unwrap().as_ref(),ProjectRecord::Closure(c) if !c.logical_settled)
            );
        }
    });
}
#[test]
fn false_cancelled_reports_and_contradictory_recovered_cancellation_are_refused() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "cancel-authority",
        );
        let mut source = first(&mut session).await;
        assert!(
            session
                .report_source(&mut source, CommandCompletion::Cancelled, "not cancelled")
                .await
                .is_err()
        );
        let text = summary(&session);
        let continuation = session
            .report_source(&mut source, CommandCompletion::Completed, &text)
            .await
            .unwrap()
            .unwrap();
        let ready = session.resume(continuation).await.unwrap();
        let mut receipt = session.verify(ready).await.unwrap();
        assert!(
            session
                .report_verification(&mut receipt, CommandCompletion::Cancelled, "not cancelled")
                .await
                .is_err()
        );
        session
            .report_verification(&mut receipt, CommandCompletion::Completed, "matched")
            .await
            .unwrap();
        assert!(close(&session, settlement, fixture).await.logical_settled);
        let path = storage.path.join(".project-journal");
        let original = fs::read_to_string(&path).unwrap();
        let changed = original
            .lines()
            .map(|line| {
                let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                if let Some(report) = value["record"].get_mut("VerificationReport") {
                    report["completion"] = "Cancelled".into();
                }
                value.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&path, &changed).unwrap();
        assert!(reconstruct_project_history(&storage.path, "cancel-authority").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        fs::write(path, original).unwrap();
    });
}
#[test]
fn foreign_request_context_session_is_rejected_against_the_actual_coding_header() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "foreign-context",
        );
        let mut receipt = first(&mut session).await;
        verify(&mut session, &mut receipt).await;
        close(&session, settlement, fixture).await;
        let path = storage.path.join("coding-1/.coding-journal");
        let original = fs::read_to_string(&path).unwrap();
        let changed = original
            .lines()
            .map(|line| {
                let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                if value["record"]["entry"]["event"] == "Request" {
                    let mut context: serde_json::Value =
                        serde_json::from_str(value["record"]["entry"]["text"].as_str().unwrap())
                            .unwrap();
                    context["session"] = "foreign".into();
                    value["record"]["entry"]["text"] = context.to_string().into();
                }
                value.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&path, &changed).unwrap();
        assert!(reconstruct_project_history(&storage.path, "foreign-context").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), changed);
        fs::write(path, original).unwrap();
    });
}
#[test]
fn causally_admitted_project_prefixes_recover_artifacts_without_minting_reports() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "artifact-prefix",
        );
        let mut receipt = first(&mut session).await;
        verify(&mut session, &mut receipt).await;
        close(&session, settlement, fixture).await;
        let path = storage.path.join(".project-journal");
        let original = fs::read(&path).unwrap();
        let target = fs::read(root.path.join("sample.txt")).unwrap();
        let mut end = 0;
        for (index, line) in original.split_inclusive(|b| *b == b'\n').enumerate() {
            end += line.len();
            fs::write(&path, &original[..end]).unwrap();
            let result = reconstruct_project_history(&storage.path, "artifact-prefix");
            if index == 2 {
                assert!(result.is_err(), "coding predates durable Readers admission");
                assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), target);
                assert_eq!(fs::read(&path).unwrap(), original[..end]);
                continue;
            }
            let recovered = result.unwrap();
            if index >= 3 {
                assert!(recovered.readers[0].as_ref().unwrap().parent_finished);
                assert_eq!(
                    recovered.coding[0].as_ref().unwrap().operations()[0].state,
                    OperationState::Completed
                );
            }
            if index < 5 {
                assert!(
                    recovered
                        .records
                        .iter()
                        .all(|r| !matches!(r.as_ref(), ProjectRecord::SourceReport(_)))
                );
            }
            assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), target);
            assert_eq!(fs::read(&path).unwrap(), original[..end]);
        }
        fs::write(&path, &original[..original.len() - 1]).unwrap();
        assert!(reconstruct_project_history(&storage.path, "artifact-prefix").is_err());
        fs::write(path, original).unwrap();
    });
}
fn check_recovery(
    storage: &std::path::Path,
    project: &std::path::Path,
    session: &str,
    accepted: bool,
) -> Option<ProjectHistory> {
    let paths = [
        storage.join(".project-journal"),
        storage.join("readers-1/.reader-journal"),
        storage.join("coding-1/.coding-journal"),
        project.join("sample.txt"),
    ];
    let before = paths
        .each_ref()
        .map(|path| path.exists().then(|| fs::read(path).unwrap()));
    let recovered = reconstruct_project_history(storage, session);
    assert_eq!(recovered.is_ok(), accepted);
    let output = std::process::Command::new("timeout")
        .arg("15")
        .arg(env!("CARGO_BIN_EXE_scramjet-rs"))
        .args(["project-demo", "--history", "--storage"])
        .arg(storage)
        .args(["--session", session])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(if accepted { 0 } else { 1 }),
        "{output:?}"
    );
    assert_eq!(
        paths
            .each_ref()
            .map(|path| path.exists().then(|| fs::read(path).unwrap())),
        before
    );
    recovered.ok()
}
#[test]
fn coding_artifact_prefix_requires_durable_joined_readers_and_matching_accepted_end() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "causal-prefix",
        );
        let mut source = first(&mut session).await;
        verify(&mut session, &mut source).await;
        close(&session, settlement, fixture).await;
        let path = storage.path.join(".project-journal");
        let original = fs::read_to_string(&path).unwrap();
        let admitted = original.lines().take(4).collect::<Vec<_>>().join("\n") + "\n";
        let captured = original.lines().take(3).collect::<Vec<_>>().join("\n") + "\n";
        let reader_path = storage.path.join("readers-1/.reader-journal");
        let reader = fs::read(&reader_path).unwrap();
        let mut end = 0;
        let mut last = 0;
        while end < reader.len() {
            last = end;
            end += 4 + u32::from_le_bytes(reader[end..end + 4].try_into().unwrap()) as usize;
        }
        assert_eq!(reader[last + 6], 3);
        for variant in 0..5 {
            let changed = match variant {
                0 | 1 => captured.clone(),
                4 => admitted.clone(),
                _ => {
                    admitted
                        .lines()
                        .map(|line| {
                            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                            if let Some(readers) = value["record"].get_mut("Readers") {
                                readers[if variant == 2 { "joined" } else { "ended" }] =
                                    false.into();
                            }
                            value.to_string()
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n"
                }
            };
            fs::write(&path, changed).unwrap();
            fs::write(
                &reader_path,
                if matches!(variant, 0 | 3 | 4) {
                    &reader[..last]
                } else {
                    &reader
                },
            )
            .unwrap();
            check_recovery(&storage.path, &root.path, "causal-prefix", false);
        }
        fs::write(&path, &admitted).unwrap();
        fs::write(&reader_path, &reader).unwrap();
        let recovered = check_recovery(&storage.path, &root.path, "causal-prefix", true).unwrap();
        assert_eq!(
            recovered.coding[0].as_ref().unwrap().operations()[0].state,
            OperationState::Completed
        );
        assert!(recovered.records.iter().all(|r| !matches!(
            r.as_ref(),
            ProjectRecord::Coding(_) | ProjectRecord::SourceReport(_)
        )));
        let coding_path = storage.path.join("coding-1/.coding-journal");
        let coding = fs::read_to_string(&coding_path).unwrap();
        let mut prefix = String::new();
        for line in coding.lines() {
            prefix.push_str(line);
            prefix.push('\n');
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            if value["record"]["kind"] == "Dispatched" {
                break;
            }
        }
        assert!(prefix.len() < coding.len());
        fs::write(&coding_path, &prefix).unwrap();
        let recovered = check_recovery(&storage.path, &root.path, "causal-prefix", true).unwrap();
        assert_eq!(
            recovered.coding[0].as_ref().unwrap().operations()[0].state,
            OperationState::Uncertain
        );
        fs::write(&coding_path, &coding).unwrap();
        let held = storage.path.join("coding-held");
        fs::rename(storage.path.join("coding-1"), &held).unwrap();
        fs::write(&path, captured).unwrap();
        fs::write(&reader_path, &reader[..last]).unwrap();
        let recovered = check_recovery(&storage.path, &root.path, "causal-prefix", true).unwrap();
        assert!(!recovered.readers[0].as_ref().unwrap().parent_finished);
        assert!(recovered.coding[0].is_none());
        fs::rename(held, storage.path.join("coding-1")).unwrap();
        fs::write(reader_path, reader).unwrap();
        fs::write(path, original).unwrap();
    });
}
#[test]
fn incomplete_verification_history_preserves_evidence_but_cannot_justify_logical_closure() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "incomplete-verify",
        );
        let mut source = first(&mut session).await;
        verify(&mut session, &mut source).await;
        close(&session, settlement, fixture).await;
        let path = storage.path.join(".project-journal");
        let original = fs::read_to_string(&path).unwrap();
        for closure in [Some(true), Some(false), None] {
            let changed = original
                .lines()
                .filter_map(|line| {
                    let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                    if let Some(report) = value["record"].get_mut("VerificationReport") {
                        report["completion"] = "Incomplete".into();
                    }
                    if let Some(record) = value["record"].get_mut("Closure") {
                        record["logical_settled"] = closure?.into();
                    }
                    Some(value.to_string())
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";
            fs::write(&path, changed).unwrap();
            let recovered = check_recovery(
                &storage.path,
                &root.path,
                "incomplete-verify",
                closure != Some(true),
            );
            if let Some(recovered) = recovered {
                assert_eq!(recovered.closure_recorded, closure.is_some());
                assert_eq!(
                    recovered.coding[0].as_ref().unwrap().operations()[0].state,
                    OperationState::Completed
                );
                assert!(recovered.records.iter().any(|r| matches!(r.as_ref(), ProjectRecord::SourceReport(r) if r.completion == "Completed")));
                assert!(recovered.records.iter().any(
                    |r| matches!(r.as_ref(), ProjectRecord::VerificationRead(r) if r.matched)
                ));
                assert!(recovered.records.iter().any(|r| matches!(r.as_ref(), ProjectRecord::VerificationReport(r) if r.completion == "Incomplete")));
            }
        }
        fs::write(path, original).unwrap();
        check_recovery(&storage.path, &root.path, "incomplete-verify", true);
    });
}
#[test]
fn stop_linearizes_with_both_reports_and_ready_execution_without_rewriting_prior_success() {
    execute(async {
        for at in 0..4 {
            let root = project();
            let storage = roots::OwnedRoot::new();
            let fixture = model::Fixture::start(model::Mode::Golden).await;
            let (mut session, settlement) = ProjectSession::start(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "stop-orders",
            );
            let mut source = first(&mut session).await;
            let text = summary(&session);
            if at == 0 {
                session.control().stop();
                assert!(
                    session
                        .report_source(&mut source, CommandCompletion::Completed, &text)
                        .await
                        .is_err()
                );
                assert!(
                    session
                        .report_source(&mut source, CommandCompletion::Cancelled, "stopped source")
                        .await
                        .unwrap()
                        .is_none()
                );
            } else {
                let continuation = session
                    .report_source(&mut source, CommandCompletion::Completed, &text)
                    .await
                    .unwrap()
                    .unwrap();
                let ready = session.resume(continuation).await.unwrap();
                if at == 1 {
                    session.control().stop();
                    assert!(session.verify(ready).await.is_err());
                    assert!(
                        session
                            .observer()
                            .snapshot()
                            .records
                            .iter()
                            .all(|r| !matches!(r.as_ref(), ProjectRecord::VerificationRead(_)))
                    );
                } else {
                    let mut receipt = session.verify(ready).await.unwrap();
                    if at == 2 {
                        session.control().stop();
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
                        assert!(
                            session
                                .report_verification(
                                    &mut receipt,
                                    CommandCompletion::Failed,
                                    "failed"
                                )
                                .await
                                .is_err()
                        );
                        session
                            .report_verification(
                                &mut receipt,
                                CommandCompletion::Cancelled,
                                "stopped verification",
                            )
                            .await
                            .unwrap();
                    } else {
                        session
                            .report_verification(
                                &mut receipt,
                                CommandCompletion::Completed,
                                "matched",
                            )
                            .await
                            .unwrap();
                        session.control().stop();
                    }
                }
            }
            assert!(session.submit(&request("NEW", "NEXT")).await.is_err());
            assert_eq!(fixture.requests().len(), 7);
            close(&session, settlement, fixture).await;
            let history = reconstruct_project_history(&storage.path, "stop-orders").unwrap();
            if at != 0 {
                assert!(history.records.iter().any(|r| matches!(r.as_ref(),ProjectRecord::SourceReport(r) if r.completion=="Completed")));
            }
        }
    });
}
#[test]
fn pure_ready_admission_does_not_read_a_changed_project_and_later_failure_keeps_source_report() {
    execute(async {
        let root = project();
        let storage = roots::OwnedRoot::new();
        let fixture = model::Fixture::start(model::Mode::Golden).await;
        let (mut session, settlement) = ProjectSession::start(
            runtime(fixture.endpoint),
            &root.path,
            &storage.path,
            "changed-verify",
        );
        let mut source = first(&mut session).await;
        let text = summary(&session);
        let continuation = session
            .report_source(&mut source, CommandCompletion::Completed, &text)
            .await
            .unwrap()
            .unwrap();
        fs::write(root.path.join("sample.txt"), "changed after source").unwrap();
        let ready = session.resume(continuation).await.unwrap();
        assert!(
            session
                .observer()
                .snapshot()
                .records
                .iter()
                .all(|r| !matches!(r.as_ref(), ProjectRecord::VerificationRead(_)))
        );
        let mut receipt = session.verify(ready).await.unwrap();
        assert!(
            session
                .report_verification(&mut receipt, CommandCompletion::Completed, "not matched")
                .await
                .is_err()
        );
        session
            .report_verification(&mut receipt, CommandCompletion::Failed, "checkout changed")
            .await
            .unwrap();
        assert!(close(&session, settlement, fixture).await.logical_settled);
        let history = reconstruct_project_history(&storage.path, "changed-verify").unwrap();
        assert!(history.records.iter().any(
            |r| matches!(r.as_ref(),ProjectRecord::SourceReport(r) if r.completion=="Completed")
        ));
        assert!(history.records.iter().any(
            |r| matches!(r.as_ref(),ProjectRecord::VerificationReport(r) if r.completion=="Failed")
        ));
        assert_eq!(
            fs::read_to_string(root.path.join("sample.txt")).unwrap(),
            "changed after source"
        );
    });
}
