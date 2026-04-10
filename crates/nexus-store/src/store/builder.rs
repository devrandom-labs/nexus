use std::marker::PhantomData;

use super::event_store::EventStore;
use super::raw::RawEventStore;
use super::store::Store;
use super::zero_copy_event_store::ZeroCopyEventStore;

// ═══════════════════════════════════════════════════════════════════════════
// NeedsCodec — compile-time guard
// ═══════════════════════════════════════════════════════════════════════════

/// Marker type indicating that a [`RepositoryBuilder`] has no codec configured yet.
///
/// `NeedsCodec` is `!Send`, which prevents calling `.build()` or
/// `.build_zero_copy()` — both require `C: Send + Sync + 'static`.
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
#[cfg(feature = "snapshot")]
pub struct WithSnapshot<SS, SC, T> {
    store: SS,
    codec: SC,
    trigger: T,
    schema_version: u32,
    snapshot_on_read: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// RepositoryBuilder
// ═══════════════════════════════════════════════════════════════════════════

/// Builder for creating [`EventStore`] or [`ZeroCopyEventStore`] facades
/// from a [`Store`].
///
/// Obtained via [`Store::repository()`]. The builder starts with either a
/// default codec (when the `json` feature is enabled) or [`NeedsCodec`]
/// (requiring an explicit `.codec()` call).
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
/// // With an upcaster:
/// let repo = store.repository().upcaster(MyUpcaster).build();
/// ```
pub struct RepositoryBuilder<S, C, U, Snap = NoSnapshot> {
    store: Store<S>,
    codec: C,
    upcaster: U,
    snapshot: Snap,
}

impl<S, C, U, Snap> RepositoryBuilder<S, C, U, Snap> {
    /// Replace the codec.
    ///
    /// Returns a new builder with the updated codec type, preserving
    /// the store and upcaster.
    #[must_use]
    pub fn codec<NewC>(self, codec: NewC) -> RepositoryBuilder<S, NewC, U, Snap> {
        RepositoryBuilder {
            store: self.store,
            codec,
            upcaster: self.upcaster,
            snapshot: self.snapshot,
        }
    }

    /// Replace the upcaster.
    ///
    /// Returns a new builder with the updated upcaster type, preserving
    /// the store and codec. Pass `()` for no upcasting.
    #[must_use]
    pub fn upcaster<NewU>(self, upcaster: NewU) -> RepositoryBuilder<S, C, NewU, Snap> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster,
            snapshot: self.snapshot,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// NoSnapshot — plain EventStore / ZeroCopyEventStore
// ═══════════════════════════════════════════════════════════════════════════

impl<S, C, U> RepositoryBuilder<S, C, U, NoSnapshot>
where
    S: RawEventStore,
    C: Send + Sync + 'static,
{
    /// Build an [`EventStore`] using an owning [`Codec`](crate::Codec).
    ///
    /// Requires that a codec has been configured (either via the default
    /// or an explicit `.codec()` call). This method is gated on
    /// `C: Send + Sync + 'static`, which excludes [`NeedsCodec`].
    #[must_use]
    pub fn build(self) -> EventStore<S, C, U> {
        EventStore::new(self.store, self.codec, self.upcaster)
    }

    /// Build a [`ZeroCopyEventStore`] using a [`BorrowingCodec`](crate::BorrowingCodec).
    ///
    /// Requires that a codec has been configured (either via the default
    /// or an explicit `.codec()` call). This method is gated on
    /// `C: Send + Sync + 'static`, which excludes [`NeedsCodec`].
    #[must_use]
    pub fn build_zero_copy(self) -> ZeroCopyEventStore<S, C, U> {
        ZeroCopyEventStore::new(self.store, self.codec, self.upcaster)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Snapshot builder methods
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
use super::snapshotting::Snapshotting;
#[cfg(feature = "snapshot")]
use crate::snapshot::{EveryNEvents, SnapshotStore, SnapshotTrigger};
#[cfg(feature = "snapshot")]
use std::num::NonZeroU64;

/// Default snapshot interval.
#[cfg(feature = "snapshot")]
const DEFAULT_SNAPSHOT_INTERVAL: u64 = 100;

#[cfg(all(feature = "snapshot-json", feature = "snapshot"))]
impl<S, C, U> RepositoryBuilder<S, C, U, NoSnapshot> {
    /// Configure a snapshot store with JSON codec (default).
    ///
    /// Pre-fills:
    /// - Snapshot codec: [`JsonCodec`](crate::JsonCodec)
    /// - Trigger: [`EveryNEvents(100)`](EveryNEvents)
    /// - Schema version: 1
    /// - Snapshot on read: false
    ///
    /// Override any default with `.snapshot_codec()`, `.snapshot_trigger()`, etc.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "DEFAULT_SNAPSHOT_INTERVAL is non-zero by inspection"
    )]
    pub fn snapshot_store<SS: SnapshotStore>(
        self,
        snapshot_store: SS,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, crate::JsonCodec, EveryNEvents>> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster: self.upcaster,
            snapshot: WithSnapshot {
                store: snapshot_store,
                codec: crate::JsonCodec::default(),
                trigger: EveryNEvents(
                    NonZeroU64::new(DEFAULT_SNAPSHOT_INTERVAL)
                        .expect("DEFAULT_SNAPSHOT_INTERVAL is non-zero"),
                ),
                schema_version: 1,
                snapshot_on_read: false,
            },
        }
    }
}

#[cfg(all(feature = "snapshot", not(feature = "snapshot-json")))]
impl<S, C, U> RepositoryBuilder<S, C, U, NoSnapshot> {
    /// Configure a snapshot store with an explicit codec.
    ///
    /// Pre-fills:
    /// - Trigger: [`EveryNEvents(100)`](EveryNEvents)
    /// - Schema version: 1
    /// - Snapshot on read: false
    ///
    /// Override any default with `.snapshot_trigger()`, etc.
    #[must_use]
    #[allow(
        clippy::expect_used,
        reason = "DEFAULT_SNAPSHOT_INTERVAL is non-zero by inspection"
    )]
    pub fn snapshot_store<SS: SnapshotStore, SC>(
        self,
        snapshot_store: SS,
        snapshot_codec: SC,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, EveryNEvents>> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster: self.upcaster,
            snapshot: WithSnapshot {
                store: snapshot_store,
                codec: snapshot_codec,
                trigger: EveryNEvents(
                    NonZeroU64::new(DEFAULT_SNAPSHOT_INTERVAL)
                        .expect("DEFAULT_SNAPSHOT_INTERVAL is non-zero"),
                ),
                schema_version: 1,
                snapshot_on_read: false,
            },
        }
    }
}

