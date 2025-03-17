use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{error, info, instrument};

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

struct Application;

impl Application {
    #[instrument]
    pub async fn run(routes: fn() -> Router) -> Result<(), Error> {
        let listener = TcpListener::bind("0.0.0.0:3000")
            .await
            .inspect_err(|err| error!(?err))?;

        axum::serve(listener, routes())
            .await
            .inspect_err(|err| error!("🚫{:?}🚫", err))?;

        Ok(())
    }
}

pub fn routes() -> Router {
    Router::new()
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
}
