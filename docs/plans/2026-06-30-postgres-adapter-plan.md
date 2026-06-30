# Postgres Store Adapter (nexus-postgres) Implementation Plan (#213)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a second store adapter (`nexus-postgres`) on `sqlx`-postgres that implements the adapter contract (`RawEventStore` + `WakeSource`) and runs the *unchanged* generic subscription machinery — validating the about-to-freeze adapter traits against a real, networked, concurrent backend before #204–#210 lock.

**Architecture:** A single `events` table with a `BIGINT GENERATED ALWAYS AS IDENTITY` global sequence (= `GlobalSeq`), a `xid8` writing-transaction column (for the Way-2 watermark, per the #213 decision), and a `UNIQUE (stream_id, version)` constraint that enforces per-stream optimistic concurrency atomically. Writers stay fully parallel (no serialization). Wake is `LISTEN/NOTIFY` via `sqlx`'s `PgListener`. Reads rebuild the canonical wire frame via `nexus_store::wire::encode_frame`, so a `PersistedEnvelope` from postgres is byte-identical to one from fjall.

**Tech Stack:** Rust 2024, `sqlx` (postgres, runtime-tokio), `tokio`, `nexus-store` (subscription feature), `nixosTest` (`services.postgresql`) as a Linux-only flake integration attribute.

---

## Design recap (from the #213 decision — see issue comment + `project_postgres_adapter_decision`)

The cross-stream ordering hazard: a `BIGSERIAL`/`IDENTITY` number is assigned at INSERT but rows are visible at COMMIT, and commit order ≠ insert order — so a lower number that commits late is silently skipped by a `WHERE global_seq > bookmark` `$all` cursor (event loss). **Decision: close it on the reader (Way 2)** — keep insert-time identity, tag each row with `pg_current_xact_id()`, and only consume rows below the oldest-in-flight-transaction watermark (`pg_snapshot_xmin(pg_current_snapshot())`), ordering by `(txid, global_seq)`. Writers untouched, fully parallel.

**The freeze finding this surfaces (and why it bounds this plan's scope):** the watermark fix needs the `$all` resume key to be `(txid, global_seq)`, but `Catchup::Position = GlobalSeq` is a single scalar. A fully-correct *live* `$all` subscription therefore requires widening that position type in `nexus-store` — a separate seam change with its own design fork (opaque adapter-defined position vs. an explicit composite). **That is carved OUT of this plan** (see "Out of scope" + the follow-up card). This plan proves everything that does *not* depend on it, and produces the empirical evidence (a failing-skip test) that justifies the widening decision.

---

## Scope

**In scope (this plan):**
- `nexus-postgres` crate: `RawEventStore` (`append`, `read_stream`, `read_all`) + `WakeSource` (`LISTEN/NOTIFY`) + error type.
- Per-stream correctness: `read_stream` + the generic *per-stream* `Subscription` (catch-up + live tail) running unchanged over postgres, plus a concurrent-writer linearizability test on `read_stream`.
- `read_all` watermark-filtered + `global_seq`-ordered (safe for a single catch-up pass).
- `nixosTest` integration harness (Linux-only flake attribute).
- A test that **demonstrates the `$all` cross-reopen skip** under concurrent writers (the freeze evidence).

**Out of scope (explicit follow-up):**
- Widening `Catchup::Position` from `GlobalSeq` to `(txid, global_seq)` (or opaque) so the *live* `$all` subscription is fully correct over postgres. This is a `nexus-store` seam change + a freeze decision (opaque vs composite) — file as a follow-up, referencing the skip test from Task 7.
- `SnapshotStore`, `AtomicAppend`, `StreamLister`, export/import on postgres (defer; not needed to validate the core contract).
- Connection-pool tuning, migrations framework, production hardening.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/nexus-postgres/Cargo.toml` | Crate manifest (mirror `nexus-fjall`'s shape; `sqlx` + `nexus-store` subscription feature + self dev-dep for gated tests). |
| `crates/nexus-postgres/src/lib.rs` | Re-exports + crate docs. |
| `crates/nexus-postgres/src/error.rs` | `PostgresError` enum (thiserror, `#[non_exhaustive]`, `ErrorId` fields — mirror `FjallError`'s domains). |
| `crates/nexus-postgres/src/schema.rs` | The `events` DDL + `ensure_schema(&PgPool)`. |
| `crates/nexus-postgres/src/store.rs` | `PostgresStore`: `RawEventStore` (append/read_stream/read_all), `WakeSource`, the row→`PersistedEnvelope` rebuild, `NOTIFY`-after-commit. |
| `crates/nexus-postgres/src/builder.rs` | `PostgresStore::connect(url)` / `from_pool(PgPool)` constructors. |
| `crates/nexus-postgres/src/wake.rs` | `PgListener`-backed `WakeSource` registration + `arm()` future. |
| `crates/nexus-postgres/tests/*.rs` | The 4 CLAUDE testing categories + the `$all` skip-demonstration test. |
| `flake.nix` (`packages`) | Expand the Linux-only `integration` stub into the postgres `nixosTest`. |
| root `Cargo.toml` | Add member + `sqlx` workspace dep. |

**Atomicity note:** the crate compiles incrementally (it's a new crate; nothing else depends on it), so tasks can commit independently. Run `cargo hakari generate` after the manifest lands (Task 0) and after any dep change.

---

## The schema (authoritative — Task 2 implements this)

```sql
CREATE TABLE IF NOT EXISTS events (
    global_seq     BIGINT GENERATED ALWAYS AS IDENTITY,
    stream_id      BYTEA    NOT NULL,
    version        BIGINT   NOT NULL,
    txid           xid8     NOT NULL DEFAULT pg_current_xact_id(),
    event_type     TEXT     NOT NULL,
    schema_version INTEGER  NOT NULL,
    payload        BYTEA    NOT NULL,
    metadata       BYTEA,                         -- NULL = absent metadata
    PRIMARY KEY (global_seq),
    UNIQUE (stream_id, version)                   -- atomic per-stream optimistic concurrency
);
CREATE INDEX IF NOT EXISTS events_stream_idx    ON events (stream_id, version);
CREATE INDEX IF NOT EXISTS events_watermark_idx ON events (txid, global_seq);
```

Why these choices:
- `BIGINT GENERATED ALWAYS AS IDENTITY` (not `BIGSERIAL`) — SQL-standard, prevents manual inserts into the id column; same insert-time sequence behaviour. `i64` domain → `GlobalSeq` `u64` via `u64::try_from` (always ≥1, never overflows downward).
- `txid xid8 DEFAULT pg_current_xact_id()` — the writing transaction id, the watermark key. `xid8` is 64-bit, non-wrapping.
- `UNIQUE (stream_id, version)` — the per-stream conflict arbiter. Two writers racing the same `version`: one INSERT wins, the other raises a unique violation → `AppendError::Conflict`. Atomic, no `SELECT … FOR UPDATE` needed.

---

## Task 0: Branch, crate skeleton, workspace + flake wiring

**Files:** root `Cargo.toml`, `crates/nexus-postgres/Cargo.toml`, `crates/nexus-postgres/src/lib.rs`

- [ ] **Step 1: Branch**

```bash
cd /Users/joel/Code/devrandom/nexus
git checkout -b feat/postgres-adapter
```

- [ ] **Step 2: Add `sqlx` as a workspace dependency**

```bash
cargo add --workspace --dry-run sqlx --no-default-features \
  --features "runtime-tokio-rustls,postgres,macros"
```
Review, then run without `--dry-run`. (We will use the **runtime `query` API**, not the compile-time-checked `query!` macros, so the build needs no live DB — see the design note's nix-reproducibility point. `macros` is harmless to enable but we won't use the checked macros.)

- [ ] **Step 3: Create `crates/nexus-postgres/Cargo.toml`** (mirror `nexus-fjall`)

```toml
[package]
name = "nexus-postgres"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "PostgreSQL-backed event store adapter for the Nexus event-sourcing framework"
readme = "../../README.md"
keywords = ["event-sourcing", "event-store", "postgres"]
categories = ["database"]

[dependencies]
bytes = { workspace = true }
futures = { workspace = true }
nexus = { version = "0.1.0", path = "../nexus" }
nexus-store = { version = "0.1.0", path = "../nexus-store", features = ["subscription"] }
sqlx = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync"] }
workspace-hack = { version = "0.1", path = "../workspace-hack" }

[dev-dependencies]
nexus-store-testing = { version = "0.1.0", path = "../nexus-store-testing" }
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

- [ ] **Step 4: Add the crate to the workspace** — in root `Cargo.toml` `[workspace] members`, add `"crates/nexus-postgres",` (keep alphabetical with the other `crates/` entries).

- [ ] **Step 5: Stub `crates/nexus-postgres/src/lib.rs`**

```rust
//! PostgreSQL-backed event store adapter for nexus.
//!
//! Implements [`RawEventStore`](nexus_store::RawEventStore) +
//! [`WakeSource`](nexus_store::wake::WakeSource) over `sqlx`-postgres, with
//! `LISTEN/NOTIFY` wake and a `pg_snapshot_xmin` watermark on the `$all` read
//! (the #213 ordering decision). The second adapter, written to validate the
//! adapter contract before the 1.0 freeze.

mod builder;
mod error;
mod schema;
mod store;
mod wake;

pub use builder::PostgresStore;
pub use error::PostgresError;
```

- [ ] **Step 6: Regenerate hakari, confirm the workspace resolves**

```bash
cargo hakari generate
cargo build -p nexus-postgres
```
Expected: builds (empty modules will error until later tasks add them — create empty `error.rs`/`schema.rs`/`store.rs`/`wake.rs`/`builder.rs` with a `// placeholder` line so the module tree resolves; subsequent tasks fill them).

---

## Task 1: Error type

**Files:** `crates/nexus-postgres/src/error.rs`

- [ ] **Step 1: Define `PostgresError`** (mirror `FjallError`'s failure domains, rule 3)

```rust
use nexus_store::ErrorId;
use nexus_store::envelope::EnvelopeError;

/// Errors produced by the postgres event store adapter.
///
/// Distinct failure domains (CLAUDE rule 3); diagnostic fields use
/// [`ErrorId`] to stay allocation-light on error paths.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PostgresError {
    /// A `sqlx`/postgres I/O, connection, or query error.
    #[error("postgres error: {0}")]
    Sqlx(#[source] sqlx::Error),

    /// A persisted row decoded into bytes that fail envelope validation.
    #[error("envelope integrity error in stream '{stream_id}' at version {version}")]
    EnvelopeCorrupt {
        stream_id: ErrorId,
        version: u64,
        #[source]
        source: EnvelopeError,
    },

    /// A stored row has an out-of-range / corrupt scalar (e.g. global_seq <= 0,
    /// version <= 0, schema_version == 0).
    #[error("corrupt row in stream '{stream_id}': {reason}")]
    CorruptRow {
        stream_id: ErrorId,
        reason: ErrorId<128>,
    },

    /// Building the canonical wire frame for a row failed.
    #[error("wire frame build failed in stream '{stream_id}' at version {version}: {reason}")]
    Frame {
        stream_id: ErrorId,
        version: u64,
        reason: ErrorId<128>,
    },

    /// Wake registration over `LISTEN/NOTIFY` failed.
    #[error("listen/notify wake setup failed: {0}")]
    Wake(#[source] sqlx::Error),
}
```

Note: `Sqlx` is NOT `#[from]` — we map deliberately at call sites so a unique-violation becomes `AppendError::Conflict`, not a generic `Sqlx` (rule 3).

---

## Task 2: Schema bootstrap

**Files:** `crates/nexus-postgres/src/schema.rs`

- [ ] **Step 1: `ensure_schema`** — idempotent DDL applied at connect time

```rust
use sqlx::PgPool;

use crate::error::PostgresError;

/// The events table + indexes. See the plan's schema section for rationale.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    global_seq     BIGINT GENERATED ALWAYS AS IDENTITY,
    stream_id      BYTEA    NOT NULL,
    version        BIGINT   NOT NULL,
    txid           xid8     NOT NULL DEFAULT pg_current_xact_id(),
    event_type     TEXT     NOT NULL,
    schema_version INTEGER  NOT NULL,
    payload        BYTEA    NOT NULL,
    metadata       BYTEA,
    PRIMARY KEY (global_seq),
    UNIQUE (stream_id, version)
);
CREATE INDEX IF NOT EXISTS events_stream_idx    ON events (stream_id, version);
CREATE INDEX IF NOT EXISTS events_watermark_idx ON events (txid, global_seq);
"#;

/// Apply the schema if absent. Idempotent — safe to call on every open.
pub(crate) async fn ensure_schema(pool: &PgPool) -> Result<(), PostgresError> {
    sqlx::raw_sql(SCHEMA_SQL)
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(PostgresError::Sqlx)
}
```

---

## Task 3: Builder / connect

**Files:** `crates/nexus-postgres/src/builder.rs`, `crates/nexus-postgres/src/store.rs` (struct def)

- [ ] **Step 1: `PostgresStore` struct + constructors**

In `store.rs`:
```rust
use sqlx::PgPool;

/// PostgreSQL-backed event store.
#[derive(Clone)]
pub struct PostgresStore {
    pub(crate) pool: PgPool,
}
```

In `builder.rs`:
```rust
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::error::PostgresError;
use crate::schema::ensure_schema;
use crate::store::PostgresStore;

impl PostgresStore {
    /// Connect to `url`, create a pool, and ensure the schema exists.
    ///
    /// # Errors
    /// Connection or schema-creation failure.
    pub async fn connect(url: &str) -> Result<Self, PostgresError> {
        let pool = PgPoolOptions::new()
            .connect(url)
            .await
            .map_err(PostgresError::Sqlx)?;
        Self::from_pool(pool).await
    }

    /// Build from an existing pool (ensures the schema).
    ///
    /// # Errors
    /// Schema-creation failure.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PostgresError> {
        ensure_schema(&pool).await?;
        Ok(Self { pool })
    }
}
```

---

## Task 4: `append` (atomic version check + identity seq + txid + NOTIFY)

**Files:** `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: TDD — write the append round-trip test first** (in `tests/conformance_tests.rs`, gated behind the integration harness; see Task 8 for how it runs). The driving assertions:
  - append 2 events to a fresh stream with `expected_version = None` → `read_stream` returns versions `[1,2]` with correct payloads and `global_seq` ascending.
  - append with a stale `expected_version` → `AppendError::Conflict { expected, actual }`.
  - two concurrent appends racing the same version → exactly one `Ok`, the other `AppendError::Conflict` (the UNIQUE-violation path).

- [ ] **Step 2: Implement `append`**

```rust
async fn append(
    &self,
    id: &StreamKey,
    expected_version: Option<Version>,
    envelopes: &[PendingEnvelope],
) -> Result<(), AppendError<Self::Error>> {
    let id_bytes = id.as_bytes();

    let mut tx = self.pool.begin().await.map_err(store_err)?;

    // Current stream version (NULL → 0 for a fresh stream).
    let current: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) FROM events WHERE stream_id = $1",
    )
    .bind(id_bytes)
    .fetch_one(&mut *tx)
    .await
    .map_err(store_err)?;
    let current_u64 = u64::try_from(current).unwrap_or(0);

    // Optimistic check: expected must equal the current version.
    let expected_u64 = expected_version.map_or(0, Version::as_u64);
    if expected_u64 != current_u64 {
        return Err(AppendError::Conflict {
            stream_id: ErrorId::from_display(&DisplayBytes(id_bytes)),
            expected: expected_version,
            actual: Version::new(current_u64),
        });
    }

    if envelopes.is_empty() {
        return Ok(()); // version checked; nothing to write
    }

    // Validate strictly-sequential versions from current+1, then insert.
    let mut expect = current_u64;
    for env in envelopes {
        expect = expect
            .checked_add(1)
            .ok_or_else(|| AppendError::Store(PostgresError::CorruptRow {
                stream_id: ErrorId::from_display(&DisplayBytes(id_bytes)),
                reason: ErrorId::from_display(&"version overflow"),
            }))?;
        if env.version().as_u64() != expect {
            return Err(AppendError::Conflict {
                stream_id: ErrorId::from_display(&DisplayBytes(id_bytes)),
                expected: Version::new(expect),
                actual: Some(env.version()),
            });
        }

        let version_i64 = i64::try_from(env.version().as_u64()).map_err(|_| {
            AppendError::Store(PostgresError::CorruptRow {
                stream_id: ErrorId::from_display(&DisplayBytes(id_bytes)),
                reason: ErrorId::from_display(&"version exceeds i64::MAX"),
            })
        })?;

        let result = sqlx::query(
            "INSERT INTO events (stream_id, version, event_type, schema_version, payload, metadata) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id_bytes)
        .bind(version_i64)
        .bind(env.event_type())
        .bind(i64::from(env.schema_version()))
        .bind(env.payload())
        .bind(env.metadata())
        .execute(&mut *tx)
        .await;

        if let Err(e) = result {
            // A UNIQUE(stream_id, version) violation means a concurrent writer
            // claimed this version first — that's a conflict, not a Store error.
            if is_unique_violation(&e) {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&DisplayBytes(id_bytes)),
                    expected: Version::new(expect),
                    actual: None, // a racer committed; exact actual is unknown here
                });
            }
            return Err(AppendError::Store(PostgresError::Sqlx(e)));
        }
    }

    tx.commit().await.map_err(store_err)?;

    // Wake AFTER durable commit (rule: WakeSource::wake post-commit).
    self.notify_committed(id_bytes).await;
    Ok(())
}
```

Helpers (same file):
```rust
fn store_err<E: Into<sqlx::Error>>(e: E) -> AppendError<PostgresError> {
    AppendError::Store(PostgresError::Sqlx(e.into()))
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}

