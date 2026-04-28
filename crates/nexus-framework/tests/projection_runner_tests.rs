#![cfg(feature = "testing")]
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

use nexus::{DomainEvent, Message, Version};
use nexus_framework::projection::{
    NoStatePersistence, ProjectionError, ProjectionRunner, StatePersistence, WithStatePersistence,
};
use nexus_store::projection::{
    EveryNEvents as ProjEveryNEvents, InMemoryStateStore, Projector, StateStore,
};
use nexus_store::testing::InMemoryStore;
use nexus_store::{CheckpointStore, Codec, EventStreamExt, RawEventStore, pending_envelope};

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

impl Codec<TestEvent> for TestEventCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &TestEvent) -> Result<Vec<u8>, Self::Error> {
        match event {
            TestEvent::Added(n) => {
                let mut buf = vec![0u8]; // tag
                buf.extend_from_slice(&n.to_le_bytes());
                Ok(buf)
            }
            TestEvent::Removed(n) => {
                let mut buf = vec![1u8]; // tag
                buf.extend_from_slice(&n.to_le_bytes());
                Ok(buf)
            }
        }
    }

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

/// Simple state codec for tests.
struct TestStateCodec;

impl Codec<CountState> for TestStateCodec {
    type Error = std::io::Error;

    fn encode(&self, state: &CountState) -> Result<Vec<u8>, Self::Error> {
        let mut buf = Vec::with_capacity(16);
        buf.extend_from_slice(&state.count.to_le_bytes());
        buf.extend_from_slice(&state.total.to_le_bytes());
        Ok(buf)
    }

    fn decode(&self, _name: &str, payload: &[u8]) -> Result<CountState, Self::Error> {
        if payload.len() != 16 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad len",
            ));
        }
        let count = u64::from_le_bytes(payload[..8].try_into().unwrap());
        let total = u64::from_le_bytes(payload[8..].try_into().unwrap());
        Ok(CountState { count, total })
    }
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
                .build(())
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
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
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
    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    // Run with delayed shutdown to let the runner process existing events
    let shutdown = async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };

    runner.run(shutdown).await.unwrap();

    // Verify checkpoint
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(3).unwrap()));

    // Verify state
    let persisted = state_store
        .load(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
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
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
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

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
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
    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner2
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: checkpoint at 5, state = count:5 total:95
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(5).unwrap()));

    let persisted = state_store
        .load(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
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
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
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
    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .trigger(ProjEveryNEvents(NonZeroU64::new(3).unwrap()))
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Checkpoint should be at version 5 (flushed on shutdown)
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(5).unwrap()));

    // State should reflect all 5 events (flushed on shutdown)
    let persisted = state_store
        .load(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 5,
            total: 15
        }
    );
}

#[tokio::test]
async fn runner_works_without_state_persistence() {
    let store = InMemoryStore::new();
    let stream_id = TestId("stream-1".into());

    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // No state_store — only checkpoints
    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Checkpoint should still work
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn runner_rebuilds_from_beginning_on_schema_version_bump() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
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

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // Verify: checkpoint at 3, state saved with schema v1
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(3).unwrap()));

    // Now restart with schema v2 — should rebuild from beginning
    let runner2 = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner2
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should reflect ALL 3 events from initial() — not resume from v3
    let persisted = state_store
        .load(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 60
        }
    );

    // Checkpoint should be updated to v3
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(3).unwrap()));
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Lifecycle Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_immediate_shutdown_with_no_events() {
    let store = InMemoryStore::new();
    let stream_id = TestId("empty-stream".into());

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    // Immediate shutdown — should return Ok without errors
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tx.send(()).unwrap();
    let shutdown = async {
        rx.await.ok();
    };

    runner.run(shutdown).await.unwrap();

    // No checkpoint saved (no events processed)
    let cp = store.load(&stream_id).await.unwrap();
    assert!(cp.is_none());
}

#[tokio::test]
async fn runner_graceful_shutdown_flushes_dirty_state() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
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
    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .trigger(ProjEveryNEvents(NonZeroU64::new(100).unwrap()))
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should be flushed on shutdown even though trigger didn't fire
    let persisted = state_store
        .load(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );

    // Checkpoint should also be saved
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn runner_stale_state_falls_back_to_initial() {
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
    let stream_id = TestId("stream-1".into());

    // Pre-save state with schema version 1
    use nexus_store::projection::PendingState;
    let old_state = PendingState::new(
        Version::new(5).unwrap(),
        NonZeroU32::MIN,
        TestStateCodec
            .encode(&CountState {
                count: 99,
                total: 999,
            })
            .unwrap(),
    );
    state_store.save(&stream_id, &old_state).await.unwrap();

    // Append 2 events
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // Runner with schema version 2 — stale state should be ignored
    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .state_schema_version(NonZeroU32::new(2).unwrap())
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // State should start from initial(), not from stale v1 state
    let persisted = state_store
        .load(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
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
async fn runner_returns_projector_error_on_apply_failure() {
    let store = InMemoryStore::new();
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

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    let result = runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        })
        .await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), ProjectionError::Projector(_)));
}

