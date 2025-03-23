use axum::{Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter, Result};
use tracing::instrument;
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
pub struct RegisterRequest {
    #[schema(example = "joel@tixlys.com")]
    email: String,
    password: String,
}

impl Debug for RegisterRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("RegisterRequest")
            .field("email", &self.email)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl Display for RegisterRequest {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "Email 📧: {}, Password 🔑: [REDACTED]", &self.email)
    }
}

#[utoipa::path(post, path = "/register", tags = ["User Authentication"], operation_id = "registerUser", responses((status = OK, body = String, description = "Register User")))]
#[instrument(name = "register", target = "auth::api::register")]
pub async fn route(Json(_input): Json<RegisterRequest>) -> impl IntoResponse {
    "register"
}

// TODO: ensure email is valid email. just text based validation
// TODO: ensure password matches basic password validation
