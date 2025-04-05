use super::error::Error;
use axum::http::StatusCode;
use pawz::jsend::Body;
use tracing::instrument;

pub mod health;
pub mod login;
pub mod register;

pub use health::route as health;
pub use login::route as login;
pub use register::route as register;

#[allow(dead_code)]
pub type AppResult<T> = Result<(StatusCode, Body<T>), Error>;

#[instrument]
pub async fn not_found() -> Error {
    Error::NotFound
}
