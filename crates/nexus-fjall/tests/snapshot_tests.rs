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

use std::num::NonZeroU32;

use nexus::Version;
use nexus_fjall::FjallStore;
use nexus_store::StreamKey;
use nexus_store::state::SnapshotStore;

const SV1: NonZeroU32 = NonZeroU32::MIN;

fn sk(s: &str) -> StreamKey {
    StreamKey::from_slice(s.as_bytes())
}

fn temp_store() -> (FjallStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = FjallStore::builder(dir.path().join("db")).open().unwrap();
    (store, dir)
}

/// Helper: append events to create a stream so snapshots have something to reference.
async fn setup_stream(store: &FjallStore, id: &StreamKey, event_count: u64) {
    use nexus_store::envelope::pending_envelope;
    use nexus_store::store::RawEventStore;

    let mut envs = Vec::new();
    for i in 1..=event_count {
        envs.push(
            pending_envelope(Version::new(i).unwrap())
                .event_type("TestEvent")
                .payload(format!("payload-{i}").into_bytes())
                .expect("valid payload")
                .build(),
        );
    }
    store.append(id, None, &envs).await.unwrap();
}

// ── 1. Sequence/Protocol Tests ─────────────────────────────────────

#[tokio::test]
async fn commit_then_hydrate_roundtrips() {
    let (store, _dir) = temp_store();
    let id = sk("agg-1");
    setup_stream(&store, &id, 5).await;

    store
        .commit(&id, SV1, Version::new(5).unwrap(), &vec![1, 2, 3])
        .await
        .unwrap();

    let (version, state) = store.hydrate(&id, SV1).await.unwrap().unwrap();
    assert_eq!(version, Version::new(5).unwrap());
    assert_eq!(state, vec![1, 2, 3]);
}

#[tokio::test]
async fn commit_overwrites_previous_snapshot() {
    let (store, _dir) = temp_store();
    let id = sk("agg-1");
    setup_stream(&store, &id, 10).await;

    store
        .commit(&id, SV1, Version::new(5).unwrap(), &vec![1])
        .await
        .unwrap();

    let sv2 = NonZeroU32::new(2).unwrap();
    store
        .commit(&id, sv2, Version::new(10).unwrap(), &vec![2, 3])
        .await
        .unwrap();

    let (version, state) = store.hydrate(&id, sv2).await.unwrap().unwrap();
    assert_eq!(version, Version::new(10).unwrap());
    assert_eq!(state, vec![2, 3]);

    // Old schema version should return None (filtered at store level).
    assert!(store.hydrate(&id, SV1).await.unwrap().is_none());
}

// ── 2. Lifecycle Tests ─────────────────────────────────────────────

#[tokio::test]
async fn snapshot_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("db");
    let id = sk("agg-1");

    // First session: create stream + snapshot
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        setup_stream(&store, &id, 5).await;
        store
            .commit(&id, SV1, Version::new(5).unwrap(), &vec![42, 43, 44])
            .await
            .unwrap();
    }

    // Second session: reopen + verify snapshot
    {
        let store = FjallStore::builder(&db_path).open().unwrap();
        let (version, state) = store.hydrate(&id, SV1).await.unwrap().unwrap();
        assert_eq!(version, Version::new(5).unwrap());
        assert_eq!(state, vec![42, 43, 44]);
    }
}

// ── 3. Defensive Boundary Tests ────────────────────────────────────

#[tokio::test]
async fn hydrate_unknown_id_returns_none() {
    let (store, _dir) = temp_store();
    let result = store.hydrate(&sk("nope"), SV1).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn hydrate_id_without_snapshot_returns_none() {
    let (store, _dir) = temp_store();
    let id = sk("agg-1");
    setup_stream(&store, &id, 3).await;

    let result = store.hydrate(&id, SV1).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn commit_without_event_stream_is_persisted() {
    let (store, _dir) = temp_store();
    // No event stream exists — commit writes the snapshot unconditionally.
    store
        .commit(&sk("nope"), SV1, Version::new(1).unwrap(), &vec![1])
        .await
        .unwrap();

    // And hydrate reads it back regardless of stream existence.
    let (version, state) = store.hydrate(&sk("nope"), SV1).await.unwrap().unwrap();
    assert_eq!(version, Version::new(1).unwrap());
    assert_eq!(state, vec![1]);
}

// ── 4. Isolation Tests ─────────────────────────────────────────────

#[tokio::test]
async fn different_streams_have_separate_snapshots() {
    let (store, _dir) = temp_store();
    let id1 = sk("agg-1");
    let id2 = sk("agg-2");
    setup_stream(&store, &id1, 5).await;
    setup_stream(&store, &id2, 10).await;

    store
        .commit(&id1, SV1, Version::new(5).unwrap(), &vec![1])
        .await
        .unwrap();
    store
        .commit(&id2, SV1, Version::new(10).unwrap(), &vec![2])
        .await
        .unwrap();

    let (version1, state1) = store.hydrate(&id1, SV1).await.unwrap().unwrap();
    let (version2, state2) = store.hydrate(&id2, SV1).await.unwrap().unwrap();

    assert_eq!(version1, Version::new(5).unwrap());
    assert_eq!(state1, vec![1]);
    assert_eq!(version2, Version::new(10).unwrap());
    assert_eq!(state2, vec![2]);
}
