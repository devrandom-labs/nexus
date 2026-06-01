# Bytes Envelope PR2 — Stream Collapse + Codec Collapse + Wire-Format Alignment Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Companion docs:** [`design.md`](./2026-05-27-bytes-envelope-design.md), [`deviations.md`](./2026-05-27-bytes-envelope-deviations.md). Read both before starting — the deviation log's `[PR 2 | scoping]` entry (with Resolved subsections for A/B/C) is the authoritative scope and design-decision record for PR2.

**Goal:** Collapse the stream trait family to a `futures::Stream` marker, collapse `Decode` + `BorrowingDecode` to one `Decode<E>` with a GAT, change `Encode::encode` to return `Bytes`, restore payload alignment as a 16-byte wire-format invariant, and add `BytemuckCodec` + `RkyvCodec` feature-gated codecs. Drops the `M = ()` generic from every store/stream/subscription trait.

**Architecture:** Pre-1.0 breaking change. The owned-Bytes envelope from PR1 makes the per-record GAT lifetime cliff disappear, which is what enables (a) collapsing `Decode` + `BorrowingDecode` and (b) replacing the GAT-lending `EventStream<M>` trait with a `futures::Stream<Item = Result<PersistedEnvelope, _>>` marker. The wire-format alignment guarantee is added at the row-builder layer in a new `nexus-store::wire` module, called by both `nexus-fjall` and `InMemoryStore`. New codecs (`BytemuckCodec`, `RkyvCodec`) ship as feature flags on `nexus-store` with pinned upstream versions (`bytemuck = "1"`, `rkyv = "0.8"`).

**Tech Stack:** Rust 2024, `bytes = "1"`, `futures = "0.3"` (already in workspace), new: `aligned-vec = "=0.6.4"`, `bytemuck = "1"` (optional), `rkyv = "0.8"` (optional).

**Branch:** `refactor/bytes-envelope-pr2` (branch from `main` after PR1 merges; if PR1 is still open, branch from `refactor/bytes-envelope-pr1`).

---

## File Structure

**Workspace + crate manifests:**
- Modify: `Cargo.toml` — add `aligned-vec = "=0.6.4"`, `bytemuck = "1"`, `rkyv = "0.8"` to `[workspace.dependencies]`.
- Modify: `crates/nexus-store/Cargo.toml` — add `aligned-vec` (required), `bytemuck` (optional), `rkyv` (optional); add `bytemuck` and `rkyv` feature flags; promote `futures-core` to required (drop `futures-bridge` feature flag — futures becomes load-bearing).

**Wire format (new module, shared row builder):**
- Create: `crates/nexus-store/src/wire.rs` — `pub(crate) fn build_row(event_type: &str, metadata: Option<&[u8]>, payload: &[u8]) -> Result<RowBuilt, WireError>` returning `(Bytes, RowOffsets)`. Padding to 16-byte payload alignment. Backed by `AVec<u8, ConstAlign<16>>` + `Bytes::from_owner`.
- Modify: `crates/nexus-store/src/lib.rs` — declare `mod wire`; re-export nothing (internal).

**Codec collapse:**
- Modify: `crates/nexus-store/src/codec.rs` — delete `BorrowingDecode`; new `Decode<E>` trait with `type Output<'a>` GAT and `fn decode<'a>(&self, env: &'a PersistedEnvelope) -> Result<Self::Output<'a>, Self::Error>`. Change `Encode::encode` to return `Bytes`. Add `pub mod bytemuck` block (feature-gated). Add `pub mod rkyv` block (feature-gated). Update `SerdeCodec`'s `Decode` impl to the new signature/GAT. Update `SerdeCodec`'s `Encode` impl to return `Bytes`.

**Stream trait collapse:**
- Delete: `crates/nexus-store/src/stream/cursor.rs` (~600 lines: `EventStream<M>` GAT, `BaseEventStream`, `EventStreamExt`, `Disposition`, shutdown-bias select helper).
- Delete: `crates/nexus-store/src/stream/combinators.rs` (~227 lines: `Map`, `TryMap`, `MapErr`, `TryScan`).
- Delete: `crates/nexus-store/src/stream/progress.rs` (~192 lines: `Progress`, `Step`).
- Delete: `crates/nexus-store/src/stream/owned.rs` (~307 lines: `OwnedEventStream`, `IntoStream`).
- Replace: `crates/nexus-store/src/stream/mod.rs` becomes a thin file declaring the new marker trait — or collapse the directory to `crates/nexus-store/src/stream.rs` (flat, single file).
- The new module exposes one trait:
  ```rust
  pub trait EventStream:
      futures::Stream<Item = Result<PersistedEnvelope, <Self as EventStream>::Error>> + Send
  {
      type Error: std::error::Error + Send + Sync + 'static;
  }
  ```
  plus a blanket impl for any matching `futures::Stream`. Combinators come from `futures::StreamExt`.

**Store / Subscription trait `M` drop:**
- Modify: `crates/nexus-store/src/store.rs` — drop `<M = ()>` from `RawEventStore`, `Subscription`, `SubscriptionBackend`, `SharedSubscription`, `SharedSubscriptionBackend` (whichever currently exist). Replace `RawEventStore::Stream<'a>` GAT with concrete `type Stream: EventStream + Send + 'static`. Drop the `where Self: 'a` bound. Update the blanket impl `impl<T> Subscription for Arc<T> where T: SubscriptionBackend`.

