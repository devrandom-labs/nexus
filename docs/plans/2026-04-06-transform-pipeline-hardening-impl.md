# Transform Pipeline Hardening — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace HList-based transform chain with a proc-macro-generated enum + `Upcaster` trait that validates at compile time.

**Architecture:** Proc macro `#[nexus::transforms]` validates uniqueness and contiguous versions at expansion time, generates an enum (one variant per transform) and `impl Upcaster` with a flat match loop. `EventStore` calls `upcaster.apply(morsel)` on read and `upcaster.current_version()` on write.

**Tech Stack:** Rust proc macros (syn, quote, proc-macro2), nexus-store traits, thiserror

**Design doc:** `docs/plans/2026-04-06-transform-pipeline-hardening-design.md`

---

## Phase 1: Add Upcaster trait + update EventMorsel (nexus-store)

### Task 1: Add `UpcastError::TransformFailed` variant

**Files:**
- Modify: `crates/nexus-store/src/error.rs`
- Test: `crates/nexus-store/tests/error_tests.rs`

**Step 1: Write failing test**

In `crates/nexus-store/tests/error_tests.rs`, add:

```rust
#[test]
fn transform_failed_error_displays_context() {
    let err = UpcastError::TransformFailed {
        event_type: "OrderCreated".into(),
        schema_version: Version::INITIAL,
        source: Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad json")),
    };
    let msg = err.to_string();
    assert!(msg.contains("OrderCreated"), "should contain event type");
    assert!(msg.contains("bad json"), "should contain source message");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store -- transform_failed_error_displays_context`
Expected: FAIL — `TransformFailed` variant doesn't exist.

**Step 3: Add the variant**

In `crates/nexus-store/src/error.rs`, add to `UpcastError`:

```rust
/// A schema transform failed to process the payload.
#[error("Transform failed for '{event_type}' at schema version {schema_version}: {source}")]
TransformFailed {
    event_type: String,
    schema_version: Version,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
},
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-store -- transform_failed_error_displays_context`
Expected: PASS

**Step 5: Commit**

```
feat(store): add UpcastError::TransformFailed variant
```

---

### Task 2: Add `EventMorsel::new` constructor

**Files:**
- Modify: `crates/nexus-store/src/morsel.rs`
- Test: `crates/nexus-store/tests/morsel_tests.rs`

**Step 1: Write failing test**

In `crates/nexus-store/tests/morsel_tests.rs`, add:

```rust
#[test]
fn morsel_new_creates_owned() {
    let morsel = EventMorsel::new("OrderCreated", Version::INITIAL, vec![1, 2, 3]);
    assert_eq!(morsel.event_type(), "OrderCreated");
    assert_eq!(morsel.schema_version(), Version::INITIAL);
    assert_eq!(morsel.payload(), &[1, 2, 3]);
    assert!(!morsel.is_borrowed(), "new() creates owned morsel");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store -- morsel_new_creates_owned`
Expected: FAIL — `new` method doesn't exist.

**Step 3: Add the constructor**

In `crates/nexus-store/src/morsel.rs`, add:

```rust
/// Build with owned payload (from a transform).
#[must_use]
pub fn new(event_type: &str, schema_version: Version, payload: Vec<u8>) -> Self {
    Self {
        event_type: Cow::Owned(event_type.to_owned()),
        schema_version,
        payload: Cow::Owned(payload),
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-store -- morsel_new_creates_owned`
Expected: PASS

**Step 5: Commit**

```
feat(store): add EventMorsel::new constructor for owned morsels
```

---

### Task 3: Add `Upcaster` trait

**Files:**
- Create: `crates/nexus-store/src/upcaster.rs`
- Modify: `crates/nexus-store/src/lib.rs`
- Test: `crates/nexus-store/tests/upcaster_trait_tests.rs`

**Step 1: Write failing test**

Create `crates/nexus-store/tests/upcaster_trait_tests.rs`:

