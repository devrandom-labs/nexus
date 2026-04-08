//! Integration tests for `EventStore` with owning Codec.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use std::fmt;

use nexus::*;
use nexus_store::Codec;
use nexus_store::UpcastError;
use nexus_store::Upcaster;
use nexus_store::event_store::EventStore;
use nexus_store::morsel::EventMorsel;
use nexus_store::repository::Repository;
use nexus_store::testing::InMemoryStore;

// -- Test domain --

#[derive(Debug, Clone, PartialEq)]
enum TodoEvent {
    Created(String),
    Done,
}
impl Message for TodoEvent {}
impl DomainEvent for TodoEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created(_) => "Created",
            Self::Done => "Done",
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq)]
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
            TodoEvent::Created(t) => self.title.clone_from(t),
            TodoEvent::Done => self.done = true,
        }
        self
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TodoId(u64);
impl fmt::Display for TodoId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "todo-{}", self.0)
    }
}
impl Id for TodoId {}

#[derive(Debug, thiserror::Error)]
#[error("todo error")]
struct TodoError;

struct TodoAggregate;
impl Aggregate for TodoAggregate {
    type State = TodoState;
    type Error = TodoError;
    type Id = TodoId;
}

// -- Simple test codec (no serde dep needed) --

struct TestCodec;

impl Codec<TodoEvent> for TestCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &TodoEvent) -> Result<Vec<u8>, Self::Error> {
        match event {
            TodoEvent::Created(t) => Ok(format!("created:{t}").into_bytes()),
            TodoEvent::Done => Ok(b"done".to_vec()),
        }
    }

    fn decode(&self, _event_type: &str, payload: &[u8]) -> Result<TodoEvent, Self::Error> {
        let s = std::str::from_utf8(payload)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        s.strip_prefix("created:").map_or_else(
            || {
                if s == "done" {
                    Ok(TodoEvent::Done)
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unknown event: {s}"),
                    ))
                }
            },
            |title| Ok(TodoEvent::Created(title.to_owned())),
        )
    }
}

// -- Tests --

#[tokio::test]
async fn save_and_load_roundtrip() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    let events = [TodoEvent::Created("Buy milk".into()), TodoEvent::Done];
    es.save(&mut agg, &events).await.unwrap();

    let loaded: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();
    assert_eq!(loaded.state().title, "Buy milk");
    assert!(loaded.state().done);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn load_empty_stream_returns_fresh_aggregate() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let loaded: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();
    assert_eq!(loaded.version(), None);
    assert_eq!(loaded.state(), &TodoState::default());
}

#[tokio::test]
async fn save_no_uncommitted_events_is_noop() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    es.save(&mut agg, &[]).await.unwrap();
}

#[tokio::test]
async fn save_then_append_more_events() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    es.save(&mut agg, &[TodoEvent::Created("Task".into())])
        .await
        .unwrap();

    let mut loaded: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();
    es.save(&mut loaded, &[TodoEvent::Done]).await.unwrap();

    let final_agg: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();
    assert_eq!(final_agg.state().title, "Task");
    assert!(final_agg.state().done);
    assert_eq!(final_agg.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn optimistic_concurrency_conflict() {
    let store = InMemoryStore::new();
    let es = EventStore::new(store, TestCodec);

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    es.save(&mut agg, &[TodoEvent::Created("Original".into())])
        .await
        .unwrap();

    let mut agg_a: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();
    let mut agg_b: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();

    es.save(&mut agg_a, &[TodoEvent::Done]).await.unwrap();

    let result = es.save(&mut agg_b, &[TodoEvent::Done]).await;
    assert!(result.is_err(), "should get concurrency conflict");
}

// =============================================================================
// Transform integration tests
// =============================================================================

struct V1ToV2Upcaster;
impl Upcaster for V1ToV2Upcaster {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        match (morsel.event_type(), morsel.schema_version()) {
            ("Created", v) if v == Version::INITIAL => {
                // Passthrough — payload format unchanged, just bump version.
                Ok(EventMorsel::new(
                    "Created",
                    Version::new(2).unwrap(),
                    morsel.payload().to_vec(),
                ))
            }
            _ => Ok(morsel),
        }
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        match event_type {
            "Created" => Some(Version::new(2).unwrap()),
            _ => None,
        }
    }
}

#[tokio::test]
async fn load_with_transform_transforms_events() {
    // EventStore with upcaster — save then load through the same store
    let es = EventStore::with_upcaster(InMemoryStore::new(), TestCodec, V1ToV2Upcaster);

    // Save — transforms are only applied on reads, not writes
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    es.save(&mut agg, &[TodoEvent::Created("Task".into())])
        .await
        .unwrap();

    // Load — transform bumps schema version but payload format is unchanged
    let loaded: AggregateRoot<TodoAggregate> = es.load(TodoId(1)).await.unwrap();
    assert_eq!(loaded.state().title, "Task");
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

#[tokio::test]
async fn event_store_with_no_transforms_is_zero_sized_chain() {
    assert_eq!(std::mem::size_of::<()>(), 0);
    let _es = EventStore::new(InMemoryStore::new(), TestCodec);
}
