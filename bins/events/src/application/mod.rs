#[derive(Debug)]
pub enum EventCommand {
    CreateEvent,
    DeleteEvent,
}

#[derive(debug)]
pub enum EventEvent {
    EventCreated,
    EventDeleted,
}
