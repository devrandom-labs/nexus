use axum::routing::{get, post};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

mod health;
mod login;
mod register;

pub fn router() -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/health", get(health::route))
        .route("/register", post(register::route))
        .route("/login", post(login::route))
        .layer(TraceLayer::new_for_http())
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Auth", description = "Tixlys Auth Service",),
    paths(health::route, register::route, login::route)
)]
pub struct ApiDoc;

// TODO: improve open api documentation
// TODO: add security add on for login route
// TODO: test all apis
//
#[cfg(test)]
mod tests {
    use super::router;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    pub fn get_router() -> Router {
        router().split_for_parts().0
    }

    #[tokio::test]
    async fn health_check() {
        let router = get_router();
        let health_request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(health_request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body, json!({"message": "ok."}))
    }
}