```rust
#![allow(clippy::unwrap_used, reason = "tests")]

use nexus::Version;
use nexus_store::morsel::EventMorsel;
use nexus_store::Upcaster;

#[test]
fn unit_upcaster_is_passthrough() {
    let upcaster = ();
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"payload");
    let result = upcaster.apply(morsel).unwrap();
    assert_eq!(result.event_type(), "OrderCreated");
    assert_eq!(result.schema_version(), Version::INITIAL);
    assert_eq!(result.payload(), b"payload");
    assert!(result.is_borrowed(), "passthrough should stay borrowed");
}

#[test]
fn unit_upcaster_current_version_is_none() {
    let upcaster = ();
    assert_eq!(upcaster.current_version("OrderCreated"), None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-store -- unit_upcaster`
Expected: FAIL — `Upcaster` doesn't exist.

**Step 3: Create the trait**

Create `crates/nexus-store/src/upcaster.rs`:

```rust
use crate::error::UpcastError;
use crate::morsel::EventMorsel;
use nexus::Version;

/// Upcast old events to the current schema version.
///
/// `EventStore` calls `apply()` on the read path and `current_version()`
/// on the write path. The proc macro `#[nexus::transforms]` generates
/// implementations; `()` is the no-op passthrough.
pub trait Upcaster: Send + Sync {
    /// Run all matching transforms until the morsel is at the current schema version.
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError>;

    /// Current schema version for an event type (stamped on new events).
    /// Returns `None` if the event type has no transforms.
    fn current_version(&self, event_type: &str) -> Option<Version>;
}

/// No-op upcaster — passthrough, no transforms.
impl Upcaster for () {
    #[inline]
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        Ok(morsel)
    }

    #[inline]
    fn current_version(&self, _event_type: &str) -> Option<Version> {
        None
    }
}
```

Add to `crates/nexus-store/src/lib.rs`:

```rust
pub mod upcaster;
```

And the re-export:

```rust
pub use upcaster::Upcaster;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-store -- unit_upcaster`
Expected: PASS

**Step 5: Run clippy**

Run: `cargo clippy -p nexus-store -- --deny warnings`
Expected: PASS

**Step 6: Commit**

```
feat(store): add Upcaster trait with () no-op impl
```

---

## Phase 2: Migrate EventStore to Upcaster

### Task 4: Update EventStore to accept Upcaster

**Files:**
- Modify: `crates/nexus-store/src/event_store.rs`

**Step 1: Add `with_upcaster` constructor and update load/save to use `Upcaster` trait**

Replace the `transforms: U` field and its usage:

1. Rename field `transforms` → `upcaster`
2. Change default type param from `U = ()` (already `()`)
3. Add `with_upcaster(store, codec, upcaster)` constructor
4. In `load()`: replace `run_transform_pipeline(&self.transforms, morsel)` with `self.upcaster.apply(morsel)`
5. In `save()`: replace `ChainDerivedRegistry::new(&self.transforms)` + `registry.current_version(event_name)` with `self.upcaster.current_version(event_name)`
6. Keep `with_transform()` temporarily for backward compat — will remove in Phase 3

The `load()` method changes from:

```rust
let morsel = EventMorsel::borrowed(env.event_type(), env.schema_version_as_version(), env.payload());
let transformed = run_transform_pipeline(&self.transforms, morsel)
    .map_err(|e| StoreError::Codec(Box::new(e)))?;
let event = self.codec.decode(transformed.event_type(), transformed.payload())
    .map_err(|e| StoreError::Codec(Box::new(e)))?;
```

To:

```rust
let morsel = EventMorsel::borrowed(env.event_type(), env.schema_version_as_version(), env.payload());
let transformed = self.upcaster.apply(morsel)
    .map_err(|e| StoreError::Codec(Box::new(e)))?;
let event = self.codec.decode(transformed.event_type(), transformed.payload())
    .map_err(|e| StoreError::Codec(Box::new(e)))?;
