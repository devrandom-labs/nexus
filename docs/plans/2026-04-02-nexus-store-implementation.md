# nexus-store Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the nexus-store crate — the edge layer between kernel aggregates and databases, with zero-allocation reads via GAT lending cursors, pluggable codecs, and schema evolution upcasters.

**Architecture:** Two envelope types (PendingEnvelope for writes, PersistedEnvelope for reads), four traits (Codec, EventUpcaster, EventStream, RawEventStore), and one typed facade (EventStore). Adapters implement RawEventStore; users interact with EventStore which handles serialization and upcasting internally.

**Tech Stack:** Rust edition 2024, nexus (kernel), thiserror

**Design docs:**
- `docs/plans/2026-04-02-nexus-store-design.md`
- `docs/plans/2026-04-02-nexus-store-envelope-design.md`

---

### Task 1: Create nexus-store crate scaffold

**Files:**
- Create: `crates/nexus-store/Cargo.toml`
- Create: `crates/nexus-store/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

**Step 1: Create Cargo.toml**

Create `crates/nexus-store/Cargo.toml`:
```toml
[package]
name = "nexus-store"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Event store edge layer for the Nexus event-sourcing framework"
readme = "../../README.md"
keywords = ["event-sourcing", "event-store", "cqrs", "persistence"]
categories = ["data-structures"]

[dependencies]
nexus = { version = "0.1.0", path = "../nexus" }
thiserror = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
static_assertions = { workspace = true }
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }

[lints]
workspace = true
```

**Step 2: Create empty lib.rs**

Create `crates/nexus-store/src/lib.rs`:
```rust
pub mod codec;
pub mod envelope;
pub mod error;
pub mod stream;
pub mod upcaster;
```

Create empty module files:
- `crates/nexus-store/src/codec.rs`
- `crates/nexus-store/src/envelope.rs`
- `crates/nexus-store/src/error.rs`
- `crates/nexus-store/src/stream.rs`
- `crates/nexus-store/src/upcaster.rs`

**Step 3: Add to workspace**

Add `"crates/nexus-store"` to workspace members in root `Cargo.toml`.

**Step 4: Verify**

Run: `nix develop -c cargo check -p nexus-store`
Expected: compiles

**Step 5: Regenerate hakari**

Run: `nix develop -c cargo hakari generate && nix develop -c cargo hakari manage-deps`

**Step 6: Commit**

```
feat(store): scaffold nexus-store crate
```

---

### Task 2: Implement PendingEnvelope<M>

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`
- Create: `crates/nexus-store/tests/envelope_tests.rs`

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/envelope_tests.rs`:
```rust
use nexus::Version;
use nexus_store::envelope::PendingEnvelope;

#[test]
fn pending_envelope_accessors() {
    let envelope = PendingEnvelope::<()>::new(
        "user-123".to_string(),
        Version::from_persisted(1),
        "UserCreated",
        vec![1, 2, 3],
        (),
    );

    assert_eq!(envelope.stream_id(), "user-123");
    assert_eq!(envelope.version(), Version::from_persisted(1));
    assert_eq!(envelope.event_type(), "UserCreated");
    assert_eq!(envelope.payload(), &[1, 2, 3]);
}

#[test]
fn pending_envelope_with_metadata() {
    #[derive(Debug, Clone, PartialEq)]
    struct Meta {
        correlation_id: String,
    }

    let meta = Meta {
        correlation_id: "corr-1".into(),
    };
    let envelope = PendingEnvelope::new(
        "order-1".to_string(),
        Version::from_persisted(1),
        "OrderPlaced",
        vec![4, 5, 6],
        meta.clone(),
    );

    assert_eq!(envelope.metadata(), &meta);
}

