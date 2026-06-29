//! `nexus-fjall::FjallStore` conformance against the canonical
//! [`EventStream`](nexus_store::stream::EventStream) trait contract.
//!
//! Delegates every check to [`nexus_store_testing::assert_event_stream_conformance`].
//!
//! ## Lifetime note
//!
//! `read_stream` returns a `ScanCursor` holding a live `fjall::Iter` over the
//! keyspace; that iterator becomes invalid when the `FjallStore` or its on-disk
//! directory drops. The conformance suite calls `make_stream` and uses the
//! returned stream after the closure returns, so we wrap the stream alongside
//! its owning store and `TempDir` in [`OwnedFjallStream`] (generic over the
//! opaque cursor type, which `nexus-fjall` does not export). The wrapper
//! delegates `poll_next` and keeps the underlying resources alive for the
//! stream's lifetime.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]

use std::num::NonZeroU32;

use nexus::Version;
use nexus_fjall::{FjallError, FjallStore};
use nexus_store::PendingEnvelope;
use nexus_store::StreamKey;
use nexus_store::envelope::{PersistedEnvelope, pending_envelope};
use nexus_store::store::RawEventStore;
use nexus_store::value::SchemaVersion;
use nexus_store_testing::{ConformanceRow, assert_event_stream_conformance};

/// The `read_stream` cursor plus the `FjallStore` and `TempDir` it depends on.
/// The cursor holds a live `fjall::Iter` referencing data that becomes invalid
/// when the keyspace closes or the on-disk dir is cleaned up, so we keep both
/// alive for the stream's lifetime. Generic over the opaque cursor type (the
/// concrete `ScanCursor<StreamScan>` is not exported by `nexus-fjall`).
struct OwnedFjallStream<St> {
    inner: St,
    _store: FjallStore,
    _tempdir: tempfile::TempDir,
}

impl<St> futures::Stream for OwnedFjallStream<St>
where
    St: futures::Stream<Item = Result<PersistedEnvelope, FjallError>> + Unpin,
{
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        core::pin::Pin::new(&mut self.inner).poll_next(cx)
    }
}

#[tokio::test]
async fn fjall_event_stream_conforms() {
    assert_event_stream_conformance(|rows: Vec<ConformanceRow>| async move {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let store = FjallStore::builder(tempdir.path().join("db"))
            .open()
            .expect("open fjall store");
        let stream_id = StreamKey::from_slice(b"conformance");

        if !rows.is_empty() {
            let envelopes: Vec<PendingEnvelope> = rows
                .into_iter()
                .map(|r| {
                    // `PendingEnvelope::event_type` is `&'static str`; the
                    // test process exits shortly after, so the per-row leak
                    // here is intentional and bounded.
                    let event_type: &'static str = Box::leak(r.event_type.into_boxed_str());
                    let with_payload = pending_envelope(Version::new(r.version).unwrap())
                        .event_type(event_type)
                        .payload(r.payload)
                        .expect("valid payload");
                    if r.schema_version == 1 {
                        with_payload.build()
                    } else {
                        with_payload
                            .schema_version(SchemaVersion::new(
                                NonZeroU32::new(r.schema_version).unwrap(),
                            ))
                            .build()
                    }
                })
                .collect();
            store
                .append(&stream_id, None, &envelopes)
                .await
                .expect("append rows");
        }

        let inner = store
            .read_stream(&stream_id, Version::INITIAL)
            .await
            .expect("open read_stream");

        OwnedFjallStream {
            inner,
            _store: store,
            _tempdir: tempdir,
        }
    })
    .await;
}