```

The `save()` method changes from:

```rust
let registry = ChainDerivedRegistry::new(&self.transforms);
// ...
let schema_version = registry.current_version(event_name).unwrap_or(Version::INITIAL);
```

To:

```rust
let schema_version = self.upcaster.current_version(event_name).unwrap_or(Version::INITIAL);
```

Do the same for `ZeroCopyEventStore`.

**Step 2: Add `Upcaster` bound alongside existing `TransformChain` bound**

The `Repository` impl currently bounds on `U: TransformChain`. Change to `U: Upcaster`. This requires implementing `Upcaster` for the old chain types temporarily (Task 5).

**Step 3: Run existing tests**

Run: `cargo test -p nexus-store`
Expected: Some failures due to `TransformChain` → `Upcaster` bound change. These will be fixed in Task 5.

**Step 4: Commit**

```
refactor(store): migrate EventStore from TransformChain to Upcaster
```

---

### Task 5: Bridge old TransformChain to Upcaster (temporary)

**Files:**
- Modify: `crates/nexus-store/src/upcaster.rs`

**Step 1: Implement `Upcaster` for existing chain types**

Add a blanket impl that wraps the old `TransformChain` + `run_transform_pipeline` behind the `Upcaster` trait. This is temporary glue so existing tests keep passing while we migrate.

In `crates/nexus-store/src/upcaster.rs`, add:

```rust
use crate::pipeline::run_transform_pipeline;
use crate::schema_registry::{ChainDerivedRegistry, SchemaRegistry};
use crate::transform_chain::TransformChain;

/// Wraps a `TransformChain` as an `Upcaster`.
/// Temporary bridge — will be removed when old chain types are deleted.
pub struct ChainUpcaster<C>(pub C);

impl<C: TransformChain> Upcaster for ChainUpcaster<C> {
    fn apply<'a>(&self, morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
        run_transform_pipeline(&self.0, morsel)
    }

    fn current_version(&self, event_type: &str) -> Option<Version> {
        let registry = ChainDerivedRegistry::new(&self.0);
        registry.current_version(event_type)
    }
}
```

**Step 2: Update event_store_tests.rs to use ChainUpcaster for transform tests**

For tests that use `EventStore::new(store, codec).with_transform(V1ToV2)`, temporarily wrap:

```rust
let chain = Chain::new_for_testing(V1ToV2, ());
let es = EventStore::with_upcaster(InMemoryStore::new(), TestCodec, ChainUpcaster(chain));
```

**Step 3: Run all nexus-store tests**

Run: `cargo test -p nexus-store`
Expected: PASS

**Step 4: Commit**

```
refactor(store): add ChainUpcaster bridge for backward compat
```

---

## Phase 3: Add `#[nexus::transforms]` proc macro

### Task 6: Add transforms proc macro skeleton

**Files:**
- Modify: `crates/nexus-macros/src/lib.rs`
- Modify: `crates/nexus-macros/Cargo.toml` (if needed)
- Modify: `crates/nexus/src/lib.rs` (re-export)

**Step 1: Add the proc macro entry point**

In `crates/nexus-macros/src/lib.rs`, add:

```rust
/// Generates an `Upcaster` implementation from annotated transform functions.
///
/// # Attributes
///
/// - `aggregate = Type` — the aggregate type these transforms belong to
///
/// Each method must be annotated with `#[transform(...)]`:
/// - `event = "EventName"` — the event type this transform handles
/// - `from = N` — source schema version (>= 1)
/// - `to = N` — target schema version (must be `from + 1`)
/// - `rename = "NewName"` — optional event type rename
///
/// # Compile-time validation
///
/// - No duplicate `(event, from)` pairs
/// - `to == from + 1` for each transform
/// - `from >= 1`
///
/// # Example
///
/// ```ignore
/// #[nexus::transforms(aggregate = Order)]
/// impl OrderTransforms {
///     #[transform(event = "OrderCreated", from = 1, to = 2)]
///     fn v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, MyError> {
///         Ok(payload.to_vec())
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn transforms(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = attr;
    let ast = parse_macro_input!(item as syn::ItemImpl);
    match parse_transforms(&ast, args.into()) {
        Ok(code) => code,
        Err(e) => e.to_compile_error(),
    }
    .into()
}
```

**Step 2: Implement `parse_transforms`**

This is the core macro logic. It must:

1. Parse `aggregate = Type` from outer attributes
2. Parse each method's `#[transform(event = "...", from = N, to = N)]` attributes
3. Validate: no duplicate `(event, from)`, `to == from + 1`, `from >= 1`
4. Generate: enum, `SchemaTransform` impls, `impl Upcaster`

