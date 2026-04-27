#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::expect_used, reason = "test code")]

use nexus_fjall::encoding::{encode_event_key, encode_event_value, encode_stream_version};

fn hex_dump(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Event key snapshots (new format: [u16 BE id_len][id_bytes][u64 BE version])
// ---------------------------------------------------------------------------

#[test]
fn event_key_empty_id_v0() {
    let encoded = encode_event_key(b"", 0).unwrap();
    insta::assert_snapshot!("event_key_empty_id_v0", hex_dump(&encoded));
}

#[test]
fn event_key_a_v1() {
    let encoded = encode_event_key(b"a", 1).unwrap();
    insta::assert_snapshot!("event_key_a_v1", hex_dump(&encoded));
}

#[test]
fn event_key_stream42_v7() {
    let encoded = encode_event_key(b"stream-42", 7).unwrap();
    insta::assert_snapshot!("event_key_stream42_v7", hex_dump(&encoded));
}

#[test]
fn event_key_empty_id_vmax() {
    let encoded = encode_event_key(b"", u64::MAX).unwrap();
    insta::assert_snapshot!("event_key_empty_id_vmax", hex_dump(&encoded));
}

#[test]
fn event_key_ordering_version() {
    let key_v1 = encode_event_key(b"s1", 1).unwrap();
    let key_v2 = encode_event_key(b"s1", 2).unwrap();
    insta::assert_snapshot!("event_key_s1_v1", hex_dump(&key_v1));
    insta::assert_snapshot!("event_key_s1_v2", hex_dump(&key_v2));
    assert!(
        key_v1 < key_v2,
        "version 1 must sort before version 2 in byte order"
    );
}

#[test]
fn event_key_stream_grouping() {
    let key_foo_v100 = encode_event_key(b"foo", 100).unwrap();
    let key_foobar_v1 = encode_event_key(b"foobar", 1).unwrap();
    insta::assert_snapshot!("event_key_foo_v100", hex_dump(&key_foo_v100));
    insta::assert_snapshot!("event_key_foobar_v1", hex_dump(&key_foobar_v1));
    // Length prefix ensures "foo" range doesn't overlap "foobar"
    assert!(
        key_foo_v100 < key_foobar_v1,
        "shorter ID must sort before longer ID with same prefix"
    );
}

// ---------------------------------------------------------------------------
// Stream version snapshots
// ---------------------------------------------------------------------------

#[test]
fn stream_version_zero() {
    let encoded = encode_stream_version(0);
    insta::assert_snapshot!("stream_version_zero", hex_dump(&encoded));
}

#[test]
fn stream_version_one() {
    let encoded = encode_stream_version(1);
    insta::assert_snapshot!("stream_version_one", hex_dump(&encoded));
}

#[test]
fn stream_version_999() {
    let encoded = encode_stream_version(999);
    insta::assert_snapshot!("stream_version_999", hex_dump(&encoded));
}

#[test]
fn stream_version_max() {
    let encoded = encode_stream_version(u64::MAX);
    insta::assert_snapshot!("stream_version_max", hex_dump(&encoded));
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
