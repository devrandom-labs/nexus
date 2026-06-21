//! Export contract — generic over [`RawEventStore`], raw and box-agnostic.
//!
//! Export is a **generic store capability**, defined against
//! [`RawEventStore`] only — it never touches a wire frame or an adapter
//! partition, so it works for the in-memory store, fjall, and a future
//! postgres store alike.
//!
//! Export does **no data manipulation**. `export_stream(id, from)` is a pass
//! through to [`RawEventStore::read_stream`] — it yields the stored
//! [`PersistedEnvelope`](crate::envelope::PersistedEnvelope)s verbatim. The events are not rewritten, the
//! store-local `global_seq` is not stripped, and the stream id is not stamped
//! onto each event. Two facts make that unnecessary:
//!
//! - **The caller supplies the id.** `export_stream(id, …)` is per-stream, so
//!   the caller already knows which stream the events belong to — exactly like
//!   a read. The stream id never has to ride on each event.
//! - **Import re-appends.** On restore, import writes events through the
//!   normal append path, which stamps a fresh `global_seq` itself. The old
//!   store-local value simply rides along and is ignored — stripping it at
//!   export would be work import redoes for free.
//!
//! Two traits:
//!
//! - [`StreamLister`] — enumerate the stream ids a store holds. The one new
//!   store-layer capability export needs; an all-streams export is
//!   `list_streams` ∘ `export_stream`.
//! - [`EventExporter`] — open a per-stream export (a raw read).
//!
//! The stream id *is* recorded once per stream — but in the **backup box**
//! (the CBOR default, a later card), as a per-stream section heading, never on
//! the events. A restore reads that heading to route the section back to the
//! right stream; import then ignores each event's `global_seq`. See issue
//! #145 §5.

use bytes::Bytes;
use futures::Stream;
use nexus::{Id, Version};

use crate::store::RawEventStore;
use crate::stream::EventStream;

/// Enumerate the stream ids present in a store.
///
/// The generic source of "which streams exist" — needed because a backup of
/// an arbitrary store doesn't know its ids up front, and `export_stream`
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

/// Export a single stream's events — a raw pass-through read.
///
/// `export_stream(id, from)` reads stream `id` from `from` **inclusive** (the
/// same semantics as [`RawEventStore::read_stream`]: `from = Version::INITIAL`
/// yields the whole stream from v1) up to its current head, then terminates.
/// Each yielded [`PersistedEnvelope`](crate::envelope::PersistedEnvelope) is the stored event **verbatim** — no
/// rewrite, `global_seq` intact, no per-event stream id.
///
/// `from` is inclusive because the type forbids otherwise: [`Version`] is a
/// `NonZeroU64` (minimum 1), so an exclusive `from` could never include v1 and
/// a full export would be impossible. To **resume** after the last exported
/// version `V`, pass `V.next()` (the caller's responsibility, mirroring how a
/// subscription cursor resumes).
///
/// The stream is **pull-based**: it reads as polled, in bounded memory, so a
/// consumer can write events to a file incrementally over any timespan.
///
/// `export_all` (`list_streams` ∘ `export_stream`) and continuous/live export
/// (compose with the never-ending `subscribe` cursor) are consumer-side
/// combinators, not part of this trait. The blanket impl below makes **every**
/// [`RawEventStore`] an `EventExporter` for free.
pub trait EventExporter: RawEventStore {
    /// The stream of exported events. Identical to the read stream — export
    /// performs no transform.
    type ExportStream: EventStream<Error = Self::Error> + 'static;

    /// Open a per-stream export of stream `id`, starting at `from` (inclusive).
    fn export_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::ExportStream, Self::Error>> + Send;
}

/// Every [`RawEventStore`] is an [`EventExporter`] — export is just a read.
///
/// `export_stream` forwards to [`RawEventStore::read_stream`] unchanged, so the
/// associated [`ExportStream`](EventExporter::ExportStream) is the adapter's
/// own [`Stream`](RawEventStore::Stream) type: a concrete, monomorphized
/// cursor with no boxing, no dynamic dispatch, and no per-event transform.
impl<S: RawEventStore> EventExporter for S {
    type ExportStream = S::Stream;

