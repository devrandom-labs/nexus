# nexus-framework Crate Design

**Goal:** Extract the projection runner from `nexus-store` into a new `nexus-framework` crate that depends on tokio. The store crate stays pure (traits, codecs, no runtime). The framework crate is the batteries-included layer.

## Crate Graph

```
nexus-framework --> nexus-store --> nexus (kernel)
                                    nexus-macros <-- nexus
nexus-fjall -----> nexus-store
```

## What Lives Where

### `nexus-framework` (new crate, tokio by default)

- `ProjectionRunner` + typestate builder
- `DecodedStream` adapter (converts lending `EventStream` to owned `tokio_stream::Stream`)
- `ProjectionError`, `StatePersistError`
- `StatePersistence` trait + `NoStatePersistence` / `WithStatePersistence`

Dependencies: `nexus`, `nexus-store`, `tokio` (non-optional), `tokio-stream`

### `nexus-store` (unchanged, no runtime)

Keeps all traits and data types:
- `Projector`, `ProjectionTrigger`, `EveryNEvents`, `AfterEventTypes`
- `StateStore`, `PendingState`, `PersistedState`
- `Subscription`, `CheckpointStore`, `EventStream`
- All codecs, envelopes, upcasting

### What moves out of `nexus-store`

The entire `projection/runner/` module:
- `runner/runner.rs` -> `nexus-framework`
- `runner/builder.rs` -> `nexus-framework`
- `runner/error.rs` -> `nexus-framework`
- `runner/persist.rs` -> `nexus-framework`
- `runner/mod.rs` -> deleted

The `projection/mod.rs` re-export of `pub mod runner` is removed.
The `lib.rs` re-exports of `ProjectionError`, `ProjectionRunner` are removed.

## DecodedStream Adapter

Wraps the lending `EventStream` + `Codec`. Implements `tokio_stream::Stream`.
Decodes inside `poll_next()` while the lending borrow is alive, yields owned `(Version, P::Event)`.

```rust
pub(crate) struct DecodedStream<S, EC> {
    stream: S,
    codec: EC,
}

impl<S, EC, E> Stream for DecodedStream<S, EC>
where
    S: EventStream<()>,
    EC: Codec<E>,
    E: DomainEvent,
{
    type Item = Result<(Version, E), ...>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // poll stream.next(), decode while borrow alive, return owned
    }
}
```

## Runner Event Loop

Replaces `poll_fn` with `tokio::select!`:

```rust
use tokio_stream::StreamExt;

loop {
    tokio::select! {
        _ = &mut shutdown => {
            flush_if_dirty(...);
            return Ok(());
        }
        item = decoded_stream.next() => {
            let Some(result) = item else { return Ok(()); };
            let (version, event) = result?;
            // apply, trigger, persist
        }
    }
}
```

## Dependency Upgrades

All workspace deps upgraded to latest via `cargo upgrade` before creating the new crate.