#[test]
fn pending_envelope_unit_metadata_default() {
    let envelope = PendingEnvelope::new(
        "stream".to_string(),
        Version::from_persisted(1),
        "Event",
        vec![],
        (),
    );
    assert_eq!(envelope.metadata(), &());
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --test envelope_tests`
Expected: FAIL — PendingEnvelope not found

**Step 3: Implement PendingEnvelope**

Write `crates/nexus-store/src/envelope.rs`:
```rust
use nexus::Version;

/// Event envelope for the write path — fully owned, going to the database.
///
/// Constructed internally by the `EventStore` facade from typed events
/// and codec output. All fields are private with read-only accessors.
///
/// `M` is user-defined metadata (correlation ID, tenant ID, timestamps, etc.).
/// Default is `()` for bare minimum storage.
#[derive(Debug)]
pub struct PendingEnvelope<M = ()> {
    stream_id: String,
    version: Version,
    event_type: &'static str,
    payload: Vec<u8>,
    metadata: M,
}

impl<M> PendingEnvelope<M> {
    /// Construct a new pending envelope.
    ///
    /// This is intentionally public for adapter testing. In production,
    /// the `EventStore` facade creates envelopes internally.
    #[must_use]
    pub fn new(
        stream_id: String,
        version: Version,
        event_type: &'static str,
        payload: Vec<u8>,
        metadata: M,
    ) -> Self {
        Self {
            stream_id,
            version,
            event_type,
            payload,
            metadata,
        }
    }

    #[must_use]
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub fn event_type(&self) -> &'static str {
        self.event_type
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    #[must_use]
    pub fn metadata(&self) -> &M {
        &self.metadata
    }
}
```

**Step 4: Run tests**

Run: `nix develop -c cargo test -p nexus-store --test envelope_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add PendingEnvelope<M> with private fields and accessors
```

---

### Task 3: Implement PersistedEnvelope<'a, M>

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`
- Modify: `crates/nexus-store/tests/envelope_tests.rs`

**Step 1: Write failing tests**

Add to `crates/nexus-store/tests/envelope_tests.rs`:
```rust
use nexus_store::envelope::PersistedEnvelope;

#[test]
fn persisted_envelope_borrows_from_source() {
    let stream_id = String::from("user-456");
    let event_type = String::from("UserActivated");
    let payload = vec![10, 20, 30];

    let envelope = PersistedEnvelope::<()>::new(
        &stream_id,
        Version::from_persisted(3),
        &event_type,
        &payload,
        (),
    );

    assert_eq!(envelope.stream_id(), "user-456");
    assert_eq!(envelope.version(), Version::from_persisted(3));
    assert_eq!(envelope.event_type(), "UserActivated");
    assert_eq!(envelope.payload(), &[10, 20, 30]);
}

#[test]
fn persisted_envelope_metadata_is_owned() {
    #[derive(Debug, Clone, PartialEq)]
    struct Meta {
        tenant: String,
    }

    let stream_id = "s";
    let event_type = "E";
    let payload = [1u8];

    let envelope = PersistedEnvelope::new(
        stream_id,
        Version::from_persisted(1),
        event_type,
        &payload,
        Meta {
            tenant: "acme".into(),
        },
    );

    assert_eq!(
        envelope.metadata(),
        &Meta {
            tenant: "acme".into()
        }
    );
}

#[test]
fn persisted_envelope_zero_allocation_for_core_fields() {
    // This test documents the design intent: core fields borrow,
    // only metadata is owned. The envelope itself allocates nothing
    // for stream_id, event_type, or payload.
    let source_stream = "my-stream";
    let source_type = "MyEvent";
    let source_payload = [1u8, 2, 3, 4, 5];

    let envelope = PersistedEnvelope::<()>::new(
        source_stream,
        Version::from_persisted(1),
        source_type,
        &source_payload,
        (),
    );

    // Verify the envelope borrows from the source —
    // the pointers should point into the same memory
    assert!(std::ptr::eq(envelope.stream_id().as_bytes().as_ptr(), source_stream.as_ptr()));
    assert!(std::ptr::eq(envelope.payload().as_ptr(), source_payload.as_ptr()));
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --test envelope_tests`
Expected: FAIL — PersistedEnvelope not found

**Step 3: Implement PersistedEnvelope**

Add to `crates/nexus-store/src/envelope.rs`:
```rust
/// Event envelope for the read path — borrows from database row buffer.
///
/// Zero allocation for core fields (stream_id, event_type, payload).
/// Metadata `M` is always owned (small data like UUIDs, timestamps).
///
/// Constructed internally when reading from the database.
/// The lifetime `'a` ties the envelope to the row buffer — the envelope
/// must be dropped before the cursor advances to the next row.
#[derive(Debug)]
pub struct PersistedEnvelope<'a, M = ()> {
    stream_id: &'a str,
    version: Version,
    event_type: &'a str,
    payload: &'a [u8],
    metadata: M,
}

