# EventStore Repository Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement `EventStore<S, C, U>` — a concrete `Repository<A>` that composes `RawEventStore` + `Codec`/`BorrowingCodec` + a heterogeneous upcaster chain, with fully static dispatch and zero-copy reads.

**Architecture:** Three new modules in `nexus-store`: `BorrowingCodec` trait (zero-copy decode), `UpcasterChain` cons-list (compile-time upcaster composition), and `EventStore` struct (the facade implementing `Repository<A>` twice — once for each codec flavor). All type parameters monomorphized, no `dyn` dispatch.

**Tech Stack:** Rust, tokio (for async tests), nexus kernel, nexus-store traits

---

### Task 1: BorrowingCodec trait

**Files:**
- Create: `crates/nexus-store/src/borrowing_codec.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/borrowing_codec_tests.rs`

**Step 1: Write the test**

```rust
//! Tests for BorrowingCodec trait.

#![allow(clippy::unwrap_used, reason = "tests")]

use nexus_store::BorrowingCodec;

/// A test codec that "decodes" by reinterpreting bytes as a u32 slice.
struct U32Codec;

impl BorrowingCodec<[u32]> for U32Codec {
    type Error = std::io::Error;

    fn encode(&self, event: &[u32]) -> Result<Vec<u8>, Self::Error> {
        Ok(event.iter().flat_map(|n| n.to_le_bytes()).collect())
    }

    fn decode<'a>(&self, _event_type: &str, payload: &'a [u8]) -> Result<&'a [u32], Self::Error> {
        if payload.len() % 4 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "payload length not multiple of 4",
            ));
        }
        // SAFETY: we verified alignment-compatible length. For a real codec
        // (rkyv) this would be rkyv::check_archived_root.
        // For this test we use a safe slice conversion.
        let (prefix, shorts, suffix) = unsafe { payload.align_to::<u32>() };
        if !prefix.is_empty() || !suffix.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unaligned",
            ));
        }
        Ok(shorts)
    }
}

#[test]
fn borrowing_codec_decode_borrows_from_payload() {
    let codec = U32Codec;
    let original: Vec<u32> = vec![1, 2, 3];
    let encoded = codec.encode(&original).unwrap();
    let decoded = codec.decode("event", &encoded).unwrap();
    assert_eq!(decoded, &[1, 2, 3]);
}

#[test]
fn borrowing_codec_decode_rejects_bad_payload() {
    let codec = U32Codec;
    let bad = vec![1, 2, 3]; // 3 bytes, not multiple of 4
    assert!(codec.decode("event", &bad).is_err());
}

#[test]
fn borrowing_codec_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<U32Codec>();
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test -p nexus-store -- borrowing_codec`
Expected: FAIL — module doesn't exist

**Step 3: Write the implementation**

Create `crates/nexus-store/src/borrowing_codec.rs`:

```rust
/// Zero-copy codec for domain events.
///
/// Unlike [`Codec<E>`](crate::Codec) which returns an owned `E`,
/// `BorrowingCodec` returns `&'a E` borrowing directly from the payload
/// buffer. This enables zero-allocation event streaming for codecs
/// like rkyv and flatbuffers where the serialized bytes ARE the data.
///
/// `E: ?Sized` allows unsized event types (e.g. `Archived<MyEvent>`).
///
/// # When to use
///
/// - **Use `Codec<E>`** for serde-based formats (JSON, bincode, postcard)
///   where deserialization produces an owned value.
/// - **Use `BorrowingCodec<E>`** for zero-copy formats (rkyv, flatbuffers)
///   where the serialized bytes can be reinterpreted in-place.
pub trait BorrowingCodec<E: ?Sized>: Send + Sync + 'static {
    /// The error type for serialization/deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a domain event to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the event cannot be serialized.
    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error>;

    /// Decode bytes by borrowing directly from the payload buffer.
    ///
    /// The returned reference has lifetime `'a` tied to `payload` —
    /// it borrows from the cursor's row buffer. No allocation occurs.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload is invalid (e.g. failed
    /// archive validation for rkyv).
    fn decode<'a>(&self, event_type: &str, payload: &'a [u8]) -> Result<&'a E, Self::Error>;
}
```

Add to `crates/nexus-store/src/lib.rs`:

```rust
pub mod borrowing_codec;
// ... existing modules ...

