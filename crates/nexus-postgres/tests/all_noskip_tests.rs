//! `$all` no-skip linearizability test (Task 8, Step 7).
//!
//! This file proves that the composite `(txid, global_seq)` position plus the
//! `pg_snapshot_xmin` watermark make the `$all` read gap-free **by construction**
//! under concurrent writers with controlled out-of-order commits.
//!
//! ## The hazard the old scalar position had
//!
//! With a bare `global_seq` resume scalar:
//! - Transaction A `INSERT`s to stream X, claims a **lower** `global_seq`, but
//!   has not yet committed.
//! - Transaction B `INSERT`s to stream Y, claims a **higher** `global_seq`, and
//!   **commits**.
//! - A `$all` cursor running `WHERE global_seq > $last ORDER BY global_seq`
//!   would consume B, checkpoint B's seq, then — when A finally commits — skip A
//!   forever because `A.global_seq < B.global_seq <= $last`.
//!
//! ## How the composite position + watermark closes the gap (#266)
//!
//! The `read_all` query has two predicates:
//!
//! 1. **Watermark**: `txid < pg_snapshot_xmin(pg_current_snapshot())` — only
//!    deliver rows whose writing txn is provably committed (its txid is below
//!    the snapshot's xmin, i.e., every lower txid is also committed). While A
//!    is still open, `xmin ≤ A.txid`; since `B.txid > A.txid ≥ xmin`, B's rows
//!    satisfy `B.txid ≥ xmin`, so they are withheld too.
//!
//! 2. **Exclusive `Ord` resume**: `(txid::text::bigint, global_seq) > ($1, $2)`.
//!    Once both A and B commit, `xmin` advances past both, and the next
//!    `read_all(None)` delivers both in `(txid, global_seq)` order — A first,
//!    B second — with no gap.
//!
//! ## Test structure
//!
//! 1. Open transaction A (`pool.begin()`), INSERT to stream X.
//! 2. Open transaction B, INSERT to stream Y, COMMIT B immediately.
//! 3. Run `read_all(None)` — assert it delivers **nothing** (watermark holds).
//! 4. COMMIT transaction A.
//! 5. Run `read_all(None)` again — assert both events appear in `(txid, seq)`
//!    order with A (lower txid) first. This is the event the old scalar cursor
//!    would have skipped.
//!
//! # Skip-without-DATABASE_URL
//!
//! If `DATABASE_URL` is unset the test returns immediately (passes). The
//! nixosTest supplies the URL in CI.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use futures::StreamExt;
use nexus::Version;
use nexus_postgres::{PgAllPos, PostgresStore};
use nexus_store::StreamKey;
use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use sqlx::PgPool;

// ---------------------------------------------------------------------------
// Infrastructure
// ---------------------------------------------------------------------------

/// Connect + truncate. Returns `None` (skip) if `DATABASE_URL` is unset.
async fn setup() -> Option<(PostgresStore, PgPool)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .expect("connect pool");
    // Create the schema FIRST, then truncate (see conformance_tests::setup — a
    // fresh DB has no `events` table yet; tests run serially via nextest
    // `--test-threads=1` so TRUNCATE isolates each one).
    let store = PostgresStore::from_pool(pool.clone())
        .await
        .expect("from_pool");
    sqlx::query("TRUNCATE events RESTART IDENTITY")
        .execute(&pool)
        .await
        .expect("truncate");
    Some((store, pool))
}

