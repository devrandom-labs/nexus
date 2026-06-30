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
use futures::pin_mut;
use nexus::Version;
use nexus_store::EventStream;
use nexus_store::StreamKey;
use nexus_store::envelope::{PersistedEnvelope, pending_envelope};
use nexus_store::store::RawEventStore;

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

// ═══════════════════════════════════════════════════════════════════════════
// `$all` read-path conformance (issue #266)
//
// The `$all` resume contract is adapter-defined but identical in shape across
// every adapter, so — like the per-stream `EventStream` suite above — it is
// pinned ONCE here and instantiated per adapter, so `InMemoryStore` and
// `FjallStore` can never silently diverge. The position type is opaque
// (`S::AllPosition`), but the trait bounds it `Copy + Ord`, which is all the
// suite needs: it checkpoints a real tag and feeds it back, letting `Ord` drive
// "strictly after" without knowing the concrete representation.
//
// Covered:
//  - read_all(None) yields every event across streams in position order,
//    positions strictly increasing (no dup).
//  - read_all(Some(p)) is EXCLUSIVE (strictly after p).
//  - multi-resume-cycle: consume a prefix, checkpoint the tag, resume — the
//    reconstructed stream equals the single-shot read (no gap/dup/skip across
//    the seams).
//  - read_all(None) then read_all(Some(last)): strictly-after holds at the
//    boundary (empty), and a later append surfaces only the new event.
//  - read_stream (INCLUSIVE Version) and read_all (EXCLUSIVE position) coexist
//    on one store — the intentional asymmetry (CLAUDE rule 4).
// ═══════════════════════════════════════════════════════════════════════════

/// Append one single-event batch to `id` at `version`, with the matching
/// optimistic `expected_version` (`None` for `version == 1`). Panics on any
/// store error — the suite drives a clean, conflict-free append sequence.
async fn all_append<S: RawEventStore>(store: &S, id: &str, version: u64, payload: &[u8]) {
    let expected = Version::new(version - 1);
    let env = pending_envelope(Version::new(version).expect("version must be > 0"))
        .event_type("E")
        .payload(payload.to_vec())
        .expect("valid payload")
        .build();
    store
        .append(&StreamKey::from_slice(id.as_bytes()), expected, &[env])
        .await
        .unwrap_or_else(|e| panic!("append {id}@{version} failed: {e:?}"));
}

/// Drain a full `read_all(from)` into `(position, payload)` pairs.
async fn drain_all<S: RawEventStore>(
    store: &S,
    from: Option<S::AllPosition>,
) -> Vec<(S::AllPosition, Vec<u8>)> {
    let stream = store.read_all(from).await.expect("open read_all");
    pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        let (pos, env) = item.unwrap_or_else(|e| panic!("read_all item errored: {e:?}"));
        out.push((pos, env.payload().to_vec()));
    }
    out
}

/// Assert positions are strictly increasing (monotonic, no duplicate).
fn assert_strictly_increasing<P: Copy + Ord + core::fmt::Debug>(positions: &[(P, Vec<u8>)]) {
    for w in positions.windows(2) {
        assert!(
            w[1].0 > w[0].0,
            "$all positions must be strictly increasing: {:?} then {:?}",
            w[0].0,
            w[1].0,
        );
    }
}

async fn check_all_empty_store_yields_none<S, F, Fut>(make: &F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let store = make().await;
    let got = drain_all(&store, None).await;
    assert!(
        got.is_empty(),
        "empty store: read_all(None) must yield nothing, got {} items",
        got.len(),
    );
}

async fn check_all_global_order_across_streams<S, F, Fut>(make: &F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let store = make().await;
    // Interleave appends across two streams so $all order differs from any one
    // stream's version order: a@1, a@2, b@1, a@3, a@4.
    all_append(&store, "a", 1, b"a1").await;
    all_append(&store, "a", 2, b"a2").await;
    all_append(&store, "b", 1, b"b1").await;
    all_append(&store, "a", 3, b"a3").await;
    all_append(&store, "a", 4, b"a4").await;

    let got = drain_all(&store, None).await;
    let payloads: Vec<Vec<u8>> = got.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        payloads,
        vec![
            b"a1".to_vec(),
            b"a2".to_vec(),
            b"b1".to_vec(),
            b"a3".to_vec(),
            b"a4".to_vec(),
        ],
        "read_all(None) must yield every event across streams in append (position) order",
    );
    assert_strictly_increasing(&got);
}

async fn check_all_from_is_exclusive<S, F, Fut>(make: &F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let store = make().await;
    all_append(&store, "a", 1, b"a1").await;
    all_append(&store, "a", 2, b"a2").await;
    all_append(&store, "a", 3, b"a3").await;

    let full = drain_all(&store, None).await;
    assert_eq!(full.len(), 3, "expected 3 events from a clean store");
    let checkpoint = full[0].0; // position of a@1

    let rest = drain_all(&store, Some(checkpoint)).await;
    let payloads: Vec<Vec<u8>> = rest.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        payloads,
        vec![b"a2".to_vec(), b"a3".to_vec()],
        "read_all(Some(p)) is EXCLUSIVE: p and everything at-or-below it are excluded",
    );
    assert!(
        rest[0].0 > checkpoint,
        "first resumed position {:?} must be strictly after the checkpoint {:?}",
        rest[0].0,
        checkpoint,
    );
}

