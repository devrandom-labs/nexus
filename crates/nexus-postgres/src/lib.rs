//! PostgreSQL-backed event store adapter for nexus.
// `PostgresError` and `AppendError<PostgresError>` are intentionally
// stack-allocated (using `ErrorId` array-strings) for IoT targets — same
// rationale as `FjallError`. The large-Err lint fires at every return site;
// suppress it crate-wide.
#![allow(
    clippy::result_large_err,
    reason = "PostgresError is intentionally stack-allocated (~208 bytes) for IoT targets; same rationale as FjallError"
)]
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
mod hex;
mod position;
mod schema;
mod store;
mod wake;

pub use error::PostgresError;
pub use position::PgAllPos;
pub use store::PostgresStore;
