#![allow(dead_code)]
use std::any::{Any, TypeId};
pub trait Message: Any + Send + Sync + 'static {
    fn type_id(&self) -> TypeId {
        TypeId::of::<Self>()
    }
}
//-------------------- error --------------------//
use thiserror::Error as Err;
#[derive(Debug, Err, PartialEq, Eq, PartialOrd, Ord)]
pub enum BusError {}

//-------------------- handler --------------------//
use tower::Service;

/// Message Handlers are specialized tower service.
/// they take Message as requests and Respond by ()
/// and emit BusError only.
///
/// extremely specialized.
pub trait MessageHandler<M: Message>:
    Service<M, Response = (), Error = BusError> + Send + Sync + 'static
{
}

//-------------------- tests --------------------//
#[cfg(test)]
mod test {
    use super::Message;
    use std::any::TypeId;

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
    use tower::Service;

    // mock request
    struct Request(String);
    #[derive(Debug)]
    struct Response(String);
    struct GreetingService;

    impl Service<Request> for GreetingService {
        type Response = Response;
        type Error = String;
        type Future = Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, req: Request) -> Self::Future {
            if req.0.is_empty() {
                return ready(Err("req is empty".to_string()));
            }
            ready(Ok(Response("Response".to_string())))
        }
    }
    // mock response

    #[tokio::test]
    async fn test_service_handler() {
        let mut service = GreetingService;
        let request = Request("request".to_string());
        let response = service.call(request).await;
        assert!(response.is_ok());
        assert_eq!(response.unwrap().0, "Response");
    }

    #[tokio::test]
    async fn test_service_error() {
        let mut service = GreetingService;
        let request = Request("".to_string());
        let response = service.call(request).await;
        assert!(response.is_err());
        assert_eq!(response.unwrap_err(), "req is empty");
    }

    //--------------------message handler tests--------------------//
    //
    //

    use super::BusError;
    use super::MessageHandler;

    // think: can I control what a message handler sends as an error?
    // no.
}
