//! Lending event-stream cursors, accumulator types for checkpointed scans,
//! and codec/upcaster wrappers.
//!
//! Module layout:
//! - [`cursor`] — the [`EventStream`] GAT trait, the [`EventStreamExt`]
//!   combinator surface (folds, scans, decoder entry), the [`Disposition`]
//!   return tag for interruptible variants, and the internal
//!   shutdown-bias select helper.
//! - [`progress`] — the [`Progress`] accumulator and [`Step`] per-iteration
//!   outcome used by [`EventStreamExt::try_scan_until`].
//! - [`decoder`] — the [`DecoderBuilder`] typestate plus its two terminal
//!   wrappers, [`DecodedStream`] (owning codec) and
//!   [`BorrowedDecodedStream`] (zero-copy codec).

mod cursor;
mod decoder;
mod progress;

pub use cursor::{Disposition, EventStream, EventStreamExt};
pub use decoder::{
    BorrowedDecodedStream, DecodedStream, DecoderBuilder, NeedsCodec, NoUpcaster,
    WithBorrowingCodec, WithCodec, WithUpcaster,
};
pub use progress::{Progress, Step};
