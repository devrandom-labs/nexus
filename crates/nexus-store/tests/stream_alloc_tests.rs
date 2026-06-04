//! Allocation-counting proof for the inline-decode pipeline that replaced
//! the deleted `BorrowedDecodedStream` facade.
//!
//! ## Why a separate test binary
//!
//! This file installs a `#[global_allocator]` that increments process-wide
//! atomic counters when a "gate" flag is open. The whole point of the
//! borrowing-codec path is zero allocation per event — but in our main
//! test binary 70+ tests run concurrently on multiple threads, each
//! allocating. Their allocations would race the counter and pollute the
//! measurement. By living in its own integration-test binary (each
//! `tests/*.rs` is a separate process), this file owns the counter and the
//! only allocations to count are the ones we're measuring.
//!
//! ## What is proved
//!
//! 1. `try_fold` over 1024 envelopes with a borrowing codec called inside
//!    the closure allocates ≤ 4x the bytes of folding 16 envelopes (so
//!    growth is O(1), not O(n)).
//! 2. An owning `Codec<String>` over the same data allocates ≥ N times
//!    (one String per event). This negative control proves the harness
//!    can detect linear-in-N allocation when present.
//!
//! Together: if a future refactor accidentally calls `.to_vec()` inside the
//! borrowed fold, test (1) flips from passing to failing — turning the
//! zero-copy guarantee from a comment into a checked invariant.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
#![allow(clippy::str_to_string, reason = "tests")]
#![allow(clippy::doc_markdown, reason = "tests")]
#![allow(clippy::uninlined_format_args, reason = "tests")]
#![allow(clippy::arithmetic_side_effects, reason = "tests")]
#![allow(
    clippy::shadow_unrelated,
    clippy::shadow_reuse,
    reason = "test readability: `rows` reshadowed after construction"
)]
#![allow(
    clippy::used_underscore_binding,
    reason = "_force_promotion is deliberately named to signal intent"
)]
#![allow(
    clippy::disallowed_types,
    reason = "tests need std Mutex for global allocator gate"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::convert::Infallible;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::TryStreamExt;
use nexus::Version;
use nexus_store::codec::{Decode, Encode};
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::store::GlobalSeq;
use thiserror::Error;

