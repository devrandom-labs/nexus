use super::message::Message;
use downcast_rs::impl_downcast;

pub trait DomainEvent: Message {
    fn name(&self) -> &'static str;
}

impl<D> From<D> for Box<dyn DomainEvent>
where
    D: DomainEvent,
{
    fn from(value: D) -> Self {
        Box::new(value)
    }
}

impl_downcast!(sync DomainEvent);
