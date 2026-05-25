//! Comprehensive tests for `nexus-store::stream`.
//!
//! Scope: every public item in `crates/nexus-store/src/stream.rs`.
//!
//! Coverage matrix:
//!
//! 1. `EventStream` trait contract
//!    - basic yield / empty / single-event
//!    - envelope borrows from cursor
//!    - fused after `None`
//!    - custom metadata `M` threads through
//!    - custom (non-`Infallible`) error type propagates
//!    - error mid-iteration short-circuits
//!    - large stream (1024 events)
//! 2. `EventStreamExt` combinators
//!    - `try_fold` — primitive: empty / single / many / closure-err /
//!      stream-err / init preservation
//!    - `try_for_each` — call count, ordering, short-circuit
//!    - `try_collect_map` — empty Vec / order preserved / short-circuit
//!    - `try_count` — 0 / N / stream-error
//!    - cross-consistency: `count == collect.len() == for_each-counter`
//! 3. `DecoderBuilder` typestate
//!    - codec / borrowing_codec / +upcaster / build
//! 4. `DecodedStream::try_fold` (owning codec)
//!    - empty / single / many / order / version threading
//!    - errors split correctly into `Stream`/`Upcast`/`Decode` variants
//!    - closure-error short-circuit
//!    - upcaster transform actually fires (payload modified before decode)
//!    - no-op upcaster passthrough (payload unchanged)
//! 5. `BorrowedDecodedStream::try_fold` (zero-copy codec)
//!    - same matrix as `DecodedStream`, `&E` instead of `E`
//! 6. Defensive boundary
//!    - non-monotonic versions / duplicates / gaps relayed faithfully
//!      (combinators are pure relays; detection lives in
//!      `AggregateRoot::replay`)
//! 7. Send + concurrency
//!    - streams and decoded pipelines usable across `tokio::spawn`
//!    - two disjoint streams run in parallel tasks without interference
//! 8. Property-based
//!    - `try_count == try_collect_map.len()`
//!    - `try_fold` preserves order
//!    - decoder pipeline event count matches stream length
//!    - versions in closure equal envelope versions

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::panic, reason = "proptest macros use panic")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::needless_pass_by_value, reason = "proptest")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(clippy::shadow_reuse, reason = "tests")]
#![allow(clippy::shadow_unrelated, reason = "tests")]
#![allow(clippy::as_conversions, reason = "tests")]
#![allow(clippy::cast_possible_truncation, reason = "tests")]
#![allow(clippy::cast_possible_wrap, reason = "tests")]
#![allow(clippy::cast_sign_loss, reason = "tests")]
#![allow(clippy::implicit_clone, reason = "tests")]
#![allow(clippy::missing_docs_in_private_items, reason = "tests")]
#![allow(clippy::doc_markdown, reason = "tests")]
#![allow(clippy::uninlined_format_args, reason = "tests")]
#![allow(clippy::use_self, reason = "tests")]
#![allow(clippy::items_after_statements, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(clippy::indexing_slicing, reason = "tests")]
#![allow(clippy::similar_names, reason = "tests")]
#![allow(
    clippy::disallowed_types,
    reason = "tests use std Mutex for shared in-memory state and counters"
)]
#![allow(clippy::await_holding_lock, reason = "tests")]
#![allow(clippy::missing_const_for_fn, reason = "tests")]

use std::borrow::Cow;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use arrayvec::ArrayString;
use nexus::Version;
use nexus_store::codec::{BorrowingDecode, Decode, Encode};
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::error::{DecodeStreamError, UpcastError};
use nexus_store::store::GlobalSeq;
use nexus_store::stream::{
    BaseEventStream, BorrowedDecodedStream, DecodedStream, DecoderBuilder, Disposition,
    EventStream, EventStreamExt, Progress, Step,
};
use nexus_store::upcasting::{EventMorsel, Upcaster};
use proptest::prelude::*;
use proptest::proptest;
use static_assertions::assert_impl_all;
use thiserror::Error;

// ═══════════════════════════════════════════════════════════════════════════
// Drop tracking — fresh counter per test via `DropProbe` instances inside
// envelope metadata. Used by Section 14 to prove lending discipline at
// runtime (each envelope drops exactly once at the closure boundary).
//
// Note: the *allocation-counting* tests (proving `BorrowedDecodedStream` is
// genuinely zero-copy) live in `tests/stream_alloc_tests.rs` — they require
// a `#[global_allocator]` and must run in a separate test binary so the
// counter isn't polluted by concurrent tests in the same process.
// ═══════════════════════════════════════════════════════════════════════════

static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
static DROP_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug)]
struct DropProbe;

impl Drop for DropProbe {
    fn drop(&mut self) {
        DROP_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test fixtures — streams
// ═══════════════════════════════════════════════════════════════════════════

/// Simple in-memory stream over `(version, event_type, payload)` rows.
///
/// Always `schema_version = 1`, metadata = `()`, error = [`Infallible`].
struct VecStream {
    rows: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
}

impl VecStream {
    const fn new(rows: Vec<(u64, String, Vec<u8>)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl BaseEventStream for VecStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for VecStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).expect("test version must be non-zero"),
            GlobalSeq::INITIAL,
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

/// Stream that carries a per-row `schema_version`, useful for upcaster tests.
struct SchemaStream {
    rows: Vec<(u64, String, u32, Vec<u8>)>,
    pos: usize,
}

impl SchemaStream {
    const fn new(rows: Vec<(u64, String, u32, Vec<u8>)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl BaseEventStream for SchemaStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for SchemaStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).expect("test version must be non-zero"),
            GlobalSeq::INITIAL,
            &row.1,
            row.2,
            &row.3,
            (),
        )))
    }
}

/// Stream with `String` metadata to exercise the `M` parameter.
struct MetadataStream {
    rows: Vec<(u64, String, Vec<u8>, String)>,
    pos: usize,
}

impl MetadataStream {
    const fn new(rows: Vec<(u64, String, Vec<u8>, String)>) -> Self {
        Self { rows, pos: 0 }
    }
}

impl BaseEventStream<String> for MetadataStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a, String>) -> PersistedEnvelope<'a, String>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream<String> for MetadataStream {
    type Item<'a> = PersistedEnvelope<'a, String>;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_, String>>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).expect("test version must be non-zero"),
            GlobalSeq::INITIAL,
            &row.1,
            1,
            &row.2,
            row.3.clone(),
        )))
    }
}

/// Custom non-`Infallible` error to verify error type plumbing.
#[derive(Debug, Error)]
#[error("transient I/O failure: {0}")]
struct StreamIoError(&'static str);

/// Stream that yields events 0..`fail_at`, then errors on call `fail_at`.
///
/// `fail_at = 0` => errors before the first event.
struct FailingStream {
    rows: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
    fail_at: usize,
}

impl FailingStream {
    const fn new(rows: Vec<(u64, String, Vec<u8>)>, fail_at: usize) -> Self {
        Self {
            rows,
            pos: 0,
            fail_at,
        }
    }
}

impl BaseEventStream for FailingStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for FailingStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = StreamIoError;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos == self.fail_at {
            return Err(StreamIoError("forced failure"));
        }
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).expect("test version must be non-zero"),
            GlobalSeq::INITIAL,
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test fixtures — codecs
// ═══════════════════════════════════════════════════════════════════════════

/// Owning codec for `u64` payloads (little-endian, 8 bytes).
struct U64Codec;

#[derive(Debug, Error)]
#[error("u64 decode failed: payload had {0} bytes, expected 8")]
struct U64DecodeError(usize);

impl Encode<u64> for U64Codec {
    type Error = U64DecodeError;

    fn encode(&self, value: &u64) -> Result<Vec<u8>, Self::Error> {
        Ok(value.to_le_bytes().to_vec())
    }
}

impl Decode<u64> for U64Codec {
    type Error = U64DecodeError;

    fn decode(&self, _name: &str, payload: &[u8]) -> Result<u64, Self::Error> {
        let bytes: [u8; 8] = payload
            .try_into()
            .map_err(|_| U64DecodeError(payload.len()))?;
        Ok(u64::from_le_bytes(bytes))
    }
}

/// Borrowing codec returning `&[u8]` directly from the payload (zero-copy).
struct BytesBorrowingCodec;

#[derive(Debug, Error)]
#[error("bytes borrow failed")]
struct BytesBorrowError;

impl Encode<[u8]> for BytesBorrowingCodec {
    type Error = BytesBorrowError;

    fn encode(&self, value: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Ok(value.to_vec())
    }
}

impl BorrowingDecode<[u8]> for BytesBorrowingCodec {
    type Error = BytesBorrowError;

    fn decode<'a>(&self, _name: &str, payload: &'a [u8]) -> Result<&'a [u8], Self::Error> {
        Ok(payload)
    }
}

/// Codec that unconditionally fails — used to assert
/// `DecodeStreamError::Decode` propagation.
struct AlwaysFailingOwningCodec;

#[derive(Debug, Error)]
#[error("codec hard-failed: {0}")]
struct CodecHardFail(&'static str);

impl Encode<u64> for AlwaysFailingOwningCodec {
    type Error = CodecHardFail;

    fn encode(&self, _v: &u64) -> Result<Vec<u8>, Self::Error> {
        Err(CodecHardFail("encode"))
    }
}

impl Decode<u64> for AlwaysFailingOwningCodec {
    type Error = CodecHardFail;

    fn decode(&self, _n: &str, _p: &[u8]) -> Result<u64, Self::Error> {
        Err(CodecHardFail("decode"))
    }
}

struct AlwaysFailingBorrowingCodec;

impl Encode<[u8]> for AlwaysFailingBorrowingCodec {
    type Error = CodecHardFail;

    fn encode(&self, _v: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Err(CodecHardFail("encode"))
    }
}

impl BorrowingDecode<[u8]> for AlwaysFailingBorrowingCodec {
    type Error = CodecHardFail;

    fn decode<'a>(&self, _n: &str, _p: &'a [u8]) -> Result<&'a [u8], Self::Error> {
        Err(CodecHardFail("decode"))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Test fixtures — upcasters
// ═══════════════════════════════════════════════════════════════════════════

/// Upcaster that prepends a `1` byte to every payload, verifying the
/// transform runs ahead of the codec.
struct PrependOne;

#[derive(Debug, Error)]
#[error("prepend transform failed")]
struct PrependError;

impl Upcaster for PrependOne {
    type Error = PrependError;

    fn apply<'a>(
        &self,
        morsel: EventMorsel<'a>,
    ) -> Result<EventMorsel<'a>, UpcastError<Self::Error>> {
        let mut new_payload = Vec::with_capacity(morsel.payload().len() + 1);
        new_payload.push(1);
        new_payload.extend_from_slice(morsel.payload());
        let version = Version::new(morsel.schema_version().as_u64() + 1)
            .expect("incremented version is non-zero");
        Ok(morsel
            .with_payload(Cow::Owned(new_payload))
            .with_schema_version(version))
    }

    fn current_version(&self, _event_type: &str) -> Option<Version> {
        Version::new(2)
    }
}

