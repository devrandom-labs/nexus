//! Export contract — generic over [`RawEventStore`], box-agnostic.
//!
//! Export is a **generic store capability**, defined against
//! [`RawEventStore`] + [`PortableEvent`] only — it never touches a wire
//! frame or an adapter partition, so it works for the in-memory store, fjall,
//! and a future postgres store alike. It is also **box-agnostic**: it yields
//! a lazy stream of [`PortableEvent`]s, and the *box* (the CBOR default, or
//! Agency's CESR) turns those into chunk bytes. No codec, no CBOR here.
//!
//! Two traits:
//!
//! - [`StreamLister`] — enumerate the stream ids a store holds. The one new
//!   store-layer capability export needs; an all-streams export is
//!   `list_streams` ∘ `export_stream`.
//! - [`EventExporter`] — open a lazy, async, per-stream export.
//!
//! Export is **per-stream**: `export_stream(id, from)` reads via
//! `read_stream`, so each event's `stream_id` is the call argument — no
//! global-order read, no `$all`-index attribution. A snapshot export
//! terminates when the stream is exhausted; the all-day incremental-file and
//! live-tailing loops are the consumer's (they compose this stream + a box +
//! their own writer). See issue #145 §5.
//!
//! The trait *implementations* (a blanket impl over `RawEventStore`, the
//! adapter `StreamLister` impls) are a later card; this module is the
//! contract only.

use bytes::Bytes;
use futures::Stream;
use nexus::{Id, Version};

use crate::portable::PortableEvent;
use crate::store::RawEventStore;

/// Enumerate the stream ids present in a store.
///
/// The generic source of "which streams exist" — needed because a backup of
/// an arbitrary store doesn't know its ids up front, and `read_stream`
/// requires one. Yields the raw stream-id bytes (the form the store holds
/// them in); the caller reconstitutes a typed [`Id`] if it needs one.
///
/// Lazy and async, mirroring [`RawEventStore::read_all`]: a store with many
/// streams streams its ids rather than materializing them all.
///
/// Adapters back this with whatever index already tracks streams (fjall: its
/// `streams` partition; in-memory: its map; postgres: `SELECT DISTINCT`).
pub trait StreamLister: RawEventStore {
    /// The stream of stream ids.
    type StreamList: Stream<Item = Result<Bytes, Self::Error>> + Send + 'static;

    /// Open a one-shot stream over every stream id in the store, in no
    /// guaranteed order, terminating when exhausted.
    fn list_streams(
        &self,
    ) -> impl std::future::Future<Output = Result<Self::StreamList, Self::Error>> + Send;
}

/// Export a single stream's events as a lazy stream of [`PortableEvent`]s.
///
/// `export_stream(id, from)` is the **snapshot** primitive: it reads the
/// stream from `from` (exclusive, like `read_stream`) up to its current head
/// and terminates. Each yielded event carries `id` as its `stream_id`, drops
/// `global_seq`, and keeps `version`/`schema_version`/`event_type`/
/// `metadata`/`payload` verbatim.
///
/// The stream is **pull-based**: it reads as polled, in bounded memory, so a
/// consumer can write the events to a file incrementally over any timespan
/// without blocking. Resuming a partial export is just a later
/// `export_stream(id, from)` with the last version written.
///
/// `export_all` (`list_streams` ∘ `export_stream`) and continuous/live export
/// (compose with the never-ending `subscribe` cursor) are consumer-side
/// combinators, not part of this trait.
pub trait EventExporter: RawEventStore {
    /// The stream of exported events.
    type ExportStream: Stream<Item = Result<PortableEvent, Self::Error>> + Send + 'static;

    /// Open a lazy export of stream `id`, starting after `from`.
    fn export_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::ExportStream, Self::Error>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{PendingEnvelope, PersistedEnvelope};
    use crate::error::AppendError;
    use crate::store::GlobalSeq;
    use static_assertions::assert_impl_all;

    // ── Trait surface: associated-type / supertrait bounds compile ──────────
    //
    // Export's trait *implementations* are a later card (the doc header notes a
    // blanket impl over `RawEventStore` for `EventExporter`, and per-adapter
    // `StreamLister` impls). There is no behavior to test yet. This tiny no-op
    // impl exists ONLY to pin the frozen trait surface — it forces, at compile
    // time:
    //   * `EventExporter: RawEventStore` and `StreamLister: RawEventStore`
    //     supertrait bounds,
    //   * `ExportStream: Stream<Item = Result<PortableEvent, Error>> + Send +
    //     'static`,
    //   * `StreamList:  Stream<Item = Result<Bytes,        Error>> + Send +
    //     'static`,
    //   * the exact `export_stream(&self, &impl Id, Version)` / `list_streams`
    //     method signatures returning `Send` futures.
    // The export card will REPLACE this scaffold with the real impls (and their
    // behavioral tests). When the planned blanket `impl EventExporter for
    // RawEventStore` lands, it will collide with this impl — that collision is
    // the intended tripwire telling the card author to remove the scaffold.

    #[derive(Debug, thiserror::Error)]
    #[error("noop")]
    struct NoopError;

    struct NoopStore;

    impl RawEventStore for NoopStore {
        type Error = NoopError;
        type Stream = futures::stream::Empty<Result<PersistedEnvelope, NoopError>>;
        type AllStream = futures::stream::Empty<Result<PersistedEnvelope, NoopError>>;

        fn append(
            &self,
            _id: &impl Id,
            _expected_version: Option<Version>,
            _envelopes: &[PendingEnvelope],
        ) -> impl std::future::Future<Output = Result<(), AppendError<NoopError>>> + Send {
            std::future::ready(Ok(()))
        }

        fn read_stream(
            &self,
            _id: &impl Id,
            _from: Version,
        ) -> impl std::future::Future<Output = Result<Self::Stream, NoopError>> + Send {
            std::future::ready(Ok(futures::stream::empty()))
        }

        fn read_all(
            &self,
            _from: GlobalSeq,
        ) -> impl std::future::Future<Output = Result<Self::AllStream, NoopError>> + Send {
            std::future::ready(Ok(futures::stream::empty()))
        }
    }

    impl StreamLister for NoopStore {
        type StreamList = futures::stream::Empty<Result<Bytes, NoopError>>;

        fn list_streams(
            &self,
        ) -> impl std::future::Future<Output = Result<Self::StreamList, NoopError>> + Send {
            std::future::ready(Ok(futures::stream::empty()))
        }
    }

    impl EventExporter for NoopStore {
        type ExportStream = futures::stream::Empty<Result<PortableEvent, NoopError>>;

        fn export_stream(
            &self,
            _id: &impl Id,
            _from: Version,
        ) -> impl std::future::Future<Output = Result<Self::ExportStream, NoopError>> + Send
        {
            std::future::ready(Ok(futures::stream::empty()))
        }
    }

    assert_impl_all!(NoopStore: EventExporter, StreamLister, RawEventStore);
    // The exported item types are themselves `Send + 'static` stream cursors.
    assert_impl_all!(
        futures::stream::Empty<Result<PortableEvent, NoopError>>: futures::Stream, Send
    );
    assert_impl_all!(
        futures::stream::Empty<Result<Bytes, NoopError>>: futures::Stream, Send
    );
}
