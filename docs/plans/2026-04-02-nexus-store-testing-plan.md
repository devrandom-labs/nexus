# nexus-store Comprehensive Testing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bring nexus-store to kernel-grade test coverage with all 11 testing categories.

**Architecture:** 9 tasks covering the 9 missing/incomplete categories (static assertions and security tests are already complete). Each task adds one test file or modifies one source file. Tasks are independent and can run in parallel except Task 9 (mutation testing) which depends on all others.

**Tech Stack:** proptest, trybuild, static_assertions, criterion, cargo-mutants, debug_assert!

**Baseline:** 30 tests passing across 7 test files.

---

### Task 1: Unit Tests — fill coverage gaps

**Files:**
- Modify: `crates/nexus-store/tests/envelope_tests.rs`
- Modify: `crates/nexus-store/tests/codec_tests.rs`

**Step 1: Add StoreError Display and source tests to a new file**

Create `crates/nexus-store/tests/error_tests.rs`:

```rust
use nexus::{ErrorId, Version};
use nexus_store::StoreError;

#[test]
fn conflict_error_display() {
    let err = StoreError::Conflict {
        stream_id: ErrorId::from_display(&"order-42"),
        expected: Version::from_persisted(3),
        actual: Version::from_persisted(5),
    };
    let msg = err.to_string();
    assert!(msg.contains("order-42"), "should contain stream_id");
    assert!(msg.contains('3'), "should contain expected version");
    assert!(msg.contains('5'), "should contain actual version");
}

#[test]
fn stream_not_found_display() {
    let err = StoreError::StreamNotFound {
        stream_id: ErrorId::from_display(&"missing-stream"),
    };
    assert!(err.to_string().contains("missing-stream"));
}

#[test]
fn codec_error_display_and_source() {
    let inner = std::io::Error::other("bad bytes");
    let err = StoreError::Codec(Box::new(inner));
    assert!(err.to_string().contains("Codec error"));
    assert!(std::error::Error::source(&err).is_some());
}

#[test]
fn adapter_error_display_and_source() {
    let inner = std::io::Error::other("db down");
    let err = StoreError::Adapter(Box::new(inner));
    assert!(err.to_string().contains("Adapter error"));
    assert!(std::error::Error::source(&err).is_some());
}
```

**Step 2: Add builder Debug and intermediate type tests to envelope_tests.rs**

Append to `crates/nexus-store/tests/envelope_tests.rs`:

```rust
#[test]
fn pending_envelope_debug_output() {
    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("Created")
        .payload(vec![1, 2])
        .build_without_metadata();
    let debug = format!("{envelope:?}");
    assert!(debug.contains("PendingEnvelope"));
    assert!(debug.contains("s1"));
}

#[test]
fn persisted_envelope_debug_output() {
    let envelope = PersistedEnvelope::<()>::new(
        "s1",
        Version::from_persisted(1),
        "Created",
        &[1, 2],
        (),
    );
    let debug = format!("{envelope:?}");
    assert!(debug.contains("PersistedEnvelope"));
}

#[test]
fn build_without_metadata_equals_build_unit() {
    let e1 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    let e2 = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![1])
        .build(());
    assert_eq!(e1.stream_id(), e2.stream_id());
    assert_eq!(e1.version(), e2.version());
    assert_eq!(e1.event_type(), e2.event_type());
    assert_eq!(e1.payload(), e2.payload());
    assert_eq!(e1.metadata(), e2.metadata());
}
```

**Step 3: Run tests**

Run: `nix develop --command cargo test -p nexus-store`
Expected: All existing + new tests pass.

**Step 4: Commit**

```bash
git add crates/nexus-store/tests/error_tests.rs crates/nexus-store/tests/envelope_tests.rs
git commit -m "test(store): add unit tests for StoreError display, envelope Debug, builder equivalence"
```

---

### Task 2: Integration Tests — multi-step store operations

**Files:**
- Create: `crates/nexus-store/tests/integration_tests.rs`

**Step 1: Write integration tests**

Create `crates/nexus-store/tests/integration_tests.rs` reusing the in-memory adapter pattern from `raw_store_tests.rs`:

