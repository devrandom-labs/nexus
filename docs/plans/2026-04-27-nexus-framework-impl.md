# nexus-framework Crate Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create a new `nexus-framework` crate with tokio-based `ProjectionRunner`, moving the runner out of `nexus-store`. Upgrade all workspace deps.

**Architecture:** `nexus-framework` depends on `nexus-store` and `tokio`. The runner's event loop uses `tokio::select!` over a `DecodedStream` adapter that converts the lending GAT `EventStream` into an owned `tokio_stream::Stream`. The `poll_fn` approach is deleted entirely.

**Tech Stack:** Rust (edition 2024), tokio, tokio-stream, nexus-store

---

### Task 1: Upgrade all workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)

**Step 1: Run cargo upgrade**

Run: `nix develop --command cargo upgrade --workspace`

**Step 2: Run cargo update**

Run: `nix develop --command cargo update`

**Step 3: Regenerate hakari**

Run: `nix develop --command cargo hakari generate`

**Step 4: Verify everything builds**

Run: `nix develop --command cargo test --workspace`
Expected: PASS — no regressions from dep upgrades

**Step 5: Commit**

```bash
git add -A
git commit -m "chore: upgrade all workspace dependencies"
```

---

### Task 2: Create `nexus-framework` crate scaffold

**Files:**
- Create: `crates/nexus-framework/Cargo.toml`
- Create: `crates/nexus-framework/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add member + workspace dep)

**Step 1: Create the crate directory**

```bash
mkdir -p crates/nexus-framework/src
```

**Step 2: Create `Cargo.toml`**

Create `crates/nexus-framework/Cargo.toml`:

```toml
[package]
name = "nexus-framework"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
repository.workspace = true
description = "Batteries-included framework for the Nexus event-sourcing kernel"
readme = "../../README.md"
keywords = ["event-sourcing", "cqrs", "projection", "framework"]
categories = ["asynchronous"]

[features]
testing = ["nexus-store/testing"]

[dependencies]
nexus = { version = "0.1.0", path = "../nexus" }
nexus-store = { version = "0.1.0", path = "../nexus-store", features = ["projection"] }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["macros"] }
tokio-stream = { workspace = true }
workspace-hack = { version = "0.1", path = "../workspace-hack" }

[dev-dependencies]
nexus-framework = { path = ".", features = ["testing"] }
nexus-store = { version = "0.1.0", path = "../nexus-store", features = ["projection", "testing"] }
tokio = { workspace = true, features = ["macros", "rt-multi-thread", "time"] }

[lints]
workspace = true
```

**Step 3: Create `src/lib.rs`**

Create `crates/nexus-framework/src/lib.rs`:

```rust
pub mod projection;
```

**Step 4: Create `src/projection/mod.rs` (empty placeholder)**

Create `crates/nexus-framework/src/projection/mod.rs`:

```rust
mod builder;
mod error;
mod persist;
mod runner;
mod stream;

pub use builder::ProjectionRunnerBuilder;
pub use error::{ProjectionError, StatePersistError};
pub use persist::{NoStatePersistence, StatePersistence, WithStatePersistence};
pub use runner::ProjectionRunner;
```

**Step 5: Add workspace member and dep**

In root `Cargo.toml`, add `"crates/nexus-framework"` to `workspace.members` and add `tokio-stream` to `[workspace.dependencies]`:

```toml
[workspace.dependencies]
# ... existing ...
tokio-stream = "0.1"
```

**Step 6: Commit**

```bash
git add crates/nexus-framework/ Cargo.toml
git commit -m "feat: scaffold nexus-framework crate"
```

---

### Task 3: Move error types and persistence to `nexus-framework`

**Files:**
- Create: `crates/nexus-framework/src/projection/error.rs` (copy from `nexus-store`)
- Create: `crates/nexus-framework/src/projection/persist.rs` (adapt from `nexus-store`)
- Delete: `crates/nexus-store/src/projection/runner/error.rs`
- Delete: `crates/nexus-store/src/projection/runner/persist.rs`

**Step 1: Copy `error.rs` verbatim**

Copy `crates/nexus-store/src/projection/runner/error.rs` to `crates/nexus-framework/src/projection/error.rs`. No changes needed — it only depends on `thiserror`.

**Step 2: Adapt `persist.rs` — fix imports**

Copy `crates/nexus-store/src/projection/runner/persist.rs` to `crates/nexus-framework/src/projection/persist.rs`.

Change the imports from `crate::` to `nexus_store::`:

```rust
use std::convert::Infallible;
use std::future::Future;
use std::num::NonZeroU32;

