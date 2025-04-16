#![allow(dead_code)]
use std::{
    any::{Any, TypeId, type_name},
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
    #[error("Handler not found for request type: {request_type}")]
    HandlerNotFound { request_type: String },

    #[error("Handler execution failed for request type: {request_type}")]
    HandlerExecutionFailed {
        request_type: String,
        #[source]
        source: BoxError,
    },

    #[error("Response type mismatch for request type: {request_type} (downcast failed)")]
    ResponseDowncastFailed { request_type: String },

    #[error("An unexpected error occurred: {0}")]
    InternalError(String),
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
        let type_name = type_name::<R>();
        let service = service_fn(handler);
        let invoker: AnyRequestHandler = Box::new(move |boxed_request: Box<dyn Any + Send>| {
            let mut service_clone = service.clone();
            Box::pin(async move {
                let request = match boxed_request.downcast::<R>() {
                    Ok(req) => *req,
                    Err(_) => {
                        let err_msg = format!(
                            "Internal Mediator Error: Request Downcast Failed for type {}",
                            type_name
                        );
                        let err: BoxError = err_msg.into();
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

    pub async fn send<R>(&self, request: R) -> Result<R::Response, Error>
    where
        R: Request + Any + Send,
    {
        let type_id = TypeId::of::<R>();
        let request_type = type_name::<R>().to_string();
        let handler = self
            .handlers
            .get(&type_id)
            .ok_or_else(|| Error::HandlerNotFound {
                request_type: request_type.clone(),
            })?;

        let boxed_request: Box<dyn Any + Send> = Box::new(request);
        let result_any: Box<dyn Any + Send> =
            handler(boxed_request)
                .await
                .map_err(|source| Error::HandlerExecutionFailed {
                    request_type: request_type.clone(),
                    source,
                })?;

        match result_any.downcast::<R::Response>() {
            Ok(boxed_response) => Ok(*boxed_response),
            Err(_) => Err(Error::ResponseDowncastFailed {
                request_type: request_type.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod test {

    use crate::{Error, Mediator};
    use setup::{GetPing, GetPingError, NoHandlerRequest, handle_get_ping};
    use std::any::type_name;

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

        //-------------------- no handler requests --------------------//

        #[derive(Debug)]
        pub struct NoHandlerRequest {}

        #[derive(Debug)]
        pub struct NoHandlerResponse {}

        impl Request for NoHandlerRequest {
            type Response = NoHandlerResponse;
            type Error = GetPingError;
        }
    }

    #[tokio::test]
    async fn successful_request_handler_execution() {
        let mut mediator_builder = Mediator::builder();
        mediator_builder.register(handle_get_ping);
        let mediator = mediator_builder.build();

        let req = GetPing {
            message: "test".to_string(),
        };
        let reply = mediator.send(req).await;
        assert!(reply.is_ok());
        let message = reply.unwrap().reply;
        assert_eq!(message, "pong: test".to_string());
    }

    #[tokio::test]
    async fn business_logic_error_empty() {
        let mut mediator_builder = Mediator::builder();
        mediator_builder.register(handle_get_ping);
        let mediator = mediator_builder.build();
        let req = GetPing {
            message: "".to_string(),
        };
        let reply = mediator.send(req).await;
        assert!(reply.is_err());
        let error = reply.unwrap_err();

        if let Error::HandlerExecutionFailed {
            request_type,
            source,
        } = error
        {
            assert_eq!(request_type, type_name::<GetPing>());
            assert!(
                source.is::<GetPingError>(),
                "Expected source error to be GetPingError"
            );

            if let Some(specific_error) = source.downcast_ref::<GetPingError>() {
                assert!(matches!(specific_error, GetPingError::InvalidMessage(_)));
            } else {
                panic!("Downcast to GetPingError failed");
            }
        } else {
            panic!("Expected Error::HandlerExecutionFailed, got {:?}", error);
        }
    }

    #[tokio::test]
    async fn business_logic_error_fail() {
        let mut mediator_builder = Mediator::builder();
        mediator_builder.register(handle_get_ping);
        let mediator = mediator_builder.build();
        let req = GetPing {
            message: "failme".to_string(),
        };
        let reply = mediator.send(req).await;
        assert!(reply.is_err());
        let error = reply.unwrap_err();

        if let Error::HandlerExecutionFailed {
            request_type,
            source,
        } = error
        {
            assert_eq!(request_type, type_name::<GetPing>());
            assert!(
                source.is::<GetPingError>(),
                "Expected source error to be GetPingError"
            );

            if let Some(specific_error) = source.downcast_ref::<GetPingError>() {
                assert!(matches!(specific_error, GetPingError::InternalFailure));
            } else {
                panic!("Downcast to GetPingError failed");
            }
        } else {
            panic!("Expected Error::HandlerExecutionFailed, got {:?}", error);
        }
    }

    #[tokio::test]
    async fn no_handler_error() {
        let mut mediator_builder = Mediator::builder();
        mediator_builder.register(handle_get_ping);
        let mediator = mediator_builder.build();
        let req = NoHandlerRequest {};
        let reply = mediator.send(req).await;
        assert!(reply.is_err());
        let error = reply.unwrap_err();

        if let Error::HandlerNotFound { ref request_type } = error {
            assert_eq!(request_type, type_name::<NoHandlerRequest>());
        } else {
            panic!("Expected Error::HandlerNotFound, got {:?}", error);
        }
    }
}

// TODO: try to test response downcast and request downcast invariants
