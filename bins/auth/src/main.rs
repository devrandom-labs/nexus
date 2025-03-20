use error::Error;
use pawz::{App, AppConfig, DefaultTracer};
use tracing::instrument;

mod api;
mod error;
mod routes;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), Error> {
    let app_config = AppConfig::build("tixlys");
    App::new(app_config)
        .with_tracer(DefaultTracer)
        .run(routes::routes)
        .await?;
    Ok(())
}
