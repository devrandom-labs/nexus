use super::message::Message;
use tokio::sync::mpsc::Sender;

pub struct Executor<T>(Sender<Message<T>>);
