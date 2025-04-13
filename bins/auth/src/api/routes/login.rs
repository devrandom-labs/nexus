#![allow(dead_code)]
use crate::api::AppJson;
use axum::http::StatusCode;
use pawz::jsend::Body;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display};
use tracing::instrument;
use utoipa::ToSchema;
use validator::Validate;

use super::AppResult;
use crate::api::validations::validate_password;

#[derive(Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    #[validate(email(message = "Invalid email format"))]
    email: String,
    #[validate(custom(function = "validate_password"))]
    password: String,
}

impl Debug for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginRequest")
            .field("email", &self.email)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl Display for LoginRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Email 📧: {}, Password 🔑: [REDACTED]", &self.email)
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    id: String,
    email: String,
}

#[utoipa::path(post,
               path = "/login",
               tags = ["User Authentication"],
               operation_id = "loginUser",
               request_body = LoginRequest,
               responses(
                   (status = OK, body = Body<LoginResponse>, description = "User has logged in successfully")
               ))]
#[instrument(name = "login", target = "api::auth::login")]
pub async fn route(
    AppJson(request): AppJson<LoginRequest>,
) -> AppResult<(StatusCode, AppJson<Body<LoginResponse>>)> {
    Ok((
        StatusCode::ACCEPTED,
        AppJson(Body::success(LoginResponse {
            id: "some id".to_string(),
            email: "some_email".to_string(),
        })),
    ))
}
