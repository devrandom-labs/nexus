use axum::routing::{get, post};
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

#[utoipa::path(get, path = "/health", responses((status = OK, body = String, description = "Check Application Health")))]
async fn health() -> &'static str {
    "ok."
}

#[utoipa::path(post, path = "/register", responses((status = OK, body = String, description = "Register User")))]
async fn register() -> &'static str {
    "register"
}

#[utoipa::path(post, path = "/login", responses((status = OK, body = String, description = "Login User")))]
async fn login() -> &'static str {
    "login"
}

pub fn router() -> OpenApiRouter {
    OpenApiRouter::new()
        .route("/health", get(health))
        .route("/register", post(register))
        .route("/login", post(login))
        .layer(TraceLayer::new_for_http())
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Auth", description = "Tixlys Auth Service",),
    paths(health, register, login)
)]
pub struct ApiDoc;
