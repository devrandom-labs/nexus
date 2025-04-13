use super::AppResult;
use axum::http::StatusCode;
use tracing::instrument;

#[utoipa::path(post,
               path = "/logout",
               tags = ["User Authentication"],
               operation_id = "logout",
               responses(
                   (status = OK, description = "Successfully logout and session termination", content_type = "application/json")
               )
)]
#[instrument(name = "logout", target = "api::auth::logout")]
pub async fn route() -> AppResult<StatusCode> {
    Ok(StatusCode::OK)
}
