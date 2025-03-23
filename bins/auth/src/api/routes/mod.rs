pub mod health;
pub mod login;
pub mod register;

pub use health::route as health;
pub use login::route as login;
pub use register::route as register;

#[instrument]
pub async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "Not Found")
}
