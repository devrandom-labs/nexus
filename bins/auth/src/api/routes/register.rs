use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter, Result};
use tracing::instrument;
use utoipa::ToSchema;

/// Represents a request to register a new user.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct RegisterRequest {
    /// The user's email address.
    #[schema(example = "joel@tixlys.com")]
    email: String,
    /// the user's password
    password: String,
}

impl Debug for RegisterRequest {
    /// Formats the `RegisterRequest` for debugging purposes, redacting the password.
    ///
    /// This implementation ensures that the password is not exposed in debug output.
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("RegisterRequest")
            .field("email", &self.email)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl Display for RegisterRequest {
    /// Formats the `RegisterRequest` for display, redacting the password.
    ///
    /// This implementation ensures that the password is not exposed in display output.
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(f, "Email 📧: {}, Password 🔑: [REDACTED]", &self.email)
    }
}

#[utoipa::path(post, path = "/register", tags = ["User Authentication"], operation_id = "registerUser", responses((status = OK, body = String, description = "Register User")))]
#[instrument(name = "register", target = "auth::api::register")]
pub async fn route(Json(_input): Json<RegisterRequest>) -> impl IntoResponse {
    StatusCode::OK
}

// TODO: ensure email is valid email. just text based validation
// TODO: ensure password matches basic password validation

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_request_debug() {
        let request = RegisterRequest {
            email: "test@example.com".to_string(),
            password: "secret_password".to_string(),
        };

        let debug_output = format!("{:?}", request);

        assert!(debug_output.contains("email: \"test@example.com\""));
        assert!(debug_output.contains("password: \"[REDACTED]\""));
        assert!(!debug_output.contains("password: \"secret_password\""));
    }

    #[test]
    fn test_register_request_display() {
        let request = RegisterRequest {
            email: "test@example.com".to_string(),
            password: "secret_password".to_string(),
        };

        let display_output = format!("{}", request);

        assert!(display_output.contains("test@example.com"));
        assert!(display_output.contains("[REDACTED]"));
        assert!(!display_output.contains("secret_password"))
    }
    // TODO: test validation works
}
