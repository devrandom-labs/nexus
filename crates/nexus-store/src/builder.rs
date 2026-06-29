use std::marker::PhantomData;

use crate::repository::EventStore;
use crate::store::{RawEventStore, Store};

// ═══════════════════════════════════════════════════════════════════════════
// NeedsCodec — compile-time guard
// ═══════════════════════════════════════════════════════════════════════════

/// Marker type indicating that a [`RepositoryBuilder`] has no codec configured yet.
///
/// `NeedsCodec` is `!Send`, which prevents calling `.build()` —
/// it requires `C: Send + Sync + 'static`.
/// Set a codec via [`.codec()`](RepositoryBuilder::codec) to unlock
/// the terminal methods.
///
/// You should never need to construct this type directly.
pub struct NeedsCodec(PhantomData<*const ()>);

impl NeedsCodec {
    /// Create a new `NeedsCodec` marker.
    #[cfg(not(any(feature = "json")))]
    pub(crate) const fn new() -> Self {
        Self(PhantomData)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Snapshot typestate markers
// ═══════════════════════════════════════════════════════════════════════════

/// Marker: no snapshot configured (default).
pub struct NoSnapshot;

/// Snapshot configuration, created by builder methods.
///
/// `SS` is a typed snapshot store — the codec (if needed) is composed inside
/// the store adapter (e.g., [`CodecSnapshotStore`](crate::state::CodecSnapshotStore)).
#[cfg(feature = "snapshot")]
pub struct WithSnapshot<SS, T> {
    store: SS,
    trigger: T,
    schema_version: std::num::NonZeroU32,
    snapshot_on_read: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// RepositoryBuilder
// ═══════════════════════════════════════════════════════════════════════════

/// Builder for creating an [`EventStore`] facade
/// from a [`Store`].
///
/// Obtained via [`Store::repository()`]. The builder starts with either a
/// default codec (when the `json` feature is enabled) or [`NeedsCodec`]
/// (requiring an explicit `.codec()` call).
///
/// Upcasting is not configured here — the resulting facade ships with
/// the no-upcaster [`Repository::load`](crate::Repository::load) /
/// [`save`](crate::Repository::save) path. For schema evolution, use the
/// facade's inherent [`load_with`](EventStore::load_with) /
/// [`save_with`](EventStore::save_with) methods after `build()`.
///
/// # Example
///
/// ```ignore
/// let store = Store::new(backend);
///
/// // With the `json` feature (default codec pre-filled):
/// let repo = store.repository().build();
///
/// // Custom codec:
/// let repo = store.repository().codec(MyCodec).build();
///
/// // With upcasting (drop to the facade after build):
/// let repo = store.repository().codec(MyCodec).build();
/// let root = repo.load_with(id, OrderTransforms::upcast).await?;
/// ```
pub struct RepositoryBuilder<S, C, A, Snap = NoSnapshot> {
    store: Store<S>,
    codec: C,
    snapshot: Snap,
    /// The aggregate this builder will bind the facade to (named once at
    /// [`Store::repository::<A>()`]). Threaded through every builder step so
    /// `.build()` produces an [`EventStore<S, C, A>`] that implements
    /// `Repository<A>` for exactly this `A`.
    aggregate: PhantomData<fn() -> A>,
}

impl<S, C, A, Snap> RepositoryBuilder<S, C, A, Snap> {
    /// Replace the codec.
    ///
    /// Returns a new builder with the updated codec type, preserving
    /// the store, the bound aggregate, and any snapshot configuration.
    #[must_use]
    pub fn codec<NewC>(self, codec: NewC) -> RepositoryBuilder<S, NewC, A, Snap> {
        RepositoryBuilder {
            store: self.store,
            codec,
            snapshot: self.snapshot,
            aggregate: PhantomData,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// NoSnapshot — plain EventStore
// ═══════════════════════════════════════════════════════════════════════════

impl<S, C, A> RepositoryBuilder<S, C, A, NoSnapshot>
where
    S: RawEventStore,
    C: Send + Sync + 'static,
{
    /// Build an [`EventStore`] for aggregate `A`.
    ///
    /// One terminal for any codec: the owning-vs-borrowing distinction is
    /// inferred from the codec's [`Decode::Output`](crate::Decode::Output)
    /// GAT (`E` or `&E`, unified via `Borrow<E>`), not restated at the call
    /// site. Requires a configured codec (`C: Send + Sync + 'static`, which
    /// excludes [`NeedsCodec`]).
    #[must_use]
    pub fn build(self) -> EventStore<S, C, A> {
        EventStore::new(self.store, self.codec)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Snapshot builder methods
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
use super::snapshot::Snapshotting;
#[cfg(feature = "snapshot")]
use crate::state;
#[cfg(feature = "snapshot")]
use std::num::NonZeroU64;

/// Default snapshot interval.
#[cfg(feature = "snapshot")]
const DEFAULT_SNAPSHOT_INTERVAL: u64 = 100;

/// Default snapshot schema version.
#[cfg(feature = "snapshot")]
const DEFAULT_SCHEMA_VERSION: std::num::NonZeroU32 = std::num::NonZeroU32::MIN;

#[cfg(all(feature = "snapshot-json", feature = "snapshot"))]
impl<S, C, A> RepositoryBuilder<S, C, A, NoSnapshot> {
    /// Configure a snapshot store with JSON codec (default).
    ///
    /// Accepts a byte-level [`SnapshotStore<Vec<u8>, Version>`](state::SnapshotStore)
    /// and wraps it in [`CodecSnapshotStore`](state::CodecSnapshotStore) with
    /// [`JsonCodec`](crate::JsonCodec).
    ///
    /// Pre-fills:
    /// - Trigger: [`EveryNEvents(100)`](state::EveryNEvents)
    /// - Schema version: 1
    /// - Snapshot on read: false
    ///
    /// Override any default with `.snapshot_trigger()`, etc.
    ///
    /// # Panics
    ///
    /// Cannot panic — the internal `expect` is on a compile-time constant.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "DEFAULT_SNAPSHOT_INTERVAL is non-zero by inspection"
    )]
    pub fn snapshot_store<SS>(
        self,
        snapshot_store: SS,
    ) -> RepositoryBuilder<
        S,
        C,
        A,
        WithSnapshot<state::CodecSnapshotStore<SS, crate::JsonCodec>, state::EveryNEvents>,
    > {
        let typed_store =
            state::CodecSnapshotStore::new(snapshot_store, crate::JsonCodec::default());
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            snapshot: WithSnapshot {
                store: typed_store,
                trigger: state::EveryNEvents(
                    NonZeroU64::new(DEFAULT_SNAPSHOT_INTERVAL)
                        .expect("DEFAULT_SNAPSHOT_INTERVAL is non-zero"),
                ),
                schema_version: DEFAULT_SCHEMA_VERSION,
                snapshot_on_read: false,
            },
            aggregate: PhantomData,
        }
    }
}

#[cfg(all(feature = "snapshot", not(feature = "snapshot-json")))]
impl<S, C, A> RepositoryBuilder<S, C, A, NoSnapshot> {
    /// Configure a snapshot store.
    ///
    /// Accepts a pre-composed typed [`SnapshotStore<S, Version>`](state::SnapshotStore).
    /// If your store is byte-level, compose it with
    /// [`CodecSnapshotStore`](state::CodecSnapshotStore) before passing it here.
    ///
    /// Pre-fills:
    /// - Trigger: [`EveryNEvents(100)`](state::EveryNEvents)
    /// - Schema version: 1
    /// - Snapshot on read: false
    ///
    /// Override any default with `.snapshot_trigger()`, etc.
    ///
    /// # Panics
    ///
    /// Cannot panic — the internal `expect` is on a compile-time constant.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "DEFAULT_SNAPSHOT_INTERVAL is non-zero by inspection"
    )]
    pub fn snapshot_store<SS>(
        self,
        snapshot_store: SS,
    ) -> RepositoryBuilder<S, C, A, WithSnapshot<SS, state::EveryNEvents>> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            snapshot: WithSnapshot {
                store: snapshot_store,
                trigger: state::EveryNEvents(
                    NonZeroU64::new(DEFAULT_SNAPSHOT_INTERVAL)
                        .expect("DEFAULT_SNAPSHOT_INTERVAL is non-zero"),
                ),
                schema_version: DEFAULT_SCHEMA_VERSION,
                snapshot_on_read: false,
            },
            aggregate: PhantomData,
        }
    }
}

#[cfg(feature = "snapshot")]
impl<S, C, A, SS, T> RepositoryBuilder<S, C, A, WithSnapshot<SS, T>> {
    /// Replace the snapshot trigger.
    #[must_use]
    pub fn snapshot_trigger<NewT: state::PersistTrigger>(
        self,
        trigger: NewT,
    ) -> RepositoryBuilder<S, C, A, WithSnapshot<SS, NewT>> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            snapshot: WithSnapshot {
                store: self.snapshot.store,
                trigger,
                schema_version: self.snapshot.schema_version,
                snapshot_on_read: self.snapshot.snapshot_on_read,
            },
            aggregate: PhantomData,
        }
    }

    /// Set the schema version for snapshot invalidation.
    #[must_use]
    pub const fn snapshot_schema_version(mut self, version: std::num::NonZeroU32) -> Self {
        self.snapshot.schema_version = version;
        self
    }

    /// Enable lazy snapshot creation on read (after full replay).
    #[must_use]
    pub const fn snapshot_on_read(mut self, enabled: bool) -> Self {
        self.snapshot.snapshot_on_read = enabled;
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WithSnapshot — Snapshotting<EventStore>
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
impl<S, C, A, SS, T> RepositoryBuilder<S, C, A, WithSnapshot<SS, T>>
where
    S: RawEventStore,
    C: Send + Sync + 'static,
{
    /// Build a snapshot-aware [`EventStore`] for aggregate `A` using an owning [`Codec`](crate::Codec).
    #[must_use]
    pub fn build(self) -> Snapshotting<EventStore<S, C, A>, SS, T> {
        let inner = EventStore::new(self.store, self.codec);
        let snap = self.snapshot;
        Snapshotting::new(
            inner,
            snap.store,
            snap.trigger,
            snap.schema_version,
            snap.snapshot_on_read,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Store::repository() entry points
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "json")]
impl<S: RawEventStore> Store<S> {
    /// Start building a repository facade for aggregate `A` over this store.
    ///
    /// Name the aggregate once here (`store.repository::<Order>()`); the
    /// resulting [`EventStore<S, C, A>`] then implements `Repository<A>` for
    /// exactly that `A`, so `load`/`save` infer the aggregate with no
    /// per-call annotation. The store itself stays multi-aggregate — mint one
    /// facade per aggregate type.
    ///
    /// When the `json` feature is enabled, the builder is pre-filled with
    /// [`JsonCodec`](crate::JsonCodec) as the default codec. Override it
    /// with [`.codec()`](RepositoryBuilder::codec) if needed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let store = Store::new(backend);
    ///
    /// // Use default JSON codec:
    /// let orders = store.repository::<Order>().build();
    /// let order = orders.load(id).await?;        // AggregateRoot<Order> — inferred
    ///
    /// // Override with a custom codec:
    /// let orders = store.repository::<Order>().codec(MyCodec).build();
    /// ```
    #[must_use]
    pub fn repository<A>(&self) -> RepositoryBuilder<S, crate::JsonCodec, A> {
        RepositoryBuilder {
            store: self.clone(),
            codec: crate::JsonCodec::default(),
            snapshot: NoSnapshot,
            aggregate: PhantomData,
        }
    }
}

#[cfg(not(any(feature = "json")))]
impl<S: RawEventStore> Store<S> {
    /// Start building a repository facade for aggregate `A` over this store.
    ///
    /// Name the aggregate once here (`store.repository::<Order>()`); the
    /// resulting facade implements `Repository<A>` for exactly that `A`, so
    /// `load`/`save` infer the aggregate with no per-call annotation.
    ///
    /// No default codec is available — call [`.codec()`](RepositoryBuilder::codec)
    /// to set one before calling `.build()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let store = Store::new(backend);
    /// let orders = store.repository::<Order>().codec(MyCodec).build();
    /// ```
    #[must_use]
    pub fn repository<A>(&self) -> RepositoryBuilder<S, NeedsCodec, A> {
        RepositoryBuilder {
            store: self.clone(),
            codec: NeedsCodec::new(),
            snapshot: NoSnapshot,
            aggregate: PhantomData,
        }
    }
}
