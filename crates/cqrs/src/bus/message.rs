use std::fmt::Debug;
use tokio::sync::oneshot::Sender;

/// payload should not be dynamic dispatch,
/// sum type of messages should be sent through the messages,
///
/// but reply can transfer any kind of type back, so it can be dynamic dispatch
#[derive(Debug)]
pub struct Message<T, R>
where
    R: Debug,
    T: Debug,
{
    reply: Sender<R>,
    payload: T,
}