/// Upcaster that always returns `TransformFailed`.
struct FailingUpcaster;

#[derive(Debug, Error)]
#[error("upcast transform exploded")]
struct UpcastBoom;

impl Upcaster for FailingUpcaster {
    type Error = UpcastBoom;

    fn apply<'a>(
        &self,
        morsel: EventMorsel<'a>,
    ) -> Result<EventMorsel<'a>, UpcastError<Self::Error>> {
        Err(UpcastError::TransformFailed {
            event_type: ArrayString::<64>::from(morsel.event_type()).unwrap_or_default(),
            schema_version: morsel.schema_version(),
            source: UpcastBoom,
        })
    }

    fn current_version(&self, _event_type: &str) -> Option<Version> {
        Version::new(2)
    }
}

/// Upcaster that records the (event_type, schema_version) pair it saw
/// for each morsel — used to verify ordering and schema threading.
struct RecordingUpcaster {
    seen: Arc<Mutex<Vec<(String, u64)>>>,
}

impl RecordingUpcaster {
    fn new() -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn seen(&self) -> Vec<(String, u64)> {
        self.seen.lock().expect("test mutex").clone()
    }
}

impl Upcaster for RecordingUpcaster {
    type Error = Infallible;

    fn apply<'a>(
        &self,
        morsel: EventMorsel<'a>,
    ) -> Result<EventMorsel<'a>, UpcastError<Self::Error>> {
        self.seen.lock().expect("test mutex").push((
            morsel.event_type().to_owned(),
            morsel.schema_version().as_u64(),
        ));
        Ok(morsel)
    }

    fn current_version(&self, _event_type: &str) -> Option<Version> {
        None
    }
}

// Convenience: 8-byte LE encoding for `u64` test payloads.
fn enc_u64(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 1 — EventStream trait contract
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn event_stream_yields_envelopes() {
    let mut stream = VecStream::new(vec![
        (1, "Created".into(), vec![1]),
        (2, "Updated".into(), vec![2]),
    ]);

    {
        let e1 = stream.next().await.unwrap().unwrap();
        assert_eq!(e1.version(), Version::new(1).unwrap());
        assert_eq!(e1.event_type(), "Created");
    }

    {
        let e2 = stream.next().await.unwrap().unwrap();
        assert_eq!(e2.version(), Version::new(2).unwrap());
        assert_eq!(e2.event_type(), "Updated");
    }

    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_empty() {
    let mut stream = VecStream::new(vec![]);
    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_single_event() {
    let mut stream = VecStream::new(vec![(1, "OnlyEvent".into(), vec![99])]);
    {
        let env = stream.next().await.unwrap().unwrap();
        assert_eq!(env.event_type(), "OnlyEvent");
        assert_eq!(env.payload(), &[99]);
        assert_eq!(env.version(), Version::new(1).unwrap());
        assert_eq!(env.schema_version(), 1);
    }
    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_envelope_borrows_from_cursor() {
    let mut stream = VecStream::new(vec![(1, "Event".into(), vec![42])]);

    {
        let envelope = stream.next().await.unwrap().unwrap();
        // The slice returned by payload() borrows from the cursor's row buffer.
        assert_eq!(envelope.payload(), &[42]);
    }

    assert!(stream.next().await.unwrap().is_none());
}

#[tokio::test]
async fn event_stream_fused_after_none_repeated() {
    let mut stream = VecStream::new(vec![(1, "E".into(), vec![])]);

    let _ = stream.next().await.unwrap().unwrap();

    // First and every subsequent call must return None.
    for _ in 0..16 {
        assert!(
            stream.next().await.unwrap().is_none(),
            "stream broke fused contract"
        );
    }
}

#[tokio::test]
async fn event_stream_fused_when_initially_empty() {
    let mut stream = VecStream::new(vec![]);
    for _ in 0..8 {
        assert!(stream.next().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn event_stream_threads_custom_schema_version() {
    let mut stream = SchemaStream::new(vec![
        (1, "E".into(), 1, vec![]),
        (2, "E".into(), 3, vec![]),
        (3, "E".into(), 42, vec![]),
    ]);

    let sv1 = stream.next().await.unwrap().unwrap().schema_version();
    let sv2 = stream.next().await.unwrap().unwrap().schema_version();
    let sv3 = stream.next().await.unwrap().unwrap().schema_version();

    assert_eq!((sv1, sv2, sv3), (1, 3, 42));
}

#[tokio::test]
async fn event_stream_carries_metadata_to_envelope() {
    let mut stream = MetadataStream::new(vec![
        (1, "E".into(), vec![], "alice".into()),
        (2, "E".into(), vec![], "bob".into()),
    ]);

    let m1 = stream.next().await.unwrap().unwrap().metadata().clone();
    let m2 = stream.next().await.unwrap().unwrap().metadata().clone();
    assert_eq!(m1, "alice");
    assert_eq!(m2, "bob");
}

#[tokio::test]
async fn event_stream_propagates_error_immediately() {
    let mut stream = FailingStream::new(vec![(1, "E".into(), vec![])], 0);
    let err = stream.next().await.expect_err("expected forced failure");
    assert_eq!(err.0, "forced failure");
}

#[tokio::test]
async fn event_stream_propagates_error_mid_iteration() {
    let mut stream = FailingStream::new(
        vec![
            (1, "E".into(), vec![1]),
            (2, "E".into(), vec![2]),
            (3, "E".into(), vec![3]),
        ],
        2,
    );

    assert_eq!(
        stream.next().await.unwrap().unwrap().version(),
        Version::new(1).unwrap()
    );
    assert_eq!(
        stream.next().await.unwrap().unwrap().version(),
        Version::new(2).unwrap()
    );
    assert!(stream.next().await.is_err());
}

#[tokio::test]
async fn event_stream_handles_large_sequence() {
    const N: u64 = 1024;
    let rows: Vec<_> = (1..=N).map(|v| (v, "Tick".into(), enc_u64(v))).collect();
    let mut stream = VecStream::new(rows);

    let mut count = 0u64;
    let mut last_version = 0u64;
    while let Some(env) = stream.next().await.unwrap() {
        let v = env.version().as_u64();
        assert!(v > last_version, "stream returned non-monotonic versions");
        last_version = v;
        count += 1;
    }
    assert_eq!(count, N);
    assert_eq!(last_version, N);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 2 — EventStreamExt::try_fold (primitive)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn try_fold_empty_returns_init() {
    let mut stream = VecStream::new(vec![]);
    let result: Result<u64, Infallible> = stream.try_fold(123, |acc, _env| Ok(acc + 1)).await;
    assert_eq!(result.unwrap(), 123);
}

#[tokio::test]
async fn try_fold_single_event_folds_once() {
    let mut stream = VecStream::new(vec![(1, "E".into(), vec![7])]);
    let result: Result<u64, Infallible> = stream
        .try_fold(0, |acc, env| Ok(acc + u64::from(env.payload()[0])))
        .await;
    assert_eq!(result.unwrap(), 7);
}

#[tokio::test]
async fn try_fold_multiple_events_in_order() {
    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![1]),
        (2, "E".into(), vec![2]),
        (3, "E".into(), vec![3]),
        (4, "E".into(), vec![4]),
    ]);
    let collected: Result<Vec<u8>, Infallible> = stream
        .try_fold(Vec::new(), |mut acc, env| {
            acc.push(env.payload()[0]);
            Ok(acc)
        })
        .await;
    assert_eq!(collected.unwrap(), vec![1, 2, 3, 4]);
}

#[tokio::test]
async fn try_fold_closure_error_short_circuits() {
    #[derive(Debug, Error)]
    #[error("stop at {0}")]
    struct Stop(u8);
    impl From<Infallible> for Stop {
        fn from(value: Infallible) -> Self {
            match value {}
        }
    }

    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![10]),
        (2, "E".into(), vec![20]),
        (3, "E".into(), vec![30]),
    ]);
    let calls = Arc::new(Mutex::new(0u32));
    let calls_clone = Arc::clone(&calls);

    let result: Result<u32, Stop> = stream
        .try_fold(0u32, move |acc, env| {
            *calls_clone.lock().unwrap() += 1;
            if env.payload()[0] == 20 {
                Err(Stop(env.payload()[0]))
            } else {
                Ok(acc + 1)
            }
        })
        .await;

    let err = result.unwrap_err();
    assert!(matches!(err, Stop(20)));
    // Closure was called twice (event 10 ok, event 20 errored).
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn try_fold_stream_error_short_circuits() {
    #[derive(Debug, Error)]
    enum FoldErr {
        #[error("io: {0}")]
        Io(#[from] StreamIoError),
    }

    let mut stream = FailingStream::new(
        vec![
            (1, "E".into(), vec![1]),
            (2, "E".into(), vec![2]),
            (3, "E".into(), vec![3]),
        ],
        1,
    );
    let result: Result<u32, FoldErr> = stream.try_fold(0u32, |acc, _env| Ok(acc + 1)).await;
    assert!(matches!(result, Err(FoldErr::Io(StreamIoError(_)))));
}

#[tokio::test]
async fn try_fold_init_returned_verbatim_on_empty() {
    // A non-default init must survive untouched.
    let mut stream = VecStream::new(vec![]);
    let init = vec![99u8, 88, 77];
    let out: Result<Vec<u8>, Infallible> = stream.try_fold(init.clone(), |acc, _env| Ok(acc)).await;
    assert_eq!(out.unwrap(), init);
}

#[tokio::test]
async fn try_fold_accumulator_threaded_through() {
    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![10]),
        (2, "E".into(), vec![20]),
        (3, "E".into(), vec![30]),
    ]);
    let out: Result<u32, Infallible> = stream
        .try_fold(1u32, |acc, env| Ok(acc * u32::from(env.payload()[0])))
        .await;
    // 1 * 10 * 20 * 30
    assert_eq!(out.unwrap(), 6000);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 3 — EventStreamExt::try_for_each
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn try_for_each_empty_does_not_invoke_closure() {
    let mut stream = VecStream::new(vec![]);
    let count = Arc::new(Mutex::new(0u32));
    let count_clone = Arc::clone(&count);
    let result: Result<(), Infallible> = stream
        .try_for_each(move |_env| {
            *count_clone.lock().unwrap() += 1;
            Ok(())
        })
        .await;
    result.unwrap();
    assert_eq!(*count.lock().unwrap(), 0);
}

#[tokio::test]
async fn try_for_each_called_per_event_in_order() {
    let mut stream = VecStream::new(vec![
        (1, "A".into(), vec![1]),
        (2, "B".into(), vec![2]),
        (3, "C".into(), vec![3]),
    ]);
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let order_clone = Arc::clone(&order);
    let result: Result<(), Infallible> = stream
        .try_for_each(move |env| {
            order_clone
                .lock()
                .unwrap()
                .push(env.event_type().to_owned());
            Ok(())
        })
        .await;
    result.unwrap();
    assert_eq!(order.lock().unwrap().as_slice(), &["A", "B", "C"]);
}

#[tokio::test]
async fn try_for_each_closure_error_short_circuits() {
    #[derive(Debug, Error)]
    #[error("stop")]
    struct Stop;
    impl From<Infallible> for Stop {
        fn from(value: Infallible) -> Self {
            match value {}
        }
    }

    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![1]),
        (2, "E".into(), vec![2]),
        (3, "E".into(), vec![3]),
    ]);
    let count = Arc::new(Mutex::new(0u32));
    let count_clone = Arc::clone(&count);
    let result: Result<(), Stop> = stream
        .try_for_each(move |env| {
            *count_clone.lock().unwrap() += 1;
            if env.payload()[0] == 2 {
                Err(Stop)
            } else {
                Ok(())
            }
        })
        .await;
    assert!(result.is_err());
    // Stopped after second event => third never reached.
    assert_eq!(*count.lock().unwrap(), 2);
}

