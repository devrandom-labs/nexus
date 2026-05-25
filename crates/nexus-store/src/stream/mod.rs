//! Lending event-stream cursors, accumulator types for checkpointed scans,
//! and codec/upcaster wrappers.
//!
//! Module layout:
//! - [`cursor`] — the [`EventStream`] GAT trait, the [`EventStreamExt`]
//!   combinator surface (folds, scans, decoder entry), the [`Disposition`]
//!   return tag for interruptible variants, and the internal
//!   shutdown-bias select helper.
//! - [`combinators`] — lending stream combinators ([`Map`], [`TryMap`],
//!   [`Filter`], [`TryScan`]) that wrap an `EventStream` and project its
//!   `Item<'a>` to a different shape.
//! - [`progress`] — the [`Progress`] accumulator and [`Step`] per-iteration
//!   outcome used by [`EventStreamExt::try_scan_until`].
//! - [`decoder`] — the [`DecoderBuilder`] typestate plus its two terminal
//!   wrappers, [`DecodedStream`] (owning codec) and
//!   [`BorrowedDecodedStream`] (zero-copy codec); also the
//!   [`BaseEventStream`] sub-trait the decoder bounds on.
//! - [`owned`] (feature-gated: `futures-bridge`) — the [`OwnedEventStream`]
//!   sub-trait + [`IntoStream`] bridge that exposes an owning-output
//!   lending stream as a [`futures_core::Stream`].

mod combinators;
mod cursor;
mod decoder;
#[cfg(feature = "futures-bridge")]
mod owned;
mod progress;

pub use combinators::{Map, TryMap, TryScan};
pub use cursor::{Disposition, EventStream, EventStreamExt};
pub use decoder::{
    BaseEventStream, BorrowedDecodedStream, DecodedStream, DecoderBuilder, NeedsCodec, NoUpcaster,
    WithBorrowingCodec, WithCodec, WithUpcaster,
};
#[cfg(feature = "futures-bridge")]
pub use owned::{IntoStream, OwnedEventStream};
pub use progress::{Progress, Step};
