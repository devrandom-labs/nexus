use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use tracing::instrument;

/// Represents the health status of the application.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Health {
    /// A message describing the health status.
    message: String,
}

/// Health Check Endpoint
///
/// This endpoint performs a basic health check on the application. It verifies that the server is running and responsive.
///
/// **Purpose:**
///
/// * Allows monitoring systems to verify the application's availability.
/// * Provides a simple way to check if the application is functioning correctly.
///
/// **Response:**
///
/// * On success, returns a 200 OK status with a JSON object containing the message "ok.".
/// * The response indicates that the application is running and healthy.
///
/// **Usage:**
///
/// This endpoint should be used by monitoring services or developers to quickly assess the application's status. It does not require any authentication or input parameters.
///
/// **Example Request:**
///
/// ```http
/// GET /health
/// ```
///
/// **Example Response:**
///
/// ```http
/// HTTP/1.1 200 OK
/// Content-Type: application/json
///
/// {
///     "message": "ok."
/// }
/// ```
#[utoipa::path(get,
               path = "/health",
               tag = "General",
               operation_id = "healthCheck",
               responses(
                   (status = OK, body = String, description = "Application is Healthy", content_type = "application/json")
               )
)]
#[instrument(name = "health", target = "auth::api::health")]
pub async fn route() -> (StatusCode, Json<Health>) {
    (
        StatusCode::OK,
        Json(Health {
            message: "ok.".into(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[tokio::test]
    async fn health_gives_ok() {
        let (status, body) = route().await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body.0,
            Health {
                message: "ok.".into()
            }
        );
    }
}
