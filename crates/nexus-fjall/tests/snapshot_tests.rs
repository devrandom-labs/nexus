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
use std::num::NonZeroU32;

use nexus::{Id, Version};
use nexus_fjall::FjallStore;
use nexus_store::snapshot::{PendingSnapshot, SnapshotStore};

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

impl Id for TestId {
    const BYTE_LEN: usize = 0;
}

fn tid(s: &str) -> TestId {
    TestId(s.to_owned())
}

fn temp_store() -> (FjallStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    (store, dir)
}

/// Helper: append events to create a stream so snapshots have something to reference.
async fn setup_stream(store: &FjallStore, id: &TestId, event_count: u64) {
    use nexus_store::envelope::pending_envelope;
    use nexus_store::store::RawEventStore;

    let mut envs = Vec::new();
    for i in 1..=event_count {
        envs.push(
            pending_envelope(Version::new(i).unwrap())
                .event_type("TestEvent")
                .payload(format!("payload-{i}").into_bytes())
                .build_without_metadata(),
        );
    }
    store.append(id, None, &envs).await.unwrap();
}

// ── 1. Sequence/Protocol Tests ─────────────────────────────────────

#[tokio::test]
async fn save_then_load_roundtrips() {
    let (store, _dir) = temp_store();
    let id = tid("agg-1");
    setup_stream(&store, &id, 5).await;

    let snap = PendingSnapshot::new(Version::new(5).unwrap(), SV1, vec![1, 2, 3]);
    store.save_snapshot(&id, &snap).await.unwrap();

    let loaded = store.load_snapshot(&id, SV1).await.unwrap().unwrap();
    assert_eq!(loaded.version(), Version::new(5).unwrap());
    assert_eq!(loaded.schema_version(), SV1);
    assert_eq!(loaded.payload(), &[1, 2, 3]);
}

#[tokio::test]
async fn save_overwrites_previous_snapshot() {
    let (store, _dir) = temp_store();
    let id = tid("agg-1");
    setup_stream(&store, &id, 10).await;

    let snap1 = PendingSnapshot::new(Version::new(5).unwrap(), SV1, vec![1]);
    store.save_snapshot(&id, &snap1).await.unwrap();

    let snap2 = PendingSnapshot::new(
        Version::new(10).unwrap(),
        NonZeroU32::new(2).unwrap(),
        vec![2, 3],
    );
    store.save_snapshot(&id, &snap2).await.unwrap();

    let sv2 = NonZeroU32::new(2).unwrap();
    let loaded = store.load_snapshot(&id, sv2).await.unwrap().unwrap();
    assert_eq!(loaded.version(), Version::new(10).unwrap());
    assert_eq!(loaded.schema_version(), sv2);
    assert_eq!(loaded.payload(), &[2, 3]);

    // Old schema version should return None (filtered at store level).
    assert!(store.load_snapshot(&id, SV1).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_then_load_returns_none() {
    let (store, _dir) = temp_store();
    let id = tid("agg-1");
    setup_stream(&store, &id, 3).await;

    let snap = PendingSnapshot::new(Version::new(3).unwrap(), SV1, vec![1]);
    store.save_snapshot(&id, &snap).await.unwrap();
    store.delete_snapshot(&id).await.unwrap();

    assert!(store.load_snapshot(&id, SV1).await.unwrap().is_none());
}

// ── 2. Lifecycle Tests ─────────────────────────────────────────────

#[tokio::test]
async fn snapshot_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    let id = tid("agg-1");

    // First session: create stream + snapshot
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        setup_stream(&store, &id, 5).await;
        let snap = PendingSnapshot::new(Version::new(5).unwrap(), SV1, vec![42, 43, 44]);
        store.save_snapshot(&id, &snap).await.unwrap();
    }

    // Second session: reopen + verify snapshot
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        let loaded = store.load_snapshot(&id, SV1).await.unwrap().unwrap();
        assert_eq!(loaded.version(), Version::new(5).unwrap());
        assert_eq!(loaded.schema_version(), SV1);
        assert_eq!(loaded.payload(), &[42, 43, 44]);
    }
}

// ── 3. Defensive Boundary Tests ────────────────────────────────────

#[tokio::test]
async fn load_nonexistent_stream_returns_none() {
    let (store, _dir) = temp_store();
    let result = store.load_snapshot(&tid("nope"), SV1).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn load_existing_stream_without_snapshot_returns_none() {
    let (store, _dir) = temp_store();
    let id = tid("agg-1");
    setup_stream(&store, &id, 3).await;

    let result = store.load_snapshot(&id, SV1).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn delete_nonexistent_is_ok() {
    let (store, _dir) = temp_store();
    let result = store.delete_snapshot(&tid("nope")).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn save_to_nonexistent_stream_is_noop() {
    let (store, _dir) = temp_store();
    // No stream exists — save should silently succeed (no-op)
    let snap = PendingSnapshot::new(Version::new(1).unwrap(), SV1, vec![1]);
    let result = store.save_snapshot(&tid("nope"), &snap).await;
    assert!(result.is_ok());

    // And loading returns None
    assert!(
        store
            .load_snapshot(&tid("nope"), SV1)
            .await
            .unwrap()
            .is_none()
    );
}

// ── 4. Isolation Tests ─────────────────────────────────────────────

#[tokio::test]
async fn different_streams_have_separate_snapshots() {
    let (store, _dir) = temp_store();
    let id1 = tid("agg-1");
    let id2 = tid("agg-2");
    setup_stream(&store, &id1, 5).await;
    setup_stream(&store, &id2, 10).await;

    let snap1 = PendingSnapshot::new(Version::new(5).unwrap(), SV1, vec![1]);
    store.save_snapshot(&id1, &snap1).await.unwrap();

    let snap2 = PendingSnapshot::new(Version::new(10).unwrap(), SV1, vec![2]);
    store.save_snapshot(&id2, &snap2).await.unwrap();

    let loaded1 = store.load_snapshot(&id1, SV1).await.unwrap().unwrap();
    let loaded2 = store.load_snapshot(&id2, SV1).await.unwrap().unwrap();

    assert_eq!(loaded1.version(), Version::new(5).unwrap());
    assert_eq!(loaded1.payload(), &[1]);
    assert_eq!(loaded2.version(), Version::new(10).unwrap());
    assert_eq!(loaded2.payload(), &[2]);
}
