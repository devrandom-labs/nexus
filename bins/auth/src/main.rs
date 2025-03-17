use axum::{routing::get, Router};
use error::Error;
use pawz::{App, DefaultTracer};
use tower_http::trace::TraceLayer;
use tracing::instrument;

mod error;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), Error> {
    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");
    App::new(workspace, name, version, Some(3000))
        .with_tracer(DefaultTracer)
        .run(routes)
        .await
        .map_err(|err| err.into())
}

pub async fn health() -> &'static str {
    "ok."
}

pub fn routes() -> Router {
    Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
}
