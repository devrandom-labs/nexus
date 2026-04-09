//! Tests for `SerdeCodec`, `SerdeFormat`, `Json`, and `JsonCodec`.

#![cfg(feature = "json")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use std::fmt;

use nexus::*;
use nexus_store::Codec;
use nexus_store::{Json, JsonCodec, SerdeCodec, SerdeFormat};
use serde::{Deserialize, Serialize};

// -- Test domain --

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
enum TodoEvent {
    Created { title: String },
    Done,
}

impl Message for TodoEvent {}
impl DomainEvent for TodoEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created { .. } => "Created",
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
            TodoEvent::Created { title } => self.title.clone_from(title),
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

// =============================================================================
// Unit tests — SerdeFormat (Json)
// =============================================================================

#[test]
fn json_format_roundtrip() {
    let format = Json;
    let event = TodoEvent::Created {
        title: "Buy milk".to_owned(),
    };

    let bytes = format.serialize(&event).unwrap();
    let decoded: TodoEvent = format.deserialize(&bytes).unwrap();

    assert_eq!(decoded, event);
}

#[test]
fn json_format_unit_variant_roundtrip() {
    let format = Json;
    let event = TodoEvent::Done;

    let bytes = format.serialize(&event).unwrap();
    let decoded: TodoEvent = format.deserialize(&bytes).unwrap();

    assert_eq!(decoded, event);
}

// =============================================================================
// Unit tests — JsonCodec (Codec<E> impl)
// =============================================================================

#[test]
fn json_codec_encode_decode_roundtrip() {
    let codec = JsonCodec::default();
    let event = TodoEvent::Created {
        title: "Walk the dog".to_owned(),
    };

    let bytes = codec.encode(&event).unwrap();
    let decoded: TodoEvent = codec.decode("Created", &bytes).unwrap();

    assert_eq!(decoded, event);
}

#[test]
fn json_codec_ignores_event_type_parameter() {
    let codec = JsonCodec::default();
    let event = TodoEvent::Created {
        title: "Ignored type test".to_owned(),
    };

    let bytes = codec.encode(&event).unwrap();

    // Pass a completely wrong event_type — serde JSON dispatches via the
    // internally-tagged `"type"` field, so the event_type parameter is unused.
    let decoded: TodoEvent = codec.decode("TotallyWrongType", &bytes).unwrap();

    assert_eq!(decoded, event);
}

#[test]
fn json_codec_invalid_payload_returns_error() {
    let codec = JsonCodec::default();
    let garbage: &[u8] = b"\xff\xfe not json at all";

    let result: Result<TodoEvent, _> = codec.decode("Created", garbage);

    assert!(result.is_err(), "decoding garbage bytes must fail");
}

#[test]
fn json_codec_is_constructible_via_default() {
    let _codec: JsonCodec = JsonCodec::default();
}

#[test]
fn json_codec_is_constructible_via_new() {
    let _codec: SerdeCodec<Json> = SerdeCodec::new(Json);
}

// =============================================================================
// Integration test — full EventStore round-trip
// =============================================================================

#[cfg(feature = "testing")]
mod integration {
    use super::*;
    use nexus_store::Store;
    use nexus_store::store::Repository;
    use nexus_store::testing::InMemoryStore;

    #[tokio::test]
    async fn json_codec_works_with_event_store() {
        let store = Store::new(InMemoryStore::new());
        let repo = store.repository(JsonCodec::default(), ());

        // Save a "Created" event
        let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
        repo.save(
            &mut agg,
            &[TodoEvent::Created {
                title: "Write tests".to_owned(),
            }],
        )
        .await
        .unwrap();

        // Load and verify state after Created
        let mut loaded: AggregateRoot<TodoAggregate> = repo.load(TodoId(1)).await.unwrap();
        assert_eq!(loaded.state().title, "Write tests");
        assert!(!loaded.state().done);
        assert_eq!(loaded.version(), Some(Version::new(1).unwrap()));

        // Append a Done event
        repo.save(&mut loaded, &[TodoEvent::Done]).await.unwrap();

        // Reload and verify full state
        let final_agg: AggregateRoot<TodoAggregate> = repo.load(TodoId(1)).await.unwrap();
        assert_eq!(final_agg.state().title, "Write tests");
        assert!(final_agg.state().done);
        assert_eq!(final_agg.version(), Some(Version::new(2).unwrap()));
    }
}