#[derive(Debug, Error)]
enum FoldErr {
    #[error("stream")]
    Stream(#[from] Infallible),
    #[error("decode")]
    Decode,
}

impl From<Utf8Err> for FoldErr {
    fn from(_: Utf8Err) -> Self {
        Self::Decode
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Counting allocator + gate
// ─────────────────────────────────────────────────────────────────────────────

struct CountingAllocator;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);
static ALLOC_GATE: AtomicUsize = AtomicUsize::new(0);
static ALLOC_LOCK: Mutex<()> = Mutex::new(());

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ALLOC_GATE.load(Ordering::Relaxed) > 0 {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: we are forwarding `layout` directly to `System::alloc`,
        // which has the same safety contract as `GlobalAlloc::alloc`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarding to `System::dealloc` with matching layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

async fn measure_async<R>(fut: impl std::future::Future<Output = R>) -> (R, usize, usize) {
    ALLOC_COUNT.store(0, Ordering::SeqCst);
    ALLOC_BYTES.store(0, Ordering::SeqCst);
    ALLOC_GATE.fetch_add(1, Ordering::SeqCst);
    let r = fut.await;
    ALLOC_GATE.fetch_sub(1, Ordering::SeqCst);
    let count = ALLOC_COUNT.load(Ordering::SeqCst);
    let bytes = ALLOC_BYTES.load(Ordering::SeqCst);
    (r, count, bytes)
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures
// ─────────────────────────────────────────────────────────────────────────────

/// Pre-built row: the `value: Bytes` buffer is allocated ONCE at fixture
/// construction. Cloning a row clones a `Bytes` (one Arc refcount inc,
/// zero memcpy, zero heap allocation), so `next()` allocates nothing
/// per call — preserving the allocation-counting test's premise.
#[derive(Clone)]
struct PrebuiltRow {
    version: u64,
    value: bytes::Bytes,
    event_type_range: std::ops::Range<u32>,
    payload_range: std::ops::Range<u32>,
}

struct VecStream {
    rows: Vec<PrebuiltRow>,
    pos: usize,
}

impl VecStream {
    fn new(rows: Vec<(u64, String, Vec<u8>)>) -> Self {
        let rows = rows
            .into_iter()
            .map(|(v, et, pl)| {
                let mut buf = Vec::with_capacity(et.len() + pl.len());
                buf.extend_from_slice(et.as_bytes());
                buf.extend_from_slice(&pl);
                // `Bytes::from(Vec)` produces a KIND_VEC Bytes that allocates a
                // shared state on its first clone (vtable promotion). Force the
                // promotion now, at fixture construction, by cloning once and
                // dropping the clone. This swaps the original from KIND_VEC to
                // KIND_ARC; per-row `next()` clones during the measured window
                // are then pure atomic refcount increments with zero heap
                // allocation.
                let value = bytes::Bytes::from(buf);
                let _force_promotion = value.clone();
                drop(_force_promotion);
                let et_end = u32::try_from(et.len()).expect("fits u32");
                let pl_end = u32::try_from(et.len() + pl.len()).expect("fits u32");
                PrebuiltRow {
                    version: v,
                    value,
                    event_type_range: 0..et_end,
                    payload_range: et_end..pl_end,
                }
            })
            .collect();
        Self { rows, pos: 0 }
    }
}

impl futures::Stream for VecStream {
    type Item = Result<PersistedEnvelope, Infallible>;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        if self.pos >= self.rows.len() {
            return core::task::Poll::Ready(None);
        }
        let row = self.rows[self.pos].clone();
        self.pos += 1;
        let env = PersistedEnvelope::try_new(
            Version::new(row.version).expect("non-zero"),
            GlobalSeq::INITIAL,
            row.value,
            1,
            row.event_type_range,
            row.payload_range,
            None,
        )
        .expect("test fixture");
        core::task::Poll::Ready(Some(Ok(env)))
    }
}

#[derive(Debug, Error)]
#[error("utf-8 decode failed")]
struct Utf8Err;

/// Owning codec: returns a fresh `String` per event (heap-allocates).
struct StringOwningCodec;

impl Encode<String> for StringOwningCodec {
    type Error = Utf8Err;

    fn encode(&self, value: &String) -> Result<bytes::Bytes, Self::Error> {
        Ok(bytes::Bytes::copy_from_slice(value.as_bytes()))
    }
}

impl Decode<String> for StringOwningCodec {
    type Output<'a> = String;
    type Error = Utf8Err;

    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<String, Self::Error> {
        std::str::from_utf8(env.payload())
            .map(std::borrow::ToOwned::to_owned)
            .map_err(|_| Utf8Err)
    }
}

/// Borrowing codec: validates UTF-8 in-place, returns `&str` (no allocation).
struct StrBorrowingCodec;

impl Encode<str> for StrBorrowingCodec {
    type Error = Utf8Err;

    fn encode(&self, value: &str) -> Result<bytes::Bytes, Self::Error> {
        Ok(bytes::Bytes::copy_from_slice(value.as_bytes()))
    }
}

impl Decode<str> for StrBorrowingCodec {
    type Output<'a> = &'a str;
    type Error = Utf8Err;

    fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<&'a str, Self::Error> {
        std::str::from_utf8(env.payload()).map_err(|_| Utf8Err)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn borrowed_fold_is_constant_regardless_of_stream_length() {
    let _guard = ALLOC_LOCK.lock().expect("alloc lock");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");

    let payload = b"hello".to_vec();
    let codec = StrBorrowingCodec;
    let codec_ref = &codec;

    let run = |n: u64| {
        let rows: Vec<_> = (1..=n).map(|v| (v, "E".into(), payload.clone())).collect();
        let stream = VecStream::new(rows);
        rt.block_on(async {
            let s = stream;
            let fut = s
                .map_err(FoldErr::from)
                .try_fold(0usize, |acc, env| async move {
                    let bytes: &str = codec_ref.decode(&env)?;
                    Ok::<_, FoldErr>(acc + bytes.len())
                });
            let (r, c, b) = measure_async(fut).await;
            (r.expect("fold ok"), c, b)
        })
    };

    // Warm caches and lazy statics.
    let _ = run(4);
    let (_, c_small, b_small) = run(16);
    let (_, c_large, b_large) = run(1024);

    assert!(
        b_large <= b_small.saturating_mul(4) + 4_096,
        "borrowed fold allocations look linear in N: small={}B (n=16), large={}B (n=1024)",
        b_small,
        b_large,
    );
    assert!(
        c_large <= c_small.saturating_mul(4) + 32,
        "borrowed fold alloc count looks linear in N: small={} (n=16), large={} (n=1024)",
        c_small,
        c_large,
    );
}

#[test]
fn owning_string_codec_scales_linearly_with_stream_length() {
    // Negative control: if the harness can't detect O(n) growth in a codec
    // that is *known* to allocate per event, then the borrowed-fold test
    // above could pass for the wrong reason.
    let _guard = ALLOC_LOCK.lock().expect("alloc lock");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");

    let payload = b"some text payload longer than sso".to_vec();
    let codec = StringOwningCodec;
    let codec_ref = &codec;

    let run = |n: u64| {
        let rows: Vec<_> = (1..=n).map(|v| (v, "E".into(), payload.clone())).collect();
        let stream = VecStream::new(rows);
        rt.block_on(async {
            let s = stream;
            let fut = s
                .map_err(FoldErr::from)
                .try_fold(0usize, |acc, env| async move {
                    let decoded: String = codec_ref.decode(&env)?;
                    Ok::<_, FoldErr>(acc + decoded.len())
                });
            let (r, c, b) = measure_async(fut).await;
            (r.expect("fold ok"), c, b)
        })
    };

    let _ = run(4);
    let (_, c_small, b_small) = run(16);
    let (_, c_large, b_large) = run(1024);

    assert!(
        c_large >= c_small + 900,
        "owning codec did not scale linearly: small={} (n=16), large={} (n=1024)",
        c_small,
        c_large,
    );
    assert!(
        b_large >= b_small + 900 * payload.len(),
        "owning codec bytes did not scale: small={}B (n=16), large={}B (n=1024)",
        b_small,
        b_large,
    );
}
