use nexus::kernel::{KernelError, Version};

#[test]
fn version_mismatch_display() {
    let err = KernelError::VersionMismatch {
        stream_id: String::from("user-123"),
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
        stream_id: String::from("test"),
        expected: Version::INITIAL,
        actual: Version::from_persisted(1),
    };
    let _: &dyn std::error::Error = &err;
}
