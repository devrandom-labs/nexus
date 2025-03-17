use axum::Router;
use std::fmt::Display;
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use thiserror::Error as TError;
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

#[derive(Debug, TError)]
pub enum ApplicationError {
    #[error("Invalid Configuration: {0}")]
    InvalidConfiguration(impl Into<String>),
    #[error("{0}")]
    IO(#[from] std::io::Error),
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
    fn setup_dev_tracing() {
        let filter = EnvFilter::from_default_env();
        let console = fmt::layer()
            .with_level(true)
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(filter);
        tracing_subscriber::registry().with(console).init();
    }

    #[instrument]
    #[inline]
    async fn get_listener(port: u16) -> Result<TcpListener, ApplicationError> {
        TcpListener::bind((Ipv4Addr::new(0, 0, 0, 0), port))
            .await
            .map_err(|err| err.into())
    }

    #[instrument]
    #[inline]
    async fn try_until_success(
        ports: RangeInclusive<u16>,
    ) -> Result<TcpListener, ApplicationError> {
        for port in ports {
            debug!("trying port: {}", port);
            match Self::get_listener(port).await {
                Ok(listener) => return Ok(listener),
                Err(e) => {
                    error!("port: {} in use", port);
                    continue;
                }
            }
        }
        Err(ApplicationError::IO(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            ports.map(|p| format!("{p}, ")).collect(),
        )))
    }

    #[instrument]
    pub async fn run(self, routes: fn() -> Router) -> Result<(), ApplicationError> {
        debug!("running {}:{} in {}", &self.name, &self.version, &self.env);
        match self.env {
            Env::PROD => {
                debug!("prod environment is not setup yet, please run it on local machine");
                return Err(ApplicationError::InvalidConfiguration(
                    "Production environment is disabled",
                ));
            }
            Env::LOCAL | Env::DEV => {
                debug!("setting up {} tracing functionality", &self.env);
                Self::setup_dev_tracing();
                let listener = match self.port {
                    Ok(port) => {
                        info!("starting {} on port: {}", &self.name, &port);
                        Self::get_listener(port).await?
                    }
                    None() => Self::try_until_success(3000..=3005).await?,
                };
                debug!("starting {} on {:?}", &self.name, &listener);
                info!(
                    "--------------------🚀🚀🎆{}:{}@{}🎆🚀🚀--------------------",
                    &self.project, &self.name, &self.version
                );
                axum::serve(listener, app)
                    .await
                    .inspect_err(|err| error!("🚫{:?}🚫", err))?;
            }
        }
        Ok(())
    }
}
