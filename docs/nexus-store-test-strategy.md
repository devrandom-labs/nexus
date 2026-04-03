# nexus-store Test Strategy

This document catalogs every adversarial property test in `nasa_break_everything.rs`, classifies when each earns its keep, and maps out the full spectrum of testing techniques — from proptest to formal verification — with concrete guidance on what to write and where.

## The Core Problem

```
  EventStore / ZeroCopyEventStore   ← facade (your code)
           ↓ calls
       RawEventStore                ← trait (adapter implements)
           ↓ backed by
    InMemoryStore / fjall / sqlite  ← concrete storage
```

Tests against `InMemoryStore` exercise the **facade logic** and **trait contracts**, but they _cannot_ catch bugs that only manifest in real storage engines (row ordering, partial writes, actual concurrency races, disk failures). The Mutex-backed HashMap is trivially correct for things like isolation and atomicity.

Each test falls into one of three tiers:

| Tier | What it tests | When to run | Value now |
|------|---------------|-------------|-----------|
| **A — Core Logic** | Facade code, upcaster pipeline, version arithmetic, envelope invariants | Always, every CI run | High |
| **B — Adapter Conformance** | RawEventStore contract properties that _every_ adapter must satisfy | Per-adapter, when adapters land | Template only |
| **C — Integration Stress** | Full save/load lifecycle, concurrent writes, model-based checking | Per-adapter + CI nightly | Template only |

---

## Tier A — Run Now, Every CI (33 tests)

These test **your code** — `event_store.rs`, `upcaster.rs`, `upcaster_chain.rs`, `envelope.rs`, `version.rs`. Adapter-independent.

### Envelope Invariants

| Test | What it attacks | Why it matters |
|------|-----------------|----------------|
| `attack_persisted_envelope_schema_version_zero_panics` | `PersistedEnvelope::new()` with schema_version=0 | Guarantees "schema versions start at 1" — corrupt DB rows crash visibly |
| `attack_persisted_envelope_try_new_rejects_zero` | `try_new()` fallible path | Same invariant via the error path |
| `attack_persisted_envelope_accessors_are_faithful` | All 6 accessors on PersistedEnvelope | Regression guard: catches swapped fields across 512 random inputs |
| `attack_pending_envelope_builder_preserves_all_fields` | Typestate builder end-to-end | Data flows through 4 intermediate types without loss |
| `attack_persisted_envelope_boundary_schema_versions` | schema_version across full u32 range | Boundary testing — u32::MAX could trip upcaster arithmetic |

### Version Arithmetic

| Test | What it attacks | Why it matters |
|------|-----------------|----------------|
| `attack_version_from_persisted_roundtrip` | `from_persisted(v).as_u64() == v` for all u64 | Foundational — if this breaks, every adapter is corrupt |
| `attack_version_next_chain` | `next()` called up to 100 times | Verifies increment-by-1, catches overflow at MAX |
| `attack_version_ordering` | `Version` Ord matches `u64` Ord | Wrong ordering = silent data loss in concurrency checks |
| `attack_version_next_at_max_panics` | `next()` at u64::MAX | Must panic, not wrap to 0 |

### Upcaster Pipeline

| Test | What it attacks | Why it matters |
|------|-----------------|----------------|
| `attack_upcaster_rejects_same_version` | Upcaster returns same schema_version | Must catch — otherwise infinite loop |
| `attack_upcaster_rejects_lower_version` | Returns lower schema_version | Must error, not loop or corrupt |
| `attack_upcaster_rejects_empty_event_type` | Returns empty string | Would break codec dispatch |
| `attack_upcaster_chain_limit_101_iterations` | Always-matching incrementing upcaster | Verifies 100-iteration safety limit |
| `attack_upcaster_chain_correct_final_state` | Well-behaved 1..20 step chain | Correct final version + payload preserved |
| `attack_upcaster_chain_cons_list_dispatch` | `Chain<H, T>` dispatch | First match wins, falls through to tail |
| `attack_empty_upcaster_chain` | Empty chain `()` | Must return None |
| `attack_apply_upcasters_no_op_with_empty_chain` | Empty slice | Inputs returned unchanged |
| `attack_upcaster_type_morphing_chain` | Renames event_type each step | Type routing through multi-step upcasting |
| `attack_upcaster_payload_transformation` | Modifies payload bytes | Transformed payload reaches caller |
| `attack_deep_upcaster_chain_no_stack_overflow` | 50-step chain | Stack safety for long migrations |