```rust
use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use std::collections::HashMap;
use tokio::sync::Mutex;

// --- Reuse in-memory adapter (same as raw_store_tests.rs) ---
type StoredRow = (u64, String, Vec<u8>);

struct InMemStore {
    streams: Mutex<HashMap<String, Vec<StoredRow>>>,
}

impl InMemStore {
    fn new() -> Self {
        Self { streams: Mutex::new(HashMap::new()) }
    }
}

struct InMemStream {
    events: Vec<(String, u64, String, Vec<u8>)>,
    pos: usize,
}

#[derive(Debug, thiserror::Error)]
enum TestError {
    #[error("conflict")]
    Conflict,
}

impl EventStream for InMemStream {
    type Error = TestError;
    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() { return None; }
        let row = &self.events[self.pos];
        self.pos += 1;
        Some(Ok(PersistedEnvelope::new(&row.0, Version::from_persisted(row.1), &row.2, &row.3, ())))
    }
}

impl RawEventStore for InMemStore {
    type Error = TestError;
    type Stream<'a> = InMemStream where Self: 'a;

    async fn append(&self, stream_id: &str, expected_version: Version, envelopes: &[PendingEnvelope<()>]) -> Result<(), Self::Error> {
        let mut guard = self.streams.lock().await;
        let stream = guard.entry(stream_id.to_owned()).or_default();
        let current = u64::try_from(stream.len()).unwrap_or(u64::MAX);
        if current != expected_version.as_u64() { return Err(TestError::Conflict); }
        for env in envelopes {
            stream.push((env.version().as_u64(), env.event_type().to_owned(), env.payload().to_vec()));
        }
        Ok(())
    }

    async fn read_stream(&self, stream_id: &str, from: Version) -> Result<Self::Stream<'_>, Self::Error> {
        let events = self.streams.lock().await
            .get(stream_id)
            .map(|s| s.iter()
                .filter(|(v, _, _)| *v >= from.as_u64())
                .map(|(v, t, p)| (stream_id.to_owned(), *v, t.clone(), p.clone()))
                .collect())
            .unwrap_or_default();
        Ok(InMemStream { events, pos: 0 })
    }
}

// --- Helper ---
fn make_envelopes(stream_id: &str, start: u64, count: u64) -> Vec<PendingEnvelope<()>> {
    (start..start + count).map(|v| {
        pending_envelope(stream_id.into())
            .version(Version::from_persisted(v))
            .event_type("Event")
            .payload(v.to_le_bytes().to_vec())
            .build_without_metadata()
    }).collect()
}

// --- Tests ---

#[tokio::test]
async fn multi_batch_append_then_read_all() {
    let store = InMemStore::new();

    // Batch 1: events 1-3
    store.append("s1", Version::INITIAL, &make_envelopes("s1", 1, 3)).await.unwrap();
    // Batch 2: events 4-6
    store.append("s1", Version::from_persisted(3), &make_envelopes("s1", 4, 3)).await.unwrap();

    // Read all from beginning
    let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
    let mut versions = vec![];
    while let Some(Ok(env)) = stream.next().await {
        versions.push(env.version().as_u64());
    }
    assert_eq!(versions, vec![1, 2, 3, 4, 5, 6]);
}

#[tokio::test]
async fn read_stream_from_version_filters_earlier() {
    let store = InMemStore::new();
    store.append("s1", Version::INITIAL, &make_envelopes("s1", 1, 5)).await.unwrap();

    // Read from version 3 onwards
    let mut stream = store.read_stream("s1", Version::from_persisted(3)).await.unwrap();
    let mut versions = vec![];
    while let Some(Ok(env)) = stream.next().await {
        versions.push(env.version().as_u64());
    }
    assert_eq!(versions, vec![3, 4, 5]);
}

#[tokio::test]
async fn concurrent_append_detects_conflict() {
    let store = InMemStore::new();
    store.append("s1", Version::INITIAL, &make_envelopes("s1", 1, 1)).await.unwrap();

    // Two writers both think stream is at version 1
    let r1 = store.append("s1", Version::from_persisted(1), &make_envelopes("s1", 2, 1)).await;
    // First succeeds, second should conflict (stream is now at version 2)
    let r2 = store.append("s1", Version::from_persisted(1), &make_envelopes("s1", 2, 1)).await;
    assert!(r1.is_ok());
    assert!(r2.is_err(), "Second writer should get a conflict");
}

#[tokio::test]
async fn large_batch_append_and_sequential_readback() {
    let store = InMemStore::new();
    let n = 1000u64;
    store.append("s1", Version::INITIAL, &make_envelopes("s1", 1, n)).await.unwrap();

    let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
    let mut count = 0u64;
    while let Some(Ok(env)) = stream.next().await {
        count += 1;
        // Verify payload encodes the version
        let expected_bytes = env.version().as_u64().to_le_bytes();
        assert_eq!(env.payload(), &expected_bytes);
    }
    assert_eq!(count, n);
}
```

