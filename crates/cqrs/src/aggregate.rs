use crate::event::DomainEvent;
use std::error::Error;

pub trait Aggregate {
    const TYPE: &'static str;
    type Event: DomainEvent;
    type Error: Error;
    fn apply(&mut self, event: Self::Event);
}
