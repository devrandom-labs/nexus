use sqlx::PgPool;

/// PostgreSQL-backed event store.
///
/// Implements [`RawEventStore`](nexus_store::RawEventStore) and
/// [`WakeSource`](nexus_store::wake::WakeSource) (Tasks 4–7). Constructed via
/// [`connect`](Self::connect) or [`from_pool`](Self::from_pool) in
/// [`builder`](crate::builder).
///
/// Clone is cheap: `PgPool` is already `Arc`-backed.
#[derive(Clone)]
pub struct PostgresStore {
    /// The shared connection pool. Fields used in the next dispatch (Tasks 4–7):
    /// `pool` drives `append`, `read_stream`, `read_all`, and the `NOTIFY`
    /// wake path. The `#[allow]` is removed once those impls land.
    #[allow(dead_code, reason = "field used in next dispatch (Tasks 4-7)")]
    pub(crate) pool: PgPool,
}