**Step 2: Run tests**

Run: `nix develop --command cargo test -p nexus-store -- integration`
Expected: All 4 integration tests pass.

**Step 3: Commit**

```bash
git add crates/nexus-store/tests/integration_tests.rs
git commit -m "test(store): add integration tests for multi-batch append, version filtering, concurrency, large batch"
```

---

### Task 3: Property-Based Testing (proptest)

**Files:**
- Create: `crates/nexus-store/tests/property_tests.rs`

**Step 1: Write property tests**

```rust
use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::upcaster::EventUpcaster;
use proptest::prelude::*;
use std::collections::HashMap;
use tokio::sync::Mutex;

// --- In-memory adapter (minimal, for property testing) ---
// (Same adapter pattern as integration_tests.rs — copy the InMemStore/InMemStream/TestError here)
// ... [same adapter code] ...

// --- Strategies ---

fn version_strategy() -> impl Strategy<Value = u64> {
    1..=10_000u64
}

fn stream_id_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-z][a-z0-9-]{0,19}").unwrap()
}

fn payload_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 0..256)
}

// --- Properties ---

proptest! {
    /// Appended events are read back in the same order with identical data.
    #[test]
    fn append_read_roundtrip(
        stream_id in stream_id_strategy(),
        payloads in prop::collection::vec(payload_strategy(), 1..20),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemStore::new();
            let envelopes: Vec<_> = payloads.iter().enumerate().map(|(i, p)| {
                pending_envelope(stream_id.clone())
                    .version(Version::from_persisted((i + 1) as u64))
                    .event_type("E")
                    .payload(p.clone())
                    .build_without_metadata()
            }).collect();

            store.append(&stream_id, Version::INITIAL, &envelopes).await.unwrap();

            let mut stream = store.read_stream(&stream_id, Version::INITIAL).await.unwrap();
            for (i, expected_payload) in payloads.iter().enumerate() {
                let env = stream.next().await.unwrap().unwrap();
                prop_assert_eq!(env.version().as_u64(), (i + 1) as u64);
                prop_assert_eq!(env.payload(), expected_payload.as_slice());
            }
            prop_assert!(stream.next().await.is_none());
            Ok(())
        })?;
    }

    /// Events are always returned in ascending version order.
    #[test]
    fn read_returns_ascending_versions(
        n in 1..50usize,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemStore::new();
            let envelopes: Vec<_> = (1..=n).map(|v| {
                pending_envelope("s1".into())
                    .version(Version::from_persisted(v as u64))
                    .event_type("E")
                    .payload(vec![])
                    .build_without_metadata()
            }).collect();

            store.append("s1", Version::INITIAL, &envelopes).await.unwrap();

            let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
            let mut prev = 0u64;
            while let Some(Ok(env)) = stream.next().await {
                prop_assert!(env.version().as_u64() > prev, "versions must be ascending");
                prev = env.version().as_u64();
            }
            Ok(())
        })?;
    }

    /// Events appended to stream A never appear in stream B.
    #[test]
    fn stream_isolation(
        payloads_a in prop::collection::vec(payload_strategy(), 1..10),
        payloads_b in prop::collection::vec(payload_strategy(), 1..10),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let store = InMemStore::new();
            let make = |sid: &str, payloads: &[Vec<u8>]| -> Vec<_> {
                payloads.iter().enumerate().map(|(i, p)| {
                    pending_envelope(sid.into())
                        .version(Version::from_persisted((i + 1) as u64))
                        .event_type("E")
                        .payload(p.clone())
                        .build_without_metadata()
                }).collect()
            };
            store.append("a", Version::INITIAL, &make("a", &payloads_a)).await.unwrap();
            store.append("b", Version::INITIAL, &make("b", &payloads_b)).await.unwrap();

            let mut stream_a = store.read_stream("a", Version::INITIAL).await.unwrap();
            let mut count_a = 0usize;
            while let Some(Ok(env)) = stream_a.next().await {
                prop_assert_eq!(env.payload(), payloads_a[count_a].as_slice());
                count_a += 1;
            }
            prop_assert_eq!(count_a, payloads_a.len());

            let mut stream_b = store.read_stream("b", Version::INITIAL).await.unwrap();
            let mut count_b = 0usize;
            while let Some(Ok(env)) = stream_b.next().await {
                prop_assert_eq!(env.payload(), payloads_b[count_b].as_slice());
                count_b += 1;
            }
            prop_assert_eq!(count_b, payloads_b.len());
            Ok(())
        })?;
    }

    /// Upcaster chain: applying two upcasters sequentially produces expected result.
    #[test]
    fn upcaster_chain_composition(
        payload in payload_strategy(),
    ) {
        struct V1ToV2;
        impl EventUpcaster for V1ToV2 {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == 1 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), 2, p.to_vec())
            }
        }
        struct V2ToV3;
        impl EventUpcaster for V2ToV3 {
            fn can_upcast(&self, _: &str, v: u32) -> bool { v == 2 }
            fn upcast(&self, _: &str, _: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
                ("E".into(), 3, p.to_vec())
            }
        }

        let chain: Vec<Box<dyn EventUpcaster>> = vec![Box::new(V1ToV2), Box::new(V2ToV3)];
        let mut current_type = "E".to_owned();
        let mut current_ver = 1u32;
        let mut current_payload = payload.clone();
        for upcaster in &chain {
            if upcaster.can_upcast(&current_type, current_ver) {
                let (t, v, p) = upcaster.upcast(&current_type, current_ver, &current_payload);
                current_type = t;
                current_ver = v;
                current_payload = p;
            }
        }
        prop_assert_eq!(current_ver, 3);
        prop_assert_eq!(current_payload, payload);
    }
}
```

