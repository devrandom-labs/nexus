# EventStream API Fix — Design

> **Issue:** #153

## Problem

`EventStream::next()` returns `Option<Result<PersistedEnvelope<'_>, Self::Error>>`. This forces `Some(Err(...))` at every error site and prevents `?` usage.

## Solution

Change to `Result<Option<PersistedEnvelope<'_>, Self::Error>>`.

- `Ok(Some(env))` — next event
- `Ok(None)` — stream exhausted
- `Err(e)` — error reading

## Consumer Pattern

```rust
// Before:
while let Some(result) = stream.next().await {
    let env = result.map_err(StoreError::Adapter)?;
}

// After:
while let Some(env) = stream.next().await? {
}
```

## Impact

**Trait definitions (1 file):**
- `EventStream::next()` signature
- `EventStreamExt::try_fold()` internals

**Implementations (7):**
- `FjallStream`, `FjallSubscriptionStream` (nexus-fjall)
- `InMemoryStream`, `InMemorySubscriptionStream` (nexus-store/testing)
- `VecStream` (stream_tests.rs)
- `ProbeStream` (bug_hunt_tests.rs)
- `InMemoryStream` (store_bench.rs)

**Consumers (~25 call sites):**
- Replay loops in `event_store.rs`, `zero_copy.rs`
- `EventStreamExt::try_fold()`
- Property tests, adversarial tests, fjall tests
- Benchmarks (store_bench, fjall_benchmarks)
- Examples (store-inmemory, store-and-kernel)
- Doc example in `repository.rs`

## Subscription Streams

Subscription streams never return `Ok(None)` — they block on `Notify` until new events arrive. Semantics unchanged.
