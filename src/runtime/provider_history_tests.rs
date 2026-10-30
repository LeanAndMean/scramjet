use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
fn storage() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "provider-history-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}
fn release(ledger: Arc<Mutex<ProviderLedger>>) {
    // CLOEXEC does not prevent transient lock inheritance during parallel subprocess spawning.
    flock(&ledger.lock().unwrap().file, FlockOperation::Unlock).unwrap();
    drop(ledger);
}
fn acknowledged(ledger: &ProviderLedger) -> Vec<u8> {
    ledger.acknowledged.iter().flatten().copied().collect()
}
fn endpoint() -> SocketAddr {
    "127.0.0.1:18080".parse().unwrap()
}
fn known() -> ProviderUsage {
    ProviderUsage {
        input_tokens: Some(3),
        output_tokens: Some(2),
        total_tokens: Some(5),
        cached_input_tokens: None,
        reasoning_tokens: None,
    }
}
#[test]
fn maximum_accepted_wire_identity_fits_durable_records_and_roundtrips() {
    use crate::{http::responses::ResponsesParser, provider_fixture as fixture};
    for character in ['x', '\u{0000}', 'é'] {
        let make = |count| {
            let id = character.to_string().repeat(count);
            let mut events = fixture::text_events("x", fixture::known_usage());
            for index in [0, 1, 8] {
                events[index]["response"]["id"] = serde_json::json!(id);
                events[index]["response"]["model"] = serde_json::json!("engine/é.".repeat(128));
            }
            events
        };
        let mut low = 1usize;
        let mut high = PROVIDER_FRAME_BYTES;
        while low < high {
            let mid = (low + high).div_ceil(2);
            let events = make(mid);
            if fixture::wire(&events[8..], false).len() <= PROVIDER_FRAME_BYTES {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        let events = make(low);
        assert!(fixture::wire(&make(low + 1)[8..], false).len() > PROVIDER_FRAME_BYTES);
        let mut parser = ResponsesParser::new();
        for byte in fixture::wire(&events, false) {
            parser.byte(byte).unwrap();
        }
        let parsed = parser.complete().unwrap();
        let terminal = Some(ProviderTerminalEvidence {
            response_id: parsed.response_id,
            reported_model: parsed.reported_model,
            status: parsed.status,
        });
        let record = Record::Joined {
            ordinal: 32,
            terminal: terminal.clone(),
            usage: parsed.usage.clone(),
        };
        let bytes = bounded_json_bytes(&record, RECORD_BYTES).unwrap();
        assert!(bytes.len() > 2048);
        println!(
            "maximum-wire-to-record: character={character:?} wire_frame={} record={}",
            fixture::wire(&events[8..], false).len(),
            bytes.len()
        );
        let path = storage();
        let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
        let ordinal = ledger.lock().unwrap().reserve(1).unwrap();
        ledger
            .lock()
            .unwrap()
            .joined(ordinal, terminal, parsed.usage)
            .unwrap();
        let expected = ledger.lock().unwrap().history();
        release(ledger);
        assert_eq!(
            reconstruct_provider_history(&path, endpoint()).unwrap(),
            expected
        );
    }
}
#[test]
fn reservations_and_unknown_facts_are_separate_and_bounded() {
    for unknown_remote in [false, true] {
        let path = storage();
        let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
        for n in 1..=4 {
            let mut ledger = ledger.lock().unwrap();
            let ordinal = ledger.reserve(n).unwrap();
            if !unknown_remote {
                ledger
                    .joined(
                        ordinal,
                        Some(ProviderTerminalEvidence {
                            response_id: format!("resp:/+==.{n}"),
                            reported_model: None,
                            status: "completed".into(),
                        }),
                        None,
                    )
                    .unwrap();
            }
        }
        // Unknown usage alone does not stop ordinary requests; unresolved remote demand does.
        assert_eq!(ledger.lock().unwrap().eligible(), !unknown_remote);
        assert_eq!(ledger.lock().unwrap().reserve(5).is_err(), unknown_remote);
        let expected = ledger.lock().unwrap().history();
        release(ledger);
        let recovered = reconstruct_provider_history(&path, endpoint()).unwrap();
        assert_eq!(recovered, expected);
        assert_eq!(
            recovered.unknown_usage(),
            if unknown_remote { 4 } else { 5 }
        );
        assert_eq!(
            recovered.unresolved_remote(),
            if unknown_remote { 4 } else { 1 }
        );
    }
}
#[test]
fn shared_post_budget_is_not_reset_by_reopening() {
    let path = storage();
    for id in 1..=32 {
        let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
        let mut ledger = ledger.lock().unwrap();
        let ordinal = ledger.reserve(id).unwrap();
        ledger
            .joined(
                ordinal,
                Some(ProviderTerminalEvidence {
                    response_id: format!("resp_{id}"),
                    reported_model: None,
                    status: "completed".into(),
                }),
                Some(known()),
            )
            .unwrap();
        flock(&ledger.file, FlockOperation::Unlock).unwrap();
    }
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    assert_eq!(ledger.lock().unwrap().history().remaining(), 0);
    assert!(ledger.lock().unwrap().reserve(33).is_err());
    release(ledger);
    let before = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
    assert_eq!(
        reconstruct_provider_history(&path, endpoint())
            .unwrap()
            .consumed(),
        32
    );
    assert_eq!(
        before,
        std::fs::read(path.join("provider-attempts.jsonl")).unwrap()
    );
}
#[test]
fn strict_recovery_rejects_incomplete_foreign_or_contradictory_history() {
    let path = storage();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    ledger.lock().unwrap().reserve(1).unwrap();
    release(ledger);
    let file = path.join("provider-attempts.jsonl");
    let original = std::fs::read(&file).unwrap();
    for suffix in [b"{".as_slice(),b"{\"kind\":\"Reserved\",\"ordinal\":2,\"request_id\":1}\n",
        b"{\"kind\":\"Reserved\",\"ordinal\":2,\"request_id\":2,\"unexpected\":true}\n",
        b"{\"kind\":\"Joined\",\"ordinal\":1,\"terminal\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2,\"cached_input_tokens\":null,\"reasoning_tokens\":null}}\n",
        b"{\"kind\":\"Reserved\",\"ordinal\":2,\"ordinal\":3,\"request_id\":2}\n"] {
        let mut bytes=original.clone();bytes.extend_from_slice(suffix);std::fs::write(&file,&bytes).unwrap();
        assert!(reconstruct_provider_history(&path,endpoint()).is_err());assert!(ProviderLedger::open(&path,endpoint()).is_err());
        assert_eq!(bytes,std::fs::read(&file).unwrap());
    }
    std::fs::write(&file, &original).unwrap();
    assert!(reconstruct_provider_history(&path, "127.0.0.1:18081".parse().unwrap()).is_err());
}
#[test]
fn positional_verification_and_appends_preserve_every_acknowledged_byte() {
    let path = storage();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    let file = path.join("provider-attempts.jsonl");
    let mut previous = std::fs::read(&file).unwrap();
    for ordinal in 1..=6 {
        let mut ledger = ledger.lock().unwrap();
        let mut cloned = ledger.file.try_clone().unwrap();
        cloned.seek(SeekFrom::Start(0)).unwrap();
        let mut first = [0; 1];
        cloned.read_exact(&mut first).unwrap();
        assert_eq!(first, [b'{']);
        ledger.verify_content(&[]).unwrap();
        assert_eq!(cloned.stream_position().unwrap(), 1);
        assert_eq!(ledger.reserve(ordinal as u64).unwrap(), ordinal);
        assert_eq!(cloned.stream_position().unwrap(), 1);
        let reservation = bounded_json_bytes(
            &Record::Reserved {
                ordinal,
                request_id: ordinal as u64,
            },
            RECORD_BYTES,
        )
        .unwrap();
        previous.extend_from_slice(&reservation);
        previous.push(b'\n');
        assert_eq!(std::fs::read(&file).unwrap(), previous);
        let terminal = Some(ProviderTerminalEvidence {
            response_id: format!("synthetic:/é {ordinal}"),
            reported_model: Some("gpt-6-luna".into()),
            status: "completed".into(),
        });
        cloned.seek(SeekFrom::Start(0)).unwrap();
        ledger
            .joined(ordinal, terminal.clone(), Some(known()))
            .unwrap();
        assert_eq!(cloned.stream_position().unwrap(), 0);
        let join = bounded_json_bytes(
            &Record::Joined {
                ordinal,
                terminal,
                usage: Some(known()),
            },
            RECORD_BYTES,
        )
        .unwrap();
        previous.extend_from_slice(&join);
        previous.push(b'\n');
        assert_eq!(std::fs::read(&file).unwrap(), previous);
        assert_eq!(acknowledged(&ledger), previous);
        ledger.verify_content(&[]).unwrap();
    }
    let history = ledger.lock().unwrap().history();
    release(ledger);
    assert_eq!(
        reconstruct_provider_history(&path, endpoint()).unwrap(),
        history
    );
    assert_eq!(std::fs::read(&file).unwrap(), previous);
}

#[test]
fn noncanonical_recovered_bytes_remain_exact_not_reserialized() {
    let path = storage();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    release(ledger);
    let file = path.join("provider-attempts.jsonl");
    let canonical = std::fs::read(&file).unwrap();
    let mut baseline = b" \t".to_vec();
    baseline.extend_from_slice(&canonical[..canonical.len() - 1]);
    baseline.extend_from_slice(b" \t\n");
    std::fs::write(&file, &baseline).unwrap();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    assert_eq!(acknowledged(&ledger.lock().unwrap()), baseline);
    ledger.lock().unwrap().reserve(1).unwrap();
    assert!(std::fs::read(&file).unwrap().starts_with(&baseline));
    release(ledger);
    assert_eq!(
        reconstruct_provider_history(&path, endpoint())
            .unwrap()
            .consumed(),
        1
    );
}

#[test]
fn changed_acknowledged_content_poisoning_prevents_repair_and_retry() {
    for join in [false, true] {
        for mutation in 0..4 {
            let path = storage();
            let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
            ledger.lock().unwrap().reserve(1).unwrap();
            let file = path.join("provider-attempts.jsonl");
            let original = std::fs::read(&file).unwrap();
            let mut changed = original.clone();
            match mutation {
                0 => {
                    let index = changed.iter().position(|byte| *byte == b'3').unwrap();
                    changed[index] = b'2';
                }
                1 => {
                    changed.pop();
                }
                2 => changed.extend_from_slice(b" \n"),
                3 => {
                    let index = changed.iter().rposition(|byte| *byte == b'1').unwrap();
                    changed[index] = b'2';
                }
                _ => unreachable!(),
            }
            std::fs::write(&file, &changed).unwrap();
            let mut owner = ledger.lock().unwrap();
            let result = if join {
                owner.joined(1, None, None)
            } else {
                owner.reserve(2).map(|_| ())
            };
            assert!(result.is_err());
            assert_eq!(std::fs::read(&file).unwrap(), changed);
            assert_eq!(owner.history().consumed(), 1);
            assert!(!owner.history().attempts[0].locally_joined);
            assert!(!owner.eligible());
            std::fs::write(&file, &original).unwrap();
            assert!(owner.reserve(2).is_err());
            assert!(owner.joined(1, None, None).is_err());
            assert_eq!(std::fs::read(&file).unwrap(), original);
            drop(owner);
            release(ledger);
        }
    }
}

#[test]
fn same_bytes_under_replaced_named_identity_never_acknowledge_an_append() {
    for replace_directory in [false, true] {
        let path = storage();
        let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
        ledger.lock().unwrap().reserve(1).unwrap();
        let file = path.join("provider-attempts.jsonl");
        let original = std::fs::read(&file).unwrap();
        if replace_directory {
            std::fs::rename(&path, path.with_extension("old")).unwrap();
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        } else {
            std::fs::rename(&file, path.join("old-history")).unwrap();
        }
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
            .unwrap()
            .write_all(&original)
            .unwrap();
        let mut owner = ledger.lock().unwrap();
        assert!(owner.reserve(2).is_err());
        assert!(!owner.eligible());
        assert_eq!(std::fs::read(&file).unwrap(), original);
        let mut held = Vec::new();
        let mut clone = owner.file.try_clone().unwrap();
        clone.seek(SeekFrom::Start(0)).unwrap();
        clone.read_to_end(&mut held).unwrap();
        assert_eq!(held, original);
        drop(owner);
        release(ledger);
    }
}

#[test]
fn failed_positional_write_keeps_consumed_facts_and_poisoned_owner() {
    let path = storage();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    ledger.lock().unwrap().reserve(1).unwrap();
    let file = path.join("provider-attempts.jsonl");
    let original = std::fs::read(&file).unwrap();
    let mut owner = ledger.lock().unwrap();
    owner.force_write_error();
    assert!(owner.joined(1, None, None).is_err());
    assert!(!owner.eligible());
    assert_eq!(owner.history().consumed(), 1);
    assert!(!owner.history().attempts[0].locally_joined);
    assert_eq!(std::fs::read(&file).unwrap(), original);
    drop(owner);
    release(ledger);
    let history = reconstruct_provider_history(&path, endpoint()).unwrap();
    assert_eq!(history.consumed(), 1);
    assert_eq!(history.unresolved_remote(), 1);
}

#[test]
fn ambiguous_append_failpoints_never_advance_live_acknowledgement_or_retry() {
    for join in [false, true] {
        for fault in [
            AppendFault::PartialWrite,
            AppendFault::Sync,
            AppendFault::Content,
            AppendFault::Binding,
        ] {
            let path = storage();
            let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
            ledger.lock().unwrap().reserve(1).unwrap();
            let file = path.join("provider-attempts.jsonl");
            let original = std::fs::read(&file).unwrap();
            let mut owner = ledger.lock().unwrap();
            owner.append_fault = Some(fault);
            let result = if join {
                owner.joined(
                    1,
                    Some(ProviderTerminalEvidence {
                        response_id: "synthetic:/é".into(),
                        reported_model: None,
                        status: "completed".into(),
                    }),
                    Some(known()),
                )
            } else {
                owner.reserve(2).map(|_| ())
            };
            assert!(result.is_err());
            assert_eq!(acknowledged(&owner), original);
            assert_eq!(owner.history().consumed(), 1);
            assert!(!owner.history().attempts[0].locally_joined);
            assert_eq!(owner.history().unresolved_remote(), 1);
            assert_eq!(owner.history().unknown_usage(), 1);
            assert!(!owner.eligible());
            let observed_file = if fault == AppendFault::Binding {
                path.join("lost-binding.jsonl")
            } else {
                file.clone()
            };
            let after = std::fs::read(&observed_file).unwrap();
            assert!(owner.reserve(2).is_err());
            assert!(owner.joined(1, None, None).is_err());
            assert_eq!(std::fs::read(&observed_file).unwrap(), after);
            drop(owner);
            release(ledger);
            let recovered = reconstruct_provider_history(&path, endpoint());
            if fault == AppendFault::Sync {
                let observed = recovered.unwrap();
                if join {
                    assert_eq!(observed.consumed(), 1);
                    assert!(observed.attempts[0].locally_joined);
                    assert_eq!(observed.attempts[0].usage, Some(known()));
                } else {
                    assert_eq!(observed.consumed(), 2);
                    assert!(!observed.attempts[1].locally_joined);
                }
            } else {
                assert!(recovered.is_err());
            }
            assert_eq!(std::fs::read(&observed_file).unwrap(), after);
        }
    }
}

fn validation_baseline() -> (std::path::PathBuf, [u8; 32], Vec<u8>) {
    let path = storage();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    for id in 1..=4 {
        let mut owner = ledger.lock().unwrap();
        let ordinal = owner.reserve(id).unwrap();
        owner.joined(ordinal, None, None).unwrap();
    }
    release(ledger);
    let bytes = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
    let digest = Sha256::digest(&bytes).into();
    (path, digest, bytes)
}
#[test]
fn checked_validation_baseline_charges_only_two_and_never_resumes_suffix() {
    let (path, digest, baseline) = validation_baseline();
    let ledger = ProviderLedger::validation_open(&path, endpoint(), digest).unwrap();
    let checkpoint = ledger.lock().unwrap().validation_checkpoint().unwrap();
    assert!(ProviderLedger::validation_open(&path, endpoint(), digest).is_err());
    assert!(observe_validation(&path, &checkpoint).is_err());
    for id in 5..=6 {
        let mut owner = ledger.lock().unwrap();
        assert!(owner.eligible());
        assert_eq!(owner.reserve(id).unwrap(), id as usize);
        assert!(!owner.eligible());
        let reserved = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
        assert!(reserved.starts_with(&baseline));
        owner
            .joined(
                id as usize,
                Some(ProviderTerminalEvidence {
                    response_id: format!("synthetic-:/é-{id}"),
                    reported_model: Some("gpt-6-luna".into()),
                    status: "completed".into(),
                }),
                Some(known()),
            )
            .unwrap();
        let (observed, actual, length) = owner.validation_observe().unwrap();
        assert_eq!(observed.consumed(), id as usize);
        assert_eq!(observed.unresolved_remote(), 4);
        assert_eq!(observed.unknown_usage(), 4);
        let bytes = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
        assert_eq!(actual, Sha256::digest(&bytes)[..]);
        assert_eq!(length, bytes.len());
        assert!(bytes.starts_with(&reserved));
    }
    assert!(!ledger.lock().unwrap().eligible());
    assert!(ledger.lock().unwrap().reserve(7).is_err());
    let expected = ledger.lock().unwrap().history();
    release(ledger);
    let after = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
    assert!(ProviderLedger::validation_open(&path, endpoint(), digest).is_err());
    assert!(reconstruct_provider_history(&path, endpoint()).is_err());
    assert_eq!(observe_validation(&path, &checkpoint).unwrap(), expected);
    assert_eq!(
        after,
        std::fs::read(path.join("provider-attempts.jsonl")).unwrap()
    );
}
#[test]
fn validation_metadata_expiry_is_checked_again_at_reservation() {
    let (path, digest, baseline) = validation_baseline();
    let ledger = ProviderLedger::validation_open(&path, endpoint(), digest).unwrap();
    let mut owner = ledger.lock().unwrap();
    owner
        .validation_metadata_expiry(std::time::Instant::now() + std::time::Duration::from_secs(5))
        .unwrap();
    assert!(!owner.validation_metadata_expired());
    owner
        .validation_metadata_expiry(std::time::Instant::now())
        .unwrap();
    assert!(owner.validation_metadata_expired());
    assert!(owner.reserve(5).is_err());
    assert_eq!(owner.history_ref().consumed(), 4);
    assert_eq!(
        std::fs::read(path.join("provider-attempts.jsonl")).unwrap(),
        baseline
    );
    drop(owner);
    release(ledger);
    let ordinary = ProviderLedger::open(&storage(), endpoint()).unwrap();
    let mut owner = ordinary.lock().unwrap();
    assert!(
        owner
            .validation_metadata_expiry(std::time::Instant::now())
            .is_err()
    );
    assert!(!owner.validation_metadata_expired());
    assert_eq!(owner.reserve(1).unwrap(), 1);
}

#[test]
fn validation_cancellation_is_checked_again_at_reservation() {
    let (path, digest, baseline) = validation_baseline();
    let ledger = ProviderLedger::validation_open(&path, endpoint(), digest).unwrap();
    let flag = Arc::new(AtomicBool::new(false));
    let mut owner = ledger.lock().unwrap();
    owner.validation_cancellation(flag.clone()).unwrap();
    assert!(owner.eligible());
    flag.store(true, Ordering::SeqCst);
    assert!(!owner.eligible());
    assert!(owner.reserve(5).is_err());
    assert_eq!(
        std::fs::read(path.join("provider-attempts.jsonl")).unwrap(),
        baseline
    );
}
#[test]
fn validation_reserve_itself_freezes_every_new_unknown_obligation() {
    for usage in [
        None,
        Some(ProviderUsage {
            input_tokens: Some(3),
            output_tokens: None,
            total_tokens: None,
            cached_input_tokens: None,
            reasoning_tokens: None,
        }),
    ] {
        let (path, digest, baseline) = validation_baseline();
        let ledger = ProviderLedger::validation_open(&path, endpoint(), digest).unwrap();
        let mut owner = ledger.lock().unwrap();
        owner.reserve(5).unwrap();
        assert!(!owner.eligible());
        assert!(owner.reserve(6).is_err());
        owner
            .joined(
                5,
                Some(ProviderTerminalEvidence {
                    response_id: "synthetic-response".into(),
                    reported_model: None,
                    status: "completed".into(),
                }),
                usage.clone(),
            )
            .unwrap();
        assert!(!owner.eligible());
        assert!(owner.reserve(6).is_err());
        assert_eq!(owner.history().attempts[4].usage, usage);
        assert!(owner.history().has_unknown_after(4));
        let checkpoint = owner.validation_checkpoint().unwrap();
        drop(owner);
        release(ledger);
        assert!(
            observe_validation(&path, &checkpoint)
                .unwrap()
                .has_unknown_after(4)
        );
        let bytes = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
        assert!(bytes.starts_with(&baseline));
        let mut invalid = bytes.clone();
        invalid.extend_from_slice(b"{\"kind\":\"Reserved\",\"ordinal\":6,\"request_id\":6}\n");
        std::fs::write(path.join("provider-attempts.jsonl"), &invalid).unwrap();
        assert!(observe_validation(&path, &checkpoint).is_err());
        assert_eq!(
            std::fs::read(path.join("provider-attempts.jsonl")).unwrap(),
            invalid
        );
    }
}
#[test]
fn validation_existing_only_hash_accounting_and_scoped_identity_fail_closed() {
    let empty = storage();
    assert!(ProviderLedger::validation_open(&empty, endpoint(), [0; 32]).is_err());
    assert!(!empty.join("provider-attempts.jsonl").exists());
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(empty.join("provider-attempts.jsonl"))
        .unwrap();
    assert!(ProviderLedger::validation_open(&empty, endpoint(), [0; 32]).is_err());
    assert_eq!(
        std::fs::metadata(empty.join("provider-attempts.jsonl"))
            .unwrap()
            .len(),
        0
    );
    for change in 0..4 {
        let (path, digest, original) = validation_baseline();
        let file = path.join("provider-attempts.jsonl");
        let mut bytes = original.clone();
        match change {
            0 => bytes.push(b' '),
            1 => {
                bytes.truncate(bytes.len() - 1);
            }
            2 => {
                let n = bytes.iter().position(|b| *b == b'3').unwrap();
                bytes[n] = b'2';
            }
            3 => {
                let index = std::str::from_utf8(&bytes)
                    .unwrap()
                    .find("\"terminal\":null")
                    .unwrap();
                bytes.splice(index..index+15,b"\"terminal\":{\"response_id\":\"known\",\"reported_model\":null,\"status\":\"completed\"}".iter().copied());
            }
            _ => unreachable!(),
        }
        std::fs::write(&file, &bytes).unwrap();
        assert!(ProviderLedger::validation_open(&path, endpoint(), digest).is_err());
        assert!(
            ProviderLedger::validation_open(&path, endpoint(), Sha256::digest(&bytes).into())
                .is_err()
        );
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
    let (path, digest, bytes) = validation_baseline();
    let ledger = ProviderLedger::validation_open(&path, endpoint(), digest).unwrap();
    let checkpoint = ledger.lock().unwrap().validation_checkpoint().unwrap();
    release(ledger);
    let file = path.join("provider-attempts.jsonl");
    std::fs::rename(&file, path.join("old")).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
    assert!(observe_validation(&path, &checkpoint).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
}

#[test]
fn descriptor_owner_permissions_and_lock_fail_closed() {
    let path = storage();
    let ledger = ProviderLedger::open(&path, endpoint()).unwrap();
    assert!(ProviderLedger::open(&path, endpoint()).is_err());
    assert!(reconstruct_provider_history(&path, endpoint()).is_err());
    release(ledger);
    let file = path.join("provider-attempts.jsonl");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(ProviderLedger::open(&path, endpoint()).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = path.join("alias");
    std::fs::hard_link(&file, &link).unwrap();
    assert!(ProviderLedger::open(&path, endpoint()).is_err());
    let path = storage();
    let link = path.join("provider-attempts.jsonl");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(ProviderLedger::open(&path, endpoint()).is_err());
}
