#![cfg(feature = "testing")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::shadow_reuse,
    clippy::shadow_same,
    clippy::shadow_unrelated,
    clippy::as_conversions,
    clippy::use_self,
    clippy::too_many_lines,
    clippy::no_effect_underscore_binding,
    reason = "test harness — relaxed lints for test code"
)]

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;

use nexus::{DomainEvent, Message, Version};
use nexus_framework::projection::{Projection, ProjectionError, ProjectionStatus, StartupDecision};
use nexus_store::testing::InMemoryStore;
use nexus_store::{
    Decode, Encode, EventStreamExt, InMemorySnapshotStore, RawEventStore, SnapshotStore,
    pending_envelope,
};
use nexus_store::{EveryNEvents, Projector};

// ═══════════════════════════════════════════════════════════════════════════
// Test fixtures
// ═══════════════════════════════════════════════════════════════════════════

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

impl nexus::Id for TestId {
    const BYTE_LEN: usize = 0;
}

#[derive(Debug, Clone, PartialEq)]
struct CountState {
    count: u64,
    total: u64,
}

#[derive(Debug)]
enum TestEvent {
    Added(u64),
    Removed(u64),
}

impl Message for TestEvent {}
impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            TestEvent::Added(_) => "Added",
            TestEvent::Removed(_) => "Removed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("projection overflow")]
struct TestProjectionError;

struct CountingProjector;

impl Projector for CountingProjector {
    type Event = TestEvent;
    type State = CountState;
    type Error = TestProjectionError;

    fn initial(&self) -> CountState {
        CountState { count: 0, total: 0 }
    }

    fn apply(
        &self,
        state: CountState,
        event: &TestEvent,
    ) -> Result<CountState, TestProjectionError> {
        match event {
            TestEvent::Added(n) => Ok(CountState {
                count: state.count.checked_add(1).ok_or(TestProjectionError)?,
                total: state.total.checked_add(*n).ok_or(TestProjectionError)?,
            }),
            TestEvent::Removed(n) => Ok(CountState {
                count: state.count.checked_add(1).ok_or(TestProjectionError)?,
                total: state.total.checked_sub(*n).ok_or(TestProjectionError)?,
            }),
        }
    }
}

/// Simple event codec for tests.
struct TestEventCodec;

impl Encode<TestEvent> for TestEventCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &TestEvent) -> Result<bytes::Bytes, Self::Error> {
        match event {
            TestEvent::Added(n) => {
                let mut buf = vec![0u8]; // tag
                buf.extend_from_slice(&n.to_le_bytes());
                Ok(bytes::Bytes::from(buf))
            }
            TestEvent::Removed(n) => {
                let mut buf = vec![1u8]; // tag
                buf.extend_from_slice(&n.to_le_bytes());
                Ok(bytes::Bytes::from(buf))
            }
        }
    }
}

impl Decode<TestEvent> for TestEventCodec {
    type Error = std::io::Error;

    fn decode(&self, _name: &str, payload: &[u8]) -> Result<TestEvent, Self::Error> {
        if payload.len() != 9 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad len",
            ));
        }
        let n = u64::from_le_bytes(payload[1..9].try_into().unwrap());
        match payload[0] {
            0 => Ok(TestEvent::Added(n)),
            1 => Ok(TestEvent::Removed(n)),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad tag",
            )),
        }
    }
}

/// A fresh in-memory snapshot store for projection state + position.
fn snapshot_store() -> InMemorySnapshotStore<CountState, Version> {
    InMemorySnapshotStore::<CountState, Version>::new()
}

/// Append test events to the in-memory store.
async fn append_events(store: &InMemoryStore, stream_id: &TestId, events: &[TestEvent]) {
    let codec = TestEventCodec;
    let current_len = {
        let mut stream = store
            .read_stream(stream_id, Version::INITIAL)
            .await
            .unwrap();
        stream.try_count().await.unwrap()
    };
    let base_version = u64::try_from(current_len).unwrap();

    let envelopes: Vec<_> = events
        .iter()
        .enumerate()
        .map(|(i, event)| {
            let ver = Version::new(base_version + u64::try_from(i).unwrap() + 1).unwrap();
            let payload = codec.encode(event).unwrap();
            pending_envelope(ver)
                .event_type(event.name())
                .payload(payload)
                .build()
        })
        .collect();

    let expected = Version::new(base_version).filter(|_| base_version > 0);
    store.append(stream_id, expected, &envelopes).await.unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_processes_events_and_checkpoints() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append 3 events
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    // Build runner with EveryNEvents(1) — checkpoint every event
    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    // Run with delayed shutdown to let the runner process existing events
    let shutdown = async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };

    runner
        .initialize()
        .await
        .unwrap()
        .run(shutdown)
        .await
        .unwrap();

    // Verify checkpoint + state — persisted together in the snapshot.
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position, Version::new(3).unwrap());
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );
}

