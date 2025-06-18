use rusqlite::Result;
use tracing::{debug, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

mod store;

#[instrument]
fn main() -> Result<()> {
    let filter = EnvFilter::from_default_env();
    let console = fmt::layer()
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter);
    tracing_subscriber::registry().with(console).init();
    debug!("initializing rusqlite store..");
    let _ = store::Store::new()?;
    Ok(())
}
