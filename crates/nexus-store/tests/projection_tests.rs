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
use std::num::NonZeroU32;

use nexus::Version;
use nexus_store::projection::{PendingState, PersistedState, StateStore};

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
