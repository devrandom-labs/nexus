use error::Error;
use pawz::{App, DefaultTracer};
use tracing::instrument;

mod api;
mod error;
mod routes;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), Error> {
    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");
    App::new(workspace, name, version, Some(3000))
        .with_tracer(DefaultTracer)
        .run(routes::routes)
        .await?;
    Ok(())
}
