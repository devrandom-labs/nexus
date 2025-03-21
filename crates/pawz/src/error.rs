use thiserror::Error as TError;

/// Represents errors that can occur during application execution.
#[derive(Debug, TError)]
pub enum Error {
    /// Indicates an invalid configuration.
    #[error("Invalid Configuration: {0}")]
    InvalidConfiguration(String),
    /// Represents an I/O error.
    #[error("{0}")]
    IO(#[from] std::io::Error),
    #[error("error building config: {0}")]
    Config(String),
}