impl<'a, M> PersistedEnvelope<'a, M> {
    /// Construct a new persisted envelope.
    #[must_use]
    pub fn new(
        stream_id: &'a str,
        version: Version,
        event_type: &'a str,
        payload: &'a [u8],
        metadata: M,
    ) -> Self {
        Self {
            stream_id,
            version,
            event_type,
            payload,
            metadata,
        }
    }

    #[must_use]
    pub fn stream_id(&self) -> &str {
        self.stream_id
    }

    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub fn event_type(&self) -> &str {
        self.event_type
    }

    #[must_use]
    pub fn payload(&self) -> &[u8] {
        self.payload
    }

    #[must_use]
    pub fn metadata(&self) -> &M {
        &self.metadata
    }
}
```

**Step 4: Run tests**

Run: `nix develop -c cargo test -p nexus-store --test envelope_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add PersistedEnvelope<'a, M> with zero-alloc borrowed fields
```

---

### Task 4: Implement Codec trait

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`
- Create: `crates/nexus-store/tests/codec_tests.rs`

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/codec_tests.rs`:
```rust
use nexus::*;
use nexus_store::codec::Codec;

// A simple test codec that "serializes" by Debug formatting
// and "deserializes" by matching event type names
#[derive(Clone)]
struct DebugCodec;

#[derive(Debug, Clone, PartialEq)]
enum TestEvent {
    Created(String),
    Deleted,
}
impl Message for TestEvent {}
impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::Created(_) => "Created",
            Self::Deleted => "Deleted",
        }
    }
}

impl Codec for DebugCodec {
    type Error = std::io::Error;

    fn encode<E: DomainEvent>(&self, event: &E) -> Result<Vec<u8>, Self::Error> {
        Ok(format!("{event:?}").into_bytes())
    }

    fn decode<E: DomainEvent>(&self, _event_type: &str, _payload: &[u8]) -> Result<E, Self::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "DebugCodec can only encode, not decode",
        ))
    }
}

