use super::*;

#[test]
fn missing_foreign_and_contradictory_cross_journal_artifacts_are_rejected_read_only() {
    let _gate = TEST_GATE.lock().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let root = project();
            let storage = roots::OwnedRoot::new();
            let fixture = model::Fixture::start(model::Mode::Golden).await;
            let (mut session, settlement) = ProjectSession::start(
                runtime(fixture.endpoint),
                &root.path,
                &storage.path,
                "cross-artifacts",
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
            verify(&mut session, &mut receipt).await;
            assert!(settlement.finish().await.unwrap().logical_settled);
            fixture
                .shutdown()
                .await
                .attest(&session.observer().snapshot())
                .unwrap();
            let target = fs::read(root.path.join("sample.txt")).unwrap();
            for relative in ["readers-1/.reader-journal", "coding-1/.coding-journal"] {
                let path = storage.path.join(relative);
                let original = fs::read(&path).unwrap();
                let moved = path.with_extension("held");
                fs::rename(&path, &moved).unwrap();
                assert!(reconstruct_project_history(&storage.path, "cross-artifacts").is_err());
                assert!(!path.exists());
                fs::rename(moved, &path).unwrap();
                let changed = if relative.starts_with("readers") {
                    let mut bytes = original.clone();
                    assert_eq!(bytes[25], b'p');
                    bytes[25] = b'q';
                    bytes
                } else {
                    String::from_utf8(original.clone())
                        .unwrap()
                        .lines()
                        .enumerate()
                        .map(|(index, line)| {
                            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                            if index == 0 {
                                value["session"] = "foreign".into();
                            }
                            value.to_string()
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                        .into_bytes()
                        .into_iter()
                        .chain(*b"\n")
                        .collect()
                };
                for corrupted in [&original[..original.len() - 1], changed.as_slice()] {
                    fs::write(&path, corrupted).unwrap();
                    assert!(reconstruct_project_history(&storage.path, "cross-artifacts").is_err());
                    assert_eq!(fs::read(&path).unwrap(), corrupted);
                    assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), target);
                }
                fs::write(path, original).unwrap();
            }
            let path = storage.path.join(".project-journal");
            let original = fs::read_to_string(&path).unwrap();
            for change in 0..5 {
                let changed = original
                    .lines()
                    .map(|line| {
                        let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                        match change {
                            0 => {
                                if let Some(reader) = value["record"].get_mut("Readers") {
                                    reader["facts"][0]["summary"] = "invented summary".into();
                                }
                            }
                            1 => {
                                if let Some(coding) = value["record"].get_mut("Coding") {
                                    coding["requests"][0] = 999999.into();
                                }
                            }
                            2 => {
                                if let Some(read) = value["record"].get_mut("VerificationRead") {
                                    read["file"]["identity"]["inode"] = 999999.into();
                                }
                            }
                            3 => {
                                if let Some(capture) = value["record"].get_mut("Captured") {
                                    capture["reader_storage"]["inode"] = 999999.into();
                                }
                            }
                            _ => {
                                if let Some(coding) = value["record"].get_mut("Coding") {
                                    coding["effect"] = "None".into();
                                }
                            }
                        }
                        value.to_string()
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    + "\n";
                fs::write(&path, &changed).unwrap();
                assert!(
                    reconstruct_project_history(&storage.path, "cross-artifacts").is_err(),
                    "mutation {change}"
                );
                assert_eq!(fs::read_to_string(&path).unwrap(), changed);
                assert_eq!(fs::read(root.path.join("sample.txt")).unwrap(), target);
            }
            fs::write(path, original).unwrap();
            assert!(
                reconstruct_project_history(&storage.path, "cross-artifacts")
                    .unwrap()
                    .closure_recorded
            );
        });
}