/// Render raw id bytes for an ErrorId label (lossy UTF-8 — diagnostics only).
struct DisplayBytes<'a>(&'a [u8]);
impl std::fmt::Display for DisplayBytes<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(self.0))
    }
}
```

Note: we do not insert `global_seq` or `txid` — `IDENTITY` and the `DEFAULT pg_current_xact_id()` assign them. We never `RETURNING` them on write (the read path supplies them).

---

## Task 5: `read_stream` + the row→`PersistedEnvelope` rebuild

**Files:** `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: The row type + rebuild helper** (rebuild the canonical aligned frame via `wire::encode_frame`)

```rust
use nexus_store::wire;
use nexus_store::value::{EventType, Metadata, Payload, SchemaVersion};
use nexus_store::envelope::PersistedEnvelope;

#[derive(sqlx::FromRow)]
struct EventRow {
    global_seq: i64,
    version: i64,
    event_type: String,
    schema_version: i32,
    payload: Vec<u8>,
    metadata: Option<Vec<u8>>,
}

fn row_to_envelope(row: EventRow, stream_label: ErrorId) -> Result<PersistedEnvelope, PostgresError> {
    let version = u64::try_from(row.version).ok()
        .and_then(Version::new)
        .ok_or_else(|| corrupt(stream_label, "version <= 0 or out of range"))?;
    let global_seq = u64::try_from(row.global_seq).ok()
        .and_then(GlobalSeq::new)
        .ok_or_else(|| corrupt(stream_label, "global_seq <= 0 or out of range"))?;
    let schema = u32::try_from(row.schema_version).ok()
        .and_then(|v| SchemaVersion::new(std::num::NonZeroU32::new(v)?))
        .ok_or_else(|| corrupt(stream_label, "schema_version == 0 or out of range"))?;

    let event_type = EventType::new(row.event_type.into())
        .map_err(|_| corrupt(stream_label, "event_type invalid"))?;
    let payload = Payload::new(row.payload.into())
        .map_err(|_| corrupt(stream_label, "payload too large"))?;
    let metadata = row.metadata
        .map(|m| Metadata::new(m.into()))
        .transpose()
        .map_err(|_| corrupt(stream_label, "metadata too large"))?;

    // Rebuild the canonical, 16-byte-aligned frame so a postgres envelope is
    // byte-identical to a fjall one. encode_frame owns the layout + alignment.
    let frame = wire::encode_frame(global_seq.as_u64(), schema, &event_type, &payload, metadata.as_ref())
        .map_err(|e| PostgresError::Frame {
            stream_id: stream_label,
            version: version.as_u64(),
            reason: ErrorId::from_display(&e),
        })?;

    PersistedEnvelope::try_new(
        version, global_seq, frame.value, schema,
        frame.offsets.event_type, frame.offsets.payload, frame.offsets.metadata,
    )
    .map_err(|source| PostgresError::EnvelopeCorrupt { stream_id: stream_label, version: version.as_u64(), source })
}

fn corrupt(stream_id: ErrorId, reason: &str) -> PostgresError {
    PostgresError::CorruptRow { stream_id, reason: ErrorId::from_display(&reason) }
}
```

