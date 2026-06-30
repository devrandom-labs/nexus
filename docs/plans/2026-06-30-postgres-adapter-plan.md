# Postgres Store Adapter (nexus-postgres) Implementation Plan (#213)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a second store adapter (`nexus-postgres`) on `sqlx`-postgres that implements the adapter contract (`RawEventStore` + `WakeSource`) and runs the *unchanged* generic subscription machinery — validating the about-to-freeze adapter traits against a real, networked, concurrent backend before #204–#210 lock.

**Architecture:** A single `events` table with a `BIGINT GENERATED ALWAYS AS IDENTITY` per-event sequence (`global_seq`), a `xid8` writing-transaction column (`txid`), and a `UNIQUE (stream_id, version)` constraint that enforces per-stream optimistic concurrency atomically. The adapter's [`AllPosition`](nexus_store::AllPosition) is the **composite** `PgAllPos { txid, seq }` (`Ord` = lexicographic); the `$all` read is the Way-2 watermark query ordered by `(txid, global_seq)` with **exclusive** `Ord`-based resume — so the live `$all` subscription is correct **by construction** (no late-commit skip), now that #266 made the `$all` position adapter-defined. Writers stay fully parallel (no serialization). Wake is `LISTEN/NOTIFY` via `sqlx`'s `PgListener`. Reads rebuild the canonical (now `global_seq`-free) wire frame via `nexus_store::wire::encode_frame`, so a `PersistedEnvelope` from postgres is byte-identical to one from fjall; the `$all` position rides *alongside* each item as a tag, never inside the envelope.

**Tech Stack:** Rust 2024, `sqlx` (postgres, runtime-tokio), `tokio`, `nexus-store` (subscription feature), `nixosTest` (`services.postgresql`) as a Linux-only flake integration attribute.

---

## Design recap (from the #213 decision + the #266 seam change)

The cross-stream ordering hazard: a `BIGSERIAL`/`IDENTITY` number is assigned at INSERT but rows are visible at COMMIT, and commit order ≠ insert order — so a lower number that commits late would be silently skipped by a `WHERE global_seq > bookmark` `$all` cursor (event loss). **Decision: close it on the reader (Way 2)** — keep insert-time identity, tag each row with `pg_current_xact_id()`, only consume rows below the oldest-in-flight-transaction watermark (`pg_snapshot_xmin(pg_current_snapshot())`), and order/resume by the composite `(txid, global_seq)`. Writers untouched, fully parallel.

