#![cfg(feature = "snapshot")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::as_conversions,
    reason = "test harness — relaxed lints for test code"
)]

use nexus::Version;
use nexus_store::snapshot::{PendingSnapshot, PersistedSnapshot};

#[test]
fn pending_snapshot_stores_version_and_payload() {
    let version = Version::new(42).unwrap();
    let payload = vec![1, 2, 3];
    let snap = PendingSnapshot::new(version, 1, payload.clone());

    assert_eq!(snap.version(), version);
    assert_eq!(snap.schema_version(), 1);
    assert_eq!(snap.payload(), &payload);
}

#[test]
fn pending_snapshot_schema_version_must_be_nonzero() {
    let version = Version::new(1).unwrap();
    let result = PendingSnapshot::try_new(version, 0, vec![]);
    assert!(result.is_err());
}

#[test]
fn persisted_snapshot_stores_version_and_payload() {
    let version = Version::new(10).unwrap();
    let snap = PersistedSnapshot::new(version, 2, vec![4, 5, 6]);

    assert_eq!(snap.version(), version);
    assert_eq!(snap.schema_version(), 2);
    assert_eq!(snap.payload(), &[4, 5, 6]);
}
