//! Integration tests that exercise the consumer-owned `run_projection` loop
//! (these replace the former projection-runner tests).
//!
//! The loop itself lives in `examples/projection-tokio/src/lib.rs`; the
//! nexus-framework typestate runner it supersedes has been retired. The four
//! cross-cutting test categories (sequence/protocol, lifecycle,
//! defensive-boundary, linearizability) are covered below against the plain
//! `run_projection` function.
//!
//! Conversion notes
//! ────────────────
//! • `ProjectionError::Projector(_)` / `::EventCodec(_)` matches → replaced
//!   by `is_err()` + `to_string().contains(...)` on the boxed error.
//! • All startup-decision and projection-status invariants → verified via
//!   `snapshot_store.hydrate` after the run.
//! • Two `ProjectionError` *unit* tests that directly constructed the old
//!   `ProjectionError<...>` type were not ported: that enum no longer exists.
//!   Their coverage (error display + Infallible variant) is moot — the
//!   boxed-error path is tested by the codec/projector error integration tests
//!   below.
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

use futures::StreamExt;
use nexus::{DomainEvent, Message, Version};
use nexus_example_projection_tokio::run_projection;
use nexus_store::testing::InMemoryStore;
use nexus_store::{
    Decode, Encode, EveryNEvents, InMemorySnapshotStore, Projector, RawEventStore, SnapshotStore,
    Store, Subscription, pending_envelope,
};

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
    type Output<'a> = TestEvent;
    type Error = std::io::Error;

    fn decode<'a>(
        &'a self,
        env: &'a nexus_store::PersistedEnvelope,
    ) -> Result<TestEvent, Self::Error> {
        let payload = env.payload();
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
async fn append_events(store: &Store<InMemoryStore>, stream_id: &TestId, events: &[TestEvent]) {
    let codec = TestEventCodec;
    let raw = store.raw();
    let current_len = {
        let stream = raw
            .read_stream(
                &nexus_store::StreamKey::from_slice(stream_id.as_ref()),
                Version::INITIAL,
            )
            .await
            .unwrap();
        stream.count().await
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
                .expect("valid payload")
                .build()
        })
        .collect();

    let expected = Version::new(base_version).filter(|_| base_version > 0);
    raw.append(
        &nexus_store::StreamKey::from_slice(stream_id.as_ref()),
        expected,
        &envelopes,
    )
    .await
    .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Sequence/Protocol Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_processes_events_and_checkpoints() {
    let store = Store::new(InMemoryStore::new());
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

    // Run with delayed shutdown to let the runner process existing events.
    // Pass &snapshots so we can hydrate after the call.
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    let store = Store::new(InMemoryStore::new());
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

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    let store = Store::new(InMemoryStore::new());
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
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::new(3).unwrap()),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
async fn runner_resumes_normally_after_rebuild_completes() {
    let store = Store::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Phase 1: initial run with schema v1
    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    .unwrap();

    // Phase 2: restart with schema v2 — replays from the beginning
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::new(2).unwrap(),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    .unwrap();

    // Phase 3: append more events, restart with same schema v2 — normal resume
    append_events(&store, &stream_id, &[TestEvent::Added(30)]).await;

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::new(2).unwrap(),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    let store = Store::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("empty-stream".into());

    // Immediate shutdown — should return Ok without errors
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tx.send(()).unwrap();
    let shutdown = async {
        rx.await.ok();
    };

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        shutdown,
    )
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
    let store = Store::new(InMemoryStore::new());
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

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::new(100).unwrap()),
        NonZeroU32::new(2).unwrap(),
        async {
            rx.await.ok();
        },
    )
    .await
    .unwrap();

    // Phase 3: restart with schema v2 — whether Phase 2 processed some
    // events or none, the final result must be correct (idempotent)
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::new(2).unwrap(),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    let store = Store::new(InMemoryStore::new());
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
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::new(100).unwrap()),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    let store = Store::new(InMemoryStore::new());
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
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::new(2).unwrap(),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    // Observable invariant: a first run (no prior snapshot) folds from initial(),
    // not from stale state. Verified by confirming the committed state equals the
    // fold of the two appended events from CountState { 0, 0 }.
    let store = Store::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(
        &store,
        &stream_id,
        &[TestEvent::Added(10), TestEvent::Added(20)],
    )
    .await;

    // Verify no prior snapshot exists — confirms this is truly a first run.
    let before = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap();
    assert!(before.is_none(), "expected no snapshot before first run");

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
    let store = Store::new(InMemoryStore::new());
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

    let result = run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        },
    )
    .await;

    assert!(result.is_err(), "expected projector error");
    // The boxed error preserves the #[source] chain; TestProjectionError's
    // Display is "projection overflow".
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("projection overflow"),
        "expected 'projection overflow' in error: {msg}"
    );
}

