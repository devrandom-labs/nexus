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
/// * On success, returns a 200 OK status with the string "ok." in the response body.
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
/// Content-Type: text/plain
///
/// ok.
#[utoipa::path(get,
               path = "/health",
               tag = "Infra",
               operation_id = "healthCheck",
               responses(
                   (status = OK, body = String, description = "Application is healthy")
               )
)]
pub async fn route() -> &'static str {
    "ok."
}

#[cfg(test)]
mod tests {}

// TODO: get tracing in route
// TODO: add debug trace
// TODO: have proper statuscode in return
// TODO: test out health failure route
