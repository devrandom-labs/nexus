//! Reusable trait conformance suites for [`nexus-store`] adapters.
//!
//! ## What this crate is for
//!
//! Every adapter that implements [`nexus_store::stream::EventStream`] —
//! `nexus-fjall`'s `FjallEventStream`, `nexus-store`'s `InMemoryStream`, any
//! future `SQLite` / Postgres / S3 adapter — must independently honor the
//! trait contract:
//!
//! - **Fused after `None`**: once `next()` returns `Ok(None)`, all subsequent
//!   calls must also return `Ok(None)`.
//! - **Monotonically increasing versions**: consecutive `Some(Ok(env))`
//!   results must have strictly increasing `env.version()`.
//! - **Faithful round-trip**: `event_type`, `schema_version`, and `payload`
//!   bytes that went in must come out byte-for-byte.
//!
//! Rather than re-asserting these in every adapter's test file (and missing
//! some when a new adapter ships), this crate publishes one canonical
//! conformance suite. Each adapter calls it from a single test:
//!
//! ```ignore
//! #[tokio::test]
//! async fn fjall_event_stream_conforms() {
//!     nexus_store_testing::assert_event_stream_conformance(|rows| async move {
//!         let tmp = tempfile::tempdir().unwrap();
//!         let store = FjallStore::builder(tmp.path()).open().await.unwrap();
//!         // ... write rows ...
//!         store.read_stream(&stream_id, Version::INITIAL).await.unwrap()
//!     }).await;
//! }
//! ```
//!
//! When we discover a new invariant tomorrow, adding it here re-validates
//! every adapter in one PR.
//!
//! ## What it doesn't check
//!
//! Adapter-specific concerns (large-payload tuning, on-disk format, recovery
//! after crash) are the adapter's own test suite's job. This crate covers
//! the **trait-level contract only** — what every `EventStream` implementor
//! must do regardless of where the events are stored.

#![allow(
    clippy::unwrap_used,
    reason = "test harness — assertions naturally use unwrap"
)]
#![allow(
    clippy::expect_used,
    reason = "test harness — assertions naturally use expect"
)]
#![allow(clippy::panic, reason = "test harness — failures signal via panic")]
#![allow(
    clippy::missing_panics_doc,
    reason = "test harness — every check panics on failure"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "test harness — the conformance suite has no externally-visible errors"
)]
#![allow(
    clippy::missing_const_for_fn,
    reason = "test harness — keeping signatures uniform"
)]
#![allow(
    clippy::future_not_send,
    reason = "test harness — Send is enforced on the input by trait bounds"
)]

use std::future::Future;

use futures::StreamExt;
use nexus::Version;
use nexus_store::EventStream;
use nexus_store::envelope::PersistedEnvelope;

/// One row of test data fed into an adapter for the conformance suite to
/// observe back out.
///
/// All fields must round-trip byte-for-byte through the adapter — the
/// suite asserts each independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceRow {
    pub version: u64,
    pub event_type: String,
    pub schema_version: u32,
    pub payload: Vec<u8>,
}

impl ConformanceRow {
    /// Convenience constructor with `schema_version = 1`.
    #[must_use]
    pub fn new(version: u64, event_type: &str, payload: Vec<u8>) -> Self {
        Self {
            version,
            event_type: event_type.to_owned(),
            schema_version: 1,
            payload,
        }
    }

    /// Set the schema version (defaults to 1).
    #[must_use]
    pub fn with_schema_version(mut self, schema_version: u32) -> Self {
        self.schema_version = schema_version;
        self
    }
}

/// Drain `stream` into an owned `Vec` so the borrow on each envelope ends
/// before the next iteration starts.
async fn drain<S>(stream: &mut S) -> Result<Vec<ConformanceRow>, S::Error>
where
    S: EventStream + Unpin + ?Sized,
{
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let env = item?;
        out.push(ConformanceRow {
            version: env.version().as_u64(),
            event_type: env.event_type().to_owned(),
            schema_version: env.schema_version(),
            payload: env.payload().to_vec(),
        });
    }
    Ok(out)
}