**The freeze finding this surfaced — now resolved upstream (#266).** Way-2 needs the `$all` resume key to be `(txid, global_seq)`, but the old `Catchup::Position = GlobalSeq` was a single scalar. That seam was widened **before** this adapter: `nexus-store` now owns only an `AllPosition: Copy + Ord + ...` trait + `RawEventStore::AllPosition` associated type; the `$all` stream yields **position-tagged** `(AllPosition, PersistedEnvelope)` items; `global_seq` was dropped from the wire frame (→ frame V2) and from `PersistedEnvelope`; resume is **exclusive** and `Ord`-based (`WHERE pos > from`, no successor function). Commit `52d9480` (#266/#267) merged this — it is the contract this plan now targets. **Consequence:** postgres defines `type AllPosition = PgAllPos { txid, seq }` and its live `$all` subscription is correct *by construction* (the composite position + watermark make a late-commit skip unrepresentable), so this plan's earlier "demonstrate-the-skip-then-defer-a-fix" approach is replaced by a real **no-skip-under-concurrent-writers** linearizability test (Task 8.7).

---

## Scope

**In scope (this plan):**
- `nexus-postgres` crate: `RawEventStore` (`append`, `read_stream`, `read_all`) + `WakeSource` (`LISTEN/NOTIFY`) + `PgAllPos` (`AllPosition`) + error type.
- Per-stream correctness: `read_stream` + the generic *per-stream* `Subscription` (catch-up + live tail) running unchanged over postgres, plus a concurrent-writer linearizability test on `read_stream`.
- `read_all` watermark-filtered + `(txid, global_seq)`-ordered, position-tagged, **exclusive** `Ord`-based resume — correct for both a single catch-up pass *and* the live tail (no late-commit skip).
- The generic `$all` `Subscription` (catch-up + live tail) running unchanged over postgres.
- `nixosTest` integration harness (Linux-only flake attribute).
- A test that **proves the `$all` subscription does NOT skip** under concurrent writers with controlled out-of-order commits (the freeze evidence that the #266 seam is right).

**Out of scope:**
- ~~Widening the `$all` position~~ — **done upstream in #266** (this plan consumes the result; no follow-up card).
- `SnapshotStore`, `AtomicAppend`, `StreamLister`, export/import on postgres (defer; not needed to validate the core contract).
- Connection-pool tuning, migrations framework, production hardening.
- The high-throughput WAL/logical-decoding `$all` path (deferred to #265).

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
| `crates/nexus-postgres/tests/*.rs` | The 4 CLAUDE testing categories + the `$all` no-skip linearizability test. |
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
- `BIGINT GENERATED ALWAYS AS IDENTITY` (not `BIGSERIAL`) — SQL-standard, prevents manual inserts into the id column; same insert-time sequence behaviour. The `i64` column → the `seq: u64` of `PgAllPos` via `u64::try_from` (IDENTITY starts at 1, never overflows downward).
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
  --features "runtime-tokio,postgres,macros"
```
> **No TLS backend** (decided 2026-07-01): `runtime-tokio` (not `-rustls`/`-native-tls`). The validation skeleton connects over local / unix-socket Postgres, so no TLS is exercised; `runtime-tokio-rustls` pulls `webpki-roots` (`CDLA-Permissive-2.0`, not in `deny.toml` → gate fail) and `native-tls` pulls `openssl-sys` (fights Nix reproducibility + IoT cross-compile). TLS-to-managed-Postgres is the deferred follow-up **#268**. (DONE in scaffolding commit `56e06a3`.)
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
//! `LISTEN/NOTIFY` wake and a `pg_snapshot_xmin` watermark on the `$all` read.
//! Its [`AllPosition`](nexus_store::AllPosition) is the composite
//! [`PgAllPos`] `(txid, seq)` (the #213 ordering decision, made correct by
//! construction via the #266 adapter-defined-position seam). The second
//! adapter, written to validate the adapter contract before the 1.0 freeze.

mod builder;
mod error;
mod position;
mod schema;
mod store;
mod wake;

pub use builder::PostgresStore;
pub use error::PostgresError;
pub use position::PgAllPos;
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
use nexus::ErrorId; // NB: ErrorId lives in the `nexus` kernel, NOT re-exported from nexus_store (matches nexus-fjall)
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

## Task 3.5: `PgAllPos` — the adapter-defined `$all` position

**Files:** `crates/nexus-postgres/src/position.rs`

The composite `(txid, seq)` resume position. `nexus-store` owns only the `AllPosition` trait (`Copy + Ord + Send + Sync + Debug + 'static`); the concrete type lives here (dependency direction — the store can't name its adapters). `Ord` is the derived lexicographic order on `(txid, seq)`, which is exactly the `ORDER BY txid, global_seq` / row-value `>` the read query uses.

- [ ] **Step 1: Define `PgAllPos`**

```rust
//! `PgAllPos` — postgres's [`AllPosition`](nexus_store::AllPosition).
//!
//! The composite `(txid, seq)` commit-ordered position. `txid` is the writing
//! transaction's `xid8` (`pg_current_xact_id()`); `seq` is the row's
//! `IDENTITY` `global_seq`. Lexicographic `Ord` on `(txid, seq)` is the
//! Way-2 order: resume strictly after the last delivered position, and the
//! `pg_snapshot_xmin` watermark guarantees no smaller `(txid, seq)` can become
//! visible after a larger one is consumed.

/// PostgreSQL `$all` resume position: `(txid, seq)`, lexicographic order.
///
/// Fields are **private** — the `(u64, u64)` layout is an implementation
/// detail, not the public contract. A consumer checkpoints a `PgAllPos` and
/// hands it back to resume; it reconstructs one from its two persisted scalars
/// via [`new`](Self::new) and reads them via [`txid`](Self::txid) /
/// [`seq`](Self::seq), but it cannot mutate one in place or depend on the field
/// layout. That representation-independence is what lets the position evolve
/// after the 1.0 freeze (the public surface is `new` + two accessors + `Ord`,
/// not "a struct with two pub u64s").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PgAllPos {
    /// The writing transaction id (`xid8`, 64-bit, non-wrapping).
    txid: u64,
    /// The row's `IDENTITY` `global_seq` (always >= 1).
    seq: u64,
}

impl PgAllPos {
    /// Construct from the `(txid, seq)` columns read off a row — or rebuilt from
    /// a persisted consumer checkpoint.
    #[must_use]
    pub const fn new(txid: u64, seq: u64) -> Self {
        Self { txid, seq }
    }

    /// The writing transaction id component (serialize this to checkpoint).
    #[must_use]
    pub const fn txid(self) -> u64 {
        self.txid
    }

    /// The `global_seq` component (serialize this to checkpoint).
    #[must_use]
    pub const fn seq(self) -> u64 {
        self.seq
    }
}

impl nexus_store::AllPosition for PgAllPos {}
```

> **Ord note:** field **declaration** order is the `derive(Ord)` tie-break order — `txid` first, then `seq` (private fields don't change this). It must match the SQL `ORDER BY txid, global_seq` and the row-value comparison `(txid, global_seq) > ($1, $2)` exactly, or resume skips/repeats. A sequence/protocol test asserts `PgAllPos::new(1, 9) < PgAllPos::new(2, 0)` (lower txid wins regardless of seq) to lock the tie-break.

---

## Task 4: `append` (atomic version check + identity seq + txid + NOTIFY)

**Files:** `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: TDD — write the append round-trip test first** (in `tests/conformance_tests.rs`, gated behind the integration harness; see Task 8 for how it runs). The driving assertions:
  - append 2 events to a fresh stream with `expected_version = None` → `read_stream` returns versions `[1,2]` with correct payloads and `global_seq` ascending.
  - append with a stale `expected_version` → `AppendError::Conflict { expected, actual }`.
  - two concurrent appends racing the same version → exactly one `Ok`, the other `AppendError::Conflict` (the UNIQUE-violation path).

- [ ] **Step 2: Implement `append` — a thin IO shell over a pure decision core**

The decision logic (optimistic check + strict-sequential versions + range narrowing) is a **pure, `await`-free, DB-free function** `prepare_inserts`; `append` is then a mechanical begin → read-version → *prepare* → bind-and-insert → commit → notify. This is the seam that matters: every interesting failure path is decided in code that needs **no Postgres to unit-test** (Step 3), and the async body holds only IO.

```rust
/// A validated, DB-typed insert row, borrowing its bytes from the pending
/// envelope. The narrowed `version`/`schema_version` only exist *after*
/// validation — holding a `PreparedInsert` is proof the batch passed the
/// optimistic + strict-sequential + range checks. ("Justify every type": it
/// carries the post-validation DB scalars that don't exist on `PendingEnvelope`.)
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
/// each to its DB column type. Returns the rows ready to INSERT, or the first
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
            // `u32` — narrow with try_from (rule 2), never bind an i64 to int4.
            let schema_version = i32::try_from(env.schema_version()).map_err(|_| {
                AppendError::Store(PostgresError::CorruptRow {
                    stream_id: ErrorId::from_display(id),
                    reason: ErrorId::from_display(&"schema_version exceeds i32::MAX"),
                })
            })?;
            Ok(PreparedInsert { version, schema_version, env })
        })
        .collect()
}

