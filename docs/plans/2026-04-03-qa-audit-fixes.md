# QA Audit Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all QA audit findings except C-1 (no Repository impl) and C-3 (PendingEnvelope stream_id redundancy).

**Architecture:** Changes span both `nexus` (kernel) and `nexus-store` crates. Most fixes are doc improvements, new tests, or small API additions. The largest change is C-2 (upcaster validation helper).

**Tech Stack:** Rust, proptest, tokio

---

### Task 1: C-2 — Add upcaster validation with loop guard

**Files:**
- Modify: `crates/nexus-store/src/upcaster.rs`
- Modify: `crates/nexus-store/src/error.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/upcaster_tests.rs`

**Step 1: Add `UpcastError` to `error.rs`**

Add after `StoreError`:

```rust
/// Errors from upcaster validation.
#[derive(Debug, Error)]
pub enum UpcastError {
    /// Upcaster returned the same or lower schema version.
    #[error("Upcaster did not advance schema version for '{event_type}': input {input_version}, output {output_version}")]
    VersionNotAdvanced {
        event_type: String,
        input_version: u32,
        output_version: u32,
    },

    /// Upcaster returned an empty event type.
    #[error("Upcaster returned empty event_type (input: '{input_event_type}', schema version {schema_version})")]
    EmptyEventType {
        input_event_type: String,
        schema_version: u32,
    },

    /// Upcaster chain exceeded the iteration limit.
    #[error("Upcaster chain exceeded {limit} iterations for '{event_type}' (stuck at schema version {schema_version})")]
    ChainLimitExceeded {
        event_type: String,
        schema_version: u32,
        limit: u32,
    },
}
```

**Step 2: Add `apply_upcasters()` to `upcaster.rs`**

```rust
use crate::error::UpcastError;

const DEFAULT_CHAIN_LIMIT: u32 = 100;

/// Apply upcasters safely with validation and loop detection.
///
/// Chains upcasters until none match, validating each step:
/// - Output `schema_version` must be strictly greater than input
/// - Output `event_type` must not be empty
/// - Total iterations are bounded by `DEFAULT_CHAIN_LIMIT` (100)
///
/// Returns the final `(event_type, schema_version, payload)`.
///
/// # Errors
///
/// Returns [`UpcastError`] if any validation fails or the chain limit is exceeded.
pub fn apply_upcasters(
    upcasters: &[&dyn EventUpcaster],
    event_type: &str,
    schema_version: u32,
    payload: &[u8],
) -> Result<(String, u32, Vec<u8>), UpcastError> {
    let mut current_type = event_type.to_owned();
    let mut current_version = schema_version;
    let mut current_payload = payload.to_vec();
    let mut iterations = 0u32;

    loop {
        let matched = upcasters
            .iter()
            .find(|u| u.can_upcast(&current_type, current_version));

        let Some(upcaster) = matched else {
            break;
        };

        iterations += 1;
        if iterations > DEFAULT_CHAIN_LIMIT {
            return Err(UpcastError::ChainLimitExceeded {
                event_type: current_type,
                schema_version: current_version,
                limit: DEFAULT_CHAIN_LIMIT,
            });
        }

        let input_version = current_version;
        let input_type = current_type.clone();
        let (new_type, new_version, new_payload) =
            upcaster.upcast(&current_type, current_version, &current_payload);

        if new_version <= input_version {
            return Err(UpcastError::VersionNotAdvanced {
                event_type: input_type,
                input_version,
                output_version: new_version,
            });
        }

        if new_type.is_empty() {
            return Err(UpcastError::EmptyEventType {
                input_event_type: input_type,
                schema_version: new_version,
            });
        }

        current_type = new_type;
        current_version = new_version;
        current_payload = new_payload;
    }

    Ok((current_type, current_version, current_payload))
}
```

**Step 3: Re-export from `lib.rs`**

Add `pub use error::UpcastError;` and `pub use upcaster::apply_upcasters;`.

**Step 4: Write tests in `upcaster_tests.rs`**

Add tests:
- `apply_upcasters_rejects_version_downgrade`
- `apply_upcasters_rejects_same_version`
- `apply_upcasters_rejects_empty_event_type`
- `apply_upcasters_rejects_infinite_loop`
- `apply_upcasters_chains_correctly`
- `apply_upcasters_noop_when_no_match`

**Step 5: Run tests**

Run: `cargo test -p nexus-store -- upcaster`

**Step 6: Commit**

```
feat(store): add apply_upcasters() with validation and loop guard
```

---

### Task 2: H-4 — Enforce EventStream monotonic version in InMemoryStream

**Files:**
- Modify: `crates/nexus-store/src/stream.rs` (doc update)
- Modify: `crates/nexus-store/src/testing.rs` (debug_assert)
- Test: `crates/nexus-store/tests/stream_tests.rs`

**Step 1: Update trait docs on `EventStream::next()`**

Add to the `next()` doc comment:
```rust
/// Once this method returns `None`, all subsequent calls must also
/// return `None` (fused behavior).
```