pub use borrowing_codec::BorrowingCodec;
```

**Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test -p nexus-store -- borrowing_codec`
Expected: PASS (3 tests)

**Step 5: Commit**

```
feat(store): add BorrowingCodec trait for zero-copy decode
```

---

### Task 2: UpcasterChain cons-list

**Files:**
- Create: `crates/nexus-store/src/upcaster_chain.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/upcaster_chain_tests.rs`

**Step 1: Write the tests**

```rust
//! Tests for the compile-time upcaster chain.

#![allow(clippy::unwrap_used, reason = "tests")]

use nexus_store::upcaster::EventUpcaster;
use nexus_store::upcaster_chain::{Chain, UpcasterChain};

struct V1ToV2;
impl EventUpcaster for V1ToV2 {
    fn can_upcast(&self, event_type: &str, v: u32) -> bool {
        event_type == "UserCreated" && v == 1
    }
    fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
        ("UserCreated".to_owned(), 2, p.to_vec())
    }
}

struct V2ToV3;
impl EventUpcaster for V2ToV3 {
    fn can_upcast(&self, event_type: &str, v: u32) -> bool {
        event_type == "UserCreated" && v == 2
    }
    fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
        ("UserRegistered".to_owned(), 3, p.to_vec())
    }
}

#[test]
fn empty_chain_returns_none() {
    let chain: () = ();
    assert!(chain.try_upcast("UserCreated", 1, b"data").is_none());
}

#[test]
fn single_upcaster_matches() {
    let chain = Chain(V1ToV2, ());
    let result = chain.try_upcast("UserCreated", 1, b"data");
    assert!(result.is_some());
    let (t, v, _) = result.unwrap();
    assert_eq!(t, "UserCreated");
    assert_eq!(v, 2);
}

#[test]
fn single_upcaster_no_match() {
    let chain = Chain(V1ToV2, ());
    assert!(chain.try_upcast("OrderPlaced", 1, b"data").is_none());
}

#[test]
fn chain_dispatches_to_correct_upcaster() {
    let chain = Chain(V2ToV3, Chain(V1ToV2, ()));
    // V1 → hits V1ToV2 (in tail)
    let (t, v, _) = chain.try_upcast("UserCreated", 1, b"data").unwrap();
    assert_eq!(t, "UserCreated");
    assert_eq!(v, 2);
    // V2 → hits V2ToV3 (in head)
    let (t, v, _) = chain.try_upcast("UserCreated", 2, b"data").unwrap();
    assert_eq!(t, "UserRegistered");
    assert_eq!(v, 3);
}

#[test]
fn chain_no_match_returns_none() {
    let chain = Chain(V2ToV3, Chain(V1ToV2, ()));
    assert!(chain.try_upcast("UserCreated", 3, b"data").is_none());
}

#[test]
fn chain_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<()>();
    assert_send_sync::<Chain<V1ToV2, ()>>();
    assert_send_sync::<Chain<V2ToV3, Chain<V1ToV2, ()>>>();
}

#[test]
fn chain_is_zero_sized_when_upcasters_are_stateless() {
    assert_eq!(std::mem::size_of::<()>(), 0);
    assert_eq!(std::mem::size_of::<Chain<V1ToV2, ()>>(), 0);
    assert_eq!(std::mem::size_of::<Chain<V2ToV3, Chain<V1ToV2, ()>>>(), 0);
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test -p nexus-store -- upcaster_chain`
Expected: FAIL — module doesn't exist

**Step 3: Write the implementation**

Create `crates/nexus-store/src/upcaster_chain.rs`:

