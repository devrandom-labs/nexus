# Remove StreamId

## Problem

`StreamId` is a kernel type that duplicates information the aggregate already carries (name + id). It forces users to manually construct it, pass it alongside the aggregate's own id (redundant), and lives in the wrong crate. It wraps a `String` for no reason.

## Design

### Delete from kernel

- Delete `nexus/src/stream_id.rs` entirely
- Remove `StreamId`, `InvalidStreamId`, `MAX_STREAM_ID_LEN` from `nexus/src/lib.rs` re-exports
- Remove `pub use nexus::StreamId` from `nexus-store/src/lib.rs`

### `Repository` — takes only what it needs

```rust
pub trait Repository<A: Aggregate>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn load(&self, id: A::Id) -> impl Future<Output = Result<AggregateRoot<A>, Self::Error>> + Send;
    fn save(&self, aggregate: &mut AggregateRoot<A>) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

No `stream_id` parameter. `load` takes the aggregate ID. `save` gets it from the aggregate itself via `aggregate.id()`.

### `RawEventStore` — takes `&impl Id`

```rust
pub trait RawEventStore<M = ()>: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;
    type Stream<'a>: EventStream<M, Error = Self::Error> + 'a where Self: 'a;

    fn append(
        &self,
        id: &impl Id,
        expected_version: Option<Version>,
        envelopes: &[PendingEnvelope<M>],
    ) -> impl Future<Output = Result<(), AppendError<Self::Error>>> + Send;

    fn read_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> impl Future<Output = Result<Self::Stream<'_>, Self::Error>> + Send;
}
```

The adapter (fjall) uses `id.to_string()` or whatever serialization it wants to derive storage keys. Namespacing by aggregate type is not the framework's problem — proper IDs (UUIDs, ULIDs) are globally unique.

### `expected_version: Option<Version>`

With `Version` now wrapping `NonZeroU64`, a new stream has no version — `None`. An existing stream has `Some(version)`. This replaces the old `Version::INITIAL` (0) as the "no events yet" sentinel.

### `PendingEnvelope` — drop `stream_id`

The stream identity is already in the `append` call parameter. Duplicating it in each envelope is redundant. Remove the `stream_id` field and the typestate builder step that sets it.

### `PersistedEnvelope` — drop `stream_id`

Same reasoning. The caller already knows which stream they're reading from.

### `EventStore` / `ZeroCopyEventStore` facade

The `save_with_encoder` function derives the ID from the aggregate: `aggregate.id()`. The `load` function receives the ID directly. Both pass the ID through to `RawEventStore`.

### Fjall adapter

`FjallStore` uses `id.to_string()` as the key in its `streams` partition. Everything else stays the same — numeric ID allocation, event key encoding, etc.

### Error types

`StoreError::Conflict` and `AppendError::Conflict` keep `stream_id: ErrorId` for diagnostics — populated from `ErrorId::from_display(id)`.

### Test updates

All tests that construct `StreamId` switch to using the aggregate ID directly (or a simple `Display`-implementing type for raw store tests).
