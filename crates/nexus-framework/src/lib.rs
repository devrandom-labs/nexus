//! Batteries-included runtime layer for nexus.
//!
//! `nexus-store` is runtime-agnostic by design — it depends on
//! [`futures`] for the [`Stream`](futures::Stream) trait but never on a
//! specific executor. `nexus-framework` is where IO-driven components
//! live: the pieces that need `tokio::spawn`, `tokio::select!`, channels,
//! and timers. Confining tokio to this crate keeps the kernel and store
//! reusable from any executor (or from `no_std` embedded targets, with
//! features turned off).
//!
//! # Modules
//!
//! - [`projection`] — subscription-powered CQRS projection runner. A
//!   two-phase typestate (`Configured` → `initialize()` → `Ready<S>`)
//!   that loads the persisted snapshot, subscribes to a
//!   [`nexus_store::Subscription`], and folds the stream into a typed
//!   state via a user-supplied [`nexus_store::projection::Projector`].
//!   The event loop is a `tokio::select!` between
//!   `futures::Stream::next` (the subscription) and a `shutdown` future
//!   — straightforward async shell over the pure-sync FSM in
//!   `ProjectionStatus`.
//!
//! # Why no bespoke stream combinator
//!
//! An earlier version of this runner used a hand-rolled
//! `try_fold_async_until` combinator on a GAT lending event-stream
//! trait. The 2026-05-27 bytes-envelope refactor collapsed event
//! streams to plain `futures::Stream<Item = Result<PersistedEnvelope, _>>`,
//! which means the combinator surface is just
//! [`futures::StreamExt`](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html)
//! and [`TryStreamExt`](https://docs.rs/futures/latest/futures/stream/trait.TryStreamExt.html).
//! The runner now uses `tokio::pin!` + `tokio::select!` directly — no
//! custom combinator to maintain.

pub mod projection;
