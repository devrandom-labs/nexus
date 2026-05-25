//! Unit tests for `StoreError` Display output and `source()` chain.

#![allow(clippy::unwrap_used, reason = "tests")]

use arrayvec::ArrayString;
use nexus::{KernelError, Version};
use nexus_store::StoreError;

/// Concrete `StoreError` for tests: adapter = `std::io::Error`, codec = `std::io::Error`,
/// upcaster = `std::io::Error`.
type TestStoreError = StoreError<std::io::Error, std::io::Error, std::io::Error, std::io::Error>;

fn label(s: &str) -> ArrayString<64> {
    ArrayString::try_from(s).unwrap()
}

#[test]
fn conflict_display_contains_stream_id_and_versions() {
    let err: TestStoreError = StoreError::Conflict {
        stream_id: label("order-42"),
        expected: Version::new(3),
        actual: Version::new(5),
    };
    let msg = format!("{err}");
    assert!(msg.contains("order-42"), "should contain stream_id");
    assert!(msg.contains('3'), "should contain expected version");
    assert!(msg.contains('5'), "should contain actual version");
    assert!(msg.contains("conflict"), "should mention conflict");
    // Conflict has no source error
    assert!(
        std::error::Error::source(&err).is_none(),
        "Conflict variant should have no source"
    );
}

#[test]
fn stream_not_found_display_contains_stream_id() {
    let err: TestStoreError = StoreError::StreamNotFound {
        stream_id: label("user-99"),
    };
    let msg = format!("{err}");
    assert!(msg.contains("user-99"), "should contain stream_id");
    assert!(msg.contains("not found"), "should mention 'not found'");
    // StreamNotFound has no source error
    assert!(
        std::error::Error::source(&err).is_none(),
        "StreamNotFound variant should have no source"
    );
}

#[test]
fn encode_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad json");
    let err: TestStoreError = StoreError::Encode(inner);
    let msg = format!("{err}");
    assert!(msg.contains("encode"), "should mention encode");
    assert!(msg.contains("bad json"), "should contain inner message");
    // Encode has a source chain
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "Encode variant should have a source");
    let source_msg = format!("{}", source.unwrap());
    assert!(
        source_msg.contains("bad json"),
        "source should contain inner message"
    );
}

#[test]
fn decode_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad json");
    let err: TestStoreError = StoreError::Decode(inner);
    let msg = format!("{err}");
    assert!(msg.contains("decode"), "should mention decode");
    assert!(msg.contains("bad json"), "should contain inner message");
    // Decode has a source chain
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "Decode variant should have a source");
    let source_msg = format!("{}", source.unwrap());
    assert!(
        source_msg.contains("bad json"),
        "source should contain inner message"
    );
}

#[test]
fn adapter_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "db offline");
    let err: TestStoreError = StoreError::Adapter(inner);
    let msg = format!("{err}");
    assert!(msg.contains("adapter"), "should mention adapter");
    assert!(msg.contains("db offline"), "should contain inner message");
    // Adapter has a source chain
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "Adapter variant should have a source");
    let source_msg = format!("{}", source.unwrap());
    assert!(
        source_msg.contains("db offline"),
        "source should contain inner message"
    );
}

#[test]
fn kernel_error_converts_to_store_error() {
    let kernel_err = KernelError::VersionMismatch {
        expected: Version::INITIAL,
        actual: Version::new(2).unwrap(),
    };
    let store_err: TestStoreError = kernel_err.into();
    assert!(matches!(store_err, StoreError::Kernel(_)));
    let msg = format!("{store_err}");
    assert!(msg.contains("kernel"), "should mention kernel");
    assert!(msg.contains("mismatch"), "should mention mismatch");
}

#[test]
fn kernel_error_has_source_chain() {
    let kernel_err = KernelError::RehydrationLimitExceeded { max: 100 };
    let store_err: TestStoreError = kernel_err.into();
    let source = std::error::Error::source(&store_err);
    assert!(source.is_some(), "Kernel variant should have a source");
}

#[test]
fn upcast_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad transform");
    let err: TestStoreError = StoreError::Upcast(inner);
    let msg = format!("{err}");
    assert!(msg.contains("upcast"), "should mention upcast");
    assert!(
        msg.contains("bad transform"),
        "should contain inner message"
    );
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "Upcast variant should have a source");
}
