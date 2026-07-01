//! `nexus-postgres::PostgresStore` conformance against the canonical
//! [`EventStream`](nexus_store::EventStream) and `$all` read-path contract.
//!
//! Delegates every check to [`nexus_store_testing::assert_event_stream_conformance`]
//! and [`nexus_store_testing::assert_all_stream_conformance`].
//!
//! # Skip-without-DATABASE_URL
//!
//! Every test calls [`setup`] first. If `DATABASE_URL` is unset, `setup`
//! returns `None` and the test body returns immediately — the test *passes*
//! (no assertion is reached). This keeps `nix flake check` green locally
//! while the nixosTest supplies a real URL in CI.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::num::NonZeroU32;

use futures::StreamExt;
use nexus::Version;
use nexus_postgres::PostgresStore;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::value::SchemaVersion;
use nexus_store::{AppendError, PendingEnvelope, StreamKey};
use nexus_store_testing::{
    ConformanceRow, assert_all_stream_conformance, assert_event_stream_conformance,
};
use sqlx::PgPool;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// Connect to the store and truncate the events table for isolation.
///
/// Returns `None` (skip) if `DATABASE_URL` is unset. Returns both a
/// [`PgPool`] (for raw SQL) and a [`PostgresStore`] (the SUT).
async fn setup() -> Option<(PostgresStore, PgPool)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("connect pool");
    // Truncate for isolation — each test gets a clean slate.
    sqlx::query("TRUNCATE events RESTART IDENTITY")
        .execute(&pool)
        .await
        .expect("truncate events");
    let store = PostgresStore::from_pool(pool.clone())
        .await
        .expect("from_pool");
    Some((store, pool))
}

/// Build a [`PendingEnvelope`] from a [`ConformanceRow`].
fn row_to_envelope(r: &ConformanceRow) -> PendingEnvelope {
    let version = Version::new(r.version).expect("version must be > 0");
    // Leak the string so it becomes `'static` (bounded by test process exit).
    let event_type: &'static str = Box::leak(r.event_type.clone().into_boxed_str());
    let with_payload = pending_envelope(version)
        .event_type(event_type)
        .payload(r.payload.clone())
        .expect("valid payload");
    if r.schema_version == 1 {
        with_payload.build()
    } else {
        with_payload
            .schema_version(SchemaVersion::new(
                NonZeroU32::new(r.schema_version).expect("schema_version > 0"),
            ))
            .build()
    }
}

// ---------------------------------------------------------------------------
// Step 0a: per-stream conformance
// ---------------------------------------------------------------------------

/// Run the canonical `EventStream` conformance suite against `PostgresStore`.
/// Skips if `DATABASE_URL` is unset.
#[tokio::test]
async fn postgres_event_stream_conforms() {
    let Some((store, pool)) = setup().await else {
        return;
    };
    let stream_id = StreamKey::from_slice(b"conformance");

    assert_event_stream_conformance(|rows: Vec<ConformanceRow>| {
        let store_c = store.clone();
        let pool_c = pool.clone();
        let id = stream_id.clone();
        async move {
            // Truncate between suite checks so each is isolated.
            sqlx::query("TRUNCATE events RESTART IDENTITY")
                .execute(&pool_c)
                .await
                .expect("truncate between checks");

            if !rows.is_empty() {
                let envelopes: Vec<PendingEnvelope> = rows.iter().map(row_to_envelope).collect();
                store_c
                    .append(&id, None, &envelopes)
                    .await
                    .expect("append conformance rows");
            }

            store_c
                .read_stream(&id, Version::INITIAL)
                .await
                .expect("open read_stream")
        }
    })
    .await;
}

// ---------------------------------------------------------------------------
// Step 0b: `$all` conformance
// ---------------------------------------------------------------------------

