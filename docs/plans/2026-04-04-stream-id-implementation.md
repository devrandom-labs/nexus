# StreamId Newtype Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace raw `&str` stream IDs with a validated `StreamId` newtype across the entire stack, giving compile-time guarantees and eliminating adapter-level validation.

**Architecture:** `StreamId` lives in `nexus` kernel. `PendingEnvelope` and `RawEventStore` take `StreamId`/`&StreamId`. `EventStore` facade constructs `StreamId` from aggregate name + ID. Adapters call `.as_str()` for storage keys.

**Tech Stack:** Rust 2024, thiserror, nexus kernel + nexus-store + nexus-fjall

**Lint constraints:** Workspace lints deny `unwrap_used`, `expect_used`, `panic`, `as_conversions`, `shadow_reuse`, `shadow_same`, `shadow_unrelated`. All `#[allow]` must have `reason = "..."`.

---

## Task 1: StreamId type in nexus kernel

**Files:**
- Create: `crates/nexus/src/stream_id.rs`
- Modify: `crates/nexus/src/lib.rs`

**Step 1: Write failing tests**

In `stream_id.rs`, add test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_valid_stream_id() {
        let sid = StreamId::new("Order", &"abc123").unwrap();
        assert_eq!(sid.as_str(), "Order-abc123");
    }

    #[test]
    fn rejects_empty_name() {
        assert!(matches!(StreamId::new("", &"id"), Err(InvalidStreamId::EmptyName)));
    }

    #[test]
    fn rejects_empty_id() {
        assert!(matches!(StreamId::new("Order", &""), Err(InvalidStreamId::EmptyId)));
    }

    #[test]
    fn rejects_invalid_name_chars() {
        assert!(matches!(StreamId::new("Order-1", &"id"), Err(InvalidStreamId::InvalidNameChar { ch: '-' })));
        assert!(matches!(StreamId::new("Order/x", &"id"), Err(InvalidStreamId::InvalidNameChar { ch: '/' })));
        assert!(matches!(StreamId::new("日本語", &"id"), Err(InvalidStreamId::InvalidNameChar { .. })));
    }

    #[test]
    fn allows_underscore_in_name() {
        let sid = StreamId::new("my_aggregate", &"id1").unwrap();
        assert_eq!(sid.as_str(), "my_aggregate-id1");
    }

    #[test]
    fn rejects_null_bytes() {
        assert!(matches!(StreamId::new("Order", &"id\0evil"), Err(InvalidStreamId::NullByte)));
    }

    #[test]
    fn rejects_too_long() {
        let long_id = "a".repeat(MAX_STREAM_ID_LEN);
        assert!(matches!(StreamId::new("Order", &long_id), Err(InvalidStreamId::TooLong { .. })));
    }

    #[test]
    fn from_persisted_valid() {
        let sid = StreamId::from_persisted("Order-abc123".to_owned()).unwrap();
        assert_eq!(sid.as_str(), "Order-abc123");
    }

    #[test]
    fn from_persisted_rejects_empty() {
        assert!(matches!(StreamId::from_persisted("".to_owned()), Err(InvalidStreamId::Empty)));
    }

    #[test]
    fn from_persisted_rejects_null_bytes() {
        assert!(matches!(StreamId::from_persisted("a\0b".to_owned()), Err(InvalidStreamId::NullByte)));
    }

    #[test]
    fn display_shows_inner_string() {
        let sid = StreamId::new("Order", &"123").unwrap();
        assert_eq!(format!("{sid}"), "Order-123");
    }

    #[test]
    fn clone_and_eq() {
        let a = StreamId::new("Order", &"1").unwrap();
        let b = a.clone();
        assert_eq!(a, b);
    }
}
```

**Step 2: Implement StreamId**

```rust
use std::fmt;
use thiserror::Error;

/// Maximum length of a stream ID in bytes.
pub const MAX_STREAM_ID_LEN: usize = 512;

/// A validated event stream identifier.
///
/// Format: `{aggregate_name}-{aggregate_id}` where the name is ASCII
/// alphanumeric + underscore. Cannot be empty, contain null bytes,
/// or exceed [`MAX_STREAM_ID_LEN`] bytes.
///
/// Constructed via [`StreamId::new`] (from name + ID) or
/// [`StreamId::from_persisted`] (for store adapters rehydrating from DB).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamId(String);

