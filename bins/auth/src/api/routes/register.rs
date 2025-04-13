#![allow(dead_code)]
use super::AppResult;
use axum::http::StatusCode;
use pawz::jsend::Body;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter, Result};
use tracing::instrument;
use utoipa::ToSchema;
use validator::Validate;

use crate::api::AppJson;

/// Represents a request to register a new user.
#[derive(Deserialize, Validate, ToSchema)]
pub struct RegisterRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    #[validate(email(message = "Invalid email format"))]
    email: String,
    /// the user's password
    #[schema(example = "P@ssw0rd123!")]
    #[validate(length(min = 8, message = "password should have at least 8 characters"))]
    password: String,
    agree_to_terms_and_conditions: bool,
}

impl Debug for RegisterRequest {
    /// Formats the `RegisterRequest` for debugging purposes, redacting the password.
    ///
    /// This implementation ensures that the password is not exposed in debug output.
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("RegisterRequest")
            .field("email", &self.email)
            .field("password", &"[REDACTED]")
            .field(
                "agree_to_terms_and_conditions",
                &self.agree_to_terms_and_conditions,
            )
            .finish()
    }
}

impl Display for RegisterRequest {
    /// Formats the `RegisterRequest` for display, redacting the password.
    ///
    /// This implementation ensures that the password is not exposed in display output.
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(
            f,
            "Email 📧: {}, Password 🔑: [REDACTED], Agree to terms and conditions?: {}",
            &self.email, &self.agree_to_terms_and_conditions
        )
    }
}

#[derive(Serialize, ToSchema)]
pub struct RegisterResponse {
    message: String,
}

#[utoipa::path(post,
               path = "/register",
               tags = ["Onboarding"],
               operation_id = "registerUser",
               request_body = RegisterRequest,
               responses(
                   (status = OK, body = Body<RegisterResponse>, description = "User registered successfully")
               ))]
#[instrument(name = "register", target = "api::auth::register")]
pub async fn route(
    AppJson(request): AppJson<RegisterRequest>,
) -> AppResult<(StatusCode, AppJson<Body<RegisterResponse>>)> {
    Ok((
        StatusCode::ACCEPTED,
        AppJson(Body::success(RegisterResponse {
            message: "you are registered!!".to_string(),
        })),
    ))
}
// TODO: plug in password validation function here
// TODO: test validation failure response.
// TODO: add error responses

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_request_debug() {
        let request = RegisterRequest {
            email: "test@example.com".to_string(),
            password: "secret_password".to_string(),
            agree_to_terms_and_conditions: false,
        };

        let debug_output = format!("{:?}", request);

        assert!(debug_output.contains("email: \"test@example.com\""));
        assert!(debug_output.contains("password: \"[REDACTED]\""));
        assert!(!debug_output.contains("password: \"secret_password\""));
        assert!(debug_output.contains("agree_to_terms_and_conditions"));
    }

    #[test]
    fn test_register_request_display() {
        let request = RegisterRequest {
            email: "test@example.com".to_string(),
            password: "secret_password".to_string(),
            agree_to_terms_and_conditions: false,
        };

        let display_output = format!("{}", request);

        assert!(display_output.contains("test@example.com"));
        assert!(display_output.contains("[REDACTED]"));
        assert!(!display_output.contains("secret_password"));
        assert!(display_output.contains("Agree to terms and conditions?"));
    }
}
