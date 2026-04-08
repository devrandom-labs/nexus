# Unified Event Pipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the broken upcaster system and save path with a morsel-based event pipeline using declarative `SchemaTransform`, pluggable `SchemaRegistry`, and HList chaining — with optional tower `Service` impls.

**Architecture:** Read and write paths are pipelines of `Cow`-based transformations over `EventMorsel`. `SchemaTransform` declares identity (event_type, from/to version) and provides a pure transform. `TransformChain` (HList) matches and applies. Validation is standalone functions. Tower `Service` impls are feature-gated thin adapters.

**Tech Stack:** Rust edition 2024, `no_std + alloc` core, `tower-service` (optional), `nexus::Version` for all version types.

**Design doc:** `docs/plans/2026-04-06-unified-event-pipeline-design.md`

---

## File structure (new → old mapping)

```
crates/nexus-store/src/
  morsel.rs          NEW — EventMorsel type
  schema_transform.rs NEW — SchemaTransform trait (replaces upcaster.rs trait)
  schema_registry.rs  NEW — SchemaRegistry trait + ChainDerivedRegistry
  transform_chain.rs  NEW — TransformChain trait + Chain + HList impls (replaces upcaster_chain.rs)
  pipeline.rs         NEW — validation functions + pipeline loop
  tower_service.rs    NEW — #[cfg(feature = "tower")] Service impls
  repository.rs       MODIFY — save signature change
  event_store.rs      MODIFY — use pipeline, remove save_with_encoder/run_upcasters
  envelope.rs         UNCHANGED
  codec.rs            UNCHANGED
  borrowing_codec.rs  UNCHANGED
  raw.rs              UNCHANGED
  stream.rs           UNCHANGED
  stream_label.rs     UNCHANGED
  error.rs            MODIFY — update UpcastError field types if needed
  testing.rs          UNCHANGED (InMemoryStore doesn't use upcasters)
  lib.rs              MODIFY — new module declarations + re-exports

  upcaster.rs         DELETE (replaced by schema_transform.rs + pipeline.rs)
  upcaster_chain.rs   DELETE (replaced by transform_chain.rs)
```

Kernel (unfreeze for one method):
```
crates/nexus/src/aggregate.rs  MODIFY — add apply_event method
```

---

## Phase 1: Kernel unfreeze + new core types (no breaking changes yet)

### Task 1: Add `apply_event` to kernel

**Files:**
- Modify: `crates/nexus/src/aggregate.rs` (add method to `AggregateRoot`)
- Test: `crates/nexus/tests/kernel_tests/aggregate_root_tests.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn apply_event_updates_state() {
    let mut root = AggregateRoot::<TestAggregate>::new(TestId(1));
    root.apply_event(&TestEvent::Incremented);
    assert_eq!(root.state().value, 1);
    root.apply_event(&TestEvent::Incremented);
    assert_eq!(root.state().value, 2);
}
```

Use whatever `TestAggregate` / `TestEvent` types exist in the test file. Check the existing test fixtures.

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus -- apply_event_updates_state`
Expected: FAIL — method `apply_event` not found

**Step 3: Implement — add to `AggregateRoot`**

In `crates/nexus/src/aggregate.rs`, add to `impl<A: Aggregate> AggregateRoot<A>`:

```rust
/// Apply a single event to the aggregate state.
///
/// Called by the repository after successful persistence to keep
/// the in-memory state in sync. Clone-based for panic safety.
pub fn apply_event(&mut self, event: &EventOf<A>) {
    let new_state = self.state.clone().apply(event);
    self.state = new_state;
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus -- apply_event_updates_state`
Expected: PASS

**Step 5: Re-freeze kernel**

Verify no other kernel files were touched. Commit.

Run: `cargo test -p nexus`
Expected: All kernel tests pass.

**Step 6: Commit**

```
feat(kernel): add AggregateRoot::apply_event for single-event state advancement
```

---

### Task 2: Create `EventMorsel`

**Files:**
- Create: `crates/nexus-store/src/morsel.rs`
- Modify: `crates/nexus-store/src/lib.rs` (add module)
- Test: `crates/nexus-store/tests/morsel_tests.rs`

**Step 1: Write the failing test**

```rust
use nexus::Version;
use nexus_store::morsel::EventMorsel;
use std::borrow::Cow;

#[test]
fn morsel_from_borrowed_is_zero_alloc() {
    let event_type = "OrderCreated";
    let payload = b"some bytes";
    let morsel = EventMorsel::borrowed(event_type, Version::INITIAL, payload);
    assert_eq!(morsel.event_type(), "OrderCreated");
    assert_eq!(morsel.schema_version(), Version::INITIAL);
    assert_eq!(morsel.payload(), b"some bytes");
    assert!(morsel.is_borrowed());
}

#[test]
fn morsel_with_transformed_payload() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"old");
    let morsel = morsel.with_payload(Cow::Owned(b"new".to_vec()));
    assert_eq!(morsel.payload(), b"new");
    assert!(!morsel.is_borrowed());
}