#[tokio::test]
async fn runner_resumes_from_checkpoint() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append 3 events, run, shutdown
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Append 2 more events
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(40), TestEvent::Removed(5)],
    )
    .await;

    // Run again — should resume from checkpoint (version 3)
    let runner2 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    runner2
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: checkpoint at 5, state = count:5 total:95
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position, Version::new(5).unwrap());
    assert_eq!(
        state,
        CountState {
            count: 5,
            total: 95
        }
    );
}

#[tokio::test]
async fn runner_trigger_controls_checkpoint_frequency() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append 5 events
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(1),
            TestEvent::Added(2),
            TestEvent::Added(3),
            TestEvent::Added(4),
            TestEvent::Added(5),
        ],
    )
    .await;

    // Trigger every 3 events
    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .trigger(EveryNEvents(NonZeroU64::new(3).unwrap()))
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Checkpoint should be at version 5 (flushed on shutdown), state reflects
    // all 5 events.
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position, Version::new(5).unwrap());
    assert_eq!(
        state,
        CountState {
            count: 5,
            total: 15
        }
    );
}

#[tokio::test]
async fn runner_rebuilds_from_beginning_on_schema_version_bump() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append 3 events, run with schema v1
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: snapshot at v3 with schema v1
    let (position, _) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position, Version::new(3).unwrap());

    // Now restart with schema v2 — the schema-mismatched snapshot is invisible
    // to hydrate, so the projection replays from the beginning.
    let runner2 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner2
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should reflect ALL 3 events from initial() — not resume from v3
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );
    // Snapshot for schema v2 should be at v3
    assert_eq!(position, Version::new(3).unwrap());
}

#[tokio::test]
async fn runner_resumes_normally_after_rebuild_completes() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Phase 1: initial run with schema v1
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Phase 2: restart with schema v2 — replays from the beginning
    let runner2 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner2
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Phase 3: append more events, restart with same schema v2 — normal resume
    append_events(&store, &stream_id, &[TestEvent::Added(30)]).await;

    let runner3 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner3
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: state reflects all 3 events, checkpoint at v3
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );
    assert_eq!(position, Version::new(3).unwrap());
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_immediate_shutdown_with_no_events() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("empty-stream".into());

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    // Immediate shutdown — should return Ok without errors
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tx.send(()).unwrap();
    let shutdown = async {
        rx.await.ok();
    };

    runner
        .initialize()
        .await
        .unwrap()
        .run(shutdown)
        .await
        .unwrap();

    // No snapshot saved (no events processed)
    let loaded = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn runner_rebuild_is_idempotent_after_crash_before_trigger() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Phase 1: initial run with schema v1
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
        ],
    )
    .await;

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Snapshot at v3 with schema v1
    let (position, _) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position, Version::new(3).unwrap());

    // Phase 2: start with schema v2 but immediate shutdown.
    // tokio::select! is non-deterministic — the runner may process 0..N
    // events before the shutdown branch wins. This simulates a crash at
    // an arbitrary point during the replay. The important property is
    // that Phase 3 produces the correct final state regardless.
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tx.send(()).unwrap();

    let runner2 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .trigger(EveryNEvents(NonZeroU64::new(100).unwrap()))
        .build();

    runner2
        .initialize()
        .await
        .unwrap()
        .run(async {
            rx.await.ok();
        })
        .await
        .unwrap();

    // Phase 3: restart with schema v2 — whether Phase 2 processed some
    // events or none, the final result must be correct (idempotent)
    let runner3 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner3
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Now state should be correctly rebuilt
    let (_, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );
}

#[tokio::test]
async fn runner_graceful_shutdown_flushes_dirty_state() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append 2 events
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // Trigger every 100 events — so the trigger never fires during these 2 events.
    // Only the shutdown flush should persist state.
    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .trigger(EveryNEvents(NonZeroU64::new(100).unwrap()))
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State + checkpoint should be flushed on shutdown even though the
    // trigger never fired.
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );
    assert_eq!(position, Version::new(2).unwrap());
}

#[tokio::test]
async fn runner_stale_state_falls_back_to_initial() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Pre-save a snapshot with schema version 1
    snapshots
        .commit(
            &stream_id,
            NonZeroU32::MIN,
            Version::new(5).unwrap(),
            &CountState {
                count: 99,
                total: 999,
            },
        )
        .await
        .unwrap();

    // Append 2 events
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // Runner with schema version 2 — stale state should be ignored
    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should start from initial(), not from stale v1 state
    let (_, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Defensive Boundary Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_first_run_with_state_persistence_is_not_rebuild() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // First run ever — no snapshot. Should process from beginning
    // without treating it as a "rebuild".
    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let ready = runner.initialize().await.unwrap();
    assert_eq!(ready.decision(), StartupDecision::Fresh);

    ready
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    let (_, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );
}

