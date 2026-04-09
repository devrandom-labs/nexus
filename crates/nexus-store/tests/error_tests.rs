//! Unit tests for `StoreError` and `UpcastError` Display output and `source()` chain.

#![allow(clippy::unwrap_used, reason = "tests")]

use nexus::{KernelError, Version};
use nexus_store::ToStreamLabel;
use nexus_store::{StoreError, UpcastError};

#[test]
fn conflict_display_contains_stream_id_and_versions() {
    let err = StoreError::Conflict {
        stream_id: "order-42".to_stream_label(),
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
    let err = StoreError::StreamNotFound {
        stream_id: "user-99".to_stream_label(),
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
fn codec_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::InvalidData, "bad json");
    let err = StoreError::Codec(Box::new(inner));
    let msg = format!("{err}");
    assert!(msg.contains("Codec"), "should mention Codec");
    assert!(msg.contains("bad json"), "should contain inner message");
    // Codec has a source chain
    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "Codec variant should have a source");
    let source_msg = format!("{}", source.unwrap());
    assert!(
        source_msg.contains("bad json"),
        "source should contain inner message"
    );
}

#[test]
fn adapter_display_contains_inner_message() {
    let inner = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "db offline");
    let err = StoreError::Adapter(Box::new(inner));
    let msg = format!("{err}");
    assert!(msg.contains("Adapter"), "should mention Adapter");
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
    let store_err: StoreError = kernel_err.into();
    assert!(matches!(store_err, StoreError::Kernel(_)));
    let msg = format!("{store_err}");
    assert!(msg.contains("Kernel"), "should mention Kernel");
    assert!(msg.contains("mismatch"), "should mention mismatch");
}

#[test]
fn kernel_error_has_source_chain() {
    let kernel_err = KernelError::RehydrationLimitExceeded { max: 100 };
    let store_err: StoreError = kernel_err.into();
    let source = std::error::Error::source(&store_err);
    assert!(source.is_some(), "Kernel variant should have a source");
}

#[test]
fn transform_failed_error_displays_context() {
    let err = UpcastError::TransformFailed {
        event_type: "OrderCreated".into(),
        schema_version: Version::INITIAL,
        source: Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad json",
        )),
    };
    let msg = err.to_string();
    assert!(msg.contains("OrderCreated"), "should contain event type");
    assert!(msg.contains("bad json"), "should contain source message");

    let source = std::error::Error::source(&err);
    assert!(source.is_some(), "TransformFailed should have a source");
}
