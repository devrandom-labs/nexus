# nexus-fjall Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a fjall-backed `RawEventStore` adapter with fixed-size keys, zero-alloc append path, and ready-future sync bridge.

**Architecture:** Two fjall keyspaces (`streams` for metadata point lookups, `events` for fixed 16-byte key range scans). `SingleWriterTxDatabase` for serialized atomic transactions. Numeric stream ID mapping for branchless key comparisons. See `docs/plans/2026-04-04-nexus-fjall-design.md` for full design.

**Tech Stack:** Rust 2024, fjall 2.x, nexus-store traits, thiserror

**Lint constraints:** Workspace lints deny `unwrap_used`, `expect_used`, `panic`, `as_conversions`, `shadow_reuse`, `shadow_same`, `shadow_unrelated`. All code must handle errors via `?` or `map_err`. Use `usize::from()` for widening conversions, `u64::try_from()` for narrowing.

---

## Task 1: Scaffold nexus-fjall crate

**Files:**
- Create: `crates/nexus-fjall/Cargo.toml`
- Create: `crates/nexus-fjall/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add member + fjall dep)

**Step 1: Add fjall to workspace dependencies**

In root `Cargo.toml`, add to `[workspace.dependencies]`:
```toml
fjall = "2"
```

And add `"crates/nexus-fjall"` to `[workspace] members`.

**Step 2: Create crate Cargo.toml**

```toml
[package]
name = "nexus-fjall"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Fjall-backed event store adapter for the Nexus event-sourcing framework"
readme = "../../README.md"
keywords = ["event-sourcing", "event-store", "fjall", "embedded"]
categories = ["database"]

[dependencies]
fjall = { workspace = true }
nexus-store = { version = "0.1.0", path = "../nexus-store" }
thiserror = { workspace = true }
workspace-hack = { version = "0.1", path = "../workspace-hack" }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
tempfile = "3"

[lints]
workspace = true
```

**Step 3: Create lib.rs with module stubs**

```rust
pub mod builder;
pub mod encoding;
pub mod error;
pub mod store;
pub mod stream;
```

Create empty files for each module (`builder.rs`, `encoding.rs`, `error.rs`, `store.rs`, `stream.rs`) — each containing only a comment `// TODO: implement` is NOT allowed by lints. Just leave them empty or with a single unused import that won't trigger warnings. Actually, leave them truly empty for now — we'll populate in subsequent tasks.

**Step 4: Verify it compiles**

Run: `cargo check -p nexus-fjall`
Expected: SUCCESS (empty modules compile)

**Step 5: Regenerate workspace-hack**

Run: `cargo hakari generate && cargo hakari manage-deps`

**Step 6: Commit**

```bash
git add crates/nexus-fjall/ Cargo.toml Cargo.lock
git commit -m "feat(fjall): scaffold nexus-fjall crate"
```

---

## Task 2: Encoding module (TDD)

**Files:**
- Create: `crates/nexus-fjall/src/encoding.rs`

This module contains pure functions for key/value byte encoding. No fjall dependency — independently testable.

**Step 1: Write failing tests for event key encode/decode**

In `encoding.rs`, add tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_key_round_trips() {
        let key = encode_event_key(42, 7);
        let (stream_id, version) = decode_event_key(&key).unwrap();
        assert_eq!(stream_id, 42);
        assert_eq!(version, 7);
    }

    #[test]
    fn event_key_is_big_endian_for_ordering() {
        let key_low = encode_event_key(1, 1);
        let key_high = encode_event_key(1, 2);
        assert!(key_low < key_high, "BE encoding must sort naturally");
    }

    #[test]
    fn event_key_groups_by_stream() {
        let k1 = encode_event_key(1, 100);
        let k2 = encode_event_key(2, 1);
        assert!(k1 < k2, "stream 1 events must sort before stream 2");
    }

    #[test]
    fn event_key_decode_rejects_wrong_size() {
        assert!(decode_event_key(&[0u8; 15]).is_err());
        assert!(decode_event_key(&[0u8; 17]).is_err());
        assert!(decode_event_key(&[]).is_err());
    }
}
```

Run: `cargo test -p nexus-fjall -- encoding::tests`
Expected: FAIL (functions don't exist)

**Step 2: Implement event key encode/decode**

```rust
/// Size of the event key: `[u64 BE stream_numeric_id][u64 BE version]`.
pub(crate) const EVENT_KEY_SIZE: usize = 16;

/// Encode an event key. Big-endian for natural byte ordering in the LSM tree.
#[must_use]
pub(crate) fn encode_event_key(stream_numeric_id: u64, version: u64) -> [u8; EVENT_KEY_SIZE] {
    let mut buf = [0u8; EVENT_KEY_SIZE];
    buf[..8].copy_from_slice(&stream_numeric_id.to_be_bytes());
    buf[8..].copy_from_slice(&version.to_be_bytes());
    buf
}