#[tokio::test]
async fn runner_returns_event_codec_error_on_bad_payload() {
    let store = InMemoryStore::new();
    let stream_id = TestId("stream-1".into());

    // Append a raw event with garbage payload
    let bad_envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("Added")
        .payload(vec![0xFF]) // invalid: too short
        .build(());
    store
        .append(&stream_id, None, &[bad_envelope])
        .await
        .unwrap();

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .build();

    let result = runner
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
    let store = InMemoryStore::new();
    let state_store = InMemoryStateStore::new();
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

    let runner = ProjectionRunner::builder(stream_id.clone())
        .subscription(&store)
        .checkpoint(&store)
        .projector(CountingProjector)
        .event_codec(TestEventCodec)
        .state_store(&state_store, TestStateCodec)
        .build();

    runner
        .run(async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        })
        .await
        .unwrap();

    // All 5 events processed
    let cp = store.load(&stream_id).await.unwrap();
    assert_eq!(cp, Some(Version::new(5).unwrap()));

    let persisted = state_store
        .load(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    let state: CountState = TestStateCodec.decode("", persisted.payload()).unwrap();
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
        std::io::Error,
    > = ProjectionError::Projector(TestProjectionError);
    let msg = err.to_string();
    assert!(msg.contains("projector"), "expected 'projector' in: {msg}");
}

#[test]
fn projection_error_state_variant_is_unconstructable_when_infallible() {
    let _err: ProjectionError<
        TestProjectionError,
        std::io::Error,
        std::convert::Infallible,
        std::io::Error,
        std::io::Error,
    > = ProjectionError::Projector(TestProjectionError);
    // If this compiles, the State(Infallible) variant is unconstructable. ✓
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. StatePersistence trait tests (moved from nexus-store)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn no_state_persistence_load_returns_none() {
    let sp = NoStatePersistence;
    let id = TestId("proj-1".into());
    let result: Result<Option<(CountState, _)>, _> = sp.load(&id).await;
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn no_state_persistence_save_succeeds() {
    let sp = NoStatePersistence;
    let id = TestId("proj-1".into());
    let state = CountState {
        count: 1,
        total: 10,
    };
    let version = Version::new(5).unwrap();
    let result = sp.save(&id, version, &state).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn with_state_persistence_save_then_load_roundtrips() {
    let store = InMemoryStateStore::new();
    let sp = WithStatePersistence::new(&store, TestStateCodec, NonZeroU32::MIN);

    let id = TestId("proj-1".into());
    let state = CountState {
        count: 3,
        total: 42,
    };
    let version = Version::new(10).unwrap();

    sp.save(&id, version, &state).await.unwrap();

    let loaded = sp.load(&id).await.unwrap().unwrap();
    assert_eq!(loaded.0, state);
    assert_eq!(loaded.1, version);
}

#[tokio::test]
async fn with_state_persistence_load_returns_none_when_empty() {
    let store = InMemoryStateStore::new();
    let sp = WithStatePersistence::new(&store, TestStateCodec, NonZeroU32::MIN);

    let id = TestId("proj-1".into());
    let result: Option<(CountState, _)> = sp.load(&id).await.unwrap();
    assert!(result.is_none());
}

#[test]
fn no_state_persistence_does_not_persist_state() {
    assert!(
        !<NoStatePersistence as StatePersistence<CountState>>::persists_state(&NoStatePersistence)
    );
}

#[test]
fn with_state_persistence_persists_state() {
    let store = InMemoryStateStore::new();
    let sp = WithStatePersistence::new(&store, TestStateCodec, NonZeroU32::MIN);
    assert!(sp.persists_state());
}

#[tokio::test]
async fn with_state_persistence_load_returns_none_on_schema_mismatch() {
    let store = InMemoryStateStore::new();
    let sp = WithStatePersistence::new(&store, TestStateCodec, NonZeroU32::MIN);

    let id = TestId("proj-1".into());
    sp.save(
        &id,
        Version::new(5).unwrap(),
        &CountState { count: 1, total: 1 },
    )
    .await
    .unwrap();

    // Load with a different schema version
    let sp_v2 = WithStatePersistence::new(&store, TestStateCodec, NonZeroU32::new(2).unwrap());
    let result: Option<(CountState, _)> = sp_v2.load(&id).await.unwrap();
    assert!(result.is_none());
}
