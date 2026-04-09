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
pub struct RepositoryBuilder<S, C, U> {
    store: Store<S>,
    codec: C,
    upcaster: U,
}

impl<S, C, U> RepositoryBuilder<S, C, U> {
    /// Replace the codec.
    ///
    /// Returns a new builder with the updated codec type, preserving
    /// the store and upcaster.
    #[must_use]
    pub fn codec<NewC>(self, codec: NewC) -> RepositoryBuilder<S, NewC, U> {
        RepositoryBuilder {
            store: self.store,
            codec,
            upcaster: self.upcaster,
        }
    }

    /// Replace the upcaster.
    ///
    /// Returns a new builder with the updated upcaster type, preserving
    /// the store and codec. Pass `()` for no upcasting.
    #[must_use]
    pub fn upcaster<NewU>(self, upcaster: NewU) -> RepositoryBuilder<S, C, NewU> {
        RepositoryBuilder {
            store: self.store,
            codec: self.codec,
            upcaster,
        }
    }
}

impl<S, C, U> RepositoryBuilder<S, C, U>
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
        }
    }
}
