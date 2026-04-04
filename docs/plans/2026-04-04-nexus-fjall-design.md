# nexus-fjall: Fjall Event Store Adapter

## Overview

First concrete `RawEventStore` adapter for Nexus, backed by fjall (embedded LSM-tree key-value store). Write-side only for this phase: `append` + `read_stream` for aggregate rehydration.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Sync/async bridge | Direct sync, ready futures | No runtime dependency. Fjall is sync, calls complete in microseconds (memtable/block cache). Event loop model — caller wraps in actor if needed. |
| Database type | `SingleWriterTxDatabase` | Enforces single-writer at type level. Serialized transactions eliminate race conditions in version checks. Perfect for event loop model. |
| Event key format | Fixed 16-byte `[u64 BE stream_num][u64 BE version]` | Branchless memcmp in LSM operations. Numeric stream ID mapping avoids variable-length string comparisons on every compaction, binary search, and merge. |
| Value format | 6-byte header + variable event_type + payload | `[u32 LE schema_version][u16 LE event_type_len][event_type UTF-8][payload]`. Minimal overhead. event_type stored as string for PersistedEnvelope contract. |
| Stream ID mapping | Monotonic `AtomicU64` counter | Maps string stream_id to numeric ID for fixed-size event keys. Recovered at startup by scanning `streams` keyspace. Single-writer guarantee prevents allocation races. |
| Read cursor | Collect range scan into `Vec<(Slice, Slice)>` | Fjall Slice is `Arc<[u8]>` — ref-counted, no deep copy. Owned cursor with clean GAT lifetimes. PersistedEnvelope borrows from cursor's buffers. |
| Codec | Not included | Adapter operates on raw bytes. Codec is a separate concern (nexus-rkyv planned). Tests use raw bytes directly. |

## Keyspace Layout

### `streams` — per-stream metadata (point lookups)

```
Key:   stream_id UTF-8 bytes (variable)
Value: [u64 LE: numeric_id][u64 LE: current_version] = 16 bytes fixed
```

Configuration:
- Bloom filter: `FalsePositiveRate(0.001)`
- `expect_point_read_hits(true)`
- Block size: 4KB (small values, point lookups)
- No compression (16-byte values not worth it)

### `events` — event data (range scans)

```
Key:   [u64 BE: stream_numeric_id][u64 BE: version] = 16 bytes fixed
Value: [u32 LE: schema_version][u16 LE: event_type_len][event_type UTF-8 bytes][payload bytes]
```

Configuration:
- Block size: 32KB (large, optimized for sequential range scan throughput)
- Compression: LZ4 (fast decompression for range scans)
- Prefix bloom on first 8 bytes (stream_id lookups within SSTs)

## Write Path (append)

```
append(stream_id, expected_version, envelopes)
  1. tx = db.write_tx()                             // serialized transaction
  2. tx.get(&streams, stream_id)                     // point lookup (bloom filter)
     - Found: parse (numeric_id, current_version)
       - current_version != expected_version? -> AppendError::Conflict
     - Not found: expected_version != 0? -> AppendError::Conflict
       - numeric_id = next_id.fetch_add(1, Relaxed)
  3. Validate envelope versions sequential from expected_version + 1
  4. Stack-allocate key buf [u8; 16], write numeric_id BE once
     For each envelope:
       - Overwrite version bytes in key buf
       - Encode value into reusable Vec (clear + extend, no realloc)
       - tx.insert(&events, &key_buf, &value_buf)
  5. tx.insert(&streams, stream_id, encode_stream_meta(numeric_id, new_version))
  6. tx.commit()                                     // atomic cross-keyspace
```

Zero heap allocation per event in the hot loop: stack key buffer, reused value buffer.

## Read Path (read_stream)

```
read_stream(stream_id, from_version)
  1. streams.get(stream_id)                          // point lookup
     - Not found: return empty FjallStream
     - Found: parse numeric_id
  2. Build range bounds on stack:
     - start = [numeric_id BE][from_version BE]
     - end   = [numeric_id BE][u64::MAX BE]
  3. Collect events.range(start..=end) into Vec<(Slice, Slice)>
  4. Return FjallStream { events, pos: 0, stream_id, prev_version: None }
```

## Lending Cursor (FjallStream)

```rust
struct FjallStream {
    events: Vec<(Slice, Slice)>,   // owned key-value pairs from range scan
    pos: usize,                     // current position
    stream_id: String,              // owned, PersistedEnvelope borrows &str
    prev_version: Option<u64>,      // monotonicity assertion
}
```

Each `next()` call:
1. Borrow `&self.events[self.pos]` — Slice stays alive in Vec
2. Parse version from key `[8..16]`, enforce monotonicity
3. Parse schema_version, event_type, payload from value
4. Return `PersistedEnvelope<'_>` borrowing from Slice and self.stream_id
5. Advance pos

GAT lifetime safety: PersistedEnvelope borrows with `'a` tied to `&'a mut self`. Previous envelope must be dropped before next call (Rust enforces via mutable borrow).

## Error Type

```rust
enum FjallError {
    Io(fjall::Error),
    CorruptValue { stream_id: String, version: u64 },
    CorruptMeta { stream_id: String },
}
```

## Store Struct

```rust
struct FjallStore {
    db: SingleWriterTxDatabase,
    streams: SingleWriterTxKeyspace,
    events: SingleWriterTxKeyspace,
    next_stream_id: AtomicU64,
}
```

## Builder

```rust
FjallStore::builder("/path/to/data")
    .streams_config(|opts| opts
        .filter_policy(FilterPolicy::Bloom(BloomConstructionPolicy::FalsePositiveRate(0.001)))
        .expect_point_read_hits(true)
        .data_block_size_policy(BlockSizePolicy::from_bytes(4096)))
    .events_config(|opts| opts
        .data_block_size_policy(BlockSizePolicy::from_bytes(32 * 1024))
        .data_block_compression_policy(CompressionPolicy::Lz4))
    .open()?;
```

Exposes fjall's `KeyspaceCreateOptions` directly via closures. No wrapping. Sensible defaults for those who don't customize.

Startup: `open()` scans `streams` keyspace to find `max(numeric_id) + 1`, sets `next_stream_id`.

## Crate Structure

```
crates/nexus-fjall/
  Cargo.toml
  src/
    lib.rs       — pub exports
    store.rs     — FjallStore + RawEventStore impl
    stream.rs    — FjallStream + EventStream impl
    encoding.rs  — value/key/meta encode/decode (pure functions, independently testable)
    error.rs     — FjallError
    builder.rs   — FjallStoreBuilder
```

Dependencies: `nexus-store`, `fjall`, `thiserror`

No runtime dependency. No tokio. No async-std. Pure sync returning ready futures.

## Out of Scope (Future Phases)

- Event subscriptions / projections
- Read models
- Global event ordering
- Snapshots
- rkyv codec (separate crate: nexus-rkyv)
