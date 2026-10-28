use super::*;
use crate::provider_fixture as fixture;
#[test]
fn bounded_single_buffer_overflow_is_explicit_and_does_not_clip_to_full() {
    let mut capture = ProviderCapture::new(RequestId(1), Some(1));
    let chunk = vec![b'x'; 8192];
    let pointer = capture.bytes.as_ptr();
    for _ in 0..16 {
        capture.receive(&chunk);
    }
    assert_eq!(capture.bytes.len(), PROVIDER_CAPTURE_BYTES);
    assert_eq!(capture.bytes.capacity(), PROVIDER_CAPTURE_BYTES);
    assert_eq!(capture.bytes.as_ptr(), pointer);
    assert!(!capture.overflow);
    capture.receive(b"extra");
    assert!(capture.overflow);
    assert_eq!(capture.received_bytes, PROVIDER_CAPTURE_BYTES + 5);
    assert_eq!(capture.bytes.len(), PROVIDER_CAPTURE_BYTES);
    assert!(!capture.body_eof);
    capture.receive(b"more");
    assert_eq!(capture.received_bytes, PROVIDER_CAPTURE_BYTES + 9);
    assert_eq!(capture.bytes.as_ptr(), pointer);
}
#[test]
fn replay_exact_frame_budget_and_aggregate_budget_preserve_parser_rules() {
    let terminal = fixture::wire(&fixture::text_events("hé🙂", fixture::known_usage()), false);
    let mut bytes = vec![b'x'; crate::PROVIDER_FRAME_BYTES];
    bytes[0] = b':';
    let len = bytes.len();
    bytes[len - 2..].copy_from_slice(b"\n\n");
    bytes.extend_from_slice(&terminal);
    assert_eq!(replay_provider_data(&bytes).outcome, Outcome::Success);
    bytes.insert(1, b'x');
    assert_eq!(replay_provider_data(&bytes).outcome, Outcome::ResourceLimit);
    let mut bytes = Vec::with_capacity(PROVIDER_CAPTURE_BYTES);
    let mut padding = PROVIDER_CAPTURE_BYTES - terminal.len();
    while padding > 0 {
        let size = padding.min(crate::PROVIDER_FRAME_BYTES);
        let size = if padding > size && padding - size < 3 {
            size - 3
        } else {
            size
        };
        bytes.push(b':');
        bytes.extend(std::iter::repeat_n(b'x', size - 3));
        bytes.extend_from_slice(b"\n\n");
        padding -= size;
    }
    bytes.extend_from_slice(&terminal);
    assert_eq!(bytes.len(), PROVIDER_CAPTURE_BYTES);
    assert_eq!(replay_provider_data(&bytes).outcome, Outcome::Success);
    bytes.push(b'\n');
    assert_eq!(replay_provider_data(&bytes).outcome, Outcome::ResourceLimit);
}
