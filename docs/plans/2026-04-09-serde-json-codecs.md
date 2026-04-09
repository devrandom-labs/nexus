# Serde + JSON Feature-Gated Codecs Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `serde` and `json` feature flags to `nexus-store` that provide ready-made `SerdeCodec<F>` and `JsonCodec` so users never need to hand-implement `Codec<E>`.

**Architecture:** A `serde/` subdirectory under `codec/` holds all serde-gated code. `SerdeFormat` is the format abstraction, `SerdeCodec<F>` implements `Codec<E>` generically, and `Json` is the first concrete format behind the `json` feature (which implies `serde`).

**Tech Stack:** Rust, serde, serde_json, nexus-store `Codec<E>` trait

---

### Task 1: Add feature flags and optional dependencies to Cargo.toml

**Files:**
- Modify: `crates/nexus-store/Cargo.toml`

**Step 1: Add features and move deps from dev to optional**

In `crates/nexus-store/Cargo.toml`, add features and optional deps:

```toml
[features]
testing = ["dep:tokio"]
bounded-labels = []
serde = ["dep:serde"]
json = ["serde", "dep:serde_json"]

[dependencies]
nexus = { version = "0.1.0", path = "../nexus" }
serde = { workspace = true, optional = true }
serde_json = { workspace = true, optional = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["sync"], optional = true }
workspace-hack = { version = "0.1", path = "../workspace-hack" }
```

Note: `serde` and `serde_json` remain in `[dev-dependencies]` too (tests use them unconditionally).

**Step 2: Verify compilation**

Run: `cargo check -p nexus-store` (no features — should compile without serde)
Run: `cargo check -p nexus-store --features serde` (serde feature — should compile)
Run: `cargo check -p nexus-store --features json` (json feature — should compile)

---

### Task 2: Create `SerdeFormat` trait and `SerdeCodec<F>` struct

**Files:**
- Create: `crates/nexus-store/src/codec/serde/mod.rs`
- Modify: `crates/nexus-store/src/codec/mod.rs`

**Step 1: Create `crates/nexus-store/src/codec/serde/mod.rs`**

```rust
//! Serde-based codec infrastructure.
//!
//! Enabled by the `serde` feature. Provides [`SerdeFormat`] (the format
//! abstraction) and [`SerdeCodec`] (a generic [`Codec`](crate::Codec)
//! implementation for any serde-compatible event type).

#[cfg(feature = "json")]
pub mod json;

use crate::Codec;

/// A serialization format that can encode/decode serde types to/from bytes.
///
/// Implement this for your preferred wire format (JSON, postcard, bincode,
/// etc.) and pair it with [`SerdeCodec`] to get a [`Codec`](crate::Codec)
/// for free.
///
/// # Built-in formats
///
/// - [`Json`](json::Json) (requires `json` feature)
pub trait SerdeFormat: Send + Sync + 'static {
    /// The error type for serialization/deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a value to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if serialization fails.
    fn serialize<T: serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, Self::Error>;

    /// Deserialize a value from bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if deserialization fails.
    fn deserialize<T: serde::de::DeserializeOwned>(&self, bytes: &[u8]) -> Result<T, Self::Error>;
}

/// A [`Codec`] powered by serde and a pluggable [`SerdeFormat`].
///
/// Works with any event type that implements `Serialize + DeserializeOwned`.
/// The `event_type` parameter on [`decode`](Codec::decode) is ignored —
/// serde's own enum tagging handles variant dispatch.
///
/// # Examples
///
/// ```ignore
/// use nexus_store::codec::serde::{SerdeCodec, json::Json};
///
/// let codec = SerdeCodec::new(Json);
/// // or: JsonCodec::default()
/// ```
#[derive(Debug, Clone)]
pub struct SerdeCodec<F> {
    format: F,
}

impl<F> SerdeCodec<F> {
    /// Create a new serde codec with the given format.
    pub const fn new(format: F) -> Self {
        Self { format }
    }
}

impl<F: Default> Default for SerdeCodec<F> {
    fn default() -> Self {
        Self::new(F::default())
    }
}