**Adapter: fjall:**
- Modify: `crates/nexus-fjall/src/store.rs` — drop `<M>` on impls. Switch `read_stream` to return an owned futures-Stream-based `FjallStream` (not a GAT lending cursor). `append` calls `nexus_store::wire::build_row` and writes the resulting Bytes to the fjall keyspace.
- Modify: `crates/nexus-fjall/src/encoding.rs` — keep `decode_event_value` (reads back stored row; needed for the read path which doesn't go through wire::build_row). Remove the encoder if it's now redundant (all encoding flows through `nexus_store::wire`). Update header layout to include the alignment padding bytes (these are zero-bytes between metadata and payload; the decoder skips them using the same offset arithmetic the encoder uses).
- Modify: `crates/nexus-fjall/src/stream.rs` — rewrite as `impl futures::Stream` (poll_next-based) rather than GAT lending. Will need to buffer one row ahead in the struct for `Poll::Pending` continuation.
- Modify: `crates/nexus-fjall/src/subscription_stream.rs` — same rewrite: `impl futures::Stream`.

**Adapter: InMemoryStore:**
- Modify: `crates/nexus-store/src/testing.rs` — `StoredRow` keeps its `(Bytes, RowOffsets)` shape but now built via `nexus_store::wire::build_row`. Cursor types implement `futures::Stream` directly (not the old GAT trait).

**Repository facade:**
- Modify: `crates/nexus-store/src/repository.rs` — `EventStore::load` consumes the new `EventStream` (futures-based) via `futures::StreamExt::try_fold`. `EventStore::save` passes `Bytes` from `Encode::encode` directly into `PendingEnvelope`. `ZeroCopyEventStore::load` uses `Decode::Output<'a>` GAT.
- Modify: `crates/nexus-store/src/envelope.rs` — `PendingEnvelope` builder's `.payload(...)` accepts `Bytes` (not `Vec<u8>`).

**Snapshot decorator:**
- Modify: `crates/nexus-store/src/snapshot.rs` — adjust to new `EventStream` shape (likely a one-liner: `try_fold` instead of the GAT loop).

**Framework: projection runner:**
- Modify: `crates/nexus-framework/src/projection/projection.rs` — switch the event loop from the GAT-trait `next()` to `futures::StreamExt::try_fold` (or `next().await` in a loop on a `Pin<&mut impl Stream>`).

**Tests:**
- Add: `crates/nexus-store/src/wire.rs` `#[cfg(test)] mod tests` — proptest: for any `(event_type: 0..256 bytes, metadata: Option<0..1024 bytes>, payload: 0..4096 bytes)`, `build_row` returns a `Bytes` whose payload pointer is 16-aligned; the cached offsets correctly recover each field.
- Add: `crates/nexus-store/tests/wire_alignment_tests.rs` — defensive boundary: max-length event_type, empty payload, max-length metadata, zero-length metadata. All four cross-cutting categories applied to the wire layer.
- Modify: `crates/nexus-store/tests/zero_copy_event_store_tests.rs` — remove `#[ignore]` from `zero_copy_save_and_load_roundtrip` and `zero_copy_multi_save_load`. They now PASS because alignment is restored.
- Modify: `crates/nexus-store/tests/stream_tests.rs` — replace any GAT-trait usage with `futures::StreamExt` calls.
- Modify: `crates/nexus-fjall/tests/*.rs` — any tests using `EventStreamExt::map_err` / `.try_fold` etc. switch to `futures::StreamExt`.
- Add: `crates/nexus-store/src/codec.rs` `#[cfg(all(test, feature = "bytemuck"))]` — `BytemuckCodec` round-trip test, asserts decoded `&E` pointer equality with envelope payload pointer (proves zero-copy).
- Add: `crates/nexus-store/src/codec.rs` `#[cfg(all(test, feature = "rkyv"))]` — `RkyvCodec` round-trip test.

**Examples:**
- Modify: `examples/store-and-kernel/src/main.rs` — drop any `EventStreamExt` imports; use `futures::StreamExt`.
- Modify: `examples/store-inmemory/src/main.rs` — same.

---

### Task 1: Branch + add workspace deps + nexus-store features

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/nexus-store/Cargo.toml`

- [ ] **Step 1: Create branch**

```bash
git fetch origin
git checkout -b refactor/bytes-envelope-pr2 origin/main
# If PR1 is still open and not merged:
# git checkout -b refactor/bytes-envelope-pr2 refactor/bytes-envelope-pr1
```

- [ ] **Step 2: Add workspace deps**

In `Cargo.toml` under `[workspace.dependencies]`, add:

```toml
aligned-vec = "=0.6.4"
bytemuck = "1"
rkyv = "0.8"
```

- [ ] **Step 3: Add nexus-store deps + features**

Run from repo root:

```bash
nix develop -c cargo add -p nexus-store aligned-vec
nix develop -c cargo add -p nexus-store bytemuck --optional
nix develop -c cargo add -p nexus-store rkyv --optional
```

Edit `crates/nexus-store/Cargo.toml` `[features]` block to add:

```toml
bytemuck = ["dep:bytemuck"]
rkyv = ["dep:rkyv"]
```

Remove the `futures-bridge` feature line (futures is now load-bearing, not optional). In the `[dependencies]` block change `futures-core = { workspace = true, optional = true }` to `futures-core = { workspace = true }`. Add `futures = { workspace = true }` if it isn't already in `[dependencies]` (need `StreamExt`).

In `[dev-dependencies]`, drop `features = ["testing", "futures-bridge"]` → `features = ["testing"]` on the `nexus-store` self-dep.

- [ ] **Step 4: Verify the workspace still builds**

```bash
nix develop -c cargo check --workspace --all-features
```

Expected: PASS — no code changes yet.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/nexus-store/Cargo.toml
git commit -m "$(cat <<'EOF'
chore(deps): add aligned-vec, bytemuck, rkyv for PR2 of bytes-envelope refactor

Foundation for the codec/stream collapse:
- aligned-vec = "=0.6.4" pinned (0.x, no semver between minors)
- bytemuck = "1" optional, gated by feature `bytemuck`
- rkyv = "0.8" optional, gated by feature `rkyv`
- futures-core promoted from optional `futures-bridge` to required

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Create `nexus-store::wire` module with aligned row builder

**Files:**
- Create: `crates/nexus-store/src/wire.rs`
- Modify: `crates/nexus-store/src/lib.rs` — `mod wire;`

- [ ] **Step 1: Write the failing test for alignment invariant**

Create `crates/nexus-store/src/wire.rs` with:

```rust
//! Wire-format row builder shared by `nexus-fjall` and `nexus-store::testing`.
//!
//! One canonical implementation of the on-disk row format: header bytes
//! followed by alignment padding followed by payload. The padding makes
//! the payload pointer 16-byte aligned in the resulting [`Bytes`] buffer,
//! which is a wire-format invariant — every adapter must use this
//! function to encode rows, every decoder may rely on the alignment.

use aligned_vec::{AVec, ConstAlign};
use bytes::Bytes;
use core::ops::Range;
use thiserror::Error;

/// Payload alignment in bytes. Wire-format invariant.
pub(crate) const PAYLOAD_ALIGN: usize = 16;

/// Maximum event-type length (matches `nexus-fjall` encoding).
const MAX_EVENT_TYPE_LEN: usize = 65_535;

/// Maximum metadata length.
const MAX_METADATA_LEN: usize = u32::MAX as usize;

/// Maximum payload length.
const MAX_PAYLOAD_LEN: usize = u32::MAX as usize;

/// Output of [`build_row`]: the assembled buffer plus byte ranges into it.
#[derive(Debug)]
pub(crate) struct RowBuilt {
    pub value: Bytes,
    pub offsets: RowOffsets,
}

/// Byte ranges for each field within a [`RowBuilt::value`] buffer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowOffsets {
    pub event_type: Range<u32>,
    pub metadata: Option<Range<u32>>,
    pub payload: Range<u32>,
}

/// Errors from [`build_row`].
#[derive(Debug, Error)]
pub(crate) enum WireError {
    #[error("event type length {actual} exceeds maximum {max}")]
    EventTypeTooLong { actual: usize, max: usize },
    #[error("metadata length {actual} exceeds maximum {max}")]
    MetadataTooLong { actual: usize, max: usize },
    #[error("payload length {actual} exceeds maximum {max}")]
    PayloadTooLong { actual: usize, max: usize },
}

