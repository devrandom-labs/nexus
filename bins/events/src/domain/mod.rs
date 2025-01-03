use crate::application::EventCommand;
use crate::application::EventEvent;
use crate::cqrs::Aggregate;

pub mod event;

pub enum EventErrors {}

pub struct EventAggregate {
    id: String,
}

impl EventAggregate {
    fn new(id: String) -> Self {
        EventAggregate { id }
    }
}

impl Aggregate for EventAggregate {
    type Commands = EventCommand;
    type Events = EventEvent;
    type Errors = EventErrors;
    fn handle(&self, commands: Self::Commands) -> Result<Vec<Self::Events>, Self::Errors> {
        match commands {
            EventCommand::CreateEvent => Ok(vec![EventEvent::EventCreated]),
            EventCommand::DeleteEvent => Ok(vec![EventEvent::EventDeleted]),
        }
    }
    fn apply(&mut self, _event: Self::Events) {} // shouldnt apply create the aggregate? returns a new aggregate?
    fn get_id(&self) -> &str {
        &self.id
    }
}
