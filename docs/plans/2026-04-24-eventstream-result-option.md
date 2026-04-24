# EventStream Result<Option<T>, E> Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Change `EventStream::next()` from `Option<Result<T, E>>` to `Result<Option<T>, E>` so error handling uses `?` instead of `Some(Err(...))`.

**Architecture:** Change the trait signature, update all 7 implementations, then mechanically update ~79 call sites. The consumer pattern changes from `while let Some(result) = stream.next().await { let env = result?; }` to `while let Some(env) = stream.next().await? { }`.

**Tech Stack:** Pure Rust refactor. No new dependencies.

---

### Task 1: Change `EventStream` trait and `EventStreamExt`

**Files:**
- Modify: `crates/nexus-store/src/store/stream.rs`

**Step 1: Update trait signature**

In `crates/nexus-store/src/store/stream.rs`, change the `next()` method signature (line 33-35) from:

```rust
fn next(
    &mut self,
) -> impl Future<Output = Option<Result<PersistedEnvelope<'_, M>, Self::Error>>> + Send;
```

To:

```rust
fn next(
    &mut self,
) -> impl Future<Output = Result<Option<PersistedEnvelope<'_, M>>, Self::Error>> + Send;
```

Update the doc comment (lines 26-29) from:

```rust
/// Returns `None` when the stream is exhausted. Once this method
/// returns `None`, all subsequent calls must also return `None`
/// (fused behavior).
```

To:

```rust
/// Returns `Ok(None)` when the stream is exhausted. Once this method
/// returns `Ok(None)`, all subsequent calls must also return `Ok(None)`
/// (fused behavior).
```

**Step 2: Update `EventStreamExt::try_fold()`**

Change the loop in `try_fold()` (lines 87-94) from:

```rust
async move {
    let mut acc = init;
    while let Some(result) = self.next().await {
        let env = result.map_err(E::from)?;
        acc = f(acc, env)?;
    }
    Ok(acc)
}
```

To:

```rust
async move {
    let mut acc = init;
    while let Some(env) = self.next().await.map_err(E::from)? {
        acc = f(acc, env)?;
    }
    Ok(acc)
}
```

**Step 3: Verify lib compiles**

Run: `cargo check -p nexus-store --lib`
Expected: PASS (lib has no call sites beyond the trait + ext)

**Step 4: Commit**

```bash
git add crates/nexus-store/src/store/stream.rs
git commit -m "refactor(store)!: EventStream::next() returns Result<Option<T>, E> (#153)"
```

---

### Task 2: Update `InMemoryStream` and `InMemorySubscriptionStream`

**Files:**
- Modify: `crates/nexus-store/src/testing.rs`

**Step 1: Update `InMemoryStream::next()`**

Change the method (lines 97-126) to return `Result<Option<...>, ...>`. The pattern:

- `return None;` becomes `return Ok(None);`
- `return Some(Err(...));` becomes `return Err(...);`
- `Some(Ok(...))` becomes `Ok(Some(...))`

```rust
async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
    if self.pos >= self.events.len() {
        return Ok(None);
    }
    let row = &self.events[self.pos];
    self.pos += 1;
    #[cfg(debug_assertions)]
    {
        if let Some(prev) = self.prev_version {
            debug_assert!(
                row.version > prev,
                "EventStream monotonicity violated: version {} is not greater than previous {}",
                row.version,
                prev,
            );
        }
        self.prev_version = Some(row.version);
    }
    let Some(version) = Version::new(row.version) else {
        return Err(InMemoryStoreError::CorruptVersion);
    };
    Ok(Some(PersistedEnvelope::new_unchecked(
        version,
        &row.event_type,
        row.schema_version,
        &row.payload,
        (),
    )))
}
```

**Step 2: Update `InMemorySubscriptionStream::next()`**

Same pattern for `EventStream for InMemorySubscriptionStream<'_>` (lines 287-344):

- `return None;` becomes unreachable (subscription never returns None)
- `return Some(Err(...));` becomes `return Err(...);`
- `return Some(Ok(...));` becomes `return Ok(Some(...));`

**Step 3: Verify lib compiles**

Run: `cargo check -p nexus-store --lib`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/nexus-store/src/testing.rs
git commit -m "refactor(store): update InMemoryStream impls for Result<Option<T>, E>"
```

---

### Task 3: Update `FjallStream` and `FjallSubscriptionStream`

**Files:**
- Modify: `crates/nexus-fjall/src/stream.rs`
- Modify: `crates/nexus-fjall/src/subscription_stream.rs`

**Step 1: Update `FjallStream::next()`**

In `crates/nexus-fjall/src/stream.rs` (lines 29-84), apply the same mechanical pattern:

- `return None;` becomes `return Ok(None);`
- `return Some(Err(...));` becomes `return Err(...);`
- `Some(Ok(...))` becomes `Ok(Some(...))`

The poisoned early return `if self.poisoned { return None; }` becomes `if self.poisoned { return Ok(None); }`.

**Step 2: Update `FjallSubscriptionStream::next()`**

In `crates/nexus-fjall/src/subscription_stream.rs` (lines 101-184), same pattern.

**Step 3: Update FjallStream unit tests**

In `crates/nexus-fjall/src/stream.rs` tests section (~line 107+), update assertions:
- `.unwrap().unwrap()` becomes `.unwrap().unwrap()` (Result then Option — same double unwrap, different order)

Actually: `stream.next().await.unwrap().unwrap()` was `Option<Result>` → unwrap Option, unwrap Result. Now it's `Result<Option>` → unwrap Result, unwrap Option. The code is identical but the semantics flip. Verify each test still makes sense.

For `is_none()` checks: `stream.next().await.is_none()` must become `stream.next().await.unwrap().is_none()` (unwrap the Ok, then check if Option is None).

**Step 4: Verify fjall compiles**

Run: `cargo check -p nexus-fjall`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/nexus-fjall/src/stream.rs crates/nexus-fjall/src/subscription_stream.rs
git commit -m "refactor(fjall): update FjallStream impls for Result<Option<T>, E>"
```

