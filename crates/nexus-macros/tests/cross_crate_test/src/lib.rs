//! Cross-crate test — verifies macros work from an external crate.
//! This is the real user scenario: nexus is a dependency, not the current crate.

use nexus::*;
use std::fmt;

// --- ID ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TaskId(pub u64);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "task-{}", self.0)
    }
}

impl Id for TaskId {}

// --- Events (derive macro from external crate) ---
#[derive(Debug, Clone, DomainEvent)]
pub enum TaskEvent {
    Created(TaskCreated),
    Completed(TaskCompleted),
}

#[derive(Debug, Clone)]
pub struct TaskCreated {
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct TaskCompleted;

// --- State ---
#[derive(Default, Debug)]
pub struct TaskState {
    pub title: String,
    pub done: bool,
}

impl AggregateState for TaskState {
    type Event = TaskEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(&mut self, event: &TaskEvent) {
        match event {
            TaskEvent::Created(e) => self.title = e.title.clone(),
            TaskEvent::Completed(_) => self.done = true,
        }
    }
    fn name(&self) -> &'static str {
        "Task"
    }
}

// --- Error ---
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("already exists")]
    AlreadyExists,
    #[error("already done")]
    AlreadyDone,
}

// --- Aggregate (attribute macro from external crate) ---
#[nexus::aggregate(state = TaskState, error = TaskError, id = TaskId)]
pub struct TaskAggregate;

// --- Business logic on the generated type ---
impl TaskAggregate {
    pub fn create(&mut self, title: String) -> Result<(), TaskError> {
        if !self.state().title.is_empty() {
            return Err(TaskError::AlreadyExists);
        }
        self.apply_event(TaskEvent::Created(TaskCreated { title }));
        Ok(())
    }

    pub fn complete(&mut self) -> Result<(), TaskError> {
        if self.state().done {
            return Err(TaskError::AlreadyDone);
        }
        self.apply_event(TaskEvent::Completed(TaskCompleted));
        Ok(())
    }
}

// --- Verify AggregateEntity works generically ---
pub fn generic_version<A: AggregateEntity>(agg: &A) -> Version {
    agg.version()
}

pub fn generic_take<A: AggregateEntity>(agg: &mut A) -> usize {
    agg.take_uncommitted_events().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_crate_lifecycle() {
        let mut task = TaskAggregate::new(TaskId(1));
        task.create("Write tests".into()).unwrap();
        task.complete().unwrap();

        assert_eq!(task.state().title, "Write tests");
        assert!(task.state().done);
        assert_eq!(task.current_version(), Version::from_persisted(2));
    }

    #[test]
    fn cross_crate_invariants() {
        let mut task = TaskAggregate::new(TaskId(2));
        task.create("Task".into()).unwrap();
        assert!(task.create("Again".into()).is_err());
        task.complete().unwrap();
        assert!(task.complete().is_err());
    }

    #[test]
    fn cross_crate_rehydrate() {
        let history = vec![VersionedEvent::from_persisted(
            Version::from_persisted(1),
            TaskEvent::Created(TaskCreated {
                title: "Loaded".into(),
            }),
        )];
        let task = TaskAggregate::load_from_events(TaskId(3), history).unwrap();
        assert_eq!(task.state().title, "Loaded");
        assert_eq!(task.version(), Version::from_persisted(1));
    }

    #[test]
    fn cross_crate_generic_aggregate_entity() {
        let mut task = TaskAggregate::new(TaskId(4));
        task.create("Generic".into()).unwrap();

        // These functions accept any AggregateEntity — proves the trait works
        assert_eq!(generic_version(&task), Version::INITIAL);
        assert_eq!(generic_take(&mut task), 1);
    }

    #[test]
    fn cross_crate_domain_event_name() {
        let event = TaskEvent::Created(TaskCreated {
            title: "test".into(),
        });
        assert_eq!(event.name(), "Created");

        let event = TaskEvent::Completed(TaskCompleted);
        assert_eq!(event.name(), "Completed");
    }
}
