//! Lending event-stream cursors, accumulator types for checkpointed scans,
//! and the futures-stream bridge.
//!
//! Module layout:
//! - [`cursor`] — the [`EventStream`] GAT trait, the [`BaseEventStream`]
//!   structural sub-trait for raw envelope cursors, the [`EventStreamExt`]
//!   combinator surface (folds, scans, maps), the [`Disposition`] return
//!   tag for interruptible variants, and the internal shutdown-bias
//!   select helper.
//! - [`combinators`] — lending stream combinators ([`Map`], [`TryMap`],
//!   [`TryScan`]) that wrap an `EventStream` and project its `Item<'a>`
//!   to a different shape.
//! - [`progress`] — the [`Progress`] accumulator and [`Step`] per-iteration
//!   outcome used by [`EventStreamExt::try_scan_until`].
//! - [`owned`] (feature-gated: `futures-bridge`) — the [`OwnedEventStream`]
//!   sub-trait + [`IntoStream`] bridge that exposes an owning-output
//!   lending stream as a [`futures_core::Stream`].

mod combinators;
mod cursor;
#[cfg(feature = "futures-bridge")]
mod owned;
mod progress;

pub use combinators::{Map, TryMap, TryScan};
pub use cursor::{BaseEventStream, Disposition, EventStream, EventStreamExt};
#[cfg(feature = "futures-bridge")]
pub use owned::{IntoStream, OwnedEventStream};
pub use progress::{Progress, Step};