#[test]
fn morsel_with_advanced_version() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let v2 = Version::new(2).unwrap();
    let morsel = morsel.with_schema_version(v2);
    assert_eq!(morsel.schema_version(), v2);
}

#[test]
fn morsel_with_renamed_event_type() {
    let morsel = EventMorsel::borrowed("OldName", Version::INITIAL, b"data");
    let morsel = morsel.with_event_type(Cow::Owned("NewName".to_owned()));
    assert_eq!(morsel.event_type(), "NewName");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store --test morsel_tests`
Expected: FAIL — module `morsel` not found

**Step 3: Implement `EventMorsel`**

Create `crates/nexus-store/src/morsel.rs`:

```rust
use std::borrow::Cow;
use nexus::Version;

/// A single event as it flows through the transform pipeline.
///
/// Borrows from the cursor buffer when no transform has fired (zero-copy).
/// Becomes owned after the first transform allocates.
///
/// Inspired by Polars' Morsel pattern: data + metadata wrapped in a
/// single type that flows through each pipeline step.
pub struct EventMorsel<'a> {
    event_type: Cow<'a, str>,
    schema_version: Version,
    payload: Cow<'a, [u8]>,
}

impl<'a> EventMorsel<'a> {
    /// Create a morsel borrowing from existing data (zero allocation).
    pub fn borrowed(event_type: &'a str, schema_version: Version, payload: &'a [u8]) -> Self {
        Self {
            event_type: Cow::Borrowed(event_type),
            schema_version,
            payload: Cow::Borrowed(payload),
        }
    }

    pub fn event_type(&self) -> &str {
        &self.event_type
    }

    pub fn schema_version(&self) -> Version {
        self.schema_version
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// True if all fields still borrow from the original source.
    pub fn is_borrowed(&self) -> bool {
        matches!((&self.event_type, &self.payload), (Cow::Borrowed(_), Cow::Borrowed(_)))
    }

    /// Return a new morsel with a different payload.
    pub fn with_payload(self, payload: Cow<'a, [u8]>) -> Self {
        Self { payload, ..self }
    }

    /// Return a new morsel with a different schema version.
    pub fn with_schema_version(self, schema_version: Version) -> Self {
        Self { schema_version, ..self }
    }

    /// Return a new morsel with a different event type.
    pub fn with_event_type(self, event_type: Cow<'a, str>) -> Self {
        Self { event_type, ..self }
    }
}
```

Add `pub mod morsel;` to `lib.rs` and `pub use morsel::EventMorsel;`.

**Step 4: Run tests**

Run: `cargo test -p nexus-store --test morsel_tests`
Expected: PASS

**Step 5: Commit**

```
feat(store): add EventMorsel — Cow-based data unit for the transform pipeline
```

---

### Task 3: Create `SchemaTransform` trait

**Files:**
- Create: `crates/nexus-store/src/schema_transform.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/schema_transform_tests.rs`

**Step 1: Write failing test**

```rust
use nexus::Version;
use nexus_store::schema_transform::SchemaTransform;
use std::borrow::Cow;

struct V1ToV2;

impl SchemaTransform for V1ToV2 {
    fn event_type(&self) -> &str { "OrderCreated" }
    fn from_version(&self) -> Version { Version::INITIAL }
    fn to_version(&self) -> Version { Version::new(2).unwrap() }
    fn transform<'a>(&self, payload: &'a [u8]) -> Cow<'a, [u8]> {
        let mut data = payload.to_vec();
        data.extend_from_slice(b",v2");
        Cow::Owned(data)
    }
}