#[test]
fn codec_encode_produces_bytes() {
    let codec = DebugCodec;
    let event = TestEvent::Created("hello".into());
    let bytes = codec.encode(&event).unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn codec_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DebugCodec>();
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --test codec_tests`
Expected: FAIL — Codec not found

**Step 3: Implement Codec trait**

Write `crates/nexus-store/src/codec.rs`:
```rust
use nexus::DomainEvent;

/// Pluggable serialization for domain events.
///
/// Converts between typed events and byte payloads. Monomorphized via
/// type parameter on `EventStore` for zero-cost at the call site.
///
/// Knows nothing about envelopes, streams, versions, or metadata.
/// Just bytes in, bytes out.
///
/// Users implement this for their chosen format (serde_json, postcard,
/// musli, protobuf, rkyv, etc.).
pub trait Codec: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a typed domain event to bytes.
    fn encode<E: DomainEvent>(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Deserialize bytes back to a typed domain event.
    ///
    /// `event_type` is the variant name (from `DomainEvent::name()`),
    /// provided so the codec knows which variant to construct.
    fn decode<E: DomainEvent>(&self, event_type: &str, payload: &[u8]) -> Result<E, Self::Error>;
}
```

**Step 4: Run tests**

Run: `nix develop -c cargo test -p nexus-store --test codec_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add Codec trait for pluggable event serialization
```

---

### Task 5: Implement EventUpcaster trait

**Files:**
- Modify: `crates/nexus-store/src/upcaster.rs`
- Create: `crates/nexus-store/tests/upcaster_tests.rs`

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/upcaster_tests.rs`:
```rust
use nexus_store::upcaster::EventUpcaster;

struct RenameUpcaster;

impl EventUpcaster for RenameUpcaster {
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool {
        event_type == "UserCreated" && schema_version < 2
    }

    fn upcast(
        &self,
        _event_type: &str,
        _schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>) {
        // V1 → V2: rename event type, keep payload
        ("UserRegistered".to_string(), 2, payload.to_vec())
    }
}

#[test]
fn upcaster_identifies_upgradeable_events() {
    let upcaster = RenameUpcaster;
    assert!(upcaster.can_upcast("UserCreated", 1));
    assert!(!upcaster.can_upcast("UserCreated", 2));
    assert!(!upcaster.can_upcast("OrderPlaced", 1));
}

#[test]
fn upcaster_transforms_event() {
    let upcaster = RenameUpcaster;
    let payload = b"original payload";
    let (new_type, new_version, new_payload) =
        upcaster.upcast("UserCreated", 1, payload);

    assert_eq!(new_type, "UserRegistered");
    assert_eq!(new_version, 2);
    assert_eq!(new_payload, payload);
}

#[test]
fn upcaster_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RenameUpcaster>();
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --test upcaster_tests`
Expected: FAIL — EventUpcaster not found

**Step 3: Implement EventUpcaster**

Write `crates/nexus-store/src/upcaster.rs`:
```rust
/// Schema evolution via raw byte transformation.
///
/// Operates on raw bytes BEFORE codec deserialization. Transforms old
/// event schemas to new ones without needing old Rust types.
///
/// Upcasters are chained: V1 → V2 → V3. Applied in order during reads.
/// Writes always use the current schema version.
pub trait EventUpcaster: Send + Sync {
    /// Check if this upcaster handles the given event type and version.
    fn can_upcast(&self, event_type: &str, schema_version: u32) -> bool;

    /// Transform the event payload. Returns new (event_type, schema_version, payload).
    ///
    /// Only called when `can_upcast` returned true.
    fn upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> (String, u32, Vec<u8>);
}
```

**Step 4: Run tests**

Run: `nix develop -c cargo test -p nexus-store --test upcaster_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add EventUpcaster trait for schema evolution
```

---

### Task 6: Implement EventStream trait (GAT lending cursor)

**Files:**
- Modify: `crates/nexus-store/src/stream.rs`
- Create: `crates/nexus-store/tests/stream_tests.rs`

**Step 1: Write failing tests**

Create `crates/nexus-store/tests/stream_tests.rs`:
```rust
use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

/// In-memory test stream that yields from a Vec of owned data.
struct VecStream {
    rows: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

impl VecStream {
    fn new(rows: Vec<(String, u64, String, Vec<u8>)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl EventStream for VecStream {
    type Error = std::convert::Infallible;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.rows.len() {
            return None;
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(
            &row.0,
            Version::from_persisted(row.1),
            &row.2,
            &row.3,
            (),
        )))
    }
}

#[tokio::test]
async fn event_stream_yields_envelopes() {
    let mut stream = VecStream::new(vec![
        ("s1".into(), 1, "Created".into(), vec![1]),
        ("s1".into(), 2, "Updated".into(), vec![2]),
    ]);

    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(e1.stream_id(), "s1");
    assert_eq!(e1.version(), Version::from_persisted(1));
    assert_eq!(e1.event_type(), "Created");

    let e2 = stream.next().await.unwrap().unwrap();
    assert_eq!(e2.version(), Version::from_persisted(2));
    assert_eq!(e2.event_type(), "Updated");

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn event_stream_empty() {
    let mut stream = VecStream::new(vec![]);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn event_stream_envelope_borrows_from_cursor() {
    let mut stream = VecStream::new(vec![
        ("stream".into(), 1, "Event".into(), vec![42]),
    ]);

    let envelope = stream.next().await.unwrap().unwrap();
    // Envelope borrows from the stream's internal data
    assert_eq!(envelope.payload(), &[42]);
    // Drop envelope before advancing — this is the GAT contract
    drop(envelope);

    assert!(stream.next().await.is_none());
}
```

**Step 2: Run tests to verify they fail**

Run: `nix develop -c cargo test -p nexus-store --test stream_tests`
Expected: FAIL — EventStream not found

**Step 3: Implement EventStream**

Write `crates/nexus-store/src/stream.rs`:
```rust
use crate::envelope::PersistedEnvelope;

/// GAT lending cursor for zero-allocation event streaming.
///
/// Each call to `next()` returns a `PersistedEnvelope` that borrows
/// from the cursor's internal buffer. The previous envelope must be
/// dropped before calling `next()` again — enforced by the GAT lifetime.
///
/// Used during aggregate rehydration where events are processed
/// one at a time (apply to state, drop, advance cursor).
pub trait EventStream<M = ()> {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Advance the cursor and return the next event envelope.
    ///
    /// Returns `None` when the stream is exhausted.
    /// The returned envelope borrows from `self` — drop it before
    /// calling `next()` again.
    fn next(&mut self) -> impl std::future::Future<Output = Option<Result<PersistedEnvelope<'_, M>, Self::Error>>> + Send;
}
```

**Step 4: Run tests**

Run: `nix develop -c cargo test -p nexus-store --test stream_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add EventStream GAT trait for zero-alloc lending cursor
```

---

### Task 7: Implement StoreError and RawEventStore trait

**Files:**
- Modify: `crates/nexus-store/src/error.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Create: `crates/nexus-store/tests/raw_store_tests.rs`

**Step 1: Implement StoreError**

Write `crates/nexus-store/src/error.rs`:
```rust
use nexus::error::ErrorId;
use nexus::Version;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("Concurrency conflict on '{stream_id}': expected version {expected}, actual {actual}")]
    Conflict {
        stream_id: ErrorId,
        expected: Version,
        actual: Version,
    },

    #[error("Stream '{stream_id}' not found")]
    StreamNotFound {
        stream_id: ErrorId,
    },

    #[error("Codec error: {0}")]
    Codec(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("Adapter error: {0}")]
    Adapter(#[source] Box<dyn std::error::Error + Send + Sync>),
}
```

**Step 2: Implement RawEventStore trait**

Add to `crates/nexus-store/src/lib.rs` or a dedicated `raw.rs`:

Create `crates/nexus-store/src/raw.rs`:
```rust
use crate::envelope::PendingEnvelope;
use crate::stream::EventStream;
use nexus::Version;