/// Build one row buffer with payload aligned to [`PAYLOAD_ALIGN`].
///
/// Layout: `[event_type][meta_len: u32 LE][metadata?][padding][payload]`.
/// `meta_len == u32::MAX` is the absent-metadata sentinel.
///
/// # Errors
///
/// Returns [`WireError`] if any field exceeds its maximum length.
pub(crate) fn build_row(
    event_type: &str,
    metadata: Option<&[u8]>,
    payload: &[u8],
) -> Result<RowBuilt, WireError> {
    let event_type_bytes = event_type.as_bytes();
    let event_type_len = event_type_bytes.len();
    if event_type_len > MAX_EVENT_TYPE_LEN {
        return Err(WireError::EventTypeTooLong {
            actual: event_type_len,
            max: MAX_EVENT_TYPE_LEN,
        });
    }

    let meta_bytes_len = metadata.map_or(0, <[u8]>::len);
    if meta_bytes_len > MAX_METADATA_LEN {
        return Err(WireError::MetadataTooLong {
            actual: meta_bytes_len,
            max: MAX_METADATA_LEN,
        });
    }

    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(WireError::PayloadTooLong {
            actual: payload.len(),
            max: MAX_PAYLOAD_LEN,
        });
    }

    // Header layout: [event_type][meta_len: 4B][metadata?]
    let header_len = event_type_len
        .checked_add(4)
        .and_then(|n| n.checked_add(meta_bytes_len))
        .expect("header length cannot overflow usize on practical inputs");

    // Padding so payload begins at next 16-byte boundary inside the buffer.
    let padding = (PAYLOAD_ALIGN - (header_len % PAYLOAD_ALIGN)) % PAYLOAD_ALIGN;

    let total = header_len
        .checked_add(padding)
        .and_then(|n| n.checked_add(payload.len()))
        .expect("total length cannot overflow usize on practical inputs");

    let mut buf: AVec<u8, ConstAlign<PAYLOAD_ALIGN>> =
        AVec::with_capacity(PAYLOAD_ALIGN, total);

    buf.extend_from_slice(event_type_bytes);
    let meta_len_u32 = match metadata {
        Some(_) => u32::try_from(meta_bytes_len).expect("checked above"),
        None => u32::MAX,
    };
    buf.extend_from_slice(&meta_len_u32.to_le_bytes());
    if let Some(m) = metadata {
        buf.extend_from_slice(m);
    }
    buf.resize(buf.len() + padding, 0u8);
    buf.extend_from_slice(payload);

    let event_type_end = u32::try_from(event_type_len).expect("checked above");
    let metadata_range = metadata.map(|_| {
        let start = event_type_end + 4;
        let end = start + u32::try_from(meta_bytes_len).expect("checked above");
        start..end
    });
    let payload_start =
        u32::try_from(header_len + padding).expect("bounded by checked field maxima");
    let payload_end =
        payload_start + u32::try_from(payload.len()).expect("checked above");

    Ok(RowBuilt {
        value: Bytes::from_owner(buf),
        offsets: RowOffsets {
            event_type: 0..event_type_end,
            metadata: metadata_range,
            payload: payload_start..payload_end,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn payload_ptr_aligned(row: &RowBuilt) -> bool {
        let payload_slice = &row.value[row.offsets.payload.start as usize
            ..row.offsets.payload.end as usize];
        (payload_slice.as_ptr() as usize) % PAYLOAD_ALIGN == 0
    }

    proptest! {
        #[test]
        fn payload_pointer_is_16_aligned(
            event_type in "[a-zA-Z]{0,256}",
            metadata in prop::option::of(prop::collection::vec(any::<u8>(), 0..1024)),
            payload in prop::collection::vec(any::<u8>(), 0..4096),
        ) {
            let meta_ref = metadata.as_deref();
            let row = build_row(&event_type, meta_ref, &payload).unwrap();
            prop_assert!(payload_ptr_aligned(&row));
        }

        #[test]
        fn ranges_recover_each_field(
            event_type in "[a-zA-Z]{1,128}",
            metadata in prop::option::of(prop::collection::vec(any::<u8>(), 0..256)),
            payload in prop::collection::vec(any::<u8>(), 1..1024),
        ) {
            let meta_ref = metadata.as_deref();
            let row = build_row(&event_type, meta_ref, &payload).unwrap();
            let v = &row.value;
            prop_assert_eq!(
                &v[row.offsets.event_type.start as usize..row.offsets.event_type.end as usize],
                event_type.as_bytes()
            );
            prop_assert_eq!(
                &v[row.offsets.payload.start as usize..row.offsets.payload.end as usize],
                payload.as_slice()
            );
            if let (Some(meta), Some(range)) = (meta_ref, row.offsets.metadata) {
                prop_assert_eq!(
                    &v[range.start as usize..range.end as usize],
                    meta
                );
            }
        }
    }

    #[test]
    fn empty_payload_still_aligned() {
        let row = build_row("X", None, &[]).unwrap();
        assert!(payload_ptr_aligned(&row));
        assert_eq!(row.offsets.payload.start, row.offsets.payload.end);
    }

    #[test]
    fn empty_event_type_rejected_no_we_allow_it() {
        // Empty event_type is permitted by build_row; validation lives in PendingEnvelope.
        let row = build_row("", None, b"data").unwrap();
        assert!(payload_ptr_aligned(&row));
    }

    #[test]
    fn max_event_type_accepted() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN);
        build_row(&huge, None, b"d").unwrap();
    }

    #[test]
    fn over_max_event_type_rejected() {
        let huge = "a".repeat(MAX_EVENT_TYPE_LEN + 1);
        assert!(matches!(
            build_row(&huge, None, b"d"),
            Err(WireError::EventTypeTooLong { .. })
        ));
    }
}
```

- [ ] **Step 2: Add `mod wire;` to `crates/nexus-store/src/lib.rs`**

Edit `crates/nexus-store/src/lib.rs`. Find the existing list of `mod` declarations and add `mod wire;` alongside them (alphabetical order is fine).

- [ ] **Step 3: Run the tests, verify they pass**

```bash
nix develop -c cargo nextest run -p nexus-store wire::tests
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/wire.rs crates/nexus-store/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(store): add wire::build_row with 16-byte payload alignment

New nexus-store::wire module is the single source of truth for the
on-disk row layout. Payload offset is padded to a 16-byte boundary
inside an AVec-backed Bytes buffer — a wire-format invariant honored
by every adapter.

Proptest verifies payload pointer alignment across arbitrary inputs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Migrate `nexus-fjall::store` and `nexus-store::testing` to use `wire::build_row`

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs`
- Modify: `crates/nexus-fjall/src/encoding.rs`
- Modify: `crates/nexus-store/src/testing.rs`

- [ ] **Step 1: Migrate `nexus-fjall` to call `wire::build_row`**

In `crates/nexus-fjall/src/store.rs`, find the `FjallStore::append` method. Inside the per-envelope loop where `encode_event_value` is currently called, replace the call with:

```rust
let row = nexus_store::wire::build_row(
    env.event_type(),
    env.metadata(),
    env.payload(),
).map_err(FjallError::from)?;
```

Add a `From<nexus_store::wire::WireError> for FjallError` impl (or its appropriate variant) in `crates/nexus-fjall/src/error.rs`. Re-use the existing input-validation variants where applicable; add `WireBuild` if there is no good fit.

Update the fjall keyspace write to insert `row.value` (the full Bytes including header + padding + payload) as the value, with the existing key encoding unchanged.

In `crates/nexus-fjall/src/encoding.rs`, **delete** the `encode_event_value` function (no longer used). **Keep** `decode_event_value` but update it to handle the new layout: after reading `meta_len` and skipping `metadata`, the decoder must compute the same `padding = (header_len % 16 == 0) ? 0 : 16 - (header_len % 16)` and skip it before reading the payload. Add a `decoded.payload_offset` field or equivalent so the cursor can produce a `Range<u32>` into the stored Bytes for the payload start.

- [ ] **Step 2: Migrate `InMemoryStore` to call `wire::build_row`**

In `crates/nexus-store/src/testing.rs`, find `InMemoryStore::append` and the `StoredRow` construction. Replace the direct `Bytes::from(Vec<u8>)` construction with:

```rust
let row = crate::wire::build_row(
    env.event_type(),
    env.metadata(),
    env.payload(),
).map_err(InMemoryStoreError::from)?;
let stored = StoredRow {
    value: row.value,
    offsets: row.offsets,
    version,
    global_seq,
};
```

`StoredRow`'s `offsets` field now stores the `wire::RowOffsets` type rather than ad-hoc per-field `Range<u32>` fields. Update `StoredRow` accordingly. Add a `From<wire::WireError> for InMemoryStoreError` impl.

The cursor's `next()` yields `PersistedEnvelope::try_new(row.value.clone(), row.offsets, ...)` — adapt `PersistedEnvelope::try_new` if its signature doesn't take a `RowOffsets`.

- [ ] **Step 3: Adapt `PersistedEnvelope::try_new` if needed**

If `PersistedEnvelope::try_new` currently takes separate `event_type_range`, `payload_range`, `metadata_range` arguments, change it to accept a single `wire::RowOffsets` (re-exported as `pub(crate)` from `wire`). If you prefer to keep the existing signature, write a small `From<RowOffsets> for (Range<u32>, Range<u32>, Option<Range<u32>>)` shim. Either is fine — pick whichever requires fewer call-site changes.

- [ ] **Step 4: Verify**

```bash
nix develop -c cargo check --workspace --all-features
nix develop -c cargo nextest run -p nexus-store
nix develop -c cargo nextest run -p nexus-fjall
```

Expected: PASS.

- [ ] **Step 5: Re-enable the two `#[ignore]`'d zero-copy tests**

In `crates/nexus-store/tests/zero_copy_event_store_tests.rs`, remove `#[ignore]` from both `zero_copy_save_and_load_roundtrip` and `zero_copy_multi_save_load`.

```bash
nix develop -c cargo nextest run -p nexus-store --test zero_copy_event_store_tests
```

Expected: PASS — alignment is restored, the borrowing decoder no longer hits UB.

- [ ] **Step 6: Commit**

```bash
git add crates/nexus-fjall/src/store.rs crates/nexus-fjall/src/encoding.rs crates/nexus-fjall/src/error.rs crates/nexus-store/src/testing.rs crates/nexus-store/src/envelope.rs crates/nexus-store/tests/zero_copy_event_store_tests.rs
git commit -m "$(cat <<'EOF'
refactor(store, fjall)!: route all row construction through nexus-store::wire

FjallStore::append and InMemoryStore::append now call wire::build_row
to construct the on-disk row buffer. Payload alignment is the
wire-format invariant; both adapters honor it via a single helper.

Re-enables the two zero-copy tests ignored in PR1 (#182). rkyv,
flatbuffers, and repr(C) POD borrowing decoders are unblocked.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Change `Encode<E>::encode` return type to `Bytes`

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`

- [ ] **Step 1: Change the trait signature**

In `crates/nexus-store/src/codec.rs`, find the `Encode<E>` trait. Change the return type:

```rust
pub trait Encode<E: ?Sized>: Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Serialize a typed value to bytes.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the value cannot be serialized.
    fn encode(&self, event: &E) -> Result<bytes::Bytes, Self::Error>;
}
```