impl StreamId {
    /// Construct from aggregate name and ID.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStreamId`] if:
    /// - `name` is empty or contains non-ASCII-alphanumeric/underscore characters
    /// - `id` displays as empty
    /// - The formatted result contains null bytes or exceeds [`MAX_STREAM_ID_LEN`]
    pub fn new(name: &str, id: &impl fmt::Display) -> Result<Self, InvalidStreamId> {
        if name.is_empty() {
            return Err(InvalidStreamId::EmptyName);
        }
        for ch in name.chars() {
            if !ch.is_ascii_alphanumeric() && ch != '_' {
                return Err(InvalidStreamId::InvalidNameChar { ch });
            }
        }

        let formatted = format!("{name}-{id}");

        if formatted.len() == name.len() + 1 {
            // id.Display produced empty string (format is "name-")
            return Err(InvalidStreamId::EmptyId);
        }
        if formatted.contains('\0') {
            return Err(InvalidStreamId::NullByte);
        }
        if formatted.len() > MAX_STREAM_ID_LEN {
            return Err(InvalidStreamId::TooLong {
                len: formatted.len(),
                max: MAX_STREAM_ID_LEN,
            });
        }

        Ok(Self(formatted))
    }

    /// Reconstruct from a persisted string.
    ///
    /// Validates non-empty, no null bytes, and max length.
    /// Does NOT validate the `{name}-{id}` format — historical data
    /// may use different formats.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidStreamId`] if the string is empty, contains
    /// null bytes, or exceeds [`MAX_STREAM_ID_LEN`].
    pub fn from_persisted(s: impl Into<String>) -> Result<Self, InvalidStreamId> {
        let inner: String = s.into();
        if inner.is_empty() {
            return Err(InvalidStreamId::Empty);
        }
        if inner.contains('\0') {
            return Err(InvalidStreamId::NullByte);
        }
        if inner.len() > MAX_STREAM_ID_LEN {
            return Err(InvalidStreamId::TooLong {
                len: inner.len(),
                max: MAX_STREAM_ID_LEN,
            });
        }
        Ok(Self(inner))
    }

    /// The stream ID as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for StreamId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Errors from constructing a [`StreamId`].
#[derive(Debug, Error)]
pub enum InvalidStreamId {
    /// Stream ID must not be empty.
    #[error("stream ID must not be empty")]
    Empty,

    /// Aggregate name must not be empty.
    #[error("aggregate name must not be empty")]
    EmptyName,

    /// Aggregate name contains an invalid character.
    #[error("aggregate name contains invalid character '{ch}' (only ASCII alphanumeric and underscore allowed)")]
    InvalidNameChar { ch: char },

    /// Aggregate ID must not be empty.
    #[error("aggregate ID must not be empty")]
    EmptyId,

    /// Stream ID must not contain null bytes.
    #[error("stream ID contains null byte")]
    NullByte,

