pub mod event;
pub mod location;

pub struct EventAggregate {
    event: event::Event,
    location: location::Location,
}
