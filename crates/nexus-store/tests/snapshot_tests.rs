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

use std::num::NonZeroU64;

use nexus::{Id, Version};
use nexus_store::snapshot::{
    AfterEventTypes, EveryNEvents, PendingSnapshot, PersistedSnapshot, SnapshotStore,
    SnapshotTrigger,
};

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

// ── EveryNEvents ────────────────────────────────────────────────────

#[test]
fn every_n_events_triggers_on_boundary_crossing() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Single-event saves crossing the boundary
    let v99 = Some(Version::new(99).unwrap());
    let v100 = Version::new(100).unwrap();
    assert!(trigger.should_snapshot(v99, v100, &[]));

    // Not yet at boundary
    let v98 = Some(Version::new(98).unwrap());
    let v99_ver = Version::new(99).unwrap();
    assert!(!trigger.should_snapshot(v98, v99_ver, &[]));
}

#[test]
fn every_n_events_triggers_on_batch_crossing_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Batch of 7 events crossing the 100 boundary: 96 → 103
    let old = Some(Version::new(96).unwrap());
    let new = Version::new(103).unwrap();
    assert!(trigger.should_snapshot(old, new, &[]));
}

#[test]
fn every_n_events_first_save_triggers_at_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Fresh aggregate, first save crosses boundary
    let new = Version::new(100).unwrap();
    assert!(trigger.should_snapshot(None, new, &[]));

    // Fresh aggregate, first save below boundary
    let new_50 = Version::new(50).unwrap();
    assert!(!trigger.should_snapshot(None, new_50, &[]));
}

#[test]
fn every_1_event_always_triggers() {
    let trigger = EveryNEvents(NonZeroU64::new(1).unwrap());
    assert!(trigger.should_snapshot(None, Version::new(1).unwrap(), &[]));
    assert!(trigger.should_snapshot(
        Some(Version::new(1).unwrap()),
        Version::new(2).unwrap(),
        &[],
    ));
}

// ── AfterEventTypes ─────────────────────────────────────────────────

#[test]
fn after_event_types_triggers_on_matching_event() {
    let trigger = AfterEventTypes::new(&["OrderCompleted", "OrderCancelled"]);

    assert!(trigger.should_snapshot(None, Version::new(5).unwrap(), &["OrderCompleted"]));
    assert!(trigger.should_snapshot(None, Version::new(5).unwrap(), &["OrderCancelled"]));
    assert!(!trigger.should_snapshot(None, Version::new(5).unwrap(), &["ItemAdded"]));
}

#[test]
fn after_event_types_triggers_if_any_event_in_batch_matches() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);

    assert!(trigger.should_snapshot(
        None,
        Version::new(5).unwrap(),
        &["ItemAdded", "OrderCompleted"],
    ));
}

#[test]
fn after_event_types_does_not_trigger_on_empty_events() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);
    assert!(!trigger.should_snapshot(None, Version::new(5).unwrap(), &[]));
}

// ── InMemorySnapshotStore ───────────────────────────────────────────

#[cfg(feature = "testing")]
mod in_memory_tests {
    use super::*;
    use nexus_store::snapshot::InMemorySnapshotStore;

    #[tokio::test]
    async fn load_returns_none_when_empty() {
        let store = InMemorySnapshotStore::new();
        let result = store.load_snapshot(&TestId("agg-1".into())).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = InMemorySnapshotStore::new();
        let id = TestId("agg-1".into());
        let version = Version::new(10).unwrap();
        let snap = PendingSnapshot::new(version, 1, vec![1, 2, 3]);

        store.save_snapshot(&id, &snap).await.unwrap();
        let loaded = store.load_snapshot(&id).await.unwrap().unwrap();

        assert_eq!(loaded.version(), version);
        assert_eq!(loaded.schema_version(), 1);
        assert_eq!(loaded.payload(), &[1, 2, 3]);
    }

    #[tokio::test]
    async fn save_overwrites_previous_snapshot() {
        let store = InMemorySnapshotStore::new();
        let id = TestId("agg-1".into());

        let snap1 = PendingSnapshot::new(Version::new(10).unwrap(), 1, vec![1]);
        store.save_snapshot(&id, &snap1).await.unwrap();

        let snap2 = PendingSnapshot::new(Version::new(20).unwrap(), 1, vec![2]);
        store.save_snapshot(&id, &snap2).await.unwrap();

        let loaded = store.load_snapshot(&id).await.unwrap().unwrap();
        assert_eq!(loaded.version(), Version::new(20).unwrap());
        assert_eq!(loaded.payload(), &[2]);
    }

    #[tokio::test]
    async fn different_aggregates_have_separate_snapshots() {
        let store = InMemorySnapshotStore::new();

        let snap1 = PendingSnapshot::new(Version::new(5).unwrap(), 1, vec![1]);
        store
            .save_snapshot(&TestId("agg-1".into()), &snap1)
            .await
            .unwrap();

        let snap2 = PendingSnapshot::new(Version::new(10).unwrap(), 1, vec![2]);
        store
            .save_snapshot(&TestId("agg-2".into()), &snap2)
            .await
            .unwrap();

        let loaded1 = store
            .load_snapshot(&TestId("agg-1".into()))
            .await
            .unwrap()
            .unwrap();
        let loaded2 = store
            .load_snapshot(&TestId("agg-2".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded1.version(), Version::new(5).unwrap());
        assert_eq!(loaded2.version(), Version::new(10).unwrap());
    }

    #[tokio::test]
    async fn delete_removes_snapshot() {
        let store = InMemorySnapshotStore::new();
        let id = TestId("agg-1".into());

        let snap = PendingSnapshot::new(Version::new(10).unwrap(), 1, vec![1]);
        store.save_snapshot(&id, &snap).await.unwrap();

        store.delete_snapshot(&id).await.unwrap();
        let loaded = store.load_snapshot(&id).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let store = InMemorySnapshotStore::new();
        let result = store.delete_snapshot(&TestId("nope".into())).await;
        assert!(result.is_ok());
    }
}
