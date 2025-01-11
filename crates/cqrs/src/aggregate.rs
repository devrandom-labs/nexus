use std::error::Error;

pub trait DomainEvents {
    fn get_version(&self) -> String;
}
pub trait Aggregate {
    const TYPE: &'static str;
    type Event: DomainEvents;
    type Error: Error;
    fn apply(&mut self, event: Self::Event);
}
