use axum::{Router, routing::get};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{info, instrument};

mod application;
mod error;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), error::Error> {
    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");
    info!("🚀🚀🎆{}:{}@{}🎆🚀🚀", workspace, name, version);

    Application::run(routes)
}

pub async fn health() -> &'static str {
    "ok."
}

pub fn routes() -> Router {
    Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
}
