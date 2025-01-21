use tokio::sync::oneshot::Sender;

/// payload should not be dynamic dispatch,
/// sum type of messages should be sent through the messages,
///
/// but reply can transfer any kind of type back, so it can be dynamic dispatch
pub struct Message<T, R> {
    reply: Sender<R>,
    payload: T,
}
