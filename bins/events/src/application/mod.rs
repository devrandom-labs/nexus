#[allow(dead_code)]
#[derive(Debug)]
pub enum EventCommand {
    CreateEvent,
    DeleteEvent,
}

#[allow(dead_code)]
#[derive(Debug)]
pub enum EventEvent {
    EventCreated,
    EventDeleted,
}
