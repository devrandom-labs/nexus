// examples/store-inmemory — demonstrates all nexus-store traits with an
// in-memory backend: Codec, RawEventStore, EventStream, plain-function
// upcasters, and the PendingEnvelope typestate builder. The final step
// shows the substrate path: `Store::raw()` + `map_err` + `try_fold`
// composing a typed pipeline over the lending cursor.
//
// Run with: cargo run -p nexus-example-store-inmemory

// Relaxed lints for example code — production crates should NOT do this.
#![allow(clippy::unwrap_used, reason = "example code uses unwrap for brevity")]
#![allow(clippy::expect_used, reason = "example code uses expect for clarity")]
#![allow(
    clippy::print_stdout,
    reason = "example code prints to demonstrate output"
)]
#![allow(
    clippy::str_to_string,
    reason = "example code uses to_string for readability"
)]
#![allow(clippy::shadow_reuse, reason = "example code shadows for readability")]
#![allow(
    clippy::shadow_unrelated,
    reason = "example code shadows for readability"
)]

use nexus::Id;
use nexus::Version;
use nexus_store::Store;
use nexus_store::store::RawEventStore;
use nexus_store::stream::{EventStream, EventStreamExt};
use nexus_store::testing::InMemoryStore;
use nexus_store::upcasting::EventMorsel;
use nexus_store::{Decode, Encode, pending_envelope};
use serde::{Deserialize, Serialize};
use std::fmt;

// =============================================================================
// 0. Stream ID type — needed by RawEventStore
// =============================================================================

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct TodoId(String);

impl fmt::Display for TodoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Id for TodoId {
    const BYTE_LEN: usize = 0;
}

impl AsRef<[u8]> for TodoId {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

// =============================================================================
// 1. Domain events — a simple Todo aggregate
// =============================================================================

/// The domain events for our Todo aggregate.
///
/// Each variant carries its own data. The `event_type` string used in
/// the store is derived from the variant name (e.g. `"TodoCreated"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum TodoEvent {
    #[serde(rename = "TodoCreated")]
    Created { id: String, title: String },
    #[serde(rename = "TodoCompleted")]
    Completed { id: String },
    #[serde(rename = "TodoDeleted")]
    Deleted { id: String },
}

impl TodoEvent {
    /// Returns the event type name used in the store.
    ///
    /// In a real nexus application, you'd derive `DomainEvent` and get
    /// this automatically. Here we do it by hand.
    const fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "TodoCreated",
            Self::Completed { .. } => "TodoCompleted",
            Self::Deleted { .. } => "TodoDeleted",
        }
    }
}

// =============================================================================
// 2. JsonCodec — implements Codec<TodoEvent> using serde_json
// =============================================================================

/// A JSON-based codec for `TodoEvent`.
///
/// The `Codec` trait converts between typed events and raw bytes.
/// It knows nothing about envelopes, streams, or versions — just
/// serialization.
struct JsonCodec;