/// Decode an event key into `(stream_numeric_id, version)`.
///
/// # Errors
///
/// Returns `DecodeError::InvalidSize` if `key.len() != 16`.
pub(crate) fn decode_event_key(key: &[u8]) -> Result<(u64, u64), DecodeError> {
    if key.len() != EVENT_KEY_SIZE {
        return Err(DecodeError::InvalidSize {
            expected: EVENT_KEY_SIZE,
            actual: key.len(),
        });
    }
    let mut id_buf = [0u8; 8];
    let mut ver_buf = [0u8; 8];
    id_buf.copy_from_slice(&key[..8]);
    ver_buf.copy_from_slice(&key[8..16]);
    Ok((u64::from_be_bytes(id_buf), u64::from_be_bytes(ver_buf)))
}
```

Run: `cargo test -p nexus-fjall -- encoding::tests::event_key`
Expected: PASS (4 tests)

**Step 3: Write failing tests for stream meta encode/decode**

```rust
    #[test]
    fn stream_meta_round_trips() {
        let meta = encode_stream_meta(99, 42);
        let (numeric_id, version) = decode_stream_meta(&meta).unwrap();
        assert_eq!(numeric_id, 99);
        assert_eq!(version, 42);
    }

    #[test]
    fn stream_meta_decode_rejects_wrong_size() {
        assert!(decode_stream_meta(&[0u8; 15]).is_err());
        assert!(decode_stream_meta(&[0u8; 17]).is_err());
    }
```

Run: `cargo test -p nexus-fjall -- encoding::tests::stream_meta`
Expected: FAIL

**Step 4: Implement stream meta encode/decode**

```rust
/// Size of stream metadata: `[u64 LE numeric_id][u64 LE current_version]`.
pub(crate) const STREAM_META_SIZE: usize = 16;

/// Encode stream metadata. Little-endian (no ordering requirement, just fast).
#[must_use]
pub(crate) fn encode_stream_meta(numeric_id: u64, version: u64) -> [u8; STREAM_META_SIZE] {
    let mut buf = [0u8; STREAM_META_SIZE];
    buf[..8].copy_from_slice(&numeric_id.to_le_bytes());
    buf[8..].copy_from_slice(&version.to_le_bytes());
    buf
}

/// Decode stream metadata into `(numeric_id, current_version)`.
pub(crate) fn decode_stream_meta(value: &[u8]) -> Result<(u64, u64), DecodeError> {
    if value.len() != STREAM_META_SIZE {
        return Err(DecodeError::InvalidSize {
            expected: STREAM_META_SIZE,
            actual: value.len(),
        });
    }
    let mut id_buf = [0u8; 8];
    let mut ver_buf = [0u8; 8];
    id_buf.copy_from_slice(&value[..8]);
    ver_buf.copy_from_slice(&value[8..16]);
    Ok((u64::from_le_bytes(id_buf), u64::from_le_bytes(ver_buf)))
}
```

Run: `cargo test -p nexus-fjall -- encoding::tests::stream_meta`
Expected: PASS

**Step 5: Write failing tests for event value encode/decode**

```rust
    #[test]
    fn event_value_round_trips() {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 3, "OrderCreated", b"payload-bytes").unwrap();
        let (schema_ver, event_type, payload) = decode_event_value(&buf).unwrap();
        assert_eq!(schema_ver, 3);
        assert_eq!(event_type, "OrderCreated");
        assert_eq!(payload, b"payload-bytes");
    }

    #[test]
    fn event_value_empty_payload() {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 1, "Ping", b"").unwrap();
        let (schema_ver, event_type, payload) = decode_event_value(&buf).unwrap();
        assert_eq!(schema_ver, 1);
        assert_eq!(event_type, "Ping");
        assert!(payload.is_empty());
    }

    #[test]
    fn event_value_decode_rejects_truncated_header() {
        assert!(decode_event_value(&[0u8; 5]).is_err());
        assert!(decode_event_value(&[]).is_err());
    }

    #[test]
    fn event_value_decode_rejects_truncated_event_type() {
        // Header says event_type is 10 bytes, but value is only 8 bytes total
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes()); // schema_version
        buf.extend_from_slice(&10u16.to_le_bytes()); // event_type_len = 10
        buf.extend_from_slice(b"ab"); // only 2 bytes of event_type
        assert!(decode_event_value(&buf).is_err());
    }

    #[test]
    fn event_value_reuses_buffer() {
        let mut buf = Vec::new();
        encode_event_value(&mut buf, 1, "First", b"aaa").unwrap();
        let first_cap = buf.capacity();
        encode_event_value(&mut buf, 2, "Second", b"bb").unwrap();
        // Buffer was reused, not reallocated (capacity preserved)
        assert!(buf.capacity() >= first_cap);
    }
```

Run: `cargo test -p nexus-fjall -- encoding::tests::event_value`
Expected: FAIL

**Step 6: Implement event value encode/decode**

```rust
/// Fixed header size: `[u32 LE schema_version][u16 LE event_type_len]`.
pub(crate) const EVENT_VALUE_HEADER_SIZE: usize = 6;