- [ ] **Step 2: Update `SerdeCodec`'s `Encode` impl**

```rust
impl<E, F> Encode<E> for SerdeCodec<F>
where
    E: Serialize + Send + Sync + 'static,
    F: SerdeFormat,
{
    type Error = F::Error;

    fn encode(&self, event: &E) -> Result<bytes::Bytes, Self::Error> {
        self.format.serialize(event).map(bytes::Bytes::from)
    }
}
```

- [ ] **Step 3: Verify the workspace still compiles**

```bash
nix develop -c cargo check --workspace --all-features
```

Expected: PASS — `Vec<u8> → Bytes::from(vec)` is the only mechanical adaptation needed. Repository facade likely already builds a `Bytes` value to hand to `PendingEnvelope`; the conversion shim it currently has can be deleted in the next task.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/codec.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: Encode::encode returns Bytes instead of Vec<u8>

End-to-end Bytes flow from codec to wire. SerdeCodec adapts with a
single Bytes::from on its serializer output (zero-copy ownership
transfer of the Vec backing).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Collapse `Decode` + `BorrowingDecode` → `Decode<E>` with `Output<'a>` GAT

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`

- [ ] **Step 1: Replace the two traits with one**

In `crates/nexus-store/src/codec.rs`, delete both the `Decode<E>` and `BorrowingDecode<E>` trait blocks. Replace with:

```rust
use crate::envelope::PersistedEnvelope;

/// Decode a persisted envelope to a typed value (or borrow into it).
///
/// One trait covers both owning and borrowing codecs via the
/// `Output<'a>` GAT:
/// - Owning codecs (serde/json/bincode): `type Output<'a> = E`.
/// - Borrowing codecs (rkyv):            `type Output<'a> = &'a Archived<E>`.
/// - Plain-old-data codecs (bytemuck):   `type Output<'a> = &'a E`.
///
/// The envelope is the input: its `event_type()` is the variant
/// discriminant, `payload()` is the serialized bytes. The codec
/// reaches into the envelope itself rather than receiving two
/// pre-extracted arguments.
pub trait Decode<E: ?Sized>: Send + Sync + 'static {
    /// What `decode` returns. `E` for owning codecs, `&'a E` (or
    /// `&'a Archived<E>`) for zero-copy codecs.
    type Output<'a>
    where
        Self: 'a;

    /// The error type for deserialization failures.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Decode the envelope's payload to `Output<'a>`.
    ///
    /// # Errors
    ///
    /// Returns `Self::Error` if the payload is invalid or does not
    /// match the type discriminant in `env.event_type()`.
    fn decode<'a>(
        &'a self,
        env: &'a PersistedEnvelope,
    ) -> Result<Self::Output<'a>, Self::Error>;
}
```

- [ ] **Step 2: Update `SerdeCodec`'s `Decode` impl**

```rust
impl<E, F> Decode<E> for SerdeCodec<F>
where
    E: DeserializeOwned + Send + Sync + 'static,
    F: SerdeFormat,
{
    type Output<'a> = E where Self: 'a;
    type Error = F::Error;

    fn decode<'a>(
        &'a self,
        env: &'a PersistedEnvelope,
    ) -> Result<E, Self::Error> {
        self.format.deserialize(env.payload())
    }
}
```

- [ ] **Step 3: Update consumer call sites**

Find every `BorrowingDecode` usage:

```bash
nix develop -c cargo check --workspace --all-features 2>&1 | tee /tmp/pr2-decode-errors.log
```

Expected sites to update:
- `crates/nexus-store/src/repository.rs` — `ZeroCopyEventStore::load` was using `BorrowingDecode`. It now uses `Decode` with the GAT.
- Any test that wrote `impl Decode for X { fn decode(&self, name: &str, payload: &[u8]) -> ... }`. Update to the new signature.

For the `name: &str` argument that callers used to receive: in the new signature it's `env.event_type()`. Mechanical replacement.

- [ ] **Step 4: Run tests**

```bash
nix develop -c cargo nextest run --workspace --all-features
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-store/src/codec.rs crates/nexus-store/src/repository.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: collapse Decode + BorrowingDecode into one Decode<E>

The two-trait split was a workaround for the borrowed-cursor lifetime
cliff. PR1's owned-Bytes envelope removes the cliff: the same
operation no longer needs two trait shapes.

One trait, GAT-parameterized output:
  type Output<'a>;
  fn decode<'a>(&'a self, env: &'a PersistedEnvelope) -> Result<...>;

Serde codecs: Output<'a> = E.
Rkyv:         Output<'a> = &'a Archived<E>.
Bytemuck:     Output<'a> = &'a E.

decode() takes the whole PersistedEnvelope rather than two extracted
arguments — the envelope already carries every field a codec needs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Replace the GAT-lending `EventStream` family with a `futures::Stream` marker

**Files:**
- Delete: `crates/nexus-store/src/stream/cursor.rs`
- Delete: `crates/nexus-store/src/stream/combinators.rs`
- Delete: `crates/nexus-store/src/stream/progress.rs`
- Delete: `crates/nexus-store/src/stream/owned.rs`
- Replace: `crates/nexus-store/src/stream/mod.rs` (or flatten to `stream.rs`)

- [ ] **Step 1: Flatten the stream directory to a single file**

The kernel/store convention is one file per concept (flat layout, no subdirectories — see CLAUDE.md). The stream subdirectory only existed for the GAT trait + combinators + progress + futures-bridge split. With the futures::Stream marker, everything collapses to ~30 lines. Flatten now.

```bash
git rm crates/nexus-store/src/stream/cursor.rs
git rm crates/nexus-store/src/stream/combinators.rs
git rm crates/nexus-store/src/stream/progress.rs
git rm crates/nexus-store/src/stream/owned.rs
git rm crates/nexus-store/src/stream/mod.rs
```

Create `crates/nexus-store/src/stream.rs`:

```rust
//! Marker trait identifying a futures::Stream of decoded
//! [`PersistedEnvelope`]s. Combinators come from
//! [`futures::StreamExt`](https://docs.rs/futures/latest/futures/prelude/trait.StreamExt.html).
//!
//! This module replaces the previous GAT-lending `EventStream`
//! trait family. PR1's owned-Bytes envelope removed the lifetime
//! cliff that motivated the GAT; the new shape is a marker over
//! `futures::Stream<Item = Result<PersistedEnvelope, _>>`.

use crate::envelope::PersistedEnvelope;

/// A futures-stream of persisted envelopes.
///
/// Marker trait: it requires the underlying type to be a
/// `futures::Stream` whose item is a `Result<PersistedEnvelope, _>`,
/// and exposes the error type as an associated type so call sites
/// can bound on `S: EventStream<Error = MyErr>`.
///
/// Auto-implemented for every matching `futures::Stream`.
pub trait EventStream:
    futures::Stream<Item = Result<PersistedEnvelope, <Self as EventStream>::Error>>
    + Send
{
    type Error: std::error::Error + Send + Sync + 'static;
}

impl<S, E> EventStream for S
where
    S: futures::Stream<Item = Result<PersistedEnvelope, E>> + Send,
    E: std::error::Error + Send + Sync + 'static,
{
    type Error = E;
}
```

- [ ] **Step 2: Update `lib.rs` re-exports**

In `crates/nexus-store/src/lib.rs`:
- Find the existing re-exports for `Map`, `MapErr`, `TryMap`, `TryScan`, `BaseEventStream`, `Disposition`, `EventStream`, `EventStreamExt`, `OwnedEventStream`, `IntoStream`, `Progress`, `Step` — delete all of them except `EventStream`.
- Keep `pub use stream::EventStream;`.

- [ ] **Step 3: Verify it compiles in isolation**

```bash
nix develop -c cargo check -p nexus-store --no-default-features
```

Expected: errors at every other call site that referenced the deleted types. That's the task 7 surface.

- [ ] **Step 4: Commit (stream module alone)**