#[tokio::test]
async fn runner_returns_event_codec_error_on_bad_payload() {
    let store = Store::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    // Append a raw event with garbage payload
    let bad_envelope = pending_envelope(Version::new(1).unwrap())
        .event_type("Added")
        .payload(vec![0xFF]) // invalid: too short
        .expect("valid payload")
        .build();
    store
        .raw()
        .append(
            &nexus_store::StreamKey::from_slice(stream_id.as_ref()),
            None,
            &[bad_envelope],
        )
        .await
        .unwrap();

    let result = run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        },
    )
    .await;

    assert!(result.is_err(), "expected codec error");
    // TestEventCodec::decode returns io::Error with message "bad len"
    // when payload.len() != 9.
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("bad len"),
        "expected 'bad len' in error: {msg}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Linearizability/Isolation Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_catches_up_and_processes_all_existing_events() {
    let store = Store::new(InMemoryStore::new());
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

    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
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
// 5. Checkpoint Resume and Schema-Bump Replay
//
// These tests verify hydrate-based startup invariants: that a second run
// correctly resumes from the stored checkpoint, and that a schema-version
// bump forces a full replay from initial().
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn runner_resumes_from_checkpoint_on_second_run() {
    // Observable invariant: the second run resumes from the checkpoint left by
    // the first run, so the committed state after the second run reflects only
    // the event added between runs (not a re-fold of the first event).
    let store = Store::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    .unwrap();

    // Checkpoint at v1, count:1, total:10
    let (cp1, state1) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cp1, Version::new(1).unwrap());
    assert_eq!(
        state1,
        CountState {
            count: 1,
            total: 10
        }
    );

    // Append a second event
    append_events(&store, &stream_id, &[TestEvent::Added(5)]).await;

    // Second run — must resume from v1, fold only the new event
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    .unwrap();

    let (cp2, state2) = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cp2, Version::new(2).unwrap());
    // count:2, total:15 — not count:1/total:5 (fresh) or count:2/total:20 (double-fold)
    assert_eq!(
        state2,
        CountState {
            count: 2,
            total: 15
        }
    );
}

#[tokio::test]
async fn schema_bump_resolves_to_fresh() {
    // Observable invariant: running with v2 after a v1 snapshot produces a state
    // that is a full re-fold from initial(), not a delta from the v1 checkpoint.
    let store = Store::new(InMemoryStore::new());
    let snapshots = snapshot_store();
    let stream_id = TestId("stream-1".into());

    append_events(&store, &stream_id, &[TestEvent::Added(10)]).await;

    // First run with schema v1
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::MIN,
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    .unwrap();

    // Confirm v1 snapshot was committed
    let v1 = snapshots
        .hydrate(&stream_id, NonZeroU32::MIN)
        .await
        .unwrap();
    assert!(v1.is_some(), "expected v1 snapshot to exist");

    // Second run with schema v2 — the schema-mismatched snapshot is invisible.
    // hydrate returns None → state starts from initial() → full replay.
    run_projection(
        stream_id.clone(),
        Subscription::new(&store),
        &snapshots,
        CountingProjector,
        TestEventCodec,
        EveryNEvents(NonZeroU64::MIN),
        NonZeroU32::new(2).unwrap(),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        },
    )
    .await
    .unwrap();

    // v2 snapshot must reflect a full fold of all 1 events from initial()
    let (pos2, state2) = snapshots
        .hydrate(&stream_id, NonZeroU32::new(2).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pos2, Version::new(1).unwrap());
    assert_eq!(
        state2,
        CountState {
            count: 1,
            total: 10
        }
    );
}
