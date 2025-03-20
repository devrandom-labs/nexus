use tower_http::trace::TraceLayer;
use utoipa_axum::{router::OpenApiRouter, routes};

pub fn routes() -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(health))
        .layer(TraceLayer::new_for_http())
}

#[utoipa::path(get, path = "/health", responses((status = OK, body = String, description = "Check Application Health")))]
async fn health() -> &'static str {
    "ok."
}