```bash
git add crates/nexus-store/src/stream.rs crates/nexus-store/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: collapse EventStream trait family to a futures::Stream marker

Deletes ~1370 lines:
- cursor.rs (EventStream GAT, BaseEventStream, EventStreamExt, Disposition)
- combinators.rs (Map, TryMap, MapErr, TryScan)
- progress.rs (Progress, Step)
- owned.rs (OwnedEventStream, IntoStream futures bridge)

Replaces with a 30-line marker trait over futures::Stream<Item = Result<
PersistedEnvelope, _>>. PR1's owned-Bytes envelope removed the lifetime
cliff that motivated the GAT. Combinators come from futures::StreamExt
(map, filter, try_fold, try_collect, ...).

Compilation breaks at every call site that used Map/TryMap/etc —
follow-up commits in this PR migrate them to StreamExt.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Drop `<M>` generic from `RawEventStore`, `Subscription`, `SubscriptionBackend`, and friends

**Files:**
- Modify: `crates/nexus-store/src/store.rs`

- [ ] **Step 1: Drop `<M = ()>` from every trait in `store.rs`**

In `crates/nexus-store/src/store.rs`:

For `RawEventStore<M = ()>`:
- Change to `RawEventStore`.
- Replace the GAT lending `type Stream<'a>: BaseEventStream<M, Error = Self::Error> + 'a where Self: 'a;` with a concrete owned-stream associated type:
  ```rust
  type Stream: crate::stream::EventStream<Error = Self::Error> + Send + 'static;
  ```
- Drop the `where Self: 'a` bound on `read_stream`'s return type:
  ```rust
  fn read_stream(
      &self,
      id: &impl Id,
      from: Version,
  ) -> impl std::future::Future<Output = Result<Self::Stream, Self::Error>> + Send;
  ```

For `Subscription<M: 'static = ()>`, `SubscriptionBackend<M: 'static = ()>`, `SharedSubscription<...>`, `SharedSubscriptionBackend<...>` (whichever currently exist): drop `<M>`. Their `type Stream: EventStream<M, Error = Self::Error> + Send + 'static` becomes `type Stream: EventStream<Error = Self::Error> + Send + 'static`.

Update the blanket impl `impl<T, M> Subscription<M> for Arc<T> where T: SubscriptionBackend<M>, M: 'static` to drop `<M>`:

```rust
impl<T> Subscription for Arc<T>
where
    T: SubscriptionBackend,
{
    type Stream = T::Stream;
    type Error = T::Error;
    fn subscribe(
        &self,
        id: &impl Id,
        from: Option<Version>,
    ) -> impl Future<Output = Result<Self::Stream, Self::Error>> + Send {
        T::subscribe(self, id, from)
    }
}
```

- [ ] **Step 2: Re-export adjustments**

In `crates/nexus-store/src/lib.rs`, any `pub use` that referenced `RawEventStore`, `Subscription`, etc., stay the same (the types just lost a default generic).

- [ ] **Step 3: Verify**

```bash
nix develop -c cargo check -p nexus-store --no-default-features
```

Errors are expected in `repository.rs`, `testing.rs`, `snapshot.rs` — all consumers of these traits. Follow-up steps fix them.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/store.rs crates/nexus-store/src/lib.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: drop M generic from RawEventStore / Subscription / SubscriptionBackend

M was always () in practice. Trait surface simplifies:
  RawEventStore::Stream<'a>: BaseEventStream<M> → RawEventStore::Stream: EventStream
  Subscription<M = ()> → Subscription
  SubscriptionBackend<M = ()> → SubscriptionBackend

Drops the GAT-lifetime parameter on Stream as a happy side-effect:
the owned futures::Stream marker doesn't need it. No more
`where Self: 'a` bounds chasing through facades.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Migrate `FjallStream` and the fjall subscription stream to `impl futures::Stream`

**Files:**
- Modify: `crates/nexus-fjall/src/stream.rs`
- Modify: `crates/nexus-fjall/src/subscription_stream.rs`
- Modify: `crates/nexus-fjall/src/store.rs`

- [ ] **Step 1: Rewrite `FjallStream` as a futures::Stream**

The previous `FjallStream` implemented the GAT-lending `EventStream<M>` via a hand-rolled `next()` future. The new shape implements `futures::Stream::poll_next`.

In `crates/nexus-fjall/src/stream.rs`, replace the whole impl:

```rust
use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::Bytes;
use futures::Stream;
use nexus_store::PersistedEnvelope;

use crate::encoding::decode_event_value;
use crate::error::FjallError;

/// A futures-stream over rows scanned from a fjall partition.
///
/// Rows are loaded eagerly into an in-memory `VecDeque<Bytes>` at
/// construction; `poll_next` pops one row, decodes it, and yields a
/// `PersistedEnvelope`. No async IO inside `poll_next`.
pub struct FjallStream {
    rows: std::collections::VecDeque<RowSnapshot>,
}

struct RowSnapshot {
    key: Bytes,
    value: Bytes,
}

impl FjallStream {
    pub(crate) fn from_rows(
        rows: impl IntoIterator<Item = (Bytes, Bytes)>,
    ) -> Self {
        Self {
            rows: rows
                .into_iter()
                .map(|(key, value)| RowSnapshot { key, value })
                .collect(),
        }
    }
}

impl Stream for FjallStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let Some(row) = self.rows.pop_front() else {
            return Poll::Ready(None);
        };
        let decoded = match decode_event_value(&row.key, &row.value) {
            Ok(d) => d,
            Err(e) => return Poll::Ready(Some(Err(e))),
        };
        Poll::Ready(Some(Ok(decoded.into_envelope(row.value))))
    }
}
```

The exact shape of `decode_event_value` + `into_envelope` depends on the PR1 carry-over; adapt as needed but the principle is: the row's `Bytes` is already loaded, decoder returns offsets, envelope wraps the Bytes + offsets.

- [ ] **Step 2: Rewrite the subscription stream similarly**

In `crates/nexus-fjall/src/subscription_stream.rs`, the subscription stream re-reads when caught up. The futures::Stream impl polls the underlying store on `poll_next` after exhausting the current snapshot:

```rust
pub struct FjallSubscriptionStream {
    store: std::sync::Arc<crate::FjallStore>,
    stream_id: crate::stream::OwnedStreamId,
    last_version: Option<nexus::Version>,
    current: FjallStream,
    waker_registered: bool,
}

impl Stream for FjallSubscriptionStream {
    type Item = Result<PersistedEnvelope, FjallError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.current).poll_next(cx) {
                Poll::Ready(Some(Ok(env))) => {
                    self.last_version = Some(env.version());
                    return Poll::Ready(Some(Ok(env)));
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    // Caught up. Re-read.
                    let next = self.store.read_rows_since(
                        &self.stream_id,
                        self.last_version,
                    );
                    if next.is_empty() {
                        // Wait — register a waker on the store's append signal.
                        // (Existing PR1 mechanism; preserve whatever was there.)
                        return Poll::Pending;
                    }
                    self.current = FjallStream::from_rows(next);
                    continue;
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
```

The waker / signal mechanism for "no rows yet, wait for next append" was implemented in the arc-subscription PRs; preserve the existing tokio-channel / notify shape and only swap the outer trait. Do not redesign the wake mechanism in this PR.

- [ ] **Step 3: Update `FjallStore::read_stream` to return the new stream type**

In `crates/nexus-fjall/src/store.rs`, `RawEventStore::Stream` is now a concrete type (`FjallStream`) per Task 7. Update the `read_stream` impl to return a `FjallStream` built from `FjallStream::from_rows(...)`. The eager-load step happens inside `read_stream` (async) — `poll_next` is pure sync over the pre-loaded `VecDeque`.

- [ ] **Step 4: Verify**

```bash
nix develop -c cargo check -p nexus-fjall
nix develop -c cargo nextest run -p nexus-fjall
```

Expected: PASS. Existing test fixtures using `.try_next()` continue to work because `futures::StreamExt::try_next` is the equivalent of the old `.next()`.

- [ ] **Step 5: Commit**

```bash
git add crates/nexus-fjall/src/stream.rs crates/nexus-fjall/src/subscription_stream.rs crates/nexus-fjall/src/store.rs
git commit -m "$(cat <<'EOF'
refactor(fjall)!: FjallStream and subscription stream as futures::Stream

Rewrites the two fjall cursors to implement futures::Stream directly,
matching the new nexus-store::stream::EventStream marker. Rows are
eagerly loaded at construction; poll_next is sync over a VecDeque.

Subscription stream preserves its existing wake-on-append signal
mechanism; only the outer trait shape changes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Migrate `InMemoryStore` streams to `impl futures::Stream`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

- [ ] **Step 1: Rewrite the InMemoryStore cursor as futures::Stream**

In `crates/nexus-store/src/testing.rs`, find the existing GAT-lending stream impl on `InMemoryStream` (or whatever the type is named). Replace with a `futures::Stream` impl:

```rust
use core::pin::Pin;
use core::task::{Context, Poll};
use futures::Stream;

