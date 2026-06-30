use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::error::PostgresError;
use crate::schema::ensure_schema;
use crate::store::PostgresStore;

impl PostgresStore {
    /// Connect to `url`, create a connection pool, and ensure the schema exists.
    ///
    /// # Errors
    ///
    /// Returns [`PostgresError::Sqlx`] on connection failure or schema DDL
    /// failure.
    pub async fn connect(url: &str) -> Result<Self, PostgresError> {
        let pool = PgPoolOptions::new()
            .connect(url)
            .await
            .map_err(PostgresError::Sqlx)?;
        Self::from_pool(pool).await
    }

    /// Build from an existing pool and ensure the schema exists.
    ///
    /// Prefer this when the caller already holds a `PgPool` (e.g. a shared
    /// pool across multiple stores in an application).
    ///
    /// # Errors
    ///
    /// Returns [`PostgresError::Sqlx`] if the schema DDL fails.
    pub async fn from_pool(pool: PgPool) -> Result<Self, PostgresError> {
        ensure_schema(&pool).await?;
        Ok(Self { pool })
    }
}
