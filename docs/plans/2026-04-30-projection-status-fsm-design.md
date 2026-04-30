# Projection Status FSM

**Date:** 2026-04-30
**Branch:** `feat/projection-rebuild-158`

## Problem

The projection event loop in `PreparedProjection::run()` encodes its state machine implicitly through scattered `mut` variables:

```rust
let mut state = ...;
let mut last_persisted_version = resume_from;
let mut current_version = resume_from;
let mut dirty = false;
```

Four mutable bindings, a bare `bool` for persistence status, and an impossible state (`dirty: true, current_version: None`) that's only prevented by convention. The `apply_event` function returns `(P::State, bool)` — an opaque tuple where the `bool` means "should persist."

The shell manually updates each variable after every event and checks them on shutdown. State transitions are implicit in assignment sequences, not visible as a named pattern.

## Insight

The projection event loop is a Mealy machine with three states. The subscription drives it forward. `tokio` schedules it as a `Future`. This is the same pattern as hyper (tiny framework driving a user-provided pure function through IO), applied to event-sourced projections.

Making the FSM explicit replaces scattered `mut` variables with a single typed state, eliminates impossible states structurally, and separates the pure transition logic from the async IO shell.

## Design

### `ProjectionStatus<S>` — the FSM state

Three states, write-centric naming:

```rust
enum ProjectionStatus<S> {
    /// No events processed yet.
    Idle {
        state: S,
        checkpoint: Option<Version>,
    },
    /// Events folded, write pending.
    Pending {
        state: S,
        version: Version,
        checkpoint: Option<Version>,
    },
    /// Trigger fired, state persisted at version.
    Committed {
        state: S,
        version: Version,
    },
}
```

`checkpoint` is the version at which state was last persisted (renamed from `last_persisted_version`).

`Committed` does not carry a separate `checkpoint` — the `version` IS the checkpoint.

### State transition table

| Current | Input | Trigger fires? | Next | Shell IO |
|---|---|---|---|---|
| `Idle` | Event | no | `Pending` | none |
| `Idle` | Event | yes | `Committed` | save + checkpoint |
| `Pending` | Event | no | `Pending` | none |
| `Pending` | Event | yes | `Committed` | save + checkpoint |
| `Committed` | Event | no | `Pending` | none |
| `Committed` | Event | yes | `Committed` | save + checkpoint |
| `Idle` | Shutdown | — | exit | none |
| `Pending` | Shutdown | — | exit | save + checkpoint (flush) |
| `Committed` | Shutdown | — | exit | none |

### Impossible states eliminated

| Implicit encoding | Could represent | `ProjectionStatus` |
|---|---|---|
| `dirty: true, current_version: None` | dirty without a version — impossible by construction | No variant exists for this |
| `dirty: false, current_version: Some(v)` | clean at version v | `Committed { version: v }` — `version` is always present |
| `dirty: false, current_version: None` | initial, no events | `Idle` — no `version` field |

### Transition function

Pure sync function, no IO:

```rust
fn apply_event<P, E, Trig>(
    projector: &P,
    trigger: &Trig,
    status: ProjectionStatus<P::State>,
    event: &E,
    version: Version,
) -> Result<ProjectionStatus<P::State>, P::Error>
where
    P: Projector<Event = E>,
    E: DomainEvent,
    Trig: ProjectionTrigger,
{
    let (state, checkpoint) = match status {
        ProjectionStatus::Idle { state, checkpoint } => (state, checkpoint),
        ProjectionStatus::Pending { state, checkpoint, .. } => (state, checkpoint),
        ProjectionStatus::Committed { state, version } => (state, Some(version)),
    };

    let new_state = projector.apply(state, event)?;
    let should_persist = trigger.should_project(
        checkpoint, version, iter::once(event.name()),
    );

    if should_persist {
        Ok(ProjectionStatus::Committed { state: new_state, version })
    } else {
        Ok(ProjectionStatus::Pending {
            state: new_state,
            version,
            checkpoint,
        })
    }
}
```

### Async shell

The shell interprets `ProjectionStatus` — transition to `Committed` means save, `Pending` on shutdown means flush:

```rust
let mut status = ProjectionStatus::Idle {
    state,
    checkpoint: resume_from,
};

loop {
    match tokio::select! {
        () = &mut shutdown => None,
        item = decoded.next() => item,
    } {
        None => {
            if let ProjectionStatus::Pending { ref state, version, .. } = status {
                state_persistence
                    .save(&id, version, state).await
                    .map_err(ProjectionError::State)?;
                checkpoint_store
                    .save(&id, version).await
                    .map_err(ProjectionError::Checkpoint)?;
            }
            return Ok(());
        }
        Some(Ok((version, event))) => {
            status = apply_event(&projector, &trigger, status, &event, version)
                .map_err(ProjectionError::Projector)?;
            if let ProjectionStatus::Committed { ref state, version } = status {
                state_persistence
                    .save(&id, version, state).await
                    .map_err(ProjectionError::State)?;
                checkpoint_store
                    .save(&id, version).await
                    .map_err(ProjectionError::Checkpoint)?;
            }
        }
        Some(Err(DecodeStreamError::Stream(e))) => {
            return Err(ProjectionError::Subscription(e));
        }
        Some(Err(DecodeStreamError::Codec(e))) => {
            return Err(ProjectionError::EventCodec(e));
        }
    }
}
```

One `mut` variable. No bools. No scattered state tracking. The pattern match on `ProjectionStatus` tells the shell what IO to perform.

### Module organization

```
projection/
├── mod.rs              — re-exports
├── builder.rs          — typestate builder (unchanged)
├── error.rs            — error types (unchanged)
├── status.rs           — ★ NEW: ProjectionStatus + apply_event (pure, sync)
├── runner.rs           — ProjectionRunner + initialize() (unchanged)
├── prepared.rs         — PreparedProjection + run() (async shell, drives status)
├── initialized.rs      — Initialized enum (unchanged)
├── persist.rs          — StatePersistence trait (unchanged)
├── stream.rs           — DecodedStream adapter (unchanged)
```

`status.rs` is the sync core — zero async dependencies, fully unit-testable. `prepared.rs` is the async shell that drives it via subscription + tokio.

### Architectural parallel

| Layer | hyper | nexus projection |
|---|---|---|
| Pure transform | `Service::call` | `Projector::apply` |
| FSM state | Connection state | `ProjectionStatus` |
| Transition fn | HTTP state machine | `apply_event` |
| Async shell | Connection driver | `PreparedProjection::run()` |
| IO driver | TCP stream | `Subscription` |
| Executor | `tokio::spawn` | `tokio::spawn` |

## Test strategy

### 1. Sequence/Protocol — status transitions

- `Idle → Pending` (event, no trigger)
- `Idle → Committed` (event, trigger fires)
- `Pending → Pending` (event, no trigger)
- `Pending → Committed` (event, trigger fires)
- `Committed → Pending` (event, no trigger)
- `Committed → Committed` (event, trigger fires)
- Multi-step: `Idle → Pending → Pending → Committed → Pending → Committed`

### 2. Shutdown from each state

- Shutdown from `Idle` — no IO
- Shutdown from `Pending` — flush (save + checkpoint)
- Shutdown from `Committed` — no IO

### 3. Defensive boundary

- `apply_event` propagates projector errors from each status variant
- Trigger receives correct `checkpoint` value from each variant

### 4. Property tests

- For any sequence of events, the `checkpoint` field in the resulting status always equals the version of the last `Committed` transition (or `None` / initial if no trigger ever fired)

## Scope

### In scope

- `ProjectionStatus` enum in `status.rs`
- `apply_event` refactored to take/return `ProjectionStatus`
- `PreparedProjection::run()` loop restructured to use `ProjectionStatus`
- Unit tests for all transitions in `status.rs`
- Existing `apply_event` unit tests migrated

### Out of scope

- Public API changes (none — `ProjectionStatus` is `pub(crate)`)
- `ProjectionRunner`, `Initialized`, typestate markers, builder
- `ProjectionError`, `StatePersistence`, `DecodedStream`
- Integration tests (unchanged — they test the full pipeline)
