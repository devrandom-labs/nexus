use crate::Environment;
use tracing::{debug, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

pub trait EnvironmentTracer {
    fn setup(&self, environment: &Environment);
}

#[derive(Debug)]
pub struct DefaultTracer;

impl DefaultTracer {
    #[inline]
    fn setup_dev_tracing() {
        let filter = EnvFilter::from_default_env();
        let console = fmt::layer()
            .with_level(true)
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(filter);
        tracing_subscriber::registry().with(console).init();
    }
}

impl EnvironmentTracer for DefaultTracer {
    #[instrument]
    #[inline]
    fn setup(&self, environment: &Environment) {
        debug!("setting up tracing for env: {}", environment);
        if environment == &Environment::Development {
            Self::setup_dev_tracing();
        }
    }
}

// TODO: tracer should have  tracer config, for prod.
