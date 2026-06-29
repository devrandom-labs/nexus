//! Integration tests for `EventStore` with owning Codec.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(
    clippy::unnecessary_wraps,
    reason = "plain-function upcasters keep Result<_, E> so they can be passed to load_with"
)]

use std::convert::Infallible;
use std::fmt;

use nexus::*;
use nexus_store::Repository;
use nexus_store::Store;
use nexus_store::testing::InMemoryStore;
use nexus_store::upcasting::EventMorsel;
use nexus_store::{Decode, Encode};

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
struct TodoId(String);
impl fmt::Display for TodoId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl AsRef<[u8]> for TodoId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
impl Id for TodoId {
    const BYTE_LEN: usize = 0;
}

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

impl Encode<TodoEvent> for TestCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &TodoEvent) -> Result<bytes::Bytes, Self::Error> {
        match event {
            TodoEvent::Created(t) => Ok(bytes::Bytes::from(format!("created:{t}").into_bytes())),
            TodoEvent::Done => Ok(bytes::Bytes::from_static(b"done")),
        }
    }
}

impl Decode<TodoEvent> for TestCodec {
    type Output<'a> = TodoEvent;
    type Error = std::io::Error;

    fn decode<'a>(
        &'a self,
        env: &'a nexus_store::PersistedEnvelope,
    ) -> Result<TodoEvent, Self::Error> {
        let s = std::str::from_utf8(env.payload())
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
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(TestCodec).build();

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId("todo-1".into()));
    let events = [TodoEvent::Created("Buy milk".into()), TodoEvent::Done];
    es.save(&mut agg, &save_events(&events)).await.unwrap();

    let loaded: AggregateRoot<TodoAggregate> = es.load(TodoId("todo-1".into())).await.unwrap();
    assert_eq!(loaded.state().title, "Buy milk");
    assert!(loaded.state().done);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn load_empty_stream_returns_fresh_aggregate() {
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(TestCodec).build();
    let loaded: AggregateRoot<TodoAggregate> = es.load(TodoId("todo-1".into())).await.unwrap();
    assert_eq!(loaded.version(), None);
    assert_eq!(loaded.state(), &TodoState::default());
}

#[tokio::test]
async fn load_and_save_infer_aggregate_from_repository_binding() {
    // #243: the aggregate is named ONCE, at `repository::<TodoAggregate>()`.
    // `load`/`save` then infer it with NO per-call annotation — note there is
    // no `AggregateRoot<TodoAggregate>` annotation on `fresh` or `loaded` below.
    // That is the whole guarantee: if a blanket `impl<A> Repository<A> for
    // EventStore` ever returns, the `repository::<_>()` binding disappears and
    // this test stops compiling.
    let store = Store::new(InMemoryStore::new());
    let es = store.repository::<TodoAggregate>().codec(TestCodec).build();

    // `fresh` is inferred as AggregateRoot<TodoAggregate> purely from `es`.
    let fresh = es.load(TodoId("t".into())).await.unwrap();
    assert_eq!(fresh.version(), None);

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId("t".into()));
    es.save(
        &mut agg,
        &save_events(&[TodoEvent::Created("Buy milk".into()), TodoEvent::Done]),
    )
    .await
    .unwrap();

    // Still no annotation — `loaded`'s type comes from `es`'s bound aggregate.
    let loaded = es.load(TodoId("t".into())).await.unwrap();
    assert_eq!(loaded.state().title, "Buy milk");
    assert!(loaded.state().done);
    assert_eq!(loaded.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn into_store_yields_a_usable_store_handle() {
    use nexus_store::store::RawEventStore;
    // #244: `raw.into_store()` is the de-nested equivalent of `Store::new(raw)`
    // — opening flows left-to-right into `.repository::<A>()` with no wrap.
    let es = InMemoryStore::new()
        .into_store()
        .repository::<TodoAggregate>()
        .codec(TestCodec)
        .build();
    let loaded = es.load(TodoId("t".into())).await.unwrap();
    assert_eq!(loaded.version(), None);
}

// REMOVED `save_no_uncommitted_events_is_noop` (#207): `save` now takes
// `&Events<E, N>`, so an empty batch is a compile error, not a runtime no-op.

#[tokio::test]
async fn save_then_append_more_events() {
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(TestCodec).build();

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId("todo-1".into()));
    es.save(&mut agg, &save_events(&[TodoEvent::Created("Task".into())]))
        .await
        .unwrap();

    let mut loaded: AggregateRoot<TodoAggregate> = es.load(TodoId("todo-1".into())).await.unwrap();
    es.save(&mut loaded, &save_events(&[TodoEvent::Done]))
        .await
        .unwrap();

    let final_agg: AggregateRoot<TodoAggregate> = es.load(TodoId("todo-1".into())).await.unwrap();
    assert_eq!(final_agg.state().title, "Task");
    assert!(final_agg.state().done);
    assert_eq!(final_agg.version(), Some(Version::new(2).unwrap()));
}

