#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::expect_used, reason = "test code")]

use nexus_fjall::encoding::{encode_event_key, encode_event_value, encode_stream_meta};

fn hex_dump(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Event key snapshots
// ---------------------------------------------------------------------------

#[test]
fn event_key_zero_zero() {
    let encoded = encode_event_key(0, 0);
    insta::assert_snapshot!("event_key_zero_zero", hex_dump(&encoded));
}

#[test]
fn event_key_one_one() {
    let encoded = encode_event_key(1, 1);
    insta::assert_snapshot!("event_key_one_one", hex_dump(&encoded));
}

#[test]
fn event_key_42_7() {
    let encoded = encode_event_key(42, 7);
    insta::assert_snapshot!("event_key_42_7", hex_dump(&encoded));
}

#[test]
fn event_key_max_max() {
    let encoded = encode_event_key(u64::MAX, u64::MAX);
    insta::assert_snapshot!("event_key_max_max", hex_dump(&encoded));
}

#[test]
fn event_key_zero_max() {
    let encoded = encode_event_key(0, u64::MAX);
    insta::assert_snapshot!("event_key_zero_max", hex_dump(&encoded));
}

#[test]
fn event_key_max_zero() {
    let encoded = encode_event_key(u64::MAX, 0);
    insta::assert_snapshot!("event_key_max_zero", hex_dump(&encoded));
}

#[test]
fn event_key_ordering_version() {
    let key_v1 = encode_event_key(1, 1);
    let key_v2 = encode_event_key(1, 2);
    insta::assert_snapshot!("event_key_stream1_version1", hex_dump(&key_v1));
    insta::assert_snapshot!("event_key_stream1_version2", hex_dump(&key_v2));
    assert!(
        key_v1 < key_v2,
        "version 1 must sort before version 2 in byte order"
    );
}

#[test]
fn event_key_stream_grouping() {
    let key_s1_v100 = encode_event_key(1, 100);
    let key_s2_v1 = encode_event_key(2, 1);
    insta::assert_snapshot!("event_key_stream1_version100", hex_dump(&key_s1_v100));
    insta::assert_snapshot!("event_key_stream2_version1", hex_dump(&key_s2_v1));
    assert!(
        key_s1_v100 < key_s2_v1,
        "stream 1 version 100 must sort before stream 2 version 1"
    );
}

// ---------------------------------------------------------------------------
// Stream meta snapshots
// ---------------------------------------------------------------------------

#[test]
fn stream_meta_zero_zero() {
    let encoded = encode_stream_meta(0, 0);
    insta::assert_snapshot!("stream_meta_zero_zero", hex_dump(&encoded));
}

#[test]
fn stream_meta_one_one() {
    let encoded = encode_stream_meta(1, 1);
    insta::assert_snapshot!("stream_meta_one_one", hex_dump(&encoded));
}

#[test]
fn stream_meta_999_5() {
    let encoded = encode_stream_meta(999, 5);
    insta::assert_snapshot!("stream_meta_999_5", hex_dump(&encoded));
}

#[test]
fn stream_meta_max_max() {
    let encoded = encode_stream_meta(u64::MAX, u64::MAX);
    insta::assert_snapshot!("stream_meta_max_max", hex_dump(&encoded));
}

#[test]
fn stream_meta_zero_max() {
    let encoded = encode_stream_meta(0, u64::MAX);
    insta::assert_snapshot!("stream_meta_zero_max", hex_dump(&encoded));
}

#[test]
fn stream_meta_max_zero() {
    let encoded = encode_stream_meta(u64::MAX, 0);
    insta::assert_snapshot!("stream_meta_max_zero", hex_dump(&encoded));
}

// ---------------------------------------------------------------------------
// Event value snapshots
// ---------------------------------------------------------------------------

#[test]
fn event_value_typical() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 1, "UserCreated", b"payload").unwrap();
    insta::assert_snapshot!("event_value_typical", hex_dump(&buf));
}

#[test]
fn event_value_schema_zero_empty_payload() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 0, "Empty", b"").unwrap();
    insta::assert_snapshot!("event_value_schema_zero_empty_payload", hex_dump(&buf));
}

#[test]
fn event_value_max_schema_binary_payload() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, u32::MAX, "X", b"\x00\xff").unwrap();
    insta::assert_snapshot!("event_value_max_schema_binary_payload", hex_dump(&buf));
}

#[test]
fn event_value_empty_event_type() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 1, "", b"data").unwrap();
    insta::assert_snapshot!("event_value_empty_event_type", hex_dump(&buf));
}

#[test]
fn event_value_single_char_type_empty_payload() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 1, "A", b"").unwrap();
    insta::assert_snapshot!("event_value_single_char_type_empty_payload", hex_dump(&buf));
}

#[test]
fn event_value_larger_payload() {
    let mut buf = Vec::new();
    encode_event_value(&mut buf, 42, "OrderPlaced", &[0u8; 1024]).unwrap();
    insta::assert_snapshot!("event_value_larger_payload", hex_dump(&buf));
}
