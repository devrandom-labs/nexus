use crate::error::Error;
use axum::Router;
use std::fmt::Display;
use std::net::Ipv4Addr;
use thiserror::Error as Err;
use tokio::net::TcpListener;
use tracing::{error, info, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

#[derive(Debug, PartialEq, Eq)]
pub enum Env {
    DEV,
    PROD,
    LOCAL,
}

impl Default for Env {
    fn default() -> Self {
        Self::LOCAL
    }
}

impl Display for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Env::DEV => write!(f, "DEV Environment"),
            Env::LOCAL => write!(f, "LOCAL Environment"),
            Env::PROD => write!(f, "PROD Environment"),
        }
    }
}

#[derive(Debug, Err)]
pub enum ApplicationError {
    #[error("Invalid Configuration: {0}")]
    InvalidConfiguration(Into<String>),
}

// FIXME: need this to be static string that would never be removed.
#[derive(Default, Debug)]
pub struct Application<'a> {
    project: &'a str,
    name: &'a str,
    version: &'a str,
    port: Option<String>,
    env: Env,
}

impl Application<'a> {
    pub fn new(project: &'a str, name: &'a str, version: &'a str, port: Option<String>) -> Self {
        Application {
            project,
            name,
            version,
            env,
            port,
        }
    }

    #[inline]
    pub fn setup_dev_tracing() {
        let filter = EnvFilter::from_default_env();
        let console = fmt::layer()
            .with_level(true)
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(filter);
        tracing_subscriber::registry().with(console).init();
    }

    #[instrument]
    #[inlin]
    pub async fn get_listener(port: u16) -> Result<TcpListener, ApplicationError> {
        TcpListener::bind((Ipv4Addr::new(0, 0, 0, 0), port)).await
    }

    #[instrument]
    pub async fn run(self, routes: fn() -> Router) -> Result<(), ApplicationError> {
        debug!("running {}:{} in {}", &self.name, &self.version, &self.env);
        match self.env {
            Env::PROD => {
                debug!("prod environment is not setup yet, please run it on local machine");
                Err(ApplicationError::InvalidConfiguration(
                    "Production environment is disabled",
                ))
            }
            Env::LOCAL | Env::DEV => {
                debug!("setting up {} tracing functionality", &self.env);
                Self::setup_dev_tracing();
                let listener = match self.port {
                    Ok(port) => {
                        info!("starting {} on port: {}", &self.name, &port);
                    }
                    None() => {
                        // run a loop that starts from 3000
                        // til it finds a listener
                    }
                };

                // TODO: start axum by passing router inside.
            }
        }
        info!(
            "🚀🚀🎆{}:{}@{}🎆🚀🚀",
            &self.project, &self.name, &self.version
        );
        Ok(())
    }
}
