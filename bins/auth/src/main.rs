use error::Error;
use pawz::{App, DefaultTracer};
use tower_http::trace::TraceLayer;
use tracing::instrument;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};
use utoipa_swagger_ui::SwaggerUi;

mod error;

#[derive(OpenApi)]
#[openapi(info(description = "Tixlys Auth Service"))]
struct ApiDoc;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), Error> {
    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .merge(routes())
        .split_for_parts();

    let routes = router.merge(SwaggerUi::new("/swagger").url("/api-docs/openapi.json", api));

    App::new(workspace, name, version, Some(3000))
        .with_tracer(DefaultTracer)
        .run(routes)
        .await
        .map_err(|err| err.into())
}

#[utoipa::path(get, path = "/health", responses((status = OK, body = String, description = "Check Application Health")))]
pub async fn health() -> &'static str {
    "ok."
}

pub fn routes() -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(health))
        .layer(TraceLayer::new_for_http())
}

// TODO: get openapi/swagger-ui to work
// TODO: create v1/auth/register
// TODO: create v1/auth/login
// TODO: create v1/auth/logout
// TODO: create v1/auth/refresh
// TODO: create v1/auth/veriy-email