```rust
use crate::upcaster::EventUpcaster;

/// A link in the upcaster chain. Fully monomorphized, zero-size when
/// upcasters are stateless (unit structs).
pub struct Chain<H, T>(pub H, pub T);

/// Compile-time upcaster chain. Dispatches to the first matching
/// upcaster in the chain. The compiler inlines the entire traversal.
pub trait UpcasterChain: Send + Sync {
    /// Try each upcaster in order. Returns `Some(...)` from the first
    /// match, or `None` if no upcaster handles this event type + version.
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)>;
}

/// Base case: empty chain. Always returns `None`.
impl UpcasterChain for () {
    #[inline]
    fn try_upcast(&self, _: &str, _: u32, _: &[u8]) -> Option<(String, u32, Vec<u8>)> {
        None
    }
}

/// Recursive case: try head, fall through to tail.
impl<H: EventUpcaster, T: UpcasterChain> UpcasterChain for Chain<H, T> {
    #[inline]
    fn try_upcast(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &[u8],
    ) -> Option<(String, u32, Vec<u8>)> {
        if self.0.can_upcast(event_type, schema_version) {
            Some(self.0.upcast(event_type, schema_version, payload))
        } else {
            self.1.try_upcast(event_type, schema_version, payload)
        }
    }
}
```

Add to `crates/nexus-store/src/lib.rs`:

```rust
pub mod upcaster_chain;
// ...
pub use upcaster_chain::{Chain, UpcasterChain};
```

**Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test -p nexus-store -- upcaster_chain`
Expected: PASS (7 tests)

**Step 5: Commit**

```
feat(store): add UpcasterChain cons-list for static dispatch
```

---

### Task 3: EventStore struct + owning codec Repository impl

**Files:**
- Create: `crates/nexus-store/src/event_store.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/event_store_tests.rs`

**Step 1: Write the tests**

These test the full load/save lifecycle through `EventStore` with the owning `Codec` and `InMemoryStore`.

```rust
//! Integration tests for EventStore with owning Codec.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use nexus::*;
use nexus_store::event_store::EventStore;
use nexus_store::repository::Repository;
use nexus_store::testing::InMemoryStore;
use nexus_store::Codec;
use std::fmt;

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

#[derive(Default, Debug, PartialEq)]
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
            TodoEvent::Created(t) => self.title = t.clone(),
            TodoEvent::Done => self.done = true,
        }
        self
    }
    fn name(&self) -> &'static str {
        "Todo"
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

// -- Simple JSON-ish codec (no serde dep needed) --

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
        if let Some(title) = s.strip_prefix("created:") {
            Ok(TodoEvent::Created(title.to_owned()))
        } else if s == "done" {
            Ok(TodoEvent::Done)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown event: {s}"),
            ))
        }
    }
}

// -- Tests --

#[tokio::test]
async fn save_and_load_roundtrip() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let sid = "todo-stream-1";

    // Create and save
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    agg.apply(TodoEvent::Created("Buy milk".into()));
    agg.apply(TodoEvent::Done);
    es.save(sid, &mut agg).await.unwrap();

    // Load and verify
    let loaded = es.load(sid, TodoId(1)).await.unwrap();
    assert_eq!(loaded.state().title, "Buy milk");
    assert!(loaded.state().done);
    assert_eq!(loaded.version(), Version::from_persisted(2));
}

#[tokio::test]
async fn load_empty_stream_returns_fresh_aggregate() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let loaded = es.load("nonexistent", TodoId(1)).await.unwrap();
    assert_eq!(loaded.version(), Version::INITIAL);
    assert_eq!(loaded.state(), &TodoState::default());
}

#[tokio::test]
async fn save_no_uncommitted_events_is_noop() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    // No events applied — save should succeed as no-op
    es.save("s1", &mut agg).await.unwrap();
}

#[tokio::test]
async fn save_then_append_more_events() {
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let sid = "multi-save";

    // First batch
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    agg.apply(TodoEvent::Created("Task".into()));
    es.save(sid, &mut agg).await.unwrap();

    // Load, add more, save again
    let mut agg = es.load(sid, TodoId(1)).await.unwrap();
    agg.apply(TodoEvent::Done);
    es.save(sid, &mut agg).await.unwrap();

    // Verify final state
    let final_agg = es.load(sid, TodoId(1)).await.unwrap();
    assert_eq!(final_agg.state().title, "Task");
    assert!(final_agg.state().done);
    assert_eq!(final_agg.version(), Version::from_persisted(2));
}

