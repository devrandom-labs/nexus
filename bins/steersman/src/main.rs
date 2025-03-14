use tracing::{info, instrument};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter, Layer,
};

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
}

// Phase 1
// TODO: build this docker image and run
// TODO: supply config.yaml file and check if you can see
// TODO: get config from command line
// TODO: set the config to pingora server successfully.
// TODO: set tracing in pingora server
// TODO: clean up the architectrue and maybe impl some design patterns for config or clap.
// TODO: write tests to ensure config to server capabilities and failure
// TODO: ensure tracing and metrics
// TODO: have prometheus or jaeger (depending) and get open telemetry meterics to it.