async fn append(
    &self,
    id: &StreamKey,
    expected_version: Option<Version>,
    envelopes: &[PendingEnvelope],
) -> Result<(), AppendError<Self::Error>> {
    let mut tx = self.pool.begin().await.map_err(store_err)?;

    let current = read_current_version(&mut tx, id).await?; // IO
    let rows = prepare_inserts(current, expected_version, envelopes, id)?; // PURE
    if rows.is_empty() {
        return Ok(()); // version checked; nothing to write
    }

    for row in &rows {
        let result = sqlx::query(
            "INSERT INTO events (stream_id, version, event_type, schema_version, payload, metadata) \
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
            // A UNIQUE(stream_id, version) violation means a concurrent writer
            // claimed this version first — a conflict, not a Store error.
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

    // Wake AFTER durable commit (rule: WakeSource::wake post-commit).
    self.notify_committed(id.as_bytes()).await;
    Ok(())
}
```

Helpers (same file):
```rust
/// Read the stream's current (max) version inside `conn`. Absent stream → 0.
/// A *negative* stored version is corruption surfaced as an error, NOT a silent
/// 0 (rule 2 — no `unwrap_or(sentinel)` on a failed conversion).
async fn read_current_version(
    conn: &mut sqlx::PgConnection,
    id: &StreamKey,
) -> Result<u64, AppendError<PostgresError>> {
    let current: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) FROM events WHERE stream_id = $1",
    )
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

fn store_err<E: Into<sqlx::Error>>(e: E) -> AppendError<PostgresError> {
    AppendError::Store(PostgresError::Sqlx(e.into()))
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.is_unique_violation())
}
```

> Call site passes `&mut *tx` (a `Transaction` derefs to `&mut PgConnection`) into `read_current_version`. Keeping that helper's parameter a bare `&mut PgConnection` (not `&Transaction`) lets it run on any executor.

> **No `DisplayBytes` wrapper.** `StreamKey` already implements `Display` (it renders its id bytes as lossy UTF-8 for diagnostics), so an error label is just `ErrorId::from_display(id)` — no newtype, no `String::from_utf8_lossy` allocation. (`id.as_bytes()` is still used for the `$1` *bind*, where the raw bytes are the value; only the human-facing label uses `Display`.)

Note: we do not insert `global_seq` or `txid` — `IDENTITY` and the `DEFAULT pg_current_xact_id()` assign them. We never `RETURNING` them on write (the read path supplies them).

- [ ] **Step 3: Unit-test `prepare_inserts` with NO database** (the dividend of the pure/IO split). In-module `#[cfg(test)]` tests that build `PendingEnvelope`s and assert directly on the returned `Result` — these run under the plain `nix flake check` with no `DATABASE_URL`, covering the logic most likely to harbor an off-by-one:
  - `current = 0`, `expected = None`, versions `[1,2,3]` → `Ok` of 3 rows with `version` `[1,2,3]`.
  - `current = 5`, `expected = Some(5)`, versions `[6,7]` → `Ok` of 2 rows.
  - stale `expected` (`Some(4)` when `current = 5`) → `Conflict { expected: Some(4), actual: Some(5) }`.
  - gapped/out-of-order batch (`current = 0`, versions `[1,3]` or `[2,1]`) → `Conflict` at the first offender, with the right `expected`/`actual`.
  - empty batch → `Ok(vec![])` (so `append` short-circuits *after* the version check).
  - `schema_version > i32::MAX` → `Store(CorruptRow)`, not a panic.
  This is the bulk of the append contract (CLAUDE rule 7: sequence + defensive-boundary) proven without a VM; the live-DB tests (Task 8) then only need to cover the genuinely concurrent UNIQUE-violation race and the actual persistence round-trip.