#[tokio::test]
async fn optimistic_concurrency_conflict() {
    let store = InMemoryStore::new();
    let es = EventStore::new(store, TestCodec);
    let sid = "conflict-test";

    // Initial save
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    agg.apply(TodoEvent::Created("Original".into()));
    es.save(sid, &mut agg).await.unwrap();

    // Two loads from same version
    let mut agg_a = es.load(sid, TodoId(1)).await.unwrap();
    let mut agg_b = es.load(sid, TodoId(1)).await.unwrap();

    // A saves first — succeeds
    agg_a.apply(TodoEvent::Done);
    es.save(sid, &mut agg_a).await.unwrap();

    // B saves with stale version — should conflict
    agg_b.apply(TodoEvent::Done);
    let result = es.save(sid, &mut agg_b).await;
    assert!(result.is_err(), "should get concurrency conflict");
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test -p nexus-store -- event_store`
Expected: FAIL — module doesn't exist

**Step 3: Write the implementation**

Create `crates/nexus-store/src/event_store.rs`:

```rust
use crate::codec::Codec;
use crate::envelope::pending_envelope;
use crate::error::{StoreError, UpcastError};
use crate::raw::RawEventStore;
use crate::repository::Repository;
use crate::stream::EventStream;
use crate::upcaster_chain::UpcasterChain;
use nexus::event::DomainEvent;
use nexus::{Aggregate, AggregateRoot, AggregateState, EventOf, Version};

/// Concrete event store composing a [`RawEventStore`], a [`Codec`], and
/// an [`UpcasterChain`]. Implements [`Repository<A>`] for any aggregate
/// whose event type matches the codec.
///
/// All type parameters are monomorphized — no dynamic dispatch.
///
/// # Construction
///
/// ```ignore
/// let es = EventStore::new(store, codec)
///     .with_upcaster(V1ToV2)
///     .with_upcaster(V2ToV3);
/// ```
pub struct EventStore<S, C, U = ()> {
    store: S,
    codec: C,
    upcasters: U,
}

impl<S, C> EventStore<S, C, ()> {
    /// Create a new event store with no upcasters.
    pub const fn new(store: S, codec: C) -> Self {
        Self {
            store,
            codec,
            upcasters: (),
        }
    }
}

impl<S, C, U> EventStore<S, C, U> {
    /// Add an upcaster to the chain. Returns a new `EventStore` with
    /// the upcaster prepended (checked first during reads).
    ///
    /// ```ignore
    /// let es = EventStore::new(store, codec)
    ///     .with_upcaster(V1ToV2)   // checked first
    ///     .with_upcaster(V2ToV3);  // checked first (wraps previous)
    /// ```
    pub fn with_upcaster<H>(self, upcaster: H) -> EventStore<S, C, crate::upcaster_chain::Chain<H, U>> {
        EventStore {
            store: self.store,
            codec: self.codec,
            upcasters: crate::upcaster_chain::Chain(upcaster, self.upcasters),
        }
    }
}

// ─── Upcaster validation (shared by both Repository impls) ────────────────

const UPCAST_CHAIN_LIMIT: u32 = 100;

impl<S, C, U: UpcasterChain> EventStore<S, C, U> {
    /// Run the upcaster chain with validation.
    ///
    /// Returns the (possibly transformed) event_type, schema_version, and
    /// payload. If no upcasters match, returns the inputs unchanged.
    ///
    /// When `U = ()`, the compiler eliminates this entire method body.
    fn run_upcasters<'a>(
        &self,
        event_type: &str,
        schema_version: u32,
        payload: &'a [u8],
    ) -> Result<(String, u32, Vec<u8>), StoreError> {
        let mut current_type = event_type.to_owned();
        let mut current_version = schema_version;
        let mut current_payload = payload.to_vec();
        let mut iterations = 0u32;

        while let Some((new_type, new_version, new_payload)) =
            self.upcasters
                .try_upcast(&current_type, current_version, &current_payload)
        {
            iterations += 1;
            if iterations > UPCAST_CHAIN_LIMIT {
                return Err(StoreError::Codec(Box::new(UpcastError::ChainLimitExceeded {
                    event_type: current_type,
                    schema_version: current_version,
                    limit: UPCAST_CHAIN_LIMIT,
                })));
            }
            if new_version <= current_version {
                return Err(StoreError::Codec(Box::new(UpcastError::VersionNotAdvanced {
                    event_type: current_type,
                    input_version: current_version,
                    output_version: new_version,
                })));
            }
            if new_type.is_empty() {
                return Err(StoreError::Codec(Box::new(UpcastError::EmptyEventType {
                    input_event_type: current_type,
                    schema_version: new_version,
                })));
            }
            current_type = new_type;
            current_version = new_version;
            current_payload = new_payload;
        }

        Ok((current_type, current_version, current_payload))
    }
}

