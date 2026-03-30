use nexus::kernel::{KernelError, Version};

#[test]
fn version_mismatch_display() {
    let err = KernelError::VersionMismatch {
        stream_id: "user-123".to_string(),
        expected: Version::from(3u64),
        actual: Version::from(5u64),
    };
    let msg = format!("{err}");
    assert!(msg.contains("user-123"));
    assert!(msg.contains("3"));
    assert!(msg.contains("5"));
}

#[test]
fn kernel_error_is_std_error() {
    let err = KernelError::VersionMismatch {
        stream_id: "test".to_string(),
        expected: Version::INITIAL,
        actual: Version::from(1u64),
    };
    let _: &dyn std::error::Error = &err;
}