/// Encode event value into a reusable buffer.
///
/// Layout: `[u32 LE schema_version][u16 LE event_type_len][event_type UTF-8][payload]`
///
/// # Errors
///
/// Returns `EncodeError::EventTypeTooLong` if event_type exceeds `u16::MAX` bytes.
pub(crate) fn encode_event_value(
    buf: &mut Vec<u8>,
    schema_version: u32,
    event_type: &str,
    payload: &[u8],
) -> Result<(), EncodeError> {
    let event_type_bytes = event_type.as_bytes();
    let event_type_len = u16::try_from(event_type_bytes.len()).map_err(|_| {
        EncodeError::EventTypeTooLong {
            len: event_type_bytes.len(),
        }
    })?;

    buf.clear();
    buf.reserve(EVENT_VALUE_HEADER_SIZE + event_type_bytes.len() + payload.len());
    buf.extend_from_slice(&schema_version.to_le_bytes());
    buf.extend_from_slice(&event_type_len.to_le_bytes());
    buf.extend_from_slice(event_type_bytes);
    buf.extend_from_slice(payload);
    Ok(())
}

/// Decode event value into `(schema_version, event_type, payload)`.
///
/// # Errors
///
/// Returns `DecodeError` if the value is too short or contains invalid UTF-8.
pub(crate) fn decode_event_value(value: &[u8]) -> Result<(u32, &str, &[u8]), DecodeError> {
    if value.len() < EVENT_VALUE_HEADER_SIZE {
        return Err(DecodeError::ValueTooShort {
            min: EVENT_VALUE_HEADER_SIZE,
            actual: value.len(),
        });
    }

    let mut schema_buf = [0u8; 4];
    let mut len_buf = [0u8; 2];
    schema_buf.copy_from_slice(&value[..4]);
    len_buf.copy_from_slice(&value[4..6]);

    let schema_version = u32::from_le_bytes(schema_buf);
    let event_type_len = usize::from(u16::from_le_bytes(len_buf));
    let event_type_end = EVENT_VALUE_HEADER_SIZE + event_type_len;

    if value.len() < event_type_end {
        return Err(DecodeError::ValueTooShort {
            min: event_type_end,
            actual: value.len(),
        });
    }

    let event_type =
        std::str::from_utf8(&value[EVENT_VALUE_HEADER_SIZE..event_type_end])
            .map_err(DecodeError::InvalidUtf8)?;
    let payload = &value[event_type_end..];

    Ok((schema_version, event_type, payload))
}
```

Run: `cargo test -p nexus-fjall -- encoding::tests::event_value`
Expected: PASS

**Step 7: Add the DecodeError and EncodeError types**

These should be at the top of `encoding.rs` (before the functions):

```rust
use thiserror::Error;

/// Errors from decoding stored byte layouts.
#[derive(Debug, Error)]
pub(crate) enum DecodeError {
    #[error("invalid size: expected {expected}, got {actual}")]
    InvalidSize { expected: usize, actual: usize },

    #[error("value too short: need at least {min} bytes, got {actual}")]
    ValueTooShort { min: usize, actual: usize },

    #[error("invalid UTF-8 in event type")]
    InvalidUtf8(#[source] std::str::Utf8Error),
}

/// Errors from encoding values into byte layouts.
#[derive(Debug, Error)]
pub(crate) enum EncodeError {
    #[error("event type too long: {len} bytes (max {})", u16::MAX)]
    EventTypeTooLong { len: usize },
}
```

**Step 8: Run all encoding tests**

Run: `cargo test -p nexus-fjall -- encoding::tests`
Expected: PASS (all ~9 tests)

**Step 9: Commit**

```bash
git add crates/nexus-fjall/src/encoding.rs
git commit -m "feat(fjall): encoding module with TDD for key/value byte layouts"
```

---

## Task 3: Error type

**Files:**
- Create: `crates/nexus-fjall/src/error.rs`

**Step 1: Implement FjallError**

```rust
use thiserror::Error;

/// Errors produced by the fjall event store adapter.
#[derive(Debug, Error)]
pub enum FjallError {
    /// Fjall I/O or internal database error.
    #[error("fjall error: {0}")]
    Io(#[from] fjall::Error),

    /// Stored value has corrupt or unrecognizable byte layout.
    #[error("corrupt value in stream '{stream_id}' at version {version}")]
    CorruptValue { stream_id: String, version: u64 },

    /// Stream metadata has wrong byte size.
    #[error("corrupt metadata for stream '{stream_id}'")]
    CorruptMeta { stream_id: String },
}
```

**Step 2: Verify it compiles and satisfies trait bounds**

Add a compile-time assertion in a test:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn _assert_error_bounds<T: std::error::Error + Send + Sync + 'static>() {}

    #[test]
    fn fjall_error_satisfies_raw_event_store_bounds() {
        _assert_error_bounds::<FjallError>();
    }
}
```

Run: `cargo test -p nexus-fjall -- error::tests`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-fjall/src/error.rs
git commit -m "feat(fjall): FjallError type for adapter errors"
```

---

## Task 4: FjallStream (EventStream impl)

**Files:**
- Create: `crates/nexus-fjall/src/stream.rs`

**Step 1: Write the stream struct and EventStream impl**

```rust
use crate::encoding::decode_event_key;
use crate::encoding::decode_event_value;
use crate::error::FjallError;
use fjall::Slice;
use nexus_store::PersistedEnvelope;
use nexus_store::stream::EventStream;
use nexus::Version;

/// Lending cursor over fjall event rows.
///
/// Owns a `Vec<(Slice, Slice)>` collected from a fjall range scan.
/// Each `next()` call returns a `PersistedEnvelope` borrowing from
/// the current row's `Slice` buffers.
pub struct FjallStream {
    pub(crate) events: Vec<(Slice, Slice)>,
    pub(crate) pos: usize,
    pub(crate) stream_id: String,
    #[cfg(debug_assertions)]
    pub(crate) prev_version: Option<u64>,
}

impl EventStream for FjallStream {
    type Error = FjallError;