- [ ] **Step 2: `read_stream`** — a bounded query materialized into a `futures::stream`

```rust
async fn read_stream(&self, id: &StreamKey, from: Version) -> Result<Self::Stream, Self::Error> {
    let label = ErrorId::from_display(&DisplayBytes(id.as_bytes()));
    let from_i64 = i64::try_from(from.as_u64())
        .map_err(|_| corrupt(label, "from version exceeds i64::MAX"))?;
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT global_seq, version, event_type, schema_version, payload, metadata \
         FROM events WHERE stream_id = $1 AND version >= $2 ORDER BY version",
    )
    .bind(id.as_bytes())
    .bind(from_i64)
    .fetch_all(&self.pool)
    .await
    .map_err(PostgresError::Sqlx)?;

    Ok(futures::stream::iter(
        rows.into_iter().map(move |r| row_to_envelope(r, label)),
    ))
}
```

`type Stream = futures::stream::Iter<...>;` — an owned, `Send`, `'static` iterator-backed stream of `Result<PersistedEnvelope, PostgresError>`, which satisfies `EventStream`. (Materializing the whole stream is fine for the validation skeleton; a keyset-paginated cursor is a later optimization, and the contract explicitly allows either.)

> **Implementer note on associated types:** name the concrete `futures::stream::Iter<std::vec::IntoIter<Result<PersistedEnvelope, PostgresError>>>` via a `type` alias in `store.rs` and use it for both `Stream` and `AllStream`.

