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
use std::num::{NonZeroU32, NonZeroU64};

const SV1: NonZeroU32 = NonZeroU32::MIN;

#[allow(clippy::unwrap_used, reason = "test constant")]
fn sv2() -> NonZeroU32 {
    NonZeroU32::new(2).unwrap()
}

use nexus::*;
use nexus_store::Store;
use nexus_store::state::{
    AfterEventTypes, EveryNEvents, InMemoryStateStore, PersistTrigger, StateStore,
};
use nexus_store::testing::InMemoryStore;
use nexus_store::{Repository, Snapshotting};

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
struct CounterId(String);

impl fmt::Display for CounterId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<[u8]> for CounterId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for CounterId {
    const BYTE_LEN: usize = 0;
}

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

fn repo(trigger: impl PersistTrigger, snapshot_on_read: bool) -> impl Repository<CounterAggregate> {
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let inner = store.repository().build();

    Snapshotting::new(
        inner,
        InMemoryStateStore::<CounterState>::new(),
        trigger,
        SV1,
        snapshot_on_read,
    )
}

// ── Tests ──────────────────────────────────────────────────────────

#[tokio::test]
async fn load_without_snapshot_does_full_replay() {
    let repo = repo(EveryNEvents(NonZeroU64::new(100).unwrap()), false);
    let id = CounterId("counter-1".into());

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
    let id = CounterId("counter-1".into());

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
    let id = CounterId("counter-1".into());

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
    let snap_store = InMemoryStateStore::<CounterState>::new();

    let repo_v1 = Snapshotting::new(
        inner,
        &snap_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1, // schema v1
        false,
    );

    let id = CounterId("counter-1".into());
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
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        sv2(), // schema v2 — mismatch!
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
    let id = CounterId("counter-1".into());

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
    let snap_store = InMemoryStateStore::<CounterState>::new();

    // First, save events without snapshots (no trigger, just on-read)
    let inner = store.repository().build();
    let repo_write = Snapshotting::new(
        inner,
        &snap_store,
        EveryNEvents(NonZeroU64::new(1000).unwrap()), // very high threshold
        SV1,
        false, // no on-read yet
    );

    let id = CounterId("counter-1".into());
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
        EveryNEvents(NonZeroU64::new(1000).unwrap()),
        SV1,
        true, // on-read enabled!
    );

    // First load → full replay → lazy snapshot created
    let loaded: AggregateRoot<CounterAggregate> = repo_read.load(id.clone()).await.unwrap();
    assert_eq!(loaded.state().value, 3);

    // Verify snapshot was created by checking load directly
    let snap = snap_store.load(&id, SV1).await.unwrap();
    assert!(snap.is_some());
    assert_eq!(snap.unwrap().version(), Version::new(3).unwrap());
}

#[tokio::test]
async fn empty_events_save_is_noop() {
    let repo = repo(EveryNEvents(NonZeroU64::new(1).unwrap()), false);
    let id = CounterId("counter-1".into());

    let mut agg = repo.load(id.clone()).await.unwrap();
    // Save empty slice — should not trigger snapshot
    repo.save(&mut agg, &[]).await.unwrap();

    assert_eq!(agg.version(), None);
}

