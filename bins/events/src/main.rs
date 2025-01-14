use tracing::{info, instrument};
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    prelude::*,
    EnvFilter, Layer,
};
pub mod domain;
pub mod store;

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

    let event = domain::event::Event::default();
    info!(?event);

    // TODO: create a command handler.
}
