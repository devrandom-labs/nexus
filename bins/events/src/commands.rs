pub struct EventId(String);
pub struct CreateEvent;
pub struct DeleteEvent {
    id: EventId,
}
