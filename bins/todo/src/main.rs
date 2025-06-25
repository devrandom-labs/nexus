use nexus_rusqlite::Store;
use rusqlite::Result;
use tracing::{info, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

mod events;

#[instrument]
fn main() -> Result<()> {
    let filter = EnvFilter::from_default_env();
    let console = fmt::layer()
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter);
    tracing_subscriber::registry().with(console).init();
    info!("running migrations..");
    let _ = Store::new()?;
    Ok(())
}
