use std::fmt;

/// A monotonically increasing sequence number for aggregate event history.
///
/// Versions are assigned by the kernel — user code cannot construct
/// arbitrary versions. The only public entry points are:
/// - `Version::INITIAL` — the starting version (0)
/// - `Version::from_persisted()` — for store adapters rehydrating from a database
/// - `version.next()` — derives the next version from an existing one
///
/// # Example
///
/// ```
/// use nexus::Version;
///
/// let v = Version::INITIAL;
/// assert_eq!(v.as_u64(), 0);
/// assert_eq!(v.next().as_u64(), 1);
/// assert!(v < v.next());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);

impl Version {
    /// The starting version for a new aggregate.
    pub const INITIAL: Self = Self(0);

    /// The next version in sequence.
    ///
    /// # Panics
    ///
    /// Panics if the version is `u64::MAX` (overflow). This is a hard limit —
    /// an aggregate cannot have more than `u64::MAX` events.
    #[must_use]
    pub const fn next(self) -> Self {
        match self.0.checked_add(1) {
            Some(v) => Self(v),
            None => panic!("Version overflow: cannot increment past u64::MAX"),
        }
    }

    /// The underlying integer value.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Construct a Version from a raw u64.
    ///
    /// This is `pub(crate)` — only the kernel creates versions internally.
    /// Store adapters use `from_persisted()` to reconstruct versions
    /// from database rows.
    pub(crate) const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Reconstruct a Version from persisted data.
    ///
    /// For store adapters that read version numbers from a database.
    /// This is the only public way to construct a Version from a raw number.
    #[must_use]
    pub const fn from_persisted(v: u64) -> Self {
        Self(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An event paired with its version in the aggregate's event sequence.
///
/// This type cannot be constructed outside the kernel — only
/// `AggregateRoot::apply_event` and `AggregateRoot::load_from_events`
/// produce `VersionedEvent` values. This guarantees that version
/// numbers are always assigned by the kernel, never forged by user code.
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
    /// This is `pub(crate)` — only the kernel can construct versioned events.
    /// Store adapters use `into_parts()` and `from_parts()` to
    /// serialize/deserialize, but cannot forge arbitrary version numbers
    /// from user code.
    pub(crate) const fn new(version: Version, event: E) -> Self {
        Self { version, event }
    }
}

/// Reconstruct a `VersionedEvent` from its parts.
///
/// This exists for store adapters that need to deserialize persisted events
/// back into `VersionedEvent`. It is public because adapters live in
/// separate crates, but the name `from_persisted` signals intent —
/// this is for rehydration, not for creating new events.
impl<E> VersionedEvent<E> {
    #[must_use]
    pub const fn from_persisted(version: Version, event: E) -> Self {
        Self { version, event }
    }
}