---

### Task 4: Update replay loops in `event_store.rs` and `zero_copy.rs`

**Files:**
- Modify: `crates/nexus-store/src/store/repository/event_store.rs`
- Modify: `crates/nexus-store/src/store/repository/zero_copy.rs`

**Step 1: Update event_store.rs replay loop**

Change (lines 88-89):

```rust
while let Some(result) = stream.next().await {
    let env = result.map_err(StoreError::Adapter)?;
```

To:

```rust
while let Some(env) = stream.next().await.map_err(StoreError::Adapter)? {
```

**Step 2: Update zero_copy.rs replay loop**

Same change (lines 75-76).

**Step 3: Update repository.rs doc example**

Change the doc example in `crates/nexus-store/src/store/repository/repository.rs` (lines 51-52):

```rust
///     while let Some(result) = stream.next().await {
///         let env = result.map_err(|e| StoreError::Adapter(Box::new(e)))?;
```

To:

```rust
///     while let Some(env) = stream.next().await
///         .map_err(|e| StoreError::Adapter(Box::new(e)))? {
```

**Step 4: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: FAIL — test files still use old pattern. That's expected.

**Step 5: Commit**

```bash
git add crates/nexus-store/src/store/repository/
git commit -m "refactor(store): update replay loops for Result<Option<T>, E>"
```

---

### Task 5: Update all nexus-store test files

**Files:**
- Modify: `crates/nexus-store/tests/stream_tests.rs`
- Modify: `crates/nexus-store/tests/inmemory_store_tests.rs`
- Modify: `crates/nexus-store/tests/property_tests.rs`
- Modify: `crates/nexus-store/tests/adversarial_property_tests.rs`
- Modify: `crates/nexus-store/tests/security_tests.rs`
- Modify: `crates/nexus-store/tests/repository_qa_tests.rs`
- Modify: `crates/nexus-store/tests/integration_tests.rs`
- Modify: `crates/nexus-store/tests/bug_hunt_tests.rs`
- Modify: `crates/nexus-store/benches/store_bench.rs`

**Mechanical replacements across all files:**

1. `while let Some(result) = stream.next().await {` + `let env = result...?;`
   becomes `while let Some(env) = stream.next().await...? {`

2. `stream.next().await.unwrap().unwrap()` (Option then Result)
   becomes `stream.next().await.unwrap().unwrap()` (Result then Option — same syntax, verify semantics)

3. `stream.next().await.is_none()` (checks Option is None)
   becomes `stream.next().await.unwrap().is_none()` (unwrap Ok, check Option)

4. VecStream and ProbeStream `impl EventStream for` blocks need the same signature change as Task 2.

5. `match item { None => ..., Some(Ok(env)) => ..., Some(Err(e)) => ... }`
   becomes `match item { Ok(None) => ..., Ok(Some(env)) => ..., Err(e) => ... }`

**Step 1: Update `stream_tests.rs`**

Update VecStream impl and all test assertions.

**Step 2: Update remaining test files**

Apply mechanical replacements to all files listed above. Use `cargo check -p nexus-store --tests` after each file to verify.

**Step 3: Run nexus-store tests**

Run: `cargo test -p nexus-store`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/nexus-store/tests/ crates/nexus-store/benches/
git commit -m "test(store): update all tests for Result<Option<T>, E> EventStream"
```

---

### Task 6: Update nexus-fjall test files

**Files:**
- Modify: `crates/nexus-fjall/tests/property_tests.rs`
- Modify: `crates/nexus-fjall/tests/resilience_tests.rs`
- Modify: `crates/nexus-fjall/benches/fjall_benchmarks.rs`

**Step 1: Apply same mechanical replacements as Task 5**

**Step 2: Run fjall tests**

Run: `cargo test -p nexus-fjall`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-fjall/tests/ crates/nexus-fjall/benches/
git commit -m "test(fjall): update all tests for Result<Option<T>, E> EventStream"
```

---

### Task 7: Update examples and final check

**Files:**
- Modify: `examples/store-inmemory/src/main.rs`
- Modify: `examples/store-and-kernel/src/main.rs`

**Step 1: Update store-inmemory example**

Change the three-way match (lines 298-321):

```rust
loop {
    let item = event_stream.next().await;
    match item {
        None => break,
        Some(Ok(env)) => { ... }
        Some(Err(e)) => { ... }
    }
}
```

To:

```rust
loop {
    match event_stream.next().await {
        Err(e) => {
            println!("  Error reading event: {e}");
            break;
        }
        Ok(None) => break,
        Ok(Some(env)) => { ... }
    }
}
```

**Step 2: Update store-and-kernel example**

Change the match (lines 238-250) similarly.

**Step 3: Format and clippy**

Run: `cargo fmt --all && cargo clippy --workspace -- -D warnings`
Expected: PASS

**Step 4: Run full workspace tests**

Run: `cargo test --workspace`
Expected: PASS

**Step 5: Update CLAUDE.md**

No CLAUDE.md changes needed — the EventStream documentation there doesn't mention the return type.

**Step 6: Commit**

```bash
git add examples/ 
git commit -m "refactor: update examples for Result<Option<T>, E> EventStream (#153)"
```
