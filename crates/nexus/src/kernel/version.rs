use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);

impl Version {
    pub const INITIAL: Version = Version(0);

    pub fn next(self) -> Version {
        Version(self.0 + 1)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Version {
    fn from(v: u64) -> Self {
        Version(v)
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

impl<E> VersionedEvent<E> {
    /// The version (sequence number) of this event in the aggregate's history.
    pub fn version(&self) -> Version {
        self.version
    }

    /// The domain event payload.
    pub fn event(&self) -> &E {
        &self.event
    }

    /// Consume the versioned event, returning the version and event separately.
    pub fn into_parts(self) -> (Version, E) {
        (self.version, self.event)
    }

    /// Create a new versioned event.
    ///
    /// This is `pub(crate)` — only the kernel can construct versioned events.
    /// Store adapters use `into_parts()` and `from_parts()` to
    /// serialize/deserialize, but cannot forge arbitrary version numbers
    /// from user code.
    pub(crate) fn new(version: Version, event: E) -> Self {
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
    pub fn from_persisted(version: Version, event: E) -> Self {
        Self { version, event }
    }
}
