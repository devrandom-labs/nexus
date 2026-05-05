# Projection Rebuilding via Schema-Version Detection

**Issue:** #158
**Date:** 2026-04-28

## Problem

When a projector's logic changes (new field, bug fix, calculation change), the persisted state is stale. The runner must replay all events through the new projector to regenerate correct state.

The runner already detects schema version mismatches — `StateStore::load` filters by `schema_version` and returns `None` for stale data. But the runner still resumes from the **old checkpoint position**, producing a fresh state folded from the middle of the stream. Silently wrong.

## Design: Runner-Internal Auto-Rebuild

No other framework auto-detects projection staleness. Marten, Axon, and Akka all require explicit operator action (`RebuildProjectionAsync`, `resetTokens()`, offset management API). This makes sense for server environments with operators. Nexus targets embedded/mobile where there is no operator — the app must self-heal.

### Detection Signal

The runner already has both pieces of information at startup:

| `checkpoint.load()` | `state_persistence.load()` | `persists_state()` | Meaning |
|---|---|---|---|
| `None` | `None` | any | First run → normal startup |
| `Some(v)` | `Some(s, _)` | `true` | Normal resume |
| `Some(v)` | `None` | `true` | **Schema mismatch → rebuild** |
| `Some(v)` | `None` | `false` | No state persistence → normal resume |

### Trait Change

Add one method to `StatePersistence<S>` with a default impl:

```rust
/// Whether this persistence layer actually stores state.
fn persists_state(&self) -> bool {
    true
}
```

`NoStatePersistence` overrides to return `false`.

### Runner Startup Change

Replace the current checkpoint→state→subscribe sequence with:

```rust
let last_checkpoint = checkpoint.load(&id).await?;

let (mut state, resume_from) = match state_persistence.load(&id).await? {
    Some((s, _)) => (s, last_checkpoint),
    None if state_persistence.persists_state() && last_checkpoint.is_some() => {
        (projector.initial(), None)  // rebuild: replay from beginning
    }
    None => (projector.initial(), last_checkpoint),
};
```

The event loop body is unchanged. `resume_from` feeds into `subscription.subscribe(&id, resume_from)` and initializes `last_persisted_version` / `current_version`.

### Crash Safety

If the process crashes mid-rebuild before any trigger fires, on restart the same mismatch is detected → replays from scratch. Idempotent by construction. The old checkpoint is overwritten on first trigger fire; the old state is overwritten on first save.

### No New Trait Methods for Delete

Stale checkpoint and state get naturally overwritten during the rebuild run. No `delete` methods needed on `CheckpointStore` or `StatePersistence`. These can be added later if an explicit rebuild API or cleanup story requires them.

## Test Strategy

### 1. Sequence/Protocol

- Normal run → shutdown → restart with same schema → resumes from checkpoint (no rebuild)
- Normal run → shutdown → restart with bumped schema → replays from beginning
- Rebuild completes → trigger fires → checkpoint updated → restart with same schema → normal resume

### 2. Lifecycle

- Rebuild mid-flight crash: bumped schema → kill before trigger → restart → mismatch detected again → replays from scratch

### 3. Defensive Boundary

- `NoStatePersistence` + existing checkpoint → no rebuild (normal resume)
- `WithStatePersistence` + no checkpoint + no state → first run, not rebuild

### 4. Linearizability

Not applicable — no new concurrent-access surface.

## Scope

### In Scope
- `StatePersistence::persists_state()` method
- Runner startup logic change
- Tests in `tests/projection_runner_tests.rs`

### Out of Scope
- Inline projection rebuild (#156 blocked on transaction threading)
- Explicit rebuild API / `delete` methods
- Progress reporting / callbacks
- Optimized rebuild (Marten's reverse-order optimization)
