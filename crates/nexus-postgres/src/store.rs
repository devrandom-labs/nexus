use std::num::NonZeroU32;
use std::sync::Arc;

use bytes::Bytes;
use nexus::{ErrorId, Version};
use nexus_store::PendingEnvelope;
use nexus_store::StreamKey;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::error::AppendError;
use nexus_store::notify::StreamNotifiers;
use nexus_store::store::RawEventStore;
use nexus_store::value::{EventType, Metadata, Payload, SchemaVersion};
use nexus_store::wire;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::task::JoinHandle;

use crate::error::PostgresError;
use crate::hex;
use crate::position::PgAllPos;

/// The `LISTEN/NOTIFY` channel every store instance shares. The write side
/// (`notify_committed`) publishes on it; each store's listener task subscribes.
const NOTIFY_CHANNEL: &str = "nexus_events";

/// Shared, `Arc`-owned interior of a [`PostgresStore`].
///
/// One `Inner` is created per `from_pool`; every [`PostgresStore`] clone shares
/// it. The `LISTEN/NOTIFY` listener task lives exactly as long as this `Inner`:
/// [`Drop`] aborts it, so the background connection is released when the last
/// clone of the store is dropped — a test that creates and drops many stores
/// does not leak one listener connection per store (the reason `PostgresStore`
/// is a thin `Arc<Inner>` handle rather than a bare `PgPool`).
struct Inner {
    pool: PgPool,
    /// In-process wake registry. The `LISTEN/NOTIFY` listener task drives it:
    /// every `NOTIFY` becomes a `notifiers.wake(id)`, which rouses both the
    /// per-stream and `$all` armed registrations. Reusing this audited registry
    /// keeps `LISTEN/NOTIFY` a thin transport that merely *hints* "scan sooner".
    notifiers: Arc<StreamNotifiers>,
    /// The background `PgListener` task's handle. Aborted on drop so the task
    /// (and its dedicated connection) stops with the store.
    listener_task: JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Stop the listener task with the store. Its `PgListener` holds a
        // connection for the life of the task; aborting here prevents a
        // per-store connection leak across the many create/drop cycles the
        // tests perform.
        self.listener_task.abort();
    }
}

/// PostgreSQL-backed event store.
///
/// Implements [`RawEventStore`](nexus_store::RawEventStore) and
/// [`WakeSource`](nexus_store::wake::WakeSource) (Tasks 4–7). Constructed via
/// [`connect`](Self::connect) or [`from_pool`](Self::from_pool) in
/// [`builder`](crate::builder).
///
/// Clone is cheap: it is a single `Arc` bump over the shared [`Inner`]
/// (`PgPool` is itself `Arc`-backed). All clones share one wake registry and
/// one `LISTEN/NOTIFY` listener task.
#[derive(Clone)]
pub struct PostgresStore {
    inner: Arc<Inner>,
}

impl PostgresStore {
    /// Assemble a store from a pool: build the wake registry, spawn the single
    /// `LISTEN/NOTIFY` listener task, and wrap it all in the shared [`Inner`].
    ///
    /// Called by [`from_pool`](Self::from_pool) *after* the schema is ensured.
    pub(crate) fn assemble(pool: PgPool) -> Self {
        let notifiers = StreamNotifiers::new();
        let listener_task = spawn_listener(pool.clone(), Arc::clone(&notifiers));
        Self {
            inner: Arc::new(Inner {
                pool,
                notifiers,
                listener_task,
            }),
        }
    }

    /// The connection pool. `pub(crate)` for the sibling modules (`builder`,
    /// tests) that need the raw pool.
    pub(crate) fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    /// The shared wake registry, driven by the `LISTEN/NOTIFY` listener task.
    /// `pub(crate)` so the `WakeSource` impl in [`crate::wake`] can delegate to
    /// it.
    pub(crate) fn wake_registry(&self) -> &Arc<StreamNotifiers> {
        &self.inner.notifiers
    }
}

