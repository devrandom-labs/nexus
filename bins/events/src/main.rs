use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tracing::{error, info, instrument};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter, Layer,
};

pub mod domain;

#[instrument]
#[tokio::main]
async fn main() {
    let filter = EnvFilter::from_default_env();
    let console = fmt::layer()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter);
    tracing_subscriber::registry().with(console).init();

    let workspace = "tixlys";
    let name = env!("CARGO_BIN_NAME");
    let version = env!("CARGO_PKG_VERSION");
    info!("🚀🚀🎆{}:{}@{}🎆🚀🚀", workspace, name, version);

    let app = Router::new().route("/", get(|| async { "Hello, World!" }));
    let listener = TcpListener::bind("0.0.0.0:3000")
        .await
        .inspect_err(|err| error!(?err))
        .unwrap();
    axum::serve(listener, app)
        .await
        .inspect_err(|err| error!("🚫{:?}🚫", err))
        .unwrap();
}

// Phase 1
// TODO: enable tracing for axum
// TODO: build a health check endpoint
