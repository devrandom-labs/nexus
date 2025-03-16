use axum::{Router, routing::get};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::{error, info, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

mod application;
mod error;

#[instrument]
#[tokio::main]
async fn main() -> Result<(), error::Error> {
    let filter = EnvFilter::from_default_env();
    let console = fmt::layer()
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter);

    tracing_subscriber::registry().with(console).init();

    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");

    info!("🚀🚀🎆{}:{}@{}🎆🚀🚀", workspace, name, version);

    Application::run(routes())
}

pub async fn health() -> &'static str {
    "ok."
}

struct Application;

impl Application {
    #[instrument]
    pub async fn run(routes: Router) -> Result<(), Error> {
        let listener = TcpListener::bind("0.0.0.0:3000")
            .await
            .inspect_err(|err| error!(?err))?;

        axum::serve(listener, app)
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