#[cfg(feature = "snapshot")]
impl<S, C, U, SS, SC, T> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, T>> {
    /// Replace the snapshot codec.
    #[must_use]
    pub fn snapshot_codec<NewSC>(
        self,
        codec: NewSC,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, NewSC, T>> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster: self.upcaster,
            snapshot: WithSnapshot {
                store: self.snapshot.store,
                codec,
                trigger: self.snapshot.trigger,
                schema_version: self.snapshot.schema_version,
                snapshot_on_read: self.snapshot.snapshot_on_read,
            },
        }
    }

    /// Replace the snapshot trigger.
    #[must_use]
    pub fn snapshot_trigger<NewT: SnapshotTrigger>(
        self,
        trigger: NewT,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, NewT>> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster: self.upcaster,
            snapshot: WithSnapshot {
                store: self.snapshot.store,
                codec: self.snapshot.codec,
                trigger,
                schema_version: self.snapshot.schema_version,
                snapshot_on_read: self.snapshot.snapshot_on_read,
            },
        }
    }

    /// Set the schema version for snapshot invalidation.
    #[must_use]
    pub const fn snapshot_schema_version(
        mut self,
        version: u32,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, T>> {
        self.snapshot.schema_version = version;
        self
    }

    /// Enable lazy snapshot creation on read (after full replay).
    #[must_use]
    pub const fn snapshot_on_read(
        mut self,
        enabled: bool,
    ) -> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, T>> {
        self.snapshot.snapshot_on_read = enabled;
        self
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WithSnapshot — Snapshotting<EventStore / ZeroCopyEventStore>
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "snapshot")]
impl<S, C, U, SS, SC, T> RepositoryBuilder<S, C, U, WithSnapshot<SS, SC, T>>
where
    S: RawEventStore,
    C: Send + Sync + 'static,
{
    /// Build a snapshot-aware [`EventStore`] using an owning [`Codec`](crate::Codec).
    #[must_use]
    pub fn build(self) -> Snapshotting<EventStore<S, C, U>, SS, SC, T> {
        let inner = EventStore::new(self.store, self.codec, self.upcaster);
        let snap = self.snapshot;
        Snapshotting::new(
            inner,
            snap.store,
            snap.codec,
            snap.trigger,
            snap.schema_version,
            snap.snapshot_on_read,
        )
    }

    /// Build a snapshot-aware [`ZeroCopyEventStore`] using a [`BorrowingCodec`](crate::BorrowingCodec).
    #[must_use]
    pub fn build_zero_copy(self) -> Snapshotting<ZeroCopyEventStore<S, C, U>, SS, SC, T> {
        let inner = ZeroCopyEventStore::new(self.store, self.codec, self.upcaster);
        let snap = self.snapshot;
        Snapshotting::new(
            inner,
            snap.store,
            snap.codec,
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
    /// Start building a repository facade for this store.
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
    /// let repo = store.repository().build();
    ///
    /// // Override with a custom codec:
    /// let repo = store.repository().codec(MyCodec).build();
    /// ```
    #[must_use]
    pub fn repository(&self) -> RepositoryBuilder<S, crate::JsonCodec, ()> {
        RepositoryBuilder {
            store: self.clone(),
            codec: crate::JsonCodec::default(),
            upcaster: (),
            snapshot: NoSnapshot,
        }
    }
}

#[cfg(not(any(feature = "json")))]
impl<S: RawEventStore> Store<S> {
    /// Start building a repository facade for this store.
    ///
    /// No default codec is available — call [`.codec()`](RepositoryBuilder::codec)
    /// to set one before calling `.build()` or `.build_zero_copy()`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let store = Store::new(backend);
    /// let repo = store.repository().codec(MyCodec).build();
    /// ```
    #[must_use]
    pub fn repository(&self) -> RepositoryBuilder<S, NeedsCodec, ()> {
        RepositoryBuilder {
            store: self.clone(),
            codec: NeedsCodec::new(),
            upcaster: (),
            snapshot: NoSnapshot,
        }
    }
}