#[tokio::test]
async fn multiple_save_load_cycles() {
    let repo = repo(EveryNEvents(NonZeroU64::new(3).unwrap()), false);
    let id = CounterId("counter-1".into());

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

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn sequence_save_snapshot_save_more_snapshot_again() {
    let repo = repo(EveryNEvents(NonZeroU64::new(3).unwrap()), false);
    let id = CounterId("counter-42".into());

    // Save 3 → snapshot at v3
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
    assert_eq!(agg.version(), Some(Version::new(3).unwrap()));

    // Reload from snapshot → save 3 more → snapshot at v6
    let mut agg = repo.load(id.clone()).await.unwrap();
    assert_eq!(agg.state().value, 3);
    repo.save(
        &mut agg,
        &[
            CounterEvent::Decremented,
            CounterEvent::Decremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Final reload — should use v6 snapshot
    let agg = repo.load(id).await.unwrap();
    assert_eq!(agg.state().value, 2); // 3 - 1 - 1 + 1
    assert_eq!(agg.version(), Some(Version::new(6).unwrap()));
}

#[tokio::test]
async fn sequence_batch_crossing_non_multiple_boundary() {
    // EveryNEvents(5): batch 96→103 crosses 100-boundary
    // We simulate with small numbers: EveryNEvents(5), batch 3→8 crosses 5
    let repo = repo(EveryNEvents(NonZeroU64::new(5).unwrap()), false);
    let id = CounterId("counter-1".into());

    // Save 3 events (v1-v3)
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

    // Save 5 events in batch (v4-v8) — crosses the 5 boundary
    repo.save(
        &mut agg,
        &[
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
            CounterEvent::Incremented,
        ],
    )
    .await
    .unwrap();

    // Snapshot should exist at v8. Reload and verify.
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 8);
    assert_eq!(loaded.version(), Some(Version::new(8).unwrap()));
}

#[tokio::test]
async fn sequence_snapshot_invalidation_then_new_snapshot() {
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let snap_store = InMemoryStateStore::<CounterState>::new();

    // Save with schema v1, triggers snapshot
    let inner = store.repository().build();
    let repo_v1 = Snapshotting::new(
        inner,
        &snap_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );
    let id = CounterId("counter-1".into());
    let mut agg: AggregateRoot<CounterAggregate> = repo_v1.load(id.clone()).await.unwrap();
    repo_v1
        .save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    // Switch to schema v2 — old snapshot ignored, full replay, new snapshot created
    let inner2 = store.repository().build();
    let repo_v2 = Snapshotting::new(
        inner2,
        &snap_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        sv2(),
        false,
    );
    let mut agg: AggregateRoot<CounterAggregate> = repo_v2.load(id.clone()).await.unwrap();
    assert_eq!(agg.state().value, 1);

    // Save another event → new snapshot with schema v2
    repo_v2
        .save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    // Reload should hit v2 snapshot
    let loaded: AggregateRoot<CounterAggregate> = repo_v2.load(id.clone()).await.unwrap();
    assert_eq!(loaded.state().value, 2);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));

    // Verify the snapshot has schema v2
    let snap = snap_store.load(&id, sv2()).await.unwrap().unwrap();
    assert_eq!(snap.schema_version(), sv2());
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn lifecycle_create_save_snapshot_reload_verify() {
    let repo = repo(EveryNEvents(NonZeroU64::new(2).unwrap()), false);
    let id = CounterId("counter-99".into());

    // Create new aggregate
    let mut agg = repo.load(id.clone()).await.unwrap();
    assert_eq!(agg.version(), None);
    assert_eq!(agg.state().value, 0);

    // Save 2 events → snapshot
    repo.save(
        &mut agg,
        &[CounterEvent::Incremented, CounterEvent::Decremented],
    )
    .await
    .unwrap();

    // Reload → from snapshot, state should match
    let loaded = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 0); // +1 -1
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn lifecycle_lazy_snapshot_then_subsequent_load_uses_it() {
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let snap_store = InMemoryStateStore::<CounterState>::new();

    // Save events without snapshot
    let inner = store.repository().build();
    let repo_no_snap = Snapshotting::new(
        inner,
        &snap_store,
        EveryNEvents(NonZeroU64::new(10000).unwrap()),
        SV1,
        false,
    );
    let id = CounterId("counter-1".into());
    let mut agg: AggregateRoot<CounterAggregate> = repo_no_snap.load(id.clone()).await.unwrap();
    repo_no_snap
        .save(
            &mut agg,
            &[
                CounterEvent::Incremented,
                CounterEvent::Incremented,
                CounterEvent::Incremented,
                CounterEvent::Incremented,
                CounterEvent::Incremented,
            ],
        )
        .await
        .unwrap();

    // No snapshot yet
    assert!(snap_store.load(&id, SV1).await.unwrap().is_none());

    // Load with on-read → creates lazy snapshot
    let inner2 = store.repository().build();
    let repo_on_read = Snapshotting::new(
        inner2,
        &snap_store,
        EveryNEvents(NonZeroU64::new(10000).unwrap()),
        SV1,
        true,
    );
    let _loaded: AggregateRoot<CounterAggregate> = repo_on_read.load(id.clone()).await.unwrap();

    // Snapshot now exists
    let snap = snap_store.load(&id, SV1).await.unwrap().unwrap();
    assert_eq!(snap.version(), Version::new(5).unwrap());

    // Second load uses snapshot (we can't directly prove partial replay,
    // but state correctness confirms the snapshot path works)
    let loaded2: AggregateRoot<CounterAggregate> = repo_on_read.load(id).await.unwrap();
    assert_eq!(loaded2.state().value, 5);
    assert_eq!(loaded2.version(), Some(Version::new(5).unwrap()));
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive Boundary Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn defensive_snapshot_codec_error_falls_back_to_full_replay() {
    // Use a state store that always fails decode (load).
    // We simulate this by using a CodecStateStore with a failing codec.
    use nexus_store::state::CodecStateStore;

    struct FailCodec;
    impl nexus_store::Codec<CounterState> for FailCodec {
        type Error = std::io::Error;
        fn encode(&self, _state: &CounterState) -> Result<Vec<u8>, Self::Error> {
            Ok(vec![1, 2, 3])
        }
        fn decode(&self, _event_type: &str, _payload: &[u8]) -> Result<CounterState, Self::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "always fails",
            ))
        }
    }

    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let byte_store = InMemoryStateStore::<Vec<u8>>::new();

    // Save events and snapshot with a good codec first
    let good_state_store = CodecStateStore::new(&byte_store, nexus_store::JsonCodec::default());
    let inner = store.repository().build();
    let repo_good = Snapshotting::new(
        inner,
        &good_state_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );
    let id = CounterId("counter-1".into());
    let mut agg: AggregateRoot<CounterAggregate> = repo_good.load(id.clone()).await.unwrap();
    repo_good
        .save(&mut agg, &[CounterEvent::Incremented])
        .await
        .unwrap();

    // Now load with the failing codec — should fall back to full replay
    let bad_state_store = CodecStateStore::new(&byte_store, FailCodec);
    let inner2 = store.repository().build();
    let repo_bad = Snapshotting::new(
        inner2,
        &bad_state_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );
    let loaded: AggregateRoot<CounterAggregate> = repo_bad.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 1);
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

