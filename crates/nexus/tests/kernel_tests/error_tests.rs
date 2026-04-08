use nexus::{KernelError, Version};

#[test]
fn version_mismatch_display_exact() {
    let err = KernelError::VersionMismatch {
        expected: Version::new(3).unwrap(),
        actual: Version::new(5).unwrap(),
    };
    assert_eq!(format!("{err}"), "Version mismatch: expected 3, got 5");
}

#[test]
fn version_mismatch_display_with_initial() {
    let err = KernelError::VersionMismatch {
        expected: Version::INITIAL,
        actual: Version::new(100).unwrap(),
    };
    assert_eq!(format!("{err}"), "Version mismatch: expected 1, got 100");
}

#[test]
fn rehydration_limit_exceeded_display() {
    let err = KernelError::RehydrationLimitExceeded { max: 1_000_000 };
    assert_eq!(
        format!("{err}"),
        "Rehydration limit exceeded: max 1000000 events"
    );
}

#[test]
fn version_overflow_display() {
    let err = KernelError::VersionOverflow;
    assert_eq!(
        format!("{err}"),
        "Version sequence exhausted: cannot exceed u64::MAX events"
    );
}

#[test]
fn kernel_error_implements_std_error() {
    let err = KernelError::VersionMismatch {
        expected: Version::INITIAL,
        actual: Version::new(2).unwrap(),
    };
    let _: &dyn std::error::Error = &err;
}

#[test]
fn kernel_error_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<KernelError>();
}