async fn check_all_multi_resume_cycles<S, F, Fut>(make: &F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let store = make().await;
    // 10 events interleaved across two streams.
    let mut va = 0u64;
    let mut vb = 0u64;
    let mut expected: Vec<Vec<u8>> = Vec::new();
    for i in 0..10u64 {
        if i % 2 == 0 {
            va += 1;
            let p = format!("a{va}").into_bytes();
            all_append(&store, "a", va, &p).await;
            expected.push(p);
        } else {
            vb += 1;
            let p = format!("b{vb}").into_bytes();
            all_append(&store, "b", vb, &p).await;
            expected.push(p);
        }
    }

    // Single-shot reference read.
    let full = drain_all(&store, None).await;
    let full_payloads: Vec<Vec<u8>> = full.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        full_payloads, expected,
        "single-shot read_all(None) must match append order",
    );

    // Multi-resume cycles: consume at most 3 per cycle, checkpoint the last
    // delivered tag, reopen strictly-after, repeat until drained.
    let mut acc: Vec<(S::AllPosition, Vec<u8>)> = Vec::new();
    let mut checkpoint: Option<S::AllPosition> = None;
    loop {
        let stream = store
            .read_all(checkpoint)
            .await
            .expect("open read_all cycle");
        pin_mut!(stream);
        let mut taken = 0;
        let mut advanced = false;
        while let Some(item) = stream.next().await {
            let (pos, env) = item.unwrap_or_else(|e| panic!("cycle item errored: {e:?}"));
            acc.push((pos, env.payload().to_vec()));
            checkpoint = Some(pos);
            advanced = true;
            taken += 1;
            if taken == 3 {
                break;
            }
        }
        if !advanced {
            break;
        }
    }

    let acc_payloads: Vec<Vec<u8>> = acc.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        acc_payloads, full_payloads,
        "multi-resume cycles must reconstruct the full stream exactly — no gap, dup, or skip across the seams",
    );
    assert_strictly_increasing(&acc);
}

async fn check_all_chained_none_then_after_last<S, F, Fut>(make: &F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let store = make().await;
    all_append(&store, "a", 1, b"a1").await;
    all_append(&store, "b", 1, b"b1").await;

    let full = drain_all(&store, None).await;
    assert_eq!(full.len(), 2);
    let last = full.last().expect("non-empty").0;

    // Strictly-after the last delivered position → empty at the boundary.
    let empty = drain_all(&store, Some(last)).await;
    assert!(
        empty.is_empty(),
        "read_all(Some(last)) must be empty — nothing is strictly after the last position",
    );

    // A later append makes the SAME checkpoint surface exactly the new event.
    all_append(&store, "a", 2, b"a2").await;
    let after = drain_all(&store, Some(last)).await;
    let payloads: Vec<Vec<u8>> = after.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        payloads,
        vec![b"a2".to_vec()],
        "resuming after the old last must yield exactly the newly appended event",
    );
    assert!(
        after[0].0 > last,
        "the new event's position {:?} must be strictly after the prior last {:?}",
        after[0].0,
        last,
    );
}

async fn check_read_stream_inclusive_read_all_exclusive_coexist<S, F, Fut>(make: &F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    let store = make().await;
    all_append(&store, "a", 1, b"a1").await;
    all_append(&store, "a", 2, b"a2").await;
    all_append(&store, "a", 3, b"a3").await;

    // read_stream(from) is INCLUSIVE on Version: from=2 yields versions [2, 3].
    let rs = store
        .read_stream(&StreamKey::from_slice(b"a"), Version::new(2).expect("v2"))
        .await
        .expect("open read_stream");
    pin_mut!(rs);
    let mut versions = Vec::new();
    while let Some(item) = rs.next().await {
        let env = item.unwrap_or_else(|e| panic!("read_stream item errored: {e:?}"));
        versions.push(env.version().as_u64());
    }
    assert_eq!(
        versions,
        vec![2, 3],
        "read_stream(from) is INCLUSIVE: from=2 yields versions 2 and 3",
    );

    // read_all(from) is EXCLUSIVE: from = position of a@2 yields only a3.
    let full = drain_all(&store, None).await;
    assert_eq!(full.len(), 3);
    let pos_of_a2 = full[1].0;
    let after = drain_all(&store, Some(pos_of_a2)).await;
    let payloads: Vec<Vec<u8>> = after.iter().map(|(_, p)| p.clone()).collect();
    assert_eq!(
        payloads,
        vec![b"a3".to_vec()],
        "read_all(from) is EXCLUSIVE: from=pos(a2) yields only a3 — the asymmetry with read_stream holds on one store",
    );
}

/// Run every `$all` read-path contract check against fresh stores from `make`.
///
/// Each check calls `make` to get a clean store, so adapters that need per-call
/// state (a temp dir, a fresh keyspace) must produce a fresh store each call.
///
/// Checks performed (each isolated, panics on failure):
///
/// 1. Empty store: `read_all(None)` yields nothing.
/// 2. `read_all(None)` yields every event across streams in position order,
///    strictly increasing.
/// 3. `read_all(Some(p))` is exclusive (strictly after `p`).
/// 4. Multi-resume cycles reconstruct the full stream with no gap/dup/skip.
/// 5. `read_all(None)` then `read_all(Some(last))`: empty at the boundary, then
///    surfaces only a later append.
/// 6. `read_stream` (inclusive `Version`) and `read_all` (exclusive position)
///    coexist on one store.
pub async fn assert_all_stream_conformance<S, F, Fut>(make: F)
where
    S: RawEventStore + Send + Sync,
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = S> + Send,
{
    check_all_empty_store_yields_none(&make).await;
    check_all_global_order_across_streams(&make).await;
    check_all_from_is_exclusive(&make).await;
    check_all_multi_resume_cycles(&make).await;
    check_all_chained_none_then_after_last(&make).await;
    check_read_stream_inclusive_read_all_exclusive_coexist(&make).await;
}
