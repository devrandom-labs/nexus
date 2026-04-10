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

use std::fmt;

use nexus::{Id, Version};
use nexus_store::snapshot::{PendingSnapshot, PersistedSnapshot, SnapshotStore};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Id for TestId {}

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

// ── () no-op SnapshotStore ─────────────────────────────────────────

#[tokio::test]
async fn unit_snapshot_store_returns_none() {
    let store = ();
    let id = TestId("agg-1".into());
    let result = store.load_snapshot(&id).await;
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn unit_snapshot_store_save_succeeds() {
    let store = ();
    let id = TestId("agg-1".into());
    let snap = PendingSnapshot::new(Version::new(1).unwrap(), 1, vec![1, 2, 3]);
    let result = store.save_snapshot(&id, &snap).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn unit_snapshot_store_delete_succeeds() {
    let store = ();
    let id = TestId("agg-1".into());
    let result = store.delete_snapshot(&id).await;
    assert!(result.is_ok());
}