    async fn next(&mut self) -> Option<Result<PersistedEnvelope<'_>, Self::Error>> {
        if self.pos >= self.events.len() {
            return None;
        }

        let (key, value) = &self.events[self.pos];
        self.pos += 1;

        let (_stream_num, version) = match decode_event_key(key) {
            Ok(pair) => pair,
            Err(_) => {
                return Some(Err(FjallError::CorruptValue {
                    stream_id: self.stream_id.clone(),
                    version: 0,
                }));
            }
        };

        #[cfg(debug_assertions)]
        {
            if let Some(prev) = self.prev_version {
                debug_assert!(
                    version > prev,
                    "EventStream monotonicity violated: version {version} \
                     is not greater than previous {prev}",
                );
            }
            self.prev_version = Some(version);
        }

        let (schema_version, event_type, payload) = match decode_event_value(value) {
            Ok(parts) => parts,
            Err(_) => {
                return Some(Err(FjallError::CorruptValue {
                    stream_id: self.stream_id.clone(),
                    version,
                }));
            }
        };

        Some(Ok(PersistedEnvelope::new(
            &self.stream_id,
            Version::from_persisted(version),
            event_type,
            schema_version,
            payload,
            (),
        )))
    }
}
```

**Step 2: Write unit tests with synthetic data**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::{encode_event_key, encode_event_value};

    fn make_row(stream_num: u64, version: u64, schema_ver: u32, event_type: &str, payload: &[u8]) -> (Slice, Slice) {
        let key = encode_event_key(stream_num, version);
        let mut val_buf = Vec::new();
        encode_event_value(&mut val_buf, schema_ver, event_type, payload).unwrap();
        (Slice::from(key.as_slice()), Slice::from(val_buf.as_slice()))
    }

    #[tokio::test]
    async fn empty_stream_returns_none() {
        let mut stream = FjallStream {
            events: vec![],
            pos: 0,
            stream_id: "test-1".to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        };
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn yields_events_in_order() {
        let mut stream = FjallStream {
            events: vec![
                make_row(1, 1, 1, "Created", b"a"),
                make_row(1, 2, 1, "Updated", b"b"),
            ],
            pos: 0,
            stream_id: "test-1".to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        };

        let env = stream.next().await.unwrap().unwrap();
        assert_eq!(env.version().as_u64(), 1);
        assert_eq!(env.event_type(), "Created");
        assert_eq!(env.payload(), b"a");
        drop(env);

        let env = stream.next().await.unwrap().unwrap();
        assert_eq!(env.version().as_u64(), 2);
        assert_eq!(env.event_type(), "Updated");
        assert_eq!(env.payload(), b"b");
        drop(env);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn fused_after_exhaustion() {
        let mut stream = FjallStream {
            events: vec![make_row(1, 1, 1, "A", b"")],
            pos: 0,
            stream_id: "s".to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        };
        let _ = stream.next().await; // consume
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none()); // fused
    }

    #[tokio::test]
    async fn corrupt_value_returns_error() {
        let key = encode_event_key(1, 1);
        let corrupt_value: &[u8] = &[0u8; 3]; // too short for header
        let mut stream = FjallStream {
            events: vec![(Slice::from(key.as_slice()), Slice::from(corrupt_value))],
            pos: 0,
            stream_id: "bad".to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        };
        let result = stream.next().await.unwrap();
        assert!(result.is_err());
    }
}
```

Run: `cargo test -p nexus-fjall -- stream::tests`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-fjall/src/stream.rs
git commit -m "feat(fjall): FjallStream with EventStream impl and unit tests"
```

---

## Task 5: FjallStore struct and builder

**Files:**
- Create: `crates/nexus-fjall/src/builder.rs`
- Create: `crates/nexus-fjall/src/store.rs`

**Step 1: Implement builder**

```rust
use crate::encoding::decode_stream_meta;
use crate::error::FjallError;
use crate::store::FjallStore;
use fjall::{BlockSizePolicy, BloomConstructionPolicy, CompressionPolicy, FilterPolicy, KeyspaceCreateOptions};
use std::path::Path;
use std::sync::atomic::AtomicU64;

/// Builder for configuring and opening a [`FjallStore`].
pub struct FjallStoreBuilder {
    path: std::path::PathBuf,
    streams_config: Option<Box<dyn FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions>>,
    events_config: Option<Box<dyn FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions>>,
}

impl FjallStoreBuilder {
    pub(crate) fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            streams_config: None,
            events_config: None,
        }
    }

    /// Configure the `streams` keyspace (metadata point lookups).
    #[must_use]
    pub fn streams_config(
        mut self,
        f: impl FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions + 'static,
    ) -> Self {
        self.streams_config = Some(Box::new(f));
        self
    }

    /// Configure the `events` keyspace (event data range scans).
    #[must_use]
    pub fn events_config(
        mut self,
        f: impl FnOnce(KeyspaceCreateOptions) -> KeyspaceCreateOptions + 'static,
    ) -> Self {
        self.events_config = Some(Box::new(f));
        self
    }

    /// Open the database and recover stream ID counter.
    ///
    /// # Errors
    ///
    /// Returns `FjallError::Io` if the database cannot be opened.
    pub fn open(self) -> Result<FjallStore, FjallError> {
        let db = fjall::Config::new(&self.path)
            .open_transactional()?;

        let default_streams = KeyspaceCreateOptions::default()
            .filter_policy(FilterPolicy::Bloom(
                BloomConstructionPolicy::FalsePositiveRate(0.001),
            ))
            .expect_point_read_hits(true)
            .data_block_size_policy(BlockSizePolicy::from_bytes(4_096));

        let default_events = KeyspaceCreateOptions::default()
            .data_block_size_policy(BlockSizePolicy::from_bytes(32_768))
            .data_block_compression_policy(CompressionPolicy::Lz4);

        let streams_opts = match self.streams_config {
            Some(f) => f(default_streams),
            None => default_streams,
        };
        let events_opts = match self.events_config {
            Some(f) => f(default_events),
            None => default_events,
        };

        let streams = db.open_keyspace("streams", streams_opts)?;
        let events = db.open_keyspace("events", events_opts)?;

        // Recover next_stream_id by scanning streams keyspace
        let mut max_id: u64 = 0;
        for entry in streams.iter() {
            let (_, value) = entry?;
            if let Ok((numeric_id, _)) = decode_stream_meta(&value) {
                if numeric_id > max_id {
                    max_id = numeric_id;
                }
            }
        }
        // Next ID is max + 1, or 1 if no streams exist (0 is reserved)
        let next_id = if max_id == 0 { 1 } else { max_id + 1 };

        Ok(FjallStore {
            db,
            streams,
            events,
            next_stream_id: AtomicU64::new(next_id),
        })
    }
}
```

Note: The exact fjall API for `open_transactional()` and keyspace options will need to be verified against the actual fjall 2.x API. The builder pattern allows the implementation to adjust without changing the public API.

**Step 2: Implement FjallStore struct**

In `store.rs`:

```rust
use std::sync::atomic::AtomicU64;
use std::path::Path;
use crate::builder::FjallStoreBuilder;

