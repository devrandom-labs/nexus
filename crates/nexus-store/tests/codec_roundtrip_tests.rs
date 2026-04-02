//! Codec roundtrip tests using serde_json.
//!
//! These tests demonstrate the encode/decode identity property: for any event `e`,
//! `decode(encode(e)) == e`.
//!
//! # Design note on the `Codec` trait
//!
//! The `Codec` trait's methods are generic over `E: DomainEvent`, but `DomainEvent`
//! does not require `Serialize + DeserializeOwned`. This means a generic JSON codec
//! cannot call `serde_json::to_vec(event)` inside the trait implementation because
//! the trait bound is insufficient.
//!
//! Real-world codecs will either:
//! - Use `Any` downcasting (complex, fragile)
//! - Require a wrapper trait that adds `Serialize + DeserializeOwned` bounds
//! - Work with a concrete event type
//!
//! These tests use standalone encode/decode functions that mirror the pattern a
//! real codec implementation would follow, testing the serde roundtrip identity
//! property that underpins any JSON-based `Codec` implementation.

#![allow(
    clippy::unwrap_used,
    reason = "tests use unwrap for clarity and brevity"
)]
#![allow(
    clippy::expect_used,
    reason = "tests use expect for clarity and brevity"
)]
#![allow(clippy::str_to_string, reason = "tests use to_string/to_owned freely")]
#![allow(
    clippy::shadow_reuse,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::shadow_unrelated,
    reason = "tests shadow variables for readability"
)]
#![allow(
    clippy::float_cmp,
    reason = "tests compare exact f64 values through serde roundtrip"
)]

use nexus::{DomainEvent, Message};
use serde::{Deserialize, Serialize};

/// A test event enum with three variant shapes to exercise serde's enum representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    /// Unit variant — no payload.
    UnitVariant,
    /// Tuple variant — positional fields.
    TupleVariant(String, u64),
    /// Struct variant — named fields.
    StructVariant { name: String, value: f64 },
}

impl Message for TestEvent {}

impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::UnitVariant => "UnitVariant",
            Self::TupleVariant(..) => "TupleVariant",
            Self::StructVariant { .. } => "StructVariant",
        }
    }
}

/// Encode a `TestEvent` to JSON bytes, mirroring what a JSON `Codec::encode` would do.
fn encode_test_event(event: &TestEvent) -> Vec<u8> {
    serde_json::to_vec(event).expect("TestEvent should always serialize to JSON")
}

/// Decode JSON bytes back to a `TestEvent`, mirroring what a JSON `Codec::decode` would do.
///
/// # Errors
///
/// Returns `serde_json::Error` if the payload is not valid JSON or does not match
/// the `TestEvent` schema.
fn decode_test_event(payload: &[u8]) -> Result<TestEvent, serde_json::Error> {
    serde_json::from_slice(payload)
}

#[test]
fn roundtrip_unit_variant() {
    let original = TestEvent::UnitVariant;
    let bytes = encode_test_event(&original);
    let decoded = decode_test_event(&bytes).unwrap();
    assert_eq!(original, decoded);
    assert_eq!(original.name(), "UnitVariant");
}

#[test]
fn roundtrip_tuple_variant() {
    let original = TestEvent::TupleVariant("hello".to_owned(), 42);
    let bytes = encode_test_event(&original);
    let decoded = decode_test_event(&bytes).unwrap();
    assert_eq!(original, decoded);
    assert_eq!(original.name(), "TupleVariant");
}

#[test]
fn roundtrip_struct_variant() {
    let original = TestEvent::StructVariant {
        name: "temperature".to_owned(),
        value: 98.6,
    };
    let bytes = encode_test_event(&original);
    let decoded = decode_test_event(&bytes).unwrap();
    assert_eq!(original, decoded);
    assert_eq!(original.name(), "StructVariant");
}

#[test]
fn decode_corrupted_payload() {
    let garbage: &[u8] = &[0xFF, 0xFE, 0x00, 0x01, 0xDE, 0xAD];
    let result = decode_test_event(garbage);
    assert!(result.is_err(), "corrupted payload should fail to decode");
}

#[test]
fn decode_wrong_format() {
    // Encode a StructVariant, then try to decode the raw JSON.
    // serde_json decodes based on the JSON content, not an external type hint.
    // So passing a "wrong" event_type string wouldn't matter to serde_json —
    // it deserializes purely from the payload structure.
    //
    // However, if we fabricate JSON that is valid JSON but does not match any
    // TestEvent variant, serde will reject it.
    let original = TestEvent::StructVariant {
        name: "test".to_owned(),
        value: 1.0,
    };
    let bytes = encode_test_event(&original);

    // The encoded bytes are valid JSON for a StructVariant. Decoding succeeds
    // regardless of what "event_type" hint we might pass, because serde_json
    // determines the variant from the JSON structure itself.
    let decoded = decode_test_event(&bytes).unwrap();
    assert_eq!(original, decoded);

    // Now test with JSON that is valid but represents an unknown shape.
    // For example, an object with a key that is not a valid TestEvent variant.
    let wrong_json = br#"{"UnknownVariant": {"x": 1}}"#;
    let result = decode_test_event(wrong_json);
    assert!(
        result.is_err(),
        "JSON with an unknown variant key should fail to deserialize into TestEvent. \
         serde_json uses the externally-tagged enum representation by default, so \
         the variant name in JSON must match a variant of TestEvent."
    );
}
