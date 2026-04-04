# StreamId Newtype Design

**Date:** 2026-04-04
**Status:** Approved

## Goal

Replace raw `&str` stream IDs with a validated `StreamId` newtype. Eliminates empty-key panics, prevents storage-level attacks, and gives compile-time guarantees in `PendingEnvelope` and `RawEventStore`.

## Problem

`stream_id` is a raw `&str` with zero invariant enforcement across the entire stack. It flows from user code through Repository, EventStore facade, RawEventStore, and into concrete adapters without any validation. Consequences:
- Empty string causes panic in fjall's lsm-tree
- No length bounds (could exhaust storage key limits)
- No character validation (null bytes, path traversal strings)
- Each adapter must independently validate, which is error-prone

## Design

### StreamId Type (`nexus` kernel)

```rust
// crates/nexus/src/stream_id.rs

pub struct StreamId(String);

const MAX_STREAM_ID_LEN: usize = 512;

impl StreamId {
    /// Construct from aggregate name + ID.
    pub fn new(name: &str, id: &impl Display) -> Result<Self, InvalidStreamId>

    /// Reconstruct from persisted string (for store adapters).
    pub fn from_persisted(s: impl Into<String>) -> Result<Self, InvalidStreamId>

    pub fn as_str(&self) -> &str
}
```

Derives: `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`
Implements: `Display` (delegates to inner string), `AsRef<str>`

### Validation Rules

- **Non-empty** after formatting
- **Max 512 bytes** total length
- **Name portion**: non-empty, ASCII alphanumeric + underscore only
- **Separator**: single `-`
- **ID portion**: non-empty
- **No null bytes** in the entire string

`from_persisted` validates non-empty, max length, and no null bytes. It does NOT validate the name-id format since historical data may use different formats.

### Error Type

```rust
#[derive(Debug, Error)]
pub enum InvalidStreamId {
    Empty,
    EmptyName,
    InvalidNameChar { ch: char },
    EmptyId,
    NullByte,
    TooLong { len: usize, max: usize },
}
```

### Integration

| Crate | Change |
|-------|--------|
| `nexus` | Add `StreamId`, `InvalidStreamId`. Add `AggregateRoot::stream_id()` method. |
| `nexus-store` | `PendingEnvelope` builder takes `StreamId` not `String`. `RawEventStore` takes `&StreamId` not `&str`. `Repository` takes `&StreamId`. `EventStore` facade constructs `StreamId` from aggregate. |
| `nexus-fjall` | Takes `&StreamId`, calls `.as_str()` for keys. Remove `EmptyStreamId` variant and validation. |
| `nexus-macros` | No changes. |

### Read Path

`PersistedEnvelope<'a>` keeps `stream_id: &'a str` (borrowed from DB row). No validation on read — data was valid when written, and validating would require allocation.

### Security

StreamId's unpredictability comes from the aggregate `Id` type, not StreamId itself. The framework-provided `NexusId` (when implemented) will use UUIDv4 (122 bits CSPRNG). Users with custom `Id` types are responsible for their own entropy.

### What This Eliminates

- `FjallError::EmptyStreamId` — impossible at the type level
- Empty stream_id validation in every adapter — gone
- Unbounded key length — capped at 512 bytes
- Null byte injection — rejected at construction
- Character-based attacks (path traversal, SQL injection in name portion) — name is alphanumeric + underscore only
