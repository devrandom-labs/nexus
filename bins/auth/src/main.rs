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
    // state sqlx with sqlite
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

// TODO: FINISH CQRS CRATE
// TODO: FINISH GATEWAY
// TODO: in tixlys 1st user is created automatically or profile?
// TODO: change ways to create an account on tixlys? right now email/password
// TODO: futue keriID