/// Drain a full `read_all(from)` into `(PgAllPos, event_type)` pairs.
async fn drain_all(store: &PostgresStore, from: Option<PgAllPos>) -> Vec<(PgAllPos, String)> {
    let stream = store.read_all(from).await.expect("open read_all");
    futures::pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let (pos, env) = item.expect("read_all item must be Ok");
        out.push((pos, env.event_type().to_owned()));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The `$all` no-skip linearizability test under out-of-order commits.
///
/// This is the freeze evidence that the composite position + watermark
/// (`#266`) makes the live `$all` subscription gap-free by construction.
#[tokio::test]
async fn all_noskip_out_of_order_commits() {
    let Some((store, pool)) = setup().await else {
        return;
    };

    let stream_x = StreamKey::from_slice(b"stream-x");
    let stream_y = StreamKey::from_slice(b"stream-y");

    // ── Phase 1: Transaction A opens and inserts to stream X but does NOT commit. ──
    //
    // We use an explicit `sqlx::Transaction` so we can hold it open across awaits.
    // `pool.begin()` acquires a connection and issues `BEGIN`.
    let mut tx_a = pool.begin().await.expect("BEGIN tx_a");

    // INSERT to stream X inside tx_a. This claims a `txid` (lower, because A
    // opened first) but the row is not visible until tx_a commits.
    sqlx::query(
        "INSERT INTO events (stream_id, version, event_type, schema_version, payload) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_x.as_bytes())
    .bind(1_i64)
    .bind("XEvent")
    .bind(1_i32)
    .bind(b"payload-x".as_slice())
    .execute(&mut *tx_a)
    .await
    .expect("INSERT stream X in tx_a");

    // ── Phase 2: Transaction B inserts to stream Y and COMMITS immediately. ──
    //
    // B commits before A, so B's row becomes visible first. Under the old
    // scalar-`global_seq` resume, B would be consumed and its seq checkpointed;
    // then when A commits (lower seq), A's event would be missed forever.
    let mut tx_b = pool.begin().await.expect("BEGIN tx_b");
    sqlx::query(
        "INSERT INTO events (stream_id, version, event_type, schema_version, payload) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_y.as_bytes())
    .bind(1_i64)
    .bind("YEvent")
    .bind(1_i32)
    .bind(b"payload-y".as_slice())
    .execute(&mut *tx_b)
    .await
    .expect("INSERT stream Y in tx_b");
    tx_b.commit().await.expect("COMMIT tx_b");

    // ── Phase 3: `read_all(None)` while A is still in-flight. ──
    //
    // Assert: the watermark withholds everything because A's txid is still open.
    // `pg_snapshot_xmin` will be ≤ A's txid; the predicate `events.txid < xmin`
    // is false for every row (A's txid ≥ xmin, B's txid > A's txid ≥ xmin).
    let phase3 = drain_all(&store, None).await;
    assert!(
        phase3.is_empty(),
        "Phase 3: while tx_a is open, read_all must deliver nothing — \
         the watermark withholds B (B.txid > A.txid ≥ xmin). Got: {phase3:?}"
    );

    // ── Phase 4: Commit transaction A. ──
    tx_a.commit().await.expect("COMMIT tx_a");

    // ── Phase 5: `read_all(None)` — both events must appear in (txid, seq) order. ──
    //
    // A.txid < B.txid because `pg_current_xact_id()` assigns xids as transactions
    // start (BEGIN), and A started before B. Lexicographic `Ord` on `PgAllPos`
    // sorts by txid first, so A's event must sort before B's event — exactly the
    // order a gap-free cursor must deliver. The old scalar cursor would have
    // delivered B (higher seq, already committed) and then skipped A forever.
    let phase5 = drain_all(&store, None).await;
    assert_eq!(
        phase5.len(),
        2,
        "Phase 5: both events must be delivered after both txns commit. Got: {phase5:?}"
    );

    // Positions must be strictly increasing.
    assert!(
        phase5[1].0 > phase5[0].0,
        "Phase 5: positions must be strictly increasing: {:?} then {:?}",
        phase5[0].0,
        phase5[1].0
    );

    // The first delivered event must be A's (XEvent) — lower txid sorts first.
    assert_eq!(
        phase5[0].1, "XEvent",
        "Phase 5: X's event (lower txid = A opened first) must sort before Y's event. \
         Got first={:?}, second={:?}",
        phase5[0].1, phase5[1].1
    );
    assert_eq!(
        phase5[1].1, "YEvent",
        "Phase 5: Y's event (higher txid = B opened after A) must sort after X's event"
    );
}

/// Resuming from the position of the first event (X) must yield only the
/// second event (Y) — confirming exclusive `Ord`-based resume is correct
/// after out-of-order commits (no duplicate X on the next scan).
#[tokio::test]
async fn all_noskip_exclusive_resume_after_out_of_order_commits() {
    let Some((store, pool)) = setup().await else {
        return;
    };

    let stream_x = StreamKey::from_slice(b"resume-x");
    let stream_y = StreamKey::from_slice(b"resume-y");

    // Set up: A inserts X (opens first, lower txid), B inserts Y and commits,
    // then A commits.
    let mut tx_a = pool.begin().await.expect("BEGIN tx_a");
    sqlx::query(
        "INSERT INTO events (stream_id, version, event_type, schema_version, payload) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_x.as_bytes())
    .bind(1_i64)
    .bind("XEvent2")
    .bind(1_i32)
    .bind(b"x".as_slice())
    .execute(&mut *tx_a)
    .await
    .expect("INSERT X in tx_a");

    let mut tx_b = pool.begin().await.expect("BEGIN tx_b");
    sqlx::query(
        "INSERT INTO events (stream_id, version, event_type, schema_version, payload) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_y.as_bytes())
    .bind(1_i64)
    .bind("YEvent2")
    .bind(1_i32)
    .bind(b"y".as_slice())
    .execute(&mut *tx_b)
    .await
    .expect("INSERT Y in tx_b");
    tx_b.commit().await.expect("COMMIT tx_b");
    tx_a.commit().await.expect("COMMIT tx_a");

    // Both committed. Full scan: X sorts before Y (lower txid).
    let full = drain_all(&store, None).await;
    assert_eq!(full.len(), 2);

    let pos_of_x = full[0].0;
    assert_eq!(full[0].1, "XEvent2", "X (lower txid) must sort first");
    assert_eq!(full[1].1, "YEvent2");

    // Resume strictly after X's position → only Y.
    let resumed = drain_all(&store, Some(pos_of_x)).await;
    assert_eq!(
        resumed.len(),
        1,
        "resuming after X must yield only Y, got: {resumed:?}"
    );
    assert_eq!(resumed[0].1, "YEvent2");
    assert!(
        resumed[0].0 > pos_of_x,
        "resumed position must be strictly after the checkpoint"
    );
}

/// Sanity: `read_all(None)` on a completely empty store yields nothing.
#[tokio::test]
async fn all_noskip_empty_store_yields_nothing() {
    let Some((store, _pool)) = setup().await else {
        return;
    };
    let got = drain_all(&store, None).await;
    assert!(got.is_empty(), "empty store: read_all must yield nothing");
}

/// Additional linearizability proof via the real `RawEventStore::append` API
/// (not raw SQL): ordering and no-skip invariants hold for normal API usage too.
#[tokio::test]
async fn all_noskip_via_store_api_ordering() {
    let Some((store, _pool)) = setup().await else {
        return;
    };

    let a = StreamKey::from_slice(b"api-a");
    let b_id = StreamKey::from_slice(b"api-b");

    let mk = |v: u64, et: &'static str| {
        pending_envelope(Version::new(v).unwrap())
            .event_type(et)
            .payload(format!("{et}{v}").into_bytes())
            .expect("valid")
            .build()
    };

    store
        .append(&a, None, &[mk(1, "A")])
        .await
        .expect("append A1");
    store
        .append(&b_id, None, &[mk(1, "B")])
        .await
        .expect("append B1");
    store
        .append(&a, Version::new(1), &[mk(2, "A")])
        .await
        .expect("append A2");

    let all = drain_all(&store, None).await;
    assert_eq!(all.len(), 3, "must deliver all 3 events");

    // Positions must be strictly increasing.
    for w in all.windows(2) {
        assert!(
            w[1].0 > w[0].0,
            "positions not strictly increasing: {:?} then {:?}",
            w[0].0,
            w[1].0
        );
    }

    // Every appended event type must appear.
    let types: Vec<&str> = all.iter().map(|(_, t)| t.as_str()).collect();
    assert!(types.contains(&"A"), "A events must appear in $all");
    assert!(types.contains(&"B"), "B events must appear in $all");
}