**Step 2: Add monotonicity debug_assert in `InMemoryStream`**

Add a `prev_version: Option<u64>` field and validate in `next()`.

**Step 3: Add test for `next()` after `None` (fused behavior)**

**Step 4: Run tests**

Run: `cargo test -p nexus-store -- stream`

**Step 5: Commit**

```
fix(store): document fused EventStream, add monotonicity debug_assert
```

---

### Task 3: H-5 — Document atomicity requirement on `RawEventStore::append()`

**Files:**
- Modify: `crates/nexus-store/src/raw.rs`

**Step 1: Add atomicity docs**

Add to `append()` doc comment:
```rust
/// # Atomicity
///
/// The version check and event insertion **must** be atomic.
/// If the version check and write are separate operations (e.g. SELECT
/// then INSERT), a concurrent writer can slip in between, corrupting
/// the stream. Implementations should use transactions, CAS operations,
/// or a lock to prevent this.
```

**Step 2: Commit**

```
docs(store): document atomicity requirement on RawEventStore::append
```

---

### Task 4: H-6 — Add apply() idempotency property test to kernel

**Files:**
- Modify: `crates/nexus/tests/kernel_tests/property_tests.rs`

**Step 1: Add Property 9: replay-apply equivalence produces identical state**

This already exists as Property 4. The real gap is testing that `apply()` is a pure function — replaying the same events always yields the same state regardless of how many times we rebuild. Add:

```rust
/// Property 9: State is a pure function of events
///
/// Building state from the same events via different paths
/// (apply vs replay vs replay-after-take) must all converge.
#[test]
fn prop_state_is_pure_function_of_events(events in proptest::collection::vec(arb_event(), 1..50)) {
    // Path A: apply all
    let mut a = AggregateRoot::<CountAgg>::new(PId(1));
    for e in &events { a.apply(e.clone()); }

    // Path B: replay all
    let mut b = AggregateRoot::<CountAgg>::new(PId(1));
    for (i, e) in events.iter().enumerate() {
        b.replay(Version::from_persisted((i+1) as u64), e).unwrap();
    }

    // Path C: apply, take, then replay remaining from scratch
    let mid = events.len() / 2;
    let mut c = AggregateRoot::<CountAgg>::new(PId(1));
    for e in &events[..mid] { c.apply(e.clone()); }
    let _ = c.take_uncommitted_events();
    for (i, e) in events[mid..].iter().enumerate() {
        let v = (mid + i + 1) as u64;
        c.replay(Version::from_persisted(v), e).unwrap();
    }

    prop_assert_eq!(a.state(), b.state());
    prop_assert_eq!(b.state(), c.state());
}
```

**Step 2: Run tests**

Run: `cargo test -p nexus -- prop_state_is_pure`

**Step 3: Commit**

```
test(kernel): add property test proving state is a pure function of events
```

---

### Task 5: H-7 — Bridge KernelError into StoreError

**Files:**
- Modify: `crates/nexus-store/src/error.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/error_tests.rs`

**Step 1: Add `Kernel` variant to `StoreError`**

```rust
/// Kernel error during replay (e.g. version mismatch, rehydration limit).
#[error("Kernel error: {0}")]
Kernel(#[from] nexus::KernelError),
```

**Step 2: Update exhaustiveness test in `security_tests.rs`**

Add `StoreError::Kernel(..)` arm.

**Step 3: Add test in `error_tests.rs`**

```rust
#[test]
fn kernel_error_converts_to_store_error() {
    let kernel_err = nexus::KernelError::VersionMismatch {
        stream_id: nexus::ErrorId::from_display(&"test"),
        expected: Version::INITIAL,
        actual: Version::from_persisted(1),
    };
    let store_err: StoreError = kernel_err.into();
    assert!(matches!(store_err, StoreError::Kernel(_)));
}
```

**Step 4: Update `Repository` trait docs with error handling pattern**

**Step 5: Run tests**

Run: `cargo test -p nexus-store -- error`

**Step 6: Commit**

```
feat(store): add StoreError::Kernel variant with From<KernelError>
```

---

### Task 6: M-8 — Document InMemoryStore limitations

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

**Step 1: Add doc comment to `InMemoryStore`**

```rust
/// # Limitations vs real adapters
///
/// `InMemoryStore` is useful for testing business logic but cannot
/// catch the following classes of bugs:
///
/// - **Row ordering**: Events are always returned in insertion order.
///   A real database may return rows out of order without an explicit
///   `ORDER BY`, violating the monotonic version contract.
/// - **Partial writes**: Appends are atomic (Mutex). A real database
///   may crash mid-batch, leaving a stream in an inconsistent state.
/// - **Payload size limits**: No limit on payload size. Real databases
///   have row/column size constraints.
/// - **Distributed concurrency**: Single-process only. Cannot simulate
///   multiple writers on different machines racing on the same stream.
```

**Step 2: Commit**

```
docs(store): document InMemoryStore testing limitations
```

---