/// Fjall-backed event store adapter.
///
/// Implements `RawEventStore` with two keyspaces:
/// - `streams`: per-stream metadata (numeric ID + current version)
/// - `events`: event data with fixed 16-byte keys
///
/// Uses `SingleWriterTxDatabase` for serialized atomic transactions.
pub struct FjallStore {
    pub(crate) db: fjall::SingleWriterTxDatabase,
    pub(crate) streams: fjall::SingleWriterTxKeyspace,
    pub(crate) events: fjall::SingleWriterTxKeyspace,
    pub(crate) next_stream_id: AtomicU64,
}

impl FjallStore {
    /// Create a builder for configuring and opening a `FjallStore`.
    #[must_use]
    pub fn builder(path: impl AsRef<Path>) -> FjallStoreBuilder {
        FjallStoreBuilder::new(path)
    }
}
```

**Step 3: Write test that store opens and closes**

In `store.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_closes_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();
        drop(store);
    }

    #[test]
    fn reopens_existing_database() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _store = FjallStore::builder(dir.path()).open().unwrap();
        }
        let _store = FjallStore::builder(dir.path()).open().unwrap();
    }
}
```

Run: `cargo test -p nexus-fjall -- store::tests`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/nexus-fjall/src/builder.rs crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): FjallStore struct with builder and keyspace setup"
```

---

