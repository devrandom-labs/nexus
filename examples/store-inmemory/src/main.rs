// examples/store-inmemory — demonstrates all nexus-store traits with an
// in-memory backend: Codec, RawEventStore, EventStream, EventUpcaster,
// and the PendingEnvelope typestate builder.
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
#![allow(
    clippy::significant_drop_tightening,
    reason = "lock guard lifetime is fine in example adapters"
)]

use std::collections::HashMap;

use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::upcaster::EventUpcaster;
use nexus_store::{Codec, pending_envelope};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

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

impl Codec<TodoEvent> for JsonCodec {
    type Error = CodecError;

    fn encode(&self, event: &TodoEvent) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(event).map_err(CodecError::Serialize)
    }

    fn decode(&self, event_type: &str, payload: &[u8]) -> Result<TodoEvent, Self::Error> {
        // In a real system you might dispatch on `event_type` to pick
        // different structs. Here our enum is self-describing via serde,
        // but we still validate the type name.
        match event_type {
            "TodoCreated" | "TodoCompleted" | "TodoDeleted" => {
                serde_json::from_slice(payload).map_err(CodecError::Deserialize)
            }
            other => Err(CodecError::UnknownType(other.to_owned())),
        }
    }
}

// =============================================================================
// 3. VecStore — implements RawEventStore with a Vec "database"
// =============================================================================

/// A single row in our in-memory "database".
///
/// The store deals only in raw bytes — no typed events here.
struct StoredRow {
    version: u64,
    event_type: String,
    #[allow(dead_code, reason = "schema_version stored for upcasting demo")]
    schema_version: u32,
    payload: Vec<u8>,
}

/// In-memory event store backed by a `HashMap<String, Vec<StoredRow>>`.
///
/// Each key is a stream ID, each value is the ordered list of events.
/// The `tokio::sync::Mutex` makes it safe for async access.
struct VecStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl VecStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

/// Store-level errors.
#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error("concurrency conflict: expected version {expected}, actual {actual}")]
    Conflict { expected: u64, actual: u64 },
}

impl RawEventStore for VecStore {
    type Error = StoreError;
    type Stream<'a>
        = VecStream
    where
        Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), Self::Error> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();

        // Optimistic concurrency check: the number of existing events
        // should match the expected version.
        let current_len = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current_len != expected_version.as_u64() {
            return Err(StoreError::Conflict {
                expected: expected_version.as_u64(),
                actual: current_len,
            });
        }

        for env in envelopes {
            stream.push(StoredRow {
                version: env.version().as_u64(),
                event_type: env.event_type().to_owned(),
                schema_version: 1, // current schema version
                payload: env.payload().to_vec(),
            });
        }

        drop(guard);
        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let guard = self.streams.lock().await;
        let events = guard
            .get(stream_id)
            .map(|s| {
                s.iter()
                    .filter(|row| row.version >= from.as_u64())
                    .map(|row| ReadRow {
                        stream_id: stream_id.to_owned(),
                        version: row.version,
                        event_type: row.event_type.clone(),
                        payload: row.payload.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        drop(guard);
        Ok(VecStream { events, pos: 0 })
    }
}

// =============================================================================
// 4. VecStream — implements EventStream for reading
// =============================================================================

/// An owned copy of a row, used by the stream cursor.
///
/// We copy data out of the locked store so the cursor can outlive the
/// lock. In a real DB adapter you'd hold a cursor/statement instead.
struct ReadRow {
    stream_id: String,
    version: u64,
    event_type: String,
    payload: Vec<u8>,
}

/// Lending cursor over in-memory events.
///
/// Implements `EventStream` — each call to `next()` returns a
/// `PersistedEnvelope` that borrows from the internal buffer.
struct VecStream {
    events: Vec<ReadRow>,
    pos: usize,
}

impl EventStream for VecStream {
    type Error = StoreError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(
            &row.stream_id,
            Version::from_persisted(row.version),
            &row.event_type,
            1,
            &row.payload,
            (),
        )))
    }
}

// =============================================================================
// 5. RenameUpcaster — schema evolution demo
// =============================================================================

/// Upcaster that renames `"TaskCreated"` to `"TodoCreated"`.
///
/// Imagine we originally called the event `TaskCreated` (v1) and later
/// renamed it to `TodoCreated` (v2). Old events stored as `TaskCreated`
/// are transparently upgraded during reads.
///
/// Upcasters operate on raw bytes BEFORE the codec deserializes — so
/// the old Rust type doesn't need to exist.
struct RenameUpcaster;

