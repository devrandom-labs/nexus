#![allow(dead_code)]
use std::any::{Any, TypeId};
pub trait Message: Any + Send + Sync + 'static {
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}
//-------------------- error --------------------//
use std::error::Error;
pub trait MessageHandlerError: Error + Send + Sync {}

//-------------------- handler --------------------//
use tower::Service;

/// Message Handlers are specialized tower service.
/// they take Message as requests and Respond by ()
/// and emit BusError only.
///
/// extremely specialized.
pub trait MessageHandler<M: Message, E: MessageHandlerError>:
    Service<M, Response = (), Error = E> + Send + Sync + 'static
{
}

impl<M, E, S> MessageHandler<M, E> for S
where
    M: Message,
    E: MessageHandlerError,
    S: Service<M, Response = (), Error = E> + Send + Sync + 'static,
{
}

//-------------------- utils --------------------//
//
// TODO: converst async fn to MessageHandler
// TODO: how to hold service.

//-------------------- tests --------------------//
#[cfg(test)]
mod test {
    use super::{Message, MessageHandler, MessageHandlerError};
    use std::any::TypeId;

    // TODO: Test blanked impl of message for types which have any, sync and send;

    #[test]
    fn test_message_impl() {
        struct TestMessage(String);
        impl Message for TestMessage {}
        let s = TestMessage("hello".to_string());
        assert_eq!(s.type_id(), TypeId::of::<TestMessage>()); // on way to check if Message works.
    }

    //-------------------- tower service test --------------------//

    use std::future::{ready, Ready};
    use std::task::Poll;
    use thiserror::Error;
    use tower::Service;

    // mock request
    // a service which is also a good candidate for GreetingService.
    struct Request(String);

    #[derive(Debug, Error, PartialEq)]
    #[error("{0}")]
    struct GreetingServiceError(String);
    impl MessageHandlerError for GreetingServiceError {}
    struct GreetingService;
    impl Service<Request> for GreetingService {
        type Response = ();
        type Error = GreetingServiceError; // this is too generic, should I only understand my errors?
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: Request) -> Self::Future {
            if req.0.is_empty() {
                return ready(Err(GreetingServiceError("req is empty".to_string())));
            }
            ready(Ok(()))
        }
    }
    // mock response

    #[tokio::test]
    async fn test_service_handler() {
        let mut service = GreetingService;
        let request = Request("request".to_string());
        let response = service.call(request).await;
        assert!(response.is_ok());
    }

    #[tokio::test]
    async fn test_service_error() {
        let mut service = GreetingService;
        let request = Request("".to_string());
        let response = service.call(request).await;
        assert!(response.is_err());
        assert_eq!(
            response.unwrap_err(),
            GreetingServiceError("req is empty".to_string())
        );
    }

    //--------------------message handler tests--------------------//
    //
    //

    impl Message for Request {}
    trait AssertMessageHandler<M: Message, E: MessageHandlerError> {
        fn assert_in_message_handler(self);
    }

    impl<M, E, S> AssertMessageHandler<M, E> for S
    where
        M: Message,
        E: MessageHandlerError,
        S: MessageHandler<M, E>,
    {
        fn assert_in_message_handler(self) {}
    }

    #[tokio::test]
    async fn auto_impl_message_handler() {
        let service = GreetingService;
        service.assert_in_message_handler();
    }

    #[tokio::test]
    async fn message_handlers_should_be_executabl() {}
}
