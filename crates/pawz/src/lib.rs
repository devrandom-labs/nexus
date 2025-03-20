#![allow(dead_code)]
use error::Error;
use std::fmt::{Debug, Display};
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use tokio::net::TcpListener;
use tracing::{debug, error, info, instrument};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_swagger_ui::SwaggerUi;

pub mod config;
pub mod error;
pub mod tracer;

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

#[derive(OpenApi)]
#[openapi()]
struct ApiDoc;

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
pub struct App<'a, T>
where
    T: EnvironmentTracer + Debug,
{
    project: &'a str,
    name: &'a str,
    version: &'a str,
    port: Option<u16>,
    env: Environment,
    tracer: Option<T>,
}

impl<'a, T> App<'a, T>
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
    pub fn new(project: &'a str, name: &'a str, version: &'a str, port: Option<u16>) -> Self {
        App {
            project,
            name,
            version,
            env: Environment::default(),
            port,
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
    pub async fn run(self, routes: fn() -> OpenApiRouter) -> Result<(), Error> {
        debug!("running {}:{} in {}", &self.name, &self.version, &self.env);
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
                match self.port {
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
        let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .merge(routes())
            .split_for_parts();

        let routes = router.merge(SwaggerUi::new("/swagger").url("/api-docs/openapi.json", api));

        debug!(
            "starting {} on {}",
            &self.name,
            listener.local_addr().unwrap()
        );
        info!(
            "--------------------🚀🚀🎆{}:{}@{}🎆🚀🚀--------------------\n",
            &self.project, &self.name, &self.version
        );
        axum::serve(listener, routes)
            .await
            .inspect_err(|err| error!("{:?}", err))?;
        Ok(())
    }
}

// TODO: updated openapi utoipa description, info and some texts after runtime
// TODO: divide stuff into tracing_config, openapi_config, general_config
// TODO: add tests to see if this will work.
// TODO: enable prod, default is dev unlesss specified differently.?
// TODO: add prod tracing setup.

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_development_display() {
        let dev_env = Environment::Development;
        assert_eq!(dev_env.to_string(), "Development");
    }

    #[test]
    fn test_production_display() {
        let prod_env = Environment::Production;
        assert_eq!(prod_env.to_string(), "Production");
    }
}