use nexus::{Id, Version};

use nexus_store::projection::{PendingState, StateStore};
use nexus_store::Codec;

use super::error::StatePersistError;
```

Everything else stays the same.

**Step 3: Verify it compiles**

Run: `nix develop --command cargo check -p nexus-framework`
Expected: PASS (with empty runner/builder/stream stubs)

Note: Create empty stub files for `runner.rs`, `builder.rs`, `stream.rs` to satisfy `mod` declarations:

```rust
// runner.rs, builder.rs, stream.rs — empty stubs
```

**Step 4: Commit**

```bash
git add crates/nexus-framework/src/projection/
git commit -m "feat(framework): move error and persistence types from nexus-store"
```

---

### Task 4: Create `DecodedStream` adapter

**Files:**
- Create: `crates/nexus-framework/src/projection/stream.rs`

**Step 1: Write the `DecodedStream` adapter**

Create `crates/nexus-framework/src/projection/stream.rs`:

```rust
use std::pin::Pin;
use std::task::{Context, Poll};

use nexus::{DomainEvent, Version};

use nexus_store::Codec;
use nexus_store::store::EventStream;

use tokio_stream::Stream;

/// Adapter that converts a lending GAT [`EventStream`] into an owned
/// [`tokio_stream::Stream`] by decoding events while the lending borrow
/// is alive.
///
/// Each call to `poll_next` polls the underlying `EventStream::next()`,
/// decodes the event from the borrowed envelope, and yields the owned
/// `(Version, E)` pair. The lending borrow is released before the item
/// is returned.
pub(crate) struct DecodedStream<'a, S, EC, E> {
    stream: S,
    codec: &'a EC,
    _event: std::marker::PhantomData<E>,
}

impl<'a, S, EC, E> DecodedStream<'a, S, EC, E> {
    pub(crate) fn new(stream: S, codec: &'a EC) -> Self {
        Self {
            stream,
            codec,
            _event: std::marker::PhantomData,
        }
    }
}

/// Error from the decoded stream — either a subscription/stream error or a codec error.
#[derive(Debug)]
pub(crate) enum DecodeStreamError<SubErr, CodecErr> {
    Stream(SubErr),
    Codec(CodecErr),
}

impl<S, EC, E> Stream for DecodedStream<'_, S, EC, E>
where
    S: EventStream<()> + Unpin,
    EC: Codec<E>,
    E: DomainEvent,
{
    type Item = Result<(Version, E), DecodeStreamError<S::Error, EC::Error>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Pin the next() future on the stack, poll it, decode while borrow is alive.
        let mut next_fut = std::pin::pin!(this.stream.next());
        match next_fut.as_mut().poll(cx) {
            Poll::Ready(Ok(Some(envelope))) => {
                let version = envelope.version();
                let result = this
                    .codec
                    .decode(envelope.event_type(), envelope.payload())
                    .map(|event| (version, event))
                    .map_err(DecodeStreamError::Codec);
                Poll::Ready(Some(result))
            }
            Poll::Ready(Ok(None)) => Poll::Ready(None),
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(DecodeStreamError::Stream(e)))),
            Poll::Pending => Poll::Pending,
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `nix develop --command cargo check -p nexus-framework`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-framework/src/projection/stream.rs
git commit -m "feat(framework): add DecodedStream adapter for lending-to-owned conversion"
```

---

### Task 5: Move builder to `nexus-framework`

**Files:**
- Create: `crates/nexus-framework/src/projection/builder.rs` (adapt from `nexus-store`)
- Delete: `crates/nexus-store/src/projection/runner/builder.rs`

**Step 1: Adapt `builder.rs` — fix imports**

Copy `crates/nexus-store/src/projection/runner/builder.rs` to `crates/nexus-framework/src/projection/builder.rs`.

Change the imports:

```rust
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64};

use nexus_store::projection::EveryNEvents;

use super::persist::{NoStatePersistence, WithStatePersistence};
use super::runner::ProjectionRunner;
```

Everything else stays the same.

**Step 2: Verify it compiles**

Run: `nix develop --command cargo check -p nexus-framework`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-framework/src/projection/builder.rs
git commit -m "feat(framework): move typestate builder from nexus-store"
```

---

### Task 6: Rewrite runner with `tokio::select!`

**Files:**
- Create: `crates/nexus-framework/src/projection/runner.rs`
- Delete: `crates/nexus-store/src/projection/runner/runner.rs`

