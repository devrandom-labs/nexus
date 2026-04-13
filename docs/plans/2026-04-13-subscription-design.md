# Event Subscription Design

**Issue:** #125
**Date:** 2026-04-13
**Status:** Approved

## Scope (v1)

Per-stream catch-up + live subscriptions with at-least-once delivery. Deferred items tracked in #149.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Trait placement | `nexus-store` (traits), adapters own notification | Not every adapter can support live subscriptions |
| Async dependency | `nexus-store` stays sync; async only in adapters | Keeps store crate dependency-light |
| Notification mechanism | `tokio::sync::Notify` in adapters | fjall has no built-in change feed; Notify is zero-allocation, semantically precise |
| Signal value | `()` (unit) | Subscriber might watch a different stream than was appended to; "something changed" is correct |
| Stream identity | `&impl Id` | Consistent with `RawEventStore::append()` and `read_stream()` |
| Checkpoint storage | Separate `CheckpointStore` trait | Checkpoints might live in a different backend than events |
| Delivery guarantee | At-least-once | Exactly-once via transactional checkpoint deferred to #149 |
| Per-stream vs global | Per-stream only | Global `$all` deferred to #149 |

## Trait Definitions (`nexus-store`)

### `Subscription` trait

File: `crates/nexus-store/src/store/subscription.rs`

```rust
/// A subscription to events in a single stream.
///
/// Returns an `EventStream` that never exhausts — it waits for new events
/// when caught up, rather than returning `None`.
pub trait Subscription<M: 'static> {
    type Stream<'a>: EventStream<M> where Self: 'a;
    type Error: core::error::Error + Send + Sync + 'static;

    fn subscribe<'a>(
        &'a self,
        id: &'a impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream<'a>, Self::Error>> + Send + 'a;
}
```

- `from: None` → start from beginning of stream
- `from: Some(v)` → start from after version `v`
- The returned stream never returns `None`; it waits on the adapter's notification mechanism when caught up

### `CheckpointStore` trait

File: `crates/nexus-store/src/store/checkpoint.rs`

```rust
/// Persists subscription positions for resume-after-restart.
pub trait CheckpointStore {
    type Error: core::error::Error + Send + Sync + 'static;

    fn load(
        &self,
        subscription_id: &impl Id,
    ) -> impl Future<Output = Result<Option<Version>, Self::Error>> + Send + '_;

    fn save(
        &self,
        subscription_id: &impl Id,
        version: Version,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + '_;
}
```

## fjall Implementation

### Notification

- `FjallStore` gains an internal `tokio::sync::Notify`
- `append()` calls `notify.notify_waiters()` after successful transaction commit
- `tokio` dependency added to `nexus-fjall` (feature: `sync` only — no runtime)

### `FjallSubscriptionStream`

Implements `EventStream<M>` with never-exhausting semantics:

1. **Catch-up phase:** reads all events from `from` using existing `read_stream` path
2. **Transition:** when inner stream returns `None` (exhausted), enters live phase
3. **Live phase:** awaits `notify.notified()`, then opens new `read_stream` from current position
4. **Never returns `None`** — from consumer's perspective, an infinite stream

### Checkpoint partition

- New fjall partition: `checkpoints`
- Key: subscription ID (string bytes)
- Value: version as `u64` big-endian (8 bytes)
- `FjallStore` implements `CheckpointStore`
- Added via `FjallStoreBuilder` (optional, like snapshots)

## InMemoryStore Subscription Support

- Internal `tokio::sync::Notify` signaled after each `append()`
- `InMemorySubscriptionStream` wraps in-memory storage with same catch-up → live pattern
- `CheckpointStore` implemented with `HashMap<String, Version>` behind mutex
- `nexus-store`'s `testing` feature gains `tokio` dependency (test-only)

## Testing Strategy

All tests run against both `InMemoryStore` and `FjallStore`.

### 1. Sequence/Protocol

- Subscribe, receive catch-up events, append new, verify live arrival
- Subscribe with checkpoint, verify only post-checkpoint events arrive
- Multiple sequential subscribes to same stream

### 2. Lifecycle

- Subscribe → drop → re-subscribe from checkpoint → verify continuity
- Append while no subscriber exists → subscribe later → verify catch-up
- fjall: write → close → reopen → subscribe → verify all events present

### 3. Defensive Boundary

- Subscribe to nonexistent stream (should wait, not error)
- Checkpoint with unknown subscription ID (load returns `None`)
- Subscribe with `from` version beyond current stream head

### 4. Linearizability/Isolation

- Concurrent appender + subscriber on same stream
- Multiple subscribers to same stream see identical events
- Append during catch-up phase doesn't lose events

## Architecture Diagram

```
┌─────────────────────────────────────────────────┐
│                   Consumer                       │
│  loop {                                          │
│    let event = stream.next().await;  // blocks   │
│    handle(event);                                │
│    checkpoint.save(id, event.version);           │
│  }                                               │
└────────────────────┬────────────────────────────┘
                     │ EventStream (GAT lending)
┌────────────────────▼────────────────────────────┐
│           FjallSubscriptionStream                │
│  ┌──────────┐    ┌───────────────┐              │
│  │ Catch-up │───▶│ Live (Notify) │──┐           │
│  │ read_stream   │ .notified()   │  │ loop      │
│  └──────────┘    └───────────────┘◀─┘           │
└────────────────────┬────────────────────────────┘
                     │ read_stream (existing)
┌────────────────────▼────────────────────────────┐
│              FjallStore                          │
│  append() ──▶ notify.notify_waiters()           │
│  partitions: streams | events | checkpoints     │
└─────────────────────────────────────────────────┘
```
