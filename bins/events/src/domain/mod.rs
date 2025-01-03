use crate::application::EventCommand;
use crate::application::EventEvent;
use crate::cqrs::Aggregate;

pub mod event;

pub enum EventErrors {}

pub struct EventAggregate;

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
    fn apply(&mut self, _event: Self::Events) {}
    fn get_aggregate() -> String {
        "event".to_string()
    }
}