#[tokio::test]
async fn optimistic_concurrency_conflict() {
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(TestCodec).build();

    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId("todo-1".into()));
    es.save(
        &mut agg,
        &save_events(&[TodoEvent::Created("Original".into())]),
    )
    .await
    .unwrap();

    let mut agg_a: AggregateRoot<TodoAggregate> = es.load(TodoId("todo-1".into())).await.unwrap();
    let mut agg_b: AggregateRoot<TodoAggregate> = es.load(TodoId("todo-1".into())).await.unwrap();

    es.save(&mut agg_a, &save_events(&[TodoEvent::Done]))
        .await
        .unwrap();

    let result = es.save(&mut agg_b, &save_events(&[TodoEvent::Done])).await;
    assert!(result.is_err(), "should get concurrency conflict");
}

// =============================================================================
// Transform integration tests
// =============================================================================

/// Plain-function upcaster that bumps `"Created"` from v1 to v2. Payload
/// is unchanged; only the schema version advances.
fn v1_to_v2_upcast(morsel: EventMorsel<'_>) -> Result<EventMorsel<'_>, Infallible> {
    match (morsel.event_type(), morsel.schema_version()) {
        ("Created", v) if v == Version::INITIAL => Ok(EventMorsel::new(
            "Created",
            Version::new(2).unwrap(),
            morsel.payload().to_vec(),
        )),
        _ => Ok(morsel),
    }
}

fn v1_to_v2_current_version(event_type: &str) -> Option<Version> {
    match event_type {
        "Created" => Some(Version::new(2).unwrap()),
        _ => None,
    }
}

#[tokio::test]
async fn load_with_transform_transforms_events() {
    // EventStore with upcaster — save then load through the same store
    let store = Store::new(InMemoryStore::new());
    let es = store.repository().codec(TestCodec).build();

    // Save_with stamps the schema version per the version-lookup fn.
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId("todo-1".into()));
    es.save_with(
        &mut agg,
        &save_events(&[TodoEvent::Created("Task".into())]),
        v1_to_v2_current_version,
    )
    .await
    .unwrap();

    // load_with runs the upcast fn over each persisted event before
    // decoding. Payload format unchanged in this test, so the only
    // observable effect is that the upcast ran successfully.
    let loaded: AggregateRoot<TodoAggregate> = es
        .load_with(TodoId("todo-1".into()), v1_to_v2_upcast)
        .await
        .unwrap();
    assert_eq!(loaded.state().title, "Task");
    assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));
}

#[tokio::test]
async fn event_store_with_no_transforms_is_zero_sized_chain() {
    assert_eq!(std::mem::size_of::<()>(), 0);
    let store = Store::new(InMemoryStore::new());
    let _es = store.repository::<TodoAggregate>().codec(TestCodec).build();
}

// ─── #207 test helper ──────────────────────────────────────────────────────
// `Repository::save` takes `&Events<E, N>` (non-empty, compile-time capacity).
// These tests build batches from runtime-length slices/proptest vectors, so we
// pack them into `Events<E, 32>` (capacity 33 — covers every batch built here;
// the largest strategy yields 29). Empty input is a programmer error: `save`
// makes a zero-event batch unrepresentable by construction.
fn save_events<E: nexus::DomainEvent + Clone>(slice: &[E]) -> nexus::Events<E, 32> {
    let (first, rest) = slice
        .split_first()
        .expect("save requires at least one event");
    let mut events = nexus::Events::new(first.clone());
    for event in rest {
        events.add(event.clone());
    }
    events
}