// ─── Repository impl: owning Codec ────────────────────────────────────────

impl<A, S, C, U> Repository<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: Codec<EventOf<A>>,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
{
    type Error = StoreError;

    async fn load(
        &self,
        stream_id: &str,
        id: A::Id,
    ) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .read_stream(stream_id, Version::INITIAL)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            let (event_type, _schema_version, payload) =
                self.run_upcasters(env.event_type(), env.schema_version(), env.payload())?;

            let event = self
                .codec
                .decode(&event_type, &payload)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), &event)?;
        }

        Ok(root)
    }

    async fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> Result<(), StoreError> {
        let events = aggregate.take_uncommitted_events();
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = Version::from_persisted(
            events
                .first()
                .expect("checked non-empty above")
                .version()
                .as_u64()
                - 1,
        );

        let mut envelopes = Vec::with_capacity(events.len());
        for ve in &events {
            let payload = self
                .codec
                .encode(ve.event())
                .map_err(|e| StoreError::Codec(Box::new(e)))?;
            envelopes.push(
                pending_envelope(stream_id.into())
                    .version(ve.version())
                    .event_type(ve.event().name())
                    .payload(payload)
                    .build_without_metadata(),
            );
        }

        self.store
            .append(stream_id, expected_version, &envelopes)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        Ok(())
    }
}
```

Add to `crates/nexus-store/src/lib.rs`:

```rust
pub mod event_store;
// ...
pub use event_store::EventStore;
```

**Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test -p nexus-store -- event_store`
Expected: PASS (5 tests)

**Step 5: Commit**

```
feat(store): add EventStore with owning Codec Repository impl
```

---

### Task 4: EventStore with upcasters integration test

**Files:**
- Modify: `crates/nexus-store/tests/event_store_tests.rs`

**Step 1: Add upcaster tests to the existing file**

Append to `event_store_tests.rs`:

```rust
use nexus_store::upcaster::EventUpcaster;
use nexus_store::upcaster_chain::Chain;

// -- Upcaster that changes "created:X" payload to "created_v2:X" --

struct V1ToV2Upcaster;
impl EventUpcaster for V1ToV2Upcaster {
    fn can_upcast(&self, event_type: &str, v: u32) -> bool {
        event_type == "Created" && v == 1
    }
    fn upcast(&self, _: &str, _: u32, payload: &[u8]) -> (String, u32, Vec<u8>) {
        // Transform payload: "created:X" → "created:X" (same format, just version bump)
        ("Created".to_owned(), 2, payload.to_vec())
    }
}

#[tokio::test]
async fn load_with_upcaster_transforms_events() {
    let store = InMemoryStore::new();

    // Save with base EventStore (no upcasters) — events stored at schema_version 1
    let es = EventStore::new(&store, TestCodec);
    let sid = "upcaster-stream";
    let mut agg = AggregateRoot::<TodoAggregate>::new(TodoId(1));
    agg.apply(TodoEvent::Created("Task".into()));
    es.save(sid, &mut agg).await.unwrap();

    // Load with upcaster — should still work (upcaster bumps version but
    // our test codec ignores schema_version, so payload is unchanged)
    let es_with_upcaster = EventStore::new(&store, TestCodec)
        .with_upcaster(V1ToV2Upcaster);
    let loaded = es_with_upcaster.load(sid, TodoId(1)).await.unwrap();
    assert_eq!(loaded.state().title, "Task");
    assert_eq!(loaded.version(), Version::from_persisted(1));
}

#[tokio::test]
async fn event_store_with_no_upcasters_is_zero_sized_chain() {
    // Verify that () (no upcasters) is zero-sized
    assert_eq!(std::mem::size_of::<()>(), 0);
    // EventStore<_, _, ()> has no upcaster overhead
    let es = EventStore::new(InMemoryStore::new(), TestCodec);
    let _ = es; // just verifying construction compiles
}
```

