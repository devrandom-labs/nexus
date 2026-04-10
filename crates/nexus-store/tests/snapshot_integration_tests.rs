#![cfg(all(feature = "snapshot", feature = "json", feature = "testing"))]
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

use nexus::*;
use nexus_store::Store;
use nexus_store::snapshot::{
    AfterEventTypes, EveryNEvents, InMemorySnapshotStore, SnapshotStore, SnapshotTrigger,
};
use nexus_store::store::{Repository, Snapshotting};
use nexus_store::testing::InMemoryStore;

// ── Test domain ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum CounterEvent {
    Incremented,
    Decremented,
}

impl Message for CounterEvent {}
impl DomainEvent for CounterEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Incremented => "Incremented",
            Self::Decremented => "Decremented",
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct CounterState {
    value: i64,
}

impl AggregateState for CounterState {
    type Event = CounterEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &CounterEvent) -> Self {
        match event {
            CounterEvent::Incremented => self.value += 1,
            CounterEvent::Decremented => self.value -= 1,
        }
        self
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CounterId(u64);

impl fmt::Display for CounterId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "counter-{}", self.0)
    }
}
impl Id for CounterId {}

#[derive(Debug, thiserror::Error)]
#[error("counter error")]
struct CounterError;

struct CounterAggregate;
impl Aggregate for CounterAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = CounterId;
}

// ── Helpers ────────────────────────────────────────────────────────

fn repo(
    trigger: impl SnapshotTrigger,
    snapshot_on_read: bool,
) -> impl Repository<CounterAggregate, Error = nexus_store::StoreError> {
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let inner = store.repository().build();

    Snapshotting::new(
        inner,
        InMemorySnapshotStore::new(),
        nexus_store::JsonCodec::default(),
        trigger,
        1,
        snapshot_on_read,
    )
}

// ── Tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn load_without_snapshot_does_full_replay() {
    let repo = repo(EveryNEvents(NonZeroU64::new(100).unwrap()), false);
    let id = CounterId(1);

    // Save 5 events
    let mut agg = repo.load(id.clone()).await.unwrap();
    let events = vec![
        CounterEvent::Incremented,
        CounterEvent::Incremented,
        CounterEvent::Incremented,
        CounterEvent::Decremented,
        CounterEvent::Incremented,
    ];
    repo.save(&mut agg, &events).await.unwrap();

    // Reload — should replay all 5 events
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 3); // +1 +1 +1 -1 +1 = 3
    assert_eq!(loaded.version(), Some(Version::new(5).unwrap()));
}

#[tokio::test]
async fn save_triggers_snapshot_and_load_uses_it() {
    // EveryNEvents(3) — snapshot after 3 events
    let repo = repo(EveryNEvents(NonZeroU64::new(3).unwrap()), false);
    let id = CounterId(1);

    let mut agg = repo.load(id.clone()).await.unwrap();
    let events = vec![
        CounterEvent::Incremented,
        CounterEvent::Incremented,
        CounterEvent::Incremented,
    ];
    repo.save(&mut agg, &events).await.unwrap();

    // At this point, trigger should have fired (crossed 3-boundary).
    // Reload — should hit snapshot + partial replay (0 additional events).
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 3);
    assert_eq!(loaded.version(), Some(Version::new(3).unwrap()));
}

#[tokio::test]
async fn save_below_threshold_no_snapshot_then_crosses() {
    let repo = repo(EveryNEvents(NonZeroU64::new(5).unwrap()), false);
    let id = CounterId(1);

    // Save 3 events — below threshold
    let mut agg = repo.load(id.clone()).await.unwrap();
    repo.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Save 3 more — crosses 5-boundary (v3→v6)
    repo.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Reload — should use snapshot at v6
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 6);
    assert_eq!(loaded.version(), Some(Version::new(6).unwrap()));
}