#[tokio::test]
async fn try_for_each_stream_error_propagates() {
    #[derive(Debug, Error)]
    enum E {
        #[error("io: {0}")]
        Io(#[from] StreamIoError),
    }

    let mut stream = FailingStream::new(vec![(1, "E".into(), vec![1])], 0);
    let result: Result<(), E> = stream.try_for_each(|_env| Ok(())).await;
    assert!(matches!(result, Err(E::Io(_))));
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 4 — EventStreamExt::try_collect_map
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn try_collect_map_empty_returns_empty_vec() {
    let mut stream = VecStream::new(vec![]);
    let result: Result<Vec<u8>, Infallible> =
        stream.try_collect_map(|env| Ok(env.payload()[0])).await;
    assert_eq!(result.unwrap(), Vec::<u8>::new());
}

#[tokio::test]
async fn try_collect_map_preserves_order() {
    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![10]),
        (2, "E".into(), vec![20]),
        (3, "E".into(), vec![30]),
    ]);
    let result: Result<Vec<u8>, Infallible> =
        stream.try_collect_map(|env| Ok(env.payload()[0])).await;
    assert_eq!(result.unwrap(), vec![10, 20, 30]);
}

#[tokio::test]
async fn try_collect_map_extracts_owned_event_type() {
    let mut stream = VecStream::new(vec![
        (1, "Alpha".into(), vec![]),
        (2, "Beta".into(), vec![]),
    ]);
    let result: Result<Vec<String>, Infallible> = stream
        .try_collect_map(|env| Ok(env.event_type().to_owned()))
        .await;
    assert_eq!(result.unwrap(), vec!["Alpha".to_string(), "Beta".into()]);
}

#[tokio::test]
async fn try_collect_map_closure_error_short_circuits() {
    #[derive(Debug, Error)]
    #[error("bad: {0}")]
    struct Bad(u8);
    impl From<Infallible> for Bad {
        fn from(value: Infallible) -> Self {
            match value {}
        }
    }

    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![1]),
        (2, "E".into(), vec![2]),
        (3, "E".into(), vec![3]),
    ]);
    let result: Result<Vec<u8>, Bad> = stream
        .try_collect_map(|env| {
            let b = env.payload()[0];
            if b == 2 { Err(Bad(b)) } else { Ok(b) }
        })
        .await;
    assert!(matches!(result, Err(Bad(2))));
}