/// Spawn the single background task that owns a [`PgListener`] and forwards each
/// `NOTIFY` into the store's [`StreamNotifiers`].
///
/// # Correctness — this redundant catch-up scan must NOT be optimized away
///
/// `PgListener` **auto-reconnects** on connection loss and, per the sqlx 0.8
/// docs, *"any notifications received while the connection was lost will not be
/// returned"* — a `NOTIFY` can be silently dropped. This is tolerable ONLY
/// because [`StreamNotifiers`] drives nexus-store's arm-before-confirm-rescan
/// discipline: every `wake()` (and every reopen of the generic live loop)
/// re-scans the store via `read_stream`/`read_all`, so a dropped `NOTIFY` merely
/// **delays** a wake until the next scan — it never loses the event. The
/// `NOTIFY` is a *hint to scan sooner*, never the source of truth. A future
/// refactor MUST NOT remove that redundant catch-up scan: without it, a dropped
/// `NOTIFY` would become a lost event.
fn spawn_listener(pool: PgPool, notifiers: Arc<StreamNotifiers>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Obtain a dedicated listener connection from the pool and subscribe.
        // On setup failure there is nothing to wake from; exit — the store
        // still works, wakes just fall back entirely to the catch-up scan.
        let Ok(mut listener) = PgListener::connect_with(&pool).await else {
            return;
        };
        if listener.listen(NOTIFY_CHANNEL).await.is_err() {
            return;
        }
        // `recv()` transparently reconnects on connection loss (dropping any
        // NOTIFYs sent while disconnected — see the correctness note above). It
        // only errors when the pool is closed, i.e. the store is being torn
        // down; exit the loop then so the task ends cleanly.
        while let Ok(notification) = listener.recv().await {
            // The payload is the hex-encoded raw stream id. A malformed payload
            // is dropped (see `hex::decode`): a mis-routed wake could rouse the
            // wrong stream, whereas a dropped one is caught by the next scan.
            if let Some(stream) = hex::decode(notification.payload()) {
                // `wake` bumps BOTH the per-stream and `$all` paths, so one
                // NOTIFY rouses per-stream and `$all` armed registrations alike.
                notifiers.wake(&stream);
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Per-event columns that reconstitute a `PersistedEnvelope`.
// NOTE: no `global_seq` — that is an `$all`-position concern, not an envelope one.
// ---------------------------------------------------------------------------

/// The per-event columns that reconstitute a [`PersistedEnvelope`].
///
/// No `global_seq` — it is an `$all`-position concern, not an envelope one.
/// The `$all` read (Task 6) pulls `txid`/`global_seq` separately to build
/// the [`PgAllPos`] tag.
#[derive(sqlx::FromRow)]
struct EventRow {
    version: i64,
    event_type: String,
    schema_version: i32,
    payload: Vec<u8>,
    metadata: Option<Vec<u8>>,
}

/// `$all` row = the position columns plus a flattened [`EventRow`].
///
/// `#[sqlx(flatten)]` (sqlx 0.8) reads `EventRow`'s columns from the same flat
/// result row, so there is ONE envelope-column definition reused by both reads
/// — no hand repack.
#[derive(sqlx::FromRow)]
struct AllEventRow {
    /// `xid8` read back as `bigint` via `txid::text::bigint` cast (see SQL note
    /// on `read_all`). Valid as long as the xid8 value ≤ `i64::MAX`, which is
    /// effectively forever for any real workload.
    txid: i64,
    global_seq: i64,
    #[sqlx(flatten)]
    event: EventRow,
}

// ---------------------------------------------------------------------------
// Pure helpers — no async, no DB
// ---------------------------------------------------------------------------

/// Rebuild the canonical, 16-byte-aligned V2 wire frame from a row, so a
/// postgres [`PersistedEnvelope`] is byte-identical to a fjall one.
///
/// `stream_label` is a diagnostic copy of the stream id for error messages.
fn row_to_envelope(
    row: EventRow,
    stream_label: ErrorId,
) -> Result<PersistedEnvelope, PostgresError> {
    let version = u64::try_from(row.version)
        .ok()
        .and_then(Version::new)
        .ok_or_else(|| corrupt(stream_label, "version <= 0 or out of range"))?;

    let schema = u32::try_from(row.schema_version)
        .ok()
        .and_then(NonZeroU32::new)
        .map(SchemaVersion::new)
        .ok_or_else(|| corrupt(stream_label, "schema_version == 0 or out of range"))?;

    // The validating constructor is `from_bytes(Bytes)`, NOT `::new` —
    // `String`/`Vec<u8>` both impl `Into<Bytes>`, so `.into()` targets the
    // `Bytes` arg (audit fix: use `from_bytes`, not the non-existent `::new`).
    let event_type = EventType::from_bytes(Bytes::from(row.event_type))
        .map_err(|_| corrupt(stream_label, "event_type invalid"))?;
    let payload = Payload::from_bytes(Bytes::from(row.payload))
        .map_err(|_| corrupt(stream_label, "payload too large"))?;
    let metadata = row
        .metadata
        .map(|m| Metadata::from_bytes(Bytes::from(m)))
        .transpose()
        .map_err(|_| corrupt(stream_label, "metadata too large"))?;

    // Rebuild the canonical, 16-byte-aligned V2 frame so a postgres envelope
    // is byte-identical to a fjall one. `encode_frame` owns the layout +
    // alignment; the frame carries NO global_seq (frame V2, #266) — global
    // position lives only on the `$all` stream's position tag.
    let frame =
        wire::encode_frame(schema, &event_type, &payload, metadata.as_ref()).map_err(|e| {
            PostgresError::Frame {
                stream_id: stream_label,
                version: version.as_u64(),
                reason: ErrorId::from_display(&e),
            }
        })?;

    PersistedEnvelope::try_new(
        version,
        frame.value,
        schema,
        frame.offsets.event_type,
        frame.offsets.payload,
        frame.offsets.metadata,
    )
    .map_err(|source| PostgresError::EnvelopeCorrupt {
        stream_id: stream_label,
        version: version.as_u64(),
        source,
    })
}

/// Build a [`PostgresError::CorruptRow`] from a fixed-string reason.
fn corrupt(stream_id: ErrorId, reason: &str) -> PostgresError {
    PostgresError::CorruptRow {
        stream_id,
        reason: ErrorId::from_display(&reason),
    }
}

// ---------------------------------------------------------------------------
// Pure, IO-free append decision core
// ---------------------------------------------------------------------------

/// A validated, DB-typed insert row, borrowing its bytes from the pending
/// envelope.
///
/// The narrowed `version`/`schema_version` only exist *after* validation —
/// holding a `PreparedInsert` is proof the batch passed the optimistic +
/// strict-sequential + range checks. ("Justify every type": it carries the
/// post-validation DB scalars that don't exist on `PendingEnvelope`.)
#[derive(Debug)]
struct PreparedInsert<'a> {
    version: i64,
    schema_version: i32,
    env: &'a PendingEnvelope,
}

/// Pure, IO-free batch validation + narrowing — **unit-testable without a DB.**
///
/// Given the stream's `current` (max) version (already read inside the txn):
/// enforce the optimistic-concurrency check, then that the batch versions are
/// strictly sequential from `current + 1` (overflow-checked, rule 2), narrowing
/// each to its DB column type. Returns the rows ready to `INSERT`, or the first
/// violation. No `await`, no pool — the *decision* lives here; `append`'s loop
/// is purely mechanical.
fn prepare_inserts<'a>(
    current: u64,
    expected: Option<Version>,
    envelopes: &'a [PendingEnvelope],
    id: &StreamKey,
) -> Result<Vec<PreparedInsert<'a>>, AppendError<PostgresError>> {
    // Optimistic check: `expected` must equal the stream's current version.
    if expected.map_or(0, Version::as_u64) != current {
        return Err(AppendError::Conflict {
            stream_id: ErrorId::from_display(id),
            expected,
            actual: Version::new(current),
        });
    }

    // Combinator-first (rule 6): a running `expect` counter folded through the
    // batch; `collect::<Result<Vec<_>, _>>()` short-circuits on the first error.
    let mut expect = current;
    envelopes
        .iter()
        .map(|env| {
            expect = expect.checked_add(1).ok_or_else(|| {
                AppendError::Store(PostgresError::CorruptRow {
                    stream_id: ErrorId::from_display(id),
                    reason: ErrorId::from_display(&"version overflow"),
                })
            })?;
            if env.version().as_u64() != expect {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(id),
                    expected: Version::new(expect),
                    actual: Some(env.version()),
                });
            }
            let version = i64::try_from(env.version().as_u64()).map_err(|_| {
                AppendError::Store(PostgresError::CorruptRow {
                    stream_id: ErrorId::from_display(id),
                    reason: ErrorId::from_display(&"version exceeds i64::MAX"),
                })
            })?;
            // `schema_version` column is INTEGER (i32); `env.schema_version()` is
            // `u32` — narrow with `try_from` (rule 2), never bind an i64 to int4.
            let schema_version = i32::try_from(env.schema_version()).map_err(|_| {
                AppendError::Store(PostgresError::CorruptRow {
                    stream_id: ErrorId::from_display(id),
                    reason: ErrorId::from_display(&"schema_version exceeds i32::MAX"),
                })
            })?;
            Ok(PreparedInsert {
                version,
                schema_version,
                env,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// IO helpers
// ---------------------------------------------------------------------------

/// Read the stream's current (max) version inside `conn`. Absent stream → 0.
///
/// A *negative* stored version is corruption surfaced as an error, NOT a silent
/// 0 (rule 2 — no `unwrap_or(sentinel)` on a failed conversion).
async fn read_current_version(
    conn: &mut sqlx::PgConnection,
    id: &StreamKey,
) -> Result<u64, AppendError<PostgresError>> {
    let current: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM events WHERE stream_id = $1")
            .bind(id.as_bytes())
            .fetch_one(&mut *conn)
            .await
            .map_err(store_err)?;
    u64::try_from(current).map_err(|_| {
        AppendError::Store(PostgresError::CorruptRow {
            stream_id: ErrorId::from_display(id),
            reason: ErrorId::from_display(&"stored version is negative"),
        })
    })
}

/// Map a sqlx error to an `AppendError<PostgresError>` store variant.
fn store_err<E: Into<sqlx::Error>>(e: E) -> AppendError<PostgresError> {
    AppendError::Store(PostgresError::Sqlx(e.into()))
}

/// Returns `true` if `e` is a unique-constraint violation (SQLSTATE `23505`).
fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}

// ---------------------------------------------------------------------------
// `notify_committed` — fires pg_notify after a durable commit (Task 7 Step 1)
// ---------------------------------------------------------------------------

impl PostgresStore {
    /// Fire `pg_notify('nexus_events', <hex stream id>)` after a durable
    /// commit. Best-effort — a failed `NOTIFY` must not fail an already-durable
    /// append (the catch-up scan will still pick the event up on the next poll).
    pub(crate) async fn notify_committed(&self, stream: &[u8]) {
        // The payload is the hex-encoded raw stream id (a `NOTIFY` payload must
        // be ASCII text, but a stream id is arbitrary bytes). The listener task
        // reverses this via `hex::decode` before calling `wake` — one shared,
        // unit-tested codec on both ends (see `crate::hex`).
        let payload = hex::encode(stream);
        let _ = sqlx::query("SELECT pg_notify($1, $2)")
            .bind(NOTIFY_CHANNEL)
            .bind(payload)
            .execute(self.pool())
            .await;
    }
}

// ---------------------------------------------------------------------------
// `RawEventStore` impl
// ---------------------------------------------------------------------------

/// Per-stream stream type: owned, `Send`, `'static` iterator-backed stream of
/// `Result<PersistedEnvelope, PostgresError>`.
///
/// Materialized from a `Vec` via `futures::stream::iter`. The whole result set
/// is fetched eagerly — this is the validation skeleton, not the production
/// shape (see the plan's `fetch_all` note for the keyset-paginated follow-up).
type Stream = futures::stream::Iter<std::vec::IntoIter<Result<PersistedEnvelope, PostgresError>>>;

/// `$all` stream type: owned, `Send`, `'static` iterator-backed stream of
/// `Result<(PgAllPos, PersistedEnvelope), PostgresError>`.
///
/// Distinct from [`Stream`] because `$all` items are position-tagged
/// (`(PgAllPos, PersistedEnvelope)`) while per-stream items are not
/// (`PersistedEnvelope` only). Same eager-fetch caveat as [`Stream`].
type AllStream =
    futures::stream::Iter<std::vec::IntoIter<Result<(PgAllPos, PersistedEnvelope), PostgresError>>>;

impl RawEventStore for PostgresStore {
    type Error = PostgresError;
    type Stream = Stream;
    type AllPosition = PgAllPos;
    type AllStream = AllStream;

    async fn append(
        &self,
        id: &StreamKey,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut tx = self.pool().begin().await.map_err(store_err)?;

        let current = read_current_version(&mut tx, id).await?; // IO
        let rows = prepare_inserts(current, expected_version, envelopes, id)?; // PURE
        if rows.is_empty() {
            return Ok(()); // version checked; nothing to write
        }

        for row in &rows {
            let result = sqlx::query(
                "INSERT INTO events \
                 (stream_id, version, event_type, schema_version, payload, metadata) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id.as_bytes())
            .bind(row.version)
            .bind(row.env.event_type())
            .bind(row.schema_version)
            .bind(row.env.payload())
            .bind(row.env.metadata())
            .execute(&mut *tx)
            .await;

            if let Err(e) = result {
                // A UNIQUE(stream_id, version) violation means a concurrent
                // writer claimed this version first — a conflict, not a Store
                // error (CLAUDE rule 3: one variant = one failure domain).
                if is_unique_violation(&e) {
                    return Err(AppendError::Conflict {
                        stream_id: ErrorId::from_display(id),
                        expected: Some(row.env.version()),
                        actual: None, // a racer committed; exact actual unknown here
                    });
                }
                return Err(AppendError::Store(PostgresError::Sqlx(e)));
            }
        }

        tx.commit().await.map_err(store_err)?;

        // Wake AFTER durable commit (WakeSource contract: wake post-commit).
        self.notify_committed(id.as_bytes()).await;
        Ok(())
    }

    async fn read_stream(
        &self,
        id: &StreamKey,
        from: Version,
    ) -> Result<Self::Stream, Self::Error> {
        let label = ErrorId::from_display(id);
        let from_i64 = i64::try_from(from.as_u64())
            .map_err(|_| corrupt(label, "from version exceeds i64::MAX"))?;

        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT version, event_type, schema_version, payload, metadata \
             FROM events \
             WHERE stream_id = $1 AND version >= $2 \
             ORDER BY version",
        )
        .bind(id.as_bytes())
        .bind(from_i64)
        .fetch_all(self.pool())
        .await
        .map_err(PostgresError::Sqlx)?;

        let envelopes: Vec<Result<PersistedEnvelope, PostgresError>> = rows
            .into_iter()
            .map(move |r| row_to_envelope(r, label))
            .collect();
        Ok(futures::stream::iter(envelopes))
    }

    async fn read_all(&self, from: Option<PgAllPos>) -> Result<Self::AllStream, Self::Error> {
        let label = ErrorId::default();

        // Absence is expressed as SQL NULL, NOT a magic sentinel (CLAUDE rule 3 —
        // "unknown values must be Option, not sentinels"). `from = None` binds two
        // NULLs and the `$1::bigint IS NULL` guard short-circuits the resume
        // predicate, so the read starts from the very beginning. `from = Some`
        // binds the pair and the row-value comparison applies. One query, no
        // out-of-band `-1`.
        let (from_txid, from_seq): (Option<i64>, Option<i64>) = match from {
            Some(p) => (
                Some(
                    i64::try_from(p.txid())
                        .map_err(|_| corrupt(label, "from txid exceeds i64::MAX"))?,
                ),
                Some(
                    i64::try_from(p.seq())
                        .map_err(|_| corrupt(label, "from seq exceeds i64::MAX"))?,
                ),
            ),
            None => (None, None),
        };

        // Way-2 read. TWO predicates, both required for by-construction correctness:
        //
        //   1. Watermark `txid < pg_snapshot_xmin(pg_current_snapshot())` — never
        //      consume a row whose writing txn (or any older still-in-flight txn)
        //      hasn't settled, so no smaller `(txid, seq)` can appear *after* a
        //      larger one is delivered. This is what makes the live `$all` gap-free.
        //
        //   2. Exclusive resume `$1::bigint IS NULL OR (txid::text::bigint, global_seq)
        //      > ($1, $2)` — `Ord`-based strict-after when resuming, unbounded-below
        //      when starting fresh; matches `PgAllPos`'s lexicographic `derive(Ord)`.
        //
        // `ORDER BY (txid, global_seq)` is native xid8/bigint ascending — the same
        // order `PgAllPos` sorts by (xid8 is an unsigned 64-bit, non-wrapping count).
        //
        // SQL note: `xid8` has no direct cast to `bigint`; `txid::text::bigint`
        // round-trips it (valid while the xid8 value ≤ `i64::MAX`, i.e. effectively
        // forever). `pg_snapshot_xmin(...)` returns `xid8`, so the watermark
        // comparison `txid < pg_snapshot_xmin(...)` stays native (no cast). The
        // `::text::bigint` cast defeats the `events_watermark_idx` for the resume
        // predicate — acceptable for the validation skeleton (materialize-all). For
        // a later keyset-paginated cursor, bind the resume position back *to* `xid8`
        // (`WHERE (txid, global_seq) > ($1::text::xid8, $2)`) rather than casting
        // the column down to `bigint`.
        let rows: Vec<AllEventRow> = sqlx::query_as(
            "SELECT txid::text::bigint AS txid, global_seq, \
                    version, event_type, schema_version, payload, metadata \
             FROM events \
             WHERE ($1::bigint IS NULL OR (txid::text::bigint, global_seq) > ($1, $2)) \
               AND txid < pg_snapshot_xmin(pg_current_snapshot()) \
             ORDER BY txid, global_seq",
        )
        .bind(from_txid)
        .bind(from_seq)
        .fetch_all(self.pool())
        .await
        .map_err(PostgresError::Sqlx)?;

        let tagged: Vec<Result<(PgAllPos, PersistedEnvelope), PostgresError>> = rows
            .into_iter()
            .map(move |r| {
                let txid =
                    u64::try_from(r.txid).map_err(|_| corrupt(label, "txid out of range"))?;
                let seq = u64::try_from(r.global_seq)
                    .map_err(|_| corrupt(label, "global_seq <= 0 or out of range"))?;
                // `r.event` is the flattened `EventRow` — reuse the single rebuild path.
                let env = row_to_envelope(r.event, label)?;
                Ok((PgAllPos::new(txid, seq), env))
            })
            .collect();
        Ok(futures::stream::iter(tagged))
    }
}

// ---------------------------------------------------------------------------
// DB-free unit tests for `prepare_inserts` (Task 4 Step 3)
//
// These run under `nix flake check` with no DATABASE_URL and cover the append
// contract: optimistic check, strict-sequential versions, range narrowing.
// The live-DB tests (Task 8) only need to cover the UNIQUE-violation race and
// the actual persistence round-trip.
// ---------------------------------------------------------------------------
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::as_conversions,
    reason = "test code"
)]
mod tests {
    use nexus::Version;
    use nexus_store::PendingEnvelope;
    use nexus_store::StreamKey;
    use nexus_store::envelope::pending_envelope;
    use nexus_store::error::AppendError;
    use nexus_store::value::SchemaVersion;

