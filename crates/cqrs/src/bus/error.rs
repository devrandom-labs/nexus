use std::fmt::Debug;
use thiserror::Error as Err;

#[allow(clippy::enum_variant_names)]
#[derive(Err, Debug)]
pub enum Error {
    #[error("Message send failed")]
    SendingFailed,
    #[error("Handler execution failed")]
    HandlerExecutionFailed,
    #[error("Failed to reply")]
    ReplyFailed,
}
