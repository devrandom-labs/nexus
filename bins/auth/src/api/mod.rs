use axum::{
    Json,
    extract::FromRequest,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

mod error;
mod routes;

#[derive(FromRequest)]
#[from_request(via(Json), rejection(error::Error))]
pub struct AppJson<T>(T);

impl<T> IntoResponse for AppJson<T>
where
    Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        Json(self.0).into_response()
    }
}

pub fn router() -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/register", post(routes::register))
        .route("/login", post(routes::login))
        .route("/health", get(routes::health))
        .layer(TraceLayer::new_for_http())
        .fallback(routes::not_found)
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Auth", description = "Tixlys Auth Service",),
    paths(routes::health::route, routes::register::route, routes::login::route)
)]
pub struct ApiDoc;

// TODO: improve open api documentation
// TODO: add security add on for login route
// TODO: test all apis

#[cfg(test)]
mod test {
    use super::router;
    use axum::{
        Router,
        body::Body,
        http::{Request, Response, StatusCode},
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    pub fn get_router() -> Router {
        router().split_for_parts().0
    }

    pub async fn get_response_body(response: Response<Body>) -> Value {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap()
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
        let body = get_response_body(response).await;
        assert_eq!(
            body,
            json!({"status": "success", "data": {"message": "ok."}})
        )
    }

    #[tokio::test]
    async fn test_fallback() {
        let router = get_router();
        let non_existing_route = Request::builder()
            .uri("/fallback")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(non_existing_route).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, String::from_utf8(body.to_vec()).unwrap());
    }
}
