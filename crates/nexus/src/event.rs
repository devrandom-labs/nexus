use crate::message::Message;

pub trait DomainEvent: Message {
    fn name(&self) -> &'static str;
}