/// What database adapters implement. Bytes in, bytes out.
///
/// Knows nothing about typed events or codecs. The `EventStore` facade
/// calls this trait after encoding events into `PendingEnvelope`.
pub trait RawEventStore<M = ()>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a where Self: 'a;

    /// Append events to a stream with optimistic concurrency.
    fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;

    /// Open a lending cursor to read events from a stream.
    fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::Stream<'_>, Self::Error>> + Send;
}
```

**Step 3: Write tests with in-memory adapter**

Create `crates/nexus-store/tests/raw_store_tests.rs`:
```rust
use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use std::collections::HashMap;
use std::sync::Mutex;

/// Minimal in-memory adapter for testing RawEventStore.
struct InMemoryRawStore {
    streams: Mutex<HashMap<String, Vec<(u64, String, Vec<u8>)>>>,
}

impl InMemoryRawStore {
    fn new() -> Self {
        Self {
            streams: Mutex::new(HashMap::new()),
        }
    }
}

struct InMemoryStream {
    events: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

impl EventStream for InMemoryStream {
    type Error = std::convert::Infallible;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(
            &row.0,
            Version::from_persisted(row.1),
            &row.2,
            &row.3,
            (),
        )))
    }
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("conflict")]
    Conflict,
}

impl RawEventStore for InMemoryRawStore {
    type Error = TestError;
    type Stream<'a> = InMemoryStream where Self: 'a;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), Self::Error> {
        let mut streams = self.streams.lock().unwrap();
        let stream = streams.entry(stream_id.to_string()).or_default();
        let current_version = stream.len() as u64;
        if current_version != expected_version.as_u64() {
            return Err(TestError::Conflict);
        }
        for env in envelopes {
            stream.push((
                env.version().as_u64(),
                env.event_type().to_string(),
                env.payload().to_vec(),
            ));
        }
        Ok(())
    }

    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        let streams = self.streams.lock().unwrap();
        let events = streams
            .get(stream_id)
            .map(|s| {
                s.iter()
                    .filter(|(v, _, _)| *v >= from.as_u64())
                    .map(|(v, t, p)| (stream_id.to_string(), *v, t.clone(), p.clone()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(InMemoryStream { events, pos: 0 })
    }
}

#[tokio::test]
async fn raw_store_append_and_read() {
    let store = InMemoryRawStore::new();

    let envelopes = vec![
        PendingEnvelope::new("s1".into(), Version::from_persisted(1), "Created", vec![1], ()),
        PendingEnvelope::new("s1".into(), Version::from_persisted(2), "Updated", vec![2], ()),
    ];

    store.append("s1", Version::INITIAL, &envelopes).await.unwrap();

    let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
    let e1 = stream.next().await.unwrap().unwrap();
    assert_eq!(e1.event_type(), "Created");
    drop(e1);

    let e2 = stream.next().await.unwrap().unwrap();
    assert_eq!(e2.event_type(), "Updated");
    drop(e2);

    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn raw_store_optimistic_concurrency() {
    let store = InMemoryRawStore::new();

    let e1 = vec![PendingEnvelope::new("s1".into(), Version::from_persisted(1), "E", vec![], ())];
    store.append("s1", Version::INITIAL, &e1).await.unwrap();

    // Wrong expected version — should fail
    let e2 = vec![PendingEnvelope::new("s1".into(), Version::from_persisted(2), "E", vec![], ())];
    let result = store.append("s1", Version::INITIAL, &e2).await;
    assert!(result.is_err());
}
```

**Step 4: Run tests**

Run: `nix develop -c cargo test -p nexus-store`
Expected: PASS

**Step 5: Commit**

```
feat(store): add StoreError, RawEventStore trait, and in-memory test adapter
```

---

### Task 8: Update lib.rs re-exports and static assertions

**Files:**
- Modify: `crates/nexus-store/src/lib.rs`
- Create: `crates/nexus-store/tests/static_assertions.rs`

**Step 1: Update lib.rs**

```rust
pub mod codec;
pub mod envelope;
pub mod error;
pub mod raw;
pub mod stream;
pub mod upcaster;

pub use codec::Codec;
pub use envelope::{PendingEnvelope, PersistedEnvelope};
pub use error::StoreError;
pub use raw::RawEventStore;
pub use stream::EventStream;
pub use upcaster::EventUpcaster;
```

**Step 2: Add static assertions**

Create `crates/nexus-store/tests/static_assertions.rs`:
```rust
use nexus_store::*;
use static_assertions::*;

// StoreError is a proper error
assert_impl_all!(StoreError: std::error::Error, Send, Sync, std::fmt::Debug);

// Envelope types are Send + Sync + Debug
assert_impl_all!(PendingEnvelope<()>: Send, Sync, std::fmt::Debug);
assert_impl_all!(PersistedEnvelope<'static, ()>: Send, Sync, std::fmt::Debug);

#[test]
fn static_assertions_compile() {}
```

**Step 3: Run all tests**

Run: `nix develop -c cargo test -p nexus-store`
Expected: ALL PASS

**Step 4: Commit**

```
feat(store): add re-exports and static assertions
```

---

### Summary

| Task | Component | Tests |
|------|-----------|-------|
| 1 | Crate scaffold | Compiles |
| 2 | PendingEnvelope<M> | 3 tests |
| 3 | PersistedEnvelope<'a, M> | 3 tests |
| 4 | Codec trait | 2 tests |
| 5 | EventUpcaster trait | 3 tests |
| 6 | EventStream (GAT) | 3 tests |
| 7 | RawEventStore + StoreError | 2 tests |
| 8 | Re-exports + static assertions | 1 test |

**Total: 8 tasks, ~17 tests**

**Note:** The `EventStore` facade (composes raw store + codec + upcasters) is deliberately not in this plan. It depends on all the above components and deserves its own design iteration once the foundation is tested.
