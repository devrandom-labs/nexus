//! Marker trait identifying a [`futures_core::Stream`] of decoded
//! [`PersistedEnvelope`]s.
//!
//! The public trait surface is bound to **`futures-core`** (the small,
//! near-frozen definitional crate) rather than `futures`; the two name the
//! same `Stream` trait, so this marries our `SemVer` to the stabler crate.
//! Combinators still come from [`futures::StreamExt`](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html)
//! and [`futures::TryStreamExt`](https://docs.rs/futures/latest/futures/stream/trait.TryStreamExt.html).
//!
//! This module replaced the previous GAT-lending `EventStream` trait
//! family (cursor, combinators, progress, futures-bridge). The owned-
//! `Bytes` envelope from PR1 removed the per-record lifetime cliff that
//! motivated the GAT — the new shape is a marker over
//! `futures_core::Stream<Item = Result<PersistedEnvelope, _>>`.

use crate::envelope::PersistedEnvelope;

/// A futures-stream of persisted envelopes.
///
/// Marker trait — auto-implemented for every matching
/// `futures_core::Stream<Item = Result<PersistedEnvelope, E>> + Send`.
/// The associated `Error` type is recovered from the stream's `Item`
/// so call sites can bound on `S: EventStream<Error = MyErr>`.
///
/// # Why a marker (and not just `futures_core::Stream`)
///
/// The `Error` associated type. Bounding on the bare
/// `futures_core::Stream<Item = Result<PersistedEnvelope, E>>` shape requires
/// an extra generic `E` at every consumer; the marker hides it behind a
/// projection, so call sites name `S::Error` instead of carrying an
/// extra type parameter. The marker has no methods of its own — every
/// useful operation comes from `futures::StreamExt`.
///
/// # Monotonicity
///
/// Base impls (cursors over a real backing store) MUST yield events
/// with strictly-increasing `version()`. Pure relays (combinators) MUST
/// preserve order and propagate monotonicity from the wrapped stream.
/// Violating this invariant breaks aggregate rehydration; the trait
/// does not enforce it structurally.
pub trait EventStream:
    futures_core::Stream<Item = Result<PersistedEnvelope, <Self as EventStream>::Error>> + Send
{
    /// The error type yielded by the stream's `Result` items.
    type Error: std::error::Error + Send + Sync + 'static;
}

impl<S, E> EventStream for S
where
    S: futures_core::Stream<Item = Result<PersistedEnvelope, E>> + Send,
    E: std::error::Error + Send + Sync + 'static,
{
    type Error = E;
}
