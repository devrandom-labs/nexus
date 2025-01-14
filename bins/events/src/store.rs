use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Error, SqlitePool,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("{0}")]
    CannotCreatePool(#[from] Error),
}
pub struct Store {
    pool: SqlitePool,
}
impl Store {
    pub async fn new(name: &str) -> Result<Self, StoreError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(name)
                    .create_if_missing(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal),
            )
            .await?;
        Ok(Store { pool })
    }
}