Note: The actual file needs the full adapter code copied in. The subagent implementing this should copy the `InMemStore`/`InMemStream`/`TestError` from the integration tests.

**Step 2: Run tests**

Run: `nix develop --command cargo test -p nexus-store -- property`
Expected: All 4 property tests pass (proptest runs 256 cases each by default).

**Step 3: Commit**

```bash
git add crates/nexus-store/tests/property_tests.rs
git commit -m "test(store): add property-based tests for roundtrip, ordering, isolation, upcaster composition"
```

---

### Task 4: Compile-Fail Tests (trybuild)

**Files:**
- Modify: `crates/nexus-store/Cargo.toml` (add trybuild dev-dependency)
- Create: `crates/nexus-store/tests/compile_fail_tests.rs`
- Create: `crates/nexus-store/tests/compile_fail/builder_skip_version.rs`
- Create: `crates/nexus-store/tests/compile_fail/builder_incomplete.rs`
- Create: `crates/nexus-store/tests/compile_fail/persisted_envelope_lifetime_escape.rs`

**Step 1: Add trybuild dependency**

Add to `crates/nexus-store/Cargo.toml` under `[dev-dependencies]`:
```toml
trybuild = { version = "1", features = ["diff"] }
```

**Step 2: Create test runner**

Create `crates/nexus-store/tests/compile_fail_tests.rs`:
```rust
#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
```

**Step 3: Create compile-fail test cases**

`crates/nexus-store/tests/compile_fail/builder_skip_version.rs`:
```rust
/// Skipping .version() in the typestate builder must not compile.
/// The builder enforces: new -> version -> event_type -> payload -> build.
use nexus_store::pending_envelope;

fn main() {
    // WithStreamId has no .event_type() method — must call .version() first
    let _ = pending_envelope("s1".into())
        .event_type("E");
}
```

