#![allow(dead_code)]
use std::any::{Any, TypeId};

#[derive(Clone)]
pub struct Message<T>
where
    T: Any + Send + Sync,
{
    body: T,
}

impl<T> Message<T>
where
    T: Any + Send + Sync,
{
    pub fn new(body: T) -> Self {
        Message { body }
    }

    pub fn body(&self) -> &T {
        &self.body
    }

    pub fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
}

#[cfg(test)]
mod test {
    use super::Message;
    use std::thread::spawn;

    #[test]
    fn create_message() {
        let message = Message::new(1);
        assert_eq!(message.body(), &1);
    }

    #[test]
    fn messages_between_threads() {
        let message = Message::new(String::from("Hello"));
        spawn(move || {
            assert_eq!(message.body(), "Hello");
        });
    }
}

// use std::any::{Any, TypeId};
// pub trait Message: Any + Send + Sync + 'static {
//     fn type_id(&self) -> TypeId {
//         TypeId::of::<Self>()
//     }
// }

// pub struct MessageEnvelope<T>
// where
//     T: Message,
// {
//     message: T,
// }

// impl MessageEnvelope<T>
// where
//     T: Message,
// {
//     pub fn new(message: T) -> Self {
//         MessageEnvelope { message }
//     }
// }

// //-------------------- handler --------------------//
// use std::clone::Clone;
// use tower::{BoxError, Service};

// /// Message Handlers are specialized tower service.
// /// they take Message as requests and Respond by ()
// /// and emit BusError only.
// ///
// /// extremely specialized.
// pub trait MessageHandler:
//     Service<MessageEnvelope, Response = (), Error = BoxError> + Send + Sync + Clone + 'static
// {
// }

// impl<M, S> MessageHandler<M> for S
// where
//     M: Message,
//     S: Service<MessageEnvelope<M>, Response = (), Error = BoxError> + Send + Sync + Clone + 'static,
// {
// }

// //-------------------- tests --------------------//
// #[cfg(test)]
// mod test {
//     use super::{Message, MessageHandler};
//     use std::any::TypeId;

//     // TODO: Test blanked impl of message for types which have any, sync and send;

//     #[test]
//     fn test_message_impl() {
//         struct TestMessage(String);
//         impl Message for TestMessage {}
//         let s = TestMessage("hello".to_string());
//         assert_eq!(s.type_id(), TypeId::of::<TestMessage>()); // on way to check if Message works.
//     }

//     //-------------------- tower service test --------------------//

//     use std::future::{ready, Ready};
//     use std::task::Poll;
//     use thiserror::Error;
//     use tower::{BoxError, Service};

//     // mock request
//     // a service which is also a good candidate for GreetingService.
//     struct Request(String);
//     #[derive(Debug, Error, PartialEq)]
//     #[error("{0}")]
//     struct GreetingServiceError(String);

//     #[derive(Clone)]
//     struct GreetingService;
//     // need a way to convert all ser
//     impl Service<Request> for GreetingService {
//         type Response = ();
//         type Error = BoxError; // this is too generic,
//         type Future = Ready<Result<Self::Response, Self::Error>>;

//         fn poll_ready(
//             &mut self,
//             _cx: &mut std::task::Context<'_>,
//         ) -> std::task::Poll<Result<(), Self::Error>> {
//             Poll::Ready(Ok(()))
//         }

//         fn call(&mut self, req: Request) -> Self::Future {
//             if req.0.is_empty() {
//                 return ready(Err(BoxError::from(GreetingServiceError(
//                     "req is empty".to_string(),
//                 ))));
//             }
//             ready(Ok(()))
//         }
//     }
//     // mock response

//     #[tokio::test]
//     async fn test_service_handler() {
//         let mut service = GreetingService;
//         let request = Request("request".to_string());
//         let response = service.call(request).await;
//         assert!(response.is_ok());
//     }

//     #[tokio::test]
//     async fn test_service_error() {
//         let mut service = GreetingService;
//         let request = Request("".to_string());
//         let response = service.call(request).await;
//         assert!(response.is_err());
//         // TODO: assert the type of error.
//     }

//     //--------------------message handler tests--------------------//
//     impl Message for Request {}
//     trait AssertMessageHandler<M: Message> {
//         fn assert_in_message_handler(self);
//     }

//     impl<M, S> AssertMessageHandler<M> for S
//     where
//         M: Message,
//         S: MessageHandler<M>,
//     {
//         fn assert_in_message_handler(self) {}
//     }

//     #[tokio::test]
//     async fn specific_services_are_message_handlers() {
//         let service = GreetingService;
//         service.assert_in_message_handler();
//     }

//     use tower::util::BoxCloneSyncService;

//     #[tokio::test]
//     async fn message_handlers_into_box_service() {
//         let greeting_service = GreetingService;
//         let mut service = BoxCloneSyncService::new(greeting_service);
//         let response = service.call(Request("some request".to_string())).await;
//         assert!(response.is_ok());
//     }

//     //TODO: test whether we can erase the type of request and response to Message

//     #[tokio::test]
//     async fn box_service_dynamic_dispatch() {
//         let greeting_service = GreetingService;
//     }
// }