#[tokio::test]
async fn try_collect_map_stream_error_propagates() {
    #[derive(Debug, Error)]
    enum E {
        #[error("io: {0}")]
        Io(#[from] StreamIoError),
    }
    let mut stream = FailingStream::new(vec![(1, "E".into(), vec![1])], 0);
    let result: Result<Vec<u8>, E> = stream.try_collect_map(|env| Ok(env.payload()[0])).await;
    assert!(matches!(result, Err(E::Io(_))));
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 5 — EventStreamExt::try_count
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn try_count_empty_returns_zero() {
    let mut stream = VecStream::new(vec![]);
    assert_eq!(stream.try_count().await.unwrap(), 0);
}

#[tokio::test]
async fn try_count_returns_exact_event_count() {
    for n in [1usize, 2, 5, 10, 100] {
        let rows: Vec<_> = (1..=n as u64)
            .map(|v| (v, "E".into(), vec![v as u8]))
            .collect();
        let mut stream = VecStream::new(rows);
        assert_eq!(stream.try_count().await.unwrap(), n, "n={}", n);
    }
}

#[tokio::test]
async fn try_count_stream_error_propagates() {
    let mut stream = FailingStream::new(vec![(1, "E".into(), vec![1])], 0);
    let err = stream.try_count().await.expect_err("forced failure");
    assert_eq!(err.0, "forced failure");
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 6 — Combinator consistency cross-checks
// ═══════════════════════════════════════════════════════════════════════════

fn make_rows(n: u64) -> Vec<(u64, String, Vec<u8>)> {
    (1..=n).map(|v| (v, "E".into(), vec![v as u8])).collect()
}

#[tokio::test]
async fn count_matches_collect_len() {
    let n = 17u64;
    let mut s_count = VecStream::new(make_rows(n));
    let mut s_collect = VecStream::new(make_rows(n));

    let count = s_count.try_count().await.unwrap();
    let vec: Vec<u8> = s_collect
        .try_collect_map::<u8, Infallible, _>(|env| Ok(env.payload()[0]))
        .await
        .unwrap();
    assert_eq!(count, vec.len());
    assert_eq!(count as u64, n);
}

#[tokio::test]
async fn for_each_call_count_matches_count() {
    let n = 9u64;
    let mut s_count = VecStream::new(make_rows(n));
    let mut s_for_each = VecStream::new(make_rows(n));

    let count = s_count.try_count().await.unwrap();
    let calls = Arc::new(Mutex::new(0usize));
    let calls_clone = Arc::clone(&calls);
    let _: Result<(), Infallible> = s_for_each
        .try_for_each(move |_env| {
            *calls_clone.lock().unwrap() += 1;
            Ok(())
        })
        .await;
    assert_eq!(count, *calls.lock().unwrap());
}

#[tokio::test]
async fn fold_matches_iterative_sum() {
    let n = 50u64;
    let rows: Vec<_> = (1..=n).map(|v| (v, "E".into(), enc_u64(v))).collect();
    let mut s_fold = VecStream::new(rows);

    let sum_fold: u64 = s_fold
        .try_fold::<u64, Infallible, _>(0u64, |acc, env| {
            let bytes: [u8; 8] = env.payload().try_into().unwrap();
            Ok(acc + u64::from_le_bytes(bytes))
        })
        .await
        .unwrap();

    let sum_iter: u64 = (1..=n).sum();
    assert_eq!(sum_fold, sum_iter);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 7 — DecoderBuilder typestate transitions
// ═══════════════════════════════════════════════════════════════════════════
//
// The typestate prevents `.build()` until a codec is bound. The compile-fail
// fixture `tests/compile_fail/decoder_build_without_codec.rs` verifies that
// rejection. Here we exercise the four allowed combinations.

#[tokio::test]
async fn builder_owning_codec_no_upcaster() {
    let stream = VecStream::new(vec![(1, "Tick".into(), enc_u64(42))]);
    let codec = U64Codec;
    let v: Result<Vec<u64>, DecodeStreamError<_, _, _>> = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(Vec::new(), |mut acc, _ver, e: u64| {
            acc.push(e);
            Ok(acc)
        })
        .await;
    assert_eq!(v.unwrap(), vec![42]);
}

#[tokio::test]
async fn builder_owning_codec_with_upcaster() {
    let stream = VecStream::new(vec![(1, "E".into(), enc_u64(1))]);
    let codec = U64Codec;
    let upcaster = RecordingUpcaster::new();
    let count: Result<usize, DecodeStreamError<_, _, _>> = stream
        .decoder()
        .codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold(0usize, |c, _v, _e: u64| Ok(c + 1))
        .await;
    assert_eq!(count.unwrap(), 1);
    assert_eq!(upcaster.seen().len(), 1);
}

#[tokio::test]
async fn builder_borrowing_codec_no_upcaster() {
    let stream = VecStream::new(vec![(1, "Blob".into(), vec![1, 2, 3])]);
    let codec = BytesBorrowingCodec;
    let len: Result<usize, DecodeStreamError<_, _, _>> = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(0usize, |acc, _v, b: &[u8]| Ok(acc + b.len()))
        .await;
    assert_eq!(len.unwrap(), 3);
}

#[tokio::test]
async fn builder_borrowing_codec_with_upcaster() {
    let stream = VecStream::new(vec![(1, "Blob".into(), vec![9])]);
    let codec = BytesBorrowingCodec;
    let upcaster = RecordingUpcaster::new();
    let len: Result<usize, DecodeStreamError<_, _, _>> = stream
        .decoder()
        .borrowing_codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold(0usize, |acc, _v, b: &[u8]| Ok(acc + b.len()))
        .await;
    assert_eq!(len.unwrap(), 1);
    assert_eq!(upcaster.seen().len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 8 — DecodedStream (owning codec) pipeline
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn decoded_owning_folds_to_sum() {
    let stream = VecStream::new(vec![
        (1, "Tick".into(), enc_u64(10)),
        (2, "Tick".into(), enc_u64(20)),
        (3, "Tick".into(), enc_u64(12)),
    ]);
    let codec = U64Codec;

    let total = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            0u64,
            |sum, version, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> {
                assert!(version.as_u64() >= 1);
                Ok(sum + e)
            },
        )
        .await
        .unwrap();

    assert_eq!(total, 42);
}

#[tokio::test]
async fn decoded_owning_threads_versions() {
    let stream = VecStream::new(vec![
        (1, "E".into(), enc_u64(0)),
        (2, "E".into(), enc_u64(0)),
        (3, "E".into(), enc_u64(0)),
    ]);
    let codec = U64Codec;

    let versions: Vec<u64> = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            Vec::<u64>::new(),
            |mut v, version, _e: u64| -> Result<_, DecodeStreamError<_, _, _>> {
                v.push(version.as_u64());
                Ok(v)
            },
        )
        .await
        .unwrap();

    assert_eq!(versions, vec![1, 2, 3]);
}

#[tokio::test]
async fn decoded_owning_upcaster_runs_before_codec() {
    // Upcaster prepends 1 byte, so the 8-byte u64 payload becomes 9 bytes
    // and the codec must reject it.
    let stream = VecStream::new(vec![(1, "Tick".into(), enc_u64(5))]);
    let codec = U64Codec;
    let upcaster = PrependOne;

    let result = stream
        .decoder()
        .codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold(
            0u64,
            |sum, _v, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> { Ok(sum + e) },
        )
        .await;

    assert!(matches!(
        result,
        Err(DecodeStreamError::Decode(U64DecodeError(9)))
    ));
}

#[tokio::test]
async fn decoded_owning_empty_returns_initial() {
    let stream = VecStream::new(vec![]);
    let codec = U64Codec;

    let total = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            99u64,
            |sum, _v, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> { Ok(sum + e) },
        )
        .await
        .unwrap();

    assert_eq!(total, 99);
}

#[tokio::test]
async fn decoded_owning_closure_error_short_circuits() {
    #[derive(Debug, Error)]
    #[error("custom: {0}")]
    struct CustomErr(u64);

    impl<S, C, U> From<DecodeStreamError<S, C, U>> for CustomErr
    where
        S: std::fmt::Display,
        C: std::fmt::Display,
        U: std::fmt::Display,
    {
        fn from(_e: DecodeStreamError<S, C, U>) -> Self {
            CustomErr(0)
        }
    }

    let stream = VecStream::new(vec![
        (1, "E".into(), enc_u64(10)),
        (2, "E".into(), enc_u64(99)),
        (3, "E".into(), enc_u64(30)),
    ]);
    let codec = U64Codec;

    let result = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(0u64, |sum, _v, e: u64| -> Result<u64, CustomErr> {
            if e == 99 {
                Err(CustomErr(e))
            } else {
                Ok(sum + e)
            }
        })
        .await;

    assert!(matches!(result, Err(CustomErr(99))));
}

#[tokio::test]
async fn decoded_owning_stream_error_to_decode_stream_error_stream() {
    let stream = FailingStream::new(vec![(1, "E".into(), enc_u64(1))], 0);
    let codec = U64Codec;
    let result: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold((), |(), _v, _e: u64| Ok(()))
        .await;
    assert!(matches!(result, Err(DecodeStreamError::Stream(_))));
}

#[tokio::test]
async fn decoded_owning_upcaster_error_to_decode_stream_error_upcast() {
    let stream = VecStream::new(vec![(1, "E".into(), enc_u64(1))]);
    let codec = U64Codec;
    let upcaster = FailingUpcaster;
    let result: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold((), |(), _v, _e: u64| Ok(()))
        .await;
    assert!(matches!(result, Err(DecodeStreamError::Upcast(_))));
}

#[tokio::test]
async fn decoded_owning_codec_error_to_decode_stream_error_codec() {
    let stream = VecStream::new(vec![(1, "E".into(), enc_u64(1))]);
    let codec = AlwaysFailingOwningCodec;
    let result: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold((), |(), _v, _e: u64| Ok(()))
        .await;
    assert!(matches!(result, Err(DecodeStreamError::Decode(_))));
}

#[tokio::test]
async fn decoded_owning_schema_version_visible_to_upcaster() {
    let stream = SchemaStream::new(vec![
        (1, "E".into(), 1, enc_u64(0)),
        (2, "E".into(), 7, enc_u64(0)),
        (3, "E".into(), 42, enc_u64(0)),
    ]);
    let codec = U64Codec;
    let upcaster = RecordingUpcaster::new();
    let _: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold((), |(), _v, _e: u64| Ok(()))
        .await;
    let seen = upcaster.seen();
    let observed_versions: Vec<u64> = seen.iter().map(|(_, sv)| *sv).collect();
    assert_eq!(observed_versions, vec![1, 7, 42]);
}

#[tokio::test]
async fn decoded_owning_noop_upcaster_passthrough() {
    // With the default `()` upcaster, payloads must reach the codec unmodified.
    let stream = VecStream::new(vec![(1, "Tick".into(), enc_u64(123))]);
    let codec = U64Codec;
    let v: u64 = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            0u64,
            |_a, _v, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> { Ok(e) },
        )
        .await
        .unwrap();
    assert_eq!(v, 123);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 9 — BorrowedDecodedStream (borrowing codec) pipeline
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn borrowed_folds_payload_size() {
    let stream = VecStream::new(vec![
        (1, "Blob".into(), vec![1, 2, 3]),
        (2, "Blob".into(), vec![4, 5]),
        (3, "Blob".into(), vec![6, 7, 8, 9]),
    ]);
    let codec = BytesBorrowingCodec;

    let total = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(
            0usize,
            |sum, _v, bytes: &[u8]| -> Result<usize, DecodeStreamError<_, _, _>> {
                Ok(sum + bytes.len())
            },
        )
        .await
        .unwrap();

    assert_eq!(total, 9);
}

#[tokio::test]
async fn borrowed_threads_versions() {
    let stream = VecStream::new(vec![
        (1, "E".into(), vec![]),
        (2, "E".into(), vec![]),
        (5, "E".into(), vec![]),
    ]);
    let codec = BytesBorrowingCodec;

    let versions: Vec<u64> = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(
            Vec::<u64>::new(),
            |mut v, ver, _b: &[u8]| -> Result<_, DecodeStreamError<_, _, _>> {
                v.push(ver.as_u64());
                Ok(v)
            },
        )
        .await
        .unwrap();
    assert_eq!(versions, vec![1, 2, 5]);
}

#[tokio::test]
async fn borrowed_empty_returns_initial() {
    let stream = VecStream::new(vec![]);
    let codec = BytesBorrowingCodec;
    let total: usize = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(
            7usize,
            |sum, _v, _b: &[u8]| -> Result<usize, DecodeStreamError<_, _, _>> { Ok(sum) },
        )
        .await
        .unwrap();
    assert_eq!(total, 7);
}

#[tokio::test]
async fn borrowed_closure_error_short_circuits() {
    #[derive(Debug, Error)]
    #[error("stop")]
    struct Stop;
    impl<S, C, U> From<DecodeStreamError<S, C, U>> for Stop
    where
        S: std::fmt::Display,
        C: std::fmt::Display,
        U: std::fmt::Display,
    {
        fn from(_e: DecodeStreamError<S, C, U>) -> Self {
            Stop
        }
    }

    let stream = VecStream::new(vec![
        (1, "E".into(), vec![1]),
        (2, "E".into(), vec![2]),
        (3, "E".into(), vec![3]),
    ]);
    let codec = BytesBorrowingCodec;
    let calls = Arc::new(Mutex::new(0usize));
    let calls_clone = Arc::clone(&calls);

    let result = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold((), move |(), _v, b: &[u8]| -> Result<(), Stop> {
            *calls_clone.lock().unwrap() += 1;
            if b[0] == 2 { Err(Stop) } else { Ok(()) }
        })
        .await;
    assert!(matches!(result, Err(Stop)));
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn borrowed_stream_error_to_decode_stream_error_stream() {
    let stream = FailingStream::new(vec![(1, "E".into(), vec![1])], 0);
    let codec = BytesBorrowingCodec;
    let result: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold((), |(), _v, _b: &[u8]| Ok(()))
        .await;
    assert!(matches!(result, Err(DecodeStreamError::Stream(_))));
}

#[tokio::test]
async fn borrowed_upcaster_error_to_decode_stream_error_upcast() {
    let stream = VecStream::new(vec![(1, "E".into(), vec![1])]);
    let codec = BytesBorrowingCodec;
    let upcaster = FailingUpcaster;
    let result: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .borrowing_codec(&codec)
        .upcaster(&upcaster)
        .build()
        .try_fold((), |(), _v, _b: &[u8]| Ok(()))
        .await;
    assert!(matches!(result, Err(DecodeStreamError::Upcast(_))));
}

#[tokio::test]
async fn borrowed_codec_error_to_decode_stream_error_codec() {
    let stream = VecStream::new(vec![(1, "E".into(), vec![1])]);
    let codec = AlwaysFailingBorrowingCodec;
    let result: Result<(), DecodeStreamError<_, _, _>> = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold((), |(), _v, _b: &[u8]| Ok(()))
        .await;
    assert!(matches!(result, Err(DecodeStreamError::Decode(_))));
}

#[tokio::test]
async fn borrowed_noop_upcaster_passthrough() {
    let stream = VecStream::new(vec![(1, "Blob".into(), vec![10, 20, 30])]);
    let codec = BytesBorrowingCodec;
    let observed: Vec<u8> = stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(
            Vec::new(),
            |mut acc, _v, b: &[u8]| -> Result<Vec<u8>, DecodeStreamError<_, _, _>> {
                acc.extend_from_slice(b);
                Ok(acc)
            },
        )
        .await
        .unwrap();
    assert_eq!(observed, vec![10, 20, 30]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 10 — Defensive boundary: combinators relay misbehaving streams
// faithfully (no panic, no reordering). Detection lives at the consumer
// layer (e.g. `AggregateRoot::replay`), not in `stream.rs`.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn non_monotonic_versions_pass_through_combinators() {
    // Stream yields version 3, 1, 2 — illegal per the trait contract.
    let mut stream = VecStream::new(vec![
        (3, "E".into(), vec![]),
        (1, "E".into(), vec![]),
        (2, "E".into(), vec![]),
    ]);
    let versions: Result<Vec<u64>, Infallible> = stream
        .try_collect_map(|env| Ok(env.version().as_u64()))
        .await;
    // Combinator does not validate — observed order matches yielded order.
    assert_eq!(versions.unwrap(), vec![3, 1, 2]);
}

#[tokio::test]
async fn duplicate_versions_pass_through_combinators() {
    let mut stream = VecStream::new(vec![
        (1, "A".into(), vec![]),
        (1, "B".into(), vec![]),
        (1, "C".into(), vec![]),
    ]);
    let count = stream.try_count().await.unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn version_gaps_pass_through_combinators() {
    let mut stream = VecStream::new(vec![
        (1, "E".into(), vec![]),
        (5, "E".into(), vec![]),
        (100, "E".into(), vec![]),
    ]);
    let versions: Result<Vec<u64>, Infallible> = stream
        .try_collect_map(|env| Ok(env.version().as_u64()))
        .await;
    assert_eq!(versions.unwrap(), vec![1, 5, 100]);
}

#[tokio::test]
async fn decoder_relays_non_monotonic_versions_to_closure() {
    let stream = VecStream::new(vec![
        (3, "Tick".into(), enc_u64(30)),
        (1, "Tick".into(), enc_u64(10)),
        (2, "Tick".into(), enc_u64(20)),
    ]);
    let codec = U64Codec;
    let observed: Vec<(u64, u64)> = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            Vec::new(),
            |mut acc, v, e: u64| -> Result<Vec<(u64, u64)>, DecodeStreamError<_, _, _>> {
                acc.push((v.as_u64(), e));
                Ok(acc)
            },
        )
        .await
        .unwrap();
    assert_eq!(observed, vec![(3, 30), (1, 10), (2, 20)]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 11 — Send + concurrency
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_usable_across_tokio_spawn() {
    let handle = tokio::spawn(async move {
        let mut stream = VecStream::new(vec![(1, "E".into(), vec![1]), (2, "E".into(), vec![2])]);
        stream.try_count().await
    });
    let count = handle.await.unwrap().unwrap();
    assert_eq!(count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disjoint_streams_run_in_parallel_tasks() {
    let t1 = tokio::spawn(async {
        let mut stream = VecStream::new((1..=50u64).map(|v| (v, "A".into(), enc_u64(v))).collect());
        stream
            .try_fold::<u64, Infallible, _>(0u64, |acc, env| {
                let bytes: [u8; 8] = env.payload().try_into().unwrap();
                Ok(acc + u64::from_le_bytes(bytes))
            })
            .await
            .unwrap()
    });
    let t2 = tokio::spawn(async {
        let mut stream = VecStream::new((1..=50u64).map(|v| (v, "B".into(), enc_u64(v))).collect());
        stream
            .try_fold::<u64, Infallible, _>(0u64, |acc, env| {
                let bytes: [u8; 8] = env.payload().try_into().unwrap();
                Ok(acc + u64::from_le_bytes(bytes))
            })
            .await
            .unwrap()
    });
    let (a, b) = tokio::join!(t1, t2);
    let expected: u64 = (1..=50).sum();
    assert_eq!(a.unwrap(), expected);
    assert_eq!(b.unwrap(), expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decoded_stream_usable_across_tokio_spawn() {
    let handle = tokio::spawn(async move {
        let stream = VecStream::new(vec![
            (1, "Tick".into(), enc_u64(10)),
            (2, "Tick".into(), enc_u64(32)),
        ]);
        let codec = U64Codec;
        stream
            .decoder()
            .codec(&codec)
            .build()
            .try_fold(
                0u64,
                |s, _v, e: u64| -> Result<u64, DecodeStreamError<_, _, _>> { Ok(s + e) },
            )
            .await
    });
    let total = handle.await.unwrap().unwrap();
    assert_eq!(total, 42);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 12 — Property-based tests
// ═══════════════════════════════════════════════════════════════════════════

fn version_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![
        1u64..=1024,
        Just(1u64),
        Just(2u64),
        Just(u64::MAX - 1),
        Just(u64::MAX),
    ]
}

fn payload_strategy() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        Just(Vec::<u8>::new()),
        prop::collection::vec(any::<u8>(), 1..=128),
        prop::collection::vec(any::<u8>(), 0..=4096),
    ]
}

fn event_type_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z][a-zA-Z0-9_]{0,16}"
}

fn rows_strategy() -> impl Strategy<Value = Vec<(u64, String, Vec<u8>)>> {
    prop::collection::vec(
        (
            version_strategy(),
            event_type_strategy(),
            payload_strategy(),
        ),
        0..=32,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `try_count` equals the length of `try_collect_map` for any stream.
    #[test]
    fn prop_count_matches_collect_len(rows in rows_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut s_count = VecStream::new(rows.clone());
            let mut s_collect = VecStream::new(rows.clone());

            let count = s_count.try_count().await.unwrap();
            let vec: Vec<()> = s_collect
                .try_collect_map::<(), Infallible, _>(|_env| Ok(()))
                .await
                .unwrap();
            prop_assert_eq!(count, vec.len());
            prop_assert_eq!(count, rows.len());
            Ok(())
        })?;
    }

    /// `try_fold` observes envelopes in stream order.
    #[test]
    fn prop_fold_preserves_payload_order(rows in rows_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let expected: Vec<Vec<u8>> = rows.iter().map(|(_, _, p)| p.clone()).collect();
            let mut stream = VecStream::new(rows);
            let observed: Vec<Vec<u8>> = stream
                .try_fold::<Vec<Vec<u8>>, Infallible, _>(Vec::new(), |mut acc, env| {
                    acc.push(env.payload().to_vec());
                    Ok(acc)
                })
                .await
                .unwrap();
            prop_assert_eq!(observed, expected);
            Ok(())
        })?;
    }

    /// Decoded pipeline yields exactly `rows.len()` decoded events
    /// (codec is bytes, so it can't fail on arbitrary payloads).
    #[test]
    fn prop_borrowed_decoder_event_count_matches_stream(rows in rows_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let stream = VecStream::new(rows.clone());
            let codec = BytesBorrowingCodec;
            let count = stream
                .decoder()
                .borrowing_codec(&codec)
                .build()
                .try_fold(
                    0usize,
                    |c, _v, _b: &[u8]| -> Result<usize, DecodeStreamError<_, _, _>> { Ok(c + 1) },
                )
                .await
                .unwrap();
            prop_assert_eq!(count, rows.len());
            Ok(())
        })?;
    }

    /// Versions reaching the decoder closure match the envelope versions
    /// the stream yields (no reordering, no synthesis).
    #[test]
    fn prop_decoder_versions_match_envelope_versions(rows in rows_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let expected: Vec<u64> = rows.iter().map(|(v, _, _)| *v).collect();
            let stream = VecStream::new(rows);
            let codec = BytesBorrowingCodec;
            let observed: Vec<u64> = stream
                .decoder()
                .borrowing_codec(&codec)
                .build()
                .try_fold(
                    Vec::new(),
                    |mut acc, ver, _b: &[u8]| -> Result<Vec<u64>, DecodeStreamError<_, _, _>> {
                        acc.push(ver.as_u64());
                        Ok(acc)
                    },
                )
                .await
                .unwrap();
            prop_assert_eq!(observed, expected);
            Ok(())
        })?;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 13 — moved to `tests/stream_alloc_tests.rs`.
//
// The allocation-counting proof for `BorrowedDecodedStream` requires a
// `#[global_allocator]` and a private process-wide counter. Running it in
// the same binary as the other ~80 tests would let concurrent test threads
// pollute the counter. Keeping it in its own integration test binary
// (= its own process) makes the measurement reliable.
// ═══════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════
// Section 14 — Drop tracking (lending discipline at runtime)
//
// `EventStream::next()` returns a `PersistedEnvelope<'_, M>` that owns its
// metadata `M`. The combinators move the envelope into the closure; when
// the closure returns, the envelope (and its M) drop. This section places
// a `DropProbe` in M, then asserts:
//
// (a) `DROP_COUNT` after fold equals the number of envelopes yielded.
// (b) Inside the i-th closure call, `DROP_COUNT == i - 1` — the current
//     envelope's metadata has not dropped yet.
//
// Together these prove the lending iterator contract at runtime, beyond the
// compile-fail test's static guarantee.
// ═══════════════════════════════════════════════════════════════════════════

/// Stream that yields N envelopes, each carrying a fresh [`DropProbe`].
struct DropProbeStream {
    n: usize,
    pos: usize,
    et: String,
    payload: Vec<u8>,
}

impl DropProbeStream {
    fn new(n: usize) -> Self {
        Self {
            n,
            pos: 0,
            et: "E".into(),
            payload: vec![0],
        }
    }
}

impl BaseEventStream<DropProbe> for DropProbeStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a, DropProbe>) -> PersistedEnvelope<'a, DropProbe>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream<DropProbe> for DropProbeStream {
    type Item<'a> = PersistedEnvelope<'a, DropProbe>;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_, DropProbe>>, Self::Error> {
        if self.pos >= self.n {
            return Ok(None);
        }
        self.pos += 1;
        let v = Version::new(self.pos as u64).expect("pos >= 1");
        Ok(Some(PersistedEnvelope::new_unchecked(
            v,
            GlobalSeq::INITIAL,
            &self.et,
            1,
            &self.payload,
            DropProbe,
        )))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn drop_envelope_metadata_drops_exactly_once_per_event() {
    let _guard = DROP_TEST_LOCK.lock().expect("drop lock");
    DROP_COUNT.store(0, Ordering::SeqCst);

    let mut stream = DropProbeStream::new(50);
    let count = stream.try_count().await.expect("count ok");

    assert_eq!(count, 50);
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 50);
}

#[tokio::test(flavor = "current_thread")]
async fn drop_count_inside_closure_equals_iteration_index() {
    let _guard = DROP_TEST_LOCK.lock().expect("drop lock");
    DROP_COUNT.store(0, Ordering::SeqCst);

    let mut stream = DropProbeStream::new(10);
    let observed: Vec<usize> = stream
        .try_collect_map(|env| -> Result<usize, Infallible> {
            // env is alive in this scope. Prior envelopes have dropped.
            let observed = DROP_COUNT.load(Ordering::SeqCst);
            let _ = env.version();
            Ok(observed)
            // env drops here as the closure returns.
        })
        .await
        .expect("collect ok");

    assert_eq!(observed, (0..10).collect::<Vec<_>>());
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 10);
}

