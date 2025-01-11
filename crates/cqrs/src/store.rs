use crate::event::Event;
use std::error::Error;

pub trait Store {
    type Error: Error;
    fn get_events(&self, id: &str) -> Result<Vec<String>, Self::Error>;
    fn commit(&self, aggregate_type: &str, events: &[impl Event]) -> Result<(), Self::Error>;
}