pub struct InMemoryStream {
    rows: std::collections::VecDeque<StoredRow>,
}

impl Stream for InMemoryStream {
    type Item = Result<PersistedEnvelope, InMemoryStoreError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let Some(row) = self.rows.pop_front() else {
            return Poll::Ready(None);
        };
        Poll::Ready(Some(PersistedEnvelope::try_new(
            row.value,
            row.offsets,
            row.version,
            row.global_seq,
        )))
    }
}
```

Same shape for any `InMemorySubscriptionStream` if one exists.

- [ ] **Step 2: Verify**

```bash
nix develop -c cargo check -p nexus-store --all-features
nix develop -c cargo nextest run -p nexus-store --all-features
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: InMemoryStream as futures::Stream

Matches the new EventStream marker shape. Cursor pre-loads StoredRow
clones into a VecDeque; poll_next is sync over the queue.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Update repository facade + snapshot decorator to use `futures::StreamExt`

**Files:**
- Modify: `crates/nexus-store/src/repository.rs`
- Modify: `crates/nexus-store/src/snapshot.rs`
- Modify: `crates/nexus-store/src/builder.rs` (if it references deleted types)

- [ ] **Step 1: Migrate `EventStore::load`**

In `crates/nexus-store/src/repository.rs`, find `EventStore::load`. Replace its GAT-loop body with `futures::StreamExt::try_fold`:

```rust
use futures::StreamExt;
use futures::TryStreamExt;

// ...

let stream = self.store.raw().read_stream(id, from).await.map_err(StoreError::Adapter)?;
let aggregate = stream
    .map_err(StoreError::Adapter)
    .try_fold(initial_aggregate, |mut acc, env| async move {
        let event = self.codec.decode(&env).map_err(StoreError::Codec)?;
        let upcast = self.upcaster.upcast(event, env.schema_version()).map_err(StoreError::Upcast)?;
        acc.replay(env.version(), &upcast).map_err(StoreError::Kernel)?;
        Ok(acc)
    })
    .await?;
```

Adapt to the actual current signatures — the principle: `stream.map_err(...).try_fold(initial, |acc, env| ...).await`.

For `ZeroCopyEventStore::load`, the codec returns `Output<'a>` which is `&'a Archived<E>`. The `try_fold` closure body owns the envelope by value (it's been yielded), so the borrow's `'a` is the envelope's own scope inside the closure — no lifetime issues.

- [ ] **Step 2: Migrate the snapshot decorator**

In `crates/nexus-store/src/snapshot.rs`, the `Snapshotting<R, SS, T>` decorator's `load` impl calls the inner `Repository::load`. If it touches the stream directly (e.g. to count events for the trigger), migrate that loop to `StreamExt::try_fold` similarly.

- [ ] **Step 3: Verify**

```bash
nix develop -c cargo check -p nexus-store --all-features
nix develop -c cargo nextest run -p nexus-store --all-features
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/repository.rs crates/nexus-store/src/snapshot.rs crates/nexus-store/src/builder.rs
git commit -m "$(cat <<'EOF'
refactor(store)!: facade + snapshot decorator use futures::StreamExt

EventStore::load and ZeroCopyEventStore::load drive the new
futures-stream via try_fold. Snapshot decorator's same.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: Update `nexus-framework` projection runner to use `futures::StreamExt`

**Files:**
- Modify: `crates/nexus-framework/src/projection/projection.rs`

- [ ] **Step 1: Migrate the event loop**

In `crates/nexus-framework/src/projection/projection.rs`, find the `Projection::run` event loop. The previous loop used the old GAT trait's `next()` and a manual `while let Some(...)`. Replace with:

```rust
use futures::{StreamExt, TryStreamExt};

// inside Projection::run:
let mut stream = self.subscription.subscribe(&self.stream_id, self.last_position).await
    .map_err(ProjectionError::Subscription)?;

while let Some(item) = stream.next().await {
    let env = item.map_err(ProjectionError::Subscription)?;
    let event = self.codec.decode(&env).map_err(ProjectionError::EventCodec)?;
    self.status = self.status.apply_event(&event, env.version());
    if self.trigger.should_persist(&self.status) {
        self.snapshot_store.commit(&self.id, self.schema_version, env.version(), self.status.state())
            .await
            .map_err(ProjectionError::SnapshotStore)?;
    }
}
```

Adapt to the existing field/method names. The substance: drive `.next()` on the `Pin<&mut impl Stream>`, decode, fold into `ProjectionStatus`, commit on trigger.

- [ ] **Step 2: Verify**

```bash
nix develop -c cargo check -p nexus-framework --all-features
nix develop -c cargo nextest run -p nexus-framework --all-features
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-framework/src/projection/projection.rs
git commit -m "$(cat <<'EOF'
refactor(framework)!: projection runner uses futures::StreamExt

Event loop now drives futures::Stream::next on a pinned subscription.
The old GAT-trait .next() and its lifetime gymnastics are gone.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 12: Add `BytemuckCodec` (feature `bytemuck`)

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`

- [ ] **Step 1: Add the codec module**

In `crates/nexus-store/src/codec.rs`, after the existing `pub mod serde { ... }` block, add:

```rust
#[cfg(feature = "bytemuck")]
pub mod bytemuck {
    use ::bytemuck::{AnyBitPattern, NoUninit, PodCastError};
    use bytes::Bytes;

    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;

    /// Codec for plain-old-data types.
    ///
    /// Zero-copy on the read path: [`Decode::Output<'a>`] is `&'a E`,
    /// pointing directly into the envelope's payload bytes (which are
    /// 16-byte aligned by the wire-format invariant).
    ///
    /// `E` must implement [`AnyBitPattern`] (every bit pattern is a
    /// valid value of `E`) and [`NoUninit`] (no padding/uninitialized
    /// bytes). In practice this means `#[repr(C)]` POD types with no
    /// padding.
    #[derive(Debug, Default, Clone, Copy)]
    pub struct BytemuckCodec;

    impl<E> Encode<E> for BytemuckCodec
    where
        E: NoUninit + Send + Sync + 'static,
    {
        type Error = core::convert::Infallible;

        fn encode(&self, event: &E) -> Result<Bytes, Self::Error> {
            Ok(Bytes::copy_from_slice(::bytemuck::bytes_of(event)))
        }
    }

    impl<E> Decode<E> for BytemuckCodec
    where
        E: AnyBitPattern + NoUninit + Send + Sync + 'static,
    {
        type Output<'a> = &'a E where Self: 'a;
        type Error = PodCastError;

        fn decode<'a>(
            &'a self,
            env: &'a PersistedEnvelope,
        ) -> Result<&'a E, Self::Error> {
            ::bytemuck::try_from_bytes(env.payload())
        }
    }
}
```

- [ ] **Step 2: Add a round-trip + zero-copy assertion test**

Append to the `#[cfg(test)]` module in `codec.rs`:

```rust
#[cfg(all(test, feature = "bytemuck"))]
mod bytemuck_tests {
    use super::bytemuck::BytemuckCodec;
    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;
    use crate::wire::build_row;

    #[repr(C)]
    #[derive(Clone, Copy, ::bytemuck::AnyBitPattern, ::bytemuck::NoUninit)]
    struct Pos { x: f32, y: f32, z: f32, _pad: f32 }

    #[test]
    fn round_trip_zero_copy() {
        let codec = BytemuckCodec;
        let original = Pos { x: 1.0, y: 2.0, z: 3.0, _pad: 0.0 };

        let payload = codec.encode(&original).unwrap();
        let row = build_row("Pos", None, &payload).unwrap();
        let env = PersistedEnvelope::try_new(
            row.value,
            row.offsets,
            nexus::Version::INITIAL,
            crate::store::GlobalSeq::INITIAL,
        ).unwrap();

        let decoded: &Pos = codec.decode(&env).unwrap();

        assert_eq!(decoded.x, 1.0);
        assert_eq!(decoded.y, 2.0);
        assert_eq!(decoded.z, 3.0);

        // Zero-copy: decoded pointer is inside the envelope's payload bytes.
        let env_payload_ptr = env.payload().as_ptr();
        let decoded_ptr = decoded as *const Pos as *const u8;
        assert_eq!(env_payload_ptr, decoded_ptr);
    }
}
```

