use crate::Environment;
use thiserror::Error as TError;
use tracing::{debug, error, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

#[derive(Debug, TError)]
pub enum TracingError {
    #[error("Environment is not supported: {0}")]
    UnsupportedEnvironment(String),
}

pub trait EnvironmentTracer {
    fn setup(&self, environment: &Environment);
}

#[derive(Debug)]
pub struct Tracer;

impl Tracer {
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

impl EnvironmentTracer for Tracer {
    #[instrument]
    #[inline]
    fn setup(&self, environment: &Environment) {
        debug!("setting up tracing for env: {}", environment);
        match environment {
            Environment::Development => {
                Self::setup_dev_tracing();
            }
            _ => {}
        }
    }
}

// TODO: tracer should have  tracer config, for prod.
