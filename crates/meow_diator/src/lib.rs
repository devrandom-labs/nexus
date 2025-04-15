#![allow(dead_code)]
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    error::Error as StdError,
    fmt::Debug,
    future::Future,
    pin::Pin,
    sync::Arc,
};
use thiserror::Error as ThisError;
use tower::{BoxError, Service, ServiceExt, service_fn};

/// Marker trait for requests handled by the Mediator.
/// Defines the expected Response and Error types for the handler Service.
pub trait Request: Debug + Send + 'static {
    /// The type of the successful response returned by the handler.
    type Response: Debug + Send + 'static;
    /// The type of error returned by the handler if processing fails.
    type Error: StdError + Send + Sync + 'static;
}

//-------------------- Erased Type Aliases --------------------//
type ErasedResult = Result<Box<dyn Any + Send>, BoxError>;
type BoxFuture = Pin<Box<dyn Future<Output = ErasedResult> + Send>>;
type AnyRequestHandler = Box<dyn Fn(Box<dyn Any + Send>) -> BoxFuture + Send + Sync>;

/// Errors thatcan occur during Mediator operation.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Handler not found for request type: {0}")]
    HandlerNotFound(String),
    #[error("An unexpected error occurred: {0}")]
    InternalError(String),
    #[error("Handler execution failed")]
    HandlerError(#[from] BoxError),
    #[error("Response type mismatch (downcast failed)")]
    ResponseDowncastError,
    #[error("Request type mismatch (downcast failed)")]
    RequestDowncastError,
}

#[derive(Clone, Default)]
pub struct Mediator {
    handlers: Arc<HashMap<TypeId, AnyRequestHandler>>,
}

#[derive(Default)]
pub struct MediatorBuilder {
    handlers: HashMap<TypeId, AnyRequestHandler>,
}

impl MediatorBuilder {
    /// Creates a new, empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<R, Fut, F>(&mut self, handler: F) -> &mut Self
    where
        R: Request + Any + Sync,
        F: Fn(R) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future<Output = Result<R::Response, R::Error>> + Send + 'static,
    {
        let type_id = TypeId::of::<R>();
        let service = service_fn(handler);
        let invoker: AnyRequestHandler = Box::new(move |boxed_request: Box<dyn Any + Send>| {
            let mut service_clone = service.clone();

            Box::pin(async move {
                let request = match boxed_request.downcast::<R>() {
                    Ok(req) => *req,
                    Err(_) => {
                        let err = BoxError::from(Error::RequestDowncastError);
                        return Err(err);
                    }
                };

                match service_clone.ready().await {
                    Ok(read_service) => match read_service.call(request).await {
                        Ok(response) => {
                            let response_any: Box<dyn Any + Send> = Box::new(response);
                            Ok(response_any)
                        }
                        Err(e) => Err(BoxError::from(e)),
                    },
                    Err(e) => Err(BoxError::from(e)),
                }
            })
        });

        self.handlers.insert(type_id, invoker);
        self
    }

    /// Consumes the builder and returns the configured Mediator.
    pub fn build(self) -> Mediator {
        Mediator {
            handlers: Arc::new(self.handlers),
        }
    }
}

impl Mediator {
    /// Creates a builder to construct a Mediator instance.
    pub fn builder() -> MediatorBuilder {
        MediatorBuilder::new()
    }
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

    #[test]
    fn able_to_register_async_handlers() {
        let mut mediator_builder = Mediator::builder();
        mediator_builder.register(handle_get_ping);
        let _mediator = mediator_builder.build();
    }
}
