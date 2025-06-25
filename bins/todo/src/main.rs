use rusqlite::Result;
use todo::store::Store;
use tracing::{debug, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

#[instrument]
fn main() -> Result<()> {
    let filter = EnvFilter::from_default_env();
    let console = fmt::layer()
        .with_level(true)
        .with_span_events(FmtSpan::CLOSE)
        .with_filter(filter);
    tracing_subscriber::registry().with(console).init();
    debug!("running migrations..");
    let _ = Store::new()?;
    Ok(())
}