`crates/nexus-store/tests/compile_fail/builder_incomplete.rs`:
```rust
/// Calling .build() without setting payload must not compile.
use nexus::Version;
use nexus_store::pending_envelope;

fn main() {
    // WithEventType has no .build() method — must call .payload() first
    let _ = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .build_without_metadata();
}
```

`crates/nexus-store/tests/compile_fail/persisted_envelope_lifetime_escape.rs`:
```rust
/// A PersistedEnvelope must not outlive its borrow source.
use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;

fn main() {
    let envelope = {
        let stream_id = String::from("s1");
        let payload = vec![1, 2, 3];
        PersistedEnvelope::<()>::new(
            &stream_id,
            Version::from_persisted(1),
            "E",
            &payload,
            (),
        )
        // stream_id and payload are dropped here
    };
    // Using envelope after borrow source is dropped must fail
    let _ = envelope.stream_id();
}
```

**Step 4: Run to generate .stderr baselines**

Run: `nix develop --command cargo test -p nexus-store -- compile_fail`
Expected: FAIL first time (no .stderr baselines yet).

Then run with TRYBUILD=overwrite to generate baselines:
Run: `TRYBUILD=overwrite nix develop --command cargo test -p nexus-store -- compile_fail`
Expected: .stderr files created in `tests/compile_fail/`.

**Step 5: Run again to confirm baselines match**

Run: `nix develop --command cargo test -p nexus-store -- compile_fail`
Expected: PASS.

**Step 6: Commit**

```bash
git add crates/nexus-store/Cargo.toml crates/nexus-store/tests/compile_fail_tests.rs crates/nexus-store/tests/compile_fail/
git commit -m "test(store): add compile-fail tests for typestate builder and lifetime safety"
```

---

### Task 5: Static Assertions — already complete ✅

No work needed. `crates/nexus-store/tests/static_assertions.rs` covers Send+Sync+Debug for StoreError, PendingEnvelope, PersistedEnvelope.

---

### Task 6: Adversarial Input Testing

**Files:**
- Create: `crates/nexus-store/tests/adversarial_tests.rs`

**Step 1: Write adversarial input tests**

```rust
use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::pending_envelope;

#[test]
fn unicode_stream_id() {
    let envelope = pending_envelope("user-\u{1F600}-stream".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    assert_eq!(envelope.stream_id(), "user-\u{1F600}-stream");
}

#[test]
fn rtl_and_zero_width_joiner_stream_id() {
    // Arabic + ZWJ — tests that string handling doesn't normalize or strip
    let id = "stream-\u{200D}\u{0627}\u{0644}\u{0639}";
    let envelope = pending_envelope(id.into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build_without_metadata();
    assert_eq!(envelope.stream_id(), id);
}

#[test]
fn max_version_envelope() {
    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(u64::MAX))
        .event_type("E")
        .payload(vec![1])
        .build_without_metadata();
    assert_eq!(envelope.version().as_u64(), u64::MAX);
}

#[test]
fn empty_payload() {
    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build_without_metadata();
    assert!(envelope.payload().is_empty());
}

#[test]
fn large_payload() {
    let large = vec![0xFFu8; 1_000_000]; // 1MB
    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("BigEvent")
        .payload(large.clone())
        .build_without_metadata();
    assert_eq!(envelope.payload().len(), 1_000_000);
    assert_eq!(envelope.payload(), large.as_slice());
}

#[test]
fn path_traversal_stream_id() {
    // Ensure the builder doesn't interpret or normalize path-like stream IDs
    let nasty = "../../../etc/passwd";
    let envelope = pending_envelope(nasty.into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build_without_metadata();
    assert_eq!(envelope.stream_id(), nasty);
}

#[test]
fn null_bytes_in_stream_id() {
    let id = "stream\0injected";
    let envelope = pending_envelope(id.into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build_without_metadata();
    assert_eq!(envelope.stream_id(), id);
}

#[test]
fn exotic_metadata_nested_generics() {
    #[derive(Debug)]
    struct Nested {
        inner: Vec<Option<HashMap<String, Vec<u8>>>>,
    }
    use std::collections::HashMap;
    let meta = Nested { inner: vec![Some(HashMap::new())] };
    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![1])
        .build(meta);
    assert_eq!(envelope.stream_id(), "s1");
}

#[test]
fn zero_sized_metadata() {
    #[derive(Debug)]
    struct Zst;
    let envelope = pending_envelope("s1".into())
        .version(Version::from_persisted(1))
        .event_type("E")
        .payload(vec![])
        .build(Zst);
    assert_eq!(std::mem::size_of_val(envelope.metadata()), 0);
}

#[test]
fn persisted_envelope_with_binary_payload() {
    // All 256 byte values
    let payload: Vec<u8> = (0..=255).collect();
    let envelope = PersistedEnvelope::<()>::new(
        "s1",
        Version::from_persisted(1),
        "BinaryEvent",
        &payload,
        (),
    );
    assert_eq!(envelope.payload().len(), 256);
    assert_eq!(envelope.payload()[0], 0);
    assert_eq!(envelope.payload()[255], 255);
}
```