impl EventUpcaster for RenameUpcaster {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        event_type == "TaskCreated" && schema_version < 2
    }

    fn upcast(
        &self,
        _event_type: &str,
        _schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        // Rename the event type. The payload JSON uses serde's
        // tagged enum format, so we also need to rename the tag
        // inside the JSON body.
        let json = String::from_utf8_lossy(payload);
        let upgraded = json.replace("TaskCreated", "TodoCreated");
        ("TodoCreated".to_owned(), 2, upgraded.into_bytes())
    }
}

// =============================================================================
// 6. main() — the full demo flow
// =============================================================================

#[tokio::main]
async fn main() {
    println!("=== nexus-store in-memory example ===");
    println!();

    // --- Setup ---
    let codec = JsonCodec;
    let store = VecStore::new();
    let upcaster = RenameUpcaster;
    let stream_id = "todo-1";

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
    let mut encoded: Vec<(Vec<u8>, &'static str)> = Vec::new();
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
    let mut envelopes: Vec<PendingEnvelope<()>> = Vec::new();
    for (i, (payload, event_type)) in encoded.iter().enumerate() {
        let version_num = u64::try_from(i + 1).expect("version should fit in u64");
        let envelope = pending_envelope(stream_id.into())
            .version(Version::from_persisted(version_num))
            .event_type(event_type)
            .payload(payload.clone())
            .build_without_metadata();
        println!(
            "  Envelope: stream={}, version={}, type={}",
            envelope.stream_id(),
            envelope.version(),
            envelope.event_type()
        );
        envelopes.push(envelope);
    }
    println!();

    // --- Step 4: Append to the store ---
    println!("Step 4: Append envelopes to VecStore");
    store
        .append(stream_id, Version::INITIAL, &envelopes)
        .await
        .expect("append should succeed");
    println!(
        "  Appended {} events to stream '{stream_id}'",
        envelopes.len()
    );
    println!();

    // --- Bonus: Simulate a legacy event for upcasting ---
    println!("Step 4b: Simulate a legacy 'TaskCreated' event (schema v1)");
    {
        // Manually insert a v1 event that uses the old name "TaskCreated".
        // In a real system, these would already be in the database from
        // before the rename.
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

        let mut guard = store.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        stream.push(StoredRow {
            version: u64::try_from(stream.len() + 1).unwrap_or(u64::MAX),
            event_type: "TaskCreated".to_owned(),
            schema_version: 1,
            payload: old_json.into_bytes(),
        });
        drop(guard);
        println!("  Inserted legacy event at version 4");
    }
    println!();

    // --- Step 5: Read back from the store ---
    println!("Step 5: Read events back from VecStore");
    let mut event_stream = store
        .read_stream(stream_id, Version::INITIAL)
        .await
        .expect("read_stream should succeed");

    let mut read_events: Vec<(String, u32, Vec<u8>, u64)> = Vec::new();
    loop {
        let item = event_stream.next().await;
        match item {
            None => break,
            Some(Ok(env)) => {
                let event_type = env.event_type().to_owned();
                let payload = env.payload().to_vec();
                let version = env.version().as_u64();
                // The envelope borrows from the stream — we copy what we
                // need and let it drop before the next iteration.
                println!(
                    "  Read: version={version}, type={event_type}, payload={}",
                    String::from_utf8_lossy(&payload)
                );

                // In our simple store we track schema_version=1 for all
                // rows. A real store would persist this per-event.
                read_events.push((event_type, 1, payload, version));
            }
            Some(Err(e)) => {
                println!("  Error reading event: {e}");
                break;
            }
        }
    }
    println!("  Total events read: {}", read_events.len());
    println!();

    // --- Step 6: Upcast and decode ---
    println!("Step 6: Apply upcaster, then decode with JsonCodec");
    for (event_type, schema_version, payload, version) in &read_events {
        let (final_type, _final_version, final_payload) =
            if upcaster.can_upcast(event_type, *schema_version) {
                println!("  [UPCAST] version={version}: '{event_type}' v{schema_version} -> ...");
                let result = upcaster.upcast(event_type, *schema_version, payload);
                println!("           ... '{}' v{}", result.0, result.1);
                result
            } else {
                (event_type.clone(), *schema_version, payload.clone())
            };

        let decoded = codec
            .decode(&final_type, &final_payload)
            .expect("decode should succeed");
        println!("  version={version}: {decoded:?}");
    }
    println!();

    println!("=== Done! All nexus-store traits demonstrated. ===");
}
