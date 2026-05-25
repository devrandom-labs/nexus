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

mod combinators;
mod cursor;
mod decoder;
mod progress;

pub use combinators::{Map, TryMap, TryScan};
pub use cursor::{Disposition, EventStream, EventStreamExt};
pub use decoder::{
    BaseEventStream, BorrowedDecodedStream, DecodedStream, DecoderBuilder, NeedsCodec, NoUpcaster,
    WithBorrowingCodec, WithCodec, WithUpcaster,
};
pub use progress::{Progress, Step};
