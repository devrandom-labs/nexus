# Projection API Cleanup

**Date:** 2026-04-30
**Branch:** `feat/projection-rebuild-158`
**Builds on:** `2026-04-30-projection-status-fsm-design.md` (implemented)

## Problem

The projection module has too many types for a simple lifecycle:

```
ProjectionRunnerBuilder → ProjectionRunner → Initialized enum → PreparedProjection → run()
```

Five types, three typestate markers (`Resuming`, `Rebuilding`, `Starting`), a `PhantomData<Mode>`, and a `StartupOutcome` enum that returns anonymous tuples. The names are vague — "Runner," "Prepared," "Initialized" tell you nothing about what the thing *is*.

The startup variants (`Resuming`/`Rebuilding`/`Starting`) have no behavioral difference. All three call the same `run()` with the same loop. The distinction is a label, not a branch.

## Design

### Naming

The thing we're building is a **Projection**. You configure it, then you load its state, then you run it.

```
Projection::builder(id) → ProjectionBuilder → .build() → Projection<Configured>
projection.initialize() → Projection<Ready>
projection.run(shutdown) → Result<()>
```

Two phases, one struct, two typestate markers.

### Typestate markers

```rust
/// Configured but not yet loaded.
pub struct Configured;

/// Loaded and ready to run.
pub struct Ready<S> {
    status: ProjectionStatus<S>,
    decision: StartupDecision,
}
```

`Configured` is a unit struct — no data. `Ready` carries the loaded `ProjectionStatus::Idle` and the startup decision label.

### `StartupDecision`

A label for the supervisor. No behavioral difference.

```rust
pub enum StartupDecision {
    /// First run — no checkpoint, no state.
    Fresh,
    /// Loaded checkpoint and/or state. Resuming.
    Resume,
    /// Schema mismatch — replaying from beginning.
    Rebuild,
}
```

### `Projection<Mode>`

```rust
pub struct Projection<I, Sub, Ckpt, SP, P: Projector, EC, Trig, Mode> {
    id: I,
    subscription: Sub,
    checkpoint: Ckpt,
    state_persistence: SP,
    projector: P,
    event_codec: EC,
    trigger: Trig,
    mode: Mode,
}
```

The `mode` field holds `Configured` (unit) or `Ready<P::State>` (status + decision).

### Methods by phase

**`Projection<..., Configured>`:**
- `async fn initialize(self) -> Result<Projection<..., Ready<P::State>>, ProjectionError>`

**`Projection<..., Ready<P::State>>`:**
- `fn decision(&self) -> StartupDecision`
- `fn status(&self) -> &ProjectionStatus<P::State>`
- `fn rebuild(self) -> Self` — resets status to `Idle { state: initial(), checkpoint: None }`
- `async fn run(self, shutdown) -> Result<(), ProjectionError>`

### `resolve_startup` returns `ProjectionStatus::Idle` directly

No `StartupOutcome` enum. No anonymous tuples.

```rust
fn resolve_startup<S>(
    loaded_state: Option<(S, Version)>,
    last_checkpoint: Option<Version>,
    persists_state: bool,
    initial: impl FnOnce() -> S,
) -> (ProjectionStatus<S>, StartupDecision) {
    match loaded_state {
        Some((state, _)) => (
            ProjectionStatus::Idle { state, checkpoint: last_checkpoint },
            StartupDecision::Resume,
        ),
        None if persists_state && last_checkpoint.is_some() => (
            ProjectionStatus::Idle { state: initial(), checkpoint: None },
            StartupDecision::Rebuild,
        ),
        None if last_checkpoint.is_some() => (
            ProjectionStatus::Idle { state: initial(), checkpoint: last_checkpoint },
            StartupDecision::Resume,
        ),
        None => (
            ProjectionStatus::Idle { state: initial(), checkpoint: None },
            StartupDecision::Fresh,
        ),
    }
}
```

### File structure

```
projection/
├── mod.rs          — re-exports
├── projection.rs   — Projection<Mode>, Configured, Ready, StartupDecision
│                     initialize(), run(), resolve_startup(), rebuild()
├── status.rs       — ProjectionStatus<S>, apply_event (unchanged from FSM work)
├── builder.rs      — ProjectionBuilder (renamed from ProjectionRunnerBuilder)
├── error.rs        — ProjectionError, StatePersistError (unchanged)
├── persist.rs      — StatePersistence trait + impls (unchanged)
└── stream.rs       — DecodedStream adapter (unchanged)
```

**Deleted:**
- `runner.rs` — merged into `projection.rs`
- `prepared.rs` — merged into `projection.rs`
- `initialized.rs` — replaced by `Ready` typestate

### Public API changes

| Before | After |
|--------|-------|
| `ProjectionRunner` | `Projection<..., Configured>` |
| `ProjectionRunnerBuilder` | `ProjectionBuilder` |
| `Initialized` enum | removed — `initialize()` returns `Projection<..., Ready>` |
| `PreparedProjection` | `Projection<..., Ready>` |
| `Resuming` / `Rebuilding` / `Starting` markers | `StartupDecision` enum (label only) |

### Integration test migration

The 30 integration tests use the old API. Mechanical renaming:

| Before | After |
|--------|-------|
| `ProjectionRunner::builder(id)` | `Projection::builder(id)` |
| `.build()` returns `ProjectionRunner` | `.build()` returns `Projection<..., Configured>` |
| `runner.initialize().await?` returns `Initialized` | `projection.initialize().await?` returns `Projection<..., Ready>` |
| `Initialized::run(shutdown)` | `ready.run(shutdown)` |
| `Initialized::Resuming(p)` pattern | `ready.decision() == StartupDecision::Resume` |
| `p.rebuild().run(shutdown)` | `ready.rebuild().run(shutdown)` |

## What stays unchanged

- `ProjectionStatus<S>` enum (Idle/Pending/Committed)
- `apply_event` pure transition function
- `ProjectionError` / `StatePersistError`
- `StatePersistence` / `NoStatePersistence` / `WithStatePersistence`
- `DecodedStream`
- The event loop logic inside `run()`