#[test]
fn schema_transform_declarative_identity() {
    let t = V1ToV2;
    assert_eq!(t.event_type(), "OrderCreated");
    assert_eq!(t.from_version(), Version::INITIAL);
    assert_eq!(t.to_version(), Version::new(2).unwrap());
    assert_eq!(t.new_event_type(), None); // default: no rename
}

#[test]
fn schema_transform_transforms_payload() {
    let t = V1ToV2;
    let result = t.transform(b"hello");
    assert_eq!(&*result, b"hello,v2");
}
```

**Step 2:** Run, verify fail (trait not found).

**Step 3: Implement**

Create `crates/nexus-store/src/schema_transform.rs`:

```rust
use std::borrow::Cow;
use nexus::Version;

/// Declarative schema migration.
///
/// Declares WHAT it does: which event type, from which version, to which
/// version. Provides a pure transformation on the payload bytes. The
/// pipeline handles matching, chaining, and validation.
///
/// Stateless (`&self`), composable via HList chain.
pub trait SchemaTransform: Send + Sync {
    /// The event type this transform handles.
    fn event_type(&self) -> &str;

    /// Schema version this transform upgrades FROM.
    fn from_version(&self) -> Version;

    /// Schema version this transform upgrades TO.
    fn to_version(&self) -> Version;

    /// Transform the payload bytes.
    ///
    /// Use `Cow::Borrowed(payload)` if the payload doesn't change
    /// (e.g., only the version advances). Use `Cow::Owned(...)` when
    /// the bytes are actually modified.
    fn transform<'a>(&self, payload: &'a [u8]) -> Cow<'a, [u8]>;

    /// If this transform also renames the event type, return the new name.
    /// Default: `None` (event type unchanged).
    fn new_event_type(&self) -> Option<&str> {
        None
    }
}
```

Add `pub mod schema_transform;` to `lib.rs` and `pub use schema_transform::SchemaTransform;`.

**Step 4:** Run tests, verify pass.

**Step 5: Commit**

```
feat(store): add SchemaTransform trait — declarative schema migration
```

---

### Task 4: Create `TransformChain` (HList)

**Files:**
- Create: `crates/nexus-store/src/transform_chain.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/transform_chain_tests.rs`

**Step 1: Write failing tests**

```rust
use nexus::Version;
use nexus_store::morsel::EventMorsel;
use nexus_store::transform_chain::{Chain, TransformChain};
use nexus_store::schema_transform::SchemaTransform;
use std::borrow::Cow;

// Test fixtures: V1ToV2 and V2ToV3 transforms (same as task 3)

#[test]
fn empty_chain_passes_through() {
    let chain = ();
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = chain.apply(morsel);
    assert_eq!(result.schema_version(), Version::INITIAL);
    assert!(result.is_borrowed());
}

#[test]
fn chain_applies_matching_transform() {
    let chain = Chain::new(V1ToV2, ());
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = chain.apply(morsel);
    assert_eq!(result.schema_version(), Version::new(2).unwrap());
}

#[test]
fn chain_skips_non_matching() {
    let chain = Chain::new(V1ToV2, ());
    let morsel = EventMorsel::borrowed("OtherEvent", Version::INITIAL, b"data");
    let result = chain.apply(morsel);
    assert_eq!(result.schema_version(), Version::INITIAL); // unchanged
}

#[test]
fn chain_can_transform_reports_correctly() {
    let chain = Chain::new(V1ToV2, Chain::new(V2ToV3, ()));
    assert!(chain.can_transform("OrderCreated", Version::INITIAL));
    assert!(chain.can_transform("OrderCreated", Version::new(2).unwrap()));
    assert!(!chain.can_transform("OrderCreated", Version::new(3).unwrap()));
    assert!(!chain.can_transform("Unknown", Version::INITIAL));
}
```

**Step 2:** Run, verify fail.

**Step 3: Implement**

Create `crates/nexus-store/src/transform_chain.rs`:

```rust
use std::borrow::Cow;
use nexus::Version;
use crate::morsel::EventMorsel;
use crate::schema_transform::SchemaTransform;

