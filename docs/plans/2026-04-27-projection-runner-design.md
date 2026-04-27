# Projection Runner Design

**Issue:** #157 (Subtask 3 of #124)
**Date:** 2026-04-27
**Status:** Approved
**Depends on:** #155 (core projection traits — completed on this branch)

## Scope

Subscription-powered async projection runner. Single-stream only — multi-stream projections deferred to `$all` stream (#149). Supervision/restart deferred to a separate subtask under #124.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Hosting model | `async fn run(self, shutdown: impl Future<Output = ()>)` | Caller owns spawning. Runtime-agnostic — no tokio types in public API |
| Shutdown signal | Generic `impl Future<Output = ()>` | Caller passes `CancellationToken::cancelled()`, channel, or anything. Keeps `nexus-store` runtime-free |
| Error recovery | Stop on error, return it | Runner processes or dies. Supervision layer (future subtask) handles retry/skip/dead-letter |
| Checkpoint strategy | `ProjectionTrigger`-driven + flush on shutdown | Reuses existing `EveryNEvents`, `AfterEventTypes` from subtask 1. Dirty tracking flushes partial batches on graceful shutdown |
| State serialization | Reuse existing `Codec<State>` trait | No new serialization traits. Runner takes two codecs: one for events, one for state |
| Construction | Typestate builder → opaque `ProjectionRunner` | Consistent with `RepositoryBuilder`. Users never name the 8-param struct directly |
| Multi-stream | Deferred to `$all` (#149) | Single subscription per runner. `$all` is just another stream — runner works unchanged |
| New dependencies | None | Uses existing traits only |

## Runner Struct

```rust
pub struct ProjectionRunner<Id, Sub, Ckpt, SS, P, EC, SC, Trig> {
    id: Id,              // projection identity (checkpoint + state store key)
    subscription: Sub,    // event source (Subscription<M>)
    checkpoint: Ckpt,     // resume position (CheckpointStore)
    state_store: SS,      // projection state persistence (StateStore)
    projector: P,         // pure fold (Projector)
    event_codec: EC,      // decode events from bytes (Codec<Event>)
    state_codec: SC,      // encode/decode projection state (Codec<State>)
    trigger: Trig,        // when to persist (ProjectionTrigger)
}
```

`run` consumes `self` — the runner is single-use. Restart = build a new one.

## Event Loop

```
1. Load checkpoint → Option<Version>
2. Load persisted state (if checkpoint exists) → Option<PersistedState>
3. Initialize state:
   - If persisted state exists: decode it via state_codec
   - Else: projector.initial()
4. Subscribe(id, from: checkpoint)
5. Loop:
   a. select! {
        event = stream.next() => { process }
        _ = shutdown => { flush; return Ok(()) }
      }
   b. Decode event via event_codec
   c. state = projector.apply(state, &decoded_event)?
   d. Track current version + dirty flag
   e. If trigger.should_project(old_version, new_version, [event_name]):
      - Encode state via state_codec → PendingState
      - Save to state_store
      - Save checkpoint(version)
      - Update old_version, clear dirty flag
6. On shutdown: if dirty:
   - Encode + persist state
   - Save checkpoint
```

Key invariants:
- Checkpoint and state are always saved together — no half-state
- Dirty flag tracks whether apply has been called since last persist
- `stream.next()` lending lifetime requires decode before next call (naturally satisfied)
- `select!` is the only exit from the loop since `Subscription` never returns `None`

## Typestate Builder

```rust
// Entry point
ProjectionRunner::builder(projection_id)
    .subscription(&store)        // required
    .checkpoint(&store)          // required
    .projector(MyProjector)      // required
    .event_codec(json_codec)     // required
    .state_store(&state_store)   // optional, default: ()
    .state_codec(json_codec)     // optional, default: ()
    .trigger(EveryNEvents(100))  // optional, default: EveryNEvents(1)
    .build()
```

Defaults:
- `StateStore` → `()` (no state persistence — valid for side-effect projections)
- `StateCodec` → `()` (not needed when state store is `()`)
- `Trigger` → `EveryNEvents(1)` (checkpoint every event)

Order-independent — each setter transitions one typestate slot. `.build()` is only available when all required slots are filled.

## Error Type

```rust
#[derive(Debug, thiserror::Error)]
pub enum ProjectionError<P, EC, SC, Ckpt, SS, Sub> {
    #[error("projector apply failed")]
    Projector(#[source] P),

    #[error("event codec failed")]
    EventCodec(#[source] EC),

    #[error("state codec failed")]
    StateCodec(#[source] SC),

    #[error("checkpoint store failed")]
    Checkpoint(#[source] Ckpt),

    #[error("state store failed")]
    StateStore(#[source] SS),

    #[error("subscription stream failed")]
    Subscription(#[source] Sub),
}
```

One variant per failure domain. Generic over concrete error types from each trait. When `StateStore = ()` and `StateCodec = ()`, their error types are `Infallible` — those variants become unconstructable and the compiler dead-code eliminates them.

## File Layout

```
crates/nexus-store/src/projection/
├── mod.rs              # existing — add runner re-exports
├── pending.rs          # existing
├── persisted.rs        # existing
├── projector.rs        # existing
├── store.rs            # existing (StateStore)
├── trigger.rs          # existing
├── testing.rs          # existing (InMemoryStateStore)
├── runner/
│   ├── mod.rs          # re-exports
│   ├── runner.rs       # ProjectionRunner struct + run()
│   ├── builder.rs      # typestate builder
│   └── error.rs        # ProjectionError
```

Three new files under `projection/runner/`. Behind existing `projection` feature flag — no new feature flags.

## Testing Strategy

### 1. Sequence/Protocol Tests
- Subscribe from beginning, process N events, verify state after each trigger fires
- Resume from checkpoint: process events → shutdown → rebuild runner → verify continues from checkpoint
- Multiple trigger firings in sequence: verify checkpoint advances correctly

### 2. Lifecycle Tests
- Start → process events → graceful shutdown → verify final flush
- Start with no events → shutdown immediately → verify no errors, no state persisted
- Start with stale state (wrong schema version) → verify falls back to `initial()`

### 3. Defensive Boundary Tests
- Projector returns error mid-stream → verify runner stops, returns `ProjectionError::Projector`
- Event codec fails → verify `ProjectionError::EventCodec`
- State codec fails on encode → verify `ProjectionError::StateCodec`
- Checkpoint store fails → verify `ProjectionError::Checkpoint`
- State store fails → verify `ProjectionError::StateStore`

### 4. Linearizability/Isolation Tests
- Concurrent appender + runner: append events while runner is processing, verify no events lost
- Runner shutdown during processing: verify partial batch is flushed correctly

All tests use `InMemoryStore` (subscription + checkpoint) and `InMemoryStateStore`.

## Architecture Diagram

```
                         ┌──────────────────┐
                         │   Caller/Host    │
                         │ tokio::spawn(    │
                         │   runner.run(    │
                         │     shutdown))   │
                         └────────┬─────────┘
                                  │
                    ┌─────────────▼──────────────┐
                    │     ProjectionRunner        │
                    │                             │
                    │  ┌───────────────────────┐  │
                    │  │     Event Loop        │  │
                    │  │                       │  │
                    │  │  select! {            │  │
                    │  │    stream.next() =>   │  │
                    │  │      decode           │  │
                    │  │      apply            │  │
                    │  │      trigger check    │  │
                    │  │      persist?         │  │
                    │  │    shutdown =>        │  │
                    │  │      flush + return   │  │
                    │  │  }                    │  │
                    │  └───────────────────────┘  │
                    └──┬───┬───┬───┬───┬───┬──────┘
                       │   │   │   │   │   │
          ┌────────────┘   │   │   │   │   └──────────────┐
          ▼                ▼   │   ▼   ▼                  ▼
    Subscription      Codec(E) │ Codec(S) StateStore  Projector
    (event source)    (decode) │ (ser/de) (persist)   (fold)
                               ▼
                        CheckpointStore
                        (resume position)
```

## Out of Scope

- Supervision / restart / backoff (separate subtask under #124)
- Multi-stream fan-in (depends on `$all` stream #149)
- Inline/same-transaction projections (#156)
- Rebuilding from scratch (subtask 4)
- Competing consumers / parallelism (Tier 2+)

## References

- Subscription trait: `crates/nexus-store/src/store/subscription/subscription.rs`
- CheckpointStore: `crates/nexus-store/src/store/subscription/checkpoint.rs`
- Projector: `crates/nexus-store/src/projection/projector.rs`
- StateStore: `crates/nexus-store/src/projection/store.rs`
- Codec: `crates/nexus-store/src/codec/owning.rs`
- RepositoryBuilder (pattern reference): `crates/nexus-store/src/store/repository/builder.rs`
- disintegrate PgEventListener (reference implementation): https://github.com/disintegrate-es/disintegrate
- Axon Streaming Event Processors: https://docs.axoniq.io/axon-framework-reference/4.12/events/event-processors/streaming/
- Marten Async Daemon: https://martendb.io/events/projections/async-daemon.html
