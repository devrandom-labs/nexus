use super::AppJson;
use super::error::Error;
use axum::http::StatusCode;
use pawz::jsend::Body;
use tracing::instrument;

pub mod health;
pub mod initiate_register;
pub mod login;
pub mod register;
pub mod verify_email;

pub use health::route as health;
pub use login::route as login;
pub use register::route as register;

#[allow(dead_code)]
pub type AppResult<T> = Result<(StatusCode, AppJson<Body<T>>), Error>;

#[instrument]
pub async fn not_found() -> Error {
    Error::NotFound
}
