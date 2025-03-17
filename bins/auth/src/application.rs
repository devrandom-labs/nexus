#![allow(dead_code)]
use axum::Router;
use std::fmt::Display;
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use thiserror::Error as TError;
use tokio::net::TcpListener;
use tracing::{debug, error, info, instrument};
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{self, format::FmtSpan},
    prelude::*,
};

/// Represents the environment in which the application is running.
#[derive(Debug, PartialEq, Eq)]
pub enum Env {
    /// Development environment.
    Development,
    /// Production environment.
    Production,
}

impl Default for Env {
    fn default() -> Self {
        Self::Development
    }
}

impl Display for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Env::Development => write!(f, "Development"),
            Env::Production => write!(f, "Production"),
        }
    }
}

/// Represents errors that can occur during application execution.
#[derive(Debug, TError)]
pub enum ApplicationError {
    /// Indicates an invalid configuration.
    #[error("Invalid Configuration: {0}")]
    InvalidConfiguration(String),
    /// Represents an I/O error.
    #[error("{0}")]
    IO(#[from] std::io::Error),
}

/// Represents the application configuration.
///
/// # Fields
///
/// * `project`: The project name.
/// * `name`: The application name.
/// * `version`: The application version.
/// * `port`: The port number to listen on (optional). If `None`, a port will be automatically selected from a range.
/// * `env`: The environment in which the application is running.
///
/// # Safety
///
/// The string slices (`&'a str`) must have a `'static` lifetime to ensure they are valid for the entire duration of the application.
///
#[derive(Default, Debug)]
pub struct Application<'a> {
    project: &'a str,
    name: &'a str,
    version: &'a str,
    port: Option<u16>,
    env: Env,
}

impl<'a> Application<'a> {
    /// Creates a new `Application` instance.
    ///
    /// # Arguments
    ///
    /// * `project`: The project name.
    /// * `name`: The application name.
    /// * `version`: The application version.
    /// * `port`: The port number to listen on (optional).
    pub fn new(project: &'a str, name: &'a str, version: &'a str, port: Option<u16>) -> Self {
        Application {
            project,
            name,
            version,
            env: Env::Development,
            port,
        }
    }

    /// Sets up tracing for development environment.
    #[inline]
    fn setup_dev_tracing() {
        let filter = EnvFilter::from_default_env();
        let console = fmt::layer()
            .with_level(true)
            .with_span_events(FmtSpan::CLOSE)
            .with_filter(filter);
        tracing_subscriber::registry().with(console).init();
    }

    /// Attempts to bind a `TcpListener` to the specified port.
    ///
    /// # Arguments
    ///
    /// * `port`: The port number to bind to.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `TcpListener` on success, or an `ApplicationError` on failure.
    #[instrument]
    #[inline]
    async fn get_listener(port: u16) -> Result<TcpListener, ApplicationError> {
        TcpListener::bind((Ipv4Addr::new(0, 0, 0, 0), port))
            .await
            .inspect_err(|err| error!(?err))
            .map_err(|err| err.into())
    }

    /// Attempts to find a free port within the specified range and bind a `TcpListener` to it.
    ///
    /// # Arguments
    ///
    /// * `ports`: The range of ports to try.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `TcpListener` on success, or an `ApplicationError` on failure.
    #[instrument]
    #[inline]
    async fn try_until_success(
        ports: RangeInclusive<u16>,
    ) -> Result<TcpListener, ApplicationError> {
        for port in ports {
            debug!("trying port: {}", port);
            match Self::get_listener(port).await {
                Ok(listener) => return Ok(listener),
                Err(err) => {
                    error!(?err);
                    continue;
                }
            }
        }
        Err(ApplicationError::IO(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "No Ports Available to use between 3000..=3005".to_string(),
        )))
    }

    /// Runs the application.
    ///
    /// # Arguments
    ///
    /// * `routes`: A function that returns an `axum::Router` instance.
    ///
    /// # Returns
    ///
    /// A `Result` indicating success or an `ApplicationError` on failure.
    #[instrument]
    pub async fn run(self, routes: fn() -> Router) -> Result<(), ApplicationError> {
        debug!("running {}:{} in {}", &self.name, &self.version, &self.env);
        match self.env {
            Env::Production => {
                debug!("prod environment is not setup yet, please run it on local machine");
                return Err(ApplicationError::InvalidConfiguration(
                    "Production environment is disabled for now".into(),
                ));
            }
            Env::Development => {
                debug!("setting up {} tracing functionality", &self.env);
                Self::setup_dev_tracing();
                let listener = match self.port {
                    Some(port) => {
                        info!("starting {} on port: {}", &self.name, &port);
                        Self::get_listener(port)
                            .await
                            .inspect_err(|err| error!(?err))?
                    }
                    None => Self::try_until_success(3000..=3005)
                        .await
                        .inspect_err(|err| error!(?err))?,
                };
                debug!("starting {} on {:?}", &self.name, &listener);
                info!(
                    "--------------------🚀🚀🎆{}:{}@{}🎆🚀🚀--------------------\n",
                    &self.project, &self.name, &self.version
                );
                axum::serve(listener, routes())
                    .await
                    .inspect_err(|err| error!("{:?}", err))?;
            }
        }
        Ok(())
    }
}
