//! Per-stream and `$all` subscription tests for [`PostgresStore`].
//!
//! Covers Steps 5 and 6 of Task 8:
//! - Step 5: per-stream `Subscription` (catch-up → live) over postgres.
//! - Step 6: `$all` `Subscription` (catch-up → live) over postgres, verifying
//!   strictly ascending [`PgAllPos`] positions delivered in order.
//!
//! These tests validate that the **unchanged generic subscription loop**
//! (`nexus_store::Subscription`) runs correctly with only `WakeSource`
//! implemented by `PostgresStore`. If the tests pass, the adapter trait seam
//! is correct.
//!
//! # Skip-without-DATABASE_URL
//!
//! Every test calls [`setup`] first. If `DATABASE_URL` is unset, `setup`
//! returns `None` and the test body returns immediately — the test *passes*
//! (no assertion is reached).

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::time::Duration;

use futures::StreamExt;
use nexus::Version;
use nexus_postgres::PostgresStore;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::{PendingEnvelope, Store, StreamKey, Subscription};

/// Timeout for operations that must complete quickly when a store is live.
const TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// Connect to the store, run `ensure_schema`, and truncate for isolation.
/// Returns `None` (skip) if `DATABASE_URL` is unset.
///
/// Returns the `Store<PostgresStore>` handle (for `Subscription::new`) and
/// the `PostgresStore` clone (for direct `RawEventStore` calls).
async fn setup() -> Option<Store<PostgresStore>> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect(&url)
        .await
        .expect("connect pool");
    // Create the schema FIRST, then truncate (see conformance_tests::setup — a
    // fresh DB has no `events` table yet; tests run serially via nextest
    // `--test-threads=1` so TRUNCATE isolates each one).
    let pg = PostgresStore::from_pool(pool.clone())
        .await
        .expect("from_pool");
    sqlx::query("TRUNCATE events RESTART IDENTITY")
        .execute(&pool)
        .await
        .expect("truncate events");
    Some(pg.into_store())
}

fn sk(s: &str) -> StreamKey {
    StreamKey::from_slice(s.as_bytes())
}

fn make_envelope(version: u64, event_type: &'static str, payload: Vec<u8>) -> PendingEnvelope {
    pending_envelope(Version::new(version).expect("version > 0"))
        .event_type(event_type)
        .payload(payload)
        .expect("valid payload")
        .build()
}

async fn append_one(
    store: &Store<PostgresStore>,
    id: &StreamKey,
    version: u64,
    expected: Option<Version>,
    event_type: &'static str,
) {
    let env = make_envelope(version, event_type, format!("p{version}").into_bytes());
    store.append(id, expected, &[env]).await.expect("append");
}

// ---------------------------------------------------------------------------
// Step 5: Per-stream subscription (catch-up then live)
// ---------------------------------------------------------------------------

/// Core validation: the generic subscription loop runs over postgres with
/// only our `WakeSource`. Pre-populate 2 events, drain catch-up, append a
/// 3rd (live), observe it on the tail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_per_stream_catchup_then_live() {
    let Some(handle) = setup().await else {
        return;
    };
    let id = sk("sub-catchup");

    // Pre-populate 2 events.
    append_one(&handle, &id, 1, None, "E1").await;
    append_one(&handle, &id, 2, Version::new(1), "E2").await;

    // Subscribe from the beginning (None = start from version 1).
    let stream = Subscription::new(&handle)
        .subscribe(&id, None)
        .expect("subscribe");
    futures::pin_mut!(stream);

    // Catch-up event 1.
    let env1 = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("timeout catch-up E1")
        .expect("Some")
        .expect("Ok");
    assert_eq!(env1.event_type(), "E1");
    assert_eq!(env1.version(), Version::new(1).unwrap());

    // Catch-up event 2.
    let env2 = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("timeout catch-up E2")
        .expect("Some")
        .expect("Ok");
    assert_eq!(env2.event_type(), "E2");
    assert_eq!(env2.version(), Version::new(2).unwrap());

    // Append a live event; the tail loop must wake via LISTEN/NOTIFY and deliver it.
    append_one(&handle, &id, 3, Version::new(2), "E3").await;

    let env3 = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("timeout live E3")
        .expect("Some")
        .expect("Ok");
    assert_eq!(env3.event_type(), "E3");
    assert_eq!(env3.version(), Version::new(3).unwrap());
}

/// Subscribe from a checkpoint (strict-after resume): 3 events exist, subscribe
/// from version 2 → should see only event 3.
#[tokio::test]
async fn subscribe_per_stream_from_checkpoint() {
    let Some(handle) = setup().await else {
        return;
    };
    let id = sk("sub-checkpoint");

    append_one(&handle, &id, 1, None, "E1").await;
    append_one(&handle, &id, 2, Version::new(1), "E2").await;
    append_one(&handle, &id, 3, Version::new(2), "E3").await;

    // Subscribe from version 2 → events strictly after version 2, i.e. only E3.
    let stream = Subscription::new(&handle)
        .subscribe(&id, Some(Version::new(2).unwrap()))
        .expect("subscribe from checkpoint");
    futures::pin_mut!(stream);

    let env = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("timeout E3")
        .expect("Some")
        .expect("Ok");
    assert_eq!(env.event_type(), "E3");
    assert_eq!(env.version(), Version::new(3).unwrap());
}