#[tokio::test]
async fn schema_version_mismatch_falls_back_to_full_replay() {
    // Create a repo with schema_version=1, save events+snapshot
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let inner = store.repository().build();
    let snap_store = InMemorySnapshotStore::new();

    let repo_v1 = Snapshotting::new(
        inner,
        &snap_store,
        nexus_store::JsonCodec::default(),
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        1, // schema v1
        false,
    );

    let id = CounterId(1);
    let mut agg: AggregateRoot<CounterAggregate> = repo_v1.load(id.clone()).await.unwrap();
    repo_v1
        .save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    // Now create a new repo with schema_version=2 pointing to same stores
    let inner2 = store.repository().build();
    let repo_v2 = Snapshotting::new(
        inner2,
        &snap_store,
        nexus_store::JsonCodec::default(),
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        2, // schema v2 — mismatch!
        false,
    );

    // Load should ignore the v1 snapshot and do full replay
    let loaded: AggregateRoot<CounterAggregate> = repo_v2.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 1);
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

#[tokio::test]
async fn after_event_types_trigger_snapshots_on_domain_milestone() {
    let repo = repo(AfterEventTypes::new(&["Decremented"]), false);
    let id = CounterId(1);

    let mut agg = repo.load(id.clone()).await.unwrap();

    // Save Incremented — no snapshot
    repo.save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();
    repo.save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    // Save Decremented — triggers snapshot
    repo.save(&mut agg, &[CounterEvent::Decremented])
        .await
        .unwrap();

    // Reload — snapshot at v3, state=1
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 1);
    assert_eq!(loaded.version(), Some(Version::new(3).unwrap()));
}

#[tokio::test]
async fn lazy_snapshot_on_read_after_full_replay() {
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let snap_store = InMemorySnapshotStore::new();

    // First, save events without snapshots (no trigger, just on-read)
    let inner = store.repository().build();
    let repo_write = Snapshotting::new(
        inner,
        &snap_store,
        nexus_store::JsonCodec::default(),
        EveryNEvents(NonZeroU64::new(1000).unwrap()), // very high threshold
        1,
        false, // no on-read yet
    );

    let id = CounterId(1);
    let mut agg: AggregateRoot<CounterAggregate> = repo_write.load(id.clone()).await.unwrap();
    repo_write
        .save(
            &mut agg,
            &[
                CounterEvent::Incremented,
                CounterEvent::Incremented,
                CounterEvent::Incremented,
            ],
        )
        .await
        .unwrap();

    // No snapshot yet (threshold too high).
    // Now create a repo with on-read enabled
    let inner2 = store.repository().build();
    let repo_read = Snapshotting::new(
        inner2,
        &snap_store,
        nexus_store::JsonCodec::default(),
        EveryNEvents(NonZeroU64::new(1000).unwrap()),
        1,
        true, // on-read enabled!
    );

    // First load → full replay → lazy snapshot created
    let loaded: AggregateRoot<CounterAggregate> = repo_read.load(id.clone()).await.unwrap();
    assert_eq!(loaded.state().value, 3);

    // Verify snapshot was created by checking load_snapshot directly
    let snap = snap_store.load_snapshot(&id).await.unwrap();
    assert!(snap.is_some());
    assert_eq!(snap.unwrap().version(), Version::new(3).unwrap());
}

#[tokio::test]
async fn empty_events_save_is_noop() {
    let repo = repo(EveryNEvents(NonZeroU64::new(1).unwrap()), false);
    let id = CounterId(1);

    let mut agg = repo.load(id.clone()).await.unwrap();
    // Save empty slice — should not trigger snapshot
    repo.save(&mut agg, &[]).await.unwrap();

    assert_eq!(agg.version(), None);
}

#[tokio::test]
async fn multiple_save_load_cycles() {
    let repo = repo(EveryNEvents(NonZeroU64::new(3).unwrap()), false);
    let id = CounterId(1);

    // Cycle 1: save 3 → snapshot at v3
    let mut agg = repo.load(id.clone()).await.unwrap();
    repo.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Cycle 2: reload, save 3 more → snapshot at v6
    let mut agg = repo.load(id.clone()).await.unwrap();
    assert_eq!(agg.state().value, 3);
    repo.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Cycle 3: reload, verify state
    let agg = repo.load(id).await.unwrap();
    assert_eq!(agg.state().value, 6);
    assert_eq!(agg.version(), Some(Version::new(6).unwrap()));
}
