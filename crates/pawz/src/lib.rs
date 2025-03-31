#![allow(dead_code)]
use axum::Router;
use error::Error;
use std::fmt::{Debug, Display};
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use tokio::net::TcpListener;
use tracing::{debug, error, info, instrument};

pub mod body;
pub mod config;
pub mod error;
pub mod tracer;

pub use config::AppConfig;
pub use tracer::DefaultTracer;
pub use tracer::EnvironmentTracer;

/// Represents the environment in which the application is running.
#[derive(Debug, PartialEq, Eq)]
pub enum Environment {
    Development,
    Production,
}

impl Default for Environment {
    fn default() -> Self {
        Self::Development
    }
}

impl Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Environment::Development => write!(f, "Development"),
            Environment::Production => write!(f, "Production"),
        }
    }
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
#[derive(Debug)]
pub struct App<T>
where
    T: EnvironmentTracer + Debug,
{
    app_config: AppConfig,
    env: Environment,
    tracer: Option<T>,
}

impl<T> App<T>
where
    T: EnvironmentTracer + Debug,
{
    /// Creates a new `App` instance.
    ///
    /// # Arguments
    ///
    /// * `project`: The project name.
    /// * `name`: The application name.
    /// * `version`: The application version.
    /// * `port`: The port number to listen on (optional).
    pub fn new(app_config: AppConfig) -> Self {
        App {
            app_config,
            env: Environment::default(),
            tracer: None,
        }
    }

    pub fn with_tracer(mut self, tracer: T) -> Self {
        self.tracer = Some(tracer);
        self
    }

    /// Attempts to bind a `TcpListener` to the specified port.
    ///
    /// # Arguments
    ///
    /// * `port`: The port number to bind to.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `TcpListener` on success, or an `Error` on failure.
    #[instrument]
    #[inline]
    async fn get_listener(port: u16) -> Result<TcpListener, Error> {
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
    /// A `Result` containing the `TcpListener` on success, or an `Error` on failure.
    #[instrument]
    #[inline]
    async fn try_until_success(ports: RangeInclusive<u16>) -> Result<TcpListener, Error> {
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
        Err(Error::IO(std::io::Error::new(
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
    /// A `Result` indicating success or an `Error` on failure.
    #[instrument]
    pub async fn run(self, app: Router) -> Result<(), Error> {
        let app_config = &self.app_config;
        debug!(
            "running {}:{} in {}",
            app_config.name, app_config.version, &self.env
        );
        if let Some(tracer) = &self.tracer {
            tracer.setup(&self.env);
        }
        let listener = match self.env {
            Environment::Production => {
                debug!("prod environment is not setup yet, please run it on local machine");
                return Err(Error::InvalidConfiguration(
                    "Production environment is disabled for now".into(),
                ));
            }
            Environment::Development => {
                debug!("setting up {} tracing functionality", &self.env);
                match app_config.port {
                    Some(port) => Self::try_until_success(port..=(port + 3))
                        .await
                        .inspect_err(|err| error!(?err))?,
                    None => Self::try_until_success(3000..=3005)
                        .await
                        .inspect_err(|err| error!(?err))?,
                }
            }
        };

        debug!("creating open api routes");

        debug!(
            "starting {} on {}",
            app_config.name,
            listener.local_addr().unwrap()
        );
        info!(
            "--------------------🚀🚀🎆{}:{}@{}🎆🚀🚀--------------------\n",
            app_config.project, app_config.name, app_config.version
        );
        axum::serve(listener, app)
            .await
            .inspect_err(|err| error!("{:?}", err))?;
        Ok(())
    }
}

// TODO: update the docs here
// TODO: add tests to see if this will work.
// TODO: enable prod, default is dev unlesss specified differently.?
// TODO: add prod tracing setup.