/// Assert that subsequent `next()` calls on `stream` all yield `Ok(None)`,
/// proving the fused-after-`None` contract.
async fn assert_remains_none<S>(stream: &mut S, repeats: usize)
where
    S: EventStream + Unpin + ?Sized,
{
    for i in 0..repeats {
        let next = stream.next().await;
        assert!(
            next.is_none(),
            "fused-after-None violated on repeat #{} — adapter yielded {:?}",
            i,
            match next {
                Some(Ok(_)) => "Some(Ok(_))",
                Some(Err(_)) => "Some(Err(_))",
                None => "None",
            },
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Individual contract checks
// ═══════════════════════════════════════════════════════════════════════════

async fn check_empty_stream_yields_none<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let mut stream = make(vec![]).await;
    assert!(
        stream.next().await.is_none(),
        "empty stream must yield Ok(None) on first call",
    );
    assert_remains_none(&mut stream, 8).await;
}

async fn check_single_event<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let row = ConformanceRow::new(1, "Only", vec![42]);
    let mut stream = make(vec![row.clone()]).await;

    {
        let first = stream
            .next()
            .await
            .expect("first call must yield Some")
            .expect("first call must be Ok");
        assert_eq!(first.version(), Version::new(1).unwrap());
        assert_eq!(first.event_type(), "Only");
        assert_eq!(first.payload(), &[42][..]);
    }

    assert!(
        stream.next().await.is_none(),
        "after the only event the stream must yield Ok(None)",
    );
    assert_remains_none(&mut stream, 4).await;
}

async fn check_n_events_then_fused<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    for n in [2u64, 5, 16, 64] {
        let rows: Vec<_> = (1..=n)
            .map(|v| ConformanceRow::new(v, "E", vec![u8::try_from(v % 256).unwrap_or(0)]))
            .collect();
        let mut stream = make(rows.clone()).await;
        let observed = drain(&mut stream).await.unwrap_or_else(|_| {
            panic!("drain over {n} events errored — adapter contract violated")
        });
        assert_eq!(
            observed.len(),
            rows.len(),
            "drained count mismatch for n={n}",
        );
        // The stream must now be fused.
        assert_remains_none(&mut stream, 8).await;
    }
}

async fn check_versions_strictly_monotonic<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let rows: Vec<_> = (1..=32u64)
        .map(|v| ConformanceRow::new(v, "E", vec![]))
        .collect();
    let mut stream = make(rows).await;
    let observed = drain(&mut stream).await.expect("drain ok");
    for w in observed.windows(2) {
        assert!(
            w[1].version > w[0].version,
            "versions not strictly increasing: {} followed by {}",
            w[0].version,
            w[1].version,
        );
    }
}

async fn check_event_type_round_trips<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    // Mix common, edge, and Unicode event types.
    let rows = vec![
        ConformanceRow::new(1, "Created", vec![]),
        ConformanceRow::new(2, "user.signed_up", vec![]),
        ConformanceRow::new(3, "ÉvénementUTF8", vec![]),
        ConformanceRow::new(4, "with spaces and digits 123", vec![]),
    ];
    let mut stream = make(rows.clone()).await;
    let observed = drain(&mut stream).await.expect("drain ok");
    assert_eq!(observed.len(), rows.len());
    for (i, (got, want)) in observed.iter().zip(rows.iter()).enumerate() {
        assert_eq!(
            got.event_type, want.event_type,
            "event_type mismatch at index {i}",
        );
    }
}

async fn check_schema_version_round_trips<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let rows = vec![
        ConformanceRow::new(1, "E", vec![]).with_schema_version(1),
        ConformanceRow::new(2, "E", vec![]).with_schema_version(7),
        ConformanceRow::new(3, "E", vec![]).with_schema_version(42),
        ConformanceRow::new(4, "E", vec![]).with_schema_version(u32::MAX),
    ];
    let mut stream = make(rows.clone()).await;
    let observed = drain(&mut stream).await.expect("drain ok");
    assert_eq!(observed.len(), rows.len());
    for (i, (got, want)) in observed.iter().zip(rows.iter()).enumerate() {
        assert_eq!(
            got.schema_version, want.schema_version,
            "schema_version mismatch at index {i}",
        );
    }
}

async fn check_payload_round_trips_byte_for_byte<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    // Cover: empty payload, single byte, all-zero, all-0xff, mixed binary,
    // and a moderately large payload.
    let rows = vec![
        ConformanceRow::new(1, "E", vec![]),
        ConformanceRow::new(2, "E", vec![0]),
        ConformanceRow::new(3, "E", vec![0; 64]),
        ConformanceRow::new(4, "E", vec![0xff; 64]),
        ConformanceRow::new(5, "E", (0..=255u8).collect()),
        ConformanceRow::new(
            6,
            "E",
            (0..4096u32)
                .map(|i| u8::try_from(i % 256).unwrap_or(0))
                .collect(),
        ),
    ];
    let mut stream = make(rows.clone()).await;
    let observed = drain(&mut stream).await.expect("drain ok");
    assert_eq!(observed.len(), rows.len());
    for (i, (got, want)) in observed.iter().zip(rows.iter()).enumerate() {
        assert_eq!(
            got.payload,
            want.payload,
            "payload mismatch at index {} (lengths got={} want={})",
            i,
            got.payload.len(),
            want.payload.len(),
        );
    }
}