```rust
struct TransformDef {
    fn_name: syn::Ident,
    event_type: String,
    from_version: u64,
    to_version: u64,
    rename: Option<String>,
    body: syn::Block,
    error_type: syn::Type,
}

fn parse_transforms(
    ast: &syn::ItemImpl,
    args: proc_macro2::TokenStream,
) -> Result<proc_macro2::TokenStream> {
    // 1. Parse aggregate attribute
    let mut aggregate_type: Option<syn::Type> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("aggregate") {
            aggregate_type = Some(meta.value()?.parse()?);
        } else {
            return Err(meta.error("expected `aggregate`"));
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, args)?;
    let aggregate_type = aggregate_type
        .ok_or_else(|| syn::Error::new(proc_macro2::Span::call_site(), "`aggregate` is required"))?;

    // 2. Get the struct name from the impl block
    let struct_name = match &*ast.self_ty {
        syn::Type::Path(p) => p.path.segments.last()
            .ok_or_else(|| syn::Error::new_spanned(&ast.self_ty, "expected a type name"))?,
        _ => return Err(syn::Error::new_spanned(&ast.self_ty, "expected a type name")),
    };
    let struct_ident = &struct_name.ident;

    // 3. Parse each method's #[transform] attributes
    let mut transforms = Vec::new();
    for item in &ast.items {
        let method = match item {
            syn::ImplItem::Fn(m) => m,
            _ => continue,
        };
        // Parse #[transform(...)] attribute from method
        // Extract event, from, to, rename
        // Extract return type to get Error type
        // Push to transforms vec
    }

    // 4. Validate
    // - Check no duplicate (event, from)
    // - Check to == from + 1
    // - Check from >= 1

    // 5. Generate enum, SchemaTransform impls, Upcaster impl
    // (see design doc for generated code shape)

    Ok(expanded)
}
```

The full implementation of step 3-5 is detailed but mechanical. Follow the code generation pattern from the existing `aggregate` macro (use `quote!`, return `Result<proc_macro2::TokenStream>`).

**Step 3: Re-export from nexus crate**

In `crates/nexus/src/lib.rs`, add:

```rust
#[cfg(feature = "derive")]
pub use nexus_macros::transforms;
```

**Step 4: Commit**

```
feat(macros): add #[nexus::transforms] proc macro skeleton
```

---

### Task 7: Implement macro validation

**Files:**
- Modify: `crates/nexus-macros/src/lib.rs`
- Create: `crates/nexus-macros/tests/macro_compile_fail/transforms_duplicate.rs`
- Create: `crates/nexus-macros/tests/macro_compile_fail/transforms_duplicate.stderr`
- Create: `crates/nexus-macros/tests/macro_compile_fail/transforms_non_contiguous.rs`
- Create: `crates/nexus-macros/tests/macro_compile_fail/transforms_non_contiguous.stderr`

**Step 1: Write compile-fail tests**

`transforms_duplicate.rs`:
```rust
use nexus::transforms;

struct Order;

#[derive(Debug)]
struct MyError;
impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error")
    }
}
impl std::error::Error for MyError {}

#[nexus::transforms(aggregate = Order)]
impl OrderTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn v1_to_v2_a(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }

    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn v1_to_v2_b(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }
}

fn main() {}
```

`transforms_duplicate.stderr`:
```
error: duplicate transform for event 'OrderCreated' at source version 1
```

`transforms_non_contiguous.rs`:
```rust
#[nexus::transforms(aggregate = Order)]
impl OrderTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 3)]
    fn v1_to_v3(payload: &[u8]) -> Result<Vec<u8>, MyError> {
        Ok(payload.to_vec())
    }
}
```

`transforms_non_contiguous.stderr`:
```
error: non-contiguous version: to (3) must equal from + 1 (2)
```

**Step 2: Run compile-fail tests**

Run: `cargo test -p nexus-macros -- macro_compile_fail`
Expected: FAIL until validation is implemented.

