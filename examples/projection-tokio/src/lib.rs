//! Consumer-owned projection loop over `nexus_store` primitives.
//!
//! nexus deliberately ships **no** event-loop runner — that is runtime,
//! and the runtime is the consumer's. nexus ships only the pure
//! primitives: a [`Projector`] (how to fold), a [`PersistTrigger`]
//! (when to persist), a [`Subscription`] (the cursor), and a
//! [`SnapshotStore`] (atomic `(state, position)` commit). This function
//! is one concrete loop wiring them under tokio. The Agency project
//! writes its own loop — a Zenoh-native actor that owns its cursor
//! lifecycle (Agency #138) — calling the same four primitives. Two
//! loops, no shared loop code, nothing to drift.

use std::future::Future;
use std::iter;
use std::num::NonZeroU32;

use futures::StreamExt;
use nexus::{DomainEvent, Id, Version};
use nexus_store::state::{PersistTrigger, SnapshotStore};
use nexus_store::subscription::RawSubscription;
use nexus_store::{Decode, Projector, Subscription};

/// Run a projection loop until `shutdown` resolves or the stream errors.
///
/// 1. Hydrate the snapshot to resolve the starting `state` and
///    `checkpoint` atomically; if nothing is persisted, start from
///    `projector.initial()`.
/// 2. Subscribe from `checkpoint` (the cursor never returns `None`).
/// 3. For each event: fold via `Projector`; if `PersistTrigger` fires,
///    `commit` state and position together and advance `checkpoint`.
/// 4. On shutdown, commit any unpersisted trailing state once.
///
/// # Errors
/// Propagates snapshot-store (hydrate/commit), event-codec (decode),
/// projector (apply), and subscription/stream errors via the boxed error;
/// each preserves its `#[source]` chain in `to_string()`.
#[allow(
    clippy::too_many_arguments,
    reason = "example: explicit primitive wiring is clearer than a config struct"
)]
pub async fn run_projection<I, S, SS, P, EC, Trig>(
    id: I,
    subscription: Subscription<S>,
    snapshot_store: SS,
    projector: P,
    event_codec: EC,
    trigger: Trig,
    schema_version: NonZeroU32,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    I: Id + Clone + Send + Sync,
    S: RawSubscription,
    S::Stream: Send + Unpin,
    SS: SnapshotStore<P::State, Version> + Send + Sync,
    P: Projector + Send + Sync,
    P::State: Send,
    for<'a> EC: Decode<P::Event, Output<'a> = P::Event> + Send + Sync,
    Trig: PersistTrigger + Send + Sync,
{
    // 1. Hydrate: starting state + last committed position, atomically.
    let (mut state, mut checkpoint) = match snapshot_store.hydrate(&id, schema_version).await? {
        Some((version, state)) => (state, Some(version)),
        None => (projector.initial(), None),
    };

    // 2. Subscribe from the checkpoint.
    let mut stream = subscription.subscribe(&id, checkpoint).await?;

    // 3. Drive until shutdown or stream end.
    let mut pending: Option<Version> = None;
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            next = stream.next() => {
                let Some(item) = next else { break };
                let env = item?;
                let version = env.version();
                let event = event_codec.decode(&env)?;
                state = projector.apply(state, &event)?;
                if trigger.should_persist(checkpoint, version, iter::once(event.name())) {
                    snapshot_store.commit(&id, schema_version, version, &state).await?;
                    checkpoint = Some(version);
                    pending = None;
                } else {
                    pending = Some(version);
                }
            }
        }
    }

    // 4. Flush tail: commit the last pending fold once.
    if let Some(version) = pending {
        snapshot_store
            .commit(&id, schema_version, version, &state)
            .await?;
    }
    Ok(())
}
