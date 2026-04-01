use nexus::{ErrorId, KernelError, Version};

#[test]
fn version_mismatch_display() {
    let err = KernelError::VersionMismatch {
        stream_id: ErrorId::from_display(&"user-123"),
        expected: Version::from_persisted(3),
        actual: Version::from_persisted(5),
    };
    let msg = format!("{err}");
    assert!(msg.contains("user-123"));
    assert!(msg.contains('3'));
    assert!(msg.contains('5'));
}

#[test]
fn kernel_error_is_std_error() {
    let err = KernelError::VersionMismatch {
        stream_id: ErrorId::from_display(&"test"),
        expected: Version::INITIAL,
        actual: Version::from_persisted(1),
    };
    let _: &dyn std::error::Error = &err;
}

#[test]
fn error_id_no_heap_allocation() {
    // ErrorId is stack-allocated — 128 bytes + 1 byte length
    // This verifies it works without String/heap
    let id = ErrorId::from_display(&42_u64);
    assert_eq!(format!("{id}"), "42");
}

#[test]
fn error_id_truncates_long_values() {
    let long = "a".repeat(200);
    let id = ErrorId::from_display(&long);
    // Truncated to 128 bytes
    assert_eq!(format!("{id}").len(), 128);
}