/// Codec errors.
#[derive(Debug, thiserror::Error)]
enum CodecError {
    #[error("JSON serialization failed: {0}")]
    Serialize(#[source] serde_json::Error),

    #[error("JSON deserialization failed: {0}")]
    Deserialize(#[source] serde_json::Error),

    #[error("Unknown event type: {0}")]
    UnknownType(String),
}

impl Encode<TodoEvent> for JsonCodec {
    type Error = CodecError;

    fn encode(&self, event: &TodoEvent) -> Result<bytes::Bytes, Self::Error> {
        serde_json::to_vec(event)
            .map(bytes::Bytes::from)
            .map_err(CodecError::Serialize)
    }
}

impl Decode<TodoEvent> for JsonCodec {
    type Output<'a> = TodoEvent;
    type Error = CodecError;

    fn decode<'a>(
        &'a self,
        env: &'a nexus_store::PersistedEnvelope,
    ) -> Result<TodoEvent, Self::Error> {
        // In a real system you might dispatch on `event_type` to pick
        // different structs. Here our enum is self-describing via serde,
        // but we still validate the type name.
        match env.event_type() {
            "TodoCreated" | "TodoCompleted" | "TodoDeleted" => {
                serde_json::from_slice(env.payload()).map_err(CodecError::Deserialize)
            }
            other => Err(CodecError::UnknownType(other.to_owned())),
        }
    }
}

// =============================================================================
// 3. rename_upcast — schema evolution demo (plain function)
// =============================================================================

/// Plain-function upcaster: renames `"TaskCreated"` to `"TodoCreated"`.
///
/// Imagine we originally called the event `TaskCreated` (v1) and later
/// renamed it to `TodoCreated` (v2). Old events stored as `TaskCreated`
/// are transparently upgraded during reads.
///
/// Upcast functions operate on raw bytes BEFORE the codec deserializes —
/// so the old Rust type doesn't need to exist.
fn rename_upcast(morsel: EventMorsel<'_>) -> Result<EventMorsel<'_>, std::convert::Infallible> {
    if morsel.event_type() != "TaskCreated" || morsel.schema_version().as_u64() >= 2 {
        return Ok(morsel);
    }
    // Rename the event type. The payload JSON uses serde's tagged enum
    // format, so we also rename the tag inside the JSON body.
    let json = String::from_utf8_lossy(morsel.payload());
    let upgraded = json.replace("TaskCreated", "TodoCreated");
    Ok(EventMorsel::new(
        "TodoCreated",
        Version::new(2).expect("2 is non-zero"),
        upgraded.into_bytes(),
    ))
}

// =============================================================================
// 4. main() — the full demo flow
// =============================================================================

#[tokio::main]
async fn main() {
    println!("=== nexus-store in-memory example ===");
    println!();

    // --- Setup ---
    let codec = JsonCodec;
    let store = InMemoryStore::new();
    let stream_id = TodoId("todo-1".to_owned());

    // --- Step 1: Create domain events ---
    println!("Step 1: Create domain events");
    let events = vec![
        TodoEvent::Created {
            id: "todo-1".to_owned(),
            title: "Buy milk".to_owned(),
        },
        TodoEvent::Completed {
            id: "todo-1".to_owned(),
        },
        TodoEvent::Deleted {
            id: "todo-1".to_owned(),
        },
    ];
    for event in &events {
        println!("  {event:?}");
    }
    println!();

    // --- Step 2: Encode events with the codec ---
    println!("Step 2: Encode events with JsonCodec");
    let mut encoded: Vec<(bytes::Bytes, &'static str)> = Vec::new();
    for event in &events {
        let bytes = codec.encode(event).expect("encode should succeed");
        let event_type = event.event_type();
        println!(
            "  {event_type} -> {} bytes: {}",
            bytes.len(),
            String::from_utf8_lossy(&bytes)
        );
        encoded.push((bytes, event_type));
    }
    println!();

    // --- Step 3: Build PendingEnvelopes ---
    println!("Step 3: Build PendingEnvelopes (typestate builder)");
    let mut envelopes: Vec<nexus_store::envelope::PendingEnvelope> = Vec::new();
    for (i, (payload, event_type)) in encoded.iter().enumerate() {
        let version_num = u64::try_from(i + 1).expect("version should fit in u64");
        let envelope = pending_envelope(Version::new(version_num).expect("version > 0"))
            .event_type(event_type)
            .payload(payload.clone())
            .build();
        println!(
            "  Envelope: version={}, type={}",
            envelope.version(),
            envelope.event_type()
        );
        envelopes.push(envelope);
    }
    println!();

    // --- Step 4: Append to the store ---
    println!("Step 4: Append envelopes to InMemoryStore");
    store
        .append(&stream_id, None, &envelopes)
        .await
        .expect("append should succeed");
    println!(
        "  Appended {} events to stream '{stream_id}'",
        envelopes.len(),
    );
    println!();

    // --- Bonus: Simulate a legacy event for upcasting ---
    println!("Step 4b: Simulate a legacy 'TaskCreated' event (schema v1)");
    {
        // Build a v1 event that uses the old name "TaskCreated".
        // In a real system, these would already be in the database from
        // before the rename. Here we append it through the normal API
        // with the old event type name.
        let legacy_json = serde_json::to_vec(&TodoEvent::Created {
            id: "todo-1".to_owned(),
            title: "Legacy task from v1".to_owned(),
        })
        .expect("encode legacy event");

        // Replace "TodoCreated" with "TaskCreated" in the JSON to simulate
        // the old schema.
        let legacy_str = String::from_utf8_lossy(&legacy_json);
        let old_json = legacy_str.replace("TodoCreated", "TaskCreated");
        println!("  Raw JSON (old schema): {old_json}");

        let legacy_envelope = pending_envelope(Version::new(4).expect("version > 0"))
            .event_type("TaskCreated")
            .payload(old_json.into_bytes())
            .build();

        store
            .append(&stream_id, Version::new(3), &[legacy_envelope])
            .await
            .expect("append legacy event should succeed");
        println!("  Inserted legacy event at version 4");
    }
    println!();

    // --- Step 5: Read back from the store ---
    println!("Step 5: Read events back from InMemoryStore");
    let mut event_stream = store
        .read_stream(&stream_id, Version::INITIAL)
        .await
        .expect("read should succeed");

    let mut read_events: Vec<(String, u32, Vec<u8>, u64)> = Vec::new();
    loop {
        match event_stream.next().await {
            Err(e) => {
                println!("  Error reading event: {e}");
                break;
            }
            Ok(None) => break,
            Ok(Some(env)) => {
                let event_type = env.event_type().to_owned();
                let payload = env.payload().to_vec();
                let version = env.version().as_u64();
                let schema_version = env.schema_version();
                // The envelope borrows from the stream — we copy what we
                // need and let it drop before the next iteration.
                println!(
                    "  Read: version={version}, type={event_type}, payload={}",
                    String::from_utf8_lossy(&payload)
                );

                read_events.push((event_type, schema_version, payload, version));
            }
        }
    }
    println!("  Total events read: {}", read_events.len());
    println!();

    // --- Step 6: Upcast and decode ---
    println!("Step 6: Apply upcast function, then decode with JsonCodec");
    for (event_type, schema_version, payload, version) in &read_events {
        let schema_ver = Version::new(u64::from(*schema_version)).expect("schema version > 0");
        let morsel = EventMorsel::borrowed(event_type, schema_ver, payload);
        let upcasted = rename_upcast(morsel).expect("upcast should succeed");

        if upcasted.event_type() != event_type {
            println!(
                "  [UPCAST] version={version}: '{event_type}' v{schema_version} -> '{}'",
                upcasted.event_type()
            );
        }

        let upcast_env =
            nexus_store::PersistedEnvelope::for_decode(upcasted.event_type(), upcasted.payload())
                .expect("wire build_row ok");
        let decoded = codec.decode(&upcast_env).expect("decode should succeed");
        println!("  version={version}: {decoded:?}");
    }
    println!();

    // --- Step 7: Substrate path ---
    //
    // Everything above used the high-level `RawEventStore` interface
    // directly. For the same flow expressed against the composable
    // *substrate* — `Store::raw()` exposing the underlying cursor, then
    // `EventStreamExt::map_err` + `try_fold` composing a typed pipeline
    // — see this section. This is the escape hatch power users reach
    // for when `EventStore::load` / `load_with` isn't expressive enough
    // (custom filtering, peeking, branching during load, etc.).
    println!("Step 7: Substrate path — Store::raw() + map_err + try_fold");
    let typed_store = Store::new(InMemoryStore::new());

    // Pre-populate via a separate `RawEventStore` reference.
    let payload = codec
        .encode(&TodoEvent::Created {
            id: "todo-2".to_owned(),
            title: "Substrate task".to_owned(),
        })
        .expect("encode should succeed");
    let envelope = pending_envelope(Version::INITIAL)
        .event_type("TodoCreated")
        .payload(payload)
        .build();
    typed_store
        .raw()
        .append(&TodoId("todo-2".to_owned()), None, &[envelope])
        .await
        .expect("append should succeed");

    // The substrate chain: open a raw cursor, lift the adapter error into
    // our domain error type, then fold the events into a count + last
    // title using the lending `try_fold` combinator.
    #[derive(Debug, thiserror::Error)]
    enum SubstrateErr {
        #[error("stream error: {0}")]
        Stream(String),
        #[error("decode error: {0}")]
        Decode(String),
    }
    let raw_stream = typed_store
        .raw()
        .read_stream(&TodoId("todo-2".to_owned()), Version::INITIAL)
        .await
        .expect("read_stream should succeed");
    let (count, last_title): (usize, Option<String>) = raw_stream
        .map_err(|e| SubstrateErr::Stream(e.to_string()))
        .try_fold((0usize, None::<String>), |(c, _last), env| {
            let event = codec
                .decode(&env)
                .map_err(|e| SubstrateErr::Decode(e.to_string()))?;
            let title = match event {
                TodoEvent::Created { title, .. } => Some(title),
                _ => None,
            };
            Ok::<_, SubstrateErr>((c + 1, title))
        })
        .await
        .expect("substrate pipeline should succeed");
    println!("  Substrate fold: count={count}, last_title={last_title:?}");
    println!();

    println!("=== Done! All nexus-store traits demonstrated. ===");
}
