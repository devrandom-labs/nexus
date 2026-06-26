use std::num::NonZeroU32;

use nexus::{Aggregate, AggregateRoot, DomainEvent, EventOf, Events, KernelError, Version};

use crate::repository::{ReplayFrom, Repository};
use crate::state;

/// Snapshot-aware repository decorator.
///
/// **Before reaching for snapshots, consider the
/// [Closing the Books](nexus::closing_the_books) pattern** — modeling the
/// aggregate as bounded, lifecycle-scoped streams often removes the need for
/// snapshots entirely.
///
/// Wraps an inner repository (e.g., `EventStore` or `ZeroCopyEventStore`)
/// and adds transparent snapshot support:
///
/// - **Load:** tries the snapshot store first; on hit with matching schema
///   version, restores state from the snapshot and replays only subsequent
///   events. On miss or schema mismatch, falls back to full event replay via
///   the inner repository. Optionally creates a snapshot after a full replay
///   (lazy/on-read snapshotting) if `snapshot_on_read` is enabled.
///
/// - **Save:** delegates event persistence to the inner repository, then
///   checks the trigger to optionally persist a snapshot of the current state.
///   Snapshot save is best-effort — failures are silently ignored so they
///   never block event persistence.
///
/// The trigger type `T` is a generic parameter (not `Box<dyn>`) for
/// zero-cost monomorphization — the compiler inlines `should_persist()`
/// calls entirely.
///
/// The snapshot store `SS` is generic over the aggregate's state type at the
/// `Repository` impl level — the struct itself is agnostic of the state type.
/// Codec responsibility lives in the snapshot store adapter (e.g.,
/// [`CodecSnapshotStore`](crate::state::CodecSnapshotStore)).
pub struct Snapshotting<R, SS, T> {
    inner: R,
    snapshot_store: SS,
    trigger: T,
    schema_version: NonZeroU32,
    snapshot_on_read: bool,
}

impl<R, SS, T> Snapshotting<R, SS, T> {
    /// Create a new snapshot-aware repository.
    pub const fn new(
        inner: R,
        snapshot_store: SS,
        trigger: T,
        schema_version: NonZeroU32,
        snapshot_on_read: bool,
    ) -> Self {
        Self {
            inner,
            snapshot_store,
            trigger,
            schema_version,
            snapshot_on_read,
        }
    }
}

impl<A, R, SS, T> Repository<A> for Snapshotting<R, SS, T>
where
    A: Aggregate,
    R: Repository<A> + ReplayFrom<A, Error = <R as Repository<A>>::Error>,
    <R as Repository<A>>::Error: From<KernelError>,
    SS: state::SnapshotStore<A::State, Version>,
    T: state::PersistTrigger,
    EventOf<A>: DomainEvent,
{
    type Error = <R as Repository<A>>::Error;

    async fn load(&self, id: A::Id) -> Result<AggregateRoot<A>, Self::Error> {
        // Snapshot hit → partial replay from snapshot version.
        if let Some((root, from)) = self.try_load_from_snapshot::<A>(&id).await {
            return self.inner.replay_from(root, from).await;
        }

        // Fallback: full replay.
        let root = self.inner.load(id).await?;

        // Lazy snapshot on full replay when enabled.
        if let (true, Some(version)) = (self.snapshot_on_read, root.version()) {
            self.try_save_snapshot::<A>(&root, version).await;
        }

        Ok(root)
    }

    async fn save<const N: usize>(
        &self,
        aggregate: &mut AggregateRoot<A>,
        events: &Events<EventOf<A>, N>,
    ) -> Result<(), Self::Error> {
        let old_version = aggregate.version();

        // Delegate event persistence to inner.
        self.inner.save(aggregate, events).await?;

        // Snapshot after save when the trigger fires. `events` is non-empty
        // (`&Events<_, N>`), so a successful save always advances the version.
        let Some(new_version) = aggregate.version() else {
            return Ok(());
        };
        if self.trigger.should_persist(
            old_version,
            new_version,
            events.iter().map(DomainEvent::name),
        ) {
            self.try_save_snapshot::<A>(aggregate, new_version).await;
        }

        Ok(())
    }
}

impl<R, SS, T> Snapshotting<R, SS, T>
where
    R: Send + Sync,
    SS: Send + Sync,
    T: Send + Sync,
{
    /// Try to load a snapshot. Returns `(root, next_version)` on hit.
    /// Returns `None` on miss, schema mismatch, or any error (best-effort).
    async fn try_load_from_snapshot<A>(&self, id: &A::Id) -> Option<(AggregateRoot<A>, Version)>
    where
        A: Aggregate,
        SS: state::SnapshotStore<A::State, Version>,
    {
        let (version, typed_state) = self
            .snapshot_store
            .hydrate(id, self.schema_version)
            .await
            .ok()??;
        let root = AggregateRoot::<A>::restore(id.clone(), typed_state, version);
        let next = version.next()?;
        Some((root, next))
    }

    /// Best-effort snapshot save. Errors are silently ignored.
    async fn try_save_snapshot<A>(&self, aggregate: &AggregateRoot<A>, version: Version)
    where
        A: Aggregate,
        SS: state::SnapshotStore<A::State, Version>,
    {
        let _ = self
            .snapshot_store
            .commit(
                aggregate.id(),
                self.schema_version,
                version,
                aggregate.state(),
            )
            .await;
    }
}
