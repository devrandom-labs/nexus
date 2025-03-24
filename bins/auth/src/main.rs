use error::Error;
use pawz::{App, AppConfig, DefaultTracer};
use tracing::instrument;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_swagger_ui::SwaggerUi;

mod adapters;
mod api;
mod domain;
mod error;
mod services;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), Error> {
    let (router, api) = OpenApiRouter::with_openapi(api::ApiDoc::openapi())
        .merge(api::router())
        .split_for_parts();

    let app = router.merge(SwaggerUi::new("/swagger").url("/api-docs/openapi.json", api));

    let app_config = AppConfig::build("tixlys");

    App::new(app_config)
        .with_tracer(DefaultTracer)
        .run(app)
        .await?;

    Ok(())
}
