#[derive(Default)]
pub enum EventStatus {
    #[default]
    Draft,
    Cancelled,
    Published,
    Completed,
    Deleted,
}

#[derive(Default)]
pub struct EventAggregate {
    #[allow(dead_code)]
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
    pub fn handle(&self, commands: Commands) -> Result<Vec<Events>, String> {
        match commands {
            Commands::CreateEvent { id, title } => Ok(vec![Events::EventCreated {
                id,
                title,
                status: EventStatus::Draft,
            }]),
            Commands::DeleteEvent { id } => Ok(vec![Events::EventDeleted {
                id,
                status: EventStatus::Deleted,
            }]),
        }
    }
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