**Step 2: Run tests**

Run: `nix develop --command cargo test -p nexus-store -- adversarial`
Expected: All 10 adversarial tests pass.

**Step 3: Commit**

```bash
git add crates/nexus-store/tests/adversarial_tests.rs
git commit -m "test(store): add adversarial input tests for Unicode, large payloads, path traversal, exotic metadata"
```

---

### Task 7: Codec Roundtrip Tests

**Files:**
- Create: `crates/nexus-store/tests/codec_roundtrip_tests.rs`
- Modify: `crates/nexus-store/Cargo.toml` (add serde_json dev-dependency)

**Step 1: Add serde_json dev-dependency**

Add to `crates/nexus-store/Cargo.toml` under `[dev-dependencies]`:
```toml
serde = { workspace = true }
serde_json = { workspace = true }
```

**Step 2: Write codec roundtrip tests**

```rust
use nexus::*;
use nexus_store::codec::Codec;
use serde::{Deserialize, Serialize};

// A real codec using serde_json
struct JsonCodec;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    UnitVariant,
    TupleVariant(String, u64),
    StructVariant { name: String, value: f64 },
}

impl Message for TestEvent {}
impl DomainEvent for TestEvent {
    fn name(&self) -> &'static str {
        match self {
            Self::UnitVariant => "UnitVariant",
            Self::TupleVariant(_, _) => "TupleVariant",
            Self::StructVariant { .. } => "StructVariant",
        }
    }
}

#[derive(Debug)]
struct JsonCodecError(String);
impl std::fmt::Display for JsonCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for JsonCodecError {}

impl Codec for JsonCodec {
    type Error = JsonCodecError;

    fn encode<E: DomainEvent + Serialize>(&self, event: &E) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(event).map_err(|e| JsonCodecError(e.to_string()))
    }

    fn decode<E: DomainEvent + for<'de> Deserialize<'de>>(&self, _event_type: &str, payload: &[u8]) -> Result<E, Self::Error> {
        serde_json::from_slice(payload).map_err(|e| JsonCodecError(e.to_string()))
    }
}
```

Note: The Codec trait requires `E: DomainEvent` — to also require Serialize/Deserialize, the subagent will need to check if the Codec trait's generic bounds allow it. If not, the JsonCodec will need to use a concrete type instead of being generic. **The subagent should read `crates/nexus-store/src/codec.rs` and adapt the codec implementation to work within the trait's actual bounds.** The key tests to write:

- `roundtrip_unit_variant`: encode then decode UnitVariant, assert equality
- `roundtrip_tuple_variant`: encode then decode TupleVariant, assert equality
- `roundtrip_struct_variant`: encode then decode StructVariant, assert equality
- `decode_corrupted_payload`: pass garbage bytes, assert error
- `decode_wrong_event_type`: encode one variant, try to decode as different type

**Step 3: Run tests**

Run: `nix develop --command cargo test -p nexus-store -- codec_roundtrip`
Expected: All 5 roundtrip tests pass.

**Step 4: Commit**

```bash
git add crates/nexus-store/Cargo.toml crates/nexus-store/tests/codec_roundtrip_tests.rs
git commit -m "test(store): add codec roundtrip tests with serde_json for encode/decode identity"
```

---