### EventStore Facade Logic

| Test | What it attacks | Why it matters |
|------|-----------------|----------------|
| `attack_event_store_save_load_roundtrip` | Save N events, load, check state | The fundamental contract |
| `attack_event_store_save_load_happened_events` | String events roundtrip | Tests codec routing with different variant |
| `attack_event_store_noop_save` | Save with no uncommitted events | Must be a no-op |
| `attack_event_store_load_empty_stream` | Load non-existent stream | Must return fresh aggregate at INITIAL |
| `attack_event_store_upcasters_applied_on_load` | Raw events at schema v1 + upcaster | Pipeline runs during `Repository::load()` |
| `attack_event_store_concurrent_conflict` | Two copies, both save | Second must fail |
| `attack_event_store_multi_cycle_lifecycle` | Multiple save/load/apply cycles | Version tracking across full lifecycle |
| `attack_event_store_interleaved_save_apply` | Apply → save → apply → save → load → apply → save | Complex interleaving correctness |
| `attack_mixed_event_types_roundtrip` | Interleave ValueSet and Happened | Multi-type codec dispatch |
| `attack_save_drains_uncommitted` | Verify drain + safe re-save + continue | take_uncommitted → version advance |
| `attack_aggregate_max_uncommitted_panics` | Apply beyond MAX_UNCOMMITTED=3 | Kernel safety limit fires |

### Codec Roundtrip

| Test | What it attacks | Why it matters |
|------|-----------------|----------------|
| `attack_codec_roundtrip_happened` | encode → decode string variant | Codec correctness |
| `attack_codec_roundtrip_value_set` | encode → decode numeric variant | Full i64 range |
| `attack_codec_rejects_unknown_type` | Unknown event_type | Must error, not panic |

### Error Semantics

| Test | What it attacks | Why it matters |
|------|-----------------|----------------|
| `attack_store_error_conflict_preserves_info` | `StoreError::Conflict` Display | Both versions in message |
| `attack_upcast_error_display_includes_context` | All 3 UpcastError variants | Debuggable errors |

---

## Tier B — Adapter Conformance Suite (14 tests)

Currently testing HashMap + Mutex (trivially correct). **Critical** when wired to a real adapter.

**How to reuse:** Extract into generic `fn test_X<S: RawEventStore>(store: &S)`, call from each adapter's test suite.

### RawEventStore Contract

| Test | What it attacks | Real adapter value |
|------|-----------------|---------------------|
| `attack_append_read_roundtrip_any_payload` | Append N, read, compare bytes | BLOB corruption, row truncation |
| `attack_append_read_roundtrip_evil_payloads` | Adversarial payloads (empty, 0xFF, 100KB, nulls) | Size limits, special byte handling |
| `attack_concurrency_wrong_expected_version` | Wrong expected_version | CAS / transaction logic |
| `attack_non_sequential_versions_rejected` | Random version sequences | Envelope version validation |
| `attack_duplicate_versions_rejected` | All-same versions in batch | Degenerate sequential validation |
| `attack_version_gap_rejected` | Gap in version sequence | First/last-only checks |
| `attack_multi_batch_append` | Sequential appends to same stream | Version continuity across batches |
| `attack_empty_append` | Empty slice | Edge case no-op |
| `attack_append_creates_new_stream` | First append to non-existent stream | Implicit stream creation |
| `attack_evil_stream_ids_dont_crash` | SQL injection, null bytes, BOM, 10K IDs | Injection + encoding bugs |

### EventStream Contract

| Test | Real adapter value |
|------|---------------------|
| `attack_stream_versions_are_monotonic` | Missing ORDER BY, wrong index |
| `attack_stream_fused_after_none` | Cycling or stale data |
| `attack_read_stream_from_filters_correctly` | Off-by-one in WHERE / range scan |

### Stream Isolation

| Test | Real adapter value |
|------|---------------------|
| `attack_stream_isolation` | Key-prefix scan bleed (fjall) |
| `attack_stream_creation_order_independence` | Ordering-dependent behavior |

---

## Tier C — Integration Stress (5 tests)

Heavy hitters for CI nightly. Currently smoke tests.

