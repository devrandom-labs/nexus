# nexus-store Comprehensive Testing Design

**Date:** 2026-04-02
**Branch:** feat/nexus-store
**Goal:** Replicate the kernel's 11-category testing strategy for the nexus-store crate.

## Current State

18 tests across 7 files: envelope_tests (6), codec_tests (3), stream_tests (3),
raw_store_tests (2), upcaster_tests (3), security_tests (12), static_assertions (1).

Static assertions and security tests are already complete.

## Testing Categories

### 1. Unit Tests — fill coverage gaps

Existing tests miss:
- `StoreError` Display output and `source()` chain for each variant
- `PendingEnvelope` Debug output
- `PersistedEnvelope` Debug output
- Builder intermediate types are `Send + Sync` (for async pipelines)
- `build_without_metadata` vs `build(())` equivalence

### 2. Integration Tests — multi-step store operations

- Append → append → read_stream (multi-batch append)
- `read_stream` with `from` version filtering (skip early events)
- Concurrent append to same stream (optimistic concurrency under contention)
- Large batch append (1000 events) and sequential read-back

### 3. Property-Based Testing (proptest)

Properties to verify:
- **Append-read roundtrip:** any valid envelope sequence appended is read back identically
- **Version ordering:** read_stream always returns events in version order
- **Stream isolation:** events appended to stream A never appear in stream B reads
- **Idempotent empty read:** reading a stream twice yields same results
- **Codec roundtrip:** for any event, `decode(encode(event)) == event`
- **Upcaster composition:** chaining two upcasters is equivalent to applying them sequentially

### 4. Compile-Fail Tests (trybuild)

Compile-time guarantees to verify rejection:
- **Typestate builder skip:** calling `.event_type()` on `WithStreamId` (skipping `.version()`)
- **Typestate builder incomplete:** calling `.build()` on `WithEventType` (missing `.payload()`)
- **PersistedEnvelope lifetime escape:** returning a `PersistedEnvelope` that outlives its borrow
- **EventStream lending violation:** holding two `PersistedEnvelope`s from the same stream simultaneously

### 5. Static Assertions — already complete ✅

### 6. Mutation Testing (cargo-mutants)

Run `cargo-mutants` on all 6 source files. Target: 100% viable mutant kill rate.
Files: envelope.rs, codec.rs, error.rs, raw.rs, stream.rs, upcaster.rs.

### 7. Security/Adversarial Testing — already complete ✅

### 8. Adversarial Input Testing (adapted from macro adversarial)

Edge-case inputs for builder and trait implementations:
- Unicode stream IDs (emoji, RTL, zero-width joiners)
- Maximum-length payloads (1MB+)
- Zero-byte/empty payloads
- `Version::from_persisted(u64::MAX)` in envelopes
- Exotic metadata types (nested generics, ZSTs, large structs)
- Stream IDs with path traversal attempts (`../`, null bytes)
- Event types at static str boundaries

### 9. Codec Roundtrip Tests (adapted from macro expand-compile)

The store equivalent of verifying macro expansion: verify codec encode→decode identity.
- Implement a real serde_json codec for test domain events
- Verify roundtrip for each variant shape (unit, tuple, struct)
- Verify error handling for corrupted payloads
- Verify error handling for wrong event_type during decode

### 10. Debug Assertion Contracts

Add `debug_assert!` to store code:
- `PendingEnvelope`: builder intermediate types could assert non-empty stream_id
  (but this is deferred to facade — document why)
- `EventStream::next()`: implementations should assert cursor advances monotonically
- `EventUpcaster::upcast()`: assert output version > input version in debug builds

Since nexus-store is primarily trait definitions, most contracts will be
in the test adapter implementations and documented as "implementor contracts."

### 11. Benchmarks (criterion)

- PendingEnvelope builder throughput (envelopes/sec)
- PersistedEnvelope construction (zero-alloc verification via timing)
- In-memory RawEventStore append (N events)
- In-memory RawEventStore read_stream (N events)
- EventUpcaster chain throughput (N upcasters × N events)