### Task 8: Debug Assertion Contracts

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`

Since nexus-store is primarily trait definitions, most invariants are "implementor contracts" rather than enforceable assertions. However, the builder's `build()` method is concrete code where we can add defensive assertions.

**Step 1: Add debug_assert! to builder build methods**

In `crates/nexus-store/src/envelope.rs`, inside `WithPayload::build()`:

```rust
pub fn build<M>(self, metadata: M) -> PendingEnvelope<M> {
    debug_assert!(
        !self.stream_id.is_empty(),
        "stream_id must not be empty — facade should validate before building"
    );
    debug_assert!(
        !self.event_type.is_empty(),
        "event_type must not be empty — facade should validate before building"
    );
    PendingEnvelope {
        stream_id: self.stream_id,
        version: self.version,
        event_type: self.event_type,
        payload: self.payload,
        metadata,
    }
}
```

Note: This deliberately does NOT prevent empty strings (the security tests document that validation is deferred to the facade). These assertions fire only in debug/test builds as early warning signals.

**Step 2: Run tests to verify assertions don't break existing tests**

Run: `nix develop --command cargo test -p nexus-store`
Expected: Existing security tests `c2_builder_accepts_empty_stream_id` and `c2_builder_accepts_empty_event_type` will NOW FAIL because debug_assert fires in test mode.

**Important decision:** Since the security tests document that empty strings ARE accepted by the builder (validation deferred to facade), the debug_asserts would break that contract. The subagent must choose ONE of:
- (a) Skip debug_assert for empty strings (keep only as documentation comment)
- (b) Change the security tests to expect panics
- (c) Add assertions only for properties that don't conflict with existing tests

Recommended: Option (a) — add debug_asserts only in the `build_without_metadata` path for `payload.is_empty()` being documented, and instead document the empty-string deferral as a code comment. Or add assertions to `PersistedEnvelope::new()` for version monotonicity that don't conflict.

The subagent should read the security_tests.rs and choose the approach that doesn't break existing tests.

**Step 3: Commit**

```bash
git add crates/nexus-store/src/envelope.rs
git commit -m "feat(store): add debug assertion contracts for envelope builder invariants"
```

---

### Task 9: Benchmarks (criterion)

**Files:**
- Modify: `crates/nexus-store/Cargo.toml` (add criterion + [[bench]])
- Create: `crates/nexus-store/benches/store_bench.rs`

**Step 1: Add criterion to Cargo.toml**

Add to `crates/nexus-store/Cargo.toml`:
```toml
[dev-dependencies]
criterion = { workspace = true }

[[bench]]
name = "store_bench"
harness = false
```

**Step 2: Write benchmarks**

Create `crates/nexus-store/benches/store_bench.rs`:

```rust
use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use nexus::Version;
use nexus_store::envelope::{PendingEnvelope, PersistedEnvelope};
use nexus_store::pending_envelope;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use nexus_store::upcaster::EventUpcaster;
use std::collections::HashMap;
use tokio::sync::Mutex;

// ... (in-memory adapter code, same pattern)

fn bench_builder_throughput(c: &mut Criterion) {
    c.bench_function("PendingEnvelope builder", |b| {
        b.iter(|| {
            black_box(
                pending_envelope(black_box("stream-1".into()))
                    .version(Version::from_persisted(1))
                    .event_type("UserCreated")
                    .payload(vec![1, 2, 3, 4])
                    .build_without_metadata(),
            )
        });
    });
}

fn bench_persisted_envelope_construction(c: &mut Criterion) {
    let stream_id = "stream-1";
    let event_type = "UserCreated";
    let payload = [1u8, 2, 3, 4, 5, 6, 7, 8];

    c.bench_function("PersistedEnvelope::new (zero-alloc)", |b| {
        b.iter(|| {
            black_box(PersistedEnvelope::<()>::new(
                black_box(stream_id),
                Version::from_persisted(1),
                black_box(event_type),
                black_box(&payload),
                (),
            ))
        });
    });
}

