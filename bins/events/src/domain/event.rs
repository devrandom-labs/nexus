use ulid::Ulid;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub struct EventId(Ulid);

impl EventId {
    pub fn new() -> Self {
        EventId(Ulid::new())
    }
}