impl<E, F> Codec<E> for SerdeCodec<F>
where
    E: serde::Serialize + serde::de::DeserializeOwned + Send + Sync + 'static,
    F: SerdeFormat,
{
    type Error = F::Error;

    fn encode(&self, event: &E) -> Result<Vec<u8>, Self::Error> {
        self.format.serialize(event)
    }

    fn decode(&self, _event_type: &str, payload: &[u8]) -> Result<E, Self::Error> {
        self.format.deserialize(payload)
    }
}
```

**Step 2: Wire up module in `crates/nexus-store/src/codec/mod.rs`**

Add the conditional module declaration:

```rust
mod borrowing;
mod owning;
#[cfg(feature = "serde")]
pub mod serde;

pub use borrowing::BorrowingCodec;
pub use owning::Codec;

#[cfg(feature = "serde")]
pub use self::serde::{SerdeCodec, SerdeFormat};
```

**Step 3: Add crate-level re-exports in `crates/nexus-store/src/lib.rs`**

Add after the existing `pub use codec::` lines:

```rust
#[cfg(feature = "serde")]
pub use codec::serde::{SerdeCodec, SerdeFormat};
```

**Step 4: Verify compilation**

Run: `cargo check -p nexus-store` (no features — serde module absent, compiles)
Run: `cargo check -p nexus-store --features serde` (serde module present, compiles)
Run: `cargo clippy -p nexus-store --features serde -- --deny warnings`

---

### Task 3: Create `Json` format and `JsonCodec` alias

**Files:**
- Create: `crates/nexus-store/src/codec/serde/json.rs`
- Modify: `crates/nexus-store/src/lib.rs`

**Step 1: Create `crates/nexus-store/src/codec/serde/json.rs`**

```rust
//! JSON serialization format backed by `serde_json`.
//!
//! Enabled by the `json` feature (which implies `serde`).

use super::{SerdeCodec, SerdeFormat};

/// JSON serialization format using [`serde_json`].
///
/// Zero-configuration: construct with `Json` (unit struct) or
/// `Json::default()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Json;

impl SerdeFormat for Json {
    type Error = serde_json::Error;