    /// Stream ID exceeds maximum length.
    #[error("stream ID too long: {len} bytes (max {max})")]
    TooLong { len: usize, max: usize },
}
```

**Step 3: Export from lib.rs**

Add `mod stream_id;` and `pub use stream_id::{StreamId, InvalidStreamId, MAX_STREAM_ID_LEN};` to `crates/nexus/src/lib.rs`.

**Step 4: Verify**

Run: `cargo test -p nexus`
Run: `cargo clippy -p nexus --all-targets -- --deny warnings`

**Step 5: Commit**

```bash
git commit -m "feat(nexus): add StreamId newtype with validated construction"
```

---

## Task 2: Update PendingEnvelope to use StreamId

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`
- Modify: `crates/nexus-store/src/lib.rs` (re-export StreamId if needed)

**Changes:**

1. In `PendingEnvelope<M>`, change `stream_id: String` to `stream_id: StreamId`.
2. In `PendingEnvelope::stream_id()` accessor, return `&StreamId` instead of `&str`.
3. In the typestate builder (`WithStreamId`), change field from `String` to `StreamId`.
4. The `pending_envelope()` entry point takes `StreamId` instead of `String`.
5. Update all usages of `stream_id()` that expected `&str` — they now get `&StreamId` and should call `.as_str()` where raw str is needed.

Key change in builder chain:
```rust
// Before:
pub const fn pending_envelope(stream_id: String) -> WithStreamId { ... }

// After:
pub fn pending_envelope(stream_id: StreamId) -> WithStreamId { ... }
```

**Step 1: Make the changes to envelope.rs**

**Step 2: Fix all compilation errors in nexus-store**

The `save_with_encoder` in `event_store.rs` constructs `pending_envelope(stream_id.into())`. Change to `pending_envelope(stream_id.clone())` since stream_id will be `&StreamId` and `StreamId: Clone`.

The `RawEventStore::append` still takes `&str` at this point — we'll change that in Task 3. For now, the `save_with_encoder` passes `stream_id.as_str()` to `store.append()`.

**Step 3: Fix all test compilation errors**

Tests that construct `pending_envelope("stream-1".into())` need to use `pending_envelope(StreamId::from_persisted("stream-1").unwrap())` or a test helper.

**Step 4: Verify**

Run: `cargo test -p nexus-store`
Run: `cargo clippy -p nexus-store --all-targets -- --deny warnings`

**Step 5: Commit**

```bash
git commit -m "feat(store): PendingEnvelope takes StreamId for compile-time validity"
```

---

## Task 3: Update RawEventStore and Repository traits

**Files:**
- Modify: `crates/nexus-store/src/raw.rs`
- Modify: `crates/nexus-store/src/repository.rs`
- Modify: `crates/nexus-store/src/event_store.rs`
- Modify: `crates/nexus-store/src/testing.rs`

**Changes:**

1. `RawEventStore::append` — change `stream_id: &str` to `stream_id: &StreamId`
2. `RawEventStore::read_stream` — change `stream_id: &str` to `stream_id: &StreamId`
3. `Repository::load` — change `stream_id: &str` to `stream_id: &StreamId`
4. `Repository::save` — change `stream_id: &str` to `stream_id: &StreamId`
5. `InMemoryStore` — call `stream_id.as_str()` for HashMap keys and `ErrorId::from_display`
6. `save_with_encoder` — change parameter to `stream_id: &StreamId`
7. `EventStore<S,C,U>` and `ZeroCopyEventStore<S,C,U>` Repository impls — construct `StreamId` from `aggregate.state().name()` + `aggregate.id()` and pass `&stream_id`

The `EventStore` facade's `load`/`save` is where the `StreamId` is constructed from the aggregate:
```rust
async fn load(&self, stream_id: &StreamId, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
    let mut stream = self.store.read_stream(stream_id, Version::INITIAL).await ...
}

async fn save(&self, stream_id: &StreamId, aggregate: &mut AggregateRoot<A>) -> Result<(), StoreError> {
    save_with_encoder(&self.store, &self.upcasters, |event| { ... }, stream_id, aggregate).await
}
```

**Step 1: Update trait signatures**

**Step 2: Update InMemoryStore** — `stream_id.as_str().to_owned()` for HashMap keys

**Step 3: Update EventStore and ZeroCopyEventStore impls**

**Step 4: Fix all test compilation errors** — tests create StreamId via helper or `from_persisted`

**Step 5: Verify**

Run: `cargo test -p nexus-store`
Run: `cargo clippy -p nexus-store --all-targets -- --deny warnings`

**Step 6: Commit**

```bash
git commit -m "feat(store)!: RawEventStore and Repository take &StreamId"
```

---

## Task 4: Update nexus-fjall adapter

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`
- Modify: `crates/nexus-fjall/src/error.rs`
- Modify: `crates/nexus-fjall/tests/nasa_break_everything.rs`

**Changes:**

1. `FjallStore::append` — takes `&StreamId`, calls `.as_str()` for fjall keys
2. `FjallStore::read_stream` — takes `&StreamId`, calls `.as_str()` for lookups
3. **Remove** `FjallError::EmptyStreamId` variant — impossible now
4. **Remove** empty stream_id validation checks in both methods
5. Keep `FjallError::StreamIdExhausted` — numeric counter overflow is still possible
6. Remove `set_next_stream_id_for_testing` — or keep if still needed for overflow test
7. Update all test helpers to construct `StreamId` properly
8. Update NASA tests:
   - `attack_empty_string_stream_id_returns_error` → test that `StreamId::from_persisted("")` fails (move to kernel test)
   - `attack_empty_string_stream_id_read_returns_error` → same
   - Evil stream ID tests use `StreamId::from_persisted()` for valid ones, test rejection for invalid ones at the `StreamId` level
9. Update unit tests in `store.rs` — `make_envelope` helper uses `StreamId`

**Step 1: Update store.rs impl**

**Step 2: Remove EmptyStreamId from error.rs**

**Step 3: Update all tests**

**Step 4: Verify**

Run: `cargo test -p nexus-fjall`
Run: `cargo clippy -p nexus-fjall --all-targets -- --deny warnings`

**Step 5: Commit**

```bash
git commit -m "feat(fjall)!: use StreamId, remove adapter-level validation"
```

---

## Task 5: Update examples and final verification

**Files:**
- Modify: `examples/store-inmemory/src/main.rs` (if it uses stream_id strings)
- Modify: `examples/inmemory/src/main.rs` (if applicable)
- Modify: `CLAUDE.md` (document StreamId in architecture section)

**Step 1: Fix any remaining compilation errors in examples**

**Step 2: Run full workspace checks**

Run: `cargo test --all`
Run: `cargo clippy --all-targets -- --deny warnings`
Run: `cargo fmt --all --check`
Run: `cargo hakari verify`

**Step 3: Commit**

```bash
git commit -m "feat: update examples and docs for StreamId"
```

---

## Post-Implementation Notes

**What we built:**
- `StreamId` newtype in nexus kernel with validated construction
- Compile-time guarantee in `PendingEnvelope` — can't pass invalid stream IDs
- `RawEventStore` and `Repository` traits use `&StreamId`
- Adapter-level validation removed from nexus-fjall (type system handles it)

**What's next (future tasks, NOT this plan):**
- `NexusId` with UUIDv4 (separate crate feature, not blocking)
- `AggregateRoot::stream_id()` convenience method
