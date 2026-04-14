//! Cross-crate test — verifies macros work from an external crate.
//! This is the real user scenario: nexus is a dependency, not the current crate.

use nexus::*;
use std::fmt;

// --- ID ---
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct TaskId(pub [u8; 8]);

impl TaskId {
    pub fn new(id: u64) -> Self {
        Self(id.to_be_bytes())
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "task-{}", u64::from_be_bytes(self.0))
    }
}

impl AsRef<[u8]> for TaskId {
    fn as_ref(&self) -> &[u8] {
        &self.0
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
#[derive(Default, Debug, Clone)]
pub struct TaskState {
    pub title: String,
    pub done: bool,
}

impl AggregateState for TaskState {
    type Event = TaskEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &TaskEvent) -> Self {
        match event {
            TaskEvent::Created(e) => self.title = e.title.clone(),
            TaskEvent::Completed(_) => self.done = true,
        }
        self
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

// --- Commands ---
pub struct CreateTask {
    pub title: String,
}

pub struct CompleteTask;

impl Handle<CreateTask> for TaskAggregate {
    fn handle(&self, cmd: CreateTask) -> Result<Events<TaskEvent>, TaskError> {
        if !self.state().title.is_empty() {
            return Err(TaskError::AlreadyExists);
        }
        Ok(events![TaskEvent::Created(TaskCreated {
            title: cmd.title
        })])
    }
}

impl Handle<CompleteTask> for TaskAggregate {
    fn handle(&self, _cmd: CompleteTask) -> Result<Events<TaskEvent>, TaskError> {
        if self.state().done {
            return Err(TaskError::AlreadyDone);
        }
        Ok(events![TaskEvent::Completed(TaskCompleted)])
    }
}

// --- Verify AggregateEntity works generically ---
pub fn generic_version<A: AggregateEntity>(agg: &A) -> Option<Version> {
    agg.version()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    #[test]
    fn cross_crate_lifecycle() {
        let mut task = TaskAggregate::new(TaskId::new(1));
        let decided = task
            .handle(CreateTask {
                title: "Write tests".into(),
            })
            .unwrap();
        task.root_mut().apply_events(&decided);
        task.root_mut().advance_version(Version::new(1).unwrap());

        let decided = task.handle(CompleteTask).unwrap();
        task.root_mut().apply_events(&decided);
        task.root_mut().advance_version(Version::new(2).unwrap());

        assert_eq!(task.state().title, "Write tests");
        assert!(task.state().done);
        assert_eq!(task.version(), Version::new(2));
    }

    #[test]
    fn cross_crate_invariants() {
        let mut task = TaskAggregate::new(TaskId::new(2));
        let decided = task
            .handle(CreateTask {
                title: "Task".into(),
            })
            .unwrap();
        task.root_mut().apply_events(&decided);

        assert!(
            task.handle(CreateTask {
                title: "Again".into()
            })
            .is_err()
        );

        let decided = task.handle(CompleteTask).unwrap();
        task.root_mut().apply_events(&decided);

        assert!(task.handle(CompleteTask).is_err());
    }

    #[test]
    fn cross_crate_rehydrate() {
        let mut task = TaskAggregate::new(TaskId::new(3));
        task.root_mut()
            .replay(
                Version::new(1).unwrap(),
                &TaskEvent::Created(TaskCreated {
                    title: "Loaded".into(),
                }),
            )
            .unwrap();
        assert_eq!(task.state().title, "Loaded");
        assert_eq!(task.version(), Version::new(1));
    }

    #[test]
    fn cross_crate_generic_aggregate_entity() {
        let task = TaskAggregate::new(TaskId::new(4));
        // This function accepts any AggregateEntity — proves the trait works
        assert_eq!(generic_version(&task), None);
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