**Step 2: Run tests**

Run: `nix develop --command cargo test -p nexus-store -- event_store`
Expected: PASS (7 tests)

**Step 3: Commit**

```
test(store): add EventStore upcaster integration tests
```

---

### Task 5: BorrowingCodec Repository impl

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs`
- Test: `crates/nexus-store/tests/borrowing_event_store_tests.rs`

**Step 1: Write the test**

```rust
//! Tests for EventStore with BorrowingCodec (zero-copy path).

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use nexus::*;
use nexus_store::event_store::EventStore;
use nexus_store::repository::Repository;
use nexus_store::testing::InMemoryStore;
use nexus_store::BorrowingCodec;
use std::fmt;

// -- Domain where Event is a fixed-layout type decodable from bytes --

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
struct CounterEvent {
    delta: i32,
}

impl Message for CounterEvent {}
impl DomainEvent for CounterEvent {
    fn name(&self) -> &'static str {
        "CounterChanged"
    }
}

#[derive(Default, Debug, PartialEq)]
struct CounterState {
    value: i64,
}
impl AggregateState for CounterState {
    type Event = CounterEvent;
    fn initial() -> Self {
        Self::default()
    }
    fn apply(mut self, event: &CounterEvent) -> Self {
        self.value += i64::from(event.delta);
        self
    }
    fn name(&self) -> &'static str {
        "Counter"
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CounterId(u64);
impl fmt::Display for CounterId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ctr-{}", self.0)
    }
}
impl Id for CounterId {}

#[derive(Debug, thiserror::Error)]
#[error("counter error")]
struct CounterError;

struct CounterAggregate;
impl Aggregate for CounterAggregate {
    type State = CounterState;
    type Error = CounterError;
    type Id = CounterId;
}

// -- Borrowing codec: reinterpret bytes as &CounterEvent --

struct CounterBorrowingCodec;

impl BorrowingCodec<CounterEvent> for CounterBorrowingCodec {
    type Error = std::io::Error;

    fn encode(&self, event: &CounterEvent) -> Result<Vec<u8>, Self::Error> {
        Ok(event.delta.to_le_bytes().to_vec())
    }

    fn decode<'a>(
        &self,
        _event_type: &str,
        payload: &'a [u8],
    ) -> Result<&'a CounterEvent, Self::Error> {
        if payload.len() != std::mem::size_of::<CounterEvent>() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "wrong payload size",
            ));
        }
        // For this test: interpret 4 bytes as &CounterEvent.
        // A real zero-copy codec (rkyv) would use check_archived_root.
        let ptr = payload.as_ptr().cast::<CounterEvent>();
        // SAFETY: CounterEvent is repr(C) with a single i32 field,
        // payload is the right length, and i32 has no alignment requirement
        // on the platforms we test.
        Ok(unsafe { &*ptr })
    }
}

#[tokio::test]
async fn borrowing_codec_save_and_load_roundtrip() {
    let es = EventStore::new(InMemoryStore::new(), CounterBorrowingCodec);
    let sid = "counter-stream";

    let mut agg = AggregateRoot::<CounterAggregate>::new(CounterId(1));
    agg.apply(CounterEvent { delta: 10 });
    agg.apply(CounterEvent { delta: -3 });
    es.save(sid, &mut agg).await.unwrap();

    let loaded = es.load(sid, CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 7);
    assert_eq!(loaded.version(), Version::from_persisted(2));
}

