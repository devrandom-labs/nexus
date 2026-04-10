use std::num::NonZeroU32;

use crate::codec::Codec;
use crate::error::StoreError;
use crate::snapshot::{PendingSnapshot, SnapshotStore, SnapshotTrigger};
use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Version};

use super::replay::ReplayFrom;
use super::repository::Repository;

/// Snapshot-aware repository decorator.
///
/// Wraps an inner repository (e.g., `EventStore` or `ZeroCopyEventStore`)
/// and adds transparent snapshot support:
///
/// - **Load:** tries the snapshot store first; on hit with matching schema
///   version, restores state from snapshot and replays only subsequent events.
///   On miss or schema mismatch, falls back to full event replay via the
///   inner repository. Optionally creates a snapshot after a full replay
///   (lazy/on-read snapshotting) if `snapshot_on_read` is enabled.
///
/// - **Save:** delegates event persistence to the inner repository, then
///   checks the trigger to optionally persist a snapshot of the current state.
///   Snapshot save is best-effort — failures are silently ignored so they
///   never block event persistence.
///
/// The trigger type `T` is a generic parameter (not `Box<dyn>`) for
/// zero-cost monomorphization — the compiler inlines `should_snapshot()`
/// calls entirely.
pub struct Snapshotting<R, SS, SC, T> {
    inner: R,
    snapshot_store: SS,
    snapshot_codec: SC,
    trigger: T,
    schema_version: NonZeroU32,
    snapshot_on_read: bool,
}

impl<R, SS, SC, T> Snapshotting<R, SS, SC, T> {
    /// Create a new snapshot-aware repository.
    #[allow(
        clippy::too_many_arguments,
        reason = "snapshot config is flat — all fields are semantically distinct"
    )]
    pub const fn new(
        inner: R,
        snapshot_store: SS,
        snapshot_codec: SC,
        trigger: T,
        schema_version: NonZeroU32,
        snapshot_on_read: bool,
    ) -> Self {
        Self {
            inner,
            snapshot_store,
            snapshot_codec,
            trigger,
            schema_version,
            snapshot_on_read,
        }
    }
}

impl<A, R, SS, SC, T> Repository<A> for Snapshotting<R, SS, SC, T>
where
    A: Aggregate,
    R: Repository<A, Error = StoreError> + ReplayFrom<A>,
    SS: SnapshotStore,
    SC: Codec<A::State>,
    T: SnapshotTrigger,
    EventOf<A>: DomainEvent,
{
    type Error = StoreError;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, StoreError> {
        // Try snapshot first.
        let snapshot_hit = self.try_load_from_snapshot::<A>(&id).await;

        if let Some((root, from)) = snapshot_hit {
            // Snapshot hit — partial replay from snapshot version.
            return self.inner.replay_from(root, from).await;
        }

        // Fallback: full replay.
        let root = self.inner.load(id).await?;

        // Lazy snapshot: if enabled and aggregate has events, snapshot now.
        if self.snapshot_on_read
            && let Some(version) = root.version()
        {
            self.try_save_snapshot::<A>(&root, version).await;
        }

        Ok(root)
    }

    async fn save(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &[EventOf<A>],
    ) -> Result<(), StoreError> {
        let old_version = aggregate.version();

        // Delegate event persistence to inner.
        self.inner.save(aggregate, events).await?;

        // Check trigger — maybe snapshot.
        if !events.is_empty()
            && let Some(new_version) = aggregate.version()
        {
            let event_names: Vec<&str> = events.iter().map(DomainEvent::name).collect();
            if self
                .trigger
                .should_snapshot(old_version, new_version, &event_names)
            {
                self.try_save_snapshot::<A>(aggregate, new_version).await;
            }
        }

        Ok(())
    }
}

impl<R, SS, SC, T> Snapshotting<R, SS, SC, T>
where
    R: Send + Sync,
    SS: SnapshotStore,
    SC: Send + Sync,
    T: Send + Sync,
{
    /// Try to load a snapshot. Returns `(root, next_version)` on hit.
    /// Returns `None` on miss, schema mismatch, or any error (best-effort).
    async fn try_load_from_snapshot<A>(&self, id: &A::Id) -> Option<(AggregateRoot<A>, Version)>
    where
        A: Aggregate,
        SC: Codec<A::State>,
    {
        let holder = self.snapshot_store.load_snapshot(id).await.ok()??;

        if holder.schema_version() != self.schema_version {
            return None;
        }

        let state = self.snapshot_codec.decode("", holder.payload()).ok()?;

        let version = holder.version();
        let root = AggregateRoot::<A>::restore(id.clone(), state, version);
        let next = version.next()?;
        Some((root, next))
    }

    /// Best-effort snapshot save. Errors are silently ignored.
    async fn try_save_snapshot<A>(&self, aggregate: &AggregateRoot<A>, version: Version)
    where
        A: Aggregate,
        SC: Codec<A::State>,
    {
        if let Ok(payload) = self.snapshot_codec.encode(aggregate.state()) {
            let snap = PendingSnapshot::new(version, self.schema_version, payload);
            let _ = self
                .snapshot_store
                .save_snapshot(aggregate.id(), &snap)
                .await;
        }
    }
}