---

## Task 6: `read_all` (watermark-filtered, the Way-2 read)

**Files:** `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: Implement `read_all`** — only return rows whose writing transaction is fully settled and below the oldest in-flight write

```rust
async fn read_all(&self, from: GlobalSeq) -> Result<Self::AllStream, Self::Error> {
    let label = ErrorId::default();
    let from_i64 = i64::try_from(from.as_u64())
        .map_err(|_| corrupt(label, "from global_seq exceeds i64::MAX"))?;
    // Way-2 watermark: pg_snapshot_xmin(pg_current_snapshot()) is the oldest
    // still-active xid; every row with txid < it is from a fully-resolved
    // transaction, so no in-flight write can still produce a smaller global_seq
    // behind a consumed one *within this snapshot*. Ordered by global_seq to
    // honour read_all's documented contract.
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT global_seq, version, event_type, schema_version, payload, metadata \
         FROM events \
         WHERE global_seq >= $1 AND txid < pg_snapshot_xmin(pg_current_snapshot()) \
         ORDER BY global_seq",
    )
    .bind(from_i64)
    .fetch_all(&self.pool)
    .await
    .map_err(PostgresError::Sqlx)?;

    Ok(futures::stream::iter(
        rows.into_iter().map(move |r| row_to_envelope(r, label)),
    ))
}
```

> **Scope honesty (in a code comment + the findings doc):** this makes a single `read_all` pass return only settled events in `global_seq` order — safe per call. It does **not** make the *live* `$all` subscription fully correct, because the generic loop resumes by `global_seq` alone and a late-committing lower `global_seq` can still be skipped across reopens. Closing that requires resuming by `(txid, global_seq)` — the position-widening follow-up. Task 7's skip test demonstrates this.

---

## Task 7: `WakeSource` via `LISTEN/NOTIFY`

**Files:** `crates/nexus-postgres/src/wake.rs`, `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: `notify_committed`** — fire `NOTIFY` after the append commit