#[tokio::test]
async fn runner_returns_projector_error_on_apply_failure() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append an event that will cause underflow
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Removed(1), // underflow: 0 - 1
        ],
    )
    .await;

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let result = runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        })
        .await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ProjectionError::Projector(_)));
}

#[tokio::test]
async fn runner_returns_event_codec_error_on_bad_payload() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append a raw event with garbage payload
    let bad_envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("Added")
        .payload(vec![0xFF]) // invalid: too short
        .build();
    store
        .append(&stream_id, None, &[bad_envelope])
        .await
        .unwrap();

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let result = runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        })
        .await;

    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ProjectionError::EventCodec(_)
    ));
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Linearizability/Isolation Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_catches_up_and_processes_all_existing_events() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append events before runner starts — runner must catch up
    append_events(
        &store,
        &stream_id,
        &[
            TestEvent::Added(10),
            TestEvent::Added(20),
            TestEvent::Added(30),
            TestEvent::Added(40),
            TestEvent::Added(50),
        ],
    )
    .await;

    let runner = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    runner
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // All 5 events processed
    let (position, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(position, Version::new(5).unwrap());
    assert_eq!(
        state,
        CountState {
            count: 5,
            total: 150
        }
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. ProjectionError display tests (moved from nexus-store)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn projection_error_displays_projector_variant() {
    let err: ProjectionError<
        TestProjectionError,
        std::io::Error,
        std::convert::Infallible,
        std::io::Error,
    > = ProjectionError::Projector(TestProjectionError);
    let msg = err.to_string();
    assert!(msg.contains("projector"), "expected 'projector' in: {msg}");
}

#[test]
fn projection_error_snapshot_variant_is_unconstructable_when_infallible() {
    let _err: ProjectionError<
        TestProjectionError,
        std::io::Error,
        std::convert::Infallible,
        std::io::Error,
    > = ProjectionError::Projector(TestProjectionError);
    // If this compiles, the Snapshot(Infallible) variant is unconstructable.
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Typestate / Phased Startup Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn initialize_returns_starting_on_first_run() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("fresh-stream".into());

    let runner = Projection::builder(stream_id)
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let ready = runner.initialize().await.unwrap();
    assert_eq!(ready.decision(), StartupDecision::Fresh);
}

#[tokio::test]
async fn initialize_returns_resuming_after_successful_run() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run
    Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — should resume
    let runner2 = Projection::builder(stream_id)
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let ready = runner2.initialize().await.unwrap();
    assert_eq!(ready.decision(), StartupDecision::Resume);
}

#[tokio::test]
async fn schema_bump_resolves_to_fresh() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run with schema v1
    Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run with schema v2 — the schema-mismatched snapshot is invisible
    // to hydrate, so initialize() resolves to Fresh and replays from scratch.
    let runner2 = Projection::builder(stream_id)
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    let ready = runner2.initialize().await.unwrap();
    assert_eq!(ready.decision(), StartupDecision::Fresh);
}

#[tokio::test]
async fn force_rebuild_replays_from_beginning() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // First run
    Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — force rebuild despite valid state
    let runner2 = Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let ready = runner2.initialize().await.unwrap();
    assert_eq!(ready.decision(), StartupDecision::Resume);

    // Force rebuild and run
    let rebuilt = ready.rebuild();
    assert_eq!(rebuilt.decision(), StartupDecision::Rebuild);
    rebuilt
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should be freshly computed from initial()
    let (_, state) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );
}

#[tokio::test]
async fn resuming_accessors_return_correct_values() {
    let store = Arc::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run to create checkpoint + state
    Projection::builder(stream_id.clone())
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build()
        .initialize()
        .await
        .unwrap()
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Second run — inspect Resuming variant
    let runner2 = Projection::builder(stream_id)
        .subscription(Arc::clone(&store))
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .snapshot_store(&snapshots)
        .build();

    let ready = runner2.initialize().await.unwrap();
    assert_eq!(ready.decision(), StartupDecision::Resume);

    let ProjectionStatus::Idle {
        ref state,
        checkpoint,
    } = *ready.status()
    else {
        panic!("expected Idle");
    };
    assert_eq!(checkpoint, Some(Version::new(1).unwrap()));
    assert_eq!(
        *state,
        CountState {
            count: 1,
            total: 10
        }
    );
}
