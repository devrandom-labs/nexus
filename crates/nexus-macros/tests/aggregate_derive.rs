//! Tests for #[derive(Aggregate)]

use nexus::*;
use std::fmt;

// --- Domain types ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TodoId(u64);
impl fmt::Display for TodoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "todo-{}", self.0)
    }
}
impl Id for TodoId {}

#[derive(Debug, Clone)]
struct TodoCreated {
    title: String,
}
#[derive(Debug, Clone)]
struct TodoCompleted;

#[derive(Debug, Clone, nexus::DomainEvent)]
enum TodoEvent {
    Created(TodoCreated),
    Completed(TodoCompleted),
}

#[derive(Default, Debug, Clone)]
struct TodoState {
    title: String,
    done: bool,
}
impl AggregateState for TodoState {
    type Event = TodoEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &TodoEvent) -> Self {
        match event {
            TodoEvent::Created(e) => self.title = e.title.clone(),
            TodoEvent::Completed(_) => self.done = true,
        }
        self
    }
}

#[derive(Debug, thiserror::Error)]
enum TodoError {
    #[error("already exists")]
    AlreadyExists,
    #[error("already done")]
    AlreadyDone,
}

// --- The derive macro in action ---

#[nexus::aggregate(state = TodoState, error = TodoError, id = TodoId)]
struct TodoAggregate;

// --- Business logic on the derived type ---

impl TodoAggregate {
    fn create(&mut self, title: String) -> Result<(), TodoError> {
        if !self.state().title.is_empty() {
            return Err(TodoError::AlreadyExists);
        }
        self.root_mut()
            .apply_event(&TodoEvent::Created(TodoCreated { title }));
        Ok(())
    }

    fn complete(&mut self) -> Result<(), TodoError> {
        if self.state().done {
            return Err(TodoError::AlreadyDone);
        }
        self.root_mut()
            .apply_event(&TodoEvent::Completed(TodoCompleted));
        Ok(())
    }
}

// --- Tests ---

#[test]
fn derive_aggregate_lifecycle() {
    let mut todo = TodoAggregate::new(TodoId(1));
    todo.create("Buy milk".into()).unwrap();
    todo.complete().unwrap();

    assert_eq!(todo.state().title, "Buy milk");
    assert!(todo.state().done);
    // Version is None because apply_event does not advance version
    // (version is only advanced via replay or advance_version)
    assert_eq!(todo.version(), None);
}

#[test]
fn derive_aggregate_invariants() {
    let mut todo = TodoAggregate::new(TodoId(2));
    todo.create("Test".into()).unwrap();

    assert!(matches!(
        todo.create("Again".into()),
        Err(TodoError::AlreadyExists)
    ));

    todo.complete().unwrap();
    assert!(matches!(todo.complete(), Err(TodoError::AlreadyDone)));
}

#[test]
fn derive_aggregate_rehydrate() {
    let mut todo = TodoAggregate::new(TodoId(3));
    todo.root_mut()
        .replay(
            Version::INITIAL,
            &TodoEvent::Created(TodoCreated {
                title: "Loaded".into(),
            }),
        )
        .unwrap();
    assert_eq!(todo.state().title, "Loaded");
    assert_eq!(todo.version(), Version::new(1));
}

#[test]
fn derive_aggregate_id() {
    let todo = TodoAggregate::new(TodoId(42));
    assert_eq!(todo.id(), &TodoId(42));
}

#[test]
fn derive_aggregate_debug_shows_name_and_version() {
    let todo = TodoAggregate::new(TodoId(1));
    let debug = format!("{todo:?}");
    assert!(debug.contains("TodoAggregate"));
    assert!(debug.contains("version"));
    assert!(debug.contains("id"));
}

#[test]
fn derive_aggregate_debug_does_not_leak_state() {
    let mut todo = TodoAggregate::new(TodoId(1));
    todo.create("SECRET_TITLE".into()).unwrap();
    let debug = format!("{todo:?}");
    // State must NOT appear in debug output
    assert!(
        !debug.contains("SECRET_TITLE"),
        "Debug output leaked internal state: {debug}"
    );
}
