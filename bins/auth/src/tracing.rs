use crate::application::Env;
use thiserror::Error as TError;
use tracing::{debug, error, info, instrument};
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
    fn setup(self, env: Env) -> Result<(), TracingError>;
}

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
    fn setup(self, env: Env) -> Result<(), TracingError> {
        debug!("setting up tracing for env: {}", env);
        match env {
            Env::Development => {
                Self::setup_dev_tracing();
                Ok(())
            }
            _ => Err(TracingError::UnsupportedEnvironment(env.to_string())),
        }
    }
}