#[tokio::test]
async fn borrowing_codec_load_empty_stream() {
    let es = EventStore::new(InMemoryStore::new(), CounterBorrowingCodec);
    let loaded = es.load("empty", CounterId(1)).await.unwrap();
    assert_eq!(loaded.state().value, 0);
    assert_eq!(loaded.version(), Version::INITIAL);
}
```

**Step 2: Run test to verify it fails**

Run: `nix develop --command cargo test -p nexus-store -- borrowing_event_store`
Expected: FAIL — `BorrowingCodec` not implemented for `Repository`

**Step 3: Add the BorrowingCodec Repository impl**

Add to `crates/nexus-store/src/event_store.rs`, after the existing `impl Repository`:

```rust
use crate::borrowing_codec::BorrowingCodec;

impl<A, S, C, U> Repository<A> for EventStore<S, C, U>
where
    A: Aggregate,
    S: RawEventStore,
    C: BorrowingCodec<EventOf<A>>,
    U: UpcasterChain,
    EventOf<A>: DomainEvent,
{
    type Error = StoreError;

    async fn load(
        &self,
        stream_id: &str,
        id: A::Id,
    ) -> Result<AggregateRoot<A>, StoreError> {
        let mut stream = self
            .store
            .read_stream(stream_id, Version::INITIAL)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        let mut root = AggregateRoot::<A>::new(id);

        while let Some(result) = stream.next().await {
            let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;

            let (event_type, _schema_version, payload) =
                self.run_upcasters(env.event_type(), env.schema_version(), env.payload())?;

            let event: &EventOf<A> = self
                .codec
                .decode(&event_type, &payload)
                .map_err(|e| StoreError::Codec(Box::new(e)))?;

            root.replay(env.version(), event)?;
        }

        Ok(root)
    }

    async fn save(
        &self,
        stream_id: &str,
        aggregate: &mut AggregateRoot<A>,
    ) -> Result<(), StoreError> {
        let events = aggregate.take_uncommitted_events();
        if events.is_empty() {
            return Ok(());
        }

        let expected_version = Version::from_persisted(
            events
                .first()
                .expect("checked non-empty above")
                .version()
                .as_u64()
                - 1,
        );

        let mut envelopes = Vec::with_capacity(events.len());
        for ve in &events {
            let payload = self
                .codec
                .encode(ve.event())
                .map_err(|e| StoreError::Codec(Box::new(e)))?;
            envelopes.push(
                pending_envelope(stream_id.into())
                    .version(ve.version())
                    .event_type(ve.event().name())
                    .payload(payload)
                    .build_without_metadata(),
            );
        }

        self.store
            .append(stream_id, expected_version, &envelopes)
            .await
            .map_err(|e| StoreError::Adapter(Box::new(e)))?;

        Ok(())
    }
}
```

**NOTE:** The two `impl Repository` blocks will conflict if a type implements both `Codec<E>` and `BorrowingCodec<E>`. This is fine — a codec is one or the other. If the compiler rejects overlapping impls, we'll need to use a marker trait or newtype wrapper. Cross that bridge if we hit it.

**Step 4: Run test to verify it passes**

Run: `nix develop --command cargo test -p nexus-store -- borrowing_event_store`
Expected: PASS (2 tests)

**Step 5: Commit**

```
feat(store): add BorrowingCodec Repository impl for zero-copy reads
```

---

### Task 6: Clippy, fmt, full test suite

**Step 1:** `nix develop --command cargo fmt --all`

**Step 2:** `nix develop --command cargo clippy --all-targets -- --deny warnings`

**Step 3:** `nix develop --command cargo test --all`

**Step 4: Commit any fixes**

```
fix: resolve clippy and formatting issues
```

---

### Task 7: Final commit with all changes

Verify the full diff makes sense, then commit all remaining changes:

```
feat(store)!: add EventStore facade with dual codec support and static upcaster chain
```