- [ ] **Step 3: Run the test**

```bash
nix develop -c cargo nextest run -p nexus-store --features bytemuck bytemuck_tests
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/codec.rs
git commit -m "$(cat <<'EOF'
feat(store): add BytemuckCodec under feature `bytemuck`

Zero-copy Decode<E> for plain-old-data types (AnyBitPattern + NoUninit).
Output<'a> = &'a E pointing directly into the envelope's payload bytes,
which are 16-byte aligned by wire-format invariant.

Round-trip test asserts pointer equality between envelope.payload()
and the decoded &E to prove zero-copy.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: Add `RkyvCodec` (feature `rkyv`)

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`

- [ ] **Step 1: Add the rkyv codec module**

In `crates/nexus-store/src/codec.rs`, after the `bytemuck` module, add:

```rust
#[cfg(feature = "rkyv")]
pub mod rkyv {
    use ::rkyv::{
        access,
        api::high::to_bytes,
        rancor,
        Archive, Serialize,
    };
    use bytes::Bytes;

    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;

    /// Zero-copy codec backed by rkyv 0.8.
    ///
    /// Encode: serialize `E` to its archived bytes via [`to_bytes`].
    /// Decode: validate + cast envelope payload to `&Archived<E>`.
    ///
    /// `Output<'a> = &'a <E as Archive>::Archived`. Borrows directly
    /// from envelope payload (16-byte aligned by wire-format).
    #[derive(Debug, Default, Clone, Copy)]
    pub struct RkyvCodec;

    impl<E> Encode<E> for RkyvCodec
    where
        E: for<'a> Serialize<::rkyv::ser::Serializer<'a, ::rkyv::util::AlignedVec, ::rkyv::ser::sharing::Share, rancor::Error>>
            + Send
            + Sync
            + 'static,
    {
        type Error = rancor::Error;

        fn encode(&self, event: &E) -> Result<Bytes, Self::Error> {
            let aligned = to_bytes::<rancor::Error>(event)?;
            // AlignedVec → Vec<u8> → Bytes (no alignment guarantee survives the conversion,
            // but the wire-format pads to 16 on write, so envelope's payload regains alignment).
            Ok(Bytes::from(aligned.into_vec()))
        }
    }

    impl<E> Decode<E> for RkyvCodec
    where
        E: Archive + Send + Sync + 'static,
        E::Archived: ::rkyv::bytecheck::CheckBytes<
            ::rkyv::api::high::HighValidator<'static, rancor::Error>,
        >,
    {
        type Output<'a> = &'a E::Archived where Self: 'a;
        type Error = rancor::Error;

        fn decode<'a>(
            &'a self,
            env: &'a PersistedEnvelope,
        ) -> Result<&'a E::Archived, Self::Error> {
            access::<E::Archived, rancor::Error>(env.payload())
        }
    }
}
```

(The exact rkyv 0.8 generic bounds may need adjustment to whatever the latest patch published. If `to_bytes` / `access` generic shape differs, follow the docs.rs signature for the installed rkyv version. Do NOT silently widen / weaken the bounds — match the upstream signature.)

- [ ] **Step 2: Round-trip test**

```rust
#[cfg(all(test, feature = "rkyv"))]
mod rkyv_tests {
    use super::rkyv::RkyvCodec;
    use super::{Decode, Encode};
    use crate::envelope::PersistedEnvelope;
    use crate::wire::build_row;

    #[derive(::rkyv::Archive, ::rkyv::Serialize, ::rkyv::Deserialize, Debug, PartialEq, Eq)]
    struct Move { steps: u32, dir: u8 }

    #[test]
    fn round_trip() {
        let codec = RkyvCodec;
        let original = Move { steps: 42, dir: 3 };

        let payload = codec.encode(&original).unwrap();
        let row = build_row("Move", None, &payload).unwrap();
        let env = PersistedEnvelope::try_new(
            row.value,
            row.offsets,
            nexus::Version::INITIAL,
            crate::store::GlobalSeq::INITIAL,
        ).unwrap();

        let archived = codec.decode(&env).unwrap();
        assert_eq!(archived.steps, 42);
        assert_eq!(archived.dir, 3);
    }
}
```

- [ ] **Step 3: Run the test**

```bash
nix develop -c cargo nextest run -p nexus-store --features rkyv rkyv_tests
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/nexus-store/src/codec.rs
git commit -m "$(cat <<'EOF'
feat(store): add RkyvCodec under feature `rkyv`

Decode<E>::Output<'a> = &'a <E as Archive>::Archived borrowed from
envelope payload (16-byte aligned by wire-format invariant). Encode
serializes via rkyv::to_bytes and adapts AlignedVec → Bytes.

rkyv 0.8 pinned. Codec types leak rkyv's Archive / CheckBytes bounds —
breaking changes in rkyv = breaking changes in nexus-store.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 14: Update examples

**Files:**
- Modify: `examples/store-and-kernel/src/main.rs`
- Modify: `examples/store-inmemory/src/main.rs`

- [ ] **Step 1: Replace `EventStreamExt` with `futures::StreamExt`**

In both example mains, search for `EventStreamExt`, `.try_count()`, `.try_collect()`, `.map_err`, `.try_fold`. Replace the import with `use futures::{StreamExt, TryStreamExt};` and let the trait methods resolve through `futures`.

If the examples used `IntoStream` / `OwnedEventStream`, drop those calls — the underlying stream IS a `futures::Stream` now.

- [ ] **Step 2: Verify**

```bash
nix develop -c cargo run --example store-inmemory
nix develop -c cargo run --example store-and-kernel
```

Expected: both run to completion, prior output preserved.

- [ ] **Step 3: Commit**

```bash
git add examples/store-and-kernel/src/main.rs examples/store-inmemory/src/main.rs
git commit -m "$(cat <<'EOF'
refactor(examples): migrate to futures::StreamExt

Replaces the deleted EventStreamExt / IntoStream surface with stdlib
futures combinators. No behavior change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 15: Cross-cutting test categories for the new wire format

**Files:**
- Add: `crates/nexus-store/tests/wire_alignment_tests.rs`

- [ ] **Step 1: Write the four-category tests**

