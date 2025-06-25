use thiserror::Error as TError;

#[derive(Debug, TError)]
pub enum Error {
    #[error("Connection Error")]
    Connection,

    #[error("Configuration Error")]
    Configuration,
}
