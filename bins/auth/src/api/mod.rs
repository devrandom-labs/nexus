use axum::{
    Json,
    extract::{FromRequest, Request, rejection::JsonRejection},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Serialize, de::DeserializeOwned};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use validator::Validate;

mod error;
mod routes;
mod validations;

pub fn router() -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/logout", post(routes::logout))
        .route("/register", post(routes::register))
        .route("/login", post(routes::login))
        .route("/initiate-register", post(routes::initiate_register))
        .route("/verify-email", post(routes::verify_email))
        .route("/health", get(routes::health))
        .layer(TraceLayer::new_for_http())
        .fallback(routes::not_found)
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Auth", description = "Tixlys Auth Service",),
    paths(
        routes::health::route,
        routes::register::route,
        routes::login::route,
        routes::initiate_register::route,
        routes::verify_email::route,
        routes::logout::route,
    )
)]
pub struct ApiDoc;

// TODO: improve open api documentation
// TODO: add security add on for login route
// TODO: test all apis

#[derive(Debug, Clone, Copy, Default)]
pub struct ValidJson<T>(T);

impl<T> IntoResponse for ValidJson<T>
where
    T: Serialize,
    Json<T>: IntoResponse,
{
    fn into_response(self) -> Response {
        Json(self.0).into_response()
    }
}

impl<S, T> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
{
    type Rejection = error::Error;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        value.validate()?;
        Ok(ValidJson(value))
    }
}

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
