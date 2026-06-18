//! Tests for #[derive(Aggregate)]

use nexus::*;
use std::fmt;

// --- Domain types ---

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TodoId([u8; 8]);
impl TodoId {
    fn new(id: u64) -> Self {
        Self(id.to_be_bytes())
    }
}
impl fmt::Display for TodoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "todo-{}", u64::from_be_bytes(self.0))
    }
}
impl AsRef<[u8]> for TodoId {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl Id for TodoId {
    const BYTE_LEN: usize = 8;
}

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

// --- Commands + Handle impls on the marker ---

struct CreateTodo {
    title: String,
}

struct CompleteTodo;

impl Handle<CreateTodo> for TodoAggregate {
    fn handle(state: &TodoState, cmd: CreateTodo) -> Result<Events<TodoEvent>, TodoError> {
        if !state.title.is_empty() {
            return Err(TodoError::AlreadyExists);
        }
        Ok(events![TodoEvent::Created(TodoCreated {
            title: cmd.title
        })])
    }
}

impl Handle<CompleteTodo> for TodoAggregate {
    fn handle(state: &TodoState, _cmd: CompleteTodo) -> Result<Events<TodoEvent>, TodoError> {
        if state.done {
            return Err(TodoError::AlreadyDone);
        }
        Ok(events![TodoEvent::Completed(TodoCompleted)])
    }
}

// --- Tests ---

#[test]
fn derive_aggregate_lifecycle() {
    let mut todo = AggregateRoot::<TodoAggregate>::new(TodoId::new(1));
    let created = todo
        .handle(CreateTodo {
            title: "Buy milk".into(),
        })
        .unwrap();
    todo.apply_events(&created);
    let completed = todo.handle(CompleteTodo).unwrap();
    todo.apply_events(&completed);

    assert_eq!(todo.state().title, "Buy milk");
    assert!(todo.state().done);
    // Version is None because apply_events does not advance version
    // (version is only advanced via replay or advance_version)
    assert_eq!(todo.version(), None);
}

#[test]
fn derive_aggregate_invariants() {
    let mut todo = AggregateRoot::<TodoAggregate>::new(TodoId::new(2));
    let created = todo
        .handle(CreateTodo {
            title: "Test".into(),
        })
        .unwrap();
    todo.apply_events(&created);

    assert!(matches!(
        todo.handle(CreateTodo {
            title: "Again".into()
        }),
        Err(TodoError::AlreadyExists)
    ));

    let completed = todo.handle(CompleteTodo).unwrap();
    todo.apply_events(&completed);
    assert!(matches!(
        todo.handle(CompleteTodo),
        Err(TodoError::AlreadyDone)
    ));
}

#[test]
fn derive_aggregate_rehydrate() {
    let mut todo = AggregateRoot::<TodoAggregate>::new(TodoId::new(3));
    todo.replay(
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
    let todo = AggregateRoot::<TodoAggregate>::new(TodoId::new(42));
    assert_eq!(todo.id(), &TodoId::new(42));
}

#[test]
fn derive_aggregate_debug_shows_name_and_version() {
    let todo = AggregateRoot::<TodoAggregate>::new(TodoId::new(1));
    let debug = format!("{todo:?}");
    assert!(debug.contains("AggregateRoot"));
    assert!(debug.contains("version"));
    assert!(debug.contains("id"));
}

#[test]
fn derive_aggregate_debug_does_not_leak_state() {
    let mut todo = AggregateRoot::<TodoAggregate>::new(TodoId::new(1));
    let created = todo
        .handle(CreateTodo {
            title: "SECRET_TITLE".into(),
        })
        .unwrap();
    todo.apply_events(&created);
    let debug = format!("{todo:?}");
    // State must NOT appear in debug output
    assert!(
        !debug.contains("SECRET_TITLE"),
        "Debug output leaked internal state: {debug}"
    );
}