| Test | What it attacks | When to run |
|------|-----------------|-------------|
| `attack_model_based_store_correctness` | Shadow model vs real store | Every adapter, nightly |
| `attack_concurrent_writers_exactly_one_wins` | 10 tasks racing same stream | Every adapter, nightly |
| `attack_concurrent_readers` | Parallel readers | Every adapter |
| `attack_stress_many_streams` | 5-20 streams × 10-50 events | Every adapter, nightly |
| `attack_idempotent_reads` | Read 3 times, compare | Every adapter |

---

## Beyond proptest: The Full Testing Spectrum

The 56 proptest tests above are just one tool. Below is the complete landscape of techniques that apply to nexus-store, ordered by implementation priority.

### Priority 1: State Machine Testing (proptest-state-machine)

**What:** Extends proptest with stateful testing. Define a reference state machine, generate random operation sequences, execute against real store, check postconditions after each step. Auto-shrinks failing sequences to minimal reproduction.

**Crates:** [`proptest-state-machine`](https://crates.io/crates/proptest-state-machine), [`proptest-stateful`](https://github.com/readysettech/proptest-stateful)

**Why it matters:** This is the single most powerful upgrade over current tests. The model-based test (`attack_model_based_store_correctness`) is a hand-rolled approximation — a proper state machine test is strictly more powerful because it:
- Generates longer, more complex operation sequences
- Checks postconditions after EVERY step, not just at the end
- Automatically shrinks failing sequences to the minimal reproduction
- Supports preconditions (e.g., "can only read a stream that exists")

**What bugs it finds:** Version arithmetic drift over many operations, lost writes, phantom reads, state divergence that only appears after specific operation orderings.

**Where to write it:**

```
crates/nexus-store/tests/
  state_machine_tests.rs
```

**What it should look like:**

```rust
// Reference model
struct StoreModel {
    streams: HashMap<String, Vec<(u64, String, Vec<u8>)>>,
}

// Operations
enum StoreOp {
    Append { stream_id: String, payloads: Vec<Vec<u8>> },
    Read { stream_id: String, from_version: u64 },
    ReadNonExistent { stream_id: String },
    AppendConflict { stream_id: String, wrong_version: u64, payloads: Vec<Vec<u8>> },
}

// For each op: execute against real store AND model, assert they agree
```

**Effort:** Medium. You already use proptest and have the model. ~200 lines.

---

### Priority 2: Mutation Testing (cargo-mutants)

**What:** Systematically replaces function bodies with wrong-but-compilable alternatives (return default, flip bool, delete side effect) and checks if tests catch it. If a mutant survives, you have a test gap.

**Tool:** [`cargo-mutants`](https://github.com/sourcefrog/cargo-mutants) (`cargo install cargo-mutants`)

**Why it matters:** Code coverage tells you what runs. Mutation testing tells you what is actually **verified by assertions**. It answers: "If `apply_upcasters` returned its input unchanged, would any test notice?"

**What bugs it finds:** Not bugs per se — **gaps in your test suite**. Which functions could silently return garbage without any test failing?

**How to run:**

```bash
# Full crate
cargo mutants -p nexus-store

# Just the core modules
cargo mutants -p nexus-store -- --file src/upcaster.rs --file src/event_store.rs

# Skip intentionally trivial functions
# Add #[mutants::skip] to functions that don't need testing
```

**Where:** No test file to write. Run it, read the report, add tests to cover surviving mutants.

**Effort:** Zero setup. Run and read.

---

### Priority 3: Miri — Undefined Behavior Detection

**What:** Interprets Rust's MIR to detect undefined behavior at runtime. Catches out-of-bounds access, use-after-free, invalid enum discriminants, alignment violations, data races, and aliasing violations (Stacked Borrows / Tree Borrows). A [POPL 2026 paper](https://plf.inf.ethz.ch/research/popl26-miri.html) describes it as "the first tool that can find all de-facto UB in deterministic Rust programs."

**Why it matters:** The zero-copy code path (`BorrowingCodec`, `PersistedEnvelope<'a>`) borrows from buffers — any aliasing violation or lifetime error that the compiler misses will be caught by Miri.

**What bugs it finds that others miss:** UB that is technically wrong but happens to work on your machine and CPU. Catches it before it becomes a production segfault on a different architecture.

**How to run:**

```bash
# Run all nexus-store tests under Miri
cargo +nightly miri test -p nexus-store

# Run specific tests (proptest is slow under Miri — use unit tests)
cargo +nightly miri test -p nexus-store --test envelope_tests
cargo +nightly miri test -p nexus-store --test error_tests

# With extra flags
MIRIFLAGS="-Zmiri-many-seeds=64" cargo +nightly miri test -p nexus-store
```

**Where:** No new tests needed. Run existing tests under Miri. Add a CI job.

**Gotchas:**
- 10-100x slower than normal execution. Don't run proptest with 512 cases under Miri.
- No FFI support — won't work with SQLite or other C-backed adapters.
- Requires nightly toolchain.

**Where to add in CI:**

```yaml
miri:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - run: rustup toolchain install nightly --component miri
    - run: cargo +nightly miri test -p nexus -p nexus-store --test envelope_tests --test error_tests --test stream_tests --test upcaster_tests
```

**Effort:** One CI job. No code changes.

---

### Priority 4: Kani Model Checker — Formal Proof

**What:** [Kani](https://github.com/model-checking/kani) compiles Rust to a SAT/SMT formula and exhaustively verifies all possible execution paths within bounds. You write "proof harnesses" with `kani::any()` for symbolic inputs. Kani **proves** properties hold for ALL inputs (up to bound), not just sampled ones.

**Why it matters:** proptest might sample 1024 versions and miss the one edge case. Kani proves it for all 2^64 versions (within the function's logic).

**What to prove:**
- `Version::next()` never silently wraps (always panics at MAX)
- `PersistedEnvelope` invariant: `schema_version > 0` is enforced on all construction paths
- `apply_upcasters` termination: chain limit fires before iteration count overflows
- Codec roundtrip: `decode(event_type, encode(event)) == event` for bounded events

**Where to write:**

```
crates/nexus-store/tests/
  kani_proofs.rs          ← proof harnesses, gated behind #[cfg(kani)]

crates/nexus/tests/
  kani_version_proofs.rs  ← version arithmetic proofs
```

**Example proof harness:**

```rust
#[cfg(kani)]
mod kani_proofs {
    use nexus::Version;

    #[kani::proof]
    fn version_next_never_wraps_silently() {
        let v: u64 = kani::any();
        let version = Version::from_persisted(v);
        if v < u64::MAX {
            let next = version.next();
            assert!(next.as_u64() == v + 1);
        }
        // At MAX, next() panics — Kani will verify this path too
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn upcaster_chain_always_terminates() {
        let schema_version: u32 = kani::any();
        kani::assume(schema_version >= 1);
        kani::assume(schema_version <= 100);  // bound for tractability
        // ... prove apply_upcasters terminates
    }
}
```

**How to run:**

```bash
cargo install --locked kani-verifier
cargo kani --tests -p nexus-store
```

**Gotchas:**
- Verification time grows exponentially with input bounds. Keep loops bounded.
- No async support. Test sync functions only.
- Can't handle all trait objects / dynamic dispatch.

**Effort:** Medium. One file per crate, 5-10 proof harnesses.

---

### Priority 5: Design by Contract (contracts crate)

**What:** The [`contracts`](https://crates.io/crates/contracts) crate provides `#[requires(...)]`, `#[ensures(...)]`, and `#[invariant(...)]` proc macros. Preconditions check inputs, postconditions check outputs, invariants check struct state. Uses `debug_assert!` so zero cost in release builds.

**Why it matters:** Contracts live next to the code, not in a separate test file. Every call site automatically checks them. They're documentation that executes.

**What to annotate:**

```rust
// In envelope.rs
#[ensures(ret.schema_version() > 0)]
pub const fn new(..., schema_version: u32, ...) -> Self { ... }

// In upcaster.rs — apply_upcasters
#[ensures(ret.is_ok() -> ret.unwrap().1 >= schema_version)]
pub fn apply_upcasters(...) -> Result<(String, u32, Vec<u8>), UpcastError> { ... }

// In testing.rs — InMemoryStore::append
#[requires(envelopes.iter().enumerate().all(|(i, e)| e.version().as_u64() == expected_version.as_u64() + 1 + i as u64))]
async fn append(&self, ...) -> Result<(), Self::Error> { ... }
```

**Where to add:** Directly in `src/` files. The contracts become part of the code.

**Effort:** Low. Add `contracts = "0.6"` to deps, annotate ~10 key functions.

---

### Priority 6: Coverage-Guided Fuzzing (Bolero)

**What:** [Bolero](https://github.com/camshaft/bolero) unifies proptest, libfuzzer, AFL, and Kani behind one test harness. The key upgrade over proptest: coverage-guided fuzzers learn from code coverage to explore deeper paths, rather than random sampling.

**Why it matters for codecs:** A random byte array has almost zero chance of being valid JSON/bincode/postcard. A coverage-guided fuzzer will learn the format structure and generate _almost-valid_ inputs that exercise error paths, partial parse states, and edge cases.

**Where to write:**

```
crates/nexus-store/tests/
  fuzz_codec.rs           ← fuzz decode() with arbitrary bytes
  fuzz_upcaster.rs        ← fuzz upcaster chains with arbitrary payloads
  fuzz_envelope.rs        ← fuzz PersistedEnvelope construction
```

**Example:**

```rust
use bolero::check;
use nexus_store::codec::Codec;

#[test]
fn fuzz_codec_decode_never_panics() {
    let codec = JsonCodec;
    check!()
        .with_type::<(String, Vec<u8>)>()
        .for_each(|(event_type, payload)| {
            // Must not panic — may return Err, that's fine
            let _ = codec.decode(event_type, payload);
        });
}

#[test]
fn fuzz_codec_roundtrip() {
    let codec = JsonCodec;
    check!()
        .with_type::<TestEvent>()
        .for_each(|event| {
            let encoded = codec.encode(event).unwrap();
            let decoded = codec.decode(event.name(), &encoded).unwrap();
            assert_eq!(&decoded, event);
        });
}
```

**How to run:**

```bash
cargo install cargo-bolero
# proptest backend (quick)
cargo bolero test -p nexus-store fuzz_codec_decode_never_panics
# libfuzzer backend (deep, needs nightly + Linux)
cargo +nightly bolero test -p nexus-store fuzz_codec_decode_never_panics --engine libfuzzer
```

**Effort:** Low-medium. One crate dep, ~3 test files.

---

### Priority 7: cargo-careful — Cheap UB Checks

**What:** [cargo-careful](https://github.com/RalfJung/cargo-careful) rebuilds the standard library with debug assertions enabled. Catches `mem::zeroed()` misuse, `get_unchecked` bounds violations, and `NonNull` validity — all things the std lib normally silently accepts.

**Why it matters:** Faster than Miri (2x slowdown vs 100x), FFI-compatible, catches a subset of UB with almost zero effort.

**How to run:**

```bash
cargo +nightly careful test -p nexus-store
```

**Where in CI:**

```yaml
careful:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - run: rustup toolchain install nightly
    - run: cargo install cargo-careful
    - run: cargo +nightly careful test --all
```

**Effort:** One command. One CI job.

---

### Priority 8: Snapshot Testing (insta)

**What:** [insta](https://insta.rs/) captures a "snapshot" of serialized output and fails if it changes unexpectedly. You review and accept changes explicitly.

**Why it matters for codecs:** If a codec's serialized format changes, every persisted event in every database becomes unreadable. Snapshot tests are the tripwire.

**Where to write:**

```
crates/nexus-store/tests/
  snapshot_tests.rs
```

**Example:**

```rust
use insta::assert_snapshot;

#[test]
fn snapshot_happened_event_json() {
    let codec = JsonCodec;
    let encoded = codec.encode(&TestEvent::Happened("hello".into())).unwrap();
    assert_snapshot!(String::from_utf8(encoded).unwrap());
}

#[test]
fn snapshot_store_error_display() {
    let err = StoreError::Conflict {
        stream_id: ErrorId::from_display(&"order-123"),
        expected: Version::from_persisted(5),
        actual: Version::from_persisted(8),
    };
    assert_snapshot!(err.to_string());
}
```

**Effort:** Very low. Add `insta` to dev-deps, write ~5 snapshot tests.

---

### Priority 9: Concurrency Testing (Shuttle / Loom)

**When:** When you add concurrent access beyond the current Mutex.

**Shuttle** (randomized scheduling): [`shuttle`](https://github.com/awslabs/shuttle) — scales to complex scenarios, runs many random schedules. Best for integration-level concurrency tests.

**Loom** (exhaustive interleaving): [`loom`](https://github.com/tokio-rs/loom) — proves correctness but only for tiny test cases (2-4 threads, few sync operations). Best for lock-free primitives or custom synchronization.

**Where to write:**

```
crates/nexus-store/tests/
  concurrency_shuttle.rs   ← Shuttle: randomized concurrent append/read
  concurrency_loom.rs      ← Loom: exhaustive check of lock ordering (if applicable)
```

**What to test:**
- Concurrent appends to same stream: exactly one wins
- Concurrent append + read: reader sees consistent snapshot
- Lock ordering: no deadlock possible

**When:** Not yet. These become critical when:
- You implement a lock-free adapter
- You add cross-stream transactions
- You build a concurrent read cache

**Effort:** Medium-high. Requires replacing sync primitives with shuttle/loom equivalents.

---

### Priority 10: Refinement Types (Flux)

**What:** [Flux](https://github.com/flux-rs/flux) adds liquid types (refinement types) to Rust. Attach numeric predicates to types — the compiler proves them statically. Found multiple bugs in Tock OS's process isolation.

**Why it matters:** Instead of _testing_ that schema_version > 0, you _prove_ it at compile time:

```rust
#[flux::sig(fn(schema_version: u32{v: v > 0}) -> Self)]
pub fn new(..., schema_version: u32, ...) -> Self { ... }
```

Now any call site passing 0 is a **compile error**, not a runtime panic.

**What to annotate:**
- `Version` arithmetic: prove no overflow in version ranges used by stores
- `PersistedEnvelope::new`: prove schema_version > 0 statically
- `apply_upcasters`: prove output version > input version

**Where:** Directly in `src/` files, as annotations. Requires Flux compiler plugin.

**Effort:** High. Experimental tool, requires nightly, annotation burden. But the payoff is mathematical certainty.

---

### Deterministic Simulation Testing (DST) — For Adapters

**What:** Replace all I/O with controllable mocks driven by a PRNG seed. Any failure is exactly reproducible by replaying the seed. Pioneered by FoundationDB. Used by TigerBeetle, S2, Polar Signals.

**Tools:**
- [`turmoil`](https://github.com/tokio-rs/turmoil) — Tokio's sim framework. Network + time simulation. File system simulation (unstable).
- [`madsim`](https://github.com/madsim-rs/madsim) — Drop-in tokio replacement. Overrides libc for deterministic time/entropy. Used by RisingWave.
- [Antithesis](https://antithesis.com/) — Commercial deterministic hypervisor. Free for open source.

**When:** When you have a real adapter (fjall, SQLite) and want to test failure scenarios:
- Kill process mid-append, restart, verify consistency
- Inject disk I/O errors, verify clean error propagation
- Simulate network partitions (for distributed stores)
- Replay any failure with the exact same seed

**Where to write:**

```
crates/nexus-fjall/tests/
  simulation/
    mod.rs              ← DST harness with turmoil or madsim
    crash_recovery.rs   ← kill mid-append, verify stream
    io_errors.rs        ← inject disk failures
    long_run.rs         ← simulate millions of operations
```

**Effort:** High. Requires adapting I/O layer to use simulated types. But for a database adapter, this is the gold standard.

---

### Linearizability Testing (Stateright) — For Distributed Stores

**What:** [Stateright](https://github.com/stateright/stateright) exhaustively explores all possible message orderings in a distributed system and checks linearizability — whether the system behaves as if operations were executed sequentially.

**When:** Only if nexus ever supports multi-node event stores.

**Where:** A separate `nexus-distributed-tests` crate that models the store as an actor system.

**Effort:** Very high. Only relevant for distributed architectures.

---

### TigerBeetle VOPR-Style Hash Validation

**What:** Hash the aggregate state after every replay step. Run the same event sequence through two independent code paths. Compare hashes. Any divergence = determinism bug.

**Why it matters for event sourcing specifically:** Event replay MUST be deterministic. If `replay(events)` produces different state on different runs (due to HashMap iteration order, floating point, time-dependent logic), your store is silently corrupt.

**Where to write:**

```
crates/nexus-store/tests/
  determinism_tests.rs
```

**Example:**

```rust
#[test]
fn replay_is_deterministic_across_runs() {
    let events = generate_random_events(seed);
    let state1 = replay_all(events.clone());
    let state2 = replay_all(events);
    assert_eq!(hash(state1), hash(state2));
}
```

**Effort:** Low. ~50 lines. But only catches nondeterminism bugs.

---

## Running Everything

### Current (pre-adapter)

```bash
# Tier A+B+C proptest (fast, ~0.6s)
cargo test -p nexus-store --test nasa_break_everything

# Miri (slow, catches UB)
cargo +nightly miri test -p nexus -p nexus-store --test envelope_tests --test error_tests

# cargo-careful (medium speed, catches std lib misuse)
cargo +nightly careful test -p nexus-store

# Mutation testing (slow, finds test gaps)
cargo mutants -p nexus-store
```

### CI Configuration

```yaml
# Every PR — fast checks
pr-checks:
  - cargo test -p nexus-store --test nasa_break_everything
  - cargo test -p nexus-store  # all other tests

# Every PR — UB detection
miri:
  - cargo +nightly miri test -p nexus -p nexus-store --test envelope_tests --test error_tests --test upcaster_tests

# Every PR — std lib assertions
careful:
  - cargo +nightly careful test --all

# Weekly — mutation testing
mutants:
  - cargo mutants -p nexus-store

# Nightly — extended proptest
nightly:
  - PROPTEST_CASES=2048 cargo test -p nexus-store --test nasa_break_everything
  # + per-adapter conformance when adapters exist

# Per-adapter (when fjall/sqlite land)
adapter:
  - cargo test -p nexus-fjall --test conformance_tests
  - PROPTEST_CASES=2048 cargo test -p nexus-fjall --test conformance_tests
```

### Adapter Conformance Suite Layout

When an adapter lands:

```
crates/nexus-store/tests/
  conformance/
    mod.rs              ← generic fn test_X<S: RawEventStore>(store: &S)
    raw_store.rs        ← Tier B tests as generic functions
    stream.rs           ← EventStream contract tests
    stress.rs           ← Tier C concurrent + model-based

crates/nexus-fjall/tests/
  conformance_tests.rs  ← conformance::test_X(FjallStore::new(...))

crates/nexus-store/tests/
  state_machine_tests.rs  ← proptest-state-machine (Priority 1)
  fuzz_codec.rs            ← Bolero fuzzing (Priority 6)
  snapshot_tests.rs        ← insta snapshots (Priority 8)
  determinism_tests.rs     ← VOPR-style hash checks
  kani_proofs.rs           ← formal proofs (#[cfg(kani)])
```

---

## Implementation Priority Matrix

| # | Technique | Effort | Value now | Value with adapter | File to create |
|---|-----------|--------|-----------|--------------------|-|
| 1 | proptest-state-machine | Medium | **High** | **Critical** | `state_machine_tests.rs` |
| 2 | cargo-mutants | Zero | **High** | High | _(run, no file)_ |
| 3 | Miri | Zero | **High** | High | _(CI job)_ |
| 4 | Kani proofs | Medium | High | High | `kani_proofs.rs` |
| 5 | contracts crate | Low | Medium | Medium | _(annotations in src/)_ |
| 6 | Bolero fuzzing | Low-med | Medium | **High** | `fuzz_codec.rs` |
| 7 | cargo-careful | Zero | Medium | Medium | _(CI job)_ |
| 8 | insta snapshots | Very low | Medium | **High** | `snapshot_tests.rs` |
| 9 | Shuttle/Loom | Med-high | Low | **Critical** | `concurrency_shuttle.rs` |
| 10 | Flux refinement types | High | Low | Medium | _(annotations in src/)_ |
| 11 | Deterministic sim (turmoil) | High | None | **Critical** | `simulation/*.rs` |
| 12 | Stateright linearizability | Very high | None | Niche | _(separate crate)_ |
| 13 | VOPR hash validation | Low | Medium | High | `determinism_tests.rs` |

---

## Test Count Summary

| Category | Count | Status |
|----------|-------|--------|
| proptest (nasa_break_everything.rs) | 56 | Done |
| State machine tests | ~10 | To write |
| Kani proof harnesses | ~8 | To write |
| Bolero fuzz targets | ~3 | To write |
| Snapshot tests | ~5 | To write |
| Determinism hash tests | ~3 | To write |
| Contracts annotations | ~10 | To annotate |
| Shuttle/Loom concurrency | ~5 | When needed |
| **CI-only (no code)** | Miri, careful, mutants | To configure |

Total when complete: ~100 test points + 3 CI analysis tools.