In `store.rs`:
```rust
async fn notify_committed(&self, stream: &[u8]) {
    // Channel 'nexus_events'; payload is the hex stream id. Best-effort: a
    // failed NOTIFY must not fail an already-durable append (the catch-up
    // scan will still pick the event up on the next poll).
    let payload = hex::encode(stream); // or base64; keep it ascii for NOTIFY
    let _ = sqlx::query("SELECT pg_notify('nexus_events', $1)")
        .bind(payload)
        .execute(&self.pool)
        .await;
}
```
(Add `hex` — or inline a tiny hex encoder to avoid the dep; confirm with maintainer. The payload only needs to round-trip the stream id for per-stream wake routing.)

- [ ] **Step 2: `WakeSource` + `WakeRegistration`** — a `PgListener` per registration

```rust
// wake.rs
use std::future::Future;
use futures::StreamExt;
use sqlx::postgres::PgListener;
use tokio::sync::Mutex;

use crate::error::PostgresError;
use crate::store::PostgresStore;

pub struct PgWakeRegistration {
    listener: tokio::sync::Mutex<PgListener>,
    stream: Option<Vec<u8>>, // None = $all
}

impl nexus_store::wake::WakeRegistration for PgWakeRegistration {
    fn arm(&self) -> impl Future<Output = ()> + Send + 'static {
        // NOTE: PgListener is connection-bound; arm must produce a 'static
        // future. The skeleton clones what it needs; see implementer note.
        // ...
    }
}
```