---

## Task 5: `read_stream` + the row→`PersistedEnvelope` rebuild

**Files:** `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: The row type + rebuild helper** (rebuild the canonical aligned frame via `wire::encode_frame`)

The envelope is built from the **per-event** columns only — `global_seq` is no longer part of the frame (it moved to the `$all` position tag), so `row_to_envelope` does not touch it. The `$all` read (Task 6) pulls `txid`/`global_seq` separately to build the `PgAllPos` tag.

```rust
use nexus_store::wire;
use nexus_store::value::{EventType, Metadata, Payload, SchemaVersion};
use nexus_store::envelope::PersistedEnvelope;

/// The per-event columns that reconstitute a `PersistedEnvelope`.
/// Note: NO `global_seq` — it is an `$all`-position concern, not an envelope one.
#[derive(sqlx::FromRow)]
struct EventRow {
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
    let schema = u32::try_from(row.schema_version).ok()
        .and_then(|v| SchemaVersion::new(std::num::NonZeroU32::new(v)?))
        .ok_or_else(|| corrupt(stream_label, "schema_version == 0 or out of range"))?;

    // NB: the validating constructor is `from_bytes(Bytes)`, NOT `::new` —
    // `String`/`Vec<u8>` both impl `Into<Bytes>`, so `.into()` targets the Bytes arg.
    let event_type = EventType::from_bytes(row.event_type.into())
        .map_err(|_| corrupt(stream_label, "event_type invalid"))?;
    let payload = Payload::from_bytes(row.payload.into())
        .map_err(|_| corrupt(stream_label, "payload too large"))?;
    let metadata = row.metadata
        .map(|m| Metadata::from_bytes(m.into()))
        .transpose()
        .map_err(|_| corrupt(stream_label, "metadata too large"))?;

    // Rebuild the canonical, 16-byte-aligned V2 frame so a postgres envelope is
    // byte-identical to a fjall one. encode_frame owns the layout + alignment;
    // the frame carries NO global_seq (frame V2, #266) — global position lives
    // only on the $all stream's position tag.
    let frame = wire::encode_frame(schema, &event_type, &payload, metadata.as_ref())
        .map_err(|e| PostgresError::Frame {
            stream_id: stream_label,
            version: version.as_u64(),
            reason: ErrorId::from_display(&e),
        })?;

