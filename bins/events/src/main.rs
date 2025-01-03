pub enum EventStatus {
    Draft,
    Cancelled,
    Published,
    Completed,
    Deleted,
}

pub struct EventAggregate {
    id: String,
    title: String,
    status: EventStatus,
}

pub enum Commands {
    CreateEvent { id: String, title: String },
    DeleteEvent { id: String },
}

pub enum Events {
    EventCreated {
        id: String,
        title: String,
        status: EventStatus,
    },
    EventDeleted {
        id: String,
        status: EventStatus,
    },
}

impl EventAggregate {
    pub fn apply(&mut self, events: Events) {
        match events {
            Events::EventCreated { title, status, .. } => {
                self.title = title;
                self.status = status;
            }
            Events::EventDeleted { status, .. } => {
                self.status = status;
            }
        }
    }
}

fn main() {}
