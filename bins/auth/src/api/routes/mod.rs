use super::error::Error;
use tracing::instrument;

pub mod health;
pub mod initiate_register;
pub mod login;
pub mod logout;
pub mod register;
pub mod verify_email;

pub use health::route as health;
pub use initiate_register::route as initiate_register;
pub use login::route as login;
pub use register::route as register;
pub use verify_email::route as verify_email;

#[allow(dead_code)]
pub type AppResult<T> = Result<T, Error>;

#[instrument]
pub async fn not_found() -> Error {
    Error::NotFound
}