#[tokio::test(flavor = "current_thread")]
async fn drop_envelope_drops_even_when_closure_errors() {
    #[derive(Debug, Error)]
    #[error("stop")]
    struct Stop;
    impl From<Infallible> for Stop {
        fn from(value: Infallible) -> Self {
            match value {}
        }
    }

    let _guard = DROP_TEST_LOCK.lock().expect("drop lock");
    DROP_COUNT.store(0, Ordering::SeqCst);

    let mut stream = DropProbeStream::new(10);
    let result: Result<(), Stop> = stream
        .try_for_each(|env| {
            let pos = env.version().as_u64();
            if pos == 4 { Err(Stop) } else { Ok(()) }
        })
        .await;

    assert!(matches!(result, Err(Stop)));
    // The closure consumed 4 envelopes (positions 1, 2, 3, 4), all dropped.
    // No envelope leaks even on the error path.
    assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 4);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 15 — Differential testing (DecodedStream vs BorrowedDecodedStream)
//
// Both arms must observe the exact same (version, event_type, payload-bytes)
// sequence for any input. This catches drift where one arm processes things
// differently from the other.
// ═══════════════════════════════════════════════════════════════════════════

/// Owning codec that returns `Vec<u8>` — the payload cloned per event.
struct BytesOwningCodec;