**Step 1: Write the tokio-based runner**

Create `crates/nexus-framework/src/projection/runner.rs`:

```rust
use nexus::{DomainEvent, Id};

use nexus_store::store::{CheckpointStore, Subscription};
use nexus_store::Codec;

use tokio_stream::StreamExt;

use super::error::ProjectionError;
use super::persist::StatePersistence;
use super::stream::{DecodeStreamError, DecodedStream};

use nexus_store::projection::{Projector, ProjectionTrigger};

pub struct ProjectionRunner<I, Sub, Ckpt, SP, P, EC, Trig> {
    pub(crate) id: I,
    pub(crate) subscription: Sub,
    pub(crate) checkpoint: Ckpt,
    pub(crate) state_persistence: SP,
    pub(crate) projector: P,
    pub(crate) event_codec: EC,
    pub(crate) trigger: Trig,
}

impl<I, Sub, Ckpt, SP, P, EC, Trig> ProjectionRunner<I, Sub, Ckpt, SP, P, EC, Trig>
where
    I: Id + Clone,
    Sub: Subscription<()>,
    Ckpt: CheckpointStore,
    SP: StatePersistence<P::State>,
    P: Projector,
    P::Event: Unpin,
    EC: Codec<P::Event>,
    Trig: ProjectionTrigger,
{
    /// Run the projection loop until shutdown or error.
    ///
    /// # Errors
    ///
    /// Returns immediately on any error. The supervision layer (separate
    /// component) is responsible for retry/restart policy.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> Result<(), ProjectionError<P::Error, EC::Error, SP::Error, Ckpt::Error, Sub::Error>> {
        let Self {
            id,
            subscription,
            checkpoint,
            state_persistence,
            projector,
            event_codec,
            trigger,
        } = self;

        let last_checkpoint = checkpoint
            .load(&id)
            .await
            .map_err(ProjectionError::Checkpoint)?;

        let (mut state, _) = match state_persistence
            .load(&id)
            .await
            .map_err(ProjectionError::State)?
        {
            Some((s, v)) => (s, Some(v)),
            None => (projector.initial(), None),
        };

        let resume_from = last_checkpoint;

        let stream = subscription
            .subscribe(&id, resume_from)
            .await
            .map_err(ProjectionError::Subscription)?;

        let mut decoded = DecodedStream::new(stream, &event_codec);
        let mut last_persisted_version = last_checkpoint;
        let mut current_version = resume_from;
        let mut dirty = false;

        tokio::pin!(shutdown);

        loop {
            let item = tokio::select! {
                _ = &mut shutdown => None,
                item = decoded.next() => item,
            };

            let Some(result) = item else {
                if let (true, Some(ver)) = (dirty, current_version) {
                    state_persistence
                        .save(&id, ver, &state)
                        .await
                        .map_err(ProjectionError::State)?;
                    checkpoint
                        .save(&id, ver)
                        .await
                        .map_err(ProjectionError::Checkpoint)?;
                }
                return Ok(());
            };

            let (version, event) = match result {
                Ok(pair) => pair,
                Err(DecodeStreamError::Stream(e)) => {
                    return Err(ProjectionError::Subscription(e));
                }
                Err(DecodeStreamError::Codec(e)) => {
                    return Err(ProjectionError::EventCodec(e));
                }
            };

            let event_name = event.name();
            state = projector
                .apply(state, &event)
                .map_err(ProjectionError::Projector)?;
            current_version = Some(version);
            dirty = true;

            if trigger.should_project(
                last_persisted_version,
                version,
                std::iter::once(event_name),
            ) {
                state_persistence
                    .save(&id, version, &state)
                    .await
                    .map_err(ProjectionError::State)?;
                checkpoint
                    .save(&id, version)
                    .await
                    .map_err(ProjectionError::Checkpoint)?;
                last_persisted_version = Some(version);
                dirty = false;
            }
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `nix develop --command cargo check -p nexus-framework`
Expected: PASS

**Step 3: Commit**

```bash
git add crates/nexus-framework/src/projection/runner.rs
git commit -m "feat(framework): rewrite runner with tokio::select and DecodedStream"
```

---

### Task 7: Remove runner from `nexus-store`

**Files:**
- Delete: `crates/nexus-store/src/projection/runner/` (entire directory)
- Modify: `crates/nexus-store/src/projection/mod.rs` (remove `pub mod runner`)
- Modify: `crates/nexus-store/src/lib.rs` (remove runner re-exports)
- Delete: `crates/nexus-store/tests/projection_runner_tests.rs`

**Step 1: Delete the runner module**

```bash
rm -r crates/nexus-store/src/projection/runner/
```

**Step 2: Update `projection/mod.rs`**

Remove the `pub mod runner;` line. The file should be:

```rust
mod pending;
mod persisted;
mod projector;
mod store;
#[cfg(feature = "testing")]
mod testing;
mod trigger;