    fn export_stream(
        &self,
        id: &impl Id,
        from: Version,
    ) -> impl std::future::Future<Output = Result<Self::ExportStream, Self::Error>> + Send {
        self.read_stream(id, from)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test code asserts exact values"
)]
mod tests {
    use super::*;
    use crate::envelope::{PersistedEnvelope, pending_envelope};
    use crate::store::GlobalSeq;
    use crate::testing::{InMemoryStore, InMemoryStoreError};
    use futures::StreamExt;
    use proptest::prelude::*;
    use static_assertions::assert_impl_all;
    use std::collections::HashSet;
    use std::sync::Arc;
    use tokio::sync::Barrier;

    // Every RawEventStore gains both capabilities — the blanket impl for
    // EventExporter, the adapter impl for StreamLister.
    assert_impl_all!(InMemoryStore: EventExporter, StreamLister, RawEventStore);

    // ── test Id ──────────────────────────────────────────────────────────────

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    struct Tid(String);
    impl std::fmt::Display for Tid {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl AsRef<[u8]> for Tid {
        fn as_ref(&self) -> &[u8] {
            self.0.as_bytes()
        }
    }
    impl nexus::Id for Tid {
        const BYTE_LEN: usize = 0;
    }

    fn tid(s: &str) -> Tid {
        Tid(s.to_owned())
    }

    fn env(v: u64, payload: &[u8]) -> crate::envelope::PendingEnvelope {
        pending_envelope(Version::new(v).expect("nonzero"))
            .event_type("E")
            .payload(payload.to_vec())
            .expect("valid payload")
            .build()
    }

    async fn append_one(store: &InMemoryStore, id: &Tid, v: u64, payload: &[u8]) {
        let expected = Version::new(v - 1);
        store
            .append(id, expected, &[env(v, payload)])
            .await
            .expect("append succeeds");
    }

    async fn seed(store: &InMemoryStore, id: &Tid, count: u64) {
        for v in 1..=count {
            append_one(store, id, v, format!("payload-{v}").as_bytes()).await;
        }
    }

    async fn collect_export(
        store: &InMemoryStore,
        id: &Tid,
        from: Version,
    ) -> Vec<PersistedEnvelope> {
        let stream = store.export_stream(id, from).await.expect("export opens");
        stream
            .map(|r| r.expect("no read error"))
            .collect::<Vec<_>>()
            .await
    }

    // ════════════════════════════════════════════════════════════════════════
    // EventExporter — Category 1: sequence / protocol
    // ════════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn export_stream_is_byte_for_byte_the_read_stream() {
        // The core contract: export is a verbatim pass-through of read_stream.
        // Same versions, global_seqs, schema, event_type, payload, metadata.
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        seed(&store, &id, 5).await;

        let exported = collect_export(&store, &id, Version::INITIAL).await;

        let read: Vec<PersistedEnvelope> = store
            .read_stream(&id, Version::INITIAL)
            .await
            .expect("read opens")
            .map(|r| r.expect("no read error"))
            .collect()
            .await;

        assert_eq!(exported.len(), read.len());
        for (e, r) in exported.iter().zip(read.iter()) {
            assert_eq!(e.version(), r.version());
            assert_eq!(
                e.global_seq(),
                r.global_seq(),
                "global_seq carried verbatim"
            );
            assert_eq!(e.schema_version(), r.schema_version());
            assert_eq!(e.event_type(), r.event_type());
            assert_eq!(e.payload(), r.payload());
            assert_eq!(e.metadata(), r.metadata());
        }
    }

    #[tokio::test]
    async fn export_preserves_versions_and_payloads_in_order() {
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        seed(&store, &id, 5).await;

        let exported = collect_export(&store, &id, Version::INITIAL).await;

        let versions: Vec<u64> = exported.iter().map(|e| e.version().as_u64()).collect();
        assert_eq!(versions, vec![1, 2, 3, 4, 5]);
        for (i, e) in exported.iter().enumerate() {
            let expected = format!("payload-{}", i + 1);
            assert_eq!(e.payload(), expected.as_bytes());
        }
    }