impl Encode<Vec<u8>> for BytesOwningCodec {
    type Error = Infallible;

    fn encode(&self, value: &Vec<u8>) -> Result<Vec<u8>, Self::Error> {
        Ok(value.clone())
    }
}

impl Decode<Vec<u8>> for BytesOwningCodec {
    type Error = Infallible;

    fn decode(&self, _n: &str, payload: &[u8]) -> Result<Vec<u8>, Self::Error> {
        Ok(payload.to_vec())
    }
}

async fn observe_owning(rows: Vec<(u64, String, Vec<u8>)>) -> Vec<(u64, String, Vec<u8>)> {
    let stream = VecStream::new(rows);
    let codec = BytesOwningCodec;
    stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold(
            Vec::new(),
            |mut acc, v, e: Vec<u8>| -> Result<_, DecodeStreamError<_, _, _>> {
                acc.push((v.as_u64(), "E".to_owned(), e));
                Ok(acc)
            },
        )
        .await
        .expect("owning fold ok")
}

async fn observe_borrowing(rows: Vec<(u64, String, Vec<u8>)>) -> Vec<(u64, String, Vec<u8>)> {
    let stream = VecStream::new(rows);
    let codec = BytesBorrowingCodec;
    stream
        .decoder()
        .borrowing_codec(&codec)
        .build()
        .try_fold(
            Vec::new(),
            |mut acc, v, e: &[u8]| -> Result<_, DecodeStreamError<_, _, _>> {
                acc.push((v.as_u64(), "E".to_owned(), e.to_vec()));
                Ok(acc)
            },
        )
        .await
        .expect("borrowing fold ok")
}

