use std::error::Error as StdError;
use std::fmt::Debug;
use thiserror::Error as ThisError;
use tower::BoxError;

/// Marker trait for requests handled by the Mediator.
/// Defines the expected Response and Error types for the handler Service.
pub trait Request: Debug + Send + 'static {
    /// The type of the successful response returned by the handler.
    type Response: Debug + Send + 'static;
    /// The type of error returned by the handler if processing fails.
    type Error: StdError + Send + Sync + 'static;
}

type ErasedResult = Result<Box<dyn Any + Send>, BoxError>;

/// Errors thatcan occur during Mediator operation.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Handler not found for request type: {0}")]
    HandlerNotFound(String),
    #[error("An unexpected error occurred: {0}")]
    InternalError(String),
}

#[derive(Clone, Default)]
pub struct Mediator {}

#[derive(Default)]
pub struct MediatorBuilder {}

impl MediatorBuilder {
    /// Creates a new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }
    /// Consumes the builder and returns the configured Mediator.
    pub fn build(self) -> Mediator {
        Mediator::default()
    }
}

impl Mediator {
    /// Creates a builder to construct a Mediator instance.
    pub fn builder() -> MediatorBuilder {
        MediatorBuilder::new()
    }

    // TODO: register method
    // TODO: any async method that requires the signature to be Fn(Request) -> Future<Output = Result<Response, Error>;
}

#[cfg(test)]
mod test {

    use crate::Mediator;

    mod setup {
        #![allow(dead_code)]
        use super::super::Request;
        use std::time::Duration;
        use thiserror::Error;
        use tokio::time::sleep;

        #[derive(Debug)]
        pub struct GetPing {
            pub message: String,
        }

        #[derive(Debug)]
        pub struct GetPingResponse {
            pub reply: String,
        }

        #[derive(Debug, Error, PartialEq)]
        pub enum GetPingError {
            #[error("Invalid ping message: {0}")]
            InvalidMessage(String),
            #[error("Simulated internal failure")]
            InternalFailure,
        }

        impl Request for GetPing {
            type Response = GetPingResponse;
            type Error = GetPingError;
        }

        pub async fn handle_get_ping(req: GetPing) -> Result<GetPingResponse, GetPingError> {
            sleep(Duration::from_millis(10)).await;
            if req.message.is_empty() {
                return Err(GetPingError::InvalidMessage("empty".to_string()));
            } else if req.message == "failme" {
                return Err(GetPingError::InternalFailure);
            }
            Ok(GetPingResponse {
                reply: format!("pong: {}", req.message),
            })
        }
    }

    use setup::{GetPing, GetPingError, handle_get_ping};

    #[test]
    fn mediator_builder_returns_builder() {
        let _builder = Mediator::builder();
    }

    #[tokio::test]
    async fn test_successful_ping_request() {
        let req = GetPing {
            message: "test".to_string(),
        };
        let reply = handle_get_ping(req).await;
        assert!(reply.is_ok());
        let message = reply.unwrap().reply;
        assert_eq!(message, "pong: test".to_string());
    }

    #[tokio::test]
    async fn test_empty_ping_request() {
        let req = GetPing {
            message: "".to_string(),
        };
        let reply = handle_get_ping(req).await;
        assert!(reply.is_err());
        let message = reply.unwrap_err();
        assert_eq!(message, GetPingError::InvalidMessage("empty".to_string()));
    }

    #[tokio::test]
    async fn test_failed_ping_request() {
        let req = GetPing {
            message: "failme".to_string(),
        };
        let reply = handle_get_ping(req).await;
        assert!(reply.is_err());
        let message = reply.unwrap_err();
        assert_eq!(message, GetPingError::InternalFailure);
    }
}
