#![cfg(feature = "snapshot")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::as_conversions,
    clippy::useless_vec,
    clippy::doc_markdown,
    clippy::single_call_fn,
    clippy::iter_on_single_items,
    reason = "test harness — relaxed lints for test code"
)]

use std::fmt;

use std::num::{NonZeroU32, NonZeroU64};

use nexus::{Id, Version};

const SV1: NonZeroU32 = NonZeroU32::MIN;
use nexus_store::state::{AfterEventTypes, EveryNEvents, PersistTrigger, SnapshotStore};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TestId(String);

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

// ── EveryNEvents ────────────────────────────────────────────────────

#[test]
fn every_n_events_triggers_on_boundary_crossing() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Single-event saves crossing the boundary
    let v99 = Some(Version::new(99).unwrap());
    let v100 = Version::new(100).unwrap();
    assert!(trigger.should_persist(v99, v100, std::iter::empty::<&str>()));

    // Not yet at boundary
    let v98 = Some(Version::new(98).unwrap());
    let v99_ver = Version::new(99).unwrap();
    assert!(!trigger.should_persist(v98, v99_ver, std::iter::empty::<&str>()));
}

#[test]
fn every_n_events_triggers_on_batch_crossing_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Batch of 7 events crossing the 100 boundary: 96 → 103
    let old = Some(Version::new(96).unwrap());
    let new = Version::new(103).unwrap();
    assert!(trigger.should_persist(old, new, std::iter::empty::<&str>()));
}

#[test]
fn every_n_events_first_save_triggers_at_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    // Fresh aggregate, first save crosses boundary
    let new = Version::new(100).unwrap();
    assert!(trigger.should_persist(None, new, std::iter::empty::<&str>()));

    // Fresh aggregate, first save below boundary
    let new_50 = Version::new(50).unwrap();
    assert!(!trigger.should_persist(None, new_50, std::iter::empty::<&str>()));
}

#[test]
fn every_1_event_always_triggers() {
    let trigger = EveryNEvents(NonZeroU64::new(1).unwrap());
    assert!(trigger.should_persist(None, Version::new(1).unwrap(), std::iter::empty::<&str>()));
    assert!(trigger.should_persist(
        Some(Version::new(1).unwrap()),
        Version::new(2).unwrap(),
        std::iter::empty::<&str>(),
    ));
}

// ── AfterEventTypes ─────────────────────────────────────────────────

#[test]
fn after_event_types_triggers_on_matching_event() {
    let trigger = AfterEventTypes::new(&["OrderCompleted", "OrderCancelled"]);

    assert!(trigger.should_persist(
        None,
        Version::new(5).unwrap(),
        ["OrderCompleted"].into_iter()
    ));
    assert!(trigger.should_persist(
        None,
        Version::new(5).unwrap(),
        ["OrderCancelled"].into_iter()
    ));
    assert!(!trigger.should_persist(None, Version::new(5).unwrap(), ["ItemAdded"].into_iter()));
}

#[test]
fn after_event_types_triggers_if_any_event_in_batch_matches() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);

    assert!(trigger.should_persist(
        None,
        Version::new(5).unwrap(),
        ["ItemAdded", "OrderCompleted"].into_iter(),
    ));
}

#[test]
fn after_event_types_does_not_trigger_on_empty_events() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);
    assert!(!trigger.should_persist(None, Version::new(5).unwrap(), std::iter::empty::<&str>()));
}

// ── InMemorySnapshotStore ────────────────────────────────────────

#[cfg(feature = "testing")]
mod in_memory_tests {
    use super::*;
    use nexus_store::state::InMemorySnapshotStore;

    #[tokio::test]
    async fn hydrate_returns_none_when_empty() {
        let store = InMemorySnapshotStore::<Vec<u8>, Version>::new();
        let result = store.hydrate(&TestId("agg-1".into()), SV1).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn commit_then_hydrate_roundtrips() {
        let store = InMemorySnapshotStore::<Vec<u8>, Version>::new();
        let id = TestId("agg-1".into());
        let version = Version::new(10).unwrap();

        store
            .commit(&id, NonZeroU32::new(1).unwrap(), version, &vec![1, 2, 3])
            .await
            .unwrap();
        let (loaded_version, loaded_state) = store.hydrate(&id, SV1).await.unwrap().unwrap();

        assert_eq!(loaded_version, version);
        assert_eq!(loaded_state, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn commit_overwrites_previous_snapshot() {
        let store = InMemorySnapshotStore::<Vec<u8>, Version>::new();
        let id = TestId("agg-1".into());

        store
            .commit(
                &id,
                NonZeroU32::new(1).unwrap(),
                Version::new(10).unwrap(),
                &vec![1],
            )
            .await
            .unwrap();

        store
            .commit(
                &id,
                NonZeroU32::new(1).unwrap(),
                Version::new(20).unwrap(),
                &vec![2],
            )
            .await
            .unwrap();

        let (loaded_version, loaded_state) = store.hydrate(&id, SV1).await.unwrap().unwrap();
        assert_eq!(loaded_version, Version::new(20).unwrap());
        assert_eq!(loaded_state, vec![2]);
    }

    #[tokio::test]
    async fn different_aggregates_have_separate_snapshots() {
        let store = InMemorySnapshotStore::<Vec<u8>, Version>::new();

        store
            .commit(
                &TestId("agg-1".into()),
                NonZeroU32::new(1).unwrap(),
                Version::new(5).unwrap(),
                &vec![1],
            )
            .await
            .unwrap();

        store
            .commit(
                &TestId("agg-2".into()),
                NonZeroU32::new(1).unwrap(),
                Version::new(10).unwrap(),
                &vec![2],
            )
            .await
            .unwrap();

        let (version1, _) = store
            .hydrate(&TestId("agg-1".into()), SV1)
            .await
            .unwrap()
            .unwrap();
        let (version2, _) = store
            .hydrate(&TestId("agg-2".into()), SV1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(version1, Version::new(5).unwrap());
        assert_eq!(version2, Version::new(10).unwrap());
    }
}

// ── Builder integration ─────────────────────────────────────────────

#[cfg(all(feature = "testing", feature = "snapshot-json"))]
mod builder_tests {
    use super::*;
    use nexus_store::Store;
    use nexus_store::state::InMemorySnapshotStore;
    use nexus_store::testing::InMemoryStore;

    /// Verify the builder API compiles: no snapshot → EventStore
    #[test]
    fn builder_without_snapshot_compiles() {
        let raw = InMemoryStore::new();
        let store = Store::new(raw);
        let _repo = store.repository().build();
    }

    /// Verify the builder API compiles: with snapshot → Snapshotting<EventStore>
    #[test]
    fn builder_with_snapshot_compiles() {
        let raw = InMemoryStore::new();
        let store = Store::new(raw);
        let snap = InMemorySnapshotStore::<Vec<u8>, Version>::new();
        let _repo = store.repository().snapshot_store(snap).build();
    }

    /// Verify the builder API compiles: with custom trigger
    #[test]
    fn builder_with_custom_trigger_compiles() {
        let raw = InMemoryStore::new();
        let store = Store::new(raw);
        let snap = InMemorySnapshotStore::<Vec<u8>, Version>::new();
        let _repo = store
            .repository()
            .snapshot_store(snap)
            .snapshot_trigger(AfterEventTypes::new(&["Done"]))
            .snapshot_schema_version(NonZeroU32::new(2).unwrap())
            .snapshot_on_read(true)
            .build();
    }
}