**Step 3: Implement validation in the macro**

In `parse_transforms`, after collecting all `TransformDef`:

```rust
// Validate from >= 1
for t in &transforms {
    if t.from_version < 1 {
        return Err(syn::Error::new_spanned(&t.fn_name, "from version must be >= 1"));
    }
}

// Validate to == from + 1
for t in &transforms {
    if t.to_version != t.from_version + 1 {
        return Err(syn::Error::new_spanned(
            &t.fn_name,
            format!(
                "non-contiguous version: to ({}) must equal from + 1 ({})",
                t.to_version,
                t.from_version + 1
            ),
        ));
    }
}

// Validate no duplicate (event, from)
let mut seen = std::collections::HashSet::new();
for t in &transforms {
    let key = (t.event_type.clone(), t.from_version);
    if !seen.insert(key) {
        return Err(syn::Error::new_spanned(
            &t.fn_name,
            format!(
                "duplicate transform for event '{}' at source version {}",
                t.event_type, t.from_version
            ),
        ));
    }
}
```

**Step 4: Run compile-fail tests**

Run: `cargo test -p nexus-macros -- macro_compile_fail`
Expected: PASS

**Step 5: Commit**

```
feat(macros): compile-time validation for #[nexus::transforms]
```

---

### Task 8: Implement macro code generation

**Files:**
- Modify: `crates/nexus-macros/src/lib.rs`
- Create: `crates/nexus-macros/tests/transforms_derive.rs`

**Step 1: Write integration test**

Create `crates/nexus-macros/tests/transforms_derive.rs`:

```rust
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]

use nexus::Version;
use nexus_store::morsel::EventMorsel;
use nexus_store::Upcaster;

#[derive(Debug)]
struct TestError;
impl std::fmt::Display for TestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "test error")
    }
}
impl std::error::Error for TestError {}

struct TestAggregate;

#[nexus::transforms(aggregate = TestAggregate)]
impl TestTransforms {
    #[transform(event = "OrderCreated", from = 1, to = 2)]
    fn created_v1_to_v2(payload: &[u8]) -> Result<Vec<u8>, TestError> {
        let mut data = payload.to_vec();
        data.extend_from_slice(b",v2");
        Ok(data)
    }

    #[transform(event = "OrderCreated", from = 2, to = 3)]
    fn created_v2_to_v3(payload: &[u8]) -> Result<Vec<u8>, TestError> {
        let mut data = payload.to_vec();
        data.extend_from_slice(b",v3");
        Ok(data)
    }

    #[transform(event = "OrderCancelled", from = 1, to = 2, rename = "OrderVoided")]
    fn cancelled_to_voided(payload: &[u8]) -> Result<Vec<u8>, TestError> {
        Ok(payload.to_vec())
    }
}

#[test]
fn transforms_single_step() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::INITIAL, b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert_eq!(result.schema_version(), Version::new(3).unwrap());
    assert_eq!(result.payload(), b"data,v2,v3");
    assert_eq!(result.event_type(), "OrderCreated");
}

#[test]
fn transforms_rename() {
    let morsel = EventMorsel::borrowed("OrderCancelled", Version::INITIAL, b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert_eq!(result.event_type(), "OrderVoided");
    assert_eq!(result.schema_version(), Version::new(2).unwrap());
}

#[test]
fn transforms_passthrough_current_version() {
    let morsel = EventMorsel::borrowed("OrderCreated", Version::new(3).unwrap(), b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert!(result.is_borrowed(), "current version should pass through");
}

#[test]
fn transforms_unknown_event_passthrough() {
    let morsel = EventMorsel::borrowed("Unknown", Version::INITIAL, b"data");
    let result = TestTransforms.apply(morsel).unwrap();
    assert!(result.is_borrowed());
}

#[test]
fn transforms_current_version_lookup() {
    assert_eq!(TestTransforms.current_version("OrderCreated"), Some(Version::new(3).unwrap()));
    assert_eq!(TestTransforms.current_version("OrderCancelled"), Some(Version::new(2).unwrap()));
    assert_eq!(TestTransforms.current_version("Unknown"), None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p nexus-macros -- transforms_`
