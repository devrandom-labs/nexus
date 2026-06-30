use sqlx::PgPool;

use crate::error::PostgresError;

/// The events table and indexes. See the plan's schema section for rationale.
///
/// - `BIGINT GENERATED ALWAYS AS IDENTITY` (not `BIGSERIAL`) — SQL-standard,
///   prevents manual inserts into the id column.
/// - `txid xid8 DEFAULT pg_current_xact_id()` — writing transaction id, the
///   Way-2 watermark key. `xid8` is 64-bit, non-wrapping.
/// - `UNIQUE (stream_id, version)` — per-stream conflict arbiter; a
///   concurrent INSERT that races the same version raises a unique violation →
///   `AppendError::Conflict`. Atomic, no `SELECT … FOR UPDATE` needed.
const SCHEMA_SQL: &str = r"
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
";

/// Apply the schema if absent. Idempotent — safe to call on every open.
///
/// # Errors
///
/// Returns [`PostgresError::Sqlx`] if the DDL statements fail.
pub async fn ensure_schema(pool: &PgPool) -> Result<(), PostgresError> {
    sqlx::raw_sql(SCHEMA_SQL)
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(PostgresError::Sqlx)
}
