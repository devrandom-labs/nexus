use std::fmt;
use std::num::NonZeroU64;

/// A monotonically increasing event version number.
///
/// Wraps `NonZeroU64` — event versions are always >= 1.
/// A fresh aggregate with no events has `Option<Version>` = `None`.
///
/// The only public entry points are:
/// - `Version::INITIAL` — the first event version (1)
/// - `Version::new()` — construct from a `u64` (returns `None` for 0)
/// - `version.next()` — derives the next version, returns `None` on overflow
///
/// # Example
///
/// ```
/// use nexus::Version;
///
/// let v = Version::INITIAL;
/// assert_eq!(v.as_u64(), 1);
/// assert_eq!(v.next().unwrap().as_u64(), 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(NonZeroU64);

impl Version {
    /// The first event version (1).
    pub const INITIAL: Self = Self(NonZeroU64::MIN);

    /// The next version in sequence.
    ///
    /// Returns `None` if the version is `u64::MAX` (overflow).
    #[must_use]
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }

    /// The underlying integer value. Always >= 1.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0.get()
    }

    /// Construct a Version from a `u64`.
    ///
    /// Returns `None` if `v` is 0 (invalid — versions are always >= 1).
    /// Mirrors [`NonZeroU64::new`].
    #[must_use]
    pub const fn new(v: u64) -> Option<Self> {
        match NonZeroU64::new(v) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// An event paired with its version in the aggregate's event sequence.
#[derive(Debug)]
pub struct VersionedEvent<E> {
    version: Version,
    event: E,
}

impl<E: Clone> Clone for VersionedEvent<E> {
    fn clone(&self) -> Self {
        Self {
            version: self.version,
            event: self.event.clone(),
        }
    }
}

impl<E: PartialEq> PartialEq for VersionedEvent<E> {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.event == other.event
    }
}

impl<E: Eq> Eq for VersionedEvent<E> {}

impl<E> VersionedEvent<E> {
    /// The version (sequence number) of this event in the aggregate's history.
    #[must_use]
    pub const fn version(&self) -> Version {
        self.version
    }

    /// The domain event payload.
    #[must_use]
    pub const fn event(&self) -> &E {
        &self.event
    }

    /// Consume the versioned event, returning the version and event separately.
    #[must_use]
    pub fn into_parts(self) -> (Version, E) {
        (self.version, self.event)
    }

    /// Create a new versioned event.
    ///
    /// Public because store adapters in separate crates need to
    /// reconstruct `VersionedEvent` from deserialized parts.
    #[must_use]
    pub const fn new(version: Version, event: E) -> Self {
        Self { version, event }
    }
}