Expected: FAIL — code generation not yet implemented.

**Step 3: Implement code generation**

In `parse_transforms`, after validation, generate code following the design doc pattern:

1. Generate the struct definition (if not already defined by user)
2. Generate the original impl block with user's methods
3. Generate `impl Upcaster` with the match loop
4. Generate `current_version` with static match

The generated `impl Upcaster` follows the match-loop pattern from the design doc:

```rust
let match_arms = transforms.iter().map(|t| {
    let fn_name = &t.fn_name;
    let event_type = &t.event_type;
    let from_version = t.from_version;
    let to_version = t.to_version;
    let output_event_type = t.rename.as_deref().unwrap_or(&t.event_type);

    quote! {
        (#event_type, v) if v == ::nexus::Version::new(#from_version).unwrap() => {
            let payload = Self::#fn_name(morsel.payload())
                .map_err(|e| ::nexus_store::UpcastError::TransformFailed {
                    event_type: #event_type.into(),
                    schema_version: ::nexus::Version::new(#from_version).unwrap(),
                    source: ::std::boxed::Box::new(e),
                })?;
            ::nexus_store::morsel::EventMorsel::new(
                #output_event_type,
                ::nexus::Version::new(#to_version).unwrap(),
                payload,
            )
        }
    }
});
```

For `current_version`, compute the max version per event type at macro time:

```rust
let mut max_versions: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
for t in &transforms {
    let entry = max_versions.entry(t.event_type.clone()).or_insert(1);
    if t.to_version > *entry {
        *entry = t.to_version;
    }
}
// Also track renamed events
for t in &transforms {
    if let Some(ref rename) = t.rename {
        let entry = max_versions.entry(rename.clone()).or_insert(1);
        if t.to_version > *entry {
            *entry = t.to_version;
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p nexus-macros -- transforms_`
Expected: PASS

**Step 5: Run clippy**

Run: `cargo clippy -p nexus-macros -- --deny warnings`
Expected: PASS

**Step 6: Commit**

```
feat(macros): implement code generation for #[nexus::transforms]
```

---

## Phase 4: Delete old infrastructure

### Task 9: Remove old transform chain, pipeline, schema registry

**Files:**
- Delete: `crates/nexus-store/src/transform_chain.rs`
- Delete: `crates/nexus-store/src/schema_transform.rs`
- Delete: `crates/nexus-store/src/pipeline.rs`
- Delete: `crates/nexus-store/src/schema_registry.rs`
- Modify: `crates/nexus-store/src/lib.rs` — remove modules + re-exports
- Modify: `crates/nexus-store/src/upcaster.rs` — remove `ChainUpcaster` bridge
- Modify: `crates/nexus-store/src/event_store.rs` — remove `with_transform()`
- Delete: `crates/nexus-store/tests/transform_chain_new_tests.rs`
- Delete: `crates/nexus-store/tests/schema_transform_tests.rs`
- Delete: `crates/nexus-store/tests/pipeline_tests.rs`
- Delete: `crates/nexus-store/tests/schema_registry_tests.rs`

**Step 1: Remove module declarations and re-exports from lib.rs**

Remove from `crates/nexus-store/src/lib.rs`:

```rust
// Remove these lines:
pub mod pipeline;
pub mod schema_registry;
pub mod schema_transform;
pub mod transform_chain;

pub use pipeline::run_transform_pipeline;
pub use schema_registry::{ChainDerivedRegistry, SchemaRegistry};
pub use schema_transform::SchemaTransform;
pub use transform_chain::{Chain, TransformChain};
```

**Step 2: Remove `ChainUpcaster` from upcaster.rs**

Delete the temporary bridge code added in Task 5.

**Step 3: Remove `with_transform()` from EventStore and ZeroCopyEventStore**

Remove the method entirely from `crates/nexus-store/src/event_store.rs`.

**Step 4: Delete source files**

Delete: `transform_chain.rs`, `schema_transform.rs`, `pipeline.rs`, `schema_registry.rs`

**Step 5: Delete old test files**