## Task 6: RawEventStore::append (TDD)

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`

**Step 1: Write failing tests for append**

Add to `store.rs` tests:

```rust
    use nexus_store::{PendingEnvelope, pending_envelope};
    use nexus_store::error::AppendError;
    use nexus_store::RawEventStore;
    use nexus::Version;

    fn make_envelope(stream_id: &str, version: u64, event_type: &'static str, payload: &[u8]) -> PendingEnvelope<()> {
        pending_envelope(stream_id.to_owned())
            .version(Version::from_persisted(version))
            .event_type(event_type)
            .payload(payload.to_vec())
            .build_without_metadata()
    }

    #[tokio::test]
    async fn append_to_new_stream() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let envelopes = vec![
            make_envelope("order-1", 1, "Created", b"data1"),
        ];
        let result = store.append("order-1", Version::INITIAL, &envelopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_multiple_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let envelopes = vec![
            make_envelope("order-1", 1, "Created", b"a"),
            make_envelope("order-1", 2, "Updated", b"b"),
            make_envelope("order-1", 3, "Shipped", b"c"),
        ];
        let result = store.append("order-1", Version::INITIAL, &envelopes).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn append_detects_version_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        // First append succeeds
        let first = vec![make_envelope("order-1", 1, "Created", b"a")];
        store.append("order-1", Version::INITIAL, &first).await.unwrap();

        // Second append with wrong expected_version fails
        let second = vec![make_envelope("order-1", 2, "Updated", b"b")];
        let result = store.append("order-1", Version::INITIAL, &second).await;
        assert!(matches!(result, Err(AppendError::Conflict { .. })));
    }

    #[tokio::test]
    async fn append_to_nonexistent_stream_with_nonzero_version_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let envelopes = vec![make_envelope("order-1", 6, "Created", b"a")];
        let result = store.append("order-1", Version::from_persisted(5), &envelopes).await;
        assert!(matches!(result, Err(AppendError::Conflict { .. })));
    }

    #[tokio::test]
    async fn append_sequential_batches() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let batch1 = vec![make_envelope("s", 1, "A", b"1")];
        store.append("s", Version::INITIAL, &batch1).await.unwrap();

        let batch2 = vec![
            make_envelope("s", 2, "B", b"2"),
            make_envelope("s", 3, "C", b"3"),
        ];
        store.append("s", Version::from_persisted(1), &batch2).await.unwrap();
    }
```

Run: `cargo test -p nexus-fjall -- store::tests::append`
Expected: FAIL (append not implemented)

**Step 2: Implement RawEventStore::append**

In `store.rs`, add the trait impl:

```rust
use crate::encoding::{decode_stream_meta, encode_event_key, encode_event_value, encode_stream_meta};
use crate::error::FjallError;
use crate::stream::FjallStream;
use nexus::ErrorId;
use nexus::Version;
use nexus_store::PendingEnvelope;
use nexus_store::error::AppendError;
use nexus_store::raw::RawEventStore;
use nexus_store::stream::EventStream;
use std::sync::atomic::Ordering;

impl RawEventStore for FjallStore {
    type Error = FjallError;
    type Stream<'a> = FjallStream;

    async fn append(
        &self,
        stream_id: &str,
        expected_version: Version,
        envelopes: &[PendingEnvelope<()>],
    ) -> Result<(), AppendError<Self::Error>> {
        let mut tx = self.db.write_tx();

        // 1. Look up stream metadata
        let (numeric_id, current_version) = match tx
            .get(&self.streams, stream_id)
            .map_err(|e| AppendError::Store(FjallError::Io(e)))?
        {
            Some(meta_bytes) => decode_stream_meta(&meta_bytes).map_err(|_| {
                AppendError::Store(FjallError::CorruptMeta {
                    stream_id: stream_id.to_owned(),
                })
            })?,
            None => {
                // New stream — expected_version must be 0
                if expected_version.as_u64() != 0 {
                    return Err(AppendError::Conflict {
                        stream_id: ErrorId::from_display(&stream_id),
                        expected: expected_version,
                        actual: Version::INITIAL,
                    });
                }
                let nid = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
                (nid, 0)
            }
        };

        // 2. Version check
        if current_version != expected_version.as_u64() {
            return Err(AppendError::Conflict {
                stream_id: ErrorId::from_display(&stream_id),
                expected: expected_version,
                actual: Version::from_persisted(current_version),
            });
        }

        // 3. Validate sequential envelope versions
        for (i, env) in envelopes.iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let expected_env_ver = expected_version.as_u64() + 1 + i_u64;
            if env.version().as_u64() != expected_env_ver {
                return Err(AppendError::Conflict {
                    stream_id: ErrorId::from_display(&stream_id),
                    expected: Version::from_persisted(expected_env_ver),
                    actual: env.version(),
                });
            }
        }

        // 4. Insert events — stack key buf, reused value buf
        let mut key_buf = encode_event_key(numeric_id, 0);
        let mut value_buf = Vec::new();

        for env in envelopes {
            key_buf[8..].copy_from_slice(&env.version().as_u64().to_be_bytes());

            encode_event_value(
                &mut value_buf,
                env.schema_version(),
                env.event_type(),
                env.payload(),
            )
            .map_err(|_| {
                AppendError::Store(FjallError::CorruptValue {
                    stream_id: stream_id.to_owned(),
                    version: env.version().as_u64(),
                })
            })?;

            tx.insert(&self.events, &key_buf, &value_buf);
        }