#[tokio::test]
async fn differential_both_decoder_arms_observe_same_sequence() {
    let rows: Vec<(u64, String, Vec<u8>)> = vec![
        (1, "E".into(), vec![]),
        (2, "E".into(), vec![1]),
        (3, "E".into(), vec![1, 2, 3]),
        (5, "E".into(), b"non-utf-stuff\x00\xff\xfe".to_vec()),
    ];
    let owning = observe_owning(rows.clone()).await;
    let borrowing = observe_borrowing(rows).await;
    assert_eq!(owning, borrowing);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Property: for any rows, both arms observe identical sequences.
    #[test]
    fn prop_differential_arms_agree(rows in rows_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async {
            let o = observe_owning(rows.clone()).await;
            let b = observe_borrowing(rows).await;
            prop_assert_eq!(o, b);
            Ok(())
        })?;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 16 — Failure injection proptest
//
// For any rows and arbitrary `fail_at`, exercise `FailingStream`. Three
// behaviors must always hold:
//   * If `fail_at < rows.len()`: fold returns Err and closure was called
//     exactly `fail_at` times.
//   * If `fail_at >= rows.len()`: fold returns Ok(<full result>) and closure
//     was called `rows.len()` times.
//   * The error returned is structurally `StreamIoError`, never accidentally
//     converted to a different variant.
// ═══════════════════════════════════════════════════════════════════════════

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_failure_injection_propagates_correctly(
        rows in rows_strategy(),
        fail_at in 0usize..40,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async {
            let stream = FailingStream::new(rows.clone(), fail_at);
            let calls = Arc::new(Mutex::new(0usize));
            let calls_c = Arc::clone(&calls);

            #[derive(Debug, Error)]
            enum FoldErr {
                #[error("io: {0}")]
                Io(#[from] StreamIoError),
            }

            let result: Result<usize, FoldErr> = {
                let mut s = stream;
                s.try_fold(0usize, move |acc, _env| {
                    *calls_c.lock().unwrap() += 1;
                    Ok(acc + 1)
                }).await
            };

            let call_count = *calls.lock().unwrap();
            let n = rows.len();

            // `FailingStream::next` checks `pos == fail_at` BEFORE yielding,
            // so when `fail_at <= n` the stream errors on call number
            // `fail_at + 1` (after yielding `fail_at` events).
            // When `fail_at > n` the stream exhausts cleanly with `n` events.
            if fail_at <= n {
                prop_assert!(matches!(result, Err(FoldErr::Io(_))));
                prop_assert_eq!(call_count, fail_at);
            } else {
                let count = result.expect("fold ok when no failure");
                prop_assert_eq!(count, n);
                prop_assert_eq!(call_count, n);
            }
            Ok(())
        })?;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 17 — Stateful model-based proptest
//
// PerfectModel: a frozen description of what every combinator MUST produce
// given a row sequence. For each random row sequence we run all four
// combinators on independent fresh streams and assert their results match
// the model AND each other. Catches combinator-combinator drift if one
// combinator's blanket impl is later refactored away from the others.
// ═══════════════════════════════════════════════════════════════════════════

struct PerfectModel {
    rows: Vec<(u64, String, Vec<u8>)>,
}

impl PerfectModel {
    fn count(&self) -> usize {
        self.rows.len()
    }
    fn total_payload_bytes(&self) -> usize {
        self.rows.iter().map(|r| r.2.len()).sum()
    }
    fn payloads(&self) -> Vec<Vec<u8>> {
        self.rows.iter().map(|r| r.2.clone()).collect()
    }
    fn event_types(&self) -> Vec<String> {
        self.rows.iter().map(|r| r.1.clone()).collect()
    }
    fn versions(&self) -> Vec<u64> {
        self.rows.iter().map(|r| r.0).collect()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn prop_model_matches_every_combinator(rows in rows_strategy()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        let model = PerfectModel { rows: rows.clone() };
        rt.block_on(async {
            // try_count
            let mut s1 = VecStream::new(rows.clone());
            let observed_count = s1.try_count().await.expect("count");
            prop_assert_eq!(observed_count, model.count());

            // try_fold: total payload bytes
            let mut s2 = VecStream::new(rows.clone());
            let observed_bytes: usize = s2.try_fold::<usize, Infallible, _>(0usize, |acc, env| {
                Ok(acc + env.payload().len())
            }).await.expect("fold");
            prop_assert_eq!(observed_bytes, model.total_payload_bytes());

            // try_collect_map: payloads
            let mut s3 = VecStream::new(rows.clone());
            let observed_payloads: Vec<Vec<u8>> = s3
                .try_collect_map::<Vec<u8>, Infallible, _>(|env| Ok(env.payload().to_vec()))
                .await
                .expect("collect");
            prop_assert_eq!(observed_payloads.clone(), model.payloads());

            // try_collect_map: event_types
            let mut s3b = VecStream::new(rows.clone());
            let observed_event_types: Vec<String> = s3b
                .try_collect_map::<String, Infallible, _>(|env| Ok(env.event_type().to_owned()))
                .await
                .expect("collect");
            prop_assert_eq!(observed_event_types, model.event_types());

            // try_collect_map: versions
            let mut s3c = VecStream::new(rows.clone());
            let observed_versions: Vec<u64> = s3c
                .try_collect_map::<u64, Infallible, _>(|env| Ok(env.version().as_u64()))
                .await
                .expect("collect");
            prop_assert_eq!(observed_versions, model.versions());

            // try_for_each: call count
            let calls = Arc::new(Mutex::new(0usize));
            let calls_c = Arc::clone(&calls);
            let mut s4 = VecStream::new(rows.clone());
            s4.try_for_each(move |_env| {
                *calls_c.lock().unwrap() += 1;
                Ok::<(), Infallible>(())
            }).await.expect("for_each");
            prop_assert_eq!(*calls.lock().unwrap(), model.count());

            // Cross-consistency: count == collect.len() == fold-counter == for_each-counter
            prop_assert_eq!(observed_count, observed_payloads.len());
            Ok(())
        })?;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Section 18 — Static Send/Sync assertions
//
// Compile-time checks that the wrapper types propagate Send correctly. These
// are zero-runtime-cost: they either compile or they don't. If `stream.rs`
// ever gains a `*const T` or `Rc` accidentally inside a wrapper, these break.
// ═══════════════════════════════════════════════════════════════════════════

assert_impl_all!(VecStream: Send);
assert_impl_all!(SchemaStream: Send);
assert_impl_all!(MetadataStream: Send);
assert_impl_all!(FailingStream: Send);
assert_impl_all!(U64Codec: Send, Sync);
assert_impl_all!(BytesBorrowingCodec: Send, Sync);
assert_impl_all!(BytesOwningCodec: Send, Sync);
assert_impl_all!(PrependOne: Send, Sync);
assert_impl_all!(RecordingUpcaster: Send, Sync);

// The fully-bound wrapper types must be Send when all their parts are.
assert_impl_all!(DecodedStream<'static, VecStream, U64Codec, (), ()>: Send);
assert_impl_all!(DecodedStream<'static, VecStream, U64Codec, PrependOne, ()>: Send);
assert_impl_all!(
    BorrowedDecodedStream<'static, VecStream, BytesBorrowingCodec, (), ()>: Send
);
assert_impl_all!(
    BorrowedDecodedStream<'static, VecStream, BytesBorrowingCodec, PrependOne, ()>: Send
);

// The builder is Send whenever its parts are — typestate markers carry no
// hidden `!Send`. The `.build()` typestate is enforced by *trait absence*,
// not Send-ness. This is the right model: cross-await is fine before the
// codec is bound; only `.build()` is gated.
assert_impl_all!(
    DecoderBuilder<VecStream, nexus_store::stream::NeedsCodec, nexus_store::stream::NoUpcaster>: Send
);

// Structural Send guarantee: the `EventStream` trait *requires* `next()` to
// return `impl Future<...> + Send` (see `stream.rs` line 41). A `!Send`
// stream therefore cannot implement the trait at all — proved by the
// compile-fail fixture `stream_not_send_cannot_impl.rs`. This is stronger
// than "DecodedStream<!Send, ...>: !Send" because there is no valid input
// that produces a `!Send` wrapper to begin with.

// ═══════════════════════════════════════════════════════════════════════════
// Section 19 — try_fold_async_until (interruptible async fold on DecodedStream)
//
// Mirrors fs2's `interruptWhen` + `compile.fold` contract. Coverage by the
// four mandatory categories from CLAUDE.md:
//   1. Sequence/protocol — order, async per-event side-effects, accumulator
//      threading
//   2. Lifecycle — empty/complete, shutdown-before-any-event,
//      shutdown-after-stream-ends, multi-shot disposition
//   3. Defensive — closure error, stream error, upcast error, codec error,
//      shutdown-already-fired
//   4. Concurrency — concurrent shutdown signal racing the producer
//
// Plus property: try_fold_async_until with `pending::<()>()` shutdown is
// behaviorally equivalent to try_fold (modulo Disposition::Completed).
// ═══════════════════════════════════════════════════════════════════════════

use std::future::pending;
use tokio::sync::oneshot;

// ---------- Sequence / protocol ----------

#[tokio::test]
async fn try_fold_async_until_no_shutdown_returns_completed() {
    let stream = VecStream::new(vec![
        (1, "E".into(), 1u64.to_le_bytes().to_vec()),
        (2, "E".into(), 2u64.to_le_bytes().to_vec()),
        (3, "E".into(), 3u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;
    let (sum, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            0u64,
            |acc, _v, e| async move { Ok::<_, TestErr>(acc + e) },
            pending::<()>(),
        )
        .await
        .unwrap();
    assert_eq!(sum, 6);
    assert_eq!(dispo, Disposition::Completed);
}

#[tokio::test]
async fn try_fold_async_until_threads_versions_in_order() {
    let stream = VecStream::new(vec![
        (1, "E".into(), 10u64.to_le_bytes().to_vec()),
        (2, "E".into(), 20u64.to_le_bytes().to_vec()),
        (3, "E".into(), 30u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;
    let (seen, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            Vec::<(u64, u64)>::new(),
            |mut acc, v, e| async move {
                acc.push((v.as_u64(), e));
                Ok::<_, TestErr>(acc)
            },
            pending::<()>(),
        )
        .await
        .unwrap();
    assert_eq!(seen, vec![(1, 10), (2, 20), (3, 30)]);
    assert_eq!(dispo, Disposition::Completed);
}

#[tokio::test]
async fn try_fold_async_until_awaits_per_event_side_effect() {
    // Per-event closure awaits a tokio sleep — proves async closure bodies
    // actually run to completion before next iteration.
    let stream = VecStream::new(vec![
        (1, "E".into(), 1u64.to_le_bytes().to_vec()),
        (2, "E".into(), 2u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;
    let invocations = Arc::new(AtomicUsize::new(0));
    let inv = Arc::clone(&invocations);
    let (sum, _) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            0u64,
            move |acc, _v, e| {
                let inv = Arc::clone(&inv);
                async move {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    inv.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, TestErr>(acc + e)
                }
            },
            pending::<()>(),
        )
        .await
        .unwrap();
    assert_eq!(sum, 3);
    assert_eq!(invocations.load(Ordering::SeqCst), 2);
}

// ---------- Lifecycle ----------

#[tokio::test]
async fn try_fold_async_until_empty_stream_completes_with_init() {
    let stream = VecStream::new(vec![]);
    let codec = U64Codec;
    let (acc, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            42u64,
            |acc, _v, e| async move { Ok::<_, TestErr>(acc + e) },
            pending::<()>(),
        )
        .await
        .unwrap();
    assert_eq!(acc, 42);
    assert_eq!(dispo, Disposition::Completed);
}

#[tokio::test]
async fn try_fold_async_until_shutdown_already_fired_returns_init() {
    // Pre-fired shutdown: ready future yields immediately. Fold sees no
    // events; returns (init, Interrupted).
    let stream = VecStream::new(vec![
        (1, "E".into(), 1u64.to_le_bytes().to_vec()),
        (2, "E".into(), 2u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;
    let shutdown = std::future::ready(());
    let (acc, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            999u64,
            |acc, _v, e| async move { Ok::<_, TestErr>(acc + e) },
            shutdown,
        )
        .await
        .unwrap();
    assert_eq!(
        acc, 999,
        "accumulator must be init when no events processed"
    );
    assert_eq!(dispo, Disposition::Interrupted);
}

#[tokio::test]
async fn try_fold_async_until_shutdown_after_stream_completes_yields_completed() {
    // If the stream exhausts before shutdown fires, Disposition is Completed,
    // not Interrupted — natural termination wins.
    let stream = VecStream::new(vec![
        (1, "E".into(), 1u64.to_le_bytes().to_vec()),
        (2, "E".into(), 2u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;
    // never-firing shutdown
    let (sum, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            0u64,
            |acc, _v, e| async move { Ok::<_, TestErr>(acc + e) },
            pending::<()>(),
        )
        .await
        .unwrap();
    assert_eq!(sum, 3);
    assert_eq!(dispo, Disposition::Completed);
}

// ---------- Defensive boundary ----------

#[derive(Debug, Error)]
#[error("test error: {0}")]
struct TestErr(&'static str);

impl<
    S: std::error::Error + 'static + Send + Sync,
    C: std::error::Error + 'static + Send + Sync,
    U: std::error::Error + 'static + Send + Sync,
> From<DecodeStreamError<S, C, U>> for TestErr
{
    fn from(_: DecodeStreamError<S, C, U>) -> Self {
        Self("from decode-stream")
    }
}

#[tokio::test]
async fn try_fold_async_until_closure_error_short_circuits_no_disposition() {
    let stream = VecStream::new(vec![
        (1, "E".into(), 1u64.to_le_bytes().to_vec()),
        (2, "E".into(), 2u64.to_le_bytes().to_vec()),
        (3, "E".into(), 3u64.to_le_bytes().to_vec()),
    ]);
    let codec = U64Codec;
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    let result = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            0u64,
            move |acc, _v, e| {
                let calls = Arc::clone(&calls_clone);
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if e == 2 {
                        return Err(TestErr("boom on 2"));
                    }
                    Ok(acc + e)
                }
            },
            pending::<()>(),
        )
        .await;
    assert!(result.is_err(), "closure error must propagate");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "fold must short-circuit on the failing iteration"
    );
}

#[tokio::test]
async fn try_fold_async_until_stream_error_short_circuits_no_disposition() {
    let stream = FailingStream::new(
        vec![
            (1, "E".into(), 1u64.to_le_bytes().to_vec()),
            (2, "E".into(), 2u64.to_le_bytes().to_vec()),
        ],
        1, // fail on second poll
    );
    let codec = U64Codec;
    let result = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            0u64,
            |acc, _v, e| async move { Ok::<_, TestErr>(acc + e) },
            pending::<()>(),
        )
        .await;
    assert!(result.is_err(), "stream error must propagate as Err");
}

// ---------- Concurrency (linearizability under shutdown signal) ----------

/// Stream that yields events with a small per-event async delay, so a
/// concurrent shutdown signal has time to race the producer.
struct SlowStream {
    rows: Vec<(u64, String, Vec<u8>)>,
    pos: usize,
    per_event_delay: std::time::Duration,
}

impl SlowStream {
    fn new(rows: Vec<(u64, String, Vec<u8>)>, delay: std::time::Duration) -> Self {
        Self {
            rows,
            pos: 0,
            per_event_delay: delay,
        }
    }
}

impl BaseEventStream for SlowStream {
    fn to_envelope<'a>(item: PersistedEnvelope<'a>) -> PersistedEnvelope<'a>
    where
        Self: 'a,
    {
        item
    }
}

impl EventStream for SlowStream {
    type Item<'a> = PersistedEnvelope<'a>;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        if self.pos >= self.rows.len() {
            return Ok(None);
        }
        tokio::time::sleep(self.per_event_delay).await;
        let row = &self.rows[self.pos];
        self.pos += 1;
        Ok(Some(PersistedEnvelope::new_unchecked(
            Version::new(row.0).expect("nz"),
            GlobalSeq::INITIAL,
            &row.1,
            1,
            &row.2,
            (),
        )))
    }
}

#[tokio::test]
async fn try_fold_async_until_concurrent_shutdown_preserves_partial_accumulator() {
    // A slow stream emits 100 events at 5ms intervals. After ~30ms, a
    // separate task fires shutdown. The fold should return early with an
    // accumulator strictly less than the full sum, AND with Interrupted.
    //
    // This mirrors fs2 `StreamInterruptSuite` test 11: events being computed
    // when interrupt fires are not in the accumulator.
    let rows: Vec<(u64, String, Vec<u8>)> = (1..=100u64)
        .map(|i| (i, "E".into(), i.to_le_bytes().to_vec()))
        .collect();
    let total: u64 = (1..=100u64).sum();

    let stream = SlowStream::new(rows, std::time::Duration::from_millis(5));
    let codec = U64Codec;
    let (tx, rx) = oneshot::channel::<()>();

    // Fire shutdown from a concurrent task after a small window.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let _ = tx.send(());
    });

    let shutdown = async move {
        let _ = rx.await;
    };

    let (partial, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            0u64,
            |acc, _v, e| async move { Ok::<_, TestErr>(acc + e) },
            shutdown,
        )
        .await
        .unwrap();

    assert_eq!(dispo, Disposition::Interrupted, "shutdown won the race");
    assert!(partial > 0, "at least one event should have been processed");
    assert!(
        partial < total,
        "accumulator must be strictly less than full sum (interrupted mid-stream); got {} vs total {}",
        partial,
        total
    );
}

#[tokio::test]
async fn try_fold_async_until_event_processed_before_shutdown_is_in_accumulator() {
    // Verify the converse of the above: events processed BEFORE shutdown
    // fires ARE in the accumulator. This is the contiguous-prefix guarantee:
    // every event whose closure ran to completion contributes; no event past
    // the interruption point contributes.
    let rows: Vec<(u64, String, Vec<u8>)> = (1..=10u64)
        .map(|i| (i, "E".into(), i.to_le_bytes().to_vec()))
        .collect();
    let stream = SlowStream::new(rows, std::time::Duration::from_millis(10));
    let codec = U64Codec;
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(35)).await;
        let _ = tx.send(());
    });
    let shutdown = async move {
        let _ = rx.await;
    };

    let (seen, dispo) = stream
        .decoder()
        .codec(&codec)
        .build()
        .try_fold_async_until(
            Vec::<u64>::new(),
            |mut acc, _v, e| async move {
                acc.push(e);
                Ok::<_, TestErr>(acc)
            },
            shutdown,
        )
        .await
        .unwrap();

    assert_eq!(dispo, Disposition::Interrupted);
    // Every observed event must be a contiguous prefix of 1..=N.
    for (i, v) in seen.iter().enumerate() {
        assert_eq!(*v as usize, i + 1, "events must be a contiguous prefix");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// try_scan_until — checkpointed scan over Progress/Step
//
// Coverage:
// - empty stream → initial returned, Completed, no save calls
// - all Skip → flush tail saves the last seen position
// - all Save → save fires per event, no extra flush
// - shutdown mid-stream with pending → flush tail fires on Interrupted
// - step error → propagates, no flush
// - save error → propagates, no further work
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Error)]
enum ScanError {
    #[error("stream io")]
    Stream(#[from] Infallible),
    #[error("test step error")]
    Step,
    #[error("test save error")]
    Save,
}

type SaveLog = Arc<Mutex<Vec<(Version, u64)>>>;

fn fresh_log() -> SaveLog {
    Arc::new(Mutex::new(Vec::new()))
}

fn recording_save(
    log: &SaveLog,
) -> impl FnMut(
    Version,
    u64,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u64, ScanError>> + Send>> {
    let log = Arc::clone(log);
    move |pos, state| {
        let log = Arc::clone(&log);
        Box::pin(async move {
            log.lock().unwrap().push((pos, state));
            Ok(state)
        })
    }
}

#[tokio::test]
async fn scan_empty_stream_returns_initial_progress_no_save() {
    let mut s = VecStream::new(vec![]);
    let log = fresh_log();

    let (progress, dispo) = s
        .try_scan_until::<u64, Version, ScanError, _, _, _, _, _>(
            Progress::fresh(0_u64),
            |p, _env| async move { Ok(Step::Skip(p)) },
            recording_save(&log),
            std::future::pending::<()>(),
        )
        .await
        .unwrap();

    assert_eq!(dispo, Disposition::Completed);
    assert_eq!(progress.state, 0);
    assert!(progress.saved.is_none());
    assert!(progress.seen.is_none());
    assert!(log.lock().unwrap().is_empty(), "no events → no saves");
}

#[tokio::test]
async fn scan_all_skip_flushes_tail_on_completion() {
    let mut s = VecStream::new(vec![
        (1, "T".to_string(), vec![]),
        (2, "T".to_string(), vec![]),
        (3, "T".to_string(), vec![]),
    ]);
    let log = fresh_log();

    let (progress, dispo) = s
        .try_scan_until::<u64, Version, ScanError, _, _, _, _, _>(
            Progress::fresh(0_u64),
            |p, env| {
                let version = env.version();
                async move {
                    let next = Progress {
                        state: p.state + 1,
                        saved: p.saved,
                        seen: Some(version),
                    };
                    Ok(Step::Skip(next))
                }
            },
            recording_save(&log),
            std::future::pending::<()>(),
        )
        .await
        .unwrap();

    assert_eq!(dispo, Disposition::Completed);
    assert_eq!(progress.state, 3);
    assert_eq!(progress.saved, Version::new(3));
    assert_eq!(progress.seen, Version::new(3));

    let saves = log.lock().unwrap();
    assert_eq!(saves.len(), 1, "flush tail should save once");
    assert_eq!(saves[0], (Version::new(3).unwrap(), 3));
}

#[tokio::test]
async fn scan_all_save_persists_each_no_extra_flush() {
    let mut s = VecStream::new(vec![
        (1, "T".to_string(), vec![]),
        (2, "T".to_string(), vec![]),
        (3, "T".to_string(), vec![]),
    ]);
    let log = fresh_log();

    let (progress, dispo) = s
        .try_scan_until::<u64, Version, ScanError, _, _, _, _, _>(
            Progress::fresh(0_u64),
            |p, env| {
                let version = env.version();
                async move {
                    let next = Progress {
                        state: p.state + 1,
                        saved: p.saved,
                        seen: Some(version),
                    };
                    Ok(Step::Save(next))
                }
            },
            recording_save(&log),
            std::future::pending::<()>(),
        )
        .await
        .unwrap();

    assert_eq!(dispo, Disposition::Completed);
    assert_eq!(progress.state, 3);
    assert_eq!(progress.saved, Version::new(3));
    assert_eq!(progress.seen, Version::new(3));

    let saves = log.lock().unwrap();
    assert_eq!(saves.len(), 3, "save fires per event, no extra flush");
    assert_eq!(
        saves.as_slice(),
        &[
            (Version::new(1).unwrap(), 1),
            (Version::new(2).unwrap(), 2),
            (Version::new(3).unwrap(), 3),
        ],
    );
}

#[tokio::test]
async fn scan_shutdown_with_pending_flushes_tail_as_interrupted() {
    // Use SlowStream so the loop blocks on `next()` between events. Shutdown
    // is guaranteed to fire mid-loop while waiting for the next event.
    let mut s = SlowStream::new(
        vec![
            (1, "T".to_string(), vec![]),
            (2, "T".to_string(), vec![]),
            (3, "T".to_string(), vec![]),
            (4, "T".to_string(), vec![]),
            (5, "T".to_string(), vec![]),
        ],
        std::time::Duration::from_millis(50),
    );
    let log = fresh_log();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    // Fire shutdown after ~120ms — enough for ~2 events to land before the
    // next `sleep` blocks; shutdown then wins the biased select.
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let _ = tx.send(());
    });

    let (progress, dispo) = s
        .try_scan_until::<u64, Version, ScanError, _, _, _, _, _>(
            Progress::fresh(0_u64),
            |p, env| {
                let version = env.version();
                async move {
                    let next = Progress {
                        state: p.state + 1,
                        saved: p.saved,
                        seen: Some(version),
                    };
                    Ok(Step::Skip(next))
                }
            },
            recording_save(&log),
            async move {
                let _ = rx.await;
            },
        )
        .await
        .unwrap();

    assert_eq!(dispo, Disposition::Interrupted);
    assert!(
        progress.state > 0 && progress.state < 5,
        "expected partial progress, got state = {}",
        progress.state,
    );
    // Flush tail must have aligned saved with seen for the events that did land.
    assert_eq!(progress.saved, progress.seen);

    let saves = log.lock().unwrap();
    assert_eq!(saves.len(), 1, "flush tail saves exactly once on interrupt");
    assert_eq!(saves[0].0, progress.seen.unwrap());
    assert_eq!(saves[0].1, progress.state);
}

#[tokio::test]
async fn scan_step_error_propagates_without_flush() {
    let mut s = VecStream::new(vec![(1, "T".to_string(), vec![])]);
    let log = fresh_log();

    let err = s
        .try_scan_until::<u64, Version, ScanError, _, _, _, _, _>(
            Progress::fresh(0_u64),
            |_p, _env| async move { Err(ScanError::Step) },
            recording_save(&log),
            std::future::pending::<()>(),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, ScanError::Step));
    assert!(
        log.lock().unwrap().is_empty(),
        "step error short-circuits before any save"
    );
}

#[tokio::test]
async fn scan_save_error_propagates() {
    let mut s = VecStream::new(vec![(1, "T".to_string(), vec![])]);

    // save fails on first call.
    let save = |_pos: Version, _state: u64| async move { Err(ScanError::Save) };

    let err = s
        .try_scan_until::<u64, Version, ScanError, _, _, _, _, _>(
            Progress::fresh(0_u64),
            |p, env| {
                let version = env.version();
                async move {
                    let next = Progress {
                        state: p.state + 1,
                        saved: p.saved,
                        seen: Some(version),
                    };
                    Ok(Step::Save(next))
                }
            },
            save,
            std::future::pending::<()>(),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, ScanError::Save));
}