async fn check_insertion_order_preserved<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let rows: Vec<_> = (1..=20u64)
        .map(|v| {
            let payload = vec![u8::try_from(v).unwrap_or(0)];
            ConformanceRow::new(v, "E", payload)
        })
        .collect();
    let mut stream = make(rows.clone()).await;
    let observed = drain(&mut stream).await.expect("drain ok");
    assert_eq!(observed.len(), rows.len());
    for (i, (got, want)) in observed.iter().zip(rows.iter()).enumerate() {
        assert_eq!(got, want, "row mismatch at position {i}");
    }
}

async fn check_large_sequence_completes<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let n: u64 = 1024;
    let rows: Vec<_> = (1..=n)
        .map(|v| ConformanceRow::new(v, "E", vec![]))
        .collect();
    let mut stream = make(rows).await;
    let observed = drain(&mut stream)
        .await
        .expect("large-sequence drain must succeed");
    assert_eq!(observed.len(), usize::try_from(n).unwrap_or(usize::MAX));
    let last_version = observed.last().expect("non-empty").version;
    assert_eq!(last_version, n);
    assert_remains_none(&mut stream, 4).await;
}

async fn check_envelope_accessors_consistent<S, F, Fut>(make: &F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    // Verify the envelope's accessors are mutually consistent on a single
    // call (a re-call to env.version() returns the same value, etc.).
    let row = ConformanceRow::new(1, "E", vec![1, 2, 3]).with_schema_version(7);
    let mut stream = make(vec![row]).await;
    let env: PersistedEnvelope = stream
        .next()
        .await
        .expect("first call yields Some")
        .expect("first call must be Ok");
    let v1 = env.version();
    let v2 = env.version();
    assert_eq!(v1, v2, "version() not idempotent");
    let p1 = env.payload();
    let p2 = env.payload();
    assert_eq!(p1, p2, "payload() not idempotent");
    let s1 = env.schema_version();
    let s2 = env.schema_version();
    assert_eq!(s1, s2, "schema_version() not idempotent");
    assert_eq!(env.schema_version_as_version().as_u64(), u64::from(s1));
}

// ═══════════════════════════════════════════════════════════════════════════
// Public entry point
// ═══════════════════════════════════════════════════════════════════════════

/// Run every contract check against the stream produced by `make`.
///
/// Each check panics with a descriptive message on the first failure. The
/// `make` closure is called multiple times — adapters that need per-call
/// state (a temp dir, a fresh partition) must produce a *fresh* stream
/// each call.
///
/// Checks performed (each isolated, panics on failure):
///
/// 1. Empty stream yields `Ok(None)` immediately and remains fused.
/// 2. Single event round-trips and the stream is then fused.
/// 3. N events (for N ∈ {2, 5, 16, 64}) drain to N rows, then fused.
/// 4. Versions are strictly monotonically increasing.
/// 5. Event types round-trip exactly (including Unicode and spaces).
/// 6. Schema versions round-trip exactly (1, 7, 42, `u32::MAX`).
/// 7. Payloads round-trip byte-for-byte (empty, single, all-zero, all-0xff,
///    0..=255 sweep, 4 KB pattern).
/// 8. Insertion order is preserved.
/// 9. A large sequence (1024 events) completes and remains fused.
/// 10. Envelope accessors are idempotent within a single iteration.
pub async fn assert_event_stream_conformance<S, F, Fut>(make: F)
where
    S: EventStream + Unpin + Send,
    F: Fn(Vec<ConformanceRow>) -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    check_empty_stream_yields_none(&make).await;
    check_single_event(&make).await;
    check_n_events_then_fused(&make).await;
    check_versions_strictly_monotonic(&make).await;
    check_event_type_round_trips(&make).await;
    check_schema_version_round_trips(&make).await;
    check_payload_round_trips_byte_for_byte(&make).await;
    check_insertion_order_preserved(&make).await;
    check_large_sequence_completes(&make).await;
    check_envelope_accessors_consistent(&make).await;
}
