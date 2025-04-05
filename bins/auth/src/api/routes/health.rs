use super::AppResult;
use axum::http::StatusCode;
use pawz::jsend::Body;
use serde::Serialize;
use tracing::instrument;

/// Represents the health status of the application.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
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
pub async fn route() -> AppResult<HealthResponse> {
    Ok((
        StatusCode::OK,
        Body::success(HealthResponse {
            message: "ok.".to_owned(),
        }),
    ))
}

#[cfg(test)]
mod test {
    use super::route;
    use crate::api::test::get_response_body;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use serde_json::json;

    #[tokio::test]
    async fn health_gives_ok() {
        let response = route().await;
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = get_response_body(response).await;
        assert_eq!(
            body,
            json!({"data": {"message": "ok."}, "status": "success"})
        );
    }
}
