use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
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
        assert!(!ledger.lock().unwrap().eligible());
        assert!(ledger.lock().unwrap().reserve(5).is_err());
        let expected = ledger.lock().unwrap().history();
        release(ledger);
        let recovered = reconstruct_provider_history(&path, endpoint()).unwrap();
        assert_eq!(recovered, expected);
        assert_eq!(recovered.unknown_usage(), 4);
        assert_eq!(
            recovered.unresolved_remote(),
            if unknown_remote { 4 } else { 0 }
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
