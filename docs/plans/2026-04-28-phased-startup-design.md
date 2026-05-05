# Phased Startup for ProjectionRunner

**Date:** 2026-04-28
**Branch:** `feat/projection-rebuild-158`

## Problem

`ProjectionRunner::run()` mixes two concerns:

1. **Startup** — load checkpoint, load state, resolve schema mismatch, decide where to begin
2. **Event loop** — subscribe, decode, fold, trigger, persist, flush on shutdown

These are fused into one async method. Consequences:

- Startup logic is not independently testable without running the event loop
- Schema-mismatch detection clutters the loop
- A supervisor has zero control over the startup decision — it can only start/stop/restart

## Design

Split `run()` into `initialize() -> Initialized -> run(shutdown)`.

### Typestate Markers

Three markers represent the startup decision:

```rust
pub struct Resuming;    // checkpoint + loaded state -> resume
pub struct Rebuilding;  // schema mismatch detected -> replay from beginning
pub struct Starting;    // first run ever -> process from beginning
```

### PreparedProjection

Intermediate type returned by `initialize()`, parameterized by the decision:

```rust
pub struct PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Mode> {
    id: I,
    subscription: Sub,
    checkpoint: Ckpt,
    state_persistence: SP,
    projector: P,
    event_codec: EC,
    trigger: Trig,
    state: P::State,
    resume_from: Option<Version>,
    _mode: PhantomData<Mode>,
}
```

### Initialized Enum

`initialize()` returns this enum, forcing the supervisor to handle all cases:

```rust
pub enum Initialized<I, Sub, Ckpt, SP, P, EC, Trig> {
    Resuming(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Resuming>),
    Rebuilding(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Rebuilding>),
    Starting(PreparedProjection<I, Sub, Ckpt, SP, P, EC, Trig, Starting>),
}
```

### Methods Per Typestate

**Resuming** — loaded state, has checkpoint:

- `resume_from(&self) -> Option<Version>`
- `state(&self) -> &P::State`
- `force_rebuild(self) -> PreparedProjection<..., Rebuilding>` (typestate transition)
- `run(self, shutdown) -> Result<(), ProjectionError<...>>`

**Rebuilding** — schema mismatch, replays from beginning:

- `state(&self) -> &P::State` (always `projector.initial()`)
- `run(self, shutdown) -> Result<(), ProjectionError<...>>`
- No transitions out — can only run or drop (abort)

**Starting** — first run, processes from beginning:

- `state(&self) -> &P::State` (always `projector.initial()`)
- `run(self, shutdown) -> Result<(), ProjectionError<...>>`
- No transitions out — can only run or drop (abort)

**Initialized (convenience):**

- `run(self, shutdown) -> Result<(), ProjectionError<...>>` — delegates to whichever variant

### Decision Table

| loaded_state | persists_state | checkpoint | Variant |
|---|---|---|---|
| `Some(s)` | any | any | `Resuming` |
| `None` | `true` | `Some(_)` | `Rebuilding` |
| `None` | `true` | `None` | `Starting` |
| `None` | `false` | `Some(_)` | `Resuming` |
| `None` | `false` | `None` | `Starting` |

### Compile-Time Guarantees

| Invariant | Mechanism |
|---|---|
| Can't run without initializing | `ProjectionRunner` has no `run()`, only `initialize()` |
| Can't initialize twice | `initialize()` consumes `self` |
| Can't force_rebuild on Rebuilding/Starting | `force_rebuild()` only on `Resuming` |
| Can't ignore the startup decision | `Initialized` is an enum — must match |
| Can't call run twice | `run()` consumes `self` |

### Ownership and Lifetimes

`initialize()` performs IO (load checkpoint, load state) and the pure `resolve_startup` decision. It does NOT subscribe — `Subscription::subscribe` returns `Stream<'a>` borrowing `&'a self`, which would create a self-referential struct if the stream and subscription lived in the same struct.

`run()` subscribes (creating the stream) then enters the event loop. The stream borrows the subscription for the duration of `run()`.

### API Surface

```rust
// Full supervisor control:
match runner.initialize().await? {
    Initialized::Starting(p) => p.run(shutdown).await?,
    Initialized::Resuming(p) => {
        if supervisor.should_force_rebuild(&p) {
            p.force_rebuild().run(shutdown).await?
        } else {
            p.run(shutdown).await?
        }
    }
    Initialized::Rebuilding(p) => {
        if !supervisor.approve_rebuild() {
            return Ok(());
        }
        p.run(shutdown).await?
    }
}

// Simple (no supervisor):
runner.initialize().await?.run(shutdown).await?
```

### What Changes

- `ProjectionRunner::run()` is removed
- `ProjectionRunner::initialize()` is added (consumes self, returns `Result<Initialized<...>>`)
- `PreparedProjection` is a new public type
- `Initialized` is a new public enum
- `Resuming`, `Rebuilding`, `Starting` are new public marker types
- `resolve_startup` remains a private sync function — now called by `initialize()`
- The event loop logic moves to `PreparedProjection::run()`
- `mod.rs` re-exports the new public types

### What Does NOT Change

- The sync core functions (`resolve_startup`, `apply_event`) stay identical
- The event loop logic is unchanged — only its location moves
- `ProjectionRunnerBuilder` is unchanged
- `ProjectionError` is unchanged
- `StatePersistence` and its impls are unchanged
- `DecodedStream` is unchanged