/// Run the canonical `$all` read-path conformance suite against `PostgresStore`.
/// Skips if `DATABASE_URL` is unset.
#[tokio::test]
async fn postgres_all_stream_conforms() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        return;
    };
    assert_all_stream_conformance(|| {
        let owned_url = url.clone();
        async move {
            let pg_pool = sqlx::postgres::PgPoolOptions::new()
                .connect(&owned_url)
                .await
                .expect("connect pool");
            sqlx::query("TRUNCATE events RESTART IDENTITY")
                .execute(&pg_pool)
                .await
                .expect("truncate between checks");
            PostgresStore::from_pool(pg_pool).await.expect("from_pool")
        }
    })
    .await;
}

// ---------------------------------------------------------------------------
// Step 1: Sequence/Protocol Tests
// ---------------------------------------------------------------------------

/// Append→`read_stream` produces versions `[1, 2, 3]` with correct payloads.
#[tokio::test]
async fn sequence_append_read_versions_123() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let id = StreamKey::from_slice(b"seq-proto");

    let mk_env = |v: u64, payload: Vec<u8>| {
        pending_envelope(Version::new(v).unwrap())
            .event_type("SeqEvent")
            .payload(payload)
            .expect("valid payload")
            .build()
    };

    let envs = [
        mk_env(1, b"p1".to_vec()),
        mk_env(2, b"p2".to_vec()),
        mk_env(3, b"p3".to_vec()),
    ];
    store.append(&id, None, &envs).await.expect("append batch");

    let stream = store
        .read_stream(&id, Version::INITIAL)
        .await
        .expect("open read_stream");
    futures::pin_mut!(stream);

    for expected_v in 1..=3u64 {
        let env = stream.next().await.expect("Some").expect("Ok");
        assert_eq!(
            env.version().as_u64(),
            expected_v,
            "version mismatch at position {expected_v}"
        );
        let expected_payload = format!("p{expected_v}").into_bytes();
        assert_eq!(env.payload(), expected_payload.as_slice());
    }
    assert!(
        stream.next().await.is_none(),
        "stream must be fused after all events"
    );
}

/// Append with stale `expected_version` → `AppendError::Conflict`.
#[tokio::test]
async fn sequence_stale_expected_version_conflict() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let id = StreamKey::from_slice(b"conflict-test");

    let env1 = pending_envelope(Version::new(1).unwrap())
        .event_type("E")
        .payload(b"first".to_vec())
        .expect("valid")
        .build();
    store
        .append(&id, None, &[env1])
        .await
        .expect("first append ok");

    // Try to append with expected = None (implies current = 0) but stream is at 1.
    let env2 = pending_envelope(Version::new(2).unwrap())
        .event_type("E")
        .payload(b"second".to_vec())
        .expect("valid")
        .build();
    let result = store.append(&id, None, &[env2]).await;
    assert!(
        matches!(result, Err(AppendError::Conflict { .. })),
        "expected Conflict, got {result:?}"
    );
}

/// Multi-event batch in one `append` → all versions persisted in order.
#[tokio::test]
async fn sequence_multi_event_batch() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let id = StreamKey::from_slice(b"multi-batch");

    let envs: Vec<PendingEnvelope> = (1..=5u64)
        .map(|v| {
            pending_envelope(Version::new(v).unwrap())
                .event_type("BatchEvent")
                .payload(vec![u8::try_from(v).unwrap()])
                .expect("valid")
                .build()
        })
        .collect();

    store
        .append(&id, None, &envs)
        .await
        .expect("multi-event append");

    let stream = store
        .read_stream(&id, Version::INITIAL)
        .await
        .expect("read_stream");
    futures::pin_mut!(stream);
    let mut count = 0u64;
    while let Some(item) = stream.next().await {
        let env = item.expect("Ok item");
        count += 1;
        assert_eq!(env.version().as_u64(), count);
    }
    assert_eq!(count, 5, "must have exactly 5 events");
}

// ---------------------------------------------------------------------------
// Step 2: Lifecycle Tests (close / reopen)
// ---------------------------------------------------------------------------

