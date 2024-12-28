#[derive(Debug)]
pub struct EventId(String);

#[derive(Debug)]
pub struct Event {
    id: EventId,
    title: String,
}