/// A link in the compile-time transform chain (HList).
///
/// Fully monomorphized — zero-size when transforms are stateless.
/// Constructed via `EventStore::with_transform`, not directly.
pub struct Chain<H, T>(pub(crate) H, pub(crate) T);

impl<H, T> Chain<H, T> {
    pub(crate) fn new(head: H, tail: T) -> Self {
        Self(head, tail)
    }
}

/// Compile-time transform chain. Finds the first matching transform
/// and applies it to the morsel.
pub trait TransformChain: Send + Sync {
    /// Apply the first matching transform. Returns morsel unchanged
    /// if no transform matches.
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> EventMorsel<'a>;

    /// Check if any transform handles this event type + version.
    fn can_transform(&self, event_type: &str, schema_version: Version) -> bool;
}

impl TransformChain for () {
    #[inline]
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> EventMorsel<'a> {
        morsel
    }

    #[inline]
    fn can_transform(&self, _: &str, _: Version) -> bool {
        false
    }
}

impl<H: SchemaTransform, T: TransformChain> TransformChain for Chain<H, T> {
    #[inline]
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> EventMorsel<'a> {
        if morsel.event_type() == self.0.event_type()
            && morsel.schema_version() == self.0.from_version()
        {
            let new_payload = self.0.transform(morsel.payload());
            let morsel = morsel
                .with_payload(new_payload)
                .with_schema_version(self.0.to_version());
            match self.0.new_event_type() {
                Some(name) => morsel.with_event_type(Cow::Borrowed(name)),
                None => morsel,
            }
        } else {
            self.1.apply(morsel)
        }
    }

    #[inline]
    fn can_transform(&self, event_type: &str, schema_version: Version) -> bool {
        (self.0.event_type() == event_type && self.0.from_version() == schema_version)
            || self.1.can_transform(event_type, schema_version)
    }
}
```

Add `pub mod transform_chain;` to `lib.rs` and re-exports.

**Step 4:** Run tests, verify pass.

**Step 5: Commit**

```
feat(store): add TransformChain — HList-based schema transform matching
```

---

### Task 5: Create `SchemaRegistry` trait + `ChainDerivedRegistry`

**Files:**
- Create: `crates/nexus-store/src/schema_registry.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/schema_registry_tests.rs`

**Step 1: Write failing tests**

```rust
use nexus::Version;
use nexus_store::schema_registry::{SchemaRegistry, ChainDerivedRegistry};
use nexus_store::transform_chain::Chain;
// reuse V1ToV2, V2ToV3 fixtures

#[test]
fn chain_derived_registry_walks_versions() {
    let chain = Chain::new(V1ToV2, Chain::new(V2ToV3, ()));
    let registry = ChainDerivedRegistry::new(&chain);
    assert_eq!(registry.current_version("OrderCreated"), Some(Version::new(3).unwrap()));
}

#[test]
fn chain_derived_registry_unknown_event() {
    let chain = Chain::new(V1ToV2, ());
    let registry = ChainDerivedRegistry::new(&chain);
    assert_eq!(registry.current_version("Unknown"), None);
}

#[test]
fn chain_derived_registry_no_transforms() {
    let chain = ();
    let registry = ChainDerivedRegistry::new(&chain);
    // No transforms → version 1 is current (nothing to advance from)
    assert_eq!(registry.current_version("OrderCreated"), None);
}
```

**Step 2:** Run, verify fail.

**Step 3: Implement**

Create `crates/nexus-store/src/schema_registry.rs`:

```rust
use nexus::Version;
use crate::transform_chain::TransformChain;

/// Pluggable schema version lookup.
///
/// Default impl: `ChainDerivedRegistry` — derives the current version
/// by walking the transform chain. Future impls: Confluent, Apicurio, etc.
pub trait SchemaRegistry: Send + Sync {
    /// The current schema version for an event type.
    /// Returns `None` if the event type is unknown.
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

/// Derives the current schema version by probing the transform chain.
///
/// Walks versions from 1 upward until no transform matches.
/// The first version with no matching transform is "current."
pub struct ChainDerivedRegistry<'a, C> {
    chain: &'a C,
}

