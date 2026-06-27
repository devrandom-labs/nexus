//! Unit tests for `StoreError` Display output and `source()` chain.

#![allow(clippy::unwrap_used, reason = "tests")]

use nexus::{ErrorId, KernelError, Version};
use nexus_store::{LoadWithError, StoreError};

/// Concrete `StoreError` for tests: adapter = `std::io::Error`, codec = `std::io::Error`.
type TestStoreError = StoreError<std::io::Error, std::io::Error, std::io::Error>;

/// Concrete `LoadWithError` for tests: same as `TestStoreError` plus a
/// user-supplied upcaster error type.
type TestLoadWithError =
    LoadWithError<std::io::Error, std::io::Error, std::io::Error, std::io::Error>;

fn label(s: &str) -> ErrorId {
    ErrorId::from_display(&s)
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
fn is_conflict_true_only_for_conflict_variant() {
    let conflict: TestStoreError = StoreError::Conflict {
        stream_id: label("order-42"),
        expected: Version::new(3),
        actual: Version::new(5),
    };
    assert!(
        conflict.is_conflict(),
        "Conflict variant must be a conflict"
    );

    let not_found: TestStoreError = StoreError::StreamNotFound {
        stream_id: label("user-99"),
    };
    assert!(!not_found.is_conflict(), "StreamNotFound is not a conflict");

    let decode: TestStoreError = StoreError::Decode(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "bad bytes",
    ));
    assert!(!decode.is_conflict(), "Decode is not a conflict");

    let overflow: TestStoreError = StoreError::VersionOverflow;
    assert!(!overflow.is_conflict(), "VersionOverflow is not a conflict");
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
fn load_with_upcast_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad transform");
    let err: TestLoadWithError = LoadWithError::Upcast(inner);
    let msg = format!("{err}");
    assert!(msg.contains("upcast"), "should mention upcast");
    assert!(
        msg.contains("bad transform"),
        "should contain inner message"
    );
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "Upcast variant should have a source");
}

// ── Defensive boundary (PR2 #208): over-long stream ids are truncated and
//    visually signalled with a trailing `…` in error Display output. The
//    `stream_id` field is now `nexus::ErrorId`, whose `from_display`
//    constructor caps at 64 bytes on a char boundary and appends the marker.

#[test]
fn conflict_display_truncates_overlong_stream_id_with_ellipsis() {
    // 100 bytes of input against ErrorId's 64-byte cap.
    let long = "s".repeat(100);
    let err: TestStoreError = StoreError::Conflict {
        stream_id: ErrorId::from_display(&long),
        expected: Version::new(3),
        actual: Version::new(5),
    };
    let msg = format!("{err}");
    // The full over-long id must NOT appear verbatim …
    assert!(
        !msg.contains(&long),
        "the untruncated stream id must not be rendered"
    );
    // … the loss is signalled with the ellipsis marker …
    assert!(
        msg.contains('…'),
        "truncation must be visually signalled with '…'"
    );
    // … and the surviving 61-byte prefix (64 cap − 3-byte '…') renders.
    assert!(
        msg.contains(&"s".repeat(61)),
        "the truncated prefix should still be present"
    );
    // Versions still render.
    assert!(msg.contains('3') && msg.contains('5'));
}

#[test]
fn stream_not_found_display_truncates_overlong_stream_id_with_ellipsis() {
    let long = "u".repeat(200);
    let err: TestStoreError = StoreError::StreamNotFound {
        stream_id: ErrorId::from_display(&long),
    };
    let msg = format!("{err}");
    assert!(
        !msg.contains(&long),
        "the untruncated stream id must not be rendered"
    );
    assert!(
        msg.contains('…'),
        "truncation must be visually signalled with '…'"
    );
    assert!(msg.contains("not found"), "should mention 'not found'");
}

#[test]
fn load_with_store_variant_wraps_store_error() {
    // The `#[from] StoreError` impl lets `?` promote a StoreError into
    // LoadWithError inside a load_with body.
    let inner = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "db offline");
    let store_err: TestStoreError = StoreError::Adapter(inner);
    let load_err: TestLoadWithError = store_err.into();
    let msg = format!("{load_err}");
    assert!(
        msg.contains("db offline"),
        "Store variant should forward source"
    );
    // Source chain: LoadWithError -> StoreError -> io::Error
    let source = std::error::Error::source(&load_err);
    assert!(source.is_some(), "Store variant should have a source");
}
