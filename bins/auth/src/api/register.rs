use axum::{Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct RegisterRequest {
    #[schema(example = "joel@tixlys.com")]
    email: String,
    password: String,
}

#[utoipa::path(post, path = "/register", tags = ["User Authentication"], operation_id = "registerUser", responses((status = OK, body = String, description = "Register User")))]
#[instrument(name = "register", target = "auth::api::register")]
pub async fn route(Json(_input): Json<RegisterRequest>) -> impl IntoResponse {
    "register"
}

// TODO: ensure email is valid email. just text based validation
// TODO: ensure password matches basic password validation
// TODO: password should not be shown in logs , HOW??
