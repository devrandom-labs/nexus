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
use nexus_store::projection::{
    AfterEventTypes as ProjAfterEventTypes, EveryNEvents as ProjEveryNEvents, PendingState,
    PersistedState, ProjectionTrigger, StateStore,
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
    assert_eq!(state.payload(), &payload);
}

#[test]
fn persisted_state_stores_version_and_payload() {
    let version = Version::new(10).unwrap();
    let sv = NonZeroU32::new(2).unwrap();
    let state = PersistedState::new(version, sv, vec![4, 5, 6]);

    assert_eq!(state.version(), version);
    assert_eq!(state.schema_version(), sv);
    assert_eq!(state.payload(), &[4, 5, 6]);
}

// ── () no-op StateStore ─────────────────────────────────────────

#[tokio::test]
async fn unit_state_store_returns_none() {
    let store = ();
    let id = TestId("proj-1".into());
    let result = store.load(&id, SV1).await;
    assert!(result.unwrap().is_none());
}

#[tokio::test]
async fn unit_state_store_save_succeeds() {
    let store = ();
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
    let store = ();
    let id = TestId("proj-1".into());
    let result = store.delete(&id).await;
    assert!(result.is_ok());
}

// ── EveryNEvents ────────────────────────────────────────────────────

#[test]
fn proj_every_n_events_triggers_on_boundary_crossing() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(100).unwrap());

    let v99 = Some(Version::new(99).unwrap());
    let v100 = Version::new(100).unwrap();
    assert!(trigger.should_project(v99, v100, std::iter::empty::<&str>()));

    let v98 = Some(Version::new(98).unwrap());
    let v99_ver = Version::new(99).unwrap();
    assert!(!trigger.should_project(v98, v99_ver, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_n_events_triggers_on_batch_crossing_boundary() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(100).unwrap());

    let old = Some(Version::new(96).unwrap());
    let new = Version::new(103).unwrap();
    assert!(trigger.should_project(old, new, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_n_events_first_save_triggers_at_boundary() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(100).unwrap());

    let new = Version::new(100).unwrap();
    assert!(trigger.should_project(None, new, std::iter::empty::<&str>()));

    let new_50 = Version::new(50).unwrap();
    assert!(!trigger.should_project(None, new_50, std::iter::empty::<&str>()));
}

#[test]
fn proj_every_1_event_always_triggers() {
    let trigger = ProjEveryNEvents(NonZeroU64::new(1).unwrap());
    assert!(trigger.should_project(None, Version::new(1).unwrap(), std::iter::empty::<&str>()));
    assert!(trigger.should_project(
        Some(Version::new(1).unwrap()),
        Version::new(2).unwrap(),
        std::iter::empty::<&str>(),
    ));
}

// ── AfterEventTypes ─────────────────────────────────────────────────

#[test]
fn proj_after_event_types_triggers_on_matching_event() {
    let trigger = ProjAfterEventTypes::new(&["OrderCompleted", "OrderCancelled"]);

    assert!(trigger.should_project(
        None,
        Version::new(5).unwrap(),
        ["OrderCompleted"].into_iter()
    ));
    assert!(trigger.should_project(
        None,
        Version::new(5).unwrap(),
        ["OrderCancelled"].into_iter()
    ));
    assert!(!trigger.should_project(None, Version::new(5).unwrap(), ["ItemAdded"].into_iter()));
}

#[test]
fn proj_after_event_types_triggers_if_any_event_in_batch_matches() {
    let trigger = ProjAfterEventTypes::new(&["OrderCompleted"]);

    assert!(trigger.should_project(
        None,
        Version::new(5).unwrap(),
        ["ItemAdded", "OrderCompleted"].into_iter(),
    ));
}

#[test]
fn proj_after_event_types_does_not_trigger_on_empty_events() {
    let trigger = ProjAfterEventTypes::new(&["OrderCompleted"]);
    assert!(!trigger.should_project(None, Version::new(5).unwrap(), std::iter::empty::<&str>()));
}