Delete: `transform_chain_new_tests.rs`, `schema_transform_tests.rs`, `pipeline_tests.rs`, `schema_registry_tests.rs`

**Step 6: Run all tests**

Run: `cargo test -p nexus-store`
Expected: Some tests will fail if they import removed types. Fix in Task 10.

**Step 7: Commit**

```
refactor(store)!: remove TransformChain, SchemaTransform, pipeline, SchemaRegistry
```

---

### Task 10: Migrate remaining test files

**Files:**
- Modify: `crates/nexus-store/tests/event_store_tests.rs`
- Modify: `crates/nexus-store/tests/zero_copy_event_store_tests.rs`
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`
- Modify: `crates/nexus-store/tests/property_tests.rs`
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs`
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs`
- Modify: `crates/nexus-store/tests/security_tests.rs`
- Modify: `crates/nexus-store/tests/morsel_tests.rs`

**Step 1: Find all remaining references to deleted types**

Run: `cargo test -p nexus-store 2>&1` and fix each compilation error.

For each test that used `SchemaTransform` + `Chain` + `run_transform_pipeline`, convert to either:

- **Manual `Upcaster` impl** for simple cases:
  ```rust
  struct TestUpcaster;
  impl Upcaster for TestUpcaster {
      fn apply<'a>(&self, mut morsel: EventMorsel<'a>) -> Result<EventMorsel<'a>, UpcastError> {
          loop {
              morsel = match (morsel.event_type(), morsel.schema_version()) {
                  ("TestEvent", v) if v == Version::INITIAL => {
                      let mut data = morsel.payload().to_vec();
                      data.extend_from_slice(b",v2");
                      EventMorsel::new("TestEvent", Version::new(2).unwrap(), data)
                  }
                  _ => break,
              };
          }
          Ok(morsel)
      }
      fn current_version(&self, event_type: &str) -> Option<Version> {
          match event_type {
              "TestEvent" => Some(Version::new(2).unwrap()),
              _ => None,
          }
      }
  }
  ```

- **`()` upcaster** for tests that don't need transforms

- **`#[nexus::transforms]` macro** for integration tests that should exercise the full macro

**Step 2: Run all tests**

Run: `cargo test -p nexus-store`
Expected: PASS

**Step 3: Run clippy**

Run: `cargo clippy -p nexus-store --all-targets -- --deny warnings`
Expected: PASS

**Step 4: Commit**

```
test(store): migrate all tests to Upcaster trait
```

---

## Phase 5: Update downstream crates

### Task 11: Update nexus-fjall

**Files:**
- Modify: `crates/nexus-fjall/src/store.rs` (if it imports transform types)
- Modify: any fjall test files referencing old types

**Step 1: Check for references**

Run: `cargo build -p nexus-fjall 2>&1` and fix any compilation errors from removed types.

**Step 2: Run tests**

Run: `cargo test -p nexus-fjall`
Expected: PASS

**Step 3: Commit**

```
refactor(fjall): update for Upcaster migration
```

---

### Task 12: Update examples

**Files:**
- Modify: `examples/store-inmemory/src/main.rs`
- Modify: `examples/store-and-kernel/src/main.rs`
- Modify: `examples/inmemory/src/main.rs`

**Step 1: Update any example that uses `with_transform` or imports removed types**

Replace `EventStore::new(store, codec).with_transform(T)` with `EventStore::with_upcaster(store, codec, MyTransforms)` or `EventStore::new(store, codec)` if no transforms needed.

**Step 2: Build all examples**

Run: `cargo build --examples`
Expected: PASS

**Step 3: Commit**

```
docs(examples): update for Upcaster migration
```

---

## Phase 6: Final verification

### Task 13: Full workspace check

**Step 1: Clippy all**

Run: `cargo clippy --all-targets -- --deny warnings`
Expected: PASS

**Step 2: Test all**

Run: `cargo test --all`
Expected: PASS

**Step 3: Format check**

Run: `cargo fmt --all --check`
Expected: PASS

**Step 4: Commit any final fixes**

```
chore: final cleanup after transform pipeline hardening
```