    fn serialize<T: ::serde::Serialize>(&self, value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn deserialize<T: ::serde::de::DeserializeOwned>(
        &self,
        bytes: &[u8],
    ) -> Result<T, Self::Error> {
        serde_json::from_slice(bytes)
    }
}

/// A [`SerdeCodec`] that serializes events as JSON.
///
/// This is a type alias for `SerdeCodec<Json>`. Use it when you want
/// JSON serialization without any custom configuration:
///
/// ```ignore
/// use nexus_store::JsonCodec;
///
/// let repo = store.repository(JsonCodec::default(), ());
/// ```
pub type JsonCodec = SerdeCodec<Json>;
```

**Step 2: Add crate-level re-exports in `crates/nexus-store/src/lib.rs`**

Add after the serde re-exports:

```rust
#[cfg(feature = "json")]
pub use codec::serde::json::{Json, JsonCodec};
```

**Step 3: Verify compilation**

Run: `cargo check -p nexus-store --features json`
Run: `cargo clippy -p nexus-store --features json -- --deny warnings`

---

### Task 4: Write tests for `SerdeCodec` and `JsonCodec`

**Files:**
- Create: `crates/nexus-store/tests/serde_codec_tests.rs`

**Step 1: Write tests**

The test file uses `#[cfg(feature = "serde")]` or `#[cfg(feature = "json")]` guards so it compiles under all feature combinations.

```rust
//! Tests for the feature-gated serde codec infrastructure.

#![cfg(feature = "json")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use nexus_store::{Codec, JsonCodec};
use nexus_store::codec::serde::json::Json;
use nexus_store::codec::serde::{SerdeCodec, SerdeFormat};
use serde::{Deserialize, Serialize};

// -- Test event type --

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
enum TestEvent {
    Created { title: String },
    Completed,
}

// -- SerdeFormat tests --

#[test]
fn json_format_roundtrip() {
    let json = Json;
    let event = TestEvent::Created {
        title: "buy milk".to_owned(),
    };
    let bytes = json.serialize(&event).expect("serialize");
    let decoded: TestEvent = json.deserialize(&bytes).expect("deserialize");
    assert_eq!(event, decoded);
}

#[test]
fn json_format_unit_variant_roundtrip() {
    let json = Json;
    let event = TestEvent::Completed;
    let bytes = json.serialize(&event).expect("serialize");
    let decoded: TestEvent = json.deserialize(&bytes).expect("deserialize");
    assert_eq!(event, decoded);
}

// -- SerdeCodec<Json> / JsonCodec tests --

#[test]
fn json_codec_encode_decode_roundtrip() {
    let codec = JsonCodec::default();
    let event = TestEvent::Created {
        title: "write tests".to_owned(),
    };
    let bytes = codec.encode(&event).expect("encode");
    // event_type is intentionally ignored by SerdeCodec
    let decoded = codec.decode("ignored", &bytes).expect("decode");
    assert_eq!(event, decoded);
}

#[test]
fn json_codec_ignores_event_type_parameter() {
    let codec = JsonCodec::default();
    let event = TestEvent::Completed;
    let bytes = codec.encode(&event).expect("encode");
    // Pass a completely wrong event_type — should still decode fine
    let decoded = codec.decode("TotallyWrong", &bytes).expect("decode");
    assert_eq!(event, decoded);
}

#[test]
fn json_codec_invalid_payload_returns_error() {
    let codec: SerdeCodec<Json> = SerdeCodec::new(Json);
    let result = codec.decode::<TestEvent>("Created", b"not json");
    assert!(result.is_err());
}

#[test]
fn json_codec_is_constructible_via_default() {
    let _codec = JsonCodec::default();
}

#[test]
fn json_codec_is_constructible_via_new() {
    let _codec = SerdeCodec::new(Json);
}

// -- Integration: SerdeCodec with EventStore --

// This test verifies that JsonCodec satisfies the Codec<E> trait bounds
// required by EventStore, using the InMemoryStore backend.

#[cfg(feature = "testing")]
mod integration {
    use super::*;
    use nexus::*;
    use nexus_store::Store;
    use nexus_store::store::Repository;
    use nexus_store::testing::InMemoryStore;
    use std::fmt;

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

    #[tokio::test]
    async fn json_codec_works_with_event_store() {
        let store = Store::new(InMemoryStore::default());
        let repo = store.repository(JsonCodec::default(), ());

        // Save
        let mut agg = TodoAggregate::new(TodoId(1));
        let events = nexus::events![TodoEvent::Created {
            title: "test".to_owned(),
        }];
        repo.save(&mut agg, events).await.expect("save");

        // Load
        let loaded = repo.load(&TodoId(1)).await.expect("load");
        assert_eq!(loaded.state().title, "test");
        assert!(!loaded.state().done);
    }
}
```

**Step 2: Run tests**

Run: `cargo test -p nexus-store --features "json,testing" -- serde_codec_tests`
Expected: all tests pass.

**Step 3: Run full suite to check no regressions**

Run: `cargo test -p nexus-store --features testing`
Expected: all existing tests still pass (serde module not compiled without feature).

Run: `cargo test -p nexus-store --features "json,testing"`
Expected: all tests pass including new serde codec tests.

---

### Task 5: Clippy, fmt, and full workspace verification

**Files:** None (verification only)

**Step 1: Format**

Run: `cargo fmt --all`

**Step 2: Clippy all feature combos**

Run: `cargo clippy -p nexus-store -- --deny warnings` (no features)
Run: `cargo clippy -p nexus-store --features serde -- --deny warnings`
Run: `cargo clippy -p nexus-store --features json -- --deny warnings`
Run: `cargo clippy -p nexus-store --all-features -- --deny warnings`

**Step 3: Full workspace test**

Run: `cargo test --all`

**Step 4: Workspace hack check**

Run: `cargo hakari generate --diff`
If diff exists, run: `cargo hakari generate && cargo hakari manage-deps`

---

### Task 6: Commit

**Step 1: Stage and commit**

```bash
git add crates/nexus-store/Cargo.toml \
       crates/nexus-store/src/codec/mod.rs \
       crates/nexus-store/src/codec/serde/mod.rs \
       crates/nexus-store/src/codec/serde/json.rs \
       crates/nexus-store/src/lib.rs \
       crates/nexus-store/tests/serde_codec_tests.rs
git commit -m "feat(store): add serde + json feature-gated codecs"
```