/// Subscribe to a nonexistent stream: `next()` parks (does not immediately
/// return `None`). We use a short timeout to verify it blocks, then append
/// to the stream and confirm the event is delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_per_stream_nonexistent_blocks_then_live() {
    let Some(handle) = setup().await else {
        return;
    };
    let id = sk("nonexistent");

    let stream = Subscription::new(&handle)
        .subscribe(&id, None)
        .expect("subscribe");
    futures::pin_mut!(stream);

    // Must park — not yield None immediately.
    let result = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
    assert!(
        result.is_err(),
        "subscribe on empty stream must park, not complete"
    );

    // Now append: the subscription must wake and deliver the event.
    append_one(&handle, &id, 1, None, "E1").await;

    let env = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("timeout after append")
        .expect("Some")
        .expect("Ok");
    assert_eq!(env.event_type(), "E1");
    assert_eq!(env.version(), Version::new(1).unwrap());
}

// ---------------------------------------------------------------------------
// Step 6: `$all` subscription (catch-up then live)
// ---------------------------------------------------------------------------

/// Core `$all` validation: append across two different streams, drain catch-up,
/// append a live event to a third, observe it on the tail. Assert delivered
/// positions are strictly ascending `PgAllPos`. Validates the position-tagged
/// `AllStream` + exclusive `Ord` resume driving the generic loop with no
/// per-event `global_seq` field.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_all_catchup_then_live() {
    let Some(handle) = setup().await else {
        return;
    };

    // Pre-populate events on two different streams.
    append_one(&handle, &sk("stream-a"), 1, None, "A1").await;
    append_one(&handle, &sk("stream-b"), 1, None, "B1").await;
    append_one(&handle, &sk("stream-a"), 2, Version::new(1), "A2").await;

    // Subscribe `$all` from the beginning.
    let stream = Subscription::new(&handle)
        .subscribe_all(None)
        .expect("subscribe_all");
    futures::pin_mut!(stream);

    let mut prev_pos = None;
    let mut event_types = Vec::new();

    // Drain the 3 catch-up events.
    for _ in 0..3 {
        let (pos, env) = tokio::time::timeout(TIMEOUT, stream.next())
            .await
            .expect("timeout catch-up")
            .expect("Some")
            .expect("Ok");
        if let Some(p) = prev_pos {
            assert!(
                pos > p,
                "$all positions must be strictly ascending: {pos:?} after {p:?}"
            );
        }
        prev_pos = Some(pos);
        event_types.push(env.event_type().to_owned());
    }
    // Must have seen all three events (order is (txid, global_seq) order).
    assert!(event_types.contains(&"A1".to_owned()));
    assert!(event_types.contains(&"B1".to_owned()));
    assert!(event_types.contains(&"A2".to_owned()));

    // Append a live event to a third stream.
    append_one(&handle, &sk("stream-c"), 1, None, "C1").await;

    let (live_pos, live_env) = tokio::time::timeout(TIMEOUT, stream.next())
        .await
        .expect("timeout live C1")
        .expect("Some")
        .expect("Ok");

    assert_eq!(live_env.event_type(), "C1");
    assert!(
        live_pos > prev_pos.expect("prev_pos set"),
        "$all live position must be strictly after last catch-up position"
    );
}

/// `$all` subscription with two concurrent writers on different streams:
/// every observed position is strictly increasing — no reorder, dup, or phantom
/// — and all appended events eventually surface. Genuine concurrency: a 3-way
/// `Barrier` releases both writers and the draining reader simultaneously.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscribe_all_concurrent_writers_strictly_increasing() {
    let Some(handle) = setup().await else {
        return;
    };

    let stream = Subscription::new(&handle)
        .subscribe_all(None)
        .expect("subscribe_all");
    futures::pin_mut!(stream);

    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let s1 = handle.clone();
    let s2 = handle.clone();
    let b1 = std::sync::Arc::clone(&barrier);
    let b2 = std::sync::Arc::clone(&barrier);

    let w1 = tokio::spawn(async move {
        b1.wait().await;
        for v in 1..=10u64 {
            let expected = (v > 1).then(|| Version::new(v - 1).unwrap());
            append_one(&s1, &sk("cx"), v, expected, "X").await;
        }
    });
    let w2 = tokio::spawn(async move {
        b2.wait().await;
        for v in 1..=10u64 {
            let expected = (v > 1).then(|| Version::new(v - 1).unwrap());
            append_one(&s2, &sk("cy"), v, expected, "Y").await;
        }
    });

    // Release all three (both writers + the draining reader) simultaneously.
    barrier.wait().await;

    let mut prev_pos = None;
    let mut seen_x = 0u32;
    let mut seen_y = 0u32;
    for _ in 0..20 {
        let (pos, env) = tokio::time::timeout(TIMEOUT, stream.next())
            .await
            .expect("timeout concurrent $all")
            .expect("Some")
            .expect("Ok");
        if let Some(p) = prev_pos {
            assert!(
                pos > p,
                "$all position must be strictly increasing: {pos:?} after {p:?}"
            );
        }
        prev_pos = Some(pos);
        match env.event_type() {
            "X" => seen_x += 1,
            "Y" => seen_y += 1,
            other => panic!("phantom event type observed on $all: {other}"),
        }
    }

    w1.await.expect("writer 1 ok");
    w2.await.expect("writer 2 ok");
    assert_eq!(seen_x, 10, "must observe all 10 events from stream cx");
    assert_eq!(seen_y, 10, "must observe all 10 events from stream cy");
}