```rust
//! Cross-cutting tests for the wire-format alignment invariant.
//! Each test maps to one of the 4 categories required by CLAUDE.md
//! (sequence/protocol, lifecycle, defensive boundary, linearizability).

use bytes::Bytes;
use nexus::{Id, Version};
use nexus_store::{
    testing::InMemoryStore, PendingEnvelope, RawEventStore,
};

// Sequence/protocol — multiple appends to the same stream, each yielded
// envelope's payload is 16-byte aligned.
#[tokio::test]
async fn sequence_payloads_are_aligned() {
    let store = InMemoryStore::new();
    let id = TestId("seq".into());
    let envelopes: Vec<_> = (1..=10).map(|n| build_envelope(n)).collect();

    store.append(&id, None, &envelopes).await.unwrap();

    let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
    use futures::StreamExt;
    while let Some(item) = stream.next().await {
        let env = item.unwrap();
        let payload = env.payload();
        assert_eq!(payload.as_ptr() as usize % 16, 0,
            "payload pointer not 16-aligned for version {}", env.version());
    }
}

// Lifecycle — InMemoryStore doesn't persist, so the equivalent is
// fresh-state append + clone + read.
#[tokio::test]
async fn lifecycle_clone_then_read_preserves_alignment() {
    let store = InMemoryStore::new();
    let id = TestId("lc".into());
    store.append(&id, None, &[build_envelope(1)]).await.unwrap();

    let cloned = store.clone(); // Arc bump
    let mut stream = cloned.read_stream(&id, Version::INITIAL).await.unwrap();
    use futures::StreamExt;
    let env = stream.next().await.unwrap().unwrap();
    assert_eq!(env.payload().as_ptr() as usize % 16, 0);
}

// Defensive boundary — feed pathological event_type lengths through
// the public API and confirm alignment never breaks.
#[tokio::test]
async fn boundary_long_event_type_still_aligned() {
    let store = InMemoryStore::new();
    let id = TestId("b".into());
    for et_len in [0, 1, 7, 15, 16, 17, 63, 64, 100, 1024] {
        let env = build_envelope_with_event_type_len(1, et_len);
        store.append(&id, None, &[env]).await.unwrap();
    }
    let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
    use futures::StreamExt;
    while let Some(item) = stream.next().await {
        let env = item.unwrap();
        assert_eq!(env.payload().as_ptr() as usize % 16, 0);
    }
}

// Linearizability — concurrent writer + reader, every read sees aligned payload.
#[tokio::test(flavor = "multi_thread")]
async fn linearizability_concurrent_writer_aligned_payloads() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let store = Arc::new(InMemoryStore::new());
    let id = TestId("lin".into());
    let barrier = Arc::new(Barrier::new(2));

    let writer = {
        let store = store.clone();
        let id = id.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            for n in 1..=100 {
                store.append(&id, None, &[build_envelope(n)]).await.unwrap();
            }
        })
    };

    let reader = {
        let store = store.clone();
        let id = id.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            for _ in 0..50 {
                let mut stream = store.read_stream(&id, Version::INITIAL).await.unwrap();
                use futures::StreamExt;
                while let Some(item) = stream.next().await {
                    let env = item.unwrap();
                    assert_eq!(env.payload().as_ptr() as usize % 16, 0);
                }
            }
        })
    };

    let _ = tokio::try_join!(writer, reader).unwrap();
}

// --- helpers ---

fn build_envelope(n: u64) -> PendingEnvelope {
    build_envelope_with_event_type_len(n, "MyEvent".len())
}

fn build_envelope_with_event_type_len(n: u64, et_len: usize) -> PendingEnvelope {
    let event_type: String = "E".repeat(et_len);
    PendingEnvelope::builder()
        .version(Version::new(n).unwrap())
        .event_type(event_type)
        .payload(Bytes::from(vec![0xAA_u8; 42]))
        .build(None)
}

#[derive(Clone)]
struct TestId(String);
impl AsRef<[u8]> for TestId {
    fn as_ref(&self) -> &[u8] { self.0.as_bytes() }
}
impl core::fmt::Display for TestId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
impl core::fmt::Debug for TestId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TestId({:?})", self.0)
    }
}
impl core::hash::Hash for TestId {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) { self.0.hash(state); }
}
impl PartialEq for TestId { fn eq(&self, o: &Self) -> bool { self.0 == o.0 } }
impl Eq for TestId {}
impl Id for TestId {}
```

(Adapt `PendingEnvelope::builder()` to whatever the PR1 final builder shape is — `.event_type(...).payload(...).build(None)`, etc.)

- [ ] **Step 2: Run the suite**

```bash
nix develop -c cargo nextest run -p nexus-store --test wire_alignment_tests --features testing
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/nexus-store/tests/wire_alignment_tests.rs
git commit -m "$(cat <<'EOF'
test(store): 4-category coverage for wire-format alignment invariant

Sequence, lifecycle (clone+read), defensive boundary (pathological
event_type lengths), and linearizability (concurrent writer + reader).
Every yielded PersistedEnvelope's payload pointer is asserted to be
a multiple of 16 bytes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 16: Final `nix flake check` + push + PR

**Files:** (none; verification + git only)

- [ ] **Step 1: Run the full check**

```bash
nix flake check
```

Expected: PASS (clippy deny-warnings, fmt, taplo, audit, deny, nextest across all crates, hakari verify).

If hakari complains: `nix develop -c cargo hakari generate && git add crates/workspace-hack && git commit -m "chore(deps): hakari regenerate after PR2"`.

- [ ] **Step 2: Format + amend if needed**

```bash
nix develop -c cargo fmt --all
git diff --quiet || git commit -am "style: rustfmt after PR2"
```

- [ ] **Step 3: Push + create PR**

```bash
git push -u origin refactor/bytes-envelope-pr2
gh pr create --title "refactor(store, fjall, framework)!: stream collapse + codec collapse + wire alignment — PR2 of bytes-envelope refactor" --body "$(cat <<'EOF'
## Summary

PR2 of the bytes-envelope refactor. Builds on #182 (PR1).

### What's in this PR

- **Stream trait collapse**: ~1370 lines of GAT-lending stream traits + combinators replaced with a 30-line `futures::Stream` marker. `EventStreamExt`, `Map`, `TryMap`, `MapErr`, `TryScan`, `Progress`, `Step`, `OwnedEventStream`, `IntoStream`, `BaseEventStream`, `Disposition` all deleted. Combinators come from `futures::StreamExt`.
- **Codec trait collapse**: `Decode<E>` + `BorrowingDecode<E>` merged into one `Decode<E>` with a `type Output<'a>` GAT. `Encode::encode` returns `Bytes` instead of `Vec<u8>`. `decode(name, payload)` becomes `decode(env: &PersistedEnvelope)`.
- **Wire-format alignment**: new `nexus-store::wire` module is the single canonical row builder. Payload is padded to a 16-byte boundary; fjall and InMemoryStore both call `wire::build_row`. Re-enables the two `#[ignore]`'d zero-copy tests from PR1.
- **`M` generic dropped** from `RawEventStore`, `Subscription`, `SubscriptionBackend`, `SharedSubscription`, `SharedSubscriptionBackend`. `RawEventStore::Stream<'a>` GAT becomes concrete owned `type Stream`.
- **New codecs** (feature-gated): `BytemuckCodec` for POD types, `RkyvCodec` for archived deserialization. Both zero-copy on read.

### Migration for users

- `EventStreamExt::try_fold` → `futures::TryStreamExt::try_fold` (same shape).
- `Decode::decode(name, payload)` → `Decode::decode(env)` (`env.event_type()`, `env.payload()`).
- `Encode::encode` return type `Vec<u8>` → `Bytes` (`Bytes::from(vec)` zero-copy adapter).
- `BorrowingDecode` → `Decode<E>` with `type Output<'a> = &'a T`.

## Test plan

- [x] `nix flake check` — clippy, fmt, taplo, audit, deny, nextest, hakari
- [x] Re-enabled `zero_copy_save_and_load_roundtrip` + `zero_copy_multi_save_load` PASS
- [x] New `wire::tests` proptests verify payload alignment across arbitrary inputs
- [x] New `wire_alignment_tests` cover all 4 cross-cutting categories
- [x] BytemuckCodec round-trip + pointer-equality zero-copy assertion
- [x] RkyvCodec round-trip
- [x] Examples (`store-inmemory`, `store-and-kernel`) run to completion

Companion docs in `docs/plans/`:
- `2026-05-27-bytes-envelope-design.md`
- `2026-05-27-bytes-envelope-deviations.md` (resolved A/B/C subsections under `[PR 2 | scoping]`)
- `2026-05-28-bytes-envelope-pr2-implementation.md`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 4: Verify PR is up**

```bash
gh pr view --json url -q .url
```

Expected: prints the PR URL.

---

## Self-Review Checklist

After implementation, verify:

- [ ] No `#[ignore]` attributes added during PR2. (PR1 added two; PR2 removes them. Zero net.)
- [ ] No `unwrap` / `expect` in non-test code. (`unwrap()` on `proptest` strategy values inside `proptest!` blocks is fine; production `expect("...")` requires a why-this-cannot-fail reason in the message.)
- [ ] All deleted trait re-exports also deleted from `lib.rs` `pub use` lines. Grep `EventStreamExt`, `OwnedEventStream`, `IntoStream`, `BaseEventStream`, `Disposition`, `BorrowingDecode`, `Map`, `TryMap`, `MapErr`, `TryScan`, `Progress`, `Step` across `crates/**` after Task 11 — should match only inside tests/examples that have been updated.
- [ ] `nix flake check` PASSES (not just `cargo check` — the flake covers clippy strict, taplo, audit, deny, hakari).
- [ ] Wire-format alignment proptest in `wire.rs` covers `event_type` lengths 0..256 — including 15, 16, 17 (alignment boundary cases).
- [ ] `bytes::Bytes::from_owner(avec)` is reached via `wire::build_row` — no other call site allocates the row.
- [ ] `aligned-vec = "=0.6.4"` is pinned with `=`, not `^`.
- [ ] No new `#[non_exhaustive]` attributes (banned per CLAUDE.md).
- [ ] All new error enums use `thiserror` (`WireError`, etc.).
- [ ] Deviation log appended for any divergence from this plan (per `[PR 2 | scoping]` resolution: divergences in plan execution go in the deviation log, not silently in code).
