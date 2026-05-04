#![cfg(feature = "projection")]
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

use nexus::Version;
use nexus_store::Projector;
use nexus_store::state::{
    AfterEventTypes, EveryNEvents, PendingState, PersistTrigger, PersistedState, StateStore,
};

const SV1: NonZeroU32 = NonZeroU32::MIN;

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

#[test]
fn pending_state_stores_version_and_payload() {
    let version = Version::new(42).unwrap();
    let payload = vec![1, 2, 3];
    let sv = NonZeroU32::new(1).unwrap();
    let state = PendingState::new(version, sv, payload.clone());

    assert_eq!(state.version(), version);
    assert_eq!(state.schema_version(), sv);
    assert_eq!(state.state(), &payload);
}

#[test]
fn persisted_state_stores_version_and_payload() {
    let version = Version::new(10).unwrap();
    let sv = NonZeroU32::new(2).unwrap();
    let state = PersistedState::new(version, sv, vec![4, 5, 6]);

    assert_eq!(state.version(), version);
    assert_eq!(state.schema_version(), sv);
    assert_eq!(state.state(), &[4, 5, 6]);
}

// ── () no-op StateStore ─────────────────────────────────────────

#[tokio::test]
async fn unit_state_store_returns_none() {
    let store: () = ();
    let id = TestId("proj-1".into());
    let result: Result<Option<PersistedState<Vec<u8>>>, _> = store.load(&id, SV1).await;
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn unit_state_store_save_succeeds() {
    let store: () = ();
    let id = TestId("proj-1".into());
    let state = PendingState::new(
        Version::new(1).unwrap(),
        NonZeroU32::new(1).unwrap(),
        vec![1, 2, 3],
    );
    let result = store.save(&id, &state).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn unit_state_store_delete_succeeds() {
    let store: () = ();
    let id = TestId("proj-1".into());
    let result: Result<(), _> = StateStore::<Vec<u8>>::delete(&store, &id).await;
    assert!(result.is_ok());
}

// ── EveryNEvents ────────────────────────────────────────────────────

#[test]
fn proj_every_n_events_triggers_on_boundary_crossing() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    let v99 = Some(Version::new(99).unwrap());
    let v100 = Version::new(100).unwrap();
    assert!(trigger.should_persist(v99, v100, std::iter::empty::<&str>()));

    let v98 = Some(Version::new(98).unwrap());
    let v99_ver = Version::new(99).unwrap();
    assert!(!trigger.should_persist(v98, v99_ver, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_n_events_triggers_on_batch_crossing_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    let old = Some(Version::new(96).unwrap());
    let new = Version::new(103).unwrap();
    assert!(trigger.should_persist(old, new, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_n_events_first_save_triggers_at_boundary() {
    let trigger = EveryNEvents(NonZeroU64::new(100).unwrap());

    let new = Version::new(100).unwrap();
    assert!(trigger.should_persist(None, new, std::iter::empty::<&str>()));

    let new_50 = Version::new(50).unwrap();
    assert!(!trigger.should_persist(None, new_50, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_1_event_always_triggers() {
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
fn proj_after_event_types_triggers_on_matching_event() {
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
fn proj_after_event_types_triggers_if_any_event_in_batch_matches() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);

    assert!(trigger.should_persist(
        None,
        Version::new(5).unwrap(),
        ["ItemAdded", "OrderCompleted"].into_iter(),
    ));
}

#[test]
fn proj_after_event_types_does_not_trigger_on_empty_events() {
    let trigger = AfterEventTypes::new(&["OrderCompleted"]);
    assert!(!trigger.should_persist(None, Version::new(5).unwrap(), std::iter::empty::<&str>()));
}

// ── InMemoryStateStore ───────────────────────────────────────────

#[cfg(feature = "testing")]
mod in_memory_tests {
    use super::*;
    use nexus_store::state::InMemoryStateStore;

    #[tokio::test]
    async fn load_returns_none_when_empty() {
        let store = InMemoryStateStore::<Vec<u8>>::new();
        let result = store.load(&TestId("proj-1".into()), SV1).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let store = InMemoryStateStore::<Vec<u8>>::new();
        let id = TestId("proj-1".into());
        let version = Version::new(10).unwrap();
        let state = PendingState::new(version, NonZeroU32::new(1).unwrap(), vec![1, 2, 3]);

        store.save(&id, &state).await.unwrap();
        let loaded = store.load(&id, SV1).await.unwrap().unwrap();

        assert_eq!(loaded.version(), version);
        assert_eq!(loaded.schema_version(), NonZeroU32::new(1).unwrap());
        assert_eq!(loaded.state(), &[1, 2, 3]);
    }

    #[tokio::test]
    async fn save_overwrites_previous_state() {
        let store = InMemoryStateStore::<Vec<u8>>::new();
        let id = TestId("proj-1".into());

        let state1 = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1],
        );
        store.save(&id, &state1).await.unwrap();

        let state2 = PendingState::new(
            Version::new(20).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![2],
        );
        store.save(&id, &state2).await.unwrap();

        let loaded = store.load(&id, SV1).await.unwrap().unwrap();
        assert_eq!(loaded.version(), Version::new(20).unwrap());
        assert_eq!(loaded.state(), &[2]);
    }

    #[tokio::test]
    async fn different_projections_have_separate_state() {
        let store = InMemoryStateStore::<Vec<u8>>::new();

        let state1 = PendingState::new(
            Version::new(5).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1],
        );
        store.save(&TestId("proj-1".into()), &state1).await.unwrap();

        let state2 = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![2],
        );
        store.save(&TestId("proj-2".into()), &state2).await.unwrap();

        let loaded1 = store
            .load(&TestId("proj-1".into()), SV1)
            .await
            .unwrap()
            .unwrap();
        let loaded2 = store
            .load(&TestId("proj-2".into()), SV1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded1.version(), Version::new(5).unwrap());
        assert_eq!(loaded2.version(), Version::new(10).unwrap());
    }

    #[tokio::test]
    async fn delete_removes_state() {
        let store = InMemoryStateStore::<Vec<u8>>::new();
        let id = TestId("proj-1".into());

        let state = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1],
        );
        store.save(&id, &state).await.unwrap();

        store.delete(&id).await.unwrap();
        let loaded = store.load(&id, SV1).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent_is_ok() {
        let store = InMemoryStateStore::<Vec<u8>>::new();
        let result = store.delete(&TestId("nope".into())).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn load_filters_by_schema_version() {
        let store = InMemoryStateStore::<Vec<u8>>::new();
        let id = TestId("proj-1".into());

        let state = PendingState::new(
            Version::new(10).unwrap(),
            NonZeroU32::new(1).unwrap(),
            vec![1, 2, 3],
        );
        store.save(&id, &state).await.unwrap();

        // Matching schema version returns state
        let loaded = store.load(&id, NonZeroU32::new(1).unwrap()).await.unwrap();
        assert!(loaded.is_some());

        // Mismatched schema version returns None
        let loaded = store.load(&id, NonZeroU32::new(2).unwrap()).await.unwrap();
        assert!(loaded.is_none());
    }
}

// ── Projector trait ─────────────────────────────────────────────────

/// A test projector that counts events and sums a field.
struct CountingProjector;

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

impl nexus::Message for TestEvent {}
impl nexus::DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            TestEvent::Added(_) => "Added",
            TestEvent::Removed(_) => "Removed",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("overflow")]
struct TestProjectionError;

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

#[test]
fn projector_initial_state() {
    let proj = CountingProjector;
    let state = proj.initial();
    assert_eq!(state, CountState { count: 0, total: 0 });
}

#[test]
fn projector_folds_events() {
    let proj = CountingProjector;
    let state = proj.initial();

    let state = proj.apply(state, &TestEvent::Added(10)).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 1,
            total: 10
        }
    );

    let state = proj.apply(state, &TestEvent::Added(20)).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 2,
            total: 30
        }
    );

    let state = proj.apply(state, &TestEvent::Removed(5)).unwrap();
    assert_eq!(
        state,
        CountState {
            count: 3,
            total: 25
        }
    );
}

#[test]
fn projector_returns_error_on_overflow() {
    let proj = CountingProjector;
    let state = CountState {
        count: 0,
        total: u64::MAX,
    };
    let result = proj.apply(state, &TestEvent::Added(1));
    assert!(result.is_err());
}

#[test]
fn projector_returns_error_on_underflow() {
    let proj = CountingProjector;
    let state = CountState { count: 0, total: 0 };
    let result = proj.apply(state, &TestEvent::Removed(1));
    assert!(result.is_err());
}