    PersistedEnvelope::try_new(
        version,
        frame.value,
        schema,
        frame.offsets.event_type,
        frame.offsets.payload,
        frame.offsets.metadata,
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
    let label = ErrorId::from_display(id);
    let from_i64 = i64::try_from(from.as_u64())
        .map_err(|_| corrupt(label, "from version exceeds i64::MAX"))?;
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT version, event_type, schema_version, payload, metadata \
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

`type Stream = futures::stream::Iter<std::vec::IntoIter<Result<PersistedEnvelope, PostgresError>>>` — an owned, `Send`, `'static` iterator-backed stream of `Result<PersistedEnvelope, PostgresError>`, which satisfies `EventStream`. (Materializing the whole stream is fine for the validation skeleton; a keyset-paginated cursor is a later optimization, and the contract explicitly allows either.)

> **Implementer note on associated types:** `Stream` and `AllStream` are now **two different** `futures::stream::Iter` aliases, because their items differ — `Stream` yields `Result<PersistedEnvelope, _>` (per-stream, position-free) while `AllStream` yields `Result<(PgAllPos, PersistedEnvelope), _>` (position-tagged, #266). Define both aliases in `store.rs`:
> ```rust
> type Stream    = futures::stream::Iter<std::vec::IntoIter<Result<PersistedEnvelope, PostgresError>>>;
> type AllStream = futures::stream::Iter<std::vec::IntoIter<Result<(PgAllPos, PersistedEnvelope), PostgresError>>>;
> ```

---

## Task 6: `read_all` (watermark + composite `(txid, seq)` position — the Way-2 read)

**Files:** `crates/nexus-postgres/src/store.rs`

- [ ] **Step 1: The `$all` row type** — envelope columns **plus** the position columns `txid`/`global_seq`

```rust
/// `$all` row = the position columns + a flattened `EventRow`. `#[sqlx(flatten)]`
/// (sqlx 0.8) reads `EventRow`'s columns from the same flat result row, so there
/// is ONE envelope-column definition reused by both reads — no hand repack.
#[derive(sqlx::FromRow)]
struct AllEventRow {
    txid: i64,        // xid8 read back as bigint via cast (see SQL note)
    global_seq: i64,
    #[sqlx(flatten)]
    event: EventRow,
}
```

- [ ] **Step 2: Implement `read_all`** — settled rows only (watermark), strictly after `from` (exclusive `Ord` resume), ordered + tagged by `(txid, global_seq)`

```rust
async fn read_all(&self, from: Option<PgAllPos>) -> Result<Self::AllStream, Self::Error> {
    let label = ErrorId::default();
    // Absence is expressed as SQL NULL, NOT a magic sentinel (CLAUDE rule 3 —
    // "unknown values must be Option, not sentinels"). `from = None` binds two
    // NULLs and the `$1 IS NULL` guard short-circuits the resume predicate, so
    // the read starts from the very beginning. `from = Some` binds the pair and
    // the row-value comparison applies. One query, no out-of-band -1.
    let (from_txid, from_seq): (Option<i64>, Option<i64>) = match from {
        Some(p) => (
            Some(i64::try_from(p.txid).map_err(|_| corrupt(label, "from txid exceeds i64::MAX"))?),
            Some(i64::try_from(p.seq).map_err(|_| corrupt(label, "from seq exceeds i64::MAX"))?),
        ),
        None => (None, None),
    };

    // Way-2 read. TWO predicates, both required for by-construction correctness:
    //   1. watermark `txid < pg_snapshot_xmin(pg_current_snapshot())` — never
    //      consume a row whose writing txn (or any older still-in-flight txn)
    //      hasn't settled, so no smaller (txid, seq) can appear *after* a larger
    //      one is delivered. This is what makes the live $all gap-free.
    //   2. exclusive resume `$1 IS NULL OR (txid, global_seq) > ($1, $2)` —
    //      Ord-based strict-after when resuming, unbounded-below when starting
    //      fresh; matches PgAllPos's lexicographic derive(Ord).
    // ORDER BY (txid, global_seq) is native xid8/bigint ascending — the same
    // order PgAllPos sorts by (xid8 is an unsigned 64-bit, non-wrapping count).
    let rows: Vec<AllEventRow> = sqlx::query_as(
        "SELECT txid::text::bigint AS txid, global_seq, version, event_type, \
                schema_version, payload, metadata \
         FROM events \
         WHERE ($1::bigint IS NULL OR (txid::text::bigint, global_seq) > ($1, $2)) \
           AND txid < pg_snapshot_xmin(pg_current_snapshot()) \
         ORDER BY txid, global_seq",
    )
    .bind(from_txid)
    .bind(from_seq)
    .fetch_all(&self.pool)
    .await
    .map_err(PostgresError::Sqlx)?;

    Ok(futures::stream::iter(rows.into_iter().map(move |r| {
        let txid = u64::try_from(r.txid).map_err(|_| corrupt(label, "txid out of range"))?;
        let seq = u64::try_from(r.global_seq).map_err(|_| corrupt(label, "global_seq <= 0 or out of range"))?;
        // `r.event` is the flattened EventRow — reuse the single rebuild path.
        let env = row_to_envelope(r.event, label)?;
        Ok((PgAllPos::new(txid, seq), env))
    })))
}
```

> **Why this is correct by construction (no skip).** The hazard was: a low `global_seq` that commits *after* a higher one is consumed gets skipped by a bare-scalar resume. Here the watermark refuses to deliver *any* row until every transaction with an id ≤ its `txid` has settled, and resume is the composite `(txid, seq)` — so once `(t, s)` is delivered, the only rows that can still appear have `(txid, seq) > (t, s)`. A late-committing lower position is therefore *never* jumped over. This holds across reopens of the live loop because the loop resumes from the last delivered `PgAllPos`, not a scalar. (Known Way-2 caveat: one long-running transaction holds the watermark back and stalls `$all` consumers until it finishes — documented, accepted; the high-throughput WAL path is #265.)

> **SQL note (implementer — flag if it fights you):** `xid8` has no direct cast to `bigint`; `txid::text::bigint` round-trips it (valid while the xid8 value ≤ `i64::MAX`, i.e. effectively forever). `pg_snapshot_xmin(...)` returns `xid8`, so the watermark comparison `txid < pg_snapshot_xmin(...)` stays native (no cast). If sqlx grows first-class `xid8` decoding, drop the cast. The cast defeats the `events_watermark_idx` for the resume predicate — fine for the validation skeleton (materialize-all). For a later keyset-paginated cursor, keep `txid` native in the predicate so the index applies — bind the resume position back *to* `xid8` (`WHERE (txid, global_seq) > ($1::text::xid8, $2)`) rather than casting the column down to `bigint`.

> ⚠️ **`read_stream` and `read_all` `fetch_all` the WHOLE result into a `Vec` — this skeleton is explicitly NOT production-shaped.** Resident memory is `O(rows matched)`, unbounded: a cold `$all` catch-up over a large store, or a `read_stream` over a long aggregate, loads every matched row at once. The trait contract *permits* this ("an adapter may chunk or paginate internally but is not required to") and it is the right amount of code to **validate the contract**, which is this crate's whole job (#213). It is the wrong amount of code to **run in production**: bounding resident rows is the adapter's responsibility, and the rest of the codebase already sets that bar (fjall's single lazy LSM cursor; `BatchSize`'s `1..=MAX_BATCH` resident-row invariant). The production path here is a **keyset-paginated cursor** — a `futures::stream` that pulls the next page (`... AND (key) > $last ORDER BY key LIMIT $batch`) only as the current one drains, resuming on `version` (per-stream) / native-`xid8` `(txid, global_seq)` (`$all`, see the SQL note). Track it as a named follow-up; do **not** let "the skeleton materializes" quietly become the shipped behaviour.

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
> 2. Reuse `nexus_store::notify::StreamNotifiers` directly: the `PgListener` background task calls `notifiers.wake(stream)` on each NOTIFY, and `register`/`arm` delegate to `StreamNotifiers`. **This is preferred** — it reuses the audited in-process wake machinery and keeps `LISTEN/NOTIFY` as a thin transport that just drives `wake()`. `StreamNotifiers::new()` returns `Arc<Self>` (built via `Arc::new_cyclic`); `register(&self, stream: Option<&[u8]>)`, `wake(&self, stream: &[u8])` — mirror exactly how `nexus-fjall/src/store.rs` holds `notifiers: Arc<StreamNotifiers>` and delegates.
>
> **Why option 2 is lost-wakeup-safe — keep this as a load-bearing comment at the wake-task site (verified against sqlx 0.8 docs).** `PgListener` **auto-reconnects** on connection loss, and *"any notifications received while the connection was lost will not be returned"* — NOTIFYs can be silently dropped. That is tolerable **only** because `StreamNotifiers` drives nexus-store's arm-before-confirm-rescan discipline: every `wake()` (and every reopen of the live loop) re-scans the store via `read_stream`/`read_all`, so a dropped NOTIFY merely **delays** a wake until the next scan — it never loses the event. A future refactor must NOT "optimize away" that redundant catch-up scan: without it, a dropped NOTIFY becomes a lost event. The NOTIFY is a *hint to scan sooner*, never the source of truth.

- [ ] **Step 3: `WakeSource for PostgresStore`** — delegate to a `StreamNotifiers` driven by one shared `PgListener` task (option 2 above). Mirror `nexus-fjall/src/store.rs`'s `impl WakeSource`.

- [ ] **Step 4: Wire `RawEventStore` impl** — declare `type Error = PostgresError`, `type Stream` + `type AllStream` (the **two distinct** `futures::stream::Iter` aliases from Task 5's note), and `type AllPosition = PgAllPos`, with the `append`/`read_stream`/`read_all` from Tasks 4–6.

---

## Task 8: Tests — the 4 categories + the `$all` no-skip evidence

**Files:** `crates/nexus-postgres/tests/conformance_tests.rs`, `tests/subscription_tests.rs`, `tests/all_noskip_tests.rs`

These run against a live postgres provided either by a `DATABASE_URL` env var (local dev loop) or the nixosTest VM (CI). Each test creates a uniquely-named schema/table prefix or uses `TRUNCATE events` in setup for isolation.

**Dev-deps + test mechanics (audit findings — get these right or the tests won't compile):**
- `nexus-store-testing` exports the **canonical #266 conformance suites** — `assert_event_stream_conformance` (per-stream) and `assert_all_stream_conformance` (the `$all` position-tagged read path). **Run both against `PostgresStore`** (Step 0 below); they are the exact contract checks the seam change added, so they directly validate the adapter against the same bar fjall/in-memory meet.
- `InMemoryStore` is NOT in `nexus-store-testing` — it's `nexus_store::testing::InMemoryStore` (behind the `testing` feature). The postgres tests don't need it; they exercise `PostgresStore` directly. (If a test *does* reference it, add `nexus-store = { path = "...", features = ["testing"] }` as a dev-dep.)
- `Subscription::new` takes `&Store<S>`, not `&PostgresStore` — wrap first: `let store = pg.into_store();` then `Subscription::new(&store)`. The returned subscription stream is **`!Unpin`** (it's the `unfold` of the live loop), so `futures::pin_mut!`/`tokio::pin!` it before `.next()` — mirror the fjall subscription tests exactly.

- [ ] **Step 0: Shared conformance suites** — `assert_event_stream_conformance(&store)` and `assert_all_stream_conformance(&store)` from `nexus_store_testing`, against a `PostgresStore` (wrapped per above). The fastest proof the adapter honors the post-#266 contract.
- [ ] **Step 1: Sequence/protocol** — append→read_stream versions `[1,2,3]`; append with stale expected → Conflict; multi-event batch in one append.
- [ ] **Step 2: Lifecycle** — append, drop the pool, reconnect (`from_pool`), `read_stream` rehydrates identical state (mirrors fjall's close/reopen).
- [ ] **Step 3: Defensive boundary** — `read_stream` on a nonexistent stream → empty; `read_stream(from = beyond head)` → empty; a hand-inserted corrupt row (e.g. `schema_version = 0`) → `CorruptRow`/`EnvelopeCorrupt`, not a panic.
- [ ] **Step 4: Linearizability (per-stream)** — port fjall's `reader_sees_monotonic_gapfree_while_writer_appends`: a `tokio::spawn` writer appending `5..=20` behind a `Barrier`, a concurrent `read_stream` asserting a strictly `prev+1` prefix, then a full re-scan asserting `1..=20`.
- [ ] **Step 5: Per-stream subscription (generic loop, unchanged)** — port fjall's `subscribe_catchup_then_live`: `Subscription::new(&store).subscribe(&id, None)`, drain catch-up, append a live event, observe it on the tail. **Core validation: the generic loop runs over postgres with only our `WakeSource`.**
- [ ] **Step 6: `$all` subscription (generic loop, unchanged)** — `Subscription::new(&store).subscribe_all(None)`: append across **two different streams**, drain catch-up, append a live event to a third, observe it on the tail. Assert delivered positions are strictly ascending `PgAllPos`. Validates the position-tagged `AllStream` + exclusive `Ord` resume driving the generic loop with no per-event `global_seq` field.
- [ ] **Step 7: The `$all` NO-skip test under out-of-order commits** (the freeze evidence — what #266 buys). Force the exact interleaving the old scalar position lost: transaction A `BEGIN`s and inserts to stream X (claims the lower `global_seq`) but holds open; transaction B inserts to stream Y and **commits** (higher `global_seq`); a `read_all`-driven `$all` cursor runs — assert it delivers **nothing yet** (B is withheld; the watermark sits at A's still-in-flight `txid`); A commits; re-drive the cursor from the beginning — assert it now delivers **both** events in `(txid, global_seq)` order **with A's event present** (the event a bare-`global_seq` cursor would have skipped). The linearizability proof that composite position + watermark are gap-free by construction. (Two explicit `pool.begin()` transactions with controlled commit ordering force the interleaving deterministically.)

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

- [ ] **Step 2: Run the integration test on Linux/CI** — `nix build .#postgres-integration` (or push and let the CI job run it). Confirm Tasks 8.1–8.6 pass and 8.7 (the no-skip test) proves the documented gap-free behaviour.

- [ ] **Step 3: Write the findings into `docs/plans/2026-06-30-postgres-adapter-findings.md`** — what the second adapter confirmed/surfaced: (a) the #266 adapter-defined `AllPosition` seam holds up — postgres's composite `(txid, seq)` + watermark makes live `$all` gap-free by construction (no-skip test = evidence); (b) the intentional inclusive-`read_stream` / exclusive-`read_all` asymmetry caused no friction (or note any it did); (c) anything *else* in the trait docs still describing fjall behaviour as contract. Feed these into #204–#210.

- [ ] **Step 4: ~~File a position-widening follow-up~~ — N/A.** The widening was #266 (merged); this adapter consumes it. If the no-skip test instead reveals a *residual* gap, that is a real finding — capture it in the findings doc and a fresh issue, don't silently absorb it.

- [ ] **Step 5: Commit** (the pre-commit hook runs `nix flake check`; do not run it by hand first)

```bash
git add -A
git commit -m "$(cat <<'EOF'
feat(postgres): sqlx-postgres event store adapter — contract validation (#213)

Second store adapter (nexus-postgres) on sqlx-postgres: RawEventStore
(append/read_stream/read_all) + WakeSource (LISTEN/NOTIFY) + PgAllPos
(AllPosition), validating the adapter contract against a real concurrent
backend before the 1.0 freeze.

Both the per-stream and the $all paths run the unchanged generic Subscription
loop. The $all path uses the Way-2 pg_snapshot_xmin watermark plus the
composite (txid, seq) position (the #266 adapter-defined-position seam), so the
live $all subscription is gap-free by construction — proven by a no-skip
linearizability test under out-of-order commits.

Tested via nixosTest (services.postgresql) as a Linux-only flake integration
attribute; local tests skip without DATABASE_URL.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Push + PR** (`gh pr create --base main`, "Relates to #213"). Closing #213 is the maintainer's call once the findings land in #204–#210.

---

## Self-Review

**Spec coverage vs #213 ("write a 2nd adapter to validate the contract before freeze"):**
- Second adapter implementing `RawEventStore` (incl. `AllPosition = PgAllPos`) + `WakeSource` on a networked backend: Tasks 3–7. ✓
- Runs the *unchanged* generic subscription loop, both per-stream and `$all`: Tasks 8.5 + 8.6. ✓
- Concurrent-writer correctness exercised: Task 8.4 (per-stream linearizability) + 8.7 (`$all` no-skip under out-of-order commits). ✓
- Findings fed back to #204–#210: Task 10.3. ✓
- The Way-2 decision implemented (watermark + composite position, parallel writers, no serialization): Tasks 3.5 + 6. ✓
- nixosTest as a separate integration attribute, not the local checks gate: Task 9. ✓

**Placeholder scan:** position type, schema, error enum, append, row-rebuild, read_stream, read_all, and the flake nixosTest are concrete code. The two genuinely uncertain spots (`PgListener` `arm()` `'static` shaping; the nextest-archive-in-VM wiring) are called out with explicit implementer notes + fallbacks rather than hand-waved — they are the expected DONE_WITH_CONCERNS points. The `xid8`→`bigint` cast is flagged as a skeleton choice with a drop-the-cast upgrade path.

**Type consistency:** `PostgresStore` carries `pool: PgPool`; `type AllPosition = PgAllPos` (private fields + `new`/`txid`/`seq`, derived lexicographic `Ord` = the SQL `ORDER BY txid, global_seq`); `Stream` and `AllStream` are **two distinct** `futures::stream::Iter` aliases (per-stream `PersistedEnvelope` vs position-tagged `(PgAllPos, PersistedEnvelope)`); `EventRow` is the single envelope-column shape — `AllEventRow` composes it via `#[sqlx(flatten)]`, and `row_to_envelope` is the single rebuild path both reads share (the `$all` path tags its output with the position). `append` is a thin IO shell over the pure `prepare_inserts` (+ `PreparedInsert`) decision core and the `read_current_version` helper; `store_err`/`is_unique_violation`/`corrupt` helpers and the `prepare_inserts` core are each defined once in `store.rs`; stream-id labels reuse `StreamKey`'s own `Display` (no `DisplayBytes` newtype). `PostgresError` variants match every call site.

**Correctness honesty:** the `$all` path is gap-free **by construction** (composite position + watermark, the #266 seam), not "best-effort + a deferred fix." The no-skip test (8.7) is the proof, not a demonstration of a known-broken case. If that test ever fails, it is a genuine finding to escalate (Task 10 Step 4 — findings doc + a fresh issue) — not something the plan pre-absorbs.

**Design craftsmanship (judged against the CLAUDE rules + general principles, NOT the in-house fjall patterns — those are being reworked):**
- **Pure/IO seam.** `append`'s decision logic (optimistic check, strict-sequential versions, range narrowing) is the pure `prepare_inserts`; the async body is mechanical IO. The bulk of the append contract is therefore unit-testable with **no database** (Task 4 Step 3) — directly buying back the "tests need a live DB" cost.
- **No sentinels (rule 3).** `read_all`'s `from = None` is SQL `NULL` (`$1 IS NULL OR …`), not a magic `(-1,-1)`. Stored-version corruption is an error, not `unwrap_or(0)` (rule 2).
- **No gratuitous types/allocations (rules 4, 6).** Reuse `StreamKey: Display` instead of a `DisplayBytes` newtype + `String` alloc; reuse `EventRow` via `#[sqlx(flatten)]` instead of a hand repack; `PreparedInsert` borrows envelope bytes rather than copying.
- **Representation independence at the freeze boundary.** `PgAllPos` hides its `(u64,u64)` layout behind `new`/`txid`/`seq`/`Ord`, so the public `$all`-checkpoint contract can evolve post-1.0.
- **Single-responsibility error mapping (rule 3).** A UNIQUE violation → `Conflict` (retryable); everything else → `Store`; `xid8`/text-cast and `int4` narrowing failures → `CorruptRow`. No domain is jammed into another.
