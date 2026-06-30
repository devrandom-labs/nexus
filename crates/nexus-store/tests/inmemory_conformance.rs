//! `nexus-store::testing::InMemoryStore` conformance against the canonical
//! [`EventStream`](nexus_store::stream::EventStream) trait contract.
//!
//! Delegates every check to [`nexus_store_testing::assert_event_stream_conformance`].
//! The `make_stream` closure builds a fresh `InMemoryStore`, appends the
//! requested rows, and hands back a `read_stream` cursor over them.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]

use std::num::NonZeroU32;

use nexus_store::envelope::pending_envelope;
use nexus_store::store::RawEventStore;
use nexus_store::testing::InMemoryStore;
use nexus_store::value::SchemaVersion;
use nexus_store::{PendingEnvelope, StreamKey, Version};
use nexus_store_testing::{
    ConformanceRow, assert_all_stream_conformance, assert_event_stream_conformance,
};

#[tokio::test]
async fn inmemory_event_stream_conforms() {
    assert_event_stream_conformance(|rows: Vec<ConformanceRow>| async move {
        let store = InMemoryStore::new();
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

        store
            .read_stream(&stream_id, Version::INITIAL)
            .await
            .expect("open read_stream")
    })
    .await;
}

/// `InMemoryStore` conformance against the canonical `$all` read-path contract
/// (issue #266) — the same suite `FjallStore` runs, so the two adapters cannot
/// silently diverge on `read_all` ordering, exclusivity, or resume.
#[tokio::test]
async fn inmemory_all_stream_conforms() {
    assert_all_stream_conformance(|| async { InMemoryStore::new() }).await;
}