impl<'a, C> ChainDerivedRegistry<'a, C> {
    pub fn new(chain: &'a C) -> Self {
        Self { chain }
    }
}

impl<C: TransformChain> SchemaRegistry for ChainDerivedRegistry<'_, C> {
    fn current_version(&self, event_type: &str) -> Option<Version> {
        let mut version = Version::INITIAL;
        if !self.chain.can_transform(event_type, version) {
            // No transform handles version 1 → event type unknown
            // (or already at version 1 which is current)
            return None;
        }
        loop {
            let Some(next) = version.next() else {
                return Some(version); // overflow — return last known
            };
            if !self.chain.can_transform(event_type, next) {
                return Some(next);
            }
            version = next;
        }
    }
}
```

Note: the `current_version` logic needs refinement — see design doc for iteration limit. Add a `DEFAULT_CHAIN_LIMIT` check in the loop. Refer to the existing `current_schema_version` in `event_store.rs` for the limit logic.

**Step 4:** Run tests, verify pass.

**Step 5: Commit**

```
feat(store): add SchemaRegistry trait + ChainDerivedRegistry
```

---

### Task 6: Create pipeline validation + loop

**Files:**
- Create: `crates/nexus-store/src/pipeline.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/pipeline_tests.rs`

**Step 1: Write failing tests**

```rust
use nexus::Version;
use nexus_store::morsel::EventMorsel;
use nexus_store::pipeline::run_transform_pipeline;
use nexus_store::transform_chain::Chain;
// reuse V1ToV2, V2ToV3 fixtures

#[test]
fn pipeline_no_transform_zero_alloc() {
    let chain = ();
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = run_transform_pipeline(&chain, morsel).unwrap();
    assert!(result.is_borrowed()); // zero-alloc pass-through
}

#[test]
fn pipeline_single_transform() {
    let chain = Chain::new(V1ToV2, ());
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = run_transform_pipeline(&chain, morsel).unwrap();
    assert_eq!(result.schema_version(), Version::new(2).unwrap());
}

#[test]
fn pipeline_chains_multiple_transforms() {
    let chain = Chain::new(V1ToV2, Chain::new(V2ToV3, ()));
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = run_transform_pipeline(&chain, morsel).unwrap();
    assert_eq!(result.schema_version(), Version::new(3).unwrap());
}

#[test]
fn pipeline_rejects_non_advancing_version() {
    // BadTransform: from_version=1, to_version=1 (doesn't advance)
    let chain = Chain::new(BadTransform, ());
    let morsel = EventMorsel::borrowed("Test", Version::INITIAL, b"data");
    let result = run_transform_pipeline(&chain, morsel);
    assert!(result.is_err());
}
```

**Step 2:** Run, verify fail.

**Step 3: Implement**

Create `crates/nexus-store/src/pipeline.rs`:

```rust
use nexus::Version;
use crate::error::UpcastError;
use crate::morsel::EventMorsel;
use crate::transform_chain::TransformChain;

const DEFAULT_CHAIN_LIMIT: u64 = 100;

/// Validate that a transform advanced the schema version.
fn check_version_advanced(
    event_type: &str,
    prev: Version,
    next: Version,
) -> Result<(), UpcastError> {
    if next <= prev {
        return Err(UpcastError::VersionNotAdvanced {
            event_type: event_type.to_owned(),
            input_version: prev,
            output_version: next,
        });
    }
    Ok(())
}

/// Validate event type is non-empty.
fn check_event_type_nonempty(
    new_type: &str,
    prev_type: &str,
    prev_version: Version,
) -> Result<(), UpcastError> {
    if new_type.is_empty() {
        return Err(UpcastError::EmptyEventType {
            input_event_type: prev_type.to_owned(),
            schema_version: prev_version,
        });
    }
    Ok(())
}