        // 5. Update stream metadata
        let envelope_count = u64::try_from(envelopes.len()).unwrap_or(u64::MAX);
        let new_version = current_version + envelope_count;
        let meta = encode_stream_meta(numeric_id, new_version);
        tx.insert(&self.streams, stream_id, &meta);

        // 6. Atomic commit
        tx.commit().map_err(|e| AppendError::Store(FjallError::Io(e)))?;

        Ok(())
    }

    async fn read_stream(
        &self,
        _stream_id: &str,
        _from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        // Placeholder — implemented in Task 7
        Ok(FjallStream {
            events: vec![],
            pos: 0,
            stream_id: _stream_id.to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        })
    }
}
```

**Step 3: Run append tests**

Run: `cargo test -p nexus-fjall -- store::tests::append`
Expected: PASS (5 tests)

**Step 4: Commit**

```bash
git add crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): RawEventStore::append with atomic transactions"
```

---

## Task 7: RawEventStore::read_stream (TDD)

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`

**Step 1: Write failing tests for read_stream**

```rust
    #[tokio::test]
    async fn read_empty_stream_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let mut stream = store
            .read_stream("nonexistent", Version::INITIAL)
            .await
            .unwrap();
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_after_append_returns_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let envelopes = vec![
            make_envelope("s1", 1, "Created", b"payload-1"),
            make_envelope("s1", 2, "Updated", b"payload-2"),
        ];
        store.append("s1", Version::INITIAL, &envelopes).await.unwrap();

        let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();

        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.version().as_u64(), 1);
        assert_eq!(e1.event_type(), "Created");
        assert_eq!(e1.payload(), b"payload-1");
        assert_eq!(e1.stream_id(), "s1");
        drop(e1);

        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.version().as_u64(), 2);
        assert_eq!(e2.event_type(), "Updated");
        assert_eq!(e2.payload(), b"payload-2");
        drop(e2);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn read_with_from_version_skips_earlier() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let envelopes = vec![
            make_envelope("s1", 1, "A", b"1"),
            make_envelope("s1", 2, "B", b"2"),
            make_envelope("s1", 3, "C", b"3"),
        ];
        store.append("s1", Version::INITIAL, &envelopes).await.unwrap();

        // Read from version 2 — should skip event 1
        let mut stream = store
            .read_stream("s1", Version::from_persisted(2))
            .await
            .unwrap();

        let e = stream.next().await.unwrap().unwrap();
        assert_eq!(e.version().as_u64(), 2);
        drop(e);

        let e = stream.next().await.unwrap().unwrap();
        assert_eq!(e.version().as_u64(), 3);
        drop(e);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn streams_are_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let e1 = vec![make_envelope("s1", 1, "A", b"s1-data")];
        let e2 = vec![make_envelope("s2", 1, "B", b"s2-data")];
        store.append("s1", Version::INITIAL, &e1).await.unwrap();
        store.append("s2", Version::INITIAL, &e2).await.unwrap();

        let mut stream1 = store.read_stream("s1", Version::INITIAL).await.unwrap();
        let env = stream1.next().await.unwrap().unwrap();
        assert_eq!(env.event_type(), "A");
        assert_eq!(env.payload(), b"s1-data");
        drop(env);
        assert!(stream1.next().await.is_none());

        let mut stream2 = store.read_stream("s2", Version::INITIAL).await.unwrap();
        let env = stream2.next().await.unwrap().unwrap();
        assert_eq!(env.event_type(), "B");
        assert_eq!(env.payload(), b"s2-data");
        drop(env);
        assert!(stream2.next().await.is_none());
    }
```

Run: `cargo test -p nexus-fjall -- store::tests::read`
Expected: FAIL (read_stream returns empty placeholder)

**Step 2: Implement read_stream**

Replace the placeholder `read_stream` in the `RawEventStore` impl:

```rust
    async fn read_stream(
        &self,
        stream_id: &str,
        from: Version,
    ) -> Result<Self::Stream<'_>, Self::Error> {
        // 1. Look up numeric stream ID
        let numeric_id = match self.streams.get(stream_id)? {
            Some(meta_bytes) => {
                let (nid, _version) = decode_stream_meta(&meta_bytes).map_err(|_| {
                    FjallError::CorruptMeta {
                        stream_id: stream_id.to_owned(),
                    }
                })?;
                nid
            }
            None => {
                // Stream doesn't exist — return empty cursor
                return Ok(FjallStream {
                    events: vec![],
                    pos: 0,
                    stream_id: stream_id.to_owned(),
                    #[cfg(debug_assertions)]
                    prev_version: None,
                });
            }
        };

        // 2. Build range bounds
        let start_key = encode_event_key(numeric_id, from.as_u64());
        let end_key = encode_event_key(numeric_id, u64::MAX);

        // 3. Collect range into owned Vec
        let mut collected = Vec::new();
        for entry in self.events.range(start_key..=end_key) {
            let (key, value) = entry?;
            collected.push((key, value));
        }

        Ok(FjallStream {
            events: collected,
            pos: 0,
            stream_id: stream_id.to_owned(),
            #[cfg(debug_assertions)]
            prev_version: None,
        })
    }
```