pub use pending::PendingState;
pub use persisted::PersistedState;
pub use projector::Projector;
pub use store::StateStore;
#[cfg(feature = "testing")]
pub use testing::InMemoryStateStore;
pub use trigger::{AfterEventTypes, EveryNEvents, ProjectionTrigger};
```

**Step 3: Update `lib.rs`**

Remove these two lines:

```rust
#[cfg(feature = "projection")]
pub use projection::runner::{ProjectionError, ProjectionRunner};
```

**Step 4: Delete the runner test file**

```bash
rm crates/nexus-store/tests/projection_runner_tests.rs
```

**Step 5: Verify nexus-store still builds**

Run: `nix develop --command cargo test -p nexus-store --features "projection,testing"`
Expected: PASS — all non-runner tests still pass

**Step 6: Commit**

```bash
git add -A
git commit -m "refactor(store): remove projection runner (moved to nexus-framework)"
```

---

### Task 8: Move and adapt runner tests to `nexus-framework`

**Files:**
- Create: `crates/nexus-framework/tests/projection_runner_tests.rs`

**Step 1: Create the test file**

Copy the content from the deleted `crates/nexus-store/tests/projection_runner_tests.rs`. Change imports:

- `nexus_store::projection::runner::{...}` → `nexus_framework::projection::{...}`
- Keep `nexus_store::*` imports for `InMemoryStore`, `InMemoryStateStore`, `Codec`, `RawEventStore`, `pending_envelope`, `CheckpointStore`, `EventStreamExt`, `StateStore`
- The `#![cfg(...)]` gate becomes `#![cfg(feature = "testing")]` (since `nexus-framework/testing` implies `nexus-store/testing`)

**Step 2: Run the tests**

Run: `nix develop --command cargo test -p nexus-framework --features testing`
Expected: PASS — all 10 tests pass

**Step 3: Commit**

```bash
git add crates/nexus-framework/tests/
git commit -m "test(framework): move and adapt projection runner tests"
```

---

### Task 9: Regenerate hakari, run full checks

**Step 1: Regenerate hakari**

Run: `nix develop --command cargo hakari generate`
Stage changes if any.

**Step 2: Format**

Run: `nix develop --command cargo fmt --all`

**Step 3: Clippy**

Run: `nix develop --command cargo clippy --workspace -- -D warnings`
Fix any issues.

**Step 4: Full test suite**

Run: `nix develop --command cargo test --workspace`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "chore: regenerate hakari, fmt, clippy after nexus-framework"
```

---

### Task 10: Update CLAUDE.md and push to PR

**Step 1: Update CLAUDE.md**

Add `nexus-framework` to the crate dependency graph:

```
nexus-framework --> nexus-store --> nexus (kernel)
nexus-fjall   --> nexus-store
nexus-macros <-- nexus (kernel, optional via "derive" feature)
```

Replace the `projection/runner/` section from the Store Crate architecture with a new section:

```markdown
### Framework Crate (`nexus-framework`) — Batteries-Included Runtime Layer

Depends on `nexus-store` + `tokio`. Provides IO-driven components that require an async runtime.

- **`projection/`** — Subscription-powered async projection execution.
  - `runner.rs` — `ProjectionRunner<Id, Sub, Ckpt, SP, P, EC, Trig>`: background event processor. Uses `tokio::select!` to race event stream against shutdown signal.
  - `builder.rs` — `ProjectionRunnerBuilder`: typestate builder with `!Send` markers for compile-time required field enforcement.
  - `error.rs` — `ProjectionError<P, EC, SP, Ckpt, Sub>`: one variant per failure domain. `StatePersistError<S, C>`.
  - `persist.rs` — `StatePersistence<S>` trait: `NoStatePersistence` (Infallible) and `WithStatePersistence<SS, SC>`.
  - `stream.rs` — `DecodedStream`: adapter converting lending GAT `EventStream` to owned `tokio_stream::Stream` by decoding inside `poll_next`.
```

**Step 2: Push**

```bash
git push
```