    use super::{PostgresError, prepare_inserts};

    fn stream_key() -> StreamKey {
        StreamKey::from_bytes("test-stream")
    }

    /// Build a minimal [`PendingEnvelope`] at the given `version` (1-based).
    fn make_envelope(version: u64) -> PendingEnvelope {
        let v = Version::new(version).expect("version must be >= 1 in tests");
        pending_envelope(v)
            .event_type("TestEvent")
            .payload(b"{}".as_slice())
            .unwrap()
            .build()
    }

    /// Build a `PendingEnvelope` with an explicit `schema_version`.
    fn make_envelope_sv(version: u64, sv: u32) -> PendingEnvelope {
        use std::num::NonZeroU32;
        let v = Version::new(version).expect("version >= 1");
        let schema =
            SchemaVersion::new(NonZeroU32::new(sv).expect("schema_version must be > 0 in tests"));
        pending_envelope(v)
            .event_type("TestEvent")
            .payload(b"{}".as_slice())
            .unwrap()
            .schema_version(schema)
            .build()
    }

    // 1. Sequence/protocol — happy paths
    // --------------------------------------------------------------------------

    /// `current = 0`, `expected = None`, batch versions `[1, 2, 3]` → Ok(3 rows)
    /// with version column values `[1, 2, 3]`.
    #[test]
    fn fresh_stream_three_events_ok() {
        let envs = [make_envelope(1), make_envelope(2), make_envelope(3)];
        let rows = prepare_inserts(0, None, &envs, &stream_key()).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].version, 1);
        assert_eq!(rows[1].version, 2);
        assert_eq!(rows[2].version, 3);
    }

    /// `current = 5`, `expected = Some(5)`, batch `[6, 7]` → Ok(2 rows) with
    /// correct version column values.
    #[test]
    fn existing_stream_two_events_ok() {
        let envs = [make_envelope(6), make_envelope(7)];
        let rows = prepare_inserts(5, Version::new(5), &envs, &stream_key()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].version, 6);
        assert_eq!(rows[1].version, 7);
    }

    /// Empty batch with `expected = None` and `current = 0` → Ok(empty) so
    /// `append` can short-circuit after the version check.
    #[test]
    fn empty_batch_ok() {
        let rows = prepare_inserts(0, None, &[], &stream_key()).unwrap();
        assert!(rows.is_empty());
    }

    /// Schema version round-trips as i32 correctly.
    #[test]
    fn schema_version_rounds_to_i32() {
        let envs = [make_envelope_sv(1, 42)];
        let rows = prepare_inserts(0, None, &envs, &stream_key()).unwrap();
        assert_eq!(rows[0].schema_version, 42_i32);
    }

    // 2. Conflict / defensive boundary — failure paths
    // --------------------------------------------------------------------------

    /// Stale `expected` (`Some(4)` when `current = 5`) → `Conflict`.
    #[test]
    fn stale_expected_version_conflict() {
        let envs = [make_envelope(6)];
        let err = prepare_inserts(5, Version::new(4), &envs, &stream_key()).unwrap_err();
        assert!(
            matches!(err, AppendError::Conflict { .. }),
            "expected Conflict, got {err:?}"
        );
    }

    /// `expected = Some(0)` (zero is `None` semantics) when `current = 5` →
    /// `Conflict`. Ensures the `expected.map_or(0, …) != current` path is hit.
    #[test]
    fn wrong_expected_none_when_stream_nonempty_conflict() {
        let envs = [make_envelope(6)];
        let err = prepare_inserts(5, None, &envs, &stream_key()).unwrap_err();
        assert!(matches!(err, AppendError::Conflict { .. }));
    }

    /// Gapped batch `[1, 3]` (skips version 2) → `Conflict` at the second
    /// envelope with `expected = Some(2)`, `actual = Some(3)`.
    #[test]
    fn gapped_batch_conflict() {
        let envs = [make_envelope(1), make_envelope(3)];
        let err = prepare_inserts(0, None, &envs, &stream_key()).unwrap_err();
        match err {
            AppendError::Conflict {
                expected, actual, ..
            } => {
                assert_eq!(expected, Version::new(2), "expected version 2");
                assert_eq!(actual, Some(make_envelope(3).version()), "actual version 3");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    /// Out-of-order batch `[2, 1]` → `Conflict` at the first envelope
    /// (`version = 2` instead of `1` from `current = 0`).
    #[test]
    fn out_of_order_batch_conflict() {
        let envs = [make_envelope(2), make_envelope(1)];
        let err = prepare_inserts(0, None, &envs, &stream_key()).unwrap_err();
        match err {
            AppendError::Conflict {
                expected, actual, ..
            } => {
                assert_eq!(expected, Version::new(1), "expected version 1");
                assert_eq!(actual, Some(make_envelope(2).version()), "actual version 2");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    /// `schema_version > i32::MAX` → `Store(CorruptRow)`, not a panic
    /// (CLAUDE rule 2: checked arithmetic; rule 3: one variant = one domain).
    #[test]
    fn schema_version_over_i32_max_is_store_error() {
        // Build a PendingEnvelope whose schema_version is i32::MAX + 1 = 2^31.
        // SchemaVersion is NonZeroU32 so 2^31 is a legal u32 value but overflows i32.
        let overflow_sv: u32 = (i32::MAX as u32) + 1;
        let envs = [make_envelope_sv(1, overflow_sv)];
        let err = prepare_inserts(0, None, &envs, &stream_key()).unwrap_err();
        assert!(
            matches!(err, AppendError::Store(PostgresError::CorruptRow { .. })),
            "expected Store(CorruptRow), got {err:?}"
        );
    }
}