### Task 7: M-9 — Add missing lifecycle integration tests

**Files:**
- Modify: `crates/nexus-store/tests/integration_tests.rs`

**Step 1: Add tests**

- `next_after_none_returns_none` — fused stream behavior
- `append_then_read_empty_stream_returns_nothing` — different stream
- `read_from_future_version_returns_empty` — version beyond stream end
- `single_event_stream` — minimal case

**Step 2: Run tests**

Run: `cargo test -p nexus-store -- integration`

**Step 3: Commit**

```
test(store): add lifecycle integration tests for edge cases
```

---

### Task 8: M-10 — Add fallible `PersistedEnvelope::try_new()`

**Files:**
- Modify: `crates/nexus-store/src/envelope.rs`
- Modify: `crates/nexus-store/src/error.rs`
- Test: `crates/nexus-store/tests/envelope_tests.rs`

**Step 1: Add `try_new()` method**

```rust
/// Fallible constructor — returns `Err` if `schema_version` is 0.
///
/// Prefer this over [`new()`](Self::new) in adapter code where you want
/// to surface the error instead of panicking.
pub fn try_new(
    stream_id: &'a str,
    version: Version,
    event_type: &'a str,
    schema_version: u32,
    payload: &'a [u8],
    metadata: M,
) -> Result<Self, InvalidSchemaVersion> {
    if schema_version == 0 {
        return Err(InvalidSchemaVersion);
    }
    Ok(Self { stream_id, version, event_type, schema_version, payload, metadata })
}
```

Add `InvalidSchemaVersion` as a small error type in `error.rs`.

**Step 2: Add test**

```rust
#[test]
fn try_new_rejects_zero_schema_version() {
    let result = PersistedEnvelope::<()>::try_new("s1", Version::from_persisted(1), "E", 0, &[], ());
    assert!(result.is_err());
}

#[test]
fn try_new_accepts_valid_schema_version() {
    let result = PersistedEnvelope::<()>::try_new("s1", Version::from_persisted(1), "E", 1, &[], ());
    assert!(result.is_ok());
}
```

**Step 3: Run tests**

Run: `cargo test -p nexus-store -- envelope`

**Step 4: Commit**

```
feat(store): add PersistedEnvelope::try_new() fallible constructor
```

---

### Task 9: L-11 — Document Codec object-safety limitation

**Files:**
- Modify: `crates/nexus-store/src/codec.rs`

**Step 1: Add doc note**

```rust
/// # Object safety
///
/// `Codec<E>` is **not** object-safe because the type parameter `E`
/// appears in method signatures. You cannot use `dyn Codec<MyEvent>`.
/// This is a deliberate trade-off: the generic parameter enables
/// zero-cost monomorphization and avoids boxing events on the hot path.
/// If you need dynamic dispatch, wrap the codec in a concrete type
/// that erases `E` via an internal enum or serialization format.
```

**Step 2: Commit**

```
docs(store): document Codec object-safety limitation
```

---

### Task 10: L-12 — Document ErrorId truncation

**Files:**
- Modify: `crates/nexus/src/error.rs`

**Step 1: Add doc note to `ErrorId`**

```rust
/// # Truncation
///
/// IDs longer than 64 bytes are silently truncated. This keeps error
/// construction allocation-free (safe under OOM) but means very long
/// aggregate IDs will be cut off in error messages. Keep `Id::Display`
/// output reasonably short (< 64 bytes) for best diagnostics.
```

**Step 2: Commit**

```
docs(kernel): document ErrorId 64-byte truncation behavior
```

---

### Task 11: L-13 — Improve panic message for MAX_UNCOMMITTED = 0

**Files:**
- Modify: `crates/nexus/src/aggregate.rs`

**Step 1: Add early check in `apply()`**

The current assert `self.uncommitted_events.len() < A::MAX_UNCOMMITTED` already panics when MAX_UNCOMMITTED is 0 (since 0 < 0 is false). The message is already clear. Improve by also noting "did you set MAX_UNCOMMITTED to 0?" when the limit is 0:

Just make the assert message more diagnostic:

```rust
assert!(
    self.uncommitted_events.len() < A::MAX_UNCOMMITTED,
    "Uncommitted event limit reached ({limit}). {hint}",
    limit = A::MAX_UNCOMMITTED,
    hint = if A::MAX_UNCOMMITTED == 0 {
        "MAX_UNCOMMITTED is 0 — this aggregate cannot accept any events. \
         Override Aggregate::MAX_UNCOMMITTED with a value > 0."
    } else {
        "Call take_uncommitted_events() to drain."
    },
);
```

**Step 2: Run tests**

Run: `cargo test -p nexus -- c3_apply_panics`

**Step 3: Commit**

```
fix(kernel): improve panic message when MAX_UNCOMMITTED is 0
```

---

### Task 12: Final verification

**Step 1:** `cargo clippy --all-targets -- --deny warnings`
**Step 2:** `cargo test --all`
**Step 3:** `cargo fmt --all --check`