> **Implementer note (the genuinely fiddly part — flag as DONE_WITH_CONCERNS if it fights you):** `WakeRegistration::arm` must return a `'static + Send` future that resolves on the *next* notification after `arm` is called (lost-wakeup-safe). `PgListener::recv()` borrows the listener, which conflicts with `'static`. Two viable shapes:
> 1. Spawn a background task per registration that owns the `PgListener` and forwards notifications into a `tokio::sync::watch`/`Notify`; `arm()` then mirrors fjall's `StreamNotifiers` pattern (clone a `watch::Receiver`, `mark_unchanged()`, await `changed()`) — reusing the exact lost-wakeup discipline already proven in `nexus-store`.
> 2. Reuse `nexus_store::notify::StreamNotifiers` directly: the `PgListener` background task calls `notifiers.wake(stream)` on each NOTIFY, and `register`/`arm` delegate to `StreamNotifiers`. **This is preferred** — it reuses the audited in-process wake machinery and keeps `LISTEN/NOTIFY` as a thin transport that just drives `wake()`.

- [ ] **Step 3: `WakeSource for PostgresStore`** — delegate to a `StreamNotifiers` driven by one shared `PgListener` task (option 2 above). Mirror `nexus-fjall/src/store.rs`'s `impl WakeSource`.

