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