/// Append, drop the pool (store goes away), reconnect via `from_pool`,
/// `read_stream` rehydrates identical state.
#[tokio::test]
async fn lifecycle_write_close_reopen() {
    let Some(url) = std::env::var("DATABASE_URL").ok() else {
        return;
    };
    let id = StreamKey::from_slice(b"lifecycle-reopen");

    // Phase 1: open, write, drop.
    {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect(&url)
            .await
            .expect("connect pool phase 1");
        sqlx::query("TRUNCATE events RESTART IDENTITY")
            .execute(&pool)
            .await
            .expect("truncate");
        let store = PostgresStore::from_pool(pool)
            .await
            .expect("from_pool phase 1");

        let env = pending_envelope(Version::new(1).unwrap())
            .event_type("LifecycleEvent")
            .payload(b"written-before-close".to_vec())
            .expect("valid")
            .build();
        store
            .append(&id, None, &[env])
            .await
            .expect("append phase 1");
        // `store` drops here.
    }

    // Phase 2: reconnect, verify the event survived.
    {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect(&url)
            .await
            .expect("connect pool phase 2");
        let store = PostgresStore::from_pool(pool)
            .await
            .expect("from_pool phase 2");

        let stream = store
            .read_stream(&id, Version::INITIAL)
            .await
            .expect("read_stream phase 2");
        futures::pin_mut!(stream);
        let env = stream.next().await.expect("Some").expect("Ok");
        assert_eq!(env.version().as_u64(), 1);
        assert_eq!(env.event_type(), "LifecycleEvent");
        assert_eq!(env.payload(), b"written-before-close".as_slice());
        assert!(stream.next().await.is_none(), "only one event");
    }
}

// ---------------------------------------------------------------------------
// Step 3: Defensive Boundary Tests
// ---------------------------------------------------------------------------

/// `read_stream` on a nonexistent stream → empty (no panic, no error).
#[tokio::test]
async fn defensive_read_stream_nonexistent_is_empty() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let id = StreamKey::from_slice(b"ghost-stream");

    let stream = store
        .read_stream(&id, Version::INITIAL)
        .await
        .expect("open read_stream on nonexistent stream");
    futures::pin_mut!(stream);
    assert!(
        stream.next().await.is_none(),
        "nonexistent stream must yield nothing"
    );
}

/// `read_stream(from = beyond head)` → empty.
#[tokio::test]
async fn defensive_read_stream_beyond_head_is_empty() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let id = StreamKey::from_slice(b"short-stream");

    let env = pending_envelope(Version::new(1).unwrap())
        .event_type("E")
        .payload(b"only".to_vec())
        .expect("valid")
        .build();
    store.append(&id, None, &[env]).await.expect("append");

    // Read from version 100 — beyond the head at version 1.
    let stream = store
        .read_stream(&id, Version::new(100).expect("v100"))
        .await
        .expect("open read_stream beyond head");
    futures::pin_mut!(stream);
    assert!(
        stream.next().await.is_none(),
        "read beyond head must yield nothing"
    );
}