/// Run the morsel through the transform chain until no transform matches.
///
/// Zero allocation when no transform fires (common case).
/// Validates each step: version must advance, type must be non-empty,
/// iteration count bounded.
pub fn run_transform_pipeline<'a, C: TransformChain>(
    chain: &C,
    mut morsel: EventMorsel<'a>,
) -> Result<EventMorsel<'a>, UpcastError> {
    let mut iterations = 0u64;

    loop {
        let prev_version = morsel.schema_version();
        let prev_type = morsel.event_type().to_owned(); // only allocates if we validate
        morsel = chain.apply(morsel);

        if morsel.schema_version() == prev_version {
            break; // no transform fired
        }

        // A transform fired — validate
        iterations = iterations.checked_add(1).ok_or_else(|| {
            UpcastError::ChainLimitExceeded {
                event_type: morsel.event_type().to_owned(),
                schema_version: morsel.schema_version(),
                limit: DEFAULT_CHAIN_LIMIT,
            }
        })?;
        if iterations > DEFAULT_CHAIN_LIMIT {
            return Err(UpcastError::ChainLimitExceeded {
                event_type: morsel.event_type().to_owned(),
                schema_version: morsel.schema_version(),
                limit: DEFAULT_CHAIN_LIMIT,
            });
        }
        check_version_advanced(morsel.event_type(), prev_version, morsel.schema_version())?;
        check_event_type_nonempty(morsel.event_type(), &prev_type, prev_version)?;
    }

    Ok(morsel)
}
```

Note: the `prev_type` allocation only happens AFTER a transform fires (inside the `if` block). When no transform fires, the loop breaks on the first iteration — zero allocation.

Wait — actually `prev_type` is allocated before `chain.apply()`. Move the allocation after the version check. Restructure:

```rust
loop {
    let prev_version = morsel.schema_version();
    morsel = chain.apply(morsel);

    if morsel.schema_version() == prev_version {
        break; // no transform fired, zero-alloc
    }

    // Transform fired — now allocate for validation context
    iterations = iterations.checked_add(1)...;
    check_version_advanced(...)?;
    check_event_type_nonempty(...)?;
}
```

The `check_event_type_nonempty` needs `prev_type` but we moved past it. Store it differently — the previous event_type is only needed for the error message. We can reconstruct it from the morsel before apply. Actually, the simplest fix: only allocate the prev_type string inside the error construction, not before the check. The check itself only needs the NEW type to verify non-emptiness.

Revise `check_event_type_nonempty` to only check the new type:

```rust
fn check_event_type_nonempty(new_type: &str, prev_version: Version) -> Result<(), UpcastError> {
    if new_type.is_empty() {
        return Err(UpcastError::EmptyEventType {
            input_event_type: String::from("<unknown after transform>"),
            schema_version: prev_version,
        });
    }
    Ok(())
}
```

Or better: track prev_type only when a transform fires. See implementation for details.

**Step 4:** Run tests, verify pass.

**Step 5: Commit**

```
feat(store): add pipeline — morsel-based transform loop with validation
```

---

## Phase 2: Wire up EventStore (breaking changes)

### Task 7: Update `Repository` trait signature

**Files:**
- Modify: `crates/nexus-store/src/repository.rs`
- Modify: `crates/nexus-store/src/lib.rs`

**Step 1:** Update `Repository::save` signature:

```rust
fn save(
    &self,
    aggregate: &mut AggregateRoot<A>,
    events: &[EventOf<A>],
) -> impl Future<Output = Result<(), Self::Error>> + Send;
```

Update the doc comments to reflect the new contract (events passed in, not drained from aggregate).

**Step 2:** Run `cargo check -p nexus-store` — will fail on EventStore/ZeroCopyEventStore impls. That's expected; we fix them next.

**Step 3: Commit**

```
feat(store)!: Repository::save takes events slice
```

---

### Task 8: Rewrite `EventStore` to use pipeline

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs`
- Modify: `crates/nexus-store/src/lib.rs`

This is the big task. Replace `run_upcasters`, `save_with_encoder`, and the `Repository` impls.

**Step 1:** Rewrite `event_store.rs`:

- Remove: `run_upcasters`, `save_with_encoder`, `current_schema_version`
- Remove: imports of old `upcaster`, `upcaster_chain` modules
- Import: new `morsel`, `pipeline`, `transform_chain`, `schema_registry` modules
- `with_upcaster` → `with_transform` (rename for clarity)
- `load`: use `run_transform_pipeline` with `EventMorsel::borrowed` from envelope
- `save`: implement the write path from the design doc (encode, build envelopes, append, advance+apply)
- Both `EventStore` and `ZeroCopyEventStore` get the same treatment

Key: the `EventStore` struct changes from `<S, C, U>` where `U: UpcasterChain` to `U: TransformChain`.

**Step 2:** Run `cargo check -p nexus-store`
Expected: compiles (may have warnings from unused old modules)

**Step 3:** Run `cargo test -p nexus-store --test morsel_tests --test transform_chain_tests --test pipeline_tests --test schema_registry_tests --test stream_label_tests`
Expected: all new tests pass

**Step 4: Commit**

```
feat(store)!: rewrite EventStore with morsel-based pipeline
```

---

### Task 9: Delete old upcaster modules

**Files:**
- Delete: `crates/nexus-store/src/upcaster.rs`
- Delete: `crates/nexus-store/src/upcaster_chain.rs`
- Modify: `crates/nexus-store/src/lib.rs` (remove old module declarations, add new re-exports)

**Step 1:** Remove `pub mod upcaster;` and `pub mod upcaster_chain;` from `lib.rs`.
Remove old re-exports (`EventUpcaster`, `UpcasterChain`, `Chain` from old modules).
Add new re-exports from `schema_transform`, `transform_chain`, `schema_registry`, `morsel`, `pipeline`.

**Step 2:** Delete the files.

**Step 3:** `cargo check -p nexus-store` — should compile.

**Step 4: Commit**

```
refactor(store)!: remove old EventUpcaster/UpcasterChain (replaced by SchemaTransform/TransformChain)
```

---

### Task 10: Update existing tests for new API

**Files:**
- Modify: `crates/nexus-store/tests/upcaster_tests.rs` → rename to `schema_transform_tests.rs` or merge into new test files
- Modify: `crates/nexus-store/tests/upcaster_chain_tests.rs` → delete or merge into `transform_chain_tests.rs`
- Modify: `crates/nexus-store/tests/event_store_tests.rs` — update for new `save` signature
- Modify: `crates/nexus-store/tests/property_tests.rs` — update upcaster property tests
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs` — update upcaster usage
- Modify: `crates/nexus-store/tests/security_tests.rs` — update upcaster usage
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs` — update
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs` — update save calls
- Modify: `crates/nexus-store/benches/store_bench.rs` — update

For each file: replace `EventUpcaster` impls with `SchemaTransform` impls, replace `can_upcast`/`upcast`/`try_upcast` with the declarative pattern, update `save()` calls to pass events.

Run: `cargo test -p nexus-store` after each file.

**Commit per file or per logical group.**

---

## Phase 3: Tower feature gate (optional, can be deferred)

### Task 11: Add `tower-service` dependency

**Files:**
- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/nexus-store/Cargo.toml`

Add `tower-service = { version = "0.3", optional = true }` behind `tower` feature.

**Commit:**
```
chore(store): add tower-service optional dependency
```

---

### Task 12: Implement tower `Service` impls

**Files:**
- Create: `crates/nexus-store/src/tower_service.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/tower_service_tests.rs`

Feature-gated behind `#[cfg(feature = "tower")]`. Thin adapters over the same internal logic. Tests run with `cargo test -p nexus-store --features tower`.

**Commit:**
```
feat(store): tower Service impls for EventStore (feature-gated)
```

---

## Phase 4: Update downstream (fjall, examples)

### Task 13: Update nexus-fjall

Check if fjall uses any upcaster types. Based on the grep earlier — it doesn't. But verify and update if needed.

### Task 14: Update examples

**Files:**
- Modify: `examples/store-and-kernel/src/main.rs`
- Modify: `examples/store-inmemory/src/main.rs`
- Modify: `examples/inmemory/src/main.rs`

Update `save()` calls to pass events. Update any `EventUpcaster` impls to `SchemaTransform`.

### Task 15: Final verification

Run:
```bash
cargo test --all
cargo clippy --all-targets -- --deny warnings
cargo fmt --all --check
```

All green → done.
