use super::message::Message;
use tower::{BoxError, Service};

// any service which takes message of any type T as request
// any service which gives message of any type R as response
// is eligible to be a message service.
//
// Message service can be executed in any tasks, it has to be send and sync,
//
// if a message service is executed in a task, it would need to last longer than life time of Request
//
pub trait MessageService:
    Service<Message, Response = Message, Error = BoxError> + Send + Sync + Clone
{
}

// blanket auto impl of Message Service to all services which fall in this category
// any service which has Message<T> for Request/Respone would be treated as MessageService.
impl<S> MessageService for S where
    S: Service<Message, Response = Message, Error = BoxError> + Send + Sync + Clone
{
}