#[tokio::test]
async fn defensive_snapshot_store_load_error_falls_back_to_full_replay() {
    // A state store that always errors on load
    struct ErrorStore;
    impl StateStore<CounterState> for ErrorStore {
        type Error = std::io::Error;
        async fn load(
            &self,
            _id: &impl nexus::Id,
            _schema_version: NonZeroU32,
        ) -> Result<Option<nexus_store::state::State<CounterState>>, Self::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "disk on fire",
            ))
        }
        async fn save(
            &self,
            _id: &impl nexus::Id,
            _state: &nexus_store::state::State<CounterState>,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
        async fn delete(&self, _id: &impl nexus::Id) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let raw = InMemoryStore::new();
    let store = Store::new(raw);

    // Save some events first
    let inner = store.repository().build();
    let good_repo = Snapshotting::new(
        inner,
        InMemoryStateStore::<CounterState>::new(),
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );
    let id = CounterId("counter-1".into());
    let mut agg: AggregateRoot<CounterAggregate> = good_repo.load(id.clone()).await.unwrap();
    good_repo
        .save(
            &mut agg,
            &[CounterEvent::Incremented, CounterEvent::Incremented],
        )
        .await
        .unwrap();

    // Now load with error store — should fall back to full replay
    let inner2 = store.repository().build();
    let bad_repo = Snapshotting::new(
        inner2,
        ErrorStore,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );
    let loaded: AggregateRoot<CounterAggregate> = bad_repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 2);
}

#[tokio::test]
async fn defensive_snapshot_save_failure_does_not_fail_event_save() {
    // A state store that always errors on save
    struct SaveErrorStore;
    impl StateStore<CounterState> for SaveErrorStore {
        type Error = std::io::Error;
        async fn load(
            &self,
            _id: &impl nexus::Id,
            _schema_version: NonZeroU32,
        ) -> Result<Option<nexus_store::state::State<CounterState>>, Self::Error> {
            Ok(None)
        }
        async fn save(
            &self,
            _id: &impl nexus::Id,
            _state: &nexus_store::state::State<CounterState>,
        ) -> Result<(), Self::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "snapshot write failed",
            ))
        }
        async fn delete(&self, _id: &impl nexus::Id) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let inner = store.repository().build();
    let repo = Snapshotting::new(
        inner,
        SaveErrorStore,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );

    let id = CounterId("counter-1".into());
    let mut agg: AggregateRoot<CounterAggregate> = repo.load(id.clone()).await.unwrap();

    // Save should succeed despite snapshot save failure
    let result = repo.save(&mut agg, &[CounterEvent::Incremented]).await;
    assert!(result.is_ok());
    assert_eq!(agg.state().value, 1);

    // Events persist correctly — reload works via full replay
    let loaded: AggregateRoot<CounterAggregate> = repo.load(id).await.unwrap();
    assert_eq!(loaded.state().value, 1);
}

#[tokio::test]
async fn defensive_after_event_types_empty_names_no_trigger() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);
    // Empty event names should not trigger
    assert!(!trigger.should_persist(None, Version::new(5).unwrap(), std::iter::empty::<&str>()));
    // Non-matching should not trigger
    assert!(!trigger.should_persist(
        Some(Version::new(1).unwrap()),
        Version::new(2).unwrap(),
        ["ItemAdded"].into_iter(),
    ));
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Linearizability/Isolation Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn isolation_concurrent_loads_from_same_snapshot_get_independent_copies() {
    let raw = InMemoryStore::new();
    let store = Store::new(raw);
    let snap_store = InMemoryStateStore::<CounterState>::new();

    // Save 3 events, snapshot at v3
    let inner = store.repository().build();
    let repo = Snapshotting::new(
        inner,
        &snap_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );
    let id = CounterId("counter-1".into());
    let mut agg: AggregateRoot<CounterAggregate> = repo.load(id.clone()).await.unwrap();
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

    // Load two copies concurrently
    let inner2 = store.repository().build();
    let repo2 = Snapshotting::new(
        inner2,
        &snap_store,
        EveryNEvents(NonZeroU64::new(1).unwrap()),
        SV1,
        false,
    );

    let (load_a, load_b) = tokio::join!(
        async {
            let agg: AggregateRoot<CounterAggregate> = repo2.load(id.clone()).await.unwrap();
            agg
        },
        async {
            let agg: AggregateRoot<CounterAggregate> = repo.load(id.clone()).await.unwrap();
            agg
        },
    );

    // Both should have identical state but be independent instances
    assert_eq!(load_a.state().value, 3);
    assert_eq!(load_b.state().value, 3);
    assert_eq!(load_a.version(), Some(Version::new(3).unwrap()));
    assert_eq!(load_b.version(), Some(Version::new(3).unwrap()));
}