fn bench_append(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("InMemStore::append");

    for size in [1, 10, 100, 1000] {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter_custom(|iters| {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let store = InMemStore::new();
                    let envelopes: Vec<_> = (1..=size)
                        .map(|v| {
                            pending_envelope("s1".into())
                                .version(Version::from_persisted(v))
                                .event_type("E")
                                .payload(vec![0; 64])
                                .build_without_metadata()
                        })
                        .collect();
                    let start = std::time::Instant::now();
                    rt.block_on(store.append("s1", Version::INITIAL, &envelopes)).unwrap();
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

fn bench_read_stream(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("InMemStore::read_stream");

    for size in [10, 100, 1000] {
        let store = InMemStore::new();
        let envelopes: Vec<_> = (1..=size)
            .map(|v| {
                pending_envelope("s1".into())
                    .version(Version::from_persisted(v))
                    .event_type("E")
                    .payload(vec![0; 64])
                    .build_without_metadata()
            })
            .collect();
        rt.block_on(store.append("s1", Version::INITIAL, &envelopes)).unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
                    while let Some(Ok(env)) = stream.next().await {
                        black_box(env.payload());
                    }
                });
            });
        });
    }
    group.finish();
}

fn bench_upcaster_chain(c: &mut Criterion) {
    struct NoopUpcaster;
    impl EventUpcaster for NoopUpcaster {
        fn can_upcast(&self, _: &str, v: u32) -> bool { v < 10 }
        fn upcast(&self, et: &str, v: u32, p: &[u8]) -> (String, u32, Vec<u8>) {
            (et.to_owned(), v + 1, p.to_vec())
        }
    }

    let chain: Vec<Box<dyn EventUpcaster>> = (0..5).map(|_| Box::new(NoopUpcaster) as _).collect();
    let payload = vec![0u8; 256];

    c.bench_function("upcaster chain (5 upcasters)", |b| {
        b.iter(|| {
            let mut t = "E".to_owned();
            let mut v = 1u32;
            let mut p = payload.clone();
            for u in &chain {
                if u.can_upcast(&t, v) {
                    let (nt, nv, np) = u.upcast(&t, v, &p);
                    t = nt;
                    v = nv;
                    p = np;
                }
            }
            black_box((t, v, p))
        });
    });
}

criterion_group!(
    benches,
    bench_builder_throughput,
    bench_persisted_envelope_construction,
    bench_append,
    bench_read_stream,
    bench_upcaster_chain,
);
criterion_main!(benches);
```

Note: The subagent must copy the full `InMemStore`/`InMemStream`/`TestError` adapter code into the bench file.

**Step 3: Run benchmarks**

Run: `nix develop --command cargo bench --bench store_bench -p nexus-store`
Expected: Benchmark results printed. PersistedEnvelope::new should be sub-nanosecond (zero-alloc).

**Step 4: Commit**

```bash
git add crates/nexus-store/Cargo.toml crates/nexus-store/benches/store_bench.rs
git commit -m "bench(store): add criterion benchmarks for builder, envelope, append, read, upcaster chain"
```

---

### Task 10: Mutation Testing (cargo-mutants)

**Depends on:** Tasks 1-9 (run after all tests are in place for maximum kill rate).

**Step 1: Run mutation testing**

Run: `nix develop --command cargo mutants -p nexus-store -- --test-threads=1`

If cargo-mutants is not in the nix shell, use:
`cargo install cargo-mutants && cargo mutants -p nexus-store -- --test-threads=1`

**Step 2: Analyze results**

Check `mutants.out/caught.txt`, `mutants.out/missed.txt`, `mutants.out/unviable.txt`.

Target: 100% viable mutant kill rate (0 missed).

**Step 3: If any mutants are missed, add targeted tests**

For each missed mutant, write a test that specifically exercises the mutated code path.

**Step 4: Commit results**

```bash
git add mutants.out/
git commit -m "test(store): mutation testing results — N caught, N unviable, 0 missed"
```

---

### Task 11: Run full test suite and verify

**Step 1: Run all tests**

Run: `nix develop --command cargo test -p nexus-store`
Expected: All tests pass (should be 50+ tests now).

**Step 2: Run clippy**

Run: `nix develop --command cargo clippy --all-targets -p nexus-store -- --deny warnings`
Expected: Zero warnings.

**Step 3: Run fmt check**

Run: `nix develop --command cargo fmt -p nexus-store --check`
Expected: Already formatted.