- [ ] **Step 4: Wire `RawEventStore` impl** — declare `type Stream`/`type AllStream` (the `futures::stream::Iter` alias) and `type Error = PostgresError`, with the `append`/`read_stream`/`read_all` from Tasks 4–6.

---

## Task 8: Tests — the 4 categories + the `$all` skip evidence

**Files:** `crates/nexus-postgres/tests/conformance_tests.rs`, `tests/subscription_tests.rs`, `tests/all_skip_tests.rs`

These run against a live postgres provided either by a `DATABASE_URL` env var (local dev loop) or the nixosTest VM (CI). Each test creates a uniquely-named schema/table prefix or uses `TRUNCATE events` in setup for isolation.

- [ ] **Step 1: Sequence/protocol** — append→read_stream versions `[1,2,3]`; append with stale expected → Conflict; multi-event batch in one append.
- [ ] **Step 2: Lifecycle** — append, drop the pool, reconnect (`from_pool`), `read_stream` rehydrates identical state (mirrors fjall's close/reopen).
- [ ] **Step 3: Defensive boundary** — `read_stream` on a nonexistent stream → empty; `read_stream(from = beyond head)` → empty; a hand-inserted corrupt row (e.g. `schema_version = 0`) → `CorruptRow`/`EnvelopeCorrupt`, not a panic.
- [ ] **Step 4: Linearizability (per-stream)** — port fjall's `reader_sees_monotonic_gapfree_while_writer_appends`: a `tokio::spawn` writer appending `5..=20` behind a `Barrier`, a concurrent `read_stream` asserting a strictly `prev+1` prefix, then a full re-scan asserting `1..=20`.
- [ ] **Step 5: Per-stream subscription (generic loop, unchanged)** — port fjall's `subscribe_catchup_then_live`: `Subscription::new(&store).subscribe(&id, None)`, drain catch-up, append a live event, observe it on the tail. **This is the core validation: the generic loop runs over postgres with only our `WakeSource`.**
- [ ] **Step 6: The `$all` skip-demonstration test** (the freeze evidence). Two transactions: A `BEGIN`s and inserts (gets the lower `global_seq`) but holds open; B inserts and commits (higher `global_seq`); a `read_all`-driven `$all` cursor consumes B and advances; A commits; assert the cursor **never delivers A's event** when resuming by `global_seq` alone. Document this as the empirical justification for the position-widening follow-up. (Use two explicit `pool.begin()` transactions with controlled commit ordering to force the interleaving deterministically.)

> Test-isolation helper: a `setup()` that connects via `DATABASE_URL`, runs `ensure_schema`, and `TRUNCATE events RESTART IDENTITY`. Skip the test (not fail) if `DATABASE_URL` is unset, so a `cargo test` with no DB doesn't red the suite — the nixosTest provides the URL in CI.

---

## Task 9: nixosTest integration harness (Linux-only flake attribute)

**Files:** `flake.nix`

- [ ] **Step 1: Build a runnable test archive** — use crane to produce the `nexus-postgres` test binaries as a nextest archive (so they run inside the VM without a toolchain). Add near the existing `craneLib` lets:

```nix
postgresTests = craneLib.mkCargoDerivation (commonArgs // {
  inherit cargoArtifacts;
  pname = "nexus-postgres-tests";
  buildPhaseCargoCommand = ''
    cargo nextest archive --package nexus-postgres \
      --archive-file $out/nexus-postgres.tar.zst
  '';
  doInstallCargoArtifacts = false;
});
```

- [ ] **Step 2: Expand the Linux-only `integration` stub** into the postgres test

```nix
packages = {
} // lib.optionalAttrs isLinux {
  postgres-integration = pkgs.testers.runNixOSTest {
    name = "nexus-postgres-integration";
    nodes.machine = { pkgs, ... }: {
      services.postgresql = {
        enable = true;
        ensureDatabases = [ "nexus_test" ];
        ensureUsers = [{
          name = "nexus";
          ensureDBOwnership = false;
        }];
        authentication = "local all all trust";
      };
      environment.systemPackages = [ pkgs.cargo-nextest pkgs.zstd ];
    };
    testScript = ''
      machine.wait_for_unit("postgresql.service")
      machine.succeed("su postgres -c \"createuser nexus || true\"")
      machine.succeed("su postgres -c \"psql -c 'GRANT ALL ON DATABASE nexus_test TO nexus'\"")
      machine.copy_from_host("${postgresTests}/nexus-postgres.tar.zst", "/tmp/tests.tar.zst")
      machine.succeed(
        "DATABASE_URL=postgres:///nexus_test?host=/run/postgresql "
        "cargo nextest run --archive-file /tmp/tests.tar.zst 2>&1 | tee /tmp/out"
      )
    '';
  };
};
```

> **Implementer note (flag concerns):** the exact `services.postgresql` user/auth wiring and the nextest-archive-in-VM invocation are the parts most likely to need iteration — the `unix-socket trust` auth + `DATABASE_URL` socket form is the simplest path. If the archive approach fights you, the fallback is a small dedicated `#[test]`-free binary that runs the assertions and is added to `environment.systemPackages`. Verify with `nix build .#postgres-integration` on a Linux box / CI.

- [ ] **Step 3: (Optional) CI step** — add a `postgres-integration` job to `.github/workflows/checks.yml` (ubuntu-latest) running `nix build .#postgres-integration`. Keep it a separate job from `Nix Flake Check` so it doesn't gate the fast checks.

---

## Task 10: Verify, findings writeup, commit

- [ ] **Step 1: Local build + clippy** (no DB needed — tests skip without `DATABASE_URL`)

```bash
nix develop -c cargo build -p nexus-postgres --all-targets
nix develop -c cargo clippy -p nexus-postgres --all-targets
nix develop -c cargo fmt --all
cargo hakari generate
```

- [ ] **Step 2: Run the integration test on Linux/CI** — `nix build .#postgres-integration` (or push and let the CI job run it). Confirm Tasks 8.1–8.5 pass and 8.6 (the skip test) demonstrates the documented behaviour.

- [ ] **Step 3: Write the findings into `docs/plans/2026-06-30-postgres-adapter-findings.md`** — what the contract validation surfaced: (a) the `$all` `Position = GlobalSeq` is too thin (skip test = evidence), (b) any `read_stream`/`subscribe` inclusivity friction hit, (c) anything in the trait docs that described fjall behaviour as contract. Feed these into #204–#210.

- [ ] **Step 4: File the follow-up card** — "[freeze] Widen the `$all` cursor position from `GlobalSeq` to `(txid, seq)`/opaque so a concurrent SQL adapter's live `$all` subscription is gap-safe", linking the skip test and the #213 decision. Decide opaque-vs-composite there.

- [ ] **Step 5: Commit** (the pre-commit hook runs `nix flake check`; do not run it by hand first)

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(postgres): sqlx-postgres event store adapter — contract validation (#213)

Second store adapter (nexus-postgres) on sqlx-postgres: RawEventStore
(append/read_stream/read_all) + WakeSource (LISTEN/NOTIFY), validating the
adapter contract against a real concurrent backend before the 1.0 freeze.

Per-stream path is fully correct and runs the unchanged generic Subscription
loop. Cross-stream $all uses the Way-2 pg_snapshot_xmin watermark (#213
decision) — safe per read_all pass. The residual live-$all resume gap is
demonstrated by a skip test and feeds the position-widening follow-up.

Tested via nixosTest (services.postgresql) as a Linux-only flake integration
attribute; local tests skip without DATABASE_URL.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Push + PR** (`gh pr create --base main`, "Relates to #213"). Do not close #213 — the position-widening follow-up is part of fully discharging it.

---

## Self-Review

**Spec coverage vs #213 ("write a 2nd adapter to validate the contract before freeze"):**
- Second adapter implementing `RawEventStore` + `WakeSource` on a networked backend: Tasks 3–7. ✓
- Runs the *unchanged* generic subscription loop: Task 8.5. ✓
- Concurrent-writer correctness exercised: Task 8.4 (per-stream linearizability) + 8.6 ($all hazard). ✓
- Findings fed back to #204–#210: Task 10.3. ✓
- The Way-2 decision implemented (watermark, parallel writers, no serialization): Task 6. ✓
- nixosTest as a separate integration attribute, not the local checks gate: Task 9. ✓

**Placeholder scan:** schema, error enum, append, row-rebuild, read_stream, read_all, and the flake nixosTest are concrete code. The two genuinely uncertain spots (`PgListener` `arm()` `'static` shaping; the nextest-archive-in-VM wiring) are called out with explicit implementer notes + fallbacks rather than hand-waved — they are the expected DONE_WITH_CONCERNS points.

**Type consistency:** `PostgresStore` carries `pool: PgPool`; `Stream`/`AllStream` are one `futures::stream::Iter` alias; `row_to_envelope` is the single rebuild path used by both reads; `store_err`/`corrupt`/`DisplayBytes` helpers are defined once in `store.rs`. `PostgresError` variants match every call site.

**Scope honesty:** the live-`$all` correctness gap is not hidden — it's implemented to the watermark, demonstrated by a test, documented in code + findings, and carved into a named follow-up. That is the validation working as intended, not a defect.