    #[tokio::test]
    async fn exporting_the_same_stream_twice_is_identical() {
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        seed(&store, &id, 4).await;

        let first = collect_export(&store, &id, Version::INITIAL).await;
        let second = collect_export(&store, &id, Version::INITIAL).await;

        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.version(), b.version());
            assert_eq!(a.global_seq(), b.global_seq());
            assert_eq!(a.payload(), b.payload());
        }
    }

    #[tokio::test]
    async fn export_carries_metadata_and_schema_verbatim() {
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        let pending = pending_envelope(Version::INITIAL)
            .event_type("Created")
            .payload(b"body".to_vec())
            .expect("valid payload")
            .schema_version(crate::value::SchemaVersion::from_u32(7).expect("nonzero"))
            .with_metadata(b"meta".to_vec())
            .expect("valid metadata");
        store.append(&id, None, &[pending]).await.expect("append");

        let exported = collect_export(&store, &id, Version::INITIAL).await;

        assert_eq!(exported.len(), 1);
        let e = &exported[0];
        assert_eq!(e.event_type(), "Created");
        assert_eq!(e.payload(), b"body");
        assert_eq!(e.metadata(), Some(b"meta".as_slice()));
        assert_eq!(e.schema_version(), 7);
    }

    // ════════════════════════════════════════════════════════════════════════
    // EventExporter — Category 2: lifecycle
    // ════════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn export_empty_stream_terminates_and_does_not_hang() {
        let store = InMemoryStore::new();
        let id = tid("never-appended");
        let mut stream = store
            .export_stream(&id, Version::INITIAL)
            .await
            .expect("opens");
        // Must terminate (None), not block forever.
        assert!(stream.next().await.is_none());
        // Stays terminated.
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn export_from_past_the_head_is_empty() {
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        seed(&store, &id, 3).await;

        let exported = collect_export(&store, &id, Version::new(10).expect("nonzero")).await;
        assert!(exported.is_empty(), "from past head yields nothing");
    }

    #[tokio::test]
    async fn export_from_midstream_is_inclusive_of_from() {
        // Pins the resolved semantics: `from` is INCLUSIVE (matches
        // read_stream). from=3 on a 5-event stream yields [3,4,5].
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        seed(&store, &id, 5).await;

        let exported = collect_export(&store, &id, Version::new(3).expect("nonzero")).await;
        let versions: Vec<u64> = exported.iter().map(|e| e.version().as_u64()).collect();
        assert_eq!(versions, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn export_single_event_stream_yields_one() {
        let store = InMemoryStore::new();
        let id = tid("acct-1");
        seed(&store, &id, 1).await;

        let exported = collect_export(&store, &id, Version::INITIAL).await;
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0].version(), Version::INITIAL);
    }

    // ════════════════════════════════════════════════════════════════════════
    // EventExporter — Category 3: defensive boundary
    // ════════════════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn export_unknown_stream_is_empty_not_error() {
        let store = InMemoryStore::new();
        // Seed a different stream so the store is non-empty.
        seed(&store, &tid("other"), 2).await;

        let exported = collect_export(&store, &tid("does-not-exist"), Version::INITIAL).await;
        assert!(exported.is_empty());
    }

    #[tokio::test]
    async fn export_handles_empty_and_unusual_stream_ids() {
        let store = InMemoryStore::new();
        // Empty id and a long id are both structurally legal stream ids.
        let empty = tid("");
        let long = tid(&"x".repeat(300));
        seed(&store, &empty, 2).await;
        seed(&store, &long, 3).await;

        assert_eq!(
            collect_export(&store, &empty, Version::INITIAL).await.len(),
            2
        );
        assert_eq!(
            collect_export(&store, &long, Version::INITIAL).await.len(),
            3
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // EventExporter — Category 4: linearizability / isolation
    // ════════════════════════════════════════════════════════════════════════

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_append_and_export_sees_a_consistent_prefix() {
        // A writer appends 1..=N on a stream while an exporter drains it. The
        // exporter must see a contiguous prefix starting at v1, strictly
        // increasing, each event well-formed (payload matches its version) —
        // never a gap, never a torn event.
        let store = Arc::new(InMemoryStore::new());
        let id = tid("race");
        let n = 200u64;

        let barrier = Arc::new(Barrier::new(2));

        let writer_store = Arc::clone(&store);
        let writer_id = id.clone();
        let writer_barrier = Arc::clone(&barrier);
        let writer = tokio::spawn(async move {
            writer_barrier.wait().await;
            for v in 1..=n {
                append_one(
                    &writer_store,
                    &writer_id,
                    v,
                    format!("payload-{v}").as_bytes(),
                )
                .await;
            }
        });

        barrier.wait().await;
        let stream = store
            .export_stream(&id, Version::INITIAL)
            .await
            .expect("opens");
        let exported: Vec<PersistedEnvelope> =
            stream.map(|r| r.expect("no read error")).collect().await;

        writer.await.expect("writer task");

        // Contiguous prefix from v1, strictly increasing, payload matches.
        for (expected, e) in (1u64..).zip(exported.iter()) {
            assert_eq!(
                e.version().as_u64(),
                expected,
                "exported versions must be a gapless prefix from 1",
            );
            assert_eq!(e.payload(), format!("payload-{expected}").as_bytes());
        }
        // We may not have raced the full N in, but every event we saw is a
        // valid, ordered prefix — and the final state has all N.
        assert!(
            !exported.is_empty(),
            "exporter saw at least the early prefix"
        );
    }

    // ════════════════════════════════════════════════════════════════════════
    // StreamLister
    // ════════════════════════════════════════════════════════════════════════

    async fn collect_stream_ids(store: &InMemoryStore) -> HashSet<Vec<u8>> {
        let stream = store.list_streams().await.expect("list opens");
        stream
            .map(|r| r.expect("no list error").to_vec())
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect()
    }

    #[tokio::test]
    async fn list_streams_returns_exactly_the_appended_ids() {
        let store = InMemoryStore::new();
        seed(&store, &tid("alpha"), 1).await;
        seed(&store, &tid("beta"), 2).await;
        seed(&store, &tid("gamma"), 3).await;

        let ids = collect_stream_ids(&store).await;
        let expected: HashSet<Vec<u8>> = [b"alpha".to_vec(), b"beta".to_vec(), b"gamma".to_vec()]
            .into_iter()
            .collect();
        assert_eq!(ids, expected);
    }

    #[tokio::test]
    async fn list_streams_on_empty_store_is_empty() {
        let store = InMemoryStore::new();
        let ids = collect_stream_ids(&store).await;
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn list_streams_lists_each_stream_once_regardless_of_event_count() {
        let store = InMemoryStore::new();
        seed(&store, &tid("busy"), 50).await;

        let stream = store.list_streams().await.expect("opens");
        let all: Vec<Vec<u8>> = stream
            .map(|r| r.expect("no error").to_vec())
            .collect::<Vec<_>>()
            .await;
        assert_eq!(all, vec![b"busy".to_vec()], "one entry, no per-event dupes");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        #[test]
        fn list_streams_matches_a_hashset_oracle(
            ids in proptest::collection::hash_set("[a-z]{1,12}", 0..20),
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let store = InMemoryStore::new();
                for s in &ids {
                    append_one(&store, &tid(s), 1, b"p").await;
                }
                let listed = collect_stream_ids(&store).await;
                let oracle: HashSet<Vec<u8>> =
                    ids.iter().map(|s| s.as_bytes().to_vec()).collect();
                prop_assert_eq!(listed, oracle);
                Ok(())
            })?;
        }
    }

    // Surfaces a typed error from a successful no-event read path — pins that
    // the StreamList item error type is the adapter error (not boxed).
    #[tokio::test]
    async fn stream_list_error_type_is_the_adapter_error() {
        fn assert_err_type<S: StreamLister<Error = InMemoryStoreError>>(_: &S) {}
        let store = InMemoryStore::new();
        assert_err_type(&store);
        // The GlobalSeq import keeps the path exercised in tests that need it.
        let _ = GlobalSeq::INITIAL;
    }
}