**Step 3: Run read tests**

Run: `cargo test -p nexus-fjall -- store::tests::read`
Expected: PASS (4 tests)

**Step 4: Run ALL tests**

Run: `cargo test -p nexus-fjall`
Expected: PASS (all tests across encoding, error, stream, store modules)

**Step 5: Commit**

```bash
git add crates/nexus-fjall/src/store.rs
git commit -m "feat(fjall): RawEventStore::read_stream with range scan"
```

---

## Task 8: Conformance tests, persistence, and final verification

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs` (more tests)
- Modify: `crates/nexus-fjall/src/lib.rs` (pub exports)

**Step 1: Add persistence test (survive restart)**

```rust
    #[tokio::test]
    async fn data_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();

        // Write events, close store
        {
            let store = FjallStore::builder(dir.path()).open().unwrap();
            let envs = vec![
                make_envelope("s1", 1, "Created", b"persist-me"),
            ];
            store.append("s1", Version::INITIAL, &envs).await.unwrap();
        }

        // Reopen and verify
        {
            let store = FjallStore::builder(dir.path()).open().unwrap();
            let mut stream = store.read_stream("s1", Version::INITIAL).await.unwrap();
            let env = stream.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), "Created");
            assert_eq!(env.payload(), b"persist-me");
        }
    }

    #[tokio::test]
    async fn stream_id_counter_recovers_after_reopen() {
        let dir = tempfile::tempdir().unwrap();

        // Create two streams, close
        {
            let store = FjallStore::builder(dir.path()).open().unwrap();
            let e1 = vec![make_envelope("s1", 1, "A", b"")];
            let e2 = vec![make_envelope("s2", 1, "B", b"")];
            store.append("s1", Version::INITIAL, &e1).await.unwrap();
            store.append("s2", Version::INITIAL, &e2).await.unwrap();
        }

        // Reopen, create third stream — should not collide
        {
            let store = FjallStore::builder(dir.path()).open().unwrap();
            let e3 = vec![make_envelope("s3", 1, "C", b"")];
            store.append("s3", Version::INITIAL, &e3).await.unwrap();

            // All three streams readable and isolated
            let mut st = store.read_stream("s3", Version::INITIAL).await.unwrap();
            let env = st.next().await.unwrap().unwrap();
            assert_eq!(env.event_type(), "C");
        }
    }

    #[tokio::test]
    async fn version_tracks_across_appends() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let b1 = vec![make_envelope("s", 1, "A", b"")];
        store.append("s", Version::INITIAL, &b1).await.unwrap();

        let b2 = vec![make_envelope("s", 2, "B", b"")];
        store.append("s", Version::from_persisted(1), &b2).await.unwrap();

        // Read all — should have 2 events
        let mut stream = store.read_stream("s", Version::INITIAL).await.unwrap();
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.version().as_u64(), 1);
        drop(e1);
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.version().as_u64(), 2);
        drop(e2);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn large_batch_append() {
        let dir = tempfile::tempdir().unwrap();
        let store = FjallStore::builder(dir.path()).open().unwrap();

        let envelopes: Vec<_> = (1..=100)
            .map(|i| make_envelope("big", i, "Tick", &[0u8; 256]))
            .collect();
        store.append("big", Version::INITIAL, &envelopes).await.unwrap();

        let mut stream = store.read_stream("big", Version::INITIAL).await.unwrap();
        let mut count = 0u64;
        while let Some(result) = stream.next().await {
            let _ = result.unwrap();
            count += 1;
        }
        assert_eq!(count, 100);
    }
```

Run: `cargo test -p nexus-fjall`
Expected: PASS (all tests)

**Step 2: Finalize lib.rs exports**

```rust
pub mod builder;
pub mod encoding;
pub mod error;
pub mod store;
pub mod stream;

pub use builder::FjallStoreBuilder;
pub use error::FjallError;
pub use store::FjallStore;
pub use stream::FjallStream;
```

**Step 3: Run full workspace checks**

Run: `cargo fmt --all --check`
Run: `cargo clippy -p nexus-fjall --all-targets -- --deny warnings`
Run: `cargo test -p nexus-fjall`

Expected: All PASS.

**Step 4: Commit**

```bash
git add crates/nexus-fjall/
git commit -m "feat(fjall): conformance tests, persistence, and pub API"
```

---

## Post-Implementation Notes

**What we built:**
- `FjallStore` implementing `RawEventStore<()>` with two keyspaces
- `FjallStream` implementing `EventStream<()>` as a lending cursor
- `encoding` module with pure, tested encode/decode functions
- Builder with sensible defaults and full fjall config passthrough

**What's next (future tasks, NOT this plan):**
- `nexus-rkyv` crate — rkyv `Codec` implementation
- Wire up `EventStore<FjallStore, RkyvCodec>` facade
- Metadata support (generalize from `M = ()`)
- Benchmarks
- Event subscriptions / projections