/// A hand-inserted corrupt row (`schema_version = 0`, which is invalid because
/// `SchemaVersion` wraps `NonZeroU32`) causes `read_stream` to yield a
/// `CorruptRow` error item, not a panic.
///
/// The corrupt row is inserted via raw SQL against the pool we own (the plan
/// says to use `PostgresStore::from_pool` + keep a handle to the `PgPool` —
/// here we do exactly that: `pool` is kept alongside `store`).
#[tokio::test]
async fn defensive_corrupt_row_surfaces_error_not_panic() {
    let Some((store, pool)) = setup().await else {
        return;
    };
    let id = StreamKey::from_slice(b"corrupt-stream");

    // Append a valid event first so the stream exists.
    let valid = pending_envelope(Version::new(1).unwrap())
        .event_type("GoodEvent")
        .payload(b"good".to_vec())
        .expect("valid")
        .build();
    store
        .append(&id, None, &[valid])
        .await
        .expect("append valid");

    // Hand-insert a row with schema_version = 0 (invalid — NonZeroU32 rejects 0).
    // This bypasses the adapter's validation and writes corruption straight to the DB.
    sqlx::query(
        "INSERT INTO events (stream_id, version, event_type, schema_version, payload) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id.as_bytes())
    .bind(2_i64)
    .bind("CorruptEvent")
    .bind(0_i32) // schema_version = 0 is the corrupt sentinel
    .bind(b"bad".as_slice())
    .execute(&pool)
    .await
    .expect("raw corrupt insert");

    // `read_stream` must surface an error item for the corrupt row, not panic.
    let stream = store
        .read_stream(&id, Version::INITIAL)
        .await
        .expect("open read_stream");
    futures::pin_mut!(stream);

    // First item: the valid event.
    let first = stream.next().await.expect("first Some");
    assert!(first.is_ok(), "first event should be valid: {first:?}");

    // Second item: the corrupt row → error (CorruptRow).
    let second = stream.next().await.expect("second Some");
    assert!(second.is_err(), "corrupt row must surface as Err, got Ok");
    let err = second.unwrap_err();
    assert!(
        matches!(err, nexus_postgres::PostgresError::CorruptRow { .. }),
        "expected CorruptRow, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Step 4: Linearizability / Isolation (per-stream)
// ---------------------------------------------------------------------------

/// Concurrent `tokio::spawn` writer + reader: the reader sees a monotonic
/// gap-free prefix at all times, and a full re-scan sees all events.
///
/// Uses a `Barrier` to ensure real concurrency (both tasks are released
/// together, not sequential-then-check).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linearizability_concurrent_append_and_read() {
    let Some((store, _pool)) = setup().await else {
        return;
    };

    let id = StreamKey::from_slice(b"concurrent-lin");
    let total: u64 = 20;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

    // Spawn a writer that appends events 1..=total sequentially.
    let writer_store = store.clone();
    let writer_id = id.clone();
    let writer_barrier = std::sync::Arc::clone(&barrier);
    let writer = tokio::spawn(async move {
        writer_barrier.wait().await;
        for v in 1..=total {
            let expected = Version::new(v.saturating_sub(1));
            let env = pending_envelope(Version::new(v).unwrap())
                .event_type("LinEvent")
                .payload(format!("p{v}").into_bytes())
                .expect("valid")
                .build();
            writer_store
                .append(&writer_id, expected, &[env])
                .await
                .expect("append");
            tokio::task::yield_now().await;
        }
    });

    // Concurrently, read the stream repeatedly and assert prefix monotonicity.
    // Release the reader at the same time as the writer.
    barrier.wait().await;
    let mut last_seen: u64 = 0;
    for _ in 0..50 {
        let stream = store
            .read_stream(&id, Version::INITIAL)
            .await
            .expect("read_stream snapshot");
        futures::pin_mut!(stream);
        let mut prev = 0u64;
        while let Some(item) = stream.next().await {
            let env = item.expect("Ok item");
            let v = env.version().as_u64();
            assert_eq!(
                v,
                prev + 1,
                "versions must be gap-free: got {v} after {prev}"
            );
            prev = v;
        }
        if prev > last_seen {
            last_seen = prev;
        }
        tokio::task::yield_now().await;
    }

    writer.await.expect("writer finished");

    // Full re-scan must see all events.
    let stream = store
        .read_stream(&id, Version::INITIAL)
        .await
        .expect("full re-scan");
    futures::pin_mut!(stream);
    let mut count = 0u64;
    while let Some(item) = stream.next().await {
        let env = item.expect("Ok");
        count += 1;
        assert_eq!(env.version().as_u64(), count);
    }
    assert_eq!(count, total, "full re-scan must see all {total} events");
}

// ---------------------------------------------------------------------------
// Step 3 (PgAllPos ordering): locked by compile-time test in position.rs
// but also verified here in the integration suite.
// ---------------------------------------------------------------------------

/// `PgAllPos` ord: lower `txid` wins regardless of `seq`.
/// Mirrors the lock test in `position.rs` but exercises the public API.
#[test]
fn pg_all_pos_ord_txid_first() {
    use nexus_postgres::PgAllPos;
    assert!(PgAllPos::new(1, 9) < PgAllPos::new(2, 0));
    assert!(PgAllPos::new(3, 1) < PgAllPos::new(3, 2));
    assert_eq!(PgAllPos::new(5, 5), PgAllPos::new(5, 5));
}
